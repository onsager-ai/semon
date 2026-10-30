use super::*;
use crate::events::EventCache;
use crate::storage::postgres::tests::{LOSE_ACK, fixture, ns};
use crate::storage::source_observations::conformance::shared_source_observation_conformance;
use crate::storage::source_observations::{CatalogPendingBatch, CatalogPendingSearch};

fn journal(pool: &PgPool, namespace: &str) -> PgSourceJournal {
    PgSourceJournal {
        pool: pool.clone(),
        namespace: ns(namespace),
        control: PgCancellation::default(),
    }
}
fn discover(namespace: &str, paths: &[String], complete: bool) -> SourceIntent {
    SourceIntent::Discover {
        operation: SourceOperation::capture(
            namespace,
            "discover",
            &serde_json::to_vec(&(paths, complete)).unwrap(),
        ),
        paths: paths.to_vec(),
        complete,
    }
}

#[test]
fn pg_source_shared_conformance_restart_cas_partial_foreign_and_nul() {
    let (pool, _) = fixture(|_| {});
    shared_source_observation_conformance(
        || Box::new(journal(&pool, "source\0injected'; SET ROLE x; --")),
        "source\0injected'; SET ROLE x; --",
    );
}

#[test]
fn pg_source_intent_unknown_commit_reopen_binding_and_terminal_ack() {
    let (pool, _) = fixture(|_| {});
    let namespace = "intent\0scope";
    let mut store = journal(&pool, namespace);
    let original = discover(namespace, &["first".into()], false);
    store.persist_intent(&original).unwrap();
    assert_eq!(
        store.acknowledge_intent(original.operation()),
        Err(StorageError::Conflict)
    );
    LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(
        original.apply(&mut store),
        Err(StorageError::CommitUncertain)
    );
    let before = store.claims(&["first".into()]).unwrap();
    drop(store);
    let mut store = journal(&pool, namespace);
    assert_eq!(store.pending_intents().unwrap(), vec![original.clone()]);
    for _ in 0..2 {
        assert_eq!(
            original.apply(&mut store).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
    }
    assert_eq!(store.claims(&["first".into()]).unwrap(), before);
    let mut changed = discover(namespace, &["rebound".into()], true);
    if let SourceIntent::Discover { operation, .. } = &mut changed {
        operation.id = original.operation().id.clone();
    }
    assert_eq!(store.persist_intent(&changed), Err(StorageError::Conflict));
    assert_eq!(
        changed.apply(&mut store).unwrap(),
        SourceMutationReceipt::Conflict
    );
    assert_eq!(
        store.acknowledge_intent(changed.operation()),
        Err(StorageError::Conflict)
    );
    assert_eq!(store.pending_intents().unwrap(), vec![original.clone()]);
    assert_eq!(
        store.claims(&["rebound".into()]).unwrap(),
        vec![("rebound".into(), None)]
    );
    store.acknowledge_intent(original.operation()).unwrap();
    assert!(store.pending_intents().unwrap().is_empty());
    let stale = SourceIntent::Invalidate {
        operation: SourceOperation::capture(
            namespace,
            "invalidate",
            &serde_json::to_vec(&vec![("first".to_owned(), None::<i64>)]).unwrap(),
        ),
        expected: vec![("first".into(), None)],
    };
    store.persist_intent(&stale).unwrap();
    LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(stale.apply(&mut store), Err(StorageError::CommitUncertain));
    assert_eq!(
        stale.apply(&mut store).unwrap(),
        SourceMutationReceipt::Conflict
    );
    assert_eq!(store.claims(&["first".into()]).unwrap(), before);
    store.acknowledge_intent(stale.operation()).unwrap();
}

#[test]
fn pg_source_queue_bound_capture_order_and_foreign_zero_effects() {
    let (pool, _) = fixture(|_| {});
    let mut store = journal(&pool, "queue");
    let mut intents = Vec::new();
    for i in 0..256 {
        let intent = discover("queue", &[format!("source-{i}")], false);
        store.persist_intent(&intent).unwrap();
        intents.push(intent);
    }
    assert_eq!(store.pending_intents().unwrap(), intents);
    assert_eq!(
        store.persist_intent(&discover("queue", &["overflow".into()], true)),
        Err(StorageError::Busy)
    );
    let foreign = discover("foreign", &["foreign".into()], true);
    assert_eq!(store.persist_intent(&foreign), Err(StorageError::Conflict));
    assert_eq!(
        foreign.apply(&mut store).unwrap(),
        SourceMutationReceipt::Conflict
    );
    assert_eq!(store.pending_intents().unwrap(), intents);
    assert!(
        journal(&pool, "foreign")
            .pending_intents()
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .pending_catalog_sources(256, None)
            .unwrap()
            .paths
            .is_empty()
    );
    intents[0].apply(&mut store).unwrap();
    store.acknowledge_intent(intents[0].operation()).unwrap();
    let last = discover("queue", &["after-slot".into()], true);
    store.persist_intent(&last).unwrap();
    intents.remove(0);
    intents.push(last);
    assert_eq!(store.pending_intents().unwrap(), intents);
}

#[test]
fn pg_source_forced_rls_namespace_token_injection_and_no_unscoped_rows() {
    let (pool, factory) = fixture(|_| {});
    let a = "A\0'; RESET ROLE; --";
    let intent = discover(a, &["path\0bytes".into()], true);
    intent.apply(&mut journal(&pool, a)).unwrap();
    assert_eq!(
        journal(&pool, a)
            .pending_catalog_sources(1, None)
            .unwrap()
            .paths,
        vec!["path\0bytes"]
    );
    assert!(
        journal(&pool, "B")
            .pending_catalog_sources(1, None)
            .unwrap()
            .paths
            .is_empty()
    );
    for table in [
        "semon_pg_source_state",
        "semon_pg_source_members",
        "semon_pg_source_invalidations",
        "semon_pg_source_receipts",
        "semon_pg_source_intents",
    ] {
        let mut client = factory.client();
        assert_eq!(
            client
                .query_one(&format!("SELECT count(*) FROM {table}"), &[])
                .unwrap()
                .get::<_, i64>(0),
            0
        );
        client.batch_execute("BEGIN").unwrap();
        client
            .query_one(
                "SELECT set_config('semon.namespace',$1,true)",
                &[&super::super::namespace_context(&ns("B"))],
            )
            .unwrap();
        let columns = match table {
            "semon_pg_source_state" => "(namespace) VALUES($1)",
            "semon_pg_source_members" => "(namespace,source_path) VALUES($1,'x'::bytea)",
            "semon_pg_source_invalidations" => {
                "(namespace,source_path,revision,reason,observed_at) VALUES($1,'x'::bytea,1,'changed',1)"
            }
            "semon_pg_source_receipts" => {
                "(namespace,operation_id,payload_digest,outcome) VALUES($1,'x',repeat('a',64),'applied')"
            }
            "semon_pg_source_intents" => {
                "(namespace,operation_id,payload_digest,payload) VALUES($1,'x',repeat('a',64),'{}')"
            }
            _ => unreachable!(),
        };
        let error = client
            .execute(&format!("INSERT INTO {table}{columns}"), &[&a.as_bytes()])
            .unwrap_err();
        assert_eq!(
            error.code().map(|code| code.code()),
            Some("42501"),
            "forced RLS refuses an otherwise valid foreign write"
        );
        client.batch_execute("ROLLBACK").unwrap();
    }
    // An actual cross-scope update of an otherwise valid row also changes zero.
    pool.transaction(&ns("B"), TxnKind::Write, &PgCancellation::default(), |tx| {
        assert_eq!(tx.driver.execute("UPDATE semon_pg_source_state SET backfill_state='complete' WHERE namespace_digest=sha256($1::bytea)", &[&a.as_bytes()]).map_err(sqlstate::error)?, 0);
        Ok(())
    }).unwrap();
    assert_eq!(
        journal(&pool, a).backfill_state().unwrap().as_deref(),
        Some("draining")
    );
}

#[test]
fn pg_source_event_cache_actual_producer_commit_ack_loss_restart_before_later_payload() {
    let (pool, _) = fixture(|_| {});
    let namespace = "producer\0source";
    let root = std::env::temp_dir().join("pg-observation-no-native-store");
    let options = crate::Options {
        claude_home: root.join("claude"),
        cache: root.join("index.json"),
        ..crate::Options::default()
    };
    let first = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/p/first.jsonl".into(),
    };
    let later = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/p/later.jsonl".into(),
    };
    let first_path = first.full_path(&options).to_string_lossy().into_owned();
    let later_path = later.full_path(&options).to_string_lossy().into_owned();
    let open = || {
        EventCache::source_observation_test_producer(
            namespace,
            &root,
            Box::new(journal(&pool, namespace)),
        )
        .unwrap()
    };
    let mut cache = open();
    // persist_intent commits first. A typed wrapper loses the effect ACK only,
    // rather than accidentally consuming the fault on the intent transaction.
    cache.inject_source_observations(Box::new(LoseEffectAck {
        inner: journal(&pool, namespace),
        armed: true,
    }));
    assert!(
        cache
            .discover_catalog_sources(&options, &[first], false)
            .is_err()
    );
    let store = journal(&pool, namespace);
    let pending = store.pending_intents().unwrap();
    assert_eq!(pending.len(), 1);
    let before = store.claims(std::slice::from_ref(&first_path)).unwrap();
    assert!(before[0].1.is_some());
    drop(cache);
    let mut cache = open(); // actual EventCache open binding reconciles same ID
    assert!(store.pending_intents().unwrap().is_empty());
    cache
        .discover_catalog_sources(&options, &[later], true)
        .unwrap();
    assert_eq!(
        store.claims(std::slice::from_ref(&first_path)).unwrap(),
        before
    );
    assert!(store.claims(&[later_path]).unwrap()[0].1.is_some());
    assert!(store.pending_intents().unwrap().is_empty());
    assert_eq!(
        pending[0].apply(&mut journal(&pool, namespace)).unwrap(),
        SourceMutationReceipt::AlreadyApplied
    );
    pool.transaction(
        &ns(namespace),
        TxnKind::ReadSnapshot,
        &PgCancellation::default(),
        |tx| {
            let count: i64 = tx
                .driver
                .query_one("SELECT count(*) FROM semon_pg_source_receipts", &[])
                .map_err(sqlstate::error)?
                .get(0);
            assert_eq!(
                count, 2,
                "one original operation, one different later payload"
            );
            Ok(())
        },
    )
    .unwrap();
}

struct LoseEffectAck {
    inner: PgSourceJournal,
    armed: bool,
}
impl SourceObservationStore for LoseEffectAck {
    fn persist_intent(&mut self, intent: &SourceIntent) -> Result<(), StorageError> {
        self.inner.persist_intent(intent)
    }
    fn pending_intents(&self) -> Result<Vec<SourceIntent>, StorageError> {
        self.inner.pending_intents()
    }
    fn acknowledge_intent(&mut self, op: &SourceOperation) -> Result<(), StorageError> {
        self.inner.acknowledge_intent(op)
    }
    fn discover(
        &mut self,
        op: &SourceOperation,
        paths: &[String],
        complete: bool,
    ) -> Result<SourceMutationReceipt, StorageError> {
        if self.armed {
            self.armed = false;
            LOSE_ACK.with(|flag| flag.set(true));
        }
        self.inner.discover(op, paths, complete)
    }
    fn invalidate(
        &mut self,
        op: &SourceOperation,
        expected: &[(String, Option<i64>)],
    ) -> Result<SourceMutationReceipt, StorageError> {
        self.inner.invalidate(op, expected)
    }
    fn claims(&self, paths: &[String]) -> Result<SourceClaims, StorageError> {
        self.inner.claims(paths)
    }
    fn pending_catalog_sources(
        &self,
        limit: usize,
        search: Option<&CatalogPendingSearch<'_>>,
    ) -> Result<CatalogPendingBatch, StorageError> {
        self.inner.pending_catalog_sources(limit, search)
    }
    fn backfill_state(&self) -> Result<Option<String>, StorageError> {
        self.inner.backfill_state()
    }
    fn finish_backfill(
        &mut self,
        op: &SourceOperation,
        generation: Option<&str>,
    ) -> Result<SourceMutationReceipt, StorageError> {
        self.inner.finish_backfill(op, generation)
    }
}

#[test]
fn pg_source_finish_generation_fixture_is_real_but_native_pg_publisher_remains_unported() {
    let (pool, _) = fixture(|_| {});
    let root = std::env::temp_dir().join(format!(
        "pg-source-generation-{}-{}",
        std::process::id(),
        crate::model::now_ms()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut publisher = EventCache::open_scoped(&root.join("index.json")).unwrap();
    let generation = publisher
        .publish_catalog_projection_fenced(
            &crate::slot_projection::Publication {
                rows: &[],
                transcripts: None,
            },
            None,
        )
        .unwrap()
        .expect("actual current publisher generation");
    drop(publisher);
    let namespace = "finish\0head";
    let mut store = journal(&pool, namespace);
    discover(namespace, &[], true).apply(&mut store).unwrap();
    // This is contract-input fixture setup, NOT a PG Catalog/publisher adapter.
    // No invalidation is deleted or fabricated as acknowledged work.
    pool.transaction(&ns(namespace), TxnKind::Write, &PgCancellation::default(), |tx| {
        tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2,semantic_pending=true WHERE namespace_digest=sha256($1::bytea)", &[&namespace.as_bytes(), &generation]).map_err(sqlstate::error)?;
        Ok(())
    }).unwrap();
    let finish = |value: &str| SourceIntent::FinishBackfill {
        operation: SourceOperation::capture(
            namespace,
            "finish_backfill",
            &serde_json::to_vec(&Some(value)).unwrap(),
        ),
        expected_generation: Some(value.to_owned()),
    };
    let blocked = finish(&generation);
    store.persist_intent(&blocked).unwrap();
    assert_eq!(
        blocked.apply(&mut store).unwrap(),
        SourceMutationReceipt::Conflict
    );
    store.acknowledge_intent(blocked.operation()).unwrap();
    pool.transaction(&ns(namespace), TxnKind::Write, &PgCancellation::default(), |tx| {
        tx.driver.execute("UPDATE semon_pg_source_state SET semantic_pending=false WHERE namespace_digest=sha256($1::bytea)", &[&namespace.as_bytes()]).map_err(sqlstate::error)?;
        Ok(())
    }).unwrap();
    let wrong = finish("wrong-generation");
    assert_eq!(
        wrong.apply(&mut store).unwrap(),
        SourceMutationReceipt::Conflict
    );
    assert_eq!(store.backfill_state().unwrap().as_deref(), Some("draining"));
    let intent = finish(&generation);
    store.persist_intent(&intent).unwrap();
    LOSE_ACK.with(|flag| flag.set(true));
    assert_eq!(intent.apply(&mut store), Err(StorageError::CommitUncertain));
    drop(store);
    let mut store = journal(&pool, namespace);
    assert_eq!(
        intent.apply(&mut store).unwrap(),
        SourceMutationReceipt::AlreadyApplied
    );
    store.acknowledge_intent(intent.operation()).unwrap();
    assert_eq!(store.backfill_state().unwrap().as_deref(), Some("complete"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pg_source_concurrent_claim_cas_serializes_one_winner() {
    let (pool, _) = fixture(|_| {});
    let namespace = "concurrent";
    let paths = vec!["source".to_owned()];
    discover(namespace, &paths, true)
        .apply(&mut journal(&pool, namespace))
        .unwrap();
    let expected = journal(&pool, namespace).claims(&paths).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..2 {
            let expected = expected.clone();
            let barrier = barrier.clone();
            let mut store = journal(&pool, namespace);
            handles.push(scope.spawn(move || {
                let intent = SourceIntent::Invalidate {
                    operation: SourceOperation::capture(
                        namespace,
                        "invalidate",
                        &serde_json::to_vec(&expected).unwrap(),
                    ),
                    expected,
                };
                store.persist_intent(&intent).unwrap();
                barrier.wait();
                let result = intent.apply(&mut store).unwrap();
                store.acknowledge_intent(intent.operation()).unwrap();
                result
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == SourceMutationReceipt::Applied)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == SourceMutationReceipt::Conflict)
            .count(),
        1
    );
    let after = journal(&pool, namespace).claims(&paths).unwrap();
    assert_eq!(after[0].1, expected[0].1.map(|revision| revision + 1));
    assert!(
        journal(&pool, namespace)
            .pending_intents()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn pg_source_effect_fault_rolls_back_receipt_membership_and_revision_together() {
    let (pool, factory) = fixture(|_| {});
    factory.client().batch_execute("CREATE FUNCTION source_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.source_path=convert_to('fault','UTF8') THEN RAISE EXCEPTION 'injected source fault'; END IF; RETURN NEW; END; $$; CREATE TRIGGER source_fault BEFORE INSERT ON semon_pg_source_members FOR EACH ROW EXECUTE FUNCTION source_fault();").unwrap();
    let namespace = "rollback";
    let intent = discover(namespace, &["first".into(), "fault".into()], true);
    let mut store = journal(&pool, namespace);
    store.persist_intent(&intent).unwrap();
    assert_eq!(intent.apply(&mut store), Err(StorageError::Unavailable));
    drop(store);
    let mut store = journal(&pool, namespace);
    assert_eq!(store.pending_intents().unwrap(), vec![intent.clone()]);
    assert!(
        store
            .pending_catalog_sources(256, None)
            .unwrap()
            .paths
            .is_empty()
    );
    assert_eq!(
        store.claims(&["first".into(), "fault".into()]).unwrap(),
        vec![("first".into(), None), ("fault".into(), None)]
    );
    assert_eq!(
        store.acknowledge_intent(intent.operation()),
        Err(StorageError::Conflict)
    );
    factory
        .client()
        .batch_execute("DROP TRIGGER source_fault ON semon_pg_source_members")
        .unwrap();
    assert_eq!(
        intent.apply(&mut store).unwrap(),
        SourceMutationReceipt::Applied
    );
    assert_eq!(
        store.claims(&["first".into(), "fault".into()]).unwrap(),
        vec![("first".into(), Some(1)), ("fault".into(), Some(2))]
    );
    store.acknowledge_intent(intent.operation()).unwrap();
}

#[test]
fn pg_source_recovery_refuses_before_any_namespace_write() {
    let (pool, factory) = fixture(|_| {});
    let mut store = journal(&pool, "recovery-refusal");
    let operation = SourceOperation::capture(
        "recovery-refusal",
        "recover",
        &serde_json::to_vec(&Some("")).unwrap(),
    );
    let intent = SourceIntent::Recover {
        operation: operation.clone(),
        cursor: Some(String::new()),
    };
    assert_eq!(store.recovery_is_current(), Err(StorageError::Unavailable));
    assert_eq!(store.recovery_cursor(), Err(StorageError::Unavailable));
    assert_eq!(
        store.persist_intent(&intent),
        Err(StorageError::Unavailable)
    );
    assert_eq!(
        store.recover(&operation, Some("")),
        Err(StorageError::Unavailable)
    );
    assert_eq!(intent.apply(&mut store), Err(StorageError::Unavailable));
    assert_eq!(store.apply(&intent), Err(StorageError::Unavailable));
    let counts = factory.client().query_one(
        "SELECT (SELECT count(*) FROM semon_pg_source_state), (SELECT count(*) FROM semon_pg_source_intents), (SELECT count(*) FROM semon_pg_source_receipts)", &[],
    ).unwrap();
    for column in 0..3 {
        assert_eq!(counts.get::<_, i64>(column), 0);
    }
}
