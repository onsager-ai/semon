//! Configured navigation reuses the admitted, leased native Catalog snapshot.
use super::{PgCancellation, PgPool, catalog_effects::snapshot, configured_generation};
use crate::storage::{
    StorageError, StoreConfig,
    navigation::{NativeNavigationStore, NativeResolutionObservation},
};

pub(crate) struct Navigation {
    pub(crate) config: StoreConfig,
    pub(crate) pool: PgPool,
    pub(crate) control: PgCancellation,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn captured_source_proof(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    options: &crate::Options,
    generation: &str,
    key: &str,
    reference: &crate::SessionSourceRef,
) -> Result<crate::SessionSourceProof, StorageError> {
    if key.is_empty() || key.len() > 4096 || !crate::is_input_path(&reference.root, &reference.path)
    {
        return Err(StorageError::InvalidArguments);
    }
    super::require_configured(config, pool, control)?;
    let pool = pool.bind_source(
        &config.namespace,
        config.expected_stream_epoch.as_deref(),
        control,
    )?;
    snapshot::with_admitted_snapshot(&pool, &config.namespace, control, generation, |snapshot| {
        let history = snapshot.catalog(key, true)?.is_some();
        let (row, header, _) = snapshot
            .selected(key, history, 0, 1)?
            .ok_or(StorageError::ScopeChanged)?;
        if header.1 != crate::slot_projection::VERSION {
            return Err(StorageError::ScopeChanged);
        }
        let source = row
            .sources
            .iter()
            .find(|source| {
                row.harness == reference.root
                    && crate::catalog::source_relative(options, &row.harness, source).as_deref()
                        == Some(reference.path.as_str())
                    && source.native_id == reference.native_id
                    && source.offset == reference.offset
                    && source.prefix_sha256 == reference.prefix_sha256
                    && source.tail_sha256 == reference.tail_sha256
            })
            .ok_or(StorageError::ScopeChanged)?;
        // Qualify the exact persisted prefix under this publication's lease.
        // A historical recipe's old descriptor clocks are not a witness for
        // today's covering file. The host separately verifies Native block
        // hashes while copying bytes using the returned current descriptor.
        let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
        let absolute = source.path.to_str().ok_or(StorageError::ScopeChanged)?;
        super::catalog_effects::selected_reader::native_proof(&mut tx, generation, source)?;
        tx.check()?;
        let ledger = crate::source_carrier::ledger(source);
        let relocation = super::native_keys::read_meta(
            &mut tx,
            format!("native_catalog_relocation:{absolute}").as_bytes(),
            1024 * 1024,
        )?;
        let relocated = relocation
            .as_deref()
            .map(|bytes| {
                serde_json::from_slice::<crate::events::native_commit::CatalogRelocation>(bytes)
                    .map_err(|_| StorageError::ScopeChanged)
            })
            .transpose()?;
        let (physical_path, physical_owner) = if let Some(proof) = &relocated {
            if proof.version != 1
                || proof.native_id != reference.native_id
                || (proof.prior != ledger && proof.catalog_before.as_ref() != Some(&ledger))
            {
                return Err(StorageError::ScopeChanged);
            }
            let path = std::path::Path::new(&proof.new_path);
            let mut covering = source.clone();
            covering.path = path.to_owned();
            if crate::catalog::source_relative(options, &row.harness, &covering).is_none() {
                return Err(StorageError::ScopeChanged);
            }
            (path, proof.after.stat)
        } else {
            (source.path.as_path(), ledger.stat)
        };
        let file = crate::open_input(physical_path).map_err(|_| StorageError::ScopeChanged)?;
        let stat = crate::events::Stat::from_metadata(
            &file.metadata().map_err(|_| StorageError::ScopeChanged)?,
        )
        .map_err(|_| StorageError::ScopeChanged)?;
        if stat.dev != physical_owner.dev
            || stat.ino != physical_owner.ino
            || stat.size < source.offset
        {
            return Err(StorageError::ScopeChanged);
        }
        let named =
            std::fs::symlink_metadata(physical_path).map_err(|_| StorageError::ScopeChanged)?;
        if !named.is_file()
            || crate::events::Stat::from_metadata(&named).map_err(|_| StorageError::ScopeChanged)?
                != stat
        {
            return Err(StorageError::ScopeChanged);
        }
        tx.check()?;
        Ok(crate::SessionSourceProof {
            source: reference.clone(),
            dev: stat.dev,
            ino: stat.ino,
            size: stat.size,
            modified_ns: stat.modified_ns,
            changed_ns: stat.changed_ns,
            freshness: crate::CatalogFreshness {
                state: "retained".into(),
            },
        })
    })
}

pub(crate) fn source_proof_observation(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    root: &str,
    absolute: &str,
) -> Result<Option<crate::storage::catalog::SourceProofObservation>, StorageError> {
    let generation = configured_generation(config, pool, control)?;
    snapshot::with_admitted_snapshot(pool, &config.namespace, control, &generation, |snapshot| {
        let mut tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
        let rows = tx.driver.query("SELECT r.session_key,CASE WHEN octet_length(r.metadata)<=1048576 THEN r.metadata END FROM semon_pg_native_catalog_sources s JOIN semon_pg_native_catalog_rows r USING(namespace_digest,namespace,generation,key_digest) WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.path_digest=sha256($3::bytea) AND s.source_path=$3 ORDER BY r.session_key LIMIT 1", &[&config.namespace.as_str().as_bytes(), &generation.as_bytes(), &absolute.as_bytes()]).map_err(super::sqlstate::error)?;
        let Some(found) = rows.first() else {
            return Ok(None);
        };
        let key = String::from_utf8(found.get(0)).map_err(|_| StorageError::ScopeChanged)?;
        let bytes = found
            .get::<_, Option<Vec<u8>>>(1)
            .ok_or(StorageError::Busy)?;
        crate::selected_read::charge(0, bytes.len()).map_err(|_| StorageError::Busy)?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if row.key != key || row.harness != root || row.sources.len() > 64 {
            return Err(StorageError::ScopeChanged);
        }
        let source = row
            .sources
            .iter()
            .find(|source| source.path.to_str() == Some(absolute))
            .ok_or(StorageError::ScopeChanged)?;
        let (_, ledger) =
            super::native_reader::ledger(&mut tx, absolute)?.ok_or(StorageError::ScopeChanged)?;
        if !crate::storage::catalog::source_matches_ledger(source, &ledger)
            || source.size != ledger.stat.size
            || source.modified_ns != ledger.stat.modified_ns
        {
            return Err(StorageError::ScopeChanged);
        }
        Ok(Some(crate::storage::catalog::SourceProofObservation {
            key,
            source: source.clone(),
            generation: generation.clone(),
        }))
    })
}
impl Navigation {
    fn read<T>(
        &self,
        read: impl FnOnce(&dyn NativeNavigationStore) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let generation = configured_generation(&self.config, &self.pool, &self.control)?;
        snapshot::with_admitted_snapshot(
            &self.pool,
            &self.config.namespace,
            &self.control,
            &generation,
            |snapshot| read(snapshot),
        )
    }
}
impl NativeNavigationStore for Navigation {
    fn catalog_generation(&self) -> Result<String, StorageError> {
        self.read(|snapshot| snapshot.catalog_generation())
    }
    fn bookmarked_target_keys(&self, target: &str) -> Result<Vec<String>, StorageError> {
        self.read(|snapshot| snapshot.bookmarked_target_keys(target))
    }
    fn native_resolution(
        &self,
        harness: &str,
        native_id: &str,
        scope: crate::CatalogReadScope,
        expected_generation: Option<&str>,
    ) -> Result<NativeResolutionObservation, StorageError> {
        self.read(|snapshot| {
            snapshot.native_resolution(harness, native_id, scope, expected_generation)
        })
    }
}

pub(crate) fn bookmark_identity(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    options: &crate::Options,
    source: &str,
    key: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, crate::CatalogIdentityError> {
    let generation =
        configured_generation(config, pool, control).map_err(crate::CatalogIdentityError::from)?;
    snapshot::with_admitted_generation(pool, &config.namespace, control, &generation, |snapshot| {
        Ok(crate::catalog::catalog_selection_in_snapshot(
            options,
            source,
            key,
            crate::CatalogReadScope::RetainedHistory,
            snapshot,
        )
        .map(|selected| selected.map(|(identity, _)| identity)))
    })
    .map_err(crate::CatalogIdentityError::from)?
}

/// Resolve ownership from one leased publication before asking the host for
/// bytes. Native IDs and canonical keys share the existing navigation index.
pub(crate) fn query_target(
    config: &StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    key: &str,
) -> Result<Option<String>, StorageError> {
    use crate::storage::navigation::NativeNavigationStore;
    if key.is_empty() || key.len() > 4096 {
        return Err(StorageError::InvalidArguments);
    }
    let generation = configured_generation(config, pool, control)?;
    snapshot::with_admitted_snapshot(pool, &config.namespace, control, &generation, |snapshot| {
        if snapshot.catalog(key, true)?.is_some() || snapshot.catalog(key, false)?.is_some() {
            return Ok(Some(key.to_owned()));
        }
        let mut targets = std::collections::BTreeSet::new();
        for harness in ["claude", "codex", "copilot"] {
            targets.extend(
                snapshot
                    .native_resolution(
                        harness,
                        key,
                        crate::CatalogReadScope::RetainedHistory,
                        Some(&generation),
                    )?
                    .keys,
            );
        }
        if targets.len() > 1 {
            return Err(StorageError::Conflict);
        }
        Ok(targets.into_iter().next())
    })
}
