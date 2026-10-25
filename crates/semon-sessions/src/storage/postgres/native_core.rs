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
    tx.driver
        .execute(
            &format!(
                "DELETE FROM semon_pg_native_{table} WHERE namespace=$1 AND file_id=$2 {suffix}"
            ),
            &values,
        )
        .map_err(sqlstate::error)?;
    after(tx)
}
fn ordinal(value: usize) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::InvalidArguments)
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
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.execute("INSERT INTO semon_pg_source_state(namespace) VALUES($1) ON CONFLICT(namespace_digest) DO NOTHING", &[&ns]).map_err(sqlstate::error)?;
    let locked = tx.driver.query("SELECT namespace FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if locked.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let current = tx
        .driver
        .query(
            "SELECT file_id FROM semon_pg_native_files WHERE namespace=$1 AND path=$2",
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
            let valid = tx.driver.query("SELECT file_id FROM semon_pg_native_files WHERE namespace=$1 AND path=$2 AND dev=$3::text::numeric AND ino=$4::text::numeric AND size=$5::text::numeric AND mtime_ns=$6 AND ctime_ns IS NOT DISTINCT FROM $7::bytea AND resume_at=$8::text::numeric AND head_sha256=$9 AND tail_sha256=$10", &[&ns,&path.as_bytes(),&old.stat.dev.to_string(),&old.stat.ino.to_string(),&old.stat.size.to_string(),&old.stat.modified_ns.to_be_bytes().as_slice(),&old.stat.changed_ns.map(|v|v.to_be_bytes().to_vec()),&old.offset.to_string(),&old.prefix.as_slice(),&old.tail.as_slice()]).map_err(sqlstate::error)?;
            if valid.len() != 1 {
                return Err(StorageError::Conflict);
            }
            current.get(0)
        }
        _ => return Err(StorageError::Conflict),
    };
    after(tx)?;
    if changes.is_none() && !current.is_empty() {
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
    for (message, usage) in UsageChanges::new(changes, index.usage()).iter() {
        delete(
            tx,
            "usage_identity",
            id,
            "AND message_id=$3",
            &[&message.as_bytes()],
        )?;
        if let Some(usage) = usage {
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
            r.put(tx, "usage", "file_id,message_id")?;
            for uuid in &usage.record_ids {
                tx.check()?;
                tx.driver.execute("INSERT INTO semon_pg_native_usage_identity(namespace,file_id,message_id,record_uuid) VALUES($1,$2,$3,$4)", &[&ns,&id,&message.as_bytes(),&uuid.as_bytes()]).map_err(sqlstate::error)?;
                after(tx)?;
            }
        } else {
            delete(tx, "usage", id, "AND message_id=$3", &[&message.as_bytes()])?;
        }
    }
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

#[cfg(test)]
mod tests;
