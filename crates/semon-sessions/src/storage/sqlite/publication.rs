//! Real native publication head port. This explicit transitional writer is
//! unavailable on enrolled stores: Catalog-only qualification cannot authorize
//! the publisher's still-unqualified meta, ledger/stage and acknowledgement unit.
use super::{failed, namespace};
use crate::storage::{StorageError, catalog::CatalogPublicationHead};
use rusqlite::{OptionalExtension, Transaction, params};

pub(crate) struct UnboundPublicationHead<'a, 'connection>(&'a Transaction<'connection>);

pub(crate) struct BoundPublicationHead<'a,'connection> {
    tx:&'a Transaction<'connection>,
    source:&'a super::semantic::AdmittedSource,
}
pub(crate) fn admitted_head<'a,'connection>(tx:&'a Transaction<'connection>,source:&'a super::semantic::AdmittedSource) -> Result<BoundPublicationHead<'a,'connection>,StorageError> {
    source.validate_publication(tx)?;
    Ok(BoundPublicationHead { tx,source })
}
impl CatalogPublicationHead for BoundPublicationHead<'_, '_> {
    fn base_generation(&self) -> Result<Option<String>,StorageError> {
        self.tx.query_row("SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_generation'",[self.source.namespace()],|r|r.get(0)).optional().map_err(failed)
    }
    fn publish_generation(&self,generation:&str,version:&str) -> Result<(),StorageError> {
        self.source.validate_publication(self.tx)?;
        self.tx.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,'catalog_generation',?2),(?1,'catalog_version',?3) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![self.source.namespace(),generation,version]).map_err(failed)?;
        Ok(())
    }
}

pub(crate) fn transitional_head<'a, 'connection>(
    transaction: &'a Transaction<'connection>,
) -> Result<UnboundPublicationHead<'a, 'connection>, StorageError> {
    if namespace::is_bound(transaction).map_err(failed)? {
        return Err(StorageError::NamespaceUpgradePending);
    }
    Ok(UnboundPublicationHead(transaction))
}

impl CatalogPublicationHead for UnboundPublicationHead<'_, '_> {
    fn base_generation(&self) -> Result<Option<String>, StorageError> {
        self.0
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)
    }

    fn publish_generation(&self, generation: &str, version: &str) -> Result<(), StorageError> {
        self.0.execute("INSERT OR REPLACE INTO meta(key,value) VALUES ('catalog_generation',?1),('catalog_version',?2)", params![generation, version]).map_err(failed)?;
        Ok(())
    }
}
