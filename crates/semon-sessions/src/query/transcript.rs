//! Focused MCP paging from the shared native slot projection and byte proofs.
//! No complete event index, semantic cohort or unrelated source is prepared.
use super::*;
use crate::{
    events::EventCache,
    model::summary::CatalogRow,
    slot_projection::{QueryObservation, SavedSlot, VERSION},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

const MAX_SLOTS: usize = 512;
const METADATA_BYTES: usize = 4 * 1024 * 1024;

#[cfg(test)]
thread_local! { pub(super) static SLOTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn unavailable(error: impl std::fmt::Display) -> QueryError {
    QueryError::new("io", error.to_string())
}

fn unchanged(
    db: &Connection,
    key: &str,
    history: bool,
    text: &str,
    headers: &str,
    slot_generation: &str,
) -> Result<(), QueryError> {
    let current = if history {
        crate::history_projection::catalog(db, key).map_err(unavailable)?
    } else {
        db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1", [key], |row|row.get::<_,Option<String>>(0)).optional().map_err(unavailable)?.flatten()
    };
    let current_slot: Option<String> = db
        .query_row(
            &format!("SELECT generation FROM {headers} WHERE session_key=?1"),
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if current.as_deref() != Some(text) || current_slot.as_deref() != Some(slot_generation) {
        return Err(unavailable(
            "Selected native transcript publication changed",
        ));
    }
    Ok(())
}

fn generation(db: &Connection) -> Result<String, QueryError> {
    db.query_row(
        "SELECT value FROM meta WHERE key='catalog_generation'",
        [],
        |row| row.get(0),
    )
    .map_err(unavailable)
}

/// None identifies evidence not yet owned by this projection (window namespace,
/// virtual handoff or an observation awaiting its native producer). Once admitted,
/// corrupted or changed evidence is an explicit error, never a body fallback.
pub(super) fn read(
    options: &Options,
    key: &str,
    args: &Value,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    now: i64,
) -> Result<Option<Value>, QueryError> {
    #[cfg(test)]
    SLOTS.with(|slots| slots.set(0));
    // A physical scan-window namespace can omit parent/resume files and their
    // contextual slots. The ordinary Catalog projection belongs to the complete
    // native source selection, so it cannot qualify that distinct namespace.
    if args["id"] != key || options.scan_window {
        return Ok(None);
    }
    let mut db = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(unavailable)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(unavailable)?;
    let tx = db.transaction().map_err(unavailable)?;
    let catalog_generation = generation(&tx)?;
    let projection_generation: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='slot_projection_catalog_generation'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    if projection_generation.as_deref() != Some(&catalog_generation) {
        return Ok(None);
    }
    let history = crate::history_projection::catalog(&tx, key).map_err(unavailable)?;
    let text = match history.as_ref() {
        Some(text) => Some(text.clone()),
        None => tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1", [key], |row|row.get::<_,Option<String>>(0))
            .optional().map_err(unavailable)?.flatten(),
    };
    let Some(text) = text else {
        return Ok(None);
    };
    let row: CatalogRow = serde_json::from_str(&text).map_err(unavailable)?;
    if row.sources.iter().any(|source| {
        !crate::model::catalog_source_in_window(source, crate::model::scan_cutoff(options, now))
    }) {
        return Ok(None);
    }
    let Some(saved) = &row.query else {
        return Ok(None);
    };
    // A recent recorded owner necessarily survives the output window. Older
    // owners can survive through clipped relationship context; let that native
    // selection resolve membership until its focused publication is available.
    if !options.all
        && !saved["last"].as_i64().is_some_and(|last| {
            last >= now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX))
        })
        && !matches!(saved["state"].as_str(), Some("work" | "wait"))
    {
        return Ok(None);
    }
    if saved["transcript_observation"].is_null() {
        return Ok(None);
    }
    let runtime = crate::native_evidence::query_runtime_generation(options).map_err(unavailable)?;
    // Retention preserves byte custody, not a live process observation. Its
    // pending tools and backgrounds need the same runtime qualification.
    if !crate::model::summary::query_observation_matches(saved, &row.sources, true, Some(&runtime))
        .map_err(unavailable)?
    {
        return Ok(None);
    }
    let observation: QueryObservation =
        serde_json::from_value(saved["transcript_observation"].clone()).map_err(unavailable)?;
    let (headers, table) = if history.is_some() {
        ("session_history_projections", "session_history_slots")
    } else {
        ("session_slot_projections", "session_slots")
    };
    let header: Option<(String,u32,i64,Option<String>)> = tx.query_row(&format!(
        "SELECT generation,version,total,source_generation FROM {headers} WHERE session_key=?1"
    ), [key], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(unavailable)?;
    let Some((slot_generation, version, total, source_generation)) = header else {
        return Ok(None);
    };
    let total = usize::try_from(total).map_err(unavailable)?;
    if !observation.valid(total) {
        return Err(unavailable("Native transcript observation is invalid"));
    }
    if version != VERSION
        || source_generation.as_deref()
            != Some(&crate::retention::source_generation(&row.sources).map_err(unavailable)?)
    {
        return Err(unavailable("Native transcript source generation changed"));
    }
    let position = |name: &str| integer(args, name).map(|value| value.max(0) as usize);
    let anchor = tx::Anchor::of(position("before"), position("after"), string(args, "turn"))
        .ok_or_else(|| QueryError::invalid("before, after and turn are exclusive"))?;
    let (forward, from) = match &anchor {
        tx::Anchor::Last => (false, total),
        tx::Anchor::Before(before) => (false, (*before).min(total)),
        tx::Anchor::After(after) => (true, (*after).min(total)),
        tx::Anchor::Turn(turn) => {
            let turn_column = if history.is_some() {
                "json_extract(metadata,'$.turn')"
            } else {
                "CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.turn') END"
            };
            let first: Option<i64> = tx.query_row(&format!(
                "SELECT slot FROM {table} WHERE session_key=?1 AND {turn_column}=?2 AND json_extract(metadata,'$.first')=1 ORDER BY slot LIMIT 1"
            ), params![key,turn], |row|row.get(0)).optional().map_err(unavailable)?;
            (
                true,
                usize::try_from(first.ok_or_else(|| {
                    QueryError::new(
                        "unknown_turn",
                        format!("session {key:?} has no turn {turn:?}"),
                    )
                })?)
                .map_err(unavailable)?,
            )
        }
    };
    let limit = integer(args, "limit").map_or(tx::PAGE_ENTRIES, |value| value as usize);
    // The existing proof format qualifies its complete content-free hash list.
    // Bound that request-owned allocation independently of transcript paging.
    let proof_bytes = row.sources.iter().try_fold(0_u64, |bytes, source| {
        bytes.checked_add(
            source
                .offset
                .div_ceil(crate::source_chunks::SIZE as u64)
                .checked_mul(32)?,
        )
    });
    if proof_bytes.is_none_or(|bytes| bytes > METADATA_BYTES as u64) {
        return Err(unavailable(
            "Native transcript proofs exceed their read budget",
        ));
    }
    let sources = EventCache::selected_source_proofs(&tx, options, &row.sources)
        .map_err(|_| unavailable("Original native transcript proofs are unavailable"))?;
    let incomplete: bool = if history.is_some() {
        tx.query_row(
            "SELECT incomplete FROM session_history_projections WHERE session_key=?1",
            [key],
            |row| row.get(0),
        )
        .map_err(unavailable)?
    } else {
        false
    };
    if incomplete {
        return Ok(None);
    }
    tx.commit().map_err(unavailable)?;
    let rendering = provider.map(|provider| (provider, mode));
    let records = sources.reader_with(rendering);
    // Every requested block verifies its original indexed proof. Finish fences
    // only touched sources; unrelated fragments of this session need no body I/O.
    let mut count = limit.saturating_add(1).clamp(2, MAX_SLOTS);
    let home = crate::facts::MachineFacts::of(options).home();
    let bg_running: Vec<_> = observation
        .backgrounds
        .iter()
        .map(|(_, id)| id.clone())
        .collect();
    let answer = loop {
        let (low, high) = if forward {
            (
                from.saturating_sub(1),
                from.saturating_add(count).min(total),
            )
        } else {
            (from.saturating_sub(count).saturating_sub(1), from)
        };
        let snapshot = db.transaction().map_err(unavailable)?;
        unchanged(
            &snapshot,
            key,
            history.is_some(),
            &text,
            headers,
            &slot_generation,
        )?;
        let mut statement = snapshot.prepare(&format!(
            "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM {table} WHERE session_key=?1 AND slot>=?2 AND slot<?3 ORDER BY slot"
        )).map_err(unavailable)?;
        let mut bytes = 0;
        let mut recipes = Vec::new();
        for text in statement
            .query_map(
                params![
                    key,
                    i64::try_from(low).map_err(unavailable)?,
                    i64::try_from(high).map_err(unavailable)?
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(unavailable)?
        {
            let text = text.map_err(unavailable)?;
            bytes += text.len();
            if bytes > METADATA_BYTES {
                return Err(unavailable(
                    "Native transcript metadata exceeds its read budget",
                ));
            }
            recipes.push(serde_json::from_str::<SavedSlot>(&text).map_err(unavailable)?);
        }
        drop(statement);
        snapshot.commit().map_err(unavailable)?;
        if recipes.len() != high - low {
            return Err(unavailable("Native transcript slots are incomplete"));
        }
        #[cfg(test)]
        SLOTS.with(|slots| slots.set(slots.get() + recipes.len()));
        let slots: Vec<_> = recipes
            .iter()
            .enumerate()
            .map(|(index, saved)| {
                let mut slot = saved.restore();
                observation.apply(saved, &mut slot, low + index);
                slot
            })
            .collect();
        let mut read = |path: &std::path::Path, offset| records.read(path, offset);
        let mut prompts = |file: &crate::model::SlotFile, offset| records.prompt(file, offset);
        let page = tx::page_from_window(
            &tx::RenderContext {
                home: home.as_deref(),
                harness: Some(&row.harness),
                bounded: false,
            },
            tx::PageWindow {
                sid: key,
                slots: &slots,
                start: low,
                total,
                from,
                forward,
                calls: observation.calls,
                errors: observation.errors,
                bg_running: &bg_running,
            },
            now,
            limit,
            &mut tx::Lines::provider_with_prompts(&mut read, &mut prompts),
        );
        match page {
            Ok(page) => break page,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && count < MAX_SLOTS => {
                count = (count * 2).min(MAX_SLOTS)
            }
            Err(error) => return Err(unavailable(error)),
        }
    };
    records.finish().map_err(unavailable)?;
    let snapshot = db.transaction().map_err(unavailable)?;
    unchanged(
        &snapshot,
        key,
        history.is_some(),
        &text,
        headers,
        &slot_generation,
    )?;
    snapshot.commit().map_err(unavailable)?;
    if crate::native_evidence::query_runtime_generation(options).map_err(unavailable)? != runtime {
        return Err(unavailable(
            "Native transcript observation changed during this read",
        ));
    }
    Ok(Some(answer))
}
