//! Receiver composition for the reverse body channel (Part of #594).
use std::{path::Path, sync::Mutex};

use crate::{
    body_broker::{Broker, Config, EnrollmentSource},
    body_enrollment::RevocationWatcher,
    body_wire::{Reason, Result},
    credentials::Kind,
    enrollment::Registry,
    origin_read::{EnrollmentScope, ReadClock},
};

pub(crate) struct SystemClock;
impl ReadClock for SystemClock {
    fn now_millis(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(u64::MAX, |time| time.as_millis() as u64)
    }
}
impl crate::body_cache::Clock for SystemClock {
    fn now(&self) -> u64 {
        self.now_millis() / 1000
    }
}
pub(crate) static CLOCK: SystemClock = SystemClock;
struct NoAuthority;
impl EnrollmentSource for NoAuthority {
    fn authenticated_enrollment(
        &self,
        _: crate::origin_read::EnrollmentKind,
    ) -> Result<EnrollmentScope> {
        Err(Reason::Forbidden)
    }
    fn is_current(&self, _: &EnrollmentScope) -> bool {
        false
    }
}

pub struct BodyHost<'a> {
    pub broker: Broker<'a>,
    watcher: Mutex<RevocationWatcher>,
    // Retain exclusive cache/audit custody for the receiver lifetime. Body
    // transport alone never grants a cache admission or creates an append lease.
    _custody: Mutex<Option<HostCustody>>,
}
struct HostCustody {
    _cache: crate::body_cache::BodyCache<SystemClock>,
    _audit: crate::receiver_audit::AuditStore<SystemClock>,
    _signing: crate::body_assertion::MemoryKey,
    _acls: crate::receiver_acl::Acls,
}
impl<'a> BodyHost<'a> {
    /// No directory, key, audit or cache is created for an unconfigured receiver.
    pub fn configured(
        dir: &Path,
        config: Config,
        clock: &'a (dyn ReadClock + Sync),
    ) -> Result<Option<Self>> {
        let registry = Registry::read_current(dir).map_err(|_| Reason::Unavailable)?;
        if !registry.enrollments().iter().any(|enrollment| {
            enrollment
                .credential_kinds
                .iter()
                .any(|kind| kind.kind == Kind::BodyChannel)
        }) || !dir
            .join(crate::receiver_custody::CLIENT_TRUST)
            .try_exists()
            .map_err(|_| Reason::Unavailable)?
        {
            return Ok(None);
        }
        let custody = crate::receiver_custody::ReceiverCustody::open_existing(dir)
            .map_err(|_| Reason::Unavailable)?;
        let signing = custody
            .load_signing_key()
            .map_err(|_| Reason::Unavailable)?;
        let cache = crate::body_cache::BodyCache::open(
            &custody.cache_dir(),
            &custody.cache_key_path(),
            SystemClock,
            crate::body_cache::Limits::default(),
        )
        .map_err(|_| Reason::Unavailable)?;
        let audit = crate::receiver_audit::AuditStore::open(
            &custody.audit_dir(),
            SystemClock,
            crate::receiver_audit::MAX_BYTES,
        )
        .map_err(|_| Reason::Unavailable)?;
        Ok(Some(Self {
            _custody: Mutex::new(Some(HostCustody {
                _cache: cache,
                _audit: audit,
                _signing: signing,
                _acls: crate::receiver_acl::Acls::new(custody.acl_dir()),
            })),
            broker: Broker::new(config, clock)?,
            watcher: Mutex::new(RevocationWatcher::new(dir)?),
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_test(dir: &Path, clock: &'a (dyn ReadClock + Sync)) -> Self {
        Self {
            broker: Broker::new(Config::default(), clock).unwrap(),
            watcher: Mutex::new(RevocationWatcher::new(dir).unwrap()),
            _custody: Mutex::new(None),
        }
    }

    /// Called before authentication too, so a revoked caller still wakes and
    /// clears captured work. Registry-backed work checks its own authority.
    pub fn check_revision(&self) -> Result<()> {
        self.maintain(&NoAuthority)
    }

    pub fn maintain(&self, source: &dyn EnrollmentSource) -> Result<()> {
        self.watcher
            .lock()
            .map_err(|_| Reason::Unavailable)?
            .notify_if_revision_changed(source, &self.broker)?;
        Ok(())
    }

    /// Delivery hosts retain the original owner source and sweep every active
    /// service at each authorization/release decision, including registry errors.
    pub fn sweep_owner<C: crate::body_cache::Clock>(
        &self,
        service: &mut crate::append_receiver::AppendService<'_, '_, C>,
        scope: &EnrollmentScope,
        owner: &dyn EnrollmentSource,
    ) -> Result<()> {
        let revision = self.maintain(owner);
        service.sweep_owner(scope, owner)?;
        revision
    }
}
