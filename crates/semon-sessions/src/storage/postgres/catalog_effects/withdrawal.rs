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
        if pending {
            return Ok(Outcome::Conflict);
        }
        if super::super::native_reader::ledger(tx, path)?
            .map(|(_, l)| l)
            .as_ref()
            != expected.as_ref()
        {
            return Ok(Outcome::Conflict);
        }
    }
    let previous = super::super::model_cache::generation(tx)?.ok_or(StorageError::ScopeChanged)?;
    let paths = files
        .iter()
        .map(|(p, _)| p.as_str())
        .collect::<BTreeSet<_>>();
    let withdrawn = paths
        .iter()
        .map(|p| p.as_bytes().to_vec())
        .collect::<Vec<_>>();
    // Owners reading a withdrawn Source, by bounded key pages.
    let mut removed = Vec::new();
    let mut after = None;
    while let Some(page) = owners(tx, &previous, &after, Some(&withdrawn))? {
        for (_, value) in &page {
            let hits = value
                .sources
                .iter()
                .filter(|s| paths.contains(s.path.to_string_lossy().as_ref()))
                .count();
            // A mixed-source owner requires ordinary semantic reconstruction.
            // Never drop its surviving sources or return a partial-success receipt.
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
            removed.push(value.key.clone());
        }
        after = page.last().map(|(digest, _)| digest.clone());
    }
    // Preserve relationship semantics: withdrawal of one endpoint requires the
    // connected ordinary producer, rather than a stale surviving edge.
    let mut after = None;
    while !removed.is_empty()
        && let Some(page) = owners(tx, &previous, &after, None)?
    {
        for (_, row) in &page {
            if removed.contains(&row.key) {
                continue;
            }
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
        after = page.last().map(|(digest, _)| digest.clone());
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
    // Surviving owners keep their exact immutable effects and proofs; no old
    // generation or retained native proof is mutated or deleted.
    let removed_digests = removed
        .iter()
        .map(|k| digest(k.as_bytes()))
        .collect::<Vec<_>>();
    super::carry::unmentioned(tx, &generation, &previous, &removed_digests, &withdrawn)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original) SELECT namespace,$3,key_digest,session_key,owner_generation,original FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=ANY($4::bytea[])", &[&ns,&previous.as_bytes(),&generation.as_bytes(),&removed_digests]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) SELECT namespace,$3,path_digest,source_path,page_generation,usage_generation,usage_ready FROM semon_pg_native_catalog_proofs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 ON CONFLICT DO NOTHING", &[&ns,&previous.as_bytes(),&generation.as_bytes()]).map_err(sqlstate::error)?;
    let effects = Effects {
        tx: RefCell::new(tx),
        generation: &generation,
    };
    {
        use crate::storage::catalog::SearchPublicationStore;
        for scope in [
            crate::CatalogReadScope::Current,
            crate::CatalogReadScope::RetainedHistory,
        ] {
            effects.counts(scope)?;
        }
    }
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

type Page = Vec<(Vec<u8>, CatalogRow)>;

/// One keyset page of previous Current owners, optionally only those reading
/// `paths`. Metadata is charged against the shared 1 MiB page budget.
fn owners(
    tx: &mut PgTxn<'_>,
    previous: &str,
    after: &Option<Vec<u8>>,
    paths: Option<&Vec<Vec<u8>>>,
) -> Result<Option<Page>, StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let rows = tx.driver.query("WITH page AS MATERIALIZED (SELECT r.key_digest,r.session_key,r.metadata FROM semon_pg_native_catalog_rows r WHERE r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2 AND ($3::bytea IS NULL OR r.key_digest>$3) AND ($4::bytea[] IS NULL OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources s WHERE s.namespace_digest=r.namespace_digest AND s.namespace=r.namespace AND s.generation=r.generation AND s.key_digest=r.key_digest AND s.source_path=ANY($4::bytea[]))) ORDER BY r.key_digest LIMIT 64) SELECT key_digest,session_key,CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata END FROM page ORDER BY key_digest", &[&ns, &previous.as_bytes(), after, &paths]).map_err(sqlstate::error)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let mut page = Vec::new();
    for row in rows {
        let bytes = row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?;
        let value: CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if value.key.as_bytes() != row.get::<_, Vec<u8>>(1) {
            return Err(StorageError::ScopeChanged);
        }
        page.push((row.get(0), value));
    }
    Ok(Some(page))
}
