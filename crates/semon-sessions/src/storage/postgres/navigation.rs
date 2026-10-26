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
