//! Private read-only Tree family. Public backend selection remains closed.
use super::{PgCancellation, PgPool, sqlstate, txn::{PgTxn, TxnKind}};
use crate::{events::{Ledger, CACHE_VERSION, store::{header_ledger, tree_state::{SharedUsage, TreeCheckpoint}}}, native_tree::Summary, selected_read, storage::{LogicalSourceNamespace, StorageError, StoreConfig}, Tokens};
use std::{collections::BTreeMap, fmt, io};

#[derive(Clone)]
pub(crate) struct Snapshot {
    pool: PgPool,
    namespace: LogicalSourceNamespace,
    path: String,
    ledger: Ledger,
    file: i64,
    projection: i64,
    source: i64,
    control: PgCancellation,
    poisoned: bool,
}
impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PgTreeSnapshot") }
}
fn io_error(error: StorageError) -> io::Error {
    match error {
        StorageError::Busy => io::ErrorKind::WouldBlock.into(),
        _ => io::Error::other(error),
    }
}
fn budget(error: io::Error) -> StorageError {
    let _ = error;
    StorageError::Busy
}

impl Snapshot {
    // Deliberately private until the complete IndexStore owns capture.
    #[allow(dead_code)]
    pub(crate) fn restore(pool: PgPool, namespace: LogicalSourceNamespace, path: &str, ledger: &Ledger, control: PgCancellation) -> io::Result<Summary> {
        let _scope = selected_read::Scope::ensure();
        selected_read::check()?;
        let mut snapshot = Self { pool, namespace, path: path.into(), ledger: ledger.clone(), file: 0, projection: 0, source: 0, control, poisoned: false };
        let (file, projection, source, mut summary) = snapshot.pool.transaction(&snapshot.namespace, TxnKind::ReadSnapshot, &snapshot.control, |tx| {
            let (file, projection, source) = snapshot.authority(tx, false)?;
            let mut summary = Summary::default();
            rows(tx, "tree_labels", "projection_id", projection, &["label", "value"], 1, |values| {
                let key = required(&values, 0)?;
                let value = required(&values, 1)?;
                match key.as_str() {
                    "first" => summary.first = Some(value),
                    "last" => summary.last = Some(value),
                    "cwd" => summary.cwd = Some(value),
                    "branch" => summary.branch = Some(value),
                    "codex_turn" => summary.codex_turn = serde_json::from_str(&value).map_err(|_| StorageError::Unavailable)?,
                    _ => return Err(StorageError::Unavailable),
                }
                Ok(())
            })?;
            rows(tx, "tree_models", "projection_id", projection, &["model"], 1, |values| { summary.models.insert(required(&values, 0)?); Ok(()) })?;
            rows(tx, "tree_tools", "projection_id", projection, &["tool_id", "name", "convert_to(closed::text,'UTF8')"], 1, |values| {
                let id = required(&values, 0)?;
                match required(&values, 2)?.as_str() {
                    "false" => if let Some(name) = &values[1] { summary.tools.insert(id, name.clone()); },
                    "true" => (),
                    _ => return Err(StorageError::Unavailable),
                }
                Ok(())
            })?;
            Ok((file, projection, source, summary))
        }).map_err(io_error)?;
        snapshot.file = file; snapshot.projection = projection; snapshot.source = source;
        summary.stored = Some(TreeCheckpoint::Pg(snapshot));
        Ok(summary)
    }

    fn authority(&self, tx: &mut PgTxn<'_>, bound: bool) -> Result<(i64, i64, i64), StorageError> {
        tx.check()?;
        selected_read::checkpoint("pg_tree_authority").map_err(budget)?;
        let ledger = &self.ledger;
        let receipt = header_ledger(ledger).into_bytes();
        // Compare variable metadata server-side: only three fixed bigint values
        // are allocated by the driver. No uncharged Ledger/JSON allocation.
        let result = tx.driver.query("SELECT h.file_id,h.projection_id,g.source_id FROM semon_pg_native_tree_headers h JOIN semon_pg_native_files f ON f.namespace=h.namespace AND f.file_id=h.file_id AND f.path=h.source_path JOIN semon_pg_native_tree_generations g ON g.namespace=h.namespace AND g.projection_id=h.projection_id JOIN semon_pg_native_usage_current c ON c.namespace=h.namespace AND c.source_path=h.source_path AND c.source_id=g.source_id JOIN semon_pg_native_usage_sources s ON s.namespace=c.namespace AND s.source_id=c.source_id AND s.source_path=c.source_path WHERE h.namespace=$1 AND h.source_path=$2 AND h.ledger=$3 AND g.receipt=$3 AND g.version=3 AND g.complete AND NOT g.work_pending AND s.receipt=$3 AND s.parser_version=$4 AND f.dev=$6::text::numeric AND f.ino=$7::text::numeric AND f.size=$8::text::numeric AND f.resume_at=$5::text::numeric AND f.mtime_ns=$9 AND f.ctime_ns IS NOT DISTINCT FROM $10 AND f.head_sha256=$11 AND f.tail_sha256=$12 LIMIT 1", &[&tx.namespace.as_str().as_bytes(), &self.path.as_bytes(), &receipt, &i64::from(CACHE_VERSION), &ledger.offset.to_string(), &ledger.stat.dev.to_string(), &ledger.stat.ino.to_string(), &ledger.stat.size.to_string(), &ledger.stat.modified_ns.to_be_bytes().as_slice(), &ledger.stat.changed_ns.map(|v| v.to_be_bytes().to_vec()), &ledger.prefix.as_slice(), &ledger.tail.as_slice()]).map_err(sqlstate::error)?;
        selected_read::charge(64, 0).map_err(budget)?;
        let row = result.first().ok_or(StorageError::Busy)?;
        let actual = (row.get(0), row.get(1), row.get(2));
        if bound && actual != (self.file, self.projection, self.source) { return Err(StorageError::Busy); }
        Ok(actual)
    }

    fn with<T>(&self, read: impl FnOnce(&mut PgTxn<'_>) -> Result<T, StorageError>) -> io::Result<T> {
        let _scope = selected_read::Scope::ensure();
        selected_read::check()?;
        if self.poisoned { return Err(io::ErrorKind::WouldBlock.into()); }
        self.pool.transaction(&self.namespace, TxnKind::ReadSnapshot, &self.control, |tx| {
            self.authority(tx, true)?;
            read(tx)
        }).map_err(io_error)
    }
    pub(crate) fn bind_configured(&mut self, _config: StoreConfig) {
        // StoreConfig currently only selects SQLite. It cannot rebind PG.
        self.poisoned = true;
    }
    pub(crate) fn tool_name(&self, id: &str) -> io::Result<Option<String>> {
        self.with(|tx| {
            tx.check()?;
            let lengths = tx.driver.query("SELECT coalesce(octet_length(name),0)::bigint FROM semon_pg_native_tree_tools WHERE namespace=$1 AND projection_id=$2 AND tool_id=$3 LIMIT 1", &[&tx.namespace.as_str().as_bytes(), &self.projection, &id.as_bytes()]).map_err(sqlstate::error)?;
            let Some(length) = lengths.first() else { return Ok(None); };
            selected_read::charge(64, usize::try_from(length.get::<_, i64>(0)).map_err(|_| StorageError::Unavailable)?).map_err(budget)?;
            let row = tx.driver.query_one("SELECT name FROM semon_pg_native_tree_tools WHERE namespace=$1 AND projection_id=$2 AND tool_id=$3", &[&tx.namespace.as_str().as_bytes(), &self.projection, &id.as_bytes()]).map_err(sqlstate::error)?;
            row.get::<_, Option<Vec<u8>>>(0).map(text).transpose()
        })
    }
    pub(crate) fn shared_usage(&self) -> io::Result<SharedUsage> {
        self.with(|tx| {
            let mut records = crate::claude_usage::Records::new();
            rows(tx, "tree_copied_identities", "file_id", self.file, &["message_id", "record_uuid"], 2, |values| {
                records.entry(required(&values, 0)?).or_default().insert(required(&values, 1)?); Ok(())
            })?;
            let mut tokens = BTreeMap::new();
            let mut models = BTreeMap::new();
            for message in records.keys() {
                selected_read::checkpoint("pg_tree_usage").map_err(budget)?;
                // Exact key lookup; decimal unsigned values are at most 20 bytes
                // after server-side range/integrality validation. Charge before decode.
                selected_read::charge(128, 100).map_err(budget)?;
                let columns = ["input", "cached_input", "output", "reasoning_output", "total"].map(|c| format!("CASE WHEN {c} BETWEEN 0 AND 18446744073709551615 AND trunc({c})={c} THEN ({c}::numeric(20,0))::text ELSE NULL END")).join(",");
                let row = tx.driver.query_one(&format!("SELECT {columns} FROM semon_pg_native_usage WHERE namespace=$1 AND file_id=$2 AND message_id=$3"), &[&tx.namespace.as_str().as_bytes(), &self.file, &message.as_bytes()]).map_err(sqlstate::error)?;
                let uint = |i| row.get::<_, Option<String>>(i).ok_or(StorageError::Unavailable)?.parse::<u64>().map_err(|_| StorageError::Unavailable);
                tokens.insert(message.clone(), Tokens { input: uint(0)?, cached_input: uint(1)?, output: uint(2)?, reasoning_output: uint(3)?, total: uint(4)? });
                let lengths = tx.driver.query("SELECT octet_length(model)::bigint FROM semon_pg_native_tree_usage_models WHERE namespace=$1 AND projection_id=$2 AND message_id=$3 LIMIT 1", &[&tx.namespace.as_str().as_bytes(), &self.projection, &message.as_bytes()]).map_err(sqlstate::error)?;
                if let Some(length) = lengths.first() {
                    selected_read::charge(64, usize::try_from(length.get::<_, i64>(0)).map_err(|_| StorageError::Unavailable)?).map_err(budget)?;
                    let row = tx.driver.query_one("SELECT model FROM semon_pg_native_tree_usage_models WHERE namespace=$1 AND projection_id=$2 AND message_id=$3", &[&tx.namespace.as_str().as_bytes(), &self.projection, &message.as_bytes()]).map_err(sqlstate::error)?;
                    models.insert(message.clone(), text(row.get(0))?);
                }
            }
            Ok((records, tokens, models))
        })
    }
}
fn text(bytes: Vec<u8>) -> Result<String, StorageError> { String::from_utf8(bytes).map_err(|_| StorageError::Unavailable) }
fn required(values: &[Option<String>], at: usize) -> Result<String, StorageError> { values[at].clone().ok_or(StorageError::Unavailable) }

// Adapter-local keyset reader: one fixed-size lengths row first, charge its
// dynamic bytes eightfold using the existing budget, then fetch exactly one row
// from the SAME repeatable-read snapshot. Driver allocation precedes decoding
// but follows admission; no complete family Vec<Row> is constructed.
fn rows(tx: &mut PgTxn<'_>, family: &str, owner: &str, id: i64, columns: &[&str], keys: usize, mut consume: impl FnMut(Vec<Option<String>>) -> Result<(), StorageError>) -> Result<(), StorageError> {
    let key = format!("ARRAY[{}]", columns[..keys].join(","));
    let predicate = format!("FROM semon_pg_native_{family} WHERE namespace=$1 AND {owner}=$2 AND ($3::bytea[] IS NULL OR {key}>$3) ORDER BY {key} LIMIT 1");
    let sizes = columns.iter().map(|col| format!("coalesce(octet_length({col}),0)::bigint")).collect::<Vec<_>>().join(",");
    let mut cursor: Option<Vec<Vec<u8>>> = None;
    loop {
        tx.check()?;
        selected_read::checkpoint("pg_tree_row").map_err(budget)?;
        let lengths = tx.driver.query(&format!("SELECT {sizes} {predicate}"), &[&tx.namespace.as_str().as_bytes(), &id, &cursor]).map_err(sqlstate::error)?;
        let Some(lengths) = lengths.first() else { break; };
        let dynamic = (0..columns.len()).try_fold(0usize, |sum, i| usize::try_from(lengths.get::<_, i64>(i)).map(|n| sum.saturating_add(n)).map_err(|_| StorageError::Unavailable))?;
        selected_read::charge(128, dynamic).map_err(budget)?;
        tx.check()?;
        let row = tx.driver.query_one(&format!("SELECT {} {predicate}", columns.join(",")), &[&tx.namespace.as_str().as_bytes(), &id, &cursor]).map_err(sqlstate::error)?;
        cursor = Some((0..keys).map(|i| row.get(i)).collect());
        let values = (0..columns.len()).map(|i| row.get::<_, Option<Vec<u8>>>(i).map(text).transpose()).collect::<Result<_, _>>()?;
        consume(values)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
