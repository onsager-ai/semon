//! Non-authorizing provenance for the first ordinary cold publication.
//! Cumulative semantic publication is deliberately not enabled by this module.
use crate::{
    events::store::configured::NativeOwnership,
    model::relation_stage::{OriginId, OriginPublication},
    storage::StorageError,
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

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
        "SELECT origin_epoch,epoch FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND origin_phase='sealed' AND origin_runtime_bound=1 AND origin_admission=?3 AND origin_stream=?4 AND resetting=0 UNION ALL SELECT receipt.origin_epoch,receipt.held_epoch FROM main.catalog_semantic_origin_publications receipt JOIN main.catalog_semantic_stage stage ON stage.namespace=receipt.namespace AND stage.origin_epoch=receipt.origin_epoch WHERE receipt.namespace=?1 AND receipt.admission=?3 AND receipt.stream=?4 AND stage.origin_phase='draining' AND stage.origin_runtime_bound=1 AND stage.origin_admission=receipt.admission AND stage.origin_stream=receipt.stream AND stage.origin_first_generation=receipt.generation AND stage.resetting=0",
        params![owner.config().namespace.as_str(), held, admission, owner.config().expected_stream_epoch],
        |row| Ok(OriginPublication { origin: OriginId(row.get(0)?), held_epoch: row.get(1)? }),
    ).optional()
}

fn request(
    token: &OriginPublication,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
) -> Result<String, StorageError> {
    let mut digest = Sha256::new();
    digest.update(b"semon-cold-origin-publication-v1\0");
    let mut frame = |bytes: Vec<u8>| {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    };
    frame(
        serde_json::to_vec(&(token, claims, publication.rows.len()))
            .map_err(|_| StorageError::Unavailable)?,
    );
    for row in publication.rows {
        frame(serde_json::to_vec(row).map_err(|_| StorageError::Unavailable)?);
    }
    frame(
        serde_json::to_vec(&publication.transcripts.is_some())
            .map_err(|_| StorageError::Unavailable)?,
    );
    if let Some(projections) = publication.transcripts {
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
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn replay(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
) -> Result<Option<String>, StorageError> {
    writer(db, owner)?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let receipt: Option<(String, String, bool)> = db.query_row(
        "SELECT request_digest,generation,admission=?4 AND stream=?5 FROM main.catalog_semantic_origin_publications WHERE namespace=?1 AND origin_epoch=?2 AND held_epoch=?3",
        params![owner.config().namespace.as_str(), token.origin.0, token.held_epoch, admission, owner.config().expected_stream_epoch],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(super::failed)?;
    match receipt {
        Some((digest, generation, true)) if digest == request(token, publication, claims)? => {
            Ok(Some(generation))
        }
        Some(_) => Err(StorageError::Conflict),
        None => Ok(None),
    }
}

pub(crate) fn validate_publication(
    db: &Connection,
    owner: &NativeOwnership,
    token: &OriginPublication,
    claims: &[(String, Option<i64>)],
) -> Result<(), StorageError> {
    writer(db, owner)?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let ns = owner.config().namespace.as_str();
    let valid: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND origin_epoch=?3 AND origin_phase='sealed' AND origin_admission=?4 AND origin_stream=?5 AND origin_runtime_bound=1 AND generation IS NULL AND origin_first_generation IS NULL AND resetting=0)",
        params![ns, token.held_epoch, token.origin.0, admission, owner.config().expected_stream_epoch], |row| row.get(0),
    ).map_err(super::failed)?;
    if !valid || claims.len() > 64 {
        return Err(StorageError::Conflict);
    }
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
    publication: &crate::slot_projection::Publication<'_>,
    claims: &[(String, Option<i64>)],
    generation: &str,
) -> Result<(), StorageError> {
    validate_publication(db, owner, token, claims)?;
    let admission = i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
    let ns = owner.config().namespace.as_str();
    let rows = publication.rows;
    if rows.len() > 64 {
        return Err(StorageError::InvalidArguments);
    }
    for row in rows {
        let metadata = serde_json::to_vec(row).map_err(|_| StorageError::Unavailable)?;
        db.execute(
            "INSERT INTO main.catalog_semantic_origin_owners(namespace,origin_epoch,session_key,metadata_digest,publication) VALUES(?1,?2,?3,?4,?5)",
            params![ns, token.origin.0, row.key, format!("{:x}", Sha256::digest(metadata)), generation],
        ).map_err(super::failed)?;
    }
    for (path, claim) in claims {
        // Only the scalar Ledger representation is retained. This witness is
        // never an input to native Raw/source authorization or lease admission.
        if db.execute(
            "INSERT INTO main.catalog_semantic_origin_sources(namespace,origin_epoch,source_path,accepted_claim,acknowledged,dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256) SELECT ?1,?2,?3,?4,1,dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM main.files WHERE namespace=?1 AND path=?3",
            params![ns, token.origin.0, path, claim],
        ).map_err(super::failed)? != 1 {
            return Err(StorageError::Conflict);
        }
        db.execute("UPDATE main.catalog_semantic_sources SET claim=NULL WHERE namespace=?1 AND source_path=?2 AND origin_member=1", params![ns,path]).map_err(super::failed)?;
    }
    db.execute(
        "INSERT INTO main.catalog_semantic_origin_publications(namespace,origin_epoch,held_epoch,request_digest,generation,admission,stream) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![ns, token.origin.0, token.held_epoch, request(token, publication, claims)?, generation, admission, owner.config().expected_stream_epoch],
    ).map_err(super::failed)?;
    if db.execute(
        "UPDATE main.catalog_semantic_stage SET generation=?6,origin_phase='draining',origin_first_generation=?6 WHERE namespace=?1 AND epoch=?2 AND origin_epoch=?3 AND origin_admission=?4 AND origin_stream=?5 AND origin_phase='sealed' AND origin_runtime_bound=1 AND generation IS NULL AND origin_first_generation IS NULL AND resetting=0",
        params![ns, token.held_epoch, token.origin.0, admission, owner.config().expected_stream_epoch, generation],
    ).map_err(super::failed)? != 1 {
        return Err(StorageError::Conflict);
    }
    Ok(())
}
