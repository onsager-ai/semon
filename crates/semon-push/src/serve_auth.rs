//! Receiver authentication from the current pairing registry.
use crate::{
    credentials::Secret,
    enrollment::{self, Registry},
};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct ConnectionAuthority {
    machine: Option<String>,
}

pub(super) struct RegistryAuthenticator {
    dir: PathBuf,
}
impl RegistryAuthenticator {
    fn open(dir: &Path) -> Result<Self, String> {
        enrollment::refuse_legacy_tokens(dir).map_err(|e| e.to_string())?;
        Registry::read_current(dir).map_err(|e| e.to_string())?;
        Ok(Self {
            dir: dir.to_owned(),
        })
    }
    pub(super) fn pair(
        &self,
        body: &[u8],
        now: u64,
    ) -> Result<crate::simple_pair::Response, enrollment::Error> {
        crate::simple_pair::pair(&self.dir, body, now)
    }
    pub(super) fn unpair(
        &self,
        authorization: Option<&str>,
        now: u64,
    ) -> Result<(), enrollment::Error> {
        crate::simple_pair::unpair_token(&self.dir, authorization, now)
    }
    pub(super) fn authenticate(
        &self,
        conn: &mut ConnectionAuthority,
        authorization: Option<&str>,
    ) -> Option<String> {
        enrollment::refuse_legacy_tokens(&self.dir).ok()?;
        let registry = Registry::read_current(&self.dir).ok()?;
        let bearer = authorization?.strip_prefix("Bearer ")?;
        if bearer.is_empty() || bearer.len() > 512 {
            return None;
        }
        let secret = Secret::new(bearer.to_owned());
        let credential = registry.paired_token(&secret)?;
        let machine = &credential.enrollment_id;
        if conn.machine.as_ref().is_some_and(|id| id != machine) {
            return None;
        }
        conn.machine = Some(machine.clone());
        Some(machine.clone())
    }
}

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
            .map(|r| {
                r.simple_pair
                    .tokens
                    .iter()
                    .filter(|t| t.revoked_at.is_none())
                    .count()
            })
            .map_err(|e| e.to_string())
    }
}
