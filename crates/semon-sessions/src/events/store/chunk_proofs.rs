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
    let pages = chunks.hashes.len().div_ceil(PAGE);
    transaction.execute(
        "DELETE FROM meta WHERE key>=?1 AND key<?2 AND key!=?3 AND (key>=?4 OR length(key)!=?5 OR substr(key,?6) GLOB '*[^0-9]*')",
        params![&prefix, format!("{prefix}~"), format!("{prefix}header"), format!("{prefix}{pages:010}"), (prefix.len()+10) as i64, (prefix.len()+1) as i64],
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
