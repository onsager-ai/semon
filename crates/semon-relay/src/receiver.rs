use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tiny_http::{Header, Method, Request, Response, Server, SslConfig, StatusCode};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::{
    AuthError, Clock, Frame, FrameContent, FrameMode, LEASE_DURATION_MS, LeaseRow, LeaseStatus,
    MACHINES_FILE, OrphanSummary, RequestVerifier, SignedHeaders, StreamTip, SystemClock,
    TakeoverRecord, TakeoverResult, ZERO_CHAIN, chain_line,
    lease::{LeaseValueError, rows_from_value, rows_to_value},
    load_machines,
    protocol::FrameError,
    state::{StateError, write_atomic},
};

const REGISTER_FILE: &str = "lease-register.json";
const TAKEOVER_LOG: &str = "takeovers.jsonl";
const SESSION_MODES_DIR: &str = "session-modes";
const ENVELOPES_DIR: &str = "keys";
const REWRAP_LOG: &str = "rewraps.jsonl";
const RECEIPT_CHECKPOINT_INTERVAL: u64 = 256;
/// Maximum accepted HTTP request body size.
///
/// Claude Code lines can contain multi-megabyte tool results. This bounds each
/// request while leaving substantial headroom above observed legitimate frames.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;
// A slow body can occupy one worker because tiny_http exposes no read timeout.
// Four workers keep one such client from serializing all receiver traffic.
const REQUEST_WORKERS: usize = 4;

/// Whether an accepted frame was newly stored or already present unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveOutcome {
    Stored,
    Duplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvelopeOutcome {
    Stored,
    Duplicate,
    Replaced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsFiles {
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServeConfig {
    pub tls: Option<TlsFiles>,
    pub insecure_plaintext: bool,
}

/// A receiver rejection or local storage failure.
#[derive(Debug, Error)]
pub enum ReceiveError {
    #[error("session {session} has no lease row")]
    LeaseMissing { session: String },
    #[error("lease for session {session} is still live until {lease_expires_at_ms}")]
    LeaseActive {
        session: String,
        lease_expires_at_ms: u64,
    },
    #[error(
        "takeover lost its compare-and-swap: expected epoch {expected_epoch}, current epoch is {current_epoch}"
    )]
    TakeoverConflict {
        expected_epoch: u64,
        current_epoch: u64,
    },
    #[error("sender is fenced at epoch {frame_epoch}; current epoch is {current_epoch}")]
    Fenced {
        frame_epoch: u64,
        current_epoch: u64,
    },
    #[error("frame epoch {frame_epoch} is ahead of current epoch {current_epoch}")]
    FutureEpoch {
        frame_epoch: u64,
        current_epoch: u64,
    },
    #[error(
        "machine {machine} does not hold session {session} at epoch {current_epoch}; holder is {holder_machine}"
    )]
    NotHolder {
        session: String,
        machine: String,
        current_epoch: u64,
        holder_machine: String,
    },
    #[error(
        "machine {machine} did not hold session {session} at fenced epoch {epoch}; holder was {holder_machine}"
    )]
    OrphanMachine {
        session: String,
        machine: String,
        epoch: u64,
        holder_machine: String,
    },
    #[error("epoch overflow for session {session}")]
    EpochOverflow { session: String },
    #[error("frame key already exists with different bytes")]
    Collision,
    #[error("sequence gap: expected {expected}, got {actual}")]
    Gap { expected: u64, actual: u64 },
    #[error("chain break at sequence {seq}")]
    Chain { seq: u64 },
    #[error("session {session} is already {existing}; cannot accept a {incoming} frame")]
    MixedMode {
        session: String,
        existing: &'static str,
        incoming: &'static str,
    },
    #[error("plaintext frames are disabled on this receiver")]
    PlaintextDisabled,
    #[error("encrypted frames are disabled on an insecure-plaintext receiver")]
    EncryptedDisabled,
    #[error("session {session} has no encrypted data-key envelope")]
    EnvelopeMissing { session: String },
    #[error("session {session} already has a different data-key envelope")]
    EnvelopeConflict { session: String },
    #[error("machine {machine} cannot rewrap session {session}; holder is {holder}")]
    RewrapNotHolder {
        session: String,
        machine: String,
        holder: String,
    },
    #[error("receiver storage is inconsistent: {0}")]
    Inconsistent(String),
    #[error("cannot access receiver storage: {0}")]
    Io(#[from] io::Error),
    #[error("cannot decode receiver storage: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid stored frame: {0}")]
    Frame(#[from] FrameError),
    #[error("invalid lease data: {0}")]
    LeaseValue(#[from] LeaseValueError),
    #[error("invalid receiver state: {0}")]
    State(#[from] StateError),
    #[error("refusing non-loopback listen address {0}")]
    NonLoopback(SocketAddr),
    #[error("non-loopback receiver requires --tls-cert and --tls-key")]
    NonLoopbackTls,
    #[error("non-loopback receiver requires at least one enrolled machine in {0}")]
    NonLoopbackMachines(PathBuf),
    #[error("non-loopback receiver cannot enable --insecure-plaintext")]
    NonLoopbackPlaintext,
    #[error("TLS requires both --tls-cert and --tls-key")]
    TlsPair,
    #[error("cannot read TLS file {path}: {source}")]
    TlsFile { path: PathBuf, source: io::Error },
    #[error("invalid machine allowlist: {0}")]
    Machines(#[from] crate::IdentityError),
    #[error("signed request authentication failed: {0}")]
    Auth(#[from] AuthError),
    #[error("signed request header {0} is missing or invalid")]
    AuthHeader(&'static str),
    #[error("cannot start HTTP receiver: {0}")]
    HttpServer(String),
    #[error("receiver state lock was poisoned")]
    Poisoned,
    #[error("request field {0} is missing or has the wrong type")]
    RequestField(&'static str),
    #[error("request body exceeds the {limit}-byte limit")]
    BodyTooLarge { limit: usize },
}

#[derive(Clone, Copy, Debug)]
struct Receipt {
    epoch: u64,
    acked: Option<u64>,
    mode: Option<FrameMode>,
    chain: [u8; 32],
    tag: Option<[u8; 32]>,
}

#[derive(Default)]
struct ReceiverState {
    rows: BTreeMap<String, LeaseRow>,
    takeovers: Vec<TakeoverRecord>,
    receipts: BTreeMap<(String, String, u64), Receipt>,
}

/// Disk-backed receiver for leases, live frames, and fenced orphan frames.
pub struct Receiver {
    root: PathBuf,
    clock: Arc<dyn Clock>,
    state: Mutex<ReceiverState>,
    allow_plaintext: bool,
    allow_encrypted: bool,
}

impl Receiver {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ReceiveError> {
        Self::open_with_modes(root, Arc::new(SystemClock), true, true)
    }

    /// Opens a receiver with an injected clock for deterministic lease tests.
    pub fn open_with_clock(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ReceiveError> {
        Self::open_with_modes(root, clock, true, true)
    }

    pub fn open_with_policy(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        allow_plaintext: bool,
    ) -> Result<Self, ReceiveError> {
        Self::open_with_modes(root, clock, allow_plaintext, true)
    }

    fn open_with_modes(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        allow_plaintext: bool,
        allow_encrypted: bool,
    ) -> Result<Self, ReceiveError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        set_private_dir(&root)?;
        Ok(Self {
            state: Mutex::new(ReceiverState {
                rows: load_rows(&root.join(REGISTER_FILE))?,
                takeovers: load_takeovers(&root.join(TAKEOVER_LOG))?,
                receipts: BTreeMap::new(),
            }),
            root,
            clock,
            allow_plaintext,
            allow_encrypted,
        })
    }

    /// Acquires a new session at epoch zero, renews the caller, or returns its holder.
    pub fn acquire(&self, session: &str, machine: &str) -> Result<LeaseRow, ReceiveError> {
        let mut state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        let now = self.clock.now_ms();
        let mut changed = false;
        let row = match state.rows.get(session) {
            Some(row) if row.holder_machine != machine => row.clone(),
            Some(row) => {
                let mut row = row.clone();
                row.lease_expires_at_ms = now.saturating_add(LEASE_DURATION_MS);
                state.rows.insert(session.to_owned(), row.clone());
                changed = true;
                row
            }
            None => {
                let row = LeaseRow {
                    session: session.to_owned(),
                    epoch: 0,
                    holder_machine: machine.to_owned(),
                    lease_expires_at_ms: now.saturating_add(LEASE_DURATION_MS),
                };
                state.rows.insert(session.to_owned(), row.clone());
                changed = true;
                row
            }
        };
        if changed {
            save_rows(&self.root.join(REGISTER_FILE), &state.rows)?;
        }
        Ok(row)
    }

    /// Renews only the current `(holder, epoch)` pair.
    pub fn renew(
        &self,
        session: &str,
        machine: &str,
        epoch: u64,
    ) -> Result<LeaseRow, ReceiveError> {
        let mut state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        let current =
            state
                .rows
                .get(session)
                .cloned()
                .ok_or_else(|| ReceiveError::LeaseMissing {
                    session: session.to_owned(),
                })?;
        validate_holder(&current, machine, epoch)?;
        let mut row = current;
        row.lease_expires_at_ms = self.clock.now_ms().saturating_add(LEASE_DURATION_MS);
        state.rows.insert(session.to_owned(), row.clone());
        save_rows(&self.root.join(REGISTER_FILE), &state.rows)?;
        Ok(row)
    }

    /// Performs the epoch compare-and-swap and returns live stream tips.
    pub fn takeover(
        &self,
        session: &str,
        machine: &str,
        expected_epoch: u64,
        force: bool,
    ) -> Result<TakeoverResult, ReceiveError> {
        let mut state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        let previous =
            state
                .rows
                .get(session)
                .cloned()
                .ok_or_else(|| ReceiveError::LeaseMissing {
                    session: session.to_owned(),
                })?;
        if previous.epoch != expected_epoch {
            return Err(ReceiveError::TakeoverConflict {
                expected_epoch,
                current_epoch: previous.epoch,
            });
        }
        let now = self.clock.now_ms();
        if !force && now < previous.lease_expires_at_ms {
            return Err(ReceiveError::LeaseActive {
                session: session.to_owned(),
                lease_expires_at_ms: previous.lease_expires_at_ms,
            });
        }
        let epoch = previous
            .epoch
            .checked_add(1)
            .ok_or_else(|| ReceiveError::EpochOverflow {
                session: session.to_owned(),
            })?;
        let row = LeaseRow {
            session: session.to_owned(),
            epoch,
            holder_machine: machine.to_owned(),
            lease_expires_at_ms: now.saturating_add(LEASE_DURATION_MS),
        };
        let record = TakeoverRecord {
            session: session.to_owned(),
            previous_epoch: previous.epoch,
            epoch,
            previous_holder: previous.holder_machine,
            holder_machine: machine.to_owned(),
            forced: force,
            taken_at_ms: now,
        };
        state.rows.insert(session.to_owned(), row.clone());
        save_rows(&self.root.join(REGISTER_FILE), &state.rows)?;
        append_takeover(&self.root.join(TAKEOVER_LOG), &record)?;
        state.takeovers.push(record);
        let tips = stream_tips(&self.root, session)?;
        Ok(TakeoverResult { row, tips })
    }

    pub fn lease_status(&self, session: Option<&str>) -> Result<LeaseStatus, ReceiveError> {
        let state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        Ok(LeaseStatus {
            rows: state
                .rows
                .values()
                .filter(|row| session.is_none_or(|session| row.session == session))
                .cloned()
                .collect(),
            takeovers: state
                .takeovers
                .iter()
                .filter(|record| session.is_none_or(|session| record.session == session))
                .cloned()
                .collect(),
        })
    }

    /// Returns the current row and live tips without changing the register.
    pub fn lease_tips(&self, session: &str) -> Result<TakeoverResult, ReceiveError> {
        let state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        let row = state
            .rows
            .get(session)
            .cloned()
            .ok_or_else(|| ReceiveError::LeaseMissing {
                session: session.to_owned(),
            })?;
        let tips = stream_tips(&self.root, session)?;
        Ok(TakeoverResult { row, tips })
    }

    /// Accepts exactly the next live frame across epoch boundaries.
    pub fn accept(&self, frame: &Frame) -> Result<ReceiveOutcome, ReceiveError> {
        let mut state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        self.validate_frame_mode(frame)?;
        let row = state
            .rows
            .get(&frame.key.session)
            .ok_or_else(|| ReceiveError::LeaseMissing {
                session: frame.key.session.clone(),
            })?;
        validate_holder(row, &frame.machine, frame.key.epoch)?;

        let generation_dir = generation_dir(
            &self.root,
            &frame.key.session,
            &frame.key.stream,
            frame.key.generation,
        );
        let frame_path = epoch_dir(&generation_dir, frame.key.epoch)
            .join("frames")
            .join(format!("{:020}.json", frame.key.seq));
        let existing = read_existing(&frame_path, frame)?;
        let receipt_key = (
            frame.key.session.clone(),
            frame.key.stream.clone(),
            frame.key.generation,
        );
        let mut receipt = match state.receipts.get(&receipt_key) {
            Some(receipt) => *receipt,
            None => reconcile_receipt(
                &generation_dir,
                &frame.key.session,
                &frame.key.stream,
                frame.key.generation,
            )?,
        };
        let expected = receipt.acked.map_or(0, |seq| seq.saturating_add(1));
        if frame.key.seq < expected {
            return existing.map(|_| ReceiveOutcome::Duplicate).ok_or_else(|| {
                ReceiveError::Inconsistent(format!(
                    "acknowledged sequence {} has no stored frame",
                    frame.key.seq
                ))
            });
        }
        if frame.key.seq > expected {
            return Err(ReceiveError::Gap {
                expected,
                actual: frame.key.seq,
            });
        }
        if frame.key.epoch < receipt.epoch {
            return Err(ReceiveError::Inconsistent(format!(
                "epoch moved backwards from {} to {} at sequence {}",
                receipt.epoch, frame.key.epoch, frame.key.seq
            )));
        }
        let accepted = existing.as_ref().unwrap_or(frame);
        advance_receipt(&mut receipt, accepted)?;
        if existing.is_none() {
            store_frame(&frame_path, frame)?;
        }
        state.receipts.insert(receipt_key, receipt);
        // Frame files are authoritative and synced before acknowledgement.
        // This cross-epoch receipt only bounds restart work.
        if frame.key.seq % RECEIPT_CHECKPOINT_INTERVAL == RECEIPT_CHECKPOINT_INTERVAL - 1 {
            save_receipt(&generation_dir.join("state.json"), receipt)?;
        }
        Ok(if existing.is_some() {
            ReceiveOutcome::Duplicate
        } else {
            ReceiveOutcome::Stored
        })
    }

    /// Stores a frame from an epoch that has already been fenced.
    pub fn accept_orphan(&self, frame: &Frame) -> Result<ReceiveOutcome, ReceiveError> {
        let state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        self.validate_frame_mode(frame)?;
        let row = state
            .rows
            .get(&frame.key.session)
            .ok_or_else(|| ReceiveError::LeaseMissing {
                session: frame.key.session.clone(),
            })?;
        if frame.key.epoch >= row.epoch {
            return if frame.key.epoch > row.epoch {
                Err(ReceiveError::FutureEpoch {
                    frame_epoch: frame.key.epoch,
                    current_epoch: row.epoch,
                })
            } else {
                Err(ReceiveError::NotHolder {
                    session: frame.key.session.clone(),
                    machine: frame.machine.clone(),
                    current_epoch: row.epoch,
                    holder_machine: row.holder_machine.clone(),
                })
            };
        }
        let fenced_holder = state
            .takeovers
            .iter()
            .find(|record| {
                record.session == frame.key.session && record.previous_epoch == frame.key.epoch
            })
            .map(|record| record.previous_holder.as_str())
            .ok_or_else(|| {
                ReceiveError::Inconsistent(format!(
                    "takeover log has no holder for session {} epoch {}",
                    frame.key.session, frame.key.epoch
                ))
            })?;
        if frame.machine != fenced_holder {
            return Err(ReceiveError::OrphanMachine {
                session: frame.key.session.clone(),
                machine: frame.machine.clone(),
                epoch: frame.key.epoch,
                holder_machine: fenced_holder.to_owned(),
            });
        }
        let path = orphan_dir(
            &self.root,
            &frame.key.session,
            &frame.key.stream,
            frame.key.epoch,
            frame.key.generation,
        )
        .join("frames")
        .join(format!("{:020}.json", frame.key.seq));
        let existing = read_existing(&path, frame)?;
        if existing.is_none() {
            store_frame(&path, frame)?;
        }
        Ok(if existing.is_some() {
            ReceiveOutcome::Duplicate
        } else {
            ReceiveOutcome::Stored
        })
    }

    pub fn list_orphans(&self) -> Result<Vec<OrphanSummary>, ReceiveError> {
        list_orphans(&self.root)
    }

    pub fn put_envelope(
        &self,
        session: &str,
        machine: &str,
        envelope: &[u8],
        replace: bool,
        force: bool,
    ) -> Result<EnvelopeOutcome, ReceiveError> {
        let state = self.state.lock().map_err(|_| ReceiveError::Poisoned)?;
        let path = envelope_path(&self.root, session);
        match fs::read(&path) {
            Ok(existing) if existing == envelope => Ok(EnvelopeOutcome::Duplicate),
            Ok(_existing) if !replace => Err(ReceiveError::EnvelopeConflict {
                session: session.to_owned(),
            }),
            Ok(existing) => {
                if !force {
                    let holder = state
                        .rows
                        .get(session)
                        .map(|row| row.holder_machine.as_str())
                        .ok_or_else(|| ReceiveError::LeaseMissing {
                            session: session.to_owned(),
                        })?;
                    if holder != machine {
                        return Err(ReceiveError::RewrapNotHolder {
                            session: session.to_owned(),
                            machine: machine.to_owned(),
                            holder: holder.to_owned(),
                        });
                    }
                }
                write_atomic(&path, envelope)?;
                append_rewrap(
                    &self.root.join(REWRAP_LOG),
                    session,
                    machine,
                    force,
                    &existing,
                    envelope,
                    self.clock.now_ms(),
                )?;
                Ok(EnvelopeOutcome::Replaced)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let holder = state
                    .rows
                    .get(session)
                    .map(|row| row.holder_machine.as_str())
                    .ok_or_else(|| ReceiveError::LeaseMissing {
                        session: session.to_owned(),
                    })?;
                if holder != machine {
                    return Err(ReceiveError::RewrapNotHolder {
                        session: session.to_owned(),
                        machine: machine.to_owned(),
                        holder: holder.to_owned(),
                    });
                }
                write_atomic(&path, envelope)?;
                Ok(EnvelopeOutcome::Stored)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn get_envelope(&self, session: &str) -> Result<Option<Vec<u8>>, ReceiveError> {
        match fs::read(envelope_path(&self.root, session)) {
            Ok(envelope) => Ok(Some(envelope)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn list_envelope_sessions(&self) -> Result<Vec<String>, ReceiveError> {
        let root = self.root.join(ENVELOPES_DIR);
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        sorted_entries(&root)?
            .into_iter()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension().and_then(|value| value.to_str()) == Some("age"))
                    .then(|| path.file_stem().map(std::ffi::OsStr::to_owned))
                    .flatten()
            })
            .map(|stem| decode_component(&stem))
            .collect()
    }

    pub fn list_frames(&self, session: &str) -> Result<Vec<Frame>, ReceiveError> {
        let session_dir = self.root.join(hex::encode(session.as_bytes()));
        if !session_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut frames = Vec::new();
        for stream_entry in sorted_entries(&session_dir)? {
            if !stream_entry.path().is_dir() {
                continue;
            }
            for generation_entry in sorted_entries(&stream_entry.path())? {
                if parse_number(&generation_entry.file_name(), "generation-").is_none() {
                    continue;
                }
                for epoch_entry in sorted_entries(&generation_entry.path())? {
                    if parse_number(&epoch_entry.file_name(), "epoch-").is_none() {
                        continue;
                    }
                    let frames_dir = epoch_entry.path().join("frames");
                    if !frames_dir.is_dir() {
                        continue;
                    }
                    for entry in sorted_entries(&frames_dir)? {
                        if entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                            && let Some(frame) = read_frame_path(&entry.path())?
                        {
                            frames.push(frame);
                        }
                    }
                }
            }
        }
        frames.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(frames)
    }

    /// Reads a stored live frame. This also verifies retained generations.
    pub fn read_frame(
        &self,
        session: &str,
        stream: &str,
        generation: u64,
        epoch: u64,
        seq: u64,
    ) -> Result<Option<Frame>, ReceiveError> {
        read_frame_path(
            &epoch_dir(
                &generation_dir(&self.root, session, stream, generation),
                epoch,
            )
            .join("frames")
            .join(format!("{seq:020}.json")),
        )
    }

    pub fn read_orphan(
        &self,
        session: &str,
        stream: &str,
        generation: u64,
        fenced_epoch: u64,
        seq: u64,
    ) -> Result<Option<Frame>, ReceiveError> {
        read_frame_path(
            &orphan_dir(&self.root, session, stream, fenced_epoch, generation)
                .join("frames")
                .join(format!("{seq:020}.json")),
        )
    }

    fn validate_frame_mode(&self, frame: &Frame) -> Result<(), ReceiveError> {
        let mode = frame.mode();
        if mode == FrameMode::Plaintext && !self.allow_plaintext {
            return Err(ReceiveError::PlaintextDisabled);
        }
        if mode == FrameMode::Encrypted && !self.allow_encrypted {
            return Err(ReceiveError::EncryptedDisabled);
        }
        if mode == FrameMode::Encrypted && !envelope_path(&self.root, &frame.key.session).is_file()
        {
            return Err(ReceiveError::EnvelopeMissing {
                session: frame.key.session.clone(),
            });
        }
        ensure_session_mode(&self.root, &frame.key.session, mode)
    }
}

fn advance_receipt(receipt: &mut Receipt, frame: &Frame) -> Result<(), ReceiveError> {
    match &frame.content {
        FrameContent::Plaintext { chain, line } => {
            if receipt.mode == Some(FrameMode::Encrypted) {
                return Err(ReceiveError::MixedMode {
                    session: frame.key.session.clone(),
                    existing: FrameMode::Encrypted.as_str(),
                    incoming: FrameMode::Plaintext.as_str(),
                });
            }
            if *chain != chain_line(&receipt.chain, line) {
                return Err(ReceiveError::Chain { seq: frame.key.seq });
            }
            receipt.chain = *chain;
            receipt.tag = None;
            receipt.mode = Some(FrameMode::Plaintext);
        }
        FrameContent::Encrypted(payload) => {
            if receipt.mode == Some(FrameMode::Plaintext) {
                return Err(ReceiveError::MixedMode {
                    session: frame.key.session.clone(),
                    existing: FrameMode::Plaintext.as_str(),
                    incoming: FrameMode::Encrypted.as_str(),
                });
            }
            receipt.tag = Some(payload.tag);
            receipt.mode = Some(FrameMode::Encrypted);
        }
    }
    receipt.epoch = frame.key.epoch;
    receipt.acked = Some(frame.key.seq);
    Ok(())
}

fn validate_holder(row: &LeaseRow, machine: &str, epoch: u64) -> Result<(), ReceiveError> {
    if epoch < row.epoch {
        return Err(ReceiveError::Fenced {
            frame_epoch: epoch,
            current_epoch: row.epoch,
        });
    }
    if epoch > row.epoch {
        return Err(ReceiveError::FutureEpoch {
            frame_epoch: epoch,
            current_epoch: row.epoch,
        });
    }
    if machine != row.holder_machine {
        return Err(ReceiveError::NotHolder {
            session: row.session.clone(),
            machine: machine.to_owned(),
            current_epoch: row.epoch,
            holder_machine: row.holder_machine.clone(),
        });
    }
    Ok(())
}

/// Runs the blocking HTTP receiver until its process is stopped.
pub fn serve(listen: SocketAddr, root: &Path) -> Result<(), ReceiveError> {
    serve_configured(
        listen,
        root,
        ServeConfig {
            tls: None,
            insecure_plaintext: true,
        },
    )
}

pub fn serve_configured(
    listen: SocketAddr,
    root: &Path,
    config: ServeConfig,
) -> Result<(), ReceiveError> {
    validate_serve_config(listen, root, &config)?;
    let machines = load_machines(&root.join(MACHINES_FILE))?;
    let verifier =
        (!config.insecure_plaintext).then(|| Arc::new(Mutex::new(RequestVerifier::new(machines))));
    let receiver = Arc::new(Receiver::open_with_modes(
        root,
        Arc::new(SystemClock),
        config.insecure_plaintext,
        !config.insecure_plaintext,
    )?);
    let server = match &config.tls {
        Some(tls) => Server::https(
            listen,
            SslConfig {
                certificate: read_tls_file(&tls.certificate)?,
                private_key: read_tls_file(&tls.private_key)?,
            },
        ),
        None => Server::http(listen),
    }
    .map_err(|error| ReceiveError::HttpServer(error.to_string()))?;
    let server = Arc::new(server);
    eprintln!("semon-relay receiver listening on {listen}");
    for worker in 1..REQUEST_WORKERS {
        let server = Arc::clone(&server);
        let receiver = Arc::clone(&receiver);
        let verifier = verifier.clone();
        thread::Builder::new()
            .name(format!("semon-relay-http-{worker}"))
            .spawn(move || {
                if let Err(error) = request_loop(&server, &receiver, verifier.as_deref()) {
                    eprintln!("semon-relay receiver worker stopped: {error}");
                }
            })?;
    }
    request_loop(&server, &receiver, verifier.as_deref())
}

fn request_loop(
    server: &Server,
    receiver: &Receiver,
    verifier: Option<&Mutex<RequestVerifier>>,
) -> Result<(), ReceiveError> {
    loop {
        let request = server.recv().map_err(ReceiveError::Io)?;
        handle_http_request(request, receiver, verifier);
    }
}

fn handle_http_request(
    mut request: Request,
    receiver: &Receiver,
    verifier: Option<&Mutex<RequestVerifier>>,
) {
    if request.method() != &Method::Post {
        respond_text(request, 404, "not found");
        return;
    }
    let result = process_http_request(&mut request, receiver, verifier);
    match result {
        Ok(value) => respond_json(request, 200, &value),
        Err(error) => {
            let status = error_status(&error);
            respond_json(request, status, &error_value(&error));
        }
    }
}

fn process_http_request(
    request: &mut Request,
    receiver: &Receiver,
    verifier: Option<&Mutex<RequestVerifier>>,
) -> Result<Value, ReceiveError> {
    let route = request.url().to_owned();
    let signed_headers = if let Some(verifier) = verifier {
        let headers = request_headers(request)?;
        verifier
            .lock()
            .map_err(|_| ReceiveError::Poisoned)?
            .precheck(&headers, receiver.clock.now_ms())?;
        Some(headers)
    } else {
        None
    };
    let body = read_request_body(request)?;
    let value = serde_json::from_slice::<Value>(&body)?;
    if let (Some(verifier), Some(headers)) = (verifier, signed_headers) {
        let machine = value
            .as_object()
            .and_then(|object| object.get("machine"))
            .and_then(Value::as_str)
            .ok_or(ReceiveError::RequestField("machine"))?;
        verifier
            .lock()
            .map_err(|_| ReceiveError::Poisoned)?
            .verify(
                "POST",
                &route,
                &body,
                &headers,
                machine,
                receiver.clock.now_ms(),
            )?;
    }
    handle_request(receiver, &route, &value)
}

fn read_request_body(request: &mut Request) -> Result<Vec<u8>, ReceiveError> {
    let declared = request.body_length();
    read_bounded_body(request.as_reader(), declared)
}

fn read_bounded_body(
    reader: &mut dyn Read,
    declared: Option<usize>,
) -> Result<Vec<u8>, ReceiveError> {
    if declared.is_some_and(|length| length > MAX_REQUEST_BODY_BYTES) {
        return Err(ReceiveError::BodyTooLarge {
            limit: MAX_REQUEST_BODY_BYTES,
        });
    }
    let mut body = Vec::with_capacity(declared.unwrap_or(0).min(MAX_REQUEST_BODY_BYTES));
    reader
        .take((MAX_REQUEST_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ReceiveError::BodyTooLarge {
            limit: MAX_REQUEST_BODY_BYTES,
        });
    }
    Ok(body)
}

pub fn validate_serve_config(
    listen: SocketAddr,
    root: &Path,
    config: &ServeConfig,
) -> Result<(), ReceiveError> {
    if listen.ip().is_loopback() {
        return Ok(());
    }
    if config.insecure_plaintext {
        return Err(ReceiveError::NonLoopbackPlaintext);
    }
    if config.tls.is_none() {
        return Err(ReceiveError::NonLoopbackTls);
    }
    if load_machines(&root.join(MACHINES_FILE))?.is_empty() {
        return Err(ReceiveError::NonLoopbackMachines(root.join(MACHINES_FILE)));
    }
    Ok(())
}

pub fn validate_loopback(listen: SocketAddr) -> Result<(), ReceiveError> {
    if listen.ip().is_loopback() {
        Ok(())
    } else {
        Err(ReceiveError::NonLoopback(listen))
    }
}

fn read_tls_file(path: &Path) -> Result<Vec<u8>, ReceiveError> {
    fs::read(path).map_err(|source| ReceiveError::TlsFile {
        path: path.to_path_buf(),
        source,
    })
}

fn request_headers(request: &tiny_http::Request) -> Result<SignedHeaders, ReceiveError> {
    let value = |name: &'static str| {
        request
            .headers()
            .iter()
            .find(|header| header.field.equiv(name))
            .map(|header| header.value.as_str().to_owned())
            .ok_or(ReceiveError::AuthHeader(name))
    };
    Ok(SignedHeaders {
        machine: value("X-Semon-Machine")?,
        timestamp: value("X-Semon-Timestamp")?
            .parse()
            .map_err(|_| ReceiveError::AuthHeader("X-Semon-Timestamp"))?,
        nonce: value("X-Semon-Nonce")?,
        signature: value("X-Semon-Signature")?,
    })
}

fn handle_request(receiver: &Receiver, route: &str, value: &Value) -> Result<Value, ReceiveError> {
    match route {
        "/v1/frames" => {
            let frame = Frame::from_value(value)?;
            receiver.accept(&frame)?;
            Ok(Value::Object(Map::from_iter([(
                "acked".into(),
                frame.key.seq.into(),
            )])))
        }
        "/v1/orphans" => {
            let frame = Frame::from_value(value)?;
            receiver.accept_orphan(&frame)?;
            Ok(Value::Object(Map::from_iter([(
                "acked".into(),
                frame.key.seq.into(),
            )])))
        }
        "/v1/lease/acquire" => {
            let object = request_object(value)?;
            Ok(receiver
                .acquire(
                    required_string(object, "session")?,
                    required_string(object, "machine")?,
                )?
                .to_value())
        }
        "/v1/lease/renew" => {
            let object = request_object(value)?;
            Ok(receiver
                .renew(
                    required_string(object, "session")?,
                    required_string(object, "machine")?,
                    required_integer(object, "epoch")?,
                )?
                .to_value())
        }
        "/v1/lease/status" => {
            let object = request_object(value)?;
            let _machine = required_string(object, "machine")?;
            let session = optional_string(object, "session")?;
            Ok(receiver.lease_status(session)?.to_value())
        }
        "/v1/lease/tips" => {
            let object = request_object(value)?;
            let _machine = required_string(object, "machine")?;
            Ok(receiver
                .lease_tips(required_string(object, "session")?)?
                .to_value())
        }
        "/v1/lease/takeover" => {
            let object = request_object(value)?;
            Ok(receiver
                .takeover(
                    required_string(object, "session")?,
                    required_string(object, "machine")?,
                    required_integer(object, "expected_epoch")?,
                    required_bool(object, "force")?,
                )?
                .to_value())
        }
        "/v1/orphans/list" => {
            let object = request_object(value)?;
            let _machine = required_string(object, "machine")?;
            Ok(Value::Array(
                receiver
                    .list_orphans()?
                    .iter()
                    .map(OrphanSummary::to_value)
                    .collect(),
            ))
        }
        "/v1/frames/list" => {
            let object = request_object(value)?;
            let _machine = required_string(object, "machine")?;
            Ok(Value::Array(
                receiver
                    .list_frames(required_string(object, "session")?)?
                    .iter()
                    .map(Frame::to_value)
                    .collect(),
            ))
        }
        "/v1/keys/list" => {
            let object = request_object(value)?;
            let _machine = required_string(object, "machine")?;
            Ok(Value::Array(
                receiver
                    .list_envelope_sessions()?
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ))
        }
        route if route.starts_with("/v1/keys/") && route.ends_with("/get") => {
            let object = request_object(value)?;
            let _machine = required_string(object, "machine")?;
            let session = required_string(object, "session")?;
            validate_route_session(route, "/v1/keys/", "/get", session)?;
            Ok(Value::Object(Map::from_iter([(
                "envelope".into(),
                receiver
                    .get_envelope(session)?
                    .map_or(Value::Null, |envelope| Value::String(hex::encode(envelope))),
            )])))
        }
        route if route.starts_with("/v1/keys/") => {
            let object = request_object(value)?;
            let session = required_string(object, "session")?;
            validate_route_session(route, "/v1/keys/", "", session)?;
            let outcome = receiver.put_envelope(
                session,
                required_string(object, "machine")?,
                &required_hex(object, "envelope")?,
                required_bool(object, "replace")?,
                required_bool(object, "force")?,
            )?;
            Ok(Value::Object(Map::from_iter([(
                "outcome".into(),
                Value::String(
                    match outcome {
                        EnvelopeOutcome::Stored => "stored",
                        EnvelopeOutcome::Duplicate => "duplicate",
                        EnvelopeOutcome::Replaced => "replaced",
                    }
                    .into(),
                ),
            )])))
        }
        _ => Err(ReceiveError::RequestField("route")),
    }
}

fn request_object(value: &Value) -> Result<&Map<String, Value>, ReceiveError> {
    value.as_object().ok_or(ReceiveError::RequestField("body"))
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, ReceiveError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or(ReceiveError::RequestField(field))
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<Option<&'a str>, ReceiveError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or(ReceiveError::RequestField(field)),
    }
}

fn required_integer(object: &Map<String, Value>, field: &'static str) -> Result<u64, ReceiveError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(ReceiveError::RequestField(field))
}

fn required_bool(object: &Map<String, Value>, field: &'static str) -> Result<bool, ReceiveError> {
    object
        .get(field)
        .and_then(Value::as_bool)
        .ok_or(ReceiveError::RequestField(field))
}

fn required_hex(object: &Map<String, Value>, field: &'static str) -> Result<Vec<u8>, ReceiveError> {
    let value = required_string(object, field)?;
    hex::decode(value).map_err(|_| ReceiveError::RequestField(field))
}

fn validate_route_session(
    route: &str,
    prefix: &str,
    suffix: &str,
    session: &str,
) -> Result<(), ReceiveError> {
    let encoded = route
        .strip_prefix(prefix)
        .and_then(|route| route.strip_suffix(suffix))
        .ok_or(ReceiveError::RequestField("route"))?;
    if encoded == hex::encode(session.as_bytes()) {
        Ok(())
    } else {
        Err(ReceiveError::RequestField("session"))
    }
}

fn error_status(error: &ReceiveError) -> u16 {
    match error {
        ReceiveError::Auth(AuthError::ReplayCacheFull) => 503,
        ReceiveError::BodyTooLarge { .. } => 413,
        ReceiveError::LeaseActive { .. }
        | ReceiveError::TakeoverConflict { .. }
        | ReceiveError::Fenced { .. }
        | ReceiveError::FutureEpoch { .. }
        | ReceiveError::NotHolder { .. }
        | ReceiveError::OrphanMachine { .. }
        | ReceiveError::Collision
        | ReceiveError::EnvelopeConflict { .. }
        | ReceiveError::RewrapNotHolder { .. }
        | ReceiveError::MixedMode { .. }
        | ReceiveError::EnvelopeMissing { .. }
        | ReceiveError::Gap { .. }
        | ReceiveError::Chain { .. } => 409,
        ReceiveError::LeaseMissing { .. }
        | ReceiveError::Frame(_)
        | ReceiveError::Json(_)
        | ReceiveError::PlaintextDisabled
        | ReceiveError::EncryptedDisabled
        | ReceiveError::RequestField(_) => 400,
        ReceiveError::Auth(_) | ReceiveError::AuthHeader(_) => 401,
        _ => 500,
    }
}

fn error_value(error: &ReceiveError) -> Value {
    let (code, current_epoch) = match error {
        ReceiveError::Auth(AuthError::ReplayCacheFull) => ("replay_cache_full", None),
        ReceiveError::BodyTooLarge { .. } => ("body_too_large", None),
        ReceiveError::Fenced { current_epoch, .. } => ("fenced", Some(*current_epoch)),
        ReceiveError::NotHolder { current_epoch, .. } => ("not_holder", Some(*current_epoch)),
        ReceiveError::FutureEpoch { current_epoch, .. } => ("future_epoch", Some(*current_epoch)),
        ReceiveError::TakeoverConflict { current_epoch, .. } => {
            ("takeover_conflict", Some(*current_epoch))
        }
        ReceiveError::LeaseActive { .. } => ("lease_active", None),
        ReceiveError::LeaseMissing { .. } => ("lease_missing", None),
        ReceiveError::Collision => ("collision", None),
        ReceiveError::Gap { .. } => ("gap", None),
        ReceiveError::Chain { .. } => ("chain", None),
        _ => ("receiver", None),
    };
    let mut object = Map::from_iter([
        ("error".into(), Value::String(code.into())),
        ("message".into(), Value::String(error.to_string())),
    ]);
    if let Some(epoch) = current_epoch {
        object.insert("current_epoch".into(), epoch.into());
    }
    Value::Object(object)
}

fn generation_dir(root: &Path, session: &str, stream: &str, generation: u64) -> PathBuf {
    root.join(hex::encode(session.as_bytes()))
        .join(hex::encode(stream.as_bytes()))
        .join(format!("generation-{generation}"))
}

fn epoch_dir(generation_dir: &Path, epoch: u64) -> PathBuf {
    generation_dir.join(format!("epoch-{epoch}"))
}

fn orphan_dir(root: &Path, session: &str, stream: &str, epoch: u64, generation: u64) -> PathBuf {
    root.join("orphans")
        .join(hex::encode(session.as_bytes()))
        .join(hex::encode(stream.as_bytes()))
        .join(format!("epoch-{epoch}"))
        .join(format!("generation-{generation}"))
}

fn read_existing(path: &Path, frame: &Frame) -> Result<Option<Frame>, ReceiveError> {
    let Some(stored) = read_frame_path(path)? else {
        return Ok(None);
    };
    if stored.key != frame.key {
        return Err(ReceiveError::Inconsistent(format!(
            "stored key at {} does not match its path",
            path.display()
        )));
    }
    // Observation timestamps and encrypted nonces can change on a resend. The
    // plaintext line and chain, or the keyed content tag, define idempotence.
    let same_content = match (&stored.content, &frame.content) {
        (
            FrameContent::Plaintext {
                chain: stored_chain,
                line: stored_line,
            },
            FrameContent::Plaintext { chain, line },
        ) => stored_chain == chain && stored_line == line,
        (FrameContent::Encrypted(stored), FrameContent::Encrypted(incoming)) => {
            stored.tag == incoming.tag
        }
        _ => false,
    };
    if !same_content || (!stored.machine.is_empty() && stored.machine != frame.machine) {
        return Err(ReceiveError::Collision);
    }
    Ok(Some(stored))
}

fn read_frame_path(path: &Path) -> Result<Option<Frame>, ReceiveError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(Frame::from_stored_value(&serde_json::from_slice(
            &bytes,
        )?)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn store_frame(path: &Path, frame: &Frame) -> Result<(), ReceiveError> {
    let parent = path.parent().expect("frame path has a parent");
    fs::create_dir_all(parent)?;
    set_private_dir(parent)?;
    write_atomic(path, &serde_json::to_vec(&frame.to_value())?)?;
    Ok(())
}

fn reconcile_receipt(
    generation_dir: &Path,
    session: &str,
    stream: &str,
    generation: u64,
) -> Result<Receipt, ReceiveError> {
    let mut receipt = load_receipt(&generation_dir.join("state.json"))?;
    if let Some(acked) = receipt.acked {
        let path = epoch_dir(generation_dir, receipt.epoch)
            .join("frames")
            .join(format!("{acked:020}.json"));
        let stored = read_frame_path(&path)?.ok_or_else(|| {
            ReceiveError::Inconsistent(format!(
                "receipt references missing frame {}",
                path.display()
            ))
        })?;
        if stored.key.session != session
            || stored.key.stream != stream
            || stored.key.generation != generation
            || stored.key.epoch != receipt.epoch
            || stored.key.seq != acked
            || stored.mode() != receipt.mode.unwrap_or(stored.mode())
            || match &stored.content {
                FrameContent::Plaintext { chain, .. } => *chain != receipt.chain,
                FrameContent::Encrypted(payload) => Some(payload.tag) != receipt.tag,
            }
        {
            return Err(ReceiveError::Inconsistent(format!(
                "receipt does not match {}",
                path.display()
            )));
        }
    }
    let mut frames = Vec::new();
    if !generation_dir.exists() {
        return Ok(receipt);
    }
    for epoch_entry in sorted_entries(generation_dir)? {
        let Some(epoch) = parse_number(&epoch_entry.file_name(), "epoch-") else {
            continue;
        };
        let frames_dir = epoch_entry.path().join("frames");
        if !frames_dir.is_dir() {
            continue;
        }
        for frame_entry in sorted_entries(&frames_dir)? {
            let path = frame_entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let frame = Frame::from_stored_value(&serde_json::from_slice(&fs::read(&path)?)?)?;
            if frame.key.session != session
                || frame.key.stream != stream
                || frame.key.generation != generation
                || frame.key.epoch != epoch
            {
                return Err(ReceiveError::Inconsistent(format!(
                    "stored key at {} does not match its path",
                    path.display()
                )));
            }
            if receipt.acked.is_some_and(|acked| frame.key.seq <= acked) {
                continue;
            }
            frames.push((frame.key.seq, epoch, path, frame));
        }
    }
    frames.sort_by_key(|(seq, epoch, _, _)| (*seq, *epoch));
    for (seq, epoch, path, frame) in frames {
        let expected = receipt.acked.map_or(0, |value| value.saturating_add(1));
        if seq != expected || (receipt.acked.is_some() && epoch < receipt.epoch) {
            return Err(ReceiveError::Inconsistent(format!(
                "non-contiguous live history at {}",
                path.display()
            )));
        }
        advance_receipt(&mut receipt, &frame).map_err(|error| {
            ReceiveError::Inconsistent(format!("invalid stored frame at sequence {seq}: {error}"))
        })?;
    }
    Ok(receipt)
}

fn load_receipt(path: &Path) -> Result<Receipt, ReceiveError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Receipt {
                epoch: 0,
                acked: None,
                mode: None,
                chain: ZERO_CHAIN,
                tag: None,
            });
        }
        Err(error) => return Err(error.into()),
    };
    let value: Value = serde_json::from_slice(&bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| ReceiveError::Inconsistent("receipt is not an object".into()))?;
    let epoch = object
        .get("epoch")
        .and_then(Value::as_u64)
        .ok_or_else(|| ReceiveError::Inconsistent("receipt epoch is missing".into()))?;
    let acked = object
        .get("acked")
        .and_then(Value::as_u64)
        .ok_or_else(|| ReceiveError::Inconsistent("receipt acked is missing".into()))?;
    let mode = match object.get("mode").and_then(Value::as_str) {
        None | Some("plaintext") => Some(FrameMode::Plaintext),
        Some("encrypted") => Some(FrameMode::Encrypted),
        Some(mode) => {
            return Err(ReceiveError::Inconsistent(format!(
                "unsupported receipt mode {mode}"
            )));
        }
    };
    let chain = match object.get("chain") {
        None | Some(Value::Null) if mode == Some(FrameMode::Encrypted) => ZERO_CHAIN,
        Some(Value::String(encoded)) => {
            let bytes = hex::decode(encoded).map_err(|error| {
                ReceiveError::Inconsistent(format!("invalid receipt chain: {error}"))
            })?;
            let length = bytes.len();
            bytes.try_into().map_err(|_| {
                ReceiveError::Inconsistent(format!(
                    "receipt chain has {length} bytes instead of 32"
                ))
            })?
        }
        _ => {
            return Err(ReceiveError::Inconsistent(
                "receipt chain is missing".into(),
            ));
        }
    };
    let tag = match object.get("tag") {
        None | Some(Value::Null) => None,
        Some(Value::String(encoded)) => {
            let bytes = hex::decode(encoded).map_err(|error| {
                ReceiveError::Inconsistent(format!("invalid receipt tag: {error}"))
            })?;
            let length = bytes.len();
            Some(bytes.try_into().map_err(|_| {
                ReceiveError::Inconsistent(format!("receipt tag has {length} bytes instead of 32"))
            })?)
        }
        _ => return Err(ReceiveError::Inconsistent("receipt tag is invalid".into())),
    };
    Ok(Receipt {
        epoch,
        acked: Some(acked),
        mode,
        chain,
        tag,
    })
}

fn save_receipt(path: &Path, receipt: Receipt) -> Result<(), ReceiveError> {
    let acked = receipt
        .acked
        .ok_or_else(|| ReceiveError::Inconsistent("cannot save an empty receipt".into()))?;
    let value = Value::Object(Map::from_iter([
        ("acked".into(), acked.into()),
        (
            "chain".into(),
            if receipt.mode == Some(FrameMode::Plaintext) {
                Value::String(hex::encode(receipt.chain))
            } else {
                Value::Null
            },
        ),
        ("epoch".into(), receipt.epoch.into()),
        (
            "mode".into(),
            Value::String(receipt.mode.unwrap_or(FrameMode::Plaintext).as_str().into()),
        ),
        (
            "tag".into(),
            receipt
                .tag
                .map_or(Value::Null, |tag| Value::String(hex::encode(tag))),
        ),
    ]));
    write_atomic(path, &serde_json::to_vec(&value)?)?;
    Ok(())
}

fn stream_tips(root: &Path, session: &str) -> Result<Vec<StreamTip>, ReceiveError> {
    let session_dir = root.join(hex::encode(session.as_bytes()));
    if !session_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut tips = Vec::new();
    for stream_entry in sorted_entries(&session_dir)? {
        let stream = decode_component(&stream_entry.file_name())?;
        for generation_entry in sorted_entries(&stream_entry.path())? {
            let Some(generation) = parse_number(&generation_entry.file_name(), "generation-")
            else {
                continue;
            };
            let receipt =
                reconcile_receipt(&generation_entry.path(), session, &stream, generation)?;
            if let Some(seq) = receipt.acked {
                tips.push(StreamTip {
                    stream: stream.clone(),
                    generation,
                    epoch: receipt.epoch,
                    seq,
                    mode: receipt.mode.unwrap_or(FrameMode::Plaintext),
                    chain: (receipt.mode != Some(FrameMode::Encrypted)).then_some(receipt.chain),
                    tag: receipt.tag,
                });
            }
        }
    }
    Ok(tips)
}

fn session_mode_path(root: &Path, session: &str) -> PathBuf {
    root.join(SESSION_MODES_DIR)
        .join(hex::encode(session.as_bytes()))
}

fn envelope_path(root: &Path, session: &str) -> PathBuf {
    root.join(ENVELOPES_DIR)
        .join(format!("{}.age", hex::encode(session.as_bytes())))
}

fn ensure_session_mode(
    root: &Path,
    session: &str,
    incoming: FrameMode,
) -> Result<(), ReceiveError> {
    let path = session_mode_path(root, session);
    match fs::read_to_string(&path) {
        Ok(value) => {
            let existing = match value.trim() {
                "plaintext" => FrameMode::Plaintext,
                "encrypted" => FrameMode::Encrypted,
                value => {
                    return Err(ReceiveError::Inconsistent(format!(
                        "unsupported session mode {value} in {}",
                        path.display()
                    )));
                }
            };
            if existing == incoming {
                Ok(())
            } else {
                Err(ReceiveError::MixedMode {
                    session: session.to_owned(),
                    existing: existing.as_str(),
                    incoming: incoming.as_str(),
                })
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_atomic(&path, format!("{}\n", incoming.as_str()).as_bytes())?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn list_orphans(root: &Path) -> Result<Vec<OrphanSummary>, ReceiveError> {
    let orphan_root = root.join("orphans");
    if !orphan_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    for session_entry in sorted_entries(&orphan_root)? {
        let session = decode_component(&session_entry.file_name())?;
        for stream_entry in sorted_entries(&session_entry.path())? {
            let stream = decode_component(&stream_entry.file_name())?;
            for epoch_entry in sorted_entries(&stream_entry.path())? {
                let Some(fenced_epoch) = parse_number(&epoch_entry.file_name(), "epoch-") else {
                    continue;
                };
                for generation_entry in sorted_entries(&epoch_entry.path())? {
                    let Some(generation) =
                        parse_number(&generation_entry.file_name(), "generation-")
                    else {
                        continue;
                    };
                    let frames_dir = generation_entry.path().join("frames");
                    if !frames_dir.is_dir() {
                        continue;
                    }
                    let sequences = sorted_entries(&frames_dir)?
                        .into_iter()
                        .filter_map(|entry| {
                            entry
                                .path()
                                .file_stem()
                                .and_then(|value| value.to_str())
                                .and_then(|value| value.parse::<u64>().ok())
                        })
                        .collect::<BTreeSet<_>>();
                    if let (Some(first_seq), Some(last_seq)) =
                        (sequences.first().copied(), sequences.last().copied())
                    {
                        summaries.push(OrphanSummary {
                            session: session.clone(),
                            stream: stream.clone(),
                            generation,
                            fenced_epoch,
                            frames: sequences.len() as u64,
                            first_seq,
                            last_seq,
                        });
                    }
                }
            }
        }
    }
    Ok(summaries)
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, ReceiveError> {
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn parse_number(value: &std::ffi::OsStr, prefix: &str) -> Option<u64> {
    value.to_str()?.strip_prefix(prefix)?.parse().ok()
}

fn decode_component(value: &std::ffi::OsStr) -> Result<String, ReceiveError> {
    let value = value
        .to_str()
        .ok_or_else(|| ReceiveError::Inconsistent("non-UTF-8 storage component".into()))?;
    let bytes = hex::decode(value).map_err(|error| {
        ReceiveError::Inconsistent(format!("invalid storage component: {error}"))
    })?;
    String::from_utf8(bytes)
        .map_err(|error| ReceiveError::Inconsistent(format!("invalid storage identifier: {error}")))
}

fn load_rows(path: &Path) -> Result<BTreeMap<String, LeaseRow>, ReceiveError> {
    match fs::read(path) {
        Ok(bytes) => Ok(rows_from_value(&serde_json::from_slice(&bytes)?)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn save_rows(path: &Path, rows: &BTreeMap<String, LeaseRow>) -> Result<(), ReceiveError> {
    write_atomic(path, &serde_json::to_vec(&rows_to_value(rows))?)?;
    Ok(())
}

fn load_takeovers(path: &Path) -> Result<Vec<TakeoverRecord>, ReceiveError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let value = serde_json::from_slice(line)?;
            Ok(TakeoverRecord::from_value(&value)?)
        })
        .collect()
}

fn append_takeover(path: &Path, record: &TakeoverRecord) -> Result<(), ReceiveError> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, &record.to_value())?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn append_rewrap(
    path: &Path,
    session: &str,
    machine: &str,
    forced: bool,
    previous: &[u8],
    replacement: &[u8],
    replaced_at_ms: u64,
) -> Result<(), ReceiveError> {
    let value = Value::Object(Map::from_iter([
        ("forced".into(), forced.into()),
        ("machine".into(), Value::String(machine.to_owned())),
        (
            "previous_sha256".into(),
            Value::String(hex::encode(Sha256::digest(previous))),
        ),
        ("replaced_at_ms".into(), replaced_at_ms.into()),
        (
            "replacement_sha256".into(),
            Value::String(hex::encode(Sha256::digest(replacement))),
        ),
        ("session".into(), Value::String(session.to_owned())),
    ]));
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, &value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn respond_json(request: tiny_http::Request, status: u16, body: &Value) {
    respond_text(request, status, &body.to_string());
}

fn respond_text(request: tiny_http::Request, status: u16, body: &str) {
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("content-type", "application/json") {
        response.add_header(header);
    }
    if let Err(error) = request.respond(response) {
        eprintln!("semon-relay could not answer receiver request: {error}");
    }
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_reader_accepts_a_request_at_the_64_mib_limit() {
        let mut reader = io::repeat(0).take(MAX_REQUEST_BODY_BYTES as u64);
        let body = read_bounded_body(&mut reader, Some(MAX_REQUEST_BODY_BYTES)).unwrap();
        assert_eq!(body.len(), MAX_REQUEST_BODY_BYTES);
    }

    #[test]
    fn replay_cache_capacity_is_a_retryable_service_error() {
        let error = ReceiveError::Auth(AuthError::ReplayCacheFull);
        assert_eq!(error_status(&error), 503);
        assert_eq!(
            error_value(&error).get("error").and_then(Value::as_str),
            Some("replay_cache_full")
        );
    }
}
