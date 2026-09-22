use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::EncryptedPayload;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameMode {
    Plaintext,
    Encrypted,
}

impl FrameMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plaintext => "plaintext",
            Self::Encrypted => "encrypted",
        }
    }
}

/// The line-bearing portion of a frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameContent {
    Plaintext { chain: [u8; 32], line: Vec<u8> },
    Encrypted(EncryptedPayload),
}

/// One complete source line and its replication metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub key: FrameKey,
    pub machine: String,
    pub sender_wall_ns: u64,
    pub sender_mono_ns: u64,
    pub boot_id: String,
    pub content: FrameContent,
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
    #[error("frame nonce has {0} bytes, expected 24")]
    NonceLength(usize),
    #[error("frame content tag has {0} bytes, expected 32")]
    TagLength(usize),
    #[error("unsupported frame mode {0}")]
    Mode(String),
}

impl Frame {
    pub fn plaintext(
        key: FrameKey,
        machine: String,
        chain: [u8; 32],
        sender_wall_ns: u64,
        sender_mono_ns: u64,
        boot_id: String,
        line: Vec<u8>,
    ) -> Self {
        Self {
            key,
            machine,
            sender_wall_ns,
            sender_mono_ns,
            boot_id,
            content: FrameContent::Plaintext { chain, line },
        }
    }

    pub fn encrypted(
        key: FrameKey,
        machine: String,
        sender_wall_ns: u64,
        sender_mono_ns: u64,
        boot_id: String,
        payload: EncryptedPayload,
    ) -> Self {
        Self {
            key,
            machine,
            sender_wall_ns,
            sender_mono_ns,
            boot_id,
            content: FrameContent::Encrypted(payload),
        }
    }

    pub fn mode(&self) -> FrameMode {
        match self.content {
            FrameContent::Plaintext { .. } => FrameMode::Plaintext,
            FrameContent::Encrypted(_) => FrameMode::Encrypted,
        }
    }

    pub fn plaintext_content(&self) -> Option<(&[u8; 32], &[u8])> {
        match &self.content {
            FrameContent::Plaintext { chain, line } => Some((chain, line)),
            FrameContent::Encrypted(_) => None,
        }
    }

    pub fn encrypted_payload(&self) -> Option<&EncryptedPayload> {
        match &self.content {
            FrameContent::Plaintext { .. } => None,
            FrameContent::Encrypted(payload) => Some(payload),
        }
    }

    pub fn content_tag(&self) -> Option<&[u8; 32]> {
        self.encrypted_payload().map(|payload| &payload.tag)
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut object = Map::from_iter([
            ("boot_id".into(), Value::String(self.boot_id.clone())),
            ("epoch".into(), Value::Number(self.key.epoch.into())),
            (
                "generation".into(),
                Value::Number(self.key.generation.into()),
            ),
            ("machine".into(), Value::String(self.machine.clone())),
            ("mode".into(), Value::String(self.mode().as_str().into())),
            ("sender_mono_ns".into(), self.sender_mono_ns.into()),
            ("sender_wall_ns".into(), self.sender_wall_ns.into()),
            ("seq".into(), Value::Number(self.key.seq.into())),
            ("session".into(), Value::String(self.key.session.clone())),
            ("stream".into(), Value::String(self.key.stream.clone())),
        ]);
        match &self.content {
            FrameContent::Plaintext { chain, line } => {
                object.insert("chain".into(), Value::String(hex::encode(chain)));
                object.insert("line".into(), Value::String(hex::encode(line)));
            }
            FrameContent::Encrypted(payload) => {
                object.insert(
                    "ciphertext".into(),
                    Value::String(hex::encode(&payload.ciphertext)),
                );
                object.insert("nonce".into(), Value::String(hex::encode(payload.nonce)));
                object.insert("tag".into(), Value::String(hex::encode(payload.tag)));
            }
        }
        Value::Object(object)
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
        let machine = match object.get("machine") {
            None if allow_missing_machine => String::new(),
            Some(Value::String(machine)) => machine.clone(),
            _ => return Err(FrameError::Field("machine")),
        };
        let key = FrameKey {
            session: string("session")?,
            stream: string("stream")?,
            generation: integer("generation")?,
            epoch: integer("epoch")?,
            seq: integer("seq")?,
        };
        let content = match object.get("mode").and_then(Value::as_str) {
            None | Some("plaintext") => {
                let chain = decode("chain")?;
                let chain_length = chain.len();
                let chain = chain
                    .try_into()
                    .map_err(|_| FrameError::ChainLength(chain_length))?;
                FrameContent::Plaintext {
                    chain,
                    line: decode("line")?,
                }
            }
            Some("encrypted") => {
                let nonce = decode("nonce")?;
                let nonce_length = nonce.len();
                let nonce = nonce
                    .try_into()
                    .map_err(|_| FrameError::NonceLength(nonce_length))?;
                let tag = decode("tag")?;
                let tag_length = tag.len();
                let tag = tag
                    .try_into()
                    .map_err(|_| FrameError::TagLength(tag_length))?;
                FrameContent::Encrypted(EncryptedPayload {
                    nonce,
                    ciphertext: decode("ciphertext")?,
                    tag,
                })
            }
            Some(mode) => return Err(FrameError::Mode(mode.to_owned())),
        };
        Ok(Self {
            key,
            machine,
            sender_wall_ns: integer("sender_wall_ns")?,
            sender_mono_ns: integer("sender_mono_ns")?,
            boot_id: string("boot_id")?,
            content,
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
