//! Private core row family. A file ledger here is only the core-family CAS,
//! never proof of a complete native ingest. The caller must roll back on error.
use super::{sqlstate, txn::PgTxn};
use crate::{
    events::{
        DirtyRows, FileIndex, Ledger,
        file_rows::{FileRow, encoded_native_usage},
        native_rows::{self, EventRow, NativeRowPlan, UsageChanges, UsageRow},
    },
    storage::StorageError,
};
use tokio_postgres::types::ToSql;

#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct SqlEffectDiagnostic {
    rows_attempted: usize,
    rows_completed: usize,
    sql_attempted: usize,
    sql_completed: usize,
    elapsed_us: u128,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct UsageDiagnostic {
    usage_by_id_rows: usize,
    identity_delete: SqlEffectDiagnostic,
    usage_upsert: SqlEffectDiagnostic,
    identity_insert: SqlEffectDiagnostic,
}

#[cfg(test)]
thread_local! {
    static USAGE_DIAGNOSTIC: std::cell::RefCell<UsageDiagnostic> =
        std::cell::RefCell::new(UsageDiagnostic::default());
}

#[cfg(test)]
pub(super) fn reset_usage_diagnostic() {
    USAGE_DIAGNOSTIC.with(|diagnostic| *diagnostic.borrow_mut() = UsageDiagnostic::default());
}

#[cfg(test)]
pub(super) fn usage_diagnostic() -> UsageDiagnostic {
    USAGE_DIAGNOSTIC.with(|diagnostic| *diagnostic.borrow())
}

#[cfg(test)]
fn update_usage_diagnostic(update: impl FnOnce(&mut UsageDiagnostic)) {
    USAGE_DIAGNOSTIC.with(|diagnostic| update(&mut diagnostic.borrow_mut()));
}

#[cfg(test)]
thread_local! { static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }

fn after(tx: &PgTxn<'_>) -> Result<(), StorageError> {
    #[cfg(test)]
    if FAIL_AFTER.with(|flag| match flag.get() {
        Some(1) => {
            flag.set(None);
            true
        }
        Some(n) => {
            flag.set(Some(n - 1));
            false
        }
        None => false,
    }) {
        return Err(StorageError::Unavailable);
    }
    tx.check()
}
fn json(value: &impl serde::Serialize) -> Result<Vec<u8>, StorageError> {
    serde_json::to_vec(value).map_err(|_| StorageError::Unavailable)
}
fn bytes(value: Option<&str>) -> Option<Vec<u8>> {
    value.map(|v| v.as_bytes().to_vec())
}

// Adapter-local bind construction only. Names/types are static call-site literals;
// no SQL interface or driver value is exposed outside this module.
struct Row {
    columns: Vec<&'static str>,
    binds: Vec<String>,
    values: Vec<Box<dyn ToSql + Sync + Send>>,
}
impl Row {
    fn new() -> Self {
        Self {
            columns: Vec::new(),
            binds: Vec::new(),
            values: Vec::new(),
        }
    }
    fn add(&mut self, name: &'static str, value: impl ToSql + Sync + Send + 'static) {
        self.columns.push(name);
        self.values.push(Box::new(value));
        self.binds.push(format!("${}", self.values.len()));
    }
    fn number(&mut self, name: &'static str, value: u64) {
        self.add(name, value.to_string());
        self.binds.last_mut().unwrap().push_str("::text::numeric");
    }
    fn optional_number(&mut self, name: &'static str, value: Option<u64>) {
        self.add(name, value.map(|v| v.to_string()));
        self.binds.last_mut().unwrap().push_str("::text::numeric");
    }
    // Usage pages have one static column layout and at most 64 rows. Preserve
    // each column's existing typed bind, including unsigned numeric encoding.
    fn put_many(tx: &mut PgTxn<'_>, rows: Vec<Self>) -> Result<(), StorageError> {
        tx.check()?;
        let count = rows.len();
        let columns = rows[0].columns.clone();
        let updates = columns
            .iter()
            .filter(|c| !matches!(**c, "namespace" | "file_id" | "message_id"))
            .map(|c| format!("{c}=excluded.{c}"))
            .collect::<Vec<_>>();
        let mut values = Vec::new();
        let mut tuples = Vec::new();
        for row in rows {
            let offset = values.len();
            let binds = row
                .binds
                .iter()
                .enumerate()
                .map(|(at, bind)| {
                    bind.replacen(&format!("${}", at + 1), &format!("${}", offset + at + 1), 1)
                })
                .collect::<Vec<_>>();
            tuples.push(format!("({})", binds.join(",")));
            values.extend(row.values);
        }
        let refs = values
            .iter()
            .map(|v| &**v as &(dyn ToSql + Sync))
            .collect::<Vec<_>>();
        let sql = format!(
            "INSERT INTO semon_pg_native_usage({}) VALUES {} ON CONFLICT(namespace_digest,file_id,message_id) DO UPDATE SET {}",
            columns.join(","),
            tuples.join(","),
            updates.join(",")
        );
        if tx.driver.execute(&sql, &refs).map_err(sqlstate::error)? != count as u64 {
            return Err(StorageError::Conflict);
        }
        for _ in 0..count {
            after(tx)?;
        }
        Ok(())
    }
    fn put(
        self,
        tx: &mut PgTxn<'_>,
        table: &'static str,
        key: &'static str,
    ) -> Result<(), StorageError> {
        tx.check()?;
        let updates = self
            .columns
            .iter()
            .filter(|c| !key.split(',').any(|k| k == **c) && **c != "namespace")
            .map(|c| format!("{c}=excluded.{c}"))
            .collect::<Vec<_>>();
        let refs = self
            .values
            .iter()
            .map(|v| &**v as &(dyn ToSql + Sync))
            .collect::<Vec<_>>();
        let sql = format!(
            "INSERT INTO semon_pg_native_{table}({}) VALUES({}) ON CONFLICT(namespace_digest,{key}) DO UPDATE SET {}",
            self.columns.join(","),
            self.binds.join(","),
            updates.join(",")
        );
        let affected = tx.driver.execute(&sql, &refs).map_err(sqlstate::error)?;
        if affected != 1 {
            return Err(StorageError::Conflict);
        }
        after(tx)
    }
}
fn row(tx: &PgTxn<'_>, id: i64) -> Row {
    let mut row = Row::new();
    row.add("namespace", tx.namespace.as_str().as_bytes().to_vec());
    row.add("file_id", id);
    row
}
fn delete(
    tx: &mut PgTxn<'_>,
    table: &str,
    id: i64,
    suffix: &str,
    parameters: &[&(dyn ToSql + Sync)],
) -> Result<(), StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes();
    let mut values: Vec<&(dyn ToSql + Sync)> = vec![&ns, &id];
    values.extend_from_slice(parameters);
    if matches!(table, "usage" | "usage_identity") {
        loop {
            tx.check()?;
            let count = tx.driver.execute(
                &format!("DELETE FROM semon_pg_native_{table} WHERE ctid IN (SELECT ctid FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 {suffix} LIMIT 64)"),
                &values,
            ).map_err(sqlstate::error)?;
            if count < 64 {
                break;
            }
        }
    } else {
        tx.driver
            .execute(
                &format!(
                    "DELETE FROM semon_pg_native_{table} WHERE namespace=$1 AND file_id=$2 {suffix}"
                ),
                &values,
            )
            .map_err(sqlstate::error)?;
    }
    after(tx)
}
fn ordinal(value: usize) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::InvalidArguments)
}

/// Exact core CAS admission shared with the connected partial bundle.
/// The namespace lock remains held until the caller's transaction ends.
pub(super) fn qualify(
    tx: &mut PgTxn<'_>,
    path: &str,
    expected: Option<&Ledger>,
) -> Result<(i64, bool), StorageError> {
    tx.check()?;
    if super::native_keys::read_meta(tx, b"facts-write-inflight", 1048576)?.is_some()
        || super::native_keys::read_meta(
            tx,
            format!("raw-write-inflight:{path}").as_bytes(),
            16384,
        )?
        .is_some()
    {
        #[cfg(test)]
        eprintln!("PG_INGEST_REFUSED raw_or_facts_inflight");
        return Err(StorageError::Conflict);
    }
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.execute("INSERT INTO semon_pg_source_state(namespace) VALUES($1) ON CONFLICT(namespace_digest) DO NOTHING", &[&ns]).map_err(sqlstate::error)?;
    let locked = tx.driver.query("SELECT namespace FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if locked.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let current = tx
        .driver
        .query(
            "SELECT file_id FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(path,path_digest,$2)",
            &[&ns, &path.as_bytes()],
        )
        .map_err(sqlstate::error)?;
    let id = match (expected, current.first()) {
        (None, None) => tx
            .driver
            .query_one(
                "SELECT coalesce(max(file_id),0)+1 FROM semon_pg_native_files WHERE namespace=$1",
                &[&ns],
            )
            .map_err(sqlstate::error)?
            .get(0),
        (Some(old), Some(current)) => {
            let valid = tx.driver.query("SELECT file_id FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(path,path_digest,$2) AND dev=$3::text::numeric AND ino=$4::text::numeric AND size=$5::text::numeric AND mtime_ns=$6 AND ctime_ns IS NOT DISTINCT FROM $7::bytea AND resume_at=$8::text::numeric AND head_sha256=$9 AND tail_sha256=$10", &[&ns,&path.as_bytes(),&old.stat.dev.to_string(),&old.stat.ino.to_string(),&old.stat.size.to_string(),&old.stat.modified_ns.to_be_bytes().as_slice(),&old.stat.changed_ns.map(|v|v.to_be_bytes().to_vec()),&old.offset.to_string(),&old.prefix.as_slice(),&old.tail.as_slice()]).map_err(sqlstate::error)?;
            if valid.len() != 1 {
                #[cfg(test)]
                eprintln!("PG_INGEST_REFUSED core_ledger");
                return Err(StorageError::Conflict);
            }
            current.get(0)
        }
        _ => {
            #[cfg(test)]
            eprintln!("PG_INGEST_REFUSED core_membership");
            return Err(StorageError::Conflict);
        }
    };
    Ok((id, !current.is_empty()))
}

/// Core-only family, deliberately not connected to a successful IndexStore.
/// All callers must use the existing host write transaction and propagate errors.
#[allow(dead_code)]
pub(super) fn write(
    tx: &mut PgTxn<'_>,
    path: &str,
    expected: Option<&Ledger>,
    ledger: &Ledger,
    index: &FileIndex,
    changes: Option<&DirtyRows>,
) -> Result<i64, StorageError> {
    #[cfg(test)]
    update_usage_diagnostic(|diagnostic| diagnostic.usage_by_id_rows = index.usage().len());
    let (id, exists) = qualify(tx, path, expected)?;
    after(tx)?;
    if changes.is_none() && exists {
        // Identity deletion first is required by the copied-identity triggers.
        for table in [
            "usage_identity",
            "usage",
            "codex_usage",
            "events",
            "signals",
            "tree_copied_identities",
        ] {
            delete(tx, table, id, "", &[])?;
        }
    }
    let file = FileRow::new(ledger, index);
    let mut r = row(tx, id);
    r.add("path", path.as_bytes().to_vec());
    r.number("dev", file.dev);
    r.number("ino", file.ino);
    r.number("size", file.size);
    r.add("mtime_ns", file.modified_ns.to_be_bytes().to_vec());
    r.add(
        "ctime_ns",
        file.changed_ns.map(|v| v.to_be_bytes().to_vec()),
    );
    r.number("resume_at", file.offset);
    r.add("head_sha256", file.prefix.to_vec());
    r.add("tail_sha256", file.tail.to_vec());
    for (name, value) in [
        ("entrypoint", file.entrypoint),
        ("title", file.title),
        ("agent_name", file.agent_name),
        ("last_model", file.last_model),
        ("cwd", file.cwd),
        ("branch", file.branch),
    ] {
        r.add(name, bytes(value.as_deref()));
    }
    r.add("failed", file.failed);
    r.add("first_ms", *file.first);
    r.add("last_ms", *file.last);
    r.add("pending", json(file.pending)?);
    r.add("tool_ids", json(file.tool_ids)?);
    r.add("yields", json(file.yields)?);
    r.add("busy", json(file.busy)?);
    r.add("links", json(file.links)?);
    r.add("codex_tokens", json(file.codex_tokens)?);
    r.add("codex_tokens_by_model", json(file.codex_tokens_by_model)?);
    r.add(
        "rate_limits",
        file.rate_limits.as_ref().map(json).transpose()?,
    );
    r.add(
        "codex_native_usage",
        encoded_native_usage(file.index)
            .map_err(|_| StorageError::Unavailable)?
            .map(String::into_bytes),
    );
    // Like SQLite PUT_FILE, the pricing fingerprint is owned by the pricing
    // refresh stage, not overwritten by a core file write.
    r.add("copilot", file.copilot.as_ref().map(json).transpose()?);
    r.put(tx, "files", "file_id")?;
    let full = if changes.is_none() {
        native_rows::full_changes(index)
    } else {
        DirtyRows::default()
    };
    let changes = changes.unwrap_or(&full);
    let plan = NativeRowPlan::new(index, changes);
    for (extra, rows) in [(false, &plan.events), (true, &plan.extras)] {
        for (seq, event) in rows.upserts() {
            let e = EventRow::encode(event).map_err(|_| StorageError::Unavailable)?;
            let mut r = row(tx, id);
            r.add("extra", extra);
            r.add("seq", ordinal(seq)?);
            r.add("k", e.k.into_bytes());
            r.number("o", e.o as u64);
            r.number("b", u64::from(e.b));
            r.add("t", e.t);
            r.add("id", bytes(e.id));
            r.add("n", bytes(e.n));
            r.add("code_mode", e.code_mode);
            r.add("code_mode_open", e.code_mode_open);
            r.add("parent", e.parent);
            r.add("script", i16::from(e.script));
            r.add("y", e.y.map(String::into_bytes));
            r.add("poll", e.poll);
            r.add("reply", e.reply);
            r.optional_number("reply_o", e.reply_o.map(|v| v as u64));
            r.optional_number("reply_b", e.reply_b.map(u64::from));
            r.add("reply_t", e.reply_t);
            r.add("reply_e", e.reply_e);
            r.add("reply_f", e.reply_f.map(i16::from));
            r.add("reply_m", bytes(e.reply_m));
            r.add("item", e.item);
            r.add("peer", e.has_peer);
            r.add("peer_sock", bytes(e.peer_sock));
            r.add("peer_pid", e.peer_pid);
            r.add("peer_start", bytes(e.peer_start));
            r.add("u", bytes(e.u));
            r.put(tx, "events", "file_id,extra,seq")?;
        }
        if let Some(from) = rows.delete_from() {
            delete(
                tx,
                "events",
                id,
                "AND extra=$3 AND seq >= $4",
                &[&extra, &ordinal(from)?],
            )?;
        }
    }
    for (seq, s) in plan.signals.upserts() {
        let mut r = row(tx, id);
        let kind = serde_json::to_value(s.k).map_err(|_| StorageError::Unavailable)?;
        r.add("seq", ordinal(seq)?);
        r.add(
            "k",
            kind.as_str()
                .ok_or(StorageError::Unavailable)?
                .as_bytes()
                .to_vec(),
        );
        r.number("o", s.o);
        r.add("t", s.t);
        r.number("at", u64::from(s.at));
        r.add("n", bytes(s.n.as_deref()));
        r.add("v", s.v.map(|v| v as i64));
        r.put(tx, "signals", "file_id,seq")?;
    }
    if let Some(from) = plan.signals.delete_from() {
        delete(tx, "signals", id, "AND seq >= $3", &[&ordinal(from)?])?;
    }
    write_usage(tx, id, changes, index)?;
    for (seq, u) in plan.codex_usage.upserts() {
        let mut r = row(tx, id);
        r.add("seq", ordinal(seq)?);
        r.add("model", u.model.as_bytes().to_vec());
        r.number("input", u.tokens.input);
        r.number("output", u.tokens.output);
        r.number("cache_write", u.tokens.cache_write);
        r.number("cache_read", u.tokens.cache_read);
        r.add("t", u.timestamp);
        r.put(tx, "codex_usage", "file_id,seq")?;
    }
    if let Some(from) = plan.codex_usage.delete_from() {
        delete(tx, "codex_usage", id, "AND seq >= $3", &[&ordinal(from)?])?;
    }
    after(tx)?;
    Ok(id)
}

// A page contains distinct messages from UsageChanges' original BTree order.
// Copy triggers only relate the same (message, record) across different files:
// regrouping a single file's distinct messages cannot change that relation.
// Keep identity deletion before usage replacement before identity insertion in
// every page, and retain every logical rollback/cancellation boundary.
fn write_usage(
    tx: &mut PgTxn<'_>,
    id: i64,
    changes: &DirtyRows,
    index: &FileIndex,
) -> Result<(), StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let changes = UsageChanges::new(changes, index.usage());
    let mut messages = changes.iter();
    loop {
        let page = messages.by_ref().take(64).collect::<Vec<_>>();
        if page.is_empty() {
            break;
        }
        let keys = page
            .iter()
            .map(|(message, _)| message.as_bytes().to_vec())
            .collect::<Vec<_>>();
        #[cfg(test)]
        let started = Instant::now();
        #[cfg(test)]
        update_usage_diagnostic(|d| d.identity_delete.rows_attempted += page.len());
        // One message can have many record UUIDs; limit physical deletions too.
        loop {
            tx.check()?;
            #[cfg(test)]
            update_usage_diagnostic(|d| d.identity_delete.sql_attempted += 1);
            let count = tx.driver.execute("DELETE FROM semon_pg_native_usage_identity WHERE ctid IN (SELECT ctid FROM semon_pg_native_usage_identity WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 AND message_id=ANY($3::bytea[]) LIMIT 64)", &[&ns,&id,&keys]).map_err(sqlstate::error)?;
            #[cfg(test)]
            update_usage_diagnostic(|d| d.identity_delete.sql_completed += 1);
            if count < 64 {
                break;
            }
        }
        for _ in &page {
            after(tx)?;
        }
        #[cfg(test)]
        update_usage_diagnostic(|d| {
            d.identity_delete.rows_completed += page.len();
            d.identity_delete.elapsed_us += started.elapsed().as_micros();
        });
        let mut rows = Vec::new();
        let mut removed = Vec::new();
        for &(message, usage) in &page {
            let Some(usage) = usage else {
                removed.push(message.as_bytes().to_vec());
                continue;
            };
            let u = UsageRow::encode(usage).map_err(|_| StorageError::Unavailable)?;
            let mut r = row(tx, id);
            r.add("message_id", message.as_bytes().to_vec());
            r.add("model", bytes(u.model));
            for (name, value) in [
                ("input", u.input),
                ("cached_input", u.cached_input),
                ("output", u.output),
                ("reasoning_output", u.reasoning_output),
                ("total", u.total),
                ("model_input", u.model_input),
                ("model_output", u.model_output),
                ("model_cache_write", u.model_cache_write),
                ("model_cache_read", u.model_cache_read),
                ("billing_input", u.billing_input),
                ("billing_output", u.billing_output),
                ("billing_cache_read", u.billing_cache_read),
                ("billing_cache_write_5m", u.cache_write_5m),
                ("billing_cache_write_1h", u.cache_write_1h),
                ("billing_web_search_requests", u.web_search_requests),
                ("billing_prompt_size", u.prompt_size),
            ] {
                r.number(name, value as u64);
            }
            r.add("billing_speed", bytes(u.speed));
            r.add("billing_service_tier", bytes(u.service_tier));
            r.add("billing_timestamp", u.timestamp);
            r.add("billing_split_unknown", u.split_unknown);
            r.add("record_ids", u.record_ids.into_bytes());
            rows.push(r);
        }
        if !rows.is_empty() {
            #[cfg(test)]
            let started = Instant::now();
            #[cfg(test)]
            let count = rows.len();
            #[cfg(test)]
            update_usage_diagnostic(|d| {
                d.usage_upsert.rows_attempted += count;
                d.usage_upsert.sql_attempted += 1;
            });
            Row::put_many(tx, rows)?;
            #[cfg(test)]
            update_usage_diagnostic(|d| {
                d.usage_upsert.rows_completed += count;
                d.usage_upsert.sql_completed += 1;
                d.usage_upsert.elapsed_us += started.elapsed().as_micros();
            });
        }
        if !removed.is_empty() {
            tx.check()?;
            let count = tx.driver.execute("DELETE FROM semon_pg_native_usage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 AND message_id=ANY($3::bytea[])", &[&ns,&id,&removed]).map_err(sqlstate::error)?;
            if count > removed.len() as u64 {
                return Err(StorageError::Conflict);
            }
            for _ in &removed {
                after(tx)?;
            }
        }
        let mut identities = page.iter().flat_map(|(message, usage)| {
            usage
                .iter()
                .flat_map(move |usage| usage.record_ids.iter().map(move |uuid| (*message, uuid)))
        });
        loop {
            let identities = identities.by_ref().take(64).collect::<Vec<_>>();
            if identities.is_empty() {
                break;
            }
            let keys = identities
                .iter()
                .map(|(message, _)| message.as_bytes().to_vec())
                .collect::<Vec<_>>();
            let records = identities
                .iter()
                .map(|(_, uuid)| uuid.as_bytes().to_vec())
                .collect::<Vec<_>>();
            tx.check()?;
            #[cfg(test)]
            let started = Instant::now();
            #[cfg(test)]
            update_usage_diagnostic(|d| {
                d.identity_insert.rows_attempted += identities.len();
                d.identity_insert.sql_attempted += 1;
            });
            let count = tx.driver.execute("INSERT INTO semon_pg_native_usage_identity(namespace,file_id,message_id,record_uuid) SELECT $1,$2,x.message,x.record FROM unnest($3::bytea[],$4::bytea[]) AS x(message,record)", &[&ns,&id,&keys,&records]).map_err(sqlstate::error)?;
            if count != identities.len() as u64 {
                return Err(StorageError::Conflict);
            }
            for _ in &identities {
                after(tx)?;
            }
            #[cfg(test)]
            update_usage_diagnostic(|d| {
                d.identity_insert.rows_completed += identities.len();
                d.identity_insert.sql_completed += 1;
                d.identity_insert.elapsed_us += started.elapsed().as_micros();
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
