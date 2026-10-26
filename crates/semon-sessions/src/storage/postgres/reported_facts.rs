//! Private reported observation/stamp family; no public backend qualification.
//! A complete save is one durable operation, irrespective of its record count.
#![allow(dead_code)] // Future PgIndexStore consumes this private family.
use super::{
    OperationIdentity, PgCancellation, PgPool, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::{
    events::{
        ReportedFileStamp,
        reported_rows::{self, ReportedRow},
    },
    facts::ReportedRunSnapshot,
    storage::{LogicalSourceNamespace, StorageError},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) struct Loaded {
    pub(super) runs: Vec<ReportedRunSnapshot>,
    pub(super) stamp: Option<ReportedFileStamp>,
    pub(super) revision: i64,
}

#[cfg(test)]
thread_local! { static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
thread_local! { static READ_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn boundary(tx: &PgTxn<'_>) -> Result<(), StorageError> {
    #[cfg(test)]
    if FAIL_AFTER.with(|point| match point.get() {
        Some(1) => {
            point.set(None);
            true
        }
        Some(n) => {
            point.set(Some(n - 1));
            false
        }
        None => false,
    }) {
        return Err(StorageError::Unavailable);
    }
    tx.check()
}

fn revision(tx: &mut PgTxn<'_>, lock: bool) -> Result<i64, StorageError> {
    tx.check()?;
    // Schema + STORE/publication locks have already been acquired by PgTxn.
    let sql = if lock {
        "SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE"
    } else {
        "SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1"
    };
    let rows = tx
        .driver
        .query(sql, &[&tx.namespace.as_str().as_bytes()])
        .map_err(sqlstate::error)?;
    if rows.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    Ok(rows[0].get(0))
}

/// Caller retains the exact ID and request through uncertain commit/restart.
/// Receipt replay cannot execute another writer's effect or change the request.
#[allow(clippy::too_many_arguments)] // Exact durable request, separately authorized pool/namespace/control.
pub(super) fn save(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    expected_revision: i64,
    operation_id: &str,
    runs: &[ReportedRunSnapshot],
    stamp: &ReportedFileStamp,
    control: &PgCancellation,
) -> Result<(), StorageError> {
    save_selected(
        pool,
        namespace,
        expected_revision,
        operation_id,
        runs,
        stamp,
        Some(&BTreeSet::new()),
        control,
    )
    .map(|_| ())
}

/// IndexStore-compatible result: a coherent selected retained history and stamp.
/// Receipt replay never re-applies effects. On replay the returned read snapshot
/// reflects current committed history, as a fresh IndexStore selection would.
#[allow(clippy::too_many_arguments)]
pub(super) fn save_selected(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    expected_revision: i64,
    operation_id: &str,
    runs: &[ReportedRunSnapshot],
    stamp: &ReportedFileStamp,
    selected: Option<&BTreeSet<String>>,
    control: &PgCancellation,
) -> Result<Loaded, StorageError> {
    let rows = reported_rows::plan(runs).map_err(|_| StorageError::InvalidArguments)?;
    let encoded = rows
        .iter()
        .map(ReportedRow::encode)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StorageError::InvalidArguments)?;
    let stamp = reported_rows::encode_stamp(stamp).map_err(|_| StorageError::InvalidArguments)?;
    let mut digest = Sha256::new();
    digest.update(b"reported-facts-v1\0");
    digest.update(expected_revision.to_be_bytes());
    let exact_request = serde_json::to_vec(runs).map_err(|_| StorageError::InvalidArguments)?;
    digest.update((exact_request.len() as u64).to_be_bytes());
    digest.update(exact_request);
    for value in std::iter::once(namespace.as_str().as_bytes())
        .chain(encoded.iter().map(Vec::as_slice))
        .chain(std::iter::once(stamp.as_slice()))
    {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    let digest: [u8; 32] = digest.finalize().into();
    let identity = OperationIdentity {
        operation_id,
        kind: "reported-facts-v1",
        request_digest: &digest,
    };
    let mut loaded = None;
    pool.apply_operation(namespace, &identity, control, |tx| {
        if revision(tx, true)? != expected_revision { return Err(StorageError::ScopeChanged); }
        validate_prior(tx, &rows)?;
        boundary(tx)?;
        let ns = namespace.as_str().as_bytes();
        for (batch, records) in rows.chunks(64).zip(encoded.chunks(64)) {
            tx.check()?;
            let ids = batch.iter().map(|row| row.run.last_session_id.as_bytes().to_vec()).collect::<Vec<_>>();
            let starts = batch.iter().map(|row| row.run.last_start_time).collect::<Vec<_>>();
            // Digest keeps arbitrary valid opaque IDs out of bounded B-tree keys.
            // Full equality guards both namespace and native-ID collisions.
            let count = tx.driver.execute("INSERT INTO semon_pg_native_reported_runs(namespace,session_digest,session_id,start,record) SELECT $1,sha256(x.id),x.id,x.start,x.record FROM unnest($2::bytea[],$3::bigint[],$4::bytea[]) AS x(id,start,record) ON CONFLICT(namespace_digest,session_digest,start) DO UPDATE SET record=excluded.record WHERE semon_pg_native_reported_runs.namespace=excluded.namespace AND semon_pg_native_reported_runs.session_id=excluded.session_id", &[&ns,&ids,&starts,&records]).map_err(sqlstate::error)?;
            if count != batch.len() as u64 { return Err(StorageError::ScopeChanged); }
            for _ in batch { boundary(tx)?; }
        }
        let count = tx.driver.execute("INSERT INTO semon_pg_native_reported_stamp(namespace,singleton,stamp) VALUES($1,true,$2) ON CONFLICT(namespace_digest,singleton) DO UPDATE SET stamp=excluded.stamp WHERE semon_pg_native_reported_stamp.namespace=excluded.namespace", &[&ns,&stamp]).map_err(sqlstate::error)?;
        if count != 1 { return Err(StorageError::ScopeChanged); }
        boundary(tx)?;
        loaded = Some(load_tx(tx, selected)?);
        Ok(serde_json::json!({"reportedFacts":1}))
    })?;
    match loaded {
        Some(loaded) => Ok(loaded),
        None => load(pool, namespace, selected, control),
    }
}

/// Do not overwrite an unsupported/corrupt observation into apparent validity.
/// Validate only the affected keys; unrelated history is not a save-time scan.
fn validate_prior(tx: &mut PgTxn<'_>, rows: &[ReportedRow]) -> Result<(), StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let mut budget = Budget { rows: 0, bytes: 0 };
    let stamps = tx.driver.query("SELECT singleton,octet_length(stamp)::bigint FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 LIMIT 2", &[&ns]).map_err(sqlstate::error)?;
    if stamps.len() > 1 || stamps.first().is_some_and(|row| !row.get::<_, bool>(0)) {
        return Err(StorageError::Unavailable);
    }
    if let Some(size) = stamps.first() {
        budget.charge(size.get(1))?;
        tx.check()?;
        let row = tx.driver.query_one("SELECT stamp FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton=true", &[&ns]).map_err(sqlstate::error)?;
        reported_rows::decode_stamp(&row.get::<_,Vec<u8>>(0)).map_err(|_|StorageError::Unavailable)?;
    } else if !tx.driver.query("SELECT 1 FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 LIMIT 1", &[&ns]).map_err(sqlstate::error)?.is_empty() {
        return Err(StorageError::Unavailable);
    }
    for batch in rows.chunks(64) {
        tx.check()?;
        let ids = batch
            .iter()
            .map(|row| row.run.last_session_id.as_bytes().to_vec())
            .collect::<Vec<_>>();
        let starts = batch
            .iter()
            .map(|row| row.run.last_start_time)
            .collect::<Vec<_>>();
        let sizes = tx.driver.query("SELECT x.ordinality,octet_length(r.session_id)::bigint+octet_length(r.record)::bigint FROM unnest($2::bytea[],$3::bigint[]) WITH ORDINALITY AS x(id,start,ordinality) JOIN semon_pg_native_reported_runs r ON r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.session_digest=sha256(x.id) AND r.start=x.start ORDER BY x.ordinality", &[&ns,&ids,&starts]).map_err(sqlstate::error)?;
        for size in &sizes {
            budget.charge(size.get(1))?;
        }
        tx.check()?;
        let prior = tx.driver.query("SELECT x.ordinality,r.session_id,r.record FROM unnest($2::bytea[],$3::bigint[]) WITH ORDINALITY AS x(id,start,ordinality) JOIN semon_pg_native_reported_runs r ON r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.session_digest=sha256(x.id) AND r.start=x.start ORDER BY x.ordinality", &[&ns,&ids,&starts]).map_err(sqlstate::error)?;
        if sizes.len() != prior.len() {
            return Err(StorageError::ScopeChanged);
        }
        for row in prior {
            tx.check()?;
            let ordinal: i64 = row.get(0);
            let index = usize::try_from(ordinal - 1).map_err(|_| StorageError::Unavailable)?;
            let planned = batch.get(index).ok_or(StorageError::Unavailable)?;
            let id = planned.run.last_session_id.as_bytes();
            let start = planned.run.last_start_time;
            let stored: Vec<u8> = row.get(1);
            if stored != id {
                return Err(StorageError::ScopeChanged);
            }
            let prior = ReportedRow::decode(&row.get::<_, Vec<u8>>(2))
                .map_err(|_| StorageError::Unavailable)?
                .run;
            if prior.last_session_id.as_bytes() != id || prior.last_start_time != start {
                return Err(StorageError::Unavailable);
            }
        }
    }
    Ok(())
}

struct Budget {
    rows: usize,
    bytes: usize,
}
impl Budget {
    fn charge(&mut self, bytes: i64) -> Result<(), StorageError> {
        let bytes = usize::try_from(bytes).map_err(|_| StorageError::Unavailable)?;
        self.rows = self.rows.checked_add(1).ok_or(StorageError::Busy)?;
        self.bytes = self
            .bytes
            .checked_add(
                bytes
                    .checked_mul(8)
                    .and_then(|v| v.checked_add(512))
                    .ok_or(StorageError::Busy)?,
            )
            .ok_or(StorageError::Busy)?;
        if self.rows > 131_072 || self.bytes > 64 * 1024 * 1024 {
            return Err(StorageError::Busy);
        }
        Ok(())
    }
}

fn read_rows(
    tx: &mut PgTxn<'_>,
    selected: Option<&str>,
    budget: &mut Budget,
    out: &mut Vec<ReportedRunSnapshot>,
) -> Result<(), StorageError> {
    visit_rows(tx, selected, budget, true, &mut |run| {
        out.push(run);
        Ok(())
    })
}

fn visit_rows(
    tx: &mut PgTxn<'_>,
    selected: Option<&str>,
    budget: &mut Budget,
    cumulative: bool,
    visitor: &mut impl FnMut(ReportedRunSnapshot) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let selected = selected.map(|id| id.as_bytes().to_vec());
    let mut cursor: Option<Vec<u8>> = None;
    let mut start = i64::MIN;
    loop {
        tx.check()?;
        // Fetch size before fetching/decoding the metadata body. This keyset
        // never visits unrelated history for an exact named-run selection.
        let rows = tx.driver.query("SELECT session_digest,start,octet_length(record)::bigint+octet_length(session_id)::bigint FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND ($2::bytea IS NULL OR (session_digest,start)>($2,$3)) AND ($4::bytea IS NULL OR session_digest=sha256($4)) ORDER BY session_digest,start LIMIT 1", &[&ns,&cursor,&start,&selected]).map_err(sqlstate::error)?;
        let Some(row) = rows.first() else {
            break;
        };
        if !cumulative {
            *budget = Budget { rows: 0, bytes: 0 };
        }
        budget.charge(row.get(2))?;
        #[cfg(test)]
        READ_WORK.with(|work| work.set(work.get() + 1));
        tx.check()?;
        let digest: Vec<u8> = row.get(0);
        let next_start: i64 = row.get(1);
        let data = tx.driver.query_one("SELECT session_id,record FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND session_digest=$2 AND start=$3", &[&ns,&digest,&next_start]).map_err(sqlstate::error)?;
        tx.check()?;
        let id: Vec<u8> = data.get(0);
        if Sha256::digest(&id).as_slice() != digest
            || selected.as_ref().is_some_and(|selected| selected != &id)
        {
            return Err(StorageError::ScopeChanged);
        }
        let record: Vec<u8> = data.get(1);
        let run = ReportedRow::decode(&record)
            .map_err(|_| StorageError::Unavailable)?
            .run;
        if run.last_session_id.as_bytes() != id || run.last_start_time != next_start {
            return Err(StorageError::Unavailable);
        }
        visitor(run)?;
        cursor = Some(digest);
        start = next_start;
    }
    Ok(())
}

/// Stream one current-contract snapshot in stable key order. Each record is
/// size-charged before decoding; no complete history is buffered. The caller
/// stages all callback effects and publishes its checkpoint only after success.
/// The read transaction owns its pool slot through the final callback and ACK.
pub(super) fn export_snapshot(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    mut begin: impl FnMut(i64, Option<&ReportedFileStamp>) -> Result<(), StorageError>,
    mut record: impl FnMut(ReportedRunSnapshot) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
        let header = load_tx(tx, Some(&BTreeSet::new()))?;
        begin(header.revision, header.stamp.as_ref())?;
        let mut budget = Budget { rows: 0, bytes: 0 };
        visit_rows(tx, None, &mut budget, false, &mut record)?;
        tx.check()
    })
}

pub(super) fn load(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    ids: Option<&BTreeSet<String>>,
    control: &PgCancellation,
) -> Result<Loaded, StorageError> {
    pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
        load_tx(tx, ids)
    })
}

fn load_tx(tx: &mut PgTxn<'_>, ids: Option<&BTreeSet<String>>) -> Result<Loaded, StorageError> {
    let revision = revision(tx, false)?;
    let ns = tx.namespace.as_str().as_bytes();
    let sizes = tx.driver.query("SELECT singleton,octet_length(stamp)::bigint FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 LIMIT 2", &[&ns]).map_err(sqlstate::error)?;
    let mut budget = Budget { rows: 0, bytes: 0 };
    if sizes.len() > 1 || sizes.first().is_some_and(|row| !row.get::<_, bool>(0)) {
        return Err(StorageError::Unavailable);
    }
    let stamp = if let Some(size) = sizes.first() {
        budget.charge(size.get(1))?;
        tx.check()?;
        let row = tx.driver.query_one("SELECT stamp FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton=true", &[&ns]).map_err(sqlstate::error)?;
        Some(
            reported_rows::decode_stamp(&row.get::<_, Vec<u8>>(0))
                .map_err(|_| StorageError::Unavailable)?,
        )
    } else {
        None
    };
    let mut runs = Vec::new();
    if let Some(ids) = ids {
        for id in ids {
            read_rows(tx, Some(id), &mut budget, &mut runs)?;
        }
    } else {
        read_rows(tx, None, &mut budget, &mut runs)?;
    }
    // A missing stamp cannot disguise corrupt non-rebuildable records.
    if stamp.is_none() {
        let found = tx.driver.query("SELECT 1 FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 LIMIT 1", &[&ns]).map_err(sqlstate::error)?;
        if !found.is_empty() {
            return Err(StorageError::Unavailable);
        }
    }
    runs.sort_by(|a, b| {
        (&a.last_session_id, a.last_start_time).cmp(&(&b.last_session_id, b.last_start_time))
    });
    tx.check()?;
    Ok(Loaded {
        runs,
        stamp,
        revision,
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, limits, ns};
    use super::*;
    use crate::events::EventCache;

    #[cfg(target_os = "linux")]
    struct SqliteWitness(std::fs::File);

    #[cfg(target_os = "linux")]
    impl SqliteWitness {
        fn new(root: &std::path::Path) -> Self {
            use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};
            let path = std::ffi::CString::new(root.as_os_str().as_bytes()).unwrap();
            // SAFETY: both calls use a live NUL-terminated path; ownership of
            // the new descriptor is transferred exactly once into File.
            let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
            assert!(fd >= 0);
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            let watch = unsafe {
                libc::inotify_add_watch(
                    fd,
                    path.as_ptr(),
                    libc::IN_OPEN | libc::IN_ACCESS | libc::IN_CREATE,
                )
            };
            assert!(watch >= 0);
            Self(file)
        }

        fn assert_no_access(mut self) {
            use std::io::Read;
            let mut events = [0u8; 4096];
            let error = self
                .0
                .read(&mut events)
                .expect_err("PG accessed or created a file in the SQLite fixture directory");
            assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        }
    }

    fn stamp() -> ReportedFileStamp {
        serde_json::from_str(r#"{"dev":18446744073709551615,"ino":18446744073709551615,"size":18446744073709551615,"modified_ns":340282366920938463463374607431768211455}"#).unwrap()
    }

    fn seed(pool: &PgPool, namespace: &LogicalSourceNamespace) {
        pool.transaction(
            namespace,
            TxnKind::Write,
            &PgCancellation::default(),
            |tx| {
                tx.driver
                    .execute(
                        "INSERT INTO semon_pg_source_state(namespace) VALUES($1)",
                        &[&namespace.as_str().as_bytes()],
                    )
                    .map_err(sqlstate::error)?;
                Ok(())
            },
        )
        .unwrap();
    }

    fn oracle() -> (
        Vec<ReportedRunSnapshot>,
        ReportedFileStamp,
        std::path::PathBuf,
    ) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "reported-facts118-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cache_path = root.join("index.json");
        let source = crate::storage::HostSource::new(ns("reported-facts-oracle"), "f".repeat(64))
            .unwrap();
        let input = root.join(".claude.json");
        // Three real producer observations: A, a new run B, then C overwrites A.
        // B is absent from C's input and must nevertheless survive with NULL cost.
        for (start, capture, cost) in [
            (100, 101, Some(0.125)),
            (200, 202, None),
            (100, 303, Some(0.25)),
        ] {
            std::fs::write(&input,serde_json::json!({"projects":{"/sanitized":{"lastSessionId":"原生\0id","lastStartTime":start,"lastDuration":u64::MAX,"lastCost":cost,"lastModelUsage":{"模型\0":{"inputTokens":u64::MAX}}}}}).to_string()).unwrap();
            let mut cache = EventCache::open(&cache_path, &source).unwrap();
            cache
                .refresh_reported_runs(&input, capture, &mut false)
                .unwrap();
        }
        std::fs::remove_file(&input).unwrap();
        let cache = EventCache::open(&cache_path, &source).unwrap();
        let runs = cache.reported_runs().cloned().collect::<Vec<_>>();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].last_duration, Some(u64::MAX));
        assert_eq!(runs[0].capture_at, 303);
        assert_eq!(runs[0].last_cost, Some(0.25));
        assert_eq!(runs[1].capture_at, 202);
        assert_eq!(runs[1].last_cost, None);
        assert_eq!(runs[0].last_model_usage["模型\0"].input_tokens, u64::MAX);
        drop(cache);
        let db = rusqlite::Connection::open(EventCache::path(&cache_path)).unwrap();
        let text: String = db
            .query_row(
                "SELECT value FROM meta WHERE key='claude_json_stamp'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let stamp = reported_rows::decode_stamp(text.as_bytes()).unwrap();
        drop(db);
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        (runs, stamp, root)
    }

    #[test]
    fn pg_reported_facts_actual_sqlite_producer_retained_selection_restart_namespace_ack() {
        let (runs, stamp, root) = oracle();
        #[cfg(target_os = "linux")]
        let witness = SqliteWitness::new(&root);
        let (pool, factory) = fixture(|_| {});
        let source = ns("事实\0A");
        let foreign = ns("事实\0B");
        seed(&pool, &source);
        seed(&pool, &foreign);
        let control = PgCancellation::default();
        super::super::tests::LOSE_ACK.with(|flag| flag.set(true));
        assert!(matches!(
            save(
                &pool,
                &source,
                0,
                &"a".repeat(64),
                &runs[..1],
                &stamp,
                &control
            ),
            Err(StorageError::CommitUncertain)
        ));
        save(
            &pool,
            &source,
            0,
            &"a".repeat(64),
            &runs[..1],
            &stamp,
            &control,
        )
        .unwrap();
        assert!(matches!(
            save(&pool, &source, 0, &"a".repeat(64), &runs, &stamp, &control),
            Err(StorageError::InvalidArguments)
        ));
        save(
            &pool,
            &source,
            0,
            &"b".repeat(64),
            &runs[1..],
            &stamp,
            &control,
        )
        .unwrap();
        let mut other = runs[0].clone();
        other.last_cost = Some(99.5);
        save(
            &pool,
            &foreign,
            0,
            &"a".repeat(64),
            std::slice::from_ref(&other),
            &stamp,
            &control,
        )
        .unwrap();
        assert_eq!(load(&pool, &source, None, &control).unwrap().runs, runs);
        assert_eq!(load(&pool, &foreign, None, &control).unwrap().runs, [other]);
        let named = BTreeSet::from([runs[0].last_session_id.clone()]);
        assert_eq!(
            load(&pool, &source, Some(&named), &control).unwrap().runs,
            runs
        );
        let absent = BTreeSet::from(["absent".into()]);
        assert!(
            load(&pool, &source, Some(&absent), &control)
                .unwrap()
                .runs
                .is_empty()
        );
        drop(pool);
        let pool = PgPool::new(factory, limits()).unwrap();
        let again = load(&pool, &source, None, &control).unwrap();
        assert_eq!(again.runs, runs);
        assert_eq!(again.stamp, Some(stamp));
        assert_eq!(again.revision, 0);
        crate::events::CACHE_READS
            .with(|reads| assert_eq!(reads.get(), 0, "PG phase opened SQLite"));
        #[cfg(target_os = "linux")]
        witness.assert_no_access();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pg_reported_facts_every_effect_rolls_back_records_stamp_and_receipt() {
        let (pool, _) = fixture(|_| {});
        let source = ns("faults");
        seed(&pool, &source);
        let control = PgCancellation::default();
        let original = ReportedRunSnapshot {
            last_session_id: "old".into(),
            ..Default::default()
        };
        let before = stamp();
        save(
            &pool,
            &source,
            0,
            &"a".repeat(64),
            std::slice::from_ref(&original),
            &before,
            &control,
        )
        .unwrap();
        let changes = (0..3)
            .map(|n| ReportedRunSnapshot {
                last_session_id: format!("new-{n}"),
                last_duration: Some(u64::MAX),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        for point in 1..=5 {
            FAIL_AFTER.with(|flag| flag.set(Some(point)));
            assert!(matches!(
                save(
                    &pool,
                    &source,
                    0,
                    &"b".repeat(64),
                    &changes,
                    &before,
                    &control
                ),
                Err(StorageError::Unavailable)
            ));
            let loaded = load(&pool, &source, None, &control).unwrap();
            assert_eq!(loaded.runs.as_slice(), std::slice::from_ref(&original));
            assert_eq!(loaded.stamp, Some(before.clone()));
        }
        save(
            &pool,
            &source,
            0,
            &"b".repeat(64),
            &changes,
            &before,
            &control,
        )
        .unwrap();
        assert_eq!(load(&pool, &source, None, &control).unwrap().runs.len(), 4);
        assert!(matches!(
            save(
                &pool,
                &source,
                1,
                &"c".repeat(64),
                &changes,
                &before,
                &control
            ),
            Err(StorageError::ScopeChanged)
        ));
        assert_eq!(load(&pool, &source, None, &control).unwrap().runs.len(), 4);
    }

    #[test]
    fn pg_reported_facts_selected_work_unrelated_history_and_cancel_refusal() {
        let (pool, _) = fixture(|_| {});
        let source = ns("bounded");
        seed(&pool, &source);
        let control = PgCancellation::default();
        let selected = ReportedRunSnapshot {
            last_session_id: "selected".into(),
            last_duration: Some(u64::MAX),
            ..Default::default()
        };
        let ids = BTreeSet::from(["selected".into()]);
        save(
            &pool,
            &source,
            0,
            &"a".repeat(64),
            std::slice::from_ref(&selected),
            &stamp(),
            &control,
        )
        .unwrap();
        for (index, total) in [0, 256, 2048].into_iter().enumerate() {
            if total > 0 {
                let history = (0..total)
                    .map(|n| ReportedRunSnapshot {
                        last_session_id: format!("unrelated-{n}"),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                save(
                    &pool,
                    &source,
                    0,
                    &format!("{:064x}", index + 16),
                    &history,
                    &stamp(),
                    &control,
                )
                .unwrap();
            }
            READ_WORK.with(|work| work.set(0));
            assert_eq!(
                load(&pool, &source, Some(&ids), &control)
                    .unwrap()
                    .runs
                    .as_slice(),
                std::slice::from_ref(&selected)
            );
            READ_WORK.with(|work| assert_eq!(work.get(), 1));
        }
        let cancelled = PgCancellation::default();
        cancelled.cancel();
        assert!(matches!(
            load(&pool, &source, Some(&ids), &cancelled),
            Err(StorageError::Busy)
        ));
        assert!(matches!(
            save(
                &pool,
                &source,
                0,
                &"f".repeat(64),
                &[],
                &stamp(),
                &cancelled
            ),
            Err(StorageError::Busy)
        ));
        assert_eq!(
            load(&pool, &source, Some(&ids), &control).unwrap().runs,
            [selected]
        );
    }

    #[test]
    fn pg_reported_facts_indexstore_return_and_coherent_export() {
        let (runs, stamp, root) = oracle();
        let (pool, _) = fixture(|_| {});
        let source = ns("snapshot-facts");
        seed(&pool, &source);
        let control = PgCancellation::default();
        save(
            &pool,
            &source,
            0,
            &"a".repeat(64),
            &runs[..1],
            &stamp,
            &control,
        )
        .unwrap();
        let named = BTreeSet::from([runs[0].last_session_id.clone()]);
        let returned = save_selected(
            &pool,
            &source,
            0,
            &"b".repeat(64),
            &runs[1..],
            &stamp,
            Some(&named),
            &control,
        )
        .unwrap();
        assert_eq!(returned.runs, runs);
        assert_eq!(returned.stamp, Some(stamp.clone()));
        assert_eq!(returned.revision, 0);
        let mut replacement = runs[0].clone();
        replacement.capture_at += 1000;
        let changed_stamp = self::stamp();
        let mut exported = Vec::new();
        let mut header = None;
        export_snapshot(
            &pool,
            &source,
            &control,
            |revision, observed| {
                header = Some((revision, observed.cloned()));
                // Advance the same Source after the export has pinned its snapshot.
                save(
                    &pool,
                    &source,
                    0,
                    &"c".repeat(64),
                    std::slice::from_ref(&replacement),
                    &changed_stamp,
                    &control,
                )
            },
            |run| {
                exported.push(run);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(header, Some((0, Some(stamp))));
        assert_eq!(exported, runs);
        let current = load(&pool, &source, Some(&named), &control).unwrap();
        assert_eq!(current.runs, [replacement, runs[1].clone()]);
        assert_eq!(current.stamp, Some(changed_stamp));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pg_reported_facts_export_cancellation_retains_slot_until_callback_exits() {
        use std::sync::mpsc;
        let (pool, _) = fixture(|bounds| bounds.max_connections = 1);
        let source = ns("snapshot-lease");
        seed(&pool, &source);
        let control = PgCancellation::default();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = {
            let (pool, source, control) = (pool.clone(), source.clone(), control.clone());
            std::thread::spawn(move || {
                export_snapshot(
                    &pool,
                    &source,
                    &control,
                    |_, _| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(())
                    },
                    |_| panic!("empty snapshot produced a row"),
                )
            })
        };
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        control.cancel();
        assert!(matches!(
            load(&pool, &source, None, &PgCancellation::default()),
            Err(StorageError::Busy)
        ));
        assert_eq!(pool.accounted(), 1);
        release_tx.send(()).unwrap();
        assert!(matches!(worker.join().unwrap(), Err(StorageError::Busy)));
        let loaded = load(&pool, &source, None, &PgCancellation::default()).unwrap();
        assert!(loaded.runs.is_empty());
        assert_eq!(loaded.stamp, None);
    }

    #[test]
    fn pg_reported_facts_competing_exact_requests_and_writers() {
        use std::sync::{Arc, Barrier};
        let (pool, factory) = fixture(|_| {});
        let source = ns("competing-facts");
        seed(&pool, &source);
        let first = ReportedRunSnapshot {
            last_session_id: "same\0原生".into(),
            last_duration: Some(u64::MAX),
            ..Default::default()
        };
        for (round, different) in [(0, false), (1, true)] {
            let barrier = Arc::new(Barrier::new(3));
            let workers = (0..2)
                .map(|n| {
                    let (pool, source, barrier, mut run) =
                        (pool.clone(), source.clone(), barrier.clone(), first.clone());
                    run.last_start_time = round;
                    if different && n == 1 {
                        run.last_duration = None;
                    }
                    std::thread::spawn(move || {
                        barrier.wait();
                        save(
                            &pool,
                            &source,
                            0,
                            &format!("{:064x}", round + 10),
                            &[run],
                            &stamp(),
                            &PgCancellation::default(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            barrier.wait();
            let results = workers
                .into_iter()
                .map(|w| w.join().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                results.iter().filter(|r| r.is_ok()).count(),
                if different { 1 } else { 2 }
            );
            assert_eq!(
                results
                    .iter()
                    .filter(|r| matches!(r, Err(StorageError::InvalidArguments)))
                    .count(),
                usize::from(different)
            );
        }
        let barrier = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|n| {
                let (pool, source, barrier, mut run) =
                    (pool.clone(), source.clone(), barrier.clone(), first.clone());
                run.last_start_time = n + 20;
                std::thread::spawn(move || {
                    barrier.wait();
                    save(
                        &pool,
                        &source,
                        0,
                        &format!("{:064x}", n + 20),
                        &[run],
                        &stamp(),
                        &PgCancellation::default(),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        drop(pool);
        let pool = PgPool::new(factory, limits()).unwrap();
        let loaded = load(&pool, &source, None, &PgCancellation::default()).unwrap();
        assert_eq!(loaded.runs.len(), 4);
        assert_eq!(
            loaded
                .runs
                .iter()
                .map(|r| r.last_start_time)
                .collect::<Vec<_>>(),
            [0, 1, 20, 21]
        );
        assert_eq!(loaded.stamp, Some(stamp()));
    }

    #[test]
    fn pg_reported_facts_inflight_cancel_and_deadline_leave_no_effect() {
        use std::time::{Duration, Instant};
        for cancel in [false, true] {
            let (pool, factory) = fixture(|bounds| {
                bounds.max_connections = 1;
                bounds.txn_deadline = Duration::from_millis(800);
            });
            let source = ns("blocked-facts");
            seed(&pool, &source);
            let mut blocker = factory.client();
            blocker
                .query_one(
                    "SELECT set_config('semon.namespace',$1,false)",
                    &[&super::super::namespace_context(&source)],
                )
                .unwrap();
            blocker.batch_execute("BEGIN").unwrap();
            blocker
                .query_one("SELECT revision FROM semon_pg_source_state FOR UPDATE", &[])
                .unwrap();
            let pid: i32 = blocker
                .query_one("SELECT pg_backend_pid()", &[])
                .unwrap()
                .get(0);
            let control = PgCancellation::default();
            let worker = {
                let (pool, source, control) = (pool.clone(), source.clone(), control.clone());
                std::thread::spawn(move || {
                    save(
                        &pool,
                        &source,
                        0,
                        &"a".repeat(64),
                        &[ReportedRunSnapshot {
                            last_session_id: "blocked".into(),
                            ..Default::default()
                        }],
                        &stamp(),
                        &control,
                    )
                })
            };
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let waiting: bool = blocker.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))", &[&pid]).unwrap().get(0);
                if waiting {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "writer never reached real in-flight lock wait"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(pool.accounted(), 1);
            if cancel {
                control.cancel();
            }
            assert!(matches!(worker.join().unwrap(), Err(StorageError::Busy)));
            blocker.batch_execute("ROLLBACK").unwrap();
            let loaded = load(&pool, &source, None, &PgCancellation::default()).unwrap();
            assert!(loaded.runs.is_empty());
            assert_eq!(loaded.stamp, None);
            save(
                &pool,
                &source,
                0,
                &"a".repeat(64),
                &[ReportedRunSnapshot {
                    last_session_id: "blocked".into(),
                    ..Default::default()
                }],
                &stamp(),
                &PgCancellation::default(),
            )
            .unwrap();
            assert_eq!(
                load(&pool, &source, None, &PgCancellation::default())
                    .unwrap()
                    .runs
                    .len(),
                1
            );
        }
    }

    #[test]
    fn pg_reported_facts_corrupt_state_refuses_without_replacement_or_receipt() {
        for poison in [
            "UPDATE semon_pg_native_reported_runs SET record=decode('00','hex')",
            "UPDATE semon_pg_native_reported_stamp SET stamp=decode('00','hex')",
            "DELETE FROM semon_pg_native_reported_stamp",
            "DELETE FROM semon_pg_source_state",
            "UPDATE semon_pg_source_state SET revision=revision+1",
            "ALTER TABLE semon_pg_store_owner DISABLE ROW LEVEL SECURITY; DELETE FROM semon_pg_store_owner; ALTER TABLE semon_pg_store_owner ENABLE ROW LEVEL SECURITY",
        ] {
            let (pool, factory) = fixture(|_| {});
            let source = ns("corrupt-facts");
            seed(&pool, &source);
            let control = PgCancellation::default();
            let run = ReportedRunSnapshot {
                last_session_id: "same\0原生".into(),
                last_duration: Some(u64::MAX),
                ..Default::default()
            };
            save(
                &pool,
                &source,
                0,
                &"a".repeat(64),
                std::slice::from_ref(&run),
                &stamp(),
                &control,
            )
            .unwrap();
            let mut client = factory.client();
            client
                .query_one(
                    "SELECT set_config('semon.namespace',$1,false)",
                    &[&super::super::namespace_context(&source)],
                )
                .unwrap();
            client.batch_execute(poison).unwrap();
            let state = |client: &mut ::postgres::Client| {
                [
                    "SELECT row_to_json(r)::text FROM (SELECT namespace,session_digest,session_id,start,record FROM semon_pg_native_reported_runs ORDER BY namespace_digest,session_digest,start) r",
                    "SELECT row_to_json(r)::text FROM (SELECT namespace,singleton,stamp FROM semon_pg_native_reported_stamp ORDER BY namespace_digest,singleton) r",
                    "SELECT row_to_json(r)::text FROM (SELECT namespace,operation_id,request_digest,outcome FROM semon_pg_operations ORDER BY namespace_digest,operation_id) r",
                ].into_iter().map(|sql| client.query(sql, &[]).unwrap().into_iter().map(|r| r.get::<_,String>(0)).collect::<Vec<_>>()).collect::<Vec<_>>()
            };
            let before = state(&mut client);
            assert!(
                save(
                    &pool,
                    &source,
                    0,
                    &"b".repeat(64),
                    std::slice::from_ref(&run),
                    &stamp(),
                    &control
                )
                .is_err(),
                "{poison}"
            );
            assert_eq!(state(&mut client), before, "{poison}");
            if !poison.starts_with("UPDATE semon_pg_source_state") {
                assert!(load(&pool, &source, None, &control).is_err(), "{poison}");
            }
        }
    }

    #[test]
    fn pg_reported_facts_warm_schema_drift_and_missing_namespace_refuse() {
        let (pool, factory) = fixture(|_| {});
        let source = ns("drift");
        seed(&pool, &source);
        let control = PgCancellation::default();
        save(&pool, &source, 0, &"a".repeat(64), &[], &stamp(), &control).unwrap();
        assert!(matches!(
            load(&pool, &ns("unadmitted"), None, &control),
            Err(StorageError::ScopeChanged)
        ));
        factory
            .client()
            .batch_execute("ALTER TABLE semon_pg_native_reported_runs DROP COLUMN record")
            .unwrap();
        assert!(matches!(
            save(&pool, &source, 0, &"b".repeat(64), &[], &stamp(), &control),
            Err(StorageError::ScopeChanged)
        ));
        assert!(matches!(
            load(&pool, &source, None, &control),
            Err(StorageError::ScopeChanged)
        ));
    }
}
