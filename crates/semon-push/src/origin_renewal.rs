//! Durable origin renewal attempts. Network delivery is reserved locally before
//! dispatch; uncertain attempts recover once, and never silently retry commit.
use super::*;
use crate::renewal_wire::{self as wire, Action, Begun, Delivered, Generation};

#[cfg(test)]
#[path = "origin_renewal_tests.rs"]
mod tests;

enum Response {
    Begun(Begun),
    Delivered(Delivered),
}
#[cfg(test)]
type TestExchange = dyn FnMut(&mut Renewal, bool, &dyn Fn() -> Result<u64>) -> Result<Response>;

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
    recovery_until: Option<u64>,
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
    Abandoned,
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
    if attempt.version != 1
        || attempt.old_generation == 0
        || attempt.begun.version != 1
        || attempt.begun.generations
            != vec![Generation {
                kind: crate::credentials::Kind::MirrorPush,
                generation: attempt.old_generation.checked_add(1).ok_or(ERROR)?,
            }]
        || !attempt.begun.enable_authorization_ids.is_empty()
    {
        return Err(ERROR.into());
    }
    wire::identifier(&attempt.begun.enrollment_id).map_err(|_| ERROR)?;
    wire::identifier(&attempt.begun.renewal_id).map_err(|_| ERROR)?;
    crate::provision_wire::decode_fixed::<32>(&attempt.begun.receiver_nonce).map_err(|_| ERROR)?;
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
    if let Some(a) = optional_attempt(dir)? {
        let target = format!("generation-{}", a.begun.generations[0].generation);
        if a.state == State::Delivered
            || (matches!(a.state, State::CommitReserved | State::RecoverReserved)
                && dir
                    .checked
                    .names()
                    .map_err(|_| ERROR)?
                    .iter()
                    .any(|n| n == Path::new(&target)))
        {
            return Err(BLOCKED.into());
        }
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
    #[cfg(test)]
    exchange: Option<Box<TestExchange>>,
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
        let mut renewal = Self {
            _lock: lock,
            dir,
            name,
            config,
            key,
            attempt,
            #[cfg(test)]
            exchange: None,
        };
        if renewal.attempt.is_some() {
            // Complete any durable local delivery before opening network traffic.
            renewal.reconcile_local()?;
        }
        Ok(renewal)
    }
    pub fn configuration(&self) -> &Configuration {
        &self.config
    }
    pub(crate) fn awaiting_confirmation(&self) -> bool {
        self.attempt
            .as_ref()
            .is_some_and(|a| a.state == State::Installed)
    }
    /// One private TLS connection per request, guarded through socket destruction.
    /// The guard is declared first so disconnect drains the complete request.
    fn connection(
        &self,
        clock: &dyn Fn() -> Result<u64>,
    ) -> Result<(Delivery, crate::connect::Connection)> {
        let guard = delivery_lock(&self.dir, libc::LOCK_SH)?;
        if self.dir.disabled()? {
            return Err(BLOCKED.into());
        }
        let generation = self
            .attempt
            .as_ref()
            .filter(|a| !matches!(a.state, State::Confirmed))
            .map_or(self.config.generation, |a| a.old_generation);
        let dir = self.dir.child(&format!("generation-{generation}"))?;
        let (config, key) = read_configuration(&dir, &self.name)?;
        let certificate = config
            .client_certificate_sha256
            .as_ref()
            .map(|hash| {
                let bytes = dir.read("client-certificate.der")?;
                if crate::wire::sha256_hex(&bytes) != *hash {
                    return Err(ERROR.to_owned());
                }
                validate_certificate(
                    &config.scope,
                    Some(&bytes),
                    key.public_key().as_ref(),
                    clock()?,
                )?;
                Ok(bytes.to_vec())
            })
            .transpose()?;
        let identity = certificate
            .map(|certificate| {
                dir.read("origin-key.pk8")
                    .map(|private| (certificate, private))
            })
            .transpose()?;
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let connection =
            crate::connect::Connection::open_identity(&config.endpoint, roots, identity)?;
        Ok((guard, connection))
    }
    /// Reconcile an existing attempt before allocating a new begin. Wire errors
    /// are temporary; callers retain valid old application authority and notices.
    pub fn renew(&mut self) -> Result<()> {
        self.renew_with_clock(&now)
    }
    fn renew_with_clock(&mut self, clock: &dyn Fn() -> Result<u64>) -> Result<()> {
        #[cfg(test)]
        if let Some(mut exchange) = self.exchange.take() {
            let result = self.renew_using(clock, &mut exchange);
            self.exchange = Some(exchange);
            return result;
        }
        self.renew_using(clock, &mut Self::exchange)
    }
    fn exchange(&mut self, begin: bool, clock: &dyn Fn() -> Result<u64>) -> Result<Response> {
        let (_guard, mut connection) = self.connection(clock)?;
        if begin {
            let message = wire::Begin {
                version: 1,
                enrollment_id: self.config.enrollment_id.clone(),
                credentials: self.credentials(clock()?)?,
            };
            connection
                .post("/v1/renew/begin", &message)
                .map(Response::Begun)
        } else {
            let finish = self.finish(&connection.exporter, clock()?)?;
            connection
                .post("/v1/renew/finish", &finish)
                .map(Response::Delivered)
        }
    }
    fn renew_using(
        &mut self,
        clock: &dyn Fn() -> Result<u64>,
        exchange: &mut impl FnMut(&mut Self, bool, &dyn Fn() -> Result<u64>) -> Result<Response>,
    ) -> Result<()> {
        if self.attempt.is_some() {
            self.reconcile_local()?;
        }
        self.expire(clock()?)?;
        if self
            .attempt
            .as_ref()
            .is_some_and(|a| a.state == State::Installed)
        {
            return Ok(());
        }
        if !self.pending() {
            let Response::Begun(begun) = exchange(self, true, clock)? else {
                return Err(ERROR.into());
            };
            self.prepare(begun, clock()?)?;
        }
        match exchange(self, false, clock) {
            Ok(Response::Delivered(delivery)) => self.delivered_with_clock(&delivery, clock),
            Ok(Response::Begun(_)) => Err(ERROR.into()),
            Err(error) => {
                // A refused commit is indistinguishable from lost delivery.
                // Reserve the one recovery on the next call; never resend commit.
                if self
                    .attempt
                    .as_ref()
                    .is_some_and(|a| a.state == State::RecoverReserved)
                {
                    self.abandon()?;
                }
                Err(error)
            }
        }
    }
    /// Retain terminal evidence before another begin can replace the active record.
    fn abandon(&mut self) -> Result<()> {
        let a = self.attempt.as_mut().ok_or(ERROR)?;
        a.state = State::Abandoned;
        save(&self.dir, a)?;
        let archive = format!("renewal-ended-{}.json", a.begun.renewal_id);
        replace(
            &self.dir,
            &archive,
            &serde_json::to_vec(a).map_err(|_| ERROR)?,
        )
    }
    fn expire(&mut self, now: u64) -> Result<()> {
        if self.attempt.as_ref().is_some_and(|a| match a.state {
            State::Prepared => now >= a.begun.expires_at,
            State::CommitReserved => now >= a.recovery_until.unwrap_or(a.old_valid_until),
            State::RecoverReserved => true,
            _ => false,
        }) {
            self.abandon()?;
        }
        Ok(())
    }
    pub fn pending(&self) -> bool {
        self.attempt
            .as_ref()
            .is_some_and(|a| !matches!(a.state, State::Confirmed | State::Abandoned))
    }
    pub fn credentials(&self, now: u64) -> Result<Vec<wire::Credential>> {
        if self
            .attempt
            .as_ref()
            .is_some_and(|a| matches!(a.state, State::Installed))
        {
            return Err("renewed generation awaits application confirmation".into());
        }
        if self.attempt.as_ref().is_some_and(|a| {
            a.recovery_until.is_some_and(|until| now >= until)
                && !matches!(a.state, State::Confirmed | State::Abandoned)
        }) {
            return Err(BLOCKED.into());
        }
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
    fn prepare(&mut self, begun: Begun, now: u64) -> Result<()> {
        self.expire(now)?;
        if self
            .attempt
            .as_ref()
            .is_some_and(|a| a.state == State::Abandoned)
        {
            self.abandon()?;
        }
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
            recovery_until: None,
            state: State::Prepared,
        };
        save(&self.dir, &attempt)?;
        self.attempt = Some(attempt);
        Ok(())
    }
    /// A request's action is durable before dispatch. After uncertain recovery,
    /// refuse further delivery; the receiver permits only one recovery reservation.
    fn finish(&mut self, exporter: &[u8; 32], now: u64) -> Result<wire::Finish> {
        self.expire(now)?;
        let credentials = self.credentials(now)?;
        let a = self.attempt.as_mut().ok_or(ERROR)?;
        let action = match a.state {
            State::Prepared => Action::Commit,
            State::CommitReserved => Action::Recover,
            _ => return Err(BLOCKED.into()),
        };
        if now >= a.old_valid_until
            || (action == Action::Commit && now >= a.begun.expires_at)
            || (action == Action::Recover && now >= a.recovery_until.ok_or(ERROR)?)
        {
            return Err(BLOCKED.into());
        }
        if action == Action::Commit {
            // Receiver commit cannot precede this reservation. This lower bound
            // is deliberately conservative when the delivery response is lost.
            a.recovery_until = Some(now.saturating_add(wire::OVERLAP).min(a.old_valid_until));
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
    fn delivered_with_clock(
        &mut self,
        delivery: &Delivered,
        clock: &dyn Fn() -> Result<u64>,
    ) -> Result<()> {
        self.delivered(delivery, clock()?)
    }
    /// Verify the complete delivery, stage all files, then atomically select it.
    fn delivered(&mut self, delivered: &Delivered, now: u64) -> Result<()> {
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
    fn reconcile_local(&mut self) -> Result<()> {
        let a = self.attempt.as_ref().ok_or(ERROR)?;
        let target = a.begun.generations.first().ok_or(ERROR)?;
        let generation_name = format!("generation-{}", target.generation);
        let exists = self
            .dir
            .checked
            .names()
            .map_err(|_| ERROR)?
            .iter()
            .any(|n| n == Path::new(&generation_name));
        if exists && matches!(a.state, State::CommitReserved | State::RecoverReserved) {
            let dir = self.dir.child(&generation_name)?;
            let (config, _) = read_configuration(&dir, &self.name)?;
            let old = self
                .dir
                .child(&format!("generation-{}", a.old_generation))?;
            let (previous, _) = read_configuration(&old, &self.name)?;
            let application = Zeroizing::new(
                crate::provision_wire::decode_fixed::<32>(&a.application_key).map_err(|_| ERROR)?,
            );
            let expected = wire::bearer(
                &application,
                &a.begun.enrollment_id,
                &a.begun.renewal_id,
                target,
            );
            if config.generation != target.generation
                || config.enrollment_id != previous.enrollment_id
                || config.consent_sha256 != previous.consent_sha256
                || config.credential_sha256 != crate::wire::sha256_hex(expected.expose().as_bytes())
                || dir.read("mirror-push.secret")?.as_slice() != expected.expose().as_bytes()
                || dir.read("source-consent.json")?.as_slice()
                    != old.read("source-consent.json")?.as_slice()
                || dir.read("origin-key.pk8")?.as_slice() != old.read("origin-key.pk8")?.as_slice()
            {
                return Err(ERROR.into());
            }
            dir.checked.sync().map_err(|_| ERROR)?;
            self.dir.checked.sync().map_err(|_| ERROR)?;
            self.attempt.as_mut().ok_or(ERROR)?.state = State::Delivered;
            save(&self.dir, self.attempt.as_ref().ok_or(ERROR)?)?;
        }
        if self.attempt.as_ref().is_some_and(|a| {
            matches!(
                a.state,
                State::Delivered | State::Installed | State::Confirmed
            )
        }) {
            self.reconcile()?;
        }
        Ok(())
    }
    /// Reopening a verified, durable delivery never consumes another wire delivery.
    fn reconcile(&mut self) -> Result<()> {
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
            State::Installed | State::Confirmed => {
                if self.config.generation != a.begun.generations[0].generation {
                    return Err(ERROR.into());
                }
                Ok(())
            }
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
    FAULT.with(|fault| -> Result<()> {
        let mut fault = fault.borrow_mut();
        if let Some((point, remaining)) = fault.as_mut()
            && *point == _point
        {
            if *remaining == 0 {
                return Err("injected renewal interruption".into());
            }
            *remaining -= 1;
        }
        Ok(())
    })?;
    Ok(())
}
