//! Source-scoped demand for the existing coherent asynchronous producer.
use std::{io, sync::Arc};

use crate::{Options, Refresh, RefreshPool, viewer::MachineView};

/// Retain one observer per configured source, independently of browser clients.
/// Construction and demand do not read native history. The shared pool coalesces
/// checks. Qualified publication drains persisted source revisions; cold ingestion
/// advances bounded discovery and complete-record checkpoints. Both paths yield
/// between steps and retain the existing source authority and publication fences.
pub struct SessionCatalogObserver {
    view: Arc<MachineView>,
}

impl SessionCatalogObserver {
    pub fn new(options: Options) -> Self {
        Self::with_pool(options, RefreshPool::shared())
    }

    /// Hosts may share their existing bounded refresh pool across observers.
    pub fn with_pool(options: Options, pool: Arc<RefreshPool>) -> Self {
        let view = MachineView::new(options);
        view.set_pool(pool);
        view.set_refresh(Refresh::OnInvalidate);
        Self { view }
    }

    pub(crate) fn shared_view(&self) -> Arc<MachineView> {
        self.view.clone()
    }

    /// Opaque producer correlation token for opt-in timing diagnostics. This
    /// identifies the configured cache within a process, never source authority.
    pub fn diagnostic_id(&self) -> String {
        crate::native_timing::id(&crate::events::EventCache::path(
            &self.view.evidence.options.cache,
        ))
    }

    /// Queue initial or recovery observation without waiting for a build.
    /// Errors describe a stalled/failing producer; retained reads remain usable.
    pub fn demand(&self) -> io::Result<()> {
        self.view.evidence.request_metadata_audit_if_idle();
        self.view.note_catalog_read()
    }

    /// Register a qualified persisted read's interest in future changes. This
    /// An absent in-memory owner starts no historical rebuild. Durable pending
    /// revisions or backfill checkpoints resume on the same bounded worker;
    /// existing producer failure or stall health remains observable.
    pub fn watch(&self) -> io::Result<()> {
        self.view.note_catalog_interest()
    }

    /// Call after an authorized source push, including when no Viewer is open.
    /// Refreshes are coalesced using the existing spacing and revision rules.
    pub fn changed(&self) -> io::Result<()> {
        self.view.invalidate();
        self.view.note_catalog_read()
    }

    /// Wake a producer after exact source/facts revisions are durable. Unknown
    /// writes use `changed`, which requests a conservative bounded metadata audit.
    pub fn changed_journaled(&self) -> io::Result<()> {
        self.view.invalidate_journaled();
        self.view.note_catalog_read()
    }

    /// Stop future checks immediately without waiting for running work. Hosts
    /// retire before scheduling a bounded off-reactor close/drain; source custody
    /// must remain valid until close completes.
    pub fn retire(&self) {
        self.view.retire();
    }

    /// Stop and drain work before replacing options or removing source custody.
    /// This can wait for running work; async hosts call it outside their reactor.
    pub fn close(&self) {
        self.view.close();
    }
}

/// Persist exact native source invalidations before and after an authorized Raw
/// mutation. Callers retain their source-custody/write fence through both calls.
/// Duplicate delivery is safe; publication acknowledges only the claimed revision.
pub fn invalidate_session_sources(options: &Options, inputs: &[crate::Input]) -> io::Result<()> {
    crate::events::store::enqueue_sources(options, inputs)
}
