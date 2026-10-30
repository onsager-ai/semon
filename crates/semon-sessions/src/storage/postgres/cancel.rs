use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

#[derive(Clone, Default)]
pub struct PgCancellation {
    host: Arc<AtomicBool>,
    request: Option<(Arc<AtomicBool>, Instant)>,
}
impl PgCancellation {
    pub fn cancel(&self) {
        self.host.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.host.load(Ordering::SeqCst)
            || self
                .request
                .as_ref()
                .is_some_and(|(cancelled, _)| cancelled.load(Ordering::Acquire))
    }
    /// Bind only at a read call, never in a retained checkpoint. Nested scopes
    /// already resolve to their outer request in selected_read.
    pub(crate) fn for_selected_read(&self) -> Self {
        Self {
            host: self.host.clone(),
            request: crate::selected_read::io_control(),
        }
    }
    pub(super) fn deadline(&self, pool_deadline: Instant) -> Instant {
        self.request
            .as_ref()
            .map_or(pool_deadline, |(_, request)| pool_deadline.min(*request))
    }
    pub(super) fn bounded(&self, deadline: Instant) -> Self {
        let (cancelled, existing) = self
            .request
            .clone()
            .unwrap_or_else(|| (Arc::new(AtomicBool::new(false)), deadline));
        Self {
            host: self.host.clone(),
            request: Some((cancelled, existing.min(deadline))),
        }
    }
    pub(super) async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}
