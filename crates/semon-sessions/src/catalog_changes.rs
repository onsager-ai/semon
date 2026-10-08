//! Durable bounded metadata change delivery for background aggregate consumers.
//! This library seam grants no source access or native control authority.
use crate::{
    CatalogIdentityError, CatalogReadScope, Options, events::EventCache,
    model::summary::CATALOG_VERSION,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS session_catalog_changes (
 revision INTEGER PRIMARY KEY AUTOINCREMENT,
 session_key TEXT NOT NULL,
 read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history'))
) STRICT;
CREATE INDEX IF NOT EXISTS session_catalog_change_order ON session_catalog_changes(read_scope,revision);
INSERT OR IGNORE INTO meta(key,value) VALUES('catalog_changes_epoch',lower(hex(randomblob(32))));
INSERT OR IGNORE INTO meta(key,value) VALUES('catalog_changes_floor_current','0'),('catalog_changes_floor_retained_history','0');
CREATE TRIGGER IF NOT EXISTS catalog_changes_insert AFTER INSERT ON session_catalog BEGIN
 INSERT INTO session_catalog_changes(session_key,read_scope) VALUES(NEW.session_key,'current'),(NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_changes_update AFTER UPDATE OF metadata ON session_catalog WHEN OLD.metadata IS NOT NEW.metadata BEGIN
 INSERT INTO session_catalog_changes(session_key,read_scope) VALUES(NEW.session_key,'current'),(NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_changes_delete AFTER DELETE ON session_catalog BEGIN
 INSERT INTO session_catalog_changes(session_key,read_scope) VALUES(OLD.session_key,'current'),(OLD.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_history_changes_insert AFTER INSERT ON session_history_catalog BEGIN
 INSERT INTO session_catalog_changes(session_key,read_scope) VALUES(NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_history_changes_update AFTER UPDATE OF metadata ON session_history_catalog WHEN OLD.metadata IS NOT NEW.metadata BEGIN
 INSERT INTO session_catalog_changes(session_key,read_scope) VALUES(NEW.session_key,'retained_history');
END;
CREATE TRIGGER IF NOT EXISTS catalog_history_changes_delete AFTER DELETE ON session_history_catalog BEGIN
 INSERT INTO session_catalog_changes(session_key,read_scope) VALUES(OLD.session_key,'retained_history');
END;
"#;

/// A changed key is a hydration hint, not a native identity or an authorization.
/// Consumers hydrate against this page's source generation and use absence or
/// retained lifecycle as a current-list tombstone. Repeated keys are idempotent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogChange {
    pub revision: u64,
    pub catalog_key: String,
    pub read_scope: CatalogReadScope,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogChangePage {
    pub api: u32,
    pub source_key: String,
    pub stream_epoch: String,
    pub source_generation: String,
    pub read_scope: CatalogReadScope,
    pub head: u64,
    pub compacted_through: u64,
    pub next_revision: u64,
    pub caught_up: bool,
    pub changes: Vec<CatalogChange>,
}

/// Result of one explicitly requested, bounded acknowledgement batch. Hosts
/// supply the least durable acknowledgement of every consumer of this scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogChangeCompaction {
    pub removed: usize,
    pub compacted_through: u64,
    pub head: u64,
    pub more: bool,
}

fn scope_key(scope: CatalogReadScope) -> &'static str {
    match scope {
        CatalogReadScope::Current => "current",
        CatalogReadScope::RetainedHistory => "retained_history",
    }
}

/// Compact at most 256 acknowledged hints without deleting catalog metadata or
/// native history. Authorize the source externally, as for change reads. This
/// never schedules background work. A lost reply is safe to repeat with the same
/// epoch and acknowledgement. Keep the newest scoped hint as the durable head.
pub fn compact_session_catalog_changes(
    options: &Options,
    scope: CatalogReadScope,
    stream_epoch: &str,
    acknowledged_through: u64,
    limit: usize,
) -> Result<CatalogChangeCompaction, CatalogIdentityError> {
    if limit == 0 || limit > 256 || acknowledged_through > i64::MAX as u64 {
        return Err(CatalogIdentityError::InvalidArguments);
    }
    let mut connection = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| CatalogIdentityError::Unavailable)?;
    connection
        .busy_timeout(Duration::from_millis(100))
        .map_err(|_| CatalogIdentityError::Unavailable)?;
    compact(
        &mut connection,
        scope,
        stream_epoch,
        acknowledged_through,
        limit,
    )
}

fn compact(
    connection: &mut Connection,
    scope: CatalogReadScope,
    epoch: &str,
    acknowledged: u64,
    limit: usize,
) -> Result<CatalogChangeCompaction, CatalogIdentityError> {
    let failed = |_: rusqlite::Error| CatalogIdentityError::Unavailable;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(failed)?;
    let version: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_version'",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if version != CATALOG_VERSION.to_string() {
        return Err(CatalogIdentityError::Unavailable);
    }
    let current: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_changes_epoch'",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if current != epoch {
        return Err(CatalogIdentityError::ScopeChanged);
    }
    let key = scope_key(scope);
    let floor_key = format!("catalog_changes_floor_{key}");
    let floor: u64 = tx
        .query_row("SELECT value FROM meta WHERE key=?1", [&floor_key], |r| {
            r.get::<_, String>(0)
        })
        .map_err(failed)?
        .parse()
        .map_err(|_| CatalogIdentityError::Unavailable)?;
    let head: i64 = tx
        .query_row(
            "SELECT coalesce(max(revision),0) FROM session_catalog_changes WHERE read_scope=?1",
            [key],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if acknowledged > head as u64 {
        return Err(CatalogIdentityError::ScopeChanged);
    }
    let revisions = tx
        .prepare("SELECT revision FROM session_catalog_changes WHERE read_scope=?1 AND revision<=?2 AND revision<?3 ORDER BY revision LIMIT ?4")
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
            "DELETE FROM session_catalog_changes WHERE read_scope=?1 AND revision<=?2",
            (key, through as i64),
        )
        .map_err(failed)?;
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            (&floor_key, through.to_string()),
        )
        .map_err(failed)?;
    }
    tx.commit().map_err(failed)?;
    Ok(CatalogChangeCompaction {
        removed,
        compacted_through: through,
        head: head as u64,
        more: revisions.len() > limit,
    })
}

/// Metadata-only source outbox read, independent of ViewerCore or native bytes.
/// Authorize the exact source externally. After the initial page, supply its
/// stream epoch; a replaced/rebuilt database must not reuse an old cursor.
pub fn session_catalog_changes(
    options: &Options,
    source_key: &str,
    scope: CatalogReadScope,
    stream_epoch: Option<&str>,
    after: u64,
    limit: usize,
) -> Result<CatalogChangePage, CatalogIdentityError> {
    if source_key.is_empty()
        || source_key.len() > 4096
        || limit == 0
        || limit > 256
        || after > i64::MAX as u64
        || (after > 0 && stream_epoch.is_none())
    {
        return Err(CatalogIdentityError::InvalidArguments);
    }
    let mut connection = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| CatalogIdentityError::Unavailable)?;
    connection
        .busy_timeout(Duration::from_millis(100))
        .map_err(|_| CatalogIdentityError::Unavailable)?;
    read(
        &mut connection,
        source_key,
        scope,
        stream_epoch,
        after,
        limit,
    )
}
fn read(
    connection: &mut Connection,
    source_key: &str,
    scope: CatalogReadScope,
    expected: Option<&str>,
    after: u64,
    limit: usize,
) -> Result<CatalogChangePage, CatalogIdentityError> {
    let failed = |_: rusqlite::Error| CatalogIdentityError::Unavailable;
    let tx = connection.transaction().map_err(failed)?;
    let meta = |key: &str| {
        tx.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(failed)
    };
    let valid = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
    if meta("catalog_version")?.as_deref() != Some(CATALOG_VERSION.to_string().as_str()) {
        return Err(CatalogIdentityError::Unavailable);
    }
    let epoch = meta("catalog_changes_epoch")?
        .filter(|s| valid(s))
        .ok_or(CatalogIdentityError::Unavailable)?;
    if expected.is_some_and(|value| value != epoch) {
        return Err(CatalogIdentityError::ScopeChanged);
    }
    let generation = meta("catalog_generation")?
        .filter(|s| valid(s))
        .ok_or(CatalogIdentityError::Unavailable)?;
    let scope_key = scope_key(scope);
    let floor: u64 = meta(&format!("catalog_changes_floor_{scope_key}"))?
        .ok_or(CatalogIdentityError::Unavailable)?
        .parse()
        .map_err(|_| CatalogIdentityError::Unavailable)?;
    if after < floor {
        return Err(CatalogIdentityError::ScopeChanged);
    }
    let head: i64 = tx
        .query_row(
            "SELECT coalesce(max(revision),0) FROM session_catalog_changes WHERE read_scope=?1",
            [scope_key],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if after > head as u64 {
        return Err(CatalogIdentityError::ScopeChanged);
    }
    let changes=tx.prepare("SELECT revision,CASE WHEN octet_length(session_key)<=4096 THEN session_key END,read_scope FROM session_catalog_changes WHERE read_scope=?1 AND revision>?2 ORDER BY revision LIMIT ?3").map_err(failed)?
        .query_map((scope_key,after as i64,limit as i64),|row|{
            let scope:String=row.get(2)?;
            Ok(CatalogChange{revision:row.get::<_,i64>(0)? as u64,catalog_key:row.get(1)?,read_scope:match scope.as_str(){"current"=>CatalogReadScope::Current,"retained_history"=>CatalogReadScope::RetainedHistory,_=>return Err(rusqlite::Error::InvalidQuery)}})
        }).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
    let next = changes.last().map_or(after, |change| change.revision);
    tx.commit().map_err(failed)?;
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
mod tests {
    use super::*;
    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db);
        db
    }
    fn initialize(db: &Connection) {
        db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY,metadata TEXT);CREATE TABLE session_history_catalog(session_key TEXT PRIMARY KEY,metadata TEXT);").unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute(
            "INSERT INTO meta VALUES('catalog_version',?1)",
            [CATALOG_VERSION.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO meta VALUES('catalog_generation',?1)",
            ["a".repeat(64)],
        )
        .unwrap();
    }
    #[test]
    fn change_delivery_is_atomic_scoped_and_survives_replaced_database_epochs() {
        let mut db = database();
        db.execute("INSERT INTO session_catalog VALUES('one','original')", [])
            .unwrap();
        db.execute(
            "INSERT INTO session_history_catalog VALUES('one','original')",
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
                "UPDATE session_catalog SET metadata='changed' WHERE session_key='one'",
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
            "UPDATE session_catalog SET metadata='changed' WHERE session_key='one'",
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
            Err(CatalogIdentityError::ScopeChanged)
        );
        db.execute("DELETE FROM session_catalog WHERE session_key='one'", [])
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
        db.execute(
            "INSERT INTO session_catalog VALUES('selected','original')",
            [],
        )
        .unwrap();
        let steps = |db: &Connection| {
            let mut statement=db.prepare("SELECT revision,session_key FROM session_catalog_changes WHERE read_scope='current' AND revision>0 ORDER BY revision LIMIT 1").unwrap();
            let key: String = statement.query_row([], |r| r.get(1)).unwrap();
            assert_eq!(key, "selected");
            statement.get_status(StatementStatus::VmStep)
        };
        let before = steps(&db);
        db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO session_catalog SELECT 'unrelated-'||x,'metadata' FROM n;").unwrap();
        assert_eq!(before, steps(&db));
        let page = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 1).unwrap();
        assert_eq!(page.changes[0].catalog_key, "selected");
        assert!(!page.caught_up);
        let mut query=db.prepare("EXPLAIN QUERY PLAN SELECT revision FROM session_catalog_changes WHERE read_scope='current' AND revision>0 ORDER BY revision LIMIT 1").unwrap();
        let plan: String = query.query_row([], |r| r.get(3)).unwrap();
        assert!(plan.contains("COVERING INDEX"), "{plan}");
    }

    #[test]
    fn acknowledged_compaction_is_bounded_scoped_and_rejects_lagging_cursors() {
        let mut db = database();
        for key in ["one", "two", "three", "four"] {
            db.execute("INSERT INTO session_catalog VALUES(?1,'metadata')", [key])
                .unwrap();
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
            Err(CatalogIdentityError::ScopeChanged)
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
            Err(CatalogIdentityError::ScopeChanged)
        );
        assert_eq!(
            compact(
                &mut db,
                CatalogReadScope::Current,
                &page.stream_epoch,
                page.head + 1,
                2
            ),
            Err(CatalogIdentityError::ScopeChanged)
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
        db.execute_batch("INSERT INTO session_catalog VALUES('one','metadata'),('two','metadata'); CREATE TRIGGER refuse_floor BEFORE UPDATE ON meta WHEN NEW.key='catalog_changes_floor_current' BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
        let before = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 256).unwrap();
        assert_eq!(
            compact(
                &mut db,
                CatalogReadScope::Current,
                &before.stream_epoch,
                before.head,
                1
            ),
            Err(CatalogIdentityError::Unavailable)
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
        let path = std::env::temp_dir().join(format!(
            "semon-catalog-compaction-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut db = Connection::open(&path).unwrap();
        initialize(&db);
        db.execute_batch(
            "INSERT INTO session_catalog VALUES('one','metadata'),('two','metadata');",
        )
        .unwrap();
        let page = read(&mut db, "machine", CatalogReadScope::Current, None, 0, 256).unwrap();
        let compacted = compact(
            &mut db,
            CatalogReadScope::Current,
            &page.stream_epoch,
            page.head,
            256,
        )
        .unwrap();
        drop(db);
        let mut restarted = Connection::open(&path).unwrap();
        // Migration/startup must not reset the epoch or compaction floor.
        restarted.execute_batch(SCHEMA).unwrap();
        assert_eq!(
            read(
                &mut restarted,
                "machine",
                CatalogReadScope::Current,
                Some(&page.stream_epoch),
                0,
                256
            ),
            Err(CatalogIdentityError::ScopeChanged)
        );
        restarted
            .execute(
                "UPDATE meta SET value=?1 WHERE key='catalog_generation'",
                ["b".repeat(64)],
            )
            .unwrap();
        restarted
            .execute(
                "UPDATE session_catalog SET metadata='changed' WHERE session_key='one'",
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
        std::fs::remove_file(path).unwrap();
    }
}
