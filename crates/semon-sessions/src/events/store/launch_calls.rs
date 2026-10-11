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
    let Some(index) = index.filter(|index| index.valid(ledger)) else {
        return clear(transaction, path);
    };
    let prefix = prefix(path);
    let mut put = transaction.prepare("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value")?;
    let calls: Vec<_> = index.complete.iter().chain(&index.eof).cloned().collect();
    for (page, calls) in calls.chunks(PAGE).enumerate() {
        let encoded = json(&Page {
            version: 1,
            index: page,
            calls: calls.to_vec(),
        })?;
        if encoded.len() > NATIVE_METADATA_MAX {
            return clear(transaction, path);
        }
        put.execute(params![format!("{prefix}{page:010}"), encoded])?;
    }
    let pages = calls.len().div_ceil(PAGE);
    transaction.execute("DELETE FROM meta WHERE key>=?1 AND key<?2 AND key!=?3 AND (key>=?4 OR length(key)!=?5 OR substr(key,?6) GLOB '*[^0-9]*')", params![&prefix, format!("{prefix}~"), format!("{prefix}header"), format!("{prefix}{pages:010}"), (prefix.len()+10) as i64, (prefix.len()+1) as i64])?;
    let encoded = json(&Header {
        version: 1,
        parser_version: CACHE_VERSION,
        ledger: header_ledger(ledger),
        complete: index.complete.len(),
        count: calls.len(),
        eof_record: index.eof_record.clone(),
        complete_bytes: index.complete_bytes,
        eof_bytes: index.eof_bytes,
        fingerprint: fingerprint(index)?,
    })?;
    put.execute(params![format!("{prefix}header"), encoded])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
