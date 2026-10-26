//! Indexed Tree dimensions. Only models, currently open tools and copied usage
//! requested by the selected owner cohort are materialized by ordinary reads.
use super::*;
use crate::native_tree::Summary;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_tree_generations (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,source_id INTEGER NOT NULL,
 version INTEGER NOT NULL,receipt TEXT NOT NULL,complete INTEGER NOT NULL,work_pending INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace,projection_id),
 FOREIGN KEY(namespace,source_id) REFERENCES native_usage_sources(namespace,source_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_tree_cleanup_queue ON native_tree_generations(namespace,complete,work_pending,projection_id);
CREATE TABLE IF NOT EXISTS native_tree_usage_work (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,message_id TEXT NOT NULL,tokens TEXT NOT NULL,
 PRIMARY KEY(namespace,projection_id,message_id),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_codex_work (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,identity TEXT NOT NULL,
 PRIMARY KEY(namespace,projection_id,identity),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_headers (
 namespace TEXT NOT NULL,source_path TEXT NOT NULL,file_id INTEGER NOT NULL,projection_id INTEGER NOT NULL,ledger TEXT NOT NULL,
 PRIMARY KEY(namespace,source_path),
 FOREIGN KEY(namespace,file_id) REFERENCES files(namespace,file_id),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_models (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,model TEXT NOT NULL,
 model_digest BLOB GENERATED ALWAYS AS (semon_tree_digest('tree.model',model)) STORED,
 PRIMARY KEY(namespace,projection_id,model),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS native_tree_model_addresses ON native_tree_models(namespace,projection_id,model_digest);
CREATE TABLE IF NOT EXISTS native_tree_labels (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,label TEXT NOT NULL,value TEXT NOT NULL,
 PRIMARY KEY(namespace,projection_id,label),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_tools (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,tool_id TEXT NOT NULL,name TEXT,closed INTEGER NOT NULL,
 tool_id_digest BLOB GENERATED ALWAYS AS (semon_tree_digest('tree.tool',tool_id)) STORED,
 PRIMARY KEY(namespace,projection_id,tool_id),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS native_tree_tool_addresses ON native_tree_tools(namespace,projection_id,tool_id_digest);
CREATE INDEX IF NOT EXISTS native_tree_open_tools ON native_tree_tools(namespace,projection_id,tool_id_digest) WHERE closed=0 AND name IS NOT NULL;
CREATE TABLE IF NOT EXISTS native_tree_usage_models (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,message_id TEXT NOT NULL,model TEXT NOT NULL,
 PRIMARY KEY(namespace,projection_id,message_id),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_copied_identities (
 namespace TEXT NOT NULL,file_id INTEGER NOT NULL,message_id TEXT NOT NULL,record_uuid TEXT NOT NULL,
 PRIMARY KEY(namespace,file_id,message_id,record_uuid),
 FOREIGN KEY(namespace,file_id) REFERENCES files(namespace,file_id)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_tree_copy_insert AFTER INSERT ON usage_identity BEGIN
 INSERT INTO native_tree_copied_identities
 SELECT namespace,file_id,message_id,record_uuid FROM usage_identity
 WHERE namespace=NEW.namespace AND message_id=NEW.message_id AND record_uuid=NEW.record_uuid
 AND EXISTS(SELECT 1 FROM usage_identity p WHERE p.namespace=NEW.namespace AND p.message_id=NEW.message_id AND p.record_uuid=NEW.record_uuid AND p.file_id<>NEW.file_id)
 ON CONFLICT DO NOTHING;
END;
CREATE TRIGGER IF NOT EXISTS native_tree_copy_delete AFTER DELETE ON usage_identity BEGIN
 DELETE FROM native_tree_copied_identities WHERE namespace=OLD.namespace AND file_id=OLD.file_id AND message_id=OLD.message_id AND record_uuid=OLD.record_uuid;
 DELETE FROM native_tree_copied_identities WHERE (namespace,file_id,message_id,record_uuid) IN
 (SELECT i.namespace,i.file_id,i.message_id,i.record_uuid FROM usage_identity i WHERE i.namespace=OLD.namespace AND i.message_id=OLD.message_id AND i.record_uuid=OLD.record_uuid
 AND NOT EXISTS(SELECT 1 FROM usage_identity p WHERE p.namespace=i.namespace AND p.message_id=i.message_id AND p.record_uuid=i.record_uuid AND p.file_id<>i.file_id));
END;
";

pub(super) fn schema() -> &'static str {
    SCHEMA
}

// Static test diagnostics at the native-tree boundary; never include SQL values.
pub(super) fn diagnose(statement: &'static str, error: rusqlite::Error) -> rusqlite::Error {
    #[cfg(test)]
    if let rusqlite::Error::SqliteFailure(code, message) = &error {
        let message = message.as_deref().unwrap_or("");
        let reason = if message.starts_with("Invalid function parameter type Null") {
            "function_argument_null"
        } else if message.contains("unable to delete/modify user-function") {
            "function_rebind_active"
        } else if message.contains("user-defined function raised exception") {
            "function_callback"
        } else if message.contains("unsafe use") {
            "function_unsafe"
        } else if message.contains("ON CONFLICT clause does not match") {
            "conflict_target"
        } else if message.contains("view") {
            "view_effect"
        } else if message.starts_with("no such column:") {
            "missing_column"
        } else if message.starts_with("no such table:") {
            "missing_table"
        } else if message.contains("values were supplied") {
            "column_count"
        } else if message.contains("readonly") || message.contains("read-only") {
            "readonly"
        } else if message.contains("FOREIGN KEY") || message.contains("foreign key") {
            "foreign_key"
        } else if message.contains("constraint") {
            "constraint"
        } else if message.contains("datatype") {
            "datatype"
        } else if message.contains("arguments on") {
            "function_arity"
        } else if message.contains("malformed") {
            "malformed"
        } else {
            "other_sql_contract"
        };
        eprintln!(
            "native tree statement={statement} code={} reason={reason}",
            code.extended_code
        );
    }
    #[cfg(not(test))]
    let _ = statement;
    error
}

pub(crate) fn register(c: &Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    c.create_scalar_function(
        "semon_tree_digest",
        2,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |context| {
            let domain: String = context.get(0)?;
            // Generated columns can evaluate a transient NULL during updates.
            // The authored table still requires a non-null stored key.
            let key: Option<String> = context.get(1)?;
            Ok(key.map(|key| {
                crate::events::tree_rows::witness::address(&domain, key.as_bytes()).to_vec()
            }))
        },
    )
}

#[derive(Clone)]
pub(crate) struct Snapshot {
    db: PathBuf,
    path: String,
    ledger: Ledger,
    file_id: i64,
    projection_id: i64,
    configured: Option<crate::storage::StoreConfig>,
}

/// The backend which owns the Tree checkpoint used for deferred tree facts.
///
/// A `Summary` is returned through the shared producer path.  Keeping its
/// reader typed here makes the physical SQLite dependency explicit rather than
/// allowing a future non-SQLite `FileIndex` to reopen this database.
#[derive(Clone, Debug)]
pub(crate) enum TreeCheckpoint {
    Sqlite(Snapshot),
    #[cfg(feature = "postgres")]
    Pg(crate::storage::postgres::tree_checkpoint::Snapshot),
}

impl TreeCheckpoint {
    pub(crate) fn tool_name(&self, id: &str) -> io::Result<Option<String>> {
        match self {
            Self::Sqlite(snapshot) => snapshot.tool_name(id),
            #[cfg(feature = "postgres")]
            Self::Pg(snapshot) => snapshot.tool_name(id),
        }
    }

    pub(crate) fn shared_usage(&self) -> io::Result<SharedUsage> {
        match self {
            Self::Sqlite(snapshot) => snapshot.shared_usage(),
            #[cfg(feature = "postgres")]
            Self::Pg(snapshot) => snapshot.shared_usage(),
        }
    }

    pub(crate) fn bind_configured(&mut self, config: crate::storage::StoreConfig) {
        match self {
            Self::Sqlite(snapshot) => snapshot.bind_configured(config),
            #[cfg(feature = "postgres")]
            Self::Pg(snapshot) => snapshot.bind_configured(config),
        }
    }
}
impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TreeSnapshot")
    }
}
pub(crate) type SharedUsage = (
    crate::claude_usage::Records,
    BTreeMap<String, Tokens>,
    BTreeMap<String, String>,
);
impl Snapshot {
    pub(super) fn bind_configured(&mut self, config: crate::storage::StoreConfig) {
        self.configured = Some(config);
    }

    fn with<T>(
        &self,
        read: impl FnOnce(&Connection, i64, i64) -> rusqlite::Result<T>,
    ) -> io::Result<T> {
        let c = match &self.configured {
            Some(config) => configured::connection(config).map_err(io::Error::other)?,
            None => Connection::open_with_flags(&self.db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(io::Error::other)?,
        };
        let tx = c.unchecked_transaction().map_err(io::Error::other)?;
        let admitted = match &self.configured {
            Some(config) => {
                configured::validate(&tx, config).map_err(io::Error::other)?;
                true
            }
            None => current(&tx).map_err(io::Error::other)?,
        };
        if !admitted
            || ledger_row(&tx, &self.path)
                .map_err(io::Error::other)?
                .map(|(_, ledger)| ledger)
                .as_ref()
                != Some(&self.ledger)
        {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let actual: (i64, String, i64) = tx
            .query_row(
                "SELECT file_id,ledger,projection_id FROM native_tree_headers WHERE namespace=semon_namespace() AND source_path=?1",
                [&self.path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(io::Error::other)?;
        if actual.0 != self.file_id
            || actual.1 != header_ledger(&self.ledger)
            || actual.2 != self.projection_id
        {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        read(&tx, self.file_id, self.projection_id).map_err(io::Error::other)
    }
    pub(crate) fn tool_name(&self, id: &str) -> io::Result<Option<String>> {
        self.with(|c, _, projection_id| {
            c.query_row(
                "SELECT name FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 AND tool_id=?2",
                params![projection_id, id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
        })
    }
    pub(crate) fn shared_usage(&self) -> io::Result<SharedUsage> {
        self.with(shared_usage)
    }
}

pub(super) fn shared_usage(
    c: &Connection,
    id: i64,
    projection_id: i64,
) -> rusqlite::Result<SharedUsage> {
    let query = "SELECT i.message_id,i.record_uuid FROM native_tree_copied_identities i WHERE i.namespace=semon_namespace() AND i.file_id=?1 ORDER BY i.message_id,i.record_uuid";
    let mut statement = c.prepare(query)?;
    let mut rows = statement.query([id])?;
    let mut records = crate::claude_usage::Records::new();
    while let Some(row) = rows.next()? {
        crate::selected_read::row(row, 64)?;
        records.entry(row.get(0)?).or_default().insert(row.get(1)?);
    }
    let mut tokens = BTreeMap::new();
    let mut models = BTreeMap::new();
    for message in records.keys() {
        let used = c.query_row("SELECT input,cached_input,output,reasoning_output,total FROM usage WHERE namespace=semon_namespace() AND file_id=?1 AND message_id=?2",params![id,message],|r|Ok(Tokens {input:uint(r.get(0)?),cached_input:uint(r.get(1)?),output:uint(r.get(2)?),reasoning_output:uint(r.get(3)?),total:uint(r.get(4)?)}))?;
        tokens.insert(message.clone(), used);
        if let Some(model) = c.query_row("SELECT model FROM native_tree_usage_models WHERE namespace=semon_namespace() AND projection_id=?1 AND message_id=?2",params![projection_id,message],|r|r.get::<_,String>(0)).optional()? {
            models.insert(message.clone(), model);
        }
    }
    Ok((records, tokens, models))
}

pub(super) fn write(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    summary: &Summary,
) -> rusqlite::Result<()> {
    let id: i64 = tx.query_row(
        "SELECT file_id FROM files WHERE namespace=semon_namespace() AND path=?1",
        [path],
        |r| r.get(0),
    )?;
    let source_id = native_usage::source(tx, path)?
        .ok_or(rusqlite::Error::InvalidQuery)?
        .0;
    let previous:Option<i64>=tx.query_row("SELECT g.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(namespace,projection_id) WHERE h.namespace=semon_namespace() AND h.source_path=?1 AND g.source_id=?2 AND g.version=3",params![path,source_id],|r|r.get(0)).optional()?;
    let projection_id = match previous {
        Some(id) => id,
        None => new_generation(tx, source_id, ledger)?,
    };
    tx.execute("INSERT INTO native_tree_headers(namespace,source_path,file_id,projection_id,ledger) VALUES(semon_namespace(),?1,?2,?3,?4) ON CONFLICT(namespace,source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger",params![path,id,projection_id,header_ledger(ledger)])?;
    write_values(tx, projection_id, summary).map_err(|error| diagnose("write_values", error))?;
    tx.execute(
        "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE namespace=semon_namespace() AND projection_id=?1",
        params![projection_id, header_ledger(ledger)],
    )?;
    Ok(())
}

pub(super) fn new_generation(
    tx: &Transaction<'_>,
    source_id: i64,
    ledger: &Ledger,
) -> rusqlite::Result<i64> {
    tx.query_row("INSERT INTO native_tree_generations(namespace,projection_id,source_id,version,receipt,complete) VALUES(semon_namespace(),(SELECT coalesce(max(projection_id),0)+1 FROM native_tree_generations WHERE namespace=semon_namespace()),?1,3,?2,0) RETURNING projection_id", params![source_id, header_ledger(ledger)], |r| r.get(0))
}

pub(super) fn bind_generation(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    projection_id: i64,
) -> rusqlite::Result<()> {
    let id: i64 = tx.query_row(
        "SELECT file_id FROM files WHERE namespace=semon_namespace() AND path=?1",
        [path],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE namespace=semon_namespace() AND projection_id=?1",
        params![projection_id, header_ledger(ledger)],
    )?;
    tx.execute("INSERT INTO native_tree_headers(namespace,source_path,file_id,projection_id,ledger) VALUES(semon_namespace(),?1,?2,?3,?4) ON CONFLICT(namespace,source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger",params![path,id,projection_id,header_ledger(ledger)])?;
    Ok(())
}

pub(super) fn write_values(
    tx: &Transaction<'_>,
    id: i64,
    summary: &Summary,
) -> rusqlite::Result<()> {
    let rows = crate::events::tree_rows::TreeRows::new(summary)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let turn = &rows.codex_turn;
    tx.prepare_cached("INSERT INTO native_tree_labels VALUES(semon_namespace(),?1,'codex_turn',?2) ON CONFLICT(namespace,projection_id,label) DO UPDATE SET value=excluded.value")?.execute(params![id,turn])?;
    for (label, value) in rows.labels() {
        tx.prepare_cached("INSERT INTO native_tree_labels VALUES(semon_namespace(),?1,?2,?3) ON CONFLICT(namespace,projection_id,label) DO UPDATE SET value=excluded.value")?.execute(params![id,label,value])?;
    }
    for model in rows.models {
        tx.prepare_cached("INSERT INTO native_tree_models VALUES(semon_namespace(),?1,?2) ON CONFLICT DO NOTHING")?
        .execute(params![id, model])
        .map_err(|error| diagnose("model_insert", error))?;
    }
    for (tool, name) in rows.tools {
        tx.prepare_cached("INSERT INTO native_tree_tools VALUES(semon_namespace(),?1,?2,?3,0) ON CONFLICT(namespace,projection_id,tool_id) DO UPDATE SET name=excluded.name")?.execute(params![id,tool,name]).map_err(|error| diagnose("tool_open_upsert", error))?;
    }
    for tool in rows.closed_tools {
        tx.prepare_cached("INSERT INTO native_tree_tools VALUES(semon_namespace(),?1,?2,NULL,1) ON CONFLICT(namespace,projection_id,tool_id) DO UPDATE SET closed=1")?.execute(params![id,tool]).map_err(|error| diagnose("tool_closed_upsert", error))?;
    }
    for (message, model) in rows.usage_models {
        tx.prepare_cached("INSERT INTO native_tree_usage_models VALUES(semon_namespace(),?1,?2,?3) ON CONFLICT(namespace,projection_id,message_id) DO UPDATE SET model=excluded.model")?.execute(params![id,message,model])?;
    }
    Ok(())
}

pub(super) fn restore_labels(
    c: &Connection,
    id: i64,
    summary: &mut Summary,
) -> rusqlite::Result<()> {
    let mut statement = c.prepare(
        "SELECT label,value FROM native_tree_labels WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY label",
    )?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        crate::selected_read::row(row, 64)?;
        let value = Some(row.get(1)?);
        match row.get::<_, String>(0)?.as_str() {
            "first" => summary.first = value,
            "last" => summary.last = value,
            "cwd" => summary.cwd = value,
            "branch" => summary.branch = value,
            "codex_turn" => {
                summary.codex_turn =
                    serde_json::from_str(value.as_deref().unwrap_or("")).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
            }
            _ => return Err(rusqlite::Error::InvalidQuery),
        }
    }
    Ok(())
}

pub(super) fn restore(
    c: &Connection,
    path: &str,
    ledger: &Ledger,
    summary: &mut Summary,
) -> rusqlite::Result<bool> {
    let header: Option<(i64, String, i64)> = c
        .query_row(
            "SELECT h.file_id,h.ledger,h.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(namespace,projection_id) JOIN native_usage_current c ON c.namespace=h.namespace AND c.source_path=h.source_path AND c.source_id=g.source_id WHERE h.namespace=semon_namespace() AND h.source_path=?1 AND g.complete=1 AND g.version=3 AND g.receipt=h.ledger",
            [path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, receipt, projection_id)) = header else {
        return Ok(false);
    };
    if receipt != header_ledger(ledger) {
        return Ok(false);
    }
    restore_labels(c, projection_id, summary)?;
    let mut models =
        c.prepare("SELECT model FROM native_tree_models WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY model")?;
    summary.models = models
        .query_map([projection_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut tools = c.prepare("SELECT tool_id,name FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id")?;
    summary.tools = tools
        .query_map([projection_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let db: String = c.query_row(
        "SELECT file FROM pragma_database_list WHERE name='main'",
        [],
        |r| r.get(0),
    )?;
    if !db.is_empty() {
        summary.stored = Some(TreeCheckpoint::Sqlite(Snapshot {
            db: db.into(),
            path: path.into(),
            ledger: ledger.clone(),
            file_id: id,
            projection_id,
            configured: None,
        }));
    }
    Ok(true)
}

pub(super) fn ready(c: &Connection, path: &str, ledger: &Ledger) -> rusqlite::Result<bool> {
    // Repair must use the same validity predicate as its consumer. A version-3
    // JSON root can still carry stale dimensions or interpretation dependencies;
    // marking that root done would make every subsequent load queue it again.
    let mut statement = c.prepare(native_metadata_value(c)?)?;
    Ok(read_tree_summary(c, &mut statement, path, ledger)?.is_some())
}

/// Fingerprint the current Tree dimensions, including their identity binding.
/// The compact summary deliberately omits these dimensions. A stored digest
/// lets a later completion distinguish an unchanged projection from a missing
/// or damaged witness without trusting the source ledger alone.
pub(super) fn witness(
    c: &Connection,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<String>> {
    let Some((file_id, actual)) = ledger_row(c, path)? else {
        return Ok(None);
    };
    if actual != *ledger {
        return Ok(None);
    }
    let projection: Option<i64> = c.query_row(
        "SELECT h.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(namespace,projection_id) JOIN native_usage_current u ON u.namespace=h.namespace AND u.source_path=h.source_path AND u.source_id=g.source_id JOIN native_usage_sources s ON s.namespace=g.namespace AND s.source_id=g.source_id WHERE h.namespace=semon_namespace() AND h.source_path=?1 AND h.file_id=?2 AND h.ledger=?3 AND g.receipt=h.ledger AND g.complete=1 AND g.version=3 AND s.parser_version=?4",
        params![path, file_id, header_ledger(ledger), CACHE_VERSION],
        |r| r.get(0),
    ).optional()?;
    let Some(projection) = projection else {
        return Ok(None);
    };
    // The stored digest indexes stream the current authenticated order. `row`
    // admits each encoded cell before allocation; no dimension is collected.
    let mut writer = crate::events::tree_rows::witness::Writer::new();
    for (family, scoped) in [
        "SELECT json_array(label,value) FROM native_tree_labels WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY label",
        "SELECT json_array(model) FROM native_tree_models WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY model_digest",
        "SELECT json_array(tool_id,name,closed) FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY tool_id_digest",
    ]
    .into_iter()
    .enumerate()
    {
        match family {
            0 => writer.labels(),
            1 => writer.models(),
            _ => writer.tools(),
        }
        let mut statement = c.prepare(scoped)?;
        let mut rows = statement.query([projection])?;
        while let Some(row) = rows.next()? {
            crate::selected_read::row(row, 64)?;
            crate::selected_read::checkpoint("tree_witness")
                .map_err(crate::selected_read::sql_error)?;
            let value: String = row.get(0)?;
            writer.encoded(&value);
        }
    }
    Ok(Some(writer.finish()))
}

pub(super) fn work_pending(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_tree_generations WHERE namespace=semon_namespace() AND complete=1 AND work_pending=1)",
        [],
        |r| r.get(0),
    )
}

pub(super) fn cleanup_work(c: &mut Connection) -> rusqlite::Result<bool> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !current(&tx)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let pending = cleanup_work_tx(&tx)?;
    tx.commit()?;
    Ok(pending)
}

// The caller validates its writer authority in this same transaction.
pub(super) fn cleanup_work_tx(tx: &Transaction<'_>) -> rusqlite::Result<bool> {
    let selected:Option<i64>=tx.query_row("SELECT projection_id FROM native_tree_generations WHERE namespace=semon_namespace() AND complete=1 AND work_pending=1 ORDER BY projection_id LIMIT 1",[],|r|r.get(0)).optional()?;
    if let Some(id) = selected {
        for (table, key) in [
            ("native_tree_usage_work", "message_id"),
            ("native_tree_codex_work", "identity"),
        ] {
            let predicate = "namespace=semon_namespace() AND ";
            tx.execute(&format!("DELETE FROM {table} WHERE {predicate}projection_id=?1 AND {key} IN (SELECT {key} FROM {table} WHERE {predicate}projection_id=?1 ORDER BY {key} LIMIT 512)"),[id])?;
        }
        tx.execute("UPDATE native_tree_generations SET work_pending=0 WHERE namespace=semon_namespace() AND projection_id=?1 AND NOT EXISTS(SELECT 1 FROM native_tree_usage_work WHERE namespace=semon_namespace() AND projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_codex_work WHERE namespace=semon_namespace() AND projection_id=?1)",[id])?;
    }
    work_pending(tx)
}

#[cfg(test)]
mod namespace_tests {
    use super::*;
    use rusqlite::functions::FunctionFlags;
    use std::collections::{BTreeMap, BTreeSet};

    fn bind(db: &Connection, namespace: &str) {
        configured::bind_metadata_scope(db, namespace).unwrap();
        let namespace = namespace.to_owned();
        db.create_scalar_function(
            "semon_namespace",
            0,
            FunctionFlags::SQLITE_UTF8,
            move |_| Ok(namespace.clone()),
        )
        .unwrap();
    }

    #[test]
    fn native_usage_tree_colliding_generations_restore_witness_and_rollback() {
        let mut db = Connection::open_in_memory().unwrap();
        register(&db).unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=ON; CREATE TABLE files (
            namespace TEXT NOT NULL,file_id INTEGER NOT NULL,path TEXT NOT NULL,
            dev INTEGER NOT NULL,ino INTEGER NOT NULL,size INTEGER NOT NULL,
            mtime_ns BLOB NOT NULL,resume_at INTEGER NOT NULL,head_sha256 BLOB NOT NULL,
            tail_sha256 BLOB NOT NULL,ctime_ns BLOB,codex_pricing_fingerprint TEXT,
            PRIMARY KEY(namespace,file_id),UNIQUE(namespace,path));",
        )
        .unwrap();
        db.execute_batch(metadata_schema()).unwrap();
        for ddl in native_usage::schema() {
            db.execute_batch(ddl).unwrap();
        }
        db.execute_batch(copied_usage_schema()).unwrap();
        db.execute_batch(schema()).unwrap();
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: 64,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset: 64,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let path = "same/relative.jsonl";
        let mut witnesses = Vec::new();
        for namespace in ["a", "b"] {
            bind(&db, namespace);
            let tx = db.transaction().unwrap();
            tx.execute(
                "INSERT INTO files VALUES(semon_namespace(),1,?1,1,2,64,?2,64,?3,?4,?5,NULL)",
                params![
                    path,
                    3_u128.to_be_bytes(),
                    ledger.prefix,
                    ledger.tail,
                    4_i128.to_be_bytes()
                ],
            )
            .unwrap();
            assert_eq!(
                native_usage::begin_source(&tx, path, None, &ledger, false).unwrap(),
                1
            );
            let summary = Summary {
                cwd: Some(namespace.into()),
                models: BTreeSet::from([namespace.into()]),
                ..Summary::default()
            };
            write_tree_summary(&tx, path, &ledger, Some(&summary), true).unwrap();
            assert!(ready(&tx, path, &ledger).unwrap());
            witnesses.push(witness(&tx, path, &ledger).unwrap().unwrap());
            tx.commit().unwrap();
        }
        assert_ne!(witnesses[0], witnesses[1]);
        for (i, namespace) in ["a", "b"].into_iter().enumerate() {
            bind(&db, namespace);
            assert_eq!(
                witness(&db, path, &ledger).unwrap().as_ref(),
                Some(&witnesses[i])
            );
            let mut summary = Summary::default();
            assert!(restore(&db, path, &ledger, &mut summary).unwrap());
            assert_eq!(summary.cwd.as_deref(), Some(namespace));
            assert_eq!(summary.models, BTreeSet::from([namespace.to_owned()]));
        }
        bind(&db, "a");
        let tx = db.transaction().unwrap();
        write_values(
            &tx,
            1,
            &Summary {
                cwd: Some("changed".into()),
                ..Summary::default()
            },
        )
        .unwrap();
        assert!(!ready(&tx, path, &ledger).unwrap());
        bind(&tx, "b");
        assert!(ready(&tx, path, &ledger).unwrap());
        assert_eq!(
            witness(&tx, path, &ledger).unwrap().as_ref(),
            Some(&witnesses[1])
        );
        tx.rollback().unwrap();
        bind(&db, "a");
        assert!(ready(&db, path, &ledger).unwrap());
    }

    // The recipe must reproduce the SQLite json_array witness bytes exactly,
    // including control, NUL, escape and non-BMP content.
    #[test]
    fn tree_witness_recipe_matches_sqlite_json_array_bytes() {
        use crate::events::tree_rows::witness::digest;
        use sha2::{Digest, Sha256};
        let c = rusqlite::Connection::open_in_memory().unwrap();
        register(&c).unwrap();
        c.execute_batch("CREATE TABLE native_tree_labels(projection_id,label,value);CREATE TABLE native_tree_models(projection_id,model);CREATE TABLE native_tree_tools(projection_id,tool_id,name,closed);").unwrap();
        let odd = "a\0b\u{1}\u{1f}\t\n\r\u{8}\u{c}\"\\/\u{7f}\u{2028}é🦀";
        let labels = [
            ("branch", odd),
            ("cwd", "/w"),
            ("first", "🦀"),
            ("last", ""),
        ];
        for (label, value) in labels {
            c.execute(
                "INSERT INTO native_tree_labels VALUES(7,?1,?2)",
                [label, value],
            )
            .unwrap();
        }
        for model in ["m\0", "m🦀", "Z"] {
            c.execute("INSERT INTO native_tree_models VALUES(7,?1)", [model])
                .unwrap();
        }
        for (id, name, closed) in [
            ("t\0", Some(odd), 0),
            ("t🦀", Some("n"), 0),
            ("u", Some("closed"), 1),
            ("v", None, 0),
        ] {
            c.execute(
                "INSERT INTO native_tree_tools VALUES(7,?1,?2,?3)",
                rusqlite::params![id, name, closed],
            )
            .unwrap();
        }
        let mut oracle = Sha256::new();
        for (domain, sql) in [
            (
                "semon.tree.witness.4.labels",
                "SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label",
            ),
            (
                "semon.tree.witness.4.models",
                "SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY semon_tree_digest('tree.model',model)",
            ),
            (
                "semon.tree.witness.4.tools",
                "SELECT json_array(tool_id,name,closed) FROM native_tree_tools WHERE projection_id=?1 ORDER BY semon_tree_digest('tree.tool',tool_id)",
            ),
        ] {
            oracle.update(domain.as_bytes());
            let mut statement = c.prepare(sql).unwrap();
            let mut rows = statement.query([7]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                let value: String = row.get(0).unwrap();
                oracle.update((value.len() as u64).to_be_bytes());
                oracle.update(value.as_bytes());
            }
        }
        let mut models = ["m\0", "m🦀", "Z"];
        models.sort_unstable();
        let actual = digest(
            labels,
            models,
            [
                ("t\0", Some(odd), false),
                ("t🦀", Some("n"), false),
                ("u", Some("closed"), true),
                ("v", None, false),
            ],
        );
        assert_eq!(actual, format!("tree4:{:x}", oracle.finalize()));
    }

    fn qualified_tree(summary: Summary) -> (Connection, Ledger, &'static str) {
        let mut db = Connection::open_in_memory().unwrap();
        register(&db).unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=ON; CREATE TABLE files (
            namespace TEXT NOT NULL,file_id INTEGER NOT NULL,path TEXT NOT NULL,
            dev INTEGER NOT NULL,ino INTEGER NOT NULL,size INTEGER NOT NULL,
            mtime_ns BLOB NOT NULL,resume_at INTEGER NOT NULL,head_sha256 BLOB NOT NULL,
            tail_sha256 BLOB NOT NULL,ctime_ns BLOB,codex_pricing_fingerprint TEXT,
            PRIMARY KEY(namespace,file_id),UNIQUE(namespace,path));",
        )
        .unwrap();
        db.execute_batch(metadata_schema()).unwrap();
        for ddl in native_usage::schema() {
            db.execute_batch(ddl).unwrap();
        }
        db.execute_batch(copied_usage_schema()).unwrap();
        db.execute_batch(schema()).unwrap();
        bind(&db, "witness");
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: 64,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset: 64,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let path = "qualified/witness.jsonl";
        let tx = db.transaction().unwrap();
        tx.execute(
            "INSERT INTO files VALUES(semon_namespace(),1,?1,1,2,64,?2,64,?3,?4,?5,NULL)",
            params![
                path,
                3_u128.to_be_bytes(),
                ledger.prefix,
                ledger.tail,
                4_i128.to_be_bytes()
            ],
        )
        .unwrap();
        assert_eq!(
            native_usage::begin_source(&tx, path, None, &ledger, false).unwrap(),
            1
        );
        write_tree_summary(&tx, path, &ledger, Some(&summary), true).unwrap();
        tx.commit().unwrap();
        (db, ledger, path)
    }

    fn sqlite_witness_oracle(db: &Connection, path: &str) -> (String, usize, usize) {
        use sha2::{Digest, Sha256};
        let projection: i64 = db
            .query_row(
                "SELECT projection_id FROM native_tree_headers WHERE namespace=semon_namespace() AND source_path=?1",
                [path],
                |row| row.get(0),
            )
            .unwrap();
        let mut hash = Sha256::new();
        let mut rows = 0;
        let mut bytes = 0;
        for (domain, query) in [
            (
                "semon.tree.witness.4.labels",
                "SELECT json_array(label,value) FROM native_tree_labels WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY label",
            ),
            (
                "semon.tree.witness.4.models",
                "SELECT json_array(model) FROM native_tree_models WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY model_digest",
            ),
            (
                "semon.tree.witness.4.tools",
                "SELECT json_array(tool_id,name,closed) FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY tool_id_digest",
            ),
        ] {
            hash.update(domain.as_bytes());
            let mut statement = db.prepare(query).unwrap();
            let mut result = statement.query([projection]).unwrap();
            while let Some(row) = result.next().unwrap() {
                let encoded: String = row.get(0).unwrap();
                hash.update((encoded.len() as u64).to_be_bytes());
                hash.update(encoded.as_bytes());
                rows += 1;
                bytes += 64 + encoded.len() * 8;
            }
        }
        (format!("tree4:{:x}", hash.finalize()), rows, bytes)
    }

    #[test]
    fn tree_witness_current_addresses_authenticate_closed_null_and_missing_rows() {
        let summary = Summary {
            models: BTreeSet::from(["m\0🦀".into(), "second".into()]),
            tools: BTreeMap::from([("closed\0🦀".into(), "retained-name".into())]),
            closed_tools: BTreeSet::from(["closed\0🦀".into(), "null-name".into()]),
            ..Summary::default()
        };
        let (mut db, ledger, path) = qualified_tree(summary);
        let expected = witness(&db, path, &ledger).unwrap().unwrap();
        assert!(expected.starts_with("tree4:"));
        for mutation in [
            "UPDATE native_tree_tools SET name='tampered' WHERE tool_id='null-name'",
            "UPDATE native_tree_tools SET closed=0 WHERE tool_id='null-name'",
            "DELETE FROM native_tree_models WHERE model='second'",
        ] {
            let tx = db.transaction().unwrap();
            assert!(tx.execute(mutation, []).unwrap() > 0);
            assert_ne!(witness(&tx, path, &ledger).unwrap().unwrap(), expected);
            tx.rollback().unwrap();
            assert_eq!(witness(&db, path, &ledger).unwrap().unwrap(), expected);
        }
        let encoded: String = db
            .query_row(
                "SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?1",
                [tree_summary_key(path)],
                |row| row.get(0),
            )
            .unwrap();
        let mut old: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        old["projection_witness"] = "unversioned-old-witness".into();
        let old = serde_json::to_string(&old).unwrap();
        db.execute(
            "UPDATE meta SET value=?1 WHERE namespace=semon_namespace() AND key=?2",
            params![old, tree_summary_key(path)],
        )
        .unwrap();
        let mut statement = db.prepare(native_metadata_value(&db).unwrap()).unwrap();
        assert!(
            read_tree_summary(&db, &mut statement, path, &ledger)
                .unwrap()
                .is_none()
        );
        let retained: String = db
            .query_row(
                "SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?1",
                [tree_summary_key(path)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, old, "unsupported witness remains untouched");
        assert_eq!(witness(&db, path, &ledger).unwrap().unwrap(), expected);
    }

    #[test]
    fn tree_witness_budgets_actual_qualified_json_before_decode() {
        let escaped = "a\0b\u{1}\u{1f}\"\\\u{2028}🦀";
        let summary = Summary {
            cwd: Some(escaped.into()),
            branch: Some("branch\0\\\"🦀".into()),
            models: BTreeSet::from(["model\0\u{1}\"\\🦀".into()]),
            tools: BTreeMap::from([("tool\0🦀".into(), escaped.into())]),
            closed_tools: BTreeSet::from(["closed".into()]),
            ..Summary::default()
        };
        let (db, ledger, path) = qualified_tree(summary);
        let (expected, rows, bytes) = sqlite_witness_oracle(&db, path);

        let too_small = crate::SelectedReadControl::default().with_limits(rows, bytes - 1);
        {
            let _scope = crate::selected_read::Scope::enter(&too_small);
            assert!(witness(&db, path, &ledger).is_err());
        }

        let exact = crate::SelectedReadControl::default().with_limits(rows, bytes);
        let actual = {
            let _scope = crate::selected_read::Scope::enter(&exact);
            witness(&db, path, &ledger).unwrap().unwrap()
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn tree_witness_actual_projection_cancels_while_streaming_and_recovers() {
        let summary = Summary {
            tools: (0..2048)
                .map(|index| (format!("tool-{index:04}"), format!("name-{index}")))
                .collect(),
            ..Summary::default()
        };
        let (db, ledger, path) = qualified_tree(summary);
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = cancelled.clone();
        let control = crate::SelectedReadControl::new(
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            cancelled.clone(),
        )
        .with_limits(4096, 64 * 1024 * 1024);
        crate::selected_read::set_hook("tree_witness", 3, move || {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
        });
        {
            let _scope = crate::selected_read::Scope::enter(&control);
            assert!(witness(&db, path, &ledger).is_err());
        }
        assert!(observed.load(std::sync::atomic::Ordering::Acquire));
        assert!(witness(&db, path, &ledger).unwrap().is_some());
    }
}
