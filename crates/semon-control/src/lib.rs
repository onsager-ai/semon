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
//! Every operation takes the current time as monotonic milliseconds
//! ([`monotonic_ms`]), so the deadlines are deterministic under test and
//! don't move when the wall clock is stepped. Journal lines carry wall time
//! ([`wall_ms`]) only as a record.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod canonical;
mod journal;
mod request;
mod store;

pub use journal::Journal;
pub use request::{
    Answer, Answerable, Harness, Kind, LeftReason, MAX_DENY_MESSAGE_BYTES, MatchKey, NewRequest,
    PendingRequest, RequestId, ResolvedReason, Source, State,
};
pub use store::{
    Adapter, AnswerError, DELIVERY_DEADLINE_MS, Delivery, MAX_HELD_PAYLOAD_BYTES, MAX_HOOK_WAITS,
    MAX_HOOK_WAITS_PER_SESSION, MAX_ID_BYTES, MAX_OPEN, MAX_PAYLOAD_BYTES, MAX_RETAINED_FINAL,
    REFUSAL_WINDOW_MS, RETAIN_FINAL_MS, RegisterError, RequestStore, Snapshot, ToolRun,
};

use std::{
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// Milliseconds on a monotonic clock that starts at the first call in this
/// process. Every deadline (`expires_ms`, the delivery deadline, retention,
/// refusal windows) is on this clock.
pub fn monotonic_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The wall-clock time in milliseconds since the Unix epoch (0 if the clock
/// is before it). Used only for journal lines.
pub fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_monotonic_clock_advances_with_real_time() {
        let first = super::monotonic_ms();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = super::monotonic_ms();
        assert!(second >= first + 20, "{first} then {second}");
    }
}
