mod current_history_custody {
    use super::*;
    use crate::history_projection::{catalog, qualified};
    use crate::slot_projection::Publication;
    use crate::storage::sqlite::semantic::AdmittedSource;

    fn preserve_original_harness(tx: &Transaction<'_>, old: &crate::model::summary::CatalogRow, source: &AdmittedSource) -> rusqlite::Result<bool> {
        crate::storage::catalog::preserve_original(&crate::storage::sqlite::history::OriginalHistory::admitted(tx, source).map_err(|_| rusqlite::Error::InvalidQuery)?, old)
    }
    fn publish(tx: &Transaction<'_>, publication: &Publication<'_>, source: &AdmittedSource) -> rusqlite::Result<()> {
        crate::storage::catalog::publish_history_rows(&crate::storage::sqlite::history::OriginalHistory::admitted(tx, source).map_err(|_| rusqlite::Error::InvalidQuery)?, publication.rows, publication.transcripts)
    }
    fn encode(value: &impl serde::Serialize) -> rusqlite::Result<String> { serde_json::to_string(value).map_err(|_| rusqlite::Error::InvalidQuery) }
    fn decode<T: serde::de::DeserializeOwned>(value: &str) -> rusqlite::Result<T> { serde_json::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery) }

    fn fixture(recipes: usize, padding: usize) -> (Connection, crate::model::summary::CatalogRow, Vec<(i64, String)>, AdmittedSource) {
        let root = scratch("history-custody-current");
        let (mut store, _) = opened(&root.join("index.sqlite3"));
        store.publish_catalog_with_generation(&Publication { rows: &[], transcripts: None }, None, None, false, None).unwrap();
        let source = AdmittedSource::native(store.owner.clone());
        let db = store.connection;
        let old: crate::model::summary::CatalogRow = serde_json::from_value(serde_json::json!({"key":"old","name":"original","harness":"codex","kind":"session","native_ids":["old"],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":"","effort":null,"start":null,"last":null,"tokens":[0,0,0],"cost":serde_json::to_value(crate::pricing::Cost::default()).unwrap(),"sources":[]})).unwrap();
        db.execute(
            "INSERT INTO main.session_slot_projections(namespace,session_key,generation,version,source_generation,total) VALUES(semon_namespace(),'old','original-generation',4,NULL,?1)",
            [recipes as i64],
        )
        .unwrap();
        let mut saved = Vec::new();
        for n in 0..recipes {
            // Unsupported originals are still custody, including arbitrary UTF-8.
            let text = serde_json::json!({"original":n,"padding":"水".repeat(padding)}).to_string();
            db.execute(
                "INSERT INTO main.session_slots(namespace,session_key,slot,metadata) VALUES(semon_namespace(),'old',?1,?2)",
                params![n as i64, text],
            )
            .unwrap();
            saved.push((n as i64, text));
        }
        (db, old, saved, source)
    }
    fn step(db: &mut Connection, old: &crate::model::summary::CatalogRow, source: &AdmittedSource) -> bool {
        let tx = db.transaction().unwrap();
        let done = preserve_original_harness(&tx, old, source).unwrap();
        tx.commit().unwrap();
        done
    }
    fn slots(db: &Connection) -> Vec<(i64, String)> {
        db.prepare("SELECT slot,metadata FROM session_history_slots UNION ALL SELECT slot,CAST(group_concat(bytes,'') AS TEXT) FROM (SELECT slot,bytes FROM session_history_slot_parts ORDER BY slot,part) GROUP BY slot ORDER BY slot").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }
    #[test]
    fn custody_pages_resume_without_exposing_partial_history() {
        for count in [63, 64, 65, 1025] {
            let (mut db, old, original, source) = fixture(count, 0);
            let mut steps = 0;
            loop {
                let before = slots(&db).len();
                let done = step(&mut db, &old, &source);
                steps += 1;
                assert!(slots(&db).len() - before <= 64);
                if done {
                    break;
                }
                assert!(catalog(&db, &old.key).unwrap().is_none());
                assert!(!qualified(&db, &old).unwrap());
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM session_history_projections",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    0
                );
                // A new connection/process needs only this durable checkpoint.
                assert!(db.is_autocommit());
                assert!(steps <= count.div_ceil(64));
            }
            assert_eq!(steps, count.div_ceil(64));
            assert_eq!(slots(&db), original);
            assert!(step(&mut db, &old, &source));
            assert_eq!(slots(&db), original);
            let incomplete: bool = db
                .query_row(
                    "SELECT incomplete FROM session_history_projections",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(incomplete, "unsupported originals gained authority");
            assert_eq!(
                db.query_row(
                    "SELECT value FROM meta WHERE key='history_original_header:old'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                encode(&Some((
                    "original-generation",
                    4,
                    Option::<String>::None,
                    count
                )))
                .unwrap()
            );
        }
    }
    #[test]
    fn custody_byte_pages_preserve_utf8_and_oversized_unsupported_recipes() {
        for padding in [349510, 349520, 800000] {
            let (mut db, old, original, source) = fixture(3, padding);
            let mut done = false;
            for _ in 0..16 {
                let before:i64=db.query_row("SELECT coalesce((SELECT sum(octet_length(metadata)) FROM session_history_slots),0)+coalesce((SELECT sum(length(bytes)) FROM session_history_slot_parts),0)",[],|r|r.get(0)).unwrap();
                done = step(&mut db, &old, &source);
                let after:i64=db.query_row("SELECT coalesce((SELECT sum(octet_length(metadata)) FROM session_history_slots),0)+coalesce((SELECT sum(length(bytes)) FROM session_history_slot_parts),0)",[],|r|r.get(0)).unwrap();
                assert!(after - before <= 1048576);
                if done {
                    break;
                }
                assert!(catalog(&db, &old.key).unwrap().is_none());
            }
            assert!(done);
            assert_eq!(slots(&db), original);
        }
    }
    #[test]
    fn completed_original_is_immutable_when_current_reuses_its_harness() {
        let (mut db, mut current, original, source) = fixture(65, 0);
        while !step(&mut db, &current, &source) {}
        let metadata = catalog(&db, &current.key).unwrap();
        let header: String = db
            .query_row(
                "SELECT value FROM meta WHERE key='history_original_header:old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        current.name = "New Current with original harness".into();
        let tx = db.transaction().unwrap();
        publish(
            &tx,
            &Publication {
                rows: &[current.clone()],
                transcripts: Some(&[crate::slot_projection::Projection {
                    key: current.key.clone(),
                    slots: Vec::new(),
                    repair: false,
                }]),
            },
            &source,
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(catalog(&db, &current.key).unwrap(), metadata);
        assert_eq!(slots(&db), original);
        assert_eq!(
            db.query_row(
                "SELECT value FROM meta WHERE key='history_original_header:old'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            header
        );
        assert!(!qualified(&db, &current).unwrap());
    }
    #[test]
    fn custody_preserves_unsupported_original_slot_addresses() {
        let (mut db, old, mut original, source) = fixture(65, 0);
        db.execute("UPDATE session_slots SET slot=?1 WHERE slot=0", [i64::MIN])
            .unwrap();
        original[0].0 = i64::MIN;
        assert!(!step(&mut db, &old, &source));
        assert!(step(&mut db, &old, &source));
        assert_eq!(slots(&db), original);
    }
    #[test]
    fn custody_rollback_and_changed_original_restart_cleanly() {
        let (mut db, mut old, original, source) = fixture(130, 0);
        {
            let tx = db.transaction().unwrap();
            assert!(!preserve_original_harness(&tx, &old, &source).unwrap());
            // Cancellation/failure before commit rolls back rows and checkpoint.
        }
        assert!(slots(&db).is_empty());
        assert!(!step(&mut db, &old, &source));
        old.name = "changed original generation".into();
        db.execute(
            "UPDATE session_slot_projections SET generation='replacement-generation'",
            [],
        )
        .unwrap();
        assert!(!step(&mut db, &old, &source));
        assert!(slots(&db).is_empty());
        assert!(!step(&mut db, &old, &source)); // retire stale checkpoint
        assert!(!step(&mut db, &old, &source));
        assert!(!step(&mut db, &old, &source));
        assert!(step(&mut db, &old, &source));
        assert_eq!(slots(&db), original);
        let saved: crate::model::summary::CatalogRow = decode(&catalog(&db, &old.key).unwrap().unwrap()).unwrap();
        assert_eq!(saved.name, old.name);
        assert_eq!(
            db.query_row(
                "SELECT generation FROM session_history_projections",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "replacement-generation"
        );
    }
}

// Frozen pre-extraction custody implementation, CI oracle only.
