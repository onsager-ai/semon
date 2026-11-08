//! Generation-bound, metadata-only activity pages from one selected Catalog.
use super::*;
use crate::{events::EventCache, model::summary::CatalogRow};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

fn unavailable(_: impl fmt::Display) -> QueryError {
    QueryError::new(
        "stale_projection",
        "Selected activity is unavailable or changed; resynchronize",
    )
}

/// A refusal carries its closed `reason` beside the stale-projection code.
fn refusal(reason: &'static str) -> QueryError {
    QueryError {
        reason: Some(reason),
        ..unavailable("")
    }
}

/// A continuation names a generation; a first page has none to be stale.
fn missing(expected: Option<&str>) -> QueryError {
    refusal(if expected.is_some() {
        "stale_generation"
    } else {
        "no_activity"
    })
}

pub(super) fn read(
    options: &Options,
    namespace: Option<&crate::storage::Namespace>,
    key: &str,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
) -> Result<Value, QueryError> {
    let requested = args["id"].as_str().expect("validated activity id");
    if requested != key {
        return Err(QueryError::unknown_session(requested));
    }
    let cursor = args["cursor"].as_str();
    let expected = args["generation"].as_str();
    if cursor.is_some() && expected.is_none() {
        return Err(QueryError::invalid(
            "An activity continuation requires generation",
        ));
    }
    let mut db = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let mut snapshot = db.transaction().map_err(unavailable)?;
    if let Some(namespace) = namespace {
        crate::storage::sqlite::qualify(&snapshot, namespace).map_err(unavailable)?;
    }
    crate::events::store::require_current(&snapshot).map_err(unavailable)?;
    let text = snapshot.query_row(
        "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_activity_sources WHERE session_key=?1",
        [key], |r| r.get::<_, Option<String>>(0),
    ).optional().map_err(unavailable)?.flatten().ok_or_else(|| QueryError::unknown_session(key))?;
    let row: CatalogRow = serde_json::from_str(&text).map_err(unavailable)?;
    if row.key != key
        || row.sources.is_empty()
        || row.sources.iter().any(|source| {
            crate::catalog::source_relative(options, &row.harness, source).is_none()
                || !crate::model::catalog_source_in_window(
                    source,
                    crate::model::scan_cutoff(options, now),
                )
        })
    {
        return Err(unavailable("Selected Catalog source mismatch"));
    }
    if !EventCache::selected_roles_current(&snapshot, &row.sources).map_err(unavailable)? {
        return Err(unavailable("Selected activity source roles changed"));
    }
    let saved = row
        .query
        .as_ref()
        .ok_or_else(|| unavailable("Missing query observation"))?;
    if !options.all && !options.scan_window {
        let cutoff =
            now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX));
        if !super::transcript::window_member(&snapshot, key, saved, cutoff)? {
            return Err(QueryError::unknown_session(key));
        }
    }
    let limit = integer(args, "limit").unwrap_or(100) as u32;
    let price = crate::pricing::snapshot();
    let kind = args["kind"].as_str().unwrap_or("steps");
    let facts = kind == "facts";
    if kind == "preview" && cursor.is_some() {
        return Err(QueryError::invalid("A preview has no continuation"));
    }
    if kind == "steps" && cursor.is_some_and(|cursor| cursor.parse::<u64>().is_err()) {
        return Err(QueryError::invalid("A step cursor must be an ordinal"));
    }
    let (generation, body) = if kind == "preview" {
        let (generation, preview) = crate::events::store::session_activity::preview(
            &snapshot,
            &row,
            expected,
            &price.fingerprint,
        )
        .map_err(unavailable)?
        .ok_or_else(|| missing(expected))?;
        // The preview's own fields sit beside the generation.
        let mut body = serde_json::to_value(preview).map_err(unavailable)?;
        body["kind"] = json!(kind);
        (generation, body)
    } else {
        let page = crate::events::store::session_activity::items(
            &snapshot,
            &row,
            expected,
            &price.fingerprint,
            facts,
            cursor,
            limit,
        )
        .map_err(unavailable)?
        .ok_or_else(|| missing(expected))?;
        (
            page.generation,
            json!({"kind": kind, "items": page.items, "next_cursor": page.next_cursor}),
        )
    };
    // Hand the pinned selection snapshot to the existing request-owned source
    // witness. No native body or provenance path is returned by this tool.
    snapshot.set_drop_behavior(rusqlite::DropBehavior::Ignore);
    drop(snapshot);
    let sources = crate::query_source::Sources::capture_focused_snapshot(options, &row.sources, db)
        .map_err(|error| QueryError::io(&error))?;
    let witness = sources.reader_with_mode(provider, mode);
    witness
        .verify_all()
        .map_err(|error| QueryError::io(&error))?;
    witness.finish().map_err(|error| QueryError::io(&error))?;
    let mut current = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(unavailable)?;
    current
        .busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let current = current.transaction().map_err(unavailable)?;
    if let Some(namespace) = namespace {
        crate::storage::sqlite::qualify(&current, namespace).map_err(unavailable)?;
    }
    let current_text = current
        .query_row(
            "SELECT metadata FROM session_activity_sources WHERE session_key=?1",
            [key],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(unavailable)?;
    let current_generation: Option<String> = current
        .query_row(
            "SELECT generation FROM session_activity_projections WHERE session_key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if current_text.as_deref() != Some(&text)
        || current_generation.as_deref() != Some(&generation)
    {
        return Err(refusal("stale_generation"));
    }
    crate::selected_read::check().map_err(|error| selected_error(&error))?;
    let mut answer = json!({
        "now": now,
        "session": {"id": key, "harness": row.harness, "native_ids": row.native_ids},
        "generation": generation,
    });
    answer
        .as_object_mut()
        .expect("activity answer")
        .extend(body.as_object().expect("activity body").clone());
    Ok(answer)
}
