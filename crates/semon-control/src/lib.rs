//! Two-way control, step 1: the pending-request model.
//!
//! This crate holds what the local Semon process knows about the questions
//! and permission prompts that agents are waiting on, and nothing that talks
//! to a harness, a browser or a socket. See `docs/design/two-way.md`:
//!
//! - [`canonical`] pins the payload encoding (RFC 8785, over the I-JSON safe
//!   number range) whose SHA-256 an answer must carry.
//! - [`RequestStore`] holds the requests, their states and transitions, and
//!   the caps; an [`Adapter`] delivers an answer to its harness.
//! - [`Journal`] is the plain append-only record of answers and refusals.
//!
//! Every operation takes the current time in milliseconds since the Unix
//! epoch, so the transitions are deterministic under test; [`now_ms`] gives
//! the real clock.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod canonical;
mod journal;
mod request;
mod store;

pub use journal::Journal;
pub use request::{
    Answer, Answerable, Harness, Kind, LeftReason, MatchKey, NewRequest, PendingRequest, RequestId,
    ResolvedReason, Source, State,
};
pub use store::{
    Adapter, AnswerError, DELIVERY_DEADLINE_MS, Delivery, MAX_HOOK_WAITS,
    MAX_HOOK_WAITS_PER_SESSION, MAX_ID_BYTES, MAX_OPEN, MAX_PAYLOAD_BYTES, MAX_RETAINED_FINAL,
    REFUSAL_WINDOW_MS, RETAIN_FINAL_MS, RegisterError, RequestStore, Snapshot, ToolRun,
};

use std::time::{SystemTime, UNIX_EPOCH};

/// The current time in milliseconds since the Unix epoch (0 if the clock is
/// before it).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
