//! Real relationship publication effects for the shared native commit recipe.
//! This partial family borrows the caller's transaction. It does not write files,
//! advance a ledger, publish an ingest ACK or expose an IndexStore constructor.
use super::{sqlstate, txn::PgTxn};
use crate::{
    events::{
        FileIndex,
        native_commit::{self as shared, RelationshipGroup, Relationships},
    },
    storage::StorageError,
};

#[cfg(test)]
std::thread_local! {
    static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

struct Dependencies<'a, 'tx> {
    tx: &'a mut PgTxn<'tx>,
}

impl Dependencies<'_, '_> {
    fn after_effect(&self) -> Result<(), StorageError> {
        #[cfg(test)]
        if FAIL_AFTER.with(|flag| match flag.get() {
            Some(1) => {
                flag.set(None);
                true
            }
            Some(n) => {
                flag.set(Some(n - 1));
                false
            }
            None => false,
        }) {
            return Err(StorageError::Unavailable);
        }
        self.tx.check()
    }
}

impl Relationships for Dependencies<'_, '_> {
    type Error = StorageError;
    fn metadata_error(_: serde_json::Error) -> StorageError {
        StorageError::Unavailable
    }
    fn clear(&mut self, path: &str, group: RelationshipGroup) -> Result<(), StorageError> {
        self.tx.check()?;
        let query = match group {
            RelationshipGroup::Lineage => {
                "DELETE FROM semon_pg_native_native_dependencies WHERE namespace=$1 AND source_path=$2 AND (substring(harness from 1 for 8)=$3 OR substring(harness from 1 for 7)=$4)"
            }
            RelationshipGroup::Semantic => {
                "DELETE FROM semon_pg_native_native_dependencies WHERE namespace=$1 AND source_path=$2 AND harness IN ($3,$4)"
            }
        };
        let (first, second): (&[u8], &[u8]) = match group {
            RelationshipGroup::Lineage => (b"lineage:", b"bridge:"),
            RelationshipGroup::Semantic => (b"semantic-native:codex", b"semantic-parent:codex"),
        };
        self.tx
            .driver
            .execute(
                query,
                &[
                    &self.tx.namespace.as_str().as_bytes(),
                    &path.as_bytes(),
                    &first,
                    &second,
                ],
            )
            .map_err(sqlstate::error)?;
        self.after_effect()
    }
    fn put(&mut self, path: &str, harness: &str, id: &str) -> Result<(), StorageError> {
        self.tx.check()?;
        self.tx.driver.execute("INSERT INTO semon_pg_native_native_dependencies(namespace,source_path,harness,native_id) VALUES($1,$2,$3,$4) ON CONFLICT(namespace_digest,source_path,harness,native_id) DO NOTHING", &[&self.tx.namespace.as_str().as_bytes(), &path.as_bytes(), &harness.as_bytes(), &id.as_bytes()]).map_err(sqlstate::error)?;
        self.after_effect()
    }
    fn finished(&mut self, _: RelationshipGroup) -> Result<(), StorageError> {
        self.tx.check()
    }
}

// Narrow private partial family until the complete writer can consume it.
// The allow is disclosed; there is no successful partial IndexStore commit.
#[allow(dead_code)]
pub(super) fn relationships(
    tx: &mut PgTxn<'_>,
    path: &str,
    index: &FileIndex,
    semantic_change: bool,
) -> Result<(), StorageError> {
    tx.check()?;
    let namespace = tx.namespace.as_str().as_bytes();
    tx.driver.execute("INSERT INTO semon_pg_source_state(namespace) VALUES($1) ON CONFLICT(namespace_digest) DO NOTHING", &[&namespace]).map_err(sqlstate::error)?;
    let rows = tx.driver.query("SELECT namespace FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&namespace]).map_err(sqlstate::error)?;
    if rows.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let mut effects = Dependencies { tx };
    effects.after_effect()?;
    shared::relationships(&mut effects, path, index)?;
    if semantic_change {
        super::source_journal::invalidate(effects.tx, path)?;
        effects.after_effect()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
