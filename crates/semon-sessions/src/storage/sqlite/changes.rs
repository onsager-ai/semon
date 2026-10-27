//! SQLite change delivery. SQL and transaction ownership stay inside the adapter.
use super::failed;
use crate::{
    CatalogChange, CatalogChangeCompaction, CatalogChangePage, CatalogReadScope,
    model::summary::CATALOG_VERSION, storage::StorageError,
};
use rusqlite::{Connection, OptionalExtension};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS session_catalog_changes (
 revision INTEGER PRIMARY KEY AUTOINCREMENT, namespace TEXT NOT NULL,
 session_key TEXT NOT NULL,
 read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history'))
) STRICT;
CREATE INDEX IF NOT EXISTS session_catalog_change_order ON session_catalog_changes(namespace,read_scope,revision);
CREATE TRIGGER IF NOT EXISTS catalog_history_changes_insert AFTER INSERT ON session_history_catalog BEGIN
 INSERT INTO session_catalog_changes(namespace,session_key,read_scope) VALUES(semon_namespace(),NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_history_changes_update AFTER UPDATE OF metadata ON session_history_catalog WHEN OLD.metadata IS NOT NEW.metadata BEGIN
 INSERT INTO session_catalog_changes(namespace,session_key,read_scope) VALUES(semon_namespace(),NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_history_changes_delete AFTER DELETE ON session_history_catalog BEGIN
 INSERT INTO session_catalog_changes(namespace,session_key,read_scope) VALUES(semon_namespace(),OLD.session_key,'retained_history');
END;

CREATE TRIGGER IF NOT EXISTS catalog_changes_insert AFTER INSERT ON session_catalog BEGIN
 INSERT INTO session_catalog_changes(namespace,session_key,read_scope) VALUES(NEW.namespace,NEW.session_key,'current'),(NEW.namespace,NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_changes_update AFTER UPDATE OF metadata ON session_catalog WHEN OLD.metadata IS NOT NEW.metadata BEGIN
 INSERT INTO session_catalog_changes(namespace,session_key,read_scope) VALUES(NEW.namespace,NEW.session_key,'current'),(NEW.namespace,NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_changes_delete AFTER DELETE ON session_catalog BEGIN
 INSERT INTO session_catalog_changes(namespace,session_key,read_scope) VALUES(OLD.namespace,OLD.session_key,'current'),(OLD.namespace,OLD.session_key,'retained_history');
END;

INSERT OR IGNORE INTO meta(namespace,key,value) VALUES('','catalog_changes_epoch',lower(hex(randomblob(32))));
"#;

fn scope_key(scope: CatalogReadScope) -> &'static str {
    match scope {
        CatalogReadScope::Current => "current",
        CatalogReadScope::RetainedHistory => "retained_history",
    }
}

/// Enrollment owns Source cursors; DDL owns only the physical stream epoch.
/// The caller holds the enrollment transaction. Validate binding, registry and
/// both existing values before writing either missing floor. Re-enrollment must
/// not reset compaction progress, including through an already-open connection.
pub(crate) fn initialize_source_floors(
    tx: &rusqlite::Transaction<'_>,
    namespace: &crate::storage::LogicalSourceNamespace,
) -> Result<(), StorageError> {
    let bound: String = tx
        .query_row("SELECT semon_namespace()", [], |r| r.get(0))
        .map_err(failed)?;
    if bound != namespace.as_str() {
        return Err(StorageError::NamespaceMismatch);
    }
    let valid: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_namespaces WHERE namespace=?1 AND admission_generation>0 AND format_version=1)",
        [namespace.as_str()], |r| r.get(0),
    ).map_err(failed)?;
    if !valid {
        return Err(StorageError::NamespaceUnadmitted);
    }
    let mut missing = Vec::new();
    for key in [
        "catalog_changes_floor_current",
        "catalog_changes_floor_retained_history",
    ] {
        let value: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)?;
        if let Some(value) = value {
            if !value
                .parse::<i64>()
                .is_ok_and(|n| n >= 0 && value == n.to_string())
            {
                return Err(StorageError::NamespaceUpgradePending);
            }
        } else {
            missing.push(key);
        }
    }
    for key in missing {
        tx.execute(
            "INSERT INTO meta(namespace,key,value) VALUES(semon_namespace(),?1,'0')",
            [key],
        )
        .map_err(failed)?;
    }
    Ok(())
}

pub(super) fn compact(
    connection: &mut Connection,
    scope: CatalogReadScope,
    epoch: &str,
    acknowledged: u64,
    limit: usize,
) -> Result<CatalogChangeCompaction, StorageError> {
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(failed)?;
    if !crate::events::store::current_contract(&tx).map_err(failed)? {
        return Err(StorageError::Unavailable);
    }
    let version: String = tx
        .query_row(
            "SELECT value FROM meta WHERE namespace=semon_namespace() AND key='catalog_version'",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if version != CATALOG_VERSION.to_string() {
        return Err(StorageError::Unavailable);
    }
    let current: String = tx
        .query_row(
            "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if current != epoch {
        return Err(StorageError::ScopeChanged);
    }
    let key = scope_key(scope);
    let floor_key = format!("catalog_changes_floor_{key}");
    let floor: u64 = tx
        .query_row(
            "SELECT value FROM meta WHERE namespace=semon_namespace() AND key=?1",
            [&floor_key],
            |r| r.get::<_, String>(0),
        )
        .map_err(failed)?
        .parse()
        .map_err(|_| StorageError::Unavailable)?;
    let head: i64 = tx
        .query_row(
            "SELECT coalesce(max(revision),0) FROM main.session_catalog_changes WHERE namespace=semon_namespace() AND read_scope=?1",
            [key],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if acknowledged > head as u64 {
        return Err(StorageError::ScopeChanged);
    }
    let revisions = tx
        .prepare("SELECT revision FROM main.session_catalog_changes WHERE namespace=semon_namespace() AND read_scope=?1 AND revision<=?2 AND revision<?3 ORDER BY revision LIMIT ?4")
        .map_err(failed)?
        .query_map((key, acknowledged as i64, head, limit as i64 + 1), |r| r.get::<_, i64>(0))
        .map_err(failed)?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
    let removed = revisions.len().min(limit);
    let through = revisions
        .iter()
        .take(limit)
        .next_back()
        .map(|revision| *revision as u64)
        .unwrap_or(floor);
    if removed > 0 {
        tx.execute(
            "DELETE FROM main.session_catalog_changes WHERE namespace=semon_namespace() AND read_scope=?1 AND revision<=?2",
            (key, through as i64),
        )
        .map_err(failed)?;
        tx.execute(
            "UPDATE main.meta SET value=?2 WHERE namespace=semon_namespace() AND key=?1",
            (&floor_key, through.to_string()),
        )
        .map_err(failed)?;
    }
    tx.commit().map_err(|_| StorageError::CommitUncertain)?;
    Ok(CatalogChangeCompaction {
        removed,
        compacted_through: through,
        head: head as u64,
        more: revisions.len() > limit,
    })
}

pub(super) fn read_snapshot(
    connection: &Connection,
    source_key: &str,
    scope: CatalogReadScope,
    expected: Option<&str>,
    after: u64,
    limit: usize,
) -> Result<CatalogChangePage, StorageError> {
    if !crate::events::store::current_contract(connection).map_err(failed)? {
        return Err(StorageError::Unavailable);
    }
    let meta = |key: &str| {
        connection
            .query_row(
                "SELECT value FROM meta WHERE namespace=semon_meta_scope(?1) AND key=?1",
                [key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(failed)
    };
    let valid = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
    if meta("catalog_version")?.as_deref() != Some(CATALOG_VERSION.to_string().as_str()) {
        return Err(StorageError::Unavailable);
    }
    let epoch = meta("catalog_changes_epoch")?
        .filter(|s| valid(s))
        .ok_or(StorageError::Unavailable)?;
    if expected.is_some_and(|value| value != epoch) {
        return Err(StorageError::ScopeChanged);
    }
    let generation = meta("catalog_generation")?
        .filter(|s| valid(s))
        .ok_or(StorageError::Unavailable)?;
    let scope_key = scope_key(scope);
    let floor: u64 = meta(&format!("catalog_changes_floor_{scope_key}"))?
        .ok_or(StorageError::Unavailable)?
        .parse()
        .map_err(|_| StorageError::Unavailable)?;
    if after < floor {
        return Err(StorageError::ScopeChanged);
    }
    let head: i64 = connection
        .query_row(
            "SELECT coalesce(max(revision),0) FROM session_catalog_changes WHERE read_scope=?1",
            [scope_key],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if after > head as u64 {
        return Err(StorageError::ScopeChanged);
    }
    let changes=connection.prepare("SELECT revision,CASE WHEN octet_length(session_key)<=4096 THEN session_key END,read_scope FROM session_catalog_changes WHERE read_scope=?1 AND revision>?2 ORDER BY revision LIMIT ?3").map_err(failed)?
        .query_map((scope_key,after as i64,limit as i64),|row|{
            let scope:String=row.get(2)?;
            Ok(CatalogChange{revision:row.get::<_,i64>(0)? as u64,catalog_key:row.get(1)?,read_scope:match scope.as_str(){"current"=>CatalogReadScope::Current,"retained_history"=>CatalogReadScope::RetainedHistory,_=>return Err(rusqlite::Error::InvalidQuery)}})
        }).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
    let next = changes.last().map_or(after, |change| change.revision);
    Ok(CatalogChangePage {
        api: 1,
        source_key: source_key.into(),
        stream_epoch: epoch,
        source_generation: generation,
        read_scope: scope,
        head: head as u64,
        compacted_through: floor,
        next_revision: next,
        caught_up: next == head as u64,
        changes,
    })
}

#[cfg(test)]
fn read(
    connection: &mut Connection,
    source_key: &str,
    scope: CatalogReadScope,
    expected: Option<&str>,
    after: u64,
    limit: usize,
) -> Result<CatalogChangePage, StorageError> {
    let tx = connection.transaction().map_err(failed)?;
    let page = read_snapshot(&tx, source_key, scope, expected, after, limit)?;
    tx.commit().map_err(failed)?;
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{LogicalSourceNamespace, StoreBackend, StoreConfig};
    struct Database {
        db: Option<Connection>,
        root: std::path::PathBuf,
    }
    impl std::ops::Deref for Database {
        type Target = Connection;
        fn deref(&self) -> &Connection {
            self.db.as_ref().unwrap()
        }
    }
    impl std::ops::DerefMut for Database {
        fn deref_mut(&mut self) -> &mut Connection {
            self.db.as_mut().unwrap()
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn database() -> Database {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "semon-changes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("index.sqlite3");
        drop(crate::events::store::current_search_test_connection(
            &path,
            &LogicalSourceNamespace::new("machine").unwrap(),
        ));
        Database {
            db: Some(reopen(&path)),
            root,
        }
    }
    fn reopen(path: &std::path::Path) -> Connection {
        let epoch = Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE namespace='' AND key='catalog_changes_epoch'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let config = StoreConfig {
            backend: StoreBackend::Sqlite {
                database: path.to_owned(),
            },
            namespace: LogicalSourceNamespace::new("machine").unwrap(),
            expected_stream_epoch: Some(epoch),
        };
        super::super::semantic::Stage::open(
            super::super::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection()
    }
    fn insert(db: &Connection, key: &str) {
        let row: crate::model::summary::CatalogRow = serde_json::from_value(serde_json::json!({
            "key":key,"name":key,"harness":"claude","kind":"session",
            "native_ids":[key],"parent":null,"parent_source":null,"repo":"project",
            "branch":null,"model":"","effort":null,"start":null,"last":0,
            "tokens":[0,0,0],"cost":crate::pricing::Cost::default(),"sources":[]
        }))
        .unwrap();
        db.execute("INSERT INTO main.session_catalog(namespace,session_key,last_ms,harness,metadata) VALUES(semon_namespace(),?1,0,'claude',?2)", (key, serde_json::to_string(&row).unwrap())).unwrap();
    }
    #[test]
    fn change_delivery_is_atomic_scoped_and_survives_replaced_database_epochs() {
        let mut db = database();
        insert(&db, "one");
        db.execute(
            "INSERT INTO main.session_history_catalog(namespace,session_key,metadata) SELECT namespace,session_key,metadata FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key='one'",
            [],
        )
        .unwrap();
        let page = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 1).unwrap();
        assert!(page.caught_up);
        assert_eq!(page.changes.len(), 1);
        assert_eq!(page.changes[0].catalog_key, "one");
        {
            let tx = db.transaction().unwrap();
            tx.execute(
                "UPDATE main.session_catalog SET metadata=json_set(metadata,'$.name','changed') WHERE namespace=semon_namespace() AND session_key='one'",
                [],
            )
            .unwrap();
            // A rejected catalog CAS/rolled-back publisher cannot emit a change.
            tx.rollback().unwrap();
        }
        let empty = read(
            &mut db,
            "machine",
            CatalogReadScope::Current,
            Some(&page.stream_epoch),
            page.next_revision,
            1,
        )
        .unwrap();
        assert!(empty.changes.is_empty());
        assert!(empty.caught_up);
        db.execute(
            "UPDATE main.session_catalog SET metadata=json_set(metadata,'$.name','changed') WHERE namespace=semon_namespace() AND session_key='one'",
            [],
        )
        .unwrap();
        let changed = read(
            &mut db,
            "machine",
            CatalogReadScope::Current,
            Some(&page.stream_epoch),
            page.next_revision,
            1,
        )
        .unwrap();
        assert_eq!(changed.changes.len(), 1);
        let history = read(
            &mut db,
            "machine",
            CatalogReadScope::RetainedHistory,
            Some(&page.stream_epoch),
            0,
            1,
        )
        .unwrap();
        assert_eq!(
            history.changes[0].read_scope,
            CatalogReadScope::RetainedHistory
        );
        assert!(!history.caught_up);
        let mut replaced = database();
        assert_eq!(
            read(
                &mut replaced,
                "machine",
                CatalogReadScope::Current,
                Some(&page.stream_epoch),
                0,
                1
            ),
            Err(StorageError::ScopeChanged)
        );
        db.execute("DELETE FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key='one'", [])
            .unwrap();
        let deleted = read(
            &mut db,
            "machine",
            CatalogReadScope::Current,
            Some(&page.stream_epoch),
            changed.next_revision,
            1,
        )
        .unwrap();
        assert_eq!(deleted.changes.len(), 1);
    }
    #[test]
    fn fixed_change_page_query_work_does_not_scan_unrelated_metadata() {
        use rusqlite::StatementStatus;
        let mut db = database();
        insert(&db, "selected");
        let steps = |db: &Connection| {
            let mut statement=db.prepare("SELECT revision,session_key FROM main.session_catalog_changes WHERE namespace=semon_namespace() AND read_scope='current' AND revision>0 ORDER BY revision LIMIT 1").unwrap();
            let key: String = statement.query_row([], |r| r.get(1)).unwrap();
            assert_eq!(key, "selected");
            statement.get_status(StatementStatus::VmStep)
        };
        let before = steps(&db);
        db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO main.session_catalog(namespace,session_key,last_ms,harness,metadata) SELECT semon_namespace(),'unrelated-'||x,0,'claude',json_set(metadata,'$.key','unrelated-'||x) FROM n,main.session_catalog WHERE namespace=semon_namespace() AND session_key='selected';").unwrap();
        assert_eq!(before, steps(&db));
        let page = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 1).unwrap();
        assert_eq!(page.changes[0].catalog_key, "selected");
        assert!(!page.caught_up);
        let mut query=db.prepare("EXPLAIN QUERY PLAN SELECT revision FROM main.session_catalog_changes WHERE namespace=semon_namespace() AND read_scope='current' AND revision>0 ORDER BY revision LIMIT 1").unwrap();
        let plan: String = query.query_row([], |r| r.get(3)).unwrap();
        assert!(plan.contains("COVERING INDEX"), "{plan}");
    }

    #[test]
    fn acknowledged_compaction_is_bounded_scoped_and_rejects_lagging_cursors() {
        let mut db = database();
        for key in ["one", "two", "three", "four"] {
            insert(&db, key);
        }
        let page = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 256).unwrap();
        let first = compact(
            &mut db,
            CatalogReadScope::Current,
            &page.stream_epoch,
            page.head,
            2,
        )
        .unwrap();
        assert_eq!(first.removed, 2);
        assert_eq!(first.compacted_through, page.changes[1].revision);
        assert!(first.more);
        assert_eq!(
            read(
                &mut db,
                "machine",
                CatalogReadScope::Current,
                Some(&page.stream_epoch),
                0,
                1
            ),
            Err(StorageError::ScopeChanged)
        );
        let next = read(
            &mut db,
            "machine",
            CatalogReadScope::Current,
            Some(&page.stream_epoch),
            first.compacted_through,
            256,
        )
        .unwrap();
        assert_eq!(next.changes.len(), 2);
        assert_eq!(next.compacted_through, first.compacted_through);
        // A lost compaction reply can be retried without deleting its head or
        // another scope's hints. No replay of source content is involved.
        let second = compact(
            &mut db,
            CatalogReadScope::Current,
            &page.stream_epoch,
            page.head,
            2,
        )
        .unwrap();
        assert_eq!(second.removed, 1);
        assert!(!second.more);
        let duplicate = compact(
            &mut db,
            CatalogReadScope::Current,
            &page.stream_epoch,
            page.head,
            2,
        )
        .unwrap();
        assert_eq!(duplicate.removed, 0);
        assert_eq!(duplicate.head, page.head);
        assert_eq!(duplicate.compacted_through, second.compacted_through);
        let history = read(
            &mut db,
            "machine",
            CatalogReadScope::RetainedHistory,
            Some(&page.stream_epoch),
            0,
            256,
        )
        .unwrap();
        assert_eq!(history.changes.len(), 4);
        assert_eq!(history.compacted_through, 0);
        assert_eq!(
            compact(
                &mut db,
                CatalogReadScope::Current,
                &"f".repeat(64),
                page.head,
                2
            ),
            Err(StorageError::ScopeChanged)
        );
        assert_eq!(
            compact(
                &mut db,
                CatalogReadScope::Current,
                &page.stream_epoch,
                page.head + 1,
                2
            ),
            Err(StorageError::ScopeChanged)
        );
        let catalog_rows: i64 = db
            .query_row("SELECT count(*) FROM session_catalog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            catalog_rows, 4,
            "compaction cannot delete retained catalog metadata"
        );
    }

    #[test]
    fn failed_compaction_rolls_back_hints_and_cursor_floor_together() {
        let mut db = database();
        insert(&db, "one");
        insert(&db, "two");
        db.execute_batch("CREATE TRIGGER refuse_floor BEFORE UPDATE ON main.meta WHEN NEW.namespace='machine' AND NEW.key='catalog_changes_floor_current' BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
        let before = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 256).unwrap();
        assert_eq!(
            compact(
                &mut db,
                CatalogReadScope::Current,
                &before.stream_epoch,
                before.head,
                1
            ),
            Err(StorageError::Unavailable)
        );
        assert_eq!(
            read(
                &mut db,
                "machine",
                CatalogReadScope::Current,
                Some(&before.stream_epoch),
                0,
                256
            )
            .unwrap(),
            before
        );
    }

    #[test]
    fn compaction_floor_survives_restart_and_new_source_generation() {
        let mut db = database();
        let path = db.root.join("index.sqlite3");
        insert(&db, "one");
        insert(&db, "two");
        let page = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 256).unwrap();
        let compacted = compact(
            &mut db,
            CatalogReadScope::Current,
            &page.stream_epoch,
            page.head,
            256,
        )
        .unwrap();
        db.db.take().unwrap().close().unwrap();
        let mut restarted = reopen(&path);
        // Current startup must not reset the epoch or compaction floor.
        assert_eq!(
            read(
                &mut restarted,
                "machine",
                CatalogReadScope::Current,
                Some(&page.stream_epoch),
                0,
                256
            ),
            Err(StorageError::ScopeChanged)
        );
        restarted
            .execute(
                "UPDATE main.meta SET value=?1 WHERE namespace=semon_namespace() AND key='catalog_generation'",
                ["b".repeat(64)],
            )
            .unwrap();
        restarted
            .execute(
                "UPDATE main.session_catalog SET metadata=json_set(metadata,'$.name','changed') WHERE namespace=semon_namespace() AND session_key='one'",
                [],
            )
            .unwrap();
        let resumed = read(
            &mut restarted,
            "machine",
            CatalogReadScope::Current,
            Some(&page.stream_epoch),
            compacted.compacted_through,
            256,
        )
        .unwrap();
        assert_eq!(resumed.source_generation, "b".repeat(64));
        assert_eq!(resumed.stream_epoch, page.stream_epoch);
        assert_eq!(resumed.changes.len(), 2);
        assert_eq!(resumed.changes[1].catalog_key, "one");
        drop(restarted);
    }
}
