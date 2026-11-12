//! Host-driven registry revision notifications for the body broker (Part of #594).
//!
//! Request authentication and host-driven revocation (Part of #594).
//! Pending work retains its own credential handle, so a notifying request
//! cannot substitute its authority for another owner's. No transport is installed.
use std::path::{Path, PathBuf};

use crate::{
    body_broker::{Broker, EnrollmentSource},
    body_wire::{Reason, Result},
    enrollment::{self, Registry},
};

/// Body-kind authority for one request on an already verified connection.
/// Construct a fresh source for every request, including keep-alive requests.
/// Pending authority survives unrelated registry changes until its credential ends.
pub struct RegistryEnrollmentSource {
    scope: crate::origin_read::EnrollmentScope,
    authority: crate::serve::CredentialHandle,
}
impl RegistryEnrollmentSource {
    pub fn authenticate(
        authenticator: &crate::serve::RegistryAuthenticator,
        connection: &mut crate::serve::ConnectionAuthority,
        request_credential: Option<&str>,
        now: u64,
    ) -> Result<Self> {
        let authenticated = authenticator
            .authenticated_enrollment(
                connection,
                request_credential,
                crate::credentials::Kind::BodyChannel,
                now,
            )
            .ok_or(Reason::Forbidden)?;
        let scope = crate::origin_read::EnrollmentScope::try_from(&authenticated)
            .map_err(|_| Reason::Forbidden)?;
        Ok(Self {
            scope,
            authority: authenticated.authority,
        })
    }
}
impl EnrollmentSource for RegistryEnrollmentSource {
    fn authenticated_enrollment(
        &self,
        required_kind: crate::origin_read::EnrollmentKind,
    ) -> Result<crate::origin_read::EnrollmentScope> {
        if required_kind != crate::origin_read::EnrollmentKind::BodyChannel {
            return Err(Reason::Forbidden);
        }
        if !self.is_current(&self.scope) {
            return Err(Reason::Revoked);
        }
        Ok(self.scope.clone())
    }
    fn is_current(&self, scope: &crate::origin_read::EnrollmentScope) -> bool {
        scope == &self.scope && self.authority.is_current()
    }
    fn credential_handle(
        &self,
        scope: &crate::origin_read::EnrollmentScope,
    ) -> Option<crate::serve::CredentialHandle> {
        (scope == &self.scope).then(|| self.authority.clone())
    }
}

/// One host-owned watcher per receiver registry and broker. Create it before
/// accepting work. Calls are serialized by `&mut self`; failed notifications do
/// not advance the observed revision.
///
/// Append owners must additionally call `AppendService::sweep_owner` with each
/// retained original scope and its owner source. The host owns that collection;
/// a registry revision alone does not identify the active append services.
pub struct RevocationWatcher {
    receiver_dir: PathBuf,
    revision: u64,
}
impl RevocationWatcher {
    pub fn new(receiver_dir: &Path) -> Result<Self> {
        Ok(Self {
            receiver_dir: receiver_dir.to_owned(),
            revision: current_revision(receiver_dir)?,
        })
    }

    /// Returns true after notifying the broker of a changed revision. An
    /// unreadable registry still triggers fail-closed cleanup, then returns
    /// `Unavailable`; the last successful revision remains pending for retry.
    /// The source must itself fail closed when registry reads fail.
    pub fn notify_if_revision_changed(
        &mut self,
        source: &dyn EnrollmentSource,
        broker: &Broker<'_>,
    ) -> Result<bool> {
        let revision = current_revision(&self.receiver_dir);
        if revision == Ok(self.revision) {
            return Ok(false);
        }
        broker.revocation_changed(source)?;
        self.revision = revision?;
        Ok(true)
    }
}

fn current_revision(receiver_dir: &Path) -> Result<u64> {
    enrollment::refuse_legacy_tokens(receiver_dir).map_err(|_| Reason::Unavailable)?;
    Registry::read_current(receiver_dir)
        .map(|registry| registry.revision())
        .map_err(|_| Reason::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        body_assertion::{self, Audience, Claims, MemoryKey},
        body_broker::Config,
        body_wire::{self as wire, Operation, Request},
        enrollment::Store,
        origin_read::{EnrollmentKind, EnrollmentScope, OpaqueId, ReadClock},
        receiver_audit::{Destination, Identifier},
    };
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use zeroize::Zeroizing;

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "semon-body-enrollment-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            crate::tokens::prepare_private_dir(&path, true).unwrap();
            Store::initialize(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct At;
    impl ReadClock for At {
        fn now_millis(&self) -> u64 {
            10_000
        }
    }
    impl crate::body_cache::Clock for &At {
        fn now(&self) -> u64 {
            10
        }
    }

    #[test]
    fn captured_append_authority_closes_all_owners() {
        use crate::{
            append_receiver::AppendService,
            body_broker::Principal,
            receiver_acl::Acls,
            receiver_audit::{self, AuditStore},
        };
        use std::os::unix::fs::PermissionsExt;
        for failure in ["credential", "read", "delayed"] {
            let dir = Directory::new();
            let a = source(&dir.0, "a");
            let b = source(&dir.0, "b");
            let at = At;
            let broker = Broker::new(Config::default(), &at).unwrap();
            let acl = dir.0.join("acl");
            std::fs::create_dir(&acl).unwrap();
            std::fs::set_permissions(&acl, std::fs::Permissions::from_mode(0o700)).unwrap();
            let grants = acl.join("grants.json");
            std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
            std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600)).unwrap();
            let acls = Acls::new(acl);
            let mut audit_a =
                AuditStore::open(&dir.0.join("audit-a"), &at, receiver_audit::MAX_BYTES).unwrap();
            let mut audit_b =
                AuditStore::open(&dir.0.join("audit-b"), &at, receiver_audit::MAX_BYTES).unwrap();
            let signing = MemoryKey::from_seed(Zeroizing::new([7; 32]));
            let mut services = [
                AppendService::new(
                    &broker,
                    &a,
                    &acls,
                    &signing,
                    "issuer",
                    Identifier::from_bytes([11; 32]),
                    &mut audit_a,
                ),
                AppendService::new(
                    &broker,
                    &b,
                    &acls,
                    &signing,
                    "issuer",
                    Identifier::from_bytes([11; 32]),
                    &mut audit_b,
                ),
            ];
            let mut leases = Vec::new();
            for (service, owner) in services.iter_mut().zip([&a, &b]) {
                let bytes = wire::control(&wire::PollRequest {
                    version: wire::VERSION,
                    channel_binding: [3; 32],
                })
                .unwrap();
                drop(broker.begin_poll(owner, &bytes).unwrap());
                let mut template = request(77);
                template.operation = Operation::ReadPage {
                    page: opaque("append"),
                };
                leases.push(
                    service
                        .subscribe(
                            Principal {
                                subject: "subject".into(),
                                audit_id: Identifier::from_bytes([9; 32]),
                            },
                            owner.scope.clone(),
                            template,
                            Identifier::from_bytes([4; 32]),
                        )
                        .unwrap(),
                );
            }
            for (service, (lease, _)) in services.iter_mut().zip(&leases) {
                service.maintain().unwrap();
                assert!(service.collect(*lease).is_ok());
            }
            if failure == "delayed" {
                let mut watcher = RevocationWatcher::new(&dir.0).unwrap();
                broker
                    .enqueue(&a, a.scope.clone(), [3; 32], request(1))
                    .unwrap();
                advance(&dir.0);
                let fresh = source(&dir.0, "a");
                let bytes = wire::control(&wire::PollRequest {
                    version: wire::VERSION,
                    channel_binding: [4; 32],
                })
                .unwrap();
                let poll = broker.begin_poll(&fresh, &bytes).unwrap();
                broker
                    .enqueue(&fresh, fresh.scope.clone(), [4; 32], request(2))
                    .unwrap();
                let mut fresh_audit =
                    AuditStore::open(&dir.0.join("audit-fresh"), &at, receiver_audit::MAX_BYTES)
                        .unwrap();
                let mut service = AppendService::new(
                    &broker,
                    &fresh,
                    &acls,
                    &signing,
                    "issuer",
                    Identifier::from_bytes([11; 32]),
                    &mut fresh_audit,
                );
                let mut template = request(78);
                template.operation = Operation::ReadPage {
                    page: opaque("append"),
                };
                let (lease, _watcher) = service
                    .subscribe(
                        Principal {
                            subject: "subject".into(),
                            audit_id: Identifier::from_bytes([9; 32]),
                        },
                        fresh.scope.clone(),
                        template,
                        Identifier::from_bytes([4; 32]),
                    )
                    .unwrap();
                assert!(watcher.notify_if_revision_changed(&fresh, &broker).unwrap());
                assert_eq!(broker.channel(&fresh.scope), Ok([4; 32]));
                service.sweep_owner(&fresh.scope, &fresh).unwrap();
                assert_eq!(service.collect(lease), Ok(false));
                assert!(matches!(
                    broker.take_result(&fresh, Identifier::from_bytes([1; 32])),
                    Err(Reason::Unavailable)
                ));
                assert!(
                    broker
                        .take_result(&fresh, Identifier::from_bytes([2; 32]))
                        .unwrap()
                        .is_none()
                );
                assert!(poll.try_take(&fresh).unwrap().is_some());
                continue;
            }
            advance(&dir.0);
            for (service, (lease, _)) in services.iter_mut().zip(&leases) {
                service.maintain().unwrap();
                assert!(service.collect(*lease).is_ok());
            }
            if failure == "credential" {
                let mut store = Store::open(&dir.0).unwrap();
                let mut snapshot = store.snapshot().unwrap();
                snapshot.credentials[0].revoked_at = Some(10);
                store.commit(snapshot.revision(), &snapshot).unwrap();
            } else {
                std::fs::write(dir.0.join("enrollment/registry.json"), b"invalid registry")
                    .unwrap();
            }
            for (service, (lease, _)) in services.iter_mut().zip(&leases) {
                assert_eq!(service.collect(*lease), Err(Reason::Revoked));
                service.maintain().unwrap();
                assert_eq!(service.collect(*lease), Err(Reason::LeaseClosed));
            }
        }
    }

    struct Owners {
        revoked: AtomicBool,
        checks: AtomicUsize,
    }
    impl EnrollmentSource for Owners {
        fn authenticated_enrollment(&self, kind: EnrollmentKind) -> Result<EnrollmentScope> {
            if kind != EnrollmentKind::BodyChannel {
                return Err(Reason::Forbidden);
            }
            Ok(scope("a"))
        }
        fn is_current(&self, scope: &EnrollmentScope) -> bool {
            self.checks.fetch_add(1, Ordering::Relaxed);
            scope == &self::scope("b")
                || (scope == &self::scope("a") && !self.revoked.load(Ordering::Relaxed))
        }
    }
    struct RequestOwner<'a>(&'a Owners, EnrollmentScope);
    impl EnrollmentSource for RequestOwner<'_> {
        fn authenticated_enrollment(&self, kind: EnrollmentKind) -> Result<EnrollmentScope> {
            self.0.authenticated_enrollment(kind)?;
            Ok(self.1.clone())
        }
        fn is_current(&self, scope: &EnrollmentScope) -> bool {
            self.0.is_current(scope)
        }
    }
    fn opaque(value: &str) -> OpaqueId {
        OpaqueId::new(value).unwrap()
    }
    fn scope(id: &str) -> EnrollmentScope {
        EnrollmentScope {
            enrollment_id: opaque(id),
            machine: opaque("machine"),
            namespace: opaque("namespace"),
            origin_incarnation: opaque("origin"),
            index_incarnation: opaque("index"),
            publisher_claim_epoch: Some(7),
            kinds: vec![EnrollmentKind::BodyChannel],
            verified_client_identity: opaque("client"),
        }
    }
    fn request(n: u8) -> Request {
        let mut request = Request {
            version: wire::VERSION,
            request_id: Identifier::from_bytes([n; 32]),
            assertion: String::new(),
            source_handle: opaque("handle"),
            anchor: opaque("anchor"),
            operation: Operation::ReadField {
                field: opaque("assistant"),
            },
            max_bytes: 1024,
            deadline_millis: 20_000,
            destination: Destination::Viewer,
        };
        let claims = Claims {
            issuer_key_id: "issuer".into(),
            issued_at: 10,
            expiry: 20,
            audience: Audience {
                origin_incarnation: "origin".into(),
                namespace: "namespace".into(),
            },
            claim_epoch: 7,
            nonce: [n; 32],
            subject_principal: "subject".into(),
            machine: "machine".into(),
            namespace: "namespace".into(),
            acl_grant_version: "acl".into(),
            receiver_policy_version: "policy".into(),
            request_digest: request.digest().unwrap(),
            deadline: 20,
            channel_binding: [3; 32],
        };
        let key = MemoryKey::from_seed(Zeroizing::new([7; 32]));
        request.assertion = hex::encode(body_assertion::encode(
            &body_assertion::sign(claims, &key, || 10).unwrap(),
        ));
        request
    }
    fn source(dir: &Path, name: &str) -> RegistryEnrollmentSource {
        use crate::{
            credentials::{Bearer, Kind, Secret},
            enrollment::{Binding, Enrollment, KindScope},
        };
        let enrollment = Enrollment {
            enrollment_id: "a".repeat(64),
            receiver_id: "receiver-1".into(),
            machine: "machine".into(),
            sequence: 1,
            origin_key: vec![4; 65],
            endpoint: "https://receiver.example:443".into(),
            binding: Binding::Inapplicable,
            client_certificate_pins: vec![],
            credential_kinds: vec![KindScope::new(Kind::MirrorPush, 1)],
            revoked_at: None,
        };
        let credential = Bearer {
            credential_id: "b".repeat(64),
            enrollment_id: enrollment.enrollment_id.clone(),
            kind: Kind::MirrorPush,
            sha256: Secret::new("test-bearer".into()).hash(),
            generation: 1,
            valid_from: 0,
            valid_until: u64::MAX,
            revoked_at: None,
        };
        let mut store = Store::open(dir).unwrap();
        let current = store.snapshot().unwrap();
        if current.enrollments().is_empty() {
            store
                .commit(
                    current.revision(),
                    &Registry::from_records(vec![enrollment], vec![credential]).unwrap(),
                )
                .unwrap();
        }
        drop(store);
        let authenticator = crate::serve::RegistryAuthenticator::open(dir).unwrap();
        let authenticated = authenticator
            .authenticated_enrollment(
                &mut crate::serve::ConnectionAuthority::default(),
                Some("Bearer test-bearer"),
                Kind::MirrorPush,
                10,
            )
            .unwrap();
        // Broker lifecycle tests need real credential handles; scope conversion and
        // verified body-kind authentication are covered by serve_auth_tests.
        RegistryEnrollmentSource {
            scope: scope(name),
            authority: authenticated.authority,
        }
    }

    #[test]
    fn captured_work_survives_churn_until_its_credential_ends() {
        for failure in ["credential", "read"] {
            let dir = Directory::new();
            let a = source(&dir.0, "a");
            let b = source(&dir.0, "b");
            let mut watcher = RevocationWatcher::new(&dir.0).unwrap();
            let broker = Broker::new(Config::default(), &At).unwrap();
            let bytes = wire::control(&wire::PollRequest {
                version: wire::VERSION,
                channel_binding: [3; 32],
            })
            .unwrap();
            for (owner, n) in [(&a, 1), (&b, 2)] {
                drop(broker.begin_poll(owner, &bytes).unwrap());
                broker
                    .enqueue(owner, owner.scope.clone(), [3; 32], request(n))
                    .unwrap();
            }
            // B's request must not revoke A while their captured credentials remain current.
            assert!(
                broker
                    .take_result(&b, Identifier::from_bytes([1; 32]))
                    .unwrap()
                    .is_none()
            );
            assert!(!watcher.notify_if_revision_changed(&b, &broker).unwrap());
            advance(&dir.0);
            assert_eq!(watcher.notify_if_revision_changed(&b, &broker), Ok(true));
            for n in [1, 2] {
                assert!(
                    broker
                        .take_result(&b, Identifier::from_bytes([n; 32]))
                        .unwrap()
                        .is_none()
                );
            }
            if failure == "credential" {
                let mut store = Store::open(&dir.0).unwrap();
                let mut snapshot = store.snapshot().unwrap();
                snapshot.credentials[0].revoked_at = Some(10);
                store.commit(snapshot.revision(), &snapshot).unwrap();
            } else {
                std::fs::write(dir.0.join("enrollment/registry.json"), b"invalid registry")
                    .unwrap();
            }
            let notified = watcher.notify_if_revision_changed(&b, &broker);
            assert_eq!(
                notified,
                if failure == "credential" {
                    Ok(true)
                } else {
                    Err(Reason::Unavailable)
                }
            );
            for n in [1, 2] {
                assert!(matches!(
                    broker.take_result(&b, Identifier::from_bytes([n; 32])),
                    Err(Reason::Revoked)
                ));
            }
        }
    }

    fn advance(dir: &Path) {
        let mut store = Store::open(dir).unwrap();
        let snapshot = store.snapshot().unwrap();
        store.commit(snapshot.revision(), &snapshot).unwrap();
    }

    #[test]
    fn changed_revision_notifies_once_and_preserves_other_owners_work() {
        let dir = Directory::new();
        let mut watcher = RevocationWatcher::new(&dir.0).unwrap();
        let owners = Owners {
            revoked: AtomicBool::new(false),
            checks: AtomicUsize::new(0),
        };
        let broker = Broker::new(Config::default(), &At).unwrap();
        let bytes = wire::control(&wire::PollRequest {
            version: wire::VERSION,
            channel_binding: [3; 32],
        })
        .unwrap();
        for (name, n) in [("a", 1), ("b", 2)] {
            let owner = RequestOwner(&owners, scope(name));
            drop(broker.begin_poll(&owner, &bytes).unwrap());
            broker
                .enqueue(&owners, scope(name), [3; 32], request(n))
                .unwrap();
        }
        owners.checks.store(0, Ordering::Relaxed);
        assert!(
            !watcher
                .notify_if_revision_changed(&owners, &broker)
                .unwrap()
        );
        assert_eq!(owners.checks.load(Ordering::Relaxed), 0);
        owners.revoked.store(true, Ordering::Relaxed);
        advance(&dir.0);
        assert!(
            watcher
                .notify_if_revision_changed(&owners, &broker)
                .unwrap()
        );
        assert!(owners.checks.load(Ordering::Relaxed) > 0);
        owners.checks.store(0, Ordering::Relaxed);
        assert!(
            !watcher
                .notify_if_revision_changed(&owners, &broker)
                .unwrap()
        );
        assert_eq!(owners.checks.load(Ordering::Relaxed), 0);
        assert!(matches!(
            broker.take_result(&owners, Identifier::from_bytes([1; 32])),
            Err(Reason::Revoked)
        ));
        assert!(
            broker
                .take_result(&owners, Identifier::from_bytes([2; 32]))
                .unwrap()
                .is_none()
        );
        // A second revision cannot be swallowed by the previous notification.
        advance(&dir.0);
        assert!(
            watcher
                .notify_if_revision_changed(&owners, &broker)
                .unwrap()
        );
    }

    #[test]
    fn unreadable_registry_does_not_acknowledge_a_revision() {
        let dir = Directory::new();
        let mut watcher = RevocationWatcher::new(&dir.0).unwrap();
        let revision = watcher.revision;
        let owners = Owners {
            revoked: AtomicBool::new(true),
            checks: AtomicUsize::new(0),
        };
        let broker = Broker::new(Config::default(), &At).unwrap();
        std::fs::rename(&dir.0, dir.0.with_extension("unreadable")).unwrap();
        let result = watcher.notify_if_revision_changed(&owners, &broker);
        std::fs::rename(dir.0.with_extension("unreadable"), &dir.0).unwrap();
        assert_eq!(result, Err(Reason::Unavailable));
        assert_eq!(watcher.revision, revision);
        advance(&dir.0);
        assert!(
            watcher
                .notify_if_revision_changed(&owners, &broker)
                .unwrap()
        );
    }
}
