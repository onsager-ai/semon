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

#[derive(Clone)]
pub(crate) struct Snapshot {
    db: PathBuf,
    path: String,
    ledger: Ledger,
    file_id: i64,
    projection_id: i64,
    configured: Option<crate::storage::StoreConfig>,
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
                "SELECT file_id,ledger,projection_id FROM native_tree_headers WHERE source_path=?1",
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
                "SELECT name FROM native_tree_tools WHERE projection_id=?1 AND tool_id=?2",
                params![projection_id, id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
        })
    }
    pub(crate) fn shared_usage(&self) -> io::Result<SharedUsage> {
        self.with(|c,id,projection_id|{
            let sql = "SELECT i.message_id,i.record_uuid FROM native_tree_copied_identities i WHERE i.file_id=?1 ORDER BY i.message_id,i.record_uuid";
            let mut statement = c.prepare(sql)?;
            let mut rows = statement.query([id])?;
            let mut records = crate::claude_usage::Records::new();
            while let Some(row)=rows.next()? { crate::selected_read::row(row,64)?; records.entry(row.get(0)?).or_default().insert(row.get(1)?); }
            let mut tokens = BTreeMap::new();
            let mut models = BTreeMap::new();
            for message in records.keys() {
                let used = c.query_row("SELECT input,cached_input,output,reasoning_output,total FROM usage WHERE file_id=?1 AND message_id=?2",params![id,message],|r|Ok(Tokens {input:uint(r.get(0)?),cached_input:uint(r.get(1)?),output:uint(r.get(2)?),reasoning_output:uint(r.get(3)?),total:uint(r.get(4)?)}))?;
                tokens.insert(message.clone(),used);
                if let Some(model) = c.query_row("SELECT model FROM native_tree_usage_models WHERE projection_id=?1 AND message_id=?2",params![projection_id,message],|r|r.get::<_,String>(0)).optional()? { models.insert(message.clone(),model); }
            }
            Ok((records,tokens,models))
        })
    }
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
    let previous:Option<i64>=tx.query_row("SELECT g.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(projection_id) WHERE h.source_path=?1 AND g.source_id=?2 AND g.version=3",params![path,source_id],|r|r.get(0)).optional()?;
    let projection_id = match previous {
        Some(id) => id,
        None => new_generation(tx, source_id, ledger)?,
    };
    tx.execute("INSERT INTO native_tree_headers(source_path,file_id,projection_id,ledger) VALUES(?1,?2,?3,?4) ON CONFLICT(source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger",params![path,id,projection_id,header_ledger(ledger)])?;
    write_values(tx, projection_id, summary)?;
    tx.execute(
        "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE projection_id=?1",
        params![projection_id, header_ledger(ledger)],
    )?;
    Ok(())
}

pub(super) fn new_generation(
    tx: &Transaction<'_>,
    source_id: i64,
    ledger: &Ledger,
) -> rusqlite::Result<i64> {
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
        "UPDATE native_tree_generations SET receipt=?2,complete=1 WHERE projection_id=?1",
        params![projection_id, header_ledger(ledger)],
    )?;
    tx.execute("INSERT INTO native_tree_headers(source_path,file_id,projection_id,ledger) VALUES(?1,?2,?3,?4) ON CONFLICT(source_path) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger",params![path,id,projection_id,header_ledger(ledger)])?;
    Ok(())
}

pub(super) fn write_values(
    tx: &Transaction<'_>,
    id: i64,
    summary: &Summary,
) -> rusqlite::Result<()> {
    let turn = serde_json::to_string(&summary.codex_turn)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    tx.prepare_cached("INSERT INTO native_tree_labels VALUES(?1,'codex_turn',?2) ON CONFLICT(projection_id,label) DO UPDATE SET value=excluded.value")?.execute(params![id,turn])?;
    for (label, value) in [
        ("first", &summary.first),
        ("last", &summary.last),
        ("cwd", &summary.cwd),
        ("branch", &summary.branch),
    ] {
        if let Some(value) = value {
            tx.prepare_cached("INSERT INTO native_tree_labels VALUES(?1,?2,?3) ON CONFLICT(projection_id,label) DO UPDATE SET value=excluded.value")?.execute(params![id,label,value])?;
        }
    }
    for model in &summary.models {
        tx.prepare_cached("INSERT INTO native_tree_models VALUES(?1,?2) ON CONFLICT DO NOTHING")?
            .execute(params![id, model])?;
    }
    for (tool, name) in &summary.tools {
        tx.prepare_cached("INSERT INTO native_tree_tools VALUES(?1,?2,?3,0) ON CONFLICT(projection_id,tool_id) DO UPDATE SET name=excluded.name")?.execute(params![id,tool,name])?;
    }
    for tool in &summary.closed_tools {
        tx.prepare_cached("INSERT INTO native_tree_tools VALUES(?1,?2,NULL,1) ON CONFLICT(projection_id,tool_id) DO UPDATE SET closed=1")?.execute(params![id,tool])?;
    }
    for (message, model) in &summary.usage_models {
        tx.prepare_cached("INSERT INTO native_tree_usage_models VALUES(?1,?2,?3) ON CONFLICT(projection_id,message_id) DO UPDATE SET model=excluded.model")?.execute(params![id,message,model])?;
    }
    Ok(())
}

pub(super) fn restore_labels(
    c: &Connection,
    id: i64,
    summary: &mut Summary,
) -> rusqlite::Result<()> {
    let mut statement = c.prepare(
        "SELECT label,value FROM native_tree_labels WHERE projection_id=?1 ORDER BY label",
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
            "SELECT h.file_id,h.ledger,h.projection_id FROM native_tree_headers h JOIN native_tree_generations g USING(projection_id) JOIN native_usage_current c ON c.source_path=h.source_path AND c.source_id=g.source_id WHERE h.source_path=?1 AND g.complete=1 AND g.version=3 AND g.receipt=h.ledger",
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
        c.prepare("SELECT model FROM native_tree_models WHERE projection_id=?1 ORDER BY model")?;
    summary.models = models
        .query_map([projection_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut tools = c.prepare("SELECT tool_id,name FROM native_tree_tools WHERE projection_id=?1 AND closed=0 AND name IS NOT NULL ORDER BY tool_id")?;
    summary.tools = tools
        .query_map([projection_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let db: String = c.query_row(
        "SELECT file FROM pragma_database_list WHERE name='main'",
        [],
        |r| r.get(0),
    )?;
    if !db.is_empty() {
        summary.stored = Some(Snapshot {
            db: db.into(),
            path: path.into(),
            ledger: ledger.clone(),
            file_id: id,
            projection_id,
            configured: None,
        });
    }
    Ok(true)
}

pub(super) fn ready(c: &Connection, path: &str, ledger: &Ledger) -> rusqlite::Result<bool> {
    c.query_row(configured::metadata_sql(c,
        "SELECT EXISTS(SELECT 1 FROM native_tree_headers h JOIN native_tree_generations g USING(projection_id) JOIN native_usage_current c ON c.source_path=h.source_path AND c.source_id=g.source_id JOIN meta m ON m.key='native_tree_summary:'||h.source_path WHERE h.source_path=?1 AND h.ledger=?2 AND g.complete=1 AND g.version=3 AND g.receipt=h.ledger AND CASE WHEN octet_length(m.value)<=1048576 AND json_valid(m.value) THEN json_extract(m.value,'$.version')=3 ELSE 0 END)",
        "SELECT EXISTS(SELECT 1 FROM native_tree_headers h JOIN native_tree_generations g USING(projection_id) JOIN native_usage_current c ON c.source_path=h.source_path AND c.source_id=g.source_id JOIN meta m ON m.namespace=semon_namespace() AND m.key='native_tree_summary:'||h.source_path WHERE h.source_path=?1 AND h.ledger=?2 AND g.complete=1 AND g.version=3 AND g.receipt=h.ledger AND CASE WHEN octet_length(m.value)<=1048576 AND json_valid(m.value) THEN json_extract(m.value,'$.version')=3 ELSE 0 END)")?,params![path,header_ledger(ledger)],|r|r.get(0))
}

pub(super) fn work_pending(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_tree_generations WHERE complete=1 AND work_pending=1)",
        [],
        |r| r.get(0),
    )
}

pub(super) fn cleanup_work(c: &mut Connection) -> rusqlite::Result<bool> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !current(&tx)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let selected:Option<i64>=tx.query_row("SELECT projection_id FROM native_tree_generations WHERE complete=1 AND work_pending=1 ORDER BY projection_id LIMIT 1",[],|r|r.get(0)).optional()?;
    if let Some(id) = selected {
        for (table, key) in [
            ("native_tree_usage_work", "message_id"),
            ("native_tree_codex_work", "identity"),
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE projection_id=?1 AND {key} IN (SELECT {key} FROM {table} WHERE projection_id=?1 ORDER BY {key} LIMIT 512)"),[id])?;
        }
        tx.execute("UPDATE native_tree_generations SET work_pending=0 WHERE projection_id=?1 AND NOT EXISTS(SELECT 1 FROM native_tree_usage_work WHERE projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_codex_work WHERE projection_id=?1)",[id])?;
    }
    let pending = work_pending(&tx)?;
    tx.commit()?;
    Ok(pending)
}
