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
            self.credentials.len() as u64 * 20 + self.enrollments.len() as u64 * (20 + 512);
        // Four process-local pending attempts can each commit a receipt. Reserve
        // their maximum encoded outcomes for every authority-producing writer,
        // including pairing and owner enables in other processes.
        let pending_outcomes = 4 * 8192;
        if size.saturating_add(pending_outcomes) > ADMISSION_CAP
            || size.saturating_add(terminal_delta) > CAP
        {
            return Err(Error::Full);
        }
        Ok(())
    }
    pub fn usable_machine(&self, machine: &str, now: u64) -> Option<&str> {
        self.enrollments
            .iter()
            .filter(|e| e.machine == machine && self.usable(e, now))
            .max_by_key(|e| e.sequence)
            .map(|e| e.enrollment_id.as_str())
    }
    pub(super) fn usable(&self, e: &Enrollment, now: u64) -> bool {
        let now = now.max(self.latest);
        if self.endpoint.as_deref() != Some(e.endpoint.as_str()) {
            return false;
        }
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
                        && (!s.client_cert_required
                            || e.client_certificate_pins
                                .iter()
                                .any(|p| p.generation == b.generation && p.revoked_at.is_none()))
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
            .filter(|e| e.machine == machine && self.usable(e, now))
            .max_by_key(|e| e.sequence);
        let target = live.or_else(|| {
            self.enrollments
                .iter()
                .filter(|e| e.machine == machine)
                .max_by_key(|e| e.sequence)
        });
        let tombstone = self
            .tombstones
            .iter()
            .filter(|t| t.machine == machine)
            .max_by_key(|t| t.sequence);
        let target =
            target.filter(|e| live.is_some() || tombstone.is_none_or(|t| e.sequence > t.sequence));
        let Some(e) = target else {
            return if let Some(t) = tombstone {
                if kind.is_some_and(|k| t.high_water[k.index()] == 0) {
                    return OwnerState::UnknownKind;
                }
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
                if e.revoked_at.is_some()
                    || scope.is_some_and(|s| !s.available && s.enable_authorization.is_none())
                {
                    return OwnerState::Complete {
                        endpoint: e.endpoint.clone(),
                    };
                }
                if scope.is_some_and(|s| s.available)
                    && e.credential_kinds.iter().filter(|s| s.available).count() == 1
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
    pub fn change_endpoint(
        &mut self,
        receiver: &Path,
        expected: &str,
        endpoint: &str,
        owner: &str,
        now: u64,
    ) -> Result<()> {
        let endpoint = crate::pairing::normalize_endpoint(endpoint)?;
        let mut next = self.snapshot()?;
        if next.endpoint.as_deref() != Some(expected) {
            return Err(Error::StaleSnapshot);
        }
        if expected == endpoint {
            return Ok(());
        }
        let now = now.max(next.latest);
        let mut audit = generation_audit::OwnerRecord {
            version: 1,
            observed_at: now,
            owner: owner.into(),
            enrollment_id: String::new(),
            kind: None,
            action: "change_address".into(),
            outcome: generation_audit::OwnerOutcome::Requested,
        };
        let _ = generation_audit::owner(receiver, &audit);
        next.endpoint = Some(endpoint);
        next.latest = now;
        // Bound address changes to the reserved terminal headroom as well.
        if serde_json::to_vec(&next).map_err(|_| Error::Invalid)?.len() as u64 > CAP {
            return Err(Error::Full);
        }
        self.commit(next.revision(), &next)?;
        audit.outcome = generation_audit::OwnerOutcome::Committed;
        let _ = generation_audit::owner(receiver, &audit);
        Ok(())
    }
    pub fn owner_update(
        &mut self,
        receiver: &Path,
        kind: Option<Kind>,
        action: OwnerAction,
        owner: &str,
        confirmed_id: &str,
        now: u64,
    ) -> Result<OwnerState> {
        let mut next = self.snapshot()?;
        let now = now.max(next.latest);
        let machine = next
            .enrollments
            .iter()
            .find(|e| e.enrollment_id == confirmed_id)
            .ok_or(Error::StaleSnapshot)?
            .machine
            .clone();
        let state = next.owner_state(&machine, kind, action, now);
        let OwnerState::Confirm { enrollment_id, .. } = &state else {
            return Ok(state);
        };
        // A confirmation for one enrollment never authorizes its replacement.
        if enrollment_id != confirmed_id {
            return Err(Error::StaleSnapshot);
        }
        let mut audit = generation_audit::OwnerRecord {
            version: 1,
            observed_at: now,
            owner: owner.into(),
            enrollment_id: confirmed_id.into(),
            kind,
            action: match action {
                OwnerAction::Revoke => "revoke",
                OwnerAction::Enable => "enable",
            }
            .into(),
            outcome: generation_audit::OwnerOutcome::Requested,
        };
        let audited = generation_audit::owner(receiver, &audit);
        if action == OwnerAction::Enable {
            audited?;
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
        audit.outcome = generation_audit::OwnerOutcome::Committed;
        let committed_audit = generation_audit::owner(receiver, &audit);
        if action == OwnerAction::Enable {
            committed_audit?;
        }
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
        self.compact_with_fault(receiver, now, |_| Ok(()))
    }
    pub(super) fn compact_with_fault(
        &mut self,
        receiver: &Path,
        now: u64,
        mut fault: impl FnMut(Stage) -> Result<()>,
    ) -> Result<()> {
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
                    .enrolled_pin
                    .clone();
                Some(EndedGeneration::new(b, pin, reason, now))
            })
            .collect();
        generation_audit::append(receiver, &records)?;
        fault(Stage::Synced)?;
        let has_terminal = next.enrollments.iter().any(|e| e.revoked_at.is_some());
        let receipt_count = next.pairing_receipts.len();
        let renewal_count = next.renewal_receipts.len();
        let live_renewals: Vec<_> = next
            .renewal_receipts
            .iter()
            .filter(|r| r.live(&next, now))
            .cloned()
            .collect();
        next.renewal_receipts = live_renewals;
        next.pairing_receipts.retain(|r| {
            r.valid_until > now
                && r.deliveries < 2
                && next.enrollments.iter().any(|e| {
                    e.enrollment_id == r.enrollment_id
                        && next.endpoint.as_deref() == Some(e.endpoint.as_str())
                        && e.revoked_at.is_none()
                        && e.credential_kinds
                            .iter()
                            .all(|k| k.available && k.generations.current == 1)
                })
        });
        if records.is_empty()
            && !has_terminal
            && receipt_count == next.pairing_receipts.len()
            && renewal_count == next.renewal_receipts.len()
        {
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
        let terminal: Vec<_> = next
            .enrollments
            .iter()
            .filter(|e| {
                (e.revoked_at.is_some() || next.endpoint.as_deref() != Some(e.endpoint.as_str()))
                    && !next
                        .credentials
                        .iter()
                        .any(|b| b.enrollment_id == e.enrollment_id)
            })
            .cloned()
            .collect();
        for e in terminal {
            let mut high_water = [0; 3];
            for scope in &e.credential_kinds {
                high_water[scope.kind.index()] = scope.generations.current;
            }
            let pin = e.enrolled_pin.clone();
            next.tombstones.push(Tombstone {
                enrollment_id: e.enrollment_id.clone(),
                machine: e.machine,
                sequence: e.sequence,
                pin,
                high_water,
            });
            next.enrollments
                .retain(|item| item.enrollment_id != e.enrollment_id);
            next.pairing_receipts
                .retain(|r| r.enrollment_id != e.enrollment_id);
        }
        next.renewal_admission.retain(|a| {
            next.enrollments
                .iter()
                .any(|e| e.enrollment_id == a.enrollment_id)
        });
        next.latest = now;
        self.commit_compacted(&next, fault)
    }
    fn commit_compacted(
        &mut self,
        next: &Registry,
        fault: impl FnMut(Stage) -> Result<()>,
    ) -> Result<()> {
        let current = self.snapshot()?;
        if current.revision != next.revision {
            return Err(Error::StaleSnapshot);
        }
        next.validate()?;
        for t in &current.tombstones {
            if !next.tombstones.contains(t) {
                return Err(Error::Terminal);
            }
        }
        for b in &current.credentials {
            if !next.credentials.iter().any(|n| n == b) && current.ended(b, next.latest).is_none() {
                return Err(Error::Terminal);
            }
        }
        let mut next = next.clone();
        next.revision += 1;
        self.write(&next, fault)
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
