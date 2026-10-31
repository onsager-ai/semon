//! Partial publication keeps every unmentioned Current owner. Its immutable
//! effects are copied by keyset page inside SQL, before the mentioned rows are
//! staged. No Raw, file or hash work runs here; each page checks the deadline.
use super::*;

const PAGE: i64 = 64;

// Owner-scoped generation tables. Columns exclude namespace and generation.
const OWNED: &[(&str, &str)] = &[
    (
        "catalog_rows",
        "key_digest,session_key,parent_key,metadata,last_ms,harness,repo,model",
    ),
    (
        "catalog_sources",
        "key_digest,session_key,path_digest,source_path,metadata,native_digest,folded_native_id",
    ),
    ("catalog_links", "key_digest,session_key,peer_digest,peer"),
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
pub(in crate::storage::postgres) fn unmentioned(
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
            break;
        }
        let connected: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=ANY($3::bytea[]) AND source_path=ANY($4::bytea[]))", &[&ns, &previous.as_bytes(), &keys, &claimed]).map_err(sqlstate::error)?.get(0);
        if connected {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for (table, columns) in OWNED {
            tx.check()?;
            tx.driver.execute(&format!("INSERT INTO semon_pg_native_{table}(namespace,generation,{columns}) SELECT namespace,$2::bytea,{columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND key_digest=ANY($4::bytea[])"), &[&ns, &generation.as_bytes(), &previous.as_bytes(), &keys]).map_err(sqlstate::error)?;
        }
        for (table, columns) in SOURCED {
            tx.check()?;
            tx.driver.execute(&format!("INSERT INTO semon_pg_native_{table}(namespace,generation,{columns}) SELECT namespace,$2::bytea,{columns} FROM semon_pg_native_{table} p WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND path_digest IN (SELECT path_digest FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND key_digest=ANY($4::bytea[])) ON CONFLICT DO NOTHING"), &[&ns, &generation.as_bytes(), &previous.as_bytes(), &keys]).map_err(sqlstate::error)?;
        }
        // Every carried Source keeps the proof that admitted it.
        let unproven: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources s WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.key_digest=ANY($3::bytea[]) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_proofs p WHERE p.namespace_digest=s.namespace_digest AND p.namespace=s.namespace AND p.generation=s.generation AND p.path_digest=s.path_digest))", &[&ns, &generation.as_bytes(), &keys]).map_err(sqlstate::error)?.get(0);
        if unproven {
            return Err(StorageError::ScopeChanged);
        }
        carried += keys.len() as u64;
        after = keys.last().cloned();
    }
    history(tx, generation, previous, mentioned)?;
    all_history_proofs(tx, generation)?;
    Ok(carried)
}

/// Projects retained proof references for a bounded page of History owners in
/// the successor. The History source rows identify the exact owner and source;
/// the referenced Native tuple is copied without resolving a newer path head.
fn history_proofs(
    tx: &mut PgTxn<'_>,
    generation: &str,
    digests: &[Vec<u8>],
) -> Result<(), StorageError> {
    if digests.is_empty() {
        return Ok(());
    }
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) SELECT DISTINCT p.namespace,$2::bytea,p.path_digest,p.source_path,p.page_generation,p.usage_generation,p.usage_ready FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_sources s ON s.namespace_digest=o.namespace_digest AND s.namespace=o.namespace AND s.generation=o.owner_generation AND s.key_digest=o.key_digest AND s.session_key=o.session_key JOIN semon_pg_native_catalog_proofs p ON p.namespace_digest=s.namespace_digest AND p.namespace=s.namespace AND p.generation=s.generation AND p.path_digest=s.path_digest AND p.source_path=s.source_path WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.key_digest=ANY($3::bytea[]) ON CONFLICT DO NOTHING", &[&ns, &generation.as_bytes(), &digests]).map_err(sqlstate::error)?;
    tx.check()
}

/// Project every final successor History owner after carry has completed. This
/// includes linked composite owners whose Current sources already have staged
/// proofs and whose retained source rows contain only the missing sources.
fn all_history_proofs(tx: &mut PgTxn<'_>, generation: &str) -> Result<(), StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let mut after: Option<Vec<u8>> = None;
    loop {
        tx.check()?;
        let page = tx.driver.query("SELECT key_digest FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND ($3::bytea IS NULL OR key_digest>$3) ORDER BY key_digest LIMIT $4", &[&ns, &generation.as_bytes(), &after, &PAGE]).map_err(sqlstate::error)?;
        let digests = page.iter().map(|row| row.get(0)).collect::<Vec<Vec<u8>>>();
        let Some(last) = digests.last().cloned() else {
            return Ok(());
        };
        history_proofs(tx, generation, &digests)?;
        after = Some(last);
        if digests.len() < PAGE as usize {
            return Ok(());
        }
    }
}

// Retained history stays owned by its original generation. The successor only
// maps every unmentioned key, including owners no longer Current.
fn history(
    tx: &mut PgTxn<'_>,
    generation: &str,
    previous: &str,
    mentioned: &[Vec<u8>],
) -> Result<(), StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let mut after: Option<Vec<u8>> = None;
    loop {
        tx.check()?;
        let page = tx.driver.query("WITH page AS MATERIALIZED (SELECT key_digest FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND NOT key_digest=ANY($4::bytea[]) AND ($5::bytea IS NULL OR key_digest>$5) ORDER BY key_digest LIMIT $6), copied AS (INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model,query_activity,query_active,parent_key) SELECT namespace,$2::bytea,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model,query_activity,query_active,parent_key FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND key_digest IN (SELECT key_digest FROM page) ON CONFLICT DO NOTHING) SELECT key_digest FROM page ORDER BY key_digest", &[&ns, &generation.as_bytes(), &previous.as_bytes(), &mentioned, &after, &PAGE]).map_err(sqlstate::error)?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after = Some(last.get(0));
        let digests = page.iter().map(|row| row.get(0)).collect::<Vec<Vec<u8>>>();
        retained_search(tx, generation, previous, &digests)?;
    }
}

/// Retained-history search inputs follow their history owner into the
/// successor; memberships carry no scope and posting reads filter by the
/// scope document. Current owners' rows were already carried exactly.
pub(super) fn retained_search(
    tx: &mut PgTxn<'_>,
    generation: &str,
    previous: &str,
    digests: &[Vec<u8>],
) -> Result<(), StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let scope = crate::catalog_search::scope_key(crate::CatalogReadScope::RetainedHistory);
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_search_documents(namespace,generation,scope,key_digest,session_key,fields,complete) SELECT namespace,$2::bytea,scope,key_digest,session_key,fields,complete FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND scope=$4 AND key_digest=ANY($5::bytea[]) ON CONFLICT DO NOTHING", &[&ns, &generation.as_bytes(), &previous.as_bytes(), &scope.as_bytes(), &digests]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_search_memberships(namespace,generation,key_digest,session_key,gram,last_ms) SELECT namespace,$2::bytea,key_digest,session_key,gram,last_ms FROM semon_pg_native_catalog_search_memberships WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND key_digest=ANY($4::bytea[]) ON CONFLICT DO NOTHING", &[&ns, &generation.as_bytes(), &previous.as_bytes(), &digests]).map_err(sqlstate::error)?;
    tx.check()
}
