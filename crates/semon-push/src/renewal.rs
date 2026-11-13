//! Receiver renewal state. Pending nonces are process-local; rate state and
//! committed delivery reservations share the credential registry transaction.
use crate::{
    credentials::{Bearer, Decision, Retiring, Secret},
    enrollment::{CertificatePin, Error, Registry, Store},
    provision_wire::{self, Refused},
    renewal_wire::{self as wire, Action, Begun, Credential, Generation},
    serve::VerifiedClientIdentity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Refused>;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Admission {
    pub enrollment_id: String,
    pub creations: Vec<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub begun: Begun,
    pub authentication: Vec<Generation>,
    pub origin_key_hash: [u8; 32],
    pub scope_digest: [u8; 32],
    pub application_hash: [u8; 32],
    pub created_at: u64,
    pub committed_at: u64,
    pub recovery_until: u64,
    pub credentials: Vec<Bearer>,
    pub client_certificate: Option<Vec<u8>>,
    pub deliveries: u8,
}
#[derive(Clone)]
struct Pending {
    begun: Begun,
    authentication: Vec<Generation>,
    created_at: u64,
}
#[derive(Default)]
pub struct Receiver {
    pending: BTreeMap<String, Pending>,
    latest: u64,
    terminal: BTreeMap<String, u64>,
    #[cfg(test)]
    fail_delivery: u8,
    #[cfg(test)]
    fail_registry: Option<crate::enrollment::Stage>,
}

fn enrollment<'a>(registry: &'a Registry, id: &str) -> Result<&'a crate::enrollment::Enrollment> {
    registry
        .enrollments
        .iter()
        .find(|e| {
            e.enrollment_id == id
                && e.revoked_at.is_none()
                && registry.endpoint() == Some(e.endpoint.as_str())
        })
        .ok_or(Refused)
}
fn authenticate(
    registry: &Registry,
    id: &str,
    supplied: &[Credential],
    expected: &[Generation],
    output: &[Generation],
    identity: Option<&VerifiedClientIdentity>,
    now: u64,
) -> Result<()> {
    if wire::identities(supplied)? != expected {
        return Err(Refused);
    }
    let e = enrollment(registry, id)?;
    if output.iter().any(|g| {
        e.credential_kinds
            .iter()
            .any(|s| s.kind == g.kind && s.client_cert_required)
    }) && identity.is_none()
    {
        return Err(Refused);
    }
    for credential in supplied {
        let secret = Secret::new(credential.bearer.clone());
        let authorized = |pin| matches!(registry.authorize(credential.kind, credential.kind, &secret, pin, now).decision, Decision::Allowed { enrollment_id, generation, .. } if enrollment_id == id && generation == credential.generation);
        let valid = match identity {
            Some(identity) => {
                authorized(Some(identity.spki())) || authorized(Some(identity.leaf()))
            }
            None => authorized(None),
        };
        if !valid {
            return Err(Refused);
        }
    }
    Ok(())
}
impl Receipt {
    pub(crate) fn live(&self, registry: &Registry, now: u64) -> bool {
        let now = now.max(registry.latest);
        let Ok(e) = enrollment(registry, &self.begun.enrollment_id) else {
            return false;
        };
        now < self.recovery_until
            && self.begun.generations.iter().all(|g| {
                e.credential_kinds.iter().any(|s| {
                    s.kind == g.kind && s.available && s.generations.current == g.generation
                }) && registry.credentials.iter().any(|b| {
                    b.enrollment_id == e.enrollment_id
                        && b.kind == g.kind
                        && b.generation == g.generation
                        && b.revoked_at.is_none()
                })
            })
            && self.authentication.iter().all(|g| {
                e.credential_kinds.iter().any(|s| {
                    s.kind == g.kind
                        && s.available
                        && s.generations
                            .retiring
                            .as_ref()
                            .is_some_and(|r| r.generation == g.generation && now < r.cutover_at)
                }) && registry.credentials.iter().any(|b| {
                    b.enrollment_id == e.enrollment_id
                        && b.kind == g.kind
                        && b.generation == g.generation
                        && b.revoked_at.is_none()
                        && b.valid_from <= now
                        && now < b.valid_until
                })
            })
    }
}
impl Receiver {
    pub fn handle(
        &mut self,
        dir: &Path,
        request: wire::Request,
        exporter: [u8; 32],
        identity: Option<&VerifiedClientIdentity>,
        now: u64,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let (id, renewal_id) = match &request {
            wire::Request::Begin(m) => (m.enrollment_id.clone(), String::new()),
            wire::Request::Finish(m) => (m.enrollment_id.clone(), m.renewal_id.clone()),
        };
        let result = match request {
            wire::Request::Begin(message) => self
                .begin(dir, &message, identity, now)
                .and_then(|m| provision_wire::encode_message(&m)),
            wire::Request::Finish(message) => self.finish(dir, &message, &exporter, identity, now),
        };
        if result.is_err() {
            let _ = crate::generation_audit::renewal(
                dir,
                &crate::generation_audit::RenewalRecord {
                    version: 1,
                    observed_at: now.max(self.latest),
                    enrollment_id: id,
                    renewal_id,
                    generations: vec![],
                    action: "refused".into(),
                },
            );
        }
        result.map(Zeroizing::new)
    }
    fn expire(&mut self, dir: &Path, now: u64) -> Result<()> {
        let expired: Vec<_> = self
            .pending
            .values()
            .filter(|p| now >= p.begun.expires_at)
            .map(|p| p.begun.clone())
            .collect();
        for begun in expired {
            audit(dir, &begun, "expired", now)?;
            self.pending.remove(&begun.enrollment_id);
        }
        self.terminal.retain(|_, expires| now < *expires);
        Ok(())
    }
    fn begin(
        &mut self,
        dir: &Path,
        message: &wire::Begin,
        identity: Option<&VerifiedClientIdentity>,
        now: u64,
    ) -> Result<Begun> {
        let mut store = Store::open(dir).map_err(|_| Refused)?;
        store.compact(dir, now).map_err(|_| Refused)?;
        let mut registry = store.snapshot().map_err(|_| Refused)?;
        let now = now.max(registry.latest).max(self.latest);
        self.latest = now;
        if now > registry.latest {
            registry.latest = now;
            store
                .commit(registry.revision(), &registry)
                .map_err(|_| Refused)?;
            registry = store.snapshot().map_err(|_| Refused)?;
        }
        self.expire(dir, now)?;
        self.pending.retain(|id, pending| {
            registry.enrollments.iter().any(|e| {
                e.enrollment_id == *id
                    && e.revoked_at.is_none()
                    && registry.endpoint() == Some(e.endpoint.as_str())
                    && pending.authentication.iter().all(|g| {
                        e.credential_kinds.iter().any(|s| {
                            s.kind == g.kind && s.available && s.generations.current == g.generation
                        })
                    })
                    && pending
                        .begun
                        .enable_authorization_ids
                        .iter()
                        .all(|authorization_id| {
                            e.credential_kinds.iter().any(|s| {
                                s.enable_authorization.as_ref().is_some_and(|a| {
                                    a.authorization_id == *authorization_id
                                        && a.consumed_renewal_id.is_none()
                                })
                            })
                        })
            })
        });
        let authentication = wire::identities(&message.credentials)?;
        if let Some(receipt) = registry.renewal_receipts.iter().find(|r| {
            r.begun.enrollment_id == message.enrollment_id
                && r.authentication == authentication
                && r.live(&registry, now)
        }) {
            authenticate(
                &registry,
                &message.enrollment_id,
                &message.credentials,
                &receipt.authentication,
                &receipt.begun.generations,
                identity,
                now,
            )?;
            return Ok(receipt.begun.clone());
        }
        let e = enrollment(&registry, &message.enrollment_id)?;
        let mut expected: Vec<_> = e
            .credential_kinds
            .iter()
            .filter(|s| s.available)
            .map(|s| Generation {
                kind: s.kind,
                generation: s.generations.current,
            })
            .collect();
        expected.sort_by_key(|g| g.kind.wire_name());
        // Validate all bearer and presented-certificate authority before nonce allocation.
        authenticate(
            &registry,
            &message.enrollment_id,
            &message.credentials,
            &expected,
            &expected,
            identity,
            now,
        )?;
        if let Some(pending) = self.pending.get(&message.enrollment_id) {
            if pending.authentication != expected {
                return Err(Refused);
            }
            authenticate(
                &registry,
                &message.enrollment_id,
                &message.credentials,
                &pending.authentication,
                &pending.begun.generations,
                identity,
                now,
            )?;
            return Ok(pending.begun.clone());
        }
        if registry
            .renewal_receipts
            .iter()
            .any(|r| r.begun.enrollment_id == message.enrollment_id && r.live(&registry, now))
        {
            return Err(Refused);
        }
        let mut authorizations = Vec::new();
        let mut targets = Vec::new();
        for scope in &e.credential_kinds {
            if scope.available
                || scope
                    .enable_authorization
                    .as_ref()
                    .is_some_and(|a| a.consumed_renewal_id.is_none())
            {
                targets.push(Generation {
                    kind: scope.kind,
                    generation: scope.generations.current.checked_add(1).ok_or(Refused)?,
                });
                if !scope.available {
                    authorizations.push(
                        scope
                            .enable_authorization
                            .as_ref()
                            .ok_or(Refused)?
                            .authorization_id
                            .clone(),
                    );
                }
            }
            // Never overwrite a still-live retiring slot.
            if scope.generations.retiring.as_ref().is_some_and(|r| {
                now < r.cutover_at
                    && registry.credentials.iter().any(|b| {
                        b.enrollment_id == e.enrollment_id
                            && b.kind == scope.kind
                            && b.generation == r.generation
                            && now < b.valid_until
                            && b.revoked_at.is_none()
                    })
            }) {
                return Err(Refused);
            }
        }
        targets.sort_by_key(|g| g.kind.wire_name());
        authorizations.sort();
        authenticate(
            &registry,
            &message.enrollment_id,
            &message.credentials,
            &expected,
            &targets,
            identity,
            now,
        )?;
        if authorizations.is_empty()
            && !registry.credentials.iter().any(|b| {
                b.enrollment_id == message.enrollment_id
                    && expected
                        .iter()
                        .any(|g| g.kind == b.kind && g.generation == b.generation)
                    && b.valid_until.saturating_sub(now) < wire::WINDOW
            })
        {
            return Err(Refused);
        }
        if self.pending.len() >= 4 {
            return Err(Refused);
        }
        registry.renewal_admission.retain(|a| {
            registry
                .enrollments
                .iter()
                .any(|e| e.enrollment_id == a.enrollment_id)
        });
        for a in &mut registry.renewal_admission {
            a.creations.retain(|t| now.saturating_sub(*t) < 3600);
        }
        let index = if let Some(index) = registry
            .renewal_admission
            .iter()
            .position(|a| a.enrollment_id == message.enrollment_id)
        {
            index
        } else {
            registry.renewal_admission.push(Admission {
                enrollment_id: message.enrollment_id.clone(),
                creations: vec![],
            });
            registry.renewal_admission.len() - 1
        };
        if registry.renewal_admission[index].creations.len() >= 6 {
            return Err(Refused);
        }
        let begun = Begun {
            version: 1,
            enrollment_id: message.enrollment_id.clone(),
            renewal_id: crate::enrollment::random_id().map_err(|_| Refused)?,
            receiver_nonce: provision_wire::encode(&random_nonce()?),
            expires_at: now.checked_add(wire::LIFETIME).ok_or(Refused)?,
            generations: targets,
            enable_authorization_ids: authorizations,
        };
        registry.renewal_admission[index].creations.push(now);
        registry.latest = now;
        // Pending outcomes are charged even though restart destroys the nonce state.
        registry
            .check_renewal_admission(self.pending.len() + 1)
            .map_err(|_| Refused)?;
        audit(dir, &begun, "begin", now)?;
        store
            .commit(registry.revision(), &registry)
            .map_err(|_| Refused)?;
        self.pending.insert(
            message.enrollment_id.clone(),
            Pending {
                begun: begun.clone(),
                authentication: expected,
                created_at: now,
            },
        );
        Ok(begun)
    }
    fn finish(
        &mut self,
        dir: &Path,
        message: &wire::Finish,
        exporter: &[u8; 32],
        identity: Option<&VerifiedClientIdentity>,
        now: u64,
    ) -> Result<Vec<u8>> {
        let mut store = Store::open(dir).map_err(|_| Refused)?;
        let mut registry = store.snapshot().map_err(|_| Refused)?;
        let now = now.max(registry.latest).max(self.latest);
        self.latest = now;
        if now > registry.latest {
            registry.latest = now;
            store
                .commit(registry.revision(), &registry)
                .map_err(|_| Refused)?;
            registry = store.snapshot().map_err(|_| Refused)?;
        }
        self.expire(dir, now)?;
        let application = Zeroizing::new(provision_wire::decode_fixed::<32>(
            &message.application_key,
        )?);
        let receipt = registry
            .renewal_receipts
            .iter()
            .find(|r| {
                r.begun.renewal_id == message.renewal_id
                    && r.begun.enrollment_id == message.enrollment_id
            })
            .cloned();
        if let Some(receipt) = receipt {
            if message.action != Action::Recover
                || !receipt.live(&registry, now)
                || receipt.deliveries != 1
                || receipt.application_hash
                    != <[u8; 32]>::from(Sha256::digest(application.as_ref()))
            {
                return Err(Refused);
            }
            authenticate(
                &registry,
                &message.enrollment_id,
                &message.credentials,
                &receipt.authentication,
                &receipt.begun.generations,
                identity,
                now,
            )?;
            verify(
                &registry,
                message,
                &receipt.begun,
                &receipt.authentication,
                exporter,
                &application,
            )?;
            let bytes = delivery(&receipt, &application)?;
            audit(dir, &receipt.begun, "recover_delivery_reserved", now)?;
            registry
                .renewal_receipts
                .iter_mut()
                .find(|r| r.begun.renewal_id == message.renewal_id)
                .ok_or(Refused)?
                .deliveries = 2;
            registry.latest = now;
            self.commit_delivery(&mut store, &registry)?;
            #[cfg(test)]
            if self.fail_delivery == 3 {
                self.fail_delivery = 0;
                return Err(Refused);
            }
            return Ok(bytes);
        }
        if self.terminal.contains_key(&message.renewal_id) {
            return Err(Refused);
        }
        let pending = self
            .pending
            .get(&message.enrollment_id)
            .filter(|p| p.begun.renewal_id == message.renewal_id)
            .cloned()
            .ok_or(Refused)?;
        if message.action == Action::Recover {
            return Err(Refused);
        }
        authenticate(
            &registry,
            &message.enrollment_id,
            &message.credentials,
            &pending.authentication,
            &pending.begun.generations,
            identity,
            now,
        )?;
        verify(
            &registry,
            message,
            &pending.begun,
            &pending.authentication,
            exporter,
            &application,
        )?;
        let e = enrollment(&registry, &message.enrollment_id)?.clone();
        for id in &pending.begun.enable_authorization_ids {
            if !e.credential_kinds.iter().any(|s| {
                !s.available
                    && s.enable_authorization.as_ref().is_some_and(|a| {
                        a.authorization_id == *id && a.consumed_renewal_id.is_none()
                    })
            }) {
                return Err(Refused);
            }
        }
        for g in &pending.authentication {
            if !e
                .credential_kinds
                .iter()
                .any(|s| s.kind == g.kind && s.available && s.generations.current == g.generation)
            {
                return Err(Refused);
            }
        }
        if message.action == Action::Cancel {
            // Cancellation removes pending authority even if audit custody is
            // unavailable; it uses the same terminal rule as owner revocation.
            let _ = audit(dir, &pending.begun, "cancel", now);
            registry.latest = now;
            store
                .commit(registry.revision(), &registry)
                .map_err(|_| Refused)?;
            self.pending.remove(&message.enrollment_id);
            self.terminal
                .insert(message.renewal_id.clone(), pending.begun.expires_at);
            return provision_wire::encode_message(&wire::Cancelled {
                version: 1,
                enrollment_id: message.enrollment_id.clone(),
                renewal_id: message.renewal_id.clone(),
                cancelled: true,
            });
        }
        let until = now
            .checked_add(provision_wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS)
            .ok_or(Refused)?;
        let certificate = if pending.begun.generations.iter().any(|g| {
            e.credential_kinds
                .iter()
                .any(|s| s.kind == g.kind && s.client_cert_required)
        }) {
            let pin = crate::client_ca::pin(&e.origin_key)
                .map_err(|_| Refused)?
                .pin;
            if e.enrolled_pin.as_ref() != Some(&pin) {
                return Err(Refused);
            }
            Some(
                crate::receiver_custody::ReceiverCustody::open_existing(dir)
                    .map_err(|_| Refused)?
                    .issue_client_certificate(&e.origin_key, &e.enrollment_id, now, until)
                    .map_err(|_| Refused)?,
            )
        } else {
            None
        };
        let mut recovery_until = u64::MAX;
        let mut issued = Vec::new();
        for target in &pending.begun.generations {
            let scope = registry
                .enrollments
                .iter_mut()
                .find(|e| e.enrollment_id == message.enrollment_id)
                .and_then(|e| {
                    e.credential_kinds
                        .iter_mut()
                        .find(|s| s.kind == target.kind)
                })
                .ok_or(Refused)?;
            if target.generation != scope.generations.current.checked_add(1).ok_or(Refused)? {
                return Err(Refused);
            }
            if scope.available {
                let old = registry
                    .credentials
                    .iter()
                    .find(|b| {
                        b.enrollment_id == message.enrollment_id
                            && b.kind == target.kind
                            && b.generation == scope.generations.current
                    })
                    .ok_or(Refused)?;
                let cutover_at = now
                    .checked_add(wire::OVERLAP)
                    .ok_or(Refused)?
                    .min(old.valid_until);
                recovery_until = recovery_until.min(cutover_at);
                scope.generations.retiring = Some(Retiring {
                    generation: old.generation,
                    cutover_at,
                });
            } else {
                let authorization = scope
                    .enable_authorization
                    .as_mut()
                    .filter(|a| {
                        pending
                            .begun
                            .enable_authorization_ids
                            .contains(&a.authorization_id)
                            && a.consumed_renewal_id.is_none()
                    })
                    .ok_or(Refused)?;
                authorization.consumed_renewal_id = Some(message.renewal_id.clone());
                scope.available = true;
                scope.generations.retiring = None;
            }
            scope.generations.current = target.generation;
            issued.push(Bearer {
                credential_id: crate::enrollment::random_id().map_err(|_| Refused)?,
                enrollment_id: message.enrollment_id.clone(),
                kind: target.kind,
                sha256: wire::bearer(
                    &application,
                    &message.enrollment_id,
                    &message.renewal_id,
                    target,
                )
                .hash(),
                generation: target.generation,
                valid_from: now,
                valid_until: until,
                revoked_at: None,
            });
        }
        if certificate.is_some() {
            let enrollment = registry
                .enrollments
                .iter_mut()
                .find(|e| e.enrollment_id == message.enrollment_id)
                .ok_or(Refused)?;
            for target in &pending.begun.generations {
                let pin = enrollment.enrolled_pin.clone().ok_or(Refused)?;
                if !enrollment
                    .client_certificate_pins
                    .iter()
                    .any(|p| p.generation == target.generation && p.pin == pin)
                {
                    enrollment.client_certificate_pins.push(CertificatePin {
                        pin,
                        generation: target.generation,
                        revoked_at: None,
                    });
                }
            }
        }
        let receipt = Receipt {
            begun: pending.begun.clone(),
            authentication: pending.authentication.clone(),
            origin_key_hash: Sha256::digest(&e.origin_key).into(),
            scope_digest: wire::scope_digest(&pending.authentication, &pending.begun.generations),
            application_hash: Sha256::digest(application.as_ref()).into(),
            created_at: pending.created_at,
            committed_at: now,
            recovery_until,
            credentials: issued.clone(),
            client_certificate: certificate,
            deliveries: 1,
        };
        let bytes = delivery(&receipt, &application)?;
        registry.credentials.extend(issued);
        registry
            .pairing_receipts
            .retain(|r| r.enrollment_id != message.enrollment_id);
        registry.renewal_receipts.push(receipt);
        registry.latest = now;
        registry
            .check_renewal_admission(self.pending.len().saturating_sub(1))
            .map_err(|_| Refused)?;
        audit(dir, &pending.begun, "commit_delivery_reserved", now)?;
        #[cfg(test)]
        if self.fail_delivery == 1 {
            self.fail_delivery = 0;
            return Err(Refused);
        }
        self.commit_delivery(&mut store, &registry)?;
        self.pending.remove(&message.enrollment_id);
        #[cfg(test)]
        if self.fail_delivery == 2 {
            self.fail_delivery = 0;
            return Err(Refused);
        }
        Ok(bytes)
    }
    fn commit_delivery(&mut self, store: &mut Store, registry: &Registry) -> Result<()> {
        #[cfg(test)]
        if let Some(failure) = self.fail_registry.take() {
            return store
                .commit_with_fault(registry.revision(), registry, |stage| {
                    if stage == failure {
                        Err(Error::Write)
                    } else {
                        Ok(())
                    }
                })
                .map_err(|_| Refused);
        }
        store
            .commit(registry.revision(), registry)
            .map_err(|_| Refused)
    }
}
fn random_nonce() -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| Refused)?;
    Ok(bytes)
}
fn verify(
    registry: &Registry,
    message: &wire::Finish,
    begun: &Begun,
    authentication: &[Generation],
    exporter: &[u8; 32],
    application: &[u8; 32],
) -> Result<()> {
    let e = enrollment(registry, &message.enrollment_id)?;
    let signed = wire::possession_message(
        &e.receiver_id,
        &e.endpoint,
        begun,
        authentication,
        exporter,
        message.action,
        application,
    )?;
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::ECDSA_P256_SHA256_FIXED,
        &e.origin_key,
    )
    .verify(
        &signed,
        &provision_wire::decode_fixed::<64>(&message.proof)?,
    )
    .map_err(|_| Refused)
}
fn delivery(receipt: &Receipt, application: &[u8; 32]) -> Result<Vec<u8>> {
    provision_wire::encode_message(&wire::Delivered {
        version: 1,
        enrollment_id: receipt.begun.enrollment_id.clone(),
        renewal_id: receipt.begun.renewal_id.clone(),
        credentials: receipt
            .credentials
            .iter()
            .map(|b| wire::DeliveredCredential {
                kind: b.kind,
                credential_id: b.credential_id.clone(),
                generation: b.generation,
                bearer: wire::bearer(
                    application,
                    &b.enrollment_id,
                    &receipt.begun.renewal_id,
                    &Generation {
                        kind: b.kind,
                        generation: b.generation,
                    },
                )
                .expose()
                .to_owned(),
                valid_from: b.valid_from,
                valid_until: b.valid_until,
            })
            .collect(),
        client_certificate: receipt
            .client_certificate
            .as_deref()
            .map(provision_wire::encode),
    })
}
fn audit(dir: &Path, begun: &Begun, action: &str, now: u64) -> Result<()> {
    crate::generation_audit::renewal(
        dir,
        &crate::generation_audit::RenewalRecord {
            version: 1,
            observed_at: now,
            enrollment_id: begun.enrollment_id.clone(),
            renewal_id: begun.renewal_id.clone(),
            generations: begun.generations.clone(),
            action: action.into(),
        },
    )
    .map_err(|_| Refused)
}
#[cfg(test)]
#[path = "renewal_tests.rs"]
mod tests;

impl Registry {
    pub(crate) fn check_renewal_admission(&self, pending: usize) -> std::result::Result<(), Error> {
        self.check_admission()?;
        if serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() as u64
            + pending as u64 * 4096
            > crate::enrollment::ADMISSION_CAP
        {
            return Err(Error::Full);
        }
        Ok(())
    }
    pub(crate) fn validate_renewal(&self) -> std::result::Result<(), Error> {
        let mut ids = std::collections::BTreeSet::new();
        for a in &self.renewal_admission {
            if !ids.insert(&a.enrollment_id)
                || !self
                    .enrollments
                    .iter()
                    .any(|e| e.enrollment_id == a.enrollment_id)
                || a.creations.len() > 6
                || a.creations.windows(2).any(|w| w[0] > w[1])
                || a.creations.iter().any(|t| *t > self.latest)
                || serde_json::to_vec(a).map_err(|_| Error::Invalid)?.len() > 1024
            {
                return Err(Error::Invalid);
            }
        }
        ids.clear();
        for r in &self.renewal_receipts {
            let e = self
                .enrollments
                .iter()
                .find(|e| e.enrollment_id == r.begun.enrollment_id)
                .ok_or(Error::Invalid)?;
            if !ids.insert(&r.begun.enrollment_id)
                || wire::identifier(&r.begun.renewal_id).is_err()
                || provision_wire::decode_fixed::<32>(&r.begun.receiver_nonce).is_err()
                || r.begun.version != 1
                || r.deliveries == 0
                || r.deliveries > 2
                || r.created_at > r.committed_at
                || r.begun.expires_at
                    != r.created_at
                        .checked_add(wire::LIFETIME)
                        .ok_or(Error::Invalid)?
                || r.committed_at >= r.begun.expires_at
                || r.committed_at >= r.recovery_until
                || r.scope_digest != wire::scope_digest(&r.authentication, &r.begun.generations)
                || r.origin_key_hash != <[u8; 32]>::from(Sha256::digest(&e.origin_key))
                || r.authentication.is_empty()
                || r.authentication.len() > 3
                || r.begun.generations.is_empty()
                || r.begun.generations.len() > 3
                || r.credentials.len() != r.begun.generations.len()
                || serde_json::to_vec(r).map_err(|_| Error::Invalid)?.len() > 8192
            {
                return Err(Error::Invalid);
            }
            for list in [&r.authentication, &r.begun.generations] {
                if list.iter().any(|g| g.generation == 0)
                    || list
                        .windows(2)
                        .any(|w| w[0].kind.wire_name() >= w[1].kind.wire_name())
                {
                    return Err(Error::Invalid);
                }
            }
            if r.begun.enable_authorization_ids.len() > 3
                || r.begun
                    .enable_authorization_ids
                    .iter()
                    .any(|id| wire::identifier(id).is_err())
                || r.begun
                    .enable_authorization_ids
                    .windows(2)
                    .any(|w| w[0] >= w[1])
            {
                return Err(Error::Invalid);
            }
            if r.authentication.iter().any(|old| {
                !r.begun.generations.iter().any(|new| {
                    new.kind == old.kind && old.generation.checked_add(1) == Some(new.generation)
                })
            }) {
                return Err(Error::Invalid);
            }
            let certificate_required = r.begun.generations.iter().any(|g| {
                e.credential_kinds
                    .iter()
                    .any(|s| s.kind == g.kind && s.client_cert_required)
            });
            if certificate_required != r.client_certificate.is_some()
                || r.client_certificate
                    .as_ref()
                    .is_some_and(|c| c.len() > 1536)
            {
                return Err(Error::Invalid);
            }
            let mut receipt_ids = std::collections::BTreeSet::new();
            let mut receipt_kinds = std::collections::BTreeSet::new();
            for b in &r.credentials {
                if wire::identifier(&b.credential_id).is_err()
                    || !receipt_ids.insert(&b.credential_id)
                    || !receipt_kinds.insert(b.kind)
                    || !e
                        .credential_kinds
                        .iter()
                        .any(|s| s.kind == b.kind && b.generation <= s.generations.current)
                {
                    return Err(Error::Invalid);
                }
                if let Some(current) = self
                    .credentials
                    .iter()
                    .find(|current| current.credential_id == b.credential_id)
                {
                    let mut expected = b.clone();
                    expected.revoked_at = current.revoked_at;
                    if &expected != current {
                        return Err(Error::Invalid);
                    }
                }
            }
            for g in &r.begun.generations {
                if !r.credentials.iter().any(|b| {
                    b.enrollment_id == e.enrollment_id
                        && b.kind == g.kind
                        && b.generation == g.generation
                        && b.valid_from == r.committed_at
                        && b.valid_until
                            == r.committed_at
                                .saturating_add(provision_wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS)
                        && b.revoked_at.is_none()
                }) {
                    return Err(Error::Invalid);
                }
            }
            if let Some(certificate) = &r.client_certificate {
                crate::client_ca::validate_receipt(
                    certificate,
                    &r.origin_key_hash,
                    r.committed_at
                        .saturating_add(provision_wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS),
                    r.committed_at,
                )
                .map_err(|_| Error::Invalid)?;
            }
        }
        Ok(())
    }
}
