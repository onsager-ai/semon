//! Pure read-contract advertisement for authorized host dispatch, with no
//! ViewerCore, source discovery, archive restoration or model initialization.
use crate::viewer::ViewerReply;
use serde_json::json;

/// Advertise only endpoints actually wired by the host. An advertised endpoint
/// may still reject a particular unqualified native field with an explicit 422.
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionReadEndpoints {
    pub selected_identity: bool,
    pub native_resolution: bool,
    pub selected_transcript: bool,
    pub selected_entry: bool,
}

/// The host must authorize the configured stable source key before calling this
/// helper. Single-machine public Viewer uses the explicit catalog alias `local`;
/// hostname labels and native session ids never derive source authority.
pub fn session_catalog_capabilities(
    source_key: &str,
    endpoints: SessionReadEndpoints,
) -> ViewerReply {
    if source_key.is_empty() || source_key.len() > 4096 {
        return crate::catalog::error(
            400,
            "invalid_arguments",
            "A stable source key is required.",
            false,
        );
    }
    ViewerReply {
        status:200,
        content_type:"application/json; charset=utf-8",
        body:json!({"api":1,"read_contract":"catalog-v1","source_key":source_key,
            "native_resolution":endpoints.native_resolution,"selected_identity":endpoints.selected_identity,"selected_transcript":endpoints.selected_transcript,
            "selected_entry":endpoints.selected_entry,"pagination":true,"relationship_context":false,
            "large_native_records":false,"attachment":false,"global_union":false,"full_text_search":false,
            "metadata_search":true,"filters":["harness","repo","q"],"order":"last_desc_key_asc"}).to_string().into_bytes(),
        etag:None,
    }
}
