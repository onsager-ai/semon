//! Indexed native-to-catalog navigation. This grants no native control authority.
use crate::{
    CatalogIdentityError, CatalogReadScope, Options, model::summary::CatalogRow,
    viewer::ViewerReply,
};
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
    if !valid_resolution_arguments(source_key, harness, native_id) {
        return Err(CatalogIdentityError::InvalidArguments);
    }
    let binding = crate::storage::local_navigation_binding(options, source_key)?;
    let store = crate::storage::navigation_store(binding);
    resolve_with_store(
        options,
        source_key,
        harness,
        native_id,
        scope,
        None,
        store.as_ref(),
    )
    .map_err(Into::into)
}

pub(crate) fn resolve_with_store(
    options: &Options,
    source_key: &str,
    harness: &str,
    native_id: &str,
    scope: CatalogReadScope,
    expected_generation: Option<&str>,
    store: &dyn crate::storage::navigation::NativeNavigationStore,
) -> Result<CatalogNativeResolution, crate::storage::StorageError> {
    if !valid_resolution_arguments(source_key, harness, native_id) {
        return Err(crate::storage::StorageError::InvalidArguments);
    }
    let mut result = CatalogNativeResolution {
        state: CatalogNativeResolutionState::Unavailable,
        source_key: source_key.into(),
        harness: harness.into(),
        native_id: native_id.into(),
        read_scope: scope,
        catalog_key: None,
        generation: None,
    };
    let observation = store.native_resolution(harness, native_id, scope, expected_generation)?;
    apply_observation(&mut result, Some(options), &observation)
        .map_err(|_| crate::storage::StorageError::Unavailable)?;
    Ok(result)
}

fn valid_resolution_arguments(source_key: &str, harness: &str, native_id: &str) -> bool {
    !source_key.is_empty()
        && source_key.len() <= 4096
        && !native_id.is_empty()
        && native_id.len() <= 4096
        && !native_id.chars().any(char::is_control)
        && matches!(harness, "claude" | "codex" | "copilot")
}

pub(crate) fn apply_observation(
    result: &mut CatalogNativeResolution,
    options: Option<&Options>,
    observation: &crate::storage::navigation::NativeResolutionObservation,
) -> Result<(), CatalogIdentityError> {
    result.state = CatalogNativeResolutionState::Unavailable;
    result.catalog_key = None;
    result.generation = None;
    let Some((generation, complete)) = observation.qualified_head() else {
        return Ok(());
    };
    result.generation = Some(generation.into());
    match observation.keys.as_slice() {
        [] => result.state = CatalogNativeResolutionState::Pending,
        [key] => {
            if let Some(encoded) = &observation.metadata {
                let row: CatalogRow =
                    serde_json::from_str(encoded).map_err(|_| CatalogIdentityError::Unavailable)?;
                if row.key == *key
                    && row.harness == result.harness
                    && row.sources.len() <= 64
                    && row.sources.iter().any(|s| {
                        if s.native_id != result.native_id {
                            return false;
                        }
                        let Some(options) = options else {
                            return true;
                        };
                        let home = match result.harness.as_str() {
                            "claude" => &options.claude_home,
                            "codex" => &options.codex_home,
                            "copilot" => &options.copilot_home,
                            _ => return false,
                        };
                        s.path
                            .strip_prefix(home)
                            .ok()
                            .and_then(|p| p.to_str())
                            .is_some_and(|path| crate::is_input_path(&result.harness, path))
                    })
                {
                    result.catalog_key = Some(key.clone());
                    result.state = if complete {
                        CatalogNativeResolutionState::Resolved
                    } else {
                        CatalogNativeResolutionState::Provisional
                    };
                }
            }
        }
        _ => result.state = CatalogNativeResolutionState::Ambiguous,
    }
    Ok(())
}

pub(crate) fn page_with_gate(
    options: &Options,
    key: &str,
    query: &str,
    before_read: impl FnOnce() -> Result<(), ViewerReply>,
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
    let reply = session_catalog_resolve_native(options, key, &harness, &native, scope);
    match reply {
        Ok(resolution) => ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: serde_json::json!({"api":1,"resolution":resolution})
                .to_string()
                .into_bytes(),
            etag: None,
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
