//! Version 1 body channel. Control objects are strict JSON. Response frames are
//! a big-endian u32 JSON-header length, that header, then exactly payload_bytes
//! raw canonical typed-body bytes. There is no base64 expansion or trailing data.
//! The sum of encoded frames for a response, including all metadata, is <=256 KiB.
use crate::{
    body_cache::CacheKey,
    origin_read::{OpaqueId, ResponseBinding},
    receiver_audit::Identifier,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

pub const VERSION: u8 = 1;
pub const CONTROL_CAP: usize = 16 * 1024;
pub const RESPONSE_CAP: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    InvalidRequest,
    OverBudget,
    UnsupportedVerb,
    Forbidden,
    Unavailable,
    Revoked,
    Deadline,
    ConnectionCapacity,
    PollCapacity,
    PendingQueueFull,
    ForeignRequest,
    LateResponse,
    FrameOrder,
    BindingMismatch,
    AuditFailure,
    PolicyChanged,
    LeaseCapacity,
    /// Uniform for unknown, foreign and previously closed lease identifiers.
    /// The operation that closes a live lease returns its specific reason once;
    /// no historical lease capability or enrollment lookup is retained.
    LeaseClosed,
    ContinuationInvalid,
    Cancelled,
    NoWatchers,
    AclChanged,
    ChannelRestart,
}
impl Reason {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::Unavailable
                | Self::Deadline
                | Self::ConnectionCapacity
                | Self::PollCapacity
                | Self::PendingQueueFull
        )
    }
}
pub type Result<T> = std::result::Result<T, Reason>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollRequest {
    pub version: u8,
    pub channel_binding: [u8; 32],
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum PollResponse {
    Empty { version: u8 },
    Request { version: u8, request: Box<Request> },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub key: CacheKey,
    pub anchor: OpaqueId,
    pub mapping: Identifier,
    pub continuation: Option<OpaqueId>,
    pub payload_bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Resolve {
        selector: Selector,
        authorization_only: Option<Box<Candidate>>,
    },
    ReadPage {
        page: OpaqueId,
    },
    ReadField {
        field: OpaqueId,
    },
    SubscribeAppends {
        page: OpaqueId,
        lease_id: Identifier,
        lease_deadline: u64,
        continuation: Option<OpaqueId>,
    },
    Cancel {
        request_id: Identifier,
        lease_id: Option<Identifier>,
    },
    Acknowledge {
        request_id: Identifier,
        lease_id: Identifier,
        page: OpaqueId,
        lease_deadline: u64,
        continuation: OpaqueId,
    },
}
impl Operation {
    pub fn supported(&self) -> bool {
        matches!(
            self,
            Self::Resolve { .. }
                | Self::ReadPage { .. }
                | Self::ReadField { .. }
                | Self::SubscribeAppends { .. }
                | Self::Acknowledge { .. }
                | Self::Cancel { .. }
        )
    }
    pub fn selector(&self) -> Result<Selector> {
        match self {
            Self::Resolve { selector, .. } => Ok(selector.clone()),
            Self::ReadPage { page }
            | Self::SubscribeAppends { page, .. }
            | Self::Acknowledge { page, .. } => Ok(Selector::Page { id: page.clone() }),
            Self::ReadField { field } => Ok(Selector::Field { id: field.clone() }),
            _ => Err(Reason::UnsupportedVerb),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selector {
    Page { id: OpaqueId },
    Field { id: OpaqueId },
}
impl Selector {
    pub fn cache_selector(&self) -> crate::body_cache::Selector {
        match self {
            Self::Page { id } => crate::body_cache::Selector::Page(id.as_str().into()),
            Self::Field { id } => crate::body_cache::Selector::Field(id.as_str().into()),
        }
    }
}
/// Opaque assertion transport: canonical body_assertion signing encoding followed
/// by its 64-byte Ed25519 signature, encoded as lowercase hex. No JSON claims.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub request_id: Identifier,
    pub assertion: String,
    pub source_handle: OpaqueId,
    pub anchor: OpaqueId,
    pub operation: Operation,
    pub max_bytes: u32,
    pub deadline_millis: u64,
    pub destination: crate::receiver_audit::Destination,
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        version(self.version)?;
        if self.assertion.is_empty()
            || self.assertion.len() > 8192
            || !self.assertion.len().is_multiple_of(2)
            || !self
                .assertion
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Reason::InvalidRequest);
        }
        let bytes = hex::decode(&self.assertion).map_err(|_| Reason::InvalidRequest)?;
        crate::body_assertion::decode(&bytes).map_err(|_| Reason::InvalidRequest)?;
        if self.max_bytes == 0 || self.max_bytes as usize > RESPONSE_CAP {
            return Err(Reason::OverBudget);
        }
        if matches!(self.operation, Operation::Cancel { lease_id: None, .. }) {
            return Err(Reason::InvalidRequest);
        }
        if let Operation::SubscribeAppends { lease_deadline, .. }
        | Operation::Acknowledge { lease_deadline, .. } = self.operation
            && lease_deadline == 0
        {
            return Err(Reason::InvalidRequest);
        }
        if let Operation::Resolve {
            authorization_only: Some(c),
            ..
        } = &self.operation
        {
            validate_key(&c.key)?;
            if c.payload_bytes == 0 || c.payload_bytes > RESPONSE_CAP as u64 {
                return Err(Reason::OverBudget);
            }
        }
        Ok(())
    }
    /// Digest excludes only assertion bytes (which themselves bind this digest).
    pub fn digest(&self) -> Result<[u8; 32]> {
        let mut unsigned = self.clone();
        unsigned.assertion.clear();
        Ok(
            Sha256::digest(serde_json::to_vec(&unsigned).map_err(|_| Reason::InvalidRequest)?)
                .into(),
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameHeader {
    pub version: u8,
    pub request_id: Identifier,
    pub frame: u32,
    pub final_frame: bool,
    pub payload_bytes: u32,
    pub content: FrameContent,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameContent {
    Header { binding: Box<ResponseBinding> },
    Data,
    Refused { reason: Reason, retryable: bool },
}
pub struct Frame {
    pub header: FrameHeader,
    pub payload: Vec<u8>,
}

pub fn parse_poll(bytes: &[u8]) -> Result<PollRequest> {
    let poll: PollRequest = parse(bytes, CONTROL_CAP)?;
    version(poll.version)?;
    Ok(poll)
}
pub fn parse_request(bytes: &[u8]) -> Result<Request> {
    let request: Request = parse(bytes, CONTROL_CAP)?;
    request.validate()?;
    Ok(request)
}
pub fn parse_poll_response(bytes: &[u8]) -> Result<PollResponse> {
    let response: PollResponse = parse(bytes, CONTROL_CAP)?;
    match &response {
        PollResponse::Empty { version: v } => version(*v)?,
        PollResponse::Request {
            version: v,
            request,
        } => {
            version(*v)?;
            request.validate()?;
        }
    }
    Ok(response)
}
pub fn control<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| Reason::InvalidRequest)?;
    if bytes.len() > CONTROL_CAP {
        return Err(Reason::OverBudget);
    }
    Ok(bytes)
}
fn parse<T: DeserializeOwned>(bytes: &[u8], cap: usize) -> Result<T> {
    if bytes.len() > cap {
        return Err(Reason::OverBudget);
    }
    serde_json::from_slice(bytes).map_err(|_| Reason::InvalidRequest)
}
fn version(value: u8) -> Result<()> {
    if value != VERSION {
        Err(Reason::InvalidRequest)
    } else {
        Ok(())
    }
}
pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>> {
    if frame.payload.len() != frame.header.payload_bytes as usize {
        return Err(Reason::InvalidRequest);
    }
    let header = control(&frame.header)?;
    let mut bytes = Vec::with_capacity(4 + header.len() + frame.payload.len());
    bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&frame.payload);
    parse_frame(&bytes)?;
    Ok(bytes)
}
pub fn parse_frame(bytes: &[u8]) -> Result<Frame> {
    if bytes.len() > RESPONSE_CAP {
        return Err(Reason::OverBudget);
    }
    let prefix: [u8; 4] = bytes
        .get(..4)
        .ok_or(Reason::InvalidRequest)?
        .try_into()
        .map_err(|_| Reason::InvalidRequest)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > CONTROL_CAP {
        return Err(Reason::OverBudget);
    }
    let end = 4usize.checked_add(length).ok_or(Reason::OverBudget)?;
    let header: FrameHeader = parse(
        bytes.get(4..end).ok_or(Reason::InvalidRequest)?,
        CONTROL_CAP,
    )?;
    version(header.version)?;
    let payload = bytes.get(end..).ok_or(Reason::InvalidRequest)?;
    if payload.len() != header.payload_bytes as usize {
        return Err(Reason::InvalidRequest);
    }
    match &header.content {
        FrameContent::Header { binding } => {
            if header.frame != 0 || !payload.is_empty() {
                return Err(Reason::FrameOrder);
            }
            validate_key(&binding.cache_key)?;
            if binding.permitted_range.start != 0
                || binding.permitted_range.end == 0
                || binding.permitted_range.end > RESPONSE_CAP as u64
                || binding.max_bytes == 0
                || binding.max_bytes > RESPONSE_CAP
            {
                return Err(Reason::OverBudget);
            }
        }
        FrameContent::Data => {
            if header.frame == 0 || payload.is_empty() {
                return Err(Reason::FrameOrder);
            }
        }
        FrameContent::Refused { .. } => {
            if header.frame != 0 || !header.final_frame || !payload.is_empty() {
                return Err(Reason::FrameOrder);
            }
        }
    }
    Ok(Frame {
        header,
        payload: payload.to_vec(),
    })
}
pub(crate) fn validate_key(key: &CacheKey) -> Result<()> {
    for id in [
        &key.namespace,
        &key.origin_incarnation,
        &key.activity_generation,
        &key.transcript_generation,
        &key.export_view_generation,
        &key.authorization_scope,
    ] {
        OpaqueId::new(id).map_err(|_| Reason::InvalidRequest)?;
    }
    if key.cohort.is_empty() || key.cohort.len() > 128 || key.policy_versions.len() != 3 {
        return Err(Reason::InvalidRequest);
    }
    for id in &key.policy_versions {
        Identifier::try_from(id.clone()).map_err(|_| Reason::InvalidRequest)?;
    }
    for (i, source) in key.cohort.iter().enumerate() {
        for id in [&source.source_id, &source.chain, &source.generation] {
            OpaqueId::new(id).map_err(|_| Reason::InvalidRequest)?;
        }
        if key.cohort[..i]
            .iter()
            .any(|s| s.source_id == source.source_id)
        {
            return Err(Reason::InvalidRequest);
        }
    }
    match &key.selector {
        crate::body_cache::Selector::Page(id) | crate::body_cache::Selector::Field(id) => {
            OpaqueId::new(id).map_err(|_| Reason::InvalidRequest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_poll() {
        let valid = control(&PollRequest {
            version: VERSION,
            channel_binding: [0; 32],
        })
        .unwrap();
        assert!(parse_poll(&valid).is_ok());
        let mut trailing = valid.clone();
        trailing.extend_from_slice(b" {}");
        assert!(parse_poll(&trailing).is_err());
        assert!(parse_poll(&vec![0; CONTROL_CAP + 1]).is_err());
        for extra in ["command", "runtime", "ssh", "namespace"] {
            let mut v: serde_json::Value = serde_json::from_slice(&valid).unwrap();
            v[extra] = serde_json::json!("x");
            assert!(parse_poll(&serde_json::to_vec(&v).unwrap()).is_err());
        }
        assert!(
            parse_poll(
                &control(&PollRequest {
                    version: 2,
                    channel_binding: [0; 32]
                })
                .unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn closed_operations_and_nested_fields() {
        for verb in ["exec", "ssh", "runtime", "search", "sql", "control"] {
            assert!(serde_json::from_value::<Operation>(serde_json::json!({"verb":verb})).is_err());
        }
        assert!(
            serde_json::from_value::<Operation>(
                serde_json::json!({"verb":"read_page","page":"p","command":"x"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Selector>(
                serde_json::json!({"kind":"page","id":"p","extra":1})
            )
            .is_err()
        );
    }
    #[test]
    fn append_identifiers_and_continuations_are_bounded() {
        // Identifier is exactly 64 hex characters; OpaqueId is 1..=128 bytes.
        // Serialization metadata counts toward CONTROL_CAP, without body text.
        let lease = Identifier::from_bytes([1; 32]);
        let operation = serde_json::json!({"verb":"acknowledge", "request_id":lease, "lease_id":lease, "page":"page", "lease_deadline":70_000, "continuation":"next"});
        assert!(serde_json::from_value::<Operation>(operation.clone()).is_ok());
        for (field, value) in [
            ("lease_id", "a".repeat(65)),
            ("continuation", "a".repeat(129)),
            ("page", "".into()),
        ] {
            let mut invalid = operation.clone();
            invalid[field] = value.into();
            assert!(serde_json::from_value::<Operation>(invalid).is_err());
        }
        assert_eq!(
            control(&"x".repeat(CONTROL_CAP)).err(),
            Some(Reason::OverBudget)
        );
    }
    #[test]
    fn exact_frame_lengths() {
        let f = Frame {
            header: FrameHeader {
                version: VERSION,
                request_id: Identifier::from_bytes([1; 32]),
                frame: 1,
                final_frame: true,
                payload_bytes: 1,
                content: FrameContent::Data,
            },
            payload: vec![42],
        };
        let mut encoded = encode_frame(&f).unwrap();
        assert_eq!(parse_frame(&encoded).unwrap().payload, vec![42]);
        encoded.push(0);
        assert!(parse_frame(&encoded).is_err());
        assert!(parse_frame(&vec![0; RESPONSE_CAP + 1]).is_err());
        assert!(parse_frame(&[0, 0, 0]).is_err());
    }
}
