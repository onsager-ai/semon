//! Focused reads of the published metadata catalog. This path never opens the
//! EventCache, discovers native log files, loads transcript bodies or constructs
//! a model. Runtime labels reuse the shared native observation fence.
use crate::{
    Options,
    model::summary::{CATALOG_VERSION, CatalogRow, CatalogSource},
    viewer::{ViewerReply, decoded, query_value},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io, time::UNIX_EPOCH};

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
    pub changed_ns: Option<i128>,
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
        let Some(crate::storage::catalog::SourceProofObservation {
            key,
            source,
            generation,
        }) = crate::storage::source_proof_observation(options, root, absolute)?
        else {
            return Ok(None);
        };
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
            changed_ns: source.changed_ns,
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

/// Completeness of native lineage identity, independent of runtime authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogOwnerQualification {
    #[default]
    Qualified,
    Provisional,
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
    #[serde(default)]
    pub owner_qualification: CatalogOwnerQualification,
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
        false,
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
        false,
    )
}

/// Local multi-source Query must not infer absence or unique ownership from a
/// provisional discovery. The completeness gate shares the lookup's snapshot.
pub(crate) fn qualified_history_identity(
    options: &Options,
    source_key: &str,
    key: &str,
) -> Result<Option<CatalogSessionIdentity>, CatalogIdentityError> {
    catalog_identity(
        options,
        source_key,
        key,
        CatalogReadScope::RetainedHistory,
        true,
    )
}

/// Control ownership must prove absence and uniqueness against a complete
/// current publication, rather than admitting retained history as writable.
pub(crate) fn qualified_current_identity(
    options: &Options,
    source_key: &str,
    key: &str,
) -> Result<Option<CatalogSessionIdentity>, CatalogIdentityError> {
    catalog_identity(options, source_key, key, CatalogReadScope::Current, true)
}

fn catalog_identity(
    options: &Options,
    source_key: &str,
    canonical_catalog_key: &str,
    scope: CatalogReadScope,
    require_complete: bool,
) -> Result<Option<CatalogSessionIdentity>, CatalogIdentityError> {
    catalog_selection(
        options,
        source_key,
        canonical_catalog_key,
        scope,
        require_complete,
    )
    .map(|selected| selected.map(|(identity, _)| identity))
}

/// The selected row and its identity come from one native Catalog snapshot.
/// Consumers retain its generation for subsequent indexed or source reads.
pub(crate) fn catalog_selection(
    options: &Options,
    source_key: &str,
    canonical_catalog_key: &str,
    scope: CatalogReadScope,
    require_complete: bool,
) -> Result<Option<(CatalogSessionIdentity, Value)>, CatalogIdentityError> {
    catalog_selection_read(
        options,
        source_key,
        canonical_catalog_key,
        scope,
        require_complete,
        None,
    )
}

/// Bind selected metadata and its consumer's indexed recipes to one SQLite
/// snapshot. The ordinary source-root and native-selection checks still apply.
pub(crate) fn catalog_selection_in_snapshot(
    options: &Options,
    source_key: &str,
    canonical_catalog_key: &str,
    scope: CatalogReadScope,
    snapshot: &dyn crate::storage::catalog::CatalogSnapshot,
) -> Result<Option<(CatalogSessionIdentity, Value)>, CatalogIdentityError> {
    catalog_selection_read(
        options,
        source_key,
        canonical_catalog_key,
        scope,
        false,
        Some(snapshot),
    )
}

fn catalog_selection_read(
    options: &Options,
    source_key: &str,
    canonical_catalog_key: &str,
    scope: CatalogReadScope,
    require_complete: bool,
    snapshot: Option<&dyn crate::storage::catalog::CatalogSnapshot>,
) -> Result<Option<(CatalogSessionIdentity, Value)>, CatalogIdentityError> {
    use CatalogIdentityError::{InvalidArguments, ScopeChanged, Unavailable};
    if source_key.is_empty()
        || source_key.len() > 4096
        || canonical_catalog_key.is_empty()
        || canonical_catalog_key.len() > 4096
    {
        return Err(InvalidArguments);
    }
    let request = Request {
        require_complete,
        scope,
        limit: 1,
        sid: Some(canonical_catalog_key.to_owned()),
        harness: None,
        repo: None,
        model: None,
        q: None,
        cursor: None,
    };
    let reply = match snapshot {
        Some(snapshot) => read_page_snapshot(options, source_key, &request, snapshot),
        None => crate::storage::read_local_catalog_page(options, source_key, &request),
    }
    .map_err(|_| Unavailable)?;
    let body: Value = serde_json::from_slice(&reply.body).map_err(|_| Unavailable)?;
    match reply.status {
        404 => return Ok(None),
        200 => {}
        _ if body["error"]["code"] == "catalog_scope_changed" => return Err(ScopeChanged),
        _ => return Err(Unavailable),
    }
    if require_complete
        && scope == CatalogReadScope::Current
        && body["items"][0]["lifecycle"] != "current"
    {
        return Ok(None);
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
    let provisional = body["completeness"]["state"] == "partial";
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
    Ok(Some((
        CatalogSessionIdentity {
            source_key: source_key.to_owned(),
            read_scope: scope,
            catalog_key: item.key,
            harness: item.harness,
            native_id: (!provisional && native_ids.len() == 1).then(|| native_ids[0].clone()),
            owner_qualification: if provisional {
                CatalogOwnerQualification::Provisional
            } else {
                CatalogOwnerQualification::Qualified
            },
            native_ids,
            source_refs: item.source_refs,
            machine_label: body["machine_info"]["label"].as_str().map(str::to_owned),
            generation,
            observed_at,
            freshness: item.freshness,
            facts_observation: item.facts_observation,
            native_selection: item.native_selection,
        },
        body["items"][0].clone(),
    )))
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

/// Validate focused catalog arguments before selecting source custody. This
/// performs no I/O and returns the same argument errors as the page reader.
/// It grants no authority; source scope, cursor generation and access still
/// require validation by the host and `session_catalog_page`.
pub fn validate_session_catalog_query(query: &str) -> Result<(), ViewerReply> {
    Request::parse(query).map(|_| ())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    #[serde(skip)]
    pub(crate) inclusive: bool,
    #[serde(default)]
    pub(crate) scope: CatalogReadScope,
    pub(crate) version: u32,
    pub(crate) generation: String,
    pub(crate) machine: String,
    pub(crate) harness: Option<String>,
    pub(crate) repo: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) q: Option<String>,
    pub(crate) last: i64,
    pub(crate) key: String,
}

pub(crate) struct Request {
    pub(crate) require_complete: bool,
    pub(crate) scope: CatalogReadScope,
    pub(crate) limit: usize,
    pub(crate) sid: Option<String>,
    pub(crate) harness: Option<String>,
    pub(crate) repo: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) q: Option<String>,
    pub(crate) cursor: Option<Cursor>,
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
                "Use limit=1..100, valid source filters, and a bounded metadata query or cursor.",
                false,
            )
        };
        if query.len() > 64 * 1024 {
            return Err(invalid());
        }
        let parameter = |key| {
            if query
                .split('&')
                .filter(|part| part.split('=').next() == Some(key))
                .take(2)
                .count()
                > 1
            {
                return Err(invalid());
            }
            match query_value(query, key) {
                None => Ok(None),
                Some(value) => decoded(value).map(Some).ok_or_else(invalid),
            }
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
        let q = parameter("q")?
            .map(|value| crate::catalog_search::normalize_query(&value).ok_or_else(invalid))
            .transpose()?
            .filter(|query| !query.is_empty());
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
        let no_repo = parameter("no_repo")?;
        if no_repo.as_deref().is_some_and(|value| value != "1")
            || (no_repo.is_some() && repo.is_some())
        {
            return Err(invalid());
        }
        let model = parameter("model")?;
        if model
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 256)
        {
            return Err(invalid());
        }
        if repo
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 4096)
        {
            return Err(invalid());
        }
        let repo = if no_repo.is_some() {
            Some(String::new())
        } else {
            repo
        };
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
        if sid.is_some()
            && (harness.is_some()
                || repo.is_some()
                || model.is_some()
                || q.is_some()
                || cursor.is_some())
        {
            return Err(invalid());
        }
        Ok(Self {
            require_complete: false,
            scope,
            limit,
            sid,
            harness,
            repo,
            model,
            q,
            cursor,
        })
    }
}

pub(crate) fn source_relative(
    options: &Options,
    harness: &str,
    source: &CatalogSource,
) -> Option<String> {
    let home = match harness {
        "claude" => &options.claude_home,
        "codex" => &options.codex_home,
        "copilot" => &options.copilot_home,
        _ => return None,
    };
    source_relative_from_home(home, harness, source)
}

pub(crate) fn source_relative_from_home(
    home: &std::path::Path,
    harness: &str,
    source: &CatalogSource,
) -> Option<String> {
    let relative = source.path.strip_prefix(home).ok()?.to_str()?;
    (source
        .path
        .extension()
        .is_some_and(|extension| extension == "jsonl")
        && crate::is_input_path(harness, relative))
    .then(|| relative.to_owned())
}

pub(crate) fn source_state(source: &CatalogSource) -> &'static str {
    // Catalog state compares the native file's identity and observation stamp.
    // It needs the secure input handle, without opening archive manifests or
    // preparing a logical transcript reader for this metadata-only fence.
    let Ok(file) = crate::open_input(&source.path) else {
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
        && source
            .changed_ns
            .is_some_and(|token| Some(token) == crate::events::change_time_ns(&metadata))
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

pub(crate) fn recorded_status(
    row: &CatalogRow,
    runtime: Option<&str>,
    cached: bool,
    facts_known: bool,
    retired_native: bool,
) -> Result<Value, serde_json::Error> {
    let current = row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
    if let Some(saved) = &row.query
        && crate::model::summary::query_observation_matches(saved, &row.sources, current, runtime)?
        && cached
        && (!current || (facts_known && !retired_native))
    {
        let status = match saved["state"].as_str() {
            Some("work" | "wait") if !current => Some("off"),
            Some("work" | "wait" | "idle" | "done" | "err" | "new" | "read" | "off") => {
                saved["state"].as_str()
            }
            _ => None,
        };
        if let Some(status) = status {
            return Ok(
                json!({"state":status,"observation":if current {"current"} else {"retained"}}),
            );
        }
    }
    Ok(json!({"state":null,"observation":"unavailable"}))
}

/// Bounded related rows reuse the Catalog owner's runtime and custody checks.
/// Callers share one capture and apply the owner's final response fences.
/// Only the row's published sources are checked; no bodies load.
pub(crate) fn relative_status(
    options: &Options,
    runtime: &crate::native_evidence::RuntimeObservations,
    recorded: Option<&crate::facts::source_authority::CurrentSources>,
    row: &CatalogRow,
) -> Value {
    let unavailable = || json!({"state":null,"observation":"unavailable"});
    if row.sources.is_empty() || row.sources.len() > 64 {
        return unavailable();
    }
    let current = row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
    let facts_known =
        options.facts.is_none() || recorded.is_some_and(|source| source.facts_known());
    if current
        && (!facts_known
            || (row.harness == "codex"
                && options.facts.is_some()
                && !recorded.is_some_and(|source| source.selection_known())))
    {
        return unavailable();
    }
    let mut retired_native = false;
    for source in &row.sources {
        let Some(relative) = source_relative(options, &row.harness, source) else {
            return unavailable();
        };
        if row.harness == "codex" {
            match recorded
                .map(|snapshot| snapshot.codex_current(&relative))
                .transpose()
            {
                Ok(selected) => retired_native |= selected.flatten() == Some(false),
                Err(_) => return unavailable(),
            }
        }
        if source_state(source) != "cached" {
            return unavailable();
        }
    }
    recorded_status(
        row,
        runtime.expected(row).as_deref(),
        true,
        facts_known,
        retired_native,
    )
    .unwrap_or_else(|_| unavailable())
}

pub(crate) fn page(options: &Options, machine: &str, request: Request) -> ViewerReply {
    match crate::storage::read_local_catalog_page(options, machine, &request) {
        Ok(reply) => reply,
        Err(_) => error(
            503,
            "catalog_unavailable",
            "The session catalog is unavailable. Retry after the background observation completes.",
            true,
        ),
    }
}

pub(crate) fn read_page_snapshot(
    options: &Options,
    machine: &str,
    request: &Request,
    transaction: &dyn crate::storage::catalog::CatalogSnapshot,
) -> Result<ViewerReply, Box<dyn std::error::Error>> {
    transaction.require_current()?;
    if request.cursor.as_ref().is_some_and(|cursor| {
        cursor.scope != request.scope
            || cursor.machine != machine
            || cursor.harness != request.harness
            || cursor.repo != request.repo
            || cursor.model != request.model
            || cursor.q != request.q
    }) {
        return Ok(error(
            400,
            "cursor_scope_mismatch",
            "Discard the cursor when changing machine or filters.",
            false,
        ));
    }
    let pinned = transaction.presentation_observations();
    let runtime = if pinned.is_some() {
        None
    } else {
        crate::native_evidence::query_runtime_generation(options).ok()
    };
    let runtime_binding = transaction.runtime_binding();
    let runtime_observations = match (pinned, runtime_binding.as_ref()) {
        (Some(_), _) => None,
        (None, Some(binding)) => Some(crate::native_evidence::RuntimeObservations::bound(
            binding,
            runtime
                .as_deref()
                .ok_or_else(|| io::Error::other("runtime observation unavailable"))?,
        )?),
        (None, None) => Some(crate::native_evidence::RuntimeObservations::new(options)),
    };
    // Share the same custody capture with selected family status qualification.
    // Its existing final validation still fences the whole response.
    let recorded = pinned
        .is_none()
        .then_some(options)
        .and_then(|options| options.facts.as_ref())
        .and_then(|path| crate::facts::source_authority::CurrentSources::open(path).ok());
    let facts_known = pinned.map_or_else(
        || options.facts.is_none() || recorded.as_ref().is_some_and(|source| source.facts_known()),
        |saved| saved.facts_known,
    );
    let codex_selection_known = pinned.map_or_else(
        || {
            options.facts.is_none()
                || recorded
                    .as_ref()
                    .is_some_and(|source| source.selection_known())
        },
        |saved| saved.selection_known,
    );
    let meta = |key: &str| transaction.publication_value(key);
    let version = meta("catalog_version")?;
    let Some(generation) = meta("catalog_generation")? else {
        return Err(io::Error::other("catalog not published").into());
    };
    if version.as_deref() != Some(CATALOG_VERSION.to_string().as_str()) {
        return Err(io::Error::other("catalog version").into());
    }
    if request.require_complete {
        match meta("catalog_completeness")?.as_deref() {
            None | Some("complete") => {}
            _ => {
                return Err(
                    io::Error::other("catalog ownership is unavailable or provisional").into(),
                );
            }
        }
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
    let (mut selected, search_next, search_report) = if let Some(query) = &request.q {
        let scope = crate::catalog_search::scope_key(request.scope);
        if meta("search_version")?.as_deref() != Some(crate::catalog_search::VERSION)
            || meta(&format!("search_generation_{scope}"))?.as_deref() != Some(generation.as_str())
        {
            return Ok(error(
                503,
                "search_unavailable",
                "The metadata search projection is unavailable. Retry after background publication.",
                true,
            ));
        }
        let collection_complete = match meta("catalog_completeness")?.as_deref() {
            None | Some("complete") => true,
            Some("partial") => false,
            _ => return Err(io::Error::other("catalog completeness unavailable").into()),
        };
        let complete = collection_complete
            && meta("search_backfill_complete")?.as_deref() == Some("1")
            && meta(&format!("search_incomplete_{scope}"))?.as_deref() == Some("0");
        let after = request
            .cursor
            .as_ref()
            .map(|cursor| crate::catalog_search::Position {
                inclusive: cursor.inclusive,
                last: cursor.last,
                key: cursor.key.clone(),
            });
        let found = transaction.search(crate::catalog_search::Query {
            scope: request.scope,
            query,
            harness: request.harness.as_deref(),
            repo: request.repo.as_deref(),
            model: request.model.as_deref(),
            after,
            limit: request.limit,
            index_complete: complete,
        })?;
        (found.rows, found.next, Some(found.report))
    } else {
        (transaction.rows(request)?, None, None)
    };
    let mut partial_history = std::collections::BTreeSet::new();
    for row in &selected {
        if transaction.has_history(&row.key)? {
            partial_history.insert(row.key.clone());
        }
    }

    if request.scope == CatalogReadScope::RetainedHistory {
        for current in &mut selected {
            if let Some(metadata) = transaction.history_catalog(&current.key)? {
                let history: CatalogRow = serde_json::from_str(&metadata)?;
                if history.harness != current.harness {
                    // Only explicit History identity reads can select the
                    // original harness behind a reused Current viewer key.
                    // Lists retain Current's indexed filters and ordering.
                    if request.sid.as_deref() == Some(current.key.as_str())
                        && history.key == current.key
                    {
                        if !transaction.history_qualified(&history)? {
                            return Ok(error(
                                503,
                                "catalog_unavailable",
                                "The original retained history projection is incomplete.",
                                true,
                            ));
                        }
                        *current = history;
                    }
                    continue;
                }
                // History extends source context only; current indexed fields
                // remain the list ordering/filter and native selection oracle.
                if history.key != current.key
                    || history.harness != current.harness
                    || history.last != current.last
                    || history.repo != current.repo
                    || history.model != current.model
                {
                    return Err(io::Error::other("history catalog context changed").into());
                }
                *current = history;
            }
        }
    }
    if request.sid.is_some() && selected.is_empty() {
        return Ok(error(
            404,
            "session_not_found",
            "The selected session is absent from this catalog generation. Refresh the list to resynchronize.",
            false,
        ));
    }
    let completeness = meta("catalog_completeness")?.unwrap_or_else(|| "complete".into());
    if !matches!(completeness.as_str(), "partial" | "complete") {
        return Err(io::Error::other("catalog completeness is invalid").into());
    }
    let more = selected.len() > request.limit;
    if more {
        selected.pop();
    }
    let next = if let Some(position) = search_next {
        Some(serde_json::to_string(&Cursor {
            inclusive: false,
            scope: request.scope,
            version: 1,
            generation: generation.clone(),
            machine: machine.to_owned(),
            harness: request.harness.clone(),
            repo: request.repo.clone(),
            model: request.model.clone(),
            q: request.q.clone(),
            last: position.last,
            key: position.key,
        })?)
    } else if more {
        selected
            .last()
            .map(|row| {
                serde_json::to_string(&Cursor {
                    inclusive: false,
                    scope: request.scope,
                    version: 1,
                    generation: generation.clone(),
                    machine: machine.to_owned(),
                    harness: request.harness.clone(),
                    repo: request.repo.clone(),
                    model: request.model.clone(),
                    q: request.q.clone(),
                    last: row.last.unwrap_or(0),
                    key: row.key.clone(),
                })
            })
            .transpose()?
    } else {
        None
    };
    let details = if request.sid.is_some() {
        selected
            .first()
            .map(|row| {
                transaction.selected_details(&row.key, &row.sources, &mut |relative| {
                    // ImmutableCatalog supplies its pinned relative inputs.
                    runtime_observations
                        .as_ref()
                        .map(|runtime| {
                            relative_status(options, runtime, recorded.as_ref(), relative)
                        })
                        .unwrap_or(Value::Null)
                })
            })
            .transpose()?
            .flatten()
    } else {
        None
    };
    let transcript = if request.sid.is_some() && details.is_some() {
        selected
            .first()
            .map(|row| {
                transaction.transcript_observation(
                    &row.key,
                    &generation,
                    request.scope == CatalogReadScope::RetainedHistory,
                )
            })
            .transpose()?
            .flatten()
    } else {
        None
    };
    // The legacy model tolerates absent facts/old source selection. Focused
    // observations must retain that absence rather than invent current facts.
    let machine_label = match pinned {
        Some(saved) => saved.machine_label.clone(),
        None => match &options.facts {
            None => Some(crate::facts::MachineFacts::Local.hostname(options)),
            Some(_) => recorded.as_ref().and_then(|source| source.hostname()),
        },
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
            let retired = if row.harness == "codex" && pinned.is_some() {
                pinned.is_some_and(|saved| saved.retired(source))
            } else if row.harness == "codex" {
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
            let state = transaction
                .source_state(source)
                .unwrap_or_else(|| source_state(source));
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
        let partial_summary = partial_history.contains(&row.key);
        let current = row.lifecycle == crate::model::summary::CatalogLifecycle::Current;
        // A complete membership publication can precede a new runtime facts
        // snapshot. Keep status unavailable, but expose the stale derivation so
        // readers retry instead of retaining unknown dots until navigation.
        let runtime = pinned.and_then(|saved| saved.runtime.clone()).or_else(|| {
            runtime_observations
                .as_ref()
                .and_then(|runtime| runtime.expected(&row))
        });
        let runtime_pending = current
            && facts_known
            && !retired_native
            && state == "cached"
            && runtime.is_some()
            && row.query.as_ref().is_some_and(|saved| {
                saved["runtime"].as_str() != runtime.as_deref()
                    && crate::model::summary::query_observation_matches(
                        saved,
                        &row.sources,
                        false,
                        None,
                    )
                    .unwrap_or(false)
            });
        if runtime_pending && page_state == "cached" {
            page_state = "stale";
        }
        let observation = recorded_status(
            &row,
            runtime.as_deref(),
            state == "cached",
            facts_known,
            retired_native,
        )?;
        let mut item = serde_json::to_value(row)?;
        item["status"] = observation;
        // Exact Query facts and runtime fences are internal publication data.
        // The Catalog transport retains its stable description allowlist.
        item.as_object_mut()
            .expect("catalog row object")
            .remove("query");
        if let Some(transcript) = &transcript {
            item["transcript"] = transcript.clone();
        }
        if let Some(details) = &details {
            item["details"] = details.clone();
            item["details"]["live_observation"] = json!(
                details["live_observation"] == true
                    && facts_known
                    && !retired_native
                    && state == "cached"
            );
        }
        if partial_summary {
            // Body provenance can include retained sources; current summary
            // metrics have not rebuilt those sources' usage contributions.
            item["summary_context"] = json!({"state":"incomplete","scope":"current_observation"});
        }

        item.as_object_mut()
            .expect("catalog row object")
            .remove("sources");
        item["source_refs"] = json!(refs);
        item["freshness"] = json!({"state":state});
        item["facts_observation"] = json!({"state":if runtime_pending {"stale"} else if facts_known { "cached" } else { "unavailable" }});
        item["native_selection"] = if is_codex {
            json!({"state":if !facts_known { "unavailable" } else if retired_native { "retired" } else if codex_selection_known { "cached" } else { "incomplete" }})
        } else {
            serde_json::Value::Null
        };
        if let Some(generation) = item["transcript"]["generation"].as_str() {
            item["transcript"]["errors_generation"] =
                json!(crate::transcript_range::error_generation(generation, &item,));
        }
        items.push(item);
    }
    if runtime.as_ref().is_some_and(|before| {
        crate::native_evidence::query_runtime_generation(options)
            .ok()
            .as_ref()
            != Some(before)
    }) {
        for item in &mut items {
            for section in ["ancestors", "runs"] {
                if let Some(relatives) = item
                    .get_mut("details")
                    .and_then(|details| details.get_mut("family"))
                    .and_then(|family| family.get_mut(section))
                    .and_then(Value::as_array_mut)
                {
                    for relative in relatives {
                        if relative["status"]["observation"] == "current" {
                            relative["status"] = json!({"state":null,"observation":"unavailable"});
                        }
                    }
                }
            }
            if item["status"]["observation"] == "current" {
                item["status"] = json!({"state":null,"observation":"unavailable"});
                item["facts_observation"] = json!({"state":"stale"});
                if page_state == "cached" {
                    page_state = "stale";
                }
            }
        }
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
    if search_report
        .as_ref()
        .is_some_and(|report| report.partial || !report.index_complete)
        && page_state == "cached"
    {
        page_state = "incomplete";
    }
    Ok(reply(
        200,
        json!({"api":1,"machine":machine,"read_scope":request.scope,"generation":generation,"observed_at":observed_at,
        "machine_info":{"key":machine,"label":machine_label,"freshness":if facts_known { "cached" } else { "unavailable" }},
        "capabilities":{"pagination":true,"metadata_search":true,"filters":["harness","repo","no_repo","model","q"],"order":"last_desc_key_asc",
            "full_text_search":false,"selected_session_lookup":true,"runtime_status":true,"global_union":false},
        "completeness":{"state":completeness},"freshness":if completeness == "partial" { "updating" } else { page_state },"items":items,"next_cursor":next,"search":search_report.map(|report|json!({"semantics":"unicode_lowercase_substring","fields":["name","key","repo","branch","model","harness"],"partial":report.partial,"candidates":report.candidates,"index_complete":report.index_complete,"candidate_budget":crate::catalog_search::MAX_CANDIDATES,"byte_budget":crate::catalog_search::MAX_SCAN_BYTES}))}),
    ))
}

#[cfg(test)]
#[path = "storage/sqlite/catalog_tests.rs"]
mod tests;

#[path = "catalog_union.rs"]
mod union;
pub use union::session_catalog_union;
