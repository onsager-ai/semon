//! Metadata-only permalink normalization into the canonical native reader.
use crate::{Options, viewer::ViewerReply};

/// The host authorizes one configured source before locating a Catalog key.
pub fn session_bookmark(options: &Options, source: &str, query: &str) -> ViewerReply {
    crate::catalog::warehouse::http::locate(
        &crate::catalog::warehouse::host_source(options, source),
        query,
    )
}
