//! Bounded paged native launch locations in the existing index metadata region.
use super::*;
use crate::native_handoff::{CallRef, Index};
use std::collections::VecDeque;

const PAGE: usize = 256;

use crate::events::native_pages::launch::{Header, Page};

fn prefix(path: &str) -> String {
    format!("native_launch_calls:{:x}:", Sha256::digest(path.as_bytes()))
}

pub(super) fn digest(connection: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let text: Option<Option<String>> = connection
        .query_row(
            native_metadata_value(connection)?,
            [format!("{}header", prefix(path))],
            |row| row.get(0),
        )
        .optional()?;
    Ok(text
        .flatten()
        .map(|text| format!("{:x}", Sha256::digest(text.as_bytes()))))
}

pub(super) fn clear(transaction: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let prefix = prefix(path);
    transaction.execute(
        configured::metadata_sql(
            transaction,
            "DELETE FROM meta WHERE key>=?1 AND key<?2",
            "DELETE FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2",
        )?,
        params![&prefix, format!("{prefix}~")],
    )?;
    Ok(())
}

pub(super) fn read(
    connection: &Connection,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<Index>> {
    let prefix = prefix(path);
    let text: Option<Option<String>> = connection
        .query_row(
            native_metadata_value(connection)?,
            [format!("{prefix}header")],
            |row| {
                crate::selected_read::row(row, 512)?;
                row.get(0)
            },
        )
        .optional()?;
    let Some(header) = text
        .flatten()
        .and_then(|text| serde_json::from_str::<Header>(&text).ok())
    else {
        return Ok(None);
    };
    if header.version != 1
        || header.parser_version != CACHE_VERSION
        || header.ledger != header_ledger(ledger)
        || header.complete > crate::handoff::MAX_CALLS
        || header.count < header.complete
        || header.count - header.complete > crate::handoff::MAX_CALLS
    {
        return Ok(None);
    }
    let mut statement = connection.prepare(configured::metadata_sql(connection, "SELECT key, CASE WHEN length(CAST(value AS BLOB))<=?3 THEN value ELSE NULL END FROM meta WHERE key>=?1 AND key<?2 ORDER BY key", "SELECT key, CASE WHEN length(CAST(value AS BLOB))<=?3 THEN value ELSE NULL END FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2 ORDER BY key")?)?;
    let mut rows = statement.query(params![
        &prefix,
        format!("{prefix}~"),
        NATIVE_METADATA_MAX as i64
    ])?;
    let mut calls = VecDeque::new();
    let mut page = 0;
    while let Some(row) = rows.next()? {
        crate::selected_read::row(row, PAGE * std::mem::size_of::<CallRef>())?;
        let key: String = row.get(0)?;
        if key == format!("{prefix}header") {
            continue;
        }
        if key != format!("{prefix}{page:010}") {
            return Ok(None);
        }
        let Some(part) = row
            .get::<_, Option<String>>(1)?
            .and_then(|text| serde_json::from_str::<Page>(&text).ok())
        else {
            return Ok(None);
        };
        let remaining = header.count.saturating_sub(calls.len());
        if part.version != 1
            || part.index != page
            || part.calls.len() != remaining.min(PAGE)
            || remaining == 0
        {
            return Ok(None);
        }
        calls.extend(part.calls);
        page += 1;
    }
    if calls.len() != header.count {
        return Ok(None);
    }
    crate::events::native_pages::launch::restore(header, calls, ledger)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

pub(super) fn write(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: Option<&Index>,
) -> rusqlite::Result<()> {
    write_measured(
        transaction,
        path,
        ledger,
        index,
        &mut crate::native_timing::Measurements::default(),
    )
}

pub(super) fn write_measured(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: Option<&Index>,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let started = measurements.start();
    let index = index.filter(|index| index.valid(ledger));
    measurements.record("launch_validation", started, 1);
    let Some(index) = index else {
        let started = measurements.start();
        let result = clear(transaction, path);
        measurements.record("launch_clear", started, 1);
        return result;
    };
    let started = measurements.start();
    let prefix = prefix(path);
    measurements.record("launch_prefix", started, 1);
    let started = measurements.start();
    let mut put = transaction.prepare_cached(configured::metadata_sql(transaction, "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value", "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value")?)?;
    measurements.record("launch_prepare", started, 1);
    let started = measurements.start();
    let Some(plan) = crate::events::native_pages::launch::plan(ledger, index)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
    else {
        return clear(transaction, path);
    };
    measurements.record("launch_page_serialization", started, plan.pages.len());
    measurements.count("launch_calls", index.complete.len() + index.eof.len());
    for (page, encoded) in plan.pages.iter().enumerate() {
        measurements.count("launch_page_bytes", encoded.len());
        put.execute(params![format!("{prefix}{page:010}"), encoded])?;
    }
    measurements.cleanup(
        transaction,
        &prefix,
        || plan.pages.len(),
        &crate::native_timing::LAUNCH_CLEANUP,
    )?;
    measurements.count("launch_header_bytes", plan.header.len());
    put.execute(params![format!("{prefix}header"), plan.header])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cleanup_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-launch-cleanup-{}-{}",
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
        match SqliteStore::attempt(&root.join(name)) {
            Ok((store, _)) => store,
            Err(error) => panic!("real Index schema: {error}"),
        }
    }

    fn cleanup_proof(count: usize, version: u8) -> (Ledger, FileIndex) {
        let mut index = Index::default();
        let mut offset = 0;
        for id in 0..count {
            let value = json!({"message":{"content":[{"type":"tool_use","id":format!("fixture-{id}"),"name":"Bash","input":{"command":format!("codex exec fixture-{version}")}}]}});
            let bytes = format!("{value}\n");
            index
                .observe(bytes.as_bytes(), offset, true, &value)
                .unwrap();
            offset += bytes.len() as u64;
        }
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
        assert_eq!(index.complete.len(), count);
        assert!(index.valid(&ledger));
        (
            ledger,
            FileIndex {
                handoff_index: Some(index),
                ..FileIndex::default()
            },
        )
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
    fn cleanup_diagnostics_preserve_parent_writer_launches_and_exact_state() {
        use crate::native_timing::{LAUNCH_CLEANUP, cleanup_tests as timing};
        let _recording = timing::recording();
        let root = cleanup_root();
        // Preserve the initializer's random stream epoch and authority metadata
        // by cloning its closed database, not by filtering comparison state.
        drop(cleanup_store(&root, "seed.sqlite3"));
        for name in ["ordinary.sqlite3", "measured.sqlite3", "reference.sqlite3"] {
            fs::copy(root.join("seed.sqlite3"), root.join(name)).unwrap();
        }
        let mut ordinary = cleanup_store(&root, "ordinary.sqlite3");
        let mut measured = cleanup_store(&root, "measured.sqlite3");
        let mut reference = cleanup_store(&root, "reference.sqlite3");
        let path = "/fixture/native[1]*.jsonl";
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
                "CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER preserve_header BEFORE DELETE ON meta WHEN OLD.key='{}header' BEGIN SELECT RAISE(ABORT,'launch-header-exemption'); END;",
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
            timing::assert_sample(&sample, &LAUNCH_CLEANUP, changed);
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
                    index.handoff_index
                );
                assert_eq!(
                    read(&store.connection, other, &ledger).unwrap(),
                    cleanup_proof(PAGE + 1, 1).1.handoff_index
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
        use crate::native_timing::{LAUNCH_CLEANUP, cleanup_tests as timing};
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
            let audit_rows: i64 = measured
                .connection
                .query_row("SELECT count(*) FROM temp.audit", [], |row| row.get(0))
                .unwrap();
            let audit_rows = usize::try_from(audit_rows).unwrap();
            assert!(audit_rows >= (step + 1) * 2);
            if step >= 3 {
                let recompiled_rows: i64 = measured
                    .connection
                    .query_row(
                        "SELECT count(*) FROM temp.audit WHERE value LIKE 'reprepared:%'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let recompiled_rows = usize::try_from(recompiled_rows).unwrap();
                assert!(recompiled_rows >= (step - 2) * 2);
            }
            timing::assert_sample(&sample, &LAUNCH_CLEANUP, audit_rows - audit_before);
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
                        index.handoff_index
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
        let path = "/fixture/launch-rollback.jsonl";
        let p = prefix(path);
        let (old, old_index) = cleanup_proof(PAGE + 1, 1);
        let (new, new_index) = cleanup_proof(1, 9);
        for (label, event, key) in [
            (
                "launch-cleanup-after-page",
                "DELETE",
                format!("{p}0000000001"),
            ),
            ("launch-header-after-page", "INSERT", format!("{p}header")),
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
                        old_index.handoff_index
                    );
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_launch_pages_are_bounded_generation_bound_and_repairable() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            .unwrap();
        let path = "/native/[private]*é.jsonl";
        let mut index = Index::default();
        let mut offset = 0;
        for id in 0..600 {
            let value = json!({"message":{"content":[{"type":"tool_use","id":format!("SECRET-{id}"),"name":"Bash","input":{"command":"codex exec < /private/SECRET"}}]}});
            let bytes = format!("{value}\n");
            index
                .observe(bytes.as_bytes(), offset, true, &value)
                .unwrap();
            offset += bytes.len() as u64;
        }
        let ledger = Ledger {
            stat: Stat {
                dev: 0,
                ino: 0,
                size: offset,
                modified_ns: 0,
                changed_ns: Some(0),
            },
            offset,
            prefix: [0; 32],
            tail: [0; 32],
        };
        let save = |connection: &mut Connection| {
            let transaction = connection.transaction().unwrap();
            write(&transaction, path, &ledger, Some(&index)).unwrap();
            transaction.commit().unwrap();
        };
        save(&mut connection);
        assert_eq!(
            read(&connection, path, &ledger).unwrap(),
            Some(index.clone())
        );
        assert_eq!(
            connection
                .query_row::<i64, _, _>("SELECT count(*) FROM meta", [], |row| row.get(0))
                .unwrap(),
            4
        );
        let values: Vec<String> = connection
            .prepare("SELECT value FROM meta")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(values.iter().all(|value| value.len() <= NATIVE_METADATA_MAX
            && !value.contains("SECRET")
            && !value.contains("private")));
        let page = format!("{}{:010}", prefix(path), 1);
        for mutation in 0..5 {
            match mutation {
                0 => {
                    connection
                        .execute("DELETE FROM meta WHERE key=?1", [&page])
                        .unwrap();
                }
                1 => {
                    connection
                        .execute("UPDATE meta SET value='{' WHERE key=?1", [&page])
                        .unwrap();
                }
                2 => {
                    connection
                        .execute(
                            "UPDATE meta SET value=json_set(value,'$.version',99) WHERE key=?1",
                            [&page],
                        )
                        .unwrap();
                }
                3 => {
                    connection.execute("UPDATE meta SET value=json_set(value,'$.calls[0].time',1) WHERE key=?1", [&page]).unwrap();
                }
                _ => {
                    connection
                        .execute(
                            "INSERT INTO meta(key,value) VALUES(?1,'{}')",
                            [format!("{}junk", prefix(path))],
                        )
                        .unwrap();
                }
            }
            assert!(read(&connection, path, &ledger).unwrap().is_none());
            save(&mut connection);
            assert_eq!(
                read(&connection, path, &ledger).unwrap(),
                Some(index.clone())
            );
        }
        let mut other = ledger.clone();
        other.offset -= 1;
        assert!(read(&connection, path, &other).unwrap().is_none());
        let transaction = connection.transaction().unwrap();
        write(&transaction, path, &ledger, Some(&Index::default())).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            read(&connection, path, &ledger).unwrap(),
            Some(Index::default())
        );
        assert_eq!(
            connection
                .query_row::<i64, _, _>("SELECT count(*) FROM meta", [], |row| row.get(0))
                .unwrap(),
            1
        );
    }
}
