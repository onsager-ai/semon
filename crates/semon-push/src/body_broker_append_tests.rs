// Included inside body_broker::tests; uses the deterministic loopback fixtures.
struct ExtensionSource {
    current: std::cell::RefCell<ResolvedSource>,
    evidence: std::cell::RefCell<Vec<crate::origin_read::AppendContinuity>>,
}
impl HandleResolver for ExtensionSource {
    fn resolve(
        &self,
        _: &EnrollmentScope,
        _: &OpaqueId,
    ) -> std::result::Result<Option<ResolvedSource>, Refused> {
        Ok(Some(self.current.borrow().clone()))
    }
}
impl crate::append_source::AppendPageSource for ExtensionSource {
    fn prepare(
        &self,
        source: &ResolvedSource,
        cursor: &crate::append_source::AppendCursor<'_>,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> std::result::Result<RenderPlan, Refused> {
        let mut plan = Renderer.prepare(
            source,
            &Target::Page(cursor.page.clone()),
            policy,
            budget,
            cancelled,
        )?;
        plan.continuation = Some(opaque(if cursor.previous_cohort.is_some() {
            "second"
        } else {
            "first"
        }));
        if let Some(previous) = cursor.previous_cohort {
            assert_eq!(previous[0].consumed_boundary, 99);
            plan.append_continuity = self.evidence.borrow().clone();
        }
        Ok(plan)
    }
    fn render(
        &self,
        source: &ResolvedSource,
        cursor: &crate::append_source::AppendCursor<'_>,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> std::result::Result<RenderedPage, Refused> {
        let plan = self.prepare(source, cursor, policy, budget, cancelled)?;
        Ok(RenderedPage {
            page: page(),
            export_view_generation: plan.export_view_generation,
            exact_mapping: plan.exact_mapping,
            continuation: plan.continuation,
            append_continuity: plan.append_continuity,
        })
    }
}
#[test]
fn append_extension_both_endpoints_and_capacity() {
    use crate::{
        append_receiver::AppendService,
        origin_client::{OriginClient, Step},
        origin_read::AppendContinuity,
    };
    use std::os::unix::fs::PermissionsExt;
    for case in 0..19 {
        let at = At(AtomicU64::new(10_000));
        let a = auth("extension");
        let broker = Broker::new(Config::default(), &at).unwrap();
        let root =
            std::env::temp_dir().join(format!("append-extension-{}-{case}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let acl = root.join("acl");
        std::fs::create_dir(&acl).unwrap();
        std::fs::set_permissions(&acl, std::fs::Permissions::from_mode(0o700)).unwrap();
        let grants = acl.join("grants.json");
        std::fs::write(&grants, br#"{"format":1,"grants":[{"principal":"subject","machine":"machine","namespace":"namespace","permissions":["body_read"]}]}"#).unwrap();
        std::fs::set_permissions(&grants, std::fs::Permissions::from_mode(0o600)).unwrap();
        let acls = Acls::new(acl);
        let mut receiver =
            AuditStore::open(&root.join("receiver"), &at, receiver_audit::MAX_BYTES).unwrap();
        let mut origin_audit = crate::origin_audit::AuditStore::open(
            &root.join("origin"),
            &at,
            crate::origin_audit::MAX_BYTES,
        )
        .unwrap();
        let signing = body_assertion::MemoryKey::from_seed(Zeroizing::new([7; 32]));
        let public = signing.public_key().unwrap();
        let lookup = |_: &str| Some(public);
        let mut replay = body_assertion::ReplayCache::new(32);
        let source = ExtensionSource {
            current: std::cell::RefCell::new(
                Resolver
                    .resolve(&a.scope, &opaque("handle"))
                    .unwrap()
                    .unwrap(),
            ),
            evidence: std::cell::RefCell::new(Vec::new()),
        };
        let mut client = OriginClient::new(
            OriginRead {
                enrollment: &a,
                resolver: &source,
                policies: &Policies,
                delivery_mode: crate::origin_read::DeliveryMode::Cached,
                renderer: &Renderer,
                clock: &at,
                cancelled: &|| false,
                audit: &mut origin_audit,
            },
            Loopback {
                broker: &broker,
                auth: &a,
                fail_after_header: false,
                posts: 0,
            },
            FixedEntropy,
            &mut replay,
            &lookup,
            &UnavailableExportView,
        );
        client.append_source = Some(&source);
        assert_eq!(client.step(), Step::Empty);
        let mut service = AppendService::new(
            &broker,
            &a,
            &acls,
            &signing,
            "issuer",
            id(11),
            &mut receiver,
        );
        let subscribe = |service: &mut AppendService<'_, '_, &At>| {
            let mut template = signed(77).0;
            template.operation = wire::Operation::ReadPage {
                page: opaque("append"),
            };
            service.subscribe(
                Principal {
                    subject: "subject".into(),
                    audit_id: id(9),
                },
                a.scope.clone(),
                template,
                id(4),
            )
        };
        let (lease, watcher) = subscribe(&mut service).unwrap();
        assert_eq!(client.step(), Step::Delivered);
        let previous = broker
            .state
            .lock()
            .unwrap()
            .work
            .values()
            .find_map(|w| {
                w.result
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .map(|r| r.binding.clone())
            })
            .unwrap();
        assert!(service.collect(lease).unwrap());
        let bytes = service
            .release(lease)
            .unwrap()
            .canonical_bytes()
            .unwrap()
            .len() as u64;
        service.delivered(lease, bytes).unwrap();
        service.acknowledge(lease, &opaque("first")).unwrap();
        let evidence = AppendContinuity {
            source_id: opaque("source"),
            chain: opaque("chain"),
            previous_boundary: 99,
            prefix_sha256: Identifier::try_from("a".repeat(64)).unwrap(),
        };
        let mut next = previous.clone();
        next.receiver_binding.request_id = service.active_request_for_test(lease).request_id;
        next.cache_key.cohort[0].consumed_boundary = 199;
        next.cache_key.cohort[0].prefix_sha256 = "b".repeat(64);
        next.cache_key.cohort[0].generation = "new-generation".into();
        next.cache_key.activity_generation = "new-activity".into();
        next.cache_key.transcript_generation = "new-transcript".into();
        next.append_continuity = vec![evidence];
        match case {
            0 => {}
            1 => next.append_continuity[0].prefix_sha256 = id(9),
            2 => next.cache_key.cohort[0].consumed_boundary = 98,
            3 => next.cache_key.cohort[0].chain = "other-chain".into(),
            4 => {
                let mut member = next.cache_key.cohort[0].clone();
                member.source_id = "added".into();
                next.cache_key.cohort.push(member);
            }
            5 => next.cohort_id = id(99),
            6 => next.append_continuity.clear(),
            7 => next
                .append_continuity
                .push(next.append_continuity[0].clone()),
            8 => next.append_continuity[0].source_id = opaque("extra"),
            9 => next.append_continuity[0].chain = opaque("wrong"),
            10 => next.append_continuity[0].previous_boundary = 98,
            11..=13 => {
                next.cache_key.cohort[0].consumed_boundary = 99;
                next.cache_key.cohort[0].prefix_sha256 =
                    if case == 12 { "b" } else { "a" }.repeat(64);
                if case != 13 {
                    next.append_continuity.clear();
                }
            }
            14 => next.cache_key.selector = crate::body_cache::Selector::Page("other".into()),
            15 => next.mapping.anchor = opaque("other"),
            16 => next.policy.vector = id(99),
            17 => next.cache_key.export_view_generation = "other".into(),
            _ => next.cache_key.cohort.clear(),
        }
        if case == 0 {
            let mut old_pair = previous.clone();
            let mut member = old_pair.cache_key.cohort[0].clone();
            member.source_id = "second".into();
            old_pair.cache_key.cohort.push(member.clone());
            let mut new_pair = next.clone();
            new_pair.cache_key.cohort.insert(0, member);
            assert!(crate::body_lease::append_extension(&old_pair, &new_pair));
            new_pair.cache_key.cohort.remove(0);
            assert!(!crate::body_lease::append_extension(&old_pair, &new_pair));
            let mut changed = next.clone();
            changed.source_handle = opaque("different");
            assert!(!crate::body_lease::append_extension(&previous, &changed));
            changed = next.clone();
            changed.enrollment.enrollment_id = opaque("different");
            assert!(!crate::body_lease::append_extension(&previous, &changed));
        }
        let valid = case == 0 || case == 11;
        assert_eq!(
            crate::body_lease::append_extension(&previous, &next),
            valid,
            "case {case}"
        );
        // Exercise the receiver manager independently of origin refusal and framing.
        let mut manager = crate::body_lease::LeaseManager::default();
        let principal = Principal {
            subject: "subject".into(),
            audit_id: id(9),
        };
        let (local, local_watch) = manager
            .create(
                &principal,
                a.scope.clone(),
                [3; 32],
                opaque("append"),
                &acls,
                &at,
            )
            .unwrap();
        manager
            .request(local, &a.scope, previous.receiver_binding.request_id)
            .unwrap();
        manager
            .verified(local, &a.scope, previous.clone(), 262_144)
            .unwrap();
        manager.delivered(local, &a.scope).unwrap();
        manager
            .acknowledge(
                local,
                &a.scope,
                next.receiver_binding.request_id,
                previous.receiver_binding.request_id,
                previous.continuation.as_ref().unwrap(),
                &at,
            )
            .unwrap();
        assert_eq!(
            manager.verified(local, &a.scope, next.clone(), 262_144),
            if valid {
                Ok(())
            } else {
                Err(Reason::ContinuationInvalid)
            },
            "receiver case {case}"
        );
        manager.close(local, &a.scope).unwrap();
        // Closure releases both the slot and byte reservation.
        let (replacement, replacement_watch) = manager
            .create(
                &principal,
                a.scope.clone(),
                [3; 32],
                opaque("append"),
                &acls,
                &at,
            )
            .unwrap();
        manager
            .request(replacement, &a.scope, previous.receiver_binding.request_id)
            .unwrap();
        manager
            .verified(replacement, &a.scope, previous.clone(), 262_144)
            .unwrap();
        let (second, second_watch) = manager
            .create(
                &principal,
                a.scope.clone(),
                [3; 32],
                opaque("append"),
                &acls,
                &at,
            )
            .unwrap();
        manager
            .request(second, &a.scope, previous.receiver_binding.request_id)
            .unwrap();
        manager
            .verified(second, &a.scope, previous.clone(), 262_144)
            .unwrap();
        drop((local_watch, replacement_watch, second_watch));
        // Source-driven cases also traverse the real origin and receiver service.
        if case <= 13 {
            let mut current = source.current.borrow_mut();
            current.cohort = next.cache_key.cohort.clone();
            current.cohort_id = next.cohort_id;
            current.activity_generation = opaque(&next.cache_key.activity_generation);
            current.transcript_generation = opaque(&next.cache_key.transcript_generation);
            drop(current);
            *source.evidence.borrow_mut() = next.append_continuity.clone();
            assert_eq!(
                client.step(),
                if valid {
                    Step::Delivered
                } else {
                    Step::Refused(Reason::ContinuationInvalid)
                },
                "origin case {case}"
            );
            assert_eq!(
                service.collect(lease),
                if valid {
                    Ok(true)
                } else {
                    Err(Reason::ContinuationInvalid)
                }
            );
            if !valid {
                assert_eq!(service.release(lease).err(), Some(Reason::LeaseClosed));
                let (replacement, replacement_watch) = subscribe(&mut service).unwrap();
                service.cancel(replacement).unwrap();
                drop(replacement_watch);
            }
        }
        drop(watcher);
        drop(service);
        drop(client);
        drop(receiver);
        drop(origin_audit);
        std::fs::remove_dir_all(root).unwrap();
    }
}
