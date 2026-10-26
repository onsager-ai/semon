//! Supported-current Catalog row assignment. Originals remain fenced backups;
//! Event/ledger qualification and the store-level multi-source gate stay closed.
use super::{failed, namespace};
use crate::storage::{LogicalSourceNamespace, StorageError, StoreBackend, StoreConfig};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, TransactionBehavior,
    functions::FunctionFlags,
    types::{Value, ValueRef},
};
use std::time::Duration;

pub(crate) const QUALIFIED_CATALOG_TABLES: &[&str] = &[
    "session_catalog",
    "session_catalog_sources",
    "session_slot_projections",
    "session_slots",
    "session_history_catalog",
    "session_history_projections",
    "session_history_slots",
    "session_history_slot_parts",
    "session_trace_headers",
    "session_trace_turns",
    "session_trace_edges",
    "session_trace_bookmarks",
    "session_overview",
    "session_search_coverage",
    "session_search_documents",
    "session_search_memberships",
    "session_search_counts",
    "session_catalog_changes",
    "session_catalog_invalidations",
    "catalog_backfill_sources",
];
const BYTE_LIMIT: usize = 8 * 1024 * 1024;
const ROW_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogAssignmentProgress {
    pub table: String,
    pub copied: usize,
    pub complete: bool,
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub(super) fn register(
    db: &Connection,
    source: &LogicalSourceNamespace,
) -> Result<(), StorageError> {
    crate::events::store::configured::bind_metadata_scope(db, source.as_str()).map_err(failed)?;
    let source = source.as_str().to_owned();
    db.create_scalar_function(
        "semon_namespace",
        0,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        move |_| Ok(source.clone()),
    )
    .map_err(failed)
}

pub(super) fn started(db: &Connection) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='storage_catalog_assignment' AND type='table')", [], |r| r.get(0))
}

pub(super) fn ready(db: &Connection) -> Result<bool, StorageError> {
    if !started(db).map_err(failed)? {
        return Ok(false);
    }
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM storage_catalog_assignment WHERE complete=1",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    Ok(count == QUALIFIED_CATALOG_TABLES.len() as i64)
}

/// Every unqualified native SELECT resolves a connection-local view with an
/// explicit namespace predicate. Indexed Catalog ranges use main plus the same
/// predicate, retaining namespace-leading covering indexes.
pub(super) fn scope_readers(db: &Connection) -> Result<bool, StorageError> {
    if !started(db).map_err(failed)? {
        return Ok(false);
    }
    if !ready(db)? {
        return Err(StorageError::NamespaceUpgradePending);
    }
    for table in QUALIFIED_CATALOG_TABLES {
        // Preserve the native positional row layout. Namespace is an adapter
        // qualifier, not a new native payload column.
        let mut statement = db
            .prepare(&format!("PRAGMA main.table_info({})", quote(table)))
            .map_err(failed)?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(failed)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed)?;
        let columns = names
            .iter()
            .filter(|name| name.as_str() != "namespace")
            .map(|name| quote(name))
            .collect::<Vec<_>>()
            .join(",");
        if columns.is_empty() {
            return Err(StorageError::NamespaceUpgradePending);
        }
        db.execute_batch(&format!("CREATE TEMP VIEW IF NOT EXISTS {} AS SELECT {columns} FROM main.{} WHERE namespace=semon_namespace();", quote(table), quote(table))).map_err(failed)?;
    }
    Ok(true)
}

fn columns(db: &Connection, table: &str) -> Result<(Vec<String>, Vec<String>), StorageError> {
    let mut statement = db
        .prepare(&format!("PRAGMA main.table_info({})", quote(table)))
        .map_err(failed)?;
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(5)?)))
        .map_err(failed)?;
    let mut names = Vec::new();
    let mut keys = Vec::new();
    for row in rows {
        let (name, ordinal) = row.map_err(failed)?;
        if name.len() > 256 || names.len() >= 64 {
            return Err(StorageError::NamespaceUpgradePending);
        }
        names.push(name.clone());
        if ordinal > 0 {
            keys.push((ordinal, name));
        }
    }
    keys.sort();
    if names.is_empty() || keys.is_empty() || names.iter().any(|n| n == "namespace") {
        return Err(StorageError::NamespaceUpgradePending);
    }
    Ok((names, keys.into_iter().map(|(_, name)| name).collect()))
}

fn create_destination(db: &Connection, table: &str, keys: &[String]) -> Result<(), StorageError> {
    let sql: String = db.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1 AND octet_length(sql)<=65536", [table], |r| r.get(0)).map_err(failed)?;
    let open = sql.find('(').ok_or(StorageError::NamespaceUpgradePending)?;
    let close = sql
        .rfind(')')
        .ok_or(StorageError::NamespaceUpgradePending)?;
    let mut body = sql[open + 1..close].to_owned();
    let primary = body
        .find("PRIMARY KEY")
        .ok_or(StorageError::NamespaceUpgradePending)?;
    let after = primary + "PRIMARY KEY".len();
    if body[after..].trim_start().starts_with('(') {
        let end = after
            + body[after..]
                .find(')')
                .ok_or(StorageError::NamespaceUpgradePending)?
            + 1;
        let left = body[..primary].trim_end().trim_end_matches(',');
        let right = body[end..].trim_start().trim_start_matches(',');
        body = if right.is_empty() {
            left.to_owned()
        } else {
            format!("{left},{right}")
        };
    } else {
        body = body
            .replace("PRIMARY KEY AUTOINCREMENT", "")
            .replace("PRIMARY KEY", "");
    }
    let key = keys
        .iter()
        .map(|key| quote(key))
        .collect::<Vec<_>>()
        .join(",");
    db.execute_batch(&format!(
        "CREATE TABLE {} (namespace TEXT NOT NULL,{body},PRIMARY KEY(namespace,{key})){};",
        quote(&format!("storage_copy_{table}")),
        &sql[close + 1..]
    ))
    .map_err(failed)?;
    // Preserve authored expression/partial indexes, adding namespace first.
    let indexes: Vec<(String, String)> = db.prepare("SELECT name,sql FROM sqlite_schema WHERE type='index' AND tbl_name=?1 AND sql IS NOT NULL ORDER BY name LIMIT 65").map_err(failed)?
        .query_map([table], |r| Ok((r.get(0)?, r.get(1)?))).map_err(failed)?.collect::<rusqlite::Result<_>>().map_err(failed)?;
    if indexes.len() > 64 {
        return Err(StorageError::NamespaceUpgradePending);
    }
    for (name, definition) in indexes {
        let at = definition
            .find('(')
            .ok_or(StorageError::NamespaceUpgradePending)?;
        let unique = if definition.starts_with("CREATE UNIQUE INDEX") {
            "UNIQUE "
        } else {
            ""
        };
        db.execute_batch(&format!(
            "CREATE {unique}INDEX {} ON {}(namespace,{};",
            quote(&format!("storage_ns_{name}")),
            quote(&format!("storage_copy_{table}")),
            &definition[at + 1..]
        ))
        .map_err(failed)?;
    }
    Ok(())
}

pub(super) fn guard(db: &Connection, table: &str) -> Result<(), StorageError> {
    for (operation, predicate) in [
        ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
        (
            "UPDATE",
            "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
        ),
        ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
    ] {
        db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS {} BEFORE {operation} ON {} WHEN {predicate} BEGIN SELECT RAISE(ABORT,'namespace writer refused'); END;", quote(&format!("storage_catalog_guard_{table}_{operation}")), quote(table))).map_err(failed)?;
    }
    Ok(())
}

/// Adapter-private row writer. Namespace is supplied by the admitted writer,
/// never by the row image. Callers cannot select arbitrary tables or SQL.
fn write_row(
    db: &Connection,
    namespace: &str,
    table: &str,
    values: Vec<Value>,
) -> Result<(), StorageError> {
    let canonical = table.strip_prefix("storage_copy_").unwrap_or(table);
    if !QUALIFIED_CATALOG_TABLES.contains(&canonical) {
        return Err(StorageError::InvalidArguments);
    }
    let mut bound = vec![Value::Text(namespace.to_owned())];
    bound.extend(values);
    db.execute(
        &format!(
            "INSERT INTO {} VALUES({})",
            quote(table),
            vec!["?"; bound.len()].join(",")
        ),
        rusqlite::params_from_iter(bound),
    )
    .map_err(failed)?;
    Ok(())
}

fn install_change_delivery(db: &Connection) -> Result<(), StorageError> {
    for (table, history) in [
        ("session_catalog", false),
        ("session_history_catalog", true),
    ] {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            let record = if operation == "DELETE" { "OLD" } else { "NEW" };
            let scopes: &[&str] = if history {
                &["retained_history"]
            } else {
                &["current", "retained_history"]
            };
            let mut body = String::new();
            for scope in scopes {
                body.push_str(&format!("INSERT INTO session_catalog_changes(namespace,revision,session_key,read_scope) SELECT {record}.namespace,coalesce(max(revision),0)+1,{record}.session_key,'{scope}' FROM session_catalog_changes WHERE namespace={record}.namespace;"));
            }
            let when = if operation == "UPDATE" {
                "WHEN OLD.metadata IS NOT NEW.metadata"
            } else {
                ""
            };
            db.execute_batch(&format!(
                "CREATE TRIGGER {} AFTER {operation} ON {} {when} BEGIN {body} END;",
                quote(&format!("storage_ns_changes_{table}_{operation}")),
                quote(table)
            ))
            .map_err(failed)?;
        }
    }
    Ok(())
}

pub(crate) fn assign(
    config: &StoreConfig,
    limit: usize,
) -> Result<CatalogAssignmentProgress, StorageError> {
    if limit == 0 || limit > 64 {
        return Err(StorageError::InvalidArguments);
    }
    let epoch = config
        .expected_stream_epoch
        .as_deref()
        .ok_or(StorageError::InvalidArguments)?;
    let StoreBackend::Sqlite { database } = &config.backend;
    let mut db = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(failed)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(failed)?;
    register(&db, &config.namespace)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    namespace::require_admitted(&tx, &config.namespace)?;
    let actual: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if actual != epoch {
        return Err(StorageError::ScopeChanged);
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS storage_catalog_assignment(table_name TEXT PRIMARY KEY,cursor TEXT,complete INTEGER NOT NULL CHECK(complete IN(0,1))) STRICT;").map_err(failed)?;
    let mut selected = None;
    for table in QUALIFIED_CATALOG_TABLES {
        if tx
            .query_row(
                "SELECT complete FROM storage_catalog_assignment WHERE table_name=?1",
                [*table],
                |r| r.get::<_, bool>(0),
            )
            .optional()
            .map_err(failed)?
            != Some(true)
        {
            selected = Some(*table);
            break;
        }
    }
    let Some(table) = selected else {
        return Ok(CatalogAssignmentProgress {
            table: String::new(),
            copied: 0,
            complete: true,
        });
    };
    let (names, keys) = columns(&tx, table)?;
    let cursor: Option<Option<String>> = tx
        .query_row(
            "SELECT cursor FROM storage_catalog_assignment WHERE table_name=?1",
            [table],
            |r| r.get(0),
        )
        .optional()
        .map_err(failed)?;
    if cursor.is_none() {
        create_destination(&tx, table, &keys)?;
        tx.execute(
            "INSERT INTO storage_catalog_assignment VALUES(?1,NULL,0)",
            [table],
        )
        .map_err(failed)?;
    }
    let last: Vec<serde_json::Value> = cursor
        .flatten()
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|_| StorageError::Unavailable)?
        .unwrap_or_default();
    let mut params = Vec::new();
    for value in last {
        params.push(match value {
            serde_json::Value::String(s) => Value::Text(s),
            serde_json::Value::Number(n) => {
                Value::Integer(n.as_i64().ok_or(StorageError::Unavailable)?)
            }
            _ => return Err(StorageError::Unavailable),
        });
    }
    let key_sql = keys
        .iter()
        .map(|key| quote(key))
        .collect::<Vec<_>>()
        .join(",");
    let predicate = if params.is_empty() {
        String::new()
    } else {
        format!("WHERE ({key_sql})>({})", vec!["?"; params.len()].join(","))
    };
    params.push(Value::Integer(limit as i64));
    let mut select = tx
        .prepare(&format!(
            "SELECT * FROM {} {predicate} ORDER BY {key_sql} LIMIT ?",
            quote(table)
        ))
        .map_err(failed)?;
    let mut rows = select
        .query(rusqlite::params_from_iter(params))
        .map_err(failed)?;
    let mut copied = 0;
    let mut bytes = 0;
    let mut next = None;
    let mut exhausted = true;
    while let Some(row) = rows.next().map_err(failed)? {
        let cost = (0..names.len())
            .map(|i| {
                row.get_ref(i).map(|v| match v {
                    ValueRef::Text(s) | ValueRef::Blob(s) => s.len(),
                    _ => 16,
                })
            })
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed)?
            .into_iter()
            .sum::<usize>();
        if cost > ROW_LIMIT {
            return Err(StorageError::NamespaceUpgradePending);
        }
        if bytes + cost > BYTE_LIMIT {
            exhausted = false;
            break;
        }
        let values = (0..names.len())
            .map(|i| row.get::<_, Value>(i))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed)?;
        let checkpoint = keys
            .iter()
            .map(
                |key| match &values[names.iter().position(|name| name == key).unwrap()] {
                    Value::Text(s) => Ok(serde_json::Value::String(s.clone())),
                    Value::Integer(n) => Ok(serde_json::json!(n)),
                    _ => Err(StorageError::Unavailable),
                },
            )
            .collect::<Result<Vec<_>, _>>()?;
        write_row(
            &tx,
            config.namespace.as_str(),
            &format!("storage_copy_{table}"),
            values,
        )?;
        next = Some(serde_json::to_string(&checkpoint).map_err(|_| StorageError::Unavailable)?);
        copied += 1;
        bytes += cost;
    }
    drop(rows);
    drop(select);
    // A full page is conservatively resumed; the next bounded probe finalizes.
    if copied == limit {
        exhausted = false;
    }
    if exhausted {
        tx.execute_batch(&format!(
            "ALTER TABLE {} RENAME TO {}; ALTER TABLE {} RENAME TO {};",
            quote(table),
            quote(&format!("storage_original_{table}")),
            quote(&format!("storage_copy_{table}")),
            quote(table)
        ))
        .map_err(failed)?;
        guard(&tx, table)?;
        if table == "session_catalog" {
            tx.execute_batch("CREATE INDEX storage_ns_catalog_identity ON session_catalog(namespace,session_key);").map_err(failed)?;
        }
        if table == "session_catalog_invalidations" {
            tx.execute_batch("CREATE INDEX storage_ns_source_pending ON session_catalog_invalidations(namespace,revision,source_path);").map_err(failed)?;
        }
        tx.execute(
            "UPDATE storage_catalog_assignment SET complete=1 WHERE table_name=?1",
            [table],
        )
        .map_err(failed)?;
        tx.execute(
            "UPDATE storage_writer_inventory SET qualified_version=1 WHERE table_name=?1",
            [table],
        )
        .map_err(failed)?;
    } else if let Some(cursor) = next {
        tx.execute(
            "UPDATE storage_catalog_assignment SET cursor=?2 WHERE table_name=?1",
            (table, cursor),
        )
        .map_err(failed)?;
    }
    let complete = ready(&tx)?;
    if complete {
        install_change_delivery(&tx)?;
    }
    tx.commit().map_err(|_| StorageError::CommitUncertain)?;
    Ok(CatalogAssignmentProgress {
        table: table.to_owned(),
        copied,
        complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventCache;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture {
        root: PathBuf,
        config: StoreConfig,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-catalog-ns-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let cache = root.join("index.json");
            drop(EventCache::open_scoped(&cache, &source).unwrap());
            let database = EventCache::path(&cache);
            let db = Connection::open(&database).unwrap();
            for key in ["same", "tail"] {
                db.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,10,'codex','{\"owner\":\"A\"}')", [key]).unwrap();
            }
            db.execute("INSERT INTO session_catalog_sources VALUES('same','current','codex','native-same','relative/same.jsonl')", []).unwrap();
            let epoch = db
                .query_row(
                    "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let config = StoreConfig {
                backend: StoreBackend::Sqlite { database },
                namespace: LogicalSourceNamespace::new("A").unwrap(),
                expected_stream_epoch: Some(epoch),
            };
            Self { root, config }
        }
        fn db(&self) -> Connection {
            let StoreBackend::Sqlite { database } = &self.config.backend;
            Connection::open(database).unwrap()
        }
        fn finish(&self) {
            for _ in 0..128 {
                // The explicit first batch below proves one-row interruption.
                // Drain real publisher search metadata with bounded full batches.
                if assign(&self.config, 64).unwrap().complete {
                    return;
                }
            }
            panic!("bounded assignment did not finish");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn catalog_assignment_resumes_without_losing_rows_and_preserves_originals() {
        let f = Fixture::new();
        let admission = f.config.admit_namespace().unwrap();
        let first = assign(&f.config, 1).unwrap();
        assert_eq!(first.copied, 1);
        assert!(!first.complete);
        assert_eq!(
            f.config.unenroll_namespace(&admission),
            Err(StorageError::NamespaceUpgradePending)
        );
        // Every step reopens the database: committed cursors survive restart.
        assert_eq!(
            f.db()
                .query_row(
                    "SELECT count(*) FROM storage_copy_session_catalog",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        f.finish();
        let db = f.db();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM session_catalog WHERE namespace='A'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM storage_original_session_catalog",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT metadata FROM session_catalog WHERE namespace='A' AND session_key='same'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "{\"owner\":\"A\"}"
        );
        let epoch: String = db
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!epoch.is_empty());
        assert!(
            db.execute(
                "UPDATE meta SET value='bad' WHERE key='catalog_changes_epoch'",
                []
            )
            .is_err()
        );
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        for table in QUALIFIED_CATALOG_TABLES {
            let original = quote(&format!("storage_original_{table}"));
            let triggers: i64 = db.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1 AND name GLOB 'storage_namespace_fence_*'", [format!("storage_original_{table}")], |r| r.get(0)).unwrap();
            assert_eq!(triggers, 3, "renamed original retains all admission fences");
            if *table == "session_catalog" {
                for sql in [
                    format!(
                        "INSERT INTO {original}(session_key,last_ms,harness,metadata) VALUES('new-original',11,'codex','{{}}')"
                    ),
                    format!("UPDATE {original} SET last_ms=11 WHERE session_key='same'"),
                    format!("DELETE FROM {original} WHERE session_key='same'"),
                ] {
                    let error = db.execute(&sql, []).unwrap_err();
                    assert!(
                        error
                            .to_string()
                            .contains("native namespace writer upgrade pending")
                    );
                }
            }
            let mut foreign_keys = db
                .prepare(&format!("PRAGMA main.foreign_key_list({})", quote(table)))
                .unwrap();
            assert_eq!(
                foreign_keys.query_map([], |_| Ok(())).unwrap().count(),
                0,
                "supported native schema has no production foreign keys"
            );
        }
        let scoped = f.db();
        register(&scoped, &f.config.namespace).unwrap();
        scope_readers(&scoped).unwrap();
        let projected = scoped.prepare("SELECT * FROM session_catalog").unwrap();
        assert_eq!(projected.column_names()[0], "session_key");
        assert!(!projected.column_names().contains(&"namespace"));
        assert!(db.execute("INSERT INTO session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES('A','escape',1,'codex','{}')", []).is_err());
        let pending: i64 = db
            .query_row(
                "SELECT count(*) FROM storage_writer_inventory WHERE qualified_version=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, QUALIFIED_CATALOG_TABLES.len() as i64);
    }

    #[test]
    fn real_native_publisher_refuses_enrolled_unit_before_mutation() {
        let f = Fixture::new();
        // Hold a real warm producer across enrollment and table replacement.
        let mut producer = EventCache::open_scoped(&f.root.join("index.json"), &crate::Options::default().source).unwrap();
        // Replace the table-level fixture with metadata produced by the real
        // native publication algorithm before enrollment.
        f.db()
            .execute_batch("DELETE FROM session_catalog_sources; DELETE FROM session_catalog;")
            .unwrap();
        let rows: Vec<_> = ["same", "tail"]
            .into_iter()
            .map(|key| crate::model::summary::CatalogRow {
                overview: None,
                trace: None,
                query: None,
                lifecycle: crate::model::summary::CatalogLifecycle::Current,
                key: key.into(),
                name: key.into(),
                harness: "codex".into(),
                kind: "main".into(),
                presentation_kind: None,
                native_ids: vec![key.into()],
                parent: None,
                parent_source: None,
                repo: None,
                branch: None,
                model: "unknown".into(),
                effort: None,
                start: Some(1),
                last: Some(10),
                tokens: [0.0; 3],
                cost: crate::pricing::CostBuild::default().finish(),
                sources: vec![],
            })
            .collect();
        let generation = producer
            .publish_catalog_projection_fenced(
                &crate::slot_projection::Publication {
                    rows: &rows,
                    transcripts: None,
                },
                None,
            )
            .unwrap()
            .expect("real native publisher committed its generation");
        let db = f.db();
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            generation
        );
        assert!(
            db.query_row("SELECT count(*) FROM session_catalog_changes", [], |r| r
                .get::<_, i64>(0))
                .unwrap()
                >= 4
        );
        f.config.admit_namespace().unwrap();
        f.finish();
        let options = crate::Options {
            cache: f.root.join("index.json"),
            ..crate::Options::default()
        };
        let bookmark = f.config.session_bookmark(&options, "sid=same");
        // The composed bookmark still reaches an unported Catalog helper.
        // Its configured path must refuse rather than fall back to unbound SQL.
        assert_eq!(
            bookmark.status,
            503,
            "{}",
            String::from_utf8_lossy(&bookmark.body)
        );
        let bookmark: serde_json::Value = serde_json::from_slice(&bookmark.body).unwrap();
        assert_eq!(bookmark["error"]["code"], "catalog_unavailable");
        assert_eq!(bookmark["error"]["retryable"], true);
        assert!(bookmark.get("matches").is_none());
        let db = f.db();
        let before: (i64, Option<String>, i64) = db.query_row(
            "SELECT (SELECT count(*) FROM storage_original_session_catalog),(SELECT value FROM meta WHERE key='catalog_generation'),(SELECT count(*) FROM session_catalog_changes)",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        let error = producer
            .publish_catalog_projection_fenced(
                &crate::slot_projection::Publication {
                    rows: &[],
                    transcripts: None,
                },
                before.1.as_deref(),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&StorageError::NamespaceUpgradePending.to_string())
        );
        let after: (i64, Option<String>, i64) = db.query_row(
            "SELECT (SELECT count(*) FROM storage_original_session_catalog),(SELECT value FROM meta WHERE key='catalog_generation'),(SELECT count(*) FROM session_catalog_changes)",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!(before, after);
        assert_eq!(before.0, 2);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM session_catalog WHERE namespace='A'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(super::namespace::WRITER_QUALIFICATION_VERSION, 0);
    }

    #[test]
    fn navigation_reads_pin_admission_epoch_generation_and_namespace() {
        use super::super::navigation::SqliteNavigation;
        use crate::storage::{NativeReaderBinding, navigation::NativeNavigationStore};
        let f = Fixture::new();
        let generation = "a".repeat(64);
        f.db()
            .execute(
                "INSERT OR REPLACE INTO meta VALUES('catalog_version',?1)",
                [crate::model::summary::CATALOG_VERSION.to_string()],
            )
            .unwrap();
        f.db()
            .execute(
                "INSERT OR REPLACE INTO meta VALUES('catalog_generation',?1)",
                [&generation],
            )
            .unwrap();
        f.db()
            .execute(
                "INSERT INTO session_trace_bookmarks VALUES('target','same','edge')",
                [],
            )
            .unwrap();
        for key in ["a", "b", "c", "d"] {
            f.db()
                .execute(
                    "INSERT INTO session_trace_bookmarks VALUES('bounded-target',?1,'edge')",
                    [key],
                )
                .unwrap();
        }
        assert_eq!(
            f.config.catalog_generation(),
            Err(StorageError::NamespaceUnadmitted)
        );
        assert_eq!(
            SqliteNavigation(NativeReaderBinding::Admitted(f.config.clone()))
                .bookmarked_target_keys("target"),
            Err(StorageError::NamespaceUnadmitted)
        );
        f.config.admit_namespace().unwrap();
        f.finish();
        assert_eq!(f.config.catalog_generation(), Ok(generation.clone()));
        // A private table-level second-source fixture is not public admission.
        let db = f.db();
        register(&db, &LogicalSourceNamespace::new("B").unwrap()).unwrap();
        db.execute(
            "INSERT INTO session_trace_bookmarks VALUES('B','target','foreign','edge')",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO session_catalog_sources VALUES('B','foreign','current','codex','native-same','relative/same.jsonl')", []).unwrap();
        let store = SqliteNavigation(NativeReaderBinding::Admitted(f.config.clone()));
        assert_eq!(f.config.bookmarked_target_keys("target").unwrap(), ["same"]);
        assert_eq!(
            f.config.bookmarked_target_keys("bounded-target").unwrap(),
            ["a", "b", "c"]
        );
        assert_eq!(
            f.config.bookmarked_target_keys(&"x".repeat(4097)),
            Err(StorageError::InvalidArguments)
        );
        let observation = store
            .native_resolution(
                "codex",
                "native-same",
                crate::CatalogReadScope::Current,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(observation.keys, ["same"]);
        assert_eq!(observation.metadata.as_deref(), Some("{\"owner\":\"A\"}"));
        assert_eq!(observation.generation.as_deref(), Some(generation.as_str()));
        assert!(matches!(
            store.native_resolution(
                "codex",
                "native-same",
                crate::CatalogReadScope::Current,
                Some(&"b".repeat(64))
            ),
            Err(StorageError::ScopeChanged)
        ));
        let mut stale = f.config.clone();
        stale.expected_stream_epoch = Some("b".repeat(64));
        assert_eq!(stale.catalog_generation(), Err(StorageError::ScopeChanged));
        let stale = SqliteNavigation(NativeReaderBinding::Admitted(stale));
        assert_eq!(
            stale.bookmarked_target_keys("target"),
            Err(StorageError::ScopeChanged)
        );
        let mut foreign = f.config.clone();
        foreign.namespace = LogicalSourceNamespace::new("B").unwrap();
        assert_eq!(
            SqliteNavigation(NativeReaderBinding::Admitted(foreign))
                .bookmarked_target_keys("target"),
            Err(StorageError::NamespaceMismatch)
        );
        assert_eq!(
            SqliteNavigation(NativeReaderBinding::TransitionalUnboundLocal(
                f.config.clone()
            ))
            .bookmarked_target_keys("target"),
            Err(StorageError::NamespaceUpgradePending)
        );
        assert_eq!(
            SqliteNavigation(NativeReaderBinding::TransitionalUnboundLocal(
                f.config.clone()
            ))
            .catalog_generation(),
            Err(StorageError::NamespaceUpgradePending)
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM storage_original_session_catalog",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
    }

    #[test]
    fn qualified_navigation_uses_retained_metadata_fallback_bounds_and_indexes() {
        use super::super::navigation::SqliteNavigation;
        use crate::storage::{NativeReaderBinding, navigation::NativeNavigationStore};
        let f = Fixture::new();
        let generation = "a".repeat(64);
        let db = f.db();
        db.execute(
            "INSERT OR REPLACE INTO meta VALUES('catalog_version',?1)",
            [crate::model::summary::CATALOG_VERSION.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT OR REPLACE INTO meta VALUES('catalog_generation',?1)",
            [&generation],
        )
        .unwrap();
        drop(db);
        f.config.admit_namespace().unwrap();
        f.finish();

        let db = f.db();
        register(&db, &f.config.namespace).unwrap();
        db.execute(
            "INSERT INTO session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A','same','retained','codex','retained-history','retained/same')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO session_history_catalog(namespace,session_key,metadata) VALUES('A','same','{\"owner\":\"history\"}')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A','tail','retained','codex','retained-fallback','retained/tail')",
            [],
        )
        .unwrap();
        let store = SqliteNavigation(NativeReaderBinding::Admitted(f.config.clone()));
        let retained = store
            .native_resolution(
                "codex",
                "retained-history",
                crate::CatalogReadScope::RetainedHistory,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(retained.keys, ["same"]);
        assert_eq!(
            retained.metadata.as_deref(),
            Some("{\"owner\":\"history\"}")
        );
        let fallback = store
            .native_resolution(
                "codex",
                "retained-fallback",
                crate::CatalogReadScope::RetainedHistory,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(fallback.keys, ["tail"]);
        assert_eq!(fallback.metadata.as_deref(), Some("{\"owner\":\"A\"}"));

        let oversized = format!("\"{}\"", "x".repeat(1_048_577));
        db.execute(
            "UPDATE session_catalog SET metadata=?1 WHERE namespace='A' AND session_key='tail'",
            [&oversized],
        )
        .unwrap();
        db.execute(
            "INSERT INTO session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A','tail','current','codex','bounded-metadata','current/tail')",
            [],
        )
        .unwrap();
        let bounded = store
            .native_resolution(
                "codex",
                "bounded-metadata",
                crate::CatalogReadScope::Current,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(bounded.keys, ["tail"]);
        assert_eq!(bounded.metadata, None);

        for (key, path) in [
            ("same", "ambiguous/same"),
            ("tail", "ambiguous/tail"),
            ("third", "ambiguous/third"),
        ] {
            db.execute(
                "INSERT INTO session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES('A',?1,'current','codex','ambiguous',?2)",
                (key, path),
            )
            .unwrap();
        }
        let ambiguous = store
            .native_resolution(
                "codex",
                "ambiguous",
                crate::CatalogReadScope::Current,
                Some(&generation),
            )
            .unwrap();
        assert_eq!(ambiguous.keys, ["same", "tail"]);

        for (index, sql) in [
            "EXPLAIN QUERY PLAN SELECT DISTINCT session_key FROM main.session_catalog_sources INDEXED BY storage_ns_session_catalog_current_native WHERE namespace=semon_namespace() AND lifecycle='current' AND harness='codex' AND native_id='ambiguous' ORDER BY session_key LIMIT 2",
            "EXPLAIN QUERY PLAN SELECT DISTINCT session_key FROM main.session_catalog_sources INDEXED BY storage_ns_session_catalog_native WHERE namespace=semon_namespace() AND harness='codex' AND native_id='retained-history' ORDER BY session_key LIMIT 2",
        ]
        .iter()
        .enumerate()
        {
            let plan: String = db.query_row(sql, [], |row| row.get(3)).unwrap();
            let expected = [
                "COVERING INDEX storage_ns_session_catalog_current_native",
                "COVERING INDEX storage_ns_session_catalog_native",
            ][index];
            assert!(plan.contains(expected), "{plan}");
        }
    }

    #[test]
    fn catalog_table_level_isolation_is_not_native_multi_source_admission() {
        let f = Fixture::new();
        f.config.admit_namespace().unwrap();
        f.finish();
        let mut db = f.db();
        let b = LogicalSourceNamespace::new("B").unwrap();
        register(&db, &b).unwrap();
        {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            for key in ["same", "tail"] {
                let mut values: Vec<Value> = tx
                    .query_row(
                        "SELECT * FROM storage_original_session_catalog WHERE session_key=?1",
                        [key],
                        |r| (0..r.as_ref().column_count()).map(|i| r.get(i)).collect(),
                    )
                    .unwrap();
                values[6] = Value::Text("{\"owner\":\"B\"}".into());
                write_row(&tx, "B", "session_catalog", values).unwrap();
            }
            write_row(
                &tx,
                "B",
                "session_catalog_sources",
                vec![
                    Value::Text("same".into()),
                    Value::Text("current".into()),
                    Value::Text("codex".into()),
                    Value::Text("native-same".into()),
                    Value::Text("relative/same.jsonl".into()),
                ],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        for source in ["A", "B"] {
            let db = f.db();
            register(&db, &LogicalSourceNamespace::new(source).unwrap()).unwrap();
            scope_readers(&db).unwrap();
            let metadata: String = db
                .query_row(
                    "SELECT metadata FROM session_catalog WHERE session_key='same'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&metadata).unwrap()["owner"],
                source
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM session_catalog", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
            let next: String = db.query_row("SELECT session_key FROM main.session_catalog INDEXED BY storage_ns_session_catalog_order WHERE namespace=semon_namespace() AND last_ms=10 AND session_key>'same' ORDER BY last_ms DESC,session_key LIMIT 1", [], |r| r.get(0)).unwrap();
            assert_eq!(next, "tail");
            assert_eq!(db.query_row("SELECT count(*) FROM session_catalog_sources WHERE native_id='native-same' AND source_path='relative/same.jsonl'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        }
        let mut foreign = f.config.clone();
        foreign.namespace = b;
        assert_eq!(
            foreign.admit_namespace(),
            Err(StorageError::MultiSourceUnqualified)
        );
        assert_eq!(super::super::namespace::WRITER_QUALIFICATION_VERSION, 0);
    }

    #[test]
    fn absent_or_ambiguous_assignment_binding_does_not_create_checkpoints() {
        let f = Fixture::new();
        assert_eq!(assign(&f.config, 1), Err(StorageError::NamespaceUnadmitted));
        assert!(!started(&f.db()).unwrap());
        f.config.admit_namespace().unwrap();
        f.db().execute("INSERT INTO storage_namespaces SELECT 'B',admission_generation,admitted_at,format_version,writer_qualification FROM storage_namespaces", []).unwrap();
        assert_eq!(
            assign(&f.config, 1),
            Err(StorageError::MultiSourceUnqualified)
        );
        assert!(!started(&f.db()).unwrap());
    }
}
