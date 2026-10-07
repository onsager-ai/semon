//! Focused reads of the published metadata catalog. This path never opens the
//! EventCache, walks input homes, loads transcript bodies or constructs a model.
use crate::{
    Options,
    events::EventCache,
    model::summary::{CATALOG_VERSION, CatalogRow, CatalogSource},
    viewer::{ViewerReply, decoded, query_value},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io,
    time::{Duration, UNIX_EPOCH},
};

const MAX_LIMIT: usize = 100;
const MAX_CURSOR: usize = 8192;

/// Provider-neutral identity for a retained native source generation. The path
/// is relative to `root`, never an absolute host path or a credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSourceRef {
    pub root: String,
    pub path: String,
    pub native_id: String,
    pub offset: u64,
    pub prefix_sha256: [u8; 32],
    pub tail_sha256: [u8; 32],
}

/// Observation of a source in a cached catalog projection. This is neither
/// runtime health nor authorization to mutate the native session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSourceObservation {
    pub source: SessionSourceRef,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFreshness {
    pub state: String,
}

/// A metadata-only match between one published native source and its current
/// consumed-prefix ledger. This observation grants no archive or control access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSourceProof {
    pub source: SessionSourceRef,
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub modified_ns: u128,
    pub freshness: CatalogFreshness,
}

/// Resolve an exact configured source path using indexed metadata reads only.
/// The host must validate the source identity again while copying its bytes.
pub fn session_source_proof(
    options: &Options,
    root: &str,
    path: &str,
) -> Result<Option<SessionSourceProof>, CatalogIdentityError> {
    use CatalogIdentityError::{InvalidArguments, Unavailable};
    if !crate::is_input_path(root, path) || path.len() > 4096 {
        return Err(InvalidArguments);
    }
    let home = match root {
        "claude" => &options.claude_home,
        "codex" => &options.codex_home,
        "copilot" => &options.copilot_home,
        _ => return Err(InvalidArguments),
    };
    // Recorded roots/native manifests must remain observable. Missing facts
    // cannot be treated as permission to reuse a formerly selected rollout.
    let current = options
        .facts
        .as_ref()
        .map(|file| {
            crate::facts::source_authority::CurrentSources::open(file).map_err(|_| Unavailable)
        })
        .transpose()?;
    if current.as_ref().is_some_and(|source| {
        !source.facts_known() || (root == "codex" && !source.selection_known())
    }) {
        return Err(Unavailable);
    }
    let absolute = home.join(path);
    let absolute = absolute.to_str().ok_or(InvalidArguments)?;
    let read = || -> Result<Option<SessionSourceProof>, Box<dyn std::error::Error>> {
        let mut connection = Connection::open_with_flags(
            EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(100))?;
        let transaction = connection.transaction()?;
        let encoded: Option<(String, Option<String>)> = transaction.query_row(
            "SELECT c.session_key,CASE WHEN length(CAST(c.metadata AS BLOB))<=1048576 THEN c.metadata ELSE NULL END FROM session_catalog_sources s JOIN session_catalog c ON c.session_key=s.session_key WHERE s.source_path=?1 ORDER BY s.session_key LIMIT 1",
            [absolute], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let Some((key, encoded)) = encoded else {
            return Ok(None);
        };
        let encoded = encoded.ok_or_else(|| io::Error::other("source metadata bound"))?;
        // Decode only the selected relationship metadata, never a native body.
        if encoded.len() > 1024 * 1024 {
            return Err(io::Error::other("source metadata bound").into());
        }
        let row: CatalogRow = serde_json::from_str(&encoded)?;
        if row.key != key || row.harness != root || row.sources.len() > 64 {
            return Err(io::Error::other("source mapping changed").into());
        }
        let source = row
            .sources
            .iter()
            .find(|source| source.path.to_str() == Some(absolute))
            .ok_or_else(|| io::Error::other("source mapping absent"))?;
        type Revision = (u64, u64, u64, [u8; 16], u64, [u8; 32], [u8; 32]);
        let revision: Option<Revision> = transaction.query_row(
            "SELECT dev,ino,size,mtime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1",
            [absolute], |row| Ok((row.get::<_,i64>(0)? as u64,row.get::<_,i64>(1)? as u64,row.get::<_,i64>(2)? as u64,row.get(3)?,row.get::<_,i64>(4)? as u64,row.get(5)?,row.get(6)?)),
        ).optional()?;
        if revision
            != Some((
                source.dev,
                source.ino,
                source.size,
                source.modified_ns.to_be_bytes(),
                source.offset,
                source.prefix_sha256,
                source.tail_sha256,
            ))
        {
            return Err(io::Error::other("source ledger changed").into());
        }
        let generation: String = transaction.query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        // Reuse the catalog's current-root/native-selection validation. A
        // publication race fails closed rather than mixing catalog generations.
        let identity = session_catalog_identity(options, "source-proof", &key)?
            .ok_or_else(|| io::Error::other("source catalog changed"))?;
        let observed = identity
            .source_refs
            .iter()
            .find(|observed| observed.source.root == root && observed.source.path == path)
            .ok_or_else(|| io::Error::other("source scope changed"))?;
        if identity.generation != generation
            || !matches!(observed.state.as_str(), "cached" | "incomplete")
            || observed.source.native_id != source.native_id
            || observed.source.offset != source.offset
            || observed.source.prefix_sha256 != source.prefix_sha256
        {
            return Err(io::Error::other("source observation changed").into());
        }
        if let Some(current) = &current {
            current.validate()?;
        }
        Ok(Some(SessionSourceProof {
            source: observed.source.clone(),
            dev: source.dev,
            ino: source.ino,
            size: source.size,
            modified_ns: source.modified_ns,
            freshness: CatalogFreshness {
                state: observed.state.clone(),
            },
        }))
    };
    read().map_err(|_| Unavailable)
}

/// Scoped cached identity, resolved without global event/model construction.
/// A nullable native_id preserves ambiguity in multi-native continuations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSessionIdentity {
    pub source_key: String,
    pub catalog_key: String,
    pub harness: String,
    pub native_id: Option<String>,
    pub native_ids: Vec<String>,
    pub source_refs: Vec<CatalogSourceObservation>,
    pub machine_label: Option<String>,
    pub generation: String,
    pub observed_at: Option<i64>,
    pub freshness: CatalogFreshness,
    /// Original machine-facts observation, independent of source byte availability.
    pub facts_observation: CatalogFreshness,
    /// Current native manifest observation; absent for harnesses without one.
    /// Neither observation grants runtime or credential authority.
    pub native_selection: Option<CatalogFreshness>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogIdentityError {
    InvalidArguments,
    Unavailable,
    ScopeChanged,
}
impl std::fmt::Display for CatalogIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidArguments => "Invalid scoped catalog identity arguments.",
            Self::Unavailable => "The selected catalog observation is unavailable.",
            Self::ScopeChanged => "The configured native source authority changed.",
        })
    }
}
impl std::error::Error for CatalogIdentityError {}

/// The host selects authorized options and its stable source_key. Missing keys
/// are distinct from unavailable observations. Source roots and current native
/// manifests are validated exactly as for the paged catalog; no source bodies
/// are read. Cached identity alone grants no control or provider authority.
pub fn session_catalog_identity(
    options: &Options,
    source_key: &str,
    canonical_catalog_key: &str,
) -> Result<Option<CatalogSessionIdentity>, CatalogIdentityError> {
    use CatalogIdentityError::{InvalidArguments, ScopeChanged, Unavailable};
    if source_key.is_empty()
        || source_key.len() > 4096
        || canonical_catalog_key.is_empty()
        || canonical_catalog_key.len() > 4096
    {
        return Err(InvalidArguments);
    }
    let request = Request {
        limit: 1,
        sid: Some(canonical_catalog_key.to_owned()),
        harness: None,
        repo: None,
        cursor: None,
    };
    let reply = read_page(options, source_key, &request).map_err(|_| Unavailable)?;
    let body: Value = serde_json::from_slice(&reply.body).map_err(|_| Unavailable)?;
    match reply.status {
        404 => return Ok(None),
        200 => {}
        _ if body["error"]["code"] == "catalog_scope_changed" => return Err(ScopeChanged),
        _ => return Err(Unavailable),
    }
    #[derive(Deserialize)]
    struct Item {
        key: String,
        harness: String,
        native_ids: Vec<String>,
        source_refs: Vec<CatalogSourceObservation>,
        freshness: CatalogFreshness,
        facts_observation: CatalogFreshness,
        native_selection: Option<CatalogFreshness>,
    }
    let item: Item = serde_json::from_value(body["items"][0].clone()).map_err(|_| Unavailable)?;
    let native_ids: Vec<String> = item
        .source_refs
        .iter()
        .map(|reference| reference.source.native_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if item.key != canonical_catalog_key
        || native_ids.is_empty()
        || native_ids.iter().any(String::is_empty)
        || item.native_ids != native_ids
    {
        return Err(Unavailable);
    }
    let generation = body["generation"].as_str().ok_or(Unavailable)?.to_owned();
    if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Unavailable);
    }
    let observed_at = body["observed_at"].as_i64();
    if observed_at.is_some_and(|time| !(0..=9_007_199_254_740_991).contains(&time)) {
        return Err(Unavailable);
    }
    Ok(Some(CatalogSessionIdentity {
        source_key: source_key.to_owned(),
        catalog_key: item.key,
        harness: item.harness,
        native_id: (native_ids.len() == 1).then(|| native_ids[0].clone()),
        native_ids,
        source_refs: item.source_refs,
        machine_label: body["machine_info"]["label"].as_str().map(str::to_owned),
        generation,
        observed_at,
        freshness: item.freshness,
        facts_observation: item.facts_observation,
        native_selection: item.native_selection,
    }))
}

/// Read a configured machine's published metadata page without constructing a
/// ViewerCore or opening its event cache. The host authenticates the caller and
/// chooses `options` and its stable `machine` key before calling this function.
/// Publication/refresh remains the host's coordinator responsibility. All
/// metadata is labelled cached; source disappearance and changed generations
/// remain explicit. The query contract matches GET /api/sessions.
pub fn session_catalog_page(options: &Options, machine: &str, query: &str) -> ViewerReply {
    match Request::parse(query) {
        Ok(request) => page(options, machine, request),
        Err(reply) => reply,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    generation: String,
    machine: String,
    harness: Option<String>,
    repo: Option<String>,
    last: i64,
    key: String,
}

pub(crate) struct Request {
    limit: usize,
    sid: Option<String>,
    harness: Option<String>,
    repo: Option<String>,
    cursor: Option<Cursor>,
}

pub(crate) fn error(status: u16, code: &str, message: &str, retryable: bool) -> ViewerReply {
    reply(
        status,
        json!({"error":{"code":code,"message":message,"retryable":retryable},
        "reset_cursor":code == "stale_cursor" || code == "cursor_scope_mismatch"}),
    )
}

fn reply(status: u16, body: Value) -> ViewerReply {
    ViewerReply {
        status,
        content_type: "application/json; charset=utf-8",
        body: body.to_string().into_bytes(),
        etag: None,
    }
}

impl Request {
    pub(crate) fn parse(query: &str) -> Result<Self, ViewerReply> {
        let invalid = || {
            error(
                400,
                "invalid_arguments",
                "Use limit=1..100 and valid harness, repo and cursor parameters.",
                false,
            )
        };
        let parameter = |key| match query_value(query, key) {
            None => Ok(None),
            Some(value) => decoded(value).map(Some).ok_or_else(invalid),
        };
        let limit = parameter("limit")?
            .map_or(Ok(60), |value| value.parse::<usize>())
            .map_err(|_| invalid())?;
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(invalid());
        }
        if parameter("q")?.is_some() {
            return Err(error(
                400,
                "unsupported_filter",
                "Full-text search is not available on this catalog endpoint.",
                false,
            ));
        }
        let sid = parameter("sid")?;
        if sid
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 4096)
        {
            return Err(invalid());
        }
        let harness = parameter("harness")?;
        if harness
            .as_deref()
            .is_some_and(|h| !matches!(h, "claude" | "codex" | "copilot"))
        {
            return Err(invalid());
        }
        let repo = parameter("repo")?;
        if repo
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 4096)
        {
            return Err(invalid());
        }
        let cursor = parameter("cursor")?
            .map(|value| {
                if value.len() > MAX_CURSOR {
                    return Err(invalid());
                }
                let cursor: Cursor = serde_json::from_str(&value).map_err(|_| invalid())?;
                if cursor.version != 1
                    || cursor.generation.len() != 64
                    || !cursor.generation.bytes().all(|c| c.is_ascii_hexdigit())
                    || cursor.key.is_empty()
                    || cursor.key.len() > 4096
                    || cursor.machine.len() > 4096
                {
                    return Err(invalid());
                }
                Ok(cursor)
            })
            .transpose()?;
        if sid.is_some() && (harness.is_some() || repo.is_some() || cursor.is_some()) {
            return Err(invalid());
        }
        Ok(Self {
            limit,
            sid,
            harness,
            repo,
            cursor,
        })
    }
}

/// Two direct keyset ranges avoid scanning earlier keys when many sessions
/// share the same timestamp. OFFSET and mixed-order tuple comparisons do not
/// provide that property. All ranges are read in one SQLite snapshot.
fn rows(connection: &Connection, request: &Request) -> rusqlite::Result<Vec<CatalogRow>> {
    if let Some(sid) = &request.sid {
        return read_range(
            connection,
            "sqlite_autoindex_session_catalog_1",
            "",
            "session_key = ?",
            vec![SqlValue::Text(sid.clone())],
            1,
        );
    }
    let (filters, index) = match (request.harness.is_some(), request.repo.is_some()) {
        (false, false) => ("", "session_catalog_order"),
        (true, false) => ("harness = ? AND ", "session_catalog_harness"),
        (false, true) => ("repo = ? AND ", "session_catalog_repo"),
        (true, true) => (
            "harness = ? AND repo = ? AND ",
            "session_catalog_harness_repo",
        ),
    };
    let mut base = Vec::new();
    if let Some(value) = &request.harness {
        base.push(SqlValue::Text(value.clone()));
    }
    if let Some(value) = &request.repo {
        base.push(SqlValue::Text(value.clone()));
    }
    let limit = request.limit + 1;
    let mut found = if let Some(cursor) = &request.cursor {
        let mut args = base.clone();
        args.push(SqlValue::Integer(cursor.last));
        args.push(SqlValue::Text(cursor.key.clone()));
        let mut found = read_range(
            connection,
            index,
            filters,
            "last_ms = ? AND session_key > ?",
            args,
            limit,
        )?;
        let remaining = limit - found.len();
        if remaining > 0 {
            let mut args = base;
            args.push(SqlValue::Integer(cursor.last));
            found.extend(read_range(
                connection,
                index,
                filters,
                "last_ms < ?",
                args,
                remaining,
            )?);
        }
        found
    } else {
        read_range(connection, index, filters, "1", base, limit)?
    };
    // Capacity returned by SQLite is bounded by the requested page plus one.
    found.shrink_to_fit();
    Ok(found)
}

#[cfg(test)]
thread_local! {
    static SQL_STEPS: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

fn read_range(
    connection: &Connection,
    index: &str,
    filters: &str,
    range: &str,
    mut args: Vec<SqlValue>,
    limit: usize,
) -> rusqlite::Result<Vec<CatalogRow>> {
    let sql = format!(
        "SELECT metadata,session_key,last_ms,harness,repo FROM session_catalog INDEXED BY {index} WHERE {filters}{range} ORDER BY last_ms DESC,session_key ASC LIMIT ?"
    );
    args.push(SqlValue::Integer(limit as i64));
    let mut statement = connection.prepare(&sql)?;
    let mut found = Vec::new();
    {
        let mut selected = statement.query(rusqlite::params_from_iter(args))?;
        while let Some(row) = selected.next()? {
            let encoded: String = row.get(0)?;
            let metadata: CatalogRow = serde_json::from_str(&encoded).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            if metadata.key != row.get::<_, String>(1)?
                || metadata.last.unwrap_or(0) != row.get::<_, i64>(2)?
                || metadata.harness != row.get::<_, String>(3)?
                || metadata.repo != row.get::<_, Option<String>>(4)?
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            found.push(metadata);
        }
    }
    #[cfg(test)]
    SQL_STEPS.with(|steps| {
        steps.set(steps.get() + statement.get_status(rusqlite::StatementStatus::VmStep))
    });
    Ok(found)
}

fn source_relative(options: &Options, harness: &str, source: &CatalogSource) -> Option<String> {
    let home = match harness {
        "claude" => &options.claude_home,
        "codex" => &options.codex_home,
        "copilot" => &options.copilot_home,
        _ => return None,
    };
    let relative = source.path.strip_prefix(home).ok()?.to_str()?;
    (source
        .path
        .extension()
        .is_some_and(|extension| extension == "jsonl")
        && crate::is_input_path(harness, relative))
    .then(|| relative.to_owned())
}

fn source_state(source: &CatalogSource) -> &'static str {
    let Ok(file) = crate::sealed::LogFile::open(&source.path) else {
        return "unavailable";
    };
    let Ok(metadata) = file.metadata() else {
        return "unavailable";
    };
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = (0, 0);
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|time| time.as_nanos());
    if identity == (source.dev, source.ino)
        && metadata.len() == source.size
        && modified == Some(source.modified_ns)
    {
        if source.size > source.offset {
            "incomplete"
        } else {
            "cached"
        }
    } else {
        "stale"
    }
}

pub(crate) fn page(options: &Options, machine: &str, request: Request) -> ViewerReply {
    match read_page(options, machine, &request) {
        Ok(reply) => reply,
        Err(_) => error(
            503,
            "catalog_unavailable",
            "The session catalog is unavailable. Retry after the background observation completes, or use /api/model to resynchronize.",
            true,
        ),
    }
}

fn read_page(
    options: &Options,
    machine: &str,
    request: &Request,
) -> Result<ViewerReply, Box<dyn std::error::Error>> {
    if request.cursor.as_ref().is_some_and(|cursor| {
        cursor.machine != machine
            || cursor.harness != request.harness
            || cursor.repo != request.repo
    }) {
        return Ok(error(
            400,
            "cursor_scope_mismatch",
            "Discard the cursor when changing machine or filters.",
            false,
        ));
    }
    let mut connection = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(100))?;
    let transaction = connection.transaction()?;
    let meta = |key: &str| {
        transaction
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .optional()
    };
    let version = meta("catalog_version")?;
    let Some(generation) = meta("catalog_generation")? else {
        return Err(io::Error::other("catalog not published").into());
    };
    if version.as_deref() != Some(CATALOG_VERSION.to_string().as_str()) {
        return Err(io::Error::other("catalog version").into());
    }
    if request
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.generation != generation)
    {
        return Ok(error(
            409,
            "stale_cursor",
            "The catalog changed. Discard this cursor and request the first page to resynchronize.",
            true,
        ));
    }
    let observed_at = meta("catalog_observed_at")?.and_then(|value| value.parse::<i64>().ok());
    let mut selected = rows(&transaction, request)?;
    if request.sid.is_some() && selected.is_empty() {
        return Ok(error(
            404,
            "session_not_found",
            "The selected session is absent from this catalog generation. Refresh the list to resynchronize.",
            false,
        ));
    }
    let more = selected.len() > request.limit;
    if more {
        selected.pop();
    }
    let next = if more {
        selected
            .last()
            .map(|row| {
                serde_json::to_string(&Cursor {
                    version: 1,
                    generation: generation.clone(),
                    machine: machine.to_owned(),
                    harness: request.harness.clone(),
                    repo: request.repo.clone(),
                    last: row.last.unwrap_or(0),
                    key: row.key.clone(),
                })
            })
            .transpose()?
    } else {
        None
    };
    transaction.commit()?;
    // The legacy model tolerates absent facts/old source selection. Focused
    // observations must retain that absence rather than invent current facts.
    let recorded = options
        .facts
        .as_ref()
        .and_then(|path| crate::facts::source_authority::CurrentSources::open(path).ok());
    let facts_known =
        options.facts.is_none() || recorded.as_ref().is_some_and(|source| source.facts_known());
    let codex_selection_known = options.facts.is_none()
        || recorded
            .as_ref()
            .is_some_and(|source| source.selection_known());
    let machine_label = match &options.facts {
        None => Some(crate::facts::MachineFacts::Local.hostname(options)),
        Some(_) => recorded.as_ref().and_then(|source| source.hostname()),
    };
    let mut page_state = if facts_known { "cached" } else { "unavailable" };
    let mut items = Vec::with_capacity(selected.len());
    for row in selected {
        let mut refs = Vec::new();
        let mut states = Vec::new();
        if row.sources.is_empty() {
            return Ok(error(
                503,
                "catalog_scope_changed",
                "The cached projection has no source authority. Refresh the observation.",
                true,
            ));
        }
        for source in &row.sources {
            let Some(relative) = source_relative(options, &row.harness, source) else {
                return Ok(error(
                    503,
                    "catalog_scope_changed",
                    "The configured source roots changed. Refresh the catalog before reading this page.",
                    true,
                ));
            };
            let retired = if row.harness == "codex" {
                match recorded
                    .as_ref()
                    .map(|snapshot| snapshot.codex_current(&relative))
                    .transpose()
                {
                    Ok(current) => current.flatten() == Some(false),
                    Err(_) => {
                        return Ok(error(
                            503,
                            "catalog_scope_changed",
                            "Source observation changed during this read. Retry with the current catalog.",
                            true,
                        ));
                    }
                }
            } else {
                false
            };
            if retired {
                return Ok(error(
                    503,
                    "catalog_scope_changed",
                    "The selected native source authority changed. Refresh the catalog before reading this page.",
                    true,
                ));
            }
            let state = source_state(source);
            states.push(state);
            let reference = SessionSourceRef {
                root: row.harness.clone(),
                path: relative,
                native_id: source.native_id.clone(),
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
            };
            refs.push(json!({"source":reference,"state":state}));
        }
        let state = if !facts_known || states.iter().all(|state| *state == "unavailable") {
            "unavailable"
        } else if states.contains(&"unavailable")
            || states.contains(&"incomplete")
            || (row.harness == "codex" && !codex_selection_known)
        {
            "incomplete"
        } else if states.contains(&"stale") {
            "stale"
        } else {
            "cached"
        };
        if page_state != "unavailable" {
            if matches!(state, "unavailable" | "incomplete") {
                page_state = "incomplete";
            } else if state == "stale" && page_state == "cached" {
                page_state = "stale";
            }
        }
        let is_codex = row.harness == "codex";
        let mut item = serde_json::to_value(row)?;
        item.as_object_mut()
            .expect("catalog row object")
            .remove("sources");
        item["source_refs"] = json!(refs);
        item["freshness"] = json!({"state":state});
        item["facts_observation"] =
            json!({"state":if facts_known { "cached" } else { "unavailable" }});
        item["native_selection"] = if is_codex {
            json!({"state":if !facts_known { "unavailable" } else if codex_selection_known { "cached" } else { "incomplete" }})
        } else {
            serde_json::Value::Null
        };
        items.push(item);
    }
    if recorded
        .as_ref()
        .is_some_and(|source| source.validate().is_err())
    {
        return Ok(error(
            503,
            "catalog_scope_changed",
            "Source observation changed during this read. Retry with the current catalog.",
            true,
        ));
    }
    Ok(reply(
        200,
        json!({"api":1,"machine":machine,"generation":generation,"observed_at":observed_at,
        "machine_info":{"key":machine,"label":machine_label,"freshness":if facts_known { "cached" } else { "unavailable" }},
        "capabilities":{"pagination":true,"filters":["harness","repo"],"order":"last_desc_key_asc",
            "full_text_search":false,"selected_session_lookup":true,"runtime_status":false,"global_union":false},
        "freshness":page_state,"items":items,"next_cursor":next}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Refresh, ViewerCore};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        options: Options,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-catalog-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("proc")).unwrap();
            fs::write(root.join("proc/locks"), "").unwrap();
            let options = Options {
                claude_home: root.join("claude"),
                claude_json: root.join(".claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                since: Duration::from_secs(86400),
                session: None,
                facts: Some(root.join("facts.json")),
                scan_window: false,
            };
            crate::write_facts(
                options.facts.as_ref().unwrap(),
                &crate::Facts {
                    version: crate::FACTS_VERSION,
                    hostname: "synthetic-catalog".into(),
                    repos: std::collections::BTreeMap::from([(
                        "/synthetic/project".into(),
                        Some("project".into()),
                    )]),
                    ..crate::Facts::default()
                },
            )
            .unwrap();
            Self { root, options }
        }
        fn source(&self, id: &str) -> PathBuf {
            let path = self
                .options
                .claude_home
                .join(format!("projects/project/{id}.jsonl"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path,format!("{}\n",json!({"type":"user","sessionId":id,"uuid":id,"parentUuid":null,"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"synthetic prompt"}}))).unwrap();
            path
        }
        fn publish(&self, count: usize) {
            for id in 0..count {
                self.source(&format!("session-{id:05}"));
            }
            let mut core = ViewerCore::new(self.options.clone());
            core.set_refresh(Refresh::OnInvalidate);
            core.warm().unwrap();
            core.close();
        }
        fn body(&self, query: &str) -> (u16, Value) {
            let core = ViewerCore::new(self.options.clone());
            let reply = core.respond("GET", "/api/sessions", query, None);
            (reply.status, serde_json::from_slice(&reply.body).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn cursor_query(cursor: &Value) -> String {
        format!(
            "cursor={}",
            crate::viewer::percent_encode(cursor.as_str().unwrap())
        )
    }

    #[test]
    fn exact_source_proof_requires_current_native_mapping_and_consumed_ledger() {
        let fixture = Fixture::new();
        fixture.publish(100);
        let path = "projects/project/session-00000.jsonl";
        let proof = session_source_proof(&fixture.options, "claude", path)
            .unwrap()
            .unwrap();
        assert_eq!(proof.source.native_id, "session-00000");
        assert_eq!(proof.source.offset, proof.size);
        assert_eq!(proof.freshness.state, "cached");
        assert!(
            session_source_proof(&fixture.options, "claude", "projects/project/missing.jsonl")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            session_source_proof(&fixture.options, "claude", "../bad.jsonl"),
            Err(CatalogIdentityError::InvalidArguments)
        );
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT session_key FROM session_catalog_sources WHERE source_path=?1 ORDER BY session_key LIMIT 1",[fixture.options.claude_home.join(path).to_str().unwrap()],|row|row.get(3)).unwrap();
        assert!(plan.contains("session_catalog_source_path"), "{plan}");
        // Ledger progress without catalog publication must not reuse its former
        // native mapping or manufacture a proof for a new consumed boundary.
        connection
            .execute(
                "UPDATE files SET resume_at=resume_at-1 WHERE path=?1",
                [fixture.options.claude_home.join(path).to_str().unwrap()],
            )
            .unwrap();
        assert_eq!(
            session_source_proof(&fixture.options, "claude", path),
            Err(CatalogIdentityError::Unavailable)
        );
        connection
            .execute(
                "UPDATE files SET resume_at=resume_at+1 WHERE path=?1",
                [fixture.options.claude_home.join(path).to_str().unwrap()],
            )
            .unwrap();
        fs::remove_file(fixture.options.facts.as_ref().unwrap()).unwrap();
        assert_eq!(
            session_source_proof(&fixture.options, "claude", path),
            Err(CatalogIdentityError::Unavailable)
        );
    }

    #[test]
    fn exact_source_proof_retains_incomplete_prefix_and_rejects_source_replacement() {
        use sha2::Digest;
        let fixture = Fixture::new();
        let native = fixture.source("partial");
        let complete = fs::read(&native).unwrap();
        let mut bytes = complete.clone();
        bytes.extend_from_slice(b"{\"type\":\"user\"");
        fs::write(&native, &bytes).unwrap();
        let core = ViewerCore::new(fixture.options.clone());
        core.warm().unwrap();
        core.close();
        let proof =
            session_source_proof(&fixture.options, "claude", "projects/project/partial.jsonl")
                .unwrap()
                .unwrap();
        assert_eq!(proof.source.offset, complete.len() as u64);
        assert_eq!(proof.size, bytes.len() as u64);
        assert_eq!(proof.freshness.state, "incomplete");
        assert_eq!(
            proof.source.prefix_sha256,
            <[u8; 32]>::from(sha2::Sha256::digest(&complete))
        );
        let replacement = native.with_extension("replacement");
        fs::write(&replacement, &bytes).unwrap();
        fs::rename(replacement, native).unwrap();
        assert_eq!(
            session_source_proof(&fixture.options, "claude", "projects/project/partial.jsonl"),
            Err(CatalogIdentityError::Unavailable)
        );
    }

    #[test]
    fn exact_source_proof_work_stays_fixed_as_unrelated_native_manifest_grows() {
        let fixture = Fixture::new();
        fixture.publish(1);
        let mut facts = crate::read_facts(fixture.options.facts.as_ref().unwrap()).unwrap();
        let mut steps = Vec::new();
        for count in [100, 20_000] {
            facts.codex_rollouts = Some(
                (0..count)
                    .map(|index| format!("sessions/{index:08}.jsonl"))
                    .collect(),
            );
            crate::write_facts(fixture.options.facts.as_ref().unwrap(), &facts).unwrap();
            SQL_STEPS.with(|value| value.set(0));
            let proof = session_source_proof(
                &fixture.options,
                "claude",
                "projects/project/session-00000.jsonl",
            )
            .unwrap()
            .unwrap();
            assert_eq!(proof.source.native_id, "session-00000");
            steps.push(SQL_STEPS.with(|value| value.get()));
            let mut samples = Vec::new();
            for _ in 0..31 {
                let start = std::time::Instant::now();
                assert!(
                    session_source_proof(
                        &fixture.options,
                        "claude",
                        "projects/project/session-00000.jsonl"
                    )
                    .unwrap()
                    .is_some()
                );
                samples.push(start.elapsed().as_micros());
            }
            samples.sort_unstable();
            eprintln!(
                "exact source proof manifest={count} facts_bytes={} median_us={} samples=31",
                fs::metadata(fixture.options.facts.as_ref().unwrap())
                    .unwrap()
                    .len(),
                samples[15]
            );
        }
        assert_eq!(steps[0], steps[1]);
        eprintln!("exact source proof selected catalog VM steps={steps:?}");
    }

    #[test]
    fn scoped_catalog_identity_preserves_native_ambiguity_and_source_authority() {
        let fixture = Fixture::new();
        fixture.publish(2);
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let selected = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
            .unwrap()
            .unwrap();
        assert_eq!(selected.source_key, "stable-source");
        assert_eq!(selected.native_id.as_deref(), Some("session-00000"));
        assert_eq!(selected.freshness.state, "cached");
        assert_eq!(selected.source_refs.len(), 1);
        assert_eq!(selected.machine_label.as_deref(), Some("synthetic-catalog"));
        assert!(
            session_catalog_identity(&fixture.options, "stable-source", "missing")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            session_catalog_identity(&fixture.options, "", "session-00000"),
            Err(CatalogIdentityError::InvalidArguments)
        );
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let read = |key: &str| -> Value {
            let text: String = connection
                .query_row(
                    "SELECT metadata FROM session_catalog WHERE session_key=?1",
                    [key],
                    |row| row.get(0),
                )
                .unwrap();
            serde_json::from_str(&text).unwrap()
        };
        // Model a published multi-native continuation using two valid scoped
        // source ledgers. No arbitrary first native target may become authority.
        let mut row = read("session-00000");
        let second = read("session-00001");
        row["sources"]
            .as_array_mut()
            .unwrap()
            .extend(second["sources"].as_array().unwrap().iter().cloned());
        row["native_ids"] = json!(["session-00000", "session-00001"]);
        connection
            .execute(
                "UPDATE session_catalog SET metadata=?1 WHERE session_key='session-00000'",
                [row.to_string()],
            )
            .unwrap();
        let ambiguous =
            session_catalog_identity(&fixture.options, "stable-source", "session-00000")
                .unwrap()
                .unwrap();
        assert_eq!(ambiguous.native_id, None);
        assert_eq!(ambiguous.native_ids, ["session-00000", "session-00001"]);
        fs::remove_file(
            fixture
                .options
                .claude_home
                .join("projects/project/session-00000.jsonl"),
        )
        .unwrap();
        let missing = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
            .unwrap()
            .unwrap();
        assert_eq!(missing.freshness.state, "incomplete");
        assert_eq!(missing.source_refs[0].state, "unavailable");
        assert_eq!(missing.facts_observation.state, "cached");
        assert_eq!(missing.native_selection, None);
        let mut changed = fixture.options.clone();
        changed.claude_home = fixture.root.join("different-root");
        assert_eq!(
            session_catalog_identity(&changed, "stable-source", "session-00000"),
            Err(CatalogIdentityError::ScopeChanged)
        );
        crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
    }

    #[test]
    fn focused_catalog_preserves_missing_facts_and_unknown_native_manifest() {
        let fixture = Fixture::new();
        let relative = "sessions/2026/10/01/rollout-codex-session.jsonl";
        let path = fixture.options.codex_home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{}\n", json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"codex-session","cwd":"/synthetic/project"}}))).unwrap();
        fixture.publish(1);
        let facts_path = fixture.options.facts.as_ref().unwrap();
        let mut facts = crate::read_facts(facts_path).unwrap();
        let read = || session_catalog_identity(&fixture.options, "stable-source", "codex-session");
        // Legacy None means unknown selection, not current source authority.
        assert!(facts.codex_rollouts.is_none());
        assert_eq!(read().unwrap().unwrap().freshness.state, "incomplete");
        facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
        crate::write_facts(facts_path, &facts).unwrap();
        assert_eq!(read().unwrap().unwrap().freshness.state, "cached");
        facts.codex_rollouts = Some(std::collections::BTreeSet::new());
        crate::write_facts(facts_path, &facts).unwrap();
        assert_eq!(read(), Err(CatalogIdentityError::ScopeChanged));
        fs::write(facts_path, b"corrupt facts").unwrap();
        let corrupt = read().unwrap().unwrap();
        assert_eq!(corrupt.freshness.state, "unavailable");
        assert_eq!(corrupt.machine_label, None);
        assert_eq!(corrupt.facts_observation.state, "unavailable");
        assert_eq!(corrupt.native_selection.unwrap().state, "unavailable");
        assert_eq!(corrupt.source_refs[0].state, "cached");
        assert_eq!(
            fixture.body("sid=session-00000").1["items"][0]["freshness"]["state"],
            "unavailable"
        );
        fs::remove_file(facts_path).unwrap();
        assert_eq!(read().unwrap().unwrap().freshness.state, "unavailable");
        crate::write_facts(facts_path, &facts).unwrap();
        facts.version = 999;
        crate::write_facts(facts_path, &facts).unwrap();
        assert_eq!(read().unwrap().unwrap().freshness.state, "unavailable");
    }

    #[test]
    fn unavailable_content_does_not_erase_current_machine_and_native_selection_observation() {
        let fixture = Fixture::new();
        let relative = "sessions/2026/10/01/rollout-codex-session.jsonl";
        let path = fixture.options.codex_home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{}\n", json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"codex-session","cwd":"/synthetic/project"}}))).unwrap();
        fixture.publish(1);
        let facts_path = fixture.options.facts.as_ref().unwrap();
        let mut facts = crate::read_facts(facts_path).unwrap();
        facts.codex_rollouts = Some(std::collections::BTreeSet::from([relative.to_owned()]));
        crate::write_facts(facts_path, &facts).unwrap();
        fs::remove_file(path).unwrap();
        let read = || session_catalog_identity(&fixture.options, "stable-source", "codex-session");
        let missing = read().unwrap().unwrap();
        assert_eq!(missing.freshness.state, "unavailable");
        assert_eq!(missing.source_refs[0].state, "unavailable");
        assert_eq!(missing.facts_observation.state, "cached");
        assert_eq!(missing.native_selection.unwrap().state, "cached");
        facts.codex_rollouts = None;
        crate::write_facts(facts_path, &facts).unwrap();
        assert_eq!(
            read().unwrap().unwrap().native_selection.unwrap().state,
            "incomplete"
        );
        facts.codex_rollouts = Some(std::collections::BTreeSet::new());
        crate::write_facts(facts_path, &facts).unwrap();
        assert_eq!(read(), Err(CatalogIdentityError::ScopeChanged));
    }

    #[test]
    fn focused_cold_reads_are_indexed_metadata_and_coherent_keysets() {
        let fixture = Fixture::new();
        fixture.publish(9);
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let (status, first) = fixture.body("limit=3");
        assert_eq!(status, 200);
        assert_eq!(first["freshness"], "cached");
        assert_eq!(first["refresh"]["state"], "manual");
        let first_items = first["items"].as_array().unwrap();
        assert_eq!(first_items.len(), 3);
        assert_eq!(first_items[0]["key"], "session-00000");
        assert_eq!(first_items[2]["key"], "session-00002");
        assert!(
            first_items
                .iter()
                .all(|row| row["freshness"]["state"] == "cached"
                    && row.get("sources").is_none()
                    && row.get("activity").is_none())
        );
        let (status, second) =
            fixture.body(&format!("limit=3&{}", cursor_query(&first["next_cursor"])));
        assert_eq!(status, 200);
        assert_eq!(second["items"][0]["key"], "session-00003");
        assert_eq!(second["items"][2]["key"], "session-00005");
        let (status, last) =
            fixture.body(&format!("limit=3&{}", cursor_query(&second["next_cursor"])));
        assert_eq!(status, 200);
        assert_eq!(last["items"][2]["key"], "session-00008");
        assert!(last["next_cursor"].is_null());
        let (status, wrong_scope) = fixture.body(&format!(
            "harness=claude&{}",
            cursor_query(&first["next_cursor"])
        ));
        assert_eq!(status, 400);
        assert_eq!(wrong_scope["error"]["code"], "cursor_scope_mismatch");
        assert_eq!(fixture.body("harness=codex").1["items"], json!([]));
        let (status, selected) = fixture.body("sid=session-00008");
        assert_eq!(status, 200);
        assert_eq!(selected["items"].as_array().unwrap().len(), 1);
        assert_eq!(selected["items"][0]["key"], "session-00008");
        assert_eq!(fixture.body("sid=absent").0, 404);
        assert_eq!(fixture.body("sid=session-00008&harness=claude").0, 400);
        assert_eq!(
            fixture.body("q=prompt").1["error"]["code"],
            "unsupported_filter"
        );
        assert_eq!(
            fixture.body("harness=claude&repo=project").1["items"]
                .as_array()
                .unwrap()
                .len(),
            9
        );
        crate::events::CACHE_READS.with(|reads| {
            assert_eq!(
                reads.get(),
                0,
                "catalog requests may not load the full event index"
            )
        });

        // An append/republication moves the generation; the old page cannot
        // silently select a changed logical or source snapshot.
        fixture.source("new-session");
        let core = ViewerCore::new(fixture.options.clone());
        core.warm().unwrap();
        core.close();
        let (status, stale) = fixture.body(&cursor_query(&first["next_cursor"]));
        assert_eq!(status, 409);
        assert_eq!(stale["error"]["code"], "stale_cursor");
        assert_eq!(stale["reset_cursor"], true);
    }

    #[test]
    fn focused_catalog_preserves_source_disappearance_and_access_boundaries() {
        let fixture = Fixture::new();
        fixture.publish(2);
        let source = fixture.source("session-00000");
        assert_eq!(
            fixture.body("limit=1").1["items"][0]["freshness"]["state"],
            "stale"
        );
        fs::remove_file(&source).unwrap();
        let (status, missing) = fixture.body("limit=1");
        assert_eq!(status, 200);
        assert_eq!(missing["items"][0]["freshness"]["state"], "unavailable");
        assert_eq!(
            missing["items"][0]["source_refs"][0]["state"],
            "unavailable"
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", &source).unwrap();
            assert_eq!(
                fixture.body("limit=1").1["items"][0]["freshness"]["state"],
                "unavailable"
            );
        }
        let mut changed = fixture.options.clone();
        changed.claude_home = fixture.root.join("another-claude");
        let core = ViewerCore::new(changed);
        let reply = core.respond("GET", "/api/sessions", "limit=1", None);
        assert_eq!(reply.status, 503);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["error"]["code"], "catalog_scope_changed");
        assert!(
            body.get("items").is_none(),
            "metadata from a prior configuration is not source authority"
        );
    }

    #[test]
    fn focused_catalog_requires_an_unambiguous_machine_scope_and_bounded_arguments() {
        let fixture = Fixture::new();
        fixture.publish(1);
        let core = ViewerCore::with_machines(vec![
            ("one".into(), fixture.options.clone()),
            ("two".into(), fixture.options.clone()),
        ]);
        let body = |query| {
            let reply = core.respond("GET", "/api/sessions", query, None);
            (
                reply.status,
                serde_json::from_slice::<Value>(&reply.body).unwrap(),
            )
        };
        assert_eq!(body("").1["error"]["code"], "machine_scope_required");
        assert_eq!(body("machine=one").0, 200);
        assert_eq!(body("machine=unknown").0, 404);
        for query in [
            "limit=0",
            "limit=101",
            "limit=-1",
            "harness=unknown",
            "repo=",
            "cursor=nope",
            "repo=%FF",
        ] {
            assert_eq!(body(query).0, 400, "{query}");
        }
        let empty = Fixture::new();
        assert_eq!(empty.body("").0, 503);
        assert!(
            !EventCache::path(&empty.options.cache).exists(),
            "a read-only catalog miss must not create an index"
        );
    }

    #[test]
    fn focused_catalog_demand_publishes_without_waiting_for_an_initial_model() {
        let fixture = Fixture::new();
        fixture.source("first");
        let mut core = ViewerCore::new(fixture.options.clone());
        core.set_refresh(Refresh::Background);
        let first = core.respond("GET", "/api/sessions", "", None);
        assert!(matches!(first.status, 200 | 503));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let response = core.respond("GET", "/api/sessions", "", None);
            if response.status == 200 {
                let body: Value = serde_json::from_slice(&response.body).unwrap();
                assert_eq!(body["items"][0]["key"], "first");
                assert_eq!(body["refresh"]["state"], "updating");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "catalog demand must start initial background publication"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        core.close();
    }

    #[test]
    fn focused_catalog_keyset_server_work_is_constant_with_unrelated_history() {
        let fixture = Fixture::new();
        fixture.publish(100);
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let request = Request::parse("limit=3&harness=claude&repo=project")
            .ok()
            .unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let small = rows(&connection, &request).unwrap();
        let small_steps = SQL_STEPS.with(|steps| steps.get());
        // Add unrelated, equal-time metadata to expose an accidental OFFSET
        // or whole-catalog preparation without reading any transcript bodies.
        let encoded: String = connection
            .query_row("SELECT metadata FROM session_catalog LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let mut row: CatalogRow = serde_json::from_str(&encoded).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        for id in 100..20_000 {
            row.key = format!("session-{id:05}");
            transaction.execute("INSERT INTO session_catalog(session_key,last_ms,harness,repo,metadata)VALUES(?1,?2,?3,?4,?5)",rusqlite::params![row.key,row.last.unwrap_or(0),row.harness,row.repo,serde_json::to_string(&row).unwrap()]).unwrap();
        }
        transaction.commit().unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let large = rows(&connection, &request).unwrap();
        let large_steps = SQL_STEPS.with(|steps| steps.get());
        assert_eq!(
            serde_json::to_value(small).unwrap(),
            serde_json::to_value(large).unwrap()
        );
        assert_eq!(small_steps, large_steps);
        // A late cursor in a huge equal-time group must seek its key directly.
        let request = Request {
            limit: 3,
            sid: None,
            harness: Some("claude".into()),
            repo: Some("project".into()),
            cursor: Some(Cursor {
                version: 1,
                generation: "0".repeat(64),
                machine: "".into(),
                harness: Some("claude".into()),
                repo: Some("project".into()),
                last: row.last.unwrap_or(0),
                key: "session-19000".into(),
            }),
        };
        SQL_STEPS.with(|steps| steps.set(0));
        let late = rows(&connection, &request).unwrap();
        let late_steps = SQL_STEPS.with(|steps| steps.get());
        assert_eq!(late[0].key, "session-19001");
        assert!(
            late_steps < 1000,
            "a late equal-time cursor must not scan earlier metadata: {late_steps}"
        );
        println!(
            "focused catalog SQL VM steps: 100 rows={small_steps}, 20000 rows={large_steps}, late cursor={late_steps}"
        );
    }
}
