//! Durable origin renewal attempts. Network delivery is reserved locally before
//! dispatch; uncertain attempts recover once, and never silently retry commit.
use super::*;
use crate::renewal_wire::{self as wire, Action, Begun, Delivered, Generation};

const PENDING: &str = "renewal.json";
const BLOCKED: &str = "receiver renewal recovery must complete before application traffic";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    version: u32,
    begun: Begun,
    application_key: String,
    old_generation: u64,
    old_valid_until: u64,
    state: State,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.application_key);
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Prepared,
    CommitReserved,
    RecoverReserved,
    Delivered,
    Installed,
    Confirmed,
}

fn optional_attempt(dir: &Directory) -> Result<Option<Attempt>> {
    if !dir
        .checked
        .names()
        .map_err(|_| ERROR)?
        .iter()
        .any(|n| n == Path::new(PENDING))
    {
        return Ok(None);
    }
    let attempt: Attempt = serde_json::from_slice(&dir.read(PENDING)?).map_err(|_| ERROR)?;
    if attempt.version != 1 || attempt.old_generation == 0 {
        return Err(ERROR.into());
    }
    crate::provision_wire::decode_fixed::<32>(&attempt.application_key).map_err(|_| ERROR)?;
    Ok(Some(attempt))
}
fn replace(dir: &Directory, name: &str, bytes: &[u8]) -> Result<()> {
    let stage = format!(
        "update-{}",
        crate::enrollment::random_id().map_err(|_| ERROR)?
    );
    dir.write(&stage, bytes)?;
    checkpoint("file-fsync")?;
    let from = std::ffi::CString::new(stage).map_err(|_| ERROR)?;
    let to = std::ffi::CString::new(name).map_err(|_| ERROR)?;
    checkpoint("before-rename")?;
    // SAFETY: single-component names under the checked directory descriptor.
    if unsafe {
        libc::renameat(
            dir.fd.as_raw_fd(),
            from.as_ptr(),
            dir.fd.as_raw_fd(),
            to.as_ptr(),
        )
    } != 0
    {
        return Err(ERROR.into());
    }
    checkpoint("after-rename")?;
    dir.checked.sync().map_err(|_| ERROR)?;
    checkpoint("directory-fsync")
}
fn save(dir: &Directory, attempt: &Attempt) -> Result<()> {
    replace(
        dir,
        PENDING,
        &serde_json::to_vec(attempt).map_err(|_| ERROR)?,
    )
}
pub(super) fn ready(dir: &Directory) -> Result<()> {
    if optional_attempt(dir)?
        .is_some_and(|a| !matches!(a.state, State::Installed | State::Confirmed))
    {
        return Err(BLOCKED.into());
    }
    Ok(())
}

/// The notice boundary is inclusive; attempts begin strictly inside the window.
pub fn window(valid_until: u64, now: u64) -> (bool, bool) {
    let remaining = valid_until.saturating_sub(now);
    (
        remaining <= wire::WINDOW,
        now < valid_until && remaining < wire::WINDOW,
    )
}

/// Owns the install lock throughout receipt reconciliation and publication.
/// This prevents both competing renewals and publication after disconnect wins.
pub struct Renewal {
    _lock: InstallLock,
    dir: Directory,
    name: String,
    config: Configuration,
    key: EcdsaKeyPair,
    attempt: Option<Attempt>,
}
impl Renewal {
    pub fn open(base: &Path, endpoint: &str) -> Result<Self> {
        let endpoint = receiver_endpoint(endpoint)?;
        let root = Directory::open(base)?;
        let lock = root.install_lock()?;
        let name = custody_name(&endpoint);
        let dir = root.child(&name)?;
        if dir.disabled()? {
            return Err("receiver is disconnected on this machine".into());
        }
        let (config, key) = read_configuration(&selected_directory(&dir)?, &name)?;
        let attempt = optional_attempt(&dir)?;
        Ok(Self {
            _lock: lock,
            dir,
            name,
            config,
            key,
            attempt,
        })
    }
    pub fn configuration(&self) -> &Configuration {
        &self.config
    }
    pub fn pending(&self) -> bool {
        self.attempt
            .as_ref()
            .is_some_and(|a| !matches!(a.state, State::Confirmed))
    }
    pub fn credentials(&self, now: u64) -> Result<Vec<wire::Credential>> {
        let generation = self
            .attempt
            .as_ref()
            .filter(|a| !matches!(a.state, State::Confirmed))
            .map_or(self.config.generation, |a| a.old_generation);
        let dir = self.dir.child(&format!("generation-{generation}"))?;
        let (config, _) = read_configuration(&dir, &self.name)?;
        if now >= config.scope.valid_until {
            return Err("receiver credentials expired; run semon connect --to URL".into());
        }
        let bearer = dir.read("mirror-push.secret")?;
        if crate::wire::sha256_hex(&bearer) != config.credential_sha256 {
            return Err(ERROR.into());
        }
        Ok(vec![wire::Credential {
            kind: crate::credentials::Kind::MirrorPush,
            generation,
            bearer: std::str::from_utf8(&bearer).map_err(|_| ERROR)?.into(),
        }])
    }
    /// Stage the random application key before any finish can be sent.
    pub fn prepare(&mut self, begun: Begun, now: u64) -> Result<()> {
        if self.pending()
            || now >= self.config.scope.valid_until
            || now >= begun.expires_at
            || begun.version != 1
            || begun.enrollment_id != self.config.enrollment_id
            || begun.generations
                != vec![Generation {
                    kind: crate::credentials::Kind::MirrorPush,
                    generation: self.config.generation.checked_add(1).ok_or(ERROR)?,
                }]
            || !begun.enable_authorization_ids.is_empty()
        {
            return Err(ERROR.into());
        }
        wire::identifier(&begun.renewal_id).map_err(|_| ERROR)?;
        crate::provision_wire::decode_fixed::<32>(&begun.receiver_nonce).map_err(|_| ERROR)?;
        let mut application = Zeroizing::new([0; 32]);
        getrandom::fill(application.as_mut()).map_err(|_| ERROR)?;
        let attempt = Attempt {
            version: 1,
            begun,
            application_key: crate::provision_wire::encode(application.as_ref()),
            old_generation: self.config.generation,
            old_valid_until: self.config.scope.valid_until,
            state: State::Prepared,
        };
        save(&self.dir, &attempt)?;
        self.attempt = Some(attempt);
        Ok(())
    }
    /// A request's action is durable before dispatch. After uncertain recovery,
    /// refuse further delivery; the receiver permits only one recovery reservation.
    pub fn finish(&mut self, exporter: &[u8; 32], now: u64) -> Result<wire::Finish> {
        let credentials = self.credentials(now)?;
        let a = self.attempt.as_mut().ok_or(ERROR)?;
        let action = match a.state {
            State::Prepared => Action::Commit,
            State::CommitReserved => Action::Recover,
            _ => return Err(BLOCKED.into()),
        };
        if now >= a.old_valid_until {
            return Err(BLOCKED.into());
        }
        let application = Zeroizing::new(
            crate::provision_wire::decode_fixed::<32>(&a.application_key).map_err(|_| ERROR)?,
        );
        let message = wire::possession_message(
            &self.config.receiver_id,
            &self.config.endpoint,
            &a.begun,
            &wire::identities(&credentials).map_err(|_| ERROR)?,
            exporter,
            action,
            &application,
        )
        .map_err(|_| ERROR)?;
        let signature = self
            .key
            .sign(&ring::rand::SystemRandom::new(), &message)
            .map_err(|_| ERROR)?;
        a.state = if action == Action::Commit {
            State::CommitReserved
        } else {
            State::RecoverReserved
        };
        save(&self.dir, a)?;
        Ok(wire::Finish {
            version: 1,
            enrollment_id: self.config.enrollment_id.clone(),
            renewal_id: a.begun.renewal_id.clone(),
            action,
            credentials,
            application_key: a.application_key.clone(),
            proof: crate::provision_wire::encode(signature.as_ref()),
        })
    }
    /// Verify the complete delivery, stage all files, then atomically select it.
    pub fn delivered(&mut self, delivered: &Delivered, now: u64) -> Result<()> {
        let a = self.attempt.as_ref().ok_or(ERROR)?;
        if !matches!(a.state, State::CommitReserved | State::RecoverReserved)
            || delivered.version != 1
            || delivered.enrollment_id != a.begun.enrollment_id
            || delivered.renewal_id != a.begun.renewal_id
            || delivered.credentials.len() != 1
        {
            return Err(ERROR.into());
        }
        let c = &delivered.credentials[0];
        let target = &a.begun.generations[0];
        wire::identifier(&c.credential_id).map_err(|_| ERROR)?;
        let application = Zeroizing::new(
            crate::provision_wire::decode_fixed::<32>(&a.application_key).map_err(|_| ERROR)?,
        );
        if c.kind != target.kind
            || c.generation != target.generation
            || wire::bearer(
                &application,
                &delivered.enrollment_id,
                &delivered.renewal_id,
                target,
            )
            .expose()
                != c.bearer
            || c.valid_from > now
            || now >= c.valid_until
            || c.valid_until.checked_sub(c.valid_from) != Some(7_776_000)
        {
            return Err(ERROR.into());
        }
        let old = self
            .dir
            .child(&format!("generation-{}", a.old_generation))?;
        let mut config: Configuration =
            serde_json::from_slice(&serde_json::to_vec(&self.config).map_err(|_| ERROR)?)
                .map_err(|_| ERROR)?;
        config.generation = c.generation;
        config.scope.valid_until = c.valid_until;
        config.credential_sha256 = crate::wire::sha256_hex(c.bearer.as_bytes());
        let certificate = delivered
            .client_certificate
            .as_deref()
            .map(decode_certificate)
            .transpose()?;
        if certificate.is_some() != self.config.client_certificate_sha256.is_some() {
            return Err(ERROR.into());
        }
        validate_certificate(
            &config.scope,
            certificate.as_deref(),
            self.key.public_key().as_ref(),
            now,
        )?;
        config.client_certificate_sha256 = certificate.as_deref().map(crate::wire::sha256_hex);
        let signature = self
            .key
            .sign(
                &ring::rand::SystemRandom::new(),
                &serde_json::to_vec(&config).map_err(|_| ERROR)?,
            )
            .map_err(|_| ERROR)?;
        let signed = Signed {
            configuration: config,
            public_key: crate::provision_wire::encode(self.key.public_key().as_ref()),
            signature: crate::provision_wire::encode(signature.as_ref()),
        };
        let stage_name = format!(
            "stage-{}",
            crate::enrollment::random_id().map_err(|_| ERROR)?
        );
        let stage = self.dir.create_child(&stage_name)?;
        for (name, bytes) in [
            ("source-consent.json", old.read("source-consent.json")?),
            ("origin-key.pk8", old.read("origin-key.pk8")?),
            (
                "mirror-push.secret",
                Zeroizing::new(c.bearer.as_bytes().to_vec()),
            ),
            (
                "enrollment.json",
                Zeroizing::new(serde_json::to_vec(&signed).map_err(|_| ERROR)?),
            ),
        ] {
            if name == "source-consent.json"
                && crate::wire::sha256_hex(&bytes) != self.config.consent_sha256
            {
                return Err(ERROR.into());
            }
            checkpoint("before-stage-file")?;
            stage.write(name, &bytes)?;
            checkpoint("after-stage-file")?;
        }
        if let Some(certificate) = certificate {
            stage.write("client-certificate.der", &certificate)?;
        }
        stage.checked.sync().map_err(|_| ERROR)?;
        checkpoint("stage-directory-fsync")?;
        let from = std::ffi::CString::new(stage_name).map_err(|_| ERROR)?;
        let to =
            std::ffi::CString::new(format!("generation-{}", c.generation)).map_err(|_| ERROR)?;
        // SAFETY: checked, single-component names in one locked private directory.
        if unsafe {
            libc::renameat(
                self.dir.fd.as_raw_fd(),
                from.as_ptr(),
                self.dir.fd.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(ERROR.into());
        }
        checkpoint("generation-rename")?;
        self.dir.checked.sync().map_err(|_| ERROR)?;
        checkpoint("generation-directory-fsync")?;
        self.attempt.as_mut().ok_or(ERROR)?.state = State::Delivered;
        save(&self.dir, self.attempt.as_ref().ok_or(ERROR)?)?;
        self.reconcile()
    }
    /// Reopening a verified, durable delivery never consumes another wire delivery.
    pub fn reconcile(&mut self) -> Result<()> {
        let a = self.attempt.as_mut().ok_or(ERROR)?;
        if self.dir.disabled()? {
            return Err(BLOCKED.into());
        }
        match a.state {
            State::Delivered => {
                let generation = a.begun.generations[0].generation;
                let name = format!("generation-{generation}");
                let (config, _) = read_configuration(&self.dir.child(&name)?, &self.name)?;
                if config.generation != generation || config.enrollment_id != a.begun.enrollment_id
                {
                    return Err(ERROR.into());
                }
                replace(&self.dir, "selected", name.as_bytes())?;
                a.state = State::Installed;
                save(&self.dir, a)?;
                self.config = config;
                Ok(())
            }
            State::Installed | State::Confirmed => Ok(()),
            _ => Err(BLOCKED.into()),
        }
    }
    /// Call only after authenticated application success with the selected generation.
    /// Retained old files are evidence, never an automatic fallback.
    pub fn confirm(&mut self, generation: u64) -> Result<()> {
        let a = self.attempt.as_mut().ok_or(ERROR)?;
        if a.state != State::Installed || generation != a.begun.generations[0].generation {
            return Err(ERROR.into());
        }
        a.state = State::Confirmed;
        save(&self.dir, a)
    }
}

#[cfg(test)]
thread_local! { static FAULT: std::cell::RefCell<Option<(&'static str, usize)>> = const { std::cell::RefCell::new(None) }; }
fn checkpoint(_point: &'static str) -> Result<()> {
    #[cfg(test)]
    FAULT.with(|fault| {
        let mut fault = fault.borrow_mut();
        if let Some((point, remaining)) = fault.as_mut() {
            if *point == _point {
                if *remaining == 0 {
                    return Err("injected renewal interruption".into());
                }
                *remaining -= 1;
            }
        }
        Ok(())
    })?;
    Ok(())
}
