//! Bounded paged native launch locations in the existing index metadata region.
use super::*;
use crate::native_handoff::{CallRef, Index};
use crate::native_record::Record;
use std::collections::VecDeque;

const PAGE: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    parser_version: u32,
    ledger: String,
    complete: usize,
    count: usize,
    eof_record: Option<Record>,
    complete_bytes: usize,
    eof_bytes: usize,
    fingerprint: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    version: u32,
    index: usize,
    calls: Vec<CallRef>,
}

fn prefix(path: &str) -> String {
    format!("native_launch_calls:{:x}:", Sha256::digest(path.as_bytes()))
}
fn fingerprint(index: &Index) -> rusqlite::Result<[u8; 32]> {
    Ok(Sha256::digest(json(index)?.as_bytes()).into())
}

pub(super) fn digest(connection: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let text: Option<Option<String>> = connection
        .query_row(
            NATIVE_METADATA_VALUE,
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
        "DELETE FROM meta WHERE key>=?1 AND key<?2",
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
        .query_row(NATIVE_METADATA_VALUE, [format!("{prefix}header")], |row| {
            row.get(0)
        })
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
    let mut statement = connection.prepare("SELECT key, CASE WHEN length(CAST(value AS BLOB))<=?3 THEN value ELSE NULL END FROM meta WHERE key>=?1 AND key<?2 ORDER BY key")?;
    let mut rows = statement.query(params![
        &prefix,
        format!("{prefix}~"),
        NATIVE_METADATA_MAX as i64
    ])?;
    let mut calls = VecDeque::new();
    let mut page = 0;
    while let Some(row) = rows.next()? {
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
    let eof = calls.split_off(header.complete);
    let index = Index {
        complete: calls,
        eof,
        eof_record: header.eof_record,
        complete_bytes: header.complete_bytes,
        eof_bytes: header.eof_bytes,
    };
    if !index.valid(ledger) || fingerprint(&index)? != header.fingerprint {
        return Ok(None);
    }
    Ok(Some(index))
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
    let mut put = transaction.prepare_cached("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value")?;
    measurements.record("launch_prepare", started, 1);
    let started = measurements.start();
    let calls: Vec<_> = index.complete.iter().chain(&index.eof).cloned().collect();
    measurements.record("launch_call_clone", started, 1);
    measurements.count("launch_calls", calls.len());
    for (page, calls) in calls.chunks(PAGE).enumerate() {
        let started = measurements.start();
        let encoded = json(&Page {
            version: 1,
            index: page,
            calls: calls.to_vec(),
        })?;
        measurements.record("launch_page_serialization", started, 1);
        measurements.count("launch_page_bytes", encoded.len());
        if encoded.len() > NATIVE_METADATA_MAX {
            let started = measurements.start();
            let result = clear(transaction, path);
            measurements.record("launch_clear", started, 1);
            return result;
        }
        let started = measurements.start();
        put.execute(params![format!("{prefix}{page:010}"), encoded])?;
        measurements.record("launch_page_sql", started, 1);
    }
    let started = measurements.start();
    if measurements.is_enabled() {
        measurements.cleanup(
            transaction,
            &prefix,
            || calls.len().div_ceil(PAGE),
            &crate::native_timing::LAUNCH_CLEANUP,
        )?;
    } else {
        let pages = calls.len().div_ceil(PAGE);
        transaction.execute("DELETE FROM meta WHERE key>=?1 AND key<?2 AND key!=?3 AND (key>=?4 OR length(key)!=?5 OR substr(key,?6) GLOB '*[^0-9]*')", params![&prefix, format!("{prefix}~"), format!("{prefix}header"), format!("{prefix}{pages:010}"), (prefix.len()+10) as i64, (prefix.len()+1) as i64])?;
    }
    measurements.record("launch_cleanup_sql", started, 1);
    let header = Header {
        version: 1,
        parser_version: CACHE_VERSION,
        ledger: {
            let started = measurements.start();
            let value = header_ledger(ledger);
            measurements.record("launch_header_ledger", started, 1);
            value
        },
        complete: index.complete.len(),
        count: calls.len(),
        eof_record: index.eof_record.clone(),
        complete_bytes: index.complete_bytes,
        eof_bytes: index.eof_bytes,
        fingerprint: {
            let started = measurements.start();
            let value = fingerprint(index)?;
            measurements.record("launch_header_fingerprint", started, 1);
            value
        },
    };
    let started = measurements.start();
    let encoded = json(&header)?;
    measurements.record("launch_header_serialization", started, 1);
    measurements.count("launch_header_bytes", encoded.len());
    let started = measurements.start();
    put.execute(params![format!("{prefix}header"), encoded])?;
    measurements.record("launch_header_sql", started, 1);
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
        match SqliteStore::attempt(&root.join(name), None) {
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
        crate::native_timing::cleanup_tests::publish(
            &mut store.connection,
            enabled,
            |tx, measurements| {
                write_file(
                    tx,
                    path,
                    ledger,
                    &super::super::super::DirtyRows::default(),
                    index,
                    measurements,
                )
            },
        )
    }

    #[test]
    fn cleanup_diagnostics_preserve_parent_writer_launches_and_exact_state() {
        use crate::native_timing::{LAUNCH_CLEANUP, cleanup_tests as timing};
        let _recording = timing::recording();
        let root = cleanup_root();
        // Preserve the initializer's random stream epoch and authority metadata
        // by cloning its closed database, not by filtering comparison state.
        drop(cleanup_store(&root, "seed.sqlite3"));
        for name in ["ordinary.sqlite3", "measured.sqlite3"] {
            fs::copy(root.join("seed.sqlite3"), root.join(name)).unwrap();
        }
        let mut ordinary = cleanup_store(&root, "ordinary.sqlite3");
        let mut measured = cleanup_store(&root, "measured.sqlite3");
        let path = "/fixture/native[1]*.jsonl";
        let other = "/fixture/other-prefix.jsonl";
        let (ledger, index) = cleanup_proof(PAGE + 1, 1);
        for store in [&mut ordinary, &mut measured] {
            for path in [path, other] {
                cleanup_publish(store, path, &ledger, &index, false).unwrap();
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
            for store in [&ordinary, &measured] {
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
            let sample = cleanup_publish(&mut measured, path, &current, &index, true).unwrap();
            timing::assert_sample(&sample, &LAUNCH_CLEANUP, changed);
            assert_eq!(timing::status_reads(), 2);
            assert_eq!(timing::records().len(), 1);
            assert_eq!(
                timing::snapshot(&ordinary.connection),
                timing::snapshot(&measured.connection)
            );
            for store in [&ordinary, &measured] {
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
        drop((ordinary, measured));
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
            for enabled in [false, true] {
                let mut store = cleanup_store(&root, &format!("{label}-{enabled}.sqlite3"));
                cleanup_publish(&mut store, path, &old, &old_index, false).unwrap();
                store.connection.execute_batch(&format!(
                    "CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER prior_page AFTER UPDATE ON meta WHEN NEW.key='{p}0000000000' BEGIN INSERT INTO audit VALUES('page-written'); END; CREATE TEMP TRIGGER cleanup_failure BEFORE {event} ON meta WHEN {}.key='{key}' AND EXISTS(SELECT 1 FROM audit) BEGIN SELECT RAISE(ABORT,'{label}'); END;",
                    if event == "DELETE" { "OLD" } else { "NEW" }
                )).unwrap();
                let before = timing::snapshot(&store.connection);
                timing::reset_records();
                let error = match cleanup_publish(&mut store, path, &new, &new_index, enabled) {
                    Err(error) => error,
                    Ok(_) => panic!("named failure was not reached"),
                };
                assert!(error.to_string().contains(label), "{error}");
                assert_eq!(timing::snapshot(&store.connection), before);
                assert!(timing::records().is_empty());
                assert_eq!(timing::status_reads(), if enabled { 2 } else { 0 });
                assert_eq!(
                    read(&store.connection, path, &old).unwrap(),
                    old_index.handoff_index
                );
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
