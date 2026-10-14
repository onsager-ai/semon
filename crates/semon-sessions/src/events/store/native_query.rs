//! Content-free, resumable native producer storage. This module orders and
//! fences facts; the native model alone decides entries, turns and slots.
use super::*;
use crate::model::summary::{CatalogRow, CatalogSource};

pub(crate) const ROWS: usize = 64;
pub(crate) const BYTES: usize = 1024 * 1024;
const STATE_BYTES: usize = 64 * 1024;
fn source_fact_generation(ledger: &Ledger) -> rusqlite::Result<String> {
    Ok(format!("interval-v2:{}", json(ledger)?))
}
#[cfg(test)]
thread_local! {
    pub(super) static FULL_LINK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static BUSY_FACT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DELTA_LINK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_catalog_owner_versions (
 session_key TEXT NOT NULL, scope TEXT NOT NULL, generation INTEGER NOT NULL,
 PRIMARY KEY(session_key,scope)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_catalog_owner_insert AFTER INSERT ON session_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(NEW.session_key,'current',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_owner_update AFTER UPDATE ON session_catalog
 WHEN NEW.metadata IS NOT OLD.metadata OR NEW.session_key IS NOT OLD.session_key OR NEW.lifecycle IS NOT OLD.lifecycle OR NEW.harness IS NOT OLD.harness OR NEW.parent_key IS NOT OLD.parent_key OR NEW.repo IS NOT OLD.repo OR NEW.last_ms IS NOT OLD.last_ms
BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'current',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
 INSERT INTO native_catalog_owner_versions SELECT NEW.session_key,'current',1 WHERE NEW.session_key IS NOT OLD.session_key ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_owner_delete AFTER DELETE ON session_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'current',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_history_insert AFTER INSERT ON session_history_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(NEW.session_key,'history',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_history_update AFTER UPDATE ON session_history_catalog WHEN NEW.metadata IS NOT OLD.metadata OR NEW.session_key IS NOT OLD.session_key BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'history',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
 INSERT INTO native_catalog_owner_versions SELECT NEW.session_key,'history',1 WHERE NEW.session_key IS NOT OLD.session_key ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS native_catalog_history_delete AFTER DELETE ON session_history_catalog BEGIN
 INSERT INTO native_catalog_owner_versions VALUES(OLD.session_key,'history',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1;
END;
CREATE TABLE IF NOT EXISTS native_query_jobs (
 job INTEGER PRIMARY KEY, view_key TEXT NOT NULL UNIQUE, version INTEGER NOT NULL,
 catalog_generation TEXT, runtime TEXT NOT NULL, phase TEXT NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL, state TEXT NOT NULL,
 sources INTEGER NOT NULL DEFAULT 0, owners INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE IF NOT EXISTS native_query_ready (
 view_key TEXT PRIMARY KEY, job INTEGER NOT NULL, metadata TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_query_sources (
 job INTEGER NOT NULL, path TEXT NOT NULL, metadata TEXT NOT NULL,
 present INTEGER NOT NULL, dev INTEGER, ino INTEGER, size INTEGER,
 mtime_ns BLOB, ctime_ns BLOB, resume_at INTEGER, prefix BLOB, tail BLOB,
 claim INTEGER, archive TEXT, mutation TEXT,
 PRIMARY KEY(job,path)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_query_owners (
 job INTEGER NOT NULL, identity TEXT NOT NULL, session_key TEXT NOT NULL,
 catalog TEXT, history TEXT, private_header TEXT, trace_header TEXT,
 slot_header TEXT, history_header TEXT,
 PRIMARY KEY(job,identity)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_query_rows (
 job INTEGER NOT NULL, kind TEXT NOT NULL,
 k0 INTEGER NOT NULL, k1 INTEGER NOT NULL, k2 INTEGER NOT NULL,
 k3 INTEGER NOT NULL, k4 INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 identity TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(job,kind,k0,k1,k2,k3,k4,ordinal,identity)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_query_row_identity ON native_query_rows(job,kind,identity,k0,k1,k2,k3,k4,ordinal);
";

#[derive(Clone, Debug)]
pub(crate) struct SourceWitness {
    pub(crate) source: CatalogSource,
    pub(crate) ledger: Option<Ledger>,
    pub(crate) claim: Option<i64>,
    pub(crate) archive: Option<String>,
    pub(crate) mutation: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceHeader {
    pub(crate) witness: SourceWitness,
    pub(crate) entrypoint: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) last_model: Option<String>,
    pub(crate) failed: bool,
    pub(crate) first: Option<i64>,
    pub(crate) last: Option<i64>,
    pub(crate) cwd: Option<String>,
    pub(crate) branch: Option<String>,
    /// Missing primitives are repair interest, never permission to decode a
    /// whole header or fabricate facts for an older publication.
    pub(crate) primitives_ready: bool,
    pub(crate) counts: Option<(usize, usize, usize)>,
}

#[derive(Clone, Debug)]
pub(crate) struct OwnerWitness {
    pub(crate) identity: String,
    pub(crate) key: String,
    catalog: Option<String>,
    history: Option<String>,
    private_header: Option<String>,
    trace_header: Option<String>,
    slot_header: Option<String>,
    history_header: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JobSpec {
    pub(crate) view_key: String,
    pub(crate) version: u32,
    pub(crate) catalog_generation: Option<String>,
    pub(crate) runtime: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Job {
    pub(crate) id: i64,
    pub(crate) spec: JobSpec,
    pub(crate) phase: String,
    pub(crate) revision: i64,
    pub(crate) cursor: String,
    pub(crate) state: Value,
}

/// The five native ordering columns remain typed SQL integers. ordinal keeps
/// stable insertion order when several placements share the same native key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct RowKey {
    pub(crate) order: [i64; 5],
    pub(crate) ordinal: i64,
    pub(crate) identity: String,
}

#[derive(Clone, Debug)]
pub(crate) struct StageRow {
    pub(crate) kind: RowKind,
    pub(crate) key: RowKey,
    pub(crate) facts: Value,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RowKind {
    Entry,
    Annotation,
    Group,
    Member,
    Sent,
    Started,
    Operation,
    Copied,
    Relation,
    SourceFact,
    Slot,
    Observation,
    Counter,
    Metadata,
}
impl RowKind {
    fn name(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Annotation => "annotation",
            Self::Group => "group",
            Self::Member => "member",
            Self::Sent => "sent",
            Self::Started => "started",
            Self::Operation => "operation",
            Self::Copied => "copied",
            Self::Relation => "relation",
            Self::SourceFact => "source",
            Self::Slot => "slot",
            Self::Observation => "observation",
            Self::Counter => "counter",
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Work {
    pub(crate) decoded_rows: usize,
    pub(crate) metadata_bytes: usize,
    pub(crate) vm_steps: i32,
}
#[derive(Clone, Debug)]
pub(crate) struct Page<T, K> {
    pub(crate) rows: Vec<T>,
    pub(crate) after: Option<K>,
    pub(crate) complete: bool,
    pub(crate) work: Work,
}

pub(crate) struct NativeStore {
    connection: Connection,
    options: crate::Options,
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(io::Error::new(
        io::ErrorKind::InvalidData,
        message,
    )))
}
fn guard_columns(columns: &str, length: &str) -> String {
    columns
        .split(',')
        .map(str::trim)
        .map(|column| format!("CASE WHEN ({length})<=1048576 THEN {column} END"))
        .collect::<Vec<_>>()
        .join(",")
}
fn bounded_json<T: Serialize>(value: &T, maximum: usize) -> rusqlite::Result<String> {
    let text = json(value)?;
    if text.len() > maximum {
        return Err(invalid("native producer metadata budget exceeded"));
    }
    Ok(text)
}
fn bounded_value(db: &Connection, key: &str, maximum: usize) -> rusqlite::Result<Option<String>> {
    let row: Option<(i64, Option<String>)> = db.query_row("SELECT octet_length(value),CASE WHEN octet_length(value)<=?2 THEN value END FROM meta WHERE key=?1", params![key, maximum as i64], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    match row {
        Some((_, None)) => Err(invalid("native producer witness exceeds metadata budget")),
        Some((_, text)) => Ok(text),
        None => Ok(None),
    }
}
fn header(db: &Connection, table: &str, key: &str) -> rusqlite::Result<Option<String>> {
    let columns = match table {
        "session_trace_headers" => "generation,source_generation",
        "session_slot_projections" => "generation,version,total,source_generation",
        "session_history_projections" => "generation,version,total,source_generation,incomplete",
        _ => return Err(invalid("unknown native producer header")),
    };
    let length = columns
        .split(',')
        .map(|column| format!("COALESCE(octet_length({column}),0)"))
        .collect::<Vec<_>>()
        .join("+");
    let sql = format!(
        "SELECT CASE WHEN ({length})<=65536 THEN json_array({columns}) END FROM {table} WHERE session_key=?1"
    );
    let row: Option<Option<String>> = db.query_row(&sql, [key], |r| r.get(0)).optional()?;
    match row {
        Some(None) => Err(invalid("native header witness exceeds metadata budget")),
        Some(text) => Ok(text),
        None => Ok(None),
    }
}
fn source_matches(source: &CatalogSource, ledger: &Ledger) -> bool {
    source.dev == ledger.stat.dev
        && source.ino == ledger.stat.ino
        && source.size == ledger.stat.size
        && source.modified_ns == ledger.stat.modified_ns
        && source.changed_ns == ledger.stat.changed_ns
        && source.offset == ledger.offset
        && source.prefix_sha256 == ledger.prefix
        && source.tail_sha256 == ledger.tail
}
fn capture_source(db: &Connection, source: &CatalogSource) -> rusqlite::Result<SourceWitness> {
    if source.path.to_str().is_none() {
        return Err(invalid("native source path is not portable"));
    }
    let ledger = ledger_row(db, &source.path.to_string_lossy())?.map(|(_, ledger)| ledger);
    if ledger
        .as_ref()
        .is_some_and(|ledger| !source_matches(source, ledger))
    {
        return Err(invalid("native source generation changed"));
    }
    let path = source.path.to_string_lossy();
    let mutation: Option<Option<String>> = db.query_row("SELECT CASE WHEN octet_length(evidence)<=65536 THEN evidence END FROM session_source_mutations WHERE source_path=?1", [&*path], |r| r.get(0)).optional()?;
    if matches!(mutation, Some(None)) {
        return Err(invalid(
            "native source mutation witness exceeds metadata budget",
        ));
    }
    Ok(SourceWitness {
        source: source.clone(),
        ledger,
        claim: db
            .query_row(
                "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
                [&*path],
                |r| r.get(0),
            )
            .optional()?,
        archive: bounded_value(db, &archive_index_key(&path), 16384)?,
        mutation: mutation.flatten(),
    })
}
fn check_source(db: &Connection, expected: &SourceWitness) -> rusqlite::Result<bool> {
    if !current(db)? {
        return Ok(false);
    }
    let actual = capture_source(db, &expected.source)?;
    Ok(actual.ledger == expected.ledger
        && actual.claim == expected.claim
        && actual.archive == expected.archive
        && actual.mutation == expected.mutation)
}
pub(super) fn pending(db: &Connection) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_query_jobs WHERE phase<>'ready')",
        [],
        |r| r.get(0),
    )
}

impl NativeStore {
    fn runtime_current(&self, spec: &JobSpec) -> io::Result<bool> {
        Ok(crate::native_evidence::query_runtime_generation(&self.options)? == spec.runtime)
    }
    pub(crate) fn begin(&mut self, spec: &JobSpec) -> io::Result<Job> {
        bounded_json(spec, STATE_BYTES).map_err(io::Error::other)?;
        if !self.runtime_current(spec)? {
            return Err(io::Error::other("native producer runtime changed"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !spec_current(&tx, spec).map_err(io::Error::other)? {
            return Err(io::Error::other("native producer publication changed"));
        }
        if let Some(mut old) = read_job(&tx, &spec.view_key).map_err(io::Error::other)? {
            if old.spec != *spec || !witnesses_current(&tx, old.id).map_err(io::Error::other)? {
                tx.execute(
                    "UPDATE native_query_jobs SET phase='cleanup',revision=revision+1 WHERE job=?1",
                    [old.id],
                )
                .map_err(io::Error::other)?;
                tx.execute("DELETE FROM native_query_ready WHERE job=?1", [old.id])
                    .map_err(io::Error::other)?;
                old.phase = "cleanup".into();
                old.revision += 1;
                tx.commit().map_err(io::Error::other)?;
                return Ok(old);
            }
            return Ok(old);
        }
        tx.execute("INSERT INTO meta(key,value) VALUES('native_query_epoch','1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)", []).map_err(io::Error::other)?;
        let id: i64 = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key='native_query_epoch'",
                [],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_query_jobs(job,view_key,version,catalog_generation,runtime,phase,revision,cursor,state) VALUES(?1,?2,?3,?4,?5,'capture',0,'','{}')", params![id,spec.view_key,spec.version,spec.catalog_generation,spec.runtime]).map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok(Job {
            id,
            spec: spec.clone(),
            phase: "capture".into(),
            revision: 0,
            cursor: String::new(),
            state: serde_json::json!({}),
        })
    }
    /// Witness admission is paged and private. The caller supplies the native
    /// closure; no partial admission can be qualified as a reader view.
    pub(crate) fn admit(
        &mut self,
        job: &Job,
        sources: &[SourceWitness],
        owners: &[OwnerWitness],
        cursor: &str,
    ) -> io::Result<Outcome> {
        if sources.len() + owners.len() > ROWS {
            return Err(io::Error::other("native witness row budget exceeded"));
        }
        let mut bytes = cursor.len();
        let source_text = sources
            .iter()
            .map(|s| bounded_json(&s.source, BYTES))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(io::Error::other)?;
        for (source, text) in sources.iter().zip(&source_text) {
            bytes += text.len()
                + source.source.path.as_os_str().len()
                + source.archive.as_ref().map_or(0, String::len)
                + source.mutation.as_ref().map_or(0, String::len)
                + 64;
        }
        for owner in owners {
            bytes += owner_bytes(owner);
        }
        if bytes > BYTES || cursor.len() > STATE_BYTES {
            return Err(io::Error::other("native witness metadata budget exceeded"));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(Outcome::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if job.phase != "capture" || !job_current(&tx, job).map_err(io::Error::other)? {
            return Ok(Outcome::Conflict);
        }
        for (source, text) in sources.iter().zip(source_text) {
            if !check_source(&tx, source).map_err(io::Error::other)? {
                return Ok(Outcome::Conflict);
            }
            let ledger = source.ledger.as_ref();
            tx.execute("INSERT INTO native_query_sources VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(job,path) DO NOTHING", params![job.id,source.source.path.to_string_lossy(),text,ledger.is_some(),ledger.map(|l|int(l.stat.dev)),ledger.map(|l|int(l.stat.ino)),ledger.map(|l|int(l.stat.size)),ledger.map(|l|l.stat.modified_ns.to_be_bytes()),ledger.and_then(|l|l.stat.changed_ns).map(i128::to_be_bytes),ledger.map(|l|int(l.offset)),ledger.map(|l|l.prefix),ledger.map(|l|l.tail),source.claim,source.archive,source.mutation]).map_err(io::Error::other)?;
        }
        for owner in owners {
            if !owner_current(&tx, owner).map_err(io::Error::other)? {
                return Ok(Outcome::Conflict);
            }
            tx.execute("INSERT INTO native_query_owners VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(job,identity) DO NOTHING", params![job.id,owner.identity,owner.key,owner.catalog,owner.history,owner.private_header,owner.trace_header,owner.slot_header,owner.history_header]).map_err(io::Error::other)?;
        }
        let sources_count: i64 = tx
            .query_row(
                "SELECT count(*) FROM native_query_sources WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        let owners_count: i64 = tx
            .query_row(
                "SELECT count(*) FROM native_query_owners WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        if sources_count > 64 || owners_count > 64 {
            return Err(io::Error::other("native producer closure budget exceeded"));
        }
        tx.execute("UPDATE native_query_jobs SET sources=?2,owners=?3,cursor=?4,revision=revision+1 WHERE job=?1", params![job.id,sources_count,owners_count,cursor]).map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
    pub(crate) fn reload(&self, view_key: &str) -> io::Result<Option<Job>> {
        read_job(&self.connection, view_key).map_err(io::Error::other)
    }

    /// All staged output and the native reducer cursor commit together. No
    /// implicit row deletion, unbounded materialization or final body load.
    pub(crate) fn checkpoint(
        &mut self,
        job: &Job,
        phase: &str,
        cursor: &str,
        state: &Value,
        rows: &[StageRow],
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Outcome> {
        if rows.len() > ROWS
            || !matches!(
                phase,
                "entries" | "groups" | "slots" | "metadata" | "finish"
            )
        {
            return Err(io::Error::other("invalid native producer checkpoint"));
        }
        let state = bounded_json(state, STATE_BYTES).map_err(io::Error::other)?;
        let texts = rows
            .iter()
            .map(|r| bounded_json(&r.facts, BYTES))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(io::Error::other)?;
        let bytes = state.len()
            + cursor.len()
            + rows
                .iter()
                .zip(&texts)
                .map(|(r, t)| t.len() + r.key.identity.len() + 64)
                .sum::<usize>();
        if bytes > BYTES || cursor.len() > STATE_BYTES {
            return Err(io::Error::other(
                "native checkpoint metadata budget exceeded",
            ));
        }
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(Outcome::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if matches!(job.phase.as_str(), "cleanup" | "ready")
            || !job_current(&tx, job).map_err(io::Error::other)?
        {
            return Ok(Outcome::Conflict);
        }
        let counts: (i64, i64) = tx
            .query_row(
                "SELECT sources,owners FROM native_query_jobs WHERE job=?1",
                [job.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(io::Error::other)?;
        if counts.0 == 0 || counts.1 == 0 {
            return Err(io::Error::other("native producer witnesses are incomplete"));
        }
        let mut put=tx.prepare_cached("INSERT INTO native_query_rows VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(job,kind,k0,k1,k2,k3,k4,ordinal,identity) DO UPDATE SET metadata=excluded.metadata") .map_err(io::Error::other)?;
        for (row, text) in rows.iter().zip(texts) {
            let k = &row.key;
            put.execute(params![
                job.id,
                row.kind.name(),
                k.order[0],
                k.order[1],
                k.order[2],
                k.order[3],
                k.order[4],
                k.ordinal,
                k.identity,
                text
            ])
            .map_err(io::Error::other)?;
        }
        drop(put);
        tx.execute("UPDATE native_query_jobs SET phase=?2,cursor=?3,state=?4,revision=revision+1 WHERE job=?1",params![job.id,phase,cursor,state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
    /// The native caller verifies completed closure/ranks under this same
    /// transaction. Ready is one pointer to immutable, already-staged rows.
    pub(crate) fn qualify(
        &mut self,
        job: &Job,
        metadata: &Value,
        completed: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Outcome> {
        let metadata = bounded_json(metadata, STATE_BYTES).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(Outcome::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if job.phase != "finish"
            || !job_current(&tx, job).map_err(io::Error::other)?
            || !completed(&tx, job.id).map_err(io::Error::other)?
        {
            return Ok(Outcome::Conflict);
        }
        tx.execute("INSERT INTO native_query_ready VALUES(?1,?2,?3) ON CONFLICT(view_key) DO UPDATE SET job=excluded.job,metadata=excluded.metadata",params![job.spec.view_key,job.id,metadata]).map_err(io::Error::other)?;
        tx.execute(
            "UPDATE native_query_jobs SET phase='ready',revision=revision+1 WHERE job=?1",
            [job.id],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
    pub(crate) fn ready(
        &self,
        view_key: &str,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    ) -> io::Result<Option<(Job, Value)>> {
        let Some(job) = read_job(&self.connection, view_key).map_err(io::Error::other)? else {
            return Ok(None);
        };
        if job.phase != "ready" || !self.runtime_current(&job.spec)? {
            return Ok(None);
        };
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, &job).map_err(io::Error::other)? {
            return Ok(None);
        };
        if !closure(&tx, job.id).map_err(io::Error::other)? {
            return Ok(None);
        }
        let text: Option<String> = tx
            .query_row(
                "SELECT metadata FROM native_query_ready WHERE view_key=?1 AND job=?2",
                params![view_key, job.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        text.map(|text| {
            serde_json::from_str(&text)
                .map(|value| (job, value))
                .map_err(io::Error::other)
        })
        .transpose()
    }
    pub(crate) fn cleanup(&mut self, job: &Job) -> io::Result<Outcome> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        let actual = read_job(&tx, &job.spec.view_key).map_err(io::Error::other)?;
        if actual
            .as_ref()
            .is_none_or(|a| a.id != job.id || a.phase != "cleanup" || a.revision != job.revision)
        {
            return Ok(Outcome::Conflict);
        };
        let mut budget = ROWS;
        let mut bytes_left = BYTES;
        // Row identities/cursors, not metadata values, are selected for bounded
        // cleanup. Witness count is independently bounded by the closure cap.
        let removed =
            cleanup_rows(&tx, job.id, budget, &mut bytes_left).map_err(io::Error::other)?;
        budget -= removed;
        if budget > 0 {
            budget -= cleanup_witnesses(&tx, job.id, "sources", budget, &mut bytes_left)
                .map_err(io::Error::other)?;
        }
        if budget > 0 {
            budget -= cleanup_witnesses(&tx, job.id, "owners", budget, &mut bytes_left)
                .map_err(io::Error::other)?;
        }
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_query_rows WHERE job=?1) OR EXISTS(SELECT 1 FROM native_query_sources WHERE job=?1) OR EXISTS(SELECT 1 FROM native_query_owners WHERE job=?1)",[job.id],|r|r.get(0)).map_err(io::Error::other)?;
        let header_bytes:i64=tx.query_row("SELECT 64+octet_length(view_key)+COALESCE(octet_length(catalog_generation),0)+octet_length(runtime)+octet_length(phase)+octet_length(cursor)+octet_length(state) FROM native_query_jobs WHERE job=?1",[job.id],|r|r.get(0)).map_err(io::Error::other)?;
        let finished = !remains && budget > 0 && header_bytes <= bytes_left as i64;
        if finished {
            tx.execute("DELETE FROM native_query_jobs WHERE job=?1", [job.id])
                .map_err(io::Error::other)?;
        } else {
            tx.execute(
                "UPDATE native_query_jobs SET revision=revision+1 WHERE job=?1",
                [job.id],
            )
            .map_err(io::Error::other)?;
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(if finished {
            Outcome::Written
        } else {
            Outcome::Deferred
        })
    }
    /// The native resolver can discover a new dependency without changing any
    /// captured row. Invalidating its private work preserves original custody.
    pub(crate) fn restart(&mut self, job: &Job) -> io::Result<Outcome> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if read_job(&tx, &job.spec.view_key)
            .map_err(io::Error::other)?
            .is_none_or(|actual| actual.id != job.id || actual.revision != job.revision)
        {
            return Ok(Outcome::Conflict);
        }
        tx.execute(
            "UPDATE native_query_jobs SET phase='cleanup',revision=revision+1 WHERE job=?1",
            [job.id],
        )
        .map_err(io::Error::other)?;
        tx.execute("DELETE FROM native_query_ready WHERE job=?1", [job.id])
            .map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }
}

fn spec_current(db: &Connection, spec: &JobSpec) -> rusqlite::Result<bool> {
    if !current(db)? {
        return Ok(false);
    }
    let Some(expected) = &spec.catalog_generation else {
        return Ok(true);
    };
    Ok(db
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .as_ref()
        == Some(expected))
}
fn cleanup_rows(
    tx: &Transaction<'_>,
    job: i64,
    budget: usize,
    bytes_left: &mut usize,
) -> rusqlite::Result<usize> {
    let mut q=tx.prepare("SELECT kind,k0,k1,k2,k3,k4,ordinal,identity,octet_length(metadata)+octet_length(identity)+64 FROM native_query_rows WHERE job=?1 ORDER BY kind,k0,k1,k2,k3,k4,ordinal,identity LIMIT ?2")?;
    let mut rows = q.query(params![job, budget as i64])?;
    let mut keys = Vec::new();
    while let Some(row) = rows.next()? {
        let bytes = usize::try_from(row.get::<_, i64>(8)?)
            .map_err(|_| invalid("invalid native cleanup size"))?;
        if bytes > *bytes_left {
            break;
        }
        *bytes_left -= bytes;
        keys.push((
            row.get::<_, String>(0)?,
            RowKey {
                order: [
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ],
                ordinal: row.get(6)?,
                identity: row.get(7)?,
            },
        ));
    }
    drop(rows);
    drop(q);
    let mut removed = 0;
    for (kind, k) in keys {
        removed+=tx.execute("DELETE FROM native_query_rows WHERE job=?1 AND kind=?2 AND (k0,k1,k2,k3,k4,ordinal,identity)=(?3,?4,?5,?6,?7,?8,?9)",params![job,kind,k.order[0],k.order[1],k.order[2],k.order[3],k.order[4],k.ordinal,k.identity])?;
    }
    Ok(removed)
}
fn cleanup_witnesses(
    tx: &Transaction<'_>,
    job: i64,
    table: &str,
    budget: usize,
    bytes_left: &mut usize,
) -> rusqlite::Result<usize> {
    let (table, key, length) = match table {
        "sources" => (
            "native_query_sources",
            "path",
            "octet_length(path)+octet_length(metadata)+COALESCE(octet_length(archive),0)+COALESCE(octet_length(mutation),0)+64",
        ),
        "owners" => (
            "native_query_owners",
            "identity",
            "octet_length(identity)+octet_length(session_key)+COALESCE(octet_length(catalog),0)+COALESCE(octet_length(history),0)+COALESCE(octet_length(private_header),0)+COALESCE(octet_length(trace_header),0)+COALESCE(octet_length(slot_header),0)+COALESCE(octet_length(history_header),0)",
        ),
        _ => return Err(invalid("unknown native cleanup table")),
    };
    let mut q = tx.prepare(&format!(
        "SELECT {key},{length} FROM {table} WHERE job=?1 ORDER BY {key} LIMIT ?2"
    ))?;
    let mut rows = q.query(params![job, budget as i64])?;
    let mut keys = Vec::new();
    while let Some(row) = rows.next()? {
        let bytes = usize::try_from(row.get::<_, i64>(1)?)
            .map_err(|_| invalid("invalid native cleanup size"))?;
        if bytes > *bytes_left {
            break;
        }
        *bytes_left -= bytes;
        keys.push(row.get::<_, String>(0)?);
    }
    drop(rows);
    drop(q);
    let mut removed = 0;
    for value in keys {
        removed += tx.execute(
            &format!("DELETE FROM {table} WHERE job=?1 AND {key}=?2"),
            params![job, value],
        )?;
    }
    Ok(removed)
}
fn read_job(db: &Connection, key: &str) -> rusqlite::Result<Option<Job>> {
    db.query_row("SELECT job,version,catalog_generation,runtime,phase,revision,cursor,state FROM native_query_jobs WHERE view_key=?1",[key],|r|{
        let state:String=r.get(7)?;
        Ok(Job{id:r.get(0)?,spec:JobSpec{view_key:key.into(),version:r.get(1)?,catalog_generation:r.get(2)?,runtime:r.get(3)?},phase:r.get(4)?,revision:r.get(5)?,cursor:r.get(6)?,state:from_json(7,&state)?})
    }).optional()
}
fn job_current(db: &Connection, job: &Job) -> rusqlite::Result<bool> {
    Ok(spec_current(db, &job.spec)?
        && read_job(db, &job.spec.view_key)?.is_some_and(|a| {
            a.id == job.id
                && a.revision == job.revision
                && a.phase == job.phase
                && a.spec == job.spec
        })
        && witnesses_current(db, job.id)?)
}
fn owner_bytes(owner: &OwnerWitness) -> usize {
    owner.identity.len()
        + owner.key.len()
        + [
            &owner.catalog,
            &owner.history,
            &owner.private_header,
            &owner.trace_header,
            &owner.slot_header,
            &owner.history_header,
        ]
        .into_iter()
        .map(|v| v.as_ref().map_or(0, String::len))
        .sum::<usize>()
}
fn owner_current(db: &Connection, owner: &OwnerWitness) -> rusqlite::Result<bool> {
    Ok(owner_version(db, "current", &owner.key)? == owner.catalog
        && owner_version(db, "history", &owner.key)? == owner.history
        && bounded_value(
            db,
            &crate::query_projection::prefix("header", &owner.identity),
            STATE_BYTES,
        )? == owner.private_header
        && header(db, "session_trace_headers", &owner.key)? == owner.trace_header
        && header(db, "session_slot_projections", &owner.key)? == owner.slot_header
        && header(db, "session_history_projections", &owner.key)? == owner.history_header)
}
fn owner_version(db: &Connection, scope: &str, key: &str) -> rusqlite::Result<Option<String>> {
    let table = match scope {
        "current" => "session_catalog",
        "history" => "session_history_catalog",
        _ => return Err(invalid("unknown native owner scope")),
    };
    db.query_row(&format!("SELECT json_array(COALESCE((SELECT generation FROM native_catalog_owner_versions WHERE session_key=?1 AND scope=?2),0),EXISTS(SELECT 1 FROM {table} WHERE session_key=?1))"),params![key,scope],|r|r.get(0)).map(Some)
}
fn witnesses_current(db: &Connection, job: i64) -> rusqlite::Result<bool> {
    let source_changed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM native_query_sources s LEFT JOIN files f ON f.path=s.path LEFT JOIN session_catalog_invalidations i ON i.source_path=s.path LEFT JOIN meta a ON a.key='archive_event_index:'||s.path LEFT JOIN session_source_mutations m ON m.source_path=s.path WHERE s.job=?1 AND ((f.file_id IS NOT NULL)<>s.present OR f.dev IS NOT s.dev OR f.ino IS NOT s.ino OR f.size IS NOT s.size OR f.mtime_ns IS NOT s.mtime_ns OR f.ctime_ns IS NOT s.ctime_ns OR f.resume_at IS NOT s.resume_at OR f.head_sha256 IS NOT s.prefix OR f.tail_sha256 IS NOT s.tail OR i.revision IS NOT s.claim OR a.value IS NOT s.archive OR m.evidence IS NOT s.mutation))",[job],|r|r.get(0))?;
    if source_changed {
        return Ok(false);
    };
    db.query_row("SELECT NOT EXISTS(SELECT 1 FROM native_query_owners w LEFT JOIN native_catalog_owner_versions cv ON cv.session_key=w.session_key AND cv.scope='current' LEFT JOIN native_catalog_owner_versions hv ON hv.session_key=w.session_key AND hv.scope='history' LEFT JOIN session_catalog c ON c.session_key=w.session_key LEFT JOIN session_history_catalog h ON h.session_key=w.session_key LEFT JOIN meta p ON p.key='native_query_header:'||octet_length(w.identity)||':'||w.identity||':' LEFT JOIN session_trace_headers t ON t.session_key=w.session_key LEFT JOIN session_slot_projections s ON s.session_key=w.session_key LEFT JOIN session_history_projections r ON r.session_key=w.session_key WHERE w.job=?1 AND (json_array(COALESCE(cv.generation,0),c.session_key IS NOT NULL) IS NOT w.catalog OR json_array(COALESCE(hv.generation,0),h.session_key IS NOT NULL) IS NOT w.history OR p.value IS NOT w.private_header OR COALESCE(octet_length(t.generation),0)+COALESCE(octet_length(t.source_generation),0)>65536 OR CASE WHEN t.session_key IS NOT NULL AND COALESCE(octet_length(t.generation),0)+COALESCE(octet_length(t.source_generation),0)<=65536 THEN json_array(t.generation,t.source_generation) END IS NOT w.trace_header OR COALESCE(octet_length(s.generation),0)+COALESCE(octet_length(s.source_generation),0)+COALESCE(octet_length(s.version),0)+COALESCE(octet_length(s.total),0)>65536 OR CASE WHEN s.session_key IS NOT NULL AND COALESCE(octet_length(s.generation),0)+COALESCE(octet_length(s.source_generation),0)+COALESCE(octet_length(s.version),0)+COALESCE(octet_length(s.total),0)<=65536 THEN json_array(s.generation,s.version,s.total,s.source_generation) END IS NOT w.slot_header OR COALESCE(octet_length(r.generation),0)+COALESCE(octet_length(r.source_generation),0)+COALESCE(octet_length(r.version),0)+COALESCE(octet_length(r.total),0)+COALESCE(octet_length(r.incomplete),0)>65536 OR CASE WHEN r.session_key IS NOT NULL AND COALESCE(octet_length(r.generation),0)+COALESCE(octet_length(r.source_generation),0)+COALESCE(octet_length(r.version),0)+COALESCE(octet_length(r.total),0)+COALESCE(octet_length(r.incomplete),0)<=65536 THEN json_array(r.generation,r.version,r.total,r.source_generation,r.incomplete) END IS NOT w.history_header))",[job],|r|r.get(0))
}

impl NativeStore {
    fn selected_file(&self, witness: &SourceWitness) -> io::Result<i64> {
        if !check_source(&self.connection, witness).map_err(io::Error::other)? {
            return Err(io::Error::other("native source witness changed"));
        }
        ledger_row(
            &self.connection,
            witness.source.path.to_string_lossy().as_ref(),
        )
        .map_err(io::Error::other)?
        .map(|(file, _)| file)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "native facts require original-source repair",
            )
        })
    }
    pub(crate) fn signals_page(
        &self,
        witness: &SourceWitness,
        after: Option<usize>,
    ) -> io::Result<Page<(usize, Signal), usize>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let mut q=tx.prepare(&format!("SELECT {SIGNAL_COLUMNS},64+COALESCE(octet_length(k),0)+COALESCE(octet_length(n),0) FROM signals WHERE file_id=?1 AND seq>?2 ORDER BY seq LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, after.map(seq_int).unwrap_or(-1)])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(7).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other("native signal exceeds metadata budget"));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let seq = seq_of(row.get(0).map_err(io::Error::other)?);
            output.push((
                seq,
                Signal {
                    k: untag(1, row.get(1).map_err(io::Error::other)?).map_err(io::Error::other)?,
                    o: uint(row.get(2).map_err(io::Error::other)?),
                    t: row.get(3).map_err(io::Error::other)?,
                    at: row.get(4).map_err(io::Error::other)?,
                    n: row.get(5).map_err(io::Error::other)?,
                    v: row
                        .get::<_, Option<i64>>(6)
                        .map_err(io::Error::other)?
                        .map(uint),
                },
            ));
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
            end = Some(seq);
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let remains: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM signals WHERE file_id=?1 AND seq>?2)",
                params![file, end.map(seq_int).unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    /// Count/billing facts share read_usage. Record identities page separately;
    /// its growing JSON record_ids array is never decoded by this accessor.
    pub(crate) fn usage_page(
        &self,
        witness: &SourceWitness,
        after: Option<&str>,
    ) -> io::Result<Page<(String, MessageUsage), String>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let columns = USAGE_COLUMNS.replace("record_ids", "'[]'");
        let comparison = if after.is_some() { ">" } else { ">=" };
        let length = "192+octet_length(message_id)+COALESCE(octet_length(model),0)+COALESCE(octet_length(billing_speed),0)+COALESCE(octet_length(billing_service_tier),0)";
        let columns = guard_columns(&columns, length);
        let mut q=tx.prepare(&format!("SELECT {columns},{length} FROM usage WHERE file_id=?1 AND message_id{comparison}?2 ORDER BY message_id LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, after.unwrap_or("")])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.map(str::to_owned);
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(23).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other("native usage exceeds metadata budget"));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let (id, usage) = read_usage(row).map_err(io::Error::other)?;
            end = Some(id.clone());
            output.push((id, usage));
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let comparison = if end.is_some() { ">" } else { ">=" };
        let remains:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM usage WHERE file_id=?1 AND message_id{comparison}?2)"),params![file,end.as_deref().unwrap_or("")],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn identities_page(
        &self,
        witness: &SourceWitness,
        after: Option<(&str, &str)>,
    ) -> io::Result<Page<(String, String), (String, String)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let (message, uuid) = after.unwrap_or(("", ""));
        let mut q=tx.prepare("SELECT message_id,record_uuid,octet_length(message_id)+octet_length(record_uuid) FROM usage_identity WHERE file_id=?1 AND (message_id,record_uuid)>(?2,?3) ORDER BY message_id,record_uuid LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, message, uuid])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.map(|(a, b)| (a.into(), b.into()));
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(2).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other("native identity exceeds metadata budget"));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let pair: (String, String) = (
                row.get(0).map_err(io::Error::other)?,
                row.get(1).map_err(io::Error::other)?,
            );
            end = Some(pair.clone());
            output.push(pair);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let (message, uuid) = end
            .as_ref()
            .map_or(("", ""), |(a, b)| (a.as_str(), b.as_str()));
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_identity WHERE file_id=?1 AND (message_id,record_uuid)>(?2,?3))",params![file,message,uuid],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn busy_page(
        &self,
        witness: &SourceWitness,
        after: Option<i64>,
    ) -> io::Result<Page<(i64, i64), i64>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let ready: Option<String> = tx
            .query_row(
                "SELECT generation FROM native_source_facts WHERE file_id=?1",
                [file],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        if ready.as_deref()
            != witness
                .ledger
                .as_ref()
                .map(source_fact_generation)
                .transpose()
                .map_err(io::Error::other)?
                .as_deref()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "native busy primitives require bounded repair",
            ));
        }
        let comparison = if after.is_some() { ">" } else { ">=" };
        let mut query=tx.prepare(&format!("SELECT start,end FROM native_source_busy_intervals WHERE file_id=?1 AND start{comparison}?2 ORDER BY start LIMIT 64")).map_err(io::Error::other)?;
        let output = query
            .query_map(params![file, after.unwrap_or(i64::MIN)], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(io::Error::other)?
            .collect::<rusqlite::Result<Vec<(i64, i64)>>>()
            .map_err(io::Error::other)?;
        let end = output.last().map(|interval| interval.0).or(after);
        let comparison = if end.is_some() { ">" } else { ">=" };
        let remains:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM native_source_busy_intervals WHERE file_id=?1 AND start{comparison}?2)"),params![file,end.unwrap_or(i64::MIN)],|r|r.get(0)).map_err(io::Error::other)?;
        let work = Work {
            decoded_rows: output.len(),
            metadata_bytes: output.len() * 16,
            vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        };
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn stage_page(
        &self,
        job: &Job,
        kind: RowKind,
        after: Option<&RowKey>,
    ) -> io::Result<Page<StageRow, RowKey>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)? {
            return Err(io::Error::other("native stage witness changed"));
        }
        let default = RowKey {
            order: [i64::MIN; 5],
            ordinal: i64::MIN,
            identity: String::new(),
        };
        let a = after.unwrap_or(&default);
        let mut q=tx.prepare("SELECT k0,k1,k2,k3,k4,ordinal,identity,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,octet_length(metadata)+octet_length(identity)+64 FROM native_query_rows WHERE job=?1 AND kind=?2 AND (k0,k1,k2,k3,k4,ordinal,identity)>(?3,?4,?5,?6,?7,?8,?9) ORDER BY k0,k1,k2,k3,k4,ordinal,identity LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![
                job.id,
                kind.name(),
                a.order[0],
                a.order[1],
                a.order[2],
                a.order[3],
                a.order[4],
                a.ordinal,
                a.identity
            ])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.cloned();
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(8).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native staged fact exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let text: String = row.get(7).map_err(io::Error::other)?;
            let key = RowKey {
                order: [
                    row.get(0).map_err(io::Error::other)?,
                    row.get(1).map_err(io::Error::other)?,
                    row.get(2).map_err(io::Error::other)?,
                    row.get(3).map_err(io::Error::other)?,
                    row.get(4).map_err(io::Error::other)?,
                ],
                ordinal: row.get(5).map_err(io::Error::other)?,
                identity: row.get(6).map_err(io::Error::other)?,
            };
            end = Some(key.clone());
            output.push(StageRow {
                kind,
                key,
                facts: serde_json::from_str(&text).map_err(io::Error::other)?,
            });
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let a = end.as_ref().unwrap_or(&default);
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_query_rows WHERE job=?1 AND kind=?2 AND (k0,k1,k2,k3,k4,ordinal,identity)>(?3,?4,?5,?6,?7,?8,?9))",params![job.id,kind.name(),a.order[0],a.order[1],a.order[2],a.order[3],a.order[4],a.ordinal,a.identity],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn stage_get(
        &self,
        job: &Job,
        kind: RowKind,
        identity: &str,
    ) -> io::Result<Option<StageRow>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)? {
            return Err(io::Error::other("native stage witness changed"));
        }
        let mut q=tx.prepare("SELECT k0,k1,k2,k3,k4,ordinal,identity,CASE WHEN octet_length(metadata)+octet_length(identity)+64<=1048576 THEN metadata END FROM native_query_rows WHERE job=?1 AND kind=?2 AND identity=?3 ORDER BY k0,k1,k2,k3,k4,ordinal LIMIT 2").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![job.id, kind.name(), identity])
            .map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        let text: String = row.get(7).map_err(io::Error::other)?;
        let key = RowKey {
            order: [
                row.get(0).map_err(io::Error::other)?,
                row.get(1).map_err(io::Error::other)?,
                row.get(2).map_err(io::Error::other)?,
                row.get(3).map_err(io::Error::other)?,
                row.get(4).map_err(io::Error::other)?,
            ],
            ordinal: row.get(5).map_err(io::Error::other)?,
            identity: row.get(6).map_err(io::Error::other)?,
        };
        if rows.next().map_err(io::Error::other)?.is_some() {
            return Err(io::Error::other("native staged identity is ambiguous"));
        }
        Ok(Some(StageRow {
            kind,
            key,
            facts: serde_json::from_str(&text).map_err(io::Error::other)?,
        }))
    }
    pub(crate) fn recheck(&self, job: &Job) -> io::Result<bool> {
        if !self.runtime_current(&job.spec)? {
            return Ok(false);
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        job_current(&tx, job).map_err(io::Error::other)
    }
    pub(crate) fn links_page(
        &self,
        witness: &SourceWitness,
        after: Option<(&str, &str)>,
    ) -> io::Result<Page<(String, String), (String, String)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let ready: Option<String> = tx
            .query_row(
                "SELECT generation FROM native_source_facts WHERE file_id=?1",
                [file],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        if ready.as_deref()
            != witness
                .ledger
                .as_ref()
                .map(source_fact_generation)
                .transpose()
                .map_err(io::Error::other)?
                .as_deref()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "native lineage primitives require bounded repair",
            ));
        }
        let (kind, id) = after.unwrap_or(("", ""));
        let mut q=tx.prepare("SELECT kind,CASE WHEN octet_length(native_id)<=1048576 THEN native_id END,octet_length(kind)+octet_length(native_id) FROM native_source_links WHERE file_id=?1 AND (kind,native_id)>(?2,?3) ORDER BY kind,native_id LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q.query(params![file, kind, id]).map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after.map(|(a, b)| (a.into(), b.into()));
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(2).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native lineage fact exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let pair: (String, String) = (
                row.get(0).map_err(io::Error::other)?,
                row.get(1).map_err(io::Error::other)?,
            );
            end = Some(pair.clone());
            output.push(pair);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let (kind, id) = end
            .as_ref()
            .map_or(("", ""), |(a, b)| (a.as_str(), b.as_str()));
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_source_links WHERE file_id=?1 AND (kind,native_id)>(?2,?3))",params![file,kind,id],|r|r.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
    pub(crate) fn codex_usage_page(
        &self,
        witness: &SourceWitness,
        after: Option<usize>,
    ) -> io::Result<Page<(usize, CodexUsageEvent), usize>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let mut q=tx.prepare("SELECT seq,CASE WHEN octet_length(model)<=1048576 THEN model END,input,output,cache_write,cache_read,t,64+octet_length(model) FROM codex_usage WHERE file_id=?1 AND seq>?2 ORDER BY seq LIMIT 64").map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, after.map(seq_int).unwrap_or(-1)])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(7).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native usage event exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let seq = seq_of(row.get(0).map_err(io::Error::other)?);
            if seq != end.map_or(0, |seq| seq + 1) {
                return Err(io::Error::other(
                    "native usage event sequence is incomplete",
                ));
            }
            output.push((
                seq,
                CodexUsageEvent {
                    model: row.get(1).map_err(io::Error::other)?,
                    tokens: ModelTokens {
                        input: uint(row.get(2).map_err(io::Error::other)?),
                        output: uint(row.get(3).map_err(io::Error::other)?),
                        cache_write: uint(row.get(4).map_err(io::Error::other)?),
                        cache_read: uint(row.get(5).map_err(io::Error::other)?),
                    },
                    timestamp: row.get(6).map_err(io::Error::other)?,
                },
            ));
            end = Some(seq);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let remains: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM codex_usage WHERE file_id=?1 AND seq>?2)",
                params![file, end.map(seq_int).unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
}
impl NativeStore {
    /// Scoped initialization never calls read_file or restores a selected Index.
    pub(crate) fn open(options: &crate::Options) -> io::Result<Self> {
        let (store, _) = SqliteStore::attempt_scoped(
            &super::super::EventCache::path(&options.cache),
            None,
            true,
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Self {
            connection: store.connection,
            options: options.clone(),
        })
    }
    pub(crate) fn source_witness(&self, source: &CatalogSource) -> io::Result<SourceWitness> {
        if crate::native_evidence::catalog_input(&source.path, &self.options).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "native source is outside configured roots",
            ));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !current(&tx).map_err(io::Error::other)? {
            return Err(io::Error::other("native store version changed"));
        }
        capture_source(&tx, source).map_err(io::Error::other)
    }
    pub(crate) fn owner_witness(&self, row: &CatalogRow) -> io::Result<OwnerWitness> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !current(&tx).map_err(io::Error::other)? {
            return Err(io::Error::other("native store version changed"));
        }
        let identity = crate::query_projection::identity(row);
        // Native Catalog rows are canonically encoded by their publisher. One
        // supplied row is admitted once; subsequent fences use scalar epochs.
        let encoded = bounded_json(row, BYTES).map_err(io::Error::other)?;
        let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1 AND metadata=?2) OR EXISTS(SELECT 1 FROM session_history_catalog WHERE session_key=?1 AND metadata=?2)",params![row.key,encoded],|r|r.get(0)).map_err(io::Error::other)?;
        if !matches {
            return Err(io::Error::other("native owner witness changed"));
        }
        Ok(OwnerWitness {
            identity: identity.clone(),
            key: row.key.clone(),
            catalog: owner_version(&tx, "current", &row.key).map_err(io::Error::other)?,
            history: owner_version(&tx, "history", &row.key).map_err(io::Error::other)?,
            private_header: bounded_value(
                &tx,
                &crate::query_projection::prefix("header", &identity),
                STATE_BYTES,
            )
            .map_err(io::Error::other)?,
            trace_header: header(&tx, "session_trace_headers", &row.key)
                .map_err(io::Error::other)?,
            slot_header: header(&tx, "session_slot_projections", &row.key)
                .map_err(io::Error::other)?,
            history_header: header(&tx, "session_history_projections", &row.key)
                .map_err(io::Error::other)?,
        })
    }
    pub(crate) fn source_header(
        &self,
        witness: &SourceWitness,
    ) -> io::Result<Option<SourceHeader>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(io::Error::other("native source witness changed"));
        }
        let Some(ledger) = &witness.ledger else {
            return Ok(None);
        };
        let length = "COALESCE(octet_length(entrypoint),0)+COALESCE(octet_length(title),0)+COALESCE(octet_length(agent_name),0)+COALESCE(octet_length(last_model),0)+COALESCE(octet_length(cwd),0)+COALESCE(octet_length(branch),0)";
        let columns = guard_columns(
            "entrypoint,title,agent_name,last_model,failed,first_ms,last_ms,cwd,branch",
            length,
        );
        let mut q = tx.prepare(&format!("SELECT {columns},{length},(SELECT generation FROM native_source_facts WHERE file_id=files.file_id),(SELECT events FROM native_source_facts WHERE file_id=files.file_id),(SELECT extras FROM native_source_facts WHERE file_id=files.file_id),(SELECT signals FROM native_source_facts WHERE file_id=files.file_id) FROM files WHERE path=?1")).map_err(io::Error::other)?;
        let mut rows = q
            .query([witness.source.path.to_string_lossy().as_ref()])
            .map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        if row.get::<_, i64>(9).map_err(io::Error::other)? > BYTES as i64 {
            return Err(io::Error::other(
                "native scalar header exceeds metadata budget",
            ));
        }
        let ready: Option<String> = row.get(10).map_err(io::Error::other)?;
        let primitives_ready =
            ready.as_deref() == Some(&source_fact_generation(ledger).map_err(io::Error::other)?);
        let counts = if primitives_ready {
            Some((
                seq_of(row.get(11).map_err(io::Error::other)?),
                seq_of(row.get(12).map_err(io::Error::other)?),
                seq_of(row.get(13).map_err(io::Error::other)?),
            ))
        } else {
            None
        };
        Ok(Some(SourceHeader {
            witness: witness.clone(),
            entrypoint: row.get(0).map_err(io::Error::other)?,
            title: row.get(1).map_err(io::Error::other)?,
            agent_name: row.get(2).map_err(io::Error::other)?,
            last_model: row.get(3).map_err(io::Error::other)?,
            failed: row.get(4).map_err(io::Error::other)?,
            first: row.get(5).map_err(io::Error::other)?,
            last: row.get(6).map_err(io::Error::other)?,
            cwd: row.get(7).map_err(io::Error::other)?,
            branch: row.get(8).map_err(io::Error::other)?,
            primitives_ready,
            counts,
        }))
    }

    pub(crate) fn events_page(
        &self,
        witness: &SourceWitness,
        extra: bool,
        after: Option<usize>,
    ) -> io::Result<Page<(usize, Event), usize>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(io::Error::other("native event witness changed"));
        }
        let file = ledger_row(&tx, witness.source.path.to_string_lossy().as_ref())
            .map_err(io::Error::other)?
            .map(|(id, _)| id)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "native event facts need original-source repair",
                )
            })?;
        let length = "192+COALESCE(octet_length(k),0)+COALESCE(octet_length(id),0)+COALESCE(octet_length(n),0)+COALESCE(octet_length(y),0)+COALESCE(octet_length(reply_m),0)+COALESCE(octet_length(peer_sock),0)+COALESCE(octet_length(peer_start),0)+COALESCE(octet_length(u),0)";
        let columns = guard_columns(EVENT_COLUMNS, length);
        let mut q = tx.prepare(&format!("SELECT {columns},{length} FROM events WHERE file_id=?1 AND extra=?2 AND seq>?3 ORDER BY seq LIMIT 64")).map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, extra, after.map(seq_int).unwrap_or(-1)])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut work = Work::default();
        let mut end = after;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let length = usize::try_from(row.get::<_, i64>(27).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if length > BYTES {
                return Err(io::Error::other("native event exceeds metadata budget"));
            }
            if work.metadata_bytes + length > BYTES {
                break;
            }
            let (_, seq, event) = read_event(row).map_err(io::Error::other)?;
            if seq != end.map_or(0, |seq| seq + 1) {
                return Err(io::Error::other("native event sequence is incomplete"));
            }
            if let Some(yielded) = &event.y
                && yielded.polls.len() > super::super::POLLS_MAX
            {
                return Err(io::Error::other(
                    "native yielded poll evidence exceeds parser contract",
                ));
            }
            work.metadata_bytes += length;
            work.decoded_rows += 1;
            end = Some(seq);
            output.push((seq, event));
        }
        drop(rows);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let remaining: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE file_id=?1 AND extra=?2 AND seq>?3)",
                params![file, extra, end.map(seq_int).unwrap_or(-1)],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remaining,
            work,
        })
    }
    /// Native extras inherit the last regular event at or before their byte
    /// offset, with seq0 as the native partition_point fallback. Both probes
    /// use native indexes and never walk the preceding event history.
    pub(crate) fn event_before_offset(
        &self,
        witness: &SourceWitness,
        offset: u64,
    ) -> io::Result<Option<(usize, Event)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let file = self.selected_file(witness)?;
        let seq: Option<i64> = tx.query_row(
            "SELECT seq FROM events WHERE file_id=?1 AND extra=0 AND o<=?2 ORDER BY o DESC,seq DESC LIMIT 1",
            params![file, offset as i64], |r| r.get(0),
        ).optional().map_err(io::Error::other)?;
        let seq = seq.unwrap_or(0);
        let length = "192+COALESCE(octet_length(k),0)+COALESCE(octet_length(id),0)+COALESCE(octet_length(n),0)+COALESCE(octet_length(y),0)+COALESCE(octet_length(reply_m),0)+COALESCE(octet_length(peer_sock),0)+COALESCE(octet_length(peer_start),0)+COALESCE(octet_length(u),0)";
        let columns = guard_columns(EVENT_COLUMNS, length);
        let mut query = tx
            .prepare(&format!(
                "SELECT {columns},{length} FROM events WHERE file_id=?1 AND extra=0 AND seq=?2"
            ))
            .map_err(io::Error::other)?;
        let mut rows = query.query(params![file, seq]).map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        if row.get::<_, i64>(27).map_err(io::Error::other)? > BYTES as i64 {
            return Err(io::Error::other(
                "native point event exceeds metadata budget",
            ));
        }
        let (_, seq, event) = read_event(row).map_err(io::Error::other)?;
        Ok(Some((seq, event)))
    }
    pub(crate) fn event_at(
        &self,
        witness: &SourceWitness,
        extra: bool,
        seq: usize,
    ) -> io::Result<Option<Event>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(io::Error::other("native event witness changed"));
        }
        let file = ledger_row(&tx, witness.source.path.to_string_lossy().as_ref())
            .map_err(io::Error::other)?
            .map(|(id, _)| id);
        let Some(file) = file else { return Ok(None) };
        let length = "192+COALESCE(octet_length(k),0)+COALESCE(octet_length(id),0)+COALESCE(octet_length(n),0)+COALESCE(octet_length(y),0)+COALESCE(octet_length(reply_m),0)+COALESCE(octet_length(peer_sock),0)+COALESCE(octet_length(peer_start),0)+COALESCE(octet_length(u),0)";
        let columns = guard_columns(EVENT_COLUMNS, length);
        let mut q = tx
            .prepare(&format!(
                "SELECT {columns},{length} FROM events WHERE file_id=?1 AND extra=?2 AND seq=?3"
            ))
            .map_err(io::Error::other)?;
        let mut rows = q
            .query(params![file, extra, seq_int(seq)])
            .map_err(io::Error::other)?;
        let Some(row) = rows.next().map_err(io::Error::other)? else {
            return Ok(None);
        };
        if row.get::<_, i64>(27).map_err(io::Error::other)? > BYTES as i64 {
            return Err(io::Error::other(
                "native point event exceeds metadata budget",
            ));
        }
        Ok(Some(read_event(row).map_err(io::Error::other)?.2))
    }
}

/// Writer-owned primitive rows. Append applies the native clustering delta by
/// stable interval start; directed links are the exact parser delta. Older stores are
/// deliberately not marked ready after only a partial append.
pub(super) fn write_source_facts(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    changes: &super::super::DirtyRows,
    complete: bool,
) -> rusqlite::Result<()> {
    let file: i64 = tx.query_row("SELECT file_id FROM files WHERE path=?1", [path], |r| {
        r.get(0)
    })?;
    let was_ready: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_source_facts WHERE file_id=?1 AND generation LIKE 'interval-v2:%')",[file],|r|r.get(0),
    )?;
    if complete {
        tx.execute(
            "DELETE FROM native_source_busy_intervals WHERE file_id=?1",
            [file],
        )?;
    }
    for (start, end) in &changes.busy {
        #[cfg(test)]
        BUSY_FACT_VISITS.with(|n| n.set(n.get() + 1));
        if let Some(end) = end {
            tx.execute("INSERT INTO native_source_busy_intervals VALUES(?1,?2,?3) ON CONFLICT(file_id,start) DO UPDATE SET end=excluded.end",params![file,start,end])?;
        } else {
            tx.execute(
                "DELETE FROM native_source_busy_intervals WHERE file_id=?1 AND start=?2",
                params![file, start],
            )?;
        }
    }
    for (kind, id) in &changes.links {
        #[cfg(test)]
        DELTA_LINK_VISITS.with(|n| n.set(n.get() + 1));
        let present = match kind.as_str() {
            "session" => index.links.session_ids.contains(id),
            "continued" => index.links.continued_in.contains(id),
            "bridge" => index.links.bridges.contains(id),
            _ => return Err(invalid("unsupported native link kind")),
        };
        if present {
            tx.execute(
                "INSERT OR IGNORE INTO native_source_links VALUES(?1,?2,?3)",
                params![file, kind, id],
            )?;
        } else {
            tx.execute(
                "DELETE FROM native_source_links WHERE file_id=?1 AND kind=?2 AND native_id=?3",
                params![file, kind, id],
            )?;
        }
    }
    if complete || was_ready {
        tx.execute(
            "INSERT OR REPLACE INTO native_source_facts VALUES(?1,?2,?3,?4,?5)",
            params![
                file,
                source_fact_generation(ledger)?,
                seq_int(index.events.len()),
                seq_int(index.extras.len()),
                seq_int(index.signals.len())
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Kind, SignalKind};
    use std::cell::Cell;

    struct Fixture {
        root: PathBuf,
        options: crate::Options,
        index: FileIndex,
        row: CatalogRow,
        store: NativeStore,
    }
    impl Fixture {
        fn new(earlier: usize) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-native-query-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let options = crate::Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                cache: root.join("cache"),
                ..crate::Options::default()
            };
            let path = options.claude_home.join("projects/p/chosen.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = b"fixture\n".repeat(earlier + 1);
            fs::write(&path, &bytes).unwrap();
            let ledger = Ledger {
                stat: Stat::of(&path).unwrap(),
                offset: bytes.len() as u64,
                prefix: Sha256::digest(&bytes).into(),
                tail: Sha256::digest(&bytes[bytes.len().saturating_sub(4096)..]).into(),
            };
            let mut index = FileIndex::default();
            index.events = (0..=earlier)
                .map(|seq| Event {
                    k: Kind::A,
                    o: (seq * 8) as u64,
                    t: Some(seq as i64),
                    id: Some(format!("native-{seq}")),
                    ..Event::default()
                })
                .collect();
            index.extras.push(Event {
                k: Kind::Think,
                o: 0,
                ..Event::default()
            });
            index.signals.push(Signal {
                k: SignalKind::Effort,
                o: 0,
                t: Some(0),
                at: 0,
                n: Some("high".into()),
                v: None,
            });
            index.busy = vec![(0, 1), (700_000, 700_001)];
            index.links.continued_in.insert("next".into());
            let source = ledger.source(&path, "chosen");
            let row:CatalogRow=serde_json::from_value(serde_json::json!({"key":"chosen","name":"chosen","harness":"claude","kind":"session","native_ids":["chosen"],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":"test","effort":null,"start":0,"last":earlier,"tokens":[0.0,0.0,0.0],"cost":crate::pricing::Cost::default(),"sources":[source]})).unwrap();
            let (mut writer, _) = SqliteStore::attempt_scoped(
                &super::super::super::EventCache::path(&options.cache),
                None,
                true,
            )
            .unwrap_or_else(|error| panic!("native fixture store: {error}"));
            assert_eq!(
                writer
                    .write_one(path.to_str().unwrap(), None, None, &ledger, &index)
                    .unwrap(),
                Outcome::Written
            );
            writer.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",params![row.key,row.last.unwrap(),row.harness,json(&row).unwrap()]).unwrap();
            writer
                .connection
                .execute(
                    "INSERT INTO meta(key,value) VALUES('catalog_generation','catalog-one')",
                    [],
                )
                .unwrap();
            let store = NativeStore {
                connection: writer.connection,
                options: options.clone(),
            };
            Self {
                root,
                options,
                index,
                row,
                store,
            }
        }
        fn source(&self) -> SourceWitness {
            self.store.source_witness(&self.row.sources[0]).unwrap()
        }
        fn spec(&self) -> JobSpec {
            JobSpec {
                view_key: "chosen:physical-membership".into(),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&self.options).unwrap(),
            }
        }
        fn admitted(&mut self) -> Job {
            let spec = self.spec();
            let job = self.store.begin(&spec).unwrap();
            let source = self.source();
            let owner = self.store.owner_witness(&self.row).unwrap();
            assert_eq!(
                self.store
                    .admit(&job, &[source], &[owner], "closure-complete")
                    .unwrap(),
                Outcome::Written
            );
            self.store.reload(&spec.view_key).unwrap().unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn stage_row(rank: i64, bytes: usize) -> StageRow {
        StageRow {
            kind: RowKind::Slot,
            key: RowKey {
                order: [rank, 0, 0, 0, 0],
                ordinal: 0,
                identity: format!("slot:{rank}"),
            },
            facts: serde_json::json!({"native":"x".repeat(bytes)}),
        }
    }

    #[test]
    fn native_query_pages_preserve_exact_facts_with_zero_full_index_loads_and_fixed_tail_work() {
        let mut steps = Vec::new();
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(earlier);
            let source = fixture.source();
            let reads = super::super::super::FILE_READS.with(Cell::get);
            let page = fixture
                .store
                .events_page(&source, false, earlier.checked_sub(1))
                .unwrap();
            assert_eq!(
                page.rows,
                vec![(earlier, fixture.index.events[earlier].clone())]
            );
            assert!(page.complete);
            assert_eq!(page.work.decoded_rows, 1);
            assert!(page.work.metadata_bytes < 1024);
            steps.push(page.work.vm_steps);
            assert_eq!(
                fixture.store.event_at(&source, false, earlier).unwrap(),
                Some(fixture.index.events[earlier].clone())
            );
            assert_eq!(
                fixture
                    .store
                    .event_before_offset(&source, (earlier * 8) as u64)
                    .unwrap(),
                Some((earlier, fixture.index.events[earlier].clone()))
            );
            assert_eq!(
                fixture.store.event_before_offset(&source, 0).unwrap(),
                Some((0, fixture.index.events[0].clone()))
            );
            assert_eq!(
                fixture.store.events_page(&source, true, None).unwrap().rows,
                vec![(0, fixture.index.extras[0].clone())]
            );
            assert_eq!(
                fixture.store.signals_page(&source, None).unwrap().rows,
                vec![(0, fixture.index.signals[0].clone())]
            );
            assert_eq!(
                fixture.store.busy_page(&source, None).unwrap().rows,
                vec![(0, 1), (700_000, 700_001)]
            );
            assert_eq!(
                fixture.store.links_page(&source, None).unwrap().rows,
                vec![("continued".into(), "next".into())]
            );
            assert!(
                fixture
                    .store
                    .source_header(&source)
                    .unwrap()
                    .unwrap()
                    .primitives_ready
            );
            assert_eq!(super::super::super::FILE_READS.with(Cell::get), reads);
        }
        assert!(
            steps.iter().max().unwrap() - steps.iter().min().unwrap() <= 8,
            "tail SQL work grew: {steps:?}"
        );
    }
    #[test]
    fn native_query_checkpoint_cancellation_reopen_and_atomic_ready_pointer() {
        let mut fixture = Fixture::new(128);
        let job = fixture.admitted();
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == 2
        };
        let row = stage_row(0, 0);
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "slots",
                    "one",
                    &serde_json::json!({"rank":1}),
                    std::slice::from_ref(&row),
                    &cancel
                )
                .unwrap(),
            Outcome::Deferred
        );
        let same = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        assert_eq!(same.revision, job.revision);
        assert!(
            fixture
                .store
                .stage_get(&same, RowKind::Slot, "slot:0")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &same,
                    "slots",
                    "one",
                    &serde_json::json!({"rank":1}),
                    &[row],
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let resumed = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        assert_eq!(resumed.cursor, "one");
        assert_eq!(resumed.state["rank"], 1);
        assert!(
            fixture
                .store
                .ready(&job.spec.view_key, &|_, _| Ok(true))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fixture
                .store
                .checkpoint(&resumed, "finish", "complete", &resumed.state, &[], &|| {
                    false
                })
                .unwrap(),
            Outcome::Written
        );
        let finished = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let complete = |db: &Connection, job: i64| {
            db.query_row(
                "SELECT count(*)=1 FROM native_query_rows WHERE job=?1 AND kind='slot'",
                [job],
                |r| r.get(0),
            )
        };
        assert_eq!(
            fixture
                .store
                .qualify(
                    &finished,
                    &serde_json::json!({"total":1}),
                    &complete,
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        let (ready, metadata) = fixture
            .store
            .ready(&job.spec.view_key, &complete)
            .unwrap()
            .unwrap();
        assert_eq!(metadata["total"], 1);
        assert_eq!(
            fixture
                .store
                .stage_page(&ready, RowKind::Slot, None)
                .unwrap()
                .rows
                .len(),
            1
        );
    }
    #[test]
    fn native_query_owner_scoped_work_allows_unrelated_publication_and_refuses_related_aba() {
        let mut fixture = Fixture::new(0);
        let job = fixture.admitted();
        fixture
            .store
            .connection
            .execute(
                "UPDATE meta SET value='unrelated-publication' WHERE key='catalog_generation'",
                [],
            )
            .unwrap();
        assert!(fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "entries",
                    "advanced",
                    &serde_json::json!({}),
                    &[],
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        let job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        fixture
            .store
            .connection
            .execute(
                "DELETE FROM session_catalog WHERE session_key=?1",
                [&fixture.row.key],
            )
            .unwrap();
        fixture.store.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",params![fixture.row.key,fixture.row.last.unwrap(),fixture.row.harness,json(&fixture.row).unwrap()]).unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .checkpoint(&job, "slots", "stale", &serde_json::json!({}), &[], &|| {
                    false
                })
                .unwrap(),
            Outcome::Conflict
        );
    }
    #[test]
    fn native_query_scalar_trace_witness_ignores_busy_history_bytes_and_checks_generation() {
        let mut fixture = Fixture::new(0);
        let busy = serde_json::json!({"busy":vec![(1,2);8192]}).to_string();
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO session_trace_headers VALUES(?1,'trace-one','source-one',?2)",
                params![fixture.row.key, busy],
            )
            .unwrap();
        let owner = fixture.store.owner_witness(&fixture.row).unwrap();
        assert!(owner_bytes(&owner) < 4096);
        let job = fixture.admitted();
        assert!(fixture.store.recheck(&job).unwrap());
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_trace_headers SET generation='trace-two' WHERE session_key=?1",
                [&fixture.row.key],
            )
            .unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
    }
    #[test]
    fn native_query_cleanup_respects_combined_row_and_byte_budget_then_restarts() {
        let mut fixture = Fixture::new(0);
        let mut job = fixture.admitted();
        for page in 0..3 {
            let rows = (page * 30..(page + 1) * 30)
                .map(|rank| stage_row(rank, 20_000))
                .collect::<Vec<_>>();
            assert_eq!(
                fixture
                    .store
                    .checkpoint(
                        &job,
                        "slots",
                        "page",
                        &serde_json::json!({}),
                        &rows,
                        &|| false
                    )
                    .unwrap(),
                Outcome::Written
            );
            job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        }
        assert_eq!(fixture.store.restart(&job).unwrap(), Outcome::Written);
        job = fixture.store.reload(&job.spec.view_key).unwrap().unwrap();
        let before: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_query_rows WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fixture.store.cleanup(&job).unwrap(), Outcome::Deferred);
        let after: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_query_rows WHERE job=?1",
                [job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!((1..=52).contains(&(before - after)));
        let mut attempts = 1;
        while let Some(current) = fixture.store.reload(&job.spec.view_key).unwrap() {
            assert!(attempts < 8);
            fixture.store.cleanup(&current).unwrap();
            attempts += 1;
        }
        let next = fixture.store.begin(&fixture.spec()).unwrap();
        assert!(next.id > job.id);
        assert_eq!(next.phase, "capture");
    }
    #[test]
    fn native_query_primitive_append_changes_tail_only_and_old_missing_primitives_remain_pending() {
        let fixture = Fixture::new(0);
        let path = fixture.row.sources[0].path.to_str().unwrap();
        let witness = fixture.source();
        let ledger = witness.ledger.unwrap();
        let mut index = fixture.index.clone();
        index.busy.last_mut().unwrap().1 += 10;
        index.busy.push((1_400_000, 1_400_001));
        index.links.session_ids.insert("origin".into());
        let changes = super::super::super::DirtyRows::between(&fixture.index, &index);
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_source_facts(&tx, path, &ledger, &index, &changes, false).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            fixture
                .store
                .busy_page(&fixture.source(), None)
                .unwrap()
                .rows,
            vec![(0, 1), (700_000, 700_011), (1_400_000, 1_400_001)]
        );
        fixture
            .store
            .connection
            .execute("DELETE FROM native_source_facts", [])
            .unwrap();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_source_facts(&tx, path, &ledger, &index, &changes, false).unwrap();
        tx.commit().unwrap();
        assert!(
            !fixture
                .store
                .source_header(&fixture.source())
                .unwrap()
                .unwrap()
                .primitives_ready
        );
        assert!(fixture.store.busy_page(&fixture.source(), None).is_err());
    }
    #[test]
    fn native_query_incremental_link_work_is_delta_only_across_history_growth() {
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(0);
            let source = fixture.source();
            let ledger = source.ledger.unwrap();
            let path = source.source.path.to_str().unwrap();
            let mut index = fixture.index.clone();
            index
                .links
                .session_ids
                .extend((0..earlier).map(|n| format!("historical-{n}")));
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path, &ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            let mut changes = super::super::super::DirtyRows::begin(&index);
            index.links.session_ids.insert("new-link".into());
            changes.links.insert(("session".into(), "new-link".into()));
            FULL_LINK_VISITS.with(|n| n.set(0));
            DELTA_LINK_VISITS.with(|n| n.set(0));
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path, &ledger, &index, Some(&changes), Some(&ledger)).unwrap();
            tx.commit().unwrap();
            assert_eq!(FULL_LINK_VISITS.with(Cell::get), 0);
            assert_eq!(DELTA_LINK_VISITS.with(Cell::get), 1);
        }
    }
    #[test]
    fn native_query_all_admitted_owners_and_new_closure_are_fenced() {
        let mut fixture = Fixture::new(0);
        let mut related = fixture.row.clone();
        related.key = "related".into();
        related.name = "related".into();
        fixture.store.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",params![related.key,related.last.unwrap(),related.harness,json(&related).unwrap()]).unwrap();
        let spec = fixture.spec();
        let job = fixture.store.begin(&spec).unwrap();
        let source = fixture.source();
        let owners = [
            fixture.store.owner_witness(&fixture.row).unwrap(),
            fixture.store.owner_witness(&related).unwrap(),
        ];
        assert_eq!(
            fixture
                .store
                .admit(&job, &[source], &owners, "all-owners")
                .unwrap(),
            Outcome::Written
        );
        let job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "finish",
                    "finished",
                    &serde_json::json!({}),
                    &[],
                    &|| false
                )
                .unwrap(),
            Outcome::Written
        );
        let job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert_eq!(
            fixture
                .store
                .qualify(&job, &serde_json::json!({}), &|_, _| Ok(false), &|| false)
                .unwrap(),
            Outcome::Conflict
        );
        assert!(
            fixture
                .store
                .ready(&spec.view_key, &|_, _| Ok(false))
                .unwrap()
                .is_none()
        );
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_catalog SET metadata=metadata||' ' WHERE session_key='related'",
                [],
            )
            .unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
        assert_eq!(
            fixture
                .store
                .checkpoint(
                    &job,
                    "finish",
                    "again",
                    &serde_json::json!({}),
                    &[],
                    &|| false
                )
                .unwrap(),
            Outcome::Conflict
        );
    }
    #[test]
    fn native_query_retired_ledger_witness_keeps_archive_and_custody_fences() {
        let mut fixture = Fixture::new(0);
        let path = fixture.row.sources[0].path.to_str().unwrap().to_owned();
        fixture
            .store
            .connection
            .execute("DELETE FROM files WHERE path=?1", [&path])
            .unwrap();
        let archive_key = archive_index_key(&path);
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO meta(key,value) VALUES(?1,'retained-marker-one')",
                [&archive_key],
            )
            .unwrap();
        let source = fixture.source();
        assert!(source.ledger.is_none());
        assert_eq!(source.archive.as_deref(), Some("retained-marker-one"));
        assert!(fixture.store.source_header(&source).unwrap().is_none());
        let job = fixture.admitted();
        assert!(fixture.store.recheck(&job).unwrap());
        fixture
            .store
            .connection
            .execute(
                "UPDATE meta SET value='retained-marker-two' WHERE key=?1",
                [&archive_key],
            )
            .unwrap();
        assert!(!fixture.store.recheck(&job).unwrap());
        // Missing current metadata never asserts permission to read an archive
        // body; only the consumer's original-custody resolver can supply it.
    }
    #[test]
    fn native_query_any_order_busy_delta_matches_native_and_has_fixed_growth_work() {
        for earlier in [0, 128, 1024, 8192] {
            let fixture = Fixture::new(0);
            let witness = fixture.source();
            let ledger = witness.ledger.unwrap();
            let path = witness.source.path.to_str().unwrap();
            let mut index = fixture.index.clone();
            index.busy = (0..=earlier)
                .map(|n| ((n as i64) * 700_000, (n as i64) * 700_000 + 1))
                .collect();
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            write_index_changes(&tx, path, &ledger, &index, None, None).unwrap();
            tx.commit().unwrap();
            for time in [-1000, 350_000, 300_000, 700_000, 1_000_000] {
                let mut oracle = index.busy.clone();
                oracle.push((time, time));
                let oracle = super::super::super::busy_merge(oracle);
                index.dirty_rows = Some(super::super::super::DirtyRows::begin(&index));
                super::super::super::activity(&mut index, Some(time));
                assert_eq!(
                    index.busy, oracle,
                    "native busy order earlier={earlier},time={time}"
                );
                let changes = index.dirty_rows.take().unwrap();
                assert!(changes.busy.len() <= 3);
                BUSY_FACT_VISITS.with(|n| n.set(0));
                let tx = fixture.store.connection.unchecked_transaction().unwrap();
                write_source_facts(&tx, path, &ledger, &index, &changes, false).unwrap();
                tx.commit().unwrap();
                assert!(BUSY_FACT_VISITS.with(Cell::get) <= 3);
                let mut actual = Vec::new();
                let mut after = None;
                loop {
                    let page = fixture.store.busy_page(&fixture.source(), after).unwrap();
                    actual.extend(page.rows);
                    after = page.after;
                    if page.complete {
                        break;
                    }
                }
                assert_eq!(actual, index.busy, "earlier={earlier},time={time}");
            }
        }
    }
    #[test]
    fn native_query_old_positional_busy_generation_is_not_qualified_by_partial_append() {
        let mut fixture = Fixture::new(0);
        let witness = fixture.source();
        let ledger = witness.ledger.unwrap();
        let path = witness.source.path.to_str().unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_source_facts SET generation=?1",
                [json(&ledger).unwrap()],
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute_batch("DROP TABLE native_source_busy_intervals; PRAGMA user_version=29;")
            .unwrap();
        assert!(matches!(
            init(&mut fixture.store.connection).unwrap(),
            Init::Ready
        ));
        assert!(
            !fixture
                .store
                .source_header(&fixture.source())
                .unwrap()
                .unwrap()
                .primitives_ready
        );
        let mut index = fixture.index.clone();
        index.dirty_rows = Some(super::super::super::DirtyRows::begin(&index));
        super::super::super::activity(&mut index, Some(-1000));
        let changes = index.dirty_rows.take().unwrap();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        write_source_facts(&tx, path, &ledger, &index, &changes, false).unwrap();
        tx.commit().unwrap();
        assert!(
            !fixture
                .store
                .source_header(&fixture.source())
                .unwrap()
                .unwrap()
                .primitives_ready
        );
        assert!(fixture.store.busy_page(&fixture.source(), None).is_err());
    }
    #[test]
    fn native_query_additive_migration_preserves_main_and_temp_and_public_standalone_schemas() {
        let mut fixture = Fixture::new(0);
        fixture.store.connection.execute_batch("CREATE TEMP TABLE native_migration_sentinel(value TEXT); INSERT INTO native_migration_sentinel VALUES('unchanged'); CREATE TABLE native_existing_sentinel(value TEXT); INSERT INTO native_existing_sentinel VALUES('unchanged');").unwrap();
        fixture
            .store
            .connection
            .pragma_update(None, "user_version", 28)
            .unwrap();
        assert!(matches!(
            init(&mut fixture.store.connection).unwrap(),
            Init::Ready
        ));
        for table in ["native_migration_sentinel", "native_existing_sentinel"] {
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(&format!("SELECT value FROM {table}"), [], |r| r
                        .get::<_, String>(0))
                    .unwrap(),
                "unchanged"
            );
        }
        assert_eq!(
            fixture
                .store
                .events_page(&fixture.source(), false, None)
                .unwrap()
                .rows,
            vec![(0, fixture.index.events[0].clone())]
        );
        let standalone = Connection::open_in_memory().unwrap();
        standalone
            .execute_batch(crate::causal_trace::SCHEMA)
            .unwrap();
        assert_eq!(
            standalone
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name='meta'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}
