use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The chain value before sequence zero.
pub const ZERO_CHAIN: [u8; 32] = [0; 32];

/// The stable identity of one source frame.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FrameKey {
    pub session: String,
    pub stream: String,
    pub generation: u64,
    pub epoch: u64,
    pub seq: u64,
}

/// One complete source line and its replication metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub key: FrameKey,
    pub machine: String,
    pub chain: [u8; 32],
    pub sender_wall_ns: u64,
    pub sender_mono_ns: u64,
    pub boot_id: String,
    pub line: Vec<u8>,
}

/// Errors in a wire frame.
#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame is not a JSON object")]
    NotObject,
    #[error("frame field {0} is missing or has the wrong type")]
    Field(&'static str),
    #[error("frame field {field} is not valid hex: {source}")]
    Hex {
        field: &'static str,
        source: hex::FromHexError,
    },
    #[error("frame chain has {0} bytes, expected 32")]
    ChainLength(usize),
}

impl Frame {
    pub(crate) fn to_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("boot_id".into(), Value::String(self.boot_id.clone())),
            ("chain".into(), Value::String(hex::encode(self.chain))),
            ("epoch".into(), Value::Number(self.key.epoch.into())),
            (
                "generation".into(),
                Value::Number(self.key.generation.into()),
            ),
            ("line".into(), Value::String(hex::encode(&self.line))),
            ("machine".into(), Value::String(self.machine.clone())),
            ("sender_mono_ns".into(), self.sender_mono_ns.into()),
            ("sender_wall_ns".into(), self.sender_wall_ns.into()),
            ("seq".into(), Value::Number(self.key.seq.into())),
            ("session".into(), Value::String(self.key.session.clone())),
            ("stream".into(), Value::String(self.key.stream.clone())),
        ]))
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, FrameError> {
        Self::from_value_with_legacy_machine(value, false)
    }

    pub(crate) fn from_stored_value(value: &Value) -> Result<Self, FrameError> {
        Self::from_value_with_legacy_machine(value, true)
    }

    fn from_value_with_legacy_machine(
        value: &Value,
        allow_missing_machine: bool,
    ) -> Result<Self, FrameError> {
        let object = value.as_object().ok_or(FrameError::NotObject)?;
        let string = |field: &'static str| {
            object
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(FrameError::Field(field))
        };
        let integer = |field: &'static str| {
            object
                .get(field)
                .and_then(Value::as_u64)
                .ok_or(FrameError::Field(field))
        };
        let decode = |field: &'static str| {
            let encoded = object
                .get(field)
                .and_then(Value::as_str)
                .ok_or(FrameError::Field(field))?;
            hex::decode(encoded).map_err(|source| FrameError::Hex { field, source })
        };
        let chain = decode("chain")?;
        let chain_length = chain.len();
        let chain: [u8; 32] = chain
            .try_into()
            .map_err(|_| FrameError::ChainLength(chain_length))?;
        let machine = match object.get("machine") {
            None if allow_missing_machine => String::new(),
            Some(Value::String(machine)) => machine.clone(),
            _ => return Err(FrameError::Field("machine")),
        };
        Ok(Self {
            key: FrameKey {
                session: string("session")?,
                stream: string("stream")?,
                generation: integer("generation")?,
                epoch: integer("epoch")?,
                seq: integer("seq")?,
            },
            machine,
            chain,
            sender_wall_ns: integer("sender_wall_ns")?,
            sender_mono_ns: integer("sender_mono_ns")?,
            boot_id: string("boot_id")?,
            line: decode("line")?,
        })
    }
}

/// Extends a stream chain with one verbatim source line.
pub fn chain_line(previous: &[u8; 32], line: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(previous);
    hash.update(line);
    hash.finalize().into()
}
