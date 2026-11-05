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
    // A deferred mixed/connected withdrawal is acknowledged by its exact Index
    // intent, before and after the semantic catch-up publishes its successor.
    let deferred = encoded(&("withdrawal-deferred-v1", &request))?;
    let mut replayed = 0;
    for (path, _) in files {
        let intent = tx.driver.query("SELECT evidence=$3 FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path.as_bytes(), &deferred]).map_err(sqlstate::error)?;
        if intent.first().is_some_and(|row| row.get::<_, bool>(0)) {
            replayed += 1;
        }
    }
    if replayed == files.len() {
        return Ok(Outcome::Written);
    }
    let mut relocated = 0;
    for (path, ledger) in files {
        if super::super::source_lifecycle::relocation_removed(
            tx,
            path,
            ledger.as_ref().ok_or(StorageError::InvalidArguments)?,
        )? {
            relocated += 1;
        }
    }
    if relocated == files.len() {
        return Ok(Outcome::Written);
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
    let mut connected = false;
    let mut after = None;
    while let Some(page) = owners(tx, &previous, &after, Some(&withdrawn))? {
        for (_, value) in &page {
            let hits = value
                .sources
                .iter()
                .filter(|s| paths.contains(s.path.to_string_lossy().as_ref()))
                .collect::<Vec<_>>();
            // A mixed-source owner requires ordinary semantic reconstruction.
            // Never drop its surviving sources or return a partial-success receipt.
            connected |= hits.len() != value.sources.len();
            // Composite custody is rebuilt by the semantic producer, including
            // withdrawal of its final Current source. Do not replace it with an
            // original snapshot of the remaining Current projection.
            connected |= tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 AND NOT original)", &[&ns, &previous.as_bytes(), &digest(value.key.as_bytes()), &value.key.as_bytes()]).map_err(sqlstate::error)?.get::<_, bool>(0);
            for source in hits {
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
    while !connected
        && !removed.is_empty()
        && let Some(page) = owners(tx, &previous, &after, None)?
    {
        for (_, row) in &page {
            if removed.contains(&row.key) {
                continue;
            }
            if row.parent.as_ref().is_some_and(|p| removed.contains(p))
                || crate::query_projection::SelectedRecipe::edges_for(row)
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
                connected = true;
                break;
            }
        }
        after = page.last().map(|(digest, _)| digest.clone());
    }
    if connected {
        return defer(tx, files, &deferred);
    }
    let generation = format!("{:x}", Sha256::digest(&operation));
    let stages=tx.driver.query("SELECT stage_epoch,runtime FROM semon_pg_native_catalog_stage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let Some(stage) = stages.first() else {
        // An imported Catalog can be admitted before this destination has a
        // producer runtime. Preserve its Current head and let the ordinary
        // semantic producer create the successor with a fresh runtime.
        return defer(tx, files, &deferred);
    };
    let prior_epoch: i64 = stage.get(0);
    let epoch = prior_epoch
        .checked_add(1)
        .ok_or(StorageError::ScopeChanged)?;
    let runtime: Vec<u8> = stage.get(1);
    // Private custody checkpoints precede mutation intent and publication. Each
    // retry reads the same immutable Current cohort and advances at most one
    // original owner, even when withdrawal spans several keyset pages.
    let mut advanced = false;
    let mut after = None;
    while let Some(page) = owners(tx, &previous, &after, Some(&withdrawn))? {
        use crate::storage::catalog::OriginalHistoryStore;
        let store = super::history::History {
            effects: Effects {
                tx: RefCell::new(&mut *tx),
                generation: &generation,
            },
            previous: &previous,
        };
        for (_, row) in &page {
            if store.retained(&row.key)? {
                continue;
            }
            if advanced || !catalog::preserve_original(&store, row)? {
                return Ok(Outcome::Deferred);
            }
            advanced = true;
        }
        after = page.last().map(|(digest, _)| digest.clone());
    }
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
    retained_search(tx, &generation, &previous, &withdrawn)?;
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
    super::search::publish_state(tx, &generation)?;
    if tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2,semantic_pending=false WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND catalog_generation=$3", &[&ns,&generation,&previous]).map_err(sqlstate::error)?!=1 {return Err(StorageError::Conflict);}
    if tx.driver.execute("UPDATE semon_pg_native_catalog_stage SET stage_epoch=$2,runtime=$3,generation=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton AND stage_epoch=$5", &[&ns,&epoch,&runtime,&generation.as_bytes(),&prior_epoch]).map_err(sqlstate::error)?!=1 {return Err(StorageError::Conflict);}
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,$2,$3,$4,$5)", &[&ns,&operation,&request,&generation.as_bytes(),&epoch]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_changes(namespace,generation,previous,stage_epoch) VALUES($1,$2,$3,$4)", &[&ns,&generation.as_bytes(),&previous.as_bytes(),&epoch]).map_err(sqlstate::error)?;
    super::super::changes::publish(tx, removed.iter().map(String::as_str))?;
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

/// Mixed or connected owners are rebuilt by the ordinary semantic producer.
/// This commit records only the Index intent and the `removed` invalidation;
/// the qualified Current generation stays until the catch-up publication CAS.
fn defer(
    tx: &mut PgTxn<'_>,
    files: &[(String, Option<Ledger>)],
    intent: &[u8],
) -> Result<Outcome, StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    for (path, ledger) in files {
        tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace", &[&ns,&path.as_bytes(),&intent]).map_err(sqlstate::error)?;
        super::super::native_keys::put_meta(
            tx,
            format!("source-retired:{path}").as_bytes(),
            &encoded(ledger)?,
        )?;
        super::super::source_journal::invalidate(tx, path)?;
        tx.driver.execute("UPDATE semon_pg_source_invalidations SET reason='removed' WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
    }
    tx.driver.execute("UPDATE semon_pg_source_state SET semantic_pending=true WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?;
    tx.check()?;
    Ok(Outcome::Written)
}

/// Removed owners have no Current search authority. Publish their selected
/// immutable History metadata, including its posting timestamp, in bounded
/// pages. Counts are finalized only after every page and surviving owner.
fn retained_search(
    tx: &mut PgTxn<'_>,
    generation: &str,
    previous: &str,
    withdrawn: &Vec<Vec<u8>>,
) -> Result<(), StorageError> {
    let mut after = None;
    while let Some(page) = owners(tx, previous, &after, Some(withdrawn))? {
        let store = super::history::History {
            effects: Effects {
                tx: RefCell::new(&mut *tx),
                generation,
            },
            previous,
        };
        let mut remaining = MAX;
        let rows = page
            .iter()
            .map(|(_, row)| store.selected_row(&row.key, &mut remaining))
            .collect::<Result<Vec<_>, _>>()?;
        let keys = rows.iter().map(|row| row.key.clone()).collect::<Vec<_>>();
        let scoped = rows
            .iter()
            .map(|row| crate::catalog_search::ScopedCatalogRow {
                row,
                scopes: crate::catalog_search::SearchScopeEligibility {
                    current: false,
                    retained_history: true,
                },
            })
            .collect::<Vec<_>>();
        crate::catalog_search::stage_scoped_rows_port(
            &store.effects,
            &keys,
            &scoped,
            64,
            |_, _| {},
        )?;
        after = page.last().map(|(digest, _)| digest.clone());
    }
    Ok(())
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
    let rows = tx.driver.query("WITH page AS MATERIALIZED (SELECT r.key_digest,r.session_key,r.metadata,r.query_recipe FROM semon_pg_native_catalog_rows r WHERE r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2 AND ($3::bytea IS NULL OR r.key_digest>$3) AND ($4::bytea[] IS NULL OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources s WHERE s.namespace_digest=r.namespace_digest AND s.namespace=r.namespace AND s.generation=r.generation AND s.key_digest=r.key_digest AND s.source_path=ANY($4::bytea[]))) ORDER BY r.key_digest LIMIT 64) SELECT key_digest,session_key,CASE WHEN sum(octet_length(metadata)+coalesce(octet_length(query_recipe),0)) OVER ()<=1048576 THEN metadata END,CASE WHEN sum(octet_length(metadata)+coalesce(octet_length(query_recipe),0)) OVER ()<=1048576 THEN query_recipe END FROM page ORDER BY key_digest", &[&ns, &previous.as_bytes(), after, &paths]).map_err(sqlstate::error)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let mut page = Vec::new();
    for row in rows {
        let bytes = row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?;
        let mut value: CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if value.key.as_bytes() != row.get::<_, Vec<u8>>(1) {
            return Err(StorageError::ScopeChanged);
        }
        let payload = row.get::<_, Option<Vec<u8>>>(3);
        if value.query.as_ref().is_some_and(|query| query.get("selected").is_some())
            && crate::query_projection::SelectedRecipe::detached(&value) {
            let payload = payload.ok_or(StorageError::ScopeChanged)?;
            let payload = std::str::from_utf8(&payload).map_err(|_| StorageError::ScopeChanged)?;
            value.query_recipe = Some(crate::query_projection::SelectedRecipe::decode(&value, payload)
                .map_err(|_| StorageError::ScopeChanged)?);
        } else if payload.is_some() {
            return Err(StorageError::ScopeChanged);
        }
        page.push((row.get(0), value));
    }
    Ok(Some(page))
}
