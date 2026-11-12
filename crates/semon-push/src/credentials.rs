//! Kind-scoped bearer custody and pure authorization. No transport or issuance protocol.
use crate::enrollment::Registry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
pub use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    AnalyticsPublish,
    BodyChannel,
    MirrorPush,
}

/// No serializer; exposure is explicit for future protected provisioning only.
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn generate() -> Result<Self, crate::enrollment::Error> {
        Ok(Self::new(crate::enrollment::random_id()?))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_bytes()).into()
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bearer {
    pub credential_id: String,
    pub enrollment_id: String,
    pub kind: Kind,
    pub sha256: [u8; 32],
    pub generation: u64,
    /// Inclusive Unix seconds.
    pub valid_from: u64,
    /// Exclusive Unix seconds.
    pub valid_until: u64,
    pub revoked_at: Option<u64>,
}
/// Retain old bearer records as terminal history after overlap ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generations {
    pub current: u64,
    pub retiring: Option<Retiring>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retiring {
    pub generation: u64,
    pub cutover_at: u64,
}
impl Generations {
    pub(crate) fn accepts(&self, generation: u64, now: u64) -> bool {
        generation == self.current
            || self
                .retiring
                .as_ref()
                .is_some_and(|old| old.generation == generation && now < old.cutover_at)
    }
}
/// The only outcome a transport may expose to a peer. All refusals are identical.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed {
        enrollment_id: String,
        kind: Kind,
        generation: u64,
    },
    Refused,
}
/// Local audit diagnostic only. Deliberately has no Display or serialization.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    InvalidCredential,
    Revoked,
    OutsideValidity,
    RetiredGeneration,
    ClientCertificate,
}
/// Not a wire response. Send only `decision` to transport response mapping;
/// `local_diagnostic` is solely for authenticated local audit/debug consumers.
/// No Display or serialization is implemented for this container.
#[derive(Clone, PartialEq, Eq)]
pub struct Authorization {
    pub decision: Decision,
    local_diagnostic: Option<Refusal>,
}
impl Authorization {
    #[cfg(test)]
    pub(crate) fn expected(decision: Decision, diagnostic: Option<Refusal>) -> Self {
        Self {
            decision,
            local_diagnostic: diagnostic,
        }
    }
    /// Explicit access for authenticated local audit consumers only.
    pub fn local_diagnostic(&self) -> Option<Refusal> {
        self.local_diagnostic
    }
}
impl fmt::Debug for Authorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authorization")
            .field("decision", &self.decision)
            .finish()
    }
}
impl fmt::Debug for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<local diagnostic redacted>")
    }
}
impl Registry {
    /// Recheck one previously authenticated credential without retaining its secret.
    pub(crate) fn current_credential(
        &self,
        credential_id: &str,
        verified_pin: Option<&crate::enrollment::Pin>,
        now: u64,
    ) -> Option<&Bearer> {
        let bearer = self
            .credentials
            .iter()
            .find(|c| c.credential_id == credential_id)?;
        let enrollment = self
            .enrollments
            .iter()
            .find(|e| e.enrollment_id == bearer.enrollment_id)?;
        let scope = enrollment
            .credential_kinds
            .iter()
            .find(|s| s.kind == bearer.kind)?;
        let certificate = enrollment.client_certificate_pins.iter().any(|p| {
            Some(&p.pin) == verified_pin
                && p.generation == bearer.generation
                && p.revoked_at.is_none()
        });
        credential_authority_refusal(
            enrollment,
            scope,
            bearer,
            verified_pin.is_some(),
            certificate,
            now,
        )
        .is_none()
        .then_some(bearer)
    }

    /// `verified_pin` must come from verified TLS evidence, never a request field.
    /// Time is injected Unix seconds. The scan schedule depends on stored state,
    /// not whether the presented digest matches. This is not a constant-time
    /// promise for the whole Rust function; every digest comparison is fixed-work.
    pub fn authorize(
        &self,
        kind: Kind,
        route_kind: Kind,
        secret: &Secret,
        verified_pin: Option<&crate::enrollment::Pin>,
        now: u64,
    ) -> Authorization {
        let hash = secret.hash();
        let valid = self.validate().is_ok();
        let mut selected = None;
        for bearer in &self.credentials {
            let equal = ct_eq(&hash, &bearer.sha256);
            let mut found = None;
            // Perform the same enrollment/scope/pin scans for misses and hits.
            for enrollment in &self.enrollments {
                let mut certificate = false;
                for pin in &enrollment.client_certificate_pins {
                    certificate |= (Some(&pin.pin) == verified_pin)
                        & (pin.generation == bearer.generation)
                        & pin.revoked_at.is_none();
                }
                for scope in &enrollment.credential_kinds {
                    let reason = credential_authority_refusal(
                        enrollment,
                        scope,
                        bearer,
                        verified_pin.is_some(),
                        certificate,
                        now,
                    );
                    if (enrollment.enrollment_id == bearer.enrollment_id)
                        & (scope.kind == bearer.kind)
                    {
                        found = Some(reason);
                    }
                }
            }
            // Every stored digest is compared without early exit. Branches on
            // a full 256-bit match are observable only to a holder of the valid
            // secret; a nonmatching secret follows the same all-digest scan.
            if equal & (bearer.kind == kind) & (kind == route_kind) {
                selected = Some((bearer, found.unwrap_or(Some(Refusal::InvalidCredential))));
            }
        }
        if valid && let Some((bearer, None)) = selected {
            Authorization {
                decision: Decision::Allowed {
                    enrollment_id: bearer.enrollment_id.clone(),
                    kind: bearer.kind,
                    generation: bearer.generation,
                },
                local_diagnostic: None,
            }
        } else {
            Authorization {
                decision: Decision::Refused,
                local_diagnostic: Some(if valid {
                    selected
                        .and_then(|(_, reason)| reason)
                        .unwrap_or(Refusal::InvalidCredential)
                } else {
                    Refusal::InvalidCredential
                }),
            }
        }
    }
}
fn credential_authority_refusal(
    enrollment: &crate::enrollment::Enrollment,
    scope: &crate::enrollment::KindScope,
    bearer: &Bearer,
    pin_presented: bool,
    certificate: bool,
    now: u64,
) -> Option<Refusal> {
    if enrollment.revoked_at.is_some() | scope.revoked_at.is_some() | bearer.revoked_at.is_some() {
        Some(Refusal::Revoked)
    } else if !((now >= bearer.valid_from) & (now < bearer.valid_until)) {
        Some(Refusal::OutsideValidity)
    } else if !scope.generations.accepts(bearer.generation, now) {
        Some(Refusal::RetiredGeneration)
    } else if (scope.client_cert_required | pin_presented) & !certificate {
        Some(Refusal::ClientCertificate)
    } else {
        None
    }
}

fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for i in 0..32 {
        difference |= a[i] ^ b[i];
    }
    std::hint::black_box(difference) == 0
}
