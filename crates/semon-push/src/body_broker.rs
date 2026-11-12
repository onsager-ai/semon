//! Receiver-only body broker. Listener framing, TLS, credentials and custody are
//! supplied by enrollment integration; there are no routes or native body reads.
use crate::{
    body_assertion::{self, Audience, Claims, SigningKey},
    body_cache::{BodyCache, Clock, Limits, ReadAdmission},
    origin_read::{Field, TypedField, TypedPage, UntrustedText},
    receiver_acl::{Acls, Decision as AclDecision, Permission},
    receiver_audit::{
        self, AttemptRecord, AuditStore, Destination, Outcome, PermittedRange, Purpose,
    },
};
use crate::{
    body_wire::{self as wire, FrameContent, PollResponse, Reason, Request, Result},
    origin_read::{EnrollmentKind, EnrollmentScope, ReadClock, ResponseBinding},
    receiver_audit::Identifier,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Implemented by 4a using verified connection evidence and the request's kind
/// credential. Connection/credential data stays private to the implementation.
pub trait EnrollmentSource {
    fn authenticated_enrollment(&self, required_kind: EnrollmentKind) -> Result<EnrollmentScope>;
    /// Compare the complete current scope, including credential/kind revocation.
    fn is_current(&self, scope: &EnrollmentScope) -> bool;
    /// Capture request authority for pending work. Registry-backed sources return
    /// the handle obtained at authentication; in-memory sources have no registry.
    fn revision_handle(&self, _scope: &EnrollmentScope) -> Option<crate::serve::RevisionHandle> {
        None
    }
}

pub(crate) fn authority_is_current(
    handle: Option<&crate::serve::RevisionHandle>,
    source: &dyn EnrollmentSource,
    scope: &EnrollmentScope,
    now: u64,
) -> bool {
    match handle {
        Some(handle) => handle.is_current_at(now),
        None => source.is_current(scope),
    }
}
#[derive(Clone, Copy)]
pub struct Config {
    pub max_connections: usize,
    pub poll_share_percent: u8,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            max_connections: 32,
            poll_share_percent: 50,
        }
    }
}
struct Work {
    scope: EnrollmentScope,
    authority: Option<crate::serve::RevisionHandle>,
    channel: [u8; 32],
    request: Request,
    dispatched: bool,
    next_frame: u32,
    encoded_bytes: usize,
    binding: Option<ResponseBinding>,
    payload: Vec<u8>,
    result: Option<Result<VerifiedResponse>>,
}
struct RequestCleanup<'a, 'b> {
    broker: &'a Broker<'b>,
    id: Identifier,
}
impl Drop for RequestCleanup<'_, '_> {
    fn drop(&mut self) {
        let _ = self.broker.cancel(self.id);
    }
}
pub struct VerifiedResponse {
    pub binding: ResponseBinding,
    pub payload: Vec<u8>,
}
struct Channel {
    binding: [u8; 32],
    scope: EnrollmentScope,
    authority: Option<crate::serve::RevisionHandle>,
}
#[derive(Default)]
struct State {
    polls: BTreeMap<String, [u8; 32]>,
    channels: BTreeMap<String, Channel>,
    queues: BTreeMap<String, VecDeque<Identifier>>,
    work: BTreeMap<Identifier, Work>,
}
pub struct Broker<'a> {
    pub(crate) clock: &'a (dyn ReadClock + Sync),
    poll_cap: usize,
    state: Mutex<State>,
    changed: Condvar,
}
/// A live poll reservation, released on every return/drop path.
pub struct Poll<'a, 'b> {
    broker: &'a Broker<'b>,
    scope: EnrollmentScope,
    channel: [u8; 32],
    consumed: AtomicBool,
    deadline_millis: u64,
}
impl Drop for Poll<'_, '_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.broker.state.lock() {
            state.polls.remove(self.scope.enrollment_id.as_str());
        }
    }
}
impl<'a> Broker<'a> {
    pub fn new(config: Config, clock: &'a (dyn ReadClock + Sync)) -> Result<Self> {
        if config.poll_share_percent > 50 {
            return Err(Reason::InvalidRequest);
        }
        let poll_cap = config.max_connections / 100 * config.poll_share_percent as usize
            + config.max_connections % 100 * config.poll_share_percent as usize / 100;
        Ok(Self {
            clock,
            poll_cap,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        })
    }
    pub fn begin_poll<'b>(
        &'b self,
        auth: &dyn EnrollmentSource,
        bytes: &[u8],
    ) -> Result<Poll<'b, 'a>> {
        let scope = authenticate(auth)?;
        let poll = wire::parse_poll(bytes)?;
        let mut state = self.state.lock().map_err(|_| Reason::Unavailable)?;
        self.maintain(&mut state, auth);
        let id = scope.enrollment_id.as_str();
        if state.polls.contains_key(id) || state.polls.len() >= self.poll_cap {
            return Err(Reason::PollCapacity);
        }
        // Restarted origin channel invalidates every request signed for the old loop.
        if state
            .channels
            .get(id)
            .is_some_and(|c| c.binding != poll.channel_binding)
        {
            for work in state
                .work
                .values_mut()
                .filter(|w| w.scope.enrollment_id == scope.enrollment_id && w.result.is_none())
            {
                fail(work, Reason::Unavailable);
            }
        }
        state.channels.insert(
            id.into(),
            Channel {
                binding: poll.channel_binding,
                authority: auth.revision_handle(&scope),
                scope: scope.clone(),
            },
        );
        state.polls.insert(id.into(), poll.channel_binding);
        self.changed.notify_all();
        Ok(Poll {
            broker: self,
            scope,
            channel: poll.channel_binding,
            consumed: AtomicBool::new(false),
            deadline_millis: self.clock.now_millis().saturating_add(20_000),
        })
    }
    pub fn channel(&self, scope: &EnrollmentScope) -> Result<[u8; 32]> {
        self.state
            .lock()
            .map_err(|_| Reason::Unavailable)?
            .channels
            .get(scope.enrollment_id.as_str())
            .map(|channel| channel.binding)
            .ok_or(Reason::Unavailable)
    }
    pub fn enqueue(
        &self,
        auth: &dyn EnrollmentSource,
        scope: EnrollmentScope,
        channel: [u8; 32],
        request: Request,
    ) -> Result<()> {
        request.validate()?;
        if !request.operation.supported() {
            return Err(Reason::UnsupportedVerb);
        }
        wire::control(&PollResponse::Request {
            version: wire::VERSION,
            request: Box::new(request.clone()),
        })?;
        let mut state = self.state.lock().map_err(|_| Reason::Unavailable)?;
        self.maintain(&mut state, auth);
        if !auth.is_current(&scope) {
            return Err(Reason::Revoked);
        }
        if request.deadline_millis <= self.clock.now_millis() {
            return Err(Reason::Deadline);
        }
        if state
            .channels
            .get(scope.enrollment_id.as_str())
            .map(|c| c.binding)
            != Some(channel)
        {
            return Err(Reason::Unavailable);
        }
        if state.work.contains_key(&request.request_id) {
            return Err(Reason::InvalidRequest);
        }
        if state
            .work
            .values()
            .filter(|w| {
                w.scope.enrollment_id == scope.enrollment_id && !matches!(w.result, Some(Err(_)))
            })
            .count()
            >= 4
        {
            return Err(Reason::PendingQueueFull);
        }
        let queue = state
            .queues
            .entry(scope.enrollment_id.as_str().into())
            .or_default();
        if queue.len() >= 4 {
            return Err(Reason::PendingQueueFull);
        }
        queue.push_back(request.request_id);
        state.work.insert(
            request.request_id,
            Work {
                authority: auth.revision_handle(&scope),
                scope,
                channel,
                request,
                dispatched: false,
                next_frame: 0,
                encoded_bytes: 0,
                binding: None,
                payload: Vec::new(),
                result: None,
            },
        );
        self.changed.notify_all();
        Ok(())
    }
    fn maintain(&self, state: &mut State, auth: &dyn EnrollmentSource) {
        let now = self.clock.now_millis();
        for work in state
            .work
            .values_mut()
            .filter(|w| !matches!(w.result, Some(Err(_))))
        {
            if !authority_is_current(work.authority.as_ref(), auth, &work.scope, now / 1000) {
                fail(work, Reason::Revoked);
            } else if now >= work.request.deadline_millis {
                fail(work, Reason::Deadline);
            }
        }
        let terminal: Vec<_> = state
            .work
            .iter()
            .filter(|(_, w)| matches!(w.result, Some(Err(_))))
            .map(|(id, _)| *id)
            .collect();
        for id in terminal.into_iter().rev().skip(16) {
            state.work.remove(&id);
        }
        for queue in state.queues.values_mut() {
            queue.retain(|id| state.work.get(id).is_some_and(|w| w.result.is_none()));
        }
    }
    /// Caller cancellation/drop cleanup; no retained body or queue slot survives.
    pub fn cancel(&self, id: Identifier) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| Reason::Unavailable)?;
        state.work.remove(&id);
        for queue in state.queues.values_mut() {
            queue.retain(|queued| *queued != id);
        }
        self.changed.notify_all();
        Ok(())
    }
    /// 4a's in-flight revocation hook calls this and wakes held polls/readers.
    pub fn revocation_changed(&self, auth: &dyn EnrollmentSource) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| Reason::Unavailable)?;
        self.maintain(&mut state, auth);
        // A delayed notification may follow fresh authentication at the new
        // revision. Old work must not invalidate that replacement registration.
        state.channels.retain(|_, channel| {
            authority_is_current(
                channel.authority.as_ref(),
                auth,
                &channel.scope,
                self.clock.now_millis() / 1000,
            )
        });
        self.changed.notify_all();
        Ok(())
    }
    pub fn take_result(
        &self,
        auth: &dyn EnrollmentSource,
        id: Identifier,
    ) -> Result<Option<VerifiedResponse>> {
        let mut state = self.state.lock().map_err(|_| Reason::Unavailable)?;
        self.maintain(&mut state, auth);
        let work = state.work.get_mut(&id).ok_or(Reason::LateResponse)?;
        if let Some(result) = work.result.take() {
            state.work.remove(&id);
            return result.map(Some);
        }
        Ok(None)
    }
    /// Bounded operation wait. Test the nonblocking core; no test needs a sleep.
    pub fn await_result(
        &self,
        auth: &dyn EnrollmentSource,
        id: Identifier,
        timeout: Duration,
    ) -> Result<VerifiedResponse> {
        let until = Instant::now() + timeout.min(Duration::from_secs(20));
        let mut state = self.state.lock().map_err(|_| Reason::Unavailable)?;
        loop {
            self.maintain(&mut state, auth);
            let work = state.work.get_mut(&id).ok_or(Reason::LateResponse)?;
            if let Some(result) = work.result.take() {
                state.work.remove(&id);
                return result;
            }
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.work.remove(&id);
                for queue in state.queues.values_mut() {
                    queue.retain(|queued| queued != &id);
                }
                return Err(Reason::Unavailable);
            }
            #[cfg(test)]
            tests::unexpected_wait();
            let (next, _) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| Reason::Unavailable)?;
            state = next;
        }
    }
    pub fn accept_response(
        &self,
        auth: &dyn EnrollmentSource,
        id: Identifier,
        bytes: &[u8],
    ) -> Result<()> {
        let scope = authenticate(auth)?;
        let frame = wire::parse_frame(bytes)?;
        if frame.header.request_id != id {
            return Err(Reason::ForeignRequest);
        }
        let mut state = self.state.lock().map_err(|_| Reason::Unavailable)?;
        let work = state.work.get(&id).ok_or(Reason::LateResponse)?;
        if work.scope != scope || !work.dispatched {
            return Err(Reason::ForeignRequest);
        }
        self.maintain(&mut state, auth);
        let work = state.work.get_mut(&id).ok_or(Reason::LateResponse)?;
        if work.result.is_some() {
            return Err(Reason::LateResponse);
        }
        if work
            .binding
            .as_ref()
            .is_some_and(|b| self.clock.now_millis() >= b.deadline_millis)
        {
            fail(work, Reason::Deadline);
            self.changed.notify_all();
            return Err(Reason::LateResponse);
        }
        let result = self.accept_frame(work, frame, bytes.len());
        if let Err(reason) = result {
            fail(work, reason);
        }
        // Cancel has no body delivery consumer. Retire its completed control
        // exchange immediately so it cannot consume a pending-page slot.
        if matches!(work.request.operation, wire::Operation::Cancel { .. }) && work.result.is_some()
        {
            state.work.remove(&id);
        }
        self.changed.notify_all();
        result
    }
    fn accept_frame(&self, work: &mut Work, frame: wire::Frame, length: usize) -> Result<()> {
        if frame.header.frame != work.next_frame {
            return Err(Reason::FrameOrder);
        }
        work.encoded_bytes = work
            .encoded_bytes
            .checked_add(length)
            .ok_or(Reason::OverBudget)?;
        if work.encoded_bytes > work.request.max_bytes as usize
            || work.encoded_bytes > wire::RESPONSE_CAP
        {
            return Err(Reason::OverBudget);
        }
        work.next_frame = work.next_frame.checked_add(1).ok_or(Reason::FrameOrder)?;
        match frame.header.content {
            FrameContent::Refused { reason, .. } => {
                fail(work, reason);
                return Ok(());
            }
            FrameContent::Header { binding } => {
                verify_binding(&work.request, &work.scope, &binding)?;
                if binding.deadline_millis <= self.clock.now_millis() {
                    return Err(Reason::Deadline);
                }
                work.binding = Some(*binding);
            }
            FrameContent::Data => {
                if work.binding.is_none() {
                    return Err(Reason::FrameOrder);
                }
                work.payload.extend_from_slice(&frame.payload);
            }
        }
        if work
            .binding
            .as_ref()
            .is_some_and(|binding| work.encoded_bytes > binding.max_bytes)
        {
            return Err(Reason::OverBudget);
        }
        if frame.header.final_frame {
            let binding = work.binding.take().ok_or(Reason::FrameOrder)?;
            if work.payload.len() as u64 != binding.permitted_range.len()
                || Sha256::digest(&work.payload).as_slice() != binding.cache_key.payload_digest
            {
                return Err(Reason::BindingMismatch);
            }
            work.result = Some(Ok(VerifiedResponse {
                binding,
                payload: std::mem::take(&mut work.payload),
            }));
        }
        Ok(())
    }
}
impl Poll<'_, '_> {
    pub fn try_take(&self, auth: &dyn EnrollmentSource) -> Result<Option<Request>> {
        let mut state = self.broker.state.lock().map_err(|_| Reason::Unavailable)?;
        self.take_locked(&mut state, auth)
    }
    fn take_locked(
        &self,
        state: &mut State,
        auth: &dyn EnrollmentSource,
    ) -> Result<Option<Request>> {
        self.broker.maintain(state, auth);
        if !auth.is_current(&self.scope)
            || auth
                .revision_handle(&self.scope)
                .is_some_and(|handle| !handle.is_current_at(self.broker.clock.now_millis() / 1000))
        {
            return Err(Reason::Revoked);
        }
        if self.broker.clock.now_millis() >= self.deadline_millis
            || self.consumed.load(Ordering::Relaxed)
        {
            return Ok(None);
        }
        let id = state
            .queues
            .get_mut(self.scope.enrollment_id.as_str())
            .and_then(VecDeque::pop_front);
        let Some(id) = id else {
            return Ok(None);
        };
        let work = state.work.get_mut(&id).ok_or(Reason::Unavailable)?;
        if work.channel != self.channel || work.scope != self.scope {
            fail(work, Reason::Unavailable);
            return Err(Reason::Unavailable);
        }
        work.dispatched = true;
        self.consumed.store(true, Ordering::Relaxed);
        Ok(Some(work.request.clone()))
    }
    pub fn wait(&self, auth: &dyn EnrollmentSource, timeout: Duration) -> Result<Option<Request>> {
        let until = Instant::now() + timeout.min(Duration::from_secs(20));
        let mut state = self.broker.state.lock().map_err(|_| Reason::Unavailable)?;
        loop {
            if let Some(request) = self.take_locked(&mut state, auth)? {
                return Ok(Some(request));
            }
            let remaining =
                until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(
                        self.deadline_millis
                            .saturating_sub(self.broker.clock.now_millis()),
                    ));
            if remaining.is_zero() {
                return Ok(None);
            }
            #[cfg(test)]
            tests::unexpected_wait();
            let (next, _) = self
                .broker
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| Reason::Unavailable)?;
            state = next;
        }
    }
}
fn fail(work: &mut Work, reason: Reason) {
    work.payload.clear();
    work.binding = None;
    work.result = Some(Err(reason));
}
fn authenticate(auth: &dyn EnrollmentSource) -> Result<EnrollmentScope> {
    let scope = auth.authenticated_enrollment(EnrollmentKind::BodyChannel)?;
    if !scope.kinds.contains(&EnrollmentKind::BodyChannel) || !auth.is_current(&scope) {
        return Err(Reason::Forbidden);
    }
    Ok(scope)
}
fn verify_binding(request: &Request, scope: &EnrollmentScope, b: &ResponseBinding) -> Result<()> {
    if b.enrollment != *scope
        || b.request_digest != request.digest()?
        || b.source_handle != request.source_handle
        || b.mapping.anchor != request.anchor
        || b.cache_key.namespace != scope.namespace.as_str()
        || b.cache_key.origin_incarnation != scope.origin_incarnation.as_str()
        || b.cache_key.selector != request.operation.selector()?.cache_selector()
        || b.deadline_millis > request.deadline_millis
        || b.max_bytes > request.max_bytes as usize
        || b.receiver_binding.request_id != request.request_id
        || b.receiver_binding.origin_authorization_id != b.origin_authorization.identifier()
        || b.receiver_binding.origin_policy_version != b.policy.policy_version
        || b.receiver_binding.origin_redactor_version != b.policy.redactor_version
        || b.cache_key.policy_versions
            != [
                String::from(b.policy.vector),
                String::from(b.policy.policy_version),
                String::from(b.policy.redactor_version),
            ]
    {
        return Err(Reason::BindingMismatch);
    }
    let assertion = body_assertion::decode(
        &hex::decode(&request.assertion).map_err(|_| Reason::InvalidRequest)?,
    )
    .map_err(|_| Reason::InvalidRequest)?;
    b.verify_identity(&assertion.claims, request.destination)
        .map_err(|_| Reason::BindingMismatch)?;
    Ok(())
}
/// Mountable result. Listener applies no-store and closes on capacity refusals.
pub struct HandlerResult {
    pub status: u16,
    pub reason: Option<Reason>,
    pub bytes: Vec<u8>,
}
impl HandlerResult {
    fn error(reason: Reason) -> Self {
        let status = match reason {
            Reason::PollCapacity | Reason::PendingQueueFull | Reason::ConnectionCapacity => 503,
            Reason::Forbidden | Reason::Revoked => 403,
            Reason::Unavailable | Reason::Deadline => 503,
            _ => 400,
        };
        Self {
            status,
            reason: Some(reason),
            bytes: Vec::new(),
        }
    }
}
pub fn handle_poll(
    broker: &Broker<'_>,
    auth: &dyn EnrollmentSource,
    body: &[u8],
    timeout: Duration,
) -> HandlerResult {
    let result = (|| {
        let poll = broker.begin_poll(auth, body)?;
        let response = match poll.wait(auth, timeout)? {
            Some(request) => PollResponse::Request {
                version: wire::VERSION,
                request: Box::new(request),
            },
            None => PollResponse::Empty {
                version: wire::VERSION,
            },
        };
        wire::control(&response)
    })();
    match result {
        Ok(bytes) => HandlerResult {
            status: 200,
            reason: None,
            bytes,
        },
        Err(reason) => HandlerResult::error(reason),
    }
}
pub fn handle_response(
    broker: &Broker<'_>,
    auth: &dyn EnrollmentSource,
    request_id: Identifier,
    body: &[u8],
) -> HandlerResult {
    match broker.accept_response(auth, request_id, body) {
        Ok(()) => HandlerResult {
            status: 204,
            reason: None,
            bytes: Vec::new(),
        },
        Err(reason) => HandlerResult::error(reason),
    }
}

/// Caller-authenticated identity; the opaque audit identity is supplied by the
/// owner/login adapter, never inferred from a transport credential.
pub struct Principal {
    pub subject: String,
    pub audit_id: Identifier,
}
pub struct ReadInput {
    pub scope: EnrollmentScope,
    pub request: Request,
    pub generation: Identifier,
    pub destination: Destination,
    pub candidate: Option<wire::Candidate>,
}
pub struct ReadService<'a, 'b, C: Clock> {
    pub broker: &'a Broker<'b>,
    pub enrollment: &'a dyn EnrollmentSource,
    pub acls: &'a Acls,
    pub signing_key: &'a dyn SigningKey,
    pub issuer_key_id: &'a str,
    pub receiver_policy_version: Identifier,
    pub audit: &'a mut AuditStore<C>,
    pub cache: &'a mut BodyCache<C>,
    pub origin_limits: Limits,
}
struct Prepared {
    page: TypedPage,
    attempt: receiver_audit::AttemptId,
    key: crate::body_cache::CacheKey,
    deadline: u64,
}
/// One audited delivery. The page is accessible only through a fresh release
/// check; completion records actual bytes only after the caller reports delivery.
/// Abandon/drop removes this delivery's cache entry even after release(): without
/// delivered(bytes), the journal records failure and no reusable entry remains.
/// Previously released plaintext cannot be recalled from the caller.
pub struct PendingDelivery<'s, 'a, 'b, C: Clock> {
    service: &'s mut ReadService<'a, 'b, C>,
    prepared: Option<Prepared>,
    released: bool,
    principal: String,
    scope: EnrollmentScope,
    grant: String,
}
impl<C: Clock> PendingDelivery<'_, '_, '_, C> {
    pub fn release(&mut self) -> Result<&TypedPage> {
        let prepared = self.prepared.as_ref().ok_or(Reason::LateResponse)?;
        let decision = if !self.service.enrollment.is_current(&self.scope) {
            Err(Reason::Revoked)
        } else if self.service.broker.clock.now_millis() >= prepared.deadline {
            Err(Reason::Deadline)
        } else {
            match self.service.acls.decide(
                &self.principal,
                self.scope.machine.as_str(),
                self.scope.namespace.as_str(),
                Permission::BodyRead,
            ) {
                AclDecision::Granted(grant) if grant.version() == self.grant => Ok(()),
                _ => Err(Reason::Forbidden),
            }
        };
        if let Err(reason) = decision {
            self.abandon_inner()?;
            return Err(reason);
        }
        self.released = true;
        Ok(&self.prepared.as_ref().ok_or(Reason::LateResponse)?.page)
    }
    pub fn delivered(mut self, bytes: u64) -> Result<()> {
        if !self.released {
            return Err(Reason::InvalidRequest);
        }
        let prepared = self.prepared.as_ref().ok_or(Reason::LateResponse)?;
        let expected = prepared
            .page
            .canonical_bytes()
            .map_err(|_| Reason::InvalidRequest)?
            .len() as u64;
        if bytes > expected {
            return Err(Reason::InvalidRequest);
        }
        self.service
            .audit
            .complete(prepared.attempt, Outcome::Completed, bytes)
            .map_err(|_| Reason::AuditFailure)?;
        self.prepared = None;
        Ok(())
    }
    fn abandon_inner(&mut self) -> Result<()> {
        if let Some(prepared) = self.prepared.take() {
            let discard = self
                .service
                .cache
                .discard(&prepared.key)
                .map_err(|_| Reason::Unavailable);
            let audit = self
                .service
                .audit
                .complete(prepared.attempt, Outcome::Failed, 0)
                .map_err(|_| Reason::AuditFailure);
            discard?;
            audit?;
        }
        Ok(())
    }
    pub fn abandon(mut self) -> Result<()> {
        self.abandon_inner()
    }
}
impl<C: Clock> Drop for PendingDelivery<'_, '_, '_, C> {
    fn drop(&mut self) {
        let _ = self.abandon_inner();
    }
}
impl<'a, 'b, C: Clock> ReadService<'a, 'b, C> {
    pub fn read<'s>(
        &'s mut self,
        principal: &Principal,
        input: ReadInput,
    ) -> Result<PendingDelivery<'s, 'a, 'b, C>> {
        let scope = input.scope.clone();
        let grant = match self.acls.decide(
            &principal.subject,
            scope.machine.as_str(),
            scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            AclDecision::Granted(grant) => grant.version().to_owned(),
            _ => return Err(Reason::Forbidden),
        };
        let prepared = self.read_with(principal, input, &mut |_, _| {})?;
        Ok(PendingDelivery {
            service: self,
            prepared: Some(prepared),
            released: false,
            principal: principal.subject.clone(),
            scope,
            grant,
        })
    }
    fn read_with(
        &mut self,
        principal: &Principal,
        mut input: ReadInput,
        exchange: &mut dyn FnMut(&Broker<'_>, &Request),
    ) -> Result<Prepared> {
        let grant = match self.acls.decide(
            &principal.subject,
            input.scope.machine.as_str(),
            input.scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            AclDecision::Granted(grant) => grant,
            AclDecision::Denied => return Err(Reason::Forbidden),
        };
        if !matches!(
            input.request.operation,
            wire::Operation::Resolve { .. }
                | wire::Operation::ReadPage { .. }
                | wire::Operation::ReadField { .. }
        ) {
            return Err(Reason::UnsupportedVerb);
        }
        let original = input.request.operation.clone();
        if let Some(candidate) = input.candidate.take() {
            let candidate_key = candidate.key.clone();
            input.request.operation = wire::Operation::Resolve {
                selector: original.selector()?,
                authorization_only: Some(Box::new(candidate)),
            };
            // The current origin validator refuses this. Never read cache bytes
            // on that path; a fresh request/nonce is required after unavailable.
            match self.round_trip(principal, &input, grant.version(), exchange) {
                Err(Reason::Unavailable) => {
                    // This exact candidate could not receive fresh origin
                    // admission. Remove its temporary ciphertext before fetching;
                    // a different authorization scope must not leave it behind.
                    self.cache
                        .discard(&candidate_key)
                        .map_err(|_| Reason::Unavailable)?;
                    let mut nonce = [0; 32];
                    getrandom::fill(&mut nonce).map_err(|_| Reason::Unavailable)?;
                    input.request.request_id = Identifier::from_bytes(nonce);
                    input.request.operation = original;
                }
                Err(reason) => return Err(reason),
                Ok(_) => return Err(Reason::Unavailable),
            }
        }
        self.round_trip(principal, &input, grant.version(), exchange)
    }
    fn round_trip(
        &mut self,
        principal: &Principal,
        input: &ReadInput,
        grant_version: &str,
        exchange: &mut dyn FnMut(&Broker<'_>, &Request),
    ) -> Result<Prepared> {
        let request = &input.request;
        if serde_json::to_vec(&request.destination).ok()
            != serde_json::to_vec(&input.destination).ok()
        {
            return Err(Reason::InvalidRequest);
        }
        let purpose = match request.operation.selector()? {
            wire::Selector::Page { .. } => Purpose::TranscriptPage,
            wire::Selector::Field { .. } => Purpose::FieldExpansion,
        };
        let range = PermittedRange::new(0, request.max_bytes as u64).ok_or(Reason::OverBudget)?;
        let attempt = self
            .audit
            .begin_attempt(AttemptRecord {
                request_id: request.request_id,
                authenticated_principal: principal.audit_id,
                source_handle: Identifier::from_bytes(
                    Sha256::digest(request.source_handle.as_str().as_bytes()).into(),
                ),
                generation: input.generation,
                permitted_range: range,
                purpose,
                receiver_policy_version: self.receiver_policy_version,
                origin_policy_version: None,
                origin_redactor_version: None,
                decision: receiver_audit::Decision::Granted,
                destination: input.destination,
            })
            .map_err(|_| Reason::AuditFailure)?;
        let result = (|| {
            let now = self.broker.clock.now_millis();
            let channel = self.broker.channel(&input.scope)?;
            let mut nonce = [0; 32];
            hex::decode_to_slice(String::from(request.request_id), &mut nonce)
                .map_err(|_| Reason::InvalidRequest)?;
            let deadline = request.deadline_millis / 1000;
            let claims = Claims {
                issuer_key_id: self.issuer_key_id.into(),
                issued_at: now / 1000,
                expiry: deadline.min(now / 1000 + 60),
                audience: Audience {
                    origin_incarnation: input.scope.origin_incarnation.as_str().into(),
                    namespace: input.scope.namespace.as_str().into(),
                },
                claim_epoch: input
                    .scope
                    .publisher_claim_epoch
                    .ok_or(Reason::Unavailable)?,
                nonce,
                subject_principal: principal.subject.clone(),
                machine: input.scope.machine.as_str().into(),
                namespace: input.scope.namespace.as_str().into(),
                acl_grant_version: grant_version.into(),
                receiver_policy_version: String::from(self.receiver_policy_version),
                request_digest: request.digest()?,
                deadline,
                channel_binding: channel,
            };
            let assertion = body_assertion::sign(claims.clone(), self.signing_key, || now / 1000)
                .map_err(|_| Reason::Unavailable)?;
            let mut outgoing = request.clone();
            outgoing.assertion = hex::encode(body_assertion::encode(&assertion));
            self.broker.enqueue(
                self.enrollment,
                input.scope.clone(),
                channel,
                outgoing.clone(),
            )?;
            let _cleanup = RequestCleanup {
                broker: self.broker,
                id: outgoing.request_id,
            };
            exchange(self.broker, &outgoing);
            let verified = self.broker.await_result(
                self.enrollment,
                request.request_id,
                Duration::from_millis(request.deadline_millis.saturating_sub(now)),
            )?;
            if self.broker.clock.now_millis() >= verified.binding.deadline_millis {
                return Err(Reason::Unavailable);
            }
            if verified.binding.cohort_id != input.generation {
                return Err(Reason::BindingMismatch);
            }
            verified
                .binding
                .verify_identity(&claims, input.destination)
                .map_err(|_| Reason::BindingMismatch)?;
            self.admit(principal, input, grant_version, attempt, verified, now)
        })();
        if result.is_err() {
            self.audit
                .complete(attempt, Outcome::Failed, 0)
                .map_err(|_| Reason::AuditFailure)?;
        }
        result
    }
    fn admit(
        &mut self,
        principal: &Principal,
        input: &ReadInput,
        grant_version: &str,
        attempt: receiver_audit::AttemptId,
        verified: VerifiedResponse,
        fetched: u64,
    ) -> Result<Prepared> {
        let page = decode_page(&verified.payload)?;
        if !self.enrollment.is_current(&input.scope) {
            return Err(Reason::Revoked);
        }
        match self.acls.decide(
            &principal.subject,
            input.scope.machine.as_str(),
            input.scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            AclDecision::Granted(current) if current.version() == grant_version => {}
            _ => return Err(Reason::Forbidden),
        }
        let binding = verified.binding.receiver_binding.clone();
        #[cfg(test)]
        let binding = tests::authorization_binding(binding);
        self.audit
            .authorize(attempt, binding)
            .map_err(|_| Reason::AuditFailure)?;
        #[cfg(test)]
        tests::observe_admission(&verified.binding.receiver_binding);
        let origin_limits = verified.binding.cache_limits.narrowed(self.origin_limits);
        // Admission is constructed only after authorize() has fsynced.
        let admission = ReadAdmission::after_online_authorization_and_audit(
            verified.binding.cache_key.clone(),
            Some(origin_limits),
        );
        // A fresh fetch is already in memory; lookup consumes admission but
        // never substitutes unverified cached bytes for the fetched response.
        let _cached = self
            .cache
            .lookup(admission)
            .map_err(|_| Reason::Unavailable)?;
        if !self.enrollment.is_current(&input.scope) {
            return Err(Reason::Revoked);
        }
        match self.acls.decide(
            &principal.subject,
            input.scope.machine.as_str(),
            input.scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            AclDecision::Granted(current) if current.version() == grant_version => {}
            _ => return Err(Reason::Forbidden),
        }
        if self.broker.clock.now_millis() >= verified.binding.deadline_millis {
            return Err(Reason::Unavailable);
        }
        let key = verified.binding.cache_key;
        self.cache
            .insert(
                key.clone(),
                &verified.payload,
                fetched / 1000,
                Some(origin_limits),
            )
            .map_err(|_| Reason::Unavailable)?;
        // Successful insertion owns the entry under this key, including when it
        // replaced an older entry. Only that successful read's release/abandon
        // paths may discard it; a refused insertion preserves prior custody.
        let release = (|| {
            if !self.enrollment.is_current(&input.scope) {
                return Err(Reason::Revoked);
            }
            if self.broker.clock.now_millis() >= verified.binding.deadline_millis {
                return Err(Reason::Unavailable);
            }
            match self.acls.decide(
                &principal.subject,
                input.scope.machine.as_str(),
                input.scope.namespace.as_str(),
                Permission::BodyRead,
            ) {
                AclDecision::Granted(current) if current.version() == grant_version => {}
                _ => return Err(Reason::Forbidden),
            }
            Ok(())
        })();
        if let Err(reason) = release {
            self.cache.discard(&key).map_err(|_| Reason::Unavailable)?;
            return Err(reason);
        }
        Ok(Prepared {
            page,
            attempt,
            key,
            deadline: verified.binding.deadline_millis,
        })
    }
}
/// Strict inverse of origin_read::TypedPage::canonical_bytes. Text remains typed
/// untrusted data; no HTML/native proof parser is involved.
pub fn decode_page(bytes: &[u8]) -> Result<TypedPage> {
    fn number(input: &mut &[u8]) -> Result<u64> {
        let n = input.get(..8).ok_or(Reason::InvalidRequest)?;
        let value = u64::from_be_bytes(n.try_into().map_err(|_| Reason::InvalidRequest)?);
        *input = &input[8..];
        Ok(value)
    }
    fn field<'a>(input: &mut &'a [u8]) -> Result<&'a [u8]> {
        let length = usize::try_from(number(input)?).map_err(|_| Reason::OverBudget)?;
        let value = input.get(..length).ok_or(Reason::InvalidRequest)?;
        *input = &input[length..];
        Ok(value)
    }
    let prefix = b"semon/typed-body/v1\0";
    if bytes.len() > wire::RESPONSE_CAP || !bytes.starts_with(prefix) {
        return Err(Reason::InvalidRequest);
    }
    let mut input = &bytes[prefix.len()..];
    let count = number(&mut input)?;
    if count > bytes.len() as u64 / 18 {
        return Err(Reason::InvalidRequest);
    }
    let mut fields = Vec::new();
    for _ in 0..count {
        let tag = *input.first().ok_or(Reason::InvalidRequest)?;
        input = &input[1..];
        let kind = match tag {
            0 => Field::User,
            1 => Field::Assistant,
            2 => Field::Command,
            3 => Field::ToolOutput,
            4 => Field::Error,
            _ => return Err(Reason::InvalidRequest),
        };
        if field(&mut input)? != b"untrusted_text" {
            return Err(Reason::InvalidRequest);
        }
        let text = std::str::from_utf8(field(&mut input)?).map_err(|_| Reason::InvalidRequest)?;
        fields.push(TypedField {
            field: kind,
            text: UntrustedText::new(text.into()),
        });
    }
    if !input.is_empty() {
        return Err(Reason::InvalidRequest);
    }
    let page = TypedPage { fields };
    if page
        .canonical_bytes()
        .map_err(|_| Reason::InvalidRequest)?
        .as_slice()
        != bytes
    {
        return Err(Reason::InvalidRequest);
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin_read::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use zeroize::Zeroizing;
    struct At(AtomicU64);
    impl ReadClock for At {
        fn now_millis(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }
    impl Clock for &At {
        fn now(&self) -> u64 {
            self.now_millis() / 1000
        }
    }
    fn opaque(s: &str) -> OpaqueId {
        OpaqueId::new(s).unwrap()
    }
    fn id(n: u8) -> Identifier {
        Identifier::from_bytes([n; 32])
    }
    struct Auth {
        scope: EnrollmentScope,
        revoked: AtomicBool,
    }
    impl EnrollmentSource for Auth {
        fn authenticated_enrollment(&self, _: EnrollmentKind) -> wire::Result<EnrollmentScope> {
            if self.revoked.load(Ordering::Relaxed) {
                Err(Reason::Revoked)
            } else {
                Ok(self.scope.clone())
            }
        }
        fn is_current(&self, scope: &EnrollmentScope) -> bool {
            scope == &self.scope && !self.revoked.load(Ordering::Relaxed)
        }
    }
    impl AuthenticatedEnrollment for Auth {
        fn authenticated_enrollment(&self) -> std::result::Result<EnrollmentScope, Refused> {
            if self.revoked.load(Ordering::Relaxed) {
                Err(Refused::Forbidden)
            } else {
                Ok(self.scope.clone())
            }
        }
    }
    fn auth(name: &str) -> Auth {
        Auth {
            scope: EnrollmentScope {
                enrollment_id: opaque(name),
                machine: opaque("machine"),
                namespace: opaque("namespace"),
                origin_incarnation: opaque("origin"),
                index_incarnation: opaque("index"),
                publisher_claim_epoch: Some(7),
                kinds: vec![EnrollmentKind::BodyChannel],
                verified_client_identity: opaque("client"),
            },
            revoked: AtomicBool::new(false),
        }
    }
    fn signed(n: u8) -> (Request, Claims) {
        let mut request = Request {
            version: wire::VERSION,
            request_id: id(n),
            assertion: String::new(),
            source_handle: opaque("handle"),
            anchor: opaque("anchor"),
            operation: wire::Operation::ReadField {
                field: opaque("assistant"),
            },
            max_bytes: wire::RESPONSE_CAP as u32,
            deadline_millis: 20_000,
            destination: Destination::Viewer,
        };
        let claims = Claims {
            issuer_key_id: "issuer".into(),
            issued_at: 10,
            expiry: 20,
            audience: Audience {
                origin_incarnation: "origin".into(),
                namespace: "namespace".into(),
            },
            claim_epoch: 7,
            nonce: [n; 32],
            subject_principal: "subject".into(),
            machine: "machine".into(),
            namespace: "namespace".into(),
            acl_grant_version: "acl".into(),
            receiver_policy_version: "policy".into(),
            request_digest: request.digest().unwrap(),
            deadline: 20,
            channel_binding: [3; 32],
        };
        let key = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
        request.assertion = hex::encode(body_assertion::encode(
            &body_assertion::sign(claims.clone(), &key, || 10).unwrap(),
        ));
        (request, claims)
    }
    fn poll_bytes() -> Vec<u8> {
        wire::control(&wire::PollRequest {
            version: 1,
            channel_binding: [3; 32],
        })
        .unwrap()
    }
    #[test]
    fn twenty_second_poll_deadline_uses_injected_clock() {
        let at = At(AtomicU64::new(10_000));
        let a = auth("a");
        let broker = Broker::new(Config::default(), &at).unwrap();
        let poll = broker.begin_poll(&a, &poll_bytes()).unwrap();
        at.0.store(30_000, Ordering::Relaxed);
        assert!(poll.wait(&a, Duration::from_secs(20)).unwrap().is_none());
    }
    #[test]
    fn poll_wait_takes_arriving_work_without_a_real_time_wait() {
        let at = At(AtomicU64::new(10_000));
        let a = auth("a");
        let broker = Broker::new(Config::default(), &at).unwrap();
        let poll = broker.begin_poll(&a, &poll_bytes()).unwrap();
        let (request, _) = signed(1);
        broker
            .enqueue(&a, a.scope.clone(), [3; 32], request)
            .unwrap();
        assert_eq!(
            poll.wait(&a, Duration::from_secs(20))
                .unwrap()
                .unwrap()
                .request_id,
            id(1)
        );
        assert!(poll.try_take(&a).unwrap().is_none());
    }
    #[test]
    fn capacity_expiry_revocation_and_unsupported() {
        let at = At(AtomicU64::new(10_000));
        let a = auth("a");
        let b = auth("b");
        let broker = Broker::new(
            Config {
                max_connections: 2,
                poll_share_percent: 50,
            },
            &at,
        )
        .unwrap();
        let poll = broker.begin_poll(&a, &poll_bytes()).unwrap();
        assert!(matches!(
            broker.begin_poll(&a, &poll_bytes()),
            Err(Reason::PollCapacity)
        ));
        assert!(matches!(
            broker.begin_poll(&b, &poll_bytes()),
            Err(Reason::PollCapacity)
        ));
        for n in 1..=4 {
            broker
                .enqueue(&a, a.scope.clone(), [3; 32], signed(n).0)
                .unwrap();
        }
        assert_eq!(
            broker.enqueue(&a, a.scope.clone(), [3; 32], signed(5).0),
            Err(Reason::PendingQueueFull)
        );
        let mut unsupported = signed(6).0;
        unsupported.operation = wire::Operation::Cancel {
            request_id: id(1),
            lease_id: None,
        };
        assert_eq!(
            broker.enqueue(&a, a.scope.clone(), [3; 32], unsupported),
            Err(Reason::InvalidRequest)
        );
        assert!(poll.try_take(&a).unwrap().is_some());
        at.0.store(20_000, Ordering::Relaxed);
        assert!(matches!(
            broker.take_result(&a, id(1)),
            Err(Reason::Deadline)
        ));
        assert!(poll.try_take(&a).unwrap().is_none());
        a.revoked.store(true, Ordering::Relaxed);
        broker.revocation_changed(&a).unwrap();
        assert!(matches!(poll.try_take(&a), Err(Reason::Revoked)));
        assert!(matches!(
            broker.take_result(&a, id(2)),
            Err(Reason::Deadline)
        ));
        drop(poll);
        assert!(broker.begin_poll(&b, &poll_bytes()).is_ok());
    }
    struct Resolver;
    impl HandleResolver for Resolver {
        fn resolve(
            &self,
            _: &EnrollmentScope,
            handle: &OpaqueId,
        ) -> std::result::Result<Option<ResolvedSource>, Refused> {
            if handle.as_str() == "unknown" {
                return Ok(None);
            }
            Ok(Some(ResolvedSource {
                admitted_source: opaque("admitted"),
                cohort_id: id(4),
                cohort: vec![crate::body_cache::SourceWitness {
                    source_id: "source".into(),
                    chain: "chain".into(),
                    generation: "generation".into(),
                    consumed_boundary: 99,
                }],
                activity_generation: opaque("activity"),
                transcript_generation: opaque("transcript"),
            }))
        }
    }
    struct Policies;
    impl PolicyVector for Policies {
        fn evaluate(
            &self,
            _: &EnrollmentScope,
            _: &Claims,
            _: &ResolvedSource,
            _: &Target,
        ) -> std::result::Result<PinnedPolicy, Refused> {
            Ok(PinnedPolicy {
                vector: id(5),
                policy_version: id(6),
                redactor_version: id(7),
            })
        }
    }
    struct Renderer;
    fn page() -> TypedPage {
        TypedPage {
            fields: vec![TypedField {
                field: Field::Assistant,
                text: UntrustedText::new("safe typed body".into()),
            }],
        }
    }
    impl PageRenderer for Renderer {
        fn prepare(
            &self,
            _: &ResolvedSource,
            _: &Target,
            _: PinnedPolicy,
            _: RenderBudget,
            _: &dyn Fn() -> bool,
        ) -> std::result::Result<RenderPlan, Refused> {
            Ok(RenderPlan {
                payload_bytes: page().canonical_bytes()?.len() as u64,
                export_view_generation: Some(opaque("export")),
                exact_mapping: Some(ExactMapping {
                    anchor: opaque("anchor"),
                    assertion: id(8),
                }),
                continuation: None,
            })
        }
        fn render(
            &self,
            _: &ResolvedSource,
            _: &Target,
            _: PinnedPolicy,
            _: RenderBudget,
            _: &dyn Fn() -> bool,
        ) -> std::result::Result<RenderedPage, Refused> {
            Ok(RenderedPage {
                page: page(),
                export_view_generation: Some(opaque("export")),
                exact_mapping: Some(ExactMapping {
                    anchor: opaque("anchor"),
                    assertion: id(8),
                }),
                continuation: None,
            })
        }
    }
    fn frame(
        request: Identifier,
        number: u32,
        final_frame: bool,
        content: FrameContent,
        payload: Vec<u8>,
    ) -> Vec<u8> {
        wire::encode_frame(&wire::Frame {
            header: wire::FrameHeader {
                version: 1,
                request_id: request,
                frame: number,
                final_frame,
                payload_bytes: payload.len() as u32,
                content,
            },
            payload,
        })
        .unwrap()
    }
    #[test]
    fn origin_to_broker_binding_digest_and_frame_isolation() {
        let at = At(AtomicU64::new(10_000));
        let a = auth("a");
        let b = auth("b");
        let broker = Broker::new(Config::default(), &at).unwrap();
        let path = std::env::temp_dir().join(format!("body-broker-origin-{}", std::process::id()));
        let mut audit =
            crate::origin_audit::AuditStore::open(&path, &at, crate::origin_audit::MAX_BYTES)
                .unwrap();
        for n in 1..=3 {
            let poll = broker.begin_poll(&a, &poll_bytes()).unwrap();
            let (request, claims) = signed(n);
            broker
                .enqueue(&a, a.scope.clone(), [3; 32], request)
                .unwrap();
            let request = poll.try_take(&a).unwrap().unwrap();
            let assertion =
                body_assertion::decode(&hex::decode(&request.assertion).unwrap()).unwrap();
            assert_eq!(assertion.claims.request_digest, request.digest().unwrap());
            let origin_request = ReadRequest {
                source_handle: request.source_handle.clone(),
                target: Target::Field(opaque("assistant")),
                anchor: request.anchor.clone(),
                request_digest: request.digest().unwrap(),
                channel_binding: [3; 32],
                max_bytes: request.max_bytes as usize,
                destination: request.destination,
            };
            let authorized = OriginRead {
                enrollment: &a,
                resolver: &Resolver,
                policies: &Policies,
                renderer: &Renderer,
                clock: &at,
                cancelled: &|| false,
                audit: &mut audit,
            }
            .read(&claims, &origin_request)
            .unwrap();
            authorized
                .response_binding
                .verify_identity(&claims, Destination::Viewer)
                .unwrap();
            let payload = authorized
                .page
                .as_ref()
                .unwrap()
                .canonical_bytes()
                .unwrap()
                .to_vec();
            let header = frame(
                id(n),
                0,
                false,
                FrameContent::Header {
                    binding: Box::new(authorized.response_binding.clone()),
                },
                Vec::new(),
            );
            assert_eq!(
                broker.accept_response(&b, id(n), &header),
                Err(Reason::ForeignRequest)
            );
            broker.accept_response(&a, id(n), &header).unwrap();
            if n == 2 {
                assert_eq!(
                    broker.accept_response(&a, id(n), &header),
                    Err(Reason::FrameOrder)
                );
                assert!(matches!(
                    broker.take_result(&a, id(n)),
                    Err(Reason::FrameOrder)
                ));
            } else {
                let mut bytes = payload.clone();
                if n == 3 {
                    bytes[0] ^= 1;
                }
                let data = frame(id(n), 1, true, FrameContent::Data, bytes);
                if n == 3 {
                    assert_eq!(
                        broker.accept_response(&a, id(n), &data),
                        Err(Reason::BindingMismatch)
                    );
                    assert!(matches!(
                        broker.take_result(&a, id(n)),
                        Err(Reason::BindingMismatch)
                    ));
                } else {
                    broker.accept_response(&a, id(n), &data).unwrap();
                    assert_eq!(
                        broker.accept_response(&a, id(n), &data),
                        Err(Reason::LateResponse)
                    );
                    let verified = broker.take_result(&a, id(n)).unwrap().unwrap();
                    verified
                        .binding
                        .verify_identity(&claims, Destination::Viewer)
                        .unwrap();
                    assert_eq!(
                        decode_page(&verified.payload).unwrap().fields[0]
                            .text
                            .as_str(),
                        "safe typed body"
                    );
                    use std::os::unix::fs::PermissionsExt;
                    let custody = path.join("receiver");
                    std::fs::create_dir(&custody).unwrap();
                    std::fs::set_permissions(&custody, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    let acl_dir = custody.join("acl");
                    std::fs::create_dir(&acl_dir).unwrap();
                    std::fs::set_permissions(&acl_dir, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                    let grants = acl_dir.join("grants.json");
                    std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
                    std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                    let key = custody.join("key");
                    std::fs::write(&key, [7; 32]).unwrap();
                    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
                    let mut cache =
                        BodyCache::open(&custody.join("cache"), &key, &at, Limits::default())
                            .unwrap();
                    let audit_dir = custody.join("audit");
                    let mut receiver_audit =
                        AuditStore::open(&audit_dir, &at, receiver_audit::MAX_BYTES).unwrap();
                    let acls = Acls::new(acl_dir);
                    let signing = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
                    let principal = Principal {
                        subject: "subject".into(),
                        audit_id: id(9),
                    };
                    let input = ReadInput {
                        scope: a.scope.clone(),
                        request: request.clone(),
                        generation: id(4),
                        destination: Destination::Viewer,
                        candidate: None,
                    };
                    let grant = match acls.decide(
                        "subject",
                        "machine",
                        "namespace",
                        Permission::BodyRead,
                    ) {
                        AclDecision::Granted(g) => g,
                        _ => panic!("grant"),
                    };
                    let attempt = receiver_audit
                        .begin_attempt(AttemptRecord {
                            request_id: id(n),
                            authenticated_principal: id(9),
                            source_handle: id(10),
                            generation: id(4),
                            permitted_range: PermittedRange::new(0, wire::RESPONSE_CAP as u64)
                                .unwrap(),
                            purpose: Purpose::FieldExpansion,
                            receiver_policy_version: id(11),
                            origin_policy_version: None,
                            origin_redactor_version: None,
                            decision: receiver_audit::Decision::Granted,
                            destination: Destination::Viewer,
                        })
                        .unwrap();
                    let mut service = ReadService {
                        broker: &broker,
                        enrollment: &a,
                        acls: &acls,
                        signing_key: &signing,
                        issuer_key_id: "issuer",
                        receiver_policy_version: id(11),
                        audit: &mut receiver_audit,
                        cache: &mut cache,
                        origin_limits: Limits::default(),
                    };
                    let delivered = service
                        .admit(
                            &principal,
                            &input,
                            grant.version(),
                            attempt,
                            verified,
                            10_000,
                        )
                        .unwrap();
                    assert_eq!(delivered.page.fields[0].text.as_str(), "safe typed body");
                    let mut pending = PendingDelivery {
                        service: &mut service,
                        prepared: Some(delivered),
                        released: false,
                        principal: principal.subject.clone(),
                        scope: a.scope.clone(),
                        grant: grant.version().to_owned(),
                    };
                    assert_eq!(
                        pending.release().unwrap().fields[0].text.as_str(),
                        "safe typed body"
                    );
                    pending.delivered(payload.len() as u64).unwrap();
                    let journal =
                        std::fs::read_to_string(audit_dir.join(receiver_audit::AUDIT_FILE))
                            .unwrap();
                    assert!(
                        journal.find("\"event\":\"authorized\"").unwrap()
                            < journal.find("\"event\":\"completion\"").unwrap()
                    );
                    assert!(!journal.contains("safe typed body"));
                    // A consumed/invalid audit attempt refuses before a new
                    // admission or cache insertion, even with verified bytes.
                    let invalid = VerifiedResponse {
                        binding: authorized.response_binding.clone(),
                        payload: payload.clone(),
                    };
                    assert!(matches!(
                        service.admit(
                            &principal,
                            &input,
                            grant.version(),
                            attempt,
                            invalid,
                            10_000
                        ),
                        Err(Reason::AuditFailure)
                    ));
                    let mut denied = ReadInput {
                        scope: a.scope.clone(),
                        request: request.clone(),
                        generation: id(4),
                        destination: Destination::Viewer,
                        candidate: None,
                    };
                    denied.request.source_handle = opaque("unknown");
                    assert!(matches!(
                        service.read(
                            &Principal {
                                subject: "other".into(),
                                audit_id: id(12)
                            },
                            denied
                        ),
                        Err(Reason::Forbidden)
                    ));
                }
            }
            authorized
                .finish(&mut audit, Outcome::Completed, payload.len() as u64)
                .unwrap();
        }
        drop(audit);
        std::fs::remove_dir_all(path).unwrap();
    }
    pub(super) fn unexpected_wait() {
        panic!(
            "body broker tests must supply a terminal result before waiting; use the injected clock or nonblocking core"
        );
    }
    thread_local! {
        static FAIL_AUTHORIZE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static ADMISSION: std::cell::RefCell<Option<(std::path::PathBuf, usize)>> = const { std::cell::RefCell::new(None) };
    }
    pub(super) fn authorization_binding(
        mut binding: receiver_audit::AuthorizationBinding,
    ) -> receiver_audit::AuthorizationBinding {
        FAIL_AUTHORIZE.with(|fail| {
            if fail.get() {
                binding.request_id = id(99);
            }
        });
        binding
    }
    pub(super) fn observe_admission(binding: &receiver_audit::AuthorizationBinding) {
        ADMISSION.with(|slot| {
            if let Some((path, count)) = slot.borrow_mut().as_mut() {
                let journal = std::fs::read_to_string(path).unwrap();
                let events: Vec<serde_json::Value> = journal.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
                let expected = serde_json::to_value(binding).unwrap();
                assert!(events.iter().any(|event| event["event"] == "authorized" && event["binding"] == expected));
                *count += 1;
            }
        });
    }
    #[derive(Clone, Copy, Debug)]
    enum Failure {
        Mismatch,
        Digest,
        Audit,
        Unavailable,
        Timeout,
        Revoked,
        Oversize,
        Fallback,
        FallbackDeadline,
        FallbackDrop,
    }
    #[test]
    fn service_failures_do_not_cache_and_fallback_requires_admission() {
        use std::os::unix::fs::PermissionsExt;
        for (index, mode) in [
            Failure::Mismatch,
            Failure::Digest,
            Failure::Audit,
            Failure::Unavailable,
            Failure::Timeout,
            Failure::Revoked,
            Failure::Oversize,
            Failure::Fallback,
            Failure::FallbackDeadline,
            Failure::FallbackDrop,
        ]
        .into_iter()
        .enumerate()
        {
            let at = At(AtomicU64::new(10_000));
            let a = auth("a");
            let broker = Broker::new(Config::default(), &at).unwrap();
            let registration = broker.begin_poll(&a, &poll_bytes()).unwrap();
            drop(registration);
            let root =
                std::env::temp_dir().join(format!("body-service-{}-{index}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let acl = root.join("acl");
            std::fs::create_dir(&acl).unwrap();
            std::fs::set_permissions(&acl, std::fs::Permissions::from_mode(0o700)).unwrap();
            let grants = acl.join("grants.json");
            std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
            std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600)).unwrap();
            let key_path = root.join("key");
            std::fs::write(&key_path, [7; 32]).unwrap();
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let cache_dir = root.join("cache");
            let mut cache = BodyCache::open(&cache_dir, &key_path, &at, Limits::default()).unwrap();
            let audit_dir = root.join("audit");
            let mut receiver =
                AuditStore::open(&audit_dir, &at, receiver_audit::MAX_BYTES).unwrap();
            let mut origin = crate::origin_audit::AuditStore::open(
                &root.join("origin"),
                &at,
                crate::origin_audit::MAX_BYTES,
            )
            .unwrap();
            let journal_path = audit_dir.join(receiver_audit::AUDIT_FILE);
            ADMISSION.with(|slot| *slot.borrow_mut() = Some((journal_path.clone(), 0)));
            let acls = Acls::new(acl);
            let signing = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
            let principal = Principal {
                subject: "subject".into(),
                audit_id: id(9),
            };
            let (request, _) = signed(21);
            // Candidate is metadata only; seed a real encrypted candidate for the
            // fallback case, then prove the returned text comes from fresh origin.
            let seed_claims = body_assertion::decode(&hex::decode(&request.assertion).unwrap())
                .unwrap()
                .claims;
            let seed_request = ReadRequest {
                source_handle: request.source_handle.clone(),
                target: Target::Field(opaque("assistant")),
                anchor: request.anchor.clone(),
                request_digest: request.digest().unwrap(),
                channel_binding: [3; 32],
                max_bytes: wire::RESPONSE_CAP,
                destination: Destination::Viewer,
            };
            let seed = OriginRead {
                enrollment: &a,
                resolver: &Resolver,
                policies: &Policies,
                renderer: &Renderer,
                clock: &at,
                cancelled: &|| false,
                audit: &mut origin,
            }
            .read(&seed_claims, &seed_request)
            .unwrap();
            let mut candidate = None;
            if matches!(
                mode,
                Failure::Fallback | Failure::FallbackDeadline | Failure::FallbackDrop
            ) {
                let old = TypedPage {
                    fields: vec![TypedField {
                        field: Field::Assistant,
                        text: UntrustedText::new("cached stale body".into()),
                    }],
                }
                .canonical_bytes()
                .unwrap();
                let mut old_key = seed.response_binding.cache_key.clone();
                old_key.payload_digest = Sha256::digest(&*old).into();
                cache.insert(old_key.clone(), &old, 10, None).unwrap();
                candidate = Some(wire::Candidate {
                    key: old_key,
                    anchor: opaque("anchor"),
                    mapping: id(8),
                    continuation: None,
                    payload_bytes: old.len() as u64,
                });
            }
            seed.finish(&mut origin, Outcome::Cancelled, 0).unwrap();
            let input = ReadInput {
                scope: a.scope.clone(),
                request,
                generation: id(4),
                destination: Destination::Viewer,
                candidate,
            };
            let mut calls = 0;
            let mut service = ReadService {
                broker: &broker,
                enrollment: &a,
                acls: &acls,
                signing_key: &signing,
                issuer_key_id: "issuer",
                receiver_policy_version: id(11),
                audit: &mut receiver,
                cache: &mut cache,
                origin_limits: Limits::default(),
            };
            FAIL_AUTHORIZE.with(|fail| fail.set(matches!(mode, Failure::Audit)));
            let result = service.read_with(&principal, input, &mut |broker, outgoing| {
                calls += 1;
                let poll = broker.begin_poll(&a, &poll_bytes()).unwrap();
                let dispatched = poll.try_take(&a).unwrap().unwrap();
                assert_eq!(dispatched.request_id, outgoing.request_id);
                if matches!(mode, Failure::Timeout) {
                    at.0.store(20_000, Ordering::Relaxed);
                    return;
                }
                if matches!(mode, Failure::Revoked) {
                    a.revoked.store(true, Ordering::Relaxed);
                    broker.revocation_changed(&a).unwrap();
                    return;
                }
                if matches!(mode, Failure::Unavailable)
                    || (matches!(
                        mode,
                        Failure::Fallback | Failure::FallbackDeadline | Failure::FallbackDrop
                    ) && calls == 1)
                {
                    if matches!(
                        mode,
                        Failure::Fallback | Failure::FallbackDeadline | Failure::FallbackDrop
                    ) {
                        assert!(matches!(
                            outgoing.operation,
                            wire::Operation::Resolve {
                                authorization_only: Some(_),
                                ..
                            }
                        ));
                        ADMISSION.with(|slot| assert_eq!(slot.borrow().as_ref().unwrap().1, 0));
                    }
                    broker
                        .accept_response(
                            &a,
                            outgoing.request_id,
                            &frame(
                                outgoing.request_id,
                                0,
                                true,
                                FrameContent::Refused {
                                    reason: Reason::Unavailable,
                                    retryable: true,
                                },
                                Vec::new(),
                            ),
                        )
                        .unwrap();
                    return;
                }
                if matches!(
                    mode,
                    Failure::Fallback | Failure::FallbackDeadline | Failure::FallbackDrop
                ) {
                    assert!(matches!(
                        outgoing.operation,
                        wire::Operation::ReadField { .. }
                    ));
                    assert_ne!(outgoing.request_id, id(21));
                    // The prior candidate had the seed assertion's different
                    // authorization scope. Unavailable must discard that exact
                    // entry before the fresh response has any admission.
                    assert_eq!(
                        std::fs::read_dir(&cache_dir)
                            .unwrap()
                            .map(|entry| entry.unwrap().path())
                            .filter(|path| path.extension().is_some_and(|ext| ext == "entry"))
                            .count(),
                        0,
                        "{mode:?}: candidate survived unavailable"
                    );
                }
                let claims = body_assertion::decode(&hex::decode(&outgoing.assertion).unwrap())
                    .unwrap()
                    .claims;
                let request = ReadRequest {
                    source_handle: outgoing.source_handle.clone(),
                    target: Target::Field(opaque("assistant")),
                    anchor: outgoing.anchor.clone(),
                    request_digest: outgoing.digest().unwrap(),
                    channel_binding: [3; 32],
                    max_bytes: if matches!(mode, Failure::Oversize) {
                        16 * 1024
                    } else {
                        outgoing.max_bytes as usize
                    },
                    destination: outgoing.destination,
                };
                let authorized = OriginRead {
                    enrollment: &a,
                    resolver: &Resolver,
                    policies: &Policies,
                    renderer: &Renderer,
                    clock: &at,
                    cancelled: &|| false,
                    audit: &mut origin,
                }
                .read(&claims, &request)
                .unwrap();
                let mut binding = authorized.response_binding.clone();
                if matches!(mode, Failure::Mismatch) {
                    binding.mapping.anchor = opaque("wrong");
                }
                let mut payload = authorized
                    .page
                    .as_ref()
                    .unwrap()
                    .canonical_bytes()
                    .unwrap()
                    .to_vec();
                if matches!(mode, Failure::Digest) {
                    payload[0] ^= 1;
                }
                let header = frame(
                    outgoing.request_id,
                    0,
                    false,
                    FrameContent::Header {
                        binding: Box::new(binding),
                    },
                    Vec::new(),
                );
                let accepted = broker.accept_response(&a, outgoing.request_id, &header);
                if matches!(mode, Failure::Mismatch) {
                    assert_eq!(accepted, Err(Reason::BindingMismatch));
                } else {
                    accepted.unwrap();
                    if matches!(mode, Failure::Oversize) {
                        let work = broker
                            .state
                            .lock()
                            .unwrap()
                            .work
                            .get(&outgoing.request_id)
                            .unwrap()
                            .encoded_bytes;
                        let oversized = frame(
                            outgoing.request_id,
                            1,
                            true,
                            FrameContent::Data,
                            vec![0; 16 * 1024 - work + 1],
                        );
                        assert_eq!(
                            broker.accept_response(&a, outgoing.request_id, &oversized),
                            Err(Reason::OverBudget)
                        );
                    } else {
                        let accepted = broker.accept_response(
                            &a,
                            outgoing.request_id,
                            &frame(
                                outgoing.request_id,
                                1,
                                true,
                                FrameContent::Data,
                                payload.clone(),
                            ),
                        );
                        if matches!(mode, Failure::Digest) {
                            assert_eq!(accepted, Err(Reason::BindingMismatch));
                        } else {
                            accepted.unwrap();
                        }
                    }
                }
                authorized
                    .finish(&mut origin, Outcome::Cancelled, 0)
                    .unwrap();
            });
            if matches!(
                mode,
                Failure::Fallback | Failure::FallbackDeadline | Failure::FallbackDrop
            ) {
                let prepared = result.unwrap();
                assert_eq!(prepared.page.fields[0].text.as_str(), "safe typed body");
                let before = std::fs::read_to_string(&journal_path).unwrap();
                assert!(!before.contains("\"outcome\":\"completed\""));
                let grant =
                    match acls.decide("subject", "machine", "namespace", Permission::BodyRead) {
                        AclDecision::Granted(g) => g.version().to_owned(),
                        _ => panic!("grant"),
                    };
                let mut pending = PendingDelivery {
                    service: &mut service,
                    prepared: Some(prepared),
                    released: false,
                    principal: "subject".into(),
                    scope: a.scope.clone(),
                    grant,
                };
                assert_eq!(
                    pending.release().unwrap().fields[0].text.as_str(),
                    "safe typed body"
                );
                if matches!(mode, Failure::FallbackDeadline) {
                    at.0.store(20_000, Ordering::Relaxed);
                    assert!(matches!(pending.release(), Err(Reason::Deadline)));
                } else if matches!(mode, Failure::Fallback) {
                    a.revoked.store(true, Ordering::Relaxed);
                    assert!(matches!(pending.release(), Err(Reason::Revoked)));
                }
                drop(pending);
                assert_eq!(
                    service.cache.entry_count(),
                    0,
                    "{mode:?}: abandon retained a cache entry"
                );
                assert_eq!(
                    std::fs::read_dir(&cache_dir)
                        .unwrap()
                        .map(|entry| entry.unwrap().path())
                        .filter(|path| path.extension().is_some_and(|ext| ext == "entry"))
                        .count(),
                    0,
                    "{mode:?}: abandon retained ciphertext"
                );
                let after = std::fs::read_to_string(&journal_path).unwrap();
                assert!(!after.contains("\"outcome\":\"completed\""));
                let completions: Vec<serde_json::Value> = after
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .filter(|event| event["event"] == "completion")
                    .collect();
                assert_eq!(completions.len(), 2, "{mode:?}");
                assert!(
                    completions
                        .iter()
                        .all(|event| event["outcome"] == "failed" && event["bytes"] == 0),
                    "{mode:?}"
                );
                assert_eq!(calls, 2);
                ADMISSION.with(|slot| assert_eq!(slot.borrow().as_ref().unwrap().1, 1));
            } else {
                let expected = match mode {
                    Failure::Mismatch | Failure::Digest => Reason::BindingMismatch,
                    Failure::Audit => Reason::AuditFailure,
                    Failure::Unavailable => Reason::Unavailable,
                    Failure::Timeout => Reason::Deadline,
                    Failure::Revoked => Reason::Revoked,
                    Failure::Oversize => Reason::OverBudget,
                    Failure::Fallback | Failure::FallbackDeadline | Failure::FallbackDrop => {
                        unreachable!()
                    }
                };
                assert!(
                    matches!(result, Err(reason) if reason == expected),
                    "{mode:?}"
                );
                assert_eq!(service.cache.entry_count(), 0, "{mode:?}");
                assert_eq!(
                    std::fs::read_dir(&cache_dir)
                        .unwrap()
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().is_some_and(|ext| ext == "entry"))
                        .count(),
                    0,
                    "{mode:?}"
                );
                let events: Vec<serde_json::Value> = std::fs::read_to_string(&journal_path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert!(
                    events.iter().any(|e| e["event"] == "completion"
                        && e["outcome"] == "failed"
                        && e["bytes"] == 0),
                    "{mode:?}"
                );
                assert!(
                    !events
                        .iter()
                        .any(|e| e["event"] == "completion" && e["outcome"] == "completed")
                );
                ADMISSION.with(|slot| assert_eq!(slot.borrow().as_ref().unwrap().1, 0));
            }
            FAIL_AUTHORIZE.with(|fail| fail.set(false));
            ADMISSION.with(|slot| *slot.borrow_mut() = None);
            drop(cache);
            drop(receiver);
            drop(origin);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    struct Loopback<'a, 'b> {
        broker: &'a Broker<'b>,
        auth: &'a Auth,
        fail_after_header: bool,
        posts: usize,
    }
    impl crate::origin_client::PollTransport for Loopback<'_, '_> {
        fn poll(
            &mut self,
            channel: [u8; 32],
            _: u64,
        ) -> std::result::Result<
            crate::origin_client::PollOutcome,
            crate::origin_client::TransportError,
        > {
            let bytes = wire::control(&wire::PollRequest {
                version: 1,
                channel_binding: channel,
            })
            .unwrap();
            // Zero timeout takes the handler's nonblocking path with the fake clock.
            let result = handle_poll(self.broker, self.auth, &bytes, Duration::ZERO);
            assert_eq!(result.status, 200);
            Ok(match wire::parse_poll_response(&result.bytes).unwrap() {
                PollResponse::Request { request, .. } => {
                    crate::origin_client::PollOutcome::Request(wire::control(&request).unwrap())
                }
                PollResponse::Empty { .. } => crate::origin_client::PollOutcome::Empty,
            })
        }
        fn post_response(
            &mut self,
            id: Identifier,
            bytes: &[u8],
        ) -> std::result::Result<(), crate::origin_client::TransportError> {
            if self.fail_after_header && self.posts == 1 {
                return Err(crate::origin_client::TransportError::Unavailable);
            }
            self.posts += 1;
            let response = handle_response(self.broker, self.auth, id, bytes);
            if response.status == 204 {
                Ok(())
            } else {
                Err(crate::origin_client::TransportError::Unavailable)
            }
        }
    }
    struct FixedEntropy;
    impl crate::origin_client::Entropy for FixedEntropy {
        fn channel_binding(
            &mut self,
        ) -> std::result::Result<[u8; 32], crate::origin_client::TransportError> {
            Ok([3; 32])
        }
    }
    #[test]
    fn client_refusals_and_interrupted_response() {
        use crate::origin_client::{
            OriginClient, PollOutcome, PollTransport, Step, TransportError,
        };
        struct Raw {
            input: Option<Vec<u8>>,
            output: Vec<Vec<u8>>,
            fail: bool,
        }
        impl PollTransport for Raw {
            fn poll(
                &mut self,
                _: [u8; 32],
                _: u64,
            ) -> std::result::Result<PollOutcome, TransportError> {
                Ok(self
                    .input
                    .take()
                    .map_or(PollOutcome::Empty, PollOutcome::Request))
            }
            fn post_response(
                &mut self,
                _: Identifier,
                bytes: &[u8],
            ) -> std::result::Result<(), TransportError> {
                if self.fail && !self.output.is_empty() {
                    return Err(TransportError::Unavailable);
                }
                self.output.push(bytes.to_vec());
                Ok(())
            }
        }
        let at = At(AtomicU64::new(10_000));
        let a = auth("raw-client");
        let key = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
        let public = key.public_key().unwrap();
        let lookup = |_: &str| Some(public);
        for case in 0..12 {
            let path = std::env::temp_dir()
                .join(format!("body-client-refuse-{}-{case}", std::process::id()));
            let mut audit =
                crate::origin_audit::AuditStore::open(&path, &at, crate::origin_audit::MAX_BYTES)
                    .unwrap();
            let mut replay = body_assertion::ReplayCache::new(32);
            let (mut request, mut claims) = signed(61);
            match case {
                0 => {
                    request.operation = wire::Operation::Cancel {
                        request_id: id(1),
                        lease_id: None,
                    }
                }
                2 => claims.audience.namespace = "foreign".into(),
                3 => claims.channel_binding = [9; 32],
                4 => {
                    claims.issued_at = 1;
                    claims.expiry = 9;
                    claims.deadline = 9;
                }
                5 => claims.request_digest = [0; 32],
                _ => {}
            }
            if (2..=5).contains(&case) {
                request.assertion = hex::encode(body_assertion::encode(
                    &body_assertion::sign(claims, &key, || if case == 4 { 1 } else { 10 }).unwrap(),
                ));
            }
            if case == 10 {
                request.source_handle = opaque("unknown");
                let mut claims = signed(61).1;
                claims.request_digest = request.digest().unwrap();
                request.assertion = hex::encode(body_assertion::encode(
                    &body_assertion::sign(claims, &key, || 10).unwrap(),
                ));
            }
            if case == 11 {
                let mut assertion =
                    body_assertion::decode(&hex::decode(&request.assertion).unwrap()).unwrap();
                assertion.signature[0] ^= 1;
                request.assertion = hex::encode(body_assertion::encode(&assertion));
            }
            let mut input = wire::control(&request).unwrap();
            if case == 1 {
                let mut value: serde_json::Value = serde_json::from_slice(&input).unwrap();
                value["extra"] = true.into();
                input = serde_json::to_vec(&value).unwrap();
            }
            if case == 6 {
                input = vec![b' '; wire::CONTROL_CAP + 1];
            }
            if case == 7 {
                input = b"{malformed".to_vec();
            }
            let cancelled = std::cell::Cell::new(false);
            let is_cancelled = || cancelled.get();
            let mut client = OriginClient::new(
                OriginRead {
                    enrollment: &a,
                    resolver: &Resolver,
                    policies: &Policies,
                    renderer: &Renderer,
                    clock: &at,
                    cancelled: &is_cancelled,
                    audit: &mut audit,
                },
                Raw {
                    input: Some(input),
                    output: Vec::new(),
                    fail: case == 8,
                },
                FixedEntropy,
                &mut replay,
                &lookup,
                &UnavailableExportView,
            );
            let result = client.step();
            match case {
                0 => assert_eq!(result, Step::Refused(Reason::InvalidRequest)),
                1 => assert_eq!(result, Step::Refused(Reason::InvalidRequest)),
                2..=5 | 10 | 11 => assert_eq!(result, Step::Refused(Reason::Forbidden)),
                6 | 7 => assert_eq!(result, Step::Dropped),
                8 => {
                    assert_eq!(result, Step::Disconnected);
                    assert_eq!(client.transport.output.len(), 1);
                }
                9 => {
                    assert_eq!(result, Step::Delivered);
                    client.transport.input = Some(wire::control(&request).unwrap());
                    assert_eq!(client.step(), Step::Refused(Reason::Forbidden));
                    cancelled.set(true);
                    assert_eq!(
                        client.step(),
                        Step::Stopped(crate::origin_client::StopReason::Cancelled)
                    );
                }
                _ => unreachable!(),
            }
            drop(audit);
            if case == 8 {
                let journal =
                    std::fs::read_to_string(path.join(crate::origin_audit::AUDIT_FILE)).unwrap();
                assert!(journal.contains("\"outcome\":\"failed\""));
                assert!(!journal.contains("safe typed body"));
            }
            std::fs::remove_dir_all(path).unwrap();
        }
    }
    #[test]
    fn client_loopback_service_delivery_and_dual_audit() {
        use crate::origin_client::{OriginClient, Step};
        use std::os::unix::fs::PermissionsExt;
        let at = At(AtomicU64::new(10_000));
        let a = auth("client-loopback");
        let broker = Broker::new(Config::default(), &at).unwrap();
        let root =
            std::env::temp_dir().join(format!("body-client-loopback-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let acl = root.join("acl");
        std::fs::create_dir(&acl).unwrap();
        std::fs::set_permissions(&acl, std::fs::Permissions::from_mode(0o700)).unwrap();
        let grants = acl.join("grants.json");
        std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
        std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600)).unwrap();
        let key_path = root.join("key");
        std::fs::write(&key_path, [7; 32]).unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut cache =
            BodyCache::open(&root.join("cache"), &key_path, &at, Limits::default()).unwrap();
        let mut receiver =
            AuditStore::open(&root.join("receiver"), &at, receiver_audit::MAX_BYTES).unwrap();
        let mut origin_audit = crate::origin_audit::AuditStore::open(
            &root.join("origin"),
            &at,
            crate::origin_audit::MAX_BYTES,
        )
        .unwrap();
        let signing = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
        let public = signing.public_key().unwrap();
        let lookup = |_: &str| Some(public);
        let mut replay = body_assertion::ReplayCache::new(32);
        let cancelled = || false;
        let mut client = OriginClient::new(
            OriginRead {
                enrollment: &a,
                resolver: &Resolver,
                policies: &Policies,
                renderer: &Renderer,
                clock: &at,
                cancelled: &cancelled,
                audit: &mut origin_audit,
            },
            Loopback {
                broker: &broker,
                auth: &a,
                fail_after_header: false,
                posts: 0,
            },
            FixedEntropy,
            &mut replay,
            &lookup,
            &UnavailableExportView,
        );
        let limits = CacheLimits {
            cache_ttl_seconds: std::num::NonZeroU64::new(1),
            ..CacheLimits::default()
        };
        client.limits = &limits;
        assert_eq!(client.step(), Step::Empty);
        let acls = Acls::new(acl);
        let principal = Principal {
            subject: "subject".into(),
            audit_id: id(9),
        };
        let mut service = ReadService {
            broker: &broker,
            enrollment: &a,
            acls: &acls,
            signing_key: &signing,
            issuer_key_id: "issuer",
            receiver_policy_version: id(11),
            audit: &mut receiver,
            cache: &mut cache,
            origin_limits: Limits::default(),
        };
        let journal = root.join("receiver").join(receiver_audit::AUDIT_FILE);
        ADMISSION.with(|slot| *slot.borrow_mut() = Some((journal.clone(), 0)));
        let prepared = service
            .read_with(
                &principal,
                ReadInput {
                    scope: a.scope.clone(),
                    request: signed(77).0,
                    generation: id(4),
                    destination: Destination::Viewer,
                    candidate: None,
                },
                &mut |_, _| {
                    assert_eq!(client.step(), Step::Delivered);
                },
            )
            .unwrap();
        let cache_key = prepared.key.clone();
        let bytes = prepared.page.canonical_bytes().unwrap().len() as u64;
        let grant = match acls.decide("subject", "machine", "namespace", Permission::BodyRead) {
            AclDecision::Granted(g) => g.version().to_owned(),
            _ => panic!("grant"),
        };
        let mut pending = PendingDelivery {
            service: &mut service,
            prepared: Some(prepared),
            released: false,
            principal: "subject".into(),
            scope: a.scope.clone(),
            grant,
        };
        assert_eq!(
            pending.release().unwrap().fields[0].text.as_str(),
            "safe typed body"
        );
        pending.delivered(bytes).unwrap();
        ADMISSION.with(|slot| assert_eq!(slot.borrow().as_ref().unwrap().1, 1));
        ADMISSION.with(|slot| *slot.borrow_mut() = None);
        // A same-key fetch with an origin total of S permits lookup of the
        // existing S-byte entry but refuses insertion's additional 128-byte
        // state reserve. The broker must preserve that existing ciphertext.
        let entry_path = std::fs::read_dir(root.join("cache"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "entry"))
            .unwrap();
        let record = std::fs::read(&entry_path).unwrap();
        let same_key_limits = CacheLimits {
            cache_total_bytes: std::num::NonZeroU64::new(record.len() as u64),
            cache_ttl_seconds: std::num::NonZeroU64::new(1),
            ..CacheLimits::default()
        };
        client.limits = &same_key_limits;
        let refused = service.read_with(
            &principal,
            ReadInput {
                scope: a.scope.clone(),
                request: signed(78).0,
                generation: id(4),
                destination: Destination::Viewer,
                candidate: None,
            },
            &mut |_, _| assert_eq!(client.step(), Step::Delivered),
        );
        assert!(matches!(refused, Err(Reason::Unavailable)));
        assert_eq!(service.cache.entry_count(), 1);
        assert_eq!(std::fs::read(&entry_path).unwrap(), record);
        assert!(
            service
                .cache
                .lookup(ReadAdmission::after_online_authorization_and_audit(
                    cache_key.clone(),
                    None
                ))
                .unwrap()
                .is_some()
        );
        // The header's one-second limit reached insertion, not just parsing.
        at.0.store(11_000, Ordering::Relaxed);
        assert!(
            service
                .cache
                .lookup(ReadAdmission::after_online_authorization_and_audit(
                    cache_key, None
                ))
                .unwrap()
                .is_none()
        );
        for (dir, file) in [
            ("origin", crate::origin_audit::AUDIT_FILE),
            ("receiver", receiver_audit::AUDIT_FILE),
        ] {
            let journal = std::fs::read_to_string(root.join(dir).join(file)).unwrap();
            assert!(journal.contains("\"outcome\":\"completed\""));
            assert!(!journal.contains("safe typed body"));
        }
        a.revoked.store(true, Ordering::Relaxed);
        assert_eq!(
            client.step(),
            Step::Stopped(crate::origin_client::StopReason::Revoked)
        );
        drop(cache);
        drop(receiver);
        drop(origin_audit);
        std::fs::remove_dir_all(root).unwrap();
    }
    struct Appends {
        rewrite: AtomicBool,
        renders: AtomicU64,
    }
    impl crate::append_source::AppendPageSource for Appends {
        fn prepare(
            &self,
            source: &ResolvedSource,
            cursor: &crate::append_source::AppendCursor<'_>,
            policy: PinnedPolicy,
            budget: RenderBudget,
            cancelled: &dyn Fn() -> bool,
        ) -> std::result::Result<RenderPlan, Refused> {
            let mut plan = Renderer.prepare(
                source,
                &Target::Page(cursor.page.clone()),
                policy,
                budget,
                cancelled,
            )?;
            if self.rewrite.load(Ordering::Relaxed) {
                plan.export_view_generation = Some(opaque("rewritten"));
            }
            plan.continuation = Some(opaque(if cursor.after.is_none() {
                "next-1"
            } else {
                "next-2"
            }));
            Ok(plan)
        }
        fn render(
            &self,
            source: &ResolvedSource,
            cursor: &crate::append_source::AppendCursor<'_>,
            policy: PinnedPolicy,
            budget: RenderBudget,
            cancelled: &dyn Fn() -> bool,
        ) -> std::result::Result<RenderedPage, Refused> {
            self.renders.fetch_add(1, Ordering::Relaxed);
            let mut rendered = Renderer.render(
                source,
                &Target::Page(cursor.page.clone()),
                policy,
                budget,
                cancelled,
            )?;
            if self.rewrite.load(Ordering::Relaxed) {
                rendered.export_view_generation = Some(opaque("rewritten"));
            }
            rendered.continuation = Some(opaque(if cursor.after.is_none() {
                "next-1"
            } else {
                "next-2"
            }));
            Ok(rendered)
        }
    }
    #[test]
    fn append_loopback_ack_gate_receipts_and_watcher_cancel() {
        use crate::{
            append_receiver::AppendService,
            origin_client::{OriginClient, Step},
        };
        use std::os::unix::fs::PermissionsExt;
        // Trace: fixed 10_000 ms, request deadline 12_000 ms, lease 70_000 ms.
        // No clock advance: both pages have independent assertions and attempts.
        // Each page has two frames; each receiver attempt has one receipt.
        let at = At(AtomicU64::new(10_000));
        let a = auth("append-loopback");
        let broker = Broker::new(Config::default(), &at).unwrap();
        let root =
            std::env::temp_dir().join(format!("body-append-loopback-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let acl = root.join("acl");
        std::fs::create_dir(&acl).unwrap();
        std::fs::set_permissions(&acl, std::fs::Permissions::from_mode(0o700)).unwrap();
        let grants = acl.join("grants.json");
        std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
        std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600)).unwrap();
        let acls = Acls::new(acl);
        let mut receiver =
            AuditStore::open(&root.join("receiver"), &at, receiver_audit::MAX_BYTES).unwrap();
        let mut origin_audit = crate::origin_audit::AuditStore::open(
            &root.join("origin"),
            &at,
            crate::origin_audit::MAX_BYTES,
        )
        .unwrap();
        let signing = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
        let public = signing.public_key().unwrap();
        let lookup = |_: &str| Some(public);
        let mut replay = body_assertion::ReplayCache::new(32);
        let cancelled = || false;
        let mut client = OriginClient::new(
            OriginRead {
                enrollment: &a,
                resolver: &Resolver,
                policies: &Policies,
                renderer: &Renderer,
                clock: &at,
                cancelled: &cancelled,
                audit: &mut origin_audit,
            },
            Loopback {
                broker: &broker,
                auth: &a,
                fail_after_header: false,
                posts: 0,
            },
            FixedEntropy,
            &mut replay,
            &lookup,
            &UnavailableExportView,
        );
        let appends = Appends {
            rewrite: AtomicBool::new(false),
            renders: AtomicU64::new(0),
        };
        client.append_source = Some(&appends);
        assert_eq!(client.step(), Step::Empty);
        let mut service = AppendService::new(
            &broker,
            &a,
            &acls,
            &signing,
            "issuer",
            id(11),
            &mut receiver,
        );
        let mut template = signed(77).0;
        template.operation = wire::Operation::ReadPage {
            page: opaque("append"),
        };
        let (lease, watcher) = service
            .subscribe(
                Principal {
                    subject: "subject".into(),
                    audit_id: id(9),
                },
                a.scope.clone(),
                template,
                id(4),
            )
            .unwrap();
        assert_eq!(client.step(), Step::Delivered);
        assert!(service.collect(lease).unwrap());
        assert_eq!(
            service.acknowledge(lease, &opaque("next-1")),
            Err(Reason::InvalidRequest)
        );
        assert_eq!(client.step(), Step::Empty);
        let first = service.release(lease).unwrap();
        let bytes = first.canonical_bytes().unwrap().len() as u64;
        assert_eq!(first.fields[0].text.as_str(), "safe typed body");
        service.delivered(lease, bytes).unwrap();
        assert_eq!(client.step(), Step::Empty);
        assert_eq!(
            service.acknowledge(lease, &opaque("wrong")),
            Err(Reason::InvalidRequest)
        );
        assert_eq!(client.step(), Step::Empty);
        service.acknowledge(lease, &opaque("next-1")).unwrap();
        // Hold the valid request while three independently signed malformed
        // acknowledgements exercise the actual origin handler. None may render
        // or advance its state; the original request must still succeed.
        let valid_ack = broker
            .state
            .lock()
            .unwrap()
            .work
            .values()
            .find(|work| matches!(work.request.operation, wire::Operation::Acknowledge { .. }))
            .unwrap()
            .request
            .clone();
        broker.cancel(valid_ack.request_id).unwrap();
        for variant in 0..3u8 {
            let mut invalid = valid_ack.clone();
            invalid.request_id = id(100 + variant);
            if let wire::Operation::Acknowledge {
                page,
                continuation,
                request_id,
                ..
            } = &mut invalid.operation
            {
                match variant {
                    0 => *page = opaque("wrong-page"),
                    1 => *continuation = opaque("wrong-continuation"),
                    _ => *request_id = id(99),
                }
            }
            let mut claims = body_assertion::decode(&hex::decode(&valid_ack.assertion).unwrap())
                .unwrap()
                .claims;
            claims.nonce = [100 + variant; 32];
            claims.request_digest = invalid.digest().unwrap();
            invalid.assertion = hex::encode(body_assertion::encode(
                &body_assertion::sign(claims, &signing, || 10).unwrap(),
            ));
            broker
                .enqueue(&a, a.scope.clone(), [3; 32], invalid.clone())
                .unwrap();
            let before = appends.renders.load(Ordering::Relaxed);
            assert_eq!(client.step(), Step::Refused(Reason::ContinuationInvalid));
            assert_eq!(appends.renders.load(Ordering::Relaxed), before);
            assert!(matches!(
                broker.take_result(&a, invalid.request_id),
                Err(Reason::ContinuationInvalid)
            ));
        }
        broker
            .enqueue(&a, a.scope.clone(), [3; 32], valid_ack)
            .unwrap();
        assert_eq!(
            service.acknowledge(lease, &opaque("next-1")),
            Err(Reason::InvalidRequest)
        );
        assert_eq!(client.step(), Step::Delivered);
        assert!(service.collect(lease).unwrap());
        let second = service.release(lease).unwrap();
        service
            .delivered(lease, second.canonical_bytes().unwrap().len() as u64)
            .unwrap();
        let mut replay_ack = service.active_request_for_test(lease);
        replay_ack.request_id = id(104);
        let mut replay_claims =
            body_assertion::decode(&hex::decode(&replay_ack.assertion).unwrap())
                .unwrap()
                .claims;
        replay_claims.nonce = [104; 32];
        replay_claims.request_digest = replay_ack.digest().unwrap();
        replay_ack.assertion = hex::encode(body_assertion::encode(
            &body_assertion::sign(replay_claims, &signing, || 10).unwrap(),
        ));
        broker
            .enqueue(&a, a.scope.clone(), [3; 32], replay_ack)
            .unwrap();
        let before = appends.renders.load(Ordering::Relaxed);
        assert_eq!(client.step(), Step::Refused(Reason::ContinuationInvalid));
        assert_eq!(appends.renders.load(Ordering::Relaxed), before);
        broker.take_result(&a, id(104)).err().unwrap();
        drop(watcher);
        service.maintain().unwrap();
        assert_eq!(service.release(lease).err(), Some(Reason::LeaseClosed));
        assert_eq!(client.step(), Step::Refused(Reason::Cancelled));
        // Fresh lease gets a fresh attempt. A changed export generation on its
        // second request produces no body and closes with continuation_invalid.
        let mut template = signed(78).0;
        template.operation = wire::Operation::ReadPage {
            page: opaque("append"),
        };
        let (rewritten, rewrite_watcher) = service
            .subscribe(
                Principal {
                    subject: "subject".into(),
                    audit_id: id(9),
                },
                a.scope.clone(),
                template,
                id(4),
            )
            .unwrap();
        assert_eq!(client.step(), Step::Delivered);
        assert!(service.collect(rewritten).unwrap());
        let third = service.release(rewritten).unwrap();
        service
            .delivered(rewritten, third.canonical_bytes().unwrap().len() as u64)
            .unwrap();
        appends.rewrite.store(true, Ordering::Relaxed);
        service.acknowledge(rewritten, &opaque("next-1")).unwrap();
        assert_eq!(client.step(), Step::Refused(Reason::ContinuationInvalid));
        assert_eq!(service.collect(rewritten), Err(Reason::ContinuationInvalid));
        assert_eq!(service.release(rewritten).err(), Some(Reason::LeaseClosed));
        drop(rewrite_watcher);
        drop(service);
        drop(client);
        let receiver_text =
            std::fs::read_to_string(root.join("receiver").join(receiver_audit::AUDIT_FILE))
                .unwrap();
        assert_eq!(receiver_text.matches("\"event\":\"attempted\"").count(), 4);
        assert_eq!(receiver_text.matches("\"event\":\"authorized\"").count(), 3);
        assert_eq!(receiver_text.matches("\"event\":\"receipt\"").count(), 3);
        assert_eq!(receiver_text.matches("\"event\":\"completion\"").count(), 4);
        assert!(!receiver_text.contains("safe typed body"));
        let origin_text =
            std::fs::read_to_string(root.join("origin").join(crate::origin_audit::AUDIT_FILE))
                .unwrap();
        assert_eq!(origin_text.matches("\"outcome\":\"completed\"").count(), 3);
        assert_eq!(origin_text.matches("\"outcome\":\"failed\"").count(), 1);
        assert!(!origin_text.contains("safe typed body"));
        drop(receiver);
        drop(origin_audit);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn append_service_failure_and_invalidation_loopback() {
        use crate::{
            append_receiver::AppendService,
            origin_client::{OriginClient, Step},
        };
        use std::os::unix::fs::PermissionsExt;
        // Every independent case starts at 10_000 ms: page expires at 12_000,
        // lease at 70_000. Partial delivery reports exactly 1 byte. Injected
        // receipt/complete failures occur once, permitting failed completion.
        for case in 0..11 {
            let at = At(AtomicU64::new(10_000));
            let a = auth("append-failures");
            let foreign = auth("foreign");
            let broker = Broker::new(Config::default(), &at).unwrap();
            let root =
                std::env::temp_dir().join(format!("append-failure-{}-{case}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let acl = root.join("acl");
            std::fs::create_dir(&acl).unwrap();
            std::fs::set_permissions(&acl, std::fs::Permissions::from_mode(0o700)).unwrap();
            let grants = acl.join("grants.json");
            std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
            std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600)).unwrap();
            let acls = Acls::new(acl);
            let mut receiver =
                AuditStore::open(&root.join("receiver"), &at, receiver_audit::MAX_BYTES).unwrap();
            let mut origin_audit = crate::origin_audit::AuditStore::open(
                &root.join("origin"),
                &at,
                if case == 8 {
                    1
                } else {
                    crate::origin_audit::MAX_BYTES
                },
            )
            .unwrap();
            let signing = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
            let public = signing.public_key().unwrap();
            let lookup = |_: &str| Some(public);
            let mut replay = body_assertion::ReplayCache::new(32);
            let cancelled = || false;
            let appends = Appends {
                rewrite: AtomicBool::new(false),
                renders: AtomicU64::new(0),
            };
            let mut client = OriginClient::new(
                OriginRead {
                    enrollment: &a,
                    resolver: &Resolver,
                    policies: &Policies,
                    renderer: &Renderer,
                    clock: &at,
                    cancelled: &cancelled,
                    audit: &mut origin_audit,
                },
                Loopback {
                    broker: &broker,
                    auth: &a,
                    fail_after_header: false,
                    posts: 0,
                },
                FixedEntropy,
                &mut replay,
                &lookup,
                &UnavailableExportView,
            );
            client.append_source = Some(&appends);
            assert_eq!(client.step(), Step::Empty);
            let mut service = AppendService::new(
                &broker,
                &a,
                &acls,
                &signing,
                "issuer",
                id(11),
                &mut receiver,
            );
            let subscribe = |service: &mut AppendService<'_, '_, &At>| {
                let mut template = signed(77).0;
                template.operation = wire::Operation::ReadPage {
                    page: opaque("append"),
                };
                service.subscribe(
                    Principal {
                        subject: "subject".into(),
                        audit_id: id(9),
                    },
                    a.scope.clone(),
                    template,
                    id(4),
                )
            };
            let (lease, watcher) = subscribe(&mut service).unwrap();
            if case == 8 {
                assert_eq!(client.step(), Step::Refused(Reason::AuditFailure));
                assert_eq!(service.collect(lease), Err(Reason::AuditFailure));
                assert_eq!(client.transport.posts, 1);
                assert_eq!(appends.renders.load(Ordering::Relaxed), 0);
                drop(watcher);
                drop(service);
                drop(client);
                assert_eq!(
                    std::fs::metadata(root.join("origin").join(crate::origin_audit::AUDIT_FILE))
                        .unwrap()
                        .len(),
                    0
                );
                drop(receiver);
                drop(origin_audit);
                std::fs::remove_dir_all(root).unwrap();
                continue;
            }
            assert_eq!(client.step(), Step::Delivered);
            let buffer_binding = broker
                .state
                .lock()
                .unwrap()
                .work
                .values()
                .find_map(|work| {
                    work.result
                        .as_ref()
                        .and_then(|result| result.as_ref().ok())
                        .map(|response| response.binding.clone())
                })
                .unwrap();
            assert!(service.collect(lease).unwrap());
            // Cases 9/10 release at 11_999, then report seven delivered bytes
            // after deadline/revocation. Accounting must precede closure.
            if case >= 9 {
                at.0.store(11_999, Ordering::Relaxed);
            }
            let bytes = service
                .release(lease)
                .unwrap()
                .canonical_bytes()
                .unwrap()
                .len() as u64;
            let expected = match case {
                0 => {
                    let journal_path = root.join("receiver").join(receiver_audit::AUDIT_FILE);
                    let journal_before = std::fs::read(&journal_path).unwrap();
                    let renders_before = appends.renders.load(Ordering::Relaxed);
                    let request_before = service.active_request_for_test(lease).request_id;
                    service.enrollment = &foreign;
                    service.maintain().unwrap();
                    let mut foreign_template = signed(90).0;
                    foreign_template.operation = wire::Operation::ReadPage {
                        page: opaque("append"),
                    };
                    // B has no established broker channel: subscribe refuses,
                    // but its preliminary maintenance must leave A untouched.
                    assert_eq!(
                        service
                            .subscribe(
                                Principal {
                                    subject: "subject".into(),
                                    audit_id: id(9)
                                },
                                foreign.scope.clone(),
                                foreign_template,
                                id(4)
                            )
                            .err(),
                        Some(Reason::Unavailable)
                    );
                    assert_eq!(service.delivered(lease, 7), Err(Reason::LeaseClosed));
                    assert_eq!(std::fs::read(&journal_path).unwrap(), journal_before);
                    assert_eq!(appends.renders.load(Ordering::Relaxed), renders_before);
                    assert_eq!(
                        service.active_request_for_test(lease).request_id,
                        request_before
                    );
                    assert_eq!(service.release(lease).err(), Some(Reason::LeaseClosed));
                    assert_eq!(service.release(id(99)).err(), Some(Reason::LeaseClosed));
                    service.enrollment = &a;
                    service.delivered(lease, bytes).unwrap();
                    service.acknowledge(lease, &opaque("next-1")).unwrap();
                    assert_eq!(client.step(), Step::Delivered);
                    assert!(service.collect(lease).unwrap());
                    assert_eq!(appends.renders.load(Ordering::Relaxed), renders_before + 1);
                    // Two 262_144-byte reservations exactly fill 524_288.
                    // A second-page excess is rejected before aggregate growth;
                    // a third page cannot exist because lease capacity is two.
                    let mut manager = crate::body_lease::LeaseManager::default();
                    let principal = Principal {
                        subject: "subject".into(),
                        audit_id: id(9),
                    };
                    let (left, left_watch) = manager
                        .create(
                            &principal,
                            a.scope.clone(),
                            [3; 32],
                            opaque("append"),
                            &acls,
                            &at,
                        )
                        .unwrap();
                    let (right, right_watch) = manager
                        .create(
                            &principal,
                            a.scope.clone(),
                            [3; 32],
                            opaque("append"),
                            &acls,
                            &at,
                        )
                        .unwrap();
                    manager
                        .request(left, &a.scope, buffer_binding.receiver_binding.request_id)
                        .unwrap();
                    manager
                        .request(right, &a.scope, buffer_binding.receiver_binding.request_id)
                        .unwrap();
                    manager
                        .verified(left, &a.scope, buffer_binding.clone(), 262_144)
                        .unwrap();
                    assert_eq!(
                        manager.verified(right, &a.scope, buffer_binding.clone(), 262_145),
                        Err(Reason::BindingMismatch)
                    );
                    manager
                        .verified(right, &a.scope, buffer_binding.clone(), 262_144)
                        .unwrap();
                    drop((left_watch, right_watch));
                    let (second, second_watcher) = subscribe(&mut service).unwrap();
                    assert_eq!(subscribe(&mut service).err(), Some(Reason::LeaseCapacity));
                    assert_eq!(client.step(), Step::Delivered);
                    assert!(service.collect(second).unwrap());
                    service.cancel(second).unwrap();
                    drop(second_watcher);
                    assert_eq!(client.step(), Step::Refused(Reason::Cancelled));
                    Reason::Cancelled
                }
                1 => {
                    service.delivered(lease, bytes).unwrap();
                    at.0.store(70_000, Ordering::Relaxed);
                    service.enrollment = &foreign;
                    service.sweep_owner(&a.scope, &a).unwrap();
                    service.enrollment = &a;
                    Reason::Deadline
                }
                2 => {
                    a.revoked.store(true, Ordering::Relaxed);
                    Reason::Revoked
                }
                3 => {
                    std::fs::write(&grants, br#"{"format":1,"grants":[]}"#).unwrap();
                    Reason::AclChanged
                }
                4 => {
                    let poll = wire::control(&wire::PollRequest {
                        version: wire::VERSION,
                        channel_binding: [4; 32],
                    })
                    .unwrap();
                    drop(broker.begin_poll(&a, &poll).unwrap());
                    Reason::ChannelRestart
                }
                5 => {
                    assert_eq!(service.delivered(lease, 1), Err(Reason::Cancelled));
                    Reason::Cancelled
                }
                6 | 7 => {
                    service.audit.fail_next_followup(case == 7);
                    assert_eq!(service.delivered(lease, bytes), Err(Reason::AuditFailure));
                    Reason::AuditFailure
                }
                9 | 10 => {
                    let reason = if case == 9 {
                        at.0.store(12_000, Ordering::Relaxed);
                        Reason::Deadline
                    } else {
                        a.revoked.store(true, Ordering::Relaxed);
                        Reason::Revoked
                    };
                    assert_eq!(service.delivered(lease, 7), Err(reason));
                    reason
                }
                _ => unreachable!(),
            };
            if case == 0 {
                service.cancel(lease).unwrap();
            }
            if (2..=4).contains(&case) {
                assert_eq!(service.release(lease).err(), Some(expected));
            }
            assert_eq!(service.release(lease).err(), Some(Reason::LeaseClosed));
            assert_eq!(
                service.acknowledge(lease, &opaque("next-1")),
                Err(Reason::LeaseClosed)
            );
            // No followup page is queued: any remaining exchange is Cancel only.
            assert!(
                broker
                    .state
                    .lock()
                    .unwrap()
                    .work
                    .values()
                    .all(
                        |work| matches!(work.request.operation, wire::Operation::Cancel { .. })
                            || work.result.is_some()
                    )
            );
            drop(watcher);
            drop(service);
            drop(client);
            let journal =
                std::fs::read_to_string(root.join("receiver").join(receiver_audit::AUDIT_FILE))
                    .unwrap();
            assert!(!journal.contains("safe typed body"));
            if case >= 6 {
                assert!(journal.contains("\"outcome\":\"failed\""));
            }
            if case == 5 {
                assert!(journal.contains("\"outcome\":\"cancelled\",\"bytes\":1"));
            }
            if case >= 9 {
                assert!(journal.contains("\"outcome\":\"failed\",\"bytes\":7"));
                assert!(journal.contains("\"event\":\"receipt\""));
                assert!(!journal.contains("\"outcome\":\"completed\""));
            }
            drop(receiver);
            drop(origin_audit);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn lease_scope_capacity_and_clock_boundaries() {
        use crate::body_lease::LeaseManager;
        use std::os::unix::fs::PermissionsExt;
        // Trace: created at 10_000, expiry exactly 70_000 (exclusive).
        // Two leases fill capacity; foreign scope is refused before state lookup.
        let at = At(AtomicU64::new(10_000));
        let a = auth("lease-bounds");
        let foreign = auth("foreign");
        let root = std::env::temp_dir().join(format!("lease-bounds-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let grants = root.join("grants.json");
        std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
        std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600)).unwrap();
        let acls = Acls::new(root.clone());
        let principal = Principal {
            subject: "subject".into(),
            audit_id: id(9),
        };
        let mut leases = LeaseManager::default();
        let (first, watcher) = leases
            .create(
                &principal,
                a.scope.clone(),
                [3; 32],
                opaque("page"),
                &acls,
                &at,
            )
            .unwrap();
        let (_, second_watcher) = leases
            .create(
                &principal,
                a.scope.clone(),
                [3; 32],
                opaque("page"),
                &acls,
                &at,
            )
            .unwrap();
        assert_eq!(
            leases
                .create(
                    &principal,
                    a.scope.clone(),
                    [3; 32],
                    opaque("page"),
                    &acls,
                    &at
                )
                .err(),
            Some(Reason::LeaseCapacity)
        );
        assert_eq!(
            leases.check(first, &foreign.scope, [3; 32], &foreign, &acls, &at),
            Err(Reason::LeaseClosed)
        );
        assert_eq!(
            leases.check(id(99), &foreign.scope, [3; 32], &foreign, &acls, &at),
            Err(Reason::LeaseClosed)
        );
        assert_eq!(
            leases.check(first, &a.scope, [4; 32], &a, &acls, &at),
            Err(Reason::ChannelRestart)
        );
        at.0.store(69_999, Ordering::Relaxed);
        assert!(
            leases
                .check(first, &a.scope, [3; 32], &a, &acls, &at)
                .is_ok()
        );
        at.0.store(70_000, Ordering::Relaxed);
        assert_eq!(
            leases.check(first, &a.scope, [3; 32], &a, &acls, &at),
            Err(Reason::Deadline)
        );
        at.0.store(10_000, Ordering::Relaxed);
        a.revoked.store(true, Ordering::Relaxed);
        assert_eq!(
            leases.check(first, &a.scope, [3; 32], &a, &acls, &at),
            Err(Reason::Revoked)
        );
        a.revoked.store(false, Ordering::Relaxed);
        std::fs::write(&grants, br#"{"format":1,"grants":[]}"#).unwrap();
        assert_eq!(
            leases.check(first, &a.scope, [3; 32], &a, &acls, &at),
            Err(Reason::AclChanged)
        );
        drop(watcher);
        assert_eq!(
            leases.check(first, &a.scope, [3; 32], &a, &acls, &at),
            Err(Reason::NoWatchers)
        );
        leases.close(first, &a.scope).unwrap();
        assert_eq!(
            leases.close(first, &a.scope).err(),
            Some(Reason::LeaseClosed)
        );
        drop(second_watcher);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn abandoned_expired_requests_release_capacity_and_cancel_cleans_queue() {
        let at = At(AtomicU64::new(10_000));
        let a = auth("a");
        let broker = Broker::new(Config::default(), &at).unwrap();
        let poll = broker.begin_poll(&a, &poll_bytes()).unwrap();
        drop(poll);
        for n in 1..=4 {
            broker
                .enqueue(&a, a.scope.clone(), [3; 32], signed(n).0)
                .unwrap();
        }
        at.0.store(20_001, Ordering::Relaxed);
        let mut next = signed(5).0;
        next.deadline_millis = 30_000;
        broker.enqueue(&a, a.scope.clone(), [3; 32], next).unwrap();
        broker.cancel(id(5)).unwrap();
        assert!(!broker.state.lock().unwrap().work.contains_key(&id(5)));
        assert!(
            broker
                .state
                .lock()
                .unwrap()
                .queues
                .values()
                .all(|q| q.is_empty())
        );
        for n in 6..=30 {
            let mut request = signed(n).0;
            request.deadline_millis = 30_000;
            broker
                .enqueue(&a, a.scope.clone(), [3; 32], request)
                .unwrap();
            at.0.store(30_001, Ordering::Relaxed);
            broker.revocation_changed(&a).unwrap();
            at.0.store(20_001, Ordering::Relaxed);
        }
        assert!(broker.state.lock().unwrap().work.len() <= 16);
    }
    #[test]
    fn typed_payload_refuses_trailing_unknown_tags_and_empty() {
        let mut bytes = page().canonical_bytes().unwrap().to_vec();
        assert!(decode_page(&bytes).is_ok());
        bytes.push(0);
        assert!(decode_page(&bytes).is_err());
        assert!(decode_page(b"html").is_err());
    }
}
