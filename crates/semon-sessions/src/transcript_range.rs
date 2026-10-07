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
    time::{Duration, UNIX_EPOCH},
};

const MAX_LIMIT: usize = 100;
const PAGE_SOURCE_BYTES: usize = 1024 * 1024;
const PAGE_SOURCE_REQUESTS: usize = 256;
const RECORD_BYTES: usize = 128 * 1024;
const READ_CHUNK: usize = 4096;

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
/// LocalThenProvider is an explicit host opt-in for mixed live/archived sources.
/// The host must authorize the complete read before and after this call, because
/// its provider callback is not invoked for available qualified local bytes.
#[derive(Clone, Copy, Debug, Default)]
pub enum SessionSourceReadMode {
    #[default]
    ProviderOnly,
    LocalThenProvider,
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
        "scope",
        "field_chunk",
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
    let selected_identity = if history {
        catalog::session_catalog_history_identity(options, source_key, &key)
    } else {
        catalog::session_catalog_identity(options, source_key, &key)
    };
    let identity = match selected_identity {
        Ok(Some(identity)) => identity,
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
    let metadata = catalog::session_catalog_page(
        options,
        source_key,
        &format!(
            "sid={}&scope={}",
            crate::viewer::percent_encode(&key),
            if history {
                "retained_history"
            } else {
                "current"
            }
        ),
    );
    if metadata.status != 200 {
        return Ok(metadata);
    }
    let metadata: Value = serde_json::from_slice(&metadata.body)?;
    if metadata["generation"] != identity.generation {
        return Ok(stale());
    }
    let session = metadata["items"][0].clone();
    let mut connection = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(100))?;
    let transaction = connection.transaction()?;
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
        return Ok(stale());
    }
    let projection: Option<(String, u32, i64)> = transaction
        .query_row(
            "SELECT generation,version,total FROM session_slot_projections WHERE session_key=?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((generation, version, total)) = projection else {
        return Ok(unavailable());
    };
    let total = usize::try_from(total)?;
    if version != VERSION {
        return Ok(unavailable());
    }
    if argument("generation").is_some_and(|expected| expected != generation) {
        return Ok(stale());
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
        "SELECT CASE WHEN length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_slots WHERE session_key=?1 AND slot>=?2 AND slot<?3 ORDER BY slot")?;
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
    let row_text: String = transaction.query_row(
        "SELECT metadata FROM session_catalog WHERE session_key=?1",
        [&key],
        |row| row.get(0),
    )?;
    let row: crate::model::summary::CatalogRow = serde_json::from_str(&row_text)?;
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
    Ok(render(options, selection, provider, mode))
}

fn stale() -> ViewerReply {
    let mut reply = catalog::error(
        409,
        "stale_projection",
        "The selected projection changed. Resynchronize this session while retaining native entry identities.",
        true,
    );
    let mut body: Value = serde_json::from_slice(&reply.body).expect("owned JSON reply");
    body["resynchronize"] = json!(true);
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

struct Composite<'a> {
    local: &'a Local<'a>,
    provider: Option<&'a dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
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

struct Local<'a> {
    sources: &'a BTreeMap<PathBuf, CatalogSource>,
    references: &'a BTreeMap<PathBuf, SessionSourceRef>,
}
impl SessionSourceReader for Local<'_> {
    fn read_range(
        &self,
        reference: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        let path = self
            .references
            .iter()
            .find_map(|(path, known)| (known == reference).then_some(path))
            .ok_or_else(invalid)?;
        let source = self.sources.get(path).ok_or_else(invalid)?;
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
            || max > crate::SESSION_SOURCE_CHUNK_MAX
            || offset > reference.offset
            || max as u64 > reference.offset - offset
        {
            return Err(invalid());
        }
        let check = |metadata: &std::fs::Metadata| {
            #[cfg(unix)]
            let identity = {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            };
            #[cfg(not(unix))]
            let identity = (0, 0);
            identity == (source.dev, source.ino)
                && metadata.len() == source.size
                && metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|time| time.as_nanos())
                    == Some(source.modified_ns)
        };
        if !check(&file.metadata()?) {
            return Err(invalid());
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; max];
        let read = file.read(&mut bytes)?;
        bytes.truncate(read);
        if !check(&file.metadata()?) {
            return Err(invalid());
        }
        Ok(crate::SessionSourceRange {
            generation,
            length: source.size,
            offset,
            bytes: Arc::from(bytes),
            cached: true,
        })
    }
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
) -> ViewerReply {
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
        sources: &selection.sources,
        references: &references,
    };
    let composite = Composite {
        local: &local,
        provider,
        mode,
    };
    let reader: &dyn SessionSourceReader = &composite;
    let mut bytes_left = PAGE_SOURCE_BYTES;
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
                .min(READ_CHUNK + 11)
                .min(bytes_left);
            if max == 0 && at != end || requests_left == 0 {
                return Err(invalid());
            }
            let mut text = String::new();
            let mut complete = at == end;
            if max > 0 {
                requests_left -= 1;
                let known = generations.get(&file.path);
                let part =
                    reader.read_range(reference, known.map(|known| known.0.as_str()), at, max)?;
                if part.offset != at
                    || part.bytes.is_empty()
                    || part.bytes.len() > max
                    || part.length < reference.offset
                    || part.generation.is_empty()
                    || part.generation.len() > 4096
                    || known
                        .is_some_and(|known| known.0 != part.generation || known.1 != part.length)
                {
                    return Err(invalid());
                }
                bytes_left -= part.bytes.len();
                fetched += part.bytes.len();
                generations
                    .entry(file.path.clone())
                    .and_modify(|known| known.2 &= part.cached)
                    .or_insert((part.generation, part.length, part.cached));
                let chunk = crate::json_string::decode_json_string_chunk(
                    &part.bytes,
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
            let mut entry = json!({"k":kind,"field":{"name":field.kind.name(),"chunks":field.checkpoints.len().saturating_sub(1),"complete":complete},"clipped":!complete});
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
            let max = READ_CHUNK
                .min(bytes_left)
                .min(RECORD_BYTES.saturating_sub(bytes.len()))
                .min(usize::try_from(reference.offset - at).unwrap_or(usize::MAX));
            if max == 0 || requests_left == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "native record exceeds bounded page budget",
                ));
            }
            requests_left -= 1;
            let known = generations.get(path);
            let part =
                reader.read_range(reference, known.map(|known| known.0.as_str()), at, max)?;
            if part.offset != at
                || part.bytes.is_empty()
                || part.bytes.len() > max
                || part.length < reference.offset
                || part.generation.is_empty()
                || part.generation.len() > 4096
                || known.is_some_and(|known| known.0 != part.generation || known.1 != part.length)
            {
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
    let slots: Vec<_> = selection.slots.iter().map(SavedSlot::restore).collect();
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

        if let Some(preview) = tool_preview {
            entry["field"] = preview["field"].clone();
            let clipped =
                preview["clipped"] == true || entry["out"].as_str() != preview["out"].as_str();
            entry["field"]["complete"] = json!(!clipped);
            entry["clipped"] = json!(clipped);
        }
        entry["slot"] = json!(absolute);
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
            entry["entry_id"] = json!(format!(
                "{:x}",
                Sha256::digest(
                    format!(
                        "{}|{}|{}|{}|{}|{}|{}",
                        selection.identity.source_key,
                        selection.identity.catalog_key,
                        reference.native_id,
                        reference.path,
                        position,
                        slot.block,
                        entry["k"].as_str().unwrap_or("unknown")
                    )
                    .as_bytes()
                )
            ));
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
    let state = if failures > 0 || incomplete_entries > 0 {
        "incomplete"
    } else {
        selection.identity.freshness.state.as_str()
    };
    use sha2::{Digest, Sha256};
    let content_sources:Vec<_>=generations.iter().filter_map(|(path,(generation,length,cached))|references.get(path).map(|source|
        json!({"source":source,"generation_hash":format!("{:x}",Sha256::digest(generation.as_bytes())),"length":length,"cached":cached}))).collect();
    let content_state = if failures > 0 || incomplete_entries > 0 {
        "incomplete"
    } else if generations.values().any(|source| !source.2) {
        "available"
    } else {
        "cached"
    };
    reply(
        200,
        json!({"api":1,"identity":selection.identity,"session":selection.session,
        "projection":{"version":VERSION,"generation":selection.generation,"total":selection.total},
        "range":{"first":selection.first,"end":selection.end,"next":(selection.end<selection.total).then_some(selection.end)},
        "entries":entries,"freshness":{"state":state},"content_observation":{"state":content_state,"sources":content_sources},
        "relationship_context":{"state":"incomplete","turn_ids":turn_ids,"handoffs":[]},
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
    if query_value(query, "field_chunk").is_none() || query_value(query, "after").is_none() {
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
        sources: &selection.sources,
        references: &references,
    };
    let composite = Composite {
        local: &local,
        provider,
        mode,
    };
    let reader: &dyn SessionSourceReader = &composite;
    let result = (|| -> io::Result<(String, bool, usize)> {
        let reference = references.get(&file.path).ok_or_else(invalid)?;
        if field.checkpoints.first() != Some(&field.start)
            || field.checkpoints.last() != Some(&field.end)
            || !field.checkpoints.windows(2).all(|pair| {
                pair[0] < pair[1]
                    && pair[1] - pair[0] <= crate::json_string::JSON_STRING_CHECKPOINT_BYTES + 11
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
            generation = Some(part.generation);
            length = Some(part.length);
            bytes.extend_from_slice(&part.bytes);
        }
        if at + bytes.len() as u64 != end {
            return Err(invalid());
        }
        let decoded = crate::json_string::decode_json_string_chunk(
            &bytes,
            true,
            crate::json_string::JSON_STRING_CHUNK_DECODED_MAX,
        )?;
        if !decoded.complete || decoded.consumed != bytes.len() {
            return Err(invalid());
        }
        Ok((
            decoded.text,
            end == field.record_offset + field.end,
            bytes.len(),
        ))
    })();
    match result {
        Ok((text, complete, bytes)) => reply(
            200,
            json!({"api":1,"identity":selection.identity,
            "projection":{"version":VERSION,"generation":selection.generation},"slot":selection.first,
            "field":{"name":field.kind.name(),"chunk":chunk,"next":(!complete).then_some(chunk+1),"complete":complete},
            "text":text,"freshness":{"state":"cached"},"provenance":{"source":references.get(&file.path),"offset":field.record_offset,"block":field.block,"native_event_id":field.native_event_id},"observation":{"source_bytes":bytes}}),
        ),
        Err(_) => catalog::error(
            503,
            "field_unavailable",
            "The selected immutable field range is unavailable. Resynchronize the session before retrying.",
            true,
        ),
    }
}
