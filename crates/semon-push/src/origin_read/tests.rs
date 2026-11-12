use super::*;
use crate::{body_assertion::Audience, origin_audit::AUDIT_FILE};
use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const SECRET: &str = "private-body-never-in-audit";
fn opaque(s: &str) -> OpaqueId {
    OpaqueId::new(s).unwrap()
}
fn id(n: u8) -> Identifier {
    Identifier::from_bytes([n; 32])
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "origin-read-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn journal(&self) -> String {
        fs::read_to_string(self.0.join(AUDIT_FILE)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct At(Cell<u64>);
impl Clock for &At {
    fn now(&self) -> u64 {
        self.0.get() / 1000
    }
}
impl ReadClock for At {
    fn now_millis(&self) -> u64 {
        self.0.get()
    }
}
struct Enrollment {
    revoked: Cell<bool>,
}
impl AuthenticatedEnrollment for Enrollment {
    fn authenticated_enrollment(&self) -> Result<EnrollmentScope> {
        if self.revoked.get() {
            return Err(Refused::Forbidden);
        }
        Ok(scope())
    }
}
fn scope() -> EnrollmentScope {
    EnrollmentScope {
        enrollment_id: opaque("enrollment"),
        machine: opaque("machine"),
        namespace: opaque("namespace"),
        origin_incarnation: opaque("origin"),
        index_incarnation: opaque("index"),
        publisher_claim_epoch: Some(7),
        kinds: vec![EnrollmentKind::BodyChannel],
        verified_client_identity: opaque("client"),
    }
}
fn claims() -> Claims {
    Claims {
        issuer_key_id: "issuer".into(),
        issued_at: 10,
        expiry: 60,
        audience: Audience {
            origin_incarnation: "origin".into(),
            namespace: "namespace".into(),
        },
        claim_epoch: 7,
        nonce: [1; 32],
        subject_principal: "subject".into(),
        machine: "machine".into(),
        namespace: "namespace".into(),
        acl_grant_version: "acl".into(),
        receiver_policy_version: "receiver-policy".into(),
        request_digest: [2; 32],
        deadline: 50,
        channel_binding: [3; 32],
    }
}
fn request() -> ReadRequest {
    ReadRequest {
        source_handle: opaque("handle"),
        target: Target::Field(opaque("assistant")),
        anchor: opaque("anchor"),
        request_digest: [2; 32],
        channel_binding: [3; 32],
        max_bytes: MAX_BYTES,
        destination: Destination::Viewer,
    }
}
struct Resolver {
    missing: bool,
}
impl HandleResolver for Resolver {
    fn resolve(&self, _: &EnrollmentScope, _: &OpaqueId) -> Result<Option<ResolvedSource>> {
        Ok((!self.missing).then(source))
    }
}
fn source() -> ResolvedSource {
    ResolvedSource {
        admitted_source: opaque("admitted"),
        cohort_id: id(4),
        cohort: vec![SourceWitness {
            source_id: "source".into(),
            chain: "chain".into(),
            generation: "generation".into(),
            consumed_boundary: 900,
        }],
        activity_generation: opaque("activity"),
        transcript_generation: opaque("transcript"),
    }
}
struct Policies(Cell<bool>);
impl PolicyVector for Policies {
    fn evaluate(
        &self,
        _: &EnrollmentScope,
        _: &Claims,
        _: &ResolvedSource,
        _: &Target,
    ) -> Result<PinnedPolicy> {
        Ok(PinnedPolicy {
            vector: id(if self.0.get() { 9 } else { 5 }),
            policy_version: id(6),
            redactor_version: id(7),
        })
    }
}
fn page() -> TypedPage {
    TypedPage {
        fields: vec![TypedField {
            field: Field::Assistant,
            text: UntrustedText::new(SECRET.into()),
        }],
    }
}
fn plan() -> RenderPlan {
    RenderPlan {
        payload_bytes: page().canonical_bytes().unwrap().len() as u64,
        export_view_generation: Some(opaque("export")),
        exact_mapping: Some(ExactMapping {
            anchor: opaque("anchor"),
            assertion: id(8),
        }),
        continuation: Some(opaque("next")),
    }
}
struct Renderer<'a> {
    fixture: &'a Fixture,
    calls: Cell<usize>,
    action: &'a dyn Fn(),
    failure: Option<Refused>,
    oversized: bool,
}
impl PageRenderer for Renderer<'_> {
    fn prepare(
        &self,
        _: &ResolvedSource,
        _: &Target,
        _: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderPlan> {
        assert!(budget.bytes <= MAX_BYTES);
        assert_eq!(budget.deadline_millis, 12_000);
        assert!(!cancelled());
        let mut p = plan();
        if self.oversized {
            p.payload_bytes = MAX_BYTES as u64;
        }
        Ok(p)
    }
    fn render(
        &self,
        _: &ResolvedSource,
        _: &Target,
        _: PinnedPolicy,
        budget: RenderBudget,
        _: &dyn Fn() -> bool,
    ) -> Result<RenderedPage> {
        self.calls.set(self.calls.get() + 1);
        let journal = self.fixture.journal();
        assert!(journal.contains("\"event\":\"authorized\""));
        assert!(!journal.contains("completion"));
        assert!(budget.bytes < MAX_BYTES);
        assert_eq!(budget.deadline_millis, 12_000);
        (self.action)();
        if let Some(reason) = self.failure {
            return Err(reason);
        }
        let p = plan();
        Ok(RenderedPage {
            page: page(),
            export_view_generation: p.export_view_generation,
            exact_mapping: p.exact_mapping,
            continuation: p.continuation,
        })
    }
}
struct Valid;
impl ExportViewValidator for Valid {
    fn validate(&self, _: &CachedCandidate, _: &ResolvedSource, _: PinnedPolicy) -> Result<()> {
        Ok(())
    }
}

#[test]
fn happy_path_digest_binding_and_explicit_completion() {
    let f = Fixture::new();
    let at = At(Cell::new(10_000));
    let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
    let enrollment = Enrollment {
        revoked: Cell::new(false),
    };
    let policies = Policies(Cell::new(false));
    let renderer = Renderer {
        fixture: &f,
        calls: Cell::new(0),
        action: &|| {},
        failure: None,
        oversized: false,
    };
    let result = OriginRead {
        enrollment: &enrollment,
        resolver: &Resolver { missing: false },
        policies: &policies,
        delivery_mode: DeliveryMode::Cached,
        renderer: &renderer,
        clock: &at,
        cancelled: &|| false,
        audit: &mut audit,
    }
    .read(&claims(), &request())
    .unwrap();
    let bytes = page().canonical_bytes().unwrap();
    assert_eq!(
        result.response_binding.cache_key.payload_digest,
        <[u8; 32]>::from(Sha256::digest(&*bytes))
    );
    assert_eq!(result.response_binding.receiver_binding.request_id, id(1));
    assert_eq!(
        result
            .response_binding
            .receiver_binding
            .origin_authorization_id,
        result.response_binding.origin_authorization.identifier()
    );
    assert!(!format!("{:?}", result.page.as_ref().unwrap()).contains(SECRET));
    result
        .finish(&mut audit, Outcome::Completed, bytes.len() as u64)
        .unwrap();
    let journal = f.journal();
    assert!(journal.contains("\"outcome\":\"completed\""));
    assert!(!journal.contains(SECRET));
    assert_eq!(renderer.calls.get(), 1);
}

#[test]
fn delivery_mode_is_required_and_binds_identity_and_origin_audit() {
    let mut audit_bindings = Vec::new();
    for mode in [DeliveryMode::Cached, DeliveryMode::StreamThrough] {
        let f = Fixture::new();
        let at = At(Cell::new(10_000));
        let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
        let renderer = Renderer {
            fixture: &f,
            calls: Cell::new(0),
            action: &|| {},
            failure: None,
            oversized: false,
        };
        let authorized = OriginRead {
            enrollment: &Enrollment {
                revoked: Cell::new(false),
            },
            resolver: &Resolver { missing: false },
            policies: &Policies(Cell::new(false)),
            delivery_mode: mode,
            renderer: &renderer,
            clock: &at,
            cancelled: &|| false,
            audit: &mut audit,
        }
        .read(&claims(), &request())
        .unwrap();
        let binding = &authorized.response_binding;
        binding
            .verify_identity(&claims(), Destination::Viewer)
            .unwrap();
        let mut changed = binding.clone();
        changed.delivery_mode = if mode == DeliveryMode::Cached {
            DeliveryMode::StreamThrough
        } else {
            DeliveryMode::Cached
        };
        assert_eq!(
            changed.verify_identity(&claims(), Destination::Viewer),
            Err(Refused::InvalidRequest)
        );
        let value = serde_json::to_value(binding).unwrap();
        for bad in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!("unknown")),
            Some(serde_json::json!(1)),
        ] {
            let mut invalid = value.clone();
            invalid.as_object_mut().unwrap().remove("delivery_mode");
            if let Some(bad) = bad {
                invalid["delivery_mode"] = bad;
            }
            assert!(serde_json::from_value::<ResponseBinding>(invalid.clone()).is_err());
            let header = serde_json::to_vec(&serde_json::json!({
                "version": 1, "request_id": id(1), "frame": 0,
                "final_frame": false, "payload_bytes": 0,
                "content": { "kind": "header", "binding": invalid }
            }))
            .unwrap();
            let mut frame = (header.len() as u32).to_be_bytes().to_vec();
            frame.extend(header);
            assert!(crate::body_wire::parse_frame(&frame).is_err());
        }
        let event: serde_json::Value =
            serde_json::from_str(f.journal().lines().next().unwrap()).unwrap();
        audit_bindings.push(event["record"]["export_view_binding"].clone());
        authorized
            .finish(&mut audit, Outcome::Completed, 7)
            .unwrap();
        assert!(f.journal().contains("\"bytes\":7"));
    }
    assert_ne!(audit_bindings[0], audit_bindings[1]);
}

#[test]
fn policy_revocation_deadline_cancellation_and_renderer_failure_close_audits() {
    for mode in 0..5 {
        let f = Fixture::new();
        let at = At(Cell::new(10_000));
        let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
        let enrollment = Enrollment {
            revoked: Cell::new(false),
        };
        let policies = Policies(Cell::new(false));
        let cancelled = Cell::new(false);
        let action = || match mode {
            0 => policies.0.set(true),
            1 => enrollment.revoked.set(true),
            2 => at.0.set(12_000),
            3 => cancelled.set(true),
            _ => {}
        };
        let renderer = Renderer {
            fixture: &f,
            calls: Cell::new(0),
            action: &action,
            failure: (mode == 4).then_some(Refused::Unavailable),
            oversized: false,
        };
        let result = OriginRead {
            enrollment: &enrollment,
            resolver: &Resolver { missing: false },
            policies: &policies,
            delivery_mode: DeliveryMode::Cached,
            renderer: &renderer,
            clock: &at,
            cancelled: &|| cancelled.get(),
            audit: &mut audit,
        }
        .read(&claims(), &request());
        assert_eq!(
            result.err(),
            Some(match mode {
                0 | 1 => Refused::PolicyChanged,
                2 => Refused::Deadline,
                _ => Refused::Unavailable,
            })
        );
        let journal = f.journal();
        assert!(journal.contains("completion"));
        assert!(journal.contains(if mode == 3 { "cancelled" } else { "failed" }));
        assert!(!journal.contains(SECRET));
    }
}

#[test]
fn scope_and_unknown_handle_refuse_uniformly_before_render() {
    for mode in 0..10 {
        let f = Fixture::new();
        let at = At(Cell::new(10_000));
        let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
        let mut c = claims();
        match mode {
            0 => c.audience.origin_incarnation = "other".into(),
            1 => c.audience.namespace = "other".into(),
            2 => c.claim_epoch = 8,
            3 => c.machine = "other".into(),
            4 => c.namespace = "other".into(),
            5 => c.channel_binding = [0; 32],
            6 => c.request_digest = [0; 32],
            _ => {}
        }
        struct ScopeStub(EnrollmentScope);
        impl AuthenticatedEnrollment for ScopeStub {
            fn authenticated_enrollment(&self) -> Result<EnrollmentScope> {
                Ok(self.0.clone())
            }
        }
        let mut s = scope();
        if mode == 7 {
            s.kinds.clear();
        }
        if mode == 8 {
            s.publisher_claim_epoch = None;
        }
        let renderer = Renderer {
            fixture: &f,
            calls: Cell::new(0),
            action: &|| {},
            failure: None,
            oversized: false,
        };
        let result = OriginRead {
            enrollment: &ScopeStub(s),
            resolver: &Resolver { missing: mode == 9 },
            policies: &Policies(Cell::new(false)),
            delivery_mode: DeliveryMode::Cached,
            renderer: &renderer,
            clock: &at,
            cancelled: &|| false,
            audit: &mut audit,
        }
        .read(&c, &request());
        assert_eq!(result.err(), Some(Refused::Forbidden));
        assert_eq!(renderer.calls.get(), 0);
        assert!(f.journal().is_empty());
    }
}

#[test]
fn budget_deadline_and_audit_failure_refuse_before_render() {
    for mode in 0..4 {
        let f = Fixture::new();
        let at = At(Cell::new(if mode == 1 { 50_000 } else { 10_000 }));
        let mut audit = AuditStore::open(
            &f.0,
            &at,
            if mode == 2 {
                0
            } else {
                crate::origin_audit::MAX_BYTES
            },
        )
        .unwrap();
        let renderer = Renderer {
            fixture: &f,
            calls: Cell::new(0),
            action: &|| {},
            failure: None,
            oversized: mode == 3,
        };
        let mut r = request();
        if mode == 0 {
            r.max_bytes = 1;
        }
        let result = OriginRead {
            enrollment: &Enrollment {
                revoked: Cell::new(false),
            },
            resolver: &Resolver { missing: false },
            policies: &Policies(Cell::new(false)),
            delivery_mode: DeliveryMode::Cached,
            renderer: &renderer,
            clock: &at,
            cancelled: &|| false,
            audit: &mut audit,
        }
        .read(&claims(), &r);
        assert_eq!(
            result.err(),
            Some(match mode {
                1 => Refused::Deadline,
                2 => Refused::AuditFailure,
                _ => Refused::OverBudget,
            })
        );
        assert_eq!(renderer.calls.get(), 0);
        assert!(f.journal().is_empty());
    }
}

#[test]
fn dropping_authorized_leaves_attempted_history() {
    let f = Fixture::new();
    let at = At(Cell::new(10_000));
    let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
    let renderer = Renderer {
        fixture: &f,
        calls: Cell::new(0),
        action: &|| {},
        failure: None,
        oversized: false,
    };
    drop(
        OriginRead {
            enrollment: &Enrollment {
                revoked: Cell::new(false),
            },
            resolver: &Resolver { missing: false },
            policies: &Policies(Cell::new(false)),
            delivery_mode: DeliveryMode::Cached,
            renderer: &renderer,
            clock: &at,
            cancelled: &|| false,
            audit: &mut audit,
        }
        .read(&claims(), &request())
        .unwrap(),
    );
    drop(audit);
    let _reopened = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
    assert_eq!(f.journal().lines().count(), 1);
}

#[test]
fn authorization_only_requires_exact_origin_witness_and_never_renders() {
    let f = Fixture::new();
    let at = At(Cell::new(10_000));
    let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
    let enrollment = Enrollment {
        revoked: Cell::new(false),
    };
    let policies = Policies(Cell::new(false));
    let renderer = Renderer {
        fixture: &f,
        calls: Cell::new(0),
        action: &|| {},
        failure: None,
        oversized: false,
    };
    let r = request();
    let c = claims();
    let p = plan();
    let policy = policies
        .evaluate(&scope(), &c, &source(), &r.target)
        .unwrap();
    let candidate = CachedCandidate {
        key: cache_key(
            &scope(),
            &source(),
            &r,
            p.export_view_generation.as_ref().unwrap(),
            policy,
            &c,
            Sha256::digest(&*page().canonical_bytes().unwrap()).into(),
        ),
        mapping: p.exact_mapping.unwrap(),
        continuation: p.continuation,
        payload_bytes: p.payload_bytes,
    };
    let mut core = OriginRead {
        enrollment: &enrollment,
        resolver: &Resolver { missing: false },
        policies: &policies,
        delivery_mode: DeliveryMode::Cached,
        renderer: &renderer,
        clock: &at,
        cancelled: &|| false,
        audit: &mut audit,
    };
    assert_eq!(
        core.authorization_only(&c, &r, &candidate, &UnavailableExportView)
            .err(),
        Some(Refused::Unavailable)
    );
    let authorized = core.authorization_only(&c, &r, &candidate, &Valid).unwrap();
    assert!(authorized.page.is_none());
    authorized
        .finish(&mut audit, Outcome::Completed, 0)
        .unwrap();
    assert_eq!(renderer.calls.get(), 0);
    assert_eq!(f.journal().lines().count(), 2);

    struct Flip<'a>(&'a Policies);
    impl ExportViewValidator for Flip<'_> {
        fn validate(&self, _: &CachedCandidate, _: &ResolvedSource, _: PinnedPolicy) -> Result<()> {
            self.0.0.set(true);
            Ok(())
        }
    }
    let mut core = OriginRead {
        enrollment: &enrollment,
        resolver: &Resolver { missing: false },
        policies: &policies,
        delivery_mode: DeliveryMode::Cached,
        renderer: &renderer,
        clock: &at,
        cancelled: &|| false,
        audit: &mut audit,
    };
    assert_eq!(
        core.authorization_only(&c, &r, &candidate, &Flip(&policies))
            .err(),
        Some(Refused::PolicyChanged)
    );
    assert_eq!(f.journal().lines().count(), 4);
}

#[test]
fn cancellation_or_deadline_during_prepare_leaves_journal_empty() {
    struct Preparing<'a> {
        inner: Renderer<'a>,
        action: &'a dyn Fn(),
    }
    impl PageRenderer for Preparing<'_> {
        fn prepare(
            &self,
            source: &ResolvedSource,
            target: &Target,
            policy: PinnedPolicy,
            budget: RenderBudget,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<RenderPlan> {
            let plan = self
                .inner
                .prepare(source, target, policy, budget, cancelled)?;
            (self.action)();
            Ok(plan)
        }
        fn render(
            &self,
            _: &ResolvedSource,
            _: &Target,
            _: PinnedPolicy,
            _: RenderBudget,
            _: &dyn Fn() -> bool,
        ) -> Result<RenderedPage> {
            panic!("preparation refusal must precede render")
        }
    }
    for deadline in [false, true] {
        let f = Fixture::new();
        let at = At(Cell::new(10_000));
        let cancelled = Cell::new(false);
        let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
        let action = || {
            if deadline {
                at.0.set(12_000);
            } else {
                cancelled.set(true);
            }
        };
        let renderer = Preparing {
            inner: Renderer {
                fixture: &f,
                calls: Cell::new(0),
                action: &|| {},
                failure: None,
                oversized: false,
            },
            action: &action,
        };
        let result = OriginRead {
            enrollment: &Enrollment {
                revoked: Cell::new(false),
            },
            resolver: &Resolver { missing: false },
            policies: &Policies(Cell::new(false)),
            delivery_mode: DeliveryMode::Cached,
            renderer: &renderer,
            clock: &at,
            cancelled: &|| cancelled.get(),
            audit: &mut audit,
        }
        .read(&claims(), &request());
        assert_eq!(
            result.err(),
            Some(if deadline {
                Refused::Deadline
            } else {
                Refused::Unavailable
            })
        );
        assert_eq!(renderer.inner.calls.get(), 0);
        assert!(f.journal().is_empty());
    }
}

#[test]
fn cancellation_or_deadline_during_final_policy_recheck_closes_audit() {
    struct Rechecking<'a> {
        calls: Cell<usize>,
        action: &'a dyn Fn(),
    }
    impl PolicyVector for Rechecking<'_> {
        fn evaluate(
            &self,
            scope: &EnrollmentScope,
            claims: &Claims,
            source: &ResolvedSource,
            target: &Target,
        ) -> Result<PinnedPolicy> {
            self.calls.set(self.calls.get() + 1);
            if self.calls.get() == 2 {
                (self.action)();
            }
            Policies(Cell::new(false)).evaluate(scope, claims, source, target)
        }
    }
    for deadline in [false, true] {
        let f = Fixture::new();
        let at = At(Cell::new(10_000));
        let cancelled = Cell::new(false);
        let mut audit = AuditStore::open(&f.0, &at, crate::origin_audit::MAX_BYTES).unwrap();
        let action = || {
            if deadline {
                at.0.set(12_000);
            } else {
                cancelled.set(true);
            }
        };
        let policies = Rechecking {
            calls: Cell::new(0),
            action: &action,
        };
        let renderer = Renderer {
            fixture: &f,
            calls: Cell::new(0),
            action: &|| {},
            failure: None,
            oversized: false,
        };
        let result = OriginRead {
            enrollment: &Enrollment {
                revoked: Cell::new(false),
            },
            resolver: &Resolver { missing: false },
            policies: &policies,
            delivery_mode: DeliveryMode::Cached,
            renderer: &renderer,
            clock: &at,
            cancelled: &|| cancelled.get(),
            audit: &mut audit,
        }
        .read(&claims(), &request());
        assert_eq!(
            result.err(),
            Some(if deadline {
                Refused::Deadline
            } else {
                Refused::Unavailable
            })
        );
        assert_eq!(policies.calls.get(), 2);
        assert_eq!(renderer.calls.get(), 1);
        let journal = f.journal();
        assert_eq!(journal.lines().count(), 2);
        assert!(journal.contains(if deadline {
            "\"outcome\":\"failed\""
        } else {
            "\"outcome\":\"cancelled\""
        }));
    }
}

#[test]
fn exported_ranges_are_deterministic_local_bounded_and_nonempty() {
    for target in [Target::Page(opaque("page")), Target::Field(opaque("field"))] {
        for continuation in [None, Some(opaque("next"))] {
            let a = exported_range(&target, 42, continuation.as_ref()).unwrap();
            let b = exported_range(&target, 42, continuation.as_ref()).unwrap();
            assert_eq!((a.start, a.end), (0, 42));
            assert_eq!((a.start, a.end), (b.start, b.end));
            assert_eq!(
                exported_range(&target, 0, continuation.as_ref()).err(),
                Some(Refused::Unavailable)
            );
            for size in [MAX_BYTES as u64 + 1, u64::MAX] {
                assert_eq!(
                    exported_range(&target, size, continuation.as_ref()).err(),
                    Some(Refused::OverBudget)
                );
            }
            assert_eq!(
                exported_range(&target, MAX_BYTES as u64, continuation.as_ref())
                    .unwrap()
                    .end,
                MAX_BYTES as u64
            );
        }
    }
}

#[test]
fn errors_and_body_debug_cannot_disclose_text() {
    for reason in [
        Refused::Unavailable,
        Refused::Forbidden,
        Refused::PolicyChanged,
        Refused::Deadline,
        Refused::OverBudget,
        Refused::AuditFailure,
        Refused::InvalidRequest,
    ] {
        assert!(!format!("{reason:?} {reason}").contains(SECRET));
    }
    assert!(!format!("{:?}", page()).contains(SECRET));
    assert_eq!(
        OpaqueId::new("../native/path").err(),
        Some(Refused::InvalidRequest)
    );
    assert_eq!(
        OpaqueId::new(&"a".repeat(129)).err(),
        Some(Refused::InvalidRequest)
    );
    assert_eq!(
        TypedPage { fields: vec![] }.canonical_bytes().err(),
        Some(Refused::Unavailable)
    );
}
