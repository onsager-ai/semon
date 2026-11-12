use super::*;
use crate::{
    credentials::{Decision, Secret},
    generation_audit::{self, EndedGeneration, EndedReason},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerAction {
    Revoke,
    Enable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerState {
    Confirm {
        enrollment_id: String,
        endpoint: String,
    },
    Complete {
        endpoint: String,
    },
    UnknownMachine,
    UnknownKind,
    LastKind,
    AlreadyEnabled,
    NoAuthority {
        endpoint: String,
    },
}
impl Registry {
    pub(crate) fn check_admission(&self) -> Result<()> {
        let size = serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() as u64;
        // Reserve the maximum terminal timestamp/availability deltas per record.
        let terminal_delta =
            self.credentials.len() as u64 * 20 + self.enrollments.len() as u64 * 20;
        if size > ADMISSION_CAP || size.saturating_add(terminal_delta) > CAP {
            return Err(Error::Full);
        }
        Ok(())
    }
    pub fn usable_machine(&self, machine: &str, now: u64) -> bool {
        self.enrollments
            .iter()
            .any(|e| e.machine == machine && self.usable(e, now))
    }
    fn usable(&self, e: &Enrollment, now: u64) -> bool {
        self.credentials
            .iter()
            .filter(|b| b.enrollment_id == e.enrollment_id)
            .any(|b| {
                let scope = e.credential_kinds.iter().find(|s| s.kind == b.kind);
                scope.is_some_and(|s| {
                    e.revoked_at.is_none()
                        && s.available
                        && b.revoked_at.is_none()
                        && b.valid_from <= now
                        && now < b.valid_until
                        && s.generations.accepts(b.generation, now)
                })
            })
    }
    pub fn owner_state(
        &self,
        machine: &str,
        kind: Option<Kind>,
        action: OwnerAction,
        now: u64,
    ) -> OwnerState {
        let live = self
            .enrollments
            .iter()
            .filter(|e| e.machine == machine && e.revoked_at.is_none())
            .max_by_key(|e| e.sequence);
        let target = live.or_else(|| {
            self.enrollments
                .iter()
                .filter(|e| e.machine == machine)
                .max_by_key(|e| e.sequence)
        });
        let Some(e) = target else {
            return if self.tombstones.iter().any(|t| t.machine == machine) {
                if action == OwnerAction::Revoke {
                    OwnerState::Complete {
                        endpoint: String::new(),
                    }
                } else {
                    OwnerState::NoAuthority {
                        endpoint: String::new(),
                    }
                }
            } else {
                OwnerState::UnknownMachine
            };
        };
        let scope = kind.and_then(|k| e.credential_kinds.iter().find(|s| s.kind == k));
        if kind.is_some() && scope.is_none() {
            return OwnerState::UnknownKind;
        }
        match action {
            OwnerAction::Revoke => {
                if e.revoked_at.is_some() || scope.is_some_and(|s| !s.available) {
                    return OwnerState::Complete {
                        endpoint: e.endpoint.clone(),
                    };
                }
                if scope.is_some() && e.credential_kinds.iter().filter(|s| s.available).count() == 1
                {
                    return OwnerState::LastKind;
                }
            }
            OwnerAction::Enable => {
                let Some(scope) = scope else {
                    return OwnerState::UnknownKind;
                };
                if e.revoked_at.is_some() || !self.usable(e, now) {
                    return OwnerState::NoAuthority {
                        endpoint: e.endpoint.clone(),
                    };
                }
                if scope.available {
                    return OwnerState::AlreadyEnabled;
                }
                if scope
                    .enable_authorization
                    .as_ref()
                    .is_some_and(|a| a.consumed_renewal_id.is_none())
                {
                    return OwnerState::Complete {
                        endpoint: e.endpoint.clone(),
                    };
                }
            }
        }
        OwnerState::Confirm {
            enrollment_id: e.enrollment_id.clone(),
            endpoint: e.endpoint.clone(),
        }
    }
    pub(crate) fn revoke_enrollment(&mut self, id: &str, kind: Option<Kind>, now: u64) {
        if let Some(e) = self.enrollments.iter_mut().find(|e| e.enrollment_id == id) {
            if kind.is_none() {
                e.revoked_at = Some(now);
            }
            for scope in &mut e.credential_kinds {
                if kind.is_none_or(|k| k == scope.kind) {
                    scope.available = false;
                    scope.enable_authorization = None;
                }
            }
        }
        for b in &mut self.credentials {
            if b.enrollment_id == id && kind.is_none_or(|k| k == b.kind) && b.revoked_at.is_none() {
                b.revoked_at = Some(now);
            }
        }
    }
    pub(crate) fn ended(&self, b: &Bearer, now: u64) -> Option<EndedReason> {
        let e = self
            .enrollments
            .iter()
            .find(|e| e.enrollment_id == b.enrollment_id)?;
        let scope = e.credential_kinds.iter().find(|s| s.kind == b.kind)?;
        if b.revoked_at.is_some() || e.revoked_at.is_some() || !scope.available {
            Some(EndedReason::Revoked)
        } else if now >= b.valid_until {
            Some(EndedReason::Expired)
        } else if !scope.generations.accepts(b.generation, now) {
            Some(EndedReason::Retired)
        } else {
            None
        }
    }
}
impl Store {
    pub fn owner_update(
        &mut self,
        receiver: &Path,
        machine: &str,
        kind: Option<Kind>,
        action: OwnerAction,
        owner: &str,
        confirmed_id: &str,
        now: u64,
    ) -> Result<OwnerState> {
        let mut next = self.snapshot()?;
        let now = now.max(next.latest);
        let state = next.owner_state(machine, kind, action, now);
        let OwnerState::Confirm { enrollment_id, .. } = &state else {
            return Ok(state);
        };
        // A confirmation for one enrollment never authorizes its replacement.
        if enrollment_id != confirmed_id {
            return Err(Error::StaleSnapshot);
        }
        match action {
            OwnerAction::Revoke => next.revoke_enrollment(confirmed_id, kind, now),
            OwnerAction::Enable => {
                generation_audit::append(receiver, &[])?;
                let scope = next
                    .enrollments
                    .iter_mut()
                    .find(|e| e.enrollment_id == confirmed_id)
                    .ok_or(Error::Invalid)?
                    .credential_kinds
                    .iter_mut()
                    .find(|s| Some(s.kind) == kind)
                    .ok_or(Error::Invalid)?;
                scope.enable_authorization = Some(EnableAuthorization {
                    enrollment_id: confirmed_id.into(),
                    kind: scope.kind,
                    owner: owner.into(),
                    authorization_id: random_id()?,
                    created_at: now,
                    consumed_renewal_id: None,
                });
            }
        }
        next.latest = now;
        if action == OwnerAction::Enable {
            next.check_admission()?;
        }
        self.commit(next.revision(), &next)?;
        Ok(OwnerState::Complete {
            endpoint: match state {
                OwnerState::Confirm { endpoint, .. } => endpoint,
                _ => unreachable!(),
            },
        })
    }
    /// Audit-first compaction. The store's exclusive registry lock prevents a
    /// generation changing between the evidence check and the durable replacement.
    pub fn compact(&mut self, receiver: &Path, now: u64) -> Result<()> {
        let mut next = self.snapshot()?;
        let now = now.max(next.latest);
        let records: Vec<_> = next
            .credentials
            .iter()
            .filter_map(|b| {
                let reason = next.ended(b, now)?;
                let pin = next
                    .enrollments
                    .iter()
                    .find(|e| e.enrollment_id == b.enrollment_id)?
                    .client_certificate_pins
                    .iter()
                    .find(|p| p.generation == b.generation)
                    .map(|p| p.pin.clone());
                Some(EndedGeneration::new(b, pin, reason, now))
            })
            .collect();
        generation_audit::append(receiver, &records)?;
        if records.is_empty() {
            return Ok(());
        }
        next.credentials
            .retain(|b| !records.iter().any(|r| r.credential_id == b.credential_id));
        for e in &mut next.enrollments {
            e.client_certificate_pins.retain(|p| {
                next.credentials
                    .iter()
                    .any(|b| b.enrollment_id == e.enrollment_id && b.generation == p.generation)
            });
        }
        next.latest = now;
        self.commit_compacted(&next)
    }
    fn commit_compacted(&mut self, next: &Registry) -> Result<()> {
        let current = self.snapshot()?;
        if current.revision != next.revision {
            return Err(Error::StaleSnapshot);
        }
        next.validate()?;
        let mut next = next.clone();
        next.revision += 1;
        self.write(&next, |_| Ok(()))
    }
    pub(crate) fn authorize_cutover(
        receiver: &Path,
        kind: Kind,
        secret: &Secret,
        pin: Option<&Pin>,
        now: u64,
    ) -> Result<Registry> {
        let current = Registry::read_current(receiver)?;
        let now = now.max(current.latest);
        let Decision::Allowed {
            enrollment_id,
            generation,
            ..
        } = current.authorize(kind, kind, secret, pin, now).decision
        else {
            return Err(Error::Invalid);
        };
        let needs_write = current
            .enrollments
            .iter()
            .find(|e| e.enrollment_id == enrollment_id)
            .and_then(|e| e.credential_kinds.iter().find(|s| s.kind == kind))
            .is_some_and(|s| {
                s.generations.current == generation && s.generations.retiring.is_some()
            });
        if !needs_write {
            return Ok(current);
        }
        let mut store = Store::open(receiver)?;
        let mut next = store.snapshot()?;
        let Decision::Allowed {
            enrollment_id,
            generation,
            ..
        } = next
            .authorize(kind, kind, secret, pin, now.max(next.latest))
            .decision
        else {
            return Err(Error::Invalid);
        };
        let scope = next
            .enrollments
            .iter_mut()
            .find(|e| e.enrollment_id == enrollment_id)
            .and_then(|e| e.credential_kinds.iter_mut().find(|s| s.kind == kind))
            .ok_or(Error::Invalid)?;
        if scope.generations.current == generation && scope.generations.retiring.take().is_some() {
            next.latest = now.max(next.latest);
            store.commit(next.revision(), &next)?;
            return store.snapshot();
        }
        Ok(next)
    }
}
