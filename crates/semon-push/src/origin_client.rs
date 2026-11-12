//! One origin's serial, transport-independent body channel. No body retries or spool.
//! Transports must bound their own I/O by the supplied deadline and return promptly
//! on cancellation. Scheduling belongs to the caller: step never sleeps.
use crate::{
    body_assertion::{self, Audience, Expected, ReplayCache},
    body_cache::Clock,
    body_wire::{self as wire, Frame, FrameContent, FrameHeader, Operation, Reason},
    origin_read::{self, CachedCandidate, ExactMapping, ExportViewValidator, OriginRead, Target},
    receiver_audit::{Identifier, Outcome},
};
use zeroize::Zeroizing;
static DEFAULT_LIMITS: origin_read::CacheLimits = origin_read::CacheLimits {
    cache_ttl_seconds: None,
    cache_per_source_bytes: None,
    cache_total_bytes: None,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Unavailable,
    Cancelled,
    Revoked,
}
pub enum PollOutcome {
    Empty,
    Request(Vec<u8>),
}
pub trait PollTransport {
    /// Return one strict request envelope (not the surrounding PollResponse).
    /// No redirects, credential selection, retries or body persistence are implied.
    fn poll(
        &mut self,
        channel_binding: [u8; 32],
        deadline_millis: u64,
    ) -> Result<PollOutcome, TransportError>;
    /// Success means the entire frame was accepted; error must terminate this
    /// response. Partial writes are not retried or counted as accepted payload.
    fn post_response(
        &mut self,
        request_id: Identifier,
        frame_bytes: &[u8],
    ) -> Result<(), TransportError>;
}
pub trait Entropy {
    fn channel_binding(&mut self) -> Result<[u8; 32], TransportError>;
}
pub struct SystemEntropy;
impl Entropy for SystemEntropy {
    fn channel_binding(&mut self) -> Result<[u8; 32], TransportError> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|_| TransportError::Unavailable)?;
        Ok(bytes)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Cancelled,
    Revoked,
    AuditFailure,
    Entropy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Waiting { next_poll_not_before: u64 },
    Empty,
    Delivered,
    Refused(Reason),
    Dropped,
    Disconnected,
    Stopped(StopReason),
}
/// Delay is data, capped at twenty seconds; the host schedules the next step.
#[derive(Default)]
pub struct Backoff {
    failures: u32,
    pub next_poll_not_before: u64,
}
impl Backoff {
    fn fail(&mut self, now: u64) {
        let delay = 250u64
            .saturating_mul(1u64 << self.failures.min(7))
            .min(20_000);
        self.failures = self.failures.saturating_add(1);
        self.next_poll_not_before = now.saturating_add(delay);
    }
    fn reset(&mut self) {
        self.failures = 0;
        self.next_poll_not_before = 0;
    }
}
struct OriginLease {
    request: Identifier,
    deadline: u64,
    binding: origin_read::ResponseBinding,
}
pub struct OriginClient<'a, C: Clock, T: PollTransport, E: Entropy> {
    pub origin: OriginRead<'a, C>,
    pub transport: T,
    pub replay: &'a mut ReplayCache,
    pub lookup: &'a dyn Fn(&str) -> Option<[u8; 32]>,
    pub export_views: &'a dyn ExportViewValidator,
    pub limits: &'a dyn origin_read::OriginLimits,
    entropy: E,
    channel: Option<[u8; 32]>,
    scope: Option<origin_read::EnrollmentScope>,
    backoff: Backoff,
    stopped: Option<StopReason>,
    pub append_source: Option<&'a dyn crate::append_source::AppendPageSource>,
    leases: std::collections::BTreeMap<Identifier, OriginLease>,
}
impl<'a, C: Clock, T: PollTransport, E: Entropy> OriginClient<'a, C, T, E> {
    pub fn new(
        origin: OriginRead<'a, C>,
        transport: T,
        entropy: E,
        replay: &'a mut ReplayCache,
        lookup: &'a dyn Fn(&str) -> Option<[u8; 32]>,
        export_views: &'a dyn ExportViewValidator,
    ) -> Self {
        Self {
            origin,
            transport,
            entropy,
            replay,
            lookup,
            export_views,
            limits: &DEFAULT_LIMITS,
            channel: None,
            scope: None,
            backoff: Backoff::default(),
            stopped: None,
            append_source: None,
            leases: std::collections::BTreeMap::new(),
        }
    }
    pub fn next_poll_not_before(&self) -> u64 {
        self.backoff.next_poll_not_before
    }
    fn stop(&mut self, reason: StopReason) -> Step {
        self.stopped = Some(reason);
        self.channel = None;
        self.leases.clear();
        Step::Stopped(reason)
    }
    fn disconnect(&mut self, error: TransportError) -> Step {
        self.channel = None;
        self.leases.clear();
        match error {
            TransportError::Revoked => self.stop(StopReason::Revoked),
            TransportError::Cancelled => self.stop(StopReason::Cancelled),
            TransportError::Unavailable => {
                self.backoff.fail(self.origin.clock.now_millis());
                Step::Disconnected
            }
        }
    }
    /// One serial poll/response operation. The transport may hold a bounded poll;
    /// this state machine performs no waiting, sleeping or background work.
    pub fn step(&mut self) -> Step {
        if let Some(reason) = self.stopped {
            return Step::Stopped(reason);
        }
        if (self.origin.cancelled)() {
            return self.stop(StopReason::Cancelled);
        }
        let scope = match self.origin.enrollment.authenticated_enrollment() {
            Ok(scope)
                if scope
                    .kinds
                    .contains(&origin_read::EnrollmentKind::BodyChannel) =>
            {
                scope
            }
            _ => return self.stop(StopReason::Revoked),
        };
        if self.scope.as_ref().is_some_and(|old| old != &scope) {
            return self.stop(StopReason::Revoked);
        }
        self.scope = Some(scope.clone());
        let now = self.origin.clock.now_millis();
        if now < self.backoff.next_poll_not_before {
            return Step::Waiting {
                next_poll_not_before: self.backoff.next_poll_not_before,
            };
        }
        let channel = match self.channel {
            Some(channel) => channel,
            None => match self.entropy.channel_binding() {
                Ok(channel) => {
                    self.channel = Some(channel);
                    channel
                }
                Err(_) => return self.stop(StopReason::Entropy),
            },
        };
        let bytes = match self.transport.poll(channel, now.saturating_add(20_000)) {
            Ok(PollOutcome::Empty) => {
                self.backoff.reset();
                return Step::Empty;
            }
            Ok(PollOutcome::Request(bytes)) => bytes,
            Err(error) => return self.disconnect(error),
        };
        self.backoff.reset();
        let request = match wire::parse_request(&bytes) {
            Ok(request) => request,
            Err(reason) => {
                return match request_id(&bytes) {
                    Some(id) => self.refuse(id, reason),
                    None => Step::Dropped,
                };
            }
        };
        let id = request.request_id;
        if !request.operation.supported() {
            return self.refuse(id, Reason::UnsupportedVerb);
        }
        let audience = Audience {
            origin_incarnation: scope.origin_incarnation.as_str().into(),
            namespace: scope.namespace.as_str().into(),
        };
        let assertion = hex::decode(&request.assertion)
            .ok()
            .and_then(|bytes| body_assertion::decode(&bytes).ok());
        let Some(assertion) = assertion else {
            return self.refuse(id, Reason::Forbidden);
        };
        let claims = match body_assertion::verify(
            &assertion,
            self.lookup,
            &Expected {
                audience: &audience,
                channel_binding: &channel,
                deadline: request.deadline_millis / 1000,
                skew: body_assertion::MAX_SKEW,
            },
            self.replay,
            || self.origin.clock.now_millis() / 1000,
        ) {
            Ok(claims) => claims,
            Err(_) => return self.refuse(id, Reason::Forbidden),
        };
        if id != Identifier::from_bytes(claims.nonce) {
            return self.refuse(id, Reason::Forbidden);
        }
        if self.origin.clock.now_millis() >= request.deadline_millis {
            return self.refuse(id, Reason::Deadline);
        }
        if request.digest().ok() != Some(claims.request_digest)
            || claims.machine != scope.machine.as_str()
            || claims.namespace != scope.namespace.as_str()
            || scope.publisher_claim_epoch != Some(claims.claim_epoch)
        {
            return self.refuse(id, Reason::Forbidden);
        }
        let now = self.origin.clock.now_millis();
        self.leases.retain(|_, lease| now < lease.deadline);
        if let Operation::Cancel { lease_id, .. } = &request.operation {
            let Some(lease_id) = lease_id else {
                return self.refuse(id, Reason::LeaseClosed);
            };
            let reason = if self.leases.remove(lease_id).is_some() {
                Reason::Cancelled
            } else {
                Reason::LeaseClosed
            };
            return self.refuse(id, reason);
        }
        let append = match &request.operation {
            Operation::SubscribeAppends {
                lease_id,
                lease_deadline,
                continuation,
                ..
            } => {
                if self.leases.contains_key(lease_id) {
                    return self.refuse(id, Reason::LeaseClosed);
                }
                if self.leases.len() >= crate::body_lease::LEASE_CAP {
                    return self.refuse(id, Reason::LeaseCapacity);
                }
                Some((*lease_id, *lease_deadline, continuation.as_ref()))
            }
            Operation::Acknowledge {
                request_id,
                lease_id,
                lease_deadline,
                continuation,
                page,
            } => {
                let Some(lease) = self.leases.get(lease_id) else {
                    return self.refuse(id, Reason::LeaseClosed);
                };
                if lease.request != *request_id
                    || lease.binding.continuation.as_ref() != Some(continuation)
                    || lease.binding.cache_key.selector
                        != crate::body_cache::Selector::Page(page.as_str().into())
                    || lease.binding.source_handle != request.source_handle
                    || lease.binding.mapping.anchor != request.anchor
                {
                    return self.refuse(id, Reason::ContinuationInvalid);
                }
                Some((*lease_id, *lease_deadline, Some(continuation)))
            }
            _ => None,
        };
        if let Some((lease, deadline, _)) = append
            && (deadline <= now
                || deadline > now.saturating_add(crate::body_lease::LEASE_WINDOW_MILLIS))
        {
            self.leases.remove(&lease);
            return self.refuse(id, Reason::Deadline);
        }
        let target = match request.operation.selector() {
            Ok(wire::Selector::Page { id }) => Target::Page(id),
            Ok(wire::Selector::Field { id }) => Target::Field(id),
            Err(reason) => return self.refuse(id, reason),
        };
        let digest = match request.digest() {
            Ok(digest) => digest,
            Err(reason) => return self.refuse(id, reason),
        };
        let read = origin_read::ReadRequest {
            source_handle: request.source_handle.clone(),
            target,
            anchor: request.anchor.clone(),
            request_digest: digest,
            channel_binding: channel,
            max_bytes: request.max_bytes as usize,
            destination: request.destination,
        };
        let limits = match self.limits.cache_limits() {
            Ok(limits) => limits,
            Err(error) => return self.refuse(id, reason(error)),
        };
        let authorized = if let Some((lease_id, _, after)) = append {
            let Some(source) = self.append_source else {
                return self.refuse(id, Reason::Unavailable);
            };
            let Target::Page(page) = &read.target else {
                return self.refuse(id, Reason::InvalidRequest);
            };
            let renderer = crate::append_source::AppendRenderer {
                source,
                cursor: crate::append_source::AppendCursor {
                    lease_id,
                    page,
                    after,
                },
            };
            self.origin.read_append(claims, &read, &renderer)
        } else {
            match &request.operation {
                Operation::Resolve {
                    authorization_only: Some(candidate),
                    ..
                } => self.origin.authorization_only_with_cache_limits(
                    claims,
                    &read,
                    &CachedCandidate {
                        key: candidate.key.clone(),
                        mapping: ExactMapping {
                            anchor: candidate.anchor.clone(),
                            assertion: candidate.mapping,
                        },
                        continuation: candidate.continuation.clone(),
                        payload_bytes: candidate.payload_bytes,
                    },
                    self.export_views,
                    limits,
                ),
                _ => self.origin.read_with_cache_limits(claims, &read, limits),
            }
        };
        let authorized = match authorized {
            Ok(authorized) => authorized,
            Err(error) => {
                if let Some((lease_id, _, _)) = append {
                    self.leases.remove(&lease_id);
                }
                let refusal = if append.is_some() && error == origin_read::Refused::PolicyChanged {
                    Reason::ContinuationInvalid
                } else {
                    reason(error)
                };
                return self.refuse(id, refusal);
            }
        };
        if let Some((lease_id, _, _)) = append {
            let invalid = authorized.response_binding.continuation.is_none()
                || self.leases.get(&lease_id).is_some_and(|previous| {
                    !crate::body_lease::same_generation(
                        &previous.binding,
                        &authorized.response_binding,
                    )
                });
            if invalid {
                self.leases.remove(&lease_id);
                if authorized
                    .finish(self.origin.audit, Outcome::Failed, 0)
                    .is_err()
                {
                    return self.stop(StopReason::AuditFailure);
                }
                return self.refuse(id, Reason::ContinuationInvalid);
            }
        }
        let lease_binding = authorized.response_binding.clone();
        // Build and bound the complete response before sending any frame. All
        // temporary canonical payload and encoded frames are zeroized on drop.
        let frames = response_frames(id, &authorized, request.max_bytes as usize);
        let mut emitted = 0;
        let mut failure = None;
        match frames {
            Err(error) => failure = Some(error),
            Ok(frames) => {
                for (frame, payload_bytes) in frames {
                    if (self.origin.cancelled)() {
                        failure = Some(Reason::Unavailable);
                        break;
                    }
                    if self
                        .origin
                        .enrollment
                        .authenticated_enrollment()
                        .ok()
                        .as_ref()
                        != Some(&scope)
                    {
                        failure = Some(Reason::Revoked);
                        break;
                    }
                    if self.origin.clock.now_millis() >= authorized.response_binding.deadline_millis
                    {
                        failure = Some(Reason::Deadline);
                        break;
                    }
                    if let Err(error) = self.transport.post_response(id, &frame) {
                        let result = self.disconnect(error);
                        let outcome = if error == TransportError::Cancelled {
                            Outcome::Cancelled
                        } else {
                            Outcome::Failed
                        };
                        if authorized
                            .finish(self.origin.audit, outcome, emitted)
                            .is_err()
                        {
                            return self.stop(StopReason::AuditFailure);
                        }
                        return result;
                    }
                    emitted += payload_bytes;
                }
            }
        }
        let cancelled = (self.origin.cancelled)();
        let outcome = if cancelled {
            Outcome::Cancelled
        } else if failure.is_some() {
            Outcome::Failed
        } else {
            Outcome::Completed
        };
        if authorized
            .finish(self.origin.audit, outcome, emitted)
            .is_err()
        {
            return self.stop(StopReason::AuditFailure);
        }
        if cancelled {
            return self.stop(StopReason::Cancelled);
        }
        if failure == Some(Reason::Revoked) {
            return self.stop(StopReason::Revoked);
        }
        if let Some(error) = failure {
            // No refusal can follow an accepted header; reconnect discards the
            // incomplete response and never resumes its body.
            if emitted == 0 && error == Reason::OverBudget {
                return self.refuse(id, error);
            }
            return self.disconnect(TransportError::Unavailable);
        }
        if let Some((lease_id, deadline, _)) = append {
            self.leases.insert(
                lease_id,
                OriginLease {
                    request: id,
                    deadline,
                    binding: lease_binding,
                },
            );
        }
        Step::Delivered
    }
    fn refuse(&mut self, id: Identifier, reason: Reason) -> Step {
        let bytes = encode(
            id,
            0,
            true,
            FrameContent::Refused {
                reason,
                retryable: reason.retryable(),
            },
            Vec::new(),
        );
        match bytes {
            Ok(bytes) => match self.transport.post_response(id, &bytes) {
                Ok(()) => Step::Refused(reason),
                Err(error) => self.disconnect(error),
            },
            Err(_) => Step::Dropped,
        }
    }
    /// Drives steps until stopped or scheduling is required. The host must yield
    /// until next_poll_not_before on Waiting; this wrapper never busy-waits it.
    pub fn run(&mut self, stop: &dyn Fn() -> bool) -> Step {
        loop {
            if stop() {
                return self.stop(StopReason::Cancelled);
            }
            if let terminal @ (Step::Waiting { .. } | Step::Stopped(_)) = self.step() {
                return terminal;
            }
        }
    }
}
fn request_id(bytes: &[u8]) -> Option<Identifier> {
    // Bounded prefix extraction also permits refusing an oversized valid envelope
    // whose correlation field is present. Never parse an unbounded JSON object.
    let prefix = bytes.get(..bytes.len().min(wire::CONTROL_CAP))?;
    struct Correlation<'a>(&'a std::cell::Cell<Option<Identifier>>);
    impl<'de> serde::de::Visitor<'de> for Correlation<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("request object")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
            while let Some(key) = map.next_key::<String>()? {
                if key == "request_id" {
                    self.0.set(Some(map.next_value::<Identifier>()?));
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }
    // Correlation alone is safe even if a later field is malformed or exceeds
    // the cap. It can only address a closed refusal, never authorize dispatch.
    let id = std::cell::Cell::new(None);
    use serde::Deserializer;
    let _ = serde_json::Deserializer::from_slice(prefix).deserialize_map(Correlation(&id));
    id.get()
}
fn reason(error: origin_read::Refused) -> Reason {
    match error {
        origin_read::Refused::Unavailable => Reason::Unavailable,
        origin_read::Refused::Forbidden => Reason::Forbidden,
        origin_read::Refused::PolicyChanged => Reason::PolicyChanged,
        origin_read::Refused::Deadline => Reason::Deadline,
        origin_read::Refused::OverBudget => Reason::OverBudget,
        origin_read::Refused::AuditFailure => Reason::AuditFailure,
        origin_read::Refused::InvalidRequest => Reason::InvalidRequest,
    }
}
fn encode(
    id: Identifier,
    number: u32,
    final_frame: bool,
    content: FrameContent,
    payload: Vec<u8>,
) -> wire::Result<Zeroizing<Vec<u8>>> {
    let frame = Frame {
        header: FrameHeader {
            version: wire::VERSION,
            request_id: id,
            frame: number,
            final_frame,
            payload_bytes: payload.len() as u32,
            content,
        },
        payload: Zeroizing::new(payload),
    };
    wire::encode_frame(&frame)
}
type EncodedFrames = Vec<(Zeroizing<Vec<u8>>, u64)>;
fn response_frames(
    id: Identifier,
    authorized: &origin_read::Authorized,
    limit: usize,
) -> wire::Result<EncodedFrames> {
    let payload = authorized
        .page
        .as_ref()
        .map(|page| page.canonical_bytes())
        .transpose()
        .map_err(reason)?;
    let mut frames = vec![(
        encode(
            id,
            0,
            payload.is_none(),
            FrameContent::Header {
                binding: Box::new(authorized.response_binding.clone()),
            },
            Vec::new(),
        )?,
        0,
    )];
    if let Some(payload) = payload {
        frames.push((
            encode(id, 1, true, FrameContent::Data, payload.to_vec())?,
            payload.len() as u64,
        ));
    }
    let total: usize = frames.iter().map(|(bytes, _)| bytes.len()).sum();
    if total > limit.min(wire::RESPONSE_CAP) {
        return Err(Reason::OverBudget);
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_is_bounded_and_saturating() {
        let mut b = Backoff::default();
        b.fail(100);
        assert_eq!(b.next_poll_not_before, 350);
        b.fail(100);
        assert_eq!(b.next_poll_not_before, 600);
        for _ in 0..40 {
            b.fail(100);
        }
        assert_eq!(b.next_poll_not_before, 20_100);
        b.fail(u64::MAX);
        assert_eq!(b.next_poll_not_before, u64::MAX);
        b.reset();
        b.fail(0);
        assert_eq!(b.next_poll_not_before, 250);
    }
    #[test]
    fn cache_limits_are_strict_and_only_narrow() {
        use origin_read::CacheLimits;
        let defaults = crate::body_cache::Limits::default();
        let absent: CacheLimits = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.narrowed(defaults).ttl_seconds, 900);
        let larger: CacheLimits =
            serde_json::from_str(r#"{"cache_ttl_seconds":9999,"cache_total_bytes":9999999999}"#)
                .unwrap();
        assert_eq!(larger.narrowed(defaults).ttl_seconds, 900);
        assert_eq!(
            larger.narrowed(defaults).global_bytes,
            defaults.global_bytes
        );
        for invalid in [
            r#"{"cache_ttl_seconds":0}"#,
            r#"{"cache_total_bytes":-1}"#,
            r#"{"extra":1}"#,
            r#"{"cache_per_source_bytes":"1"}"#,
        ] {
            assert!(serde_json::from_str::<CacheLimits>(invalid).is_err());
        }
    }
    #[test]
    fn correlation_is_bounded_and_strict() {
        let id = Identifier::from_bytes([3; 32]);
        let bytes =
            serde_json::to_vec(&serde_json::json!({"request_id": id, "verb":"unknown"})).unwrap();
        assert_eq!(request_id(&bytes), Some(id));
        assert_eq!(request_id(b"{bad"), None);
        assert_eq!(request_id(&vec![b' '; wire::CONTROL_CAP + 1]), None);
    }
}
