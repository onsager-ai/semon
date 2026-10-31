//! Inactive selected Query payload reads over the already admitted snapshot.
use super::*;
use crate::storage::{
    catalog::CatalogObservationOwner,
    query::{
        QueryRecipeLoadError as Error, QueryRecipeObservation, QueryRecipeOwner,
        QueryRecipeResidence,
    },
};

fn authority(error: std::io::Error) -> Error {
    match error
        .get_ref()
        .and_then(|error| error.downcast_ref::<StorageError>())
    {
        Some(
            StorageError::ScopeChanged
            | StorageError::NamespaceUpgradePending
            | StorageError::NamespaceUnadmitted,
        ) => Error::ObservationChanged,
        Some(error) => Error::Storage(*error),
        None => Error::ObservationChanged,
    }
}

impl Snapshot<'_, '_> {
    /// `owner` is supplied by the physical selection, independently of display
    /// lifecycle. Authentication reuses the native receipt or imported proof.
    pub(in crate::storage::postgres) fn query_recipe_observation(
        &self,
        owner: CatalogObservationOwner,
        selected: &CatalogRow,
    ) -> Result<QueryRecipeObservation, Error> {
        let (original, _, origin) = self
            .row_observation(selected, Some(owner))
            .map_err(authority)?
            .ok_or(Error::ObservationChanged)?;
        let history = owner == CatalogObservationOwner::History;
        let generation = if history {
            self.owner(&selected.key)?
                .ok_or(Error::ObservationChanged)?
                .generation
        } else {
            self.generation.clone()
        };
        let table = if history {
            "semon_pg_native_catalog_history_headers"
        } else {
            "semon_pg_native_catalog_rows"
        };
        let mut tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let destination = super::super::super::model_cache::binding(&mut tx)?;
        let ns = tx.namespace.as_str().as_bytes();
        let row = tx.driver.query(&format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3"), &[&ns, &generation.as_bytes(), &selected.key.as_bytes()]).map_err(sqlstate::error)?.into_iter().next().ok_or(Error::ObservationChanged)?;
        let metadata = text(
            row.get::<_, Option<Vec<u8>>>(0)
                .ok_or(StorageError::HistoryRetentionBounds)?,
        )?;
        let residence = match origin {
            super::super::observations::Origin::Native(_) => {
                let metadata_digest = crate::storage::proof_codec::digest(metadata.as_bytes());
                if history {
                    QueryRecipeResidence::NativeHistory { metadata_digest }
                } else {
                    QueryRecipeResidence::NativeCatalog { metadata_digest }
                }
            }
            super::super::observations::Origin::Imported(id) => {
                let record = tx.driver.query("SELECT publication FROM semon_pg_native_selected_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND id=$2 AND key_digest=sha256($3::bytea) AND session_key=$3", &[&ns, &id.0.as_bytes(), &selected.key.as_bytes()]).map_err(sqlstate::error)?.into_iter().next().ok_or(Error::ObservationChanged)?;
                QueryRecipeResidence::Immutable {
                    owner: if history {
                        QueryRecipeOwner::History
                    } else {
                        QueryRecipeOwner::Catalog
                    },
                    generation: id,
                    publication: text(record.get(0))?,
                }
            }
        };
        tx.check()?;
        QueryRecipeObservation::qualified(residence, metadata, original, destination)
            .map_err(Error::from)
    }

    pub(in crate::storage::postgres) fn load_query_recipe(
        &self,
        observation: &QueryRecipeObservation,
    ) -> Result<Option<crate::query_projection::SelectedRecipe>, Error> {
        let owner = match &observation.residence {
            QueryRecipeResidence::NativeCatalog { .. }
            | QueryRecipeResidence::Immutable {
                owner: QueryRecipeOwner::Catalog,
                ..
            } => CatalogObservationOwner::Catalog,
            _ => CatalogObservationOwner::History,
        };
        // This rechecks exact physical metadata, grants, namespace, admission,
        // stream, original source prefix and imported proof in this same lease.
        let fresh = self.query_recipe_observation(owner, observation.row())?;
        let same_residence = match (&fresh.residence, &observation.residence) {
            (
                QueryRecipeResidence::NativeCatalog { metadata_digest: a },
                QueryRecipeResidence::NativeCatalog { metadata_digest: b },
            )
            | (
                QueryRecipeResidence::NativeHistory { metadata_digest: a },
                QueryRecipeResidence::NativeHistory { metadata_digest: b },
            ) => a == b,
            (
                QueryRecipeResidence::Immutable {
                    owner: a,
                    generation: ag,
                    publication: ap,
                },
                QueryRecipeResidence::Immutable {
                    owner: b,
                    generation: bg,
                    publication: bp,
                },
            ) => std::mem::discriminant(a) == std::mem::discriminant(b) && ag == bg && ap == bp,
            _ => false,
        };
        if !same_residence
            || fresh.metadata != observation.metadata
            || fresh.original != observation.original
            || fresh.destination != observation.destination
        {
            return Err(Error::ObservationChanged);
        }
        let (table, generation, immutable) = match &observation.residence {
            QueryRecipeResidence::NativeCatalog { .. } => (
                "semon_pg_native_catalog_rows",
                self.generation.clone(),
                false,
            ),
            QueryRecipeResidence::NativeHistory { .. } => (
                "semon_pg_native_catalog_history_headers",
                self.owner(&observation.row().key)?
                    .ok_or(Error::ObservationChanged)?
                    .generation,
                false,
            ),
            QueryRecipeResidence::Immutable { generation, .. } => (
                "semon_pg_native_selected_generations",
                generation.0.clone(),
                true,
            ),
        };
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let column = if immutable { "id" } else { "generation" };
        let cells = tx.driver.query(&format!("SELECT octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576,CASE WHEN octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576 THEN metadata END,CASE WHEN octet_length(metadata)+coalesce(octet_length(query_recipe),0)<=1048576 THEN query_recipe END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND {column}=$2 AND key_digest=sha256($3::bytea) AND session_key=$3"), &[&tx.namespace.as_str().as_bytes(), &generation.as_bytes(), &observation.row().key.as_bytes()]).map_err(sqlstate::error)?.into_iter().next().ok_or(Error::ObservationChanged)?;
        if !cells.get::<_, bool>(0) {
            return Err(StorageError::HistoryRetentionBounds.into());
        }
        let metadata = cells
            .get::<_, Option<Vec<u8>>>(1)
            .ok_or(Error::CorruptPayload)?;
        let payload = cells.get::<_, Option<Vec<u8>>>(2);
        crate::selected_read::charge(64, metadata.len() + payload.as_ref().map_or(0, Vec::len))
            .map_err(|_| StorageError::Busy)?;
        if !immutable && metadata != observation.metadata.as_bytes() {
            return Err(Error::ObservationChanged);
        }
        tx.check()?;
        if observation.row().query.is_none() {
            return if payload.is_none() {
                Ok(None)
            } else {
                Err(Error::CorruptPayload)
            };
        }
        let payload = payload.ok_or(Error::MissingPayload)?;
        let payload = std::str::from_utf8(&payload).map_err(|_| Error::CorruptPayload)?;
        observation
            .decode(payload)
            .map(Some)
            .map_err(|_| Error::CorruptPayload)
    }
}
