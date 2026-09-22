//! Loopback-only replication of complete Claude Code session lines.

mod discovery;
mod lease;
mod protocol;
mod receiver;
mod sender;
mod state;

pub use discovery::{DiscoveredStream, discover_streams};
pub use lease::{
    Clock, LEASE_DURATION_MS, LEASE_RENEW_INTERVAL_MS, LeaseRow, LeaseStatus, OrphanSummary,
    StreamTip, SystemClock, TakeoverRecord, TakeoverResult,
};
pub use protocol::{Frame, FrameKey, ZERO_CHAIN, chain_line};
pub use receiver::{ReceiveError, ReceiveOutcome, Receiver, serve, validate_loopback};
pub use sender::{
    HttpTransport, LagSummary, PassReport, RelayError, Sender, StreamReport, TakeoverCommandError,
    Transport, TransportError, initialize_takeover_state, read_machine_identity, run_pass,
    takeover_session, verify_takeover_source,
};
pub use state::{RelayState, StateError, StreamState, load_state, save_state};
