//! Partial publication keeps every unmentioned Current owner. Its immutable
//! effects are copied by keyset page inside SQL, before the mentioned rows are
//! staged. No Raw, file or hash work runs here; each page checks the deadline.
use super::*;

const PAGE: i64 = 64;

const TRACE_MEMBERS: &[(&str, &str)] = &[
    (
        "catalog_trace_turns",
        "turn_digest,turn_id,start_id,metadata",
    ),
    ("catalog_trace_edges", "edge_digest,edge_id,metadata"),
    (
        "catalog_trace_bookmarks",
        "target_digest,target,edge_digest,edge_id",
    ),
];

/// Copies complete trace families into the successor generation. A trace
/// already staged there wins as a genuine successor observation; its children
/// must never be mixed with an older family. History creation supplies the
/// source cohort; later carry preserves the header's existing source generation.
pub(super) fn trace_families(
    tx: &mut PgTxn<'_>,
    generation: &str,
    previous: &str,
    owners: &[(Vec<u8>, Vec<u8>)],
    source_generation: Option<&str>,
) -> Result<(), StorageError> {
    if owners.is_empty() {
        return Ok(());
    }
    if generation == previous {
        return Err(StorageError::InvalidArguments);
    }
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let generation_bytes = generation.as_bytes().to_vec();
    let previous_bytes = previous.as_bytes().to_vec();
    let digests = owners.iter().map(|owner| owner.0.clone()).collect::<Vec<_>>();
    let keys = owners.iter().map(|owner| owner.1.clone()).collect::<Vec<_>>();
    let source_filter = if source_generation.is_some() {
        " AND h.source_generation=$6"
    } else {
        ""
    };
    let mut args: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![
        &ns,
        &generation_bytes,
        &previous_bytes,
        &digests,
        &keys,
    ];
    let expected_source = source_generation.map(|value| value.as_bytes().to_vec());
    if let Some(expected_source) = &expected_source {
        args.push(expected_source);
    }
    tx.check()?;
    let state_sql = format!(
        "SELECT
            EXISTS(
                SELECT 1 FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                JOIN semon_pg_native_catalog_trace_turns t
                  ON t.namespace_digest=sha256($1::bytea) AND t.namespace=$1 AND t.generation=$2
                 AND t.key_digest=requested.key_digest AND t.session_key=requested.session_key
                WHERE NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers h
                    WHERE h.namespace_digest=t.namespace_digest AND h.namespace=t.namespace
                      AND h.generation=t.generation AND h.key_digest=t.key_digest AND h.session_key=t.session_key
                )
            ) OR EXISTS(
                SELECT 1 FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                JOIN semon_pg_native_catalog_trace_edges e
                  ON e.namespace_digest=sha256($1::bytea) AND e.namespace=$1 AND e.generation=$2
                 AND e.key_digest=requested.key_digest AND e.session_key=requested.session_key
                WHERE NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers h
                    WHERE h.namespace_digest=e.namespace_digest AND h.namespace=e.namespace
                      AND h.generation=e.generation AND h.key_digest=e.key_digest AND h.session_key=e.session_key
                )
            ) OR EXISTS(
                SELECT 1 FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                JOIN semon_pg_native_catalog_trace_bookmarks b
                  ON b.namespace_digest=sha256($1::bytea) AND b.namespace=$1 AND b.generation=$2
                 AND b.key_digest=requested.key_digest AND b.session_key=requested.session_key
                WHERE NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers h
                    WHERE h.namespace_digest=b.namespace_digest AND h.namespace=b.namespace
                      AND h.generation=b.generation AND h.key_digest=b.key_digest AND h.session_key=b.session_key
                )
            ) OR EXISTS(
                SELECT 1 FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                JOIN semon_pg_native_catalog_trace_turns t
                  ON t.namespace_digest=sha256($1::bytea) AND t.namespace=$1 AND t.generation=$3
                 AND t.key_digest=requested.key_digest AND t.session_key=requested.session_key
                WHERE NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers source
                    WHERE source.namespace_digest=t.namespace_digest AND source.namespace=t.namespace
                      AND source.generation=t.generation AND source.key_digest=t.key_digest AND source.session_key=t.session_key
                ) AND NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers target
                    WHERE target.namespace_digest=t.namespace_digest AND target.namespace=t.namespace
                      AND target.generation=$2 AND target.key_digest=t.key_digest AND target.session_key=t.session_key
                )
            ) OR EXISTS(
                SELECT 1 FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                JOIN semon_pg_native_catalog_trace_edges e
                  ON e.namespace_digest=sha256($1::bytea) AND e.namespace=$1 AND e.generation=$3
                 AND e.key_digest=requested.key_digest AND e.session_key=requested.session_key
                WHERE NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers source
                    WHERE source.namespace_digest=e.namespace_digest AND source.namespace=e.namespace
                      AND source.generation=e.generation AND source.key_digest=e.key_digest AND source.session_key=e.session_key
                ) AND NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers target
                    WHERE target.namespace_digest=e.namespace_digest AND target.namespace=e.namespace
                      AND target.generation=$2 AND target.key_digest=e.key_digest AND target.session_key=e.session_key
                )
            ) OR EXISTS(
                SELECT 1 FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                JOIN semon_pg_native_catalog_trace_bookmarks b
                  ON b.namespace_digest=sha256($1::bytea) AND b.namespace=$1 AND b.generation=$3
                 AND b.key_digest=requested.key_digest AND b.session_key=requested.session_key
                WHERE NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers source
                    WHERE source.namespace_digest=b.namespace_digest AND source.namespace=b.namespace
                      AND source.generation=b.generation AND source.key_digest=b.key_digest AND source.session_key=b.session_key
                ) AND NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers target
                    WHERE target.namespace_digest=b.namespace_digest AND target.namespace=b.namespace
                      AND target.generation=$2 AND target.key_digest=b.key_digest AND target.session_key=b.session_key
                )
            ),
            EXISTS(
                SELECT 1 FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                JOIN semon_pg_native_catalog_trace_headers h
                  ON h.namespace_digest=sha256($1::bytea) AND h.namespace=$1 AND h.generation=$3
                 AND h.key_digest=requested.key_digest AND h.session_key=requested.session_key
                WHERE NOT EXISTS(
                    SELECT 1 FROM semon_pg_native_catalog_trace_headers target
                    WHERE target.namespace_digest=h.namespace_digest AND target.namespace=h.namespace
                      AND target.generation=$2 AND target.key_digest=h.key_digest AND target.session_key=h.session_key
                ){source_filter} AND octet_length(h.metadata)>1048576
            ),
            count(*)
            FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
            JOIN semon_pg_native_catalog_trace_headers h
              ON h.namespace_digest=sha256($1::bytea) AND h.namespace=$1 AND h.generation=$3
             AND h.key_digest=requested.key_digest AND h.session_key=requested.session_key
            WHERE NOT EXISTS(
                SELECT 1 FROM semon_pg_native_catalog_trace_headers target
                WHERE target.namespace_digest=h.namespace_digest AND target.namespace=h.namespace
                  AND target.generation=$2 AND target.key_digest=h.key_digest AND target.session_key=h.session_key
            ){source_filter}"
    );
    let state = tx.driver.query_one(&state_sql, &args).map_err(sqlstate::error)?;
    if state.get::<_, bool>(0) {
        return Err(StorageError::ScopeChanged);
    }
    if state.get::<_, bool>(1) {
        return Err(StorageError::Busy);
    }
    let eligible: i64 = state.get(2);
    for (table, columns) in TRACE_MEMBERS {
        let selected_columns = columns
            .split(',')
            .map(|column| format!("m.{column}"))
            .collect::<Vec<_>>()
            .join(",");
        tx.check()?;
        tx.driver.execute(
            &format!(
                "INSERT INTO semon_pg_native_{table}(namespace,generation,key_digest,session_key,{columns})
                 SELECT m.namespace,$2,m.key_digest,m.session_key,{selected_columns}
                 FROM semon_pg_native_{table} m
                 JOIN unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
                   ON requested.key_digest=m.key_digest AND requested.session_key=m.session_key
                 JOIN semon_pg_native_catalog_trace_headers h
                   ON h.namespace_digest=m.namespace_digest AND h.namespace=m.namespace
                  AND h.generation=m.generation AND h.key_digest=m.key_digest AND h.session_key=m.session_key
                  {source_filter}
                 WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.generation=$3
                   AND NOT EXISTS(
                       SELECT 1 FROM semon_pg_native_catalog_trace_headers target
                       WHERE target.namespace_digest=m.namespace_digest AND target.namespace=m.namespace
                         AND target.generation=$2 AND target.key_digest=m.key_digest AND target.session_key=m.session_key
                   )"
            ),
            &args,
        )
        .map_err(sqlstate::error)?;
    }
    tx.check()?;
    let inserted = tx.driver.execute(
        &format!(
            "INSERT INTO semon_pg_native_catalog_trace_headers(namespace,generation,key_digest,session_key,trace_generation,source_generation,metadata)
             SELECT h.namespace,$2,h.key_digest,h.session_key,h.trace_generation,h.source_generation,h.metadata
             FROM unnest($4::bytea[],$5::bytea[]) AS requested(key_digest,session_key)
             JOIN semon_pg_native_catalog_trace_headers h
               ON h.namespace_digest=sha256($1::bytea) AND h.namespace=$1 AND h.generation=$3
              AND h.key_digest=requested.key_digest AND h.session_key=requested.session_key
              {source_filter}
             WHERE NOT EXISTS(
                 SELECT 1 FROM semon_pg_native_catalog_trace_headers target
                 WHERE target.namespace_digest=h.namespace_digest AND target.namespace=h.namespace
                   AND target.generation=$2 AND target.key_digest=h.key_digest AND target.session_key=h.session_key
             )"
        ),
        &args,
    ).map_err(sqlstate::error)?;
    if inserted != eligible as u64 {
        return Err(StorageError::ScopeChanged);
    }
    tx.check()
}

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
        let page = tx.driver.query("WITH page AS MATERIALIZED (SELECT key_digest,session_key FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND NOT key_digest=ANY($4::bytea[]) AND ($5::bytea IS NULL OR key_digest>$5) ORDER BY key_digest LIMIT $6), copied AS (INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model,query_activity,query_active,parent_key) SELECT namespace,$2::bytea,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model,query_activity,query_active,parent_key FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$3 AND key_digest IN (SELECT key_digest FROM page) ON CONFLICT DO NOTHING) SELECT key_digest,session_key FROM page ORDER BY key_digest", &[&ns, &generation.as_bytes(), &previous.as_bytes(), &mentioned, &after, &PAGE]).map_err(sqlstate::error)?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after = Some(last.get(0));
        let digests = page.iter().map(|row| row.get(0)).collect::<Vec<Vec<u8>>>();
        let mut owners = Vec::with_capacity(page.len());
        for row in &page {
            let key_digest: Vec<u8> = row.get(0);
            let key = row.get::<_, Vec<u8>>(1);
            if digest(&key) != key_digest {
                return Err(StorageError::ScopeChanged);
            }
            owners.push((key_digest, key));
        }
        trace_families(tx, generation, previous, &owners, None)?;
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
