//! Selected immutable slot/range reads, independent of the global event model.
use crate::{
    CatalogIdentityError, Options, SessionSourceReader, SessionSourceRef,
    catalog::{self, CatalogSessionIdentity},
    events::EventCache,
    model::summary::CatalogSource,
    slot_projection::{SavedSlot, VERSION},
    viewer::{ViewerReply, decoded, query_value},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const MAX_LIMIT: usize = 100;
const PAGE_SOURCE_BYTES: usize = 1024 * 1024;
const PAGE_SOURCE_REQUESTS: usize = 256;
const RECORD_BYTES: usize = 128 * 1024;
const READ_CHUNK: usize = 4096;

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_LOCAL_READ: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static AFTER_SELECTION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

fn reply(status: u16, value: Value) -> ViewerReply {
    ViewerReply {
        status,
        content_type: "application/json; charset=utf-8",
        body: value.to_string().into_bytes(),
        etag: None,
    }
}
fn unavailable() -> ViewerReply {
    catalog::error(
        503,
        "transcript_unavailable",
        "The selected transcript projection is unavailable. Retry after observation completes.",
        true,
    )
}

/// Read one selected transcript window. The host chooses authorized options
/// and source key. A provider reader is borrowed for this request only; no
/// authority is retained in a ViewerCore, projection or workspace cache.
/// Large native records currently remain explicitly incomplete; no global
/// model/full selected source fallback occurs. Field spans will qualify those
/// previews and expansions independently of native record length.
pub fn session_transcript_range(
    options: &Options,
    source_key: &str,
    query: &str,
    reader: Option<&dyn SessionSourceReader>,
) -> ViewerReply {
    session_transcript_range_with_mode(
        options,
        source_key,
        query,
        reader,
        SessionSourceReadMode::ProviderOnly,
    )
}

/// Provider-only preserves the callback boundary for every requested range.
/// LocalThenProvider and LocalOrPinnedProvider are explicit host opt-ins for
/// mixed live/archived sources.
/// The host must authorize the complete read before and after this call, because
/// its provider callback is not invoked for available qualified local bytes.
#[derive(Clone, Copy, Debug, Default)]
pub enum SessionSourceReadMode {
    #[default]
    ProviderOnly,
    LocalThenProvider,
    /// Qualify a changed local carrier with the exact ledger's persisted block
    /// proofs, or admit the exact selected provider when that qualification fails
    /// before any bytes are read. The provider must independently qualify the
    /// original consumed prefix; unqualified local bytes are never exposed.
    /// Changes during local I/O still fail closed without provider fallback.
    LocalOrPinnedProvider,
}

pub fn session_transcript_range_with_mode(
    options: &Options,
    source_key: &str,
    query: &str,
    reader: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
) -> ViewerReply {
    match read(options, source_key, query, reader, mode) {
        Ok(reply) => reply,
        Err(_) => unavailable(),
    }
}

/// Locate a recorded turn using only its source-qualified slot projection.
/// Hosts authorize this metadata query before and after the read. Consumers
/// pass the returned projection generation into the subsequent range read so
/// a rewrite cannot reinterpret the recorded slot address.
pub fn session_turn_anchor(options: &Options, source_key: &str, query: &str) -> ViewerReply {
    let read = || -> Result<ViewerReply, Box<dyn std::error::Error>> {
        let Some(key) = query_value(query, "sid")
            .and_then(decoded)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
        else {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Supply one session key and turn.",
                false,
            ));
        };
        let Some(turn) = query_value(query, "turn")
            .and_then(decoded)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
        else {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Supply one session key and turn.",
                false,
            ));
        };
        if query_value(query, "scope").is_some_and(|value| value != "retained_history") {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Turn anchors require retained history.",
                false,
            ));
        }
        for name in ["sid", "turn", "scope", "machine"] {
            if query
                .split('&')
                .filter(|part| part.split('=').next() == Some(name))
                .count()
                > 1
            {
                return Ok(catalog::error(
                    400,
                    "invalid_arguments",
                    "Duplicate anchor argument.",
                    false,
                ));
            }
        }
        let mut db = Connection::open_with_flags(
            EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.busy_timeout(Duration::from_millis(100))?;
        let tx = db.transaction()?;
        let identity = match catalog::catalog_selection_in_snapshot(
            options,
            source_key,
            &key,
            crate::CatalogReadScope::RetainedHistory,
            &tx,
        ) {
            Ok(Some((identity, _))) => identity,
            Ok(None) => {
                return Ok(catalog::error(
                    404,
                    "session_not_found",
                    "Recorded session unavailable.",
                    false,
                ));
            }
            Err(_) => return Ok(unavailable()),
        };
        let generation: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='slot_projection_catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if generation.as_deref() != Some(identity.generation.as_str()) {
            return Ok(stale(ProjectionFence::CatalogPublication));
        }
        let history = crate::history_projection::catalog(&tx, &key)?.is_some();
        let (table, header) = if history {
            ("session_history_slots", "session_history_projections")
        } else {
            ("session_slots", "session_slot_projections")
        };
        let projection: Option<(String, u32)> = tx
            .query_row(
                &format!("SELECT generation,version FROM {header} WHERE session_key=?1"),
                [&key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((projection, VERSION)) = projection else {
            return Ok(unavailable());
        };
        let projection = entry_generation(&projection);
        let slot:Option<i64>=tx.query_row(&format!("SELECT slot FROM {table} WHERE session_key=?1 AND json_extract(metadata,'$.turn')=?2 ORDER BY slot LIMIT 1"),rusqlite::params![key,turn],|row|row.get(0)).optional()?;
        tx.commit()?;
        Ok(reply(
            200,
            json!({"api":1,"source_key":source_key,"catalog_key":key,"turn":turn,"slot":slot,"generation":projection}),
        ))
    };
    read().unwrap_or_else(|_| unavailable())
}

/// Indexed annotation addresses for one immutable selected projection. No native
/// bodies are opened. Each page holds at most 100 identities, even when failures
/// are spread throughout a large transcript.
pub fn session_annotations(options: &Options, source_key: &str, query: &str) -> ViewerReply {
    let read = || -> Result<ViewerReply, Box<dyn std::error::Error>> {
        let argument = |name| query_value(query, name).and_then(decoded);
        let invalid_arguments = || {
            catalog::error(
                400,
                "invalid_arguments",
                "Use one selected session, retained scope, errors or signals, and limit=1..100.",
                false,
            )
        };
        for name in [
            "sid",
            "machine",
            "scope",
            "kind",
            "after",
            "limit",
            "generation",
        ] {
            if query
                .split('&')
                .filter(|part| part.split('=').next() == Some(name))
                .count()
                > 1
                || query_value(query, name).is_some_and(|value| decoded(value).is_none())
            {
                return Ok(invalid_arguments());
            }
        }
        let Some(key) = argument("sid").filter(|key| !key.is_empty() && key.len() <= 4096) else {
            return Ok(invalid_arguments());
        };
        if argument("scope").as_deref() != Some("retained_history") {
            return Ok(invalid_arguments());
        }
        let kind = argument("kind").unwrap_or_default();
        if !matches!(kind.as_str(), "errors" | "signals") {
            return Ok(invalid_arguments());
        }
        let limit = argument("limit")
            .map(|value| value.parse::<usize>())
            .transpose();
        let after = argument("after")
            .map(|value| value.parse::<usize>())
            .transpose();
        let (Ok(limit), Ok(after)) = (limit, after) else {
            return Ok(invalid_arguments());
        };
        let limit = limit.unwrap_or(100);
        let after = after.unwrap_or(0);
        if !(1..=MAX_LIMIT).contains(&limit) || (after > 0 && argument("generation").is_none()) {
            return Ok(invalid_arguments());
        }
        let mut db = Connection::open_with_flags(
            EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.busy_timeout(Duration::from_millis(100))?;
        let tx = db.transaction()?;
        let (identity, session) = match catalog::catalog_selection_in_snapshot(
            options,
            source_key,
            &key,
            crate::CatalogReadScope::RetainedHistory,
            &tx,
        ) {
            Ok(Some(selected)) => selected,
            Ok(None) => {
                return Ok(catalog::error(
                    404,
                    "session_not_found",
                    "Recorded session unavailable.",
                    false,
                ));
            }
            Err(_) => return Ok(unavailable()),
        };
        let catalog_generation: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key='slot_projection_catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if catalog_generation.as_deref() != Some(identity.generation.as_str()) {
            return Ok(stale(ProjectionFence::CatalogPublication));
        }
        let history = crate::history_projection::catalog(&tx, &key)?.is_some();
        let header = if history {
            "session_history_projections"
        } else {
            "session_slot_projections"
        };
        let slots = if history {
            "session_history_slots"
        } else {
            "session_slots"
        };
        let projection: Option<(String, u32)> = tx
            .query_row(
                &format!("SELECT generation,version FROM {header} WHERE session_key=?1"),
                [&key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((stored, version)) = projection else {
            return Ok(unavailable());
        };
        if version != VERSION {
            return Ok(unavailable());
        }
        let working = selected_working(&session);
        let predicate = if kind == "signals" {
            "json_extract(metadata,'$.recipe.recipe')='Signal'"
        } else if working {
            "((json_extract(metadata,'$.recipe.recipe') IN ('Tool','Yielded','Sent') AND json_extract(metadata,'$.recipe.outcome')='Err') OR (json_extract(metadata,'$.recipe.recipe')='Operation' AND json_extract(metadata,'$.recipe.ok')=0))"
        } else {
            "((json_extract(metadata,'$.recipe.recipe') IN ('Tool','Yielded','Sent') AND json_extract(metadata,'$.recipe.outcome') IN ('Err','Pending')) OR (json_extract(metadata,'$.recipe.recipe')='Operation' AND json_extract(metadata,'$.recipe.ok')=0))"
        };
        // Error membership includes pending calls only when the same qualified
        // observation renders them unfinished. Fence paging when that changes,
        // even if the immutable source/slot generation did not move.
        let generation = if kind == "errors" {
            error_generation(&entry_generation(&stored), &session)
        } else {
            entry_generation(&stored)
        };
        if argument("generation").is_some_and(|expected| expected != generation) {
            return Ok(stale(if kind == "errors" {
                ProjectionFence::ErrorsGeneration
            } else {
                ProjectionFence::ProjectionGeneration
            }));
        }
        let total: i64 = tx.query_row(
            &format!("SELECT count(*) FROM {slots} WHERE session_key=?1 AND {predicate}"),
            [&key],
            |row| row.get(0),
        )?;
        let total = usize::try_from(total)?;
        if after > total {
            return Ok(invalid_arguments());
        }
        let mut statement = tx.prepare(&format!("SELECT slot FROM {slots} WHERE session_key=?1 AND {predicate} ORDER BY slot LIMIT ?2 OFFSET ?3"))?;
        let entries: Vec<Value> = statement
            .query_map(
                rusqlite::params![key, i64::try_from(limit)?, i64::try_from(after)?],
                |row| Ok(json!({"slot":row.get::<_,i64>(0)?})),
            )?
            .collect::<rusqlite::Result<_>>()?;
        drop(statement);
        tx.commit()?;
        let end = after + entries.len();
        Ok(reply(
            200,
            json!({"api":1,"source_key":source_key,"catalog_key":key,"scope":"retained_history","kind":kind,"generation":generation,"total":total,"first":after,"entries":entries,"next_cursor":if end<total {Some(end)}else{None}}),
        ))
    };
    read().unwrap_or_else(|_| unavailable())
}

/// A persisted pending recipe never grants live authority. Both transcript
/// presentation and error membership use this qualified selected observation.
pub(crate) fn selected_working(session: &Value) -> bool {
    session["lifecycle"] == "current"
        && session["details"]["live_observation"] == true
        && matches!(session["details"]["state"].as_str(), Some("work" | "wait"))
}

/// Source-slot identity and qualified pending membership fence error paging.
pub(crate) fn error_generation(projection: &str, session: &Value) -> String {
    use sha2::{Digest, Sha256};
    let working = selected_working(session);
    let activity = if working {
        json!([
            session["details"]["activity"]["tool"],
            session["details"]["activity"]["since"]
        ])
    } else {
        Value::Null
    };
    format!(
        "{:x}",
        Sha256::digest(format!("native-errors-v1|{projection}|{working}|{activity}").as_bytes())
    )
}

fn entry_generation(stored: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(format!("signal-entry-identity-v2|{stored}").as_bytes())
    )
}

/// An exact selected catalog row may carry this body-free observation. It is
/// coherent with the catalog transaction and grants no provider/control access.
pub(crate) fn observation(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    catalog_generation: &str,
    history: bool,
) -> rusqlite::Result<Option<Value>> {
    let published: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='slot_projection_catalog_generation'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if published.as_deref() != Some(catalog_generation) {
        return Ok(None);
    }
    let history = history && crate::history_projection::catalog(tx, key)?.is_some();
    let table = if history {
        "session_history_projections"
    } else {
        "session_slot_projections"
    };
    let projection: Option<(String, u32, i64)> = tx
        .query_row(
            &format!("SELECT generation,version,total FROM {table} WHERE session_key=?1"),
            [key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(projection.filter(|(_, version, total)| *version == VERSION && (0..=9_007_199_254_740_991).contains(total)).map(|(generation, _, total)| json!({"generation":entry_generation(&generation),"total":total})))
}

struct Selection {
    identity: CatalogSessionIdentity,
    session: Value,
    generation: String,
    total: usize,
    first: usize,
    end: usize,
    start: usize,
    slots: Vec<SavedSlot>,
    sources: BTreeMap<PathBuf, CatalogSource>,
    history_incomplete: bool,
    context: Value,
}

enum SelectedPart {
    Transcript,
    Script,
    Output,
    Attachment(usize),
}

fn read(
    options: &Options,
    source_key: &str,
    query: &str,
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
) -> Result<ViewerReply, Box<dyn std::error::Error>> {
    for name in [
        "sid",
        "limit",
        "after",
        "generation",
        "errors_generation",
        "scope",
        "field_chunk",
        "part",
        "image",
    ] {
        if query_value(query, name).is_some_and(|value| decoded(value).is_none()) {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Invalid query encoding.",
                false,
            ));
        }
    }
    let argument = |name| query_value(query, name).and_then(decoded);
    let history = match argument("scope").as_deref() {
        None | Some("current") => false,
        Some("retained_history") => true,
        _ => {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Unknown read scope.",
                false,
            ));
        }
    };
    let Some(key) = argument("sid") else {
        return Ok(catalog::error(
            400,
            "invalid_arguments",
            "A canonical sid is required.",
            false,
        ));
    };
    if source_key.is_empty() || source_key.len() > 4096 || key.is_empty() || key.len() > 4096 {
        return Ok(catalog::error(
            400,
            "invalid_arguments",
            "Invalid selected identity.",
            false,
        ));
    }
    let limit = argument("limit")
        .map(|value| value.parse::<usize>())
        .transpose();
    let after = argument("after")
        .map(|value| value.parse::<usize>())
        .transpose();
    let (Ok(limit), Ok(after)) = (limit, after) else {
        return Ok(catalog::error(
            400,
            "invalid_arguments",
            "Use limit=1..100 and a non-negative slot address.",
            false,
        ));
    };
    let limit = limit.unwrap_or(60);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Ok(catalog::error(
            400,
            "invalid_arguments",
            "Use limit=1..100.",
            false,
        ));
    }
    let mut connection = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(100))?;
    let transaction = connection.transaction()?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::run(options, "transcript_before_snapshot");
    let selected = catalog::catalog_selection_in_snapshot(
        options,
        source_key,
        &key,
        if history {
            crate::CatalogReadScope::RetainedHistory
        } else {
            crate::CatalogReadScope::Current
        },
        &transaction,
    );
    let (identity, session) = match selected {
        Ok(Some(selected)) => selected,
        Ok(None) => {
            return Ok(catalog::error(
                404,
                "session_not_found",
                "The selected session is absent from this catalog.",
                false,
            ));
        }
        Err(CatalogIdentityError::InvalidArguments) => {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Invalid selected identity.",
                false,
            ));
        }
        Err(CatalogIdentityError::ScopeChanged) => {
            return Ok(catalog::error(
                503,
                "catalog_scope_changed",
                "The configured native source authority changed.",
                true,
            ));
        }
        Err(CatalogIdentityError::Unavailable) => return Ok(unavailable()),
    };
    #[cfg(test)]
    if let Some(hook) = AFTER_SELECTION.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let catalog_generation: Option<String> = transaction
        .query_row(
            "SELECT value FROM meta WHERE key='slot_projection_catalog_generation'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if catalog_generation.is_none() {
        return Ok(unavailable());
    }
    if catalog_generation.as_deref() != Some(identity.generation.as_str()) {
        return Ok(stale(ProjectionFence::CatalogPublication));
    }
    let history_projection =
        history && crate::history_projection::catalog(&transaction, &key)?.is_some();
    let history_incomplete = history_projection
        && transaction.query_row(
            "SELECT incomplete FROM session_history_projections WHERE session_key=?1",
            [&key],
            |row| row.get::<_, bool>(0),
        )?;
    let header_table = if history_projection {
        "session_history_projections"
    } else {
        "session_slot_projections"
    };
    let slot_table = if history_projection {
        "session_history_slots"
    } else {
        "session_slots"
    };
    let projection: Option<(String, u32, i64)> = transaction
        .query_row(
            &format!("SELECT generation,version,total FROM {header_table} WHERE session_key=?1"),
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((stored_generation, version, total)) = projection else {
        return Ok(unavailable());
    };
    let total = usize::try_from(total)?;
    if version != VERSION {
        return Ok(unavailable());
    }
    // Entry identity distinguishes signals expanded from one native record.
    // Fence pre-fix loaded ranges without rewriting retained source recipes.
    let generation = entry_generation(&stored_generation);
    if argument("generation").is_some_and(|expected| expected != generation) {
        return Ok(stale(ProjectionFence::ProjectionGeneration));
    }
    if argument("errors_generation")
        .is_some_and(|expected| expected != error_generation(&generation, &session))
    {
        return Ok(stale(ProjectionFence::ErrorsGeneration));
    }
    let first = after.unwrap_or_else(|| total.saturating_sub(limit));
    if first > total {
        return Ok(catalog::error(
            400,
            "invalid_range",
            "The slot address exceeds this projection.",
            false,
        ));
    }
    let end = first.saturating_add(limit).min(total);
    let start = first.saturating_sub(1);
    let mut statement = transaction.prepare(
        &format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM {slot_table} WHERE session_key=?1 AND slot>=?2 AND slot<?3 ORDER BY slot"))?;
    let texts: Vec<String> = statement
        .query_map(
            rusqlite::params![
                key,
                i64::try_from(start)?,
                i64::try_from(end.saturating_add(1).min(total))?
            ],
            |row| row.get(0),
        )?
        .collect::<Result<_, _>>()?;
    let slots = texts
        .into_iter()
        .map(|text| serde_json::from_str::<SavedSlot>(&text))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if slots.len() != end.saturating_add(1).min(total) - start {
        return Ok(unavailable());
    }
    let row_text: String = if history_projection {
        crate::history_projection::catalog(&transaction, &key)?.ok_or_else(invalid)?
    } else {
        transaction.query_row(
            "SELECT metadata FROM session_catalog WHERE session_key=?1",
            [&key],
            |row| row.get(0),
        )?
    };
    let row: crate::model::summary::CatalogRow = serde_json::from_str(&row_text)?;
    let context = crate::causal_trace::range_context(
        &transaction,
        &key,
        &slots[first - start..end - start],
        &row.sources,
    )?;
    let sources = row
        .sources
        .into_iter()
        .map(|source| (source.path.clone(), source))
        .collect();
    transaction.commit()?;
    let selection = Selection {
        identity,
        session,
        generation,
        total,
        first,
        end,
        start,
        slots,
        sources,
        history_incomplete,
        context,
    };
    let attachment = if argument("part").as_deref() == Some("attachment") {
        match argument("image")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value < 64)
        {
            Some(index) => Some(index),
            None => {
                return Ok(catalog::error(
                    400,
                    "invalid_arguments",
                    "A recorded image ordinal is required.",
                    false,
                ));
            }
        }
    } else {
        None
    };
    let part = match argument("part").as_deref() {
        None => SelectedPart::Transcript,
        Some("script" | "output" | "attachment")
            if argument("generation").is_some()
                && argument("after").is_some()
                && selection.end == selection.first + 1
                && argument("field_chunk").is_none() =>
        {
            match (argument("part").as_deref(), attachment) {
                (_, Some(index)) => SelectedPart::Attachment(index),
                (Some("output"), _) => SelectedPart::Output,
                _ => SelectedPart::Script,
            }
        }
        _ => {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Selected part reads require generation, after and limit=1.",
                false,
            ));
        }
    };
    if let Some(chunk) = argument("field_chunk") {
        let Ok(chunk) = chunk.parse::<usize>() else {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Invalid field chunk.",
                false,
            ));
        };
        if argument("generation").is_none() || selection.end != selection.first + 1 {
            return Ok(catalog::error(
                400,
                "invalid_arguments",
                "Field reads require generation, after and limit=1.",
                false,
            ));
        }
        return Ok(render_field(options, selection, provider, chunk, mode));
    }
    Ok(render(options, selection, provider, mode, part))
}

// Closed, body-free diagnostics let a host distinguish metadata/cursor races
// from rejected native bytes without exposing paths or provider errors.
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ProjectionFence {
    CatalogPublication,
    ProjectionGeneration,
    ErrorsGeneration,
    LocalSourceRevision(crate::source_carrier::RevisionReason),
    SourceGeneration,
    SourceRange,
}

fn stale(reason: ProjectionFence) -> ViewerReply {
    let mut reply = catalog::error(
        409,
        "stale_projection",
        "The selected projection changed. Resynchronize this session while retaining native entry identities.",
        true,
    );
    let mut body: Value = serde_json::from_slice(&reply.body).expect("owned JSON reply");
    body["resynchronize"] = json!(true);
    if let ProjectionFence::LocalSourceRevision(detail) = reason {
        body["fence_reason"] = json!("local_source_revision");
        body["local_revision_reason"] = json!(detail);
    } else {
        body["fence_reason"] = json!(reason);
    }
    reply.body = body.to_string().into_bytes();
    reply
}

#[derive(Debug)]
struct MissingLocalSource;
impl std::fmt::Display for MissingLocalSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("selected local source is absent")
    }
}
impl std::error::Error for MissingLocalSource {}

#[derive(Debug)]
struct LocalRevisionChanged(crate::source_carrier::RevisionReason, bool);
impl std::fmt::Display for LocalRevisionChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("selected local revision changed before reading")
    }
}
impl std::error::Error for LocalRevisionChanged {}

struct Composite<'a> {
    local: &'a Local<'a>,
    provider: Option<&'a dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
    // Request-local carrier choice. Once local bytes were accepted, a later
    // revision failure cannot switch that source to another carrier.
    routes: std::sync::Mutex<BTreeMap<(String, String), SourceCarrier>>,
}

#[derive(Clone, Copy)]
enum SourceCarrier {
    Local,
    Provider,
}
impl SessionSourceReader for Composite<'_> {
    fn read_range(
        &self,
        source: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        if matches!(self.mode, SessionSourceReadMode::ProviderOnly) {
            return self
                .provider
                .unwrap_or(self.local)
                .read_range(source, expected, offset, max);
        }
        if matches!(self.mode, SessionSourceReadMode::LocalOrPinnedProvider) {
            let mut routes = self.routes.lock().map_err(|_| invalid())?;
            let key = (source.root.clone(), source.path.clone());
            if let Some(carrier) = routes.get(&key).copied() {
                drop(routes);
                return match carrier {
                    SourceCarrier::Provider => self
                        .provider
                        .ok_or_else(invalid)?
                        .read_range(source, expected, offset, max),
                    SourceCarrier::Local => self.local.read_range(source, expected, offset, max),
                };
            }
            if routes.len() >= 64 {
                return Err(invalid());
            }
            // Serialize only the first bounded local read. No guard crosses
            // provider I/O, and each physical source chooses independently.
            let result = self.local.read_range(source, expected, offset, max);
            match result {
                Ok(part) => {
                    routes.insert(key, SourceCarrier::Local);
                    return Ok(part);
                }
                Err(error)
                    if self.provider.is_some()
                        && error.get_ref().is_some_and(|error| {
                            error.is::<MissingLocalSource>()
                                || error
                                    .downcast_ref::<LocalRevisionChanged>()
                                    .is_some_and(|error| error.1)
                        }) =>
                {
                    routes.insert(key, SourceCarrier::Provider);
                    drop(routes);
                    return self
                        .provider
                        .ok_or(error)?
                        .read_range(source, expected, offset, max);
                }
                Err(error) => return Err(error),
            }
        }
        match self.local.read_range(source, expected, offset, max) {
            Err(error)
                if error
                    .get_ref()
                    .is_some_and(|error| error.is::<MissingLocalSource>()) =>
            {
                self.provider
                    .ok_or(error)?
                    .read_range(source, expected, offset, max)
            }
            result => result,
        }
    }
}

type LocalBlock = (PathBuf, usize, Arc<[u8]>);

struct Local<'a> {
    options: &'a Options,
    sources: &'a BTreeMap<PathBuf, CatalogSource>,
    references: &'a BTreeMap<PathBuf, SessionSourceRef>,
    mode: SessionSourceReadMode,
    observed: std::sync::Mutex<BTreeMap<PathBuf, crate::events::Stat>>,
    proofs: std::sync::Mutex<BTreeMap<PathBuf, crate::events::store::chunk_proofs::Focused>>,
    block: std::sync::Mutex<Option<LocalBlock>>,
    bytes_left: std::sync::Mutex<usize>,
}
impl SessionSourceReader for Local<'_> {
    fn read_range(
        &self,
        reference: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        use crate::events::Stat;
        use crate::source_carrier::{RevisionReason, ledger, mismatch, recheck};
        let path = self
            .references
            .iter()
            .find_map(|(path, known)| (known == reference).then_some(path))
            .ok_or_else(invalid)?;
        let source = self.sources.get(path).ok_or_else(invalid)?;
        let ledger = ledger(source);
        let mut file = crate::sealed::LogFile::open(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::Error::new(io::ErrorKind::NotFound, MissingLocalSource)
            } else {
                error
            }
        })?;
        let generation: String = source
            .prefix_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if expected.is_some_and(|expected| expected != generation)
            || max == 0
            || max > crate::SESSION_SOURCE_CHUNK_MAX
            || offset > reference.offset
            || max as u64 > reference.offset - offset
        {
            return Err(invalid());
        }
        let before = Stat::from_metadata(&file.metadata()?)?;
        let reason = mismatch(&ledger.stat, &before);
        let rejected = |reason| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                LocalRevisionChanged(reason, true),
            )
        };
        let mut observed = self.observed.lock().map_err(|_| invalid())?;
        if let Some(prior) = observed.get(path)
            && prior != &before
        {
            return Err(rejected(
                mismatch(prior, &before).unwrap_or(RevisionReason::MissingChangeToken),
            ));
        }
        if before.size < reference.offset {
            return Err(rejected(RevisionReason::SizeShrank));
        }
        let bytes = if let Some(reason) = reason {
            if !matches!(self.mode, SessionSourceReadMode::LocalOrPinnedProvider) {
                return Err(rejected(reason));
            }
            // Proof headers bind the original full ledger, including prefix,
            // consumed boundary and physical witness. Only covering blocks are
            // opened and hashed; no complete-prefix resume or parser replay.
            let mut proofs = self.proofs.lock().map_err(|_| invalid())?;
            if let std::collections::btree_map::Entry::Vacant(entry) = proofs.entry(path.clone()) {
                let proof = crate::events::store::chunk_proofs::Focused::open(
                    &EventCache::path(&self.options.cache),
                    path.to_str().ok_or_else(invalid)?,
                    &ledger,
                )
                .map_err(|_| rejected(reason))?;
                entry.insert(proof);
            }
            let proof = proofs.get(path).ok_or_else(invalid)?;
            let end = offset + max as u64;
            let first = offset / crate::source_chunks::SIZE as u64;
            let last = (end - 1) / crate::source_chunks::SIZE as u64;
            let mut bytes = Vec::with_capacity(max);
            for index in first..=last {
                let index_usize = usize::try_from(index).map_err(|_| invalid())?;
                let mut cached = self.block.lock().map_err(|_| invalid())?;
                let part = if let Some((_, _, bytes)) =
                    cached.as_ref().filter(|(known, block, _)| {
                        before.changed_ns.is_some() && known == path && *block == index_usize
                    }) {
                    bytes.clone()
                } else {
                    let start = index * crate::source_chunks::SIZE as u64;
                    let charge =
                        (reference.offset - start).min(crate::source_chunks::SIZE as u64) as usize;
                    let mut remaining = self.bytes_left.lock().map_err(|_| invalid())?;
                    *remaining = remaining.checked_sub(charge).ok_or_else(invalid)?;
                    let bytes: Arc<[u8]> = proof
                        .read(&mut file, index_usize)
                        .map_err(|_| rejected(reason))?
                        .into();
                    *cached = Some((path.clone(), index_usize, bytes.clone()));
                    bytes
                };
                let start = index * crate::source_chunks::SIZE as u64;
                let lo = offset.saturating_sub(start) as usize;
                let hi = (end - start).min(part.len() as u64) as usize;
                bytes.extend_from_slice(&part[lo..hi]);
            }
            bytes
        } else {
            let mut remaining = self.bytes_left.lock().map_err(|_| invalid())?;
            *remaining = remaining.checked_sub(max).ok_or_else(invalid)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; max];
            file.read_exact(&mut bytes)?;
            bytes
        };
        #[cfg(test)]
        AFTER_LOCAL_READ.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::run(self.options, "local_source_after_read");
        recheck(path, &file, &before).map_err(|_| {
            let after = file
                .metadata()
                .ok()
                .and_then(|metadata| Stat::from_metadata(&metadata).ok());
            io::Error::new(
                io::ErrorKind::InvalidData,
                LocalRevisionChanged(
                    after
                        .as_ref()
                        .and_then(|after| mismatch(&before, after))
                        .unwrap_or(RevisionReason::Identity),
                    false,
                ),
            )
        })?;
        observed.insert(path.clone(), before);
        Ok(crate::SessionSourceRange {
            generation,
            length: if matches!(self.mode, SessionSourceReadMode::LocalOrPinnedProvider) {
                reference.offset
            } else {
                source.size
            },
            offset,
            bytes: Arc::from(bytes),
            cached: true,
        })
    }
}
/// Share the selected reader's local-generation validation and explicit provider
/// fallback with bounded native presentation reads. No model or body cache.
pub(crate) fn with_source_reader<T>(
    options: &Options,
    harness: &str,
    sources: &[CatalogSource],
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
    read: impl FnOnce(&dyn SessionSourceReader, &BTreeMap<PathBuf, SessionSourceRef>) -> io::Result<T>,
) -> io::Result<T> {
    if sources.is_empty() || sources.len() > 64 {
        return Err(invalid());
    }
    let home = match harness {
        "claude" => &options.claude_home,
        "codex" => &options.codex_home,
        "copilot" => &options.copilot_home,
        _ => return Err(invalid()),
    };
    let sources: BTreeMap<_, _> = sources
        .iter()
        .map(|source| (source.path.clone(), source.clone()))
        .collect();
    let mut references = BTreeMap::new();
    for (path, source) in &sources {
        let relative = path
            .strip_prefix(home)
            .map_err(|_| invalid())?
            .to_str()
            .ok_or_else(invalid)?;
        if !crate::is_input_path(harness, relative) {
            return Err(invalid());
        }
        references.insert(
            path.clone(),
            SessionSourceRef {
                root: harness.into(),
                path: relative.into(),
                native_id: source.native_id.clone(),
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
            },
        );
    }
    let local = Local {
        options,
        mode,
        observed: std::sync::Mutex::default(),
        proofs: std::sync::Mutex::default(),
        block: std::sync::Mutex::default(),
        bytes_left: std::sync::Mutex::new(64 * 1024 * 1024),
        sources: &sources,
        references: &references,
    };
    let composite = Composite {
        local: &local,
        provider,
        mode,
        routes: std::sync::Mutex::default(),
    };
    read(&composite, &references)
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "native source generation or range unavailable",
    )
}

fn render(
    options: &Options,
    selection: Selection,
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
    part: SelectedPart,
) -> ViewerReply {
    let attachment = match part {
        SelectedPart::Attachment(index) => Some(index),
        _ => None,
    };
    let references: BTreeMap<PathBuf, SessionSourceRef> = selection
        .identity
        .source_refs
        .iter()
        .filter_map(|observation| {
            let reference = &observation.source;
            let root = match reference.root.as_str() {
                "claude" => &options.claude_home,
                "codex" => &options.codex_home,
                "copilot" => &options.copilot_home,
                _ => return None,
            };
            Some((root.join(&reference.path), reference.clone()))
        })
        .collect();
    let local = Local {
        options,
        mode,
        observed: std::sync::Mutex::default(),
        proofs: std::sync::Mutex::default(),
        block: std::sync::Mutex::default(),
        bytes_left: std::sync::Mutex::new(if attachment.is_some() {
            12 * 1024 * 1024
        } else {
            PAGE_SOURCE_BYTES
        }),
        sources: &selection.sources,
        references: &references,
    };
    let composite = Composite {
        local: &local,
        provider,
        mode,
        routes: std::sync::Mutex::default(),
    };
    let reader: &dyn SessionSourceReader = &composite;
    // Presentation deliberately tolerates malformed records and unavailable
    // archive bytes. A rejected source generation is different: never replace
    // accepted content with empty entries under the same projection proof.
    let source_changed = std::cell::Cell::new(None);
    let source_read = |reference: &SessionSourceRef, expected: Option<&str>, offset, max| {
        let result = reader.read_range(reference, expected, offset, max);
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::InvalidData)
        {
            let reason = result
                .as_ref()
                .err()
                .and_then(|error| error.get_ref())
                .and_then(|error| error.downcast_ref::<LocalRevisionChanged>())
                .map(|error| ProjectionFence::LocalSourceRevision(error.0))
                .unwrap_or(ProjectionFence::SourceGeneration);
            source_changed.set(Some(reason));
        }
        result
    };
    let source_budget = if attachment.is_some() {
        12 * 1024 * 1024
    } else {
        PAGE_SOURCE_BYTES
    };
    let record_budget = if attachment.is_some() {
        source_budget
    } else {
        RECORD_BYTES
    };
    let read_chunk = if attachment.is_some() {
        64 * 1024
    } else {
        READ_CHUNK
    };
    let mut bytes_left = source_budget;
    let mut requests_left = PAGE_SOURCE_REQUESTS;
    let mut generations = BTreeMap::<PathBuf, (String, u64, bool)>::new();
    let mut fetched = 0usize;
    let mut field_previews = BTreeMap::new();
    let mut field_records = BTreeMap::new();
    for absolute in selection.first..selection.end {
        let saved = &selection.slots[absolute - selection.start];
        let Some(field) = &saved.field else {
            continue;
        };
        let Some(file) = &saved.file else {
            continue;
        };
        let result = (|| -> io::Result<Value> {
            let reference = references.get(&file.path).ok_or_else(invalid)?;
            let at = field
                .record_offset
                .checked_add(field.start)
                .ok_or_else(invalid)?;
            let end = field
                .record_offset
                .checked_add(field.end)
                .ok_or_else(invalid)?;
            if at > end
                || end > reference.offset
                || field.checkpoints.first() != Some(&field.start)
                || field.checkpoints.last() != Some(&field.end)
            {
                return Err(invalid());
            }
            let max = usize::try_from(end - at)
                .unwrap_or(usize::MAX)
                .min(READ_CHUNK + field.scalar_slack())
                .min(bytes_left);
            if max == 0 && at != end || requests_left == 0 {
                return Err(invalid());
            }
            let mut text = String::new();
            let mut complete = at == end;
            if max > 0 {
                requests_left -= 1;
                let known = generations.get(&file.path);
                let part = source_read(reference, known.map(|known| known.0.as_str()), at, max)?;
                if part.offset != at
                    || part.bytes.is_empty()
                    || part.bytes.len() > max
                    || part.length < reference.offset
                    || part.generation.is_empty()
                    || part.generation.len() > 4096
                    || known
                        .is_some_and(|known| known.0 != part.generation || known.1 != part.length)
                {
                    source_changed.set(Some(ProjectionFence::SourceRange));
                    return Err(invalid());
                }
                bytes_left -= part.bytes.len();
                fetched += part.bytes.len();
                generations
                    .entry(file.path.clone())
                    .and_modify(|known| known.2 &= part.cached)
                    .or_insert((part.generation, part.length, part.cached));
                let chunk = crate::json_string::decode_json_string_layered_chunk(
                    &part.bytes,
                    field.layers,
                    at + part.bytes.len() as u64 == end,
                    READ_CHUNK,
                )?;
                text = chunk.text;
                complete = chunk.complete && at + chunk.consumed as u64 == end;
            }
            let kind = match saved.recipe {
                crate::slot_projection::Recipe::U => "u",
                crate::slot_projection::Recipe::A => "a",
                crate::slot_projection::Recipe::Think => "think",
                crate::slot_projection::Recipe::Tool { .. } => "tool",
                _ => return Err(invalid()),
            };
            let mut entry = json!({"k":kind,"field":{"name":field.kind.name(),"layers":field.layers,"chunks":field.checkpoints.len().saturating_sub(1),"complete":complete,"provenance":{"source":reference,"offset":field.record_offset,"block":field.block,"native_event_id":field.native_event_id}},"clipped":!complete});
            entry[field.kind.name()] = json!(text);
            Ok(entry)
        })();
        if let Ok(preview) = &result
            && preview["k"] == "tool"
        {
            use crate::native_field::NativeFieldKind;
            let record = match field.kind {
                NativeFieldKind::ClaudeResult => {
                    let mut blocks = vec![Value::Null; field.block as usize + 1];
                    blocks[field.block as usize] =
                        json!({"type":"tool_result","content":preview["out"]});
                    json!({"uuid":field.native_event_id,"message":{"content":blocks}})
                }
                NativeFieldKind::CodexResult => {
                    json!({"payload":{"type":"function_call_output","output":preview["out"]}})
                }
                NativeFieldKind::CodexResultObject => {
                    json!({"payload":{"type":"function_call_output","output":{"output":preview["out"]}}})
                }
                NativeFieldKind::Text => return unavailable(),
            };
            let known = field_records
                .entry((file.path.clone(), field.record_offset))
                .or_insert_with(|| record.clone());
            if matches!(field.kind, NativeFieldKind::ClaudeResult)
                && let Some(blocks) = known["message"]["content"].as_array_mut()
            {
                blocks.resize(blocks.len().max(field.block as usize + 1), Value::Null);
                blocks[field.block as usize] =
                    record["message"]["content"][field.block as usize].clone();
            }
        }
        field_previews.insert(absolute, result);
    }
    let mut read = |path: &Path, offset: u64| -> io::Result<(Option<Value>, u64)> {
        if let Some(record) = field_records.get(&(path.to_owned(), offset)) {
            return Ok((Some(record.clone()), 0));
        }
        let reference = references.get(path).ok_or_else(invalid)?;
        if offset >= reference.offset {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        let mut at = offset;
        loop {
            let max = read_chunk
                .min(bytes_left)
                .min(record_budget.saturating_sub(bytes.len()))
                .min(usize::try_from(reference.offset - at).unwrap_or(usize::MAX));
            if max == 0 || requests_left == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "native record exceeds bounded page budget",
                ));
            }
            requests_left -= 1;
            let known = generations.get(path);
            let part = source_read(reference, known.map(|known| known.0.as_str()), at, max)?;
            if part.offset != at
                || part.bytes.is_empty()
                || part.bytes.len() > max
                || part.length < reference.offset
                || part.generation.is_empty()
                || part.generation.len() > 4096
                || known.is_some_and(|known| known.0 != part.generation || known.1 != part.length)
            {
                source_changed.set(Some(ProjectionFence::SourceRange));
                return Err(invalid());
            }
            generations
                .entry(path.to_owned())
                .and_modify(|known| known.2 &= part.cached)
                .or_insert((part.generation, part.length, part.cached));
            bytes_left -= part.bytes.len();
            fetched += part.bytes.len();
            if let Some(newline) = part.bytes.iter().position(|byte| *byte == b'\n') {
                bytes.extend_from_slice(&part.bytes[..=newline]);
                let record = crate::tx::parse_native_record(&bytes).ok_or_else(invalid)?;
                return Ok((Some(record), bytes.len() as u64));
            }
            bytes.extend_from_slice(&part.bytes);
            at += part.bytes.len() as u64;
        }
    };
    let mut slots: Vec<_> = selection.slots.iter().map(SavedSlot::restore).collect();
    // Apply qualified observations before rendering, so pending calls reuse the
    // shared live/unfinished presentation without persisting runtime authority.
    let details = &selection.session["details"];
    let working = selected_working(&selection.session);
    for (saved, slot) in selection.slots.iter().zip(&mut slots) {
        use crate::{
            model::{Shown, SlotKind},
            slot_projection::{Outcome, Recipe},
        };
        let pending = matches!(
            saved.recipe,
            Recipe::Tool {
                outcome: Outcome::Pending,
                ..
            } | Recipe::Yielded {
                outcome: Outcome::Pending,
                ..
            } | Recipe::Sent {
                outcome: Outcome::Pending,
                ..
            }
        );
        if !pending {
            continue;
        }
        let observed = working
            && slot.t.is_some()
            && slot.call_name() == details["activity"]["tool"].as_str()
            && slot.t == details["activity"]["since"].as_i64();
        let state = if observed {
            Shown::Live
        } else if !working {
            Shown::Unfinished
        } else {
            Shown::Unknown
        };
        match &mut slot.kind {
            SlotKind::Tool { shown, .. }
            | SlotKind::Yielded { shown, .. }
            | SlotKind::Sent { shown, .. } => *shown = state,
            _ => {}
        }
    }
    let mut lines = crate::tx::Lines::provider(&mut read);
    let mut entries = Vec::new();
    let mut incomplete_entries = 0;
    let mut turn_ids = std::collections::BTreeSet::new();
    for absolute in selection.first..selection.end {
        let index = absolute - selection.start;
        let slot = &slots[index];
        let before = lines.failures;
        let field_preview = field_previews.remove(&absolute);
        let field_failed = field_preview.as_ref().is_some_and(Result::is_err);
        let tool_preview = field_preview
            .as_ref()
            .and_then(|value| value.as_ref().ok())
            .filter(|value| value["k"] == "tool")
            .cloned();
        let mut entry = if let Some(preview) = field_preview.filter(|_| {
            !matches!(
                selection.slots[index].recipe,
                crate::slot_projection::Recipe::Tool { .. }
            )
        }) {
            preview.unwrap_or_else(|_| {
                let kind = match selection.slots[index].recipe {
                    crate::slot_projection::Recipe::U => "u",
                    crate::slot_projection::Recipe::A => "a",
                    crate::slot_projection::Recipe::Think => "think",
                    _ => "end",
                };
                json!({"k":kind,"text":""})
            })
        } else {
            crate::tx::render(
                &crate::tx::RenderContext {
                    home: None,
                    harness: Some(&selection.identity.harness),
                    bounded: true,
                },
                &mut lines,
                &slots,
                slot,
                index,
                crate::model::now_ms(),
            )
            .unwrap_or_else(|| json!({"k":"end"}))
        };

        if let Some(reason) = source_changed.get() {
            return stale(reason);
        }
        if matches!(slot.kind, crate::model::SlotKind::Think) && entry["text"] == "" {
            entry
                .as_object_mut()
                .expect("native entry object")
                .remove("text");
        }
        if working
            && absolute + 1 == selection.total
            && matches!(slot.kind, crate::model::SlotKind::Think)
            && entry["text"].as_str().is_none_or(str::is_empty)
            && !field_failed
            && lines.failures == before
        {
            entry["pending"] = json!(true);
        }
        if matches!(slot.kind, crate::model::SlotKind::Think)
            && selection.identity.harness == "claude"
            && let Some(secs) = crate::tx::thought_secs(&slots, index)
        {
            entry["secs"] = json!(secs);
        }
        if !selection.slots[index].images.is_empty() {
            entry["img"] = json!(selection.slots[index].images);
        }
        if let Some(index) = attachment {
            let Some(image) = entry["img"].as_array().and_then(|images| images.get(index)) else {
                return catalog::error(
                    404,
                    "attachment_unavailable",
                    "This recorded image is unavailable.",
                    false,
                );
            };
            let Some((content_type, body)) = crate::tx::selected_image(&mut lines, slot, image)
            else {
                if let Some(reason) = source_changed.get() {
                    return stale(reason);
                }
                return catalog::error(
                    404,
                    "attachment_unavailable",
                    "This recorded image is unavailable.",
                    false,
                );
            };
            return ViewerReply {
                status: 200,
                content_type,
                body,
                etag: None,
            };
        }
        if matches!(part, SelectedPart::Script) {
            let Some((text, truncated)) = crate::tx::selected_script(&mut lines, slot) else {
                if let Some(reason) = source_changed.get() {
                    return stale(reason);
                }
                return catalog::error(
                    422,
                    "field_unsupported",
                    "The recorded script is unavailable for this native entry.",
                    false,
                );
            };
            entry["scriptText"] = json!(text);
            entry["scriptTruncated"] = json!(truncated);
        }
        if matches!(part, SelectedPart::Output) {
            let Some(output) = crate::tx::selected_output(&mut lines, slot) else {
                if let Some(reason) = source_changed.get() {
                    return stale(reason);
                }
                return catalog::error(
                    422,
                    "field_unsupported",
                    "The recorded tool output is unavailable for this native entry.",
                    false,
                );
            };
            entry["out"] = output["text"].clone();
            if let Some(cut) = output.get("cut") {
                entry["cut"] = cut.clone();
            }
            entry["outputTruncated"] = output["truncated"].clone();
        }
        if let Some(preview) = tool_preview {
            entry["field"] = preview["field"].clone();
            let clipped = preview["clipped"] == true
                || entry["out"]
                    .as_str()
                    .is_some_and(|output| Some(output) != preview["out"].as_str());
            entry["field"]["complete"] = json!(!clipped);
            entry["clipped"] = json!(clipped);
        }
        entry["slot"] = json!(absolute);
        entry["observed_at"] = json!(slot.t);
        let incomplete = field_failed || lines.failures > before;
        if incomplete {
            incomplete_entries += 1;
        }
        entry["freshness"] = json!({"state":if incomplete {"incomplete"} else {"cached"}});
        if let Some(turn) = &slot.turn {
            turn_ids.insert(turn.clone());
            entry["turn"] = json!(turn);
        }
        if let Some(file) = slot.file.as_ref()
            && let Some(reference) = references.get(&file.path)
        {
            let native_event_id = selection.slots[index].native_event_id.clone().or_else(|| {
                selection.slots[index]
                    .field
                    .as_ref()
                    .filter(|field| {
                        matches!(field.kind, crate::native_field::NativeFieldKind::Text)
                    })
                    .and_then(|field| field.native_event_id.clone())
                    .or_else(|| lines.native_event_id(&file.path, slot.offset))
            });
            let position = native_event_id
                .clone()
                .unwrap_or_else(|| slot.offset.to_string());
            // Generation is observation state, never part of entry identity.
            use sha2::{Digest, Sha256};
            let mut identity = format!(
                "{}|{}|{}|{}|{}|{}|{}",
                selection.identity.source_key,
                selection.identity.catalog_key,
                reference.native_id,
                reference.path,
                position,
                slot.block,
                entry["k"].as_str().unwrap_or("unknown")
            );
            if let crate::slot_projection::Recipe::Signal { signal } =
                &selection.slots[index].recipe
            {
                identity.push_str(&format!("|{:?}", signal.kind));
            }
            entry["entry_id"] = json!(format!("{:x}", Sha256::digest(identity.as_bytes())));
            entry["provenance"] = json!({"source":reference,"native_event_id":native_event_id,"offset":slot.offset,"block":slot.block});
        }
        if entry.get("entry_id").is_none() {
            use sha2::{Digest, Sha256};
            let recipe =
                serde_json::to_string(&selection.slots[index]).expect("selected slot decoded JSON");
            entry["entry_id"] = json!(format!(
                "{:x}",
                Sha256::digest(
                    format!(
                        "{}|{}|{}",
                        selection.identity.source_key, selection.identity.catalog_key, recipe
                    )
                    .as_bytes()
                )
            ));
            entry["provenance"] = Value::Null;
        }
        entries.push(entry);
    }
    let failures = lines.failures;
    drop(lines);
    if let Some(reason) = source_changed.get() {
        return stale(reason);
    }
    let state = if failures > 0 || incomplete_entries > 0 || selection.history_incomplete {
        "incomplete"
    } else {
        selection.identity.freshness.state.as_str()
    };
    use sha2::{Digest, Sha256};
    let content_sources:Vec<_>=generations.iter().filter_map(|(path,(generation,length,cached))|references.get(path).map(|source|
        json!({"source":source,"generation_hash":format!("{:x}",Sha256::digest(generation.as_bytes())),"length":length,"cached":cached}))).collect();
    let content_state = if failures > 0 || incomplete_entries > 0 || selection.history_incomplete {
        "incomplete"
    } else if generations.values().any(|source| !source.2) {
        "available"
    } else {
        "cached"
    };
    reply(
        200,
        json!({"api":1,"identity":selection.identity,"session":selection.session,
        "projection":{"version":VERSION,"generation":selection.generation,"errors_generation":error_generation(&selection.generation, &selection.session),"total":selection.total,"history_incomplete":selection.history_incomplete},
        "range":{"first":selection.first,"end":selection.end,"next":(selection.end<selection.total).then_some(selection.end)},
        "entries":entries,"freshness":{"state":state},"content_observation":{"state":content_state,"sources":content_sources},
        "relationship_context": {"state":selection.context["state"],"turn_ids":turn_ids,"turns":selection.context["turns"],
          "handoffs":selection.context["handoffs"],"sessions":selection.context["sessions"],"previews":selection.context["previews"]},
        "limits":{"source_bytes":PAGE_SOURCE_BYTES,"source_requests":PAGE_SOURCE_REQUESTS,"record_bytes":RECORD_BYTES},
        "observation":{"source_bytes":fetched,"incomplete_entries":incomplete_entries}}),
    )
}

/// Selected native string expansion. The range query must supply `after`,
/// `limit=1`, the observed projection `generation`, and `field_chunk` ordinal.
/// Each ordinal names a persisted scalar checkpoint; no arbitrary byte cursor
/// or prefix replay is accepted.
pub fn session_entry_field(
    options: &Options,
    source_key: &str,
    query: &str,
    reader: Option<&dyn SessionSourceReader>,
) -> ViewerReply {
    session_entry_field_with_mode(
        options,
        source_key,
        query,
        reader,
        SessionSourceReadMode::ProviderOnly,
    )
}

pub fn session_entry_field_with_mode(
    options: &Options,
    source_key: &str,
    query: &str,
    reader: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
) -> ViewerReply {
    if (query_value(query, "field_chunk").is_none()
        && !matches!(
            query_value(query, "part"),
            Some("script" | "output" | "attachment")
        ))
        || query_value(query, "after").is_none()
    {
        return catalog::error(
            400,
            "invalid_arguments",
            "A selected slot and field chunk are required.",
            false,
        );
    }
    session_transcript_range_with_mode(options, source_key, query, reader, mode)
}

fn render_field(
    options: &Options,
    selection: Selection,
    provider: Option<&dyn SessionSourceReader>,
    chunk: usize,
    mode: SessionSourceReadMode,
) -> ViewerReply {
    let saved = &selection.slots[selection.first - selection.start];
    let (Some(field), Some(file)) = (&saved.field, &saved.file) else {
        return catalog::error(
            422,
            "field_unsupported",
            "This native entry has no qualified string field projection.",
            false,
        );
    };
    if chunk
        .checked_add(1)
        .is_none_or(|next| next >= field.checkpoints.len())
    {
        return catalog::error(
            400,
            "invalid_arguments",
            "The field chunk is outside this projection.",
            false,
        );
    }
    let references: BTreeMap<PathBuf, SessionSourceRef> = selection
        .identity
        .source_refs
        .iter()
        .filter_map(|observation| {
            let root = match observation.source.root.as_str() {
                "claude" => &options.claude_home,
                "codex" => &options.codex_home,
                "copilot" => &options.copilot_home,
                _ => return None,
            };
            Some((
                root.join(&observation.source.path),
                observation.source.clone(),
            ))
        })
        .collect();
    let local = Local {
        options,
        mode,
        observed: std::sync::Mutex::default(),
        proofs: std::sync::Mutex::default(),
        block: std::sync::Mutex::default(),
        bytes_left: std::sync::Mutex::new(PAGE_SOURCE_BYTES),
        sources: &selection.sources,
        references: &references,
    };
    let composite = Composite {
        local: &local,
        provider,
        mode,
        routes: std::sync::Mutex::default(),
    };
    let reader: &dyn SessionSourceReader = &composite;
    let result = (|| -> io::Result<(String, bool, usize, Value)> {
        let reference = references.get(&file.path).ok_or_else(invalid)?;
        if field.checkpoints.first() != Some(&field.start)
            || field.checkpoints.last() != Some(&field.end)
            || !field.checkpoints.windows(2).all(|pair| {
                pair[0] < pair[1]
                    && pair[1] - pair[0]
                        <= crate::json_string::JSON_STRING_CHECKPOINT_BYTES
                            + field.scalar_slack() as u64
            })
        {
            return Err(invalid());
        }
        let start = *field.checkpoints.get(chunk).ok_or_else(invalid)?;
        let end = *field
            .checkpoints
            .get(chunk.checked_add(1).ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        let at = field.record_offset.checked_add(start).ok_or_else(invalid)?;
        let end = field.record_offset.checked_add(end).ok_or_else(invalid)?;
        if at >= end || end > reference.offset {
            return Err(invalid());
        }
        let mut bytes = Vec::with_capacity((end - at) as usize);
        let mut generation = None::<String>;
        let mut length = None;
        let mut cached = true;
        for _ in 0..32 {
            let offset = at + bytes.len() as u64;
            if offset == end {
                break;
            }
            let max = usize::try_from(end - offset).map_err(|_| invalid())?;
            let part = reader.read_range(reference, generation.as_deref(), offset, max)?;
            if part.offset != offset
                || part.bytes.is_empty()
                || part.bytes.len() > max
                || part.length < reference.offset
                || part.generation.is_empty()
                || part.generation.len() > 4096
                || generation
                    .as_ref()
                    .is_some_and(|known| *known != part.generation)
                || length.is_some_and(|known| known != part.length)
            {
                return Err(invalid());
            }
            cached &= part.cached;
            generation = Some(part.generation);
            length = Some(part.length);
            bytes.extend_from_slice(&part.bytes);
        }
        if at + bytes.len() as u64 != end {
            return Err(invalid());
        }
        let decoded = crate::json_string::decode_json_string_layered_chunk(
            &bytes,
            field.layers,
            true,
            crate::json_string::JSON_STRING_CHUNK_DECODED_MAX,
        )?;
        if !decoded.complete || decoded.consumed != bytes.len() {
            return Err(invalid());
        }
        use sha2::{Digest, Sha256};
        let generation = generation.ok_or_else(invalid)?;
        let content = json!({"state":if cached {"cached"} else {"available"},"sources":[{
            "source":reference,"generation_hash":format!("{:x}", Sha256::digest(generation.as_bytes())),
            "length":length,"cached":cached}]});
        Ok((
            decoded.text,
            end == field.record_offset + field.end,
            bytes.len(),
            content,
        ))
    })();
    match result {
        Ok((text, complete, bytes, content)) => reply(
            200,
            json!({"api":1,"identity":selection.identity,
            "projection":{"version":VERSION,"generation":selection.generation},"slot":selection.first,
            "field":{"name":field.kind.name(),"layers":field.layers,"chunk":chunk,"next":(!complete).then_some(chunk+1),"complete":complete},
            "text":text,"freshness":{"state":"cached"},"content_observation":content,"provenance":{"source":references.get(&file.path),"offset":field.record_offset,"block":field.block,"native_event_id":field.native_event_id},"observation":{"source_bytes":bytes}}),
        ),
        Err(error)
            if error
                .get_ref()
                .is_some_and(|error| error.is::<LocalRevisionChanged>()) =>
        {
            let detail = error
                .get_ref()
                .and_then(|error| error.downcast_ref::<LocalRevisionChanged>())
                .expect("checked local revision");
            stale(ProjectionFence::LocalSourceRevision(detail.0))
        }
        Err(_) => catalog::error(
            503,
            "field_unavailable",
            "The selected immutable field range is unavailable. Resynchronize the session before retrying.",
            true,
        ),
    }
}
