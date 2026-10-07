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

/// Read intent is separate from native control/source-selection authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogReadScope {
    #[default]
    Current,
    RetainedHistory,
}

/// Scoped cached identity, resolved without global event/model construction.
/// A nullable native_id preserves ambiguity in multi-native continuations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSessionIdentity {
    pub source_key: String,
    pub read_scope: CatalogReadScope,
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
    catalog_identity(
        options,
        source_key,
        canonical_catalog_key,
        CatalogReadScope::Current,
    )
}

/// Resolve retained history without treating retired native selection as current.
/// Hosts must independently authorize history access and verify immutable source
/// generations when providing bytes. This identity never grants control access.
pub fn session_catalog_history_identity(
    options: &Options,
    source_key: &str,
    canonical_catalog_key: &str,
) -> Result<Option<CatalogSessionIdentity>, CatalogIdentityError> {
    catalog_identity(
        options,
        source_key,
        canonical_catalog_key,
        CatalogReadScope::RetainedHistory,
    )
}

fn catalog_identity(
    options: &Options,
    source_key: &str,
    canonical_catalog_key: &str,
    scope: CatalogReadScope,
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
        scope,
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
        read_scope: scope,
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
    #[serde(default)]
    scope: CatalogReadScope,
    version: u32,
    generation: String,
    machine: String,
    harness: Option<String>,
    repo: Option<String>,
    last: i64,
    key: String,
}

pub(crate) struct Request {
    scope: CatalogReadScope,
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
        let scope = match parameter("scope")?.as_deref() {
            None | Some("current") => CatalogReadScope::Current,
            Some("retained_history") => CatalogReadScope::RetainedHistory,
            _ => return Err(invalid()),
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
            scope,
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
    let (filters, index) = match request.scope {
        CatalogReadScope::Current => (
            format!("lifecycle='current' AND {filters}"),
            index.replacen("session_catalog_", "session_catalog_current_", 1),
        ),
        CatalogReadScope::RetainedHistory => (filters.to_owned(), index.to_owned()),
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
            &index,
            &filters,
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
                &index,
                &filters,
                "last_ms < ?",
                args,
                remaining,
            )?);
        }
        found
    } else {
        read_range(connection, &index, &filters, "1", base, limit)?
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
        cursor.scope != request.scope
            || cursor.machine != machine
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
                    scope: request.scope,
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
    let mut page_state = if facts_known || request.scope == CatalogReadScope::RetainedHistory {
        "cached"
    } else {
        "unavailable"
    };
    let mut items = Vec::with_capacity(selected.len());
    for row in selected {
        let mut retired_native = false;
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
            retired_native |= retired;
            if retired && request.scope == CatalogReadScope::Current {
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
        let state = if (request.scope == CatalogReadScope::Current && !facts_known)
            || states.iter().all(|state| *state == "unavailable")
        {
            "unavailable"
        } else if states.contains(&"unavailable")
            || states.contains(&"incomplete")
            || (request.scope == CatalogReadScope::Current
                && row.harness == "codex"
                && !codex_selection_known)
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
            json!({"state":if !facts_known { "unavailable" } else if retired_native { "retired" } else if codex_selection_known { "cached" } else { "incomplete" }})
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
        json!({"api":1,"machine":machine,"read_scope":request.scope,"generation":generation,"observed_at":observed_at,
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
    fn current_catalog_index_work_is_constant_with_unrelated_retained_history() {
        let fixture = Fixture::new();
        fixture.publish(1);
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let query = "SELECT metadata FROM session_catalog INDEXED BY session_catalog_current_harness_repo WHERE lifecycle='current' AND harness='claude' AND repo='project' ORDER BY last_ms DESC,session_key ASC LIMIT 3";
        let measure = || {
            let mut statement = connection.prepare(query).unwrap();
            let values = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (
                values,
                statement.get_status(rusqlite::StatementStatus::VmStep),
            )
        };
        connection.execute("INSERT INTO session_catalog(session_key,lifecycle,last_ms,harness,repo,metadata) VALUES('initial-retained','retained',0,'claude','project','{}')", []).unwrap();
        let small = measure();
        let transaction = connection.unchecked_transaction().unwrap();
        for id in 0..20_000 {
            transaction.execute("INSERT INTO session_catalog(session_key,lifecycle,last_ms,harness,repo,metadata) VALUES(?1,'retained',0,'claude','project','{}')", [format!("retained-{id:05}")]).unwrap();
        }
        transaction.commit().unwrap();
        let large = measure();
        assert_eq!(small, large);
        let plan: String = connection
            .query_row(&format!("EXPLAIN QUERY PLAN {query}"), [], |row| row.get(3))
            .unwrap();
        assert!(plan.contains("session_catalog_current_harness_repo"));
        assert!(!plan.contains("SCAN"));
        eprintln!(
            "current catalog SQL VM steps: one retained={}, 20001 retained={}",
            small.1, large.1
        );
    }

    #[test]
    fn schema_ten_retention_migration_binds_only_coherent_supported_recipes() {
        for coherent in [true, false] {
            let fixture = Fixture::new();
            fixture.publish(1);
            let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
            let before: String = connection
                .query_row(
                    "SELECT generation FROM session_slot_projections",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            connection.execute_batch("DROP INDEX session_catalog_current_order; DROP INDEX session_catalog_current_harness; DROP INDEX session_catalog_current_repo; DROP INDEX session_catalog_current_harness_repo; DROP INDEX session_catalog_current_parent; DROP INDEX session_catalog_current_native; DROP INDEX session_catalog_current_source_path; ALTER TABLE session_catalog DROP COLUMN lifecycle; ALTER TABLE session_catalog_sources DROP COLUMN lifecycle; ALTER TABLE session_slot_projections DROP COLUMN source_generation; UPDATE session_catalog SET metadata=json_remove(metadata,'$.lifecycle'); PRAGMA user_version=10;").unwrap();
            if !coherent {
                connection.execute("UPDATE meta SET value='stale' WHERE key='slot_projection_catalog_generation'", []).unwrap();
            }
            drop(EventCache::open(&fixture.options.cache));
            let (after, source): (String, Option<String>) = connection
                .query_row(
                    "SELECT generation,source_generation FROM session_slot_projections",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(before, after);
            assert_eq!(source.is_some(), coherent);
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                    .unwrap(),
                11
            );
        }
    }

    #[test]
    fn retained_projection_survives_parser_reset_and_source_reappearance() {
        let fixture = Fixture::new();
        fixture.publish(1);
        let relative = "projects/project/session-00000.jsonl";
        let before = crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .unwrap();
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        fs::remove_file(fixture.options.claude_home.join(relative)).unwrap();
        connection
            .execute("UPDATE meta SET value='0' WHERE key='cache_version'", [])
            .unwrap();
        let core = ViewerCore::new(fixture.options.clone());
        core.warm().unwrap();
        core.close();
        let retained = crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .unwrap();
        assert_eq!(retained.lifecycle, "retained");
        assert_eq!(before.source, retained.source);
        assert_eq!(before.projection_generation, retained.projection_generation);
        fixture.source("session-00000");
        let core = ViewerCore::new(fixture.options.clone());
        core.warm().unwrap();
        core.close();
        let current = crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .unwrap();
        assert_eq!(current.lifecycle, "current");
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM session_catalog_sources WHERE lifecycle='current'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn source_projection_readiness_defers_oversized_metadata_and_source_context() {
        let fixture = Fixture::new();
        fixture.publish(1);
        let relative = "projects/project/session-00000.jsonl";
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let encoded: String = connection
            .query_row("SELECT metadata FROM session_catalog", [], |row| row.get(0))
            .unwrap();
        let mut row: CatalogRow = serde_json::from_str(&encoded).unwrap();
        row.name = "x".repeat(1024 * 1024);
        connection
            .execute(
                "UPDATE session_catalog SET metadata=?1",
                [serde_json::to_string(&row).unwrap()],
            )
            .unwrap();
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_none()
        );
        row = serde_json::from_str(&encoded).unwrap();
        row.sources = vec![row.sources[0].clone(); 65];
        connection
            .execute(
                "UPDATE session_catalog SET metadata=?1",
                [serde_json::to_string(&row).unwrap()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE session_slot_projections SET source_generation=?1",
                [crate::retention::source_generation(&row.sources).unwrap()],
            )
            .unwrap();
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn source_projection_readiness_never_initializes_and_rejects_stale_headers() {
        let fixture = Fixture::new();
        let relative = "projects/project/session-00000.jsonl";
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_none()
        );
        assert!(!EventCache::path(&fixture.options.cache).exists());
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", "../outside.jsonl").is_err()
        );
        fixture.publish(1);
        let ready = crate::source_projection_ready(&fixture.options, "claude", relative)
            .unwrap()
            .unwrap();
        assert_eq!(ready.projection_version, crate::slot_projection::VERSION);
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        connection.execute("INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at) VALUES(?1,1,'changed',0)", [fixture.options.claude_home.join(relative).to_str().unwrap()]).unwrap();
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_none()
        );
        connection
            .execute("DELETE FROM session_catalog_invalidations", [])
            .unwrap();
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_some()
        );
        let hash:String=connection.query_row("SELECT source_generation FROM session_slot_projections WHERE session_key='session-00000'",[],|row|row.get(0)).unwrap();
        connection
            .execute(
                "UPDATE session_slot_projections SET source_generation=NULL",
                [],
            )
            .unwrap();
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_none()
        );
        connection
            .execute(
                "UPDATE session_slot_projections SET source_generation=?1,version=version+1",
                [hash],
            )
            .unwrap();
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_none()
        );
        connection
            .execute("UPDATE session_slot_projections SET version=version-1", [])
            .unwrap();
        connection.execute("UPDATE meta SET value='not-coherent' WHERE key='slot_projection_catalog_generation'",[]).unwrap();
        assert!(
            crate::source_projection_ready(&fixture.options, "claude", relative)
                .unwrap()
                .is_none()
        );
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
        let history =
            session_catalog_history_identity(&fixture.options, "stable-source", "codex-session")
                .unwrap()
                .unwrap();
        assert_eq!(history.read_scope, CatalogReadScope::RetainedHistory);
        assert_eq!(history.native_selection.unwrap().state, "retired");
        assert_eq!(history.source_refs[0].source.native_id, "codex-session");
        assert_eq!(history.source_refs[0].state, "cached");
        let (status, page) = fixture.body("scope=retained_history&sid=codex-session");
        assert_eq!(status, 200);
        assert_eq!(page["read_scope"], "retained_history");
        assert_eq!(page["items"][0]["native_selection"]["state"], "retired");

        fs::write(facts_path, b"corrupt facts").unwrap();
        let corrupt = read().unwrap().unwrap();
        assert_eq!(corrupt.freshness.state, "unavailable");
        assert_eq!(corrupt.machine_label, None);
        assert_eq!(corrupt.facts_observation.state, "unavailable");
        assert_eq!(corrupt.native_selection.unwrap().state, "unavailable");
        assert_eq!(corrupt.source_refs[0].state, "cached");
        let history =
            session_catalog_history_identity(&fixture.options, "stable-source", "codex-session")
                .unwrap()
                .unwrap();
        assert_eq!(history.freshness.state, "cached");
        assert_eq!(history.facts_observation.state, "unavailable");
        assert_eq!(history.native_selection.unwrap().state, "unavailable");
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
    fn cursors_bind_retained_read_intent_and_unknown_scope_is_rejected() {
        let fixture = Fixture::new();
        fixture.publish(3);
        let (status, first) = fixture.body("limit=1&scope=retained_history");
        assert_eq!(status, 200);
        let cursor = crate::viewer::percent_encode(first["next_cursor"].as_str().unwrap());
        assert_eq!(fixture.body(&format!("limit=1&cursor={cursor}")).0, 400);
        assert_eq!(
            fixture
                .body(&format!("limit=1&scope=retained_history&cursor={cursor}"))
                .0,
            200
        );
        assert!(Request::parse("scope=unknown").is_err());
        let current = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
            .unwrap()
            .unwrap();
        assert_eq!(current.read_scope, CatalogReadScope::Current);
    }

    #[test]
    fn selected_slot_ranges_are_native_bounded_and_stable_across_unrelated_history() {
        let fixture = Fixture::new();
        let path = fixture.source("selected");
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file,"{}",json!({"type":"assistant","sessionId":"selected","uuid":"answer-1","parentUuid":"selected","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"selected answer unique body"}]}})).unwrap();
        writeln!(file,"{}",json!({"type":"assistant","sessionId":"selected","uuid":"large-answer","parentUuid":"answer-1","timestamp":"2026-10-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"large answer unique body ".repeat(16000)}]}})).unwrap();
        drop(file);
        fixture.publish(2);
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let get = |query: &str| {
            let reply = crate::session_transcript_range(&fixture.options, "source", query, None);
            (
                reply.status,
                serde_json::from_slice::<Value>(&reply.body).unwrap(),
            )
        };
        let (status, first) = get("sid=selected&after=0&limit=2");
        assert_eq!(status, 200, "{first}");
        assert_eq!(first["range"], json!({"first":0,"end":2,"next":2}));
        assert_eq!(first["entries"][0]["k"], "u");
        assert_eq!(first["entries"][0]["text"], "synthetic prompt");
        assert_eq!(first["entries"][1]["text"], "selected answer unique body");
        assert_eq!(
            first["entries"][1]["provenance"]["native_event_id"],
            "answer-1"
        );
        let (_, latest) = get("sid=selected&limit=1");
        assert_eq!(latest["range"]["first"], 2);
        assert_eq!(latest["freshness"]["state"], "cached");
        assert_eq!(latest["entries"][0]["clipped"], true);
        assert!(latest["observation"]["source_bytes"].as_u64().unwrap() <= 4096 + 11);
        let generation = latest["projection"]["generation"].as_str().unwrap();
        let chunks = latest["entries"][0]["field"]["chunks"].as_u64().unwrap();
        let mut expanded = String::new();
        for chunk in 0..chunks {
            let query =
                format!("sid=selected&after=2&limit=1&generation={generation}&field_chunk={chunk}");
            let reply = crate::session_entry_field(&fixture.options, "source", &query, None);
            assert_eq!(reply.status, 200);
            let body: Value = serde_json::from_slice(&reply.body).unwrap();
            assert!(body["observation"]["source_bytes"].as_u64().unwrap() <= 64 * 1024 + 11);
            expanded.push_str(body["text"].as_str().unwrap());
        }
        assert_eq!(expanded, "large answer unique body ".repeat(16000));
        assert_eq!(get("sid=selected&after=%GG").0, 400);
        assert_eq!(get("sid=selected&scope=unknown").0, 400);
        assert_eq!(get("sid=selected&generation=wrong").0, 409);
        assert_eq!(get("sid=selected&after=99").0, 400);
        assert_eq!(get("sid=missing").0, 404);
        crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
        fixture.publish(50);
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let (_, unchanged) = get("sid=selected&after=0&limit=2");
        assert_eq!(
            first["projection"]["generation"],
            unchanged["projection"]["generation"]
        );
        assert_eq!(
            first["entries"][1]["entry_id"],
            unchanged["entries"][1]["entry_id"]
        );
        crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let leaked:i64=connection.query_row("SELECT COUNT(*) FROM session_slots WHERE metadata LIKE '%selected answer unique body%' OR metadata LIKE '%large answer unique body%'",[],|row|row.get(0)).unwrap();
        assert_eq!(leaked, 0);
        let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT metadata FROM session_slots WHERE session_key='selected' AND slot>=0 AND slot<2 ORDER BY slot",[],|row|row.get(3)).unwrap();
        assert!(
            plan.contains("SEARCH session_slots USING PRIMARY KEY"),
            "{plan}"
        );
    }

    #[test]
    fn selected_provider_ranges_render_native_records_without_restoring_sources() {
        let fixture = Fixture::new();
        let path = fixture.source("archived");
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file,"{}",json!({"type":"assistant","sessionId":"archived","uuid":"archived-answer","parentUuid":"archived","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"retained native answer"}]}})).unwrap();
        drop(file);
        fixture.publish(1);
        let bytes = fs::read(&path).unwrap();
        let ready = crate::source_projection_ready(
            &fixture.options,
            "claude",
            "projects/project/archived.jsonl",
        )
        .unwrap()
        .unwrap();
        assert_eq!(ready.lifecycle, "current");
        fs::remove_file(&path).unwrap();
        // A closed/reopened background model retires the native index. History
        // metadata and immutable native recipe generation must survive it.
        let reopened = ViewerCore::new(fixture.options.clone());
        reopened.warm().unwrap();
        reopened.close();
        let retained = crate::source_projection_ready(
            &fixture.options,
            "claude",
            "projects/project/archived.jsonl",
        )
        .unwrap()
        .unwrap();
        assert_eq!(retained.lifecycle, "retained");
        assert_eq!(retained.source, ready.source);
        assert_eq!(retained.projection_generation, ready.projection_generation);
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let removed: i64 = connection
            .query_row(
                "SELECT count(*) FROM files WHERE path=?1",
                [path.to_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            removed, 0,
            "retained provenance must not pretend to be a current file ledger"
        );
        let retired:i64=connection.query_row("SELECT count(*) FROM session_catalog_sources WHERE session_key='archived' AND lifecycle='retained'",[],|row|row.get(0)).unwrap();
        assert_eq!(retired, 1);
        struct Provider {
            bytes: Vec<u8>,
            changed: bool,
            calls: std::sync::atomic::AtomicUsize,
        }
        impl crate::SessionSourceReader for Provider {
            fn read_range(
                &self,
                source: &SessionSourceRef,
                expected: Option<&str>,
                offset: u64,
                max: usize,
            ) -> io::Result<crate::SessionSourceRange> {
                assert_eq!(source.root, "claude");
                assert_eq!(source.native_id, "archived");
                assert_eq!(source.path, "projects/project/archived.jsonl");
                assert_eq!(source.offset, self.bytes.len() as u64);
                use sha2::{Digest, Sha256};
                let full: [u8; 32] = Sha256::digest(&self.bytes).into();
                assert_eq!(source.prefix_sha256, full);
                let generation: String = full.iter().map(|byte| format!("{byte:02x}")).collect();
                if let Some(expected) = expected {
                    assert_eq!(expected, generation);
                }
                let call = self.calls.fetch_add(1, Ordering::Relaxed);
                let offset = usize::try_from(offset).unwrap();
                Ok(crate::SessionSourceRange {
                    generation: if self.changed && call > 0 {
                        "changed".into()
                    } else {
                        generation
                    },
                    length: self.bytes.len() as u64,
                    offset: offset as u64,
                    bytes: std::sync::Arc::from(
                        &self.bytes[offset..offset.saturating_add(max).min(self.bytes.len())],
                    ),
                    cached: true,
                })
            }
        }
        let provider = Provider {
            bytes: bytes.clone(),
            changed: false,
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        crate::events::CACHE_READS.with(|reads| reads.set(0));
        let reply = crate::session_transcript_range(
            &fixture.options,
            "source",
            "sid=archived&after=0&limit=2",
            Some(&provider),
        );
        assert_eq!(reply.status, 200);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["entries"][1]["text"], "retained native answer");
        assert_eq!(body["identity"]["freshness"]["state"], "unavailable");
        assert_eq!(body["entries"][1]["freshness"]["state"], "cached");
        assert_eq!(provider.calls.load(Ordering::Relaxed), 2);
        assert!(!path.exists());
        let changed = Provider {
            bytes,
            changed: true,
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let reply = crate::session_transcript_range(
            &fixture.options,
            "source",
            "sid=archived&after=0&limit=2",
            Some(&changed),
        );
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(body["freshness"]["state"], "incomplete");
        assert_eq!(body["entries"][1]["freshness"]["state"], "incomplete");
        crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
        assert!(!path.exists());
    }

    #[test]
    fn source_observer_publishes_first_push_without_a_viewer_core() {
        let fixture = Fixture::new();
        fixture.source("first-push");
        let observer = crate::SessionCatalogObserver::new(fixture.options.clone());
        assert!(!EventCache::path(&fixture.options.cache).exists());
        observer.changed().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let reply = session_catalog_page(&fixture.options, "configured", "sid=first-push");
            if reply.status == 200 {
                let body: Value = serde_json::from_slice(&reply.body).unwrap();
                if body["items"]
                    .as_array()
                    .is_some_and(|items| items.len() == 1)
                {
                    assert_eq!(body["items"][0]["key"], "first-push");
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "source publication did not complete"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        observer.close();
        assert!(EventCache::path(&fixture.options.cache).exists());
    }

    #[test]
    fn default_local_viewer_advertises_a_nonempty_stable_catalog_source() {
        let fixture = Fixture::new();
        fixture.publish(1);
        let core = ViewerCore::new(fixture.options.clone());
        let inventory = core.respond("GET", "/api/session-sources", "limit=1", None);
        assert_eq!(inventory.status, 200);
        let inventory: Value = serde_json::from_slice(&inventory.body).unwrap();
        assert_eq!(
            inventory["items"],
            serde_json::json!([{"source_key":"local","label":"local"}])
        );
        assert!(inventory["next_cursor"].is_null());
        let capability = core.respond("GET", "/api/session-capabilities", "", None);
        assert_eq!(capability.status, 200);
        let body: Value = serde_json::from_slice(&capability.body).unwrap();
        assert_eq!(body["source_key"], "local");
        for (path, query) in [
            ("/api/session-identity", "sid=session-00000"),
            (
                "/api/session-transcript",
                "machine=local&sid=session-00000&after=0&limit=1",
            ),
        ] {
            let reply = core.respond("GET", path, query, None);
            assert_eq!(
                reply.status,
                200,
                "{}",
                String::from_utf8_lossy(&reply.body)
            );
            let body: Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(body["identity"]["source_key"], "local");
        }
        let reply = core.respond(
            "GET",
            "/api/sessions",
            "machine=local&sid=session-00000",
            None,
        );
        assert_eq!(reply.status, 200);
        let body: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["machine"], "local");
        assert_eq!(
            core.respond(
                "GET",
                "/api/session-identity",
                "machine=wrong&sid=session-00000",
                None
            )
            .status,
            404
        );
    }

    #[test]
    fn native_tool_outputs_use_bounded_string_fields_and_preserve_call_context() {
        for harness in [
            "claude",
            "claude-block",
            "codex",
            "codex-encoded",
            "codex-object",
        ] {
            let fixture = Fixture::new();
            let body = "native tool output line \"quoted\" \n".repeat(15000);
            let key = if harness.starts_with("claude") {
                "claude-tools"
            } else {
                "codex-tools"
            };
            let (path, records) = if harness.starts_with("claude") {
                let content = if harness == "claude-block" {
                    json!([{"type":"text","text":body}])
                } else {
                    json!(body)
                };
                (
                    fixture.source(key),
                    vec![
                        json!({"type":"assistant","sessionId":key,"uuid":"native-call","parentUuid":key,"timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"native-tool","name":"Bash","input":{"command":"printf fixture"}}]}}),
                        json!({"type":"user","sessionId":key,"uuid":"native-result","parentUuid":"native-call","timestamp":"2026-10-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"native-tool","content":content}]}}),
                    ],
                )
            } else {
                let relative = "sessions/2026/10/01/rollout-codex-tools.jsonl";
                let path = fixture.options.codex_home.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                let facts_path = fixture.options.facts.as_ref().unwrap();
                let mut facts = crate::read_facts(facts_path).unwrap();
                facts.codex_rollouts =
                    Some(std::collections::BTreeSet::from([relative.to_owned()]));
                crate::write_facts(facts_path, &facts).unwrap();
                let output = match harness {
                    "codex-encoded" => {
                        json!(json!({"output":body,"metadata":{"exit_code":0}}).to_string())
                    }
                    "codex-object" => json!({"output":body,"metadata":{"exit_code":0}}),
                    _ => json!(body),
                };
                (
                    path,
                    vec![
                        json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":key,"cwd":"/synthetic/project"}}),
                        json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call","name":"exec_command","call_id":"native-tool","arguments":"{\"cmd\":\"printf fixture\"}"}}),
                        json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"native-tool","output":output}}),
                    ],
                )
            };
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            for record in records {
                writeln!(file, "{record}").unwrap();
            }
            drop(file);
            fixture.publish(1);
            crate::events::CACHE_READS.with(|reads| reads.set(0));
            let reply = crate::session_transcript_range(
                &fixture.options,
                "source",
                &format!("sid={key}&after=0&limit=100"),
                None,
            );
            assert_eq!(
                reply.status,
                200,
                "{harness}: {}",
                String::from_utf8_lossy(&reply.body)
            );
            let page: Value = serde_json::from_slice(&reply.body).unwrap();
            let entry = page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["k"] == "tool")
                .expect("native tool slot");
            assert_eq!(entry["field"]["name"], "out", "{harness}: {entry}");
            assert!(
                entry["ok"] == true || entry["ok"].is_null(),
                "{harness}: {entry}"
            );
            assert!(
                entry["arg"]
                    .as_str()
                    .is_some_and(|arg| arg.contains("printf fixture")),
                "{harness}: {entry}"
            );
            assert!(
                entry["out"]
                    .as_str()
                    .unwrap()
                    .starts_with("native tool output line")
            );
            assert!(page["observation"]["source_bytes"].as_u64().unwrap() < 16 * 1024);
            let generation = page["projection"]["generation"].as_str().unwrap();
            let slot = entry["slot"].as_u64().unwrap();
            let chunks = entry["field"]["chunks"].as_u64().unwrap();
            let mut expanded = String::new();
            for chunk in 0..chunks {
                let reply = crate::session_entry_field(
                    &fixture.options,
                    "source",
                    &format!(
                        "sid={key}&after={slot}&limit=1&generation={generation}&field_chunk={chunk}"
                    ),
                    None,
                );
                assert_eq!(reply.status, 200);
                let part: Value = serde_json::from_slice(&reply.body).unwrap();
                assert_eq!(part["field"]["name"], "out");
                expanded.push_str(part["text"].as_str().unwrap());
            }
            assert_eq!(expanded, body);
            crate::events::CACHE_READS.with(|reads| assert_eq!(reads.get(), 0));
        }
    }

    #[test]
    fn mixed_local_and_provider_reads_never_fallback_for_replaced_sources() {
        let fixture = Fixture::new();
        let path = fixture.source("mixed");
        fixture.publish(1);
        struct Reject(std::sync::atomic::AtomicUsize);
        impl crate::SessionSourceReader for Reject {
            fn read_range(
                &self,
                _: &SessionSourceRef,
                _: Option<&str>,
                _: u64,
                _: usize,
            ) -> io::Result<crate::SessionSourceRange> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no authorized archive",
                ))
            }
        }
        let provider = Reject(std::sync::atomic::AtomicUsize::new(0));
        let get = || {
            crate::session_transcript_range_with_mode(
                &fixture.options,
                "source",
                "sid=mixed&after=0&limit=1",
                Some(&provider),
                crate::SessionSourceReadMode::LocalThenProvider,
            )
        };
        let first = get();
        assert_eq!(first.status, 200);
        let body: Value = serde_json::from_slice(&first.body).unwrap();
        assert_eq!(body["entries"][0]["text"], "synthetic prompt");
        let identity = body["entries"][0]["entry_id"].clone();
        let provenance = body["entries"][0]["provenance"].clone();
        assert_eq!(provider.0.load(Ordering::Relaxed), 0);
        fs::remove_file(&path).unwrap();
        fs::write(&path, "{}\n").unwrap();
        let changed = get();
        let body: Value = serde_json::from_slice(&changed.body).unwrap();
        assert_eq!(body["entries"][0]["freshness"]["state"], "incomplete");
        assert_eq!(body["entries"][0]["entry_id"], identity);
        assert_eq!(body["entries"][0]["provenance"], provenance);
        assert_eq!(
            provider.0.load(Ordering::Relaxed),
            0,
            "replacement must resynchronize, not restore old source bytes"
        );
        fs::remove_file(&path).unwrap();
        let missing = get();
        let body: Value = serde_json::from_slice(&missing.body).unwrap();
        assert_eq!(body["entries"][0]["freshness"]["state"], "incomplete");
        assert_eq!(
            provider.0.load(Ordering::Relaxed),
            1,
            "only absent top-level source requests host reader"
        );
    }

    #[test]
    fn current_page_excludes_retained_rows_with_bounded_work_and_history_keeps_them() {
        let fixture = Fixture::new();
        fixture.publish(2);
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        connection.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='session-00001'",[]).unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let (status, before) = fixture.body("limit=1");
        assert_eq!(status, 200);
        let baseline = SQL_STEPS.with(|steps| steps.get());
        assert_eq!(before["items"].as_array().unwrap().len(), 1);
        assert_eq!(before["items"][0]["key"], "session-00000");
        connection.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO session_catalog(session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) SELECT 'retained-'||printf('%05d',x),'retained',last_ms,harness,repo,parent_key,json_set(metadata,'$.key','retained-'||printf('%05d',x)) FROM n,session_catalog WHERE session_key='session-00001'").unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let (status, after) = fixture.body("limit=1");
        assert_eq!(status, 200);
        assert_eq!(after["items"], before["items"]);
        assert_eq!(SQL_STEPS.with(|steps| steps.get()), baseline);
        let (status, history) = fixture.body("scope=retained_history&limit=2");
        assert_eq!(status, 200);
        assert_eq!(history["items"].as_array().unwrap().len(), 2);
        assert!(history["next_cursor"].is_string());
        // Local absence does not revoke native-current authority by itself.
        assert!(
            session_catalog_identity(&fixture.options, "source", "session-00001")
                .unwrap()
                .is_some()
        );
        assert!(Request::parse("scope=unknown").is_err());
        let current = session_catalog_identity(&fixture.options, "stable-source", "session-00000")
            .unwrap()
            .unwrap();
        assert_eq!(current.read_scope, CatalogReadScope::Current);
    }

    #[test]
    fn current_page_excludes_retained_rows_with_bounded_work_and_history_keeps_them() {
        let fixture = Fixture::new();
        fixture.publish(2);
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        connection.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE session_key='session-00001'",[]).unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let (status, before) = fixture.body("limit=1");
        assert_eq!(status, 200);
        let baseline = SQL_STEPS.with(|steps| steps.get());
        assert_eq!(before["items"].as_array().unwrap().len(), 1);
        assert_eq!(before["items"][0]["key"], "session-00000");
        connection.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO session_catalog(session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) SELECT 'retained-'||printf('%05d',x),'retained',last_ms,harness,repo,parent_key,json_set(metadata,'$.key','retained-'||printf('%05d',x)) FROM n,session_catalog WHERE session_key='session-00001'").unwrap();
        SQL_STEPS.with(|steps| steps.set(0));
        let (status, after) = fixture.body("limit=1");
        assert_eq!(status, 200);
        assert_eq!(after["items"], before["items"]);
        assert_eq!(SQL_STEPS.with(|steps| steps.get()), baseline);
        let (status, history) = fixture.body("scope=retained_history&limit=2");
        assert_eq!(status, 200);
        assert_eq!(history["items"].as_array().unwrap().len(), 2);
        assert!(history["next_cursor"].is_string());
        // Local absence does not revoke native-current authority by itself.
        assert!(
            session_catalog_identity(&fixture.options, "source", "session-00001")
                .unwrap()
                .is_some()
        );
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
    fn selected_native_index_and_scoped_builder_preserve_membership_and_oracle() {
        let fixture = Fixture::new();
        fixture.publish(3);
        let connection = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        let metadata = rows(&transaction, &Request::parse("sid=session-00000").unwrap()).unwrap();
        let source = &metadata[0];
        let ids = source.native_ids.iter().cloned().collect();
        let mut selected = EventCache::from_selected(&transaction, &source.sources, &ids).unwrap();
        assert_eq!(selected.paths().count(), 1);
        let paths: Vec<_> = source
            .sources
            .iter()
            .map(|source| crate::inputs::Input {
                root: crate::inputs::InputRoot::parse(&metadata[0].harness).unwrap(),
                path: source_relative(&fixture.options, &metadata[0].harness, source).unwrap(),
            })
            .collect();
        let now = crate::model::now_ms();
        let mut all = EventCache::open(&fixture.options.cache);
        let full = crate::model::build(
            &fixture.options,
            &mut all,
            &mut false,
            &mut crate::model::Texts::default(),
            now,
        )
        .unwrap();
        let built = crate::model::build_sources(
            &fixture.options,
            &mut selected,
            &mut false,
            &mut crate::model::Texts::default(),
            now,
            Some(&paths),
        )
        .unwrap();
        assert_eq!(built.sessions.len(), 1);
        assert_eq!(
            serde_json::to_value(&built.sessions[&source.key]).unwrap(),
            serde_json::to_value(&full.sessions[&source.key]).unwrap()
        );
        assert_eq!(
            crate::tx::page(&built, &source.key, &crate::tx::Anchor::Last, now).unwrap(),
            crate::tx::page(&full, &source.key, &crate::tx::Anchor::Last, now).unwrap()
        );
        assert_eq!(selected.paths().count(), 1);
        let files: i64 = connection
            .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
            .unwrap();
        let catalog: i64 = connection
            .query_row("SELECT count(*) FROM session_catalog", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            (files, catalog),
            (3, 3),
            "scoped native projection must not prune unrelated global rows"
        );
        let mut conflicting = source.sources.clone();
        conflicting[0].offset += 1;
        assert!(EventCache::from_selected(&transaction, &conflicting, &ids).is_err());
        conflicting.push(source.sources[0].clone());
        assert!(EventCache::from_selected(&transaction, &conflicting, &ids).is_err());
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
            scope: CatalogReadScope::Current,
            limit: 3,
            sid: None,
            harness: Some("claude".into()),
            repo: Some("project".into()),
            cursor: Some(Cursor {
                scope: CatalogReadScope::Current,
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
