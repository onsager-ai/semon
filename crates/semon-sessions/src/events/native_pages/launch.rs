//! Existing native launch wire format, independent of the SQL engine.
use super::Plan;
use crate::{
    events::{CACHE_VERSION, Ledger},
    native_handoff::{CallRef, Index},
    native_record::Record,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
const PAGE: usize = 256;
impl Header {
    pub(crate) fn valid(&self, ledger: &Ledger) -> bool {
        self.version == 1
            && self.parser_version == CACHE_VERSION
            && self.ledger == crate::events::store::header_ledger(ledger)
            && self.complete <= crate::handoff::MAX_CALLS
            && self.count >= self.complete
            && self.count - self.complete <= crate::handoff::MAX_CALLS
    }
}

pub(crate) fn restore(
    header: Header,
    mut calls: std::collections::VecDeque<CallRef>,
    ledger: &Ledger,
) -> serde_json::Result<Option<Index>> {
    if !header.valid(ledger) || calls.len() != header.count {
        return Ok(None);
    }
    let eof = calls.split_off(header.complete);
    let index = Index {
        complete: calls,
        eof,
        eof_record: header.eof_record,
        complete_bytes: header.complete_bytes,
        eof_bytes: header.eof_bytes,
    };
    if !index.valid(ledger)
        || <[u8; 32]>::from(Sha256::digest(serde_json::to_string(&index)?.as_bytes()))
            != header.fingerprint
    {
        return Ok(None);
    }
    Ok(Some(index))
}
pub(crate) fn plan(ledger: &Ledger, index: &Index) -> serde_json::Result<Option<Plan>> {
    if !index.valid(ledger) {
        return Ok(None);
    }
    let calls: Vec<_> = index.complete.iter().chain(&index.eof).cloned().collect();
    let pages = calls
        .chunks(PAGE)
        .enumerate()
        .map(|(index, calls)| {
            serde_json::to_string(&Page {
                version: 1,
                index,
                calls: calls.to_vec(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if pages.iter().any(|page| page.len() > 1024 * 1024) {
        return Ok(None);
    }
    let header = serde_json::to_string(&Header {
        version: 1,
        parser_version: CACHE_VERSION,
        ledger: crate::events::store::header_ledger(ledger),
        complete: index.complete.len(),
        count: calls.len(),
        eof_record: index.eof_record.clone(),
        complete_bytes: index.complete_bytes,
        eof_bytes: index.eof_bytes,
        fingerprint: Sha256::digest(serde_json::to_string(index)?.as_bytes()).into(),
    })?;
    Ok(Some(Plan {
        header,
        pages,
        tree: Vec::new(),
    }))
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Header {
    pub(crate) version: u32,
    pub(crate) parser_version: u32,
    pub(crate) ledger: String,
    pub(crate) complete: usize,
    pub(crate) count: usize,
    pub(crate) eof_record: Option<Record>,
    pub(crate) complete_bytes: usize,
    pub(crate) eof_bytes: usize,
    pub(crate) fingerprint: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Page {
    pub(crate) version: u32,
    pub(crate) index: usize,
    pub(crate) calls: Vec<CallRef>,
}
