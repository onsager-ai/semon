//! A retained recipe's consumed cohort and the publication epochs that fence
//! replacement. These tags are projection identity, never byte-read authority.
use super::{Namespace, PgProjectionTransaction, StorageError, backend};
use crate::model::summary::CatalogSource;
use std::collections::BTreeMap;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    pub(super) sources: Vec<CatalogSource>,
    epochs: BTreeMap<std::path::PathBuf, u64>,
}

impl Observation {
    pub(super) fn covers(&self, previous: &Self) -> bool {
        previous.sources.iter().all(|old| self.sources.iter().any(|source| {
            source.path == old.path
                && source.native_id == old.native_id
                && source.dev == old.dev
                && source.ino == old.ino
                && source.offset >= old.offset
                && self.epochs.get(&source.path).is_some_and(|epoch| previous.epochs.get(&old.path) == Some(epoch))
                && (source.offset != old.offset || (source.prefix_sha256 == old.prefix_sha256 && source.tail_sha256 == old.tail_sha256))
        }))
    }
}

impl PgProjectionTransaction<'_> {
    pub(super) async fn current_observation(
        &self,
        namespace: &Namespace,
        key: &str,
        sources: Vec<CatalogSource>,
    ) -> Result<Observation, StorageError> {
        let records = self.tx.query(
            "SELECT path,epoch FROM semon_projection.units WHERE namespace=$1 AND unit_key=$2 ORDER BY path",
            &[&namespace.binding(), &key],
        ).await.map_err(backend)?;
        let epochs = records.into_iter().map(|record| {
            let epoch: i64 = record.get(1);
            Ok((std::path::PathBuf::from(record.get::<_, String>(0)), u64::try_from(epoch).map_err(backend)?))
        }).collect::<Result<BTreeMap<_, _>, StorageError>>()?;
        if sources.iter().any(|source| !epochs.contains_key(&source.path)) {
            return Err(backend("outgoing source epoch unavailable"));
        }
        Ok(Observation { sources, epochs })
    }
}
