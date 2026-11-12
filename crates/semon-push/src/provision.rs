//! Receiver-owned public-TLS pairing controller. Local authorization is independent
//! of machine transport credentials. Core alone owns session/gate/guess state.
use crate::{
    credentials::Kind,
    enrollment::{Binding, Store},
    pairing::{self, Core, Transcript},
    provision_wire::{self as wire, Refused, Request},
    receiver_custody::{self, ReceiverCustody},
};
use std::{collections::BTreeMap, path::Path};
use zeroize::Zeroizing;

struct Pending {
    exporter: [u8; 32],
    nonce: [u8; 32],
    transcript: Option<Transcript>,
}
pub struct Controller {
    core: Core,
    custody: ReceiverCustody,
    endpoint: String,
    machine: String,
    pending: BTreeMap<String, Pending>,
    unavailable: Option<&'static str>,
}
impl Controller {
    /// The caller is the authenticated local OS owner and authorizes exactly one
    /// machine. Codes must be delivered only to its trusted terminal callback.
    pub fn open(dir: &Path, endpoint: &str, machine: String) -> Result<Self, String> {
        if !semon_sessions::is_machine_name(&machine) {
            return Err("invalid enrollment machine".into());
        }
        let endpoint = pairing::normalize_endpoint(endpoint).map_err(|e| e.to_string())?;
        let custody = ReceiverCustody::open_or_initialize(dir).map_err(|e| e.to_string())?;
        let core = Core::open(
            Store::open(dir).map_err(|e| e.to_string())?,
            &endpoint,
            custody.receiver_id().into(),
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            core,
            custody,
            endpoint,
            machine,
            pending: BTreeMap::new(),
            unavailable: None,
        })
    }
    pub fn pending(&mut self, now: u64) -> Vec<(String, pairing::State)> {
        self.pending
            .keys()
            .filter_map(|id| {
                self.core
                    .state(id, now)
                    .ok()
                    .map(|state| (id.clone(), state))
            })
            .collect()
    }
    pub fn cancel(&mut self, id: &str, now: u64) -> Result<(), Refused> {
        self.core.cancel(id, now).map_err(|_| Refused)
    }
    #[cfg(test)]
    pub(crate) fn reserve_probe(&mut self, id: &str, exporter: [u8; 32], now: u64) -> bool {
        let Some(pending) = self.pending.get(id) else {
            return false;
        };
        let malformed = Transcript {
            version: 0,
            session_id: id.into(),
            endpoint: self.endpoint.clone(),
            exporter: exporter.to_vec(),
            origin_key: vec![],
            scope: [0; 32],
            origin_nonce: [0; 32],
            receiver_nonce: pending.nonce,
            origin_role: "origin".into(),
            receiver_role: "receiver".into(),
            profile: "public_tls".into(),
        };
        let _ = self.core.reserve(&wire::encode(&exporter), malformed, now);
        self.core.state(id, now) == Ok(pairing::State::Rejected)
    }
    /// Malformed messages on a known pending connection spend an attempt. Foreign
    /// identifiers never charge another connection. Dropping a reserved ticket
    /// does not refund attempts, including timeout/disconnect paths.
    pub fn malformed(&mut self, exporter: [u8; 32], now: u64) {
        if let Some((id, pending)) = self.pending.iter().find(|(_, p)| p.exporter == exporter) {
            let malformed = Transcript {
                version: 0,
                session_id: id.clone(),
                endpoint: self.endpoint.clone(),
                exporter: exporter.to_vec(),
                origin_key: vec![],
                scope: [0; 32],
                origin_nonce: [0; 32],
                receiver_nonce: pending.nonce,
                origin_role: "origin".into(),
                receiver_role: "receiver".into(),
                profile: "public_tls".into(),
            };
            let _ = self.core.reserve(&wire::encode(&exporter), malformed, now);
        }
    }
    pub fn handle(
        &mut self,
        request: Request,
        exporter: [u8; 32],
        now: u64,
        show: impl FnOnce(&str, &crate::credentials::Secret) -> Result<(), Refused>,
    ) -> Result<Zeroizing<Vec<u8>>, Refused> {
        self.handle_with_clock(request, exporter, &|| now, show)
    }
    /// Diagnostic exists only for the last authenticated approval; transport
    /// consumes it while still holding the controller mutex.
    pub(crate) fn take_unavailable(&mut self) -> Option<&'static str> {
        self.unavailable.take()
    }
    pub fn handle_with_clock(
        &mut self,
        request: Request,
        exporter: [u8; 32],
        clock: &dyn Fn() -> u64,
        show: impl FnOnce(&str, &crate::credentials::Secret) -> Result<(), Refused>,
    ) -> Result<Zeroizing<Vec<u8>>, Refused> {
        self.unavailable = None;
        let now = clock();
        let bytes = match request {
            Request::Open(_) => {
                // Remove terminal transport metadata; Core retains its own bounded outcomes.
                self.pending.retain(|id, _| {
                    matches!(
                        self.core.state(id, now),
                        Ok(pairing::State::Pending
                            | pairing::State::Confirmed
                            | pairing::State::Approved)
                    )
                });
                let created = self
                    .core
                    .create_session(wire::encode(&exporter), now)
                    .map_err(|_| Refused)?;
                self.pending.insert(
                    created.session_id.clone(),
                    Pending {
                        exporter,
                        nonce: created.receiver_nonce,
                        transcript: None,
                    },
                );
                if show(&created.session_id, &pairing::display_code(&created.code)).is_err() {
                    self.core
                        .cancel(&created.session_id, now)
                        .map_err(|_| Refused)?;
                    return Err(Refused);
                }
                wire::encode_message(&wire::Opened {
                    version: wire::VERSION,
                    session_id: created.session_id,
                    receiver_nonce: wire::encode(&created.receiver_nonce),
                    endpoint: self.endpoint.clone(),
                    receiver_id: self.custody.receiver_id().into(),
                    issuer_key_id: self.custody.issuer().key_id.clone(),
                    issuer_public_key: wire::encode(&self.custody.issuer().public_key),
                })?
            }
            Request::Confirm(message) => {
                let pending = self
                    .pending
                    .get_mut(&message.session_id)
                    .filter(|p| p.exporter == exporter)
                    .ok_or(Refused)?;
                let transcript =
                    message.transcript(self.endpoint.clone(), pending.nonce, exporter)?;
                // Reserve before validation/KDF; all bound incorrect submissions charge.
                let attempt = self
                    .core
                    .reserve(&wire::encode(&exporter), transcript.clone(), now)
                    .map_err(|_| Refused)?;
                let verified = attempt.verify(&message.mac()?).map_err(|_| Refused)?;
                let proof = self.core.confirm(verified, clock()).map_err(|_| Refused)?;
                pending.transcript = Some(transcript);
                wire::encode_message(&wire::Confirmed {
                    version: wire::VERSION,
                    receiver_mac: wire::encode(proof.expose()),
                })?
            }
            Request::Approve(message) => {
                let pending = self
                    .pending
                    .get(&message.session_id)
                    .filter(|p| p.exporter == exporter)
                    .ok_or(Refused)?;
                let transcript = pending.transcript.as_ref().ok_or(Refused)?;
                if self
                    .core
                    .state(&message.session_id, now)
                    .map_err(|_| Refused)?
                    != pairing::State::Confirmed
                {
                    return Err(Refused);
                }
                if message.input.commitment() != transcript.scope {
                    return Err(Refused);
                }
                let evidence = wire::verify_possession(transcript, &message.proof)?;
                self.unavailable = if message
                    .input
                    .kinds
                    .iter()
                    .any(|(kind, _)| *kind == Kind::BodyChannel)
                {
                    Some("body_channel available after client-certificate issuance lands")
                } else if message
                    .input
                    .kinds
                    .iter()
                    .any(|(kind, _)| *kind == Kind::AnalyticsPublish)
                {
                    Some(
                        "analytics_publish available after origin connect / producer admission lands",
                    )
                } else {
                    None
                };
                if self.unavailable.is_some() {
                    return Err(Refused);
                }
                self.validate_input(&message.input, now)?;
                let grants = self.grants(&message.input.binding)?;
                self.core.approve(evidence, now).map_err(|_| Refused)?;
                let delivery = self
                    .core
                    .activate(&message.session_id, message.input, now)
                    .map_err(|_| Refused)?;
                // Fresh activation decision is held here; grants never precede commit.
                if let Some(grants) = grants {
                    self.custody
                        .install_current_owner_grants(&delivery.enrollment_id, &grants)
                        .map_err(|_| Refused)?;
                }
                delivery_bytes(delivery)?
            }
            Request::Cancel(message) => {
                self.pending
                    .get(&message.session_id)
                    .filter(|p| p.exporter == exporter)
                    .ok_or(Refused)?;
                self.cancel(&message.session_id, now)?;
                wire::encode_message(&wire::Cancelled {
                    version: wire::VERSION,
                })?
            }
            Request::Recover(message) => {
                let transcript = message.transcript.rebuild()?;
                if transcript.endpoint != self.endpoint {
                    return Err(Refused);
                }
                let mut signed = b"semon-asi-recovery-pop-v1\0".to_vec();
                signed.extend_from_slice(&transcript.digest());
                signed.extend_from_slice(transcript.session_id.as_bytes());
                signed.extend_from_slice(&exporter);
                let proof = wire::decode_fixed::<64>(&message.proof)?;
                ring::signature::UnparsedPublicKey::new(
                    &ring::signature::ECDSA_P256_SHA256_FIXED,
                    &transcript.origin_key,
                )
                .verify(&signed, &proof)
                .map_err(|_| Refused)?;
                let evidence = pairing::VerifiedApproval {
                    session_id: transcript.session_id.clone(),
                    origin_key: transcript.origin_key.clone(),
                    transcript_digest: transcript.digest(),
                    summary_digest: transcript.scope,
                };
                let registry = self.core.registry().map_err(|_| Refused)?;
                let receipt = registry
                    .pairing_receipts
                    .iter()
                    .find(|r| r.session_id == evidence.session_id)
                    .ok_or(Refused)?;
                let enrollment = registry
                    .enrollments()
                    .iter()
                    .find(|e| e.enrollment_id == receipt.enrollment_id)
                    .ok_or(Refused)?;
                if enrollment
                    .credential_kinds
                    .iter()
                    .any(|kind| kind.kind != Kind::MirrorPush)
                {
                    return Err(Refused);
                }
                let key = pairing::ConfirmedKey::recovered(
                    Zeroizing::new(wire::decode_fixed(&message.application_key)?),
                    transcript,
                );
                let delivery = self
                    .core
                    .deliver(&key, evidence, now)
                    .map_err(|_| Refused)?;
                // Revalidate current committed authority before completing interrupted ACL installation.
                let registry = self.core.registry().map_err(|_| Refused)?;
                let enrollment = registry
                    .enrollments()
                    .iter()
                    .find(|e| e.enrollment_id == delivery.enrollment_id && e.revoked_at.is_none())
                    .ok_or(Refused)?;
                if enrollment.machine != self.machine {
                    return Err(Refused);
                }
                if let Some(grants) = self.grants(&enrollment.binding)? {
                    self.custody
                        .install_current_owner_grants(&delivery.enrollment_id, &grants)
                        .map_err(|_| Refused)?;
                }
                delivery_bytes(delivery)?
            }
        };
        Ok(Zeroizing::new(bytes))
    }
    fn grants(
        &self,
        binding: &Binding,
    ) -> Result<Option<Vec<receiver_custody::OwnerGrant>>, Refused> {
        match binding {
            Binding::Inapplicable => Ok(None),
            Binding::Admitted { namespace, .. } => receiver_custody::owner_grants(
                self.custody.owner(),
                &self.machine,
                namespace,
                false,
            )
            .map(Some)
            .map_err(|_| Refused),
        }
    }
    fn validate_input(&self, input: &pairing::ActivationInput, now: u64) -> Result<(), Refused> {
        if input.receiver_id != self.custody.receiver_id()
            || input.machine != self.machine
            || input.valid_until <= now
            || input.valid_until > now.saturating_add(wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS)
            || !input.client_certificate_pins.is_empty()
            || input.kinds.is_empty()
            || input
                .kinds
                .iter()
                .any(|(kind, certificate)| *certificate || *kind != Kind::MirrorPush)
            || !matches!(input.binding, Binding::Inapplicable)
            || input.kinds.len() != 1
        {
            return Err(Refused);
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "provision_tests.rs"]
mod tests;

fn delivery_bytes(delivery: pairing::Delivery) -> Result<Vec<u8>, Refused> {
    wire::encode_message(&wire::Activated {
        version: wire::VERSION,
        enrollment_id: delivery.enrollment_id,
        credentials: delivery
            .credentials
            .into_iter()
            .map(|(kind, secret)| (kind, secret.expose().to_owned()))
            .collect(),
    })
}
