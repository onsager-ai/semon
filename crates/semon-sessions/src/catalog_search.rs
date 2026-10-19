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

pub(crate) const VERSION: &str = "2";
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
CREATE TABLE IF NOT EXISTS session_search_memberships (
    session_key TEXT NOT NULL,
    gram TEXT NOT NULL,
    current_last_ms INTEGER,
    history_last_ms INTEGER,
    PRIMARY KEY(session_key,gram),
    CHECK(current_last_ms IS NOT NULL OR history_last_ms IS NOT NULL)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_search_current_order ON session_search_memberships(gram,current_last_ms DESC,session_key ASC) WHERE current_last_ms IS NOT NULL;
CREATE INDEX IF NOT EXISTS session_search_history_order ON session_search_memberships(gram,history_last_ms DESC,session_key ASC) WHERE history_last_ms IS NOT NULL;
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

fn membership(scope: &str) -> rusqlite::Result<(&'static str, &'static str)> {
    match scope {
        "current" => Ok(("current_last_ms", "history_last_ms")),
        "retained_history" => Ok(("history_last_ms", "current_last_ms")),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

/// A key belongs entirely to one physical representation, across both scopes.
/// Both probes use selected-key ranges; neither traverses other owners.
fn representation(connection: &Connection, key: &str) -> rusqlite::Result<(bool, bool)> {
    let mut statement = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM session_search_postings WHERE read_scope IN ('current','retained_history') AND session_key=?1),EXISTS(SELECT 1 FROM session_search_memberships WHERE session_key=?1)")?;
    let found = statement.query_row([key], |row| Ok((row.get(0)?, row.get(1)?)))?;
    record_steps(&statement);
    if found == (true, true) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(found)
}

type LegacyPosting = (String, String, i64);

/// Qualify every old scope before changing either. Corrupt large scalar values
/// are rejected by SQLite before allocation; 1,025 is an excess sentinel.
fn old_positions(connection: &Connection, key: &str) -> rusqlite::Result<Vec<LegacyPosting>> {
    let (legacy, compact) = representation(connection, key)?;
    if compact {
        // Bound the union before applying either scope predicate: otherwise a
        // corrupt owner with arbitrarily many other-scope rows could make an
        // absent-scope check traverse the complete owner first.
        let (physical, valid): (i64, i64) = connection.prepare_cached("SELECT COUNT(*),COUNT(valid) FROM (SELECT CASE WHEN octet_length(gram)<=12 THEN 1 ELSE NULL END AS valid FROM session_search_memberships WHERE session_key=?1 LIMIT ?2)")?.query_row(params![key,(MAX_GRAMS*2+1) as i64], |row|Ok((row.get(0)?,row.get(1)?)))?;
        if physical > (MAX_GRAMS * 2) as i64 || physical != valid {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let mut found = Vec::new();
    for scope in ["current", "retained_history"] {
        if legacy {
            let mut statement = connection.prepare_cached("SELECT CASE WHEN octet_length(gram)<=12 THEN gram ELSE NULL END,last_ms FROM session_search_postings WHERE read_scope=?1 AND session_key=?2 ORDER BY gram LIMIT ?3")?;
            let rows = statement
                .query_map(params![scope, key, (MAX_GRAMS + 1) as i64], |row| {
                    Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.len() > MAX_GRAMS {
                return Err(rusqlite::Error::InvalidQuery);
            }
            for (gram, last) in rows {
                found.push((
                    scope.to_owned(),
                    gram.ok_or(rusqlite::Error::InvalidQuery)?,
                    last,
                ));
            }
        }
        if compact {
            let (column, _) = membership(scope)?;
            let sql = format!(
                "SELECT COUNT(*) FROM (SELECT 1 FROM session_search_memberships WHERE session_key=?1 AND {column} IS NOT NULL LIMIT ?2)"
            );
            let count: i64 = connection
                .prepare_cached(&sql)?
                .query_row(params![key, (MAX_GRAMS + 1) as i64], |row| row.get(0))?;
            if count > MAX_GRAMS as i64 {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
    }
    Ok(found)
}

fn convert_key(transaction: &Transaction<'_>, key: &str) -> rusqlite::Result<()> {
    let positions = old_positions(transaction, key)?;
    if positions.is_empty() {
        return Ok(());
    }
    for (scope, gram, last) in positions {
        let (column, _) = membership(&scope)?;
        let sql = format!(
            "INSERT INTO session_search_memberships(session_key,gram,{column}) VALUES(?1,?2,?3) ON CONFLICT(session_key,gram) DO UPDATE SET {column}=excluded.{column}"
        );
        transaction
            .prepare_cached(&sql)?
            .execute(params![key, gram, last])?;
    }
    transaction.prepare_cached("DELETE FROM session_search_postings WHERE read_scope IN ('current','retained_history') AND session_key=?1")?.execute([key])?;
    Ok(())
}

pub(crate) fn clear(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch("DELETE FROM session_search_postings; DELETE FROM session_search_memberships; DELETE FROM session_search_counts; DELETE FROM session_search_documents; UPDATE session_search_coverage SET incomplete=0;")
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
#[cfg(test)]
pub(crate) fn publish_row(
    transaction: &Transaction<'_>,
    scope: crate::CatalogReadScope,
    row: &CatalogRow,
) -> rusqlite::Result<()> {
    let scope = scope_key(scope);
    remove_row(transaction, scope, &row.key)?;
    // Reuse the existing bounded posting inserts, then advance counts with one
    // source-key range scan. Per-gram SQL programs dominated cold batches.
    insert_row(transaction, scope, row, false)?;
    let (column, _) = membership(scope)?;
    transaction.prepare_cached(&format!("INSERT INTO session_search_counts(read_scope,gram,members) SELECT ?1,gram,1 FROM session_search_memberships WHERE session_key=?2 AND {column} IS NOT NULL ON CONFLICT(read_scope,gram) DO UPDATE SET members=members+excluded.members"))?.execute(params![scope,row.key])?;
    Ok(())
}

/// Update a bounded producer cohort's current/history postings together. The
/// temporary key set never becomes another read model or survives the transaction.
pub(crate) fn publish_current_rows(
    transaction: &Transaction<'_>,
    rows: &[&CatalogRow],
) -> rusqlite::Result<()> {
    publish_current_rows_order(
        transaction,
        rows,
        64,
        #[cfg(test)]
        None,
    )
}

#[cfg(test)]
pub(crate) fn publish_current_rows_probe(
    transaction: &Transaction<'_>,
    rows: &[&CatalogRow],
    owners: usize,
    probe: Option<&mut Option<crate::native_timing::SearchTraversalProbe>>,
) -> rusqlite::Result<()> {
    assert!(matches!(owners, 64 | 16 | 8));
    assert!(rows.len() <= 64);
    publish_current_rows_order(transaction, rows, owners, probe)
}

fn posting_order<'a>(
    rows: &[&'a CatalogRow],
    prepared: &'a [(String, BTreeSet<String>, bool)],
    owners: usize,
) -> Vec<(&'a String, &'a CatalogRow)> {
    let mut postings = rows
        .iter()
        .zip(prepared)
        .flat_map(|(row, (_, grams, complete))| {
            grams
                .iter()
                .filter(move |_| *complete)
                .map(move |gram| (gram, *row))
        })
        .collect::<Vec<_>>();
    if owners == 64 {
        // The release path retains the original gram/key comparator exactly.
        postings.sort_unstable_by(|(left, a), (right, b)| {
            left.cmp(right).then_with(|| a.key.cmp(&b.key))
        });
    } else {
        let mut keys = rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>();
        keys.sort_unstable();
        let groups = keys
            .into_iter()
            .enumerate()
            .map(|(position, key)| (key, position / owners))
            .collect::<std::collections::BTreeMap<_, _>>();
        postings.sort_unstable_by(|(left, a), (right, b)| {
            groups[a.key.as_str()]
                .cmp(&groups[b.key.as_str()])
                .then_with(|| left.cmp(right))
                .then_with(|| a.key.cmp(&b.key))
        });
    }
    postings
}

/// Run on the same immutable fixture cohorts before opening measured stores.
/// These validation sorts must never enter preparation or full-search timing.
#[cfg(test)]
pub(crate) fn assert_posting_multiset(rows: &[CatalogRow]) -> rusqlite::Result<()> {
    assert!(rows.len() <= 64);
    let rows = rows.iter().collect::<Vec<_>>();
    let prepared = rows
        .iter()
        .map(|row| prepared_fields(row))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut original = rows
        .iter()
        .zip(&prepared)
        .flat_map(|(row, (_, grams, complete))| {
            grams
                .iter()
                .filter(move |_| *complete)
                .map(move |gram| (gram, *row))
        })
        .collect::<Vec<_>>();
    original
        .sort_unstable_by(|(left, a), (right, b)| left.cmp(right).then_with(|| a.key.cmp(&b.key)));
    for owners in [64, 16, 8] {
        let mut actual = posting_order(&rows, &prepared, owners);
        actual.sort_unstable_by(|(left, a), (right, b)| {
            left.cmp(right).then_with(|| a.key.cmp(&b.key))
        });
        assert_eq!(original.len(), actual.len());
        assert!(
            original
                .iter()
                .zip(&actual)
                .all(|((left, a), (right, b))| left == right && std::ptr::eq(*a, *b))
        );
    }
    Ok(())
}

fn publish_current_rows_order(
    transaction: &Transaction<'_>,
    rows: &[&CatalogRow],
    owners: usize,
    #[cfg(test)] mut probe: Option<&mut Option<crate::native_timing::SearchTraversalProbe>>,
) -> rusqlite::Result<()> {
    let producer = crate::native_timing::enabled()
        .then(|| {
            transaction
                .path()
                .map(std::path::Path::new)
                .map(crate::native_timing::id)
        })
        .flatten();
    let mut measurements = crate::native_timing::Measurements::new(producer.is_some());
    for rows in rows.chunks(64) {
        #[cfg(test)]
        let search_before = probe
            .as_ref()
            .map(|_| crate::native_timing::ProbeSample::read(transaction));
        let started = measurements.start();
        transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_search_members(session_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM catalog_search_members;")?;
        for row in rows {
            transaction
                .prepare_cached("INSERT INTO catalog_search_members VALUES(?1)")?
                .execute([&row.key])?;
        }
        measurements.record("members", started, rows.len());
        let started = measurements.start();
        for row in rows {
            old_positions(transaction, &row.key)?;
        }
        for scope in ["current", "retained_history"] {
            remove_members(transaction, scope)?;
        }
        measurements.record("removal", started, 2);
        let started = measurements.start();
        let prepared = rows
            .iter()
            .map(|row| prepared_fields(row))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        measurements.record("normalization", started, rows.len());
        let started = measurements.start();
        for (row, (encoded, _, complete)) in rows.iter().zip(&prepared) {
            for scope in ["current", "retained_history"] {
                insert_document(transaction, scope, row, encoded, *complete)?;
            }
        }
        measurements.record("documents", started, rows.len() * 2);
        // The ordering index groups grams, while its primary key groups source
        // keys. At most 64 source ranges are resident here. Gram-major writes
        // revisit their bounded primary-key pages while keeping the ordering
        // index's current leaf hot, instead of cycling through all gram leaves
        // for each source and repeatedly spilling dirty pages into the WAL.
        let started = measurements.start();
        #[cfg(test)]
        let preparation_started = probe.as_ref().map(|_| std::time::Instant::now());
        let postings = posting_order(rows, &prepared, owners);
        #[cfg(test)]
        let preparation_us = preparation_started
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        measurements.record("posting_preparation", started, postings.len());
        #[cfg(test)]
        let postings_before = probe
            .as_ref()
            .map(|_| crate::native_timing::ProbeSample::read(transaction));
        let started = measurements.start();
        let mut posting = transaction.prepare_cached("INSERT INTO session_search_memberships(session_key,gram,current_last_ms,history_last_ms) VALUES(?1,?2,?3,?3)")?;
        for (gram, row) in &postings {
            posting.execute(params![row.key, gram, row.last.unwrap_or(0)])?;
        }
        // Only the numerical probe releases before its after-posting sample;
        // the ordinary release/Auto path retains the original lexical lifetime.
        #[cfg(test)]
        if probe.is_some() {
            drop(posting);
        }
        #[cfg(test)]
        let postings_after = probe
            .as_ref()
            .map(|_| crate::native_timing::ProbeSample::read(transaction));
        measurements.record("postings", started, postings.len() * 2);
        measurements.count("posting_rows", postings.len());
        let started = measurements.start();
        for scope in ["current", "retained_history"] {
            // CROSS JOIN keeps at most 64 members as the outer loop; each inner
            // lookup uses the existing (scope,source key,gram) primary key.
            let (column, _) = membership(scope)?;
            transaction.prepare_cached(&format!("INSERT INTO session_search_counts(read_scope,gram,members) SELECT ?1,p.gram,COUNT(*) FROM catalog_search_members m CROSS JOIN session_search_memberships p WHERE p.session_key=m.session_key AND p.{column} IS NOT NULL GROUP BY p.gram ON CONFLICT(read_scope,gram) DO UPDATE SET members=members+excluded.members"))?.execute([scope])?;
        }
        measurements.record("counts", started, 2);
        #[cfg(test)]
        if let Some(probe) = probe.as_deref_mut() {
            let counts_after = crate::native_timing::ProbeSample::read(transaction);
            *probe = Some(crate::native_timing::SearchTraversalProbe {
                search: search_before.as_ref().unwrap().interval(&counts_after),
                postings: postings_before
                    .as_ref()
                    .unwrap()
                    .interval(postings_after.as_ref().unwrap()),
                counts: postings_after.as_ref().unwrap().interval(&counts_after),
                preparation_us,
                sampling_us: search_before.as_ref().unwrap().sampling_us()
                    + postings_before.as_ref().unwrap().sampling_us()
                    + postings_after.as_ref().unwrap().sampling_us()
                    + counts_after.sampling_us(),
                owners: rows.len(),
                physical_rows: postings.len(),
                logical_positions: postings.len() * 2,
            });
        }
    }
    if let Some(producer) = producer {
        measurements.emit(&producer, "search_rows");
    }
    Ok(())
}

/// Remove only this cohort's old documents and postings. Aggregate shared gram
/// decrements once, using source-key lookups rather than scanning all members of
/// each gram. The temporary tables and count changes roll back with publication.
fn remove_members(transaction: &Transaction<'_>, scope: &str) -> rusqlite::Result<()> {
    transaction.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_search_removals(gram TEXT PRIMARY KEY,members INTEGER NOT NULL) WITHOUT ROWID; DELETE FROM catalog_search_removals;")?;
    transaction.prepare_cached("INSERT INTO catalog_search_removals SELECT p.gram,COUNT(*) FROM catalog_search_members m CROSS JOIN session_search_postings p WHERE p.read_scope=?1 AND p.session_key=m.session_key GROUP BY p.gram")?.execute([scope])?;
    let (column, other) = membership(scope)?;
    transaction.prepare_cached(&format!("INSERT INTO catalog_search_removals SELECT p.gram,COUNT(*) FROM catalog_search_members m CROSS JOIN session_search_memberships p WHERE p.session_key=m.session_key AND p.{column} IS NOT NULL GROUP BY p.gram ON CONFLICT(gram) DO UPDATE SET members=members+excluded.members"))?.execute([])?;
    transaction.prepare_cached("UPDATE session_search_counts SET members=members-(SELECT r.members FROM catalog_search_removals r WHERE r.gram=session_search_counts.gram) WHERE read_scope=?1 AND gram IN (SELECT gram FROM catalog_search_removals)")?.execute([scope])?;
    transaction.prepare_cached("DELETE FROM session_search_counts WHERE read_scope=?1 AND members=0 AND gram IN (SELECT gram FROM catalog_search_removals)")?.execute([scope])?;
    transaction.prepare_cached("UPDATE session_search_coverage SET incomplete=incomplete-(SELECT COUNT(*) FROM catalog_search_members m CROSS JOIN session_search_documents d WHERE d.read_scope=?1 AND d.session_key=m.session_key AND d.complete=0) WHERE read_scope=?1")?.execute([scope])?;
    transaction.prepare_cached("DELETE FROM session_search_postings WHERE read_scope=?1 AND session_key IN (SELECT session_key FROM catalog_search_members)")?.execute([scope])?;
    transaction.prepare_cached(&format!("DELETE FROM session_search_memberships WHERE {other} IS NULL AND session_key IN (SELECT session_key FROM catalog_search_members)"))?.execute([])?;
    transaction.prepare_cached(&format!("UPDATE session_search_memberships SET {column}=NULL WHERE {column} IS NOT NULL AND session_key IN (SELECT session_key FROM catalog_search_members)"))?.execute([])?;
    transaction.prepare_cached("DELETE FROM session_search_documents WHERE read_scope=?1 AND session_key IN (SELECT session_key FROM catalog_search_members)")?.execute([scope])?;
    Ok(())
}

/// A complete rebuild starts with empty postings and documents. Aggregate
/// their exact member counts once, rather than updating each gram per row.
pub(crate) fn backfill_row(
    transaction: &Transaction<'_>,
    scope: crate::CatalogReadScope,
    row: &CatalogRow,
) -> rusqlite::Result<()> {
    insert_row(transaction, scope_key(scope), row, false)
}

/// Current and retained-history search share the same allowlisted fields.
/// Normalize and tokenize one current row once before inserting both scopes.
pub(crate) fn backfill_catalog_row(
    transaction: &Transaction<'_>,
    row: &CatalogRow,
) -> rusqlite::Result<()> {
    if row.lifecycle != crate::model::summary::CatalogLifecycle::Current {
        return backfill_row(transaction, crate::CatalogReadScope::RetainedHistory, row);
    }
    if representation(transaction, &row.key)? != (false, false) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let (encoded, grams, complete) = prepared_fields(row)?;
    for scope in ["current", "retained_history"] {
        insert_document(transaction, scope, row, &encoded, complete)?;
    }
    if complete {
        let mut posting = transaction.prepare_cached("INSERT INTO session_search_memberships(session_key,gram,current_last_ms,history_last_ms) VALUES(?1,?2,?3,?3)")?;
        for gram in grams {
            posting.execute(params![row.key, gram, row.last.unwrap_or(0)])?;
        }
    }
    Ok(())
}

/// Rebuild the ordering index once after bulk insertion. This runs inside the
/// Catalog publication transaction: readers retain the previous complete index,
/// and rollback restores both its contents and schema.
pub(crate) fn begin_backfill(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    clear(transaction)?;
    transaction.execute_batch("DROP INDEX IF EXISTS session_search_order; DROP INDEX IF EXISTS session_search_current_order; DROP INDEX IF EXISTS session_search_history_order;")
}

pub(crate) fn finish_backfill(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch("CREATE INDEX IF NOT EXISTS session_search_order ON session_search_postings(read_scope,gram,last_ms DESC,session_key ASC); CREATE INDEX IF NOT EXISTS session_search_current_order ON session_search_memberships(gram,current_last_ms DESC,session_key ASC) WHERE current_last_ms IS NOT NULL; CREATE INDEX IF NOT EXISTS session_search_history_order ON session_search_memberships(gram,history_last_ms DESC,session_key ASC) WHERE history_last_ms IS NOT NULL;")?;
    transaction.execute(
        "INSERT INTO session_search_counts(read_scope,gram,members) SELECT read_scope,gram,COUNT(*) FROM session_search_postings GROUP BY read_scope,gram",
        [],
    )?;
    for scope in ["current", "retained_history"] {
        let (column, _) = membership(scope)?;
        transaction.execute(&format!("INSERT INTO session_search_counts(read_scope,gram,members) SELECT ?1,gram,COUNT(*) FROM session_search_memberships WHERE {column} IS NOT NULL GROUP BY gram ON CONFLICT(read_scope,gram) DO UPDATE SET members=members+excluded.members"), [scope])?;
    }
    Ok(())
}

fn insert_row(
    transaction: &Transaction<'_>,
    scope: &str,
    row: &CatalogRow,
    maintain_counts: bool,
) -> rusqlite::Result<()> {
    convert_key(transaction, &row.key)?;
    let (encoded, grams, complete) = prepared_fields(row)?;
    insert_fields(
        transaction,
        scope,
        row,
        &encoded,
        &grams,
        complete,
        maintain_counts,
    )
}

fn prepared_fields(row: &CatalogRow) -> rusqlite::Result<(String, BTreeSet<String>, bool)> {
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
    Ok((encoded, grams, complete))
}

fn insert_fields(
    transaction: &Transaction<'_>,
    scope: &str,
    row: &CatalogRow,
    encoded: &str,
    grams: &BTreeSet<String>,
    complete: bool,
    maintain_counts: bool,
) -> rusqlite::Result<()> {
    insert_document(transaction, scope, row, encoded, complete)?;
    if complete {
        // Single posting statements avoid multi-row rollback work over many
        // nonadjacent ordering-index pages. Exact counts still advance once
        // per producer cohort inside this same publication transaction.
        let (column, _) = membership(scope)?;
        let sql = format!(
            "INSERT INTO session_search_memberships(session_key,gram,{column}) VALUES(?1,?2,?3) ON CONFLICT(session_key,gram) DO UPDATE SET {column}=excluded.{column}"
        );
        let mut posting = transaction.prepare_cached(&sql)?;
        let mut count = maintain_counts.then(|| transaction.prepare_cached("INSERT INTO session_search_counts(read_scope,gram,members) VALUES(?1,?2,1) ON CONFLICT(read_scope,gram) DO UPDATE SET members=members+1")).transpose()?;
        for gram in grams {
            posting.execute(params![row.key, gram, row.last.unwrap_or(0)])?;
            if let Some(count) = &mut count {
                count.execute(params![scope, gram])?;
            }
        }
    }
    Ok(())
}

fn insert_document(
    transaction: &Transaction<'_>,
    scope: &str,
    row: &CatalogRow,
    encoded: &str,
    complete: bool,
) -> rusqlite::Result<()> {
    transaction.prepare_cached("INSERT INTO session_search_documents(read_scope,session_key,fields,complete) VALUES(?1,?2,?3,?4)")?.execute(params![scope,row.key,encoded,complete])?;
    if !complete {
        transaction
            .prepare_cached(
                "UPDATE session_search_coverage SET incomplete=incomplete+1 WHERE read_scope=?1",
            )?
            .execute([scope])?;
    }
    Ok(())
}

fn remove_row(transaction: &Transaction<'_>, scope: &str, key: &str) -> rusqlite::Result<()> {
    convert_key(transaction, key)?;
    let (column, other) = membership(scope)?;
    transaction.prepare_cached("UPDATE session_search_coverage SET incomplete=incomplete-1 WHERE read_scope=?1 AND EXISTS(SELECT 1 FROM session_search_documents WHERE read_scope=?1 AND session_key=?2 AND complete=0)")?.execute(params![scope,key])?;
    transaction.prepare_cached(&format!("UPDATE session_search_counts SET members=members-1 WHERE read_scope=?1 AND gram IN (SELECT gram FROM session_search_memberships WHERE session_key=?2 AND {column} IS NOT NULL)"))?.execute(params![scope,key])?;
    transaction.prepare_cached(&format!("DELETE FROM session_search_counts WHERE read_scope=?1 AND members=0 AND gram IN (SELECT gram FROM session_search_memberships WHERE session_key=?2 AND {column} IS NOT NULL)"))?.execute(params![scope,key])?;
    transaction
        .prepare_cached(&format!(
            "DELETE FROM session_search_memberships WHERE session_key=?1 AND {other} IS NULL"
        ))?
        .execute([key])?;
    transaction.prepare_cached(&format!("UPDATE session_search_memberships SET {column}=NULL WHERE session_key=?1 AND {column} IS NOT NULL"))?.execute([key])?;
    transaction
        .prepare_cached(
            "DELETE FROM session_search_documents WHERE read_scope=?1 AND session_key=?2",
        )?
        .execute(params![scope, key])?;
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
    pub(crate) inclusive: bool,
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
    pub(crate) model: Option<&'a str>,
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
        model,
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
            || repo.is_some_and(|wanted| {
                if wanted.is_empty() {
                    row_repo.is_some()
                } else {
                    Some(wanted) != row_repo.as_deref()
                }
            })
        {
            candidates += 1;
            checked = Some(position.clone());
            continue;
        }
        let row: CatalogRow =
            serde_json::from_str(&encoded).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if model.is_some_and(|wanted| wanted != row.model) {
            candidates += 1;
            checked = Some(position.clone());
            continue;
        }
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

fn posting_sql(scope: crate::CatalogReadScope, range: &str, limit: usize) -> String {
    let (column, index) = match scope {
        crate::CatalogReadScope::Current => ("current_last_ms", "session_search_current_order"),
        crate::CatalogReadScope::RetainedHistory => {
            ("history_last_ms", "session_search_history_order")
        }
    };
    let compact_range = range.replace("last_ms", column);
    format!(
        "SELECT last_ms,session_key FROM session_search_postings INDEXED BY session_search_order WHERE read_scope=?1 AND gram=?2 AND {range} UNION ALL SELECT {column} AS last_ms,session_key FROM session_search_memberships INDEXED BY {index} WHERE gram=?2 AND {column} IS NOT NULL AND {compact_range} ORDER BY last_ms DESC,session_key ASC LIMIT ?{limit}"
    )
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
                    mut args: Vec<rusqlite::types::Value>,
                    limit: usize|
     -> rusqlite::Result<()> {
        let sql = posting_sql(scope, range, args.len() + 1);
        args.push(rusqlite::types::Value::Integer(limit as i64));
        let mut statement = connection.prepare(&sql)?;
        found.extend(
            statement
                .query_map(rusqlite::params_from_iter(args), |row| {
                    Ok(Position {
                        inclusive: false,
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
        read(
            if after.inclusive {
                "last_ms=?3 AND session_key>=?4"
            } else {
                "last_ms=?3 AND session_key>?4"
            },
            args,
            limit,
        )?;
        let remaining = limit - found.len();
        if remaining > 0 {
            let mut args = base;
            args.push(rusqlite::types::Value::Integer(after.last));
            let sql = posting_sql(scope, "last_ms<?3", 4);
            args.push(rusqlite::types::Value::Integer(remaining as i64));
            let mut statement = connection.prepare(&sql)?;
            found.extend(
                statement
                    .query_map(rusqlite::params_from_iter(args), |row| {
                        Ok(Position {
                            inclusive: false,
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
    for position in &found {
        representation(connection, &position.key)?;
    }
    Ok(found)
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    use crate::{CatalogReadScope, model::summary::CatalogLifecycle};

    const LOGICAL_POSITIONS: &str = "SELECT json_array(read_scope,session_key,gram,last_ms) FROM (SELECT read_scope,session_key,gram,last_ms FROM session_search_postings UNION ALL SELECT 'current',session_key,gram,current_last_ms FROM session_search_memberships WHERE current_last_ms IS NOT NULL UNION ALL SELECT 'retained_history',session_key,gram,history_last_ms FROM session_search_memberships WHERE history_last_ms IS NOT NULL) ORDER BY read_scope,session_key,gram";
    const PHYSICAL_COMPACT: &str = "SELECT json_array(session_key,gram,current_last_ms,history_last_ms) FROM session_search_memberships ORDER BY session_key,gram";

    fn strings(connection: &Connection, sql: &str) -> Vec<String> {
        connection
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn legacy_key(connection: &Connection, key: &str) {
        for (scope, column) in [
            ("current", "current_last_ms"),
            ("retained_history", "history_last_ms"),
        ] {
            connection.execute(&format!("INSERT INTO session_search_postings SELECT ?1,session_key,gram,{column} FROM session_search_memberships WHERE session_key=?2 AND {column} IS NOT NULL"), params![scope,key]).unwrap();
        }
        connection
            .execute(
                "DELETE FROM session_search_memberships WHERE session_key=?1",
                [key],
            )
            .unwrap();
    }

    fn row(key: &str, name: &str) -> CatalogRow {
        CatalogRow {
            overview: None,
            trace: None,
            query: None,
            lifecycle: CatalogLifecycle::Current,
            key: key.into(),
            name: name.into(),
            harness: "claude".into(),
            kind: "session".into(),
            presentation_kind: None,
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
                model: None,
                after,
                limit,
                index_complete: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn cohort_replacement_preserves_shared_counts_coverage_and_rollback() {
        let mut reference = database();
        let mut batched = database();
        let rows = (0..130)
            .map(|id| {
                row(
                    &format!("member-{id:03}"),
                    if id % 3 == 0 {
                        "shared rare"
                    } else {
                        "shared common"
                    },
                )
            })
            .chain([row("large", &"x".repeat(MAX_DOCUMENT_BYTES + 1))])
            .collect::<Vec<_>>();
        let replacements = rows
            .iter()
            .take(65)
            .map(|old| row(&old.key, "replacement shared"))
            .chain([
                row("large", "now complete"),
                row("member-129", &"x".repeat(MAX_DOCUMENT_BYTES + 1)),
            ])
            .collect::<Vec<_>>();
        let snapshot = |connection: &Connection| {
            [
                LOGICAL_POSITIONS,
                PHYSICAL_COMPACT,
                "SELECT json_array(read_scope,session_key,gram,last_ms) FROM session_search_postings ORDER BY read_scope,session_key,gram",
                "SELECT json_array(read_scope,gram,members) FROM session_search_counts ORDER BY read_scope,gram",
                "SELECT json_array(read_scope,session_key,fields,complete) FROM session_search_documents ORDER BY read_scope,session_key",
                "SELECT json_array(read_scope,incomplete) FROM session_search_coverage ORDER BY read_scope",
            ]
            .map(|sql| {
                connection.prepare(sql).unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
            })
        };
        for cohort in [&rows, &replacements] {
            let before = snapshot(&batched);
            {
                let transaction = batched.transaction().unwrap();
                publish_current_rows(&transaction, &cohort.iter().collect::<Vec<_>>()).unwrap();
            }
            assert_eq!(snapshot(&batched), before, "failed publication rolls back");
            let transaction = reference.transaction().unwrap();
            for row in cohort {
                for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
                    publish_row(&transaction, scope, row).unwrap();
                }
            }
            transaction.commit().unwrap();
            let transaction = batched.transaction().unwrap();
            publish_current_rows(&transaction, &cohort.iter().collect::<Vec<_>>()).unwrap();
            transaction.commit().unwrap();
            assert_eq!(snapshot(&batched), snapshot(&reference));
        }
    }

    #[test]
    fn backfilled_member_counts_support_subsequent_updates_and_rollback() {
        let mut connection = database();
        let rows = [
            row("one", "shared rare"),
            row("two", "shared common"),
            row("three", "shared common"),
            row(
                "unicode",
                &(0x400..0x480)
                    .filter_map(char::from_u32)
                    .collect::<String>(),
            ),
            row("large", &"x".repeat(MAX_DOCUMENT_BYTES + 1)),
        ];
        let transaction = connection.transaction().unwrap();
        begin_backfill(&transaction).unwrap();
        for row in &rows {
            backfill_catalog_row(&transaction, row).unwrap();
        }
        let mut retained = row("retained", "shared rare");
        retained.lifecycle = CatalogLifecycle::Retained;
        backfill_catalog_row(&transaction, &retained).unwrap();
        finish_backfill(&transaction).unwrap();
        transaction.commit().unwrap();
        // The bulk insert's full batches and final partial batch must preserve
        // every Unicode gram and both scopes, including incomplete coverage.
        let mut incremental = database();
        let transaction = incremental.transaction().unwrap();
        for row in &rows {
            for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
                publish_row(&transaction, scope, row).unwrap();
            }
        }
        publish_row(&transaction, CatalogReadScope::RetainedHistory, &retained).unwrap();
        transaction.commit().unwrap();
        let mut batched = database();
        let transaction = batched.transaction().unwrap();
        publish_current_rows(&transaction, &rows.iter().collect::<Vec<_>>()).unwrap();
        publish_row(&transaction, CatalogReadScope::RetainedHistory, &retained).unwrap();
        transaction.commit().unwrap();
        for sql in [
            LOGICAL_POSITIONS,
            PHYSICAL_COMPACT,
            "SELECT json_array(read_scope,session_key,gram,last_ms) FROM session_search_postings ORDER BY read_scope,session_key,gram",
            "SELECT json_array(read_scope,gram,members) FROM session_search_counts ORDER BY read_scope,gram",
            "SELECT json_array(read_scope,session_key,fields,complete) FROM session_search_documents ORDER BY read_scope,session_key",
            "SELECT json_array(read_scope,incomplete) FROM session_search_coverage ORDER BY read_scope",
        ] {
            let read = |connection: &Connection| {
                connection
                    .prepare(sql)
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap()
            };
            assert_eq!(read(&connection), read(&incremental), "{sql}");
            assert_eq!(read(&connection), read(&batched), "bounded cohort: {sql}");
        }
        let count = |connection: &Connection, scope: &str, gram: &str| -> i64 {
            connection
                .query_row(
                    "SELECT members FROM session_search_counts WHERE read_scope=?1 AND gram=?2",
                    params![scope, gram],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(count(&connection, "current", "sha"), 3);
        assert_eq!(count(&connection, "retained_history", "rar"), 2);
        assert_eq!(count(&connection, "retained_history", "sha"), 4);
        let postings = |connection: &Connection| {
            connection
                .prepare("SELECT session_key FROM (SELECT last_ms,session_key FROM session_search_postings WHERE read_scope='current' AND gram='sha' UNION ALL SELECT current_last_ms,session_key FROM session_search_memberships WHERE gram='sha' AND current_last_ms IS NOT NULL) ORDER BY last_ms DESC,session_key ASC")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let original = postings(&connection);
        assert_eq!(original, ["one", "three", "two"]);
        {
            let transaction = connection.transaction().unwrap();
            begin_backfill(&transaction).unwrap();
            backfill_catalog_row(&transaction, &row("replacement", "other")).unwrap();
            // An interrupted publication must restore the old search and its
            // ordering index, including the exact aggregate member counts.
        }
        assert_eq!(postings(&connection), original);
        assert_eq!(count(&connection, "current", "sha"), 3);
        let ordering_index: String = connection
            .query_row(
                "SELECT name FROM sqlite_schema WHERE type='index' AND name='session_search_order'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ordering_index, "session_search_order");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM session_search_documents WHERE session_key='retained' AND read_scope='current'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        {
            let transaction = connection.transaction().unwrap();
            publish_row(
                &transaction,
                CatalogReadScope::Current,
                &row("two", "other"),
            )
            .unwrap();
        }
        assert_eq!(count(&connection, "current", "sha"), 3);
        let transaction = connection.transaction().unwrap();
        publish_row(
            &transaction,
            CatalogReadScope::Current,
            &row("two", "other"),
        )
        .unwrap();
        transaction.commit().unwrap();
        assert_eq!(count(&connection, "current", "sha"), 2);
        assert_eq!(count(&connection, "retained_history", "sha"), 4);
        assert_eq!(count(&connection, "current", "rar"), 1);
        assert_eq!(
            connection
                .query_row(
                    "SELECT incomplete FROM session_search_coverage WHERE read_scope='current'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn mixed_storage_scoped_changes_preserve_independent_history_and_counts() {
        let mut connection = database();
        let mut current = row("shared-key", "current needle");
        current.last = Some(0);
        let mut history = row("shared-key", "historical token");
        history.last = Some(-7);
        let transaction = connection.transaction().unwrap();
        publish_row(&transaction, CatalogReadScope::Current, &current).unwrap();
        publish_row(&transaction, CatalogReadScope::RetainedHistory, &history).unwrap();
        transaction.commit().unwrap();
        let original = strings(&connection, LOGICAL_POSITIONS);
        assert!(
            original
                .iter()
                .any(|value| value.contains("current") && value.ends_with(",0]"))
        );
        assert!(original.iter().any(|value| value.ends_with(",-7]")));
        legacy_key(&connection, "shared-key");
        assert_eq!(strings(&connection, LOGICAL_POSITIONS), original);
        let history_before = strings(
            &connection,
            "SELECT json_array(gram,last_ms) FROM session_search_postings WHERE read_scope='retained_history' ORDER BY gram",
        );
        let counts_before = strings(
            &connection,
            "SELECT json_array(gram,members) FROM session_search_counts WHERE read_scope='retained_history' ORDER BY gram",
        );
        let document_before = strings(
            &connection,
            "SELECT fields FROM session_search_documents WHERE read_scope='retained_history'",
        );
        let changed = row("shared-key", "replacement current");
        {
            let transaction = connection.transaction().unwrap();
            publish_row(&transaction, CatalogReadScope::Current, &changed).unwrap();
        }
        assert_eq!(strings(&connection, LOGICAL_POSITIONS), original);
        assert_eq!(
            representation(&connection, "shared-key").unwrap(),
            (true, false)
        );
        let transaction = connection.transaction().unwrap();
        publish_row(&transaction, CatalogReadScope::Current, &changed).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            representation(&connection, "shared-key").unwrap(),
            (false, true)
        );
        assert_eq!(
            strings(
                &connection,
                "SELECT json_array(gram,history_last_ms) FROM session_search_memberships WHERE history_last_ms IS NOT NULL ORDER BY gram"
            ),
            history_before
        );
        assert_eq!(
            strings(
                &connection,
                "SELECT json_array(gram,members) FROM session_search_counts WHERE read_scope='retained_history' ORDER BY gram"
            ),
            counts_before
        );
        assert_eq!(
            strings(
                &connection,
                "SELECT fields FROM session_search_documents WHERE read_scope='retained_history'"
            ),
            document_before
        );
        let transaction = connection.transaction().unwrap();
        remove_row(&transaction, "current", "shared-key").unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            strings(
                &connection,
                "SELECT json_array(gram,history_last_ms) FROM session_search_memberships ORDER BY gram"
            ),
            history_before
        );
        assert_eq!(connection.query_row("SELECT COUNT(*) FROM session_search_memberships WHERE current_last_ms IS NOT NULL", [], |row| row.get::<_,i64>(0)).unwrap(), 0);
        assert_eq!(
            strings(
                &connection,
                "SELECT json_array(gram,members) FROM session_search_counts WHERE read_scope='retained_history' ORDER BY gram"
            ),
            counts_before
        );
        let transaction = connection.transaction().unwrap();
        publish_current_rows(&transaction, &[&current]).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            strings(
                &connection,
                "SELECT json_array(gram,members) FROM session_search_counts WHERE read_scope='current' ORDER BY gram"
            ),
            strings(
                &connection,
                "SELECT json_array(gram,members) FROM session_search_counts WHERE read_scope='retained_history' ORDER BY gram"
            )
        );
        assert_eq!(connection.query_row("SELECT COUNT(*) FROM session_search_memberships WHERE current_last_ms IS NULL OR history_last_ms IS NULL OR current_last_ms!=history_last_ms", [], |row| row.get::<_,i64>(0)).unwrap(), 0);
    }

    #[test]
    fn overlap_and_oversized_old_scopes_refuse_without_deleting_history() {
        let mut connection = database();
        let selected = row("selected", "needle");
        let transaction = connection.transaction().unwrap();
        publish_current_rows(&transaction, &[&selected]).unwrap();
        transaction.commit().unwrap();
        connection
            .execute(
                "INSERT INTO session_search_postings VALUES('retained_history','selected','old',0)",
                [],
            )
            .unwrap();
        let before = strings(&connection, LOGICAL_POSITIONS);
        assert!(
            posting_page(
                &connection,
                CatalogReadScope::Current,
                "nee",
                None,
                MAX_CANDIDATES + 1
            )
            .is_err()
        );
        {
            let transaction = connection.transaction().unwrap();
            assert!(publish_current_rows(&transaction, &[&selected]).is_err());
        }
        assert_eq!(strings(&connection, LOGICAL_POSITIONS), before);
        connection
            .execute("DELETE FROM session_search_postings", [])
            .unwrap();
        legacy_key(&connection, "selected");
        connection
            .execute(
                "DELETE FROM session_search_postings WHERE read_scope='retained_history'",
                [],
            )
            .unwrap();
        for n in 0..=MAX_GRAMS {
            connection.execute("INSERT INTO session_search_postings VALUES('retained_history','selected',?1,0)", [format!("{n:04}")]).unwrap();
        }
        let before = strings(&connection, LOGICAL_POSITIONS);
        {
            let transaction = connection.transaction().unwrap();
            assert!(publish_current_rows(&transaction, &[&selected]).is_err());
            assert!(remove_row(&transaction, "current", "selected").is_err());
        }
        assert_eq!(strings(&connection, LOGICAL_POSITIONS), before);
        assert_eq!(
            representation(&connection, "selected").unwrap(),
            (true, false)
        );
    }

    #[test]
    fn oversized_compact_history_refuses_before_an_absent_current_scope_scan() {
        let mut connection = database();
        connection.execute("WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x+1 FROM n WHERE x<?1) INSERT INTO session_search_memberships SELECT 'selected',printf('%05d',x),NULL,0 FROM n",[(MAX_GRAMS*2) as i64]).unwrap();
        let before = strings(&connection, PHYSICAL_COMPACT);
        let selected = row("selected", "replacement");
        {
            let transaction = connection.transaction().unwrap();
            assert!(publish_current_rows(&transaction, &[&selected]).is_err());
            assert!(remove_row(&transaction, "current", "selected").is_err());
        }
        assert_eq!(
            strings(&connection, PHYSICAL_COMPACT),
            before,
            "absent Current must not permit an unbounded History scan or deletion"
        );
    }

    #[test]
    fn mixed_index_pages_share_one_candidate_budget_and_tie_cursor() {
        let mut connection = database();
        let transaction = connection.transaction().unwrap();
        for n in 0..520 {
            let mut selected = row(&format!("member-{n:04}"), "needle");
            selected.last = Some(if n < 510 { 10 } else { 9 });
            publish_current_rows(&transaction, &[&selected]).unwrap();
        }
        transaction.commit().unwrap();
        for n in (0..520).step_by(2) {
            legacy_key(&connection, &format!("member-{n:04}"));
        }
        for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
            let first = posting_page(&connection, scope, "nee", None, MAX_CANDIDATES + 1).unwrap();
            assert_eq!(first.len(), MAX_CANDIDATES + 1);
            assert_eq!(
                first.iter().map(|p| p.key.clone()).collect::<Vec<_>>(),
                (0..513)
                    .map(|n| format!("member-{n:04}"))
                    .collect::<Vec<_>>()
            );
            let last = posting_page(
                &connection,
                scope,
                "nee",
                Some(&first[MAX_CANDIDATES - 1]),
                MAX_CANDIDATES + 1,
            )
            .unwrap();
            assert_eq!(
                last.iter().map(|p| p.key.clone()).collect::<Vec<_>>(),
                (512..520)
                    .map(|n| format!("member-{n:04}"))
                    .collect::<Vec<_>>()
            );
            let plan = strings_plan(&connection, scope);
            assert!(
                plan.iter().any(|v| v.contains("MERGE (UNION ALL)")),
                "{plan:?}"
            );
            assert!(
                plan.iter()
                    .any(|v| v.contains("COVERING INDEX session_search_order")),
                "{plan:?}"
            );
            assert!(
                plan.iter()
                    .any(|v| v.contains(if scope == CatalogReadScope::Current {
                        "COVERING INDEX session_search_current_order"
                    } else {
                        "COVERING INDEX session_search_history_order"
                    })),
                "{plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|v| v.contains("SCAN ") || v.contains("TEMP B-TREE")),
                "{plan:?}"
            );
        }
        let selected_work = |connection: &Connection, scope| {
            SQL_STEPS.with(|value| value.set(0));
            let page = posting_page(connection, scope, "nee", None, 2).unwrap();
            (
                page.iter().map(|p| p.key.clone()).collect::<Vec<_>>(),
                SQL_STEPS.with(|value| value.get()),
            )
        };
        let before_current = selected_work(&connection, CatalogReadScope::Current);
        let before_history = selected_work(&connection, CatalogReadScope::RetainedHistory);
        connection.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO session_search_postings SELECT 'retained_history','unrelated-'||printf('%05d',x),'irr',1 FROM n;").unwrap();
        assert_eq!(
            selected_work(&connection, CatalogReadScope::Current),
            before_current
        );
        assert_eq!(
            selected_work(&connection, CatalogReadScope::RetainedHistory),
            before_history
        );
    }

    fn strings_plan(connection: &Connection, scope: CatalogReadScope) -> Vec<String> {
        connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                posting_sql(scope, "1", 3)
            ))
            .unwrap()
            .query_map(params![scope_key(scope), "nee", 2], |row| row.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
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
            model: None,
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
                model: None,
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
                model: None,
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
