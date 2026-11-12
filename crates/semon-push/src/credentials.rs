//! Kind-scoped bearer custody and pure authorization. No transport or issuance protocol.
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::enrollment::{Enrollment, Registry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    AnalyticsPublish,
    BodyChannel,
    MirrorPush,
}

/// A secret has no serializer and always redacts formatting. Exposure is explicit
/// for the future protected provisioning transport, never registry persistence.
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

/// At most one previous generation can overlap the current one. Old bearer
/// records remain as terminal history after the overlap ends.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed {
        enrollment_id: String,
        kind: Kind,
        generation: u64,
    },
    Refused(Refusal),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Unknown secret, wrong kind and wrong route deliberately share one outcome.
    InvalidCredential,
    Revoked,
    OutsideValidity,
    RetiredGeneration,
    ClientCertificate,
}

impl Registry {
    /// `verified_pin` must come from successful TLS client-certificate verification;
    /// parsing a request-supplied fingerprint does not establish that evidence.
    /// `now` is injected Unix time; this function does no I/O and reads no clock.
    pub fn authorize(
        &self,
        kind: Kind,
        route_kind: Kind,
        secret: &Secret,
        verified_pin: Option<&crate::enrollment::Pin>,
        now: u64,
    ) -> Decision {
        if self.validate().is_err() {
            return Decision::Refused(Refusal::InvalidCredential);
        }
        let hash = secret.hash();
        let mut matched = None;
        for bearer in &self.credentials {
            // Compare every digest, including records of other kinds.
            let equal = ct_eq(&hash, &bearer.sha256);
            if equal && bearer.kind == kind && kind == route_kind {
                matched = Some(bearer);
            }
        }
        let Some(bearer) = matched else {
            return Decision::Refused(Refusal::InvalidCredential);
        };
        let Some(enrollment) = self
            .enrollments
            .iter()
            .find(|e| e.enrollment_id == bearer.enrollment_id)
        else {
            return Decision::Refused(Refusal::InvalidCredential);
        };
        authorize_enrollment(enrollment, bearer, verified_pin, now)
    }
}
fn authorize_enrollment(
    enrollment: &Enrollment,
    bearer: &Bearer,
    pin: Option<&crate::enrollment::Pin>,
    now: u64,
) -> Decision {
    let Some(scope) = enrollment
        .credential_kinds
        .iter()
        .find(|s| s.kind == bearer.kind)
    else {
        return Decision::Refused(Refusal::InvalidCredential);
    };
    let refused = if enrollment.revoked_at.is_some()
        || scope.revoked_at.is_some()
        || bearer.revoked_at.is_some()
    {
        Some(Refusal::Revoked)
    } else if now < bearer.valid_from || now >= bearer.valid_until {
        Some(Refusal::OutsideValidity)
    } else if !scope.generations.accepts(bearer.generation, now) {
        Some(Refusal::RetiredGeneration)
    } else if (scope.client_cert_required || pin.is_some())
        && !pin.is_some_and(|pin| {
            enrollment
                .client_certificate_pins
                .iter()
                .any(|p| &p.pin == pin && p.generation == bearer.generation)
        })
    {
        Some(Refusal::ClientCertificate)
    } else {
        None
    };
    match refused {
        Some(reason) => Decision::Refused(reason),
        None => Decision::Allowed {
            enrollment_id: enrollment.enrollment_id.clone(),
            kind: bearer.kind,
            generation: bearer.generation,
        },
    }
}
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for i in 0..32 {
        difference |= a[i] ^ b[i];
    }
    std::hint::black_box(difference) == 0
}
