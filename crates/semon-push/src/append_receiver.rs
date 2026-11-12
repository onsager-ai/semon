//! Nonblocking receiver append service. Hosts drive maintain at every exchange.
//! Bodies are held for one page only and never admitted to the encrypted cache.
use crate::{
    body_assertion::{self, Audience, Claims, SigningKey},
    body_broker::{Broker, EnrollmentSource, Principal, decode_page},
    body_cache::Clock,
    body_lease::{LeaseManager, Watcher},
    body_wire::{self as wire, Operation, Reason, Request, Result},
    origin_read::{EnrollmentScope, OpaqueId, TypedPage},
    receiver_acl::{Acls, Decision, Permission},
    receiver_audit::{
        self, AttemptId, AttemptRecord, AuditStore, Identifier, Outcome, PermittedRange, Purpose,
    },
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

struct Active {
    principal: Principal,
    scope: EnrollmentScope,
    template: Request,
    generation: Identifier,
    attempt: Option<AttemptId>,
    payload: Option<Zeroizing<Vec<u8>>>,
    released: bool,
    continuation: Option<OpaqueId>,
    delivered: bool,
    delivered_bytes: u64,
    page_deadline: u64,
    grant: String,
}
pub struct AppendService<'a, 'b, C: Clock> {
    pub broker: &'a Broker<'b>,
    pub enrollment: &'a dyn EnrollmentSource,
    pub acls: &'a Acls,
    pub signing_key: &'a dyn SigningKey,
    pub issuer_key_id: &'a str,
    pub receiver_policy_version: Identifier,
    pub audit: &'a mut AuditStore<C>,
    leases: LeaseManager,
    active: BTreeMap<Identifier, Active>,
}
impl<'a, 'b, C: Clock> AppendService<'a, 'b, C> {
    pub fn new(
        broker: &'a Broker<'b>,
        enrollment: &'a dyn EnrollmentSource,
        acls: &'a Acls,
        signing_key: &'a dyn SigningKey,
        issuer_key_id: &'a str,
        receiver_policy_version: Identifier,
        audit: &'a mut AuditStore<C>,
    ) -> Self {
        Self {
            broker,
            enrollment,
            acls,
            signing_key,
            issuer_key_id,
            receiver_policy_version,
            audit,
            leases: LeaseManager::default(),
            active: BTreeMap::new(),
        }
    }
    pub fn subscribe(
        &mut self,
        principal: Principal,
        scope: EnrollmentScope,
        template: Request,
        generation: Identifier,
    ) -> Result<(Identifier, Watcher)> {
        self.maintain()?;
        if !self.enrollment.is_current(&scope) {
            return Err(Reason::Revoked);
        }
        let page = match template.operation.selector()? {
            wire::Selector::Page { id } => id,
            _ => return Err(Reason::InvalidRequest),
        };
        let grant = match self.acls.decide(
            &principal.subject,
            scope.machine.as_str(),
            scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            Decision::Granted(grant) => grant.version().to_owned(),
            _ => return Err(Reason::Forbidden),
        };
        let channel = self.broker.channel(&scope)?;
        let (id, watcher) = self.leases.create(
            &principal,
            scope.clone(),
            channel,
            page,
            self.acls,
            self.broker.clock,
        )?;
        let request_id = random_id()?;
        let operation = self.leases.request(id, &scope, request_id)?;
        self.active.insert(
            id,
            Active {
                principal,
                scope,
                template,
                generation,
                attempt: None,
                payload: None,
                released: false,
                continuation: None,
                delivered: false,
                delivered_bytes: 0,
                page_deadline: u64::MAX,
                grant,
            },
        );
        if let Err(reason) = self.enqueue(id, request_id, operation) {
            let _ = self.close(id, reason);
            return Err(reason);
        }
        Ok((id, watcher))
    }
    fn check(&self, id: Identifier) -> Result<()> {
        let active = self.active.get(&id).ok_or(Reason::LeaseClosed)?;
        if !self.enrollment.is_current(&active.scope) {
            return Err(Reason::Revoked);
        }
        if active.payload.is_some() && self.broker.clock.now_millis() >= active.page_deadline {
            return Err(Reason::Deadline);
        }
        let channel = self
            .broker
            .channel(&active.scope)
            .map_err(|_| Reason::ChannelRestart)?;
        self.leases.check(
            id,
            &active.scope,
            channel,
            self.enrollment,
            self.acls,
            self.broker.clock,
        )
    }
    /// Recheck at each poll/control exchange and before every release. Closed IDs
    /// remain unavailable; cancellation is sent only through the bounded queue.
    pub fn revocation_changed(&mut self) -> Result<()> {
        self.maintain()
    }
    /// The transport calls this when the enrollment connection is lost. A new
    /// connection starts fresh subscriptions, even if the credential is current.
    pub fn disconnected(&mut self, scope: &EnrollmentScope) -> Result<()> {
        let ids: Vec<_> = self
            .active
            .iter()
            .filter(|(_, active)| active.scope == *scope)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.close(id, Reason::ChannelRestart)?;
        }
        Ok(())
    }
    pub fn maintain(&mut self) -> Result<()> {
        let closed: Vec<_> = self
            .active
            .keys()
            .filter_map(|id| self.check(*id).err().map(|reason| (*id, reason)))
            .collect();
        for (id, reason) in closed {
            self.close(id, reason)?;
        }
        Ok(())
    }
    fn sign(&self, active: &Active, request: &mut Request) -> Result<()> {
        let grant = match self.acls.decide(
            &active.principal.subject,
            active.scope.machine.as_str(),
            active.scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            Decision::Granted(grant) => grant.version().to_owned(),
            _ if matches!(request.operation, Operation::Cancel { .. }) => active.grant.clone(),
            _ => return Err(Reason::AclChanged),
        };
        let now = self.broker.clock.now_millis();
        let mut nonce = [0; 32];
        hex::decode_to_slice(String::from(request.request_id), &mut nonce)
            .map_err(|_| Reason::InvalidRequest)?;
        let claims = Claims {
            issuer_key_id: self.issuer_key_id.into(),
            issued_at: now / 1000,
            expiry: request.deadline_millis / 1000,
            deadline: request.deadline_millis / 1000,
            audience: Audience {
                origin_incarnation: active.scope.origin_incarnation.as_str().into(),
                namespace: active.scope.namespace.as_str().into(),
            },
            claim_epoch: active
                .scope
                .publisher_claim_epoch
                .ok_or(Reason::Unavailable)?,
            nonce,
            subject_principal: active.principal.subject.clone(),
            machine: active.scope.machine.as_str().into(),
            namespace: active.scope.namespace.as_str().into(),
            acl_grant_version: grant,
            receiver_policy_version: String::from(self.receiver_policy_version),
            request_digest: request.digest()?,
            channel_binding: self.broker.channel(&active.scope)?,
        };
        request.assertion = hex::encode(body_assertion::encode(
            &body_assertion::sign(claims, self.signing_key, || now / 1000)
                .map_err(|_| Reason::Unavailable)?,
        ));
        Ok(())
    }
    fn enqueue(
        &mut self,
        id: Identifier,
        request_id: Identifier,
        operation: Operation,
    ) -> Result<()> {
        self.check(id)?;
        let active = self.active.get(&id).ok_or(Reason::LeaseClosed)?;
        let mut request = active.template.clone();
        request.request_id = request_id;
        request.operation = operation;
        request.deadline_millis = self.broker.clock.now_millis().saturating_add(2_000);
        let attempt = self
            .audit
            .begin_attempt(AttemptRecord {
                request_id,
                authenticated_principal: active.principal.audit_id,
                source_handle: Identifier::from_bytes(
                    Sha256::digest(request.source_handle.as_str().as_bytes()).into(),
                ),
                generation: active.generation,
                permitted_range: PermittedRange::new(0, request.max_bytes as u64)
                    .ok_or(Reason::OverBudget)?,
                purpose: Purpose::StreamRange,
                receiver_policy_version: self.receiver_policy_version,
                origin_policy_version: None,
                origin_redactor_version: None,
                decision: receiver_audit::Decision::Granted,
                destination: request.destination,
            })
            .map_err(|_| Reason::AuditFailure)?;
        let result = (|| {
            self.sign(active, &mut request)?;
            self.broker.enqueue(
                self.enrollment,
                active.scope.clone(),
                self.broker.channel(&active.scope)?,
                request.clone(),
            )
        })();
        if let Err(reason) = result {
            self.audit
                .complete(attempt, Outcome::Failed, 0)
                .map_err(|_| Reason::AuditFailure)?;
            return Err(reason);
        }
        let active = self.active.get_mut(&id).ok_or(Reason::LeaseClosed)?;
        active.template = request;
        active.attempt = Some(attempt);
        active.delivered = false;
        active.released = false;
        active.delivered_bytes = 0;
        Ok(())
    }
    /// Collect a fully verified page without waiting. Authorization is persisted
    /// before the payload becomes available to release.
    pub fn collect(&mut self, id: Identifier) -> Result<bool> {
        if let Err(reason) = self.check(id) {
            self.close(id, reason)?;
            return Err(reason);
        }
        let active = self.active.get(&id).ok_or(Reason::LeaseClosed)?;
        if active.payload.is_some() || active.delivered {
            return Ok(false);
        }
        let result = self
            .broker
            .take_result(self.enrollment, active.template.request_id);
        let verified = match result {
            Ok(Some(verified)) => verified,
            Ok(None) => return Ok(false),
            Err(reason) => {
                self.close(id, reason)?;
                return Err(reason);
            }
        };
        let result = (|| {
            self.check(id)?;
            let active = self.active.get(&id).ok_or(Reason::LeaseClosed)?;
            if verified.binding.cohort_id != active.generation {
                return Err(Reason::ContinuationInvalid);
            }
            decode_page(&verified.payload)?;
            let attempt = active.attempt.ok_or(Reason::AuditFailure)?;
            self.audit
                .authorize(attempt, verified.binding.receiver_binding.clone())
                .map_err(|_| Reason::AuditFailure)?;
            self.leases.verified(
                id,
                &active.scope,
                verified.binding.clone(),
                verified.payload.len(),
            )?;
            self.check(id)?;
            Ok(())
        })();
        if let Err(reason) = result {
            self.close(id, reason)?;
            return Err(reason);
        }
        let active = self.active.get_mut(&id).ok_or(Reason::LeaseClosed)?;
        active.page_deadline = verified.binding.deadline_millis;
        active.continuation = verified.binding.continuation;
        active.payload = Some(Zeroizing::new(verified.payload));
        Ok(true)
    }
    pub fn release(&mut self, id: Identifier) -> Result<TypedPage> {
        if let Err(reason) = self.check(id) {
            self.close(id, reason)?;
            return Err(reason);
        }
        let active = self.active.get_mut(&id).ok_or(Reason::LeaseClosed)?;
        let page = decode_page(active.payload.as_ref().ok_or(Reason::InvalidRequest)?)?;
        if active.released {
            return Err(Reason::InvalidRequest);
        }
        active.released = true;
        Ok(page)
    }
    /// Report the whole admitted page after delivery. A page is one bounded range
    /// receipt and one completed attempt; partial delivery closes the lease.
    pub fn delivered(&mut self, id: Identifier, bytes: u64) -> Result<()> {
        if let Err(reason) = self.check(id) {
            self.close(id, reason)?;
            return Err(reason);
        }
        let active = self.active.get_mut(&id).ok_or(Reason::LeaseClosed)?;
        let expected = active.payload.as_ref().ok_or(Reason::InvalidRequest)?.len() as u64;
        if !active.released || bytes > expected {
            return Err(Reason::InvalidRequest);
        }
        if bytes < expected {
            let attempt = active.attempt.ok_or(Reason::AuditFailure)?;
            active.delivered_bytes = bytes;
            if bytes > 0 {
                self.audit
                    .receipt(
                        attempt,
                        PermittedRange::new(0, bytes).ok_or(Reason::InvalidRequest)?,
                        bytes,
                    )
                    .map_err(|_| Reason::AuditFailure)?;
            }
            self.close(id, Reason::Cancelled)?;
            return Err(Reason::Cancelled);
        }
        let attempt = active.attempt.ok_or(Reason::AuditFailure)?;
        active.delivered_bytes = bytes;
        let range = PermittedRange::new(0, bytes).ok_or(Reason::InvalidRequest)?;
        let result = self
            .audit
            .receipt(attempt, range, bytes)
            .and_then(|()| self.audit.complete(attempt, Outcome::Completed, bytes));
        if result.is_err() {
            let _ = self.close(id, Reason::AuditFailure);
            return Err(Reason::AuditFailure);
        }
        let active = self.active.get_mut(&id).ok_or(Reason::LeaseClosed)?;
        active.attempt = None;
        active.payload = None;
        active.delivered = true;
        self.leases.delivered(id, &active.scope)
    }
    pub fn acknowledge(&mut self, id: Identifier, continuation: &OpaqueId) -> Result<()> {
        if let Err(reason) = self.check(id) {
            self.close(id, reason)?;
            return Err(reason);
        }
        let active = self.active.get(&id).ok_or(Reason::LeaseClosed)?;
        if !active.delivered || active.continuation.as_ref() != Some(continuation) {
            return Err(Reason::InvalidRequest);
        }
        let request = random_id()?;
        let operation = self.leases.acknowledge(
            id,
            &active.scope,
            request,
            active.template.request_id,
            continuation,
            self.broker.clock,
        )?;
        if let Err(reason) = self.enqueue(id, request, operation) {
            self.close(id, reason)?;
            return Err(reason);
        }
        Ok(())
    }
    pub fn cancel(&mut self, id: Identifier) -> Result<()> {
        self.close(id, Reason::Cancelled)
    }
    fn close(&mut self, id: Identifier, reason: Reason) -> Result<()> {
        let active = self.active.remove(&id).ok_or(Reason::LeaseClosed)?;
        let operation = self.leases.close(id, &active.scope)?;
        self.broker.cancel(active.template.request_id)?;
        if let Some(attempt) = active.attempt {
            self.audit
                .complete(
                    attempt,
                    if matches!(reason, Reason::Cancelled | Reason::NoWatchers) {
                        Outcome::Cancelled
                    } else {
                        Outcome::Failed
                    },
                    active.delivered_bytes,
                )
                .map_err(|_| Reason::AuditFailure)?;
        }
        // Revoked/changed channels cannot carry an old capability. The origin
        // independently expires its state, or clears it on disconnect/restart.
        if self.enrollment.is_current(&active.scope) && self.broker.channel(&active.scope).is_ok() {
            let mut request = active.template.clone();
            request.request_id = random_id()?;
            request.operation = operation;
            request.deadline_millis = self.broker.clock.now_millis().saturating_add(2_000);
            if self.sign(&active, &mut request).is_ok() {
                self.broker.enqueue(
                    self.enrollment,
                    active.scope.clone(),
                    self.broker.channel(&active.scope)?,
                    request,
                )?;
            }
        }
        Ok(())
    }
}
impl<C: Clock> Drop for AppendService<'_, '_, C> {
    fn drop(&mut self) {
        let ids: Vec<_> = self.active.keys().copied().collect();
        for id in ids {
            let _ = self.close(id, Reason::Cancelled);
        }
    }
}
fn random_id() -> Result<Identifier> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| Reason::Unavailable)?;
    Ok(Identifier::from_bytes(bytes))
}
