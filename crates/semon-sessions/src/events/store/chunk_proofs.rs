//! Paged content-free byte proofs, owned by the existing native ledger CAS.
use super::*;
use crate::source_chunks::Chunks;
use std::sync::Arc;

const PAGE: usize = 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    parser_version: u32,
    ledger: String,
    count: usize,
    fingerprint: [u8; 32],
    #[serde(default)]
    resume: Option<crate::source_chunks::PrefixState>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    version: u32,
    index: usize,
    hashes: Vec<[u8; 32]>,
}

fn prefix(path: &str) -> String {
    // Fixed ASCII keys make exact namespace cleanup independent of path glob
    // characters; host paths and raw source bytes never enter proof values.
    format!(
        "native_source_chunks:{:x}:",
        Sha256::digest(path.as_bytes())
    )
}

pub(super) fn digest(connection: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let encoded: Option<Option<String>> = connection
        .query_row(
            NATIVE_METADATA_VALUE,
            [format!("{}header", prefix(path))],
            |row| row.get(0),
        )
        .optional()?;
    Ok(encoded
        .flatten()
        .map(|value| format!("{:x}", Sha256::digest(value.as_bytes()))))
}

pub(super) fn read(
    connection: &Connection,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<Arc<Chunks>>> {
    let prefix = prefix(path);
    let header: Option<Option<String>> = connection
        .query_row(NATIVE_METADATA_VALUE, [format!("{prefix}header")], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(header) = header
        .flatten()
        .and_then(|text| serde_json::from_str::<Header>(&text).ok())
    else {
        return Ok(None);
    };
    if header.version != 1
        || header.parser_version != CACHE_VERSION
        || header.ledger != header_ledger(ledger)
        || usize::try_from(ledger.offset.div_ceil(crate::source_chunks::SIZE as u64))
            != Ok(header.count)
    {
        return Ok(None);
    }
    let mut statement = connection.prepare(
        "SELECT key, CASE WHEN length(CAST(value AS BLOB))<=?3 THEN value ELSE NULL END FROM meta WHERE key>=?1 AND key<?2 ORDER BY key"
    )?;
    let mut rows = statement.query(params![
        &prefix,
        format!("{prefix}~"),
        NATIVE_METADATA_MAX as i64
    ])?;
    let mut hashes = Vec::new();
    let mut page = 0;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        if key == format!("{prefix}header") {
            continue;
        }
        if key != format!("{prefix}{page:010}") {
            return Ok(None);
        }
        let Some(encoded) = row.get::<_, Option<String>>(1)? else {
            return Ok(None);
        };
        let Some(part) = serde_json::from_str::<Page>(&encoded).ok() else {
            return Ok(None);
        };
        let remaining = header.count.saturating_sub(hashes.len());
        if part.version != 1
            || part.index != page
            || part.hashes.len() != remaining.min(PAGE)
            || remaining == 0
        {
            return Ok(None);
        }
        hashes.extend(part.hashes);
        page += 1;
    }
    let chunks = Chunks {
        offset: ledger.offset,
        hashes,
        resume: header.resume,
    };
    if !chunks.valid(ledger.offset) || chunks.fingerprint() != header.fingerprint {
        return Ok(None);
    }
    Ok(Some(Arc::new(chunks)))
}

pub(super) fn clear(transaction: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let prefix = prefix(path);
    transaction.execute(
        "DELETE FROM meta WHERE key>=?1 AND key<?2",
        params![&prefix, format!("{prefix}~")],
    )?;
    Ok(())
}

pub(super) fn write(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    chunks: Option<&Chunks>,
) -> rusqlite::Result<()> {
    write_measured(
        transaction,
        path,
        ledger,
        chunks,
        &mut crate::native_timing::Measurements::default(),
    )
}

pub(super) fn write_measured(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    chunks: Option<&Chunks>,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let started = measurements.start();
    let chunks = chunks.filter(|chunks| chunks.valid(ledger.offset));
    measurements.record("chunks_validation", started, 1);
    let Some(chunks) = chunks else {
        let started = measurements.start();
        let result = clear(transaction, path);
        measurements.record("chunks_clear", started, 1);
        return result;
    };
    let started = measurements.start();
    let prefix = prefix(path);
    measurements.record("chunks_prefix", started, 1);
    let started = measurements.start();
    let mut put = transaction.prepare_cached("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value")?;
    measurements.record("chunks_prepare", started, 1);
    for (index, hashes) in chunks.hashes.chunks(PAGE).enumerate() {
        let started = measurements.start();
        let key = format!("{prefix}{index:010}");
        let encoded = json(&Page {
            version: 1,
            index,
            hashes: hashes.to_vec(),
        })?;
        measurements.record("chunks_page_serialization", started, 1);
        measurements.count("chunks_page_bytes", encoded.len());
        measurements.count("chunks_hashes", hashes.len());
        let started = measurements.start();
        put.execute(params![key, encoded])?;
        measurements.record("chunks_page_sql", started, 1);
    }
    let started = measurements.start();
    measurements.cleanup(
        transaction,
        &prefix,
        || chunks.hashes.len().div_ceil(PAGE),
        &crate::native_timing::CHUNKS_CLEANUP,
    )?;
    measurements.record("chunks_cleanup_sql", started, 1);
    let key = format!("{prefix}header");
    let header = Header {
        version: 1,
        parser_version: CACHE_VERSION,
        ledger: {
            let started = measurements.start();
            let value = header_ledger(ledger);
            measurements.record("chunks_header_ledger", started, 1);
            value
        },
        count: chunks.hashes.len(),
        fingerprint: {
            let started = measurements.start();
            let value = chunks.fingerprint();
            measurements.record("chunks_header_fingerprint", started, 1);
            value
        },
        resume: chunks.resume.clone(),
    };
    let started = measurements.start();
    let encoded = json(&header)?;
    measurements.record("chunks_header_serialization", started, 1);
    measurements.count("chunks_header_bytes", encoded.len());
    let started = measurements.start();
    put.execute(params![key, encoded])?;
    measurements.record("chunks_header_sql", started, 1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleanup_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-chunks-cleanup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn cleanup_store(root: &Path, name: &str) -> SqliteStore {
        match SqliteStore::attempt(&root.join(name), None) {
            Ok((store, _)) => store,
            Err(error) => panic!("real Index schema: {error}"),
        }
    }

    fn cleanup_proof(count: usize, byte: u8) -> (Ledger, FileIndex) {
        let offset = count as u64 * crate::source_chunks::SIZE as u64;
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: offset,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let index = FileIndex {
            source_chunks: Some(Arc::new(Chunks {
                offset,
                hashes: vec![[byte; 32]; count],
                resume: None,
            })),
            ..FileIndex::default()
        };
        (ledger, index)
    }

    fn cleanup_publish(
        store: &mut SqliteStore,
        path: &str,
        ledger: &Ledger,
        index: &FileIndex,
        enabled: bool,
    ) -> rusqlite::Result<crate::native_timing::Measurements> {
        cleanup_publish_with(store, path, ledger, index, enabled, false)
    }

    fn cleanup_publish_with(
        store: &mut SqliteStore,
        path: &str,
        ledger: &Ledger,
        index: &FileIndex,
        enabled: bool,
        uncached_reference: bool,
    ) -> rusqlite::Result<crate::native_timing::Measurements> {
        use crate::native_timing::{Measurements, cleanup_tests as timing};
        let write = |tx: &Transaction<'_>, measurements: &mut Measurements| {
            write_file(
                tx,
                path,
                ledger,
                &super::super::super::DirtyRows::default(),
                index,
                measurements,
            )
        };
        if uncached_reference {
            assert!(!enabled);
            timing::publish_uncached_reference(&mut store.connection, write)
        } else {
            timing::publish(&mut store.connection, enabled, write)
        }
    }

    #[test]
    fn cleanup_diagnostics_preserve_parent_writer_proofs_and_exact_state() {
        use crate::native_timing::{CHUNKS_CLEANUP, cleanup_tests as timing};
        let _recording = timing::recording();
        let root = cleanup_root();
        // Clone one closed real initializer so the random stream epoch and all
        // authority metadata begin byte-identical rather than being normalized.
        drop(cleanup_store(&root, "seed.sqlite3"));
        for name in ["ordinary.sqlite3", "measured.sqlite3", "reference.sqlite3"] {
            fs::copy(root.join("seed.sqlite3"), root.join(name)).unwrap();
        }
        let mut ordinary = cleanup_store(&root, "ordinary.sqlite3");
        let mut measured = cleanup_store(&root, "measured.sqlite3");
        let mut reference = cleanup_store(&root, "reference.sqlite3");
        let path = "/fixture/parent[1]*.jsonl";
        let other = "/fixture/other-prefix.jsonl";
        let (ledger, index) = cleanup_proof(PAGE + 1, 1);
        for (store, uncached_reference) in [
            (&mut ordinary, false),
            (&mut measured, false),
            (&mut reference, true),
        ] {
            for path in [path, other] {
                cleanup_publish_with(store, path, &ledger, &index, false, uncached_reference)
                    .unwrap();
            }
            store.connection.execute_batch(&format!(
                "CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER preserve_header BEFORE DELETE ON meta WHEN OLD.key='{}header' BEGIN SELECT RAISE(ABORT,'chunk-header-exemption'); END;",
                prefix(path)
            )).unwrap();
        }
        assert_eq!(
            timing::snapshot(&ordinary.connection),
            timing::snapshot(&measured.connection)
        );
        for (count, changed) in [(PAGE + 1, 5), (PAGE, 6), (1, 5), (0, 6)] {
            for store in [&ordinary, &measured, &reference] {
                for suffix in [
                    "-1",
                    "00000000x0",
                    "0000000000bad",
                    "0000000002",
                    "header:extra",
                ] {
                    store
                        .connection
                        .execute(
                            "INSERT INTO meta VALUES(?1,'invalid')",
                            [format!("{}{suffix}", prefix(path))],
                        )
                        .unwrap();
                }
            }
            let (current, index) = cleanup_proof(count, 9);
            timing::reset_records();
            let disabled = cleanup_publish(&mut ordinary, path, &current, &index, false).unwrap();
            timing::assert_disabled(&disabled);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            timing::reset_records();
            let old =
                cleanup_publish_with(&mut reference, path, &current, &index, false, true).unwrap();
            timing::assert_uncached_reference(&old);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            let sample = cleanup_publish(&mut measured, path, &current, &index, true).unwrap();
            timing::assert_sample(&sample, &CHUNKS_CLEANUP, changed);
            assert_eq!(timing::status_reads(), 2);
            assert_eq!(timing::records().len(), 1);
            assert_eq!(
                timing::snapshot(&ordinary.connection),
                timing::snapshot(&measured.connection)
            );
            assert_eq!(
                timing::snapshot(&reference.connection),
                timing::snapshot(&measured.connection)
            );
            assert_eq!(
                digest(&reference.connection, path).unwrap(),
                digest(&measured.connection, path).unwrap()
            );
            for store in [&ordinary, &measured, &reference] {
                assert_eq!(
                    read(&store.connection, path, &current).unwrap(),
                    index.source_chunks
                );
                assert_eq!(
                    read(&store.connection, other, &ledger).unwrap(),
                    cleanup_proof(PAGE + 1, 1).1.source_chunks
                );
                assert_eq!(
                    ledger_row(&store.connection, path).unwrap().unwrap().1,
                    current
                );
                assert!(digest(&store.connection, path).unwrap().is_some());
            }
        }
        drop((ordinary, measured, reference));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_cleanup_rebinds_alternating_prefixes_and_reprepares_after_ddl() {
        use crate::native_timing::{CHUNKS_CLEANUP, cleanup_tests as timing};
        let _recording = timing::recording();
        let root = cleanup_root();
        drop(cleanup_store(&root, "seed.sqlite3"));
        for name in ["ordinary.sqlite3", "measured.sqlite3", "reference.sqlite3"] {
            fs::copy(root.join("seed.sqlite3"), root.join(name)).unwrap();
        }
        let mut ordinary = cleanup_store(&root, "ordinary.sqlite3");
        let mut measured = cleanup_store(&root, "measured.sqlite3");
        let mut reference = cleanup_store(&root, "reference.sqlite3");
        let paths = [
            "/fixture/alternating-a[1]*.jsonl",
            "/fixture/alternating-b.jsonl",
        ];
        let unrelated = "/fixture/malformed-other-prefix.jsonl";
        let (seed_ledger, seed_index) = cleanup_proof(PAGE + 1, 1);
        for (store, uncached) in [
            (&mut ordinary, false),
            (&mut measured, false),
            (&mut reference, true),
        ] {
            for path in paths.into_iter().chain([unrelated]) {
                cleanup_publish_with(store, path, &seed_ledger, &seed_index, false, uncached)
                    .unwrap();
            }
            store
                .connection
                .execute(
                    "INSERT INTO meta VALUES(?1,'untouched-malformed')",
                    [format!("{}bad", prefix(unrelated))],
                )
                .unwrap();
            // The real parent has returned the cleanup statement to its cache.
            // Actual main DDL and TEMP triggers now invalidate that warm SQL.
            store.connection.execute_batch(&format!(
                "CREATE INDEX cleanup_meta_value ON meta(value); CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER cleanup_audit AFTER DELETE ON meta BEGIN INSERT INTO audit VALUES(OLD.key); END; CREATE TEMP TRIGGER preserve_headers BEFORE DELETE ON meta WHEN OLD.key IN ('{}header','{}header') BEGIN SELECT RAISE(ABORT,'alternating-header-exemption'); END;",
                prefix(paths[0]), prefix(paths[1])
            )).unwrap();
        }
        let mut expected = [
            (seed_ledger.clone(), seed_index.clone()),
            (seed_ledger.clone(), seed_index.clone()),
        ];
        let mut audit_before = 0;
        let mut previous_digest = [
            digest(&reference.connection, paths[0]).unwrap(),
            digest(&reference.connection, paths[1]).unwrap(),
        ];
        for (step, (selected, count)) in [
            (0, PAGE + 1),
            (1, PAGE * 2 + 1),
            (0, 0),
            (1, 1),
            (0, PAGE + 1),
            (1, 0),
            (0, 1),
        ]
        .into_iter()
        .enumerate()
        {
            let path = paths[selected];
            for store in [&ordinary, &measured, &reference] {
                for suffix in ["bad", "0000000009"] {
                    store
                        .connection
                        .execute(
                            "INSERT INTO meta VALUES(?1,'invalid')",
                            [format!("{}{suffix}", prefix(path))],
                        )
                        .unwrap();
                }
                if step == 3 {
                    // Replace both main and TEMP schema objects after further
                    // reuse; the new trigger must run on the cached statement.
                    store.connection.execute_batch("DROP INDEX cleanup_meta_value; CREATE INDEX cleanup_meta_key_value ON meta(key,value); DROP TRIGGER temp.cleanup_audit; CREATE TEMP TRIGGER cleanup_audit AFTER DELETE ON meta BEGIN INSERT INTO audit VALUES('reprepared:' || OLD.key); END;").unwrap();
                }
            }
            let (current, index) = cleanup_proof(count, (step + 2) as u8);
            timing::reset_records();
            let old =
                cleanup_publish_with(&mut reference, path, &current, &index, false, true).unwrap();
            timing::assert_uncached_reference(&old);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            let disabled = cleanup_publish(&mut ordinary, path, &current, &index, false).unwrap();
            timing::assert_disabled(&disabled);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            let sample = cleanup_publish(&mut measured, path, &current, &index, true).unwrap();
            assert_eq!(timing::status_reads(), 2);
            assert_eq!(timing::records().len(), 1);
            // Every operation deletes the two injected malformed/obsolete keys;
            // page shrink can add deletes witnessed by the real trigger.
            let audit_rows: usize = measured
                .connection
                .query_row("SELECT count(*) FROM temp.audit", [], |row| row.get(0))
                .unwrap();
            assert!(audit_rows >= (step + 1) * 2);
            if step >= 3 {
                let recompiled_rows: usize = measured
                    .connection
                    .query_row(
                        "SELECT count(*) FROM temp.audit WHERE value LIKE 'reprepared:%'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert!(recompiled_rows >= (step - 2) * 2);
            }
            timing::assert_sample(&sample, &CHUNKS_CLEANUP, audit_rows - audit_before);
            audit_before = audit_rows;
            for store in [&ordinary, &measured] {
                assert_eq!(
                    timing::snapshot(&store.connection),
                    timing::snapshot(&reference.connection)
                );
            }
            let old_ledger = expected[selected].0.clone();
            expected[selected] = (current.clone(), index);
            let current_digest = digest(&reference.connection, path).unwrap();
            assert!(current_digest.is_some());
            assert_ne!(current_digest, previous_digest[selected]);
            previous_digest[selected] = current_digest;
            for store in [&ordinary, &measured, &reference] {
                for (path, (ledger, index)) in paths.into_iter().zip(&expected) {
                    assert_eq!(
                        read(&store.connection, path, ledger).unwrap(),
                        index.source_chunks
                    );
                    assert_eq!(
                        ledger_row(&store.connection, path).unwrap().unwrap().1,
                        *ledger
                    );
                    assert_eq!(
                        digest(&store.connection, path).unwrap(),
                        digest(&reference.connection, path).unwrap()
                    );
                }
                if old_ledger != current {
                    assert!(
                        read(&store.connection, path, &old_ledger)
                            .unwrap()
                            .is_none()
                    );
                }
                assert!(
                    read(&store.connection, unrelated, &seed_ledger)
                        .unwrap()
                        .is_none()
                );
                let untouched: String = store
                    .connection
                    .query_row(
                        "SELECT value FROM meta WHERE key=?1",
                        [format!("{}bad", prefix(unrelated))],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(untouched, "untouched-malformed");
                assert_eq!(
                    ledger_row(&store.connection, unrelated).unwrap().unwrap().1,
                    seed_ledger
                );
            }
        }
        drop((ordinary, measured, reference));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_diagnostic_errors_roll_back_parent_pages_and_never_emit() {
        use crate::native_timing::cleanup_tests as timing;
        let _recording = timing::recording();
        let root = cleanup_root();
        let path = "/fixture/chunk-rollback.jsonl";
        let p = prefix(path);
        let (old, old_index) = cleanup_proof(PAGE + 1, 1);
        let (new, new_index) = cleanup_proof(1, 9);
        for (label, event, key) in [
            (
                "chunk-cleanup-after-page",
                "DELETE",
                format!("{p}0000000001"),
            ),
            ("chunk-header-after-page", "INSERT", format!("{p}header")),
        ] {
            let mut expected_error = None;
            for (enabled, uncached_reference) in [(false, false), (true, false), (false, true)] {
                let mut store = cleanup_store(
                    &root,
                    &format!("{label}-{enabled}-{uncached_reference}.sqlite3"),
                );
                cleanup_publish_with(
                    &mut store,
                    path,
                    &old,
                    &old_index,
                    false,
                    uncached_reference,
                )
                .unwrap();
                store.connection.execute_batch(&format!(
                    "CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER prior_page AFTER UPDATE ON meta WHEN NEW.key='{p}0000000000' BEGIN INSERT INTO audit VALUES('page-written'); END; CREATE TEMP TRIGGER cleanup_failure BEFORE {event} ON meta WHEN {}.key='{key}' AND EXISTS(SELECT 1 FROM audit) BEGIN SELECT RAISE(ABORT,'{label}'); END;",
                    if event == "DELETE" { "OLD" } else { "NEW" }
                )).unwrap();
                // Reuse after ABORT must reset the cached statement without
                // retaining partial pages, TEMP side effects or parameters.
                for _ in 0..2 {
                    let before = timing::snapshot(&store.connection);
                    timing::reset_records();
                    let error = match cleanup_publish_with(
                        &mut store,
                        path,
                        &new,
                        &new_index,
                        enabled,
                        uncached_reference,
                    ) {
                        Err(error) => error,
                        Ok(_) => panic!("named failure was not reached"),
                    };
                    assert!(error.to_string().contains(label), "{error}");
                    let actual_error = error.to_string();
                    if let Some(expected) = &expected_error {
                        assert_eq!(&actual_error, expected);
                    } else {
                        expected_error = Some(actual_error);
                    }
                    assert_eq!(timing::snapshot(&store.connection), before);
                    assert!(timing::records().is_empty());
                    assert_eq!(timing::status_reads(), if enabled { 2 } else { 0 });
                    assert_eq!(
                        read(&store.connection, path, &old).unwrap(),
                        old_index.source_chunks
                    );
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paged_proofs_round_trip_and_refuse_corruption_or_a_different_ledger() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            .unwrap();
        let path = "/fixture/parent[1]*.jsonl";
        let chunks = Chunks {
            offset: crate::source_chunks::SIZE as u64 * 1024 + 19,
            hashes: vec![[42; 32]; 1025],
            resume: None,
        };
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: chunks.offset,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset: chunks.offset,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let transaction = connection.transaction().unwrap();
        write(&transaction, path, &ledger, Some(&chunks)).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            read(&connection, path, &ledger).unwrap().unwrap().as_ref(),
            &chunks
        );
        let max: i64 = connection
            .query_row(
                "SELECT max(length(CAST(value AS BLOB))) FROM meta",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(max < NATIVE_METADATA_MAX as i64);
        let mut changed = ledger.clone();
        changed.stat.modified_ns += 1;
        assert!(read(&connection, path, &changed).unwrap().is_none());
        let key = format!("{}0000000000", prefix(path));
        let encoded: String = connection
            .query_row("SELECT value FROM meta WHERE key=?1", [&key], |row| {
                row.get(0)
            })
            .unwrap();
        let mut page: Page = serde_json::from_str(&encoded).unwrap();
        page.hashes[0][0] ^= 1;
        connection
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                params![&key, json(&page).unwrap()],
            )
            .unwrap();
        assert!(read(&connection, path, &ledger).unwrap().is_none());
        let transaction = connection.transaction().unwrap();
        write(&transaction, path, &ledger, Some(&chunks)).unwrap();
        transaction.commit().unwrap();
        assert!(read(&connection, path, &ledger).unwrap().is_some());
        connection
            .execute(
                "DELETE FROM meta WHERE key=?1",
                [format!("{}0000000001", prefix(path))],
            )
            .unwrap();
        assert!(read(&connection, path, &ledger).unwrap().is_none());
    }

    #[test]
    fn shrinking_or_removing_a_proof_cleans_only_its_exact_namespace() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            .unwrap();
        connection
            .execute("INSERT INTO meta VALUES ('reported_runs','retained')", [])
            .unwrap();
        let chunks = Chunks {
            offset: crate::source_chunks::SIZE as u64 * 1025,
            hashes: vec![[1; 32]; 1025],
            resume: None,
        };
        let mut ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: chunks.offset,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset: chunks.offset,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let transaction = connection.transaction().unwrap();
        for path in ["one[?]", "two*"] {
            write(&transaction, path, &ledger, Some(&chunks)).unwrap();
        }
        transaction.commit().unwrap();
        for suffix in [
            "-1",
            "00000000x0",
            "0000000000bad",
            "0000000002",
            "header:extra",
        ] {
            connection
                .execute(
                    "INSERT INTO meta VALUES (?1,'invalid')",
                    [format!("{}{suffix}", prefix("one[?]"))],
                )
                .unwrap();
        }
        assert!(read(&connection, "one[?]", &ledger).unwrap().is_none());
        let short = Chunks {
            offset: 19,
            hashes: vec![[2; 32]],
            resume: None,
        };
        ledger.offset = short.offset;
        ledger.stat.size = short.offset;
        let transaction = connection.transaction().unwrap();
        write(&transaction, "one[?]", &ledger, Some(&short)).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            read(&connection, "one[?]", &ledger)
                .unwrap()
                .unwrap()
                .as_ref(),
            &short
        );
        let transaction = connection.transaction().unwrap();
        clear(&transaction, "one[?]").unwrap();
        transaction.commit().unwrap();
        assert!(read(&connection, "one[?]", &ledger).unwrap().is_none());
        let count: i64 = connection
            .query_row("SELECT count(*) FROM meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 4,
            "the other proof's two pages/header and observed runs survive"
        );
    }
}
