//! Indexed native-to-catalog navigation. This grants no native control authority.
use crate::{CatalogIdentityError, CatalogReadScope, Options, viewer::ViewerReply};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogNativeResolutionState {
    Resolved,
    Provisional,
    Pending,
    Ambiguous,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogNativeResolution {
    pub state: CatalogNativeResolutionState,
    pub source_key: String,
    pub harness: String,
    pub native_id: String,
    pub read_scope: CatalogReadScope,
    pub catalog_key: Option<String>,
    pub generation: Option<String>,
}

/// The host must authorize `source_key` and supply precisely its configured
/// options. A resolved read alias is not evidence of runtime/control readiness.
pub fn session_catalog_resolve_native(
    options: &Options,
    source_key: &str,
    harness: &str,
    native_id: &str,
    scope: CatalogReadScope,
) -> Result<CatalogNativeResolution, CatalogIdentityError> {
    let store = crate::storage::runtime::RuntimeStore::for_source(options, source_key);
    if store.is_postgres() {
        return store.resolve_native(source_key, harness, native_id, scope);
    }
    crate::catalog::warehouse::host_resolve_native(options, source_key, harness, native_id, scope)
}

pub(crate) fn valid_resolution_arguments(source_key: &str, harness: &str, native_id: &str) -> bool {
    !source_key.is_empty()
        && source_key.len() <= 4096
        && !native_id.is_empty()
        && native_id.len() <= 4096
        && !native_id.chars().any(char::is_control)
        && matches!(harness, "claude" | "codex" | "copilot")
}

pub(crate) fn page_with_reader(
    key: &str,
    query: &str,
    before_read: impl FnOnce() -> Result<(), ViewerReply>,
    read: impl FnOnce(
        &str,
        &str,
        CatalogReadScope,
    ) -> Result<CatalogNativeResolution, CatalogIdentityError>,
) -> ViewerReply {
    use crate::viewer::{decoded, query_value};
    let value = |name| query_value(query, name).and_then(decoded);
    let scope = match value("scope").as_deref() {
        None | Some("current") => CatalogReadScope::Current,
        Some("retained_history") => CatalogReadScope::RetainedHistory,
        _ => return crate::catalog::error(400, "invalid_arguments", "Unknown read scope.", false),
    };
    let (Some(harness), Some(native)) = (value("harness"), value("native_id")) else {
        return crate::catalog::error(
            400,
            "invalid_arguments",
            "Explicit harness and native_id are required.",
            false,
        );
    };
    if !valid_resolution_arguments(key, &harness, &native) {
        return crate::catalog::error(
            400,
            "invalid_arguments",
            "Explicit harness and native_id are required.",
            false,
        );
    }
    if let Err(reply) = before_read() {
        return reply;
    }
    let reply = read(&harness, &native, scope);
    match reply {
        Ok(resolution) => ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: serde_json::json!({"api":1,"resolution":resolution})
                .to_string()
                .into_bytes(),
            etag: None,
            extra_headers: Vec::new(),
        },
        Err(CatalogIdentityError::InvalidArguments) => crate::catalog::error(
            400,
            "invalid_arguments",
            "Explicit harness and native_id are required.",
            false,
        ),
        Err(_) => crate::catalog::error(
            503,
            "catalog_unavailable",
            "Native identity observation is unavailable.",
            true,
        ),
    }
}
