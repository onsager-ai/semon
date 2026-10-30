//! Bounded ordinary IndexStore publication. All proof binding, immutable effects,
//! head CAS and claim acknowledgements share one transaction. No Raw grant.
use super::*;
use crate::{events::Outcome, storage::semantic_owner};

/// The ordinary semantic stage owns this lifecycle observation independently
/// of its surviving rows, including a publication containing only withdrawals.
#[derive(serde::Serialize)]
pub(in crate::storage::postgres) struct Semantic<'a> {
    pub(in crate::storage::postgres) runtime: &'a str,
    pub(in crate::storage::postgres) withdrawals: &'a [crate::model::relation_stage::Withdrawal],
    pub(in crate::storage::postgres) prior_owners:
        &'a std::collections::BTreeMap<String, CatalogRow>,
    pub(in crate::storage::postgres) retired_owners: &'a [CatalogRow],
    pub(in crate::storage::postgres) lifecycle_authority: Option<ValidatedLifecycle<'a>>,
}

/// Physical qualification is performed before opening the publication writer.
/// The private constructor prevents a SQL-only caller from manufacturing it.
#[derive(serde::Serialize)]
#[serde(transparent)]
pub(in crate::storage::postgres) struct ValidatedLifecycle<'a>(
    &'a crate::model::relation_stage::LifecycleAuthority,
);

pub(in crate::storage::postgres) fn preflight(
    authority: Option<&crate::model::relation_stage::LifecycleAuthority>,
) -> Result<Option<ValidatedLifecycle<'_>>, StorageError> {
    authority
        .map(|authority| {
            if !authority.current().unwrap_or(false) {
                return Err(StorageError::Conflict);
            }
            Ok(ValidatedLifecycle(authority))
        })
        .transpose()
}

fn retirement_plan(
    tx: &mut PgTxn<'_>,
    expected: Option<&str>,
    claims: &[(String, Option<i64>)],
    rows: &[CatalogRow],
    semantic: &Semantic<'_>,
) -> Result<Vec<Vec<u8>>, StorageError> {
    let paths = claims
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    let prior = super::super::semantic_owners::affected(tx, expected, &paths)?;
    if encoded(&prior)? != encoded(semantic.prior_owners)? {
        return Err(StorageError::Conflict);
    }
    if let Some(authority) = &semantic.lifecycle_authority {
        if authority.0.runtime != semantic.runtime
            || (authority.0.facts.is_none() && !semantic.retired_owners.is_empty())
            || !super::super::current_sources::lifecycle_recorded(tx, authority.0)?
        {
            return Err(StorageError::Conflict);
        }
    } else if !semantic.retired_owners.is_empty()
        || (!prior.is_empty()
            && rows
                .iter()
                .any(|row| row.query.is_some() || row.trace.is_some()))
    {
        return Err(StorageError::Conflict);
    }
    let mut outgoing = Vec::new();
    for old in prior.values() {
        if old.sources.iter().any(|source| {
            !paths
                .iter()
                .any(|path| source.path == std::path::Path::new(path))
        }) {
            return Err(StorageError::Conflict);
        }
        if rows.iter().any(|row| row.key == old.key) {
            continue;
        }
        let withdrawn = old.sources.iter().all(|source| {
            semantic
                .withdrawals
                .iter()
                .any(|withdrawal| source.path == std::path::Path::new(&withdrawal.path))
        });
        if !withdrawn {
            for source in &old.sources {
                if rows
                    .iter()
                    .flat_map(|row| &row.sources)
                    .any(|new| new.path == source.path)
                {
                    continue;
                }
                let path = source.path.to_str().ok_or(StorageError::ScopeChanged)?;
                let removed: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed')", &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?.get(0);
                if !removed
                    || (super::super::native_keys::read_meta(
                        tx,
                        format!("source-retired:{path}").as_bytes(),
                        16384,
                    )?
                    .is_none()
                        && super::super::native_reader::ledger(tx, path)?.is_some())
                {
                    return Err(StorageError::Conflict);
                }
            }
        }
        outgoing.push(old.clone());
    }
    if encoded(&outgoing)? != encoded(&semantic.retired_owners)? {
        return Err(StorageError::Conflict);
    }
    Ok(outgoing
        .iter()
        .map(|row| digest(row.key.as_bytes()))
        .collect())
}

/// A new destination operation over an already applied, source-authorized
/// snapshot. Original producer receipts remain immutable; no claims are ACKed
/// and no live source lease or unfinished producer stage is synthesized.
pub(in crate::storage::postgres) fn import(
    tx: &mut PgTxn<'_>,
    import: &crate::storage::transfer::reported::CatalogImport,
) -> Result<String, StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let admission = super::super::model_cache::binding(tx)?.admission;
    let payload = import.payload(admission)?;
    let operation = digest(&payload);
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let old = tx.driver.query("SELECT payload=$3 AND generation=$4 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id=$2", &[&ns,&operation,&payload,&import.generation().as_bytes()]).map_err(sqlstate::error)?;
    if let Some(row) = old.first() {
        if !row.get::<_, bool>(0) {
            return Err(StorageError::Conflict);
        }
        return Ok(import.generation().to_owned());
    }
    if super::super::model_cache::generation(tx)?.is_some() {
        return Err(StorageError::Conflict);
    }
    // Original receipt revisions belong to their producer's journal. The
    // typed completed-source import supplies current snapshot authority;
    // comparing those counters across physical backends would reject reverse.
    let receipt = tx.driver.query("SELECT epoch FROM semon_pg_native_semantic_publications WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 2", &[&ns,&import.generation().as_bytes()]).map_err(sqlstate::error)?;
    if receipt.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let epoch: i64 = receipt[0].get(0);
    let missing: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.generation=$2 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_selected_refs r WHERE r.namespace_digest=c.namespace_digest AND r.namespace=c.namespace AND r.key_digest=c.key_digest AND r.session_key=c.session_key AND r.kind='current'::bytea)) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners h WHERE h.namespace_digest=sha256($1::bytea) AND h.namespace=$1 AND h.generation=$2 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_selected_refs r WHERE r.namespace_digest=h.namespace_digest AND r.namespace=h.namespace AND r.key_digest=h.key_digest AND r.session_key=h.session_key AND r.kind='history'::bytea))", &[&ns,&import.generation().as_bytes()]).map_err(sqlstate::error)?.get(0);
    if missing {
        return Err(StorageError::ScopeChanged);
    }
    let changed = tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND catalog_generation IS NULL", &[&ns,&import.generation()]).map_err(sqlstate::error)?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,$2,$3,$4,$5)", &[&ns,&operation,&payload,&import.generation().as_bytes(),&epoch]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_changes(namespace,generation,previous,stage_epoch) VALUES($1,$2,NULL,$3)", &[&ns,&import.generation().as_bytes(),&epoch]).map_err(sqlstate::error)?;
    tx.check()?;
    Ok(import.generation().to_owned())
}

/// The ordinary port has no caller supplied operation ID. Derive one from the
/// complete request, including its base head and exact claims. Keep full bytes
/// beside the digest so a collision never aliases two publication requests.
pub(super) fn request(
    publication: &crate::slot_projection::Publication<'_>,
    expected: Option<&str>,
    claims: &[(String, Option<i64>)],
    closed: bool,
    semantic: Option<&Semantic<'_>>,
) -> Result<(Vec<u8>, Vec<u8>), StorageError> {
    if publication.rows.len() > 64 || claims.len() > 64 {
        return Err(StorageError::Busy);
    }
    let recipes = publication.transcripts.map(|projections| {
        projections
            .iter()
            .map(|p| (&p.key, &p.slots, p.repair))
            .collect::<Vec<_>>()
    });
    let derivations = publication
        .rows
        .iter()
        .map(|row| {
            (
                &row.overview,
                row.trace
                    .as_ref()
                    .map(|trace| trace.publication_witness(row)),
            )
        })
        .collect::<Vec<_>>();
    let payload = encoded(&(
        "catalog-publication-v1",
        publication.rows,
        derivations,
        recipes,
        expected,
        claims,
        closed,
        semantic,
    ))?;
    Ok((digest(&payload), payload))
}

pub(in crate::storage::postgres) fn publish(
    tx: &mut PgTxn<'_>,
    publication: &crate::slot_projection::Publication<'_>,
    expected: Option<&str>,
    claims: &[(String, Option<i64>)],
    closed: bool,
    semantic: Option<Semantic<'_>>,
) -> Result<(Outcome, Option<String>), StorageError> {
    let (operation, payload) = request(publication, expected, claims, closed, semantic.as_ref())?;
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    // Serialize against native advancement and other publishers, including
    // replay. An acknowledged operation is checked before its now-stale CAS.
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE source_state_lock");
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE operation_replay_lookup");
    let old = tx.driver.query("SELECT payload=$3,CASE WHEN octet_length(generation)=64 THEN generation END FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND operation_id=$2", &[&ns, &operation, &payload]).map_err(sqlstate::error)?;
    if let Some(row) = old.first() {
        if !row.get::<_, bool>(0) {
            return Err(StorageError::Conflict);
        }
        let generation = String::from_utf8(
            row.get::<_, Option<Vec<u8>>>(1)
                .ok_or(StorageError::ScopeChanged)?,
        )
        .map_err(|_| StorageError::ScopeChanged)?;
        return Ok((Outcome::Written, Some(generation)));
    }
    if super::super::model_cache::generation(tx)?.as_deref() != expected {
        return Ok((Outcome::Conflict, None));
    }
    let paths = publication
        .rows
        .iter()
        .flat_map(|row| &row.sources)
        .map(|source| path(source).map(str::to_owned))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?
        .into_iter()
        .collect::<Vec<_>>();
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE admission_capture");
    let fence = admission::capture(tx, &paths)?;
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE source_claims");
    for (path, revision) in claims {
        let actual = tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path.as_bytes()]).map_err(sqlstate::error)?;
        if actual.first().map(|row| row.get::<_, i64>(0)) != *revision {
            return Ok((Outcome::Conflict, None));
        }
    }
    let mut withdrawals = std::collections::BTreeSet::new();
    if let Some(semantic) = &semantic {
        #[cfg(test)]
        eprintln!("PG_PUBLISH_STAGE facts_withdrawal_authority");
        if semantic.withdrawals.len() > 64 {
            return Err(StorageError::Busy);
        }
        for withdrawal in semantic.withdrawals {
            if !withdrawals.insert(withdrawal.path.as_str())
                || withdrawal.runtime != semantic.runtime
                || !claims.iter().any(|(path, _)| path == &withdrawal.path)
                || paths.contains(&withdrawal.path)
                || !super::super::current_sources::withdrawal_recorded(tx, withdrawal)?
            {
                return Err(StorageError::Conflict);
            }
        }
    }
    if closed {
        let outstanding: i64 = tx.driver.query_one("SELECT count(*) FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0);
        if outstanding
            != claims
                .iter()
                .filter(|(_, revision)| revision.is_some())
                .count() as i64
        {
            return Ok((Outcome::Conflict, None));
        }
    }
    let runtime = publication
        .rows
        .iter()
        .filter_map(|row| row.query.as_ref()?.get("runtime")?.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE owner_state");
    let prior = tx.driver.query("SELECT stage_epoch,runtime,generation FROM semon_pg_native_catalog_stage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND singleton FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let old_epoch = prior.first().map(|row| row.get::<_, i64>(0));
    let old_runtime = prior
        .first()
        .map(|row| String::from_utf8(row.get(1)))
        .transpose()
        .map_err(|_| StorageError::ScopeChanged)?;
    let old_generation = prior
        .first()
        .and_then(|row| row.get::<_, Option<Vec<u8>>>(2))
        .map(String::from_utf8)
        .transpose()
        .map_err(|_| StorageError::ScopeChanged)?;
    // Semantic lifecycle authority survives even when there are no row Query
    // summaries. Generic publications retain their existing row/head binding.
    let bound = if let Some(semantic) = &semantic {
        let binding = super::super::model_cache::binding(tx)?;
        let witnessed = binding
            .witness(semantic.runtime)
            .map_err(|_| StorageError::ScopeChanged)?;
        if !publication.rows.is_empty()
            && (runtime.len() != 1 || runtime.first().copied() != Some(witnessed.as_str()))
        {
            return Err(StorageError::ScopeChanged);
        }
        semantic_owner::bound_runtime(tx.namespace.as_str(), &binding.admission, &witnessed)?
    } else {
        match (runtime.first(), runtime.len(), publication.rows.is_empty()) {
            (Some(runtime), 1, _) => {
                let binding = super::super::model_cache::binding(tx)?;
                semantic_owner::bound_runtime(tx.namespace.as_str(), &binding.admission, runtime)?
            }
            (None, 0, true) if expected.is_some() => {
                old_runtime.clone().ok_or(StorageError::ScopeChanged)?
            }
            _ => return Err(StorageError::ScopeChanged),
        }
    };
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE owner_transition");
    semantic_owner::begin(
        semantic_owner::Observed {
            epoch: old_epoch,
            runtime: old_runtime.as_deref(),
            generation: old_generation.as_deref(),
            invalid_withdrawal: false,
        },
        None,
        false,
        expected,
        &bound,
        true,
    )?;
    let epoch = old_epoch
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(StorageError::ScopeChanged)?;
    let generation = format!("{:x}", Sha256::digest(&operation));
    // A partial request keeps every unmentioned Current owner. Owners connected
    // to a republished or claimed Source must be republished by the producer.
    let mut leaving = if let Some(semantic) = &semantic {
        retirement_plan(tx, expected, claims, publication.rows, semantic)?
    } else {
        Vec::new()
    };
    let mut carried = None;
    if let Some(previous) = expected {
        let mut mentioned = publication
            .rows
            .iter()
            .map(|row| digest(row.key.as_bytes()))
            .collect::<Vec<_>>();
        mentioned.extend(leaving.iter().cloned());
        // A claimed Source that no row reads and has durable removal or exact
        // semantic Facts withdrawal authority is withdrawn:
        // owners reading only withdrawn Sources leave Current, exactly as the
        // isolated withdrawal does; mixed owners must be republished above.
        let mut retired = Vec::new();
        for (path, _) in claims {
            if !paths.contains(path)
                && (withdrawals.contains(path.as_str())
                    || super::super::native_keys::read_meta(
                        tx,
                        format!("source-retired:{path}").as_bytes(),
                        16384,
                    )?
                    .is_some())
            {
                retired.push(path.as_bytes().to_vec());
            }
        }
        if !retired.is_empty() {
            let withdrawn = tx.driver.query("SELECT r.key_digest FROM semon_pg_native_catalog_rows r WHERE r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2 AND NOT r.key_digest=ANY($3::bytea[]) AND EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources s WHERE s.namespace_digest=r.namespace_digest AND s.namespace=r.namespace AND s.generation=r.generation AND s.key_digest=r.key_digest AND s.source_path=ANY($4::bytea[])) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_sources s WHERE s.namespace_digest=r.namespace_digest AND s.namespace=r.namespace AND s.generation=r.generation AND s.key_digest=r.key_digest AND NOT s.source_path=ANY($4::bytea[])) ORDER BY r.key_digest LIMIT 65", &[&ns, &previous.as_bytes(), &mentioned, &retired]).map_err(sqlstate::error)?.iter().map(|row| row.get(0)).collect::<Vec<Vec<u8>>>();
            if withdrawn.len() > 64 {
                return Err(StorageError::Busy);
            }
            leaving.extend(withdrawn.iter().cloned());
            leaving.sort();
            leaving.dedup();
            mentioned.extend(withdrawn);
        }
        let claimed = paths
            .iter()
            .map(String::as_str)
            .chain(claims.iter().map(|(path, _)| path.as_str()))
            .map(|path| path.as_bytes().to_vec())
            .collect::<Vec<_>>();
        carried = Some((previous, mentioned, claimed));
    }
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE binding_stage_bound");
    let refs = admission::stage_bound(tx, &generation, publication, &fence)?;
    for reference in refs {
        tx.check()?;
        #[cfg(test)]
        eprintln!("PG_PUBLISH_STAGE proof_insert");
        tx.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) VALUES($1,$2,sha256($3::bytea),$3,$4,$5,$6) ON CONFLICT DO NOTHING", &[&ns, &generation.as_bytes(), &reference.path.as_bytes(), &reference.pages.0.as_bytes(), &reference.usage_generation, &reference.usage_ready]).map_err(sqlstate::error)?;
    }
    #[cfg(test)]
    if expected.is_some() {
        eprintln!("PG_PUBLISH_STAGE history_preserve");
    }
    if let Some(previous) = expected
        && !history::preserve(tx, &generation, previous, publication, &leaving)?
    {
        // Only private custody checkpoints commit, before any successor row
        // or owner is carried; the head, claims and stage stay with the old
        // generation until custody completes and the CAS below succeeds.
        return Ok((Outcome::Deferred, None));
    }
    if let Some((previous, mentioned, claimed)) = carried {
        #[cfg(test)]
        eprintln!("PG_PUBLISH_STAGE carry_unmentioned");
        super::carry::unmentioned(tx, &generation, previous, &mentioned, &claimed)?;
        #[cfg(test)]
        eprintln!("PG_PUBLISH_STAGE carry_retained_search");
        super::carry::retained_search(tx, &generation, previous, &leaving)?;
        // Staged counts predate the carried owners' memberships; the
        // unadmitted successor's aggregate is rebuilt once, as withdrawal does.
        use crate::storage::catalog::SearchPublicationStore;
        tx.driver.execute("DELETE FROM semon_pg_native_catalog_search_counts WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&ns, &generation.as_bytes()]).map_err(sqlstate::error)?;
        let effects = Effects {
            tx: RefCell::new(&mut *tx),
            generation: &generation,
        };
        #[cfg(test)]
        eprintln!("PG_PUBLISH_STAGE search_counts");
        for scope in [
            crate::CatalogReadScope::Current,
            crate::CatalogReadScope::RetainedHistory,
        ] {
            effects.counts(scope)?;
        }
    }
    let expected_value = expected.map(str::to_owned);
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE state_cas");
    let changed = tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2,semantic_pending=false WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND catalog_generation IS NOT DISTINCT FROM $3", &[&ns, &generation, &expected_value]).map_err(sqlstate::error)?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE invalidation_removal");
    for (path, revision) in claims {
        if let Some(revision) = revision
            && tx.driver.execute("DELETE FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND revision=$3", &[&ns, &path.as_bytes(), revision]).map_err(sqlstate::error)? != 1
        {
            return Err(StorageError::Conflict);
        }
    }
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE catalog_stage_update");
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_stage(namespace,singleton,stage_epoch,runtime,generation) VALUES($1,true,$2,$3,$4) ON CONFLICT(namespace_digest,singleton) DO UPDATE SET stage_epoch=excluded.stage_epoch,runtime=excluded.runtime,generation=excluded.generation WHERE semon_pg_native_catalog_stage.namespace=excluded.namespace AND semon_pg_native_catalog_stage.stage_epoch=$5", &[&ns, &epoch, &bound.as_bytes(), &generation.as_bytes(), &old_epoch.unwrap_or(0)]).map_err(sqlstate::error)?;
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE operation_insert");
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,$2,$3,$4,$5)", &[&ns, &operation, &payload, &generation.as_bytes(), &epoch]).map_err(sqlstate::error)?;
    let binding = super::super::model_cache::binding(tx)?;
    let native = encoded(&(binding.admission, &binding.stream_epoch, &generation))?;
    for row in publication.rows {
        let key = format!(
            "catalog-native-observation:{:x}",
            Sha256::digest(encoded(row)?)
        );
        #[cfg(test)]
        eprintln!("PG_PUBLISH_STAGE native_observation");
        super::super::native_keys::put_meta(tx, key.as_bytes(), &native)?;
    }
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE catalog_change_insert");
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_changes(namespace,generation,previous,stage_epoch) VALUES($1,$2,$3,$4)", &[&ns, &generation.as_bytes(), &expected.map(str::as_bytes), &epoch]).map_err(sqlstate::error)?;
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE withdrawal_key_lookup");
    let withdrawn_keys = if let Some(previous) = expected {
        tx.driver.query("SELECT session_key FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=ANY($3::bytea[])", &[&ns, &previous.as_bytes(), &leaving]).map_err(sqlstate::error)?
            .iter().map(|row| String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)).collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    #[cfg(test)]
    eprintln!("PG_PUBLISH_STAGE change_feed_publish");
    super::super::changes::publish(
        tx,
        publication
            .rows
            .iter()
            .map(|row| row.key.as_str())
            .chain(withdrawn_keys.iter().map(String::as_str)),
    )?;
    tx.check()?;
    Ok((Outcome::Written, Some(generation)))
}
