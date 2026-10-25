//! Indexed Tree dimensions. Only models, currently open tools and copied usage
//! requested by the selected owner cohort are materialized by ordinary reads.
use super::*;
use crate::native_tree::Summary;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_tree_generations (
 projection_id INTEGER PRIMARY KEY AUTOINCREMENT, source_id INTEGER NOT NULL,
 version INTEGER NOT NULL,receipt TEXT NOT NULL,complete INTEGER NOT NULL,work_pending INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX IF NOT EXISTS native_tree_cleanup_queue ON native_tree_generations(complete,work_pending,projection_id);
CREATE TABLE IF NOT EXISTS native_tree_usage_work (
 projection_id INTEGER NOT NULL,message_id TEXT NOT NULL,tokens TEXT NOT NULL,
 PRIMARY KEY(projection_id,message_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_codex_work (
 projection_id INTEGER NOT NULL,identity TEXT NOT NULL,
 PRIMARY KEY(projection_id,identity)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_headers (
 source_path TEXT PRIMARY KEY, file_id INTEGER NOT NULL, projection_id INTEGER NOT NULL, ledger TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_models (
 projection_id INTEGER NOT NULL,model TEXT NOT NULL,PRIMARY KEY(projection_id,model)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_labels (
 projection_id INTEGER NOT NULL,label TEXT NOT NULL,value TEXT NOT NULL,
 PRIMARY KEY(projection_id,label)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_tools (
 projection_id INTEGER NOT NULL,tool_id TEXT NOT NULL,name TEXT,closed INTEGER NOT NULL,
 PRIMARY KEY(projection_id,tool_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_tree_open_tools ON native_tree_tools(projection_id,tool_id,name) WHERE closed=0 AND name IS NOT NULL;
CREATE TABLE IF NOT EXISTS native_tree_usage_models (
 projection_id INTEGER NOT NULL,message_id TEXT NOT NULL,model TEXT NOT NULL,
 PRIMARY KEY(projection_id,message_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_copied_identities (
 file_id INTEGER NOT NULL,message_id TEXT NOT NULL,record_uuid TEXT NOT NULL,
 PRIMARY KEY(file_id,message_id,record_uuid)
) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_tree_copy_insert AFTER INSERT ON usage_identity BEGIN
 INSERT INTO native_tree_copied_identities
 SELECT file_id,message_id,record_uuid FROM usage_identity
 WHERE message_id=NEW.message_id AND record_uuid=NEW.record_uuid
 AND EXISTS(SELECT 1 FROM usage_identity p WHERE p.message_id=NEW.message_id AND p.record_uuid=NEW.record_uuid AND p.file_id<>NEW.file_id)
 ON CONFLICT DO NOTHING;
END;
CREATE TRIGGER IF NOT EXISTS native_tree_copy_delete AFTER DELETE ON usage_identity BEGIN
 DELETE FROM native_tree_copied_identities WHERE file_id=OLD.file_id AND message_id=OLD.message_id AND record_uuid=OLD.record_uuid;
 DELETE FROM native_tree_copied_identities WHERE (file_id,message_id,record_uuid) IN
 (SELECT i.file_id,i.message_id,i.record_uuid FROM usage_identity i WHERE i.message_id=OLD.message_id AND i.record_uuid=OLD.record_uuid
 AND NOT EXISTS(SELECT 1 FROM usage_identity p WHERE p.message_id=i.message_id AND p.record_uuid=i.record_uuid AND p.file_id<>i.file_id));
END;
";

// Composite Tree and copied-usage families share physical namespace ownership.
// Public creation still selects the singleton family until full qualification.
const COMPOSITE_SCHEMA: &str = "
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
 PRIMARY KEY(namespace,projection_id,model),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_labels (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,label TEXT NOT NULL,value TEXT NOT NULL,
 PRIMARY KEY(namespace,projection_id,label),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_tree_tools (
 namespace TEXT NOT NULL,projection_id INTEGER NOT NULL,tool_id TEXT NOT NULL,name TEXT,closed INTEGER NOT NULL,
 PRIMARY KEY(namespace,projection_id,tool_id),
 FOREIGN KEY(namespace,projection_id) REFERENCES native_tree_generations(namespace,projection_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_tree_open_tools ON native_tree_tools(namespace,projection_id,tool_id,name) WHERE closed=0 AND name IS NOT NULL;
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

pub(super) fn schema(composite: bool) -> &'static str {
    if composite { COMPOSITE_SCHEMA } else { SCHEMA }
}

pub(super) fn sql<'a>(
    c: &Connection,
    legacy: &'a str,
    scoped: &'a str,
) -> rusqlite::Result<&'a str> {
    Ok(if composite(c)? { scoped } else { legacy })
}
fn composite(c: &Connection) -> rusqlite::Result<bool> {
    c.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_table_info('native_tree_headers') WHERE name='namespace' AND pk=1)")?.query_row([], |r| r.get(0))
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
}

impl TreeCheckpoint {
    pub(crate) fn tool_name(&self, id: &str) -> io::Result<Option<String>> {
        match self {
            Self::Sqlite(snapshot) => snapshot.tool_name(id),
        }
    }

    pub(crate) fn shared_usage(&self) -> io::Result<SharedUsage> {
        match self {
            Self::Sqlite(snapshot) => snapshot.shared_usage(),
        }
    }

    pub(crate) fn bind_configured(&mut self, config: crate::storage::StoreConfig) {
        match self {
            Self::Sqlite(snapshot) => snapshot.bind_configured(config),
        }
    }
}
impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TreeSnapshot")
    }
}
type SharedUsage = (
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
                sql(&tx, "SELECT file_id,ledger,projection_id FROM native_tree_headers WHERE source_path=?1", "SELECT file_id,ledger,projection_id FROM native_tree_headers WHERE namespace=semon_namespace() AND source_path=?1").map_err(io::Error::other)?,
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
                sql(c, "SELECT name FROM native_tree_tools WHERE projection_id=?1 AND tool_id=?2", "SELECT name FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 AND tool_id=?2")?,
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
    let query = sql(
        c,
        "SELECT i.message_id,i.record_uuid FROM native_tree_copied_identities i WHERE i.file_id=?1 ORDER BY i.message_id,i.record_uuid",
        "SELECT i.message_id,i.record_uuid FROM native_tree_copied_identities i WHERE i.namespace=semon_namespace() AND i.file_id=?1 ORDER BY i.message_id,i.record_uuid",
    )?;
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
        let used = c.query_row(copied_usage_sql(c, "SELECT input,cached_input,output,reasoning_output,total FROM usage WHERE file_id=?1 AND message_id=?2", "SELECT input,cached_input,output,reasoning_output,total FROM usage WHERE namespace=semon_namespace() AND file_id=?1 AND message_id=?2")?,params![id,message],|r|Ok(Tokens {input:uint(r.get(0)?),cached_input:uint(r.get(1)?),output:uint(r.get(2)?),reasoning_output:uint(r.get(3)?),total:uint(r.get(4)?)}))?;
        tokens.insert(message.clone(), used);
        if let Some(model) = c.query_row(sql(c, "SELECT model FROM native_tree_usage_models WHERE projection_id=?1 AND message_id=?2", "SELECT model FROM native_tree_usage_models WHERE namespace=semon_namespace() AND projection_id=?1 AND message_id=?2")?,params![projection_id,message],|r|r.get::<_,String>(0)).optional()? {
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
        configured::sql(
            tx,
            "SELECT file_id FROM files WHERE path=?1",
            "SELECT file_id FROM files WHERE namespace=semon_namespace() AND path=?1",
        )?,
        [path],
        |r| r.get(0),
    )?;
    let source_id = native_usage::source(tx, path)?
        .ok_or(rusqlite::Error::InvalidQuery)?
        .0;
    let previous:Option<i64>=tx.query_row(sql(tx, "SELECT g.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(projection_id) WHERE h.source_path=?1 AND g.source_id=?2 AND g.version=3", "SELECT g.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(namespace,projection_id) WHERE h.namespace=semon_namespace() AND h.source_path=?1 AND g.source_id=?2 AND g.version=3")?,params![path,source_id],|r|r.get(0)).optional()?;
    let projection_id = match previous {
        Some(id) => id,
        None => new_generation(tx, source_id, ledger)?,
    };
    tx.execute(sql(tx, "INSERT INTO native_tree_headers(source_path,file_id,projection_id,ledger) VALUES(?1,?2,?3,?4) ON CONFLICT(source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger", "INSERT INTO native_tree_headers(namespace,source_path,file_id,projection_id,ledger) VALUES(semon_namespace(),?1,?2,?3,?4) ON CONFLICT(namespace,source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger")?,params![path,id,projection_id,header_ledger(ledger)])?;
    write_values(tx, projection_id, summary)?;
    tx.execute(
        sql(tx, "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE projection_id=?1", "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE namespace=semon_namespace() AND projection_id=?1")?,
        params![projection_id, header_ledger(ledger)],
    )?;
    Ok(())
}

pub(super) fn new_generation(
    tx: &Transaction<'_>,
    source_id: i64,
    ledger: &Ledger,
) -> rusqlite::Result<i64> {
    if composite(tx)? {
        return tx.query_row("INSERT INTO native_tree_generations(namespace,projection_id,source_id,version,receipt,complete) VALUES(semon_namespace(),(SELECT coalesce(max(projection_id),0)+1 FROM native_tree_generations WHERE namespace=semon_namespace()),?1,3,?2,0) RETURNING projection_id", params![source_id, header_ledger(ledger)], |r| r.get(0));
    }
    tx.execute(
        "INSERT INTO native_tree_generations(source_id,version,receipt,complete) VALUES(?1,3,?2,0)",
        params![source_id, header_ledger(ledger)],
    )?;
    Ok(tx.last_insert_rowid())
}

pub(super) fn bind_generation(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    projection_id: i64,
) -> rusqlite::Result<()> {
    let id: i64 = tx.query_row(
        configured::sql(
            tx,
            "SELECT file_id FROM files WHERE path=?1",
            "SELECT file_id FROM files WHERE namespace=semon_namespace() AND path=?1",
        )?,
        [path],
        |r| r.get(0),
    )?;
    tx.execute(
        sql(tx, "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE projection_id=?1", "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE namespace=semon_namespace() AND projection_id=?1")?,
        params![projection_id, header_ledger(ledger)],
    )?;
    tx.execute(sql(tx, "INSERT INTO native_tree_headers(source_path,file_id,projection_id,ledger) VALUES(?1,?2,?3,?4) ON CONFLICT(source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger", "INSERT INTO native_tree_headers(namespace,source_path,file_id,projection_id,ledger) VALUES(semon_namespace(),?1,?2,?3,?4) ON CONFLICT(namespace,source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger")?,params![path,id,projection_id,header_ledger(ledger)])?;
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
    tx.prepare_cached(sql(tx, "INSERT INTO native_tree_labels VALUES(?1,'codex_turn',?2) ON CONFLICT(projection_id,label) DO UPDATE SET value=excluded.value", "INSERT INTO native_tree_labels VALUES(semon_namespace(),?1,'codex_turn',?2) ON CONFLICT(namespace,projection_id,label) DO UPDATE SET value=excluded.value")?)?.execute(params![id,turn])?;
    for (label, value) in rows.labels() {
            tx.prepare_cached(sql(tx, "INSERT INTO native_tree_labels VALUES(?1,?2,?3) ON CONFLICT(projection_id,label) DO UPDATE SET value=excluded.value", "INSERT INTO native_tree_labels VALUES(semon_namespace(),?1,?2,?3) ON CONFLICT(namespace,projection_id,label) DO UPDATE SET value=excluded.value")?)?.execute(params![id,label,value])?;
    }
    for model in rows.models {
        tx.prepare_cached(sql(
            tx,
            "INSERT INTO native_tree_models VALUES(?1,?2) ON CONFLICT DO NOTHING",
            "INSERT INTO native_tree_models VALUES(semon_namespace(),?1,?2) ON CONFLICT DO NOTHING",
        )?)?
        .execute(params![id, model])?;
    }
    for (tool, name) in rows.tools {
        tx.prepare_cached(sql(tx, "INSERT INTO native_tree_tools VALUES(?1,?2,?3,0) ON CONFLICT(projection_id,tool_id) DO UPDATE SET name=excluded.name", "INSERT INTO native_tree_tools VALUES(semon_namespace(),?1,?2,?3,0) ON CONFLICT(namespace,projection_id,tool_id) DO UPDATE SET name=excluded.name")?)?.execute(params![id,tool,name])?;
    }
    for tool in rows.closed_tools {
        tx.prepare_cached(sql(tx, "INSERT INTO native_tree_tools VALUES(?1,?2,NULL,1) ON CONFLICT(projection_id,tool_id) DO UPDATE SET closed=1", "INSERT INTO native_tree_tools VALUES(semon_namespace(),?1,?2,NULL,1) ON CONFLICT(namespace,projection_id,tool_id) DO UPDATE SET closed=1")?)?.execute(params![id,tool])?;
    }
    for (message, model) in rows.usage_models {
        tx.prepare_cached(sql(tx, "INSERT INTO native_tree_usage_models VALUES(?1,?2,?3) ON CONFLICT(projection_id,message_id) DO UPDATE SET model=excluded.model", "INSERT INTO native_tree_usage_models VALUES(semon_namespace(),?1,?2,?3) ON CONFLICT(namespace,projection_id,message_id) DO UPDATE SET model=excluded.model")?)?.execute(params![id,message,model])?;
    }
    Ok(())
}

pub(super) fn restore_labels(
    c: &Connection,
    id: i64,
    summary: &mut Summary,
) -> rusqlite::Result<()> {
    let mut statement = c.prepare(
        sql(c, "SELECT label,value FROM native_tree_labels WHERE projection_id=?1 ORDER BY label", "SELECT label,value FROM native_tree_labels WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY label")?,
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
            sql(c, "SELECT h.file_id,h.ledger,h.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(projection_id) JOIN native_usage_current c ON c.source_path=h.source_path AND c.source_id=g.source_id WHERE h.source_path=?1 AND g.complete=1 AND g.version=3 AND g.receipt=h.ledger", "SELECT h.file_id,h.ledger,h.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(namespace,projection_id) JOIN native_usage_current c ON c.namespace=h.namespace AND c.source_path=h.source_path AND c.source_id=g.source_id WHERE h.namespace=semon_namespace() AND h.source_path=?1 AND g.complete=1 AND g.version=3 AND g.receipt=h.ledger")?,
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
        c.prepare(sql(c, "SELECT model FROM native_tree_models WHERE projection_id=?1 ORDER BY model", "SELECT model FROM native_tree_models WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY model")?)?;
    summary.models = models
        .query_map([projection_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut tools = c.prepare(sql(c, "SELECT tool_id,name FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id", "SELECT tool_id,name FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id")?)?;
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
        sql(c, "SELECT h.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(projection_id) JOIN native_usage_current u ON u.source_path=h.source_path AND u.source_id=g.source_id JOIN native_usage_sources s ON s.source_id=g.source_id WHERE h.source_path=?1 AND h.file_id=?2 AND h.ledger=?3 AND g.receipt=h.ledger AND g.complete=1 AND g.version=3 AND s.parser_version=?4", "SELECT h.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(namespace,projection_id) JOIN native_usage_current u ON u.namespace=h.namespace AND u.source_path=h.source_path AND u.source_id=g.source_id JOIN native_usage_sources s ON s.namespace=g.namespace AND s.source_id=g.source_id WHERE h.namespace=semon_namespace() AND h.source_path=?1 AND h.file_id=?2 AND h.ledger=?3 AND g.receipt=h.ledger AND g.complete=1 AND g.version=3 AND s.parser_version=?4")?,
        params![path, file_id, header_ledger(ledger), CACHE_VERSION],
        |r| r.get(0),
    ).optional()?;
    let Some(projection) = projection else {
        return Ok(None);
    };
    let mut digest = Sha256::new();
    for sql in [
        "SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label",
        "SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY model",
        "SELECT json_array(tool_id,name) FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id",
    ] {
        digest.update(sql.as_bytes());
        let scoped = match sql {
            "SELECT json_array(label,value) FROM native_tree_labels WHERE projection_id=?1 ORDER BY label" => {
                "SELECT json_array(label,value) FROM native_tree_labels WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY label"
            }
            "SELECT json_array(model) FROM native_tree_models WHERE projection_id=?1 ORDER BY model" => {
                "SELECT json_array(model) FROM native_tree_models WHERE namespace=semon_namespace() AND projection_id=?1 ORDER BY model"
            }
            _ => {
                "SELECT json_array(tool_id,name) FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id"
            }
        };
        let mut statement = c.prepare(self::sql(c, sql, scoped)?)?;
        let mut rows = statement.query([projection])?;
        while let Some(row) = rows.next()? {
            crate::selected_read::row(row, 64)?;
            crate::selected_read::checkpoint("tree_witness")
                .map_err(crate::selected_read::sql_error)?;
            let value: String = row.get(0)?;
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
    }
    Ok(Some(format!("{:x}", digest.finalize())))
}

pub(super) fn work_pending(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row(
        sql(c, "SELECT EXISTS(SELECT 1 FROM native_tree_generations WHERE complete=1 AND work_pending=1)", "SELECT EXISTS(SELECT 1 FROM native_tree_generations WHERE namespace=semon_namespace() AND complete=1 AND work_pending=1)")?,
        [],
        |r| r.get(0),
    )
}

pub(super) fn cleanup_work(c: &mut Connection) -> rusqlite::Result<bool> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !current(&tx)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let selected:Option<i64>=tx.query_row(sql(&tx, "SELECT projection_id FROM native_tree_generations WHERE complete=1 AND work_pending=1 ORDER BY projection_id LIMIT 1", "SELECT projection_id FROM native_tree_generations WHERE namespace=semon_namespace() AND complete=1 AND work_pending=1 ORDER BY projection_id LIMIT 1")?,[],|r|r.get(0)).optional()?;
    if let Some(id) = selected {
        for (table, key) in [
            ("native_tree_usage_work", "message_id"),
            ("native_tree_codex_work", "identity"),
        ] {
            let predicate = if composite(&tx)? {
                "namespace=semon_namespace() AND "
            } else {
                ""
            };
            tx.execute(&format!("DELETE FROM {table} WHERE {predicate}projection_id=?1 AND {key} IN (SELECT {key} FROM {table} WHERE {predicate}projection_id=?1 ORDER BY {key} LIMIT 512)"),[id])?;
        }
        tx.execute(sql(&tx, "UPDATE native_tree_generations SET work_pending=0 WHERE projection_id=?1 AND NOT EXISTS(SELECT 1 FROM native_tree_usage_work WHERE projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_codex_work WHERE projection_id=?1)", "UPDATE native_tree_generations SET work_pending=0 WHERE namespace=semon_namespace() AND projection_id=?1 AND NOT EXISTS(SELECT 1 FROM native_tree_usage_work WHERE namespace=semon_namespace() AND projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_codex_work WHERE namespace=semon_namespace() AND projection_id=?1)")?,[id])?;
    }
    let pending = work_pending(&tx)?;
    tx.commit()?;
    Ok(pending)
}

#[cfg(test)]
mod namespace_tests {
    use super::*;
    use rusqlite::functions::FunctionFlags;
    use std::collections::BTreeSet;

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
        db.execute_batch(
            "PRAGMA foreign_keys=ON; CREATE TABLE files (
            namespace TEXT NOT NULL,file_id INTEGER NOT NULL,path TEXT NOT NULL,
            dev INTEGER NOT NULL,ino INTEGER NOT NULL,size INTEGER NOT NULL,
            mtime_ns BLOB NOT NULL,resume_at INTEGER NOT NULL,head_sha256 BLOB NOT NULL,
            tail_sha256 BLOB NOT NULL,ctime_ns BLOB,codex_pricing_fingerprint TEXT,
            PRIMARY KEY(namespace,file_id),UNIQUE(namespace,path));",
        )
        .unwrap();
        db.execute_batch(metadata_schema(true)).unwrap();
        for ddl in native_usage::schema(true) {
            db.execute_batch(ddl).unwrap();
        }
        db.execute_batch(copied_usage_schema(true)).unwrap();
        db.execute_batch(schema(true)).unwrap();
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
}
