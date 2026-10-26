//! Partial publication keeps every unmentioned Current owner. Its immutable
//! effects are copied by keyset page inside SQL, before the mentioned rows are
//! staged. No Raw, file or hash work runs here; each page checks the deadline.
use super::*;

const PAGE: i64 = 64;

// Owner-scoped generation tables. Columns exclude namespace and generation.
const OWNED: &[(&str, &str)] = &[
    (
        "catalog_rows",
        "key_digest,session_key,metadata,last_ms,harness,repo,model",
    ),
    (
        "catalog_sources",
        "key_digest,session_key,path_digest,source_path,metadata",
    ),
    (
        "catalog_recipe_headers",
        "key_digest,session_key,recipe_generation,source_generation,version,total",
    ),
    (
        "catalog_recipe_rows",
        "key_digest,session_key,slot,metadata",
    ),
    (
        "catalog_trace_headers",
        "key_digest,session_key,trace_generation,source_generation,metadata",
    ),
    (
        "catalog_trace_turns",
        "key_digest,session_key,turn_digest,turn_id,start_id,metadata",
    ),
    (
        "catalog_trace_edges",
        "key_digest,session_key,edge_digest,edge_id,metadata",
    ),
    (
        "catalog_trace_bookmarks",
        "key_digest,session_key,target_digest,target,edge_digest,edge_id",
    ),
    (
        "catalog_search_documents",
        "scope,key_digest,session_key,fields,complete",
    ),
    (
        "catalog_search_memberships",
        "key_digest,session_key,gram,last_ms",
    ),
    (
        "catalog_history_owners",
        "key_digest,session_key,owner_generation,original",
    ),
    (
        "catalog_history_headers",
        "key_digest,session_key,recipe_generation,source_generation,version,total,incomplete,metadata,original_header",
    ),
    (
        "catalog_history_slots",
        "key_digest,session_key,slot,metadata",
    ),
    (
        "catalog_history_sources",
        "key_digest,session_key,path_digest,source_path,metadata",
    ),
    (
        "catalog_history_progress",
        "key_digest,session_key,metadata",
    ),
];

// Source-scoped proof inputs of the carried owners.
const SOURCED: &[(&str, &str)] = &[
    (
        "catalog_proofs",
        "path_digest,source_path,page_generation,usage_generation,usage_ready",
    ),
    (
        "catalog_dependencies",
        "path_digest,source_path,harness_digest,harness,native_digest,native_id",
    ),
];

/// Copies previous Current owners absent from `mentioned`. An unmentioned
/// owner reading a claimed Source is connected to this change: the semantic
/// producer must republish it, so the request refuses instead of keeping a
/// stale owner or silently dropping it.
pub(super) fn unmentioned(
    tx: &mut PgTxn<'_>,
    generation: &str,
    previous: &str,
    mentioned: &[Vec<u8>],
    claimed: &[Vec<u8>],
) -> Result<u64, StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let mut after: Option<Vec<u8>> = None;
    let mut carried = 0;
    loop {
        tx.check()?;
        let page = tx.driver.query("SELECT key_digest FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND NOT key_digest=ANY($3::bytea[]) AND ($4::bytea IS NULL OR key_digest>$4) ORDER BY key_digest LIMIT $5", &[&ns, &previous.as_bytes(), &mentioned, &after, &PAGE]).map_err(sqlstate::error)?;
        let keys = page.iter().map(|row| row.get(0)).collect::<Vec<Vec<u8>>>();
        if keys.is_empty() {
            return Ok(carried);
        }
        let connected: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=ANY($3::bytea[]) AND source_path=ANY($4::bytea[]))", &[&ns, &previous.as_bytes(), &keys, &claimed]).map_err(sqlstate::error)?.get(0);
        if connected {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for (table, columns) in OWNED {
            tx.check()?;
            tx.driver.execute(&format!("INSERT INTO semon_pg_native_{table}(namespace,generation,{columns}) SELECT namespace,$2,{columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND key_digest=ANY($4::bytea[])"), &[&ns, &generation.as_bytes(), &previous.as_bytes(), &keys]).map_err(sqlstate::error)?;
        }
        for (table, columns) in SOURCED {
            tx.check()?;
            tx.driver.execute(&format!("INSERT INTO semon_pg_native_{table}(namespace,generation,{columns}) SELECT namespace,$2,{columns} FROM semon_pg_native_{table} p WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND path_digest IN (SELECT path_digest FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND key_digest=ANY($4::bytea[])) ON CONFLICT DO NOTHING"), &[&ns, &generation.as_bytes(), &previous.as_bytes(), &keys]).map_err(sqlstate::error)?;
        }
        // Every carried Source keeps the proof that admitted it.
        let unproven: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources s WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.key_digest=ANY($3::bytea[]) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_proofs p WHERE p.namespace_digest=s.namespace_digest AND p.namespace=s.namespace AND p.generation=s.generation AND p.path_digest=s.path_digest))", &[&ns, &generation.as_bytes(), &keys]).map_err(sqlstate::error)?.get(0);
        if unproven {
            return Err(StorageError::ScopeChanged);
        }
        carried += keys.len() as u64;
        after = keys.last().cloned();
    }
}
