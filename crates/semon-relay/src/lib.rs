//! Encrypted failover replication of complete Claude Code session lines.

mod auth;
mod crypto;
mod deletion;
mod discovery;
pub use deletion::{
    ForgetFlushReport, ForgetReport, ForgetSelector, flush_forgets, forget_before_day, queue_forget,
};
mod identity;
mod lease;
mod protocol;
mod receiver;
mod restore;
mod sender;
mod snapshots;
mod state;

pub use auth::{AuthError, REPLAY_WINDOW_MS, RequestSigner, RequestVerifier, SignedHeaders};
pub use crypto::{
    CONTENT_TAG_BYTES, CryptoError, DATA_KEY_BYTES, DataKey, EncryptedPayload, FRAME_NONCE_BYTES,
    content_tag, decrypt_envelope, decrypt_frame, encrypt_envelope, encrypt_frame,
    generate_data_key,
};
pub use discovery::{
    DiscoveredStream, discover_codex_streams, discover_streams, filter_by_session,
};
pub use identity::{
    AGE_IDENTITY_FILE, IdentityError, InitOutcome, MACHINES_FILE, MachineIdentity, RECIPIENTS_FILE,
    SIGNING_KEY_FILE, enroll_machine, enroll_recipient, fingerprint, init, load_age_identity,
    load_machines, load_recipients, load_signing_key, parse_verifying_key,
};
pub use lease::{
    Clock, LEASE_DURATION_MS, LEASE_RENEW_INTERVAL_MS, LeaseRow, LeaseStatus, OrphanSummary,
    StreamTip, SystemClock, TakeoverRecord, TakeoverResult,
};
pub use protocol::{
    FRAME_BATCH_MAX_BYTES, FRAME_BATCH_MAX_FRAMES, FRAME_PAGE_MAX_BYTES, FRAME_PAGE_MAX_FRAMES,
    Frame, FrameContent, FrameKey, FrameMode, FramePage, ZERO_CHAIN, chain_line,
};
pub use receiver::{
    EnvelopeOutcome, MAX_REQUEST_BODY_BYTES, ReceiveError, ReceiveOutcome, Receiver, ServeConfig,
    TlsFiles, serve, serve_configured, validate_loopback, validate_serve_config,
};
pub use restore::{
    GitBranchCheck, RestoreError, RestoreReport, RestoredStream, UnfinishedToolCall,
    restore_session, restore_session_encrypted,
};
pub use sender::{
    HttpTransport, LagSummary, PassReport, RelayError, Sender, StreamReport, TakeoverCommandError,
    Transport, TransportError, VerifyError, VerifyReport, initialize_takeover_state,
    read_machine_identity, run_pass, takeover_session, takeover_session_encrypted,
    verify_encrypted_frame, verify_encrypted_frames, verify_encrypted_session,
    verify_takeover_source,
};
pub use state::{RelayState, StateError, StreamState, load_state, save_state};

pub use snapshots::{
    SnapshotCache, SnapshotPacket, capture_snapshot, inspect_snapshot_manifest,
    list_snapshot_heads, list_snapshot_history, list_snapshot_roots, load_snapshot_packet,
    persist_snapshot_packet, publish_snapshot, read_snapshot_file, restore_snapshot,
    restore_snapshot_for_session, snapshot_root_id,
};
