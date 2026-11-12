//! Receiver authentication. TLS evidence is constructed only from verified peers.
use std::path::{Path, PathBuf};

use crate::{
    credentials::{Decision, Kind, Secret},
    enrollment::{self, Binding, ClaimEpoch, Pin, Registry},
};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedClientIdentity {
    spki: Pin,
    leaf: Pin,
}
impl VerifiedClientIdentity {
    pub fn spki(&self) -> &Pin {
        &self.spki
    }
    pub fn leaf(&self) -> &Pin {
        &self.leaf
    }
    /// Domain-separated identifier; does not expose the certificate pin itself.
    pub fn opaque_id(&self) -> crate::origin_read::OpaqueId {
        let Pin::Spki(pin) = &self.spki else {
            unreachable!("verified identity always has an SPKI pin")
        };
        let mut hash = Sha256::new();
        hash.update(b"semon.verified-client-identity.v1\0");
        hash.update(pin);
        let id: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        crate::origin_read::OpaqueId::new(&id).expect("SHA-256 hex is an opaque identifier")
    }
    pub(super) fn from_verified_der(
        der: &rustls::pki_types::CertificateDer<'_>,
    ) -> Result<Self, rustls::Error> {
        let parsed = rustls::server::ParsedCertificate::try_from(der)?;
        Ok(Self {
            spki: Pin::Spki(Sha256::digest(parsed.subject_public_key_info().as_ref()).into()),
            leaf: Pin::Leaf(Sha256::digest(der.as_ref()).into()),
        })
    }
}

/// One connection cannot change enrollment after its first authorized request.
#[derive(Default)]
pub struct ConnectionAuthority {
    pub(super) verified_client_identity: Option<VerifiedClientIdentity>,
    enrollment_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AuthenticatedEnrollment {
    pub enrollment_id: String,
    pub machine: String,
    pub namespace: Option<String>,
    pub origin_incarnation: Option<String>,
    pub index_incarnation: Option<String>,
    pub publisher_claim_epoch: Option<ClaimEpoch>,
    pub kinds: Vec<Kind>,
    pub verified_client_identity: Option<VerifiedClientIdentity>,
    pub renewal_due: bool,
    pub authority: CredentialHandle,
}
impl TryFrom<&AuthenticatedEnrollment> for crate::origin_read::EnrollmentScope {
    type Error = crate::origin_read::Refused;
    fn try_from(value: &AuthenticatedEnrollment) -> Result<Self, Self::Error> {
        use crate::origin_read::{OpaqueId, Refused};
        let required =
            |v: &Option<String>| OpaqueId::new(v.as_deref().ok_or(Refused::InvalidRequest)?);
        Ok(Self {
            enrollment_id: OpaqueId::new(&value.enrollment_id)?,
            machine: OpaqueId::new(&value.machine)?,
            namespace: required(&value.namespace)?,
            origin_incarnation: required(&value.origin_incarnation)?,
            index_incarnation: required(&value.index_incarnation)?,
            publisher_claim_epoch: match &value.publisher_claim_epoch {
                Some(ClaimEpoch::Assigned(epoch)) => Some(*epoch),
                _ => None,
            },
            kinds: value.kinds.iter().copied().map(Into::into).collect(),
            verified_client_identity: value
                .verified_client_identity
                .as_ref()
                .ok_or(Refused::InvalidRequest)?
                .opaque_id(),
        })
    }
}
impl From<Kind> for crate::origin_read::EnrollmentKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::AnalyticsPublish => Self::Analytics,
            Kind::BodyChannel => Self::BodyChannel,
            Kind::MirrorPush => Self::Mirror,
        }
    }
}
impl From<crate::origin_read::EnrollmentKind> for Kind {
    fn from(kind: crate::origin_read::EnrollmentKind) -> Self {
        match kind {
            crate::origin_read::EnrollmentKind::Analytics => Self::AnalyticsPublish,
            crate::origin_read::EnrollmentKind::BodyChannel => Self::BodyChannel,
            crate::origin_read::EnrollmentKind::Mirror => Self::MirrorPush,
        }
    }
}

/// Authority of one authenticated credential, independent of registry churn.
/// An unreadable registry invalidates this handle. No secret or digest is retained.
#[derive(Clone, Debug)]
pub struct CredentialHandle {
    dir: PathBuf,
    credential_id: String,
    enrollment_id: String,
    kind: Kind,
    generation: u64,
    verified_pin: Option<Pin>,
    valid_from: u64,
    valid_until: u64,
}
impl CredentialHandle {
    pub fn is_current(&self) -> bool {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok_and(|now| self.is_current_at(now.as_secs()))
    }
    /// Re-read current authority, including any cutover scheduled after admission.
    pub fn is_current_at(&self, now: u64) -> bool {
        now >= self.valid_from
            && now < self.valid_until
            && enrollment::refuse_legacy_tokens(&self.dir).is_ok()
            && Registry::read_current(&self.dir).is_ok_and(|registry| {
                registry
                    .current_credential(&self.credential_id, self.verified_pin.as_ref(), now)
                    .is_some_and(|credential| {
                        credential.enrollment_id == self.enrollment_id
                            && credential.kind == self.kind
                            && credential.generation == self.generation
                    })
            })
    }
}

pub struct RegistryAuthenticator {
    dir: PathBuf,
    renewal: std::sync::Mutex<crate::renewal::Receiver>,
}
impl RegistryAuthenticator {
    pub fn open(dir: &Path) -> Result<Self, String> {
        enrollment::refuse_legacy_tokens(dir).map_err(|e| e.to_string())?;
        Registry::read_current(dir)
            .map_err(|e| e.to_string().replace("DIR", &dir.display().to_string()))?;
        Ok(Self {
            dir: dir.to_owned(),
            renewal: std::sync::Mutex::new(crate::renewal::Receiver::default()),
        })
    }
    pub(crate) fn renew(
        &self,
        request: crate::renewal_wire::Request,
        exporter: [u8; 32],
        identity: Option<&VerifiedClientIdentity>,
        now: u64,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, crate::provision_wire::Refused> {
        enrollment::refuse_legacy_tokens(&self.dir).map_err(|_| crate::provision_wire::Refused)?;
        self.renewal
            .try_lock()
            .map_err(|_| crate::provision_wire::Refused)?
            .handle(&self.dir, request, exporter, identity, now)
    }
    /// Current state is read for every request; callers never receive local diagnostics.
    pub fn authenticated_enrollment(
        &self,
        conn: &mut ConnectionAuthority,
        request_credential: Option<&str>,
        required_kind: Kind,
        now: u64,
    ) -> Option<AuthenticatedEnrollment> {
        enrollment::refuse_legacy_tokens(&self.dir).ok()?;
        let registry = Registry::read_current(&self.dir).ok()?;
        let bearer = request_credential?.strip_prefix("Bearer ")?;
        if bearer.is_empty() || bearer.len() > 512 {
            return None;
        }
        let secret = Secret::new(bearer.to_owned());
        let identity = conn.verified_client_identity.as_ref();
        if required_kind == Kind::BodyChannel && identity.is_none() {
            return None;
        }
        let pins: Vec<Option<&Pin>> = match identity {
            Some(identity) => vec![Some(&identity.spki), Some(&identity.leaf)],
            None => vec![None],
        };
        let (decision, verified_pin) = pins.into_iter().find_map(|pin| {
            match registry
                .authorize(required_kind, required_kind, &secret, pin, now)
                .decision
            {
                Decision::Allowed { enrollment_id, .. } => Some((enrollment_id, pin.cloned())),
                Decision::Refused => None,
            }
        })?;
        if conn
            .enrollment_id
            .as_ref()
            .is_some_and(|id| id != &decision)
        {
            return None;
        }
        let registry = enrollment::Store::authorize_cutover(
            &self.dir,
            required_kind,
            &secret,
            verified_pin.as_ref(),
            now,
        )
        .ok()?;
        let enrollment = registry
            .enrollments()
            .iter()
            .find(|e| e.enrollment_id == decision)?;
        let (namespace, origin, index, epoch) = match &enrollment.binding {
            Binding::Inapplicable => (None, None, None, None),
            Binding::Admitted {
                namespace,
                origin_incarnation,
                index_incarnation,
                publisher_claim_epoch,
                ..
            } => (
                Some(namespace.clone()),
                Some(origin_incarnation.clone()),
                Some(index_incarnation.clone()),
                Some(publisher_claim_epoch.clone()),
            ),
        };
        let credential = registry.credentials().iter().find(|c| {
            c.enrollment_id == decision && c.kind == required_kind && c.sha256 == secret.hash()
        })?;
        let scope = enrollment
            .credential_kinds
            .iter()
            .find(|s| s.kind == required_kind)?;
        let valid_until = scope
            .generations
            .retiring
            .as_ref()
            .filter(|old| old.generation == credential.generation)
            .map_or(credential.valid_until, |old| {
                credential.valid_until.min(old.cutover_at)
            });
        conn.enrollment_id = Some(decision.clone());
        Some(AuthenticatedEnrollment {
            enrollment_id: decision,
            machine: enrollment.machine.clone(),
            namespace,
            origin_incarnation: origin,
            index_incarnation: index,
            publisher_claim_epoch: epoch,
            kinds: enrollment
                .credential_kinds
                .iter()
                .filter(|k| k.available)
                .map(|k| k.kind)
                .collect(),
            verified_client_identity: identity.cloned(),
            renewal_due: credential
                .valid_until
                .saturating_sub(now.max(registry.latest))
                <= crate::renewal_wire::WINDOW
                || enrollment.credential_kinds.iter().any(|s| {
                    s.enable_authorization
                        .as_ref()
                        .is_some_and(|a| a.consumed_renewal_id.is_none())
                }),
            authority: CredentialHandle {
                dir: self.dir.clone(),
                credential_id: credential.credential_id.clone(),
                enrollment_id: credential.enrollment_id.clone(),
                kind: credential.kind,
                generation: credential.generation,
                verified_pin,
                valid_from: credential.valid_from,
                valid_until,
            },
        })
    }
}

/// Registry-only authentication; legacy custody never enables another path.
pub(super) struct Authenticator(RegistryAuthenticator);
impl Authenticator {
    pub(super) fn open(dir: &Path) -> Result<Self, String> {
        RegistryAuthenticator::open(dir).map(Self)
    }
    pub(super) fn registry(&self) -> &RegistryAuthenticator {
        &self.0
    }
    pub(super) fn count(&self) -> Result<usize, String> {
        Registry::read_current(&self.0.dir)
            .map(|r| r.credentials().len())
            .map_err(|e| e.to_string())
    }
    #[cfg(test)]
    pub(super) fn authorize(
        &self,
        conn: &mut ConnectionAuthority,
        credential: Option<&str>,
        kind: Kind,
        now: u64,
    ) -> Option<String> {
        self.0
            .authenticated_enrollment(conn, credential, kind, now)
            .map(|e| e.machine)
    }
}
