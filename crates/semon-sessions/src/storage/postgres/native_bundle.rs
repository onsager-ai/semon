//! Connected private file-family increment. This never issues an ingest ACK.
//! Chunk-proof generations, launch calls and relocation qualification remain
//! prerequisites of the complete file commit; public qualification stays zero.
use super::{native_core, native_usage::Usage, sqlstate, txn::PgTxn};
use crate::{
    events::{
        DirtyRows, native_aux,
        native_commit::{self as shared, Effects, Input},
        store::native_usage::algorithm,
    },
    storage::StorageError,
};

/// Deliberately cannot be converted into an EventReceipt or operation ACK.
#[derive(Debug)]
#[allow(dead_code)] // Private coverage disclosure, never an ingest receipt.
pub(super) struct PartialFile {
    pub(super) missing: &'static [&'static str],
}

struct Writer<'a, 'tx> {
    tx: &'a mut PgTxn<'tx>,
    source: i64,
    pages: Option<super::native_pages::Prepared>,
    pending: Option<super::native_pages::Pending>,
}

#[cfg(test)]
thread_local! { static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
static BOUNDARIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

pub(super) fn page_boundary(tx: &PgTxn<'_>) -> Result<(), StorageError> {
    #[cfg(test)]
    BOUNDARIES.with(|count| count.set(count.get() + 1));
    #[cfg(test)]
    if FAIL_AFTER.with(|point| match point.get() {
        Some(1) => {
            point.set(None);
            true
        }
        Some(n) => {
            point.set(Some(n - 1));
            false
        }
        None => false,
    }) {
        return Err(StorageError::Unavailable);
    }
    tx.check()
}

impl Writer<'_, '_> {
    fn boundary(&self) -> Result<(), StorageError> {
        page_boundary(self.tx)
    }

    fn metadata(&mut self, key: &str, value: Option<String>) -> Result<(), StorageError> {
        self.tx.check()?;
        let ns = self.tx.namespace.as_str().as_bytes();
        if let Some(value) = value {
            self.tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,key) DO UPDATE SET value=excluded.value", &[&ns,&key.as_bytes(),&value.as_bytes()]).map_err(sqlstate::error)?;
        } else {
            self.tx
                .driver
                .execute(
                    "DELETE FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2",
                    &[&ns, &key.as_bytes()],
                )
                .map_err(sqlstate::error)?;
        }
        self.boundary()
    }
}

impl Effects for Writer<'_, '_> {
    type Error = StorageError;
    fn planning_started(&mut self) {}
    fn planning_finished(&mut self) {}
    fn begin(&mut self, input: &Input<'_>) -> Result<(), StorageError> {
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE begin");
        // Existing transaction admission owns the store-shared lock before
        // SourceState. Reject the exact core CAS before any family effects.
        native_core::qualify(self.tx, input.path, input.expected)?;
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE relocation");
        let pending = self.tx.driver.query("SELECT old_path FROM semon_pg_native_relocation_candidates WHERE namespace=$1 AND new_path=$2 LIMIT 65", &[&self.tx.namespace.as_str().as_bytes(),&input.path.as_bytes()]).map_err(sqlstate::error)?;
        if pending.len() > 64 { return Err(StorageError::ScopeChanged); }
        // Qualification/reuse still belongs to the unported Raw+Catalog
        // producer. Only advance or retire its existing evidence here, using
        // the same algorithm as SQLite, within this borrowed transaction.
        for row in pending {
            let old = String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?;
            let key = format!("native_catalog_relocation:{old}");
            let value = self.tx.driver.query_opt("SELECT CASE WHEN octet_length(value)<=16384 THEN value END FROM semon_pg_native_meta WHERE namespace=$1 AND key=$2", &[&self.tx.namespace.as_str().as_bytes(),&key.as_bytes()]).map_err(sqlstate::error)?;
            let encoded = value.and_then(|row| row.get::<_,Option<Vec<u8>>>(0)).map(String::from_utf8).transpose().map_err(|_| StorageError::ScopeChanged)?;
            let advanced = shared::advance_relocation(encoded.as_deref(), input.expected, input.ledger, input.changes.is_some()).map_err(|_| StorageError::Unavailable)?;
            self.metadata(&key, advanced)?;
        }
        let mut usage = Usage::new(self.tx)?;
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE usage_current");
        use algorithm::Effects as _;
        let current = usage.current(input.path)?;
        match (input.expected, current) {
            (None, None) => {}
            (Some(expected), Some((_, receipt)))
                if receipt == crate::events::store::header_ledger(expected) => {}
            _ => return Err(StorageError::ScopeChanged),
        }
        self.source = algorithm::begin(
            &mut usage,
            input.path,
            input.expected,
            input.ledger,
            input.changes.is_some(),
        )?;
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE begin_complete");
        self.boundary()
    }
    fn file(&mut self, input: &Input<'_>, changes: &DirtyRows) -> Result<(), StorageError> {
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE core");
        native_core::write(
            self.tx,
            input.path,
            input.expected,
            input.ledger,
            input.index,
            input.changes.map(|_| changes),
        )?;
        self.boundary()?;
        Ok(())
    }
    fn auxiliary(&mut self, input: &Input<'_>, stage: shared::Auxiliary) -> Result<(), StorageError> {
        use shared::Auxiliary;
        match stage {
            Auxiliary::Header => self.metadata(
                &format!("native_codex_header:{}", input.path),
                native_aux::codex_header(input.ledger, input.index.codex_header.as_ref()).map_err(|_| StorageError::Unavailable)?,
            ),
            Auxiliary::Sidecar => {
                if input.index.claude_sidecar.is_some() || crate::native_sidecar::agent_path(std::path::Path::new(input.path)) {
                    self.metadata(&format!("native_claude_sidecar:{}", input.path), native_aux::claude_sidecar(input.ledger, input.index.claude_sidecar.as_ref()).map_err(|_| StorageError::Unavailable)?)?;
                }
                Ok(())
            }
            Auxiliary::Chunks => {
                let pages = self.pages.take().ok_or(StorageError::ScopeChanged)?;
                self.pending = Some(super::native_pages::write_chunks(self.tx, self.source, input.path, input.ledger, pages)?);
                Ok(())
            }
            Auxiliary::Parent => self.metadata(
                &format!("native_parent_marker:{}", input.path),
                native_aux::parent_marker(input.ledger, input.index.marker_index.as_ref()).map_err(|_| StorageError::Unavailable)?,
            ),
            Auxiliary::Launch => {
                let pending = self.pending.take().ok_or(StorageError::ScopeChanged)?;
                super::native_pages::write_launch(self.tx, pending)?;
                Ok(())
            }
        }
    }
    fn usage(&mut self, input: &Input<'_>) -> Result<(), StorageError> {
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE usage_write");
        algorithm::write(
            &mut Usage::new(self.tx)?,
            input.path,
            input.ledger,
            input.index,
        )?;
        self.boundary()?;
        Ok(())
    }
    fn tree(&mut self, input: &Input<'_>) -> Result<(), StorageError> {
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE tree_publish");
        if let Some(summary) = &input.index.tree_summary
            // Tree reads persisted Usage and current store pricing authority.
            && !super::tree_writer::publish(
                self.tx,
                input.path,
                input.ledger,
                input.expected,
                self.source,
                summary,
                input.index.native_usage_observations.is_some(),
            )?
        {
            return Err(StorageError::ScopeChanged);
        }
        self.boundary()
    }
    fn relationships(&mut self, input: &Input<'_>) -> Result<(), StorageError> {
        #[cfg(test)]
        eprintln!("BUNDLE_STAGE relationships");
        super::native_commit::relationships(self.tx, input.path, input.index, false)?;
        self.boundary()
    }
    fn archive(&mut self, path: &str) -> Result<(), StorageError> {
        self.metadata(&format!("archive_event_index:{path}"), None)
    }
    fn invalidate(&mut self, path: &str, changed: bool) -> Result<(), StorageError> {
        if changed {
            super::source_journal::invalidate(self.tx, path)?;
        }
        self.boundary()
    }
    fn retire_mutation(&mut self, path: &str) -> Result<(), StorageError> {
        self.tx.driver.execute("DELETE FROM semon_pg_native_source_mutations WHERE namespace=$1 AND source_path=$2", &[&self.tx.namespace.as_str().as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?;
        self.boundary()
    }
}

/// Runs real connected effects in the caller's transaction, without recording
/// a durable ingest receipt. Caller must propagate errors to roll back all
/// families. This partial result must never acknowledge Raw or native ingest.
#[allow(dead_code)] // Complete IndexStore remains closed pending the named families.
pub(super) fn stage(tx: &mut PgTxn<'_>, input: Input<'_>) -> Result<PartialFile, StorageError> {
    if input.index.tree_summary.is_none() && input.index.copilot.is_none() {
        return Err(StorageError::NamespaceUpgradePending);
    }
    if input.index.tree_summary.is_none() {
        // A genuine Copilot source has no Tree. Until Tree retirement is
        // connected, refuse a path that still owns another native Tree.
        let existing: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_tree_headers WHERE namespace=$1 AND source_path=$2)", &[&tx.namespace.as_str().as_bytes(), &input.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
        if existing {
            return Err(StorageError::NamespaceUpgradePending);
        }
    }
    let pages = super::native_pages::prepare(input.ledger, input.index)?;
    shared::write(
        &mut Writer {
            tx,
            source: 0,
            pages: Some(pages),
            pending: None,
        },
        input,
    )?;
    Ok(PartialFile {
        missing: &[
            "Catalog proof references, reader leases and GC",
            "selected proof/root qualification and worker ownership",
            "relocation qualification",
            "complete durable ingest receipt and Raw qualification",
        ],
    })
}

#[cfg(test)]
mod tests;
