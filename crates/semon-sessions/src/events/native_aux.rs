//! Content-free native metadata plans shared by the physical file writers.
use super::{CACHE_VERSION, Ledger, store::header_ledger};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodexHeaderRecord {
    pub(crate) version: u32,
    pub(crate) parser_version: u32,
    pub(crate) ledger: String,
    pub(crate) header: crate::native_header::CodexHeader,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaudeSidecarRecord {
    pub(crate) version: u32,
    pub(crate) parser_version: u32,
    pub(crate) ledger: String,
    pub(crate) sidecar: crate::native_sidecar::ClaudeSidecar,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ParentMarkerRecord {
    pub(crate) version: u32,
    pub(crate) parser_version: u32,
    pub(crate) ledger: String,
    pub(crate) index: crate::native_marker::Index,
}

pub(crate) fn codex_header(
    ledger: &Ledger,
    header: Option<&crate::native_header::CodexHeader>,
) -> Result<Option<String>, serde_json::Error> {
    header
        .map(|header| {
            serde_json::to_string(&CodexHeaderRecord {
                version: 1,
                parser_version: CACHE_VERSION,
                ledger: header_ledger(ledger),
                header: header.clone(),
            })
        })
        .transpose()
        .map(|value| value.filter(|text| text.len() <= 1048576))
}

pub(crate) fn claude_sidecar(
    ledger: &Ledger,
    sidecar: Option<&crate::native_sidecar::ClaudeSidecar>,
) -> Result<Option<String>, serde_json::Error> {
    sidecar
        .map(|sidecar| {
            serde_json::to_string(&ClaudeSidecarRecord {
                version: 1,
                parser_version: CACHE_VERSION,
                ledger: header_ledger(ledger),
                sidecar: sidecar.clone(),
            })
        })
        .transpose()
        .map(|value| value.filter(|text| text.len() <= 1048576))
}

pub(crate) fn parent_marker(
    ledger: &Ledger,
    index: Option<&crate::native_marker::Index>,
) -> Result<Option<String>, serde_json::Error> {
    index
        .filter(|index| index.valid(ledger))
        .map(|index| {
            serde_json::to_string(&ParentMarkerRecord {
                version: 1,
                parser_version: CACHE_VERSION,
                ledger: header_ledger(ledger),
                index: index.clone(),
            })
        })
        .transpose()
}
