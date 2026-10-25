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
pub(super) fn save(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    expected_revision: i64,
    operation_id: &str,
    runs: &[ReportedRunSnapshot],
    stamp: &ReportedFileStamp,
    control: &PgCancellation,
) -> Result<(), StorageError> {
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
    pool.apply_operation(namespace, &identity, control, |tx| {
        if revision(tx, true)? != expected_revision { return Err(StorageError::ScopeChanged); }
        boundary(tx)?;
        let ns = namespace.as_str().as_bytes();
        for (row, record) in rows.iter().zip(&encoded) {
            tx.check()?;
            let id = row.run.last_session_id.as_bytes();
            // Digest keeps arbitrary valid opaque IDs out of bounded B-tree keys.
            // Full equality guards both namespace and native-ID collisions.
            let count = tx.driver.execute("INSERT INTO semon_pg_native_reported_runs(namespace,session_digest,session_id,start,record) VALUES($1,sha256($2::bytea),$2,$3,$4) ON CONFLICT(namespace_digest,session_digest,start) DO UPDATE SET record=excluded.record WHERE semon_pg_native_reported_runs.namespace=excluded.namespace AND semon_pg_native_reported_runs.session_id=excluded.session_id", &[&ns,&id,&row.run.last_start_time,record]).map_err(sqlstate::error)?;
            if count != 1 { return Err(StorageError::ScopeChanged); }
            boundary(tx)?;
        }
        let count = tx.driver.execute("INSERT INTO semon_pg_native_reported_stamp(namespace,singleton,stamp) VALUES($1,true,$2) ON CONFLICT(namespace_digest,singleton) DO UPDATE SET stamp=excluded.stamp WHERE semon_pg_native_reported_stamp.namespace=excluded.namespace", &[&ns,&stamp]).map_err(sqlstate::error)?;
        if count != 1 { return Err(StorageError::ScopeChanged); }
        boundary(tx)?;
        Ok(serde_json::json!({"reportedFacts":1}))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, limits, ns};
    use super::*;
    use crate::events::EventCache;

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
        let input = root.join(".claude.json");
        for (start, capture) in [(100, 101), (200, 202)] {
            std::fs::write(&input,serde_json::json!({"projects":{"/sanitized":{"lastSessionId":"原生\0id","lastStartTime":start,"lastDuration":u64::MAX,"lastCost":0.125,"lastModelUsage":{"模型\0":{"inputTokens":u64::MAX}}}}}).to_string()).unwrap();
            let mut cache = EventCache::open(&cache_path).unwrap();
            cache
                .refresh_reported_runs(&input, capture, &mut false)
                .unwrap();
        }
        std::fs::remove_file(&input).unwrap();
        let cache = EventCache::open(&cache_path).unwrap();
        let runs = cache.reported_runs().cloned().collect::<Vec<_>>();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].last_duration, Some(u64::MAX));
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
            &[other.clone()],
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
            assert_eq!(loaded.runs, [original.clone()]);
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
        budget.charge(row.get(2))?;
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
        out.push(run);
        cursor = Some(digest);
        start = next_start;
    }
    Ok(())
}

pub(super) fn load(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    ids: Option<&BTreeSet<String>>,
    control: &PgCancellation,
) -> Result<Loaded, StorageError> {
    pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
        let revision = revision(tx, false)?;
        let ns = namespace.as_str().as_bytes();
        let sizes = tx.driver.query("SELECT singleton,octet_length(stamp)::bigint FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 LIMIT 2", &[&ns]).map_err(sqlstate::error)?;
        let mut budget = Budget { rows: 0, bytes: 0 };
        if sizes.len() > 1 || sizes.first().is_some_and(|row| !row.get::<_,bool>(0)) { return Err(StorageError::Unavailable); }
        let stamp = if let Some(size) = sizes.first() {
            budget.charge(size.get(1))?;
            tx.check()?;
            let row = tx.driver.query_one("SELECT stamp FROM semon_pg_native_reported_stamp WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton=true", &[&ns]).map_err(sqlstate::error)?;
            Some(reported_rows::decode_stamp(&row.get::<_,Vec<u8>>(0)).map_err(|_| StorageError::Unavailable)?)
        } else { None };
        let mut runs = Vec::new();
        if let Some(ids) = ids {
            for id in ids { read_rows(tx, Some(id), &mut budget, &mut runs)?; }
        } else { read_rows(tx, None, &mut budget, &mut runs)?; }
        // A missing stamp cannot disguise corrupt non-rebuildable records.
        if stamp.is_none() {
            let found = tx.driver.query("SELECT 1 FROM semon_pg_native_reported_runs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 LIMIT 1", &[&ns]).map_err(sqlstate::error)?;
            if !found.is_empty() { return Err(StorageError::Unavailable); }
        }
        runs.sort_by(|a,b| (&a.last_session_id,a.last_start_time).cmp(&(&b.last_session_id,b.last_start_time)));
        tx.check()?;
        Ok(Loaded { runs, stamp, revision })
    })
}
