//! Receiver authentication. TLS evidence is constructed only from verified peers.
use std::path::{Path, PathBuf};

use crate::{
    credentials::{Decision, Kind, Secret},
    enrollment::{self, Binding, ClaimEpoch, Pin, Registry},
    tokens::{Tokens, check_tokens_mode},
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
    pub revocation: RevisionHandle,
}

/// A changed revision requires a new authorization decision before further release.
/// An unreadable registry also invalidates this handle. No secret is retained.
#[derive(Clone, Debug)]
pub struct RevisionHandle {
    dir: PathBuf,
    revision: u64,
}
impl RevisionHandle {
    pub fn is_current(&self) -> bool {
        Registry::read_current(&self.dir).is_ok_and(|r| r.revision() == self.revision)
    }
}

pub struct RegistryAuthenticator {
    dir: PathBuf,
}
impl RegistryAuthenticator {
    pub fn open(dir: &Path) -> Result<Self, String> {
        enrollment::refuse_legacy_tokens(dir).map_err(|e| e.to_string())?;
        Registry::read_current(dir).map_err(|e| e.to_string())?;
        Ok(Self {
            dir: dir.to_owned(),
        })
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
        let decision = pins.into_iter().find_map(|pin| {
            match registry
                .authorize(required_kind, required_kind, &secret, pin, now)
                .decision
            {
                Decision::Allowed { enrollment_id, .. } => Some(enrollment_id),
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
                .filter(|k| k.revoked_at.is_none())
                .map(|k| k.kind)
                .collect(),
            verified_client_identity: identity.cloned(),
            revocation: RevisionHandle {
                dir: self.dir.clone(),
                revision: registry.revision(),
            },
        })
    }
}

/// The sole transitional seam, removed in the pairing cutover PR.
pub(super) enum Authenticator {
    // removed in the pairing cutover PR
    Legacy(Tokens),
    Registry(RegistryAuthenticator),
}
impl Authenticator {
    pub(super) fn open(dir: &Path) -> Result<Self, String> {
        // An incomplete or invalid enrollment directory must never enable legacy auth.
        match std::fs::symlink_metadata(dir.join(enrollment::DIRECTORY)) {
            Ok(_) => RegistryAuthenticator::open(dir).map(Self::Registry),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                check_tokens_mode(dir)?;
                Tokens::open(dir).map(Self::Legacy)
            }
            Err(e) => Err(e.to_string()),
        }
    }
    pub(super) fn count(&self) -> Result<usize, String> {
        match self {
            Self::Legacy(tokens) => Ok(tokens.len()),
            Self::Registry(auth) => Registry::read_current(&auth.dir)
                .map(|r| r.credentials().len())
                .map_err(|e| e.to_string()),
        }
    }
    pub(super) fn authorize(
        &self,
        conn: &mut ConnectionAuthority,
        credential: Option<&str>,
        kind: Kind,
        now: u64,
    ) -> Option<String> {
        match self {
            Self::Registry(auth) => auth
                .authenticated_enrollment(conn, credential, kind, now)
                .map(|e| e.machine),
            Self::Legacy(tokens)
                if kind == Kind::MirrorPush && conn.verified_client_identity.is_none() =>
            {
                let machine = tokens.authorize(credential)?;
                if conn.enrollment_id.as_ref().is_some_and(|id| id != &machine) {
                    return None;
                }
                conn.enrollment_id = Some(machine.clone());
                Some(machine)
            }
            Self::Legacy(_) => None,
        }
    }
}
