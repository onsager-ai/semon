use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::{Map, Value};
use thiserror::Error;
use tiny_http::{Header, Method, Response, Server, StatusCode};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::{
    Clock, Frame, LEASE_DURATION_MS, LeaseRow, LeaseStatus, OrphanSummary, StreamTip, SystemClock,
    TakeoverRecord, TakeoverResult, ZERO_CHAIN, chain_line,
    lease::{LeaseValueError, rows_from_value, rows_to_value},
    protocol::FrameError,
    state::{StateError, write_atomic},
};

const REGISTER_FILE: &str = "lease-register.json";
const TAKEOVER_LOG: &str = "takeovers.jsonl";
const RECEIPT_CHECKPOINT_INTERVAL: u64 = 256;

/// Whether an accepted frame was newly stored or already present unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveOutcome {
    Stored,
    Duplicate,
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
    #[error("cannot start HTTP receiver: {0}")]
    HttpServer(String),
    #[error("receiver state lock was poisoned")]
    Poisoned,
    #[error("request field {0} is missing or has the wrong type")]
    RequestField(&'static str),
}

#[derive(Clone, Copy, Debug)]
struct Receipt {
    epoch: u64,
    acked: Option<u64>,
    chain: [u8; 32],
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
}

impl Receiver {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ReceiveError> {
        Self::open_with_clock(root, Arc::new(SystemClock))
    }

    /// Opens a receiver with an injected clock for deterministic lease tests.
    pub fn open_with_clock(
        root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
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
        if accepted.chain != chain_line(&receipt.chain, &accepted.line) {
            return Err(ReceiveError::Chain { seq: frame.key.seq });
        }
        if existing.is_none() {
            store_frame(&frame_path, frame)?;
        }
        receipt.epoch = frame.key.epoch;
        receipt.acked = Some(frame.key.seq);
        receipt.chain = accepted.chain;
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
    validate_loopback(listen)?;
    let receiver = Receiver::open(root)?;
    let server =
        Server::http(listen).map_err(|error| ReceiveError::HttpServer(error.to_string()))?;
    eprintln!("semon-relay receiver listening on {listen}");
    for mut request in server.incoming_requests() {
        let route = request.url().to_owned();
        if request.method() != &Method::Post {
            respond_text(request, 404, "not found");
            continue;
        }
        let mut body = Vec::new();
        let result = request
            .as_reader()
            .read_to_end(&mut body)
            .map_err(ReceiveError::Io)
            .and_then(|_| serde_json::from_slice(&body).map_err(ReceiveError::Json))
            .and_then(|value| handle_request(&receiver, &route, &value));
        match result {
            Ok(value) => respond_json(request, 200, &value),
            Err(error) => {
                let status = error_status(&error);
                respond_json(request, status, &error_value(&error));
            }
        }
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

fn error_status(error: &ReceiveError) -> u16 {
    match error {
        ReceiveError::LeaseActive { .. }
        | ReceiveError::TakeoverConflict { .. }
        | ReceiveError::Fenced { .. }
        | ReceiveError::FutureEpoch { .. }
        | ReceiveError::NotHolder { .. }
        | ReceiveError::OrphanMachine { .. }
        | ReceiveError::Collision
        | ReceiveError::Gap { .. }
        | ReceiveError::Chain { .. } => 409,
        ReceiveError::LeaseMissing { .. }
        | ReceiveError::Frame(_)
        | ReceiveError::Json(_)
        | ReceiveError::RequestField(_) => 400,
        _ => 500,
    }
}

fn error_value(error: &ReceiveError) -> Value {
    let (code, current_epoch) = match error {
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
    // Observation timestamps can change after a sender process restart. The
    // source bytes and their derived chain are the idempotent payload.
    if stored.line != frame.line
        || stored.chain != frame.chain
        || (!stored.machine.is_empty() && stored.machine != frame.machine)
    {
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
            || stored.chain != receipt.chain
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
        if frame.chain != chain_line(&receipt.chain, &frame.line) {
            return Err(ReceiveError::Inconsistent(format!(
                "stored chain is broken at sequence {seq}"
            )));
        }
        receipt.epoch = epoch;
        receipt.acked = Some(seq);
        receipt.chain = frame.chain;
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
                chain: ZERO_CHAIN,
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
    let encoded = object
        .get("chain")
        .and_then(Value::as_str)
        .ok_or_else(|| ReceiveError::Inconsistent("receipt chain is missing".into()))?;
    let chain = hex::decode(encoded)
        .map_err(|error| ReceiveError::Inconsistent(format!("invalid receipt chain: {error}")))?;
    let length = chain.len();
    Ok(Receipt {
        epoch,
        acked: Some(acked),
        chain: chain.try_into().map_err(|_| {
            ReceiveError::Inconsistent(format!("receipt chain has {length} bytes instead of 32"))
        })?,
    })
}

fn save_receipt(path: &Path, receipt: Receipt) -> Result<(), ReceiveError> {
    let acked = receipt
        .acked
        .ok_or_else(|| ReceiveError::Inconsistent("cannot save an empty receipt".into()))?;
    let value = Value::Object(Map::from_iter([
        ("acked".into(), acked.into()),
        ("chain".into(), Value::String(hex::encode(receipt.chain))),
        ("epoch".into(), receipt.epoch.into()),
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
                    chain: receipt.chain,
                });
            }
        }
    }
    Ok(tips)
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
