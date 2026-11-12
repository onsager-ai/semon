//! Bounded receiver lease bookkeeping. Append payloads never enter BodyCache.
use crate::{
    body_broker::{EnrollmentSource, Principal},
    body_wire::{Operation, Reason, Result},
    origin_read::{EnrollmentScope, OpaqueId, ReadClock, ResponseBinding},
    receiver_acl::{Acls, Decision, Permission},
    receiver_audit::Identifier,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

pub const LEASE_WINDOW_MILLIS: u64 = 60_000;
pub const LEASE_CAP: usize = 2;
pub const PAGE_CAP: usize = 256 * 1024;
pub const ENROLLMENT_BYTE_CAP: usize = LEASE_CAP * PAGE_CAP;

/// Dropping the last handle stops the lease at the next bounded exchange.
pub struct Watcher(Arc<AtomicUsize>);
impl Clone for Watcher {
    fn clone(&self) -> Self {
        self.0.fetch_add(1, Ordering::Relaxed);
        Self(self.0.clone())
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
struct Lease {
    scope: EnrollmentScope,
    channel: [u8; 32],
    principal: String,
    grant: String,
    watchers: Arc<AtomicUsize>,
    page: OpaqueId,
    deadline: u64,
    request: Option<Identifier>,
    delivered: bool,
    binding: Option<ResponseBinding>,
    bytes: usize,
}
#[derive(Default)]
pub struct LeaseManager {
    leases: BTreeMap<Identifier, Lease>,
}
impl LeaseManager {
    pub fn create(
        &mut self,
        principal: &Principal,
        scope: EnrollmentScope,
        channel: [u8; 32],
        page: OpaqueId,
        acls: &Acls,
        clock: &dyn ReadClock,
    ) -> Result<(Identifier, Watcher)> {
        let grant = match acls.decide(
            &principal.subject,
            scope.machine.as_str(),
            scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            Decision::Granted(grant) => grant.version().to_owned(),
            Decision::Denied => return Err(Reason::Forbidden),
        };
        if self
            .leases
            .values()
            .filter(|l| l.scope.enrollment_id == scope.enrollment_id)
            .count()
            >= LEASE_CAP
        {
            return Err(Reason::LeaseCapacity);
        }
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| Reason::Unavailable)?;
        let id = Identifier::from_bytes(random);
        if self.leases.contains_key(&id) {
            return Err(Reason::Unavailable);
        }
        let watchers = Arc::new(AtomicUsize::new(1));
        self.leases.insert(
            id,
            Lease {
                scope,
                channel,
                principal: principal.subject.clone(),
                grant,
                watchers: watchers.clone(),
                page,
                deadline: clock.now_millis().saturating_add(LEASE_WINDOW_MILLIS),
                request: None,
                delivered: false,
                binding: None,
                bytes: 0,
            },
        );
        Ok((id, Watcher(watchers)))
    }
    pub fn check(
        &self,
        id: Identifier,
        scope: &EnrollmentScope,
        channel: [u8; 32],
        auth: &dyn EnrollmentSource,
        acls: &Acls,
        clock: &dyn ReadClock,
    ) -> Result<()> {
        let lease = self
            .leases
            .get(&id)
            .filter(|l| l.scope == *scope)
            .ok_or(Reason::LeaseClosed)?;
        if !auth.is_current(scope) {
            return Err(Reason::Revoked);
        }
        if lease.channel != channel {
            return Err(Reason::ChannelRestart);
        }
        if clock.now_millis() >= lease.deadline {
            return Err(Reason::Deadline);
        }
        if lease.watchers.load(Ordering::Relaxed) == 0 {
            return Err(Reason::NoWatchers);
        }
        match acls.decide(
            &lease.principal,
            scope.machine.as_str(),
            scope.namespace.as_str(),
            Permission::BodyRead,
        ) {
            Decision::Granted(grant) if grant.version() == lease.grant => Ok(()),
            _ => Err(Reason::AclChanged),
        }
    }
    /// Reserves the only in-flight page. Every caller must run check immediately
    /// before this operation and before releasing the verified page.
    pub fn request(
        &mut self,
        id: Identifier,
        scope: &EnrollmentScope,
        request: Identifier,
    ) -> Result<Operation> {
        let lease = self
            .leases
            .get_mut(&id)
            .filter(|l| l.scope == *scope)
            .ok_or(Reason::LeaseClosed)?;
        if lease.request.is_some() || lease.binding.is_some() {
            return Err(Reason::InvalidRequest);
        }
        lease.request = Some(request);
        Ok(Operation::SubscribeAppends {
            page: lease.page.clone(),
            lease_id: id,
            lease_deadline: lease.deadline,
            continuation: None,
        })
    }
    /// Called after receiver authorize has persisted, before any body release.
    pub fn verified(
        &mut self,
        id: Identifier,
        scope: &EnrollmentScope,
        binding: ResponseBinding,
        bytes: usize,
    ) -> Result<()> {
        let lease = self
            .leases
            .get(&id)
            .filter(|l| l.scope == *scope)
            .ok_or(Reason::LeaseClosed)?;
        if lease.request != Some(binding.receiver_binding.request_id)
            || binding.continuation.is_none()
            || bytes > PAGE_CAP
        {
            return Err(Reason::BindingMismatch);
        }
        if lease.binding.as_ref().map_or_else(
            || !binding.append_continuity.is_empty(),
            |previous| !append_extension(previous, &binding),
        ) {
            return Err(Reason::ContinuationInvalid);
        }
        let total: usize = self
            .leases
            .values()
            .filter(|l| l.scope.enrollment_id == scope.enrollment_id)
            .map(|l| l.bytes)
            .sum();
        if total.saturating_add(bytes) > ENROLLMENT_BYTE_CAP {
            return Err(Reason::OverBudget);
        }
        let lease = self.leases.get_mut(&id).ok_or(Reason::LeaseClosed)?;
        lease.binding = Some(binding);
        lease.bytes = bytes;
        lease.delivered = false;
        Ok(())
    }
    pub fn delivered(&mut self, id: Identifier, scope: &EnrollmentScope) -> Result<()> {
        let lease = self
            .leases
            .get_mut(&id)
            .filter(|l| l.scope == *scope)
            .ok_or(Reason::LeaseClosed)?;
        if lease.binding.is_none() || lease.delivered {
            return Err(Reason::InvalidRequest);
        }
        lease.delivered = true;
        Ok(())
    }
    /// Acknowledgement is the sole renewal/next-page gate; replay is refused.
    pub fn acknowledge(
        &mut self,
        id: Identifier,
        scope: &EnrollmentScope,
        request: Identifier,
        previous: Identifier,
        continuation: &OpaqueId,
        clock: &dyn ReadClock,
    ) -> Result<Operation> {
        let lease = self
            .leases
            .get_mut(&id)
            .filter(|l| l.scope == *scope)
            .ok_or(Reason::LeaseClosed)?;
        let binding = lease.binding.as_ref().ok_or(Reason::InvalidRequest)?;
        if !lease.delivered
            || lease.request != Some(previous)
            || binding.continuation.as_ref() != Some(continuation)
            || clock.now_millis() >= lease.deadline
        {
            return Err(Reason::InvalidRequest);
        }
        lease.deadline = clock.now_millis().saturating_add(LEASE_WINDOW_MILLIS);
        lease.request = Some(request);
        lease.delivered = false;
        lease.bytes = 0;
        Ok(Operation::Acknowledge {
            request_id: previous,
            lease_id: id,
            page: lease.page.clone(),
            lease_deadline: lease.deadline,
            continuation: continuation.clone(),
        })
    }
    pub fn close(&mut self, id: Identifier, scope: &EnrollmentScope) -> Result<Operation> {
        let lease = self
            .leases
            .get(&id)
            .filter(|l| l.scope == *scope)
            .ok_or(Reason::LeaseClosed)?;
        let request_id = lease.request.unwrap_or(id);
        self.leases.remove(&id);
        Ok(Operation::Cancel {
            request_id,
            lease_id: Some(id),
        })
    }
}
/// Both endpoints use the same acknowledged-prefix extension contract.
pub(crate) fn append_extension(a: &ResponseBinding, b: &ResponseBinding) -> bool {
    let grown = a
        .cache_key
        .cohort
        .iter()
        .filter(|old| {
            b.cache_key.cohort.iter().any(|new| {
                new.source_id == old.source_id && new.consumed_boundary > old.consumed_boundary
            })
        })
        .count();
    a.enrollment == b.enrollment
        && a.source_handle == b.source_handle
        && a.cohort_id == b.cohort_id
        && a.cache_key.cohort.len() == b.cache_key.cohort.len()
        && b.append_continuity.len() == grown
        && a.cache_key.cohort.iter().enumerate().all(|(i, old)| {
            !a.cache_key.cohort[..i]
                .iter()
                .any(|s| s.source_id == old.source_id)
                && b.cache_key
                    .cohort
                    .iter()
                    .filter(|s| s.source_id == old.source_id)
                    .count()
                    == 1
                && b.cache_key
                    .cohort
                    .iter()
                    .find(|s| s.source_id == old.source_id)
                    .is_some_and(|new| {
                        old.chain == new.chain
                            && old.consumed_boundary <= new.consumed_boundary
                            && if old.consumed_boundary == new.consumed_boundary {
                                old.prefix_sha256 == new.prefix_sha256
                            } else {
                                let mut evidence = b
                                    .append_continuity
                                    .iter()
                                    .filter(|e| e.source_id.as_str() == old.source_id);
                                evidence.next().is_some_and(|e| {
                                    e.chain.as_str() == old.chain
                                        && e.previous_boundary == old.consumed_boundary
                                        && String::from(e.prefix_sha256) == old.prefix_sha256
                                }) && evidence.next().is_none()
                            }
                    })
        })
        && a.cache_key.selector == b.cache_key.selector
        && a.mapping.anchor == b.mapping.anchor
        && a.cache_key.export_view_generation == b.cache_key.export_view_generation
        && a.policy == b.policy
}
