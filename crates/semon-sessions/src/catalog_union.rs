//! Merge bounded Catalog pages from authorized workspace sources.
use crate::{OverviewSource, ViewerReply};

/// The host supplies authorized source options and a workspace identity. One
/// source database is open at a time; retained memory is at most two pages.
/// Cold sources contribute pending coverage. A changed source set or publication
/// resets pagination, matching the focused Catalog's live-write convention.
pub fn session_catalog_union(
    sources: &[OverviewSource],
    workspace: &str,
    query: &str,
) -> ViewerReply {
    super::warehouse::http::list(sources, workspace, query, true)
}
