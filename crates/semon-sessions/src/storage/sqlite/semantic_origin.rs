//! Non-authorizing provenance for the first ordinary cold publication.
//! Cumulative semantic publication is deliberately not enabled by this module.
use crate::{
    events::store::configured::NativeOwnership,
    model::relation_stage::{OriginId, OriginPublication},
    storage::StorageError,
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn writer(db: &Connection, owner: &NativeOwnership) -> Result<(), StorageError> {
    owner.validate_ready(db)?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let valid: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_semantic_profiles WHERE namespace=?1 AND admission=?2 AND recipe=?3)",
        params![owner.config().namespace.as_str(), admission, crate::events::store::configured::semantic_recipe()],
        |row| row.get(0),
    ).map_err(super::failed)?;
    if !valid {
        return Err(StorageError::NamespaceUpgradePending);
    }
    db.create_scalar_function(
        "semon_semantic_writer",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(super::failed)
}

pub(crate) fn empty_current(db: &Connection, ns: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM main.session_catalog WHERE namespace=?1 AND lifecycle='current') AND NOT EXISTS(SELECT 1 FROM main.session_catalog_sources WHERE namespace=?1 AND lifecycle='current') AND NOT EXISTS(SELECT 1 FROM main.storage_catalog_native_observation_grants WHERE namespace=?1 AND owner_kind='catalog') AND NOT EXISTS(SELECT 1 FROM main.storage_proof_refs WHERE namespace=?1 AND kind='current')",
        [ns], |row| row.get(0),
    )
}

/// Called only inside the owned journal's already reserved discovery transaction.
pub(crate) fn discover(
    db: &Connection,
    owner: &NativeOwnership,
    paths: &[String],
    complete: bool,
) -> Result<(), StorageError> {
    if paths.len() > 64 {
        return Err(StorageError::InvalidArguments);
    }
    writer(db, owner)?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let ns = owner.config().namespace.as_str();
    let stage: Option<Option<String>> = db
        .query_row(
            "SELECT origin_phase FROM main.catalog_semantic_stage WHERE namespace=?1",
            [ns],
            |row| row.get(0),
        )
        .optional()
        .map_err(super::failed)?;
    if stage.is_none() {
        if super::head(db).map_err(super::failed)?.is_some() {
            return Ok(());
        }
        // Missing derived head metadata must never turn an existing family into
        // a new cold baseline. All lookups are namespace-leading existence seeks.
        if !empty_current(db, ns).map_err(super::failed)? {
            return Err(StorageError::Conflict);
        }
        if db.execute("UPDATE storage_semantic_profiles SET epoch=epoch+1 WHERE namespace=?1 AND epoch<9223372036854775807", [ns]).map_err(super::failed)? != 1 {
            return Err(StorageError::Conflict);
        }
        let stream = owner
            .config()
            .expected_stream_epoch
            .as_deref()
            .ok_or(StorageError::ScopeChanged)?;
        db.execute(
            "INSERT INTO main.catalog_semantic_stage(namespace,singleton,epoch,phase,generation,runtime,revision,origin_epoch,origin_phase,origin_admission,origin_stream) SELECT ?1,1,epoch,'indexing',NULL,'',(SELECT revision FROM main.storage_source_journal WHERE namespace=?1),epoch,'discovering',?2,?3 FROM storage_semantic_profiles WHERE namespace=?1",
            params![ns, admission, stream],
        ).map_err(super::failed)?;
    } else if stage != Some(Some("discovering".into())) {
        // Ordinary discovery still journals new paths. A sealed/draining origin
        // does not enroll those paths or let them inherit its comparison scope.
        return Ok(());
    }
    let valid: bool = db.query_row(
        "SELECT origin_admission=?2 AND origin_stream=?3 AND generation IS NULL FROM main.catalog_semantic_stage WHERE namespace=?1",
        params![ns, admission, owner.config().expected_stream_epoch], |row| row.get(0),
    ).map_err(super::failed)?;
    if !valid || super::head(db).map_err(super::failed)?.is_some() {
        return Err(StorageError::Conflict);
    }
    for (path, claim) in
        super::super::source_journal::claims(db, ns, paths).map_err(super::failed)?
    {
        db.execute(
            "INSERT INTO main.catalog_semantic_sources(namespace,source_path,claim,origin_member,origin_claim) VALUES(?1,?2,?3,1,?3) ON CONFLICT(namespace,source_path) DO NOTHING",
            params![ns, path, claim],
        ).map_err(super::failed)?;
    }
    if complete {
        db.execute(
            "UPDATE main.catalog_semantic_stage SET origin_phase='sealed',origin_sealed_revision=(SELECT revision FROM main.storage_source_journal WHERE namespace=?1) WHERE namespace=?1 AND origin_phase='discovering'",
            [ns],
        ).map_err(super::failed)?;
    }
    Ok(())
}

pub(crate) fn pending(db: &Connection, ns: &str) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND origin_phase IN('discovering','sealed'))", [ns], |row| row.get(0))
}

pub(crate) fn publication(
    db: &Connection,
    owner: &NativeOwnership,
    held: Option<i64>,
) -> rusqlite::Result<Option<OriginPublication>> {
    let admission = i64::try_from(owner.admission())
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    // A committed first group retains its original held epoch. Reopening a
    // producer must recover that receipt token, rather than treating the next
    // group as an uncommitted publication under the live stage epoch.
    db.query_row(
        "SELECT origin_epoch,epoch,key_epoch FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND origin_phase='sealed' AND origin_runtime_bound=1 AND origin_admission=?3 AND origin_stream=?4 AND resetting=0 UNION ALL SELECT receipt.origin_epoch,receipt.held_epoch,receipt.key_epoch FROM main.catalog_semantic_origin_publications receipt JOIN main.catalog_semantic_stage stage ON stage.namespace=receipt.namespace AND stage.origin_epoch=receipt.origin_epoch WHERE receipt.namespace=?1 AND receipt.admission=?3 AND receipt.stream=?4 AND stage.origin_phase='draining' AND stage.origin_runtime_bound=1 AND stage.origin_admission=receipt.admission AND stage.origin_stream=receipt.stream AND stage.origin_first_generation=receipt.generation AND stage.resetting=0 ORDER BY 2,3 LIMIT 1",
        params![owner.config().namespace.as_str(), held, admission, owner.config().expected_stream_epoch],
        |row| Ok(OriginPublication { origin: OriginId(row.get(0)?), held_epoch: row.get(1)?, key_epoch: row.get(2)? }),
    ).optional()
}

fn cohort_digests(
    token: &OriginPublication,
    expected_generation: Option<&str>,
    model_witness: &str,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
) -> Result<(String, String), StorageError> {
    if publication.rows.len() > 64 || claims.len() > 64 {
        return Err(StorageError::InvalidArguments);
    }
    let mut selected_owners = BTreeSet::new();
    let mut selected_paths = BTreeSet::new();
    for row in publication.rows {
        if !selected_owners.insert(row.key.clone()) {
            return Err(StorageError::InvalidArguments);
        }
        selected_paths.extend(
            row.sources
                .iter()
                .map(|source| source.path.to_string_lossy().into_owned()),
        );
    }
    let mut selected_claims = BTreeMap::new();
    for (path, claim) in claims {
        if selected_claims.insert(path.clone(), *claim).is_some() {
            return Err(StorageError::InvalidArguments);
        }
    }
    let claimed_paths = selected_claims.keys().cloned().collect::<BTreeSet<_>>();
    if selected_paths != claimed_paths {
        return Err(StorageError::InvalidArguments);
    }
    let selection = serde_json::to_vec(&(
        token.origin,
        token.held_epoch,
        token.key_epoch,
        expected_generation,
        selected_owners,
        selected_claims,
    ))
    .map_err(|_| StorageError::Unavailable)?;
    let mut selection_digest = Sha256::new();
    selection_digest.update(b"semon-cold-origin-selection-v2\0");
    selection_digest.update((selection.len() as u64).to_be_bytes());
    selection_digest.update(selection);
    let selection_digest = format!("{:x}", selection_digest.finalize());

    let mut digest = Sha256::new();
    digest.update(b"semon-cold-origin-group-v2\0");
    digest.update((selection_digest.len() as u64).to_be_bytes());
    digest.update(selection_digest.as_bytes());
    let mut frame = |bytes: Vec<u8>| {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    };
    frame(model_witness.as_bytes().to_vec());
    frame(serde_json::to_vec(&publication.rows.len()).map_err(|_| StorageError::Unavailable)?);
    for row in publication.rows {
        frame(serde_json::to_vec(row).map_err(|_| StorageError::Unavailable)?);
    }
    frame(
        serde_json::to_vec(&publication.transcripts.is_some())
            .map_err(|_| StorageError::Unavailable)?,
    );
    if let Some(projections) = publication.transcripts {
        frame(serde_json::to_vec(&projections.len()).map_err(|_| StorageError::Unavailable)?);
        for projection in projections {
            frame(
                serde_json::to_vec(&(&projection.key, projection.repair, projection.slots.len()))
                    .map_err(|_| StorageError::Unavailable)?,
            );
            for slot in &projection.slots {
                frame(serde_json::to_vec(slot).map_err(|_| StorageError::Unavailable)?);
            }
        }
    }
    frame(serde_json::to_vec(&publication.observations).map_err(|_| StorageError::Unavailable)?);
    Ok((selection_digest, format!("{:x}", digest.finalize())))
}

pub(crate) fn replay(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    expected_generation: Option<&str>,
    model_witness: &str,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
) -> Result<Option<String>, StorageError> {
    writer(db, owner)?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let (selection_digest, group_digest) =
        cohort_digests(token, expected_generation, model_witness, publication, claims)?;
    let receipt: Option<(String, String, bool)> = db.query_row(
        "SELECT group_digest,generation,admission=?6 AND stream=?7 AND expected_generation IS ?8 FROM main.catalog_semantic_origin_publications WHERE namespace=?1 AND origin_epoch=?2 AND held_epoch=?3 AND key_epoch=?4 AND selection_digest=?5",
        params![owner.config().namespace.as_str(), token.origin.0, token.held_epoch, token.key_epoch, selection_digest, admission, owner.config().expected_stream_epoch, expected_generation],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(super::failed)?;
    match receipt {
        Some((digest, generation, true)) if digest == group_digest => {
            Ok(Some(generation))
        }
        Some(_) => Err(StorageError::Conflict),
        None => Ok(None),
    }
}

fn validate_cohort_members(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    expected_generation: Option<&str>,
    model_witness: &str,
    generation: &str,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
) -> Result<(), StorageError> {
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let ns = owner.config().namespace.as_str();
    let stage: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND origin_epoch=?3 AND key_epoch=?4 AND generation=?5 AND origin_admission=?6 AND origin_stream=?7 AND origin_runtime_bound=1 AND resetting=0 AND origin_phase='draining')",
            params![ns, token.held_epoch, token.origin.0, token.key_epoch, generation, admission, owner.config().expected_stream_epoch],
            |row| row.get(0),
        )
        .map_err(super::failed)?;
    if !stage || super::head(db).map_err(super::failed)?.as_deref() != Some(generation) {
        return Err(StorageError::Conflict);
    }
    let _ = cohort_digests(
        token,
        expected_generation,
        model_witness,
        publication,
        claims,
    )?;
    for (path, _) in claims {
        let enrolled: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND origin_member=1)",
            params![ns, path],
            |row| row.get(0),
        ).map_err(super::failed)?;
        if !enrolled {
            return Err(StorageError::Conflict);
        }
    }
    Ok(())
}

/// Preserve matching source evidence and withdraw owner comparisons touched by
/// the selected claim before its journal row is removed. A later validated
/// publication may write successor comparisons in this same transaction.
pub(crate) fn invalidate_comparisons(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    owners: &BTreeSet<String>,
    claims: &[(String, Option<i64>)],
) -> Result<(), StorageError> {
    writer(db, owner)?;
    let ns = owner.config().namespace.as_str();
    for key in owners {
        db.execute(
            "DELETE FROM main.catalog_semantic_origin_owners WHERE namespace=?1 AND origin_epoch=?2 AND session_key=?3",
            params![ns, token.origin.0, key],
        )
        .map_err(super::failed)?;
    }
    for (path, claim) in claims {
        let same: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_origin_sources witness JOIN main.files file ON file.namespace=witness.namespace AND file.path=witness.source_path WHERE witness.namespace=?1 AND witness.origin_epoch=?2 AND witness.source_path=?3 AND witness.accepted_claim IS ?4 AND witness.acknowledged=1 AND witness.dev IS file.dev AND witness.ino IS file.ino AND witness.size IS file.size AND witness.mtime_ns IS file.mtime_ns AND witness.ctime_ns IS file.ctime_ns AND witness.resume_at IS file.resume_at AND witness.head_sha256 IS file.head_sha256 AND witness.tail_sha256 IS file.tail_sha256)",
                params![ns, token.origin.0, path, claim],
                |row| row.get(0),
            )
            .map_err(super::failed)?;
        if !same {
            db.execute(
                "DELETE FROM main.catalog_semantic_origin_sources WHERE namespace=?1 AND origin_epoch=?2 AND source_path=?3",
                params![ns, token.origin.0, path],
            )
            .map_err(super::failed)?;
        }
    }
    Ok(())
}

/// The publisher calls this after its selected Catalog/native effects have
/// passed validation, in the transaction that will commit their receipt.
pub(crate) fn record_receipt(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    expected_generation: Option<&str>,
    model_witness: &str,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
    generation: &str,
) -> Result<(), StorageError> {
    writer(db, owner)?;
    validate_cohort_members(
        db,
        owner,
        token,
        expected_generation,
        model_witness,
        generation,
        publication,
        claims,
    )?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let (selection_digest, group_digest) =
        cohort_digests(token, expected_generation, model_witness, publication, claims)?;
    db.execute(
        "INSERT INTO main.catalog_semantic_origin_publications(namespace,origin_epoch,held_epoch,key_epoch,selection_digest,group_digest,expected_generation,generation,admission,stream) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![owner.config().namespace.as_str(), token.origin.0, token.held_epoch, token.key_epoch, selection_digest, group_digest, expected_generation, generation, admission, owner.config().expected_stream_epoch],
    )
    .map_err(super::failed)?;
    Ok(())
}

pub(crate) fn validate_publication(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    expected_generation: Option<&str>,
    model_witness: &str,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
) -> Result<(), StorageError> {
    writer(db, owner)?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let ns = owner.config().namespace.as_str();
    let valid: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND origin_epoch=?3 AND key_epoch=?4 AND origin_phase='sealed' AND origin_admission=?5 AND origin_stream=?6 AND origin_runtime_bound=1 AND generation IS NULL AND origin_first_generation IS NULL AND resetting=0 AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_origin_publications receipt WHERE receipt.namespace=main.catalog_semantic_stage.namespace AND receipt.origin_epoch=main.catalog_semantic_stage.origin_epoch AND receipt.held_epoch=main.catalog_semantic_stage.epoch AND receipt.key_epoch=main.catalog_semantic_stage.key_epoch))",
        params![ns, token.held_epoch, token.origin.0, token.key_epoch, admission, owner.config().expected_stream_epoch], |row| row.get(0),
    ).map_err(super::failed)?;
    if !valid || expected_generation.is_some() {
        return Err(StorageError::Conflict);
    }
    let _ = cohort_digests(token, expected_generation, model_witness, publication, claims)?;
    for (path, _) in claims {
        let enrolled: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND origin_member=1)", params![ns,path], |row| row.get(0)).map_err(super::failed)?;
        if !enrolled {
            return Err(StorageError::Conflict);
        }
    }
    Ok(())
}

/// Ordinary native validation and head CAS run before this hook. Receipt capture,
/// exact claim ACK, native grants and COMMIT remain in that same transaction.
pub(crate) fn record(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    expected_generation: Option<&str>,
    model_witness: &str,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
    generation: &str,
) -> Result<(), StorageError> {
    validate_publication(
        db,
        owner,
        token,
        expected_generation,
        model_witness,
        publication,
        claims,
    )?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let ns = owner.config().namespace.as_str();
    for row in publication.rows {
        let metadata = serde_json::to_vec(row).map_err(|_| StorageError::Unavailable)?;
        db.execute(
            "INSERT INTO main.catalog_semantic_origin_owners(namespace,origin_epoch,session_key,metadata_digest,publication) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,origin_epoch,session_key) DO UPDATE SET metadata_digest=excluded.metadata_digest,publication=excluded.publication",
            params![ns, token.origin.0, row.key, format!("{:x}", Sha256::digest(metadata)), generation],
        ).map_err(super::failed)?;
    }
    for (path, claim) in claims {
        // Only the scalar Ledger representation is retained. This witness is
        // never an input to native Raw/source authorization or lease admission.
        let same: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_origin_sources witness JOIN main.files file ON file.namespace=witness.namespace AND file.path=witness.source_path WHERE witness.namespace=?1 AND witness.origin_epoch=?2 AND witness.source_path=?3 AND witness.accepted_claim IS ?4 AND witness.acknowledged=1 AND witness.dev IS file.dev AND witness.ino IS file.ino AND witness.size IS file.size AND witness.mtime_ns IS file.mtime_ns AND witness.ctime_ns IS file.ctime_ns AND witness.resume_at IS file.resume_at AND witness.head_sha256 IS file.head_sha256 AND witness.tail_sha256 IS file.tail_sha256)",
            params![ns, token.origin.0, path, claim],
            |row| row.get(0),
        ).map_err(super::failed)?;
        if !same && db.execute(
            "INSERT INTO main.catalog_semantic_origin_sources(namespace,origin_epoch,source_path,accepted_claim,acknowledged,dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256) SELECT ?1,?2,?3,?4,1,dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM main.files WHERE namespace=?1 AND path=?3 ON CONFLICT(namespace,origin_epoch,source_path) DO UPDATE SET accepted_claim=excluded.accepted_claim,acknowledged=1,dev=excluded.dev,ino=excluded.ino,size=excluded.size,mtime_ns=excluded.mtime_ns,ctime_ns=excluded.ctime_ns,resume_at=excluded.resume_at,head_sha256=excluded.head_sha256,tail_sha256=excluded.tail_sha256",
            params![ns, token.origin.0, path, claim],
        ).map_err(super::failed)? != 1 {
            return Err(StorageError::Conflict);
        }
        let enrolled: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND origin_member=1)",
            params![ns, path],
            |row| row.get(0),
        ).map_err(super::failed)?;
        if !enrolled {
            return Err(StorageError::Conflict);
        }
        db.execute(
            "UPDATE main.catalog_semantic_sources SET claim=NULL,ack_done=1 WHERE namespace=?1 AND source_path=?2 AND origin_member=1 AND (claim IS NOT NULL OR ack_done=0)",
            params![ns, path],
        )
        .map_err(super::failed)?;
    }
    if db.execute(
        "UPDATE main.catalog_semantic_stage SET generation=?7,origin_phase='draining',origin_first_generation=?7 WHERE namespace=?1 AND epoch=?2 AND origin_epoch=?3 AND key_epoch=?4 AND origin_admission=?5 AND origin_stream=?6 AND origin_phase='sealed' AND origin_runtime_bound=1 AND generation IS NULL AND origin_first_generation IS NULL AND resetting=0",
        params![ns, token.held_epoch, token.origin.0, token.key_epoch, admission, owner.config().expected_stream_epoch, generation],
    ).map_err(super::failed)? != 1 {
        return Err(StorageError::Conflict);
    }
    record_receipt(
        db,
        owner,
        token,
        expected_generation,
        model_witness,
        publication,
        claims,
        generation,
    )?;
    Ok(())
}
