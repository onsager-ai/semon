//! Real native publication head port. This explicit transitional writer is
//! unavailable on enrolled stores: Catalog-only qualification cannot authorize
//! the publisher's still-unqualified meta, ledger/stage and acknowledgement unit.
use super::{failed, namespace};
use crate::storage::{StorageError, catalog::CatalogPublicationHead};
use rusqlite::{OptionalExtension, Transaction, params};

pub(crate) const SLOT_SCHEMA: &str = "
CREATE TABLE session_slot_projections (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, generation TEXT NOT NULL,
 source_generation TEXT, version INTEGER NOT NULL, total INTEGER NOT NULL,
 PRIMARY KEY(namespace,session_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX session_slot_projection_version ON session_slot_projections(namespace,version,session_key);
CREATE TABLE session_slots (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, slot INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(namespace,session_key,slot)
) STRICT, WITHOUT ROWID;
CREATE INDEX session_slots_turn ON session_slots(namespace,session_key,CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.turn') END,slot);
";

pub(crate) struct CurrentWriter<'a> {
    db: &'a rusqlite::Connection,
    namespace: Option<&'a str>,
}
impl<'a> CurrentWriter<'a> {
    pub(crate) fn unbound(db: &'a rusqlite::Connection) -> Self {
        Self {
            db,
            namespace: None,
        }
    }
    pub(crate) fn admitted(
        db: &'a rusqlite::Connection,
        source: &'a super::semantic::AdmittedSource,
    ) -> Result<Self, StorageError> {
        source.validate_publication(db)?;
        Ok(Self {
            db,
            namespace: Some(source.namespace()),
        })
    }
}
impl crate::storage::catalog::CurrentPublicationStore for CurrentWriter<'_> {
    type Error = rusqlite::Error;
    fn current(
        &self,
        row: &crate::model::summary::CatalogRow,
        metadata: &str,
    ) -> rusqlite::Result<bool> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_catalog(namespace,session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) VALUES(?1,?2,'current',?3,?4,?5,?6,?7) ON CONFLICT(namespace,session_key) DO UPDATE SET lifecycle='current',last_ms=excluded.last_ms,harness=excluded.harness,repo=excluded.repo,parent_key=excluded.parent_key,metadata=excluded.metadata WHERE session_catalog.lifecycle != 'current' OR session_catalog.metadata != excluded.metadata OR session_catalog.parent_key IS NOT excluded.parent_key"
        } else {
            "INSERT INTO main.session_catalog(session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) VALUES(?2,'current',?3,?4,?5,?6,?7) ON CONFLICT(session_key) DO UPDATE SET lifecycle=excluded.lifecycle,last_ms=excluded.last_ms,harness=excluded.harness,repo=excluded.repo,parent_key=excluded.parent_key,metadata=excluded.metadata WHERE session_catalog.lifecycle != 'current' OR session_catalog.metadata != excluded.metadata OR session_catalog.parent_key IS NOT excluded.parent_key"
        };
        self.db
            .prepare_cached(sql)?
            .execute(params![
                self.namespace,
                row.key,
                row.last.unwrap_or(0),
                row.harness,
                row.repo,
                row.parent,
                metadata
            ])
            .map(|n| n > 0)
    }
    fn retire_sources(&self, key: &str) -> rusqlite::Result<()> {
        let sql = if self.namespace.is_some() {
            "UPDATE main.session_catalog_sources SET lifecycle='retained' WHERE namespace=?1 AND session_key=?2"
        } else {
            "UPDATE main.session_catalog_sources SET lifecycle='retained' WHERE session_key=?2"
        };
        self.db
            .prepare_cached(sql)?
            .execute(params![self.namespace, key])?;
        Ok(())
    }
    fn clear_dependencies(
        &self,
        source: &crate::model::summary::CatalogSource,
    ) -> rusqlite::Result<()> {
        let sql = if self.namespace.is_some() {
            "DELETE FROM main.catalog_native_dependencies WHERE namespace=?1 AND source_path=?2 AND harness NOT LIKE 'lineage:%' AND harness NOT LIKE 'bridge:%'"
        } else {
            "DELETE FROM main.catalog_native_dependencies WHERE source_path=?2 AND harness NOT LIKE 'lineage:%' AND harness NOT LIKE 'bridge:%'"
        };
        self.db
            .prepare_cached(sql)?
            .execute(params![self.namespace, source.path.to_string_lossy()])?;
        Ok(())
    }
    fn dependency(
        &self,
        source: &crate::model::summary::CatalogSource,
        harness: &str,
        native: &str,
    ) -> rusqlite::Result<()> {
        let sql = if self.namespace.is_some() {
            "INSERT OR IGNORE INTO main.catalog_native_dependencies(namespace,source_path,harness,native_id) VALUES(?1,?2,?3,?4)"
        } else {
            "INSERT OR IGNORE INTO main.catalog_native_dependencies(source_path,harness,native_id) VALUES(?2,?3,?4)"
        };
        self.db.prepare_cached(sql)?.execute(params![
            self.namespace,
            source.path.to_string_lossy(),
            harness,
            native
        ])?;
        Ok(())
    }
    fn source(
        &self,
        row: &crate::model::summary::CatalogRow,
        source: &crate::model::summary::CatalogSource,
    ) -> rusqlite::Result<()> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES(?1,?2,'current',?3,?4,?5) ON CONFLICT(namespace,session_key,source_path) DO UPDATE SET lifecycle='current',harness=excluded.harness,native_id=excluded.native_id"
        } else {
            "INSERT INTO main.session_catalog_sources(session_key,harness,native_id,source_path) VALUES(?2,?3,?4,?5) ON CONFLICT(session_key,source_path) DO UPDATE SET lifecycle='current',harness=excluded.harness,native_id=excluded.native_id"
        };
        self.db.prepare_cached(sql)?.execute(params![
            self.namespace,
            row.key,
            row.harness,
            source.native_id,
            source.path.to_string_lossy()
        ])?;
        Ok(())
    }
}

pub(crate) struct SlotWriter<'a> {
    db: &'a rusqlite::Connection,
    namespace: Option<&'a str>,
}
impl<'a> SlotWriter<'a> {
    pub(crate) fn unbound(db: &'a rusqlite::Connection) -> Self {
        Self {
            db,
            namespace: None,
        }
    }
    pub(crate) fn admitted(
        db: &'a rusqlite::Connection,
        source: &'a super::semantic::AdmittedSource,
    ) -> Result<Self, StorageError> {
        source.validate_publication(db)?;
        Ok(Self {
            db,
            namespace: Some(source.namespace()),
        })
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
    fn header(
        &self,
        h: &crate::storage::catalog::SlotPublicationHeader<'_>,
    ) -> Result<bool, Self::Error> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_slot_projections(namespace,session_key,generation,version,total,source_generation) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(namespace,session_key) DO UPDATE SET generation=excluded.generation,version=excluded.version,total=excluded.total,source_generation=excluded.source_generation WHERE session_slot_projections.generation != excluded.generation OR session_slot_projections.version != excluded.version OR session_slot_projections.source_generation IS NOT excluded.source_generation"
        } else {
            "INSERT INTO main.session_slot_projections(session_key,generation,version,total,source_generation) VALUES(?2,?3,?4,?5,?6) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,version=excluded.version,total=excluded.total,source_generation=excluded.source_generation WHERE session_slot_projections.generation != excluded.generation OR session_slot_projections.version != excluded.version OR session_slot_projections.source_generation IS NOT excluded.source_generation"
        };
        self.db
            .execute(
                sql,
                params![
                    self.namespace,
                    h.key,
                    h.generation,
                    crate::slot_projection::VERSION,
                    h.total,
                    h.source_generation
                ],
            )
            .map(|n| n > 0)
            .map_err(crate::events::store::failure)
    }
    fn replace(&self, key: &str, metadata: &[String]) -> Result<(), Self::Error> {
        let (remove, put) = if self.namespace.is_some() {
            (
                "DELETE FROM main.session_slots WHERE namespace=?1 AND session_key=?2",
                "INSERT INTO main.session_slots(namespace,session_key,slot,metadata) VALUES(?1,?2,?3,?4)",
            )
        } else {
            (
                "DELETE FROM main.session_slots WHERE session_key=?2",
                "INSERT INTO main.session_slots(session_key,slot,metadata) VALUES(?2,?3,?4)",
            )
        };
        self.db
            .execute(remove, params![self.namespace, key])
            .map_err(crate::events::store::failure)?;
        let mut put = self
            .db
            .prepare(put)
            .map_err(crate::events::store::failure)?;
        for (slot, metadata) in metadata.iter().enumerate() {
            put.execute(params![
                self.namespace,
                key,
                i64::try_from(slot).map_err(|_| Self::capacity())?,
                metadata
            ])
            .map_err(crate::events::store::failure)?;
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

const CATALOG_INSERT: &str = "INSERT INTO session_catalog(session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) VALUES ";
const CATALOG_CONFLICT: &str = " ON CONFLICT(session_key) DO UPDATE SET lifecycle='current', last_ms=excluded.last_ms, harness=excluded.harness, repo=excluded.repo, parent_key=excluded.parent_key, metadata=excluded.metadata WHERE session_catalog.lifecycle != 'current' OR session_catalog.metadata != excluded.metadata OR session_catalog.parent_key IS NOT excluded.parent_key";
pub(crate) fn put_catalog_batch(
    transaction: &Transaction<'_>,
    rows: &[crate::model::summary::CatalogRow],
    metadata: &[String],
) -> rusqlite::Result<Vec<bool>> {
    let values = (0..rows.len())
        .map(|position| {
            let first = position * 6 + 1;
            format!(
                "(?{first},'current',?{},?{},?{},?{},?{})",
                first + 1,
                first + 2,
                first + 3,
                first + 4,
                first + 5
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("{CATALOG_INSERT}{values}{CATALOG_CONFLICT} RETURNING session_key");
    let mut statement = transaction.prepare(&sql)?;
    for (position, (row, metadata)) in rows.iter().zip(metadata).enumerate() {
        let first = position * 6 + 1;
        statement.raw_bind_parameter(first, &row.key)?;
        statement.raw_bind_parameter(first + 1, row.last.unwrap_or(0))?;
        statement.raw_bind_parameter(first + 2, &row.harness)?;
        statement.raw_bind_parameter(first + 3, &row.repo)?;
        statement.raw_bind_parameter(first + 4, &row.parent)?;
        statement.raw_bind_parameter(first + 5, metadata)?;
    }
    let mut changed = vec![false; rows.len()];
    {
        let mut returned = statement.raw_query();
        while let Some(row) = returned.next()? {
            let rusqlite::types::ValueRef::Text(key) = row.get_ref(0)? else {
                return Err(rusqlite::Error::InvalidQuery);
            };
            let Some(position) = rows.iter().position(|row| row.key.as_bytes() == key) else {
                return Err(rusqlite::Error::InvalidQuery);
            };
            if std::mem::replace(&mut changed[position], true) {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
    }
    // Finalize RETURNING before source/search work or its after-DML sample.
    statement.finalize()?;
    Ok(changed)
}
