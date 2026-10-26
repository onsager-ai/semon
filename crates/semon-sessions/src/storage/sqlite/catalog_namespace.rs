//! Namespace binding and current Catalog reader/writer fences.
use super::failed;
use crate::storage::{LogicalSourceNamespace, StorageError};
use rusqlite::{Connection, functions::FunctionFlags};

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
    "session_descriptions",
    "session_search_coverage",
    "session_search_documents",
    "session_search_memberships",
    "session_search_counts",
    "session_catalog_changes",
    "session_catalog_invalidations",
    "catalog_backfill_sources",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogAssignmentProgress {
    pub table: String,
    pub copied: usize,
    pub complete: bool,
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub(crate) fn register(
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

/// Every unqualified native SELECT resolves a connection-local view with an
/// explicit namespace predicate. Indexed Catalog ranges use main plus the same
/// predicate, retaining namespace-leading covering indexes.
pub(crate) fn scope_readers(db: &Connection) -> Result<bool, StorageError> {
    for table in QUALIFIED_CATALOG_TABLES {
        let current: bool = if *table == "session_catalog_changes" {
            // Revisions order the shared stream globally; namespace owns each
            // row and leads the delivery index, rather than the revision PK.
            db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND type='TEXT' AND [notnull]=1) AND EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='revision' AND type='INTEGER' AND pk=1) AND EXISTS(SELECT 1 FROM pragma_index_info('session_catalog_change_order','main') WHERE seqno=0 AND name='namespace')", [table], |row|row.get(0)).map_err(failed)?
        } else {
            db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [table], |row| row.get(0)).map_err(failed)?
        };
        if !current {
            return Err(StorageError::NamespaceUpgradePending);
        }
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
    db.execute_batch("CREATE TEMP VIEW IF NOT EXISTS meta AS SELECT namespace,key,value FROM main.meta WHERE namespace=semon_meta_scope(key);").map_err(failed)?;
    Ok(true)
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
