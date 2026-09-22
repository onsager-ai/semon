//! Loopback-only replication of complete Claude Code session lines.

mod discovery;
mod protocol;
mod receiver;
mod sender;
mod state;

pub use discovery::{DiscoveredStream, discover_streams};
pub use protocol::{Frame, FrameKey, ZERO_CHAIN, chain_line};
pub use receiver::{ReceiveError, ReceiveOutcome, Receiver, serve, validate_loopback};
pub use sender::{
    HttpTransport, LagSummary, PassReport, RelayError, Sender, StreamReport, Transport,
    TransportError, run_pass,
};
pub use state::{RelayState, StateError, StreamState, load_state, save_state};
