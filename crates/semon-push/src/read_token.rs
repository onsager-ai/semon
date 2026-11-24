//! Durable owner read and snapshot write authority, separate from machine push credentials.
use crate::{
    credentials::{Bearer, Secret},
    enrollment::{Error, Registry, Store},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Snapshot,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    tokens: Vec<Bearer<Scope>>,
    audit: Vec<Audit>,
    public_url: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Audit {
    time: u64,
    action: String,
    token_id: String,
}

pub fn public_origin(value: &str) -> std::result::Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid public URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "public URL must be an HTTP(S) origin without credentials, path, query or fragment"
                .into(),
        );
    }
    Ok(url.origin().ascii_serialization())
}
pub fn configure(dir: &Path, value: Option<&str>) -> Result<()> {
    let value = value
        .map(public_origin)
        .transpose()
        .map_err(|_| Error::Invalid)?;
    let mut store = Store::open(dir)?;
    store.pairing_transaction(0, |r| {
        r.read_tokens.public_url = value;
        Ok(())
    })
}
pub fn create(dir: &Path, now: u64) -> Result<(Secret, Option<String>)> {
    mint(dir, now, Scope::Read)
}

pub fn create_snapshot(dir: &Path, now: u64) -> Result<Secret> {
    mint(dir, now, Scope::Snapshot).map(|(secret, _)| secret)
}

fn mint(dir: &Path, now: u64, scope: Scope) -> Result<(Secret, Option<String>)> {
    let secret = Secret::generate()?;
    let mut store = Store::open(dir)?;
    store.pairing_transaction(0, |r| {
        let now = now.max(r.latest);
        let id = crate::enrollment::random_id()?;
        r.read_tokens.tokens.push(Bearer {
            credential_id: id.clone(),
            enrollment_id: id.clone(),
            kind: scope,
            sha256: secret.hash(),
            generation: 1,
            valid_from: now,
            valid_until: u64::MAX,
            revoked_at: None,
        });
        r.read_tokens.audit.push(Audit {
            time: now,
            action: "created".into(),
            token_id: id,
        });
        r.latest = now;
        r.check_admission()?;
        Ok((secret, r.read_tokens.public_url.clone()))
    })
}
pub fn list(dir: &Path) -> Result<Vec<(String, u64, bool, Scope)>> {
    Ok(Registry::read_current(dir)?
        .read_tokens
        .tokens
        .iter()
        .map(|t| {
            (
                t.credential_id.clone(),
                t.valid_from,
                t.revoked_at.is_some(),
                t.kind,
            )
        })
        .collect())
}
pub fn revoke(dir: &Path, id: &str, now: u64) -> Result<()> {
    let mut store = Store::open(dir)?;
    store.pairing_transaction(0, |r| {
        let now = now.max(r.latest);
        let token = r
            .read_tokens
            .tokens
            .iter_mut()
            .find(|t| t.credential_id == id)
            .ok_or(Error::Invalid)?;
        if token.revoked_at.is_none() {
            token.revoked_at = Some(now);
            r.read_tokens.audit.push(Audit {
                time: now,
                action: "revoked".into(),
                token_id: id.into(),
            });
        }
        r.latest = now;
        Ok(())
    })
}

pub struct Access(pub PathBuf);
impl semon_sessions::ViewerAccess for Access {
    fn issue_pairing_code(&self) -> std::result::Result<serde_json::Value, String> {
        let now = crate::simple_pair::now()?;
        let (code, expires) = crate::simple_pair::issue(&self.0, now).map_err(|e| e.to_string())?;
        Ok(
            serde_json::json!({"code": crate::pairing::display_code(&code).expose(), "expires": expires}),
        )
    }

    fn resolve(&self, secret: &str) -> Option<String> {
        identify(&self.0, secret, Scope::Read)
            .filter(|(_, revoked)| !revoked)
            .map(|(id, _)| id)
    }

    fn management_snapshot(&self) -> std::io::Result<Option<serde_json::Value>> {
        crate::manage::latest(&self.0)
    }
}

pub(crate) fn identify(dir: &Path, secret: &str, scope: Scope) -> Option<(String, bool)> {
    if secret.is_empty() || secret.len() > 512 {
        return None;
    }
    let hash = Secret::new(secret.to_owned()).hash();
    Registry::read_current(dir)
        .ok()?
        .read_tokens
        .tokens
        .iter()
        .find(|t| t.kind == scope && crate::credentials::ct_eq(&hash, &t.sha256))
        .map(|t| (t.credential_id.clone(), t.revoked_at.is_some()))
}

impl Registry {
    pub(crate) fn validate_read_tokens(&self) -> Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        let mut hashes = std::collections::BTreeSet::new();
        for t in &self.read_tokens.tokens {
            if t.enrollment_id != t.credential_id
                || t.generation != 1
                || t.valid_until != u64::MAX
                || t.valid_from > self.latest
                || t.revoked_at
                    .is_some_and(|n| n < t.valid_from || n > self.latest)
                || !ids.insert(&t.credential_id)
                || !hashes.insert(t.sha256)
            {
                return Err(Error::Invalid);
            }
        }
        if self
            .read_tokens
            .public_url
            .as_ref()
            .is_some_and(|u| public_origin(u).as_ref() != Ok(u))
            || self.read_tokens.audit.iter().any(|a| {
                a.time > self.latest
                    || !ids.contains(&a.token_id)
                    || !matches!(a.action.as_str(), "created" | "revoked")
            })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub(crate) fn read_tokens_follow(&self, old: &Self) -> Result<()> {
        if !self.read_tokens.audit.starts_with(&old.read_tokens.audit) {
            return Err(Error::Terminal);
        }
        for previous in &old.read_tokens.tokens {
            let next = self
                .read_tokens
                .tokens
                .iter()
                .find(|t| t.credential_id == previous.credential_id)
                .ok_or(Error::Terminal)?;
            let mut expected = previous.clone();
            expected.revoked_at = next.revoked_at;
            if &expected != next
                || (previous.revoked_at.is_some() && previous.revoked_at != next.revoked_at)
            {
                return Err(Error::Terminal);
            }
        }
        Ok(())
    }
}
