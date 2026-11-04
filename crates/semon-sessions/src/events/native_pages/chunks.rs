//! Existing native chunk wire format, independent of the SQL engine.
use super::Plan;
use crate::{
    events::{CACHE_VERSION, Ledger},
    source_chunks::Chunks,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
const PAGE: usize = 1024;
pub(crate) fn plan(ledger: &Ledger, chunks: &Chunks) -> serde_json::Result<Option<Plan>> {
    if !chunks.valid(ledger.offset) {
        return Ok(None);
    }
    let mut pages = Vec::new();
    let mut nodes = Vec::new();
    for (index, hashes) in chunks.hashes.chunks(PAGE).enumerate() {
        let mut page = Page {
            version: 1,
            index,
            hashes: hashes.to_vec(),
            sha256: String::new(),
        };
        let canonical = page.canonical()?;
        nodes.push(<[u8; 32]>::from(Sha256::digest(canonical.as_bytes())));
        page.sha256 = format!("{:x}", Sha256::digest(canonical.as_bytes()));
        pages.push(serde_json::to_string(&page)?);
    }
    let mut tree = Vec::new();
    let root = loop {
        tree.push(
            nodes
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()?,
        );
        if nodes.len() <= 1 {
            break nodes
                .first()
                .copied()
                .unwrap_or_else(|| Sha256::digest([]).into());
        }
        nodes = nodes
            .chunks(2)
            .map(|pair| {
                let mut hash = Sha256::new();
                hash.update(pair[0]);
                hash.update(pair.get(1).unwrap_or(&pair[0]));
                hash.finalize().into()
            })
            .collect();
    };
    let header = serde_json::to_string(&Header {
        version: 1,
        parser_version: CACHE_VERSION,
        ledger: crate::events::store::header_ledger(ledger),
        count: chunks.hashes.len(),
        consumed_offset: Some(ledger.offset),
        fingerprint: chunks.fingerprint(),
        pages_root: root,
        resume: chunks.resume.clone(),
    })?;
    Ok(Some(Plan {
        header,
        pages,
        tree,
    }))
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Header {
    pub(crate) version: u32,
    pub(crate) parser_version: u32,
    pub(crate) ledger: String,
    pub(crate) count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) consumed_offset: Option<u64>,
    pub(crate) fingerprint: [u8; 32],
    pub(crate) pages_root: [u8; 32],
    #[serde(default)]
    pub(crate) resume: Option<crate::source_chunks::PrefixState>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
    pub(crate) version: u32,
    pub(crate) index: usize,
    pub(crate) hashes: Vec<[u8; 32]>,
    pub(crate) sha256: String,
}

impl Page {
    pub(crate) fn canonical(&self) -> serde_json::Result<String> {
        // The witness covers only the page payload, never its own digest.
        #[derive(Serialize)]
        struct Payload<'a> {
            version: u32,
            index: usize,
            hashes: &'a [[u8; 32]],
        }
        serde_json::to_string(&Payload {
            version: self.version,
            index: self.index,
            hashes: &self.hashes,
        })
    }
}
