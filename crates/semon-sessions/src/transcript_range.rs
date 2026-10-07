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
    match read(options, source_key, query, reader) {
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
) -> Result<ViewerReply, Box<dyn std::error::Error>> {
    for name in ["sid", "limit", "after", "generation", "scope"] {
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
    Ok(render(options, selection, provider))
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
        let mut file = crate::sealed::LogFile::open(path)?;
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
    let reader = provider.unwrap_or(&local);
    let mut bytes_left = PAGE_SOURCE_BYTES;
    let mut requests_left = PAGE_SOURCE_REQUESTS;
    let mut generations = BTreeMap::<PathBuf, (String, u64)>::new();
    let mut fetched = 0usize;
    let mut read = |path: &Path, offset: u64| -> io::Result<(Option<Value>, u64)> {
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
                .or_insert((part.generation, part.length));
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
        let mut entry = crate::tx::render(
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
        .unwrap_or_else(|| json!({"k":"end"}));
        entry["slot"] = json!(absolute);
        let incomplete = lines.failures > before;
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
            let native_event_id = lines.native_event_id(&file.path, slot.offset);
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
    let state = if failures > 0 {
        "incomplete"
    } else {
        selection.identity.freshness.state.as_str()
    };
    reply(
        200,
        json!({"api":1,"identity":selection.identity,"session":selection.session,
        "projection":{"version":VERSION,"generation":selection.generation,"total":selection.total},
        "range":{"first":selection.first,"end":selection.end,"next":(selection.end<selection.total).then_some(selection.end)},
        "entries":entries,"freshness":{"state":state},
        "relationship_context":{"state":"incomplete","turn_ids":turn_ids,"handoffs":[]},
        "limits":{"source_bytes":PAGE_SOURCE_BYTES,"source_requests":PAGE_SOURCE_REQUESTS,"record_bytes":RECORD_BYTES},
        "observation":{"source_bytes":fetched,"incomplete_entries":incomplete_entries}}),
    )
}
