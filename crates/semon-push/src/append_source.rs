//! Origin-owned complete-record append rendering. No production handle encoding.
use crate::{
    origin_read::{
        OpaqueId, PageRenderer, PinnedPolicy, RenderBudget, RenderPlan, RenderedPage,
        ResolvedSource, Target,
    },
    receiver_audit::Identifier,
};

type Result<T> = std::result::Result<T, crate::origin_read::Refused>;

/// Implementations use native tail custody and validate the entire admitted cohort.
/// Continuations are opaque export identities, never native byte offsets. Prepare
/// and render must describe the same complete admitted records; OriginRead checks
/// their exact mapping, length, continuation and live policy/source witnesses.
/// `prepare` returns `RenderPlan::append_continuity` for every grown member;
/// `render` must return identical `RenderedPage::append_continuity`. Hash each
/// [0, previous_boundary) from the CURRENT pinned source, honoring budget and
/// cancellation. This injected seam has no production hashing implementation.
pub trait AppendPageSource {
    fn prepare(
        &self,
        source: &ResolvedSource,
        cursor: &AppendCursor<'_>,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderPlan>;
    fn render(
        &self,
        source: &ResolvedSource,
        cursor: &AppendCursor<'_>,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderedPage>;
}
pub struct AppendCursor<'a> {
    /// Last acknowledged cohort. Evidence must be recomputed from the current
    /// pinned source, never copied from these previous witnesses.
    pub previous_cohort: Option<&'a [crate::body_cache::SourceWitness]>,
    pub lease_id: Identifier,
    pub page: &'a OpaqueId,
    pub after: Option<&'a OpaqueId>,
}
pub(crate) struct AppendRenderer<'a> {
    pub source: &'a dyn AppendPageSource,
    pub cursor: AppendCursor<'a>,
}
impl PageRenderer for AppendRenderer<'_> {
    fn prepare(
        &self,
        source: &ResolvedSource,
        _: &Target,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderPlan> {
        self.source
            .prepare(source, &self.cursor, policy, budget, cancelled)
    }
    fn render(
        &self,
        source: &ResolvedSource,
        _: &Target,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderedPage> {
        self.source
            .render(source, &self.cursor, policy, budget, cancelled)
    }
}
