//! One-time pairing and long-lived, revocable push credentials.
use crate::{
    credentials::{Bearer, Kind, Secret},
    enrollment::{Error, Registry, Store},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

fn constant_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for i in 0..32 {
        difference |= a[i] ^ b[i];
    }
    std::hint::black_box(difference) == 0
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    pending: Option<Pending>,
    burns: Vec<u64>,
    pause_until: u64,
    pub tokens: Vec<Bearer>,
    audit: Vec<Audit>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    sha256: [u8; 32],
    expires: u64,
    wrong: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Audit {
    time: u64,
    action: String,
    machine_id: String,
    replaced: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub code: String,
    pub machine_id: String,
    pub hostname: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub token: String,
    pub machine_id: String,
}

pub fn now() -> std::result::Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|e| e.to_string())
}

/// Only a new empty receiver directory may acquire a registry automatically.
pub fn initialize(dir: &Path) -> std::result::Result<(), String> {
    crate::tokens::prepare_private_dir(dir, true)?;
    if !dir.join(crate::enrollment::DIRECTORY).exists() {
        let mut entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
        if entries.next().is_some() {
            return Err(
                "receiver directory has no registry; preserve it and use a new empty directory"
                    .into(),
            );
        }
        Store::initialize(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn issue(dir: &Path, now: u64) -> Result<(Secret, u64)> {
    let code = crate::pairing::generate_code()?;
    let mut store = Store::open(dir)?;
    store.pairing_transaction(0, |r| {
        let now = now.max(r.latest);
        if now < r.simple_pair.pause_until {
            return Err(Error::Invalid);
        }
        let expires = now
            .checked_add(crate::pairing::WINDOW)
            .ok_or(Error::Invalid)?;
        r.simple_pair.pending = Some(Pending {
            sha256: code.hash(),
            expires,
            wrong: 0,
        });
        r.latest = now;
        Ok((code, expires))
    })
}

pub(crate) fn pair(dir: &Path, body: &[u8], now: u64) -> Result<Response> {
    if body.len() > 8192 {
        return Err(Error::Invalid);
    }
    let request: Request = serde_json::from_slice(body).map_err(|_| Error::Invalid)?;
    if request.version != 1
        || !semon_sessions::machine_id::valid(&request.machine_id)
        || !semon_sessions::received_registry::valid_owner_name(&request.hostname)
    {
        return Err(Error::Invalid);
    }
    let mut store = Store::open(dir)?;
    // A refused guess is a successful state transaction: its attempt and pause
    // must survive process restarts, including the third wrong guess.
    store.pairing_transaction(0, |r| {
        let now = now.max(r.latest);
        r.latest = now;
        if now < r.simple_pair.pause_until {
            return Ok(Err(Error::Invalid));
        }
        let Some(pending) = r.simple_pair.pending.as_mut() else {
            return Ok(Err(Error::Invalid));
        };
        if now >= pending.expires {
            r.simple_pair.pending = None;
            return Ok(Err(Error::Invalid));
        }
        let correct = crate::pairing::parse_code(&request.code)
            .is_ok_and(|code| constant_eq(&code.hash(), &pending.sha256));
        if !correct {
            pending.wrong += 1;
            if pending.wrong == 3 {
                r.simple_pair.pending = None;
                r.simple_pair
                    .burns
                    .retain(|t| now.saturating_sub(*t) < 3600);
                r.simple_pair.burns.push(now);
                if r.simple_pair.burns.len() >= 3 {
                    r.simple_pair.pause_until = now.saturating_add(1800);
                }
            }
            return Ok(Err(Error::Invalid));
        }
        let token = Secret::generate()?;
        let credential_id = crate::enrollment::random_id()?;
        let replaced = r
            .simple_pair
            .tokens
            .iter()
            .any(|t| t.enrollment_id == request.machine_id);
        revoke(r, &request.machine_id, now);
        r.ensure_machine(&request.machine_id, Some(&request.hostname))?;
        r.simple_pair.tokens.push(Bearer {
            credential_id,
            enrollment_id: request.machine_id.clone(),
            kind: Kind::Push,
            sha256: token.hash(),
            generation: 1,
            valid_from: now,
            valid_until: u64::MAX,
            revoked_at: None,
        });
        r.simple_pair.audit.push(Audit {
            time: now,
            action: "paired".into(),
            machine_id: request.machine_id.clone(),
            replaced,
        });
        r.simple_pair.pending = None;
        r.check_admission()?;
        Ok(Ok(Response {
            version: 1,
            token: token.expose().into(),
            machine_id: request.machine_id,
        }))
    })?
}

fn revoke(r: &mut Registry, machine: &str, now: u64) {
    for token in &mut r.simple_pair.tokens {
        if token.enrollment_id == machine && token.revoked_at.is_none() {
            token.revoked_at = Some(now);
        }
    }
}

pub fn unpair(dir: &Path, machine: &str, now: u64) -> Result<()> {
    let mut store = Store::open(dir)?;
    store.pairing_transaction(0, |r| {
        if !r.machines.iter().any(|m| m.id == machine) {
            return Err(Error::Invalid);
        }
        let now = now.max(r.latest);
        r.latest = now;
        revoke(r, machine, now);
        r.simple_pair.audit.push(Audit {
            time: now,
            action: "unpaired".into(),
            machine_id: machine.into(),
            replaced: false,
        });
        Ok(())
    })
}

pub(crate) fn unpair_token(dir: &Path, authorization: Option<&str>, now: u64) -> Result<()> {
    let secret = authorization
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .ok_or(Error::Invalid)?;
    let secret = Secret::new(secret.into());
    let mut store = Store::open(dir)?;
    store.pairing_transaction(0, |r| {
        let machine = r
            .paired_token(&secret)
            .ok_or(Error::Invalid)?
            .enrollment_id
            .clone();
        let now = now.max(r.latest);
        r.latest = now;
        revoke(r, &machine, now);
        r.simple_pair.audit.push(Audit {
            time: now,
            action: "unpaired".into(),
            machine_id: machine,
            replaced: false,
        });
        Ok(())
    })
}

impl Registry {
    pub(crate) fn paired_token(&self, secret: &Secret) -> Option<&Bearer> {
        let hash = secret.hash();
        self.simple_pair
            .tokens
            .iter()
            .find(|b| constant_eq(&hash, &b.sha256) && b.revoked_at.is_none())
    }
    pub(crate) fn validate_simple_pair(&self) -> Result<()> {
        let s = &self.simple_pair;
        if s.pending.as_ref().is_some_and(|p| p.wrong >= 3)
            || s.burns.len() > 6
            || s.burns.iter().any(|t| *t > self.latest)
            || s.audit.iter().any(|a| {
                a.time > self.latest
                    || !self.machines.iter().any(|m| m.id == a.machine_id)
                    || !matches!(a.action.as_str(), "paired" | "unpaired")
            })
        {
            return Err(Error::Invalid);
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut hashes = std::collections::BTreeSet::new();
        let mut live = std::collections::BTreeSet::new();
        for b in &s.tokens {
            if b.kind != Kind::Push
                || b.generation != 1
                || b.valid_until != u64::MAX
                || b.valid_from > self.latest
                || b.revoked_at.is_some_and(|t| t > self.latest)
                || !self.machines.iter().any(|m| m.id == b.enrollment_id)
                || !ids.insert(&b.credential_id)
                || !hashes.insert(b.sha256)
                || (b.revoked_at.is_none() && !live.insert(&b.enrollment_id))
            {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub(crate) fn simple_pair_follows(&self, old: &Self) -> Result<()> {
        if !self.simple_pair.audit.starts_with(&old.simple_pair.audit) {
            return Err(Error::Terminal);
        }
        for previous in &old.simple_pair.tokens {
            let next = self
                .simple_pair
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub url: String,
    pub token: String,
    pub machine_id: String,
}
impl Config {
    pub fn path() -> std::result::Result<PathBuf, String> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .ok_or("pairing needs XDG_CONFIG_HOME or HOME")?;
        Ok(base.join("semon/pair.json"))
    }
    pub fn load() -> std::result::Result<Self, String> {
        Self::load_at(&Self::path()?)
    }
    fn load_at(path: &Path) -> std::result::Result<Self, String> {
        use std::io::Read;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| format!("{}: {e}; run semon pair URL CODE", path.display()))?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.len() > 8192
        {
            return Err("pair.json must be a regular mode-0600 file under 8 KiB".into());
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(8193)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let config: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        crate::check_url(&config.url)?;
        crate::Token::new(&config.token)?;
        if !semon_sessions::machine_id::valid(&config.machine_id) {
            return Err("invalid paired machine ID".into());
        }
        Ok(config)
    }
    pub fn save_at(&self, path: &Path) -> std::result::Result<(), String> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let parent = path.parent().ok_or("pair.json has no parent")?;
        crate::create_private_dir(parent).map_err(|e| e.to_string())?;
        let stage = parent.join(format!(
            ".pair-{}",
            crate::enrollment::random_id().map_err(|e| e.to_string())?
        ));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&stage)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            std::fs::rename(&stage, path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        let _ = std::fs::remove_file(stage);
        result.map_err(|e: std::io::Error| e.to_string())
    }
}
fn client() -> std::result::Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}
#[cfg(not(target_os = "macos"))]
fn local_hostname() -> std::result::Result<String, String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|name| name.trim().to_owned())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn local_hostname() -> std::result::Result<String, String> {
    let mut buffer = [0u8; 256];
    // SAFETY: buffer is writable for the supplied length; gethostname retains no pointer.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let end = buffer
        .iter()
        .position(|b| *b == 0)
        .ok_or("hostname is too long")?;
    String::from_utf8(buffer[..end].to_vec()).map_err(|e| e.to_string())
}

pub fn pair_machine(url: &str, code: &str) -> std::result::Result<String, String> {
    use std::io::Read;
    let url = crate::check_url(url)?;
    crate::pairing::parse_code(code).map_err(|_| "invalid pairing code")?;
    let machine_id = semon_sessions::machine_id::load_or_create().map_err(|e| e.to_string())?;
    let hostname = local_hostname()?;
    let response = client()?
        .post(format!("{url}/v1/pair"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(&Request {
                version: 1,
                code: code.into(),
                machine_id: machine_id.clone(),
                hostname,
            })
            .map_err(|e| e.to_string())?,
        )
        .send()
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("pairing refused ({})", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8192 {
        return Err("pairing response exceeds 8 KiB".into());
    }
    let reply: Response = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if reply.version != 1 || reply.machine_id != machine_id {
        return Err("invalid pairing response".into());
    }
    crate::Token::new(&reply.token)?;
    Config {
        url: url.clone(),
        token: reply.token,
        machine_id: machine_id.clone(),
    }
    .save_at(&Config::path()?)?;
    Ok(format!("Paired with {url} as {machine_id}."))
}
pub fn unpair_machine() -> std::result::Result<(), String> {
    let path = Config::path()?;
    let config = Config::load()?;
    let result = client().and_then(|c| {
        c.post(format!("{}/v1/unpair", config.url))
            .bearer_auth(config.token)
            .body("")
            .send()
            .map_err(|e| e.to_string())
    });
    match result {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => eprintln!(
            "warning: receiver did not confirm revocation ({}); revoke {} on the receiver",
            response.status(),
            config.machine_id
        ),
        Err(e) => eprintln!(
            "warning: receiver unreachable ({e}); revoke {} on the receiver",
            config.machine_id
        ),
    }
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
