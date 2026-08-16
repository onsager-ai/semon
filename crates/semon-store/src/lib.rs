//! A local, content-addressed trace store with a structural forensic boundary.
//!
//! The database has two storage regions:
//!
//! - `canonical_traces` contains only transferable semantic JSON. Ordinary
//!   [`TraceStore::fetch_trace`] and [`TraceStore::list_traces`] reads return
//!   [`CanonicalTrace`], a type with no raw-record field.
//! - `raw_carrier_records` contains opaque carrier bytes and minimal carrier
//!   provenance. They are available only through the deliberately separate
//!   [`TraceStore::fetch_raw_carrier_records`] call. Raw bytes and carrier names
//!   are never inputs to identity or ordinary retrieval.
//!
//! A capture inserts into both regions in one SQLite transaction. Repeated
//! captures of the same semantics retain one canonical row and append one raw
//! row per successful capture, including byte-identical captures.
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
mod replication;
mod store;

pub use model::{
    CanonicalTrace, CaptureResult, NewRawCarrierRecord, RawCarrierRecord, SemanticCore, TraceId,
};
pub use replication::{REPLICATION_ENDPOINT_ENV, ReplicationError, ShipReport, ship};
pub use store::{StoreError, TraceStore};

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
