//! Private immutable Current/recipe/trace/search and retained-history effects.
//! This is NOT Catalog publication:
//! no head, claim ACK, readiness, reader lease or backend qualification is issued.
//! The complete admitted publisher must own the surrounding PgTxn and CAS.
#![allow(dead_code)]
use super::{sqlstate, txn::PgTxn};
use crate::{
    model::summary::{CatalogRow, CatalogSource},
    storage::{
        StorageError,
        catalog::{
            self, CurrentPublicationStore, SlotPublicationHeader, SlotPublicationStore,
            TracePublicationStore,
        },
    },
};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use tokio_postgres::types::ToSql;

pub(super) mod admission;
pub(super) mod publication;
pub(super) mod selected_reader;
pub(super) mod snapshot;
pub(super) mod withdrawal;

const MAX: usize = 1024 * 1024;

fn digest(value: &[u8]) -> Vec<u8> {
    Sha256::digest(value).to_vec()
}
fn encoded(value: &impl serde::Serialize) -> Result<Vec<u8>, StorageError> {
    let bytes = serde_json::to_vec(value).map_err(|_| StorageError::Unavailable)?;
    if bytes.len() > MAX {
        return Err(StorageError::Busy);
    }
    Ok(bytes)
}
fn path(source: &CatalogSource) -> Result<&str, StorageError> {
    source.path.to_str().ok_or(StorageError::InvalidArguments)
}

struct Effects<'a, 'tx> {
    tx: RefCell<&'a mut PgTxn<'tx>>,
    generation: &'a str,
}

#[cfg(test)]
thread_local! {
    static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static BOUNDARIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn boundary(tx: &PgTxn<'_>) -> Result<(), StorageError> {
    #[cfg(test)]
    {
        BOUNDARIES.with(|count| count.set(count.get() + 1));
        let fail = FAIL_AFTER.with(|point| match point.get() {
            Some(1) => {
                point.set(None);
                true
            }
            Some(n) => {
                point.set(Some(n - 1));
                false
            }
            None => false,
        });
        if fail {
            return Err(StorageError::Unavailable);
        }
    }
    tx.check()
}

impl Effects<'_, '_> {
    // Adapter-local fixed-table bind construction. Every immutable conflict
    // compares ALL columns, including the full opaque identity and namespace.
    // A digest collision or same-generation changed request therefore refuses.
    fn put(
        &self,
        table: &'static str,
        columns: &[&'static str],
        key: &'static str,
        values: &[&(dyn ToSql + Sync)],
    ) -> Result<(), StorageError> {
        let tx = self.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes().to_vec();
        let generation = self.generation.as_bytes();
        let mut args: Vec<&(dyn ToSql + Sync)> = vec![&ns, &generation];
        args.extend_from_slice(values);
        let mut names = vec!["namespace", "generation"];
        names.extend_from_slice(columns);
        let binds = (1..=args.len())
            .map(|n| format!("${n}"))
            .collect::<Vec<_>>();
        let equality = names
            .iter()
            .map(|name| format!("{table}.{name} IS NOT DISTINCT FROM excluded.{name}"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = format!(
            "INSERT INTO {table}({}) VALUES({}) ON CONFLICT(namespace_digest,generation,{key}) DO UPDATE SET generation=excluded.generation WHERE {equality}",
            names.join(","),
            binds.join(",")
        );
        if tx.driver.execute(&sql, &args).map_err(sqlstate::error)? != 1 {
            return Err(StorageError::ScopeChanged);
        }
        boundary(&tx)
    }
}

impl TracePublicationStore for Effects<'_, '_> {
    type Error = StorageError;
    fn invalid(_: std::io::Error) -> StorageError {
        StorageError::Unavailable
    }
    fn matches_header(
        &self,
        key: &str,
        generation: &str,
        source: &str,
    ) -> Result<bool, StorageError> {
        let tx = self.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes().to_vec();
        let rows = tx.driver.query("SELECT session_key=$4 AND trace_generation=$5 AND source_generation=$6 FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1) AND namespace=$1 AND generation=$2 AND key_digest=$3", &[&ns, &self.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes(), &generation.as_bytes(), &source.as_bytes()]).map_err(sqlstate::error)?;
        Ok(rows.first().is_some_and(|row| row.get(0)))
    }
    fn header(
        &self,
        key: &str,
        generation: &str,
        source: &str,
        metadata: &str,
    ) -> Result<bool, StorageError> {
        if metadata.len() > MAX {
            return Err(StorageError::Busy);
        }
        self.put(
            "semon_pg_native_catalog_trace_headers",
            &[
                "key_digest",
                "session_key",
                "trace_generation",
                "source_generation",
                "metadata",
            ],
            "key_digest",
            &[
                &digest(key.as_bytes()),
                &key.as_bytes(),
                &generation.as_bytes(),
                &source.as_bytes(),
                &metadata.as_bytes(),
            ],
        )?;
        Ok(true)
    }
    fn clear(&self, _key: &str) -> Result<(), StorageError> {
        // Immutable generations never delete an outgoing trace cohort.
        self.tx.borrow().check()
    }
    fn turn(
        &self,
        key: &str,
        id: Option<&str>,
        start: Option<&str>,
        metadata: &str,
    ) -> Result<(), StorageError> {
        let id = id.ok_or(StorageError::InvalidArguments)?;
        if metadata.len() > MAX {
            return Err(StorageError::Busy);
        }
        self.put(
            "semon_pg_native_catalog_trace_turns",
            &[
                "key_digest",
                "session_key",
                "turn_digest",
                "turn_id",
                "start_id",
                "metadata",
            ],
            "key_digest,turn_digest",
            &[
                &digest(key.as_bytes()),
                &key.as_bytes(),
                &digest(id.as_bytes()),
                &id.as_bytes(),
                &start.map(str::as_bytes),
                &metadata.as_bytes(),
            ],
        )
    }
    fn edge(&self, key: &str, id: Option<&str>, metadata: &str) -> Result<(), StorageError> {
        let id = id.ok_or(StorageError::InvalidArguments)?;
        if metadata.len() > MAX {
            return Err(StorageError::Busy);
        }
        self.put(
            "semon_pg_native_catalog_trace_edges",
            &[
                "key_digest",
                "session_key",
                "edge_digest",
                "edge_id",
                "metadata",
            ],
            "key_digest,edge_digest",
            &[
                &digest(key.as_bytes()),
                &key.as_bytes(),
                &digest(id.as_bytes()),
                &id.as_bytes(),
                &metadata.as_bytes(),
            ],
        )
    }
    fn bookmark(&self, key: &str, target: &str, edge: &str) -> Result<(), StorageError> {
        self.put(
            "semon_pg_native_catalog_trace_bookmarks",
            &[
                "key_digest",
                "session_key",
                "target_digest",
                "target",
                "edge_digest",
                "edge_id",
            ],
            "key_digest,target_digest,edge_digest",
            &[
                &digest(key.as_bytes()),
                &key.as_bytes(),
                &digest(target.as_bytes()),
                &target.as_bytes(),
                &digest(edge.as_bytes()),
                &edge.as_bytes(),
            ],
        )
    }
}

impl CurrentPublicationStore for Effects<'_, '_> {
    type Error = StorageError;
    fn current(&self, row: &CatalogRow, metadata: &str) -> Result<bool, StorageError> {
        if metadata.len() > MAX || row.sources.len() > 64 {
            return Err(StorageError::Busy);
        }
        self.put(
            "semon_pg_native_catalog_rows",
            &["key_digest", "session_key", "metadata"],
            "key_digest",
            &[
                &digest(row.key.as_bytes()),
                &row.key.as_bytes(),
                &metadata.as_bytes(),
            ],
        )?;
        Ok(true)
    }
    fn retire_sources(&self, _key: &str) -> Result<(), StorageError> {
        // A generation owns its exact source cohort; old generations remain
        // untouched. There are no mutable path/current rows to retire here.
        self.tx.borrow().check()
    }
    fn clear_dependencies(&self, _source: &CatalogSource) -> Result<(), StorageError> {
        // As for source mappings, dependencies are immutable per generation.
        self.tx.borrow().check()
    }
    fn dependency(
        &self,
        source: &CatalogSource,
        harness: &str,
        native: &str,
    ) -> Result<(), StorageError> {
        let path = path(source)?.as_bytes();
        self.put(
            "semon_pg_native_catalog_dependencies",
            &[
                "path_digest",
                "source_path",
                "harness_digest",
                "harness",
                "native_digest",
                "native_id",
            ],
            "path_digest,harness_digest,native_digest",
            &[
                &digest(path),
                &path,
                &digest(harness.as_bytes()),
                &harness.as_bytes(),
                &digest(native.as_bytes()),
                &native.as_bytes(),
            ],
        )
    }
    fn source(&self, row: &CatalogRow, source: &CatalogSource) -> Result<(), StorageError> {
        let path = path(source)?.as_bytes();
        self.put(
            "semon_pg_native_catalog_sources",
            &[
                "key_digest",
                "session_key",
                "path_digest",
                "source_path",
                "metadata",
            ],
            "key_digest,path_digest",
            &[
                &digest(row.key.as_bytes()),
                &row.key.as_bytes(),
                &digest(path),
                &path,
                &encoded(source)?,
            ],
        )
    }
}

impl SlotPublicationStore for Effects<'_, '_> {
    type Error = StorageError;
    fn data(_: serde_json::Error) -> StorageError {
        StorageError::Unavailable
    }
    fn capacity() -> StorageError {
        StorageError::Busy
    }
    fn header(&self, header: &SlotPublicationHeader<'_>) -> Result<bool, StorageError> {
        let version = i64::from(crate::slot_projection::VERSION);
        self.put(
            "semon_pg_native_catalog_recipe_headers",
            &[
                "key_digest",
                "session_key",
                "recipe_generation",
                "source_generation",
                "version",
                "total",
            ],
            "key_digest",
            &[
                &digest(header.key.as_bytes()),
                &header.key.as_bytes(),
                &header.generation.as_bytes(),
                &header.source_generation.as_bytes(),
                &version,
                &header.total,
            ],
        )?;
        // Equal immutable headers still verify/fill all exact recipe rows.
        Ok(true)
    }
    fn replace(&self, key: &str, metadata: &[String]) -> Result<(), StorageError> {
        for (slot, text) in metadata.iter().enumerate() {
            if text.len() > MAX {
                return Err(StorageError::Busy);
            }
            let slot = i64::try_from(slot).map_err(|_| StorageError::Busy)?;
            self.put(
                "semon_pg_native_catalog_recipe_rows",
                &["key_digest", "session_key", "slot", "metadata"],
                "key_digest,slot",
                &[
                    &digest(key.as_bytes()),
                    &key.as_bytes(),
                    &slot,
                    &text.as_bytes(),
                ],
            )?;
        }
        Ok(())
    }
}

/// Execute real shared Current/source/dependency and recipe algorithms in the
/// caller's borrowed transaction. Returning () deliberately cannot become a
/// Catalog receipt: full History/overview/proof/admission qualification is missing.
pub(super) fn stage(
    tx: &mut PgTxn<'_>,
    generation: &str,
    publication: &crate::slot_projection::Publication<'_>,
) -> Result<(), StorageError> {
    tx.check()?;
    if generation.len() != 64 || !generation.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(StorageError::InvalidArguments);
    }
    if publication.rows.len() > 64 {
        return Err(StorageError::Busy);
    }
    let effects = Effects {
        tx: RefCell::new(tx),
        generation,
    };
    for row in publication.rows {
        let metadata = encoded(row)?;
        let metadata = std::str::from_utf8(&metadata).map_err(|_| StorageError::Unavailable)?;
        effects.current(row, metadata)?;
        catalog::publish_current_sources(&effects, row)?;
        if let Some(trace) = &row.trace {
            crate::causal_trace::publish_with_store(&effects, row, trace)?;
        }
    }
    if let Some(projections) = publication.transcripts {
        catalog::publish_slot_rows(&effects, publication.rows, projections)?;
    }
    crate::catalog_search::publish_current_rows_port(
        &effects,
        &publication.rows.iter().collect::<Vec<_>>(),
        64,
        |_, _| {},
    )?;
    Ok(())
}

pub(super) mod history;
mod search;

#[cfg(test)]
mod tests;
