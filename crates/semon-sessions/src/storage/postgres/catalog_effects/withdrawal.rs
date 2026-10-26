//! Source withdrawal owns a fresh immutable Catalog generation. Native rows,
//! archive custody and every older proof/lease remain intact.
use super::*;
use crate::events::{Ledger, Outcome};
use std::collections::BTreeSet;

pub(in crate::storage::postgres) fn remove(
    tx: &mut PgTxn<'_>,
    files: &[(String, Option<Ledger>)],
) -> Result<Outcome, StorageError> {
    if files.is_empty() {
        return Ok(Outcome::Written);
    }
    if files.len() > 64
        || files
            .iter()
            .any(|(p, l)| p.is_empty() || p.len() > 4096 || l.is_none())
    {
        return Err(StorageError::InvalidArguments);
    }
    let request = encoded(&("source-withdrawal-v1", files))?;
    let operation = digest(&request);
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    // Reconcile the exact full-value ACK before inspecting a successor head.
    let ack=tx.driver.query("SELECT payload=$3 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id=$2", &[&ns,&operation,&request]).map_err(sqlstate::error)?;
    if let Some(row) = ack.first() {
        return if row.get::<_, bool>(0) {
            Ok(Outcome::Written)
        } else {
            Err(StorageError::Conflict)
        };
    }
    let active:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND active)", &[&ns]).map_err(sqlstate::error)?.get(0);
    if active {
        return Ok(Outcome::Conflict);
    }
    for (path, expected) in files {
        let pending: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2)", &[&ns, &path.as_bytes()]).map_err(sqlstate::error)?.get(0);
        if pending { return Ok(Outcome::Conflict); }
        if super::super::native_reader::ledger(tx, path)?
            .map(|(_, l)| l)
            .as_ref()
            != expected.as_ref()
        {
            return Ok(Outcome::Conflict);
        }
    }
    let previous = super::super::model_cache::generation(tx)?.ok_or(StorageError::ScopeChanged)?;
    let physical=tx.driver.query("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 ORDER BY session_key LIMIT 65", &[&ns,&previous.as_bytes()]).map_err(sqlstate::error)?;
    if physical.len() > 64 {
        return Err(StorageError::Busy);
    }
    let paths = files
        .iter()
        .map(|(p, _)| p.as_str())
        .collect::<BTreeSet<_>>();
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    let mut used = 0usize;
    for row in physical {
        let bytes = row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?;
        used = used.saturating_add(bytes.len());
        if used > MAX {
            return Err(StorageError::Busy);
        }
        let value: CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if value.key.as_bytes() != row.get::<_, Vec<u8>>(0) {
            return Err(StorageError::ScopeChanged);
        }
        let hits = value
            .sources
            .iter()
            .filter(|s| paths.contains(s.path.to_string_lossy().as_ref()))
            .count();
        if hits == 0 {
            kept.push(value);
            continue;
        }
        // A mixed-source owner requires ordinary semantic reconstruction. Never
        // drop its surviving sources or return a partial-success receipt.
        if hits != value.sources.len() {
            return Err(StorageError::NamespaceUpgradePending);
        }
        for source in &value.sources {
            let ledger = files
                .iter()
                .find(|(p, _)| Some(p.as_str()) == source.path.to_str())
                .and_then(|(_, l)| l.as_ref())
                .ok_or(StorageError::ScopeChanged)?;
            if !catalog::source_matches_ledger(source, ledger) {
                return Ok(Outcome::Conflict);
            }
        }
        removed.push(value.key);
    }
    // Preserve relationship semantics: withdrawal of one endpoint requires the
    // connected ordinary producer, rather than a stale surviving edge.
    for row in &kept {
        if row.parent.as_ref().is_some_and(|p| removed.contains(p))
            || row
                .query
                .as_ref()
                .and_then(|q| q["selected"]["edges"].as_array())
                .is_some_and(|edges| {
                    edges.iter().any(|e| {
                        ["from", "to"].iter().any(|f| {
                            e["query"][*f]
                                .as_str()
                                .is_some_and(|k| removed.iter().any(|r| r == k))
                        })
                    })
                })
        {
            return Err(StorageError::NamespaceUpgradePending);
        }
    }
    let generation = format!("{:x}", Sha256::digest(&operation));
    let stage=tx.driver.query_one("SELECT stage_epoch,runtime FROM semon_pg_native_catalog_stage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let prior_epoch: i64 = stage.get(0);
    let epoch = prior_epoch
        .checked_add(1)
        .ok_or(StorageError::ScopeChanged)?;
    let runtime: Vec<u8> = stage.get(1);
    for (path, _) in files {
        let evidence = encoded(&("withdrawal-intent-v1", &request, &previous))?;
        tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace", &[&ns,&path.as_bytes(),&evidence]).map_err(sqlstate::error)?;
        super::super::source_journal::invalidate(tx, path)?;
        tx.driver.execute("UPDATE semon_pg_source_invalidations SET reason='removed' WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
    }
    // Fixed immutable families only. Copy exact payloads for surviving owners;
    // do not mutate/delete any old generation or its retained native proof.
    for (table, columns) in [
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
    ] {
        let sql = format!(
            "INSERT INTO semon_pg_native_{table}(namespace,generation,{columns}) SELECT namespace,$3,{columns} FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND NOT(session_key=ANY($4::bytea[]))"
        );
        let keys = removed
            .iter()
            .map(|k| k.as_bytes().to_vec())
            .collect::<Vec<_>>();
        tx.driver
            .execute(
                &sql,
                &[&ns, &previous.as_bytes(), &generation.as_bytes(), &keys],
            )
            .map_err(sqlstate::error)?;
        tx.check()?;
    }
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original) SELECT namespace,$3,key_digest,session_key,owner_generation,original FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&ns,&previous.as_bytes(),&generation.as_bytes()]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) SELECT namespace,$3,path_digest,source_path,page_generation,usage_generation,usage_ready FROM semon_pg_native_catalog_proofs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&ns,&previous.as_bytes(),&generation.as_bytes()]).map_err(sqlstate::error)?;
    let effects = Effects {
        tx: RefCell::new(tx),
        generation: &generation,
    };
    for row in &kept {
        let metadata = encoded(row)?;
        use catalog::CurrentPublicationStore;
        effects.current(
            row,
            std::str::from_utf8(&metadata).map_err(|_| StorageError::ScopeChanged)?,
        )?;
        catalog::publish_current_sources(&effects, row)?;
    }
    crate::catalog_search::publish_current_rows_port(
        &effects,
        &kept.iter().collect::<Vec<_>>(),
        64,
        |_, _| {},
    )?;
    let tx = effects.tx.into_inner();
    if tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2,semantic_pending=false WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND catalog_generation=$3", &[&ns,&generation,&previous]).map_err(sqlstate::error)?!=1 {return Err(StorageError::Conflict);}
    if tx.driver.execute("UPDATE semon_pg_native_catalog_stage SET stage_epoch=$2,runtime=$3,generation=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton AND stage_epoch=$5", &[&ns,&epoch,&runtime,&generation.as_bytes(),&prior_epoch]).map_err(sqlstate::error)?!=1 {return Err(StorageError::Conflict);}
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,$2,$3,$4,$5)", &[&ns,&operation,&request,&generation.as_bytes(),&epoch]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_changes(namespace,generation,previous,stage_epoch) VALUES($1,$2,$3,$4)", &[&ns,&generation.as_bytes(),&previous.as_bytes(),&epoch]).map_err(sqlstate::error)?;
    for (path, ledger) in files {
        super::super::native_keys::put_meta(
            tx,
            format!("source-retired:{path}").as_bytes(),
            &encoded(ledger)?,
        )?;
        tx.driver.execute("DELETE FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed'", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
    }
    tx.check()?;
    Ok(Outcome::Written)
}
