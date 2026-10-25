//! Pure values for the persisted `files` row.
//!
//! This is intentionally only the shared value plan: adapters retain physical
//! allocation, SQL, transaction boundaries and error precedence.

use std::collections::BTreeMap;

use super::{FileIndex, Ledger, Links, ModelTokens, RateLimits};
use crate::Tokens;

/// Borrowed values in the existing `PUT_FILE` bind order.
pub(crate) struct FileRow<'a> {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) size: u64,
    pub(crate) modified_ns: u128,
    pub(crate) offset: u64,
    pub(crate) prefix: &'a [u8; 32],
    pub(crate) tail: &'a [u8; 32],
    pub(crate) entrypoint: &'a Option<String>,
    pub(crate) title: &'a Option<String>,
    pub(crate) agent_name: &'a Option<String>,
    pub(crate) last_model: &'a Option<String>,
    pub(crate) failed: bool,
    pub(crate) first: &'a Option<i64>,
    pub(crate) last: &'a Option<i64>,
    pub(crate) cwd: &'a Option<String>,
    pub(crate) branch: &'a Option<String>,
    pub(crate) pending: &'a BTreeMap<String, usize>,
    pub(crate) tool_ids: &'a BTreeMap<String, Vec<usize>>,
    pub(crate) yields: &'a BTreeMap<String, usize>,
    pub(crate) busy: &'a Vec<(i64, i64)>,
    pub(crate) links: &'a Links,
    pub(crate) codex_tokens: &'a Tokens,
    pub(crate) codex_tokens_by_model: &'a BTreeMap<String, ModelTokens>,
    pub(crate) rate_limits: &'a Option<RateLimits>,
    pub(crate) index: &'a FileIndex,
    pub(crate) copilot: &'a Option<crate::copilot::Index>,
    pub(crate) changed_ns: Option<i128>,
}

impl<'a> FileRow<'a> {
    pub(crate) fn new(ledger: &'a Ledger, index: &'a FileIndex) -> Self {
        let FileIndex {
            dirty_rows: _,
            native_usage_observations: _,
            tree_summary: _,
            source_chunks: _,
            marker_index: _,
            handoff_index: _,
            codex_header: _,
            claude_sidecar: _,
            events: _,
            extras: _,
            signals: _,
            pending,
            tool_ids,
            yields,
            busy,
            links,
            entrypoint,
            title,
            agent_name,
            last_model,
            failed,
            first,
            last,
            cwd,
            branch,
            usage_by_id: _,
            codex_tokens,
            codex_tokens_by_model,
            codex_usage_events: _,
            codex_native_usage: _,
            copilot,
            rate_limits,
        } = index;
        let Ledger {
            stat:
                super::Stat {
                    dev,
                    ino,
                    size,
                    modified_ns,
                    changed_ns,
                },
            offset,
            prefix,
            tail,
        } = ledger;
        Self {
            dev: *dev,
            ino: *ino,
            size: *size,
            modified_ns: *modified_ns,
            offset: *offset,
            prefix,
            tail,
            entrypoint,
            title,
            agent_name,
            last_model,
            failed: *failed,
            first,
            last,
            cwd,
            branch,
            pending,
            tool_ids,
            yields,
            busy,
            links,
            codex_tokens,
            codex_tokens_by_model,
            rate_limits,
            index,
            copilot,
            changed_ns: *changed_ns,
        }
    }
}
