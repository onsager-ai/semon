//! Rebuildable metadata-only substring postings, ordered like the catalog.
//! Candidate and byte budgets bound verification; incomplete pages continue from
//! the last checked posting rather than silently scanning the complete catalog.
use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::model::summary::CatalogRow;

#[cfg(test)]
thread_local! { pub(crate) static SQL_STEPS:std::cell::Cell<i32>=const {std::cell::Cell::new(0)}; }
fn record_steps(statement: &rusqlite::Statement<'_>) {
    #[cfg(test)]
    SQL_STEPS.with(|steps| {
        steps.set(steps.get() + statement.reset_status(rusqlite::StatementStatus::VmStep))
    });
    #[cfg(not(test))]
    let _ = statement;
}

pub(crate) const VERSION: &str = "1";
pub(crate) const MAX_CANDIDATES: usize = 512;
pub(crate) const MAX_SCAN_BYTES: usize = 2 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: usize = 8192;
const MAX_GRAMS: usize = 1024;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_search_coverage (
    read_scope TEXT PRIMARY KEY CHECK(read_scope IN ('current','retained_history')),
    incomplete INTEGER NOT NULL CHECK(incomplete >= 0)
) STRICT, WITHOUT ROWID;
INSERT OR IGNORE INTO session_search_coverage VALUES('current',0),('retained_history',0);
CREATE TABLE IF NOT EXISTS session_search_documents (
    read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history')),
    session_key TEXT NOT NULL,
    fields TEXT NOT NULL,
    complete INTEGER NOT NULL CHECK(complete IN (0,1)),
    PRIMARY KEY(read_scope,session_key)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS session_search_postings (
    read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history')),
    session_key TEXT NOT NULL,
    gram TEXT NOT NULL,
    last_ms INTEGER NOT NULL,
    PRIMARY KEY(read_scope,session_key,gram)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_search_order ON session_search_postings(read_scope,gram,last_ms DESC,session_key ASC);
CREATE TABLE IF NOT EXISTS session_search_counts (
    read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history')),
    gram TEXT NOT NULL,
    members INTEGER NOT NULL CHECK(members >= 0),
    PRIMARY KEY(read_scope,gram)
) STRICT, WITHOUT ROWID;
";

pub(crate) fn scope_key(scope: crate::CatalogReadScope) -> &'static str {
    match scope {
        crate::CatalogReadScope::Current => "current",
        crate::CatalogReadScope::RetainedHistory => "retained_history",
    }
}

pub(crate) fn normalize_query(value: &str) -> Option<String> {
    if value.len() > 1024 || value.chars().count() > 256 || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.trim().to_lowercase())
}

fn grams(fields: &[String]) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for field in fields {
        let chars: Vec<_> = field.chars().collect();
        for width in 1..=3 {
            for window in chars.windows(width) {
                result.insert(window.iter().collect());
            }
        }
    }
    result
}

fn fields(row: &CatalogRow) -> Vec<String> {
    [
        Some(row.name.as_str()),
        Some(row.key.as_str()),
        row.repo.as_deref(),
        row.branch.as_deref(),
        Some(row.model.as_str()),
        Some(row.harness.as_str()),
    ]
    .into_iter()
    .flatten()
    .map(str::to_lowercase)
    .collect()
}

pub(crate) fn row_present(transaction: &Transaction<'_>, key: &str) -> rusqlite::Result<bool> {
    transaction.query_row("SELECT EXISTS(SELECT 1 FROM session_search_documents WHERE read_scope='current' AND session_key=?1) AND EXISTS(SELECT 1 FROM session_search_documents WHERE read_scope='retained_history' AND session_key=?1)", [key], |row|row.get(0))
}

/// Called inside the existing source/generation-CAS publication transaction.
/// Native body strings, paths, tool arguments and runtime state are excluded.
pub(crate) fn publish_row(
    transaction: &Transaction<'_>,
    scope: crate::CatalogReadScope,
    row: &CatalogRow,
) -> rusqlite::Result<()> {
    let scope = scope_key(scope);
    remove_row(transaction, scope, &row.key)?;
    let fields = fields(row);
    let complete = fields.iter().map(String::len).sum::<usize>() <= MAX_DOCUMENT_BYTES;
    let grams = if complete {
        grams(&fields)
    } else {
        BTreeSet::new()
    };
    let complete = complete && grams.len() <= MAX_GRAMS;
    let encoded = serde_json::to_string(if complete { fields.as_slice() } else { &[] })
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    transaction.execute("INSERT INTO session_search_documents(read_scope,session_key,fields,complete) VALUES(?1,?2,?3,?4)",params![scope,row.key,encoded,complete])?;
    if !complete {
        transaction.execute(
            "UPDATE session_search_coverage SET incomplete=incomplete+1 WHERE read_scope=?1",
            [scope],
        )?;
    }
    if complete {
        let mut posting = transaction.prepare("INSERT INTO session_search_postings(read_scope,session_key,gram,last_ms) VALUES(?1,?2,?3,?4)")?;
        let mut count = transaction.prepare("INSERT INTO session_search_counts(read_scope,gram,members) VALUES(?1,?2,1) ON CONFLICT(read_scope,gram) DO UPDATE SET members=members+1")?;
        for gram in grams {
            posting.execute(params![scope, row.key, gram, row.last.unwrap_or(0)])?;
            count.execute(params![scope, gram])?;
        }
    }
    Ok(())
}

fn remove_row(transaction: &Transaction<'_>, scope: &str, key: &str) -> rusqlite::Result<()> {
    transaction.execute("UPDATE session_search_coverage SET incomplete=incomplete-1 WHERE read_scope=?1 AND EXISTS(SELECT 1 FROM session_search_documents WHERE read_scope=?1 AND session_key=?2 AND complete=0)",params![scope,key])?;
    transaction.execute("UPDATE session_search_counts SET members=members-1 WHERE read_scope=?1 AND gram IN (SELECT gram FROM session_search_postings WHERE read_scope=?1 AND session_key=?2)",params![scope,key])?;
    transaction.execute("DELETE FROM session_search_counts WHERE read_scope=?1 AND members=0 AND gram IN (SELECT gram FROM session_search_postings WHERE read_scope=?1 AND session_key=?2)",params![scope,key])?;
    transaction.execute(
        "DELETE FROM session_search_postings WHERE read_scope=?1 AND session_key=?2",
        params![scope, key],
    )?;
    transaction.execute(
        "DELETE FROM session_search_documents WHERE read_scope=?1 AND session_key=?2",
        params![scope, key],
    )?;
    Ok(())
}

pub(crate) fn retire_key(transaction: &Transaction<'_>, key: &str) -> rusqlite::Result<()> {
    let current: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1 AND lifecycle='current')",
        [key],
        |row| row.get(0),
    )?;
    if !current {
        remove_row(transaction, "current", key)?;
    }
    Ok(())
}

/// Current membership retirement does not erase retained-history search rows.
pub(crate) fn retire_missing(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    let mut statement = transaction.prepare("SELECT session_key FROM session_search_documents WHERE read_scope='current' AND session_key NOT IN (SELECT session_key FROM catalog_members) LIMIT 512")?;
    loop {
        let retired = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if retired.is_empty() {
            break;
        }
        for key in retired {
            remove_row(transaction, "current", &key)?;
        }
    }
    Ok(())
}

/// Pick a selective gram using persisted exact member counts. This performs at
/// most query-length primary-key reads; it never counts/scans postings on reads.
pub(crate) fn anchor(
    connection: &Connection,
    scope: crate::CatalogReadScope,
    query: &str,
) -> rusqlite::Result<Option<String>> {
    let chars: Vec<_> = query.chars().collect();
    let width = chars.len().min(3);
    if width == 0 {
        return Ok(None);
    }
    let wanted: BTreeSet<String> = chars
        .windows(width)
        .map(|window| window.iter().collect())
        .collect();
    let mut statement = connection
        .prepare("SELECT members FROM session_search_counts WHERE read_scope=?1 AND gram=?2")?;
    let mut best = None;
    for gram in wanted {
        let members: Option<i64> = statement
            .query_row(params![scope_key(scope), gram], |row| row.get(0))
            .optional()?;
        record_steps(&statement);
        let Some(members) = members else {
            return Ok(None);
        };
        if best.as_ref().is_none_or(|(count, _)| members < *count) {
            best = Some((members, gram));
        }
    }
    Ok(best.map(|(_, gram)| gram))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Report {
    pub(crate) partial: bool,
    pub(crate) candidates: usize,
    pub(crate) index_complete: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_grams_cover_unicode_substrings_without_cross_field_matches() {
        assert_eq!(normalize_query("  ÉCHO "), Some("écho".into()));
        assert!(normalize_query("bad\nquery").is_none());
        let fields = vec!["héllo world".to_owned(), "other".to_owned()];
        let indexed = grams(&fields);
        for field in &fields {
            let chars: Vec<_> = field.chars().collect();
            for width in 1..=3 {
                for part in chars.windows(width) {
                    assert!(indexed.contains(&part.iter().collect::<String>()));
                }
            }
        }
        assert!(!indexed.contains("dor"));
    }
}

#[derive(Clone)]
pub(crate) struct Position {
    pub(crate) last: i64,
    pub(crate) key: String,
}
pub(crate) struct Selection {
    pub(crate) rows: Vec<CatalogRow>,
    pub(crate) next: Option<Position>,
    pub(crate) report: Report,
}

pub(crate) struct Query<'a> {
    pub(crate) scope: crate::CatalogReadScope,
    pub(crate) query: &'a str,
    pub(crate) harness: Option<&'a str>,
    pub(crate) repo: Option<&'a str>,
    pub(crate) after: Option<Position>,
    pub(crate) limit: usize,
    pub(crate) index_complete: bool,
}
pub(crate) fn read(connection: &Connection, request: Query<'_>) -> rusqlite::Result<Selection> {
    let Query {
        scope,
        query,
        harness,
        repo,
        after,
        limit,
        index_complete,
    } = request;
    let Some(gram) = anchor(connection, scope, query)? else {
        return Ok(Selection {
            rows: Vec::new(),
            next: None,
            report: Report {
                partial: false,
                candidates: 0,
                index_complete,
            },
        });
    };
    let postings = posting_page(connection, scope, &gram, after.as_ref(), MAX_CANDIDATES + 1)?;
    let more = postings.len() > MAX_CANDIDATES;
    let mut selected = Vec::with_capacity(limit);
    let mut checked = after;
    let mut candidates = 0;
    let mut bytes = 0;
    let mut partial = false;
    let mut document=connection.prepare("SELECT octet_length(fields),CASE WHEN octet_length(fields)<=?3 THEN fields ELSE NULL END FROM session_search_documents WHERE read_scope=?1 AND session_key=?2 AND complete=1")?;
    let metadata_sql = match scope {
        crate::CatalogReadScope::Current => {
            "SELECT octet_length(metadata),CASE WHEN octet_length(metadata)<=?2 THEN metadata ELSE NULL END,last_ms,harness,repo,lifecycle FROM session_catalog WHERE session_key=?1 AND lifecycle='current'"
        }
        crate::CatalogReadScope::RetainedHistory => {
            "SELECT octet_length(COALESCE(history.metadata,catalog.metadata)),CASE WHEN octet_length(COALESCE(history.metadata,catalog.metadata))<=?2 THEN COALESCE(history.metadata,catalog.metadata) ELSE NULL END,catalog.last_ms,catalog.harness,catalog.repo,catalog.lifecycle FROM session_catalog AS catalog LEFT JOIN session_history_catalog AS history USING(session_key) WHERE session_key=?1 AND (history.metadata IS NOT NULL OR catalog.lifecycle='current')"
        }
    };
    let mut metadata = connection.prepare(metadata_sql)?;
    for position in postings.iter().take(MAX_CANDIDATES) {
        let (length, encoded): (i64, Option<String>) = document.query_row(
            params![
                scope_key(scope),
                position.key,
                (MAX_SCAN_BYTES - bytes) as i64
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        record_steps(&document);
        let Some(encoded) = encoded else {
            partial = true;
            break;
        };
        bytes += usize::try_from(length).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let fields: Vec<String> =
            serde_json::from_str(&encoded).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if !fields.iter().any(|field| field.contains(query)) {
            candidates += 1;
            checked = Some(position.clone());
            continue;
        }
        let row = metadata
            .query_row(
                params![position.key, (MAX_SCAN_BYTES - bytes) as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?;
        record_steps(&metadata);
        let Some((length, encoded, last, row_harness, row_repo, lifecycle)) = row else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let Some(encoded) = encoded else {
            partial = true;
            break;
        };
        bytes += usize::try_from(length).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if harness.is_some_and(|wanted| wanted != row_harness)
            || repo.is_some_and(|wanted| Some(wanted) != row_repo.as_deref())
        {
            candidates += 1;
            checked = Some(position.clone());
            continue;
        }
        let row: CatalogRow =
            serde_json::from_str(&encoded).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if row.key != position.key
            || row.last.unwrap_or(0) != last
            || last != position.last
            || row.harness != row_harness
            || row.repo != row_repo
            || row.lifecycle.as_str() != lifecycle
            || fields != self::fields(&row)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        candidates += 1;
        if selected.len() == limit {
            return Ok(Selection {
                rows: selected,
                next: checked,
                report: Report {
                    partial: false,
                    candidates,
                    index_complete,
                },
            });
        }
        checked = Some(position.clone());
        selected.push(row);
    }
    let partial = partial || more;
    if partial && checked.is_none() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(Selection {
        rows: selected,
        next: partial.then_some(checked).flatten(),
        report: Report {
            partial,
            candidates,
            index_complete,
        },
    })
}

fn posting_page(
    connection: &Connection,
    scope: crate::CatalogReadScope,
    gram: &str,
    after: Option<&Position>,
    limit: usize,
) -> rusqlite::Result<Vec<Position>> {
    let mut found = Vec::with_capacity(limit);
    let mut read = |range: &str,
                    args: Vec<rusqlite::types::Value>,
                    limit: usize|
     -> rusqlite::Result<()> {
        let sql = format!(
            "SELECT last_ms,session_key FROM session_search_postings INDEXED BY session_search_order WHERE read_scope=? AND gram=? AND {range} ORDER BY last_ms DESC,session_key ASC LIMIT ?"
        );
        let mut args = args;
        args.push(rusqlite::types::Value::Integer(limit as i64));
        let mut statement = connection.prepare(&sql)?;
        found.extend(
            statement
                .query_map(rusqlite::params_from_iter(args), |row| {
                    Ok(Position {
                        last: row.get(0)?,
                        key: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        record_steps(&statement);
        Ok(())
    };
    let base = vec![
        rusqlite::types::Value::Text(scope_key(scope).into()),
        rusqlite::types::Value::Text(gram.into()),
    ];
    if let Some(after) = after {
        let mut args = base.clone();
        args.push(rusqlite::types::Value::Integer(after.last));
        args.push(rusqlite::types::Value::Text(after.key.clone()));
        read("last_ms=? AND session_key>?", args, limit)?;
        let remaining = limit - found.len();
        if remaining > 0 {
            let mut args = base;
            args.push(rusqlite::types::Value::Integer(after.last));
            let sql = "SELECT last_ms,session_key FROM session_search_postings INDEXED BY session_search_order WHERE read_scope=? AND gram=? AND last_ms<? ORDER BY last_ms DESC,session_key ASC LIMIT ?";
            args.push(rusqlite::types::Value::Integer(remaining as i64));
            let mut statement = connection.prepare(sql)?;
            found.extend(
                statement
                    .query_map(rusqlite::params_from_iter(args), |row| {
                        Ok(Position {
                            last: row.get(0)?,
                            key: row.get(1)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
            record_steps(&statement);
        }
    } else {
        read("1", base, limit)?;
    }
    Ok(found)
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    use crate::{CatalogReadScope, model::summary::CatalogLifecycle};

    fn row(key: &str, name: &str) -> CatalogRow {
        CatalogRow {
            lifecycle: CatalogLifecycle::Current,
            key: key.into(),
            name: name.into(),
            harness: "claude".into(),
            kind: "session".into(),
            native_ids: vec![key.into()],
            parent: None,
            parent_source: None,
            repo: Some("fixture".into()),
            branch: None,
            model: "fixture-model".into(),
            effort: None,
            start: Some(1),
            last: Some(1),
            tokens: [0.0; 3],
            cost: crate::pricing::Cost {
                usd: None,
                unpriced_models: Vec::new(),
                split_unknown_messages: 0,
                by_model: Default::default(),
                by_day: Default::default(),
            },
            sources: Vec::new(),
        }
    }
    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection.execute_batch("CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY,last_ms INTEGER,harness TEXT,repo TEXT,lifecycle TEXT,metadata TEXT); CREATE TABLE session_history_catalog(session_key TEXT PRIMARY KEY,metadata TEXT); CREATE TEMP TABLE catalog_members(session_key TEXT PRIMARY KEY) WITHOUT ROWID;").unwrap();
        connection
    }
    fn put(transaction: &Transaction<'_>, row: &CatalogRow) {
        let encoded = serde_json::to_string(row).unwrap();
        transaction
            .execute(
                "INSERT OR REPLACE INTO session_catalog VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    row.key,
                    row.last.unwrap_or(0),
                    row.harness,
                    row.repo,
                    row.lifecycle.as_str(),
                    encoded
                ],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT OR REPLACE INTO session_history_catalog VALUES(?1,?2)",
                params![row.key, encoded],
            )
            .unwrap();
        publish_row(transaction, CatalogReadScope::Current, row).unwrap();
        publish_row(transaction, CatalogReadScope::RetainedHistory, row).unwrap();
    }
    fn search(
        connection: &Connection,
        query: &str,
        after: Option<Position>,
        limit: usize,
    ) -> Selection {
        read(
            connection,
            Query {
                scope: CatalogReadScope::Current,
                query: &normalize_query(query).unwrap(),
                harness: None,
                repo: None,
                after,
                limit,
                index_complete: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn retained_search_reads_current_metadata_before_a_history_copy_exists() {
        let mut connection = database();
        let transaction = connection.transaction().unwrap();
        put(&transaction, &row("current", "Harbor queue"));
        transaction
            .execute("DELETE FROM session_history_catalog", [])
            .unwrap();
        transaction.commit().unwrap();
        let query = || Query {
            scope: CatalogReadScope::RetainedHistory,
            query: "harbor",
            harness: None,
            repo: None,
            after: None,
            limit: 60,
            index_complete: true,
        };
        let found = read(&connection, query()).unwrap();
        assert_eq!(found.rows.len(), 1);
        assert_eq!(found.rows[0].key, "current");
        assert!(!found.report.partial);
        // An absent retained source is not a readable current fallback.
        connection
            .execute("UPDATE session_catalog SET lifecycle='retained'", [])
            .unwrap();
        assert!(read(&connection, query()).is_err());
    }

    #[test]
    fn search_order_work_and_exact_metadata_are_independent_of_unrelated_history() {
        let mut connection = database();
        let transaction = connection.transaction().unwrap();
        for id in 0..3 {
            put(
                &transaction,
                &row(&format!("target-{id:05}"), "Needle ÉCHO"),
            );
        }
        transaction.commit().unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let first_started = std::time::Instant::now();
        let first = search(&connection, "NEEDLE", None, 2);
        let first_micros = first_started.elapsed().as_micros();
        let baseline = SQL_STEPS.with(|steps| steps.get());
        assert_eq!(
            first
                .rows
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            vec!["target-00000", "target-00001"]
        );
        assert!(!first.report.partial);
        let next = first.next.clone().unwrap();
        let template = serde_json::to_string(&row("unrelated", "Irrelevant history")).unwrap();
        connection.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO session_catalog SELECT 'unrelated-'||printf('%05d',x),1,'claude','fixture','current',json_set(?1,'$.key','unrelated-'||printf('%05d',x),'$.native_ids',json_array('unrelated-'||printf('%05d',x))) FROM n",[template]).unwrap();
        connection.execute_batch("INSERT INTO session_search_postings SELECT 'current',session_key,'irr',last_ms FROM session_catalog WHERE session_key LIKE 'unrelated-%'; INSERT INTO session_search_counts VALUES('current','irr',20000);").unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let grown_started = std::time::Instant::now();
        let grown = search(&connection, "needle", None, 2);
        let grown_micros = grown_started.elapsed().as_micros();
        let changed = SQL_STEPS.with(|steps| steps.get());
        assert_eq!(
            grown
                .rows
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            vec!["target-00000", "target-00001"]
        );
        assert_eq!(changed, baseline);
        let first_bytes = serde_json::to_vec(&first.rows).unwrap();
        assert_eq!(serde_json::to_vec(&grown.rows).unwrap(), first_bytes);
        eprintln!(
            "metadata q fixed-two-row SQL VM {baseline}->{changed}; single in-memory read {first_micros}->{grown_micros}us; response {} bytes; unrelated rows0->20000 (not a restart or browser journey)",
            first_bytes.len()
        );
        let last = search(&connection, "needle", Some(next), 2);
        assert_eq!(last.rows[0].key, "target-00002");
        assert!(last.next.is_none());
        assert_eq!(search(&connection, "écho", None, 2).rows.len(), 2);
        let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT last_ms,session_key FROM session_search_postings INDEXED BY session_search_order WHERE read_scope='current' AND gram='nee' ORDER BY last_ms DESC,session_key ASC LIMIT 3",[],|row|row.get(3)).unwrap();
        assert!(
            plan.contains("COVERING INDEX session_search_order"),
            "{plan}"
        );
    }

    #[test]
    fn false_positives_continue_without_skipping_matches_and_updates_are_atomic() {
        let mut connection = database();
        let transaction = connection.transaction().unwrap();
        for id in 0..600 {
            put(
                &transaction,
                &row(
                    &format!("candidate-{id:05}"),
                    "abc abcX bcXY cXYZ XYZa YZab Zabc",
                ),
            );
        }
        put(&transaction, &row("candidate-00900", "abcXYZabc"));
        transaction.commit().unwrap();
        let first = search(&connection, "abcXYZabc", None, 2);
        assert!(first.rows.is_empty());
        assert!(first.report.partial);
        assert_eq!(first.report.candidates, MAX_CANDIDATES);
        let last = search(&connection, "abcXYZabc", first.next, 2);
        assert_eq!(last.rows.len(), 1);
        assert_eq!(last.rows[0].key, "candidate-00900");
        assert!(last.next.is_none());
        {
            let transaction = connection.transaction().unwrap();
            put(&transaction, &row("candidate-00900", "replacement"));
            // An interrupted publication must retain old metadata and postings.
        }
        assert_eq!(
            search(&connection, "abcXYZabc", None, 2).report.candidates,
            MAX_CANDIDATES
        );
        let transaction = connection.transaction().unwrap();
        put(&transaction, &row("candidate-00900", "replacement"));
        transaction.commit().unwrap();
        let old = search(&connection, "abcXYZabc", None, 2);
        let old = search(&connection, "abcXYZabc", old.next, 2);
        assert!(old.rows.is_empty());
        assert_eq!(search(&connection, "replacement", None, 2).rows.len(), 1);
        connection.execute_batch("DELETE FROM catalog_members; INSERT INTO catalog_members SELECT session_key FROM session_catalog WHERE session_key!='candidate-00900';").unwrap();
        let transaction = connection.transaction().unwrap();
        retire_missing(&transaction).unwrap();
        transaction.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='candidate-00900'",[]).unwrap();
        transaction.execute("UPDATE session_history_catalog SET metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='candidate-00900'",[]).unwrap();
        transaction.commit().unwrap();
        assert!(search(&connection, "replacement", None, 2).rows.is_empty());
        let history = read(
            &connection,
            Query {
                scope: CatalogReadScope::RetainedHistory,
                query: "replacement",
                harness: None,
                repo: None,
                after: None,
                limit: 2,
                index_complete: true,
            },
        )
        .unwrap();
        assert_eq!(history.rows[0].key, "candidate-00900");
    }
    #[test]
    fn oversized_metadata_preserves_explicit_coverage_and_rollback() {
        let mut connection = database();
        let oversized = row("oversized", &"x".repeat(MAX_DOCUMENT_BYTES + 1));
        let transaction = connection.transaction().unwrap();
        put(&transaction, &oversized);
        transaction.commit().unwrap();
        let incomplete = |connection: &Connection, scope: &str| -> i64 {
            connection
                .query_row(
                    "SELECT incomplete FROM session_search_coverage WHERE read_scope=?1",
                    [scope],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(incomplete(&connection, "current"), 1);
        let response = read(
            &connection,
            Query {
                scope: CatalogReadScope::Current,
                query: "x",
                harness: None,
                repo: None,
                after: None,
                limit: 2,
                index_complete: false,
            },
        )
        .unwrap();
        assert!(response.rows.is_empty());
        assert!(!response.report.index_complete);
        assert!(response.next.is_none());
        {
            let transaction = connection.transaction().unwrap();
            put(&transaction, &row("oversized", "replacement"));
        }
        assert_eq!(incomplete(&connection, "current"), 1);
        let transaction = connection.transaction().unwrap();
        remove_row(&transaction, "current", "oversized").unwrap();
        transaction.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='oversized'",[]).unwrap();
        transaction.execute("UPDATE session_history_catalog SET metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='oversized'",[]).unwrap();
        transaction.commit().unwrap();
        assert_eq!(incomplete(&connection, "current"), 0);
        assert_eq!(incomplete(&connection, "retained_history"), 1);
    }
}
