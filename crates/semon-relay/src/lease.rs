use std::{collections::BTreeMap, time::SystemTime};

use serde_json::{Map, Value};
use thiserror::Error;

/// Receiver-owned lease duration: three minutes.
pub const LEASE_DURATION_MS: u64 = 3 * 60 * 1_000;
/// Sender renewal cadence: one third of the lease duration.
pub const LEASE_RENEW_INTERVAL_MS: u64 = 60 * 1_000;

/// The receiver's source of lease time.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Wall clock used by the persistent receiver register.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::UNIX_EPOCH
            .elapsed()
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64
    }
}

/// One receiver-side lease and epoch register row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseRow {
    pub session: String,
    pub epoch: u64,
    pub holder_machine: String,
    pub lease_expires_at_ms: u64,
}

/// One successful epoch transition. Existing records are never changed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TakeoverRecord {
    pub session: String,
    pub previous_epoch: u64,
    pub epoch: u64,
    pub previous_holder: String,
    pub holder_machine: String,
    pub forced: bool,
    pub taken_at_ms: u64,
}

/// Register rows and their append-only takeover history.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LeaseStatus {
    pub rows: Vec<LeaseRow>,
    pub takeovers: Vec<TakeoverRecord>,
}

/// The live contiguous tip of one stream generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamTip {
    pub stream: String,
    pub generation: u64,
    pub epoch: u64,
    pub seq: u64,
    pub chain: [u8; 32],
}

/// A successful takeover and the watermarks the new holder must adopt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TakeoverResult {
    pub row: LeaseRow,
    pub tips: Vec<StreamTip>,
}

/// A separately listed group of retained frames from a fenced sender.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrphanSummary {
    pub session: String,
    pub stream: String,
    pub generation: u64,
    pub fenced_epoch: u64,
    pub frames: u64,
    pub first_seq: u64,
    pub last_seq: u64,
}

#[derive(Debug, Error)]
pub enum LeaseValueError {
    #[error("{kind} is not a JSON object")]
    NotObject { kind: &'static str },
    #[error("{kind} field {field} is missing or has the wrong type")]
    Field {
        kind: &'static str,
        field: &'static str,
    },
    #[error("stream tip chain is not valid hex: {0}")]
    ChainHex(hex::FromHexError),
    #[error("stream tip chain has {0} bytes, expected 32")]
    ChainLength(usize),
    #[error("unsupported lease register version {0}")]
    Version(u64),
    #[error("duplicate lease row for session {0}")]
    DuplicateSession(String),
}

impl LeaseRow {
    pub(crate) fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("epoch".into(), self.epoch.into()),
            (
                "holder_machine".into(),
                Value::String(self.holder_machine.clone()),
            ),
            (
                "lease_expires_at_ms".into(),
                self.lease_expires_at_ms.into(),
            ),
            ("session".into(), Value::String(self.session.clone())),
        ]))
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, LeaseValueError> {
        let object = object(value, "lease row")?;
        Ok(Self {
            session: string(object, "lease row", "session")?,
            epoch: integer(object, "lease row", "epoch")?,
            holder_machine: string(object, "lease row", "holder_machine")?,
            lease_expires_at_ms: integer(object, "lease row", "lease_expires_at_ms")?,
        })
    }
}

impl TakeoverRecord {
    pub(crate) fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("epoch".into(), self.epoch.into()),
            ("forced".into(), self.forced.into()),
            (
                "holder_machine".into(),
                Value::String(self.holder_machine.clone()),
            ),
            ("previous_epoch".into(), self.previous_epoch.into()),
            (
                "previous_holder".into(),
                Value::String(self.previous_holder.clone()),
            ),
            ("session".into(), Value::String(self.session.clone())),
            ("taken_at_ms".into(), self.taken_at_ms.into()),
        ]))
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, LeaseValueError> {
        let object = object(value, "takeover record")?;
        Ok(Self {
            session: string(object, "takeover record", "session")?,
            previous_epoch: integer(object, "takeover record", "previous_epoch")?,
            epoch: integer(object, "takeover record", "epoch")?,
            previous_holder: string(object, "takeover record", "previous_holder")?,
            holder_machine: string(object, "takeover record", "holder_machine")?,
            forced: boolean(object, "takeover record", "forced")?,
            taken_at_ms: integer(object, "takeover record", "taken_at_ms")?,
        })
    }
}

impl LeaseStatus {
    pub(crate) fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            (
                "rows".into(),
                Value::Array(self.rows.iter().map(LeaseRow::to_value).collect()),
            ),
            (
                "takeovers".into(),
                Value::Array(
                    self.takeovers
                        .iter()
                        .map(TakeoverRecord::to_value)
                        .collect(),
                ),
            ),
        ]))
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, LeaseValueError> {
        let object = object(value, "lease status")?;
        Ok(Self {
            rows: array(object, "lease status", "rows")?
                .iter()
                .map(LeaseRow::from_value)
                .collect::<Result<_, _>>()?,
            takeovers: array(object, "lease status", "takeovers")?
                .iter()
                .map(TakeoverRecord::from_value)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl StreamTip {
    pub(crate) fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("chain".into(), Value::String(hex::encode(self.chain))),
            ("epoch".into(), self.epoch.into()),
            ("generation".into(), self.generation.into()),
            ("seq".into(), self.seq.into()),
            ("stream".into(), Value::String(self.stream.clone())),
        ]))
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, LeaseValueError> {
        let object = object(value, "stream tip")?;
        let chain = hex::decode(string(object, "stream tip", "chain")?)
            .map_err(LeaseValueError::ChainHex)?;
        let length = chain.len();
        Ok(Self {
            stream: string(object, "stream tip", "stream")?,
            generation: integer(object, "stream tip", "generation")?,
            epoch: integer(object, "stream tip", "epoch")?,
            seq: integer(object, "stream tip", "seq")?,
            chain: chain
                .try_into()
                .map_err(|_| LeaseValueError::ChainLength(length))?,
        })
    }
}

impl TakeoverResult {
    pub(crate) fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("row".into(), self.row.to_value()),
            (
                "tips".into(),
                Value::Array(self.tips.iter().map(StreamTip::to_value).collect()),
            ),
        ]))
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, LeaseValueError> {
        let object = object(value, "takeover result")?;
        Ok(Self {
            row: LeaseRow::from_value(field(object, "takeover result", "row")?)?,
            tips: array(object, "takeover result", "tips")?
                .iter()
                .map(StreamTip::from_value)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl OrphanSummary {
    pub(crate) fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("fenced_epoch".into(), self.fenced_epoch.into()),
            ("first_seq".into(), self.first_seq.into()),
            ("frames".into(), self.frames.into()),
            ("generation".into(), self.generation.into()),
            ("last_seq".into(), self.last_seq.into()),
            ("session".into(), Value::String(self.session.clone())),
            ("stream".into(), Value::String(self.stream.clone())),
        ]))
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, LeaseValueError> {
        let object = object(value, "orphan summary")?;
        Ok(Self {
            session: string(object, "orphan summary", "session")?,
            stream: string(object, "orphan summary", "stream")?,
            generation: integer(object, "orphan summary", "generation")?,
            fenced_epoch: integer(object, "orphan summary", "fenced_epoch")?,
            frames: integer(object, "orphan summary", "frames")?,
            first_seq: integer(object, "orphan summary", "first_seq")?,
            last_seq: integer(object, "orphan summary", "last_seq")?,
        })
    }
}

pub(crate) fn rows_to_value(rows: &BTreeMap<String, LeaseRow>) -> Value {
    Value::Object(Map::from_iter([
        (
            "rows".into(),
            Value::Array(rows.values().map(LeaseRow::to_value).collect()),
        ),
        ("version".into(), 1_u64.into()),
    ]))
}

pub(crate) fn rows_from_value(
    value: &Value,
) -> Result<BTreeMap<String, LeaseRow>, LeaseValueError> {
    let object = object(value, "lease register")?;
    let version = integer(object, "lease register", "version")?;
    if version != 1 {
        return Err(LeaseValueError::Version(version));
    }
    let mut rows = BTreeMap::new();
    for value in array(object, "lease register", "rows")? {
        let row = LeaseRow::from_value(value)?;
        let session = row.session.clone();
        if rows.insert(session.clone(), row).is_some() {
            return Err(LeaseValueError::DuplicateSession(session));
        }
    }
    Ok(rows)
}

fn object<'a>(
    value: &'a Value,
    kind: &'static str,
) -> Result<&'a Map<String, Value>, LeaseValueError> {
    value.as_object().ok_or(LeaseValueError::NotObject { kind })
}

fn field<'a>(
    object: &'a Map<String, Value>,
    kind: &'static str,
    name: &'static str,
) -> Result<&'a Value, LeaseValueError> {
    object
        .get(name)
        .ok_or(LeaseValueError::Field { kind, field: name })
}

fn string(
    object: &Map<String, Value>,
    kind: &'static str,
    name: &'static str,
) -> Result<String, LeaseValueError> {
    field(object, kind, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or(LeaseValueError::Field { kind, field: name })
}

fn integer(
    object: &Map<String, Value>,
    kind: &'static str,
    name: &'static str,
) -> Result<u64, LeaseValueError> {
    field(object, kind, name)?
        .as_u64()
        .ok_or(LeaseValueError::Field { kind, field: name })
}

fn boolean(
    object: &Map<String, Value>,
    kind: &'static str,
    name: &'static str,
) -> Result<bool, LeaseValueError> {
    field(object, kind, name)?
        .as_bool()
        .ok_or(LeaseValueError::Field { kind, field: name })
}

fn array<'a>(
    object: &'a Map<String, Value>,
    kind: &'static str,
    name: &'static str,
) -> Result<&'a [Value], LeaseValueError> {
    field(object, kind, name)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or(LeaseValueError::Field { kind, field: name })
}
