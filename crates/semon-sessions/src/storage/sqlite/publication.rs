//! Real native publication head port. This explicit transitional writer is
//! unavailable on enrolled stores: Catalog-only qualification cannot authorize
//! the publisher's still-unqualified meta, ledger/stage and acknowledgement unit.
use super::{failed, namespace};
use crate::storage::{StorageError, catalog::CatalogPublicationHead};
use rusqlite::{OptionalExtension, Transaction, params};

pub(crate) struct SlotWriter<'a> {
    db: &'a rusqlite::Connection,
    namespace: Option<&'a str>,
}
impl<'a> SlotWriter<'a> {
    pub(crate) fn unbound(db: &'a rusqlite::Connection) -> Self {
        Self { db, namespace: None }
    }
    pub(super) fn admitted(db: &'a rusqlite::Connection, source: &'a super::semantic::AdmittedSource) -> Result<Self, StorageError> {
        source.validate_publication(db)?;
        Ok(Self { db, namespace: Some(source.namespace()) })
    }
}
impl crate::storage::catalog::SlotPublicationStore for SlotWriter<'_> {
    type Error = crate::events::StoreError;
    fn data(error: serde_json::Error) -> Self::Error {
        crate::events::StoreError::Data(error.to_string())
    }
    fn capacity() -> Self::Error {
        crate::events::StoreError::Data("slot count exceeds SQL range".into())
    }
    fn header(&self, h: &crate::storage::catalog::SlotPublicationHeader<'_>) -> Result<bool, Self::Error> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_slot_projections(namespace,session_key,generation,version,total,source_generation) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(namespace,session_key) DO UPDATE SET generation=excluded.generation,version=excluded.version,total=excluded.total,source_generation=excluded.source_generation WHERE session_slot_projections.generation != excluded.generation OR session_slot_projections.version != excluded.version OR session_slot_projections.source_generation IS NOT excluded.source_generation"
        } else {
            "INSERT INTO main.session_slot_projections(session_key,generation,version,total,source_generation) VALUES(?2,?3,?4,?5,?6) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,version=excluded.version,total=excluded.total,source_generation=excluded.source_generation WHERE session_slot_projections.generation != excluded.generation OR session_slot_projections.version != excluded.version OR session_slot_projections.source_generation IS NOT excluded.source_generation"
        };
        self.db.execute(sql,params![self.namespace,h.key,h.generation,crate::slot_projection::VERSION,h.total,h.source_generation]).map(|n|n>0).map_err(crate::events::store::failure)
    }
    fn replace(&self, key: &str, metadata: &[String]) -> Result<(), Self::Error> {
        let (remove, put) = if self.namespace.is_some() {
            ("DELETE FROM main.session_slots WHERE namespace=?1 AND session_key=?2", "INSERT INTO main.session_slots(namespace,session_key,slot,metadata) VALUES(?1,?2,?3,?4)")
        } else {
            ("DELETE FROM main.session_slots WHERE session_key=?2", "INSERT INTO main.session_slots(session_key,slot,metadata) VALUES(?2,?3,?4)")
        };
        self.db.execute(remove,params![self.namespace,key]).map_err(crate::events::store::failure)?;
        let mut put = self.db.prepare(put).map_err(crate::events::store::failure)?;
        for (slot, metadata) in metadata.iter().enumerate() {
            put.execute(params![self.namespace,key,i64::try_from(slot).map_err(|_| Self::capacity())?,metadata]).map_err(crate::events::store::failure)?;
        }
        Ok(())
    }
}

pub(crate) struct UnboundPublicationHead<'a, 'connection>(&'a Transaction<'connection>);

pub(crate) struct BoundPublicationHead<'a, 'connection> {
    tx: &'a Transaction<'connection>,
    source: &'a super::semantic::AdmittedSource,
}
pub(crate) fn admitted_head<'a, 'connection>(
    tx: &'a Transaction<'connection>,
    source: &'a super::semantic::AdmittedSource,
) -> Result<BoundPublicationHead<'a, 'connection>, StorageError> {
    source.validate_publication(tx)?;
    Ok(BoundPublicationHead { tx, source })
}
impl CatalogPublicationHead for BoundPublicationHead<'_, '_> {
    fn base_generation(&self) -> Result<Option<String>, StorageError> {
        self.tx
            .query_row(
                "SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_generation'",
                [self.source.namespace()],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)
    }
    fn publish_generation(&self, generation: &str, version: &str) -> Result<(), StorageError> {
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
