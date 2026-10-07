//! Source-scoped demand for the existing coherent asynchronous producer.
use std::{io, sync::Arc};

use crate::{Options, Refresh, RefreshPool, viewer::MachineView};

/// Retain one observer per configured source, independently of browser clients.
/// Construction and demand do not read native history. The shared pool coalesces
/// checks; its existing producer still performs a complete selected-machine
/// observation. This is not a bounded incremental publication contract.
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

    /// Queue initial or recovery observation without waiting for a build.
    /// Errors describe a stalled/failing producer; retained reads remain usable.
    pub fn demand(&self) -> io::Result<()> {
        self.view.note_catalog_read()
    }

    /// Call after an authorized source push, including when no Viewer is open.
    /// Refreshes are coalesced using the existing spacing and revision rules.
    pub fn changed(&self) -> io::Result<()> {
        self.view.invalidate();
        self.view.note_catalog_read()
    }

    /// Stop and drain work before replacing options or removing source custody.
    /// This can wait for running work; async hosts call it outside their reactor.
    pub fn close(&self) {
        self.view.close();
    }
}
