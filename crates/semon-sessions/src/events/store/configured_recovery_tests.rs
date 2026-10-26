// Recovery uses the real current SQLite producer and its admitted SourceJournal.
mod current_recovery {
    use super::*;
    use crate::storage::source_observations::{
        SourceIntent, SourceMutationReceipt, SourceObservationStore, SourceOperation,
    };

    fn fixture(count: usize) -> Fixture {
        let f = Fixture::current("recovery-current");
        let options = crate::Options {
            source: crate::storage::HostSource::new(f.config.namespace.clone(), "a".repeat(64)).unwrap(),
            cache: f.root.join("index.json"),
            codex_home: f.root.join("codex"),
            claude_home: f.root.join("claude"),
            proc_root: f.root.join("proc"),
            ..Default::default()
        };
        fs::create_dir_all(&options.proc_root).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        let mut cache = EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let mut inputs = Vec::new();
        for n in 0..count {
            let relative = format!("sessions/2026/10/26/rollout-{n:03}.jsonl");
            let raw = options.codex_home.join(&relative);
            fs::create_dir_all(raw.parent().unwrap()).unwrap();
            fs::write(&raw, format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"recover-{n:03}\",\"cwd\":\"/sanitized\"}}}}\n")).unwrap();
            inputs.push(crate::Input { root: crate::InputRoot::Codex, path: relative });
        }
        for batch in inputs.chunks(64) {
            let prepared = crate::model::prepare_sources(&options, &mut cache, batch, 1_790_000_000_000).unwrap();
            assert_eq!(cache.publish_partial_catalog_fenced(&prepared).unwrap().0, Outcome::Written);
        }
        let db = connection(&f.config).unwrap();
        db.execute("INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_namespace(),'catalog_recovery_after','')", []).unwrap();
        f
    }

    fn journal(f: &Fixture) -> SourceJournal {
        SourceJournal::owned(NativeOwnership::restore(&f.config).unwrap())
    }
    fn intent(f: &Fixture, cursor: Option<&str>) -> SourceIntent {
        SourceIntent::Recover {
            operation: SourceOperation::capture(f.config.namespace.as_str(), "recover", &serde_json::to_vec(&cursor).unwrap()),
            cursor: cursor.map(str::to_owned),
        }
    }
    fn count(f: &Fixture, table: &str) -> i64 {
        f.db().query_row(&format!("SELECT count(*) FROM {table} WHERE namespace=?1"), [f.config.namespace.as_str()], |r| r.get(0)).unwrap()
    }
    fn cache(f: &Fixture) -> EventCache {
        let source = crate::storage::HostSource::new(f.config.namespace.clone(), "a".repeat(64)).unwrap();
        EventCache::open_scoped(&f.root.join("index.json"), &source).unwrap()
    }
    fn recover(j: &mut SourceJournal, i: &SourceIntent) -> (SourceMutationReceipt, bool) {
        let SourceIntent::Recover { operation, cursor } = i else { panic!("recovery intent") };
        j.recover(operation, cursor.as_deref()).unwrap()
    }

    #[test]
    fn recovery_receipt_unknown_ack_replays_original_done_after_restart_and_peer_progress() {
        let f = fixture(65);
        let mut j = journal(&f);
        let original = intent(&f, Some(""));
        j.persist_intent(&original).unwrap();
        FAIL_EFFECT.with(|p| p.set(Some("recovery_commit_reply")));
        assert_eq!(original.apply(&mut j), Err(StorageError::CommitUncertain));
        assert_eq!(count(&f, "catalog_backfill_sources"), 64);
        assert!(j.pending_intents().unwrap().is_empty());
        drop(j);
        let mut j = journal(&f);
        for done in [false, true] {
            let next = intent(&f, j.recovery_cursor().unwrap().as_deref());
            j.persist_intent(&next).unwrap();
            assert_eq!(recover(&mut j, &next), (SourceMutationReceipt::Applied, done));
        }
        let before = f.state();
        assert_eq!(recover(&mut j, &original), (SourceMutationReceipt::AlreadyApplied, false));
        assert_eq!(f.state(), before);
        assert_eq!(count(&f, "catalog_backfill_sources"), 65);
    }

    #[test]
    fn recovery_owner_receipt_fault_rolls_back_page_receipt_and_ack() {
        let f = fixture(1);
        let mut j = journal(&f);
        let i = intent(&f, Some(""));
        j.persist_intent(&i).unwrap();
        let before = f.state();
        FAIL_EFFECT.with(|p| p.set(Some("recovery_receipt")));
        assert!(i.apply(&mut j).is_err());
        assert_eq!(f.state(), before);
        assert_eq!(j.pending_intents().unwrap(), vec![i.clone()]);
        assert_eq!(j.recovery_cursor().unwrap().as_deref(), Some(""));
        assert_eq!(recover(&mut j, &i), (SourceMutationReceipt::Applied, false));
        assert!(j.pending_intents().unwrap().is_empty());
        assert_eq!(count(&f, "catalog_backfill_sources"), 1);
    }

    #[test]
    fn recovery_real_receipt_trigger_fault_rolls_back_page() {
        let f = fixture(1);
        let mut j = journal(&f);
        let i = intent(&f, Some(""));
        j.persist_intent(&i).unwrap();
        f.db().execute_batch("CREATE TRIGGER fail_current_receipt BEFORE INSERT ON source_observation_receipts WHEN NEW.recovery_done IS NOT NULL BEGIN SELECT RAISE(ABORT,'receipt fault'); END;").unwrap();
        let before = f.state();
        assert!(i.apply(&mut j).is_err());
        assert_eq!(f.state(), before);
        f.db().execute_batch("DROP TRIGGER fail_current_receipt;").unwrap();
        assert_eq!(recover(&mut j, &i).0, SourceMutationReceipt::Applied);
    }

    #[test]
    fn recovery_cursor_conflict_has_terminal_receipt_and_no_page() {
        let f = fixture(1);
        let mut j = journal(&f);
        let i = intent(&f, Some(""));
        j.persist_intent(&i).unwrap();
        let db = connection(&f.config).unwrap();
        db.execute("UPDATE main.meta SET value='/newer' WHERE namespace=semon_namespace() AND key='catalog_recovery_after'", []).unwrap();
        assert_eq!(recover(&mut j, &i), (SourceMutationReceipt::Conflict, false));
        assert_eq!(count(&f, "catalog_backfill_sources"), 0);
        assert!(j.pending_intents().unwrap().is_empty());
        db.execute("UPDATE main.meta SET value='' WHERE namespace=semon_namespace() AND key='catalog_recovery_after'", []).unwrap();
        let before = f.state();
        assert_eq!(recover(&mut j, &i), (SourceMutationReceipt::Conflict, false));
        assert_eq!(f.state(), before);
    }

    #[test]
    fn recovery_event_cache_one_page_per_call() {
        let f = fixture(130);
        let mut c = cache(&f);
        for expected in [64, 128, 130] {
            assert!(!c.discover_recovery_sources().unwrap());
            assert_eq!(count(&f, "catalog_backfill_sources"), expected);
        }
        assert!(c.discover_recovery_sources().unwrap());
        assert_eq!(count(&f, "source_observation_intents"), 0);
    }

    #[test]
    fn recovery_event_cache_reconciles_same_unknown_ack_without_second_page() {
        let f = fixture(65);
        let mut c = cache(&f);
        FAIL_EFFECT.with(|p| p.set(Some("recovery_commit_reply")));
        assert!(c.discover_recovery_sources().is_err());
        let original = c.pending_source_operation.clone().unwrap();
        assert_eq!(count(&f, "catalog_backfill_sources"), 64);
        assert!(!c.discover_recovery_sources().unwrap());
        assert!(c.pending_source_operation.is_none());
        assert_eq!(count(&f, "catalog_backfill_sources"), 64);
        let digest: String = f.db().query_row("SELECT payload_digest FROM source_observation_receipts WHERE namespace=?1 AND operation_id=?2", params![f.config.namespace.as_str(), original.operation().id], |r| r.get(0)).unwrap();
        assert_eq!(digest, original.operation().payload_digest);
    }

    #[test]
    fn recovery_event_cache_restart_replays_pending_capture() {
        let f = fixture(65);
        let i = intent(&f, Some(""));
        journal(&f).persist_intent(&i).unwrap();
        let mut c = cache(&f);
        c.reconcile_pending_source_operation().unwrap();
        assert_eq!(count(&f, "catalog_backfill_sources"), 64);
        assert!(!c.discover_recovery_sources().unwrap());
        assert_eq!(count(&f, "catalog_backfill_sources"), 64);
        assert_eq!(count(&f, "source_observation_intents"), 0);
    }

    #[test]
    fn recovery_event_cache_binding_namespace_is_not_replaced() {
        let f = fixture(1);
        let mut c = cache(&f);
        c.observation_namespace = "foreign-binding".into();
        let before = f.state();
        assert!(c.discover_recovery_sources().is_err());
        assert_eq!(f.state(), before);
        assert_eq!(c.pending_source_operation.as_ref().unwrap().operation().namespace, "foreign-binding");
    }

    #[test]
    fn recovery_receipt_trigger_cannot_complete_over_new_marker() {
        let f = fixture(0);
        let mut j = journal(&f);
        let i = intent(&f, Some(""));
        j.persist_intent(&i).unwrap();
        f.db().execute_batch("CREATE TRIGGER request_current_recovery AFTER INSERT ON source_observation_receipts WHEN NEW.recovery_done IS NOT NULL BEGIN INSERT OR REPLACE INTO meta(namespace,key,value) VALUES(NEW.namespace,'catalog_recovery_after',''); END;").unwrap();
        assert_eq!(recover(&mut j, &i), (SourceMutationReceipt::Applied, true));
        assert!(!j.recovery_is_current().unwrap());
        assert_eq!(j.recovery_cursor().unwrap().as_deref(), Some(""));
        let before = f.state();
        assert_eq!(recover(&mut j, &i), (SourceMutationReceipt::AlreadyApplied, true));
        assert_eq!(f.state(), before);
    }

    #[test]
    fn recovery_event_cache_defers_competing_pending_pages() {
        let f = fixture(65);
        let first = intent(&f, Some(""));
        let second = intent(&f, Some(""));
        let mut j = journal(&f);
        j.persist_intent(&first).unwrap();
        j.persist_intent(&second).unwrap();
        let mut c = cache(&f);
        c.reconcile_pending_source_operation().unwrap();
        assert!(!c.discover_recovery_sources().unwrap());
        assert_eq!(count(&f, "catalog_backfill_sources"), 64);
        assert_eq!(count(&f, "source_observation_intents"), 1);
        assert!(!c.discover_recovery_sources().unwrap());
        assert_eq!(count(&f, "catalog_backfill_sources"), 64);
        assert_eq!(count(&f, "source_observation_intents"), 0);
        let result: String = f.db().query_row("SELECT outcome FROM source_observation_receipts WHERE namespace=?1 AND operation_id=?2", params![f.config.namespace.as_str(), second.operation().id], |r| r.get(0)).unwrap();
        assert_eq!(result, "conflict");
    }

    #[test]
    fn recovery_unknown_ack_keeps_exact_operation_identity() {
        let f = fixture(1);
        let mut j = journal(&f);
        let i = intent(&f, Some(""));
        j.persist_intent(&i).unwrap();
        FAIL_EFFECT.with(|p| p.set(Some("recovery_commit_reply")));
        assert_eq!(i.apply(&mut j), Err(StorageError::CommitUncertain));
        let before = f.state();
        assert_eq!(i.apply(&mut journal(&f)).unwrap(), SourceMutationReceipt::AlreadyApplied);
        assert_eq!(f.state(), before);
    }

    #[test]
    fn recovery_epoch_change_refuses_pending_intent_without_effect() {
        let f = fixture(1);
        let mut j = journal(&f);
        let i = intent(&f, Some(""));
        j.persist_intent(&i).unwrap();
        let mut wrong = f.config.clone();
        wrong.expected_stream_epoch = Some("b".repeat(64));
        let before = f.state();
        assert!(NativeOwnership::restore(&wrong).is_err());
        assert_eq!(j.pending_intents().unwrap(), vec![i]);
        assert_eq!(f.state(), before);
    }

    #[test]
    fn recovery_foreign_operation_refuses_without_receipt() {
        let f = fixture(1);
        let mut j = journal(&f);
        let op = SourceOperation::capture("foreign", "recover", &serde_json::to_vec(&Some("")).unwrap());
        let before = f.state();
        assert_eq!(j.recover(&op, Some("")), Err(StorageError::NamespaceMismatch));
        assert_eq!(f.state(), before);
    }
}
