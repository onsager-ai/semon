//! Bounded ordinary IndexStore publication. All proof binding, immutable effects,
//! head CAS and claim acknowledgements share one transaction. No Raw grant.
use super::*;
use crate::{events::Outcome, storage::semantic_owner};

/// The ordinary port has no caller supplied operation ID. Derive one from the
/// complete request, including its base head and exact claims. Keep full bytes
/// beside the digest so a collision never aliases two publication requests.
pub(super) fn request(
    publication: &crate::slot_projection::Publication<'_>,
    expected: Option<&str>,
    claims: &[(String, Option<i64>)],
    closed: bool,
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
    ))?;
    Ok((digest(&payload), payload))
}

pub(in crate::storage::postgres) fn publish(
    tx: &mut PgTxn<'_>,
    publication: &crate::slot_projection::Publication<'_>,
    expected: Option<&str>,
    claims: &[(String, Option<i64>)],
    closed: bool,
) -> Result<(Outcome, Option<String>), StorageError> {
    let (operation, payload) = request(publication, expected, claims, closed)?;
    publish_operation(
        tx,
        publication,
        expected,
        claims,
        closed,
        &operation,
        &payload,
    )
}

fn publish_operation(
    tx: &mut PgTxn<'_>,
    publication: &crate::slot_projection::Publication<'_>,
    expected: Option<&str>,
    claims: &[(String, Option<i64>)],
    closed: bool,
    operation: &[u8],
    payload: &[u8],
) -> Result<(Outcome, Option<String>), StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    // Serialize against native advancement and other publishers, including
    // replay. An acknowledged operation is checked before its now-stale CAS.
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
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
    let fence = admission::capture(tx, &paths)?;
    for (path, revision) in claims {
        let actual = tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path.as_bytes()]).map_err(sqlstate::error)?;
        if actual.first().map(|row| row.get::<_, i64>(0)) != *revision {
            return Ok((Outcome::Conflict, None));
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
    // A withdrawal-only request has no rows; it keeps the bound runtime of
    // the head it replaces rather than inventing one.
    let bound = match (runtime.first(), runtime.len(), publication.rows.is_empty()) {
        (Some(runtime), 1, _) => {
            let binding = super::super::model_cache::binding(tx)?;
            semantic_owner::bound_runtime(tx.namespace.as_str(), &binding.admission, runtime)?
        }
        (None, 0, true) if expected.is_some() => {
            old_runtime.clone().ok_or(StorageError::ScopeChanged)?
        }
        _ => return Err(StorageError::ScopeChanged),
    };
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
    let generation = format!("{:x}", Sha256::digest(operation));
    // A partial request keeps every unmentioned Current owner. Owners connected
    // to a republished or claimed Source must be republished by the producer.
    if let Some(previous) = expected {
        let mentioned = publication
            .rows
            .iter()
            .map(|row| digest(row.key.as_bytes()))
            .collect::<Vec<_>>();
        let claimed = paths
            .iter()
            .map(String::as_str)
            .chain(claims.iter().map(|(path, _)| path.as_str()))
            .map(|path| path.as_bytes().to_vec())
            .collect::<Vec<_>>();
        super::carry::unmentioned(tx, &generation, previous, &mentioned, &claimed)?;
    }
    let refs = admission::stage_bound(tx, &generation, publication, &fence)?;
    if let Some(previous) = expected
        && !history::preserve(tx, &generation, previous, publication)?
    {
        return Err(StorageError::Busy);
    }
    for reference in refs {
        tx.check()?;
        tx.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) VALUES($1,$2,sha256($3::bytea),$3,$4,$5,$6)", &[&ns, &generation.as_bytes(), &reference.path.as_bytes(), &reference.pages.0.as_bytes(), &reference.usage_generation, &reference.usage_ready]).map_err(sqlstate::error)?;
    }
    let expected_value = expected.map(str::to_owned);
    let changed = tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2,semantic_pending=false WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND catalog_generation IS NOT DISTINCT FROM $3", &[&ns, &generation, &expected_value]).map_err(sqlstate::error)?;
    if changed != 1 {
        return Err(StorageError::Conflict);
    }
    for (path, revision) in claims {
        if let Some(revision) = revision
            && tx.driver.execute("DELETE FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND revision=$3", &[&ns, &path.as_bytes(), revision]).map_err(sqlstate::error)? != 1
        {
            return Err(StorageError::Conflict);
        }
    }
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_stage(namespace,singleton,stage_epoch,runtime,generation) VALUES($1,true,$2,$3,$4) ON CONFLICT(namespace_digest,singleton) DO UPDATE SET stage_epoch=excluded.stage_epoch,runtime=excluded.runtime,generation=excluded.generation WHERE semon_pg_native_catalog_stage.namespace=excluded.namespace AND semon_pg_native_catalog_stage.stage_epoch=$5", &[&ns, &epoch, &bound.as_bytes(), &generation.as_bytes(), &old_epoch.unwrap_or(0)]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,$2,$3,$4,$5)", &[&ns, &operation, &payload, &generation.as_bytes(), &epoch]).map_err(sqlstate::error)?;
    tx.driver.execute("INSERT INTO semon_pg_native_catalog_changes(namespace,generation,previous,stage_epoch) VALUES($1,$2,$3,$4)", &[&ns, &generation.as_bytes(), &expected.map(str::as_bytes), &epoch]).map_err(sqlstate::error)?;
    tx.check()?;
    Ok((Outcome::Written, Some(generation)))
}
