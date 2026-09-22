use std::{
    collections::BTreeMap,
    fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::{Map, Value};
use thiserror::Error;
use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::{
    Frame, ZERO_CHAIN, chain_line,
    protocol::FrameError,
    state::{StateError, write_atomic},
};

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
    #[error("receiver only accepts epoch 0 in M1, got {0}")]
    Epoch(u64),
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
    #[error("invalid receiver state: {0}")]
    State(#[from] StateError),
    #[error("refusing non-loopback listen address {0}")]
    NonLoopback(SocketAddr),
    #[error("cannot start HTTP receiver: {0}")]
    HttpServer(String),
    #[error("receiver state lock was poisoned")]
    Poisoned,
}

#[derive(Clone, Copy, Debug)]
struct Receipt {
    acked: Option<u64>,
    chain: [u8; 32],
}

/// Disk-backed receiver for framed source lines.
pub struct Receiver {
    root: PathBuf,
    receipts: Mutex<BTreeMap<(String, String, u64, u64), Receipt>>,
}

impl Receiver {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ReceiveError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        set_private_dir(&root)?;
        Ok(Self {
            root,
            receipts: Mutex::new(BTreeMap::new()),
        })
    }

    /// Accepts exactly the next contiguous frame, or an identical duplicate.
    pub fn accept(&self, frame: &Frame) -> Result<ReceiveOutcome, ReceiveError> {
        if frame.key.epoch != 0 {
            return Err(ReceiveError::Epoch(frame.key.epoch));
        }
        let epoch_dir = self.epoch_dir(frame);
        let frame_path = epoch_dir
            .join("frames")
            .join(format!("{:020}.json", frame.key.seq));
        let existing = match fs::read(&frame_path) {
            Ok(bytes) => {
                let stored = Frame::from_value(&serde_json::from_slice(&bytes)?)?;
                if stored.key != frame.key {
                    return Err(ReceiveError::Inconsistent(format!(
                        "stored key at {} does not match its path",
                        frame_path.display()
                    )));
                }
                if stored.line != frame.line {
                    return Err(ReceiveError::Collision);
                }
                Some(stored)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };

        let receipt_key = (
            frame.key.session.clone(),
            frame.key.stream.clone(),
            frame.key.generation,
            frame.key.epoch,
        );
        let mut receipts = self.receipts.lock().map_err(|_| ReceiveError::Poisoned)?;
        let mut receipt = match receipts.get(&receipt_key) {
            Some(receipt) => *receipt,
            None => reconcile_receipt(&epoch_dir, frame)?,
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

        let accepted = existing.as_ref().unwrap_or(frame);
        let expected_chain = chain_line(&receipt.chain, &accepted.line);
        if accepted.chain != expected_chain {
            return Err(ReceiveError::Chain { seq: frame.key.seq });
        }
        if existing.is_none() {
            fs::create_dir_all(frame_path.parent().expect("frame path has a parent"))?;
            set_private_dir(&epoch_dir)?;
            set_private_dir(frame_path.parent().expect("frame path has a parent"))?;
            write_atomic(&frame_path, &serde_json::to_vec(&frame.to_value())?)?;
        }
        receipt.acked = Some(frame.key.seq);
        receipt.chain = accepted.chain;
        receipts.insert(receipt_key, receipt);
        // Frame files are synced before an ack. The compact receipt is only an
        // acceleration index: after a crash, reconcile_receipt advances it
        // from those authoritative files. Periodic checkpoints avoid a second
        // fsync per frame without weakening acknowledgement durability.
        if frame.key.seq % RECEIPT_CHECKPOINT_INTERVAL == RECEIPT_CHECKPOINT_INTERVAL - 1 {
            save_receipt(&epoch_dir.join("state.json"), receipt)?;
        }
        Ok(if existing.is_some() {
            ReceiveOutcome::Duplicate
        } else {
            ReceiveOutcome::Stored
        })
    }

    /// Reads a stored frame. This is also useful to verify retained generations.
    pub fn read_frame(
        &self,
        session: &str,
        stream: &str,
        generation: u64,
        epoch: u64,
        seq: u64,
    ) -> Result<Option<Frame>, ReceiveError> {
        let path = key_dir(&self.root, session, stream, generation, epoch)
            .join("frames")
            .join(format!("{seq:020}.json"));
        match fs::read(path) {
            Ok(bytes) => Ok(Some(Frame::from_value(&serde_json::from_slice(&bytes)?)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn epoch_dir(&self, frame: &Frame) -> PathBuf {
        key_dir(
            &self.root,
            &frame.key.session,
            &frame.key.stream,
            frame.key.generation,
            frame.key.epoch,
        )
    }
}

/// Runs the blocking HTTP receiver until its process is stopped.
pub fn serve(listen: SocketAddr, root: &Path) -> Result<(), ReceiveError> {
    validate_loopback(listen)?;
    let receiver = Receiver::open(root)?;
    let server =
        Server::http(listen).map_err(|error| ReceiveError::HttpServer(error.to_string()))?;
    eprintln!("semon-relay receiver listening on {listen}");
    for mut request in server.incoming_requests() {
        if request.method() != &Method::Post || request.url() != "/v1/frames" {
            respond(request, 404, "not found");
            continue;
        }
        let mut body = Vec::new();
        let result = request
            .as_reader()
            .read_to_end(&mut body)
            .map_err(ReceiveError::Io)
            .and_then(|_| serde_json::from_slice(&body).map_err(ReceiveError::Json))
            .and_then(|value| Frame::from_value(&value).map_err(ReceiveError::Frame))
            .and_then(|frame| receiver.accept(&frame).map(|_| frame.key.seq));
        match result {
            Ok(seq) => {
                let body = serde_json::to_string(&Value::Object(Map::from_iter([(
                    "acked".into(),
                    Value::Number(seq.into()),
                )])))
                .expect("ack JSON is serializable");
                respond(request, 200, &body);
            }
            Err(error) => {
                let status = match error {
                    ReceiveError::Collision
                    | ReceiveError::Gap { .. }
                    | ReceiveError::Chain { .. }
                    | ReceiveError::Epoch(_) => 409,
                    ReceiveError::Frame(_) | ReceiveError::Json(_) => 400,
                    _ => 500,
                };
                respond(request, status, &error.to_string());
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

fn key_dir(root: &Path, session: &str, stream: &str, generation: u64, epoch: u64) -> PathBuf {
    // Hex encoding keeps untrusted protocol identifiers from becoming paths.
    root.join(hex::encode(session.as_bytes()))
        .join(hex::encode(stream.as_bytes()))
        .join(format!("generation-{generation}"))
        .join(format!("epoch-{epoch}"))
}

fn load_receipt(path: &Path) -> Result<Receipt, ReceiveError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Receipt {
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
        acked: Some(acked),
        chain: chain.try_into().map_err(|_| {
            ReceiveError::Inconsistent(format!("receipt chain has {length} bytes instead of 32"))
        })?,
    })
}

fn reconcile_receipt(epoch_dir: &Path, incoming: &Frame) -> Result<Receipt, ReceiveError> {
    let state_path = epoch_dir.join("state.json");
    let mut receipt = load_receipt(&state_path)?;
    if let Some(acked) = receipt.acked {
        let path = epoch_dir.join("frames").join(format!("{acked:020}.json"));
        let stored = Frame::from_value(&serde_json::from_slice(&fs::read(&path)?)?)?;
        if stored.key.session != incoming.key.session
            || stored.key.stream != incoming.key.stream
            || stored.key.generation != incoming.key.generation
            || stored.key.epoch != incoming.key.epoch
            || stored.key.seq != acked
            || stored.chain != receipt.chain
        {
            return Err(ReceiveError::Inconsistent(format!(
                "receipt does not match {}",
                path.display()
            )));
        }
    }

    loop {
        let next = receipt.acked.map_or(0, |seq| seq.saturating_add(1));
        let path = epoch_dir.join("frames").join(format!("{next:020}.json"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        };
        let stored = Frame::from_value(&serde_json::from_slice(&bytes)?)?;
        if stored.key.session != incoming.key.session
            || stored.key.stream != incoming.key.stream
            || stored.key.generation != incoming.key.generation
            || stored.key.epoch != incoming.key.epoch
            || stored.key.seq != next
        {
            return Err(ReceiveError::Inconsistent(format!(
                "stored key at {} does not match its path",
                path.display()
            )));
        }
        if stored.chain != chain_line(&receipt.chain, &stored.line) {
            return Err(ReceiveError::Inconsistent(format!(
                "stored chain is broken at sequence {next}"
            )));
        }
        receipt.acked = Some(next);
        receipt.chain = stored.chain;
    }
    Ok(receipt)
}

fn save_receipt(path: &Path, receipt: Receipt) -> Result<(), ReceiveError> {
    let acked = receipt
        .acked
        .ok_or_else(|| ReceiveError::Inconsistent("cannot save an empty receipt".into()))?;
    let value = Value::Object(Map::from_iter([
        ("acked".into(), Value::Number(acked.into())),
        ("chain".into(), Value::String(hex::encode(receipt.chain))),
    ]));
    write_atomic(path, &serde_json::to_vec(&value)?)?;
    Ok(())
}

fn respond(request: tiny_http::Request, status: u16, body: &str) {
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
