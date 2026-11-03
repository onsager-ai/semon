//! Owned immutable selected metadata and durable reference/lease union.
//! No Raw deletion, repair, provider creation or Options database fallback.
#![allow(dead_code)] // Private qualification cohort; public factory remains closed.
use super::failed;
use crate::events::store::configured::NativeOwnership;
use crate::model::summary::CatalogRow;
use crate::storage::catalog::CatalogObservationOwner;
use crate::storage::proof_codec::{self, LEASE_CAP, PresentationManifest, TTL_MAX};
use crate::storage::proof_lifetime::{
    ProofGenerationId, ProofReaderLease, ReaderPage, ReaderSnapshot,
};
use crate::storage::{NativeReaderBinding, StorageError};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const DIRECT_CAPTURE_MAX_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
std::thread_local! {
    static FAIL_PRESENTATION_CAPTURE_ONCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn fail_presentation_capture_once() {
    FAIL_PRESENTATION_CAPTURE_ONCE.with(|value| value.set(true));
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS storage_proof_generations(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,publication TEXT NOT NULL,proof TEXT NOT NULL,admission INTEGER NOT NULL,stream TEXT NOT NULL,metadata TEXT NOT NULL,query_recipe TEXT CHECK(query_recipe IS NULL OR octet_length(metadata)+octet_length(query_recipe)<=1048576),slots TEXT NOT NULL,prefix TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_presentations(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_presentation_roots(namespace TEXT NOT NULL,publication TEXT NOT NULL,base TEXT NOT NULL,manifest TEXT NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(namespace,digest)) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS storage_proof_presentation_immutable BEFORE UPDATE ON storage_proof_presentations BEGIN SELECT RAISE(ABORT,'immutable presentation'); END;
CREATE TRIGGER IF NOT EXISTS storage_proof_presentation_root_immutable BEFORE UPDATE ON storage_proof_presentation_roots BEGIN SELECT RAISE(ABORT,'immutable presentation root'); END;
CREATE TABLE IF NOT EXISTS storage_proof_sources(namespace TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,path TEXT NOT NULL,ledger TEXT NOT NULL,PRIMARY KEY(namespace,id,session_key,path)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_proof_source_path ON storage_proof_sources(namespace,path,ledger,id);
CREATE TABLE IF NOT EXISTS storage_proof_refs(namespace TEXT NOT NULL,session_key TEXT NOT NULL,kind TEXT NOT NULL CHECK(kind IN('current','history')),id TEXT NOT NULL,PRIMARY KEY(namespace,session_key,kind,id)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_catalog_native_observation_grants(namespace TEXT NOT NULL,session_key TEXT NOT NULL,owner_kind TEXT NOT NULL CHECK(owner_kind IN('catalog','history')),metadata_digest TEXT NOT NULL CHECK(length(metadata_digest)=64 AND metadata_digest NOT GLOB '*[^0-9a-f]*'),admission INTEGER NOT NULL CHECK(admission>0),stream TEXT NOT NULL CHECK(length(stream)=64 AND lower(stream) NOT GLOB '*[^0-9a-f]*'),direct_capture TEXT CHECK(direct_capture IS NULL OR octet_length(direct_capture)<=8388608),PRIMARY KEY(namespace,session_key,owner_kind)) STRICT, WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS storage_catalog_native_observation_grants_insert BEFORE INSERT ON storage_catalog_native_observation_grants WHEN semon_semantic_publisher()<>1 BEGIN SELECT RAISE(ABORT,'native observation publisher refused'); END;
CREATE TRIGGER IF NOT EXISTS storage_catalog_native_observation_grants_update BEFORE UPDATE ON storage_catalog_native_observation_grants WHEN semon_semantic_publisher()<>1 BEGIN SELECT RAISE(ABORT,'native observation publisher refused'); END;
CREATE TRIGGER IF NOT EXISTS storage_catalog_native_observation_grants_delete BEFORE DELETE ON storage_catalog_native_observation_grants WHEN semon_semantic_publisher()<>1 BEGIN SELECT RAISE(ABORT,'native observation publisher refused'); END;
CREATE TABLE IF NOT EXISTS storage_proof_leases(namespace TEXT NOT NULL,token TEXT NOT NULL,id TEXT NOT NULL,session_key TEXT NOT NULL,admission INTEGER NOT NULL,stream TEXT NOT NULL,owner_epoch INTEGER NOT NULL,serial INTEGER NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(namespace,token)) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS storage_proof_reader_owners(namespace TEXT PRIMARY KEY,epoch INTEGER NOT NULL CHECK(epoch>0),next_lease INTEGER NOT NULL CHECK(next_lease>=0)) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_proof_lease_generation ON storage_proof_leases(namespace,id,session_key,expires);
CREATE TRIGGER IF NOT EXISTS storage_proof_generation_immutable BEFORE UPDATE ON storage_proof_generations BEGIN SELECT RAISE(ABORT,'immutable proof generation'); END;
CREATE TRIGGER IF NOT EXISTS storage_proof_source_immutable BEFORE UPDATE ON storage_proof_sources BEGIN SELECT RAISE(ABORT,'immutable proof source'); END;
";

pub(super) fn initialize(db: &Connection) -> Result<(), StorageError> {
    db.execute_batch(SCHEMA).map_err(failed)?;
    for statement in SCHEMA.lines().map(str::trim) {
        if let Some(rest) = statement.strip_prefix("CREATE TABLE IF NOT EXISTS ") {
            let name = rest.split('(').next().ok_or(StorageError::Unavailable)?;
            super::catalog_namespace::guard(db, name)?;
        }
    }
    validate(db)
}

pub(super) fn validate(db: &Connection) -> Result<(), StorageError> {
    // Compare the complete authored current physical contract, including key
    // order, STRICT/WITHOUT ROWID, indexes and immutable triggers. Never adopt
    // a partial shape.
    // Each authored definition occupies one line. Trigger bodies contain an
    // internal semicolon; keep the complete body when comparing sqlite_schema.
    for statement in SCHEMA
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let statement = statement
            .strip_suffix(';')
            .ok_or(StorageError::Unavailable)?;
        let Some((kind, rest)) = ["TABLE", "INDEX", "TRIGGER"].into_iter().find_map(|kind| {
            statement
                .strip_prefix(&format!("CREATE {kind} IF NOT EXISTS "))
                .map(|rest| (kind, rest))
        }) else {
            continue;
        };
        let name = if kind == "TABLE" {
            rest.split('(').next()
        } else {
            rest.split_whitespace().next()
        }
        .ok_or(StorageError::Unavailable)?;
        let actual: Option<String> = db
            .query_row(
                "SELECT sql FROM main.sqlite_schema WHERE type=?1 AND name=?2",
                params![kind.to_ascii_lowercase(), name],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)?;
        let expected = format!("CREATE {kind} {rest}");
        if actual.as_deref() != Some(expected.as_str()) {
            return Err(StorageError::NamespaceUpgradePending);
        }
        if kind == "TABLE" {
            super::catalog_namespace::validate_guard(db, name)?;
        }
    }
    Ok(())
}

pub(crate) struct PresentationInputs<'a> {
    pub(crate) catalog_generation: &'a str,
    pub(crate) rows: &'a [CatalogRow],
    pub(crate) projections: Option<&'a [crate::slot_projection::Projection]>,
    pub(crate) observations: Option<&'a crate::storage::selected::PresentationObservations>,
}

pub(crate) struct PreparedPresentations<'a> {
    pub(crate) inputs: PresentationInputs<'a>,
    pub(crate) presentations: &'a [(String, String)],
    pub(crate) overview_budget: &'a mut crate::overview::SummaryPublicationBudget,
}

/// Request and owner fences shared by ordinary issuance and replay.
pub(crate) struct OrdinaryReceipt<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) admission: u64,
    pub(crate) stream: &'a str,
    pub(crate) base_generation: Option<&'a str>,
    pub(crate) source_revision: i64,
    pub(crate) claims: &'a str,
    pub(crate) metadata: &'a str,
}

struct PreparedPresentationRoot {
    base: String,
    manifest: String,
    digest: String,
}

struct PreparedProofSource {
    path: String,
    ledger: String,
}

struct PreparedProofRow {
    key: String,
    metadata: String,
    query_recipe: Option<String>,
    slots: String,
    prefix: String,
    presentation: String,
    sources: Vec<PreparedProofSource>,
}

struct PreparedProofPublication {
    id: String,
    publication: String,
    proof: String,
    admission: i64,
    stream: String,
    rows: Vec<PreparedProofRow>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExistingReceipt {
    Fresh,
    ExactReplay,
}

/// Validate the selected-receipt input shape shared by ordinary and Stage
/// issuers. Unknown observation fields remain valid; the typed object itself
/// and one projection per Catalog row are required for a nonempty receipt.
fn validate_receipt_inputs(inputs: &PresentationInputs<'_>) -> Result<(), StorageError> {
    let PresentationInputs {
        rows,
        projections,
        observations,
        ..
    } = inputs;
    if rows.is_empty() {
        return Ok(());
    }
    if observations.is_none() {
        return Err(StorageError::Unavailable);
    }
    let projections = projections.ok_or(StorageError::Unavailable)?;
    let mut row_keys = std::collections::BTreeSet::new();
    if rows.iter().any(|row| !row_keys.insert(row.key.as_str())) {
        return Err(StorageError::ScopeChanged);
    }
    let mut projection_keys = std::collections::BTreeSet::new();
    if projections
        .iter()
        .any(|projection| !projection_keys.insert(projection.key.as_str()))
    {
        return Err(StorageError::ScopeChanged);
    }
    if row_keys.iter().any(|key| !projection_keys.contains(key)) {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

fn validate_receipt_presentations(
    rows: &[CatalogRow],
    presentations: &[(String, String)],
) -> Result<(), StorageError> {
    if rows.len() != presentations.len() {
        return Err(StorageError::ScopeChanged);
    }
    let row_keys = rows
        .iter()
        .map(|row| row.key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let presentation_keys = presentations
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if row_keys.len() != rows.len()
        || presentation_keys.len() != presentations.len()
        || row_keys != presentation_keys
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum PublicationError {
    Storage(StorageError),
    BudgetOverflow,
}

impl From<StorageError> for PublicationError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

#[cfg(test)]
std::thread_local! {
    static PREPARED_QUERY_RECIPE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Qualify an inactive payload writer in one fixture's ordinary publication.
/// The guard changes no authority and never escapes the calling thread.
#[cfg(test)]
pub(crate) fn with_prepared_query_recipe_for_test<T>(publish: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PREPARED_QUERY_RECIPE.with(|enabled| enabled.set(self.0));
        }
    }
    let _restore = Restore(PREPARED_QUERY_RECIPE.with(|enabled| enabled.replace(true)));
    publish()
}

#[cfg(test)]
fn prepared_query_recipe_for_test(
    row: &CatalogRow,
    metadata: &str,
) -> Result<Option<String>, StorageError> {
    if !PREPARED_QUERY_RECIPE.with(std::cell::Cell::get) {
        return Ok(None);
    }
    let recipe = row
        .query_recipe
        .as_ref()
        .ok_or(StorageError::ScopeChanged)?;
    let payload = serde_json::to_string(recipe).map_err(|_| StorageError::Unavailable)?;
    proof_codec::validate_query_recipe_cell(metadata, Some(&payload))?;
    crate::query_projection::SelectedRecipe::decode(row, &payload)
        .map_err(|_| StorageError::ScopeChanged)?;
    Ok(Some(payload))
}

pub(crate) fn publish(
    tx: &Transaction<'_>,
    namespace: &str,
    admission: u64,
    stream: &str,
    publication: &str,
    proof: &str,
    presentations: PreparedPresentations<'_>,
) -> Result<(), PublicationError> {
    validate_receipt_inputs(&presentations.inputs)?;
    validate_receipt_presentations(presentations.inputs.rows, presentations.presentations)?;
    let PreparedPresentations {
        inputs,
        presentations: captured_presentations,
        overview_budget,
    } = presentations;
    let prepared = prepare_proof_publication(
        namespace,
        admission,
        stream,
        publication,
        proof,
        &inputs,
        captured_presentations,
    )?;
    for row in &prepared.rows {
        insert_proof_row(tx, namespace, &prepared, row)?;
        publish_current_ref(
            tx,
            namespace,
            admission,
            stream,
            &prepared.id,
            &row.key,
            overview_budget,
        )?;
    }
    Ok(())
}

/// Publish an ordinary Catalog receipt. Immutable rows are inserted only for a
/// fresh receipt; a byte exact receipt replay still reapplies local Current
/// custody so later mutable owner reconciliation remains authoritative.
pub(crate) fn publish_ordinary(
    tx: &Transaction<'_>,
    receipt: &OrdinaryReceipt<'_>,
    base: &str,
    presentations: PreparedPresentations<'_>,
) -> Result<(), PublicationError> {
    let OrdinaryReceipt {
        namespace,
        admission,
        stream,
        base_generation,
        source_revision,
        claims,
        metadata,
    } = *receipt;
    validate_receipt_inputs(&presentations.inputs)?;
    validate_receipt_presentations(presentations.inputs.rows, presentations.presentations)?;
    let PreparedPresentations {
        inputs,
        presentations: captured_presentations,
        overview_budget,
    } = presentations;
    let root = prepare_presentation_root(base, captured_presentations, &inputs)?;
    let publication = catalog_publication_id(namespace, inputs.catalog_generation, base)?;
    let proof = root.digest.clone();
    let prepared = prepare_proof_publication(
        namespace,
        admission,
        stream,
        &publication,
        &proof,
        &inputs,
        captured_presentations,
    )?;
    let sql_admission = prepared.admission;
    let existing = classify_ordinary_receipt(tx, receipt, &publication, &root, &prepared)?;
    if existing == ExistingReceipt::Fresh {
        insert_presentation_root(tx, namespace, &publication, &root)?;
        tx.execute(
            "INSERT INTO storage_semantic_publications(namespace,generation,epoch,admission,base_generation,source_revision,proof_digest,claims,metadata) VALUES(?1,?2,NULL,?3,?4,?5,?6,?7,?8)",
            params![
                namespace,
                publication,
                sql_admission,
                base_generation,
                source_revision,
                proof,
                claims,
                metadata
            ],
        )
        .map_err(failed)?;
    }
    for row in &prepared.rows {
        if existing == ExistingReceipt::Fresh {
            insert_proof_row(tx, namespace, &prepared, row)?;
        }
        publish_current_ref(
            tx,
            namespace,
            admission,
            stream,
            &prepared.id,
            &row.key,
            overview_budget,
        )?;
    }
    Ok(())
}

/// Authenticate a retry against the currently owned immutable receipt. Captured
/// context comes from that receipt: a retry must not observe a new live context.
pub(crate) fn replay_ordinary(
    tx: &Transaction<'_>,
    receipt: &OrdinaryReceipt<'_>,
    inputs: PresentationInputs<'_>,
    metadata: &[String],
    observed_at: i64,
) -> Result<bool, StorageError> {
    validate_receipt_inputs(&inputs)?;
    let Some(first) = inputs.rows.first() else {
        return Ok(false);
    };
    let admission = i64::try_from(receipt.admission).map_err(|_| StorageError::ScopeChanged)?;
    let saved: Option<(String, String, String, String)> = tx
        .query_row(
            "SELECT g.id,g.publication,r.base,r.manifest FROM storage_proof_refs f JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current' AND g.admission=?3 AND g.stream=?4 AND p.epoch IS NULL AND p.base_generation IS ?5 AND p.source_revision=?6 AND p.claims=?7 AND p.metadata=?8 AND octet_length(r.manifest)<=1048576 ORDER BY g.id LIMIT 1",
            params![
                receipt.namespace,
                first.key,
                admission,
                receipt.stream,
                receipt.base_generation,
                receipt.source_revision,
                receipt.claims,
                receipt.metadata
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(failed)?;
    let Some((id, publication, base, manifest)) = saved else {
        return Ok(false);
    };
    let manifest = proof_codec::decode_presentation_manifest(&manifest)?;
    if manifest.catalog_generation != inputs.catalog_generation {
        return Ok(false);
    }
    let mut captured = Vec::with_capacity(inputs.rows.len());
    for row in inputs.rows {
        let text: String = tx
            .query_row(
                "SELECT metadata FROM storage_proof_presentations WHERE namespace=?1 AND id=?2 AND session_key=?3 AND octet_length(metadata)<=8388608",
                params![receipt.namespace, id, row.key],
                |saved| saved.get(0),
            )
            .optional()
            .map_err(failed)?
            .ok_or(StorageError::ScopeChanged)?;
        captured.push((row.key.clone(), text));
    }
    let projection_witness = inputs
        .projections
        .unwrap_or_default()
        .iter()
        .map(|projection| (&projection.key, &projection.slots, projection.repair))
        .collect::<Vec<_>>();
    let base_bytes = serde_json::to_vec(&(
        "ordinary-catalog-presentation-v1",
        inputs.catalog_generation,
        metadata,
        &projection_witness,
        inputs.observations,
        &captured,
        observed_at,
    ))
    .map_err(|_| StorageError::Unavailable)?;
    if proof_codec::digest(&base_bytes) != base
        || catalog_publication_id(receipt.namespace, inputs.catalog_generation, &base)?
            != publication
    {
        return Ok(false);
    }
    let root = prepare_presentation_root(&base, &captured, &inputs)?;
    let prepared = prepare_proof_publication(
        receipt.namespace,
        receipt.admission,
        receipt.stream,
        &publication,
        &root.digest,
        &inputs,
        &captured,
    )?;
    if prepared.id != id
        || classify_ordinary_receipt(tx, receipt, &publication, &root, &prepared)?
            != ExistingReceipt::ExactReplay
    {
        return Err(StorageError::ScopeChanged);
    }
    for row in inputs.rows {
        let current: bool = tx
            .query_row(
                "SELECT count(*)=1 AND min(id)=?3 FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current'",
                params![receipt.namespace, row.key, id],
                |saved| saved.get(0),
            )
            .map_err(failed)?;
        if !current {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(true)
}

fn prepare_proof_publication(
    namespace: &str,
    admission: u64,
    stream: &str,
    publication: &str,
    proof: &str,
    inputs: &PresentationInputs<'_>,
    presentations: &[(String, String)],
) -> Result<PreparedProofPublication, StorageError> {
    let id = ProofGenerationId::derive(namespace, admission, stream, proof);
    let sql_admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    let rows = inputs.rows;
    let projections = inputs.projections;
    let mut prepared_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let presentation = presentations
            .iter()
            .find(|(key, _)| key == &row.key)
            .ok_or(StorageError::Unavailable)?;
        let metadata = serde_json::to_string(row).map_err(|_| StorageError::Unavailable)?;
        let projection = projections
            .unwrap_or_default()
            .iter()
            .find(|projection| projection.key == row.key)
            .ok_or(StorageError::Unavailable)?;
        let slots =
            serde_json::to_string(&projection.slots).map_err(|_| StorageError::Unavailable)?;
        let prefix = crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::Unavailable)?;
        // These values came directly from the model inputs in this owner
        // transaction. Production does not decode this freshly encoded row.
        #[cfg(not(test))]
        let query_recipe: Option<String> = None;
        #[cfg(test)]
        let query_recipe = prepared_query_recipe_for_test(row, &metadata)?;
        let mut sources = row
            .sources
            .iter()
            .map(|source| PreparedProofSource {
                path: source.path.to_string_lossy().into_owned(),
                ledger: crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(
                    source,
                )),
            })
            .collect::<Vec<_>>();
        sources.sort_by(|left, right| left.path.cmp(&right.path));
        if sources.windows(2).any(|pair| pair[0].path == pair[1].path) {
            return Err(StorageError::ScopeChanged);
        }
        prepared_rows.push(PreparedProofRow {
            key: row.key.clone(),
            metadata,
            query_recipe,
            slots,
            prefix,
            presentation: presentation.1.clone(),
            sources,
        });
    }
    Ok(PreparedProofPublication {
        id: id.0,
        publication: publication.to_owned(),
        proof: proof.to_owned(),
        admission: sql_admission,
        stream: stream.to_owned(),
        rows: prepared_rows,
    })
}

fn insert_proof_row(
    tx: &Transaction<'_>,
    namespace: &str,
    publication: &PreparedProofPublication,
    row: &PreparedProofRow,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO storage_proof_generations(namespace,id,session_key,publication,proof,admission,stream,metadata,slots,prefix,query_recipe) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            namespace,
            publication.id,
            row.key,
            publication.publication,
            publication.proof,
            publication.admission,
            publication.stream,
            row.metadata,
            row.slots,
            row.prefix,
            row.query_recipe
        ],
    )
    .map_err(failed)?;
    tx.execute(
        "INSERT INTO storage_proof_presentations VALUES(?1,?2,?3,?4)",
        params![namespace, publication.id, row.key, row.presentation],
    )
    .map_err(failed)?;
    for source in &row.sources {
        tx.execute(
            "INSERT INTO storage_proof_sources VALUES(?1,?2,?3,?4,?5)",
            params![
                namespace,
                publication.id,
                row.key,
                source.path,
                source.ledger
            ],
        )
        .map_err(failed)?;
    }
    Ok(())
}

fn publish_current_ref(
    tx: &Transaction<'_>,
    namespace: &str,
    admission: u64,
    stream: &str,
    id: &str,
    session_key: &str,
    overview_budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<(), PublicationError> {
    retain_current_refs_for_unowned_history(
        tx,
        namespace,
        session_key,
        admission,
        stream,
        overview_budget,
    )?;
    tx.execute(
        "DELETE FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current'",
        params![namespace, session_key],
    )
    .map_err(failed)?;
    tx.execute(
        "INSERT INTO storage_proof_refs VALUES(?1,?2,'current',?3)",
        params![namespace, session_key, id],
    )
    .map_err(failed)?;
    // Exact History references are installed by the verified composer.
    // Reconstructing them from path/ledger matches would retain unrelated
    // generations and let imported proof sources acquire local authority.
    Ok(())
}

fn classify_ordinary_receipt(
    tx: &Transaction<'_>,
    receipt: &OrdinaryReceipt<'_>,
    publication: &str,
    root: &PreparedPresentationRoot,
    proof: &PreparedProofPublication,
) -> Result<ExistingReceipt, StorageError> {
    let OrdinaryReceipt {
        namespace,
        base_generation,
        source_revision,
        claims,
        metadata,
        ..
    } = *receipt;
    let semantic_count: i64 = tx
        .query_row(
            "SELECT count(*) FROM storage_semantic_publications WHERE namespace=?1 AND generation=?2",
            params![namespace, publication],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let semantic_matches: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_semantic_publications WHERE namespace=?1 AND generation=?2 AND epoch IS NULL AND admission=?3 AND base_generation IS ?4 AND source_revision=?5 AND proof_digest=?6 AND claims=?7 AND metadata=?8)",
            params![
                namespace,
                publication,
                proof.admission,
                base_generation,
                source_revision,
                proof.proof,
                claims,
                metadata
            ],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let root_count: i64 = tx
        .query_row(
            "SELECT count(*) FROM storage_proof_presentation_roots WHERE namespace=?1 AND (publication=?2 OR digest=?3)",
            params![namespace, publication, root.digest],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let generation_count: i64 = tx
        .query_row(
            "SELECT count(*) FROM storage_proof_generations WHERE namespace=?1 AND id=?2",
            params![namespace, proof.id],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let presentation_count: i64 = tx
        .query_row(
            "SELECT count(*) FROM storage_proof_presentations WHERE namespace=?1 AND id=?2",
            params![namespace, proof.id],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let source_count: i64 = tx
        .query_row(
            "SELECT count(*) FROM storage_proof_sources WHERE namespace=?1 AND id=?2",
            params![namespace, proof.id],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if semantic_count == 0
        && root_count == 0
        && generation_count == 0
        && presentation_count == 0
        && source_count == 0
    {
        return Ok(ExistingReceipt::Fresh);
    }
    let source_total = proof.rows.iter().try_fold(0usize, |total, row| {
        total
            .checked_add(row.sources.len())
            .ok_or(StorageError::ScopeChanged)
    })?;
    let expected_counts = (
        i64::try_from(proof.rows.len()).map_err(|_| StorageError::ScopeChanged)?,
        i64::try_from(proof.rows.len()).map_err(|_| StorageError::ScopeChanged)?,
        i64::try_from(source_total).map_err(|_| StorageError::ScopeChanged)?,
    );
    if semantic_count != 1
        || !semantic_matches
        || root_count != 1
        || generation_count != expected_counts.0
        || presentation_count != expected_counts.1
        || source_count != expected_counts.2
    {
        return Err(StorageError::ScopeChanged);
    }
    let root_matches: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_proof_presentation_roots WHERE namespace=?1 AND digest=?2 AND publication=?3 AND base=?4 AND manifest=?5)",
            params![namespace, root.digest, publication, root.base, root.manifest],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if !root_matches {
        return Err(StorageError::ScopeChanged);
    }
    for row in &proof.rows {
        let generation_matches: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3 AND publication=?4 AND proof=?5 AND admission=?6 AND stream=?7 AND metadata=?8 AND slots=?9 AND prefix=?10 AND query_recipe IS ?11)",
                params![
                    namespace,
                    proof.id,
                    row.key,
                    proof.publication,
                    proof.proof,
                    proof.admission,
                    proof.stream,
                    row.metadata,
                    row.slots,
                    row.prefix,
                    row.query_recipe.as_deref()
                ],
                |saved| saved.get(0),
            )
            .map_err(failed)?;
        if !generation_matches {
            return Err(StorageError::ScopeChanged);
        }
        let presentation_matches: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM storage_proof_presentations WHERE namespace=?1 AND id=?2 AND session_key=?3 AND metadata=?4)",
                params![namespace, proof.id, row.key, row.presentation],
                |saved| saved.get(0),
            )
            .map_err(failed)?;
        if !presentation_matches {
            return Err(StorageError::ScopeChanged);
        }
        for source in &row.sources {
            let source_matches: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM storage_proof_sources WHERE namespace=?1 AND id=?2 AND session_key=?3 AND path=?4 AND ledger=?5)",
                    params![namespace, proof.id, row.key, source.path, source.ledger],
                    |saved| saved.get(0),
                )
                .map_err(failed)?;
            if !source_matches {
                return Err(StorageError::ScopeChanged);
            }
        }
    }
    Ok(ExistingReceipt::ExactReplay)
}

/// Retain outgoing owners' exact Current receipts after History composition,
/// then release their Current references before grant reconciliation.
pub(crate) fn retain_outgoing_history_refs(
    tx: &Transaction<'_>,
    namespace: &str,
    admission: u64,
    stream: &str,
    outgoing_owners: &[CatalogRow],
    overview_budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<(), PublicationError> {
    // History refs are either the exact local composition set or retention
    // references copied from an exact local Current grant. They never grant
    // History origin.
    for row in outgoing_owners {
        retain_stage_history_origin(tx, namespace, admission, stream, row, overview_budget)?;
        retain_current_refs_for_unowned_history(
            tx,
            namespace,
            &row.key,
            admission,
            stream,
            overview_budget,
        )?;
        tx.execute(
            "DELETE FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current'",
            params![namespace, row.key],
        )
        .map_err(failed)?;
    }
    Ok(())
}

/// Retirement can transfer an exact local Stage owner into original History.
/// A transported receipt or an unrelated Current owner cannot issue this grant.
fn retain_stage_history_origin(
    db: &Connection,
    namespace: &str,
    admission: u64,
    stream: &str,
    outgoing: &CatalogRow,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<(), PublicationError> {
    let mut row = outgoing.clone();
    row.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
    let history = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
    row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let metadata = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
    let digest = proof_codec::digest(metadata.as_bytes());
    let grant = format!("{admission}:{stream}");
    let stage_origin: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.session_history_catalog h JOIN main.meta m ON m.namespace=h.namespace WHERE h.namespace=?1 AND h.session_key=?2 AND h.metadata=?3 AND m.key='catalog_native_observation:'||?4 AND m.value=?5)",
        params![namespace, row.key, history, digest, grant],
        |row| row.get(0),
    ).map_err(failed)?;
    let sql_admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    let prefix = crate::retention::source_generation(&row.sources)
        .map_err(|_| StorageError::Unavailable)?;
    if !stage_origin
        || has_local_history_origin(
            db, namespace, &row.key, &history, &metadata, admission, stream,
        )?
        || !immutable_observation_candidate(
            db, namespace, &row.key, "current", &prefix, stream, &metadata,
        )
        .map_err(ObservationPreparationError::into_storage)?
    {
        return Ok(());
    }
    // Charge the bounded immutable cells before the verifier materializes them.
    let bytes: i64 = db.query_row(
        "SELECT coalesce(sum(bytes),0) FROM (SELECT octet_length(g.metadata)+octet_length(g.slots)+octet_length(r.manifest)+octet_length(v.metadata) AS bytes FROM storage_proof_refs f JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key JOIN storage_proof_presentation_roots r ON r.namespace=g.namespace AND r.publication=g.publication AND r.digest=g.proof JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current' AND (g.prefix=?3 OR g.metadata=?4) AND g.stream=?5 LIMIT 2)",
        params![namespace, row.key, prefix, metadata, stream],
        |row| row.get(0),
    ).map_err(failed)?;
    let bytes = usize::try_from(bytes).map_err(|_| StorageError::ScopeChanged)?;
    if budget
        .charge(
            bytes
                .saturating_add(history.len())
                .saturating_add(metadata.len()),
        )
        .is_err()
    {
        return Err(PublicationError::BudgetOverflow);
    }
    let current = crate::storage::QueryRuntimeBinding {
        namespace: crate::storage::LogicalSourceNamespace::new(namespace)?,
        admission,
        stream_epoch: stream.to_owned(),
    };
    let verified = verify_immutable_observation(db, &current, &row, "current", &prefix, true)
        .map_err(ObservationPreparationError::into_storage)?;
    if verified.record.metadata != metadata || verified.record.admission != admission {
        return Err(StorageError::ScopeChanged.into());
    }
    db.execute(
        "INSERT INTO storage_proof_refs(namespace,session_key,kind,id) VALUES(?1,?2,'history',?3) ON CONFLICT DO NOTHING",
        params![namespace, row.key, verified.record.id.0],
    ).map_err(failed)?;
    db.execute(
        "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream,direct_capture) VALUES(?1,?2,'history',?3,?4,?5,NULL) ON CONFLICT(namespace,session_key,owner_kind) DO UPDATE SET metadata_digest=excluded.metadata_digest,admission=excluded.admission,stream=excluded.stream,direct_capture=NULL",
        params![namespace, row.key, digest, sql_admission, stream],
    ).map_err(failed)?;
    Ok(())
}

/// Atomically retain only the publication receipts selected by the verified
/// History planner. Every ID must still be referenced by this local owner.
pub(crate) fn replace_verified_history_refs(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    receipt_ids: &[String],
) -> Result<(), StorageError> {
    if receipt_ids.len() > 64
        || receipt_ids.iter().any(String::is_empty)
        || receipt_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != receipt_ids.len()
    {
        return Err(StorageError::HistoryRetentionBounds);
    }
    for id in receipt_ids {
        let referenced: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind IN('current','history') AND id=?3)",
                params![namespace, session_key, id],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if !referenced {
            return Err(StorageError::ScopeChanged);
        }
    }
    db.execute(
        "DELETE FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='history'",
        params![namespace, session_key],
    )
    .map_err(failed)?;
    for id in receipt_ids {
        db.execute(
            "INSERT INTO storage_proof_refs(namespace,session_key,kind,id) VALUES(?1,?2,'history',?3)",
            params![namespace, session_key, id],
        )
        .map_err(failed)?;
    }
    Ok(())
}

fn normalized_owner_metadata(metadata: &str, session_key: &str) -> Result<String, StorageError> {
    let mut row: CatalogRow =
        serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
    if row.key != session_key {
        return Err(StorageError::ScopeChanged);
    }
    row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)
}

fn has_local_history_origin(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    metadata: &str,
    normalized_metadata: &str,
    admission: u64,
    stream: &str,
) -> Result<bool, StorageError> {
    let digest = proof_codec::digest(normalized_metadata.as_bytes());
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.session_history_catalog h JOIN main.storage_catalog_native_observation_grants g ON g.namespace=h.namespace AND g.session_key=h.session_key WHERE h.namespace=?1 AND h.session_key=?2 AND h.metadata=?3 AND g.owner_kind='history' AND g.metadata_digest=?4 AND g.admission=?5 AND g.stream=?6)",
        params![namespace, session_key, metadata, digest, admission, stream],
        |row| row.get(0),
    )
    .map_err(failed)
}

/// Preserve exact Current refs as retention pointers for original History
/// when its local History grant is absent. These refs cannot authorize reuse:
/// verified History reads require a matching local History origin grant.
pub(super) fn retain_current_refs_for_unowned_history(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    admission: u64,
    stream: &str,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<(), PublicationError> {
    let metadata_bytes: Option<i64> = db
        .query_row(
            "SELECT octet_length(metadata) FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
            params![namespace, session_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| PublicationError::Storage(failed(error)))?;
    let Some(metadata_bytes) = metadata_bytes else {
        return Ok(());
    };
    let metadata_bytes = usize::try_from(metadata_bytes).map_err(|_| StorageError::ScopeChanged)?;
    if budget.charge(metadata_bytes).is_err() {
        return Err(PublicationError::BudgetOverflow);
    }
    let metadata: String = db
        .query_row(
            "SELECT metadata FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
            params![namespace, session_key],
            |row| row.get(0),
        )
        .map_err(|error| PublicationError::Storage(failed(error)))?;
    let normalized = normalized_owner_metadata(&metadata, session_key)?;
    if has_local_history_origin(
        db,
        namespace,
        session_key,
        &metadata,
        &normalized,
        admission,
        stream,
    )? {
        return Ok(());
    }
    let digest = proof_codec::digest(normalized.as_bytes());
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    let has_local_current: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM main.storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog' AND metadata_digest=?3 AND admission=?4 AND stream=?5)",
            params![namespace, session_key, digest, admission, stream],
            |row| row.get(0),
        )
        .map_err(|error| PublicationError::Storage(failed(error)))?;
    if !has_local_current {
        return Ok(());
    }
    let mut statement = db
        .prepare(
            "SELECT id FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current' ORDER BY id LIMIT 65",
        )
        .map_err(|error| PublicationError::Storage(failed(error)))?;
    let ids = statement
        .query_map(params![namespace, session_key], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| PublicationError::Storage(failed(error)))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| PublicationError::Storage(failed(error)))?;
    if ids.len() > 64 {
        return Err(StorageError::HistoryRetentionBounds.into());
    }
    for id in ids {
        db.execute(
            "INSERT INTO storage_proof_refs(namespace,session_key,kind,id) VALUES(?1,?2,'history',?3) ON CONFLICT DO NOTHING",
            params![namespace, session_key, id],
        )
        .map_err(|error| PublicationError::Storage(failed(error)))?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicationReceiptRef {
    Current,
    History,
}

impl PublicationReceiptRef {
    fn kind(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::History => "history",
        }
    }
}

/// Exact owner and publication context used to authenticate stored receipts.
pub(crate) struct PublicationReceiptRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) session_key: &'a str,
    pub(crate) stream: &'a str,
    pub(crate) publication_admission: u64,
    pub(crate) previous: &'a CatalogRow,
    pub(crate) exact_owner_metadata: &'a str,
    pub(crate) kind: PublicationReceiptRef,
}

/// Authenticate local publication receipts that may supply predecessor
/// slots. Current ownership also requires its exact native observation grant;
/// retained History ownership requires the durable local History reference.
/// The full CWD fingerprint is checked against an exact path-to-CWD map.
pub(crate) fn verified_publication_receipts(
    db: &Connection,
    request: PublicationReceiptRequest<'_>,
    budget: &mut crate::overview::SummaryPublicationBudget,
) -> Result<Option<Vec<crate::storage::catalog::VerifiedHistoryReceipt>>, StorageError> {
    let PublicationReceiptRequest {
        namespace,
        session_key,
        stream,
        publication_admission,
        previous,
        exact_owner_metadata,
        kind,
    } = request;
    if kind == PublicationReceiptRef::History {
        let normalized = normalized_owner_metadata(exact_owner_metadata, session_key)?;
        if !has_local_history_origin(
            db,
            namespace,
            session_key,
            exact_owner_metadata,
            &normalized,
            publication_admission,
            stream,
        )? {
            return Ok(Some(Vec::new()));
        }
    }
    let refs: i64 = db
        .query_row(
            "SELECT count(*) FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind=?3",
            params![namespace, session_key, kind.kind()],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if refs <= 0 || refs > 64 {
        return Ok(Some(Vec::new()));
    }
    let bytes: i64 = db
        .query_row(
            "SELECT COALESCE(sum(octet_length(f.id)+octet_length(g.publication)+octet_length(g.proof)+octet_length(g.prefix)+octet_length(g.metadata)+octet_length(g.slots)+octet_length(r.base)+octet_length(r.manifest)+octet_length(v.metadata)),0) FROM storage_proof_refs f JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind=?3",
            params![namespace, session_key, kind.kind()],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let Ok(bytes) = usize::try_from(bytes) else {
        return Ok(Some(Vec::new()));
    };
    if budget.charge(bytes).is_err() {
        return Ok(None);
    }
    let mut statement = db
        .prepare(
            "SELECT g.id,g.publication,g.proof,g.admission,g.stream,g.metadata,g.slots,g.prefix,r.base,r.manifest,v.metadata FROM storage_proof_refs f JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind=?3 ORDER BY g.id LIMIT 65",
        )
        .map_err(failed)?;
    let rows = statement
        .query_map(params![namespace, session_key, kind.kind()], |row| {
            Ok((
                proof_codec::GenerationRecord {
                    namespace: namespace.to_owned(),
                    id: ProofGenerationId(row.get(0)?),
                    session_key: session_key.to_owned(),
                    publication: row.get(1)?,
                    proof: row.get(2)?,
                    admission: u64::try_from(row.get::<_, i64>(3)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    stream: row.get(4)?,
                    metadata: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    slots: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    prefix: row.get(7)?,
                },
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(failed)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(failed)?;
    drop(statement);
    let mut verified = Vec::new();
    for (record, base, manifest, presentation) in rows {
        let (Some(manifest), Some(presentation)) = (manifest, presentation) else {
            continue;
        };
        let decoded_bytes = record
            .metadata
            .len()
            .checked_add(record.slots.len().saturating_mul(2))
            .and_then(|bytes| bytes.checked_add(base.len()))
            .and_then(|bytes| bytes.checked_add(manifest.len()))
            .and_then(|bytes| bytes.checked_add(presentation.len()));
        if decoded_bytes.is_none_or(|bytes| budget.charge(bytes).is_err()) {
            return Ok(None);
        }
        if record.stream != stream || record.session_key != session_key {
            continue;
        }
        if kind == PublicationReceiptRef::Current && record.metadata != exact_owner_metadata {
            continue;
        }
        if kind == PublicationReceiptRef::Current {
            let admission =
                i64::try_from(record.admission).map_err(|_| StorageError::Unavailable)?;
            let digest = proof_codec::digest(record.metadata.as_bytes());
            let granted: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog' AND metadata_digest=?3 AND admission=?4 AND stream=?5)",
                    params![namespace, session_key, digest, admission, stream],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            if !granted {
                continue;
            }
        }
        let id = record.id.clone();
        let publication = record.publication.clone();
        let proof = record.proof.clone();
        let admission = record.admission;
        let prefix = record.prefix.clone();
        let Ok((record, original)) = proof_codec::decode_generation(
            record,
            proof_codec::ExpectedGeneration {
                namespace,
                id: &id,
                admission,
                stream,
                proof: &proof,
                publication: &publication,
                prefix: &prefix,
            },
        ) else {
            continue;
        };
        if original.lifecycle != crate::model::summary::CatalogLifecycle::Current {
            continue;
        }
        if original.key != previous.key
            || original.harness != previous.harness
            || original
                .sources
                .iter()
                .any(|source| !previous.sources.contains(source))
        {
            continue;
        }
        if kind == PublicationReceiptRef::Current && original.sources != previous.sources {
            continue;
        }
        let Ok(Some(observations)) = proof_codec::verify_presentation_observations(
            session_key,
            &record.metadata,
            &record.slots,
            &presentation,
            &base,
            &manifest,
            &record.proof,
        ) else {
            continue;
        };
        let Ok(source_generation) = crate::retention::source_generation(&original.sources) else {
            continue;
        };
        let Some(saved_fingerprint) = serde_json::to_value(&observations).ok().and_then(|value| {
            value
                .get("runtime_cwd_fingerprints")?
                .get(&source_generation)?
                .as_str()
                .map(str::to_owned)
        }) else {
            continue;
        };
        if observations.runtime_for(&original).is_none() {
            continue;
        }
        let Ok(slots) =
            serde_json::from_str::<Vec<crate::slot_projection::SavedSlot>>(&record.slots)
        else {
            continue;
        };
        if slots.len() > 64 {
            continue;
        }
        let source_by_path: std::collections::BTreeMap<_, _> = original
            .sources
            .iter()
            .map(|source| (&source.path, source))
            .collect();
        let mut slot_cwds = std::collections::BTreeMap::new();
        let mut slots_valid = true;
        for slot in &slots {
            for (path, cwd) in slot.source_cwds() {
                if !source_by_path.contains_key(path) {
                    slots_valid = false;
                    break;
                }
                if slot_cwds
                    .insert(path.clone(), cwd.map(str::to_owned))
                    .is_some_and(|saved| saved != cwd.map(str::to_owned))
                {
                    slots_valid = false;
                    break;
                }
            }
            if !slots_valid {
                break;
            }
        }
        if !slots_valid {
            continue;
        }
        let source_paths: std::collections::BTreeSet<_> = original
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        if source_paths.len() != original.sources.len() {
            continue;
        }
        let ledger_bytes = original
            .sources
            .len()
            .checked_mul(std::mem::size_of::<crate::events::Ledger>());
        if ledger_bytes.is_none_or(|bytes| budget.charge(bytes).is_err()) {
            return Ok(None);
        }
        let cwd_bytes = super::catalog::runtime_source_cwd_bytes(db, &original.sources)
            .map_err(|_| StorageError::Unavailable)?;
        let current_cwds_budgeted = budget.charge(cwd_bytes).is_ok();
        let current_cwds = if current_cwds_budgeted {
            super::catalog::runtime_source_cwds_by_path_if_current(db, &original.sources)
                .ok()
                .flatten()
        } else {
            None
        };
        let cwd_by_path = match current_cwds {
            Some(current) => current,
            None if slot_cwds
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                == source_paths =>
            {
                slot_cwds
            }
            None => continue,
        };
        if cwd_by_path
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            != source_paths
        {
            continue;
        }
        let cwds: std::collections::BTreeSet<_> =
            cwd_by_path.values().filter_map(Clone::clone).collect();
        if crate::native_evidence::runtime::cwd_fingerprint(&cwds) != saved_fingerprint {
            continue;
        }
        let slot_texts = slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| StorageError::Unavailable)?;
        verified.push(crate::storage::catalog::VerifiedHistoryReceipt::verified(
            record.id.0,
            source_generation,
            original.sources,
            slot_texts,
            saved_fingerprint,
            cwd_by_path,
        ));
    }
    Ok(Some(verified))
}

/// Add durable references before the existing proof-copy/collection algorithm.
/// An absent S2 family is an unbound current profile, never a reader fallback.
pub(crate) fn referenced(db: &Connection, path: &str) -> rusqlite::Result<Vec<String>> {
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='storage_proof_sources')",
        [],
        |r| r.get(0),
    )?;
    if !present {
        return Ok(Vec::new());
    }
    let mut statement = db.prepare("SELECT DISTINCT s.ledger FROM storage_proof_sources s WHERE s.namespace=semon_namespace() AND s.path=?1 AND (EXISTS(SELECT 1 FROM storage_proof_refs r WHERE r.namespace=s.namespace AND r.id=s.id AND r.session_key=s.session_key) OR EXISTS(SELECT 1 FROM storage_proof_leases l JOIN storage_proof_generations g ON g.namespace=l.namespace AND g.id=l.id AND g.session_key=l.session_key JOIN storage_namespaces n ON n.namespace=l.namespace WHERE l.namespace=s.namespace AND l.id=s.id AND l.session_key=s.session_key AND l.admission=n.admission_generation AND l.owner_epoch=(SELECT epoch FROM storage_proof_reader_owners o WHERE o.namespace=l.namespace) AND l.stream=g.stream AND l.stream=(SELECT value FROM main.meta WHERE namespace='' AND key='catalog_changes_epoch') AND l.expires>?2)) LIMIT 257")?;
    let values = statement
        .query_map(params![path, crate::model::now_ms()], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if values.len() > 256 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(values)
}

/// Delete at most `budget` metadata generations. This never touches Raw or
/// interprets lease expiry as completion of a remote write.
pub(crate) fn collect(
    tx: &Transaction<'_>,
    namespace: &str,
    budget: usize,
) -> rusqlite::Result<usize> {
    let candidates = tx.prepare("SELECT id,session_key FROM storage_proof_generations g WHERE namespace=?1 AND NOT EXISTS(SELECT 1 FROM storage_proof_refs r WHERE r.namespace=g.namespace AND r.id=g.id AND r.session_key=g.session_key) AND NOT EXISTS(SELECT 1 FROM storage_proof_leases l JOIN storage_namespaces n ON n.namespace=l.namespace WHERE l.namespace=g.namespace AND l.id=g.id AND l.session_key=g.session_key AND l.admission=n.admission_generation AND l.owner_epoch=(SELECT epoch FROM storage_proof_reader_owners o WHERE o.namespace=l.namespace) AND l.stream=g.stream AND l.stream=(SELECT value FROM main.meta WHERE namespace='' AND key='catalog_changes_epoch') AND l.expires>?2) ORDER BY id,session_key LIMIT ?3")?.query_map(params![namespace,crate::model::now_ms(),budget.min(64) as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, key) in &candidates {
        tx.execute("DELETE FROM storage_proof_presentations WHERE namespace=?1 AND id=?2 AND session_key=?3", params![namespace,id,key])?;

        tx.execute(
            "DELETE FROM storage_proof_sources WHERE namespace=?1 AND id=?2 AND session_key=?3",
            params![namespace, id, key],
        )?;
        tx.execute(
            "DELETE FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3",
            params![namespace, id, key],
        )?;
    }
    Ok(candidates.len())
}

pub(crate) struct Lease {
    owner: NativeOwnership,
    owner_epoch: i64,
    serial: i64,
    token: String,
    snapshot: ReaderSnapshot,
    row: CatalogRow,
    released: bool,
}
fn ttl(ttl: u32) -> Result<i64, StorageError> {
    if ttl == 0 || ttl > TTL_MAX {
        return Err(StorageError::InvalidArguments);
    }
    crate::model::now_ms()
        .checked_add(i64::from(ttl))
        .ok_or(StorageError::InvalidArguments)
}
/// One host runtime owns reader leases independently of writer batch epochs.
/// Clone this handle for concurrent requests; open explicitly on takeover.
#[derive(Clone)]
pub(crate) struct ReaderRuntime {
    owner: NativeOwnership,
    epoch: i64,
}
impl NativeReaderBinding {
    pub(crate) fn reader_runtime(&self) -> Result<ReaderRuntime, StorageError> {
        let Self::Semantic(owner) = self else {
            return Err(StorageError::NamespaceUpgradePending);
        };
        let mut db = super::source_journal::SourceJournal::owned(owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        owner.validate_ready(&tx)?;
        let ns = owner.config().namespace.as_str();
        if tx.execute("INSERT INTO storage_proof_reader_owners VALUES(?1,1,0) ON CONFLICT(namespace) DO UPDATE SET epoch=epoch+1,next_lease=0 WHERE epoch<9223372036854775807", [ns]).map_err(failed)? != 1 {
            return Err(StorageError::Conflict);
        }
        let epoch = tx
            .query_row(
                "SELECT epoch FROM storage_proof_reader_owners WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(ReaderRuntime {
            owner: owner.clone(),
            epoch,
        })
    }
}
impl ReaderRuntime {
    fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        self.owner.validate_ready(db)?;
        let epoch: i64 = db
            .query_row(
                "SELECT epoch FROM storage_proof_reader_owners WHERE namespace=?1",
                [self.owner.config().namespace.as_str()],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if epoch != self.epoch {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
    pub(crate) fn current_generation(&self, key: &str) -> Result<ProofGenerationId, StorageError> {
        let db = super::source_journal::SourceJournal::owned(self.owner.clone()).connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
        self.validate(&db)?;
        let id = db.query_row("SELECT id FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind='current'", params![self.owner.config().namespace.as_str(), key], |r| r.get(0)).map_err(failed)?;
        Ok(ProofGenerationId(id))
    }
    /// Retry unknown ACK with the SAME token AND expected generation. Expiry
    /// never selects a new generation or reports a remote operation completed.
    pub(crate) fn selected_generation(
        &self,
        key: &str,
        expected: &ProofGenerationId,
        token: &str,
        ttl_ms: u32,
    ) -> Result<Lease, StorageError> {
        let owner = &self.owner;
        let sql_admission =
            i64::try_from(owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidArguments);
        }
        let expires = ttl(ttl_ms)?;
        let mut db = super::source_journal::SourceJournal::owned(owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        let ns = owner.config().namespace.as_str();
        let existing: Option<(String, String, i64)> = tx.query_row("SELECT id,session_key,serial FROM storage_proof_leases WHERE namespace=?1 AND token=?2", params![ns, token], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(failed)?;
        let (id, serial) = if let Some((id, session, serial)) = existing {
            if session != key || id != expected.0 {
                return Err(StorageError::Conflict);
            }
            (id, serial)
        } else {
            // Expired/fenced rows are no longer admitted readers. Their removal
            // cannot rebind a retry: expected generation is an explicit input.
            tx.execute("DELETE FROM storage_proof_leases WHERE namespace=?1 AND token IN (SELECT token FROM storage_proof_leases WHERE namespace=?1 AND (expires<=?2 OR owner_epoch<>?3) LIMIT 64)", params![ns, crate::model::now_ms(), self.epoch]).map_err(failed)?;
            let count: i64 = tx
                .query_row(
                    "SELECT count(*) FROM storage_proof_leases WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            if count >= LEASE_CAP {
                return Err(StorageError::Busy);
            }
            if tx.execute("UPDATE storage_proof_reader_owners SET next_lease=next_lease+1 WHERE namespace=?1 AND epoch=?2 AND next_lease<9223372036854775807", params![ns, self.epoch]).map_err(failed)? != 1 {
                return Err(StorageError::Conflict);
            }
            let serial: i64 = tx
                .query_row(
                    "SELECT next_lease FROM storage_proof_reader_owners WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(failed)?;
            tx.execute(
                "INSERT INTO storage_proof_leases VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    ns,
                    token,
                    expected.0,
                    key,
                    sql_admission,
                    owner.config().expected_stream_epoch,
                    self.epoch,
                    serial,
                    expires
                ],
            )
            .map_err(failed)?;
            (expected.0.clone(), serial)
        };
        let (publication, proof, metadata, slots, prefix, admission, stream): (String, String, String, String, String, i64, String) = tx.query_row("SELECT g.publication,p.proof_digest,g.metadata,g.slots,g.prefix,g.admission,g.stream FROM storage_proof_generations g JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3", params![ns, id, key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(failed)?;
        let admission = u64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
        validate_publication_owner(&tx, owner, &publication, admission)?;
        if Some(&stream) != owner.config().expected_stream_epoch.as_ref() {
            return Err(StorageError::ScopeChanged);
        }
        let (record, row) = proof_codec::decode_generation(
            proof_codec::GenerationRecord {
                namespace: ns.into(),
                id: ProofGenerationId(id.clone()),
                session_key: key.into(),
                publication: publication.clone(),
                proof: proof.clone(),
                admission,
                stream: stream.clone(),
                metadata,
                slots,
                prefix: prefix.clone(),
            },
            proof_codec::ExpectedGeneration {
                namespace: ns,
                id: expected,
                admission,
                stream: owner
                    .config()
                    .expected_stream_epoch
                    .as_deref()
                    .ok_or(StorageError::ScopeChanged)?,
                proof: &proof,
                publication: &publication,
                prefix: &prefix,
            },
        )?;
        let prefix = record.prefix;
        let lease = Lease {
            owner: owner.clone(),
            owner_epoch: self.epoch,
            serial,
            token: token.into(),
            snapshot: ReaderSnapshot {
                namespace: owner.config().namespace.clone(),
                admission,
                stream_epoch: stream,
                publication_generation: publication,
                proof_generation: ProofGenerationId(id),
                consumed_prefix: prefix,
            },
            row,
            released: false,
        };
        lease.validate(&tx)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(lease)
    }
}
impl Lease {
    fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        self.owner.validate_ready(db)?;
        let proof: Option<String> = db.query_row("SELECT g.proof FROM storage_proof_generations g JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof WHERE g.namespace=?1 AND g.id=?2 AND g.session_key=?3", params![self.snapshot.namespace.as_str(), self.snapshot.proof_generation.0, self.row.key], |r| r.get(0)).optional().map_err(failed)?;
        let Some(proof) = proof else {
            return Err(StorageError::ScopeChanged);
        };
        validate_publication_owner(
            db,
            &self.owner,
            &self.snapshot.publication_generation,
            self.snapshot.admission,
        )?;
        if ProofGenerationId::derive(
            self.snapshot.namespace.as_str(),
            self.snapshot.admission,
            &self.snapshot.stream_epoch,
            &proof,
        ) != self.snapshot.proof_generation
        {
            return Err(StorageError::ScopeChanged);
        }
        validate_presentation(db, &self.snapshot, &self.row.key, &proof)?;
        let metadata = serde_json::to_string(&self.row).map_err(|_| StorageError::Unavailable)?;
        let sql_admission =
            i64::try_from(self.owner.admission()).map_err(|_| StorageError::ScopeChanged)?;
        let producer_admission =
            i64::try_from(self.snapshot.admission).map_err(|_| StorageError::ScopeChanged)?;
        let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM storage_proof_leases l JOIN storage_proof_generations g ON g.namespace=l.namespace AND g.id=l.id AND g.session_key=l.session_key WHERE l.namespace=?1 AND l.token=?2 AND l.id=?3 AND l.session_key=?4 AND l.admission=?5 AND l.stream=?6 AND l.owner_epoch=?8 AND l.owner_epoch=(SELECT epoch FROM storage_proof_reader_owners o WHERE o.namespace=l.namespace) AND l.expires>?7 AND g.admission=?13 AND g.stream=l.stream AND g.metadata=?9 AND g.prefix=?10 AND g.publication=?11 AND l.serial=?12)", params![self.snapshot.namespace.as_str(), self.token, self.snapshot.proof_generation.0, self.row.key, sql_admission, self.snapshot.stream_epoch, crate::model::now_ms(), self.owner_epoch, metadata, self.snapshot.consumed_prefix, self.snapshot.publication_generation, self.serial, producer_admission], |r| r.get(0)).map_err(failed)?;
        if self.released || !valid {
            return Err(StorageError::ScopeChanged);
        }
        Ok(())
    }
}

pub(crate) fn imported_catalog(
    db: &Connection,
    binding: &crate::storage::QueryRuntimeBinding,
) -> Result<bool, StorageError> {
    let admission = i64::try_from(binding.admission).map_err(|_| StorageError::ScopeChanged)?;
    let max_bytes = i64::try_from(crate::storage::transfer::MAX_PAGE_BYTES)
        .map_err(|_| StorageError::ScopeChanged)?;
    db.query_row("SELECT EXISTS(SELECT 1 FROM main.meta p WHERE p.namespace=?1 AND p.key='native_transfer_v1:catalog-admission' AND CASE WHEN octet_length(p.value)<=?4 THEN json_extract(p.value,'$[0]')='catalog-import-v1' AND json_extract(p.value,'$[1].namespace')=?1 AND json_extract(p.value,'$[1].stream_epoch')=?2 AND json_extract(p.value,'$[3]')=?3 AND EXISTS(SELECT 1 FROM main.meta a WHERE a.namespace=p.namespace AND a.key='native_transfer_v1:catalog-owner:'||json_extract(p.value,'$[2]') AND a.value=?5) ELSE 0 END)", params![binding.namespace.as_str(), binding.stream_epoch, admission, max_bytes, format!("{}:{}",binding.admission,binding.stream_epoch)], |r| r.get(0)).map_err(failed)
}

#[derive(Debug)]
struct ObservationFieldMatches {
    query: bool,
    sources: bool,
    native_ids: bool,
    lifecycle: bool,
    harness: bool,
    last: bool,
    repo: bool,
    model: bool,
}

struct ObservationRowComparison {
    selected_equal: bool,
    fields: ObservationFieldMatches,
}

fn observation_row_metadata(
    db: &Connection,
    namespace: &str,
    key: &str,
    history: bool,
) -> Option<CatalogRow> {
    let sql = if history {
        "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2"
    } else {
        "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM main.session_catalog WHERE namespace=?1 AND session_key=?2"
    };
    let metadata = db
        .query_row(sql, params![namespace, key], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()
        .ok()?
        .flatten()?;
    serde_json::from_str(&metadata).ok()
}

fn observation_row_comparison(
    selected: &CatalogRow,
    actual: &CatalogRow,
) -> Option<ObservationRowComparison> {
    let mut selected = selected.clone();
    selected.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let mut normalized_actual = actual.clone();
    normalized_actual.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let selected_equal =
        serde_json::to_string(&selected).ok()? == serde_json::to_string(&normalized_actual).ok()?;
    Some(ObservationRowComparison {
        selected_equal,
        fields: ObservationFieldMatches {
            query: selected.query == actual.query,
            sources: selected.sources == actual.sources,
            native_ids: selected.native_ids == actual.native_ids,
            lifecycle: selected.lifecycle == actual.lifecycle,
            harness: selected.harness == actual.harness,
            last: selected.last == actual.last,
            repo: selected.repo == actual.repo,
            model: selected.model == actual.model,
        },
    })
}

fn log_observation_row_comparison(
    label: &'static str,
    comparison: Option<ObservationRowComparison>,
) {
    if let Some(comparison) = comparison {
        let ObservationFieldMatches {
            query,
            sources,
            native_ids,
            lifecycle,
            harness,
            last,
            repo,
            model,
        } = comparison.fields;
        eprintln!(
            "semon sessions native: observation_direct_grant_fields {label}={} query={query} sources={sources} native_ids={native_ids} lifecycle={lifecycle} harness={harness} last={last} repo={repo} model={model}",
            comparison.selected_equal
        );
    } else {
        eprintln!(
            "semon sessions native: observation_direct_grant_fields {label}=unavailable query=unavailable sources=unavailable native_ids=unavailable lifecycle=unavailable harness=unavailable last=unavailable repo=unavailable model=unavailable"
        );
    }
}

/// SQL failures must abort producer transactions; unavailable evidence can
/// leave one owner unchanged. Reader APIs retain their StorageError contract.
#[derive(Debug)]
pub(crate) enum ObservationPreparationError {
    Evidence(StorageError),
    Sql(rusqlite::Error),
}

impl From<StorageError> for ObservationPreparationError {
    fn from(error: StorageError) -> Self {
        Self::Evidence(error)
    }
}

impl ObservationPreparationError {
    fn into_storage(self) -> StorageError {
        match self {
            Self::Evidence(error) => error,
            Self::Sql(error) => failed(error),
        }
    }
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static FAIL_OBSERVATION_READ: std::cell::Cell<Option<(&'static str, &'static str)>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
struct ObservationReadFault<'a>(&'a Connection);

#[cfg(test)]
impl<'a> ObservationReadFault<'a> {
    fn install(db: &'a Connection, owner: CatalogObservationOwner) -> Option<Self> {
        let table = FAIL_OBSERVATION_READ.with(|fault| {
            fault
                .get()
                .filter(|(kind, _)| *kind == owner.kind())
                .map(|(_, table)| table)
        })?;
        // Deny the real preparation statement without damaging the fixture DB.
        db.authorizer(Some(
            move |context: rusqlite::hooks::AuthContext<'_>| match context.action {
                rusqlite::hooks::AuthAction::Read { table_name, .. } if table_name == table => {
                    rusqlite::hooks::Authorization::Deny
                }
                _ => rusqlite::hooks::Authorization::Allow,
            },
        ))
        .unwrap();
        Some(Self(db))
    }
}

#[cfg(test)]
impl Drop for ObservationReadFault<'_> {
    fn drop(&mut self) {
        self.0
            .authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            )
            .unwrap();
    }
}

fn immutable_observation_candidate(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    reference_kind: &str,
    prefix: &str,
    stream: &str,
    metadata: &str,
) -> Result<bool, ObservationPreparationError> {
    // Only route into the immutable verifier; this predicate grants no authority.
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_proof_refs f LEFT JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind=?3 AND (g.id IS NULL OR (g.stream=?4 AND (g.prefix=?5 OR g.metadata=?6))))",
        params![namespace, session_key, reference_kind, stream, prefix, metadata],
        |row| row.get(0),
    )
    .map_err(ObservationPreparationError::Sql)
}

fn exact_immutable_observation_owner(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    reference_kind: &str,
    admission: i64,
    stream: &str,
    metadata: &str,
) -> Result<bool, StorageError> {
    // A dangling ref is unusable immutable state and must not enable direct authority.
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_proof_refs f LEFT JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind=?3 AND (g.id IS NULL OR (g.admission=?4 AND g.stream=?5 AND g.metadata=?6)))",
        params![namespace, session_key, reference_kind, admission, stream, metadata],
        |row| row.get(0),
    )
    .map_err(failed)
}

fn direct_capture_for_row(
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    imported: bool,
    direct_grant: Option<&(Option<String>, bool)>,
) -> Result<Option<crate::storage::selected::PresentationObservations>, StorageError> {
    if imported {
        return Ok(None);
    }
    let Some((capture, oversized)) = direct_grant else {
        return Ok(None);
    };
    if *oversized {
        return Err(StorageError::HistoryRetentionBounds);
    }
    let Some(capture) = capture else {
        return Ok(None);
    };
    crate::selected_read::charge(1, capture.len()).map_err(|_| StorageError::Busy)?;
    let observations: crate::storage::selected::PresentationObservations =
        serde_json::from_str(capture).map_err(|_| StorageError::ScopeChanged)?;
    let source_runtime = observations
        .source_runtime
        .as_deref()
        .ok_or(StorageError::ScopeChanged)?;
    let captured_global = current
        .witness(source_runtime)
        .map_err(|_| StorageError::ScopeChanged)?;
    if observations.runtime.as_deref() != Some(captured_global.as_str())
        || !observations.is_runtime_capture_for(selected)
        || observations.runtime_for(selected).is_none()
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(Some(observations))
}

fn same_runtime_inputs(
    left: &crate::storage::selected::PresentationObservations,
    right: &crate::storage::selected::PresentationObservations,
    row: &CatalogRow,
) -> bool {
    // Compare producer inputs without comparing their admission-qualified global witness.
    left.source_runtime == right.source_runtime
        && left.runtime_for(row).is_some()
        && left.runtime_for(row) == right.runtime_for(row)
}

struct VerifiedObservation {
    binding: crate::storage::QueryRuntimeBinding,
    row: CatalogRow,
    observations: Option<crate::storage::selected::PresentationObservations>,
    record: proof_codec::GenerationRecord,
    presentation: String,
}

fn diagnose_observation_refusal(stage: &'static str) {
    if cfg!(test) || crate::native_timing::enabled() {
        eprintln!("catalog_retained_observation_refused stage={stage}");
    }
}

#[cfg(test)]
fn diagnose_immutable_candidate_probe(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    reference_kind: &str,
    prefix: &str,
) {
    let unavailable = || {
        eprintln!(
            "catalog_retained_observation_refused stage=immutable_candidate_probe unavailable=true"
        );
    };
    let Ok(admission) = i64::try_from(current.admission) else {
        unavailable();
        return;
    };
    let mut normalized = selected.clone();
    normalized.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let Ok(metadata) = serde_json::to_string(&normalized) else {
        unavailable();
        return;
    };
    let counts = db.query_row(
        "WITH bounded_refs AS (SELECT namespace,session_key,kind,id FROM storage_proof_refs WHERE namespace=?1 AND session_key=?2 AND kind=?3 ORDER BY id LIMIT 65), candidates AS (SELECT CASE WHEN g.id IS NOT NULL THEN 1 ELSE 0 END AS generation,CASE WHEN g.id IS NOT NULL AND g.stream=?4 THEN 1 ELSE 0 END AS stream,CASE WHEN g.id IS NOT NULL AND g.admission=?5 THEN 1 ELSE 0 END AS admission,CASE WHEN g.id IS NOT NULL AND g.prefix=?6 THEN 1 ELSE 0 END AS prefix,CASE WHEN g.id IS NOT NULL AND g.metadata=?7 THEN 1 ELSE 0 END AS metadata,CASE WHEN p.generation IS NOT NULL AND r.digest IS NOT NULL AND v.id IS NOT NULL THEN 1 ELSE 0 END AS joins FROM bounded_refs f LEFT JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key LEFT JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission LEFT JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest LEFT JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key) SELECT count(*),count(CASE WHEN generation=1 THEN 1 END),count(CASE WHEN stream=1 THEN 1 END),count(CASE WHEN admission=1 THEN 1 END),count(CASE WHEN prefix=1 THEN 1 END),count(CASE WHEN metadata=1 THEN 1 END),count(CASE WHEN joins=1 THEN 1 END),count(CASE WHEN stream=1 AND prefix=1 AND joins=1 THEN 1 END),count(CASE WHEN stream=1 AND admission=1 AND prefix=1 AND metadata=1 AND joins=1 THEN 1 END) FROM candidates",
        params![
            current.namespace.as_str(),
            selected.key,
            reference_kind,
            current.stream_epoch,
            admission,
            prefix,
            metadata,
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        },
    );
    match counts {
        Ok((
            refs,
            generation,
            stream,
            admission,
            prefix,
            metadata,
            joins,
            stream_prefix_joins,
            exact,
        )) => {
            eprintln!(
                "catalog_retained_observation_refused stage=immutable_candidate_probe refs={refs} overflow={} generation={generation} stream={stream} admission={admission} prefix={prefix} metadata={metadata} joins={joins} stream_prefix_joins={stream_prefix_joins} exact={exact}",
                refs > 64
            );
        }
        Err(_) => unavailable(),
    }
}

/// A same-prefix receipt may supply predecessor custody without owning the
/// selected metadata. Authenticate it before deciding which authority to use.
fn authenticate_immutable_observation(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    reference_kind: &str,
    prefix: &str,
) -> Result<VerifiedObservation, ObservationPreparationError> {
    let mut statement = db
        .prepare(
            "SELECT g.id,g.publication,g.proof,g.admission,g.stream,CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,g.prefix,r.base,CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END,CASE WHEN octet_length(v.metadata)<=8388608 THEN v.metadata END FROM storage_proof_refs f JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind=?3 AND g.prefix=?4 AND g.stream=?5 ORDER BY g.id LIMIT 2",
        )
        .map_err(ObservationPreparationError::Sql)?;
    let rows = statement
        .query_map(
            params![
                current.namespace.as_str(),
                selected.key,
                reference_kind,
                prefix,
                current.stream_epoch
            ],
            |row| {
                Ok((
                    proof_codec::GenerationRecord {
                        namespace: current.namespace.as_str().to_owned(),
                        id: ProofGenerationId(row.get(0)?),
                        session_key: selected.key.clone(),
                        publication: row.get(1)?,
                        proof: row.get(2)?,
                        admission: u64::try_from(row.get::<_, i64>(3)?).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                3,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        stream: row.get(4)?,
                        metadata: row.get(5)?,
                        slots: row.get(6)?,
                        prefix: row.get(7)?,
                    },
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .map_err(ObservationPreparationError::Sql)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ObservationPreparationError::Sql)?;
    let [(record, base, manifest, presentation)] = rows.as_slice() else {
        if cfg!(test) || crate::native_timing::enabled() {
            eprintln!(
                "catalog_retained_observation_refused stage=immutable_candidate_cardinality count={}",
                rows.len()
            );
        }
        #[cfg(test)]
        diagnose_immutable_candidate_probe(db, current, selected, reference_kind, prefix);
        return Err(StorageError::ScopeChanged.into());
    };
    crate::selected_read::charge(
        64,
        record.metadata.len() + record.slots.len() + manifest.len() + presentation.len(),
    )
    .map_err(|_| StorageError::Busy)?;
    let observations = proof_codec::verify_presentation_observations(
        &selected.key,
        &record.metadata,
        &record.slots,
        presentation,
        base,
        manifest,
        &record.proof,
    )
    .inspect_err(|_| diagnose_observation_refusal("immutable_presentation"))?;
    let original: CatalogRow =
        serde_json::from_str(&record.metadata).map_err(|_| StorageError::Unavailable)?;
    let original_binding = proof_codec::observation_binding(record.clone(), current, &original)
        .inspect_err(|_| diagnose_observation_refusal("immutable_original_binding"))?;
    if original.lifecycle != crate::model::summary::CatalogLifecycle::Current
        || crate::retention::source_generation(&original.sources)
            .map_err(|_| StorageError::Unavailable)?
            != prefix
    {
        diagnose_observation_refusal("immutable_original_lifecycle_or_prefix");
        return Err(StorageError::ScopeChanged.into());
    }
    Ok(VerifiedObservation {
        binding: original_binding,
        row: original,
        observations,
        record: record.clone(),
        presentation: presentation.clone(),
    })
}

fn authorize_immutable_observation(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    reference_kind: &str,
    local_only: bool,
    verified: &VerifiedObservation,
) -> Result<(), ObservationPreparationError> {
    let record = &verified.record;
    let grant = format!("{}:{}", current.admission, current.stream_epoch);
    let transfer_grant = if local_only {
        false
    } else {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.meta m JOIN main.meta a ON a.namespace=m.namespace AND a.key='native_transfer_v1:catalog-owner:'||m.value WHERE m.namespace=?1 AND m.key='native_transfer_v1:publication:'||?2 AND a.value=?3)",
            params![current.namespace.as_str(), record.publication, grant],
            |row| row.get::<_, bool>(0),
        )
        .map_err(ObservationPreparationError::Sql)?
    };
    let stage_grant = if record.admission == current.admission {
        let digest = proof_codec::digest(record.metadata.as_bytes());
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key=?2 AND value=?3)",
            params![
                current.namespace.as_str(),
                format!("catalog_native_observation:{digest}"),
                grant
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(ObservationPreparationError::Sql)?
    } else {
        false
    };
    let mut physical_owner = selected.clone();
    physical_owner.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let physical_owner_metadata =
        serde_json::to_string(&physical_owner).map_err(|_| StorageError::Unavailable)?;
    let physical_owner_digest = proof_codec::digest(physical_owner_metadata.as_bytes());
    let receipt_metadata_digest = proof_codec::digest(record.metadata.as_bytes());
    // A grant for newly composed owner metadata cannot authorize an older
    // receipt whose authenticated row differs from that physical owner.
    let physical_owner_grant = if record.admission == current.admission
        && record.stream == current.stream_epoch
        && physical_owner_digest == receipt_metadata_digest
    {
        let owner_kind = match reference_kind {
            "current" => Some("catalog"),
            "history" => Some("history"),
            _ => None,
        };
        if let Some(owner_kind) = owner_kind {
            let admission =
                i64::try_from(current.admission).map_err(|_| StorageError::ScopeChanged)?;
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM main.storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3 AND metadata_digest=?4 AND admission=?5 AND stream=?6)",
                params![
                    current.namespace.as_str(),
                    selected.key,
                    owner_kind,
                    physical_owner_digest,
                    admission,
                    current.stream_epoch,
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(ObservationPreparationError::Sql)?
        } else {
            false
        }
    } else {
        false
    };
    if !stage_grant && !transfer_grant && !physical_owner_grant {
        diagnose_observation_refusal("immutable_destination_or_local_grant");
        return Err(StorageError::ScopeChanged.into());
    }
    Ok(())
}

/// Reading a receipt's observations requires its own destination grant.
fn verify_immutable_observation(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    reference_kind: &str,
    prefix: &str,
    local_only: bool,
) -> Result<VerifiedObservation, ObservationPreparationError> {
    let verified =
        authenticate_immutable_observation(db, current, selected, reference_kind, prefix)?;
    authorize_immutable_observation(db, current, selected, reference_kind, local_only, &verified)?;
    Ok(verified)
}

/// Reuse preserves each selected owner's original receipt and grants. Absence
/// of immutable ownership requires issuance; malformed existing ownership fails.
pub(super) fn reusable_presentations(
    tx: &Transaction<'_>,
    current: &crate::storage::QueryRuntimeBinding,
    prepared: &crate::model::PreparedSources,
) -> Result<bool, StorageError> {
    let ns = current.namespace.as_str();
    let catalog_generation: String = tx
        .query_row(
            "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_generation'",
            [ns],
            |row| row.get(0),
        )
        .map_err(failed)?;
    // Capture after publication effects, in the same transaction, so changed
    // History, relative context and source observations cannot masquerade as reuse.
    let requested = capture_presentations(
        tx,
        ns,
        PresentationInputs {
            catalog_generation: &catalog_generation,
            rows: &prepared.rows,
            projections: Some(&prepared.transcripts),
            observations: prepared.presentation_observations(),
        },
        0,
    )?;
    let mut reusable = !prepared.rows.is_empty();
    if prepared
        .rows
        .iter()
        .any(|row| !prepared.transcripts.iter().any(|p| p.key == row.key))
        && (!prepared.pids.is_empty()
            || !prepared.lineage_dependencies.is_empty()
            || !prepared.relationships.relations.is_empty()
            || prepared.relationships.endpoints.values().any(|endpoint| {
                endpoint.pid.is_some() || !endpoint.names.is_empty() || !endpoint.paths.is_empty()
            }))
    {
        // A row without a presentation has no persisted context witness for
        // these producer inputs. Authenticate its old proof below, then issue.
        reusable = false;
    }
    for (row, (_, presentation)) in prepared.rows.iter().zip(&requested) {
        let prefix = crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::Unavailable)?;
        let metadata = serde_json::to_string(row).map_err(|_| StorageError::Unavailable)?;
        // Inner joins in the verifier must not hide a broken relevant ref
        // beside a valid owner. Older, unrelated source generations can remain
        // after an ordinary Index advance and do not qualify this selection.
        // Exact metadata, however, must encode the same source prefix.
        let broken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_proof_refs f LEFT JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key LEFT JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission LEFT JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest LEFT JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind='current' AND (g.id IS NULL OR (g.stream=?3 AND (g.prefix=?4 OR g.metadata=?5) AND ((g.metadata=?5 AND g.prefix<>?4) OR p.generation IS NULL OR r.digest IS NULL OR v.id IS NULL))))",
            params![ns, row.key, current.stream_epoch, prefix, metadata],
            |row| row.get(0),
        ).map_err(failed)?;
        if broken {
            return Err(StorageError::ScopeChanged);
        }
        let owned = immutable_observation_candidate(
            tx,
            ns,
            &row.key,
            "current",
            &prefix,
            &current.stream_epoch,
            &metadata,
        )
        .map_err(ObservationPreparationError::into_storage)?;
        if !owned {
            reusable = false;
            continue;
        }
        let verified = verify_immutable_observation(tx, current, row, "current", &prefix, false)
            .map_err(ObservationPreparationError::into_storage)?;
        let mut sources = row
            .sources
            .iter()
            .map(|source| {
                (
                    source.path.to_string_lossy().into_owned(),
                    crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(source)),
                )
            })
            .collect::<Vec<_>>();
        sources.sort();
        let saved_sources = tx.prepare(
            "SELECT path,ledger FROM storage_proof_sources WHERE namespace=?1 AND id=?2 AND session_key=?3 ORDER BY path LIMIT 65",
        ).map_err(failed)?.query_map(params![ns, verified.record.id.0, row.key], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
        if sources != saved_sources {
            return Err(StorageError::ScopeChanged);
        }
        let slots = serde_json::to_string(
            &prepared
                .transcripts
                .iter()
                .find(|projection| projection.key == row.key)
                .map(|projection| &projection.slots),
        )
        .map_err(|_| StorageError::Unavailable)?;
        let observations_match = match (
            verified.observations.as_ref(),
            prepared.presentation_observations(),
        ) {
            (Some(saved), Some(requested)) => saved.same_publication_inputs_for(requested, row),
            (None, None) => true,
            _ => false,
        };
        let normalize = |text: &str| -> Result<serde_json::Value, StorageError> {
            let witness: Option<crate::storage::selected::PresentationWitness> =
                serde_json::from_str(text).map_err(|_| StorageError::Unavailable)?;
            let mut value = serde_json::to_value(witness).map_err(|_| StorageError::Unavailable)?;
            if let Some(catalog) = value
                .get_mut("catalog")
                .and_then(serde_json::Value::as_object_mut)
            {
                // capture_presentations authors these from the publication's
                // chosen generation/clock, rather than producer observations.
                for key in [
                    "catalog_generation",
                    "slot_projection_catalog_generation",
                    "catalog_observed_at",
                ] {
                    catalog.remove(key);
                }
            }
            Ok(value)
        };
        reusable &= serde_json::to_value(&verified.row).map_err(|_| StorageError::Unavailable)?
            == serde_json::to_value(row).map_err(|_| StorageError::Unavailable)?
            && verified.record.slots == slots
            && observations_match
            && normalize(&verified.presentation)? == normalize(presentation)?;
    }
    Ok(reusable)
}

/// Query observations use original producer coordinates only after the same
/// destination admission grant that qualifies a retained selected lease.
pub(crate) fn observation_binding(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
) -> Result<crate::storage::QueryRuntimeBinding, StorageError> {
    observation_binding_for_owner(db, current, selected, None, false)
        .map_err(ObservationPreparationError::into_storage)
}

/// Runtime qualification knows which physical row was selected. Identity-only
/// callers above retain their separate original-publication lookup contract.
pub(crate) fn observation_binding_for_physical_owner(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    owner: CatalogObservationOwner,
) -> Result<crate::storage::QueryRuntimeBinding, StorageError> {
    observation_binding_for_owner(db, current, selected, Some(owner), false)
        .map_err(ObservationPreparationError::into_storage)
}

pub(crate) fn observation_binding_with_observations(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    owner: CatalogObservationOwner,
) -> Result<
    (
        crate::storage::QueryRuntimeBinding,
        crate::storage::selected::PresentationObservations,
    ),
    StorageError,
> {
    let (reference_kind, owner_kind) = match owner {
        CatalogObservationOwner::Catalog => ("current", "catalog"),
        CatalogObservationOwner::History => ("history", "history"),
    };
    let mut normalized = selected.clone();
    normalized.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let prefix = crate::retention::source_generation(&selected.sources)
        .map_err(|_| StorageError::Unavailable)?;
    let selected_metadata =
        serde_json::to_string(&normalized).map_err(|_| StorageError::Unavailable)?;
    let selected_digest = proof_codec::digest(selected_metadata.as_bytes());
    let admission = i64::try_from(current.admission).map_err(|_| StorageError::ScopeChanged)?;
    let imported = imported_catalog(db, current)?;
    let exact_semantic_owner = if imported {
        false
    } else {
        exact_immutable_observation_owner(
            db,
            current.namespace.as_str(),
            &selected.key,
            reference_kind,
            admission,
            &current.stream_epoch,
            &selected_metadata,
        )?
    };
    let direct_grant = db
        .query_row(
            "SELECT CASE WHEN ?7=0 AND ?8=0 AND octet_length(direct_capture)<=8388608 THEN direct_capture END,?7=0 AND ?8=0 AND direct_capture IS NOT NULL AND octet_length(direct_capture)>8388608 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3 AND metadata_digest=?4 AND admission=?5 AND stream=?6",
            params![
                current.namespace.as_str(),
                selected.key,
                owner_kind,
                selected_digest,
                admission,
                current.stream_epoch,
                imported,
                exact_semantic_owner
            ],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, bool>(1)?)),
        )
        .optional()
        .map_err(failed)?;
    let exact_owner_grant = direct_grant.is_some();
    let proof_candidate = if imported {
        false
    } else {
        immutable_observation_candidate(
            db,
            current.namespace.as_str(),
            &selected.key,
            reference_kind,
            &prefix,
            &current.stream_epoch,
            &selected_metadata,
        )
        .map_err(ObservationPreparationError::into_storage)?
    };
    #[cfg(test)]
    eprintln!(
        "catalog_runtime_capture_selection owner={owner_kind} imported={imported} exact_semantic_owner={exact_semantic_owner} proof_candidate={proof_candidate} exact_owner_grant={exact_owner_grant} direct_capture_present={}",
        direct_grant
            .as_ref()
            .is_some_and(|(capture, _)| capture.is_some())
    );
    if !proof_candidate
        && let Some(observations) =
            direct_capture_for_row(current, selected, imported, direct_grant.as_ref())
                .inspect_err(|_| diagnose_observation_refusal("direct_capture"))?
    {
        #[cfg(test)]
        eprintln!("catalog_runtime_capture_selected origin=direct_without_proof");
        return Ok((current.clone(), observations));
    }
    let verified =
        authenticate_immutable_observation(db, current, selected, reference_kind, &prefix)
            .map_err(ObservationPreparationError::into_storage)?;
    // A composed or receipt-less owner can retain an older same-prefix receipt
    // for custody. Its exact grant and capture authorize only the selected row;
    // do not borrow that grant to read the predecessor's observations.
    if !exact_semantic_owner
        && let Some(direct) =
            direct_capture_for_row(current, selected, imported, direct_grant.as_ref())
                .inspect_err(|_| diagnose_observation_refusal("selected_direct_capture"))?
    {
        return Ok((current.clone(), direct));
    }
    authorize_immutable_observation(db, current, selected, reference_kind, false, &verified)
        .map_err(ObservationPreparationError::into_storage)?;
    let original = verified.row;
    let original_binding = verified.binding;
    let observations = verified
        .observations
        .ok_or(StorageError::ScopeChanged)
        .inspect_err(|_| diagnose_observation_refusal("immutable_observations_present"))?;
    let original_runtime = observations
        .runtime_for(&original)
        .ok_or(StorageError::ScopeChanged)
        .inspect_err(|_| diagnose_observation_refusal("immutable_original_row_runtime"))?;
    normalized.lifecycle = original.lifecycle;
    let exact_original_metadata = serde_json::to_value(&original)
        .map_err(|_| StorageError::Unavailable)?
        == serde_json::to_value(&normalized).map_err(|_| StorageError::Unavailable)?;
    if !exact_original_metadata && !exact_owner_grant {
        diagnose_observation_refusal("selected_metadata_or_exact_owner_grant");
        return Err(StorageError::ScopeChanged);
    }
    let selected_runtime = observations.runtime_for(selected);
    if selected_runtime.as_deref() != Some(original_runtime.as_str()) {
        if !exact_original_metadata
            && exact_owner_grant
            && let Some(direct) =
                direct_capture_for_row(current, selected, imported, direct_grant.as_ref())
                    .inspect_err(|_| diagnose_observation_refusal("selected_direct_capture"))?
        {
            #[cfg(test)]
            eprintln!("catalog_runtime_capture_selected origin=direct_selected_row");
            return Ok((current.clone(), direct));
        }
        diagnose_observation_refusal("selected_row_runtime_equality");
        return Err(StorageError::ScopeChanged);
    }
    if !imported {
        match direct_capture_for_row(current, selected, imported, direct_grant.as_ref()) {
            Ok(Some(direct)) if !same_runtime_inputs(&direct, &observations, selected) => {
                #[cfg(test)]
                eprintln!("catalog_runtime_capture_selected origin=direct_changed_inputs");
                return Ok((current.clone(), direct));
            }
            Err(StorageError::Busy) => return Err(StorageError::Busy),
            _result => {
                #[cfg(test)]
                eprintln!(
                    "catalog_runtime_capture_direct_validation valid={} capture_present={} error={:?}",
                    matches!(&_result, Ok(Some(_))),
                    direct_grant
                        .as_ref()
                        .is_some_and(|(capture, _)| capture.is_some()),
                    _result.as_ref().err()
                );
            }
        }
    }
    #[cfg(test)]
    eprintln!("catalog_runtime_capture_selected origin=immutable");
    Ok((original_binding, observations))
}

fn observation_binding_for_owner(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    physical_owner: Option<CatalogObservationOwner>,
    local_only: bool,
) -> Result<crate::storage::QueryRuntimeBinding, ObservationPreparationError> {
    observation_authority_for_owner(db, current, selected, physical_owner, local_only)
        .map(|(binding, _)| binding)
}

/// Preserve the actual authority selected by the existing verifier so a later
/// recipe read cannot re-resolve its owner from a lifecycle display tag.
fn observation_authority_for_owner(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    selected: &CatalogRow,
    physical_owner: Option<CatalogObservationOwner>,
    local_only: bool,
) -> Result<
    (
        crate::storage::QueryRuntimeBinding,
        crate::storage::query::QueryRecipeResidence,
    ),
    ObservationPreparationError,
> {
    let ns = current.namespace.as_str();
    let owner_kind = physical_owner.map(CatalogObservationOwner::kind);
    let proof_kind = physical_owner.map(|owner| match owner {
        CatalogObservationOwner::Catalog => "current",
        CatalogObservationOwner::History => "history",
    });
    let grant = format!("{}:{}", current.admission, current.stream_epoch);
    let mut metadata = selected.clone();
    metadata.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let current_metadata =
        serde_json::to_string(&metadata).map_err(|_| StorageError::Unavailable)?;
    metadata.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
    let retained_metadata = if physical_owner.is_some()
        || selected.lifecycle == crate::model::summary::CatalogLifecycle::Current
    {
        current_metadata.clone()
    } else {
        serde_json::to_string(&metadata).map_err(|_| StorageError::Unavailable)?
    };
    let admission = i64::try_from(current.admission).map_err(|_| StorageError::ScopeChanged)?;
    for (retained, original) in [(false, &current_metadata), (true, &retained_metadata)] {
        if (physical_owner.is_some()
            || selected.lifecycle == crate::model::summary::CatalogLifecycle::Current)
            && retained
        {
            continue;
        }
        let native: Option<(String, Option<String>, bool)> = db
            .query_row(
                "SELECT owner_kind,CASE WHEN octet_length(direct_capture)<=8388608 THEN direct_capture END,direct_capture IS NOT NULL AND octet_length(direct_capture)>8388608 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind IN('catalog','history') AND metadata_digest=?3 AND admission=?4 AND stream=?5 AND (?6 IS NULL OR owner_kind=?6) ORDER BY owner_kind LIMIT 1",
                params![
                    ns,
                    selected.key,
                    proof_codec::digest(original.as_bytes()),
                    admission,
                    current.stream_epoch,
                    owner_kind
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(ObservationPreparationError::Sql)?;
        if let Some((native, capture, oversized)) = native {
            let reference_kind = proof_kind.unwrap_or(match selected.lifecycle {
                crate::model::summary::CatalogLifecycle::Current => "current",
                crate::model::summary::CatalogLifecycle::Retained => "history",
            });
            let prefix = crate::retention::source_generation(&selected.sources)
                .map_err(|_| StorageError::Unavailable)?;
            if immutable_observation_candidate(
                db,
                ns,
                &selected.key,
                reference_kind,
                &prefix,
                &current.stream_epoch,
                &current_metadata,
            )? {
                let verified = authenticate_immutable_observation(
                    db,
                    current,
                    selected,
                    reference_kind,
                    &prefix,
                )?;
                // A different owner needs its own authenticated capture. A
                // metadata digest alone cannot lend authority to a receipt.
                if (verified.record.metadata == *original
                    && verified.record.admission == current.admission)
                    || direct_capture_for_row(
                        current,
                        selected,
                        imported_catalog(db, current)?,
                        Some(&(capture, oversized)),
                    )?
                    .is_none()
                {
                    authorize_immutable_observation(
                        db,
                        current,
                        selected,
                        reference_kind,
                        local_only,
                        &verified,
                    )?;
                }
            }
            let metadata_digest = proof_codec::digest(original.as_bytes());
            let residence = match native.as_str() {
                "catalog" => {
                    crate::storage::query::QueryRecipeResidence::NativeCatalog { metadata_digest }
                }
                "history" => {
                    crate::storage::query::QueryRecipeResidence::NativeHistory { metadata_digest }
                }
                _ => return Err(StorageError::ScopeChanged.into()),
            };
            return Ok((current.clone(), residence));
        }
    }
    let current_digest = proof_codec::digest(current_metadata.as_bytes());
    let retained_digest = proof_codec::digest(retained_metadata.as_bytes());
    if cfg!(test) || crate::native_timing::enabled() {
        // Observe at most the two owner slots after ordinary authority missed.
        // This diagnostic never supplies authority or changes its outcome.
        let slots = db
            .prepare(
                "SELECT owner_kind='catalog',metadata_digest IN (?3,?4),admission=?5,stream=?6 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 ORDER BY owner_kind LIMIT 2",
            )
            .and_then(|mut statement| {
                let slots = statement.query_map(
                    params![
                        ns,
                        selected.key,
                        current_digest,
                        retained_digest,
                        admission,
                        current.stream_epoch
                    ],
                    |row| {
                        Ok((
                            row.get::<_, bool>(0)?,
                            row.get::<_, bool>(1)?,
                            row.get::<_, bool>(2)?,
                            row.get::<_, bool>(3)?,
                        ))
                    },
                )?;
                slots.collect::<rusqlite::Result<Vec<_>>>()
            });
        match slots {
            Ok(slots) => {
                for (catalog, owner) in [(true, "catalog"), (false, "history")] {
                    let slot = slots.iter().find(|slot| slot.0 == catalog);
                    let present = slot.is_some();
                    let digest_match = slot.is_some_and(|slot| slot.1);
                    let admission_match = slot.is_some_and(|slot| slot.2);
                    let stream_match = slot.is_some_and(|slot| slot.3);
                    eprintln!(
                        "semon sessions native: observation_direct_grant_miss owner={owner} present={present} digest_match={digest_match} admission_match={admission_match} stream_match={stream_match}"
                    );
                }
            }
            Err(_) => {
                eprintln!("semon sessions native: observation_direct_grant_diagnostic=unavailable");
            }
        }
        let current_row = observation_row_metadata(db, ns, &selected.key, false);
        let history_row = observation_row_metadata(db, ns, &selected.key, true);
        log_observation_row_comparison(
            "selected_current_equal",
            current_row
                .as_ref()
                .and_then(|row| observation_row_comparison(selected, row)),
        );
        log_observation_row_comparison(
            "selected_history_equal",
            history_row
                .as_ref()
                .and_then(|row| observation_row_comparison(selected, row)),
        );
    }
    // Stage grants hash the exact metadata and bind its admission/stream. The
    // joined immutable presentation root is checked below against g.metadata.
    let mut statement = db
        .prepare(
            "SELECT DISTINCT g.id,g.publication,g.proof,g.admission,g.stream,
                CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,
                CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,
                g.prefix,r.base,
                CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END,
                CASE WHEN octet_length(v.metadata)<=8388608 THEN v.metadata END,
                ((NOT ?11 AND EXISTS(
                    SELECT 1 FROM main.meta m JOIN main.meta a
                      ON a.namespace=m.namespace AND a.key='native_transfer_v1:catalog-owner:'||m.value
                    WHERE m.namespace=f.namespace
                      AND m.key='native_transfer_v1:publication:'||g.publication
                      AND a.value=?3
                )) OR (
                    g.admission=?8 AND g.stream=?9 AND EXISTS(
                        SELECT 1 FROM main.meta m
                        WHERE m.namespace=f.namespace
                          AND m.key='catalog_native_observation:'||(CASE WHEN g.metadata=?4 THEN ?6 ELSE ?7 END)
                          AND m.value=?3
                    ))
                ),f.kind
             FROM storage_proof_refs f
             JOIN storage_proof_generations g
               ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key
             JOIN storage_semantic_publications p
               ON p.namespace=g.namespace AND p.generation=g.publication
              AND p.proof_digest=g.proof AND p.admission=g.admission
             JOIN storage_proof_presentation_roots r
               ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest
             JOIN storage_proof_presentations v
               ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key
             WHERE f.namespace=?1 AND f.session_key=?2 AND g.metadata IN (?4,?5)
               AND (?10 IS NULL OR f.kind=?10)
             ORDER BY g.id LIMIT 1",
        )
        .map_err(ObservationPreparationError::Sql)?;
    let values = statement
        .query_map(
            params![
                ns,
                selected.key,
                grant,
                current_metadata,
                retained_metadata,
                current_digest,
                retained_digest,
                admission,
                current.stream_epoch,
                proof_kind,
                local_only
            ],
            |r| {
                let admission = u64::try_from(r.get::<_, i64>(3)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                Ok((
                    proof_codec::GenerationRecord {
                        namespace: ns.to_owned(),
                        id: ProofGenerationId(r.get(0)?),
                        session_key: selected.key.clone(),
                        publication: r.get(1)?,
                        proof: r.get(2)?,
                        admission,
                        stream: r.get(4)?,
                        metadata: r.get(5)?,
                        slots: r.get(6)?,
                        prefix: r.get(7)?,
                    },
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, bool>(11)?,
                    r.get::<_, String>(12)?,
                ))
            },
        )
        .map_err(ObservationPreparationError::Sql)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ObservationPreparationError::Sql)?;
    let (record, base, manifest, presentation, granted, reference_kind) = values
        .into_iter()
        .next()
        .ok_or(StorageError::ScopeChanged)
        .inspect_err(|_| {
            if crate::native_timing::enabled() {
                eprintln!(
                    "semon sessions native: observation_stage_refused boundary=candidate_absent"
                );
            }
        })?;
    if !granted {
        if crate::native_timing::enabled() {
            eprintln!("semon sessions native: observation_stage_refused boundary=grant_mismatch");
        }
        return Err(StorageError::ScopeChanged.into());
    }
    crate::selected_read::charge(
        64,
        record.metadata.len() + record.slots.len() + manifest.len() + presentation.len(),
    )
    .map_err(|_| StorageError::Busy)?;
    proof_codec::verify_presentation_digest(&base, &manifest, &record.proof).inspect_err(|_| {
        if crate::native_timing::enabled() {
            eprintln!(
                "semon sessions native: observation_stage_refused boundary=presentation_digest"
            );
        }
    })?;
    proof_codec::verify_presentation_entry(
        &selected.key,
        &record.metadata,
        &record.slots,
        &presentation,
        &manifest,
    )
    .inspect_err(|_| {
        if crate::native_timing::enabled() {
            eprintln!(
                "semon sessions native: observation_stage_refused boundary=presentation_entry"
            );
        }
    })?;
    let residence = crate::storage::query::QueryRecipeResidence::Immutable {
        owner: match reference_kind.as_str() {
            "current" => crate::storage::query::QueryRecipeOwner::Catalog,
            "history" => crate::storage::query::QueryRecipeOwner::History,
            _ => return Err(StorageError::ScopeChanged.into()),
        },
        generation: record.id.clone(),
        publication: record.publication.clone(),
    };
    proof_codec::observation_binding(record, current, selected)
        .inspect_err(|_| {
            if crate::native_timing::enabled() {
                eprintln!("semon sessions native: observation_stage_refused boundary=decoded_row");
            }
        })
        .map(|binding| (binding, residence))
        .map_err(ObservationPreparationError::Evidence)
}

/// Issue a token only after the selected physical row and its existing grant or
/// imported original receipt qualify together in this snapshot. `owner` comes
/// from the selection's actual Catalog/History read, never its display lifecycle.
pub(crate) fn query_recipe_observation(
    db: &Connection,
    current: &crate::storage::QueryRuntimeBinding,
    owner: CatalogObservationOwner,
    selected: &CatalogRow,
) -> Result<crate::storage::query::QueryRecipeObservation, StorageError> {
    let metadata: String = db.query_row(
        &format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.{} WHERE namespace=?1 AND session_key=?2", owner.table()),
        params![current.namespace.as_str(), selected.key],
        |row| row.get(0),
    ).map_err(failed)?;
    let stored: CatalogRow =
        serde_json::from_str(&metadata).map_err(|_| StorageError::Unavailable)?;
    if serde_json::to_value(&stored).map_err(|_| StorageError::Unavailable)?
        != serde_json::to_value(selected).map_err(|_| StorageError::Unavailable)?
    {
        return Err(StorageError::ScopeChanged);
    }
    let (original, residence) =
        observation_authority_for_owner(db, current, &stored, Some(owner), false)
            .map_err(ObservationPreparationError::into_storage)?;
    crate::storage::query::QueryRecipeObservation::qualified(
        residence,
        metadata,
        original,
        current.clone(),
    )
}

/// Attach only the request-local payload; metadata and its exact persisted
/// reference continue to be the observation checked by later witnesses.
pub(crate) fn attach_query_recipe(
    db: &Connection,
    reader: &NativeReaderBinding,
    owner: CatalogObservationOwner,
    row: &mut CatalogRow,
) -> Result<usize, StorageError> {
    use crate::storage::query::QueryRecipeLoadError as Error;
    let current = reader.runtime_binding_at(db)?;
    let observation = query_recipe_observation(db, &current, owner, row)?;
    let (recipe, bytes) =
        load_query_recipe_with_bytes(db, reader, &observation).map_err(|error| match error {
            Error::Storage(error) => error,
            _ => StorageError::ScopeChanged,
        })?;
    row.query_recipe = recipe;
    Ok(bytes)
}

/// The reader owns the admitted snapshot. A token never reselects another
/// residence when its original owner, generation or grant no longer qualifies.
/// None means authenticated inline metadata (or no Query observation) with no
/// external cell. A detached reference never resolves to None.
pub(crate) fn load_query_recipe(
    db: &Connection,
    reader: &NativeReaderBinding,
    observation: &crate::storage::query::QueryRecipeObservation,
) -> Result<
    Option<crate::query_projection::SelectedRecipe>,
    crate::storage::query::QueryRecipeLoadError,
> {
    load_query_recipe_with_bytes(db, reader, observation).map(|(recipe, _)| recipe)
}

fn load_query_recipe_with_bytes(
    db: &Connection,
    reader: &NativeReaderBinding,
    observation: &crate::storage::query::QueryRecipeObservation,
) -> Result<
    (Option<crate::query_projection::SelectedRecipe>, usize),
    crate::storage::query::QueryRecipeLoadError,
> {
    use crate::storage::query::{
        QueryRecipeLoadError as Error, QueryRecipeOwner, QueryRecipeResidence,
    };
    let current = reader.runtime_binding_at(db)?;
    if current != observation.destination {
        return Err(Error::ObservationChanged);
    }
    let selected = observation.row();
    let namespace = current.namespace.as_str();
    let current_admission =
        i64::try_from(current.admission).map_err(|_| Error::ObservationChanged)?;
    let payload = match &observation.residence {
        QueryRecipeResidence::NativeCatalog { metadata_digest }
        | QueryRecipeResidence::NativeHistory { metadata_digest } => {
            let (owner, kind) = match &observation.residence {
                QueryRecipeResidence::NativeCatalog { .. } => {
                    (CatalogObservationOwner::Catalog, "current")
                }
                _ => (CatalogObservationOwner::History, "history"),
            };
            let cells: Option<(Option<String>, Option<String>, bool)> = db.query_row(
                &format!("SELECT CASE WHEN octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576 THEN metadata END,CASE WHEN octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576 THEN query_recipe END,octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576 FROM main.{} WHERE namespace=?1 AND session_key=?2", owner.table()),
                params![namespace, selected.key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(failed)?;
            let (metadata, payload, within) = cells.ok_or(Error::ObservationChanged)?;
            if !within {
                return Err(StorageError::HistoryRetentionBounds.into());
            }
            let metadata = metadata.ok_or(Error::CorruptPayload)?;
            crate::selected_read::charge(
                1,
                metadata.len() + payload.as_ref().map_or(0, String::len),
            )
            .map_err(|_| StorageError::Busy)?;
            if metadata != observation.metadata || observation.original != current {
                return Err(Error::ObservationChanged);
            }
            let mut normalized = selected.clone();
            normalized.lifecycle = crate::model::summary::CatalogLifecycle::Current;
            let normalized =
                serde_json::to_string(&normalized).map_err(|_| Error::CorruptPayload)?;
            if proof_codec::digest(normalized.as_bytes()) != *metadata_digest {
                return Err(Error::ObservationChanged);
            }
            let prefix = crate::retention::source_generation(&selected.sources)
                .map_err(|_| Error::CorruptPayload)?;
            // Preserve immutable-corruption refusal ahead of mutable authority.
            if immutable_observation_candidate(
                db,
                namespace,
                &selected.key,
                kind,
                &prefix,
                &current.stream_epoch,
                &normalized,
            )
            .map_err(ObservationPreparationError::into_storage)?
            {
                let verified =
                    authenticate_immutable_observation(db, &current, selected, kind, &prefix)
                        .map_err(ObservationPreparationError::into_storage)?;
                // This token reads the native row's recipe, never the
                // predecessor receipt's payload or original coordinates.
                if verified.record.metadata == normalized
                    && verified.record.admission == current.admission
                {
                    authorize_immutable_observation(db, &current, selected, kind, false, &verified)
                        .map_err(ObservationPreparationError::into_storage)?;
                }
            }
            let granted: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3 AND metadata_digest=?4 AND admission=?5 AND stream=?6)",
                params![namespace, selected.key, owner.kind(), metadata_digest, current_admission, current.stream_epoch],
                |row| row.get(0),
            ).map_err(failed)?;
            if !granted {
                return Err(Error::ObservationChanged);
            }
            payload
        }
        QueryRecipeResidence::Immutable {
            owner,
            generation,
            publication,
        } => {
            let original_admission = i64::try_from(observation.original.admission)
                .map_err(|_| Error::ObservationChanged)?;
            let kind = match owner {
                QueryRecipeOwner::Catalog => "current",
                QueryRecipeOwner::History => "history",
            };
            let cells = db.query_row(
                "SELECT g.proof,g.admission,g.stream,
                    CASE WHEN octet_length(g.metadata)+coalesce(octet_length(g.query_recipe),0)<=1048576 THEN g.metadata END,
                    CASE WHEN octet_length(g.metadata)+coalesce(octet_length(g.query_recipe),0)<=1048576 THEN g.query_recipe END,
                    CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,g.prefix,r.base,
                    CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END,
                    CASE WHEN octet_length(v.metadata)<=8388608 THEN v.metadata END,
                    octet_length(g.metadata)+coalesce(octet_length(g.query_recipe),0)<=1048576
                 FROM storage_proof_refs f
                 JOIN storage_proof_generations g ON g.namespace=f.namespace AND g.id=f.id AND g.session_key=f.session_key
                 JOIN storage_semantic_publications p ON p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission
                 JOIN storage_proof_presentation_roots r ON r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest
                 JOIN storage_proof_presentations v ON v.namespace=g.namespace AND v.id=g.id AND v.session_key=g.session_key
                 WHERE f.namespace=?1 AND f.session_key=?2 AND f.kind=?3 AND g.id=?4 AND g.publication=?5 AND g.admission=?6 AND g.stream=?7",
                params![namespace, selected.key, kind, generation.0, publication, original_admission, observation.original.stream_epoch],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?, row.get::<_, Option<String>>(5)?, row.get::<_, String>(6)?, row.get::<_, String>(7)?, row.get::<_, Option<String>>(8)?, row.get::<_, Option<String>>(9)?, row.get::<_, bool>(10)?)),
            ).optional().map_err(failed)?.ok_or(Error::ObservationChanged)?;
            let (
                proof,
                admission,
                stream,
                metadata,
                payload,
                slots,
                prefix,
                base,
                manifest,
                presentation,
                within,
            ) = cells;
            if !within {
                return Err(StorageError::HistoryRetentionBounds.into());
            }
            let metadata = metadata.ok_or(Error::CorruptPayload)?;
            let slots = slots.ok_or(StorageError::HistoryRetentionBounds)?;
            let manifest = manifest.ok_or(StorageError::HistoryRetentionBounds)?;
            // Presentation observations retain their independent 8 MiB bound.
            let presentation = presentation.ok_or(StorageError::HistoryRetentionBounds)?;
            crate::selected_read::charge(
                64,
                metadata.len()
                    + payload.as_ref().map_or(0, String::len)
                    + slots.len()
                    + manifest.len()
                    + presentation.len(),
            )
            .map_err(|_| StorageError::Busy)?;
            let grant = format!("{}:{}", current.admission, current.stream_epoch);
            let digest = proof_codec::digest(metadata.as_bytes());
            let granted: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM main.meta m JOIN main.meta a ON a.namespace=m.namespace AND a.key='native_transfer_v1:catalog-owner:'||m.value WHERE m.namespace=?1 AND m.key='native_transfer_v1:publication:'||?2 AND a.value=?3) OR (?4=?5 AND EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key='catalog_native_observation:'||?6 AND value=?3))",
                params![namespace, publication, grant, admission, current_admission, digest],
                |row| row.get(0),
            ).map_err(failed)?;
            if !granted {
                return Err(Error::ObservationChanged);
            }
            proof_codec::verify_presentation_digest(&base, &manifest, &proof)?;
            proof_codec::verify_presentation_entry(
                &selected.key,
                &metadata,
                &slots,
                &presentation,
                &manifest,
            )?;
            let original: CatalogRow =
                serde_json::from_str(&metadata).map_err(|_| Error::CorruptPayload)?;
            if original.lifecycle != crate::model::summary::CatalogLifecycle::Current {
                return Err(Error::ObservationChanged);
            }
            let record = proof_codec::GenerationRecord {
                namespace: namespace.to_owned(),
                id: generation.clone(),
                session_key: selected.key.clone(),
                publication: publication.clone(),
                proof,
                admission: u64::try_from(admission).map_err(|_| Error::ObservationChanged)?,
                stream,
                metadata,
                slots,
                prefix,
            };
            if proof_codec::observation_binding(record, &current, selected)? != observation.original
            {
                return Err(Error::ObservationChanged);
            }
            payload
        }
    };
    // Inline selected metadata remains the ordinary producer's current
    // representation. It has authenticated above and owns no external cell.
    if !crate::query_projection::SelectedRecipe::detached(selected) {
        return if payload.is_none() {
            Ok((None, 0))
        } else {
            Err(Error::CorruptPayload)
        };
    }
    let payload = payload.ok_or(crate::storage::query::QueryRecipeLoadError::MissingPayload)?;
    observation
        .decode(&payload)
        .map(|recipe| (Some(recipe), payload.len()))
        .map_err(|_| crate::storage::query::QueryRecipeLoadError::CorruptPayload)
}

/// Semantic Stage retains exact-row meta grants beside its immutable
/// publication receipts. Direct Catalog grants use owner slots below.
pub(crate) fn native_observation_grant(
    db: &Connection,
    namespace: &str,
    admission: u64,
    stream: &str,
    metadata: &str,
) -> Result<(), StorageError> {
    db.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,?2,?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value", params![namespace,format!("catalog_native_observation:{}",proof_codec::digest(metadata.as_bytes())),format!("{admission}:{stream}")]).map_err(failed)?;
    Ok(())
}

/// The direct Catalog writer owns one producer grant for the latest
/// `session_catalog` metadata of each session. Semantic Stage keeps its
/// immutable receipt grants in `meta` through `native_observation_grant`.
pub(crate) fn native_catalog_observation_grant(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    admission: u64,
    stream: &str,
    metadata: &str,
    observations: Option<&crate::storage::selected::PresentationObservations>,
) -> Result<(), StorageError> {
    let mut row: CatalogRow =
        serde_json::from_str(metadata).map_err(|_| StorageError::Unavailable)?;
    if row.key != session_key {
        return Err(StorageError::Unavailable);
    }
    // Catalog lifecycle is a display change. The publisher grant hashes the
    // row in its producer form so either retained display tag selects it.
    row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let metadata = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    let exact_semantic_owner = exact_immutable_observation_owner(
        db,
        namespace,
        session_key,
        "current",
        admission,
        stream,
        &metadata,
    )?;
    let direct_capture = if exact_semantic_owner {
        None
    } else {
        match observations.and_then(|observations| observations.runtime_capture_for(&row)) {
            Some(capture) => {
                if !capture.is_runtime_capture_for(&row) {
                    return Err(StorageError::ScopeChanged);
                }
                let capture =
                    serde_json::to_string(&capture).map_err(|_| StorageError::Unavailable)?;
                if capture.len() > DIRECT_CAPTURE_MAX_BYTES {
                    return Err(StorageError::HistoryRetentionBounds);
                }
                Some(capture)
            }
            None => None,
        }
    };
    db.execute(
        "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream,direct_capture) VALUES(?1,?2,'catalog',?3,?4,?5,?6) ON CONFLICT(namespace,session_key,owner_kind) DO UPDATE SET metadata_digest=excluded.metadata_digest,admission=excluded.admission,stream=excluded.stream,direct_capture=excluded.direct_capture",
        params![
            namespace,
            session_key,
            proof_codec::digest(metadata.as_bytes()),
            admission,
            stream,
            direct_capture
        ],
    )
    .map_err(failed)?;
    Ok(())
}

/// Store a fresh composed-row runtime capture in the existing physical
/// History grant. This grants the new row's observation only; it never grants
/// authority to carry predecessor slots.
pub(crate) fn native_history_observation_grant(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    admission: u64,
    stream: &str,
    metadata: &str,
    capture: &str,
) -> Result<(), StorageError> {
    if capture.len() > DIRECT_CAPTURE_MAX_BYTES || stream.len() != 64 {
        return Err(StorageError::HistoryRetentionBounds);
    }
    let mut row: CatalogRow =
        serde_json::from_str(metadata).map_err(|_| StorageError::Unavailable)?;
    if row.key != session_key {
        return Err(StorageError::ScopeChanged);
    }
    row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let owner_metadata = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
    let capture_value: crate::storage::selected::PresentationObservations =
        serde_json::from_str(capture).map_err(|_| StorageError::ScopeChanged)?;
    if !capture_value.is_runtime_capture_for(&row) {
        return Err(StorageError::ScopeChanged);
    }
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    let direct_capture = if exact_immutable_observation_owner(
        db,
        namespace,
        session_key,
        "history",
        admission,
        stream,
        &owner_metadata,
    )? {
        None
    } else {
        Some(capture)
    };
    db.execute(
        "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream,direct_capture) VALUES(?1,?2,'history',?3,?4,?5,?6) ON CONFLICT(namespace,session_key,owner_kind) DO UPDATE SET metadata_digest=excluded.metadata_digest,admission=excluded.admission,stream=excluded.stream,direct_capture=excluded.direct_capture",
        params![
            namespace,
            session_key,
            proof_codec::digest(owner_metadata.as_bytes()),
            admission,
            stream,
            direct_capture
        ],
    )
    .map_err(failed)?;
    Ok(())
}

pub(crate) fn clear_native_observation_captures_owned_by_proofs(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    admission: u64,
    stream: &str,
) -> Result<(), StorageError> {
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    for (owner_kind, reference_kind, table) in [
        ("catalog", "current", "session_catalog"),
        ("history", "history", "session_history_catalog"),
    ] {
        let metadata: Option<Option<String>> = db
            .query_row(
                &format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.{table} WHERE namespace=?1 AND session_key=?2"),
                params![namespace, session_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)?;
        let metadata = match metadata {
            None => continue,
            Some(None) => return Err(StorageError::HistoryRetentionBounds),
            Some(Some(metadata)) => metadata,
        };
        let mut row: CatalogRow =
            serde_json::from_str(&metadata).map_err(|_| StorageError::Unavailable)?;
        if row.key != session_key {
            return Err(StorageError::Unavailable);
        }
        row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
        let normalized = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
        if exact_immutable_observation_owner(
            db,
            namespace,
            session_key,
            reference_kind,
            admission,
            stream,
            &normalized,
        )? {
            let digest = proof_codec::digest(normalized.as_bytes());
            db.execute(
                "UPDATE storage_catalog_native_observation_grants SET direct_capture=NULL WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3 AND metadata_digest=?4 AND admission=?5 AND stream=?6 AND direct_capture IS NOT NULL",
                params![namespace, session_key, owner_kind, digest, admission, stream],
            )
            .map_err(failed)?;
        }
    }
    Ok(())
}

impl CatalogObservationOwner {
    fn table(self) -> &'static str {
        match self {
            Self::Catalog => "session_catalog",
            Self::History => "session_history_catalog",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::History => "history",
        }
    }
}

/// Prepared against an exact stored preimage and its existing producer authority.
/// Apply in the same transaction; SQL faults must remain SQL faults so the cost
/// producer cannot mistake a failed grant write for unavailable source evidence.
pub(crate) struct PreparedObservationUpdate {
    namespace: String,
    key: String,
    owner: CatalogObservationOwner,
    stored: String,
    metadata: String,
    before_digest: String,
    after_digest: String,
    admission: i64,
    stream: String,
    direct_capture: Option<String>,
    existing_slot: bool,
    history_activity: (Option<i64>, bool),
}

impl PreparedObservationUpdate {
    pub(crate) fn publish(self, db: &Transaction<'_>) -> rusqlite::Result<()> {
        let changed = match self.owner {
            CatalogObservationOwner::History => db.execute(
                "UPDATE main.session_history_catalog SET metadata=?3,query_activity=?5,query_active=?6 WHERE namespace=?1 AND session_key=?2 AND metadata=?4",
                params![self.namespace, self.key, self.metadata, self.stored, self.history_activity.0, self.history_activity.1],
            ),
            CatalogObservationOwner::Catalog => db.execute(
                "UPDATE main.session_catalog SET metadata=?3 WHERE namespace=?1 AND session_key=?2 AND metadata=?4",
                params![self.namespace, self.key, self.metadata, self.stored],
            ),
        }?;
        if changed != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if self.existing_slot {
            let changed = db.execute(
                "UPDATE storage_catalog_native_observation_grants SET metadata_digest=?6,direct_capture=?8 WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3 AND metadata_digest=?4 AND admission=?5 AND stream=?7",
                params![self.namespace, self.key, self.owner.kind(), self.before_digest, self.admission, self.after_digest, self.stream, self.direct_capture],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::InvalidQuery);
            }
        } else {
            db.execute(
                "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream,direct_capture) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![self.namespace, self.key, self.owner.kind(), self.after_digest, self.admission, self.stream, self.direct_capture],
            )?;
        }
        Ok(())
    }
}

/// Query repair constructs only a Query postimage; its caller still qualifies
/// the source-backed Query recipe before entering this publication transaction.
pub(crate) fn publish_repaired_query_observation(
    db: &Transaction<'_>,
    current: &crate::storage::QueryRuntimeBinding,
    owner: CatalogObservationOwner,
    selected: &CatalogRow,
    query: &serde_json::Value,
) -> Result<(), StorageError> {
    let source_generation = crate::retention::source_generation(&selected.sources)
        .map_err(|_| StorageError::Unavailable)?;
    if selected.key.is_empty()
        || !query.is_object()
        || query["version"] != 1
        || query["source_generation"].as_str() != Some(source_generation.as_str())
    {
        return Err(StorageError::ScopeChanged);
    }
    let mut repaired = selected.clone();
    repaired.query = Some(query.clone());
    let prepared = prepare_observation_update(db, current, owner, selected, &repaired, false)
        .map_err(ObservationPreparationError::into_storage)?;
    if selected.query.as_ref() == Some(query) {
        return Ok(());
    }
    prepared.publish(db).map_err(failed)
}

/// Cost publication cannot use an imported receipt to mint local authority.
/// Its only mutable fields are the recomputed cost and embedded cost summaries.
pub(crate) fn prepare_repriced_observation(
    db: &Transaction<'_>,
    current: &crate::storage::QueryRuntimeBinding,
    owner: CatalogObservationOwner,
    selected: &CatalogRow,
    cost: &crate::pricing::Cost,
    query: Option<&serde_json::Value>,
) -> Result<PreparedObservationUpdate, ObservationPreparationError> {
    let mut repriced = selected.clone();
    repriced.cost = cost.clone();
    repriced.query = query.cloned();
    prepare_observation_update(db, current, owner, selected, &repriced, true)
}

fn prepare_observation_update(
    db: &Transaction<'_>,
    current: &crate::storage::QueryRuntimeBinding,
    owner: CatalogObservationOwner,
    selected: &CatalogRow,
    repaired: &CatalogRow,
    local_only: bool,
) -> Result<PreparedObservationUpdate, ObservationPreparationError> {
    #[cfg(test)]
    let _fault = ObservationReadFault::install(db, owner);
    let table = owner.table();
    let owner_kind = owner.kind();
    let namespace = current.namespace.as_str();
    let stored: Option<Option<String>> = db
        .query_row(
            &format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.{table} WHERE namespace=?1 AND session_key=?2"),
            params![namespace, selected.key],
            |row| row.get(0),
        )
        .optional()
        .map_err(ObservationPreparationError::Sql)?;
    let stored = match stored {
        Some(Some(stored)) => stored,
        Some(None) => return Err(StorageError::HistoryRetentionBounds.into()),
        None => return Err(StorageError::ScopeChanged.into()),
    };
    let stored_row: CatalogRow =
        serde_json::from_str(&stored).map_err(|_| StorageError::Unavailable)?;
    if serde_json::to_value(&stored_row).map_err(|_| StorageError::Unavailable)?
        != serde_json::to_value(selected).map_err(|_| StorageError::Unavailable)?
    {
        return Err(StorageError::ScopeChanged.into());
    }

    // Repricing requires this owner's local grant or immutable local Stage
    // proof. Query repair retains its ordinary imported observation contract.
    let proven = observation_binding_for_owner(
        db,
        current,
        selected,
        local_only.then_some(owner),
        local_only,
    )?;
    if proven.namespace.as_str() != current.namespace.as_str()
        || proven.stream_epoch != current.stream_epoch
    {
        return Err(StorageError::ScopeChanged.into());
    }

    let mut before = selected.clone();
    before.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let before_metadata = serde_json::to_string(&before).map_err(|_| StorageError::Unavailable)?;
    let before_digest = proof_codec::digest(before_metadata.as_bytes());
    let admission = i64::try_from(current.admission).map_err(|_| StorageError::ScopeChanged)?;
    let proof_kind = match owner {
        CatalogObservationOwner::Catalog => "current",
        CatalogObservationOwner::History => "history",
    };
    let exact_semantic_owner = exact_immutable_observation_owner(
        db,
        namespace,
        &selected.key,
        proof_kind,
        admission,
        &current.stream_epoch,
        &before_metadata,
    )?;
    let owner_slot: Option<(String, i64, String, Option<String>, bool)> = db
        .query_row(
            "SELECT metadata_digest,admission,stream,CASE WHEN octet_length(direct_capture)<=8388608 THEN direct_capture END,direct_capture IS NOT NULL AND octet_length(direct_capture)>8388608 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind=?3",
            params![namespace, selected.key, owner_kind],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(ObservationPreparationError::Sql)?;
    if owner_slot
        .as_ref()
        .is_some_and(|(digest, slot_admission, stream, _, _)| {
            digest != &before_digest
                || *slot_admission != admission
                || stream != &current.stream_epoch
        })
    {
        return Err(StorageError::ScopeChanged.into());
    }
    if owner_slot
        .as_ref()
        .is_some_and(|(_, _, _, _, oversized)| *oversized)
    {
        return Err(StorageError::HistoryRetentionBounds.into());
    }
    let direct_capture = if exact_semantic_owner {
        None
    } else {
        owner_slot
            .as_ref()
            .and_then(|(_, _, _, capture, _)| capture.as_ref())
            .and_then(|capture| {
                serde_json::from_str::<crate::storage::selected::PresentationObservations>(capture)
                    .ok()
                    .filter(|observations| {
                        observations.is_runtime_capture_for(repaired)
                            && observations.runtime_for(repaired).is_some()
                    })
                    .map(|_| capture.clone())
            })
    };

    let metadata = serde_json::to_string(repaired).map_err(|_| StorageError::Unavailable)?;
    let mut normalized = repaired.clone();
    normalized.lifecycle = crate::model::summary::CatalogLifecycle::Current;
    let normalized = serde_json::to_string(&normalized).map_err(|_| StorageError::Unavailable)?;
    if metadata.len() > 1_048_576 || normalized.len() > 1_048_576 {
        return Err(StorageError::HistoryRetentionBounds.into());
    }
    Ok(PreparedObservationUpdate {
        namespace: namespace.to_owned(),
        key: selected.key.clone(),
        owner,
        stored,
        metadata,
        before_digest,
        after_digest: proof_codec::digest(normalized.as_bytes()),
        admission,
        stream: current.stream_epoch.clone(),
        direct_capture,
        existing_slot: owner_slot.is_some(),
        history_activity: crate::storage::query::history_activity(repaired),
    })
}

/// Reconcile both owner slots from the post-publication Catalog rows, keeping
/// only exact producer evidence already attached to those rows. A changed or
/// copied row cannot create local authority.
pub(crate) fn reconcile_native_observation_owners(
    db: &Connection,
    namespace: &str,
    session_key: &str,
    admission: u64,
    stream: &str,
) -> Result<(), StorageError> {
    let admission = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    let catalog: Option<Option<String>> = db
        .query_row(
            "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",
            params![namespace, session_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let history: Option<Option<String>> = db
        .query_row(
            "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2",
            params![namespace, session_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    let normalize = |text: String| -> Result<(String, String), StorageError> {
        let mut row: CatalogRow =
            serde_json::from_str(&text).map_err(|_| StorageError::Unavailable)?;
        if row.key != session_key {
            return Err(StorageError::Unavailable);
        }
        row.lifecycle = crate::model::summary::CatalogLifecycle::Current;
        let metadata = serde_json::to_string(&row).map_err(|_| StorageError::Unavailable)?;
        Ok((proof_codec::digest(metadata.as_bytes()), metadata))
    };
    let catalog_metadata = match catalog {
        None => None,
        Some(None) => return Err(StorageError::HistoryRetentionBounds),
        Some(Some(text)) => Some(normalize(text)?),
    };
    let history_metadata = match history {
        None => None,
        Some(None) => return Err(StorageError::HistoryRetentionBounds),
        Some(Some(text)) => Some(normalize(text)?),
    };
    let catalog_digest = catalog_metadata.as_ref().map(|value| value.0.clone());
    let history_digest = history_metadata.as_ref().map(|value| value.0.clone());
    let catalog_proof_owned = catalog_metadata
        .as_ref()
        .map(|(_, metadata)| {
            exact_immutable_observation_owner(
                db,
                namespace,
                session_key,
                "current",
                admission,
                stream,
                metadata,
            )
        })
        .transpose()?
        .unwrap_or(false);
    let history_proof_owned = history_metadata
        .as_ref()
        .map(|(_, metadata)| {
            exact_immutable_observation_owner(
                db,
                namespace,
                session_key,
                "history",
                admission,
                stream,
                metadata,
            )
        })
        .transpose()?
        .unwrap_or(false);
    let mut statement = db
        .prepare(
            "SELECT owner_kind,metadata_digest,admission,stream,CASE WHEN octet_length(direct_capture)<=8388608 THEN direct_capture END,direct_capture IS NOT NULL AND octet_length(direct_capture)>8388608 FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 LIMIT 3",
        )
        .map_err(failed)?;
    let grants = statement
        .query_map(params![namespace, session_key], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })
        .map_err(failed)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(failed)?;
    if grants.len() > 2 {
        return Err(StorageError::Unavailable);
    }
    if grants.iter().any(|grant| grant.5) {
        return Err(StorageError::HistoryRetentionBounds);
    }
    let owned = |kind: &str, digest: Option<&String>| {
        digest.is_some_and(|digest| {
            grants.iter().any(|grant| {
                grant.0.as_str() == kind
                    && &grant.1 == digest
                    && grant.2 == admission
                    && grant.3.as_str() == stream
            })
        })
    };
    if catalog_proof_owned && let Some(digest) = catalog_digest.as_ref() {
        db.execute(
            "UPDATE storage_catalog_native_observation_grants SET direct_capture=NULL WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog' AND metadata_digest=?3 AND admission=?4 AND stream=?5 AND direct_capture IS NOT NULL",
            params![namespace, session_key, digest, admission, stream],
        )
        .map_err(failed)?;
    }
    if !owned("catalog", catalog_digest.as_ref()) {
        db.execute(
            "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='catalog'",
            params![namespace, session_key],
        )
        .map_err(failed)?;
    }
    let history_owned = owned("history", history_digest.as_ref());
    let history_capture = history_digest.as_ref().and_then(|digest| {
        grants
            .iter()
            .find(|grant| {
                grant.0.as_str() == "catalog"
                    && &grant.1 == digest
                    && grant.2 == admission
                    && grant.3.as_str() == stream
            })
            .map(|grant| grant.4.clone())
    });
    if history_proof_owned && let Some(digest) = history_digest.as_ref() {
        db.execute(
            "UPDATE storage_catalog_native_observation_grants SET direct_capture=NULL WHERE namespace=?1 AND session_key=?2 AND owner_kind='history' AND metadata_digest=?3 AND admission=?4 AND stream=?5 AND direct_capture IS NOT NULL",
            params![namespace, session_key, digest, admission, stream],
        )
        .map_err(failed)?;
    }
    // Preserve the old exact Catalog grant when its row becomes History,
    // including when immutable receipts own the observation. Refs alone do
    // not create a grant: the old producer digest, admission and stream match.
    if !history_owned && history_capture.is_some() {
        let digest = history_digest.as_ref().ok_or(StorageError::Unavailable)?;
        let capture = if history_proof_owned {
            None
        } else {
            history_capture.flatten()
        };
        db.execute(
            "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream,direct_capture) VALUES(?1,?2,'history',?3,?4,?5,?6) ON CONFLICT(namespace,session_key,owner_kind) DO UPDATE SET metadata_digest=excluded.metadata_digest,admission=excluded.admission,stream=excluded.stream,direct_capture=excluded.direct_capture",
            params![namespace, session_key, digest, admission, stream, capture],
        )
        .map_err(failed)?;
    } else if !history_owned {
        db.execute(
            "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=?1 AND session_key=?2 AND owner_kind='history'",
            params![namespace, session_key],
        )
        .map_err(failed)?;
    }
    Ok(())
}

fn validate_publication_owner(
    db: &Connection,
    owner: &crate::events::store::configured::NativeOwnership,
    publication: &str,
    producer_admission: u64,
) -> Result<(), StorageError> {
    if producer_admission == owner.admission() {
        return Ok(());
    }
    let stream = owner
        .config()
        .expected_stream_epoch
        .as_deref()
        .ok_or(StorageError::ScopeChanged)?;
    let binding = format!("{}:{stream}", owner.admission());
    let admitted: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM main.meta p JOIN main.meta a ON a.namespace=p.namespace AND a.key='native_transfer_v1:catalog-owner:'||p.value WHERE p.namespace=?1 AND p.key=?2 AND a.value=?3)", params![owner.config().namespace.as_str(), format!("native_transfer_v1:publication:{publication}"), binding], |r| r.get(0)).map_err(failed)?;
    if admitted {
        Ok(())
    } else {
        Err(StorageError::ScopeChanged)
    }
}

struct ControlledReader<'a> {
    reader: &'a dyn crate::SessionSourceReader,
    control: &'a crate::SelectedReadControl,
}
impl crate::SessionSourceReader for ControlledReader<'_> {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> std::io::Result<crate::SessionSourceRange> {
        self.control.check()?;
        let result = self.reader.read_range(source, expected, offset, max)?;
        self.control.check()?;
        Ok(result)
    }
}

impl ProofReaderLease for Lease {
    fn snapshot(&self) -> &ReaderSnapshot {
        &self.snapshot
    }
    fn row(&self) -> &CatalogRow {
        &self.row
    }
    fn renew(&mut self, ttl_ms: u32) -> Result<(), StorageError> {
        let expires = ttl(ttl_ms)?;
        let mut db =
            super::source_journal::SourceJournal::owned(self.owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.validate(&tx)?;
        tx.execute(
            "UPDATE storage_proof_leases SET expires=?3 WHERE namespace=?1 AND token=?2",
            params![self.snapshot.namespace.as_str(), self.token, expires],
        )
        .map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)
    }
    fn release(&mut self) -> Result<(), StorageError> {
        let mut db =
            super::source_journal::SourceJournal::owned(self.owner.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        self.owner.validate_ready(&tx)?;
        let epoch: i64 = tx
            .query_row(
                "SELECT epoch FROM storage_proof_reader_owners WHERE namespace=?1",
                [self.snapshot.namespace.as_str()],
                |r| r.get(0),
            )
            .map_err(failed)?;
        if epoch != self.owner_epoch {
            return Err(StorageError::ScopeChanged);
        }
        tx.execute("DELETE FROM storage_proof_leases WHERE namespace=?1 AND token=?2 AND id=?3 AND owner_epoch=?4 AND serial=?5", params![self.snapshot.namespace.as_str(), self.token, self.snapshot.proof_generation.0, self.owner_epoch, self.serial]).map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        self.released = true;
        Ok(())
    }
    fn read_record_with_control(
        &self,
        options: &crate::Options,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
        path: &std::path::Path,
        offset: u64,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<Option<serde_json::Value>> {
        control.check()?;
        let binding = NativeReaderBinding::Semantic(self.owner.clone());
        let db = binding.connection().map_err(std::io::Error::other)?;
        db.execute_batch("BEGIN DEFERRED")
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        let sources = crate::query_source::Sources::capture_bound_snapshot(
            options,
            &self.row.sources,
            db,
            &self.owner,
        )?;
        let records = sources.reader_with(provider);
        let source = records
            .source_ref(path)
            .ok_or_else(|| std::io::Error::other("selected source unavailable"))?;
        let controlled = ControlledReader {
            reader: &records,
            control,
        };
        let mut budget = 1_048_576;
        let record = crate::source_reader::read_record_bounded(
            &controlled,
            source,
            offset,
            1_048_576,
            &mut budget,
        )?;
        records.finish()?;
        // A fresh authority/lease check fences revocation during byte I/O.
        let db = binding.connection().map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        control.check()?;
        Ok(Some(record))
    }
    fn transcript_records(
        &self,
        options: &crate::Options,
        provider: Option<(
            &dyn crate::SessionSourceReader,
            crate::SessionSourceReadMode,
        )>,
        start: usize,
        limit: usize,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<ReaderPage> {
        control.check()?;
        if limit == 0 || limit > 8 {
            return Err(std::io::Error::other("private selected page budget"));
        }
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(std::io::Error::other)?;
        db.execute_batch("BEGIN DEFERRED")
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        let text: String = db.query_row("SELECT slots FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3", params![self.snapshot.namespace.as_str(), self.snapshot.proof_generation.0, self.row.key], |r| r.get(0)).map_err(std::io::Error::other)?;
        if text.len() > 1_048_576 {
            return Err(std::io::Error::other("private selected recipe budget"));
        }
        let slots: Option<Vec<crate::slot_projection::SavedSlot>> =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        let slots = slots.ok_or_else(|| std::io::Error::other("selected recipes unavailable"))?;
        if start > slots.len() {
            return Err(std::io::Error::other("selected page range"));
        }
        let sources = crate::query_source::Sources::capture_bound_snapshot(
            options,
            &self.row.sources,
            db,
            &self.owner,
        )?;
        let native = sources.reader_with(provider);
        let controlled = ControlledReader {
            reader: &native,
            control,
        };
        let mut records = Vec::new();
        let mut bytes = 0usize;
        for slot in slots.into_iter().skip(start).take(limit) {
            control.check()?;
            let value = if let Some(file) = &slot.file {
                let source = native
                    .source_ref(&file.path)
                    .ok_or_else(|| std::io::Error::other("selected source unavailable"))?;
                let mut budget = 1_048_576;
                Some(crate::source_reader::read_record_bounded(
                    &controlled,
                    source,
                    slot.offset,
                    1_048_576,
                    &mut budget,
                )?)
            } else {
                None
            };
            bytes = bytes.saturating_add(
                serde_json::to_vec(&value)
                    .map_err(std::io::Error::other)?
                    .len(),
            );
            if bytes > 2 * 1_048_576 {
                return Err(std::io::Error::other("private selected body budget"));
            }
            records.push((slot, value));
        }
        native.finish()?;
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        control.check()?;
        Ok(ReaderPage {
            snapshot: self.snapshot.clone(),
            records,
        })
    }
}

fn joined_digest(base: &str, manifest: &str) -> Result<String, StorageError> {
    proof_codec::presentation_digest(base, manifest)
}

/// The caller has completed every producer write in the same publication TX.
/// Capture typed metadata, never Raw bodies, before committing the proof root.
pub(crate) fn capture_presentations(
    tx: &Transaction<'_>,
    ns: &str,
    inputs: PresentationInputs<'_>,
    observed_at: i64,
) -> Result<Vec<(String, String)>, StorageError> {
    validate_receipt_inputs(&inputs)?;
    #[cfg(test)]
    if FAIL_PRESENTATION_CAPTURE_ONCE.with(|value| value.replace(false)) {
        return Err(StorageError::HistoryRetentionBounds);
    }
    use crate::storage::selected::{CapturedContext, PresentationWitness};
    let PresentationInputs {
        catalog_generation,
        rows,
        projections,
        observations,
    } = inputs;
    proof_codec::validate_catalog_generation(catalog_generation)?;
    let lookup = super::selected::Context::owned(tx);
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for row in rows {
        let projection = projections
            .unwrap_or_default()
            .iter()
            .find(|projection| projection.key == row.key)
            .ok_or(StorageError::Unavailable)?;
        let texts = projection
            .slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| StorageError::Unavailable)?;
        let header_generation = crate::slot_projection::generation(&texts, &row.sources)
            .map_err(|_| StorageError::Unavailable)?;
        let header: (String, u32, i64) = tx
            .query_row(
                "SELECT generation,version,total FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2",
                params![ns, row.key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(failed)?;
        if header.0 != header_generation
            || header.1 != crate::slot_projection::VERSION
            || usize::try_from(header.2).ok() != Some(texts.len())
        {
            return Err(StorageError::ScopeChanged);
        }
        let has_history = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=?1 AND session_key=?2)",
                params![ns, row.key],
                |r| r.get(0),
            )
            .map_err(failed)?;
        let catalog = [
            (
                "catalog_version".into(),
                crate::model::summary::CATALOG_VERSION.to_string(),
            ),
            ("catalog_generation".into(), catalog_generation.to_owned()),
            (
                "slot_projection_catalog_generation".into(),
                catalog_generation.to_owned(),
            ),
            ("catalog_completeness".into(), "partial".into()),
            ("catalog_observed_at".into(), observed_at.to_string()),
        ]
        .into_iter()
        .collect();
        let mut observations = observations.cloned().ok_or(StorageError::Unavailable)?;
        let context =
            CapturedContext::capture(&lookup, row, &projection.slots, &mut |relative, _owner| {
                observations.capture_relative_status(relative, || {
                    super::catalog::runtime_source_cwds(tx, &relative.sources)
                })
            })
            .map_err(|_| StorageError::Unavailable)?;
        let witness = Some(PresentationWitness {
            header_generation,
            version: header.1,
            total: texts.len(),
            has_history,
            catalog,
            context,
            relative_statuses: std::mem::take(&mut observations.relative_statuses),
            observations,
        });
        let text = serde_json::to_string(&witness).map_err(|_| StorageError::Unavailable)?;
        bytes = bytes.saturating_add(text.len());
        if bytes > 8 * 1048576 {
            return Err(StorageError::HistoryRetentionBounds);
        }
        result.push((row.key.clone(), text));
    }
    Ok(result)
}

pub(crate) fn bind_presentations(
    tx: &Transaction<'_>,
    ns: &str,
    publication: &str,
    base: &str,
    rows: &[(String, String)],
    inputs: PresentationInputs<'_>,
) -> Result<String, StorageError> {
    validate_receipt_inputs(&inputs)?;
    validate_receipt_presentations(inputs.rows, rows)?;
    let root = prepare_presentation_root(base, rows, &inputs)?;
    insert_presentation_root(tx, ns, publication, &root)?;
    Ok(root.digest)
}

pub(crate) fn check_presentation_budget(
    base: &str,
    rows: &[(String, String)],
    inputs: &PresentationInputs<'_>,
) -> Result<(), StorageError> {
    prepare_presentation_root(base, rows, inputs).map(|_| ())
}

fn prepare_presentation_root(
    base: &str,
    rows: &[(String, String)],
    inputs: &PresentationInputs<'_>,
) -> Result<PreparedPresentationRoot, StorageError> {
    let catalog_generation = inputs.catalog_generation;
    let catalog_rows = inputs.rows;
    let projections = inputs.projections;
    let observations = inputs.observations;
    proof_codec::validate_catalog_generation(catalog_generation)?;
    let mut entries = Vec::new();
    for (key, text) in rows {
        let row = catalog_rows
            .iter()
            .find(|row| &row.key == key)
            .ok_or(StorageError::Unavailable)?;
        let metadata = serde_json::to_string(row).map_err(|_| StorageError::Unavailable)?;
        let projection = projections
            .unwrap_or_default()
            .iter()
            .find(|projection| &projection.key == key)
            .ok_or(StorageError::Unavailable)?;
        let slots =
            serde_json::to_string(&projection.slots).map_err(|_| StorageError::Unavailable)?;
        entries.push((
            key.clone(),
            proof_codec::presentation_entry(&metadata, &slots, text)?,
        ));
    }
    let manifest = serde_json::to_string(&PresentationManifest {
        catalog_generation: (*catalog_generation).to_owned(),
        entries,
        observations: observations.cloned(),
    })
    .map_err(|_| StorageError::Unavailable)?;
    if manifest.len() > 1048576 {
        return Err(StorageError::HistoryRetentionBounds);
    }
    let joined = joined_digest(base, &manifest)?;
    Ok(PreparedPresentationRoot {
        base: base.to_owned(),
        manifest,
        digest: joined,
    })
}

fn insert_presentation_root(
    tx: &Transaction<'_>,
    ns: &str,
    publication: &str,
    root: &PreparedPresentationRoot,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO storage_proof_presentation_roots VALUES(?1,?2,?3,?4,?5)",
        params![ns, publication, root.base, root.manifest, root.digest],
    )
    .map_err(failed)?;
    Ok(())
}

/// Give an ordinary Catalog receipt an identity bound to its exact metadata
/// generation and finalized projection/presentation root.
pub(crate) fn catalog_publication_id(
    namespace: &str,
    catalog_generation: &str,
    proof_digest: &str,
) -> Result<String, StorageError> {
    let identity = serde_json::to_vec(&(
        "catalog-proof-publication-v1",
        namespace,
        catalog_generation,
        proof_digest,
    ))
    .map_err(|_| StorageError::Unavailable)?;
    Ok(proof_codec::digest(&identity))
}

pub(super) fn replayed_presentation(
    db: &Connection,
    ns: &str,
    publication: &str,
    base: &str,
    expected: &str,
) -> Result<bool, StorageError> {
    let row: Option<(String,String,String)> = db.query_row("SELECT base,manifest,digest FROM storage_proof_presentation_roots WHERE namespace=?1 AND publication=?2 AND digest=?3",params![ns,publication,expected],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(failed)?;
    let Some((saved, manifest, joined)) = row else {
        return Ok(false);
    };
    if saved != base || manifest.len() > 1048576 || joined != expected {
        return Ok(false);
    }
    proof_codec::decode_presentation_manifest(&manifest)?;
    Ok(joined_digest(&saved, &manifest)? == joined)
}
fn validate_presentation(
    db: &Connection,
    snapshot: &ReaderSnapshot,
    key: &str,
    proof: &str,
) -> Result<String, StorageError> {
    let (base,manifest,joined): (String,String,String) = db.query_row("SELECT base,manifest,digest FROM storage_proof_presentation_roots WHERE namespace=?1 AND publication=?2 AND digest=?3",params![snapshot.namespace.as_str(),snapshot.publication_generation,proof],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    if manifest.len() > 1048576 || joined != proof {
        return Err(StorageError::ScopeChanged);
    }
    proof_codec::verify_presentation_digest(&base, &manifest, &joined)?;
    let (text,metadata,slots): (String,String,String) = db.query_row("SELECT CASE WHEN octet_length(p.metadata)<=8388608 THEN p.metadata END,CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END FROM storage_proof_presentations p JOIN storage_proof_generations g ON g.namespace=p.namespace AND g.id=p.id AND g.session_key=p.session_key WHERE p.namespace=?1 AND p.id=?2 AND p.session_key=?3",params![snapshot.namespace.as_str(),snapshot.proof_generation.0,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    proof_codec::verify_presentation_entry(key, &metadata, &slots, &text, &manifest)?;
    Ok(text)
}

impl crate::storage::selected::SelectedSnapshotStore for Lease {
    fn open_selected(
        &self,
        options: &crate::Options,
        host: &crate::storage::selected::HostSourceBinding,
        control: &crate::SelectedReadControl,
    ) -> std::io::Result<crate::storage::selected::SelectedRead> {
        use std::io;
        control.check()?;
        host.validate(&self.snapshot)?;
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(io::Error::other)?;
        db.execute_batch("BEGIN DEFERRED")
            .map_err(io::Error::other)?;
        self.validate(&db).map_err(io::Error::other)?;
        // validate() authenticated both blobs in this same read transaction.
        let (text,manifest): (String,String) = db.query_row("SELECT p.metadata,r.manifest FROM storage_proof_presentations p JOIN storage_proof_generations g ON g.namespace=p.namespace AND g.id=p.id AND g.session_key=p.session_key JOIN storage_proof_presentation_roots r ON r.namespace=g.namespace AND r.publication=g.publication AND r.digest=g.proof WHERE p.namespace=?1 AND p.id=?2 AND p.session_key=?3",params![self.snapshot.namespace.as_str(),self.snapshot.proof_generation.0,self.row.key],|r|Ok((r.get(0)?,r.get(1)?))).map_err(io::Error::other)?;
        let witness: Option<crate::storage::selected::PresentationWitness> =
            serde_json::from_str(&text).map_err(io::Error::other)?;
        let mut witness =
            witness.ok_or_else(|| io::Error::other("selected presentation unavailable"))?;
        let manifest =
            proof_codec::decode_presentation_manifest(&manifest).map_err(io::Error::other)?;
        let witness_catalog_generation = witness
            .catalog
            .get("catalog_generation")
            .cloned()
            .ok_or_else(|| io::Error::other("selected catalog generation unavailable"))?;
        witness.observations = manifest
            .observations
            .ok_or_else(|| io::Error::other("selected observation witness unavailable"))?;
        witness.observations.relative_statuses = std::mem::take(&mut witness.relative_statuses);
        let text: String = db.query_row("SELECT CASE WHEN octet_length(slots)<=1048576 THEN slots END FROM storage_proof_generations WHERE namespace=?1 AND id=?2 AND session_key=?3",params![self.snapshot.namespace.as_str(),self.snapshot.proof_generation.0,self.row.key],|r|r.get(0)).map_err(io::Error::other)?;
        let slots: Option<Vec<crate::slot_projection::SavedSlot>> =
            serde_json::from_str(&text).map_err(io::Error::other)?;
        let slots = slots.ok_or_else(|| io::Error::other("selected recipes unavailable"))?;
        let texts = slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::Error::other)?;
        if texts.iter().any(|text| text.len() > 65536)
            || slots.len() != witness.total
            || witness.version != crate::slot_projection::VERSION
            || crate::slot_projection::generation(&texts, &self.row.sources)
                .map_err(io::Error::other)?
                != witness.header_generation
            || manifest.catalog_generation != witness_catalog_generation
        {
            return Err(io::Error::other("selected witness changed"));
        }
        let sources = crate::query_source::Sources::capture_bound_snapshot(
            options,
            &self.row.sources,
            db,
            &self.owner,
        )?;
        control.check()?;
        Ok(crate::storage::selected::SelectedRead {
            snapshot: self.snapshot.clone(),
            row: self.row.clone(),
            witness,
            slots,
            sources,
        })
    }
    fn finish_selected(&self, control: &crate::SelectedReadControl) -> std::io::Result<()> {
        control.check()?;
        let db = NativeReaderBinding::Semantic(self.owner.clone())
            .connection()
            .map_err(std::io::Error::other)?;
        self.validate(&db).map_err(std::io::Error::other)?;
        control.check()
    }
}
