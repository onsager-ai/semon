//! Origin body-read core, with no transport or enrollment persistence.
//!
//! Integrations supply one authenticated enrollment, admitted source resolution,
//! complete contributor governance and a bounded typed renderer. No identifier is
//! a path or authority. body_adapter supplies the bounded origin-local view;
//! exact export-view evidence remains an integration dependency. Never substitute
//! its local UID lookup for remote authority.

use crate::{
    body_assertion::Claims,
    body_cache::{CacheKey, Clock, Selector, SourceWitness},
    origin_audit::{AuditStore, AuthorizationId, AuthorizationRecord},
    receiver_audit::{
        AuthorizationBinding, Decision, Destination, Identifier, Outcome, PermittedRange, Purpose,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::Zeroizing;

pub const MAX_BYTES: usize = 256 * 1024;
pub const FOCUSED_MILLIS: u64 = 2_000;
type Result<T> = std::result::Result<T, Refused>;

/// Closed public reasons. Unknown handles and scope/authority failures all use
/// Forbidden; implementations must not attach source text or detailed errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Unavailable,
    Forbidden,
    PolicyChanged,
    Deadline,
    OverBudget,
    AuditFailure,
    InvalidRequest,
}
impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Refused {}

/// Capped opaque integration identifier; never accepts paths or free text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpaqueId(String);
impl OpaqueId {
    pub fn new(value: &str) -> Result<Self> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:@".contains(&b))
        {
            return Err(Refused::InvalidRequest);
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for OpaqueId {
    type Error = Refused;
    fn try_from(value: String) -> Result<Self> {
        Self::new(&value)
    }
}
impl From<OpaqueId> for String {
    fn from(value: OpaqueId) -> Self {
        value.0
    }
}
impl fmt::Debug for OpaqueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OpaqueId([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum EnrollmentKind {
    Analytics,
    BodyChannel,
    Mirror,
}

/// Logical authenticated_enrollment output only. No credentials, key custody,
/// transport encoding or operator-visible setup is defined here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentScope {
    pub enrollment_id: OpaqueId,
    pub machine: OpaqueId,
    pub namespace: OpaqueId,
    pub origin_incarnation: OpaqueId,
    pub index_incarnation: OpaqueId,
    pub publisher_claim_epoch: Option<u64>,
    pub kinds: Vec<EnrollmentKind>,
    pub verified_client_identity: OpaqueId,
}
pub trait AuthenticatedEnrollment {
    /// Re-read the live authenticated scope. Revoked/withdrawn enrollment refuses.
    fn authenticated_enrollment(&self) -> Result<EnrollmentScope>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Page(OpaqueId),
    Field(OpaqueId),
}
impl Target {
    fn selector(&self) -> Selector {
        match self {
            Self::Page(id) => Selector::Page(id.0.clone()),
            Self::Field(id) => Selector::Field(id.0.clone()),
        }
    }
    fn purpose(&self) -> Purpose {
        match self {
            Self::Page(_) => Purpose::TranscriptPage,
            Self::Field(_) => Purpose::FieldExpansion,
        }
    }
}

/// Only the resolver interprets admitted_source, using native-root custody and
/// before/after identity fences. The cohort must be complete, not a first-file hash.
#[derive(Clone)]
pub struct ResolvedSource {
    pub admitted_source: OpaqueId,
    pub cohort_id: Identifier,
    pub cohort: Vec<SourceWitness>,
    pub activity_generation: OpaqueId,
    pub transcript_generation: OpaqueId,
}
pub trait HandleResolver {
    /// None means no admitted handle in this scope (including foreign handles).
    fn resolve(&self, scope: &EnrollmentScope, handle: &OpaqueId)
    -> Result<Option<ResolvedSource>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedPolicy {
    /// Complete comparable contributor-policy, scan, redactor, source witness,
    /// enrollment, claim-epoch and revocation vector, not just a policy version.
    pub vector: Identifier,
    pub policy_version: Identifier,
    pub redactor_version: Identifier,
}
pub trait PolicyVector {
    /// Evaluate every contributor for the authenticated subject and typed target.
    /// Governance adapters must enforce governance::body_read_allowed and field
    /// permissions. Missing contributor/scan/consent evidence refuses uniformly.
    fn evaluate(
        &self,
        scope: &EnrollmentScope,
        claims: &Claims,
        source: &ResolvedSource,
        target: &Target,
    ) -> Result<PinnedPolicy>;
}

/// Exact opaque anchor/mapping assertion; it is never a native byte proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactMapping {
    pub anchor: OpaqueId,
    pub assertion: Identifier,
}
#[derive(Clone)]
pub struct RenderPlan {
    /// Length of canonical typed payload, including its untrusted_text wrappers.
    /// Preparation is metadata-only where supported. The body_query adapter's
    /// documented bounded fallback discards a render to measure length; without
    /// exact export evidence the core refuses before authorization or egress.
    pub payload_bytes: u64,
    pub export_view_generation: Option<OpaqueId>,
    pub exact_mapping: Option<ExactMapping>,
    pub continuation: Option<OpaqueId>,
}

/// The only body-bearing value. Formatting never prints its content.
pub struct UntrustedText(Zeroizing<String>);
impl UntrustedText {
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for UntrustedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UntrustedText([redacted])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    User,
    Assistant,
    Command,
    ToolOutput,
    Error,
}
#[derive(Debug)]
pub struct TypedField {
    pub field: Field,
    pub text: UntrustedText,
}
#[derive(Debug)]
pub struct TypedPage {
    pub fields: Vec<TypedField>,
}
impl TypedPage {
    /// Canonical library bytes for length/digest only, not a channel wire format.
    /// Every value is explicitly tagged untrusted_text. Framing is big-endian u64
    /// lengths and fixed field tags, independent of JSON/object-map ordering.
    pub fn canonical_bytes(&self) -> Result<Zeroizing<Vec<u8>>> {
        let mut out = Zeroizing::new(Vec::new());
        out.extend_from_slice(b"semon/typed-body/v1\0");
        out.extend_from_slice(&(self.fields.len() as u64).to_be_bytes());
        for field in &self.fields {
            let tag = match field.field {
                Field::User => 0,
                Field::Assistant => 1,
                Field::Command => 2,
                Field::ToolOutput => 3,
                Field::Error => 4,
            };
            let extra = 1usize
                .checked_add(8 + b"untrusted_text".len())
                .and_then(|n| n.checked_add(8))
                .and_then(|n| n.checked_add(field.text.as_str().len()))
                .ok_or(Refused::OverBudget)?;
            if out.len().checked_add(extra).is_none_or(|n| n > MAX_BYTES) {
                return Err(Refused::OverBudget);
            }
            out.push(tag);
            append(&mut out, b"untrusted_text");
            append(&mut out, field.text.as_str().as_bytes());
        }
        if self
            .fields
            .iter()
            .all(|field| field.text.as_str().is_empty())
        {
            return Err(Refused::Unavailable);
        }
        Ok(out)
    }
}
pub struct RenderedPage {
    pub page: TypedPage,
    pub export_view_generation: Option<OpaqueId>,
    pub exact_mapping: Option<ExactMapping>,
    pub continuation: Option<OpaqueId>,
}
#[derive(Clone, Copy, Debug)]
pub struct RenderBudget {
    pub bytes: usize,
    /// Absolute epoch milliseconds, narrowed to the two-second focused budget.
    pub deadline_millis: u64,
}
pub trait PageRenderer {
    /// Metadata preparation shares the operation's deadline and cancellation.
    /// Its byte ceiling is narrowed further for binding overhead before rendering.
    fn prepare(
        &self,
        source: &ResolvedSource,
        target: &Target,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderPlan>;
    /// Use only admitted origin-local typed fields and preserve untrusted_text.
    /// No whole-session replay/inference. Honor budget and cancellation while
    /// reading; the core additionally checks clock, length and witnesses on return.
    fn render(
        &self,
        source: &ResolvedSource,
        target: &Target,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderedPage>;
}

/// Page-local half-open offsets into the canonical exported typed payload.
/// Each page/field starts at zero; continuation is an identity, never an offset.
/// Withheld/empty targets are unavailable, never a zero range or native proof.
pub fn exported_range(
    _target: &Target,
    payload_bytes: u64,
    _continuation: Option<&OpaqueId>,
) -> Result<PermittedRange> {
    if payload_bytes == 0 {
        return Err(Refused::Unavailable);
    }
    if payload_bytes > MAX_BYTES as u64 {
        return Err(Refused::OverBudget);
    }
    PermittedRange::new(0, payload_bytes).ok_or(Refused::InvalidRequest)
}

pub struct ReadRequest {
    pub source_handle: OpaqueId,
    pub target: Target,
    pub anchor: OpaqueId,
    /// Digest of the exact request verified by body_assertion. The integration
    /// computes it from its accepted request; callers cannot substitute selectors.
    pub request_digest: [u8; 32],
    pub channel_binding: [u8; 32],
    pub max_bytes: usize,
    pub destination: Destination,
}

/// Optional origin cache ceilings. Zero is invalid; larger values are clamped
/// against receiver defaults. These values convey custody limits, never authority.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheLimits {
    pub cache_ttl_seconds: Option<std::num::NonZeroU64>,
    pub cache_per_source_bytes: Option<std::num::NonZeroU64>,
    pub cache_total_bytes: Option<std::num::NonZeroU64>,
}
impl CacheLimits {
    pub fn narrowed(self, receiver: crate::body_cache::Limits) -> crate::body_cache::Limits {
        let ceiling = crate::body_cache::Limits::default();
        crate::body_cache::Limits {
            ttl_seconds: receiver.ttl_seconds.min(ceiling.ttl_seconds).min(
                self.cache_ttl_seconds
                    .map_or(u64::MAX, std::num::NonZeroU64::get),
            ),
            source_bytes: receiver.source_bytes.min(ceiling.source_bytes).min(
                self.cache_per_source_bytes
                    .map_or(u64::MAX, std::num::NonZeroU64::get),
            ),
            global_bytes: receiver.global_bytes.min(ceiling.global_bytes).min(
                self.cache_total_bytes
                    .map_or(u64::MAX, std::num::NonZeroU64::get),
            ),
            response_bytes: receiver.response_bytes.min(ceiling.response_bytes),
        }
    }
    fn bind(self, bytes: &mut Vec<u8>) {
        for value in [
            self.cache_ttl_seconds,
            self.cache_per_source_bytes,
            self.cache_total_bytes,
        ] {
            bytes.extend_from_slice(&value.map_or(0, std::num::NonZeroU64::get).to_be_bytes());
        }
    }
}
pub trait OriginLimits {
    fn cache_limits(&self) -> Result<CacheLimits>;
}
impl OriginLimits for CacheLimits {
    fn cache_limits(&self) -> Result<CacheLimits> {
        Ok(*self)
    }
}

/// Exact response identity, also carried by the strict body-channel header.
/// Receiver verifies this binding then passes receiver_binding to authorize().
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseBinding {
    pub request_digest: [u8; 32],
    pub enrollment: EnrollmentScope,
    pub source_handle: OpaqueId,
    pub cohort_id: Identifier,
    pub cache_key: CacheKey,
    pub mapping: ExactMapping,
    pub continuation: Option<OpaqueId>,
    pub permitted_range: PermittedRange,
    pub origin_authorization: AuthorizationId,
    pub policy: PinnedPolicy,
    pub max_bytes: usize,
    pub deadline_millis: u64,
    pub receiver_binding: AuthorizationBinding,
    pub cache_limits: CacheLimits,
}
impl ResponseBinding {
    /// Recompute the origin's exact response identity using the signed claims.
    pub fn verify_identity(&self, claims: &Claims, destination: Destination) -> Result<()> {
        let target = match &self.cache_key.selector {
            Selector::Page(id) => Target::Page(OpaqueId::new(id)?),
            Selector::Field(id) => Target::Field(OpaqueId::new(id)?),
        };
        let request = ReadRequest {
            source_handle: self.source_handle.clone(),
            target,
            anchor: self.mapping.anchor.clone(),
            request_digest: self.request_digest,
            channel_binding: claims.channel_binding,
            max_bytes: self.max_bytes,
            destination,
        };
        validate_claims(claims, &request, &self.enrollment)?;
        let source = ResolvedSource {
            admitted_source: self.source_handle.clone(),
            cohort_id: self.cohort_id,
            cohort: self.cache_key.cohort.clone(),
            activity_generation: OpaqueId::new(&self.cache_key.activity_generation)?,
            transcript_generation: OpaqueId::new(&self.cache_key.transcript_generation)?,
        };
        let plan = RenderPlan {
            payload_bytes: self.permitted_range.len(),
            export_view_generation: Some(OpaqueId::new(&self.cache_key.export_view_generation)?),
            exact_mapping: Some(self.mapping.clone()),
            continuation: self.continuation.clone(),
        };
        let key = cache_key(
            &self.enrollment,
            &source,
            &request,
            plan.export_view_generation
                .as_ref()
                .ok_or(Refused::InvalidRequest)?,
            self.policy,
            claims,
            self.cache_key.payload_digest,
        );
        let mut identity = binding_bytes_for_response(
            &self.enrollment,
            &source,
            &request,
            &key,
            &plan,
            self.policy,
            self.deadline_millis,
            self.max_bytes,
            self.origin_authorization,
            claims,
        );
        self.cache_limits.bind(&mut identity);
        if key != self.cache_key || self.receiver_binding.response_identity != identifier(&identity)
        {
            return Err(Refused::InvalidRequest);
        }
        Ok(())
    }
}

/// Non-Clone result. No egress has happened; use finish after actual delivery.
pub struct Authorized {
    pub page: Option<TypedPage>,
    pub response_binding: ResponseBinding,
}
impl Authorized {
    /// Integration calls this after egress (or cancellation/failure) with actual
    /// payload bytes emitted. Failure stops further emission; a crash/drop leaves
    /// the durable authorization attempted, never falsely completed.
    pub fn finish<C: Clock>(
        self,
        audit: &mut AuditStore<C>,
        outcome: Outcome,
        bytes: u64,
    ) -> Result<()> {
        if outcome == Outcome::Denied
            || bytes > self.response_binding.permitted_range.len()
            || (self.page.is_none() && bytes != 0)
        {
            return Err(Refused::InvalidRequest);
        }
        audit
            .complete(self.response_binding.origin_authorization, outcome, bytes)
            .map_err(|_| Refused::AuditFailure)
    }
}

pub struct CachedCandidate {
    pub key: CacheKey,
    pub mapping: ExactMapping,
    pub continuation: Option<OpaqueId>,
    pub payload_bytes: u64,
}
pub trait ExportViewValidator {
    /// Validate the exact key/digest/mapping against origin-owned export evidence.
    /// Analytics SourceWitness alone is insufficient. No cache body lookup here.
    fn validate(
        &self,
        candidate: &CachedCandidate,
        source: &ResolvedSource,
        policy: PinnedPolicy,
    ) -> Result<()>;
}
pub struct UnavailableExportView;
impl ExportViewValidator for UnavailableExportView {
    fn validate(&self, _: &CachedCandidate, _: &ResolvedSource, _: PinnedPolicy) -> Result<()> {
        Err(Refused::Unavailable)
    }
}

/// Injected epoch-millisecond clock. Assertion/audit clocks remain epoch seconds.
pub trait ReadClock {
    fn now_millis(&self) -> u64;
}
pub struct OriginRead<'a, C: Clock> {
    pub enrollment: &'a dyn AuthenticatedEnrollment,
    pub resolver: &'a dyn HandleResolver,
    pub policies: &'a dyn PolicyVector,
    pub renderer: &'a dyn PageRenderer,
    pub clock: &'a dyn ReadClock,
    pub cancelled: &'a dyn Fn() -> bool,
    pub audit: &'a mut AuditStore<C>,
}
impl<C: Clock> OriginRead<'_, C> {
    /// claims MUST be the successful body_assertion::verify result (signature,
    /// pinned issuer and consumed nonce); this function does not repeat crypto.
    pub fn read(&mut self, claims: &Claims, request: &ReadRequest) -> Result<Authorized> {
        self.read_with_cache_limits(claims, request, CacheLimits::default())
    }
    pub fn read_with_cache_limits(
        &mut self,
        claims: &Claims,
        request: &ReadRequest,
        limits: CacheLimits,
    ) -> Result<Authorized> {
        self.run(claims, request, None, limits)
    }
    pub fn authorization_only(
        &mut self,
        claims: &Claims,
        request: &ReadRequest,
        candidate: &CachedCandidate,
        validator: &dyn ExportViewValidator,
    ) -> Result<Authorized> {
        self.authorization_only_with_cache_limits(
            claims,
            request,
            candidate,
            validator,
            CacheLimits::default(),
        )
    }
    pub fn authorization_only_with_cache_limits(
        &mut self,
        claims: &Claims,
        request: &ReadRequest,
        candidate: &CachedCandidate,
        validator: &dyn ExportViewValidator,
        limits: CacheLimits,
    ) -> Result<Authorized> {
        self.run(claims, request, Some((candidate, validator)), limits)
    }
    fn run(
        &mut self,
        claims: &Claims,
        request: &ReadRequest,
        cached: Option<(&CachedCandidate, &dyn ExportViewValidator)>,
        cache_limits: CacheLimits,
    ) -> Result<Authorized> {
        let scope = self
            .enrollment
            .authenticated_enrollment()
            .map_err(|_| Refused::Forbidden)?;
        if scope.kinds.len() > 3
            || scope
                .kinds
                .iter()
                .enumerate()
                .any(|(i, kind)| scope.kinds[..i].contains(kind))
        {
            return Err(Refused::Forbidden);
        }
        validate_claims(claims, request, &scope)?;
        let now = self.clock.now_millis();
        let deadline = claims
            .deadline
            .checked_mul(1000)
            .ok_or(Refused::InvalidRequest)?;
        let expiry = claims
            .expiry
            .checked_mul(1000)
            .ok_or(Refused::InvalidRequest)?;
        let deadline = deadline.min(expiry).min(now.saturating_add(FOCUSED_MILLIS));
        check_deadline(self.clock, deadline)?;
        if (self.cancelled)() {
            return Err(Refused::Unavailable);
        }
        let limit = request.max_bytes.min(MAX_BYTES);
        if limit == 0 {
            return Err(Refused::OverBudget);
        }
        let source = self
            .resolver
            .resolve(&scope, &request.source_handle)
            .map_err(|_| Refused::Forbidden)?
            .ok_or(Refused::Forbidden)?;
        validate_source(&source)?;
        let policy = self
            .policies
            .evaluate(&scope, claims, &source, &request.target)
            .map_err(|_| Refused::Forbidden)?;
        let plan = if let Some((candidate, validator)) = cached {
            validator.validate(candidate, &source, policy)?;
            RenderPlan {
                payload_bytes: candidate.payload_bytes,
                export_view_generation: Some(OpaqueId::new(&candidate.key.export_view_generation)?),
                exact_mapping: Some(candidate.mapping.clone()),
                continuation: candidate.continuation.clone(),
            }
        } else {
            self.renderer.prepare(
                &source,
                &request.target,
                policy,
                RenderBudget {
                    bytes: limit,
                    deadline_millis: deadline,
                },
                self.cancelled,
            )?
        };
        check_deadline(self.clock, deadline)?;
        if (self.cancelled)() {
            return Err(Refused::Unavailable);
        }
        let export = plan
            .export_view_generation
            .as_ref()
            .ok_or(Refused::Unavailable)?;
        let mapping = plan.exact_mapping.as_ref().ok_or(Refused::Unavailable)?;
        if mapping.anchor != request.anchor {
            return Err(Refused::Unavailable);
        }
        let range = exported_range(
            &request.target,
            plan.payload_bytes,
            plan.continuation.as_ref(),
        )?;
        let key = cache_key(
            &scope,
            &source,
            request,
            export,
            policy,
            claims,
            cached.map_or([0; 32], |(c, _)| c.key.payload_digest),
        );
        if let Some((candidate, _)) = cached
            && candidate.key != key
        {
            return Err(Refused::Unavailable);
        }
        // Fixed-width digests/IDs have the same encoded size before and after render.
        let mut binding_bytes = binding_bytes(
            &scope, &source, request, &key, &plan, policy, deadline, limit,
        );
        append(&mut binding_bytes, &claims.nonce);
        append(&mut binding_bytes, claims.subject_principal.as_bytes());
        append(&mut binding_bytes, claims.acl_grant_version.as_bytes());
        append(
            &mut binding_bytes,
            claims.receiver_policy_version.as_bytes(),
        );
        id_bytes(&mut binding_bytes, Identifier::from_bytes([0; 32]));
        cache_limits.bind(&mut binding_bytes);
        // Include the response identity digest in addition to all binding fields.
        let metadata_bytes = binding_bytes
            .len()
            .checked_add(32)
            .ok_or(Refused::OverBudget)?;
        let payload_limit = limit
            .checked_sub(metadata_bytes)
            .ok_or(Refused::OverBudget)?;
        if plan.payload_bytes > payload_limit as u64 {
            return Err(Refused::OverBudget);
        }
        check_deadline(self.clock, deadline)?;
        let audit_id = self
            .audit
            .begin_authorization(AuthorizationRecord {
                request_id: Identifier::from_bytes(claims.nonce),
                authenticated_principal: identifier(claims.subject_principal.as_bytes()),
                source_handle: identifier(request.source_handle.as_str().as_bytes()),
                generation: source.cohort_id,
                permitted_range: range,
                purpose: request.target.purpose(),
                receiver_policy_version: identifier(claims.receiver_policy_version.as_bytes()),
                origin_policy_version: policy.policy_version,
                origin_redactor_version: policy.redactor_version,
                enrollment_scope: identifier(&scope_bytes(&scope)),
                export_view_binding: identifier(&binding_bytes),
                decision: Decision::Granted,
                destination: request.destination,
            })
            .map_err(|_| Refused::AuditFailure)?;
        let result = (|| {
            check_deadline(self.clock, deadline)?;
            if (self.cancelled)() {
                return Err(Refused::Unavailable);
            }
            let mut key = key;
            let page = if cached.is_some() {
                None
            } else {
                let rendered = self.renderer.render(
                    &source,
                    &request.target,
                    policy,
                    RenderBudget {
                        bytes: payload_limit,
                        deadline_millis: deadline,
                    },
                    self.cancelled,
                )?;
                if rendered.export_view_generation != plan.export_view_generation
                    || rendered.exact_mapping != plan.exact_mapping
                    || rendered.continuation != plan.continuation
                {
                    return Err(Refused::PolicyChanged);
                }
                let bytes = rendered.page.canonical_bytes()?;
                if bytes.len() > payload_limit {
                    return Err(Refused::OverBudget);
                }
                if bytes.len() as u64 != plan.payload_bytes {
                    return Err(Refused::Unavailable);
                }
                key.payload_digest = Sha256::digest(&*bytes).into();
                Some(rendered.page)
            };
            if (self.cancelled)() {
                return Err(Refused::Unavailable);
            }
            check_deadline(self.clock, deadline)?;
            // Both the live enrollment and complete source/policy witness are
            // re-read immediately before creating an egress result.
            let current_scope = self
                .enrollment
                .authenticated_enrollment()
                .map_err(|_| Refused::PolicyChanged)?;
            if current_scope != scope {
                return Err(Refused::PolicyChanged);
            }
            let current_source = self
                .resolver
                .resolve(&scope, &request.source_handle)
                .map_err(|_| Refused::PolicyChanged)?
                .ok_or(Refused::PolicyChanged)?;
            if !same_source(&source, &current_source)
                || self
                    .policies
                    .evaluate(&scope, claims, &current_source, &request.target)
                    .map_err(|_| Refused::PolicyChanged)?
                    != policy
            {
                return Err(Refused::PolicyChanged);
            }
            if let Some((candidate, validator)) = cached {
                validator
                    .validate(candidate, &current_source, policy)
                    .map_err(|_| Refused::PolicyChanged)?;
            }
            check_deadline(self.clock, deadline)?;
            binding_bytes = binding_bytes_for_response(
                &scope, &source, request, &key, &plan, policy, deadline, limit, audit_id, claims,
            );
            cache_limits.bind(&mut binding_bytes);
            let receiver_binding = AuthorizationBinding {
                request_id: Identifier::from_bytes(claims.nonce),
                response_identity: identifier(&binding_bytes),
                origin_policy_version: policy.policy_version,
                origin_redactor_version: policy.redactor_version,
                origin_authorization_id: audit_id.identifier(),
            };
            check_deadline(self.clock, deadline)?;
            if (self.cancelled)() {
                return Err(Refused::Unavailable);
            }
            Ok(Authorized {
                page,
                response_binding: ResponseBinding {
                    request_digest: request.request_digest,
                    enrollment: scope,
                    source_handle: request.source_handle.clone(),
                    cohort_id: source.cohort_id,
                    cache_key: key,
                    mapping: mapping.clone(),
                    continuation: plan.continuation.clone(),
                    permitted_range: range,
                    origin_authorization: audit_id,
                    policy,
                    max_bytes: limit,
                    deadline_millis: deadline,
                    receiver_binding,
                    cache_limits,
                },
            })
        })();
        match result {
            Ok(authorized) => Ok(authorized),
            Err(reason) => {
                let outcome = if (self.cancelled)() {
                    Outcome::Cancelled
                } else {
                    Outcome::Failed
                };
                self.audit
                    .complete(audit_id, outcome, 0)
                    .map_err(|_| Refused::AuditFailure)?;
                Err(reason)
            }
        }
    }
}

fn validate_claims(c: &Claims, r: &ReadRequest, s: &EnrollmentScope) -> Result<()> {
    if c.audience.origin_incarnation != s.origin_incarnation.as_str()
        || c.audience.namespace != s.namespace.as_str()
        || c.machine != s.machine.as_str()
        || c.namespace != s.namespace.as_str()
        || s.publisher_claim_epoch != Some(c.claim_epoch)
        || !s.kinds.contains(&EnrollmentKind::BodyChannel)
        || c.channel_binding != r.channel_binding
        || c.request_digest != r.request_digest
    {
        return Err(Refused::Forbidden);
    }
    Ok(())
}
fn check_deadline(clock: &dyn ReadClock, deadline: u64) -> Result<()> {
    if clock.now_millis() >= deadline {
        Err(Refused::Deadline)
    } else {
        Ok(())
    }
}
fn validate_source(source: &ResolvedSource) -> Result<()> {
    if source.cohort.is_empty() || source.cohort.len() > 128 {
        return Err(Refused::Unavailable);
    }
    for (i, s) in source.cohort.iter().enumerate() {
        for id in [&s.source_id, &s.chain, &s.generation] {
            OpaqueId::new(id).map_err(|_| Refused::Unavailable)?;
        }
        if source.cohort[..i]
            .iter()
            .any(|other| other.source_id == s.source_id)
        {
            return Err(Refused::Unavailable);
        }
    }
    Ok(())
}
fn same_source(a: &ResolvedSource, b: &ResolvedSource) -> bool {
    a.admitted_source == b.admitted_source
        && a.cohort_id == b.cohort_id
        && a.cohort == b.cohort
        && a.activity_generation == b.activity_generation
        && a.transcript_generation == b.transcript_generation
}
fn cache_key(
    s: &EnrollmentScope,
    source: &ResolvedSource,
    r: &ReadRequest,
    export: &OpaqueId,
    p: PinnedPolicy,
    claims: &Claims,
    digest: [u8; 32],
) -> CacheKey {
    let mut authorization_scope = scope_bytes(s);
    append(
        &mut authorization_scope,
        claims.subject_principal.as_bytes(),
    );
    append(
        &mut authorization_scope,
        claims.acl_grant_version.as_bytes(),
    );
    append(
        &mut authorization_scope,
        claims.receiver_policy_version.as_bytes(),
    );
    CacheKey {
        namespace: s.namespace.0.clone(),
        origin_incarnation: s.origin_incarnation.0.clone(),
        cohort: source.cohort.clone(),
        activity_generation: source.activity_generation.0.clone(),
        transcript_generation: source.transcript_generation.0.clone(),
        export_view_generation: export.0.clone(),
        selector: r.target.selector(),
        payload_digest: digest,
        policy_versions: vec![
            String::from(p.vector),
            String::from(p.policy_version),
            String::from(p.redactor_version),
        ],
        authorization_scope: String::from(identifier(&authorization_scope)),
    }
}
fn identifier(bytes: &[u8]) -> Identifier {
    Identifier::from_bytes(Sha256::digest(bytes).into())
}
fn append(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}
fn id_bytes(out: &mut Vec<u8>, id: Identifier) {
    append(out, String::from(id).as_bytes());
}
fn optional(out: &mut Vec<u8>, id: Option<&OpaqueId>) {
    match id {
        Some(id) => {
            out.push(1);
            append(out, id.as_str().as_bytes());
        }
        None => out.push(0),
    }
}
fn scope_bytes(s: &EnrollmentScope) -> Vec<u8> {
    let mut out = Vec::new();
    for id in [
        &s.enrollment_id,
        &s.machine,
        &s.namespace,
        &s.origin_incarnation,
        &s.index_incarnation,
        &s.verified_client_identity,
    ] {
        append(&mut out, id.as_str().as_bytes());
    }
    match s.publisher_claim_epoch {
        Some(epoch) => {
            out.push(1);
            out.extend_from_slice(&epoch.to_be_bytes());
        }
        None => out.push(0),
    }
    out.extend_from_slice(&(s.kinds.len() as u64).to_be_bytes());
    for kind in &s.kinds {
        out.push(match kind {
            EnrollmentKind::Analytics => 0,
            EnrollmentKind::BodyChannel => 1,
            EnrollmentKind::Mirror => 2,
        });
    }
    out
}
#[allow(clippy::too_many_arguments)]
fn binding_bytes(
    s: &EnrollmentScope,
    source: &ResolvedSource,
    r: &ReadRequest,
    key: &CacheKey,
    plan: &RenderPlan,
    policy: PinnedPolicy,
    deadline: u64,
    limit: usize,
) -> Vec<u8> {
    let mut out = b"semon/origin-binding/v1\0".to_vec();
    append(&mut out, &r.request_digest);
    append(&mut out, &scope_bytes(s));
    append(&mut out, r.source_handle.as_str().as_bytes());
    id_bytes(&mut out, source.cohort_id);
    out.extend_from_slice(&(source.cohort.len() as u64).to_be_bytes());
    for member in &source.cohort {
        for id in [&member.source_id, &member.chain, &member.generation] {
            append(&mut out, id.as_bytes());
        }
        out.extend_from_slice(&member.consumed_boundary.to_be_bytes());
    }
    for id in [&source.activity_generation, &source.transcript_generation] {
        append(&mut out, id.as_str().as_bytes());
    }
    optional(&mut out, plan.export_view_generation.as_ref());
    out.push(match r.target {
        Target::Page(_) => 0,
        Target::Field(_) => 1,
    });
    match &r.target {
        Target::Page(id) | Target::Field(id) => append(&mut out, id.as_str().as_bytes()),
    }
    append(&mut out, r.anchor.as_str().as_bytes());
    if let Some(mapping) = &plan.exact_mapping {
        id_bytes(&mut out, mapping.assertion);
    }
    optional(&mut out, plan.continuation.as_ref());
    out.extend_from_slice(&plan.payload_bytes.to_be_bytes());
    append(&mut out, &key.payload_digest);
    for id in [
        policy.vector,
        policy.policy_version,
        policy.redactor_version,
    ] {
        id_bytes(&mut out, id);
    }
    out.extend_from_slice(&deadline.to_be_bytes());
    out.extend_from_slice(&(limit as u64).to_be_bytes());
    out.push(match r.destination {
        Destination::Viewer => 0,
        Destination::Cli => 1,
        Destination::Mcp => 2,
    });
    out
}
#[allow(clippy::too_many_arguments)]
fn binding_bytes_for_response(
    s: &EnrollmentScope,
    source: &ResolvedSource,
    r: &ReadRequest,
    key: &CacheKey,
    plan: &RenderPlan,
    policy: PinnedPolicy,
    deadline: u64,
    limit: usize,
    audit: AuthorizationId,
    claims: &Claims,
) -> Vec<u8> {
    let mut out = binding_bytes(s, source, r, key, plan, policy, deadline, limit);
    append(&mut out, &claims.nonce);
    append(&mut out, claims.subject_principal.as_bytes());
    append(&mut out, claims.acl_grant_version.as_bytes());
    append(&mut out, claims.receiver_policy_version.as_bytes());
    id_bytes(&mut out, audit.identifier());
    out
}

#[cfg(test)]
mod tests;
