//! A local, content-addressed trace store with a structural forensic boundary.
//!
//! The database has three storage regions:
//!
//! - `canonical_traces` contains only transferable semantic JSON. Ordinary
//!   [`TraceStore::fetch_trace`] and [`TraceStore::list_traces`] reads return
//!   [`CanonicalTrace`], a type with no raw-record field.
//! - `occurrences` records, for each time a trace was observed, where and
//!   when: carrier, session, a deterministic per-session sequence, timestamp,
//!   repository attribution (with its basis), a parent-occurrence edge, and
//!   who or what authored it. It carries no foreign key to
//!   `raw_carrier_records` and no read path here reconstructs it from raw, so
//!   it survives deletion of the forensic region intact. [`TraceStore::log`]
//!   is the ordinary read over this region, joined to `canonical_traces`.
//! - `raw_carrier_records` contains every complete source line as opaque
//!   carrier bytes with its own session, sequence, and timestamp. Its
//!   trace links live in `raw_record_traces`; an unprojected line has none.
//!   Raw rows are available only
//!   through the deliberately separate
//!   [`TraceStore::fetch_raw_carrier_records`] and
//!   [`TraceStore::fetch_raw_carrier_records_for_occurrences`] calls, which
//!   the `semon forensic` command is the sole CLI path to. Raw bytes and
//!   carrier names are never inputs to identity or ordinary retrieval. This
//!   region is retained in full and indefinitely — no pruning, no opt-out, no
//!   sampling — so it is the only region whose exposure is governed
//!   separately: the store file is created and re-opened with owner-only
//!   permissions (see [`TraceStore::open`]). See
//!   `docs/design/forensic-retention-and-exposure.md`.
//!
//! See `docs/design/trace-identity-and-occurrences.md` for why identity stays
//! content-addressed over `canonical_traces` alone while provenance and order
//! live in the separate `occurrences` region.
//!
//! A projected capture inserts into all three regions in one SQLite
//! transaction. A raw-only capture inserts only its forensic row. Repeated
//! captures of the same source line reuse its raw row when the key and bytes
//! agree. Different bytes at the same key retain separate rows. The
//! occurrence write is instead an UPSERT keyed on
//! `(carrier, session, sequence)`, because an occurrence is one row per time a
//! trace was *seen*, so "already recorded" cannot be expressed by content —
//! two occurrences of one trace are the region's entire purpose, not a
//! duplicate.
//!
//! # Canonical semantic form
//!
//! Identity is the lowercase hexadecimal SHA-256 digest of RFC 8785 (JCS)
//! canonical JSON bytes. The rules are:
//!
//! - insignificant source whitespace and object insertion order are discarded;
//! - object names are sorted recursively by UTF-16 code units;
//! - array order is preserved;
//! - strings preserve their Unicode scalar values without normalization and use
//!   JCS's minimal JSON escaping, so escaped and literal spellings of the same
//!   string converge;
//! - numbers use the finite IEEE-754 binary64 domain and ECMAScript's shortest
//!   round-trippable spelling, so `1`, `1.0`, and `1e0` converge and negative
//!   zero becomes `0`; integer-valued inputs outside `-(2^53-1)..=2^53-1` are
//!   rejected instead of being rounded into another identity, and this
//!   rejection applies identically no matter how the value is spelled —
//!   `9007199254740993` and `9007199254740993.0` are both rejected, not just
//!   the integer-literal spelling, because IEEE-754 double spacing means
//!   every finite `f64` past that magnitude (including ones JCS would print
//!   in exponential notation, at `1e21` and beyond) is already
//!   integer-valued, so no fractional float literal exists out there to
//!   exempt;
//! - booleans and null use their JSON literals; an absent object member remains
//!   different from a member explicitly set to null;
//! - when source JSON repeats an object name, parsing retains the last value
//!   before canonicalization.
//!
//! The semantic core is intentionally an unconstrained JSON document. The
//! store therefore makes no session/task/decision granularity commitment, and
//! later lexical or embedding matchers can operate on [`SemanticCore`] without
//! gaining access to raw carrier records.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod canonical;
mod model;
mod render;
mod store;

pub use model::{
    AuthoredBy, CanonicalTrace, CaptureResult, LogFilter, NewOccurrence, NewRawCarrierRecord,
    OccurrenceRecord, RawCarrierRecord, RepoSource, SemanticCore, TraceId,
};
pub use render::{day_bounds_ns, format_day_ns, format_timestamp_ns, render_occurrence_line};
pub use store::{
    CLAUDE_MAX_BLOCKS_PER_RECORD, ForgetSelector, OccurrenceSelector, RawBackfillLine,
    RawBackfillResult, RawSessionRekeyLine, SessionRepairCounts, StoreError, TraceStore,
};

#[cfg(test)]
mod boundary_tests {
    use super::CanonicalTrace;

    // Exact destructuring makes adding raw state to the ordinary read model a
    // compile-time test failure.
    #[allow(dead_code)]
    fn canonical_trace_has_only_identity_and_semantics(trace: CanonicalTrace) {
        let CanonicalTrace {
            id: _,
            semantic_core: _,
        } = trace;
    }
}
