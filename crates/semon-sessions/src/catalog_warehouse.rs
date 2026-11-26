//! Viewer reads over the committed warehouse. The host supplies authorized
//! inventory and repository observations. Each source is read independently.
use super::{CatalogReadScope, Request};
use crate::{
    events::{Ledger, Stat, store::warehouse},
    source_change::AppendEvidence,
    storage::Namespace,
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[path = "catalog_warehouse/sender_actions.rs"]
mod sender_actions;

#[path = "catalog_warehouse/parents.rs"]
mod parents;

#[path = "catalog_warehouse/home_feed.rs"]
pub(crate) mod home_feed;

/// Request-local reader for custody held by the source owning this Index.
/// Callers supply references from a qualified frozen warehouse binding.
pub(super) struct ArchiveReader<'a> {
    options: &'a crate::Options,
    archive: crate::archive::Archive,
    files: std::sync::Mutex<ArchiveReads>,
}

struct ArchiveReads {
    files: BTreeMap<(String, String), ArchivePrefix>,
    bytes_left: usize,
}

struct ArchivePrefix {
    reference: crate::SessionSourceRef,
    read: crate::archive::seekable::CopyRead,
}

impl<'a> ArchiveReader<'a> {
    pub(super) fn new(options: &'a crate::Options) -> Self {
        Self::with_budget(options, selected::PAGE_BYTES)
    }

    fn with_budget(options: &'a crate::Options, bytes: usize) -> Self {
        Self {
            options,
            archive: crate::archive::Archive::new(crate::archive::standalone_dir(&options.cache)),
            files: std::sync::Mutex::new(ArchiveReads {
                files: BTreeMap::new(),
                bytes_left: bytes,
            }),
        }
    }
}

impl crate::SessionSourceReader for ArchiveReader<'_> {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max_bytes: usize,
    ) -> std::io::Result<crate::SessionSourceRange> {
        use std::{io, sync::Arc};
        let unavailable = || {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Frozen native prefix is unavailable",
            )
        };
        let root = crate::InputRoot::parse(&source.root).ok_or_else(unavailable)?;
        if !crate::is_input_path(&source.root, &source.path)
            || !source.path.ends_with(".jsonl")
            || offset > source.offset
            || max_bytes > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(unavailable());
        }
        let mut reads = self.files.lock().map_err(|_| unavailable())?;
        let ArchiveReads { files, bytes_left } = &mut *reads;
        let prefix = match files.entry((source.root.clone(), source.path.clone())) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let path = root.home(self.options).join(&source.path);
                let read = crate::archive::seekable::CopyRead::open(
                    &self.archive,
                    &path,
                    source,
                    expected,
                    bytes_left,
                )?;
                entry.insert(ArchivePrefix {
                    reference: source.clone(),
                    read,
                })
            }
        };
        let generation = prefix.read.generation().to_owned();
        if prefix.reference != *source || expected.is_some_and(|value| value != generation) {
            return Err(unavailable());
        }
        let count = (max_bytes as u64).min(source.offset - offset) as usize;
        let bytes = prefix.read.range(offset, count, bytes_left)?;
        Ok(crate::SessionSourceRange {
            generation,
            length: source.offset,
            offset,
            bytes: Arc::from(bytes),
            cached: true,
        })
    }
}

const LIST: &str = concat!(
    include_str!("events/store/warehouse_list.sql"),
    include_str!("events/store/warehouse_catalog_row.sql"),
    include_str!("events/store/warehouse_list_page.sql"),
);
const SELECTED_METADATA: &str = concat!(
    include_str!("events/store/warehouse_selected_metadata.sql"),
    include_str!("events/store/warehouse_catalog_row.sql"),
    include_str!("events/store/warehouse_selected_row.sql"),
);
const RELATIVE_KEYS: &str = include_str!("events/store/warehouse_relative_keys.sql");
const EXACT: &str = include_str!("events/store/warehouse_exact.sql");
const USAGE: &str = include_str!("events/store/warehouse_list_usage.sql");
const PAGE_MEMBERS: &str = "SELECT p.value,d.harness,d.native_id,s.carrier,s.ledger
    FROM json_each(?1) p
    CROSS JOIN lineage_member root ON root.source=p.value
    CROSS JOIN lineage_member m ON m.component=root.component
    CROSS JOIN dim_session d ON d.source=m.source
    CROSS JOIN warehouse_source s ON s.source=m.source AND s.origin=?2 AND s.harness=d.harness
    ORDER BY p.value,m.source";
const STATUS: &str = "SELECT p.value,d.harness,d.native_id,s.carrier,s.ledger,d.lineage,m.source
    FROM json_each(?1) p
    CROSS JOIN lineage_member root ON root.source=p.value
    CROSS JOIN lineage_member m ON m.component=root.component
    CROSS JOIN dim_session d ON d.source=m.source
    CROSS JOIN warehouse_source s ON s.source=m.source
    ORDER BY p.value,m.source";
const RECEIPTS: &str =
    "SELECT paths.key,paths.value,s.source,s.ledger,m.sidecar_revision,m.eof_witness,a.evidence,a.source_path IS NOT NULL
    FROM json_each(?1) paths
    LEFT JOIN warehouse_current c ON paths.value AND c.path=paths.key
    LEFT JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
    LEFT JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
    LEFT JOIN maintenance_cursor m ON m.source=s.source AND f.file_id IS NOT NULL
    LEFT JOIN session_source_mutations a ON a.source_path=c.path
    ORDER BY paths.key";

fn carrier_unavailable(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    if matches!(error.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) {
        return true;
    }
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Availability {
    Expected(Option<Stat>),
    Unavailable,
}

pub(super) struct Source<'a> {
    pub(super) namespace: Namespace,
    pub(super) path: &'a std::path::Path,
    pub(super) options: &'a crate::Options,
    pub(super) inventory: BTreeMap<String, Availability>,
    pub(super) repos: BTreeMap<String, String>,
    pub(super) mode: crate::SessionSourceReadMode,
    pub(super) current_provider: Option<&'a dyn crate::SessionSourceReader>,
    focused: Option<FocusedAuthority>,
}

struct FocusedAuthority {
    recorded: Option<crate::facts::source_authority::CurrentSources>,
}

impl<'a> Source<'a> {
    fn focused(
        namespace: Namespace,
        path: &'a std::path::Path,
        options: &'a crate::Options,
        current_provider: Option<&'a dyn crate::SessionSourceReader>,
    ) -> Result<Self, ReadError> {
        let recorded = options
            .facts
            .as_deref()
            .map(crate::facts::source_authority::CurrentSources::open)
            .transpose()
            .map_err(index)?;
        Ok(Self {
            namespace,
            path,
            options,
            inventory: BTreeMap::new(),
            repos: BTreeMap::new(),
            mode: crate::SessionSourceReadMode::LocalThenProvider,
            current_provider,
            focused: Some(FocusedAuthority { recorded }),
        })
    }

    fn with_mode(mut self, mode: crate::SessionSourceReadMode) -> Self {
        self.mode = mode;
        self
    }

    fn provider_only(&self) -> bool {
        self.current_provider.is_some()
            && matches!(self.mode, crate::SessionSourceReadMode::ProviderOnly)
    }

    fn discover(
        namespace: Namespace,
        path: &'a std::path::Path,
        options: &'a crate::Options,
    ) -> Result<Self, ReadError> {
        let mut source = Self::focused(namespace, path, options, None)?;
        let facts = source
            .focused
            .as_ref()
            .and_then(|authority| authority.recorded.as_ref())
            .map(crate::facts::source_authority::CurrentSources::read_snapshot)
            .transpose()
            .map_err(index)?;
        let mut inputs: BTreeSet<_> = crate::inputs(options)
            .map_err(index)?
            .into_iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .collect();
        if let Some(facts) = facts.filter(|facts| facts.version == crate::facts::FACTS_VERSION) {
            if let Some(mut paths) = facts.codex_rollouts {
                paths.extend(facts.codex_provisional_rollouts.into_iter().flatten());
                inputs.retain(|input| input.root != crate::InputRoot::Codex);
                inputs.extend(paths.into_iter().filter_map(|path| {
                    crate::is_input_path("codex", &path).then_some(crate::Input {
                        root: crate::InputRoot::Codex,
                        path,
                    })
                }));
            }
            source.repos = facts
                .repos
                .into_iter()
                .filter_map(|(cwd, repo)| repo.map(|repo| (cwd, repo)))
                .collect();
        }
        for input in inputs {
            let full = input.full_path(options);
            let Some(path) = full.to_str() else { continue };
            let observed =
                crate::open_input(&full).and_then(|file| Stat::from_metadata(&file.metadata()?));
            let availability = match observed {
                Ok(stat) => Availability::Expected(Some(stat)),
                Err(error) if carrier_unavailable(&error) => Availability::Unavailable,
                Err(error) => return Err(index(error)),
            };
            source.inventory.insert(path.to_owned(), availability);
        }
        source.validate_authority()?;
        Ok(source)
    }

    fn availability(&self, harness: &str, carrier: &str) -> Result<Availability, ReadError> {
        let Some(authority) = &self.focused else {
            return Ok(self
                .inventory
                .get(carrier)
                .copied()
                .unwrap_or(Availability::Unavailable));
        };
        if !self.path_authorized(authority, harness, carrier)? {
            return Ok(Availability::Unavailable);
        }
        if self.provider_only() {
            return Ok(Availability::Expected(None));
        }
        match crate::open_input(std::path::Path::new(carrier))
            .and_then(|file| Stat::from_metadata(&file.metadata()?))
        {
            Ok(stat) => Ok(Availability::Expected(Some(stat))),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && self.current_provider.is_some() =>
            {
                Ok(Availability::Expected(None))
            }
            Err(error) if carrier_unavailable(&error) => Ok(Availability::Unavailable),
            Err(error) => Err(index(error)),
        }
    }

    fn path_authorized(
        &self,
        authority: &FocusedAuthority,
        harness: &str,
        carrier: &str,
    ) -> Result<bool, ReadError> {
        let Some(path) = selected::relative(self.options, harness, carrier) else {
            return Ok(false);
        };
        if harness == "codex"
            && let Some(recorded) = &authority.recorded
        {
            return Ok(recorded
                .codex_current(path)
                .map_err(|_| ReadError::Restart)?
                != Some(false));
        }
        Ok(true)
    }

    fn validate_authority(&self) -> Result<(), ReadError> {
        if let Some(recorded) = self
            .focused
            .as_ref()
            .and_then(|authority| authority.recorded.as_ref())
        {
            recorded.validate().map_err(|_| ReadError::Restart)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Position {
    last: i64,
    key: String,
    source: String,
}

impl Ord for Position {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .last
            .cmp(&self.last)
            .then_with(|| self.key.cmp(&other.key))
            .then_with(|| self.source.cmp(&other.source))
    }
}

impl PartialOrd for Position {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Boundary {
    position: Position,
    inclusive: bool,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Continuation {
    revision: String,
    query: String,
    boundary: Boundary,
}

#[derive(Debug)]
struct PromptTitle {
    carrier: String,
    ledger: String,
    native_id: String,
    offset: i64,
}

#[derive(Debug)]
pub(super) struct Row {
    position: Position,
    owner: i64,
    pub(super) harness: String,
    pub(super) title: String,
    prompt_title: Option<PromptTitle>,
    native_title: bool,
    codex_native_id: Option<String>,
    pub(super) model: String,
    effort: Option<String>,
    repo: Option<String>,
    branch: Option<String>,
    pub(super) parent: Option<String>,
    parent_source: Option<String>,
    native_ids: BTreeSet<String>,
    source_refs: Vec<crate::CatalogSourceObservation>,
    start: Option<i64>,
    kind: &'static str,
    presentation_kind: Option<&'static str>,
    pub(super) usage: [i64; 6],
    pub(super) cost: Option<crate::pricing::Cost>,
    pub(super) status: Option<&'static str>,
    details: Option<serde_json::Value>,
}

impl Row {
    fn resolve_title(
        &mut self,
        view: &selected::ReadView<'_>,
        source: &Source<'_>,
    ) -> Result<(), ReadError> {
        if let Some(id) = &self.codex_native_id
            && let Some(name) = view.codex_name(id)?
        {
            self.title = name.to_owned();
            self.native_title = true;
        } else if let Some(prompt) = self.prompt_title.take() {
            self.title = selected::prompt_title(
                view,
                source,
                prompt.carrier,
                &prompt.ledger,
                prompt.native_id,
                prompt.offset,
            )?
            .unwrap_or_else(|| "Codex subagent".into());
        }
        Ok(())
    }

    fn token_millions(&self) -> [f64; 3] {
        let [
            input,
            cached,
            output,
            shared_input,
            shared_cached,
            shared_output,
        ] = self.usage;
        let million = |tokens: i64| (tokens as f64 / 1000.0).round() / 1000.0;
        let input = input.saturating_add(shared_input);
        let cached = cached.saturating_add(shared_cached);
        [
            million(input.saturating_sub(cached).max(0)),
            million(cached),
            million(output.saturating_add(shared_output)),
        ]
    }

    fn recorded_status(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.status,
            "observation": if self.status.is_some() { "current" } else { "unavailable" },
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Coverage {
    pub(super) committed: usize,
    pub(super) pending: usize,
    pub(super) unavailable: usize,
}

#[derive(Debug)]
pub(super) struct Page<T = Row> {
    pub(super) rows: Vec<T>,
    pub(super) total: usize,
    pub(super) coverage: Coverage,
    pub(super) examined: usize,
    pub(super) next: Option<Continuation>,
    generation: String,
}

impl<T> Page<T> {
    fn map<U>(self, transform: impl FnMut(T) -> U) -> Page<U> {
        Page {
            rows: self.rows.into_iter().map(transform).collect(),
            total: self.total,
            coverage: self.coverage,
            examined: self.examined,
            next: self.next,
            generation: self.generation,
        }
    }
}

#[derive(Debug)]
pub(super) enum ReadError {
    Index(String),
    Scope,
    Restart,
    RetainedUnverified,
    RetainedUnavailable,
    RetainedPreparing,
    PeerUnavailable(String),
    ActivityUnavailable,
    SourceBudget,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(error) => f.write_str(error),
            Self::SourceBudget => f.write_str("The selected source evidence exceeds the bounded read budget."),
            Self::Scope => f.write_str("Discard the cursor when changing workspace, source or filters."),
            Self::Restart => f.write_str("The committed session view changed. Restart at the first page."),
            Self::PeerUnavailable(reason) => f.write_str(reason),
            Self::ActivityUnavailable => f.write_str("The activity evidence is outside the selected prefixes."),
            Self::RetainedPreparing => f.write_str("Preparing recorded history."),
            Self::RetainedUnavailable => f.write_str("The frozen session prefix is unavailable from its authorized source reader."),
            Self::RetainedUnverified => f.write_str("retained_identity_unverified: no verified original warehouse identity is available for this key."),
        }
    }
}

impl std::error::Error for ReadError {}

fn index(error: impl std::fmt::Display) -> ReadError {
    ReadError::Index(error.to_string())
}

fn encode(value: &impl serde::Serialize) -> Result<String, ReadError> {
    serde_json::to_string(value).map_err(index)
}

type LocalPage<T = Row> = (Vec<T>, usize, usize, Option<Boundary>);
type SourcePage<T> = (LocalPage<T>, Coverage, String);
type Identity = (i64, String, String);

struct Snapshot<'a> {
    transaction: &'a Connection,
    source: &'a Source<'a>,
    paths: String,
    repos: BTreeMap<String, String>,
    coverage: Coverage,
    revision: String,
}

struct ChildRuntime {
    child: i64,
    native_id: String,
    parent: i64,
    call: Option<String>,
    root: i64,
    qualified: bool,
}

#[derive(Default)]
struct ChildRun {
    final_receipt: bool,
    handback: bool,
    notification: Option<bool>,
}

impl ChildRun {
    fn ended(&self) -> bool {
        self.final_receipt || self.handback || self.notification.is_some()
    }
}

struct Usage {
    shared: bool,
    tokens: [i64; 6],
    pricing: crate::pricing::CostBuild,
}

impl Usage {
    fn account(&mut self, row: &rusqlite::Row<'_>) -> Result<(), ReadError> {
        let shared: bool = row.get(6).map_err(index)?;
        let start = usize::from(shared) * 3;
        for (column, total) in self.tokens[start..start + 3].iter_mut().enumerate() {
            let tokens: i64 = row.get(column + 3).map_err(index)?;
            *total = total
                .checked_add(tokens)
                .ok_or_else(|| index("Warehouse usage exceeds the supported range"))?;
        }
        if shared {
            self.shared = true;
            return Ok(());
        }
        let kind: String = row.get(1).map_err(index)?;
        let model: Option<String> = row.get(2).map_err(index)?;
        match kind.as_str() {
            "claude" => self.pricing.message(&crate::events::MessageUsage {
                model,
                billing: billing(row)?,
                ..Default::default()
            }),
            "codex-snapshot" if model.is_none() => {}
            "codex-response" | "codex-snapshot" => {
                self.pricing.codex(&crate::events::CodexUsageEvent {
                    model: model.unwrap_or_else(|| "unknown".into()),
                    tokens: billing(row)?,
                    timestamp: row.get(7).map_err(index)?,
                })
            }
            "copilot" => {
                let model = model.unwrap_or_else(|| "unknown".into());
                self.pricing
                    .unpriced_models
                    .insert(crate::pricing::normalize_model_id(&model).to_owned());
            }
            _ => return Err(index("Unknown warehouse pricing fact")),
        }
        Ok(())
    }
}

fn billing<T: serde::de::DeserializeOwned>(row: &rusqlite::Row<'_>) -> Result<T, ReadError> {
    let billing: String = row.get(8).map_err(index)?;
    serde_json::from_str(&billing).map_err(index)
}

impl<'a> Snapshot<'a> {
    fn read(transaction: &'a Connection, source: &'a Source<'a>) -> Result<Self, ReadError> {
        crate::events::store::require_current(transaction).map_err(index)?;
        crate::storage::sqlite::qualify(transaction, &source.namespace).map_err(index)?;
        let origin = source.namespace.binding();
        let incarnation: String = transaction
            .query_row("SELECT value FROM meta WHERE key='incarnation'", [], |r| {
                r.get(0)
            })
            .map_err(index)?;
        let mut digest = Sha256::new_with_prefix(encode(&(origin, incarnation))?);
        let mut authorized = Vec::new();
        let mut coverage = Coverage::default();
        let inventory = encode(
            &source
                .inventory
                .iter()
                .map(|(path, availability)| {
                    (path, matches!(availability, Availability::Expected(_)))
                })
                .collect::<BTreeMap<_, _>>(),
        )?;
        let mut probe = transaction.prepare_cached(RECEIPTS).map_err(index)?;
        let mut receipts = probe.query(params![inventory, origin]).map_err(index)?;
        while let Some(record) = receipts.next().map_err(index)? {
            let path: String = record.get(0).map_err(index)?;
            let availability = source.inventory[&path];
            let sidecar: Option<i64> = record.get(4).map_err(index)?;
            if let (Availability::Expected(Some(observed)), Some(sidecar)) = (availability, sidecar)
            {
                let source_id: i64 = record.get(2).map_err(index)?;
                let ledger: String = record.get(3).map_err(index)?;
                let committed: Ledger = serde_json::from_str(&ledger).map_err(index)?;
                let evidence: Option<String> = record.get(6).map_err(index)?;
                let receiver_mutation: bool = record.get(7).map_err(index)?;
                // Discovery can precede this source's SQLite snapshot. If a
                // commit advanced the same carrier meanwhile, verify its new
                // stat through the native input opener before admitting it.
                let observed = if selected::stable_or_grown(&observed, &committed.stat)
                    && observed != committed.stat
                {
                    crate::open_input(std::path::Path::new(&path))
                        .and_then(|file| Stat::from_metadata(&file.metadata()?))
                        .unwrap_or(observed)
                } else {
                    observed
                };
                // Local writers do not use receiver intents. During growth, keep
                // the last committed metadata for the same carrier. Selected
                // body reads still verify the committed prefix independently.
                let local_growth = source.options.facts.is_none()
                    && !receiver_mutation
                    && selected::stable_or_grown(&committed.stat, &observed);
                if committed.stat == observed
                    || local_growth
                    || evidence
                        .as_deref()
                        .and_then(|value| serde_json::from_str::<AppendEvidence>(value).ok())
                        .is_some_and(|append| {
                            observed.size > committed.stat.size
                                && append.qualifies(&committed.stat, &observed)
                        })
                {
                    let eof: Option<String> = record.get(5).map_err(index)?;
                    digest.update(encode(&(&path, source_id, ledger, sidecar, eof))?);
                    coverage.committed += 1;
                    authorized.push(path);
                    continue;
                }
            }
            match availability {
                Availability::Expected(observed) => {
                    digest.update(encode(&(path, "pending", observed))?);
                    coverage.pending += 1;
                }
                Availability::Unavailable => {
                    digest.update(encode(&(path, "unavailable"))?);
                    coverage.unavailable += 1;
                }
            }
        }
        let paths = encode(&authorized)?;
        let repos = repositories(transaction, source, &paths)?;
        digest.update(encode(&repos)?);
        if let Some(names) = crate::events::store::catalog_names_json(transaction).map_err(index)? {
            digest.update(encode(&names)?);
        }
        source.validate_authority()?;
        Ok(Self {
            transaction,
            source,
            paths,
            repos,
            coverage,
            revision: format!("{:x}", digest.finalize()),
        })
    }

    fn candidates(
        &self,
        request: &Request,
        boundary: Option<&Boundary>,
    ) -> Result<(Vec<Row>, usize), ReadError> {
        use crate::catalog_search::MAX_CANDIDATES;
        let repo = encode(&self.repos)?;
        let machine = self.source.namespace.as_str();
        let (last, key, inclusive) = boundary.map_or((None, None, false), |b| {
            (
                Some(b.position.last),
                Some(b.position.key.as_str()),
                machine > b.position.source.as_str()
                    || (machine == b.position.source && b.inclusive),
            )
        });
        let limit = request.q.as_ref().map_or(request.limit, |_| MAX_CANDIDATES);
        let mut statement = self.transaction.prepare_cached(LIST).map_err(index)?;
        let mut records = statement
            .query(params![
                self.paths,
                self.source.namespace.binding(),
                repo,
                request.harness,
                request.repo,
                request.model,
                last,
                key,
                inclusive,
                (limit + 1) as i64
            ])
            .map_err(index)?;
        let mut candidates = Vec::new();
        let mut total = 0;
        while let Some(record) = records.next().map_err(index)? {
            let Some(key) = record.get::<_, Option<String>>(0).map_err(index)? else {
                total = usize::try_from(record.get::<_, i64>(8).map_err(index)?).map_err(index)?;
                continue;
            };
            candidates.push(read_row(
                self.transaction,
                self.source,
                record,
                key,
                request.q.is_some(),
            )?);
        }
        Ok((candidates, total))
    }

    fn runtime_status(
        &self,
        owners: &[i64],
        selected: bool,
        observed: &crate::native_evidence::RuntimeObservations,
    ) -> Result<BTreeMap<i64, &'static str>, ReadError> {
        use crate::facts::source_authority::CurrentSources;
        let options = self.source.options;
        let authority = options
            .facts
            .as_deref()
            .and_then(|path| CurrentSources::open(path).ok());
        if options.facts.is_some() && !authority.as_ref().is_some_and(CurrentSources::facts_known) {
            return Ok(BTreeMap::new());
        }
        let authorized: BTreeSet<String> = serde_json::from_str(&self.paths).map_err(index)?;
        let mut graph = parents::Parents::new(self.transaction)?;
        let mut native = self.transaction.prepare_cached(
            "SELECT native_id FROM dim_session WHERE source=?1 AND harness='claude' AND NOT lineage",
        ).map_err(index)?;
        let mut member_paths = self
            .transaction
            .prepare_cached(
                "SELECT c.path FROM lineage_member root
             CROSS JOIN lineage_member m ON m.component=root.component
             LEFT JOIN warehouse_current c ON c.source=m.source WHERE root.source=?1",
            )
            .map_err(index)?;
        let mut children = Vec::new();
        for &child in owners {
            let Some(native_id) = native
                .query_row([child], |row| row.get::<_, String>(0))
                .optional()
                .map_err(index)?
            else {
                continue;
            };
            let direct = graph.owner_parents(child)?;
            let Some(&parent) = direct.first().filter(|_| direct.len() == 1) else {
                continue;
            };
            let (ancestors, roots) = graph.roots(parent)?;
            let Some(&root) = roots.first().filter(|_| roots.len() == 1) else {
                continue;
            };
            let mut qualified = true;
            for ancestor in ancestors {
                let mut member_paths = member_paths.query([ancestor]).map_err(index)?;
                while let Some(row) = member_paths.next().map_err(index)? {
                    qualified &= row
                        .get::<_, Option<String>>(0)
                        .map_err(index)?
                        .is_some_and(|path| authorized.contains(&path));
                }
            }
            children.push(ChildRuntime {
                child,
                native_id,
                parent,
                root,
                qualified,
                call: graph.parent(child)?.and_then(|parent| parent.call),
            });
        }
        let mut dependencies: BTreeSet<i64> = owners.iter().copied().collect();
        dependencies.extend(
            children
                .iter()
                .filter(|child| child.qualified)
                .map(|child| child.root),
        );
        let dependencies = encode(&dependencies)?;
        let mut statement = self.transaction.prepare_cached(STATUS).map_err(index)?;
        let mut members = statement.query([dependencies]).map_err(index)?;
        let mut phases = self
            .transaction
            .prepare_cached(
                "SELECT phase FROM fact_codex_turn WHERE source=?1 AND thread_id=?2
             ORDER BY pos DESC,block DESC,occurrence DESC LIMIT 1",
            )
            .map_err(index)?;
        let mut status = BTreeMap::new();
        let mut unqualified = BTreeSet::new();
        let mut alive = BTreeSet::new();
        let mut pids = BTreeMap::new();
        while let Some(row) = members.next().map_err(index)? {
            let owner: i64 = row.get(0).map_err(index)?;
            let harness: String = row.get(1).map_err(index)?;
            let native_id: String = row.get(2).map_err(index)?;
            let path: String = row.get(3).map_err(index)?;
            let ledger: Ledger =
                serde_json::from_str(&row.get::<_, String>(4).map_err(index)?).map_err(index)?;
            let lineage: bool = row.get(5).map_err(index)?;
            let id: i64 = row.get(6).map_err(index)?;
            let source = ledger.source(std::path::Path::new(&path), &native_id);
            let custody =
                super::relative_source_custody(options, &harness, &source, authority.as_ref());
            if custody.is_err()
                || (harness == "codex" && options.facts.is_some() && custody != Ok(Some(true)))
            {
                unqualified.insert(owner);
                continue;
            }
            let process = observed.warehouse_process(&harness, &native_id);
            if process.is_some() {
                alive.insert(owner);
            }
            if harness == "claude" && lineage {
                status.entry(owner).or_insert("done");
                if let Some((pid, state)) = process
                    && pids.get(&owner).is_none_or(|previous| pid > *previous)
                {
                    pids.insert(owner, pid);
                    status.insert(owner, state);
                }
            } else if harness == "codex" {
                let live = process.is_some();
                if selected {
                    let state = crate::events::store::committed_codex_lifecycle(
                        self.transaction,
                        &path,
                        &ledger,
                        &native_id,
                    )
                    .map_err(index)?
                    .map_or("off", |turn| turn.state(live));
                    status.entry(owner).or_insert(state);
                    continue;
                }
                let phase: Option<String> = phases
                    .query_row(params![id, native_id], |r| r.get(0))
                    .optional()
                    .map_err(index)?;
                status
                    .entry(owner)
                    .or_insert(match (phase.as_deref(), live) {
                        (Some("active"), true) => "work",
                        (Some("completed" | "aborted"), true) => "idle",
                        (Some("completed" | "aborted"), false) => "done",
                        (Some("errored"), _) => "err",
                        _ => "off",
                    });
            }
        }
        let mut evidence = self
            .transaction
            .prepare_cached(include_str!(
                "events/store/warehouse_child_runtime_events.sql"
            ))
            .map_err(index)?;
        for child in children {
            if child.qualified
                && !unqualified.contains(&child.root)
                && !unqualified.contains(&child.child)
            {
                let root_alive = alive.contains(&child.root);
                let state = if selected {
                    child_runtime_state(&mut evidence, &child, root_alive)?
                } else if root_alive {
                    "work"
                } else {
                    "done"
                };
                status.insert(child.child, state);
            }
        }
        status.retain(|owner, _| !unqualified.contains(owner));
        if authority.is_some_and(|authority| authority.validate().is_err()) {
            status.clear();
        }
        Ok(status)
    }

    fn priced_usage(
        transaction: &Connection,
        paths: &str,
        owners: &str,
        table: &std::sync::Arc<crate::pricing::EffectiveTable>,
    ) -> Result<BTreeMap<i64, Usage>, ReadError> {
        let mut usage = transaction.prepare_cached(USAGE).map_err(index)?;
        let mut facts = usage.query(params![owners, paths]).map_err(index)?;
        let mut priced = BTreeMap::<i64, Usage>::new();
        while let Some(fact) = facts.next().map_err(index)? {
            priced
                .entry(fact.get(0).map_err(index)?)
                .or_insert_with(|| Usage {
                    tokens: [0; 6],
                    shared: false,
                    pricing: crate::pricing::CostBuild::with_table(table.clone()),
                })
                .account(fact)?;
        }
        Ok(priced)
    }

    fn enrich(
        &self,
        rows: &mut [Row],
        selected: bool,
        view: &selected::ReadView<'_>,
    ) -> Result<(), ReadError> {
        if rows.is_empty() {
            return Ok(());
        }
        let owners = rows.iter().map(|row| row.owner).collect::<Vec<_>>();
        let status = self.runtime_status(&owners, selected, view.runtime())?;
        self.enrich_with_status(rows, &status)
    }

    fn enrich_with_status(
        &self,
        rows: &mut [Row],
        status: &BTreeMap<i64, &'static str>,
    ) -> Result<(), ReadError> {
        if rows.is_empty() {
            return Ok(());
        }
        let owners = encode(&rows.iter().map(|row| row.owner).collect::<Vec<_>>())?;
        let table = crate::pricing::snapshot();
        let mut priced = Self::priced_usage(self.transaction, &self.paths, &owners, &table)?;
        let mut members = self
            .transaction
            .prepare_cached(PAGE_MEMBERS)
            .map_err(index)?;
        let mut records = members
            .query(params![owners, self.source.namespace.binding()])
            .map_err(index)?;
        let mut references = BTreeMap::<i64, Vec<crate::CatalogSourceObservation>>::new();
        while let Some(record) = records.next().map_err(index)? {
            let harness: String = record.get(1).map_err(index)?;
            let native_id = record.get(2).map_err(index)?;
            let carrier: String = record.get(3).map_err(index)?;
            let ledger =
                serde_json::from_str(&record.get::<_, String>(4).map_err(index)?).map_err(index)?;
            let path = selected::relative(self.source.options, &harness, &carrier)
                .ok_or(ReadError::Restart)?;
            references
                .entry(record.get(0).map_err(index)?)
                .or_default()
                .push(observation(&harness, path, native_id, ledger, "cached"));
        }
        let mut graph = parents::Parents::new(self.transaction)?;
        let authorized: BTreeSet<String> = serde_json::from_str(&self.paths).map_err(index)?;
        let mut carrier = self
            .transaction
            .prepare_cached("SELECT path FROM warehouse_current WHERE source=?1")
            .map_err(index)?;
        let mut edges = Vec::new();
        for row in rows.iter() {
            if let Some(parent) = graph.parent(row.owner)?
                && let Some(owner) = graph.owner(parent.source)?
                && carrier
                    .query_row([owner], |record| record.get::<_, String>(0))
                    .optional()
                    .map_err(index)?
                    .is_some_and(|path| authorized.contains(path.as_str()))
            {
                edges.push((row.owner, owner, parent.method));
            }
        }
        let parent_owners = edges.iter().map(|edge| edge.1).collect::<BTreeSet<_>>();
        let keys = relative_keys(self.transaction, &encode(&parent_owners)?)?
            .into_iter()
            .map(|(owner, key, _)| (owner, key))
            .collect::<BTreeMap<_, _>>();
        let mut parents = edges
            .into_iter()
            .filter_map(|(child, parent, method)| {
                keys.get(&parent).map(|key| (child, (key.clone(), method)))
            })
            .collect::<Vec<_>>();
        parents.sort_by(|a, b| b.1.cmp(&a.1));
        let mut parents = parents.into_iter().collect::<BTreeMap<_, _>>();
        for row in rows {
            let usage = priced.remove(&row.owner).unwrap_or_else(|| Usage {
                tokens: [0; 6],
                shared: false,
                pricing: crate::pricing::CostBuild::with_table(table.clone()),
            });
            row.usage = usage.tokens;
            let mut cost = usage.pricing.finish();
            let copilot = row.harness == "copilot";
            if copilot && cost.unpriced_models.is_empty() {
                cost.unpriced_models.push(if row.model == "—" {
                    "unknown".into()
                } else {
                    row.model.clone()
                });
            }
            if copilot || usage.shared {
                cost.usd = None;
            }
            row.cost = Some(cost);
            if let Some((parent, method)) = parents.remove(&row.owner) {
                row.parent = Some(parent);
                row.parent_source = (method != "native").then_some(method);
            }
            row.source_refs = references.remove(&row.owner).unwrap_or_default();
            row.native_ids = row
                .source_refs
                .iter()
                .map(|member| member.source.native_id.clone())
                .collect();
            row.status = status.get(&row.owner).copied();
        }
        Ok(())
    }
}

fn repositories(
    transaction: &Connection,
    source: &Source<'_>,
    paths: &str,
) -> Result<BTreeMap<String, String>, ReadError> {
    let mut repos = source.repos.clone();
    if source.inventory.is_empty()
        && let Some(recorded) = source
            .focused
            .as_ref()
            .and_then(|authority| authority.recorded.as_ref())
    {
        let facts = recorded.read_snapshot().map_err(index)?;
        if facts.version == crate::facts::FACTS_VERSION {
            repos.extend(
                facts
                    .repos
                    .into_iter()
                    .filter_map(|(cwd, repo)| repo.map(|repo| (cwd, repo))),
            );
        }
    }
    if source.focused.is_some() && source.options.facts.is_none() {
        let home = crate::facts::env_home();
        let mut directories = transaction
            .prepare_cached(
                "SELECT DISTINCT f.cwd FROM json_each(?1) path
                     CROSS JOIN files f ON f.path=path.value WHERE f.cwd IS NOT NULL",
            )
            .map_err(index)?;
        let mut rows = directories.query([paths]).map_err(index)?;
        while let Some(row) = rows.next().map_err(index)? {
            let cwd: String = row.get(0).map_err(index)?;
            if let Some(repo) =
                crate::model::repo_of(&cwd, home.as_deref().map(std::path::Path::new))
            {
                repos.insert(cwd, repo);
            }
        }
    }
    Ok(repos)
}

/// Qualify the current rows used by selected presentation. Parent runtime
/// can depend on ancestor components, while shared usage joins by native UUID.
fn selected_related_paths(
    transaction: &Connection,
    source: &Source<'_>,
    owners: &str,
    shared_usage: bool,
) -> Result<Vec<String>, ReadError> {
    let mut query = transaction
        .prepare_cached(include_str!("events/store/warehouse_selected_related.sql"))
        .map_err(index)?;
    let mut candidates = query
        .query(params![owners, source.namespace.binding(), shared_usage])
        .map_err(index)?;
    let mut paths = Vec::new();
    while let Some(row) = candidates.next().map_err(index)? {
        let path: String = row.get(0).map_err(index)?;
        let harness: String = row.get(1).map_err(index)?;
        let ledger: Ledger =
            serde_json::from_str(&row.get::<_, String>(2).map_err(index)?).map_err(index)?;
        let available = match source.availability(&harness, &path)? {
            Availability::Expected(Some(stat)) => selected::stable_or_grown(&ledger.stat, &stat),
            Availability::Expected(None) => source.current_provider.is_some(),
            Availability::Unavailable => false,
        };
        if available {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn relative_keys(
    transaction: &Connection,
    owners: &str,
) -> Result<Vec<(i64, String, String)>, ReadError> {
    let mut keys = transaction.prepare_cached(RELATIVE_KEYS).map_err(index)?;
    keys.query_map([owners], |record| {
        Ok((record.get(0)?, record.get(3)?, record.get(1)?))
    })
    .map_err(index)?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(index)
}

fn selected_runtime_status(
    transaction: &Connection,
    source: &Source<'_>,
    owner: i64,
    view: &selected::ReadView<'_>,
) -> Result<Option<&'static str>, ReadError> {
    let (ancestors, _) = parents::Parents::new(transaction)?.ancestors(owner)?;
    let authorized = std::iter::once(owner).chain(ancestors).collect::<Vec<_>>();
    let paths = encode(&selected_related_paths(
        transaction,
        source,
        &encode(&authorized)?,
        false,
    )?)?;
    let snapshot = Snapshot {
        transaction,
        source,
        paths,
        repos: BTreeMap::new(),
        coverage: Coverage::default(),
        revision: String::new(),
    };
    Ok(snapshot
        .runtime_status(&[owner], true, view.runtime())?
        .remove(&owner))
}

#[allow(clippy::too_many_arguments)]
fn selected_metadata(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    key: &str,
    owner: i64,
    harness: &str,
    references: &[crate::CatalogSourceObservation],
    native_titles: bool,
    latest_turn: Option<Option<&selected::causal::TurnStart>>,
) -> Result<Row, ReadError> {
    let (ancestors, runs, incomplete, mut rows, price) = {
        let tx = view.lock()?;
        let transaction = &*tx;
        list_functions(transaction).map_err(index)?;
        let (ancestors, runs, mut incomplete) =
            parents::Parents::new(transaction)?.relatives(owner)?;
        let visible = std::iter::once(&owner).chain(&ancestors).chain(&runs);
        let identities =
            relative_keys(transaction, &encode(&visible.copied().collect::<Vec<_>>())?)?;
        let owners = identities
            .iter()
            .map(|(owner, _, _)| owner)
            .collect::<Vec<_>>();
        let paths = encode(&selected_related_paths(
            transaction,
            source,
            &encode(&owners)?,
            true,
        )?)?;
        let repos = repositories(transaction, source, &paths)?;
        let mut statement = transaction
            .prepare_cached(SELECTED_METADATA)
            .map_err(index)?;
        let mut records = statement
            .query(params![
                paths,
                source.namespace.binding(),
                encode(&repos)?,
                encode(&identities)?,
            ])
            .map_err(index)?;
        let mut rows = Vec::new();
        while let Some(record) = records.next().map_err(index)? {
            rows.push(read_row(
                transaction,
                source,
                record,
                record.get(0).map_err(index)?,
                native_titles,
            )?);
        }
        incomplete |= rows.len() != owners.len();
        let snapshot = Snapshot {
            transaction,
            source,
            paths,
            repos,
            coverage: Coverage::default(),
            revision: String::new(),
        };
        snapshot.enrich(&mut rows, true, view)?;
        let price = match selected_family_price(transaction, source, owner) {
            Ok(price) => Some(price),
            Err(ReadError::Restart) => {
                incomplete = true;
                None
            }
            Err(error) => return Err(error),
        };
        (ancestors, runs, incomplete, rows, price)
    };
    for row in &mut rows {
        row.resolve_title(view, source)?;
    }
    let mut relatives = rows
        .into_iter()
        .map(|row| (row.owner, row))
        .collect::<BTreeMap<_, _>>();
    let mut row = relatives.remove(&owner).ok_or(ReadError::Restart)?;
    if row.position.key != key || row.harness != harness {
        return Err(ReadError::Restart);
    }
    let mut presentation = |owners: Vec<i64>| {
        owners.into_iter().filter_map(|owner| {
            let relative = relatives.remove(&owner)?;
            Some(serde_json::json!({
                "key": relative.position.key, "name": relative.title, "harness": relative.harness,
                "kind": relative.presentation_kind.unwrap_or(relative.kind), "parent": relative.parent,
                "last": (relative.position.last != 0).then_some(relative.position.last),
                "status": relative.recorded_status(), "cost": relative.cost,
            }))
        }).collect::<Vec<_>>()
    };
    let family = serde_json::json!({
        "state": if incomplete { "incomplete" } else { "complete" },
        "ancestors": presentation(ancestors),
        "runs": presentation(runs),
        "usd": price.as_ref().and_then(|price| price.usd),
        "shared_usd": price.map(|price| price.shared),
    });
    row.source_refs = references.to_vec();
    let resolved_turn = if latest_turn.is_none() && selected::live_pending(&row.harness, row.status)
    {
        selected::pending_boundary(view, source, key)?
    } else {
        None
    };
    let pending =
        selected::pending_counts_runtime(&row, latest_turn.unwrap_or(resolved_turn.as_ref()))?;
    row.details = {
        let tx = view.lock()?;
        Some(selected_details(&tx, source, &row, family, pending)?)
    };
    source.validate_authority()?;
    Ok(row)
}

fn selected_details(
    transaction: &Connection,
    source: &Source<'_>,
    row: &Row,
    family: serde_json::Value,
    pending: (bool, bool, String),
) -> Result<serde_json::Value, ReadError> {
    let (cwd, copilot, path, native_id, ledger) = transaction
        .query_row(
            "SELECT f.cwd,f.copilot,c.path,d.native_id,s.ledger FROM warehouse_current c
             CROSS JOIN files f USING(path) CROSS JOIN dim_session d USING(source)
             CROSS JOIN warehouse_source s ON s.source=c.source WHERE c.source=?1",
            [row.owner],
            |record| {
                Ok((
                    record.get::<_, Option<String>>(0)?,
                    record.get::<_, Option<String>>(1)?,
                    record.get::<_, String>(2)?,
                    record.get::<_, String>(3)?,
                    record.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(index)?;
    let turn_lifecycle = if row.harness == "codex" {
        let ledger = serde_json::from_str(&ledger).map_err(index)?;
        crate::events::store::committed_codex_lifecycle(transaction, &path, &ledger, &native_id)
            .map_err(index)?
    } else {
        None
    };
    let activity = if row.status == Some("work") {
        selected::active_tool(
            transaction,
            source,
            row.owner,
            &row.position.key,
            &row.harness,
        )?
        .map(|(tool, since)| serde_json::json!({"tool":tool,"since":since}))
    } else {
        None
    };
    let copilot = copilot
        .as_deref()
        .map(serde_json::from_str::<crate::copilot::Index>)
        .transpose()
        .map_err(index)?
        .map(|projection| projection.evidence());
    let (pending, work, boundary) = pending;
    let mut statement = transaction
        .prepare_cached(include_str!("events/store/warehouse_selected_details.sql"))
        .map_err(index)?;
    let mut records = statement
        .query(params![
            row.owner,
            crate::events::ACK,
            crate::events::UNKNOWN,
            pending,
            work,
            boundary
        ])
        .map_err(index)?;
    let (mut turns, mut calls, mut errors) = (0, 0, 0);
    let mut tools = BTreeMap::new();
    let mut signals = BTreeMap::new();
    while let Some(record) = records.next().map_err(index)? {
        let count = usize::try_from(record.get::<_, i64>(2).map_err(index)?).map_err(index)?;
        match record.get::<_, String>(0).map_err(index)?.as_str() {
            "turns" => turns = count,
            "call" => {
                let tool: String = record.get(1).map_err(index)?;
                *tools.entry(crate::model::tool_label(&tool)).or_default() += count;
                calls += count;
                errors +=
                    usize::try_from(record.get::<_, i64>(3).map_err(index)?).map_err(index)?;
            }
            "signal" => {
                signals.insert(record.get::<_, String>(1).map_err(index)?, count);
            }
            _ => return Err(index("Unknown selected detail count")),
        }
    }
    let tools = crate::model::capped_tool_counts(tools);
    Ok(serde_json::json!({
        "state": row.status.unwrap_or("off"), "cwd": cwd, "family": family,
        "turn_lifecycle": turn_lifecycle, "activity": activity,
        "turns": turns, "calls": calls, "errors": errors, "tool_calls": tools,
        "signals": signals, "cost": row.cost, "copied_usage": row.usage[3..].iter().any(|count| *count > 0),
        "live_observation": row.status.is_some(),
        "copilot": copilot, "reported_runs": [], "cost_check": [],
    }))
}

fn focused_transcript_observation(
    source: &Source<'_>,
    key: &str,
) -> Result<Option<selected::TranscriptObservation>, ReadError> {
    let view = selected::ReadView::open(source)?;
    let tx = view.lock()?;
    let Some(identity) = current_identity(&tx, source, key, None)? else {
        return Ok(None);
    };
    let Some(references) = focused_members(&tx, source, &identity)? else {
        return Ok(None);
    };
    let generation = current_identity_witness(&tx, source, key, &identity, &references)?;
    let status = selected_runtime_status(&tx, source, identity.0, &view)?;
    let total = selected::entry_count(&tx, source, &identity)?;
    Ok(Some(selected::TranscriptObservation {
        errors_generation: selected::current_errors_generation(&generation, status),
        generation,
        total,
    }))
}

fn focused_metadata(source: &Source<'_>, key: &str) -> Result<Option<(Row, String)>, ReadError> {
    let view = selected::ReadView::open(source)?;
    let (identity, references, witness) = {
        let tx = view.lock()?;
        let Some(identity) = current_identity(&tx, source, key, None)? else {
            return Ok(None);
        };
        let Some(references) = focused_members(&tx, source, &identity)? else {
            return Ok(None);
        };
        let witness = current_identity_witness(&tx, source, key, &identity, &references)?;
        (identity, references, witness)
    };
    let row = selected_metadata(
        &view,
        source,
        key,
        identity.0,
        &identity.1,
        &references,
        true,
        None,
    )?;
    Ok(Some((row, witness)))
}

/// Resolve native title and runtime only for roots that have a card.
pub(crate) fn qualify_stream_cards(
    options: &crate::Options,
    cards: Vec<crate::body_query::stream_cards::Candidate>,
) -> std::io::Result<Vec<crate::body_query::StreamCard>> {
    use crate::body_query::stream_cards::State;

    let path = crate::events::EventCache::path(&options.cache);
    let namespace = Namespace::new("local").map_err(std::io::Error::other)?;
    let source = Source::focused(namespace, &path, options, None).map_err(std::io::Error::other)?;
    let mut rows = Vec::with_capacity(cards.len());
    for card in cards {
        let Some((row, _)) =
            focused_metadata(&source, &card.catalog_key).map_err(std::io::Error::other)?
        else {
            continue;
        };
        if row.owner != card.owner
            || row.parent.is_some()
            || row.kind == "subagent"
            || row.presentation_kind == Some("Approval review")
        {
            continue;
        }
        let state = match row.status {
            Some("work" | "wait") => Some(State::Active),
            Some("idle") => Some(State::Idle),
            Some("done") => Some(State::Ended),
            _ => None,
        };
        rows.push(card.finish(&row.title, state));
    }
    Ok(rows)
}

fn read_row(
    db: &Connection,
    source: &Source<'_>,
    record: &rusqlite::Row<'_>,
    key: String,
    native_titles: bool,
) -> Result<Row, ReadError> {
    let mut row = Row {
        position: Position {
            last: record.get(3).map_err(index)?,
            key,
            source: source.namespace.as_str().to_owned(),
        },
        owner: record.get(1).map_err(index)?,
        harness: record.get(2).map_err(index)?,
        title: record.get(4).map_err(index)?,
        prompt_title: None,
        native_title: record.get(19).map_err(index)?,
        codex_native_id: None,
        model: record.get(5).map_err(index)?,
        effort: record.get(18).map_err(index)?,
        repo: record.get(6).map_err(index)?,
        branch: record.get(7).map_err(index)?,
        parent: None,
        parent_source: None,
        native_ids: BTreeSet::new(),
        source_refs: Vec::new(),
        start: record.get(15).map_err(index)?,
        kind: match record.get::<_, String>(2).map_err(index)?.as_str() {
            "claude" if record.get::<_, bool>(16).map_err(index)? => "session",
            "claude" => "subagent",
            "codex" => "codex-run",
            "copilot" => "copilot-session",
            _ => return Err(index("Unknown warehouse session harness")),
        },
        presentation_kind: None,
        usage: [0; 6],
        cost: None,
        status: None,
        details: None,
    };
    if row.harness == "codex" {
        row.codex_native_id = Some(record.get(12).map_err(index)?);
    } else if row.kind == "subagent" && !row.native_title {
        let carrier: String = record.get(10).map_err(index)?;
        let ledger: Ledger =
            serde_json::from_str(&record.get::<_, String>(11).map_err(index)?).map_err(index)?;
        if let Some(agent) =
            crate::events::store::committed_claude_agent(db, &carrier, &ledger).map_err(index)?
        {
            row.title = crate::model::one_line(agent.label.as_deref().unwrap_or("Subagent"), 80);
        }
    }
    let guardian: bool = record.get(17).map_err(index)?;
    row.presentation_kind = match row.kind {
        "subagent" => Some("Subagent"),
        "codex-run" if guardian => Some("Approval review"),
        "codex-run" => Some("Codex run"),
        "copilot-session" => Some("Copilot CLI"),
        _ => None,
    };
    if guardian {
        row.native_title = true;
        row.title = "Approval review".into();
    }
    if row.harness == "codex" && record.get::<_, bool>(14).map_err(index)? {
        row.native_title = true;
        let task: Option<String> = record.get(13).map_err(index)?;
        let prompt: Option<i64> = record.get(9).map_err(index)?;
        if let Some(offset) = prompt.filter(|_| native_titles) {
            row.prompt_title = Some(PromptTitle {
                carrier: record.get(10).map_err(index)?,
                ledger: record.get(11).map_err(index)?,
                native_id: record.get(12).map_err(index)?,
                offset,
            });
        } else if let Some(task) = task
            .as_deref()
            .and_then(|task| task.rsplit('/').next())
            .filter(|task| !task.trim().is_empty())
        {
            row.title = crate::model::one_line(task, 80);
        } else if row.title == "Codex run" {
            row.title = "Codex subagent".into();
        }
    }
    Ok(row)
}

fn child_runtime_state(
    evidence: &mut rusqlite::Statement<'_>,
    child: &ChildRuntime,
    running: bool,
) -> Result<&'static str, ReadError> {
    use crate::events::{AGM_DOWN, AGM_FINAL, ASYNC};
    let mut events = evidence
        .query(params![
            child.child,
            child.parent,
            child.native_id,
            child.call,
            AGM_FINAL,
            AGM_DOWN,
            ASYNC,
        ])
        .map_err(index)?;
    let mut prior = ChildRun::default();
    let mut run = ChildRun::default();
    let mut resumed = false;
    let mut synchronous = None;
    while let Some(row) = events.next().map_err(index)? {
        let kind: i64 = row.get(0).map_err(index)?;
        let failed: bool = row.get(1).map_err(index)?;
        match kind {
            0 if resumed && !run.ended() && prior.handback && !prior.final_receipt => {
                prior.final_receipt = true;
            }
            0 => run.final_receipt = true,
            1 => run.handback = true,
            2 if resumed
                && !failed
                && !run.ended()
                && (prior.handback || prior.final_receipt)
                && prior.notification.is_none() =>
            {
                prior.notification = Some(false)
            }
            2 => {
                run.notification.get_or_insert(failed);
            }
            3 if run.ended() => {
                prior = std::mem::take(&mut run);
                resumed = true;
            }
            3 => {}
            4 => synchronous = Some(failed),
            _ => return Err(index("Unknown warehouse child runtime event")),
        }
    }
    let working = if running { "work" } else { "done" };
    let finished = if run.ended() {
        Some(run.notification.unwrap_or(false))
    } else {
        synchronous
    };
    Ok(match (resumed && !run.ended(), finished) {
        (true, _) | (_, None) => working,
        (_, Some(true)) => "err",
        (_, Some(false)) => "done",
    })
}

fn list_functions(db: &Connection) -> rusqlite::Result<()> {
    db.create_scalar_function(
        "semon_claude_project",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            let path = context.get::<String>(0)?;
            let slug = std::path::Path::new(&path)
                .parent()
                .and_then(std::path::Path::file_name)
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or_default();
            Ok(crate::model::claude_project_name(slug).to_owned())
        },
    )
}

#[cfg(test)]
thread_local! {
    static AFTER_LIST_SNAPSHOT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn after_list_snapshot(hook: impl FnOnce() + 'static) {
    AFTER_LIST_SNAPSHOT.with(|slot| slot.replace(Some(Box::new(hook))));
}

#[cfg(test)]
pub(crate) fn after_transcript_body(hook: impl FnOnce(usize) + 'static) {
    selected::AFTER_BODY.with(|slot| slot.replace(Some(Box::new(hook))));
}

const NATIVE_QUERY_RELAY_PEERS: &str = "
WITH members(source) AS MATERIALIZED (
    SELECT member.source FROM json_each(?1) owners
    JOIN lineage_member root ON root.source=owners.value
    JOIN lineage_member member ON member.component=root.component
), peers(source) AS (
    SELECT sent.source FROM members m
    JOIN fact_relay_receipt receipt ON receipt.source=m.source
    JOIN fact_relay_result sent ON sent.correlation=receipt.correlation
        AND sent.correlation IS NOT NULL AND NOT sent.failed
    JOIN fact_relay_send call ON call.source=sent.source AND call.call_id=sent.call_id
        AND (sent.call_pos IS NULL OR (call.pos=sent.call_pos AND call.block=sent.call_block))
    UNION
    SELECT receipt.source FROM members m
    JOIN fact_relay_send call ON call.source=m.source
    JOIN fact_relay_result sent ON sent.source=call.source AND sent.call_id=call.call_id
        AND NOT sent.failed AND (sent.call_pos IS NULL OR (call.pos=sent.call_pos AND call.block=sent.call_block))
    JOIN fact_relay_receipt receipt ON receipt.correlation=sent.correlation
    UNION
    SELECT sender.source FROM members m
    JOIN fact_subagent_event receipt ON receipt.source=m.source AND receipt.kind='agm'
    JOIN dim_session sender ON sender.harness='claude'
        AND sender.native_id IN (receipt.id,receipt.peer_sock)
    WHERE (receipt.flags & ?2)<>0
    UNION
    SELECT sent.source FROM members m
    JOIN dim_session receiver ON receiver.source=m.source AND receiver.harness='claude' AND NOT receiver.lineage
    JOIN fact_relay_send sent ON sent.target=receiver.native_id
    UNION
    SELECT receiver.source FROM members m
    JOIN fact_relay_send sent ON sent.source=m.source
    JOIN dim_session receiver ON receiver.harness='claude' AND NOT receiver.lineage
        AND receiver.native_id=sent.target
)
SELECT DISTINCT ownership.owner FROM peers
JOIN warehouse_current current ON current.source=peers.source
JOIN bridge_session_owner ownership ON ownership.source=peers.source";

// Include the native ID behind a qualified key, every literal spelling that
// could reserve that key, and all harness owners of those IDs. The model
// assigns aliases from the in-window files, after admission can find collisions.
fn native_query_ids(key: &str) -> BTreeSet<&str> {
    let mut ids = BTreeSet::from([key]);
    let mut bare = key;
    while let Some(rest) = bare.strip_prefix(':') {
        ids.insert(rest);
        bare = rest;
    }
    if let Some((harness, id)) = bare.split_once(':')
        && crate::InputRoot::parse(harness).is_some()
    {
        ids.insert(id);
    }
    ids
}

// These are candidates. Native derivation decides whether a receipt pairs
// with a send or an actual agent before it creates a virtual peer.
fn native_query_peer_owners(db: &Connection, key: &str) -> Result<BTreeSet<i64>, ReadError> {
    let mut candidates = db
        .prepare_cached(
            "WITH peers(source,name,pid,start,sock) AS (
                SELECT source,target,NULL,NULL,NULL FROM fact_relay_send
                UNION ALL
                SELECT source,name,pid,start,sock FROM fact_relay_receipt
                UNION ALL
                SELECT source,coalesce(name,id),peer_pid,peer_start,peer_sock
                FROM fact_subagent_event WHERE kind='agm' AND (flags & ?1)<>0
            )
            SELECT DISTINCT ownership.owner,native.harness,coalesce(peer.name,''),peer.pid,peer.start,peer.sock
            FROM peers peer
            JOIN warehouse_current current ON current.source=peer.source
            JOIN dim_session native ON native.source=peer.source
            JOIN bridge_session_owner ownership ON ownership.source=peer.source",
        )
        .map_err(index)?;
    let mut rows = candidates
        .query([crate::events::AGM_PROGRESS])
        .map_err(index)?;
    let mut owners = BTreeSet::new();
    while let Some(row) = rows.next().map_err(index)? {
        let name: String = row.get(2).map_err(index)?;
        let peer = match row.get_ref(1).map_err(index)?.as_str().map_err(index)? {
            "claude" => {
                let pid: Option<i64> = row.get(3).map_err(index)?;
                let start: Option<String> = row.get(4).map_err(index)?;
                let socket: Option<String> = row.get(5).map_err(index)?;
                crate::model::claude_peer_id(
                    &name,
                    pid.map(|pid| pid as u64),
                    start.as_deref(),
                    socket.as_deref(),
                )
            }
            "codex" => crate::model::codex_peer_id(&name),
            _ => continue,
        };
        if key == peer {
            owners.insert(row.get(0).map_err(index)?);
        }
    }
    Ok(owners)
}

fn native_query_paths(
    db: &Connection,
    source: &Source<'_>,
    key: &str,
    ids: &str,
    activity_only: bool,
) -> Result<Vec<String>, ReadError> {
    // Match members as well as owners: an in-window continuation can become
    // the native owner when the first file in its component ages out.
    let mut aliases = db
        .prepare_cached(
            "SELECT DISTINCT b.owner FROM json_each(?1) ids
             JOIN dim_session d ON d.native_id=ids.value
             JOIN bridge_session_owner b ON b.source=d.source",
        )
        .map_err(index)?;
    let mut owners = aliases
        .query_map([ids], |row| row.get::<_, i64>(0))
        .map_err(index)?
        .collect::<rusqlite::Result<BTreeSet<_>>>()
        .map_err(index)?;
    if owners.is_empty() && !activity_only {
        let bare = key.rsplit_once('@').map_or(key, |(bare, _)| bare);
        if let Some(target) = bare.strip_prefix("unsent:") {
            let mut sends = db
                .prepare_cached(
                    "SELECT DISTINCT ownership.owner FROM fact_relay_send send
                 JOIN warehouse_current current ON current.source=send.source
                 JOIN bridge_session_owner ownership ON ownership.source=send.source
                 WHERE send.target=?1",
                )
                .map_err(index)?;
            owners.extend(
                sends
                    .query_map([target], |row| row.get::<_, i64>(0))
                    .map_err(index)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(index)?,
            );
        } else if bare.starts_with("peer-") {
            owners.extend(native_query_peer_owners(db, bare)?);
        }
    }
    if !activity_only {
        let selected = encode(&owners)?;
        let mut parents = parents::Parents::new(db)?;
        for owner in owners.clone() {
            owners.extend(parents.ancestors(owner)?.0);
            owners.extend(parents.owner_children(owner)?);
        }
        let mut relays = db.prepare_cached(NATIVE_QUERY_RELAY_PEERS).map_err(index)?;
        for owner in relays
            .query_map(params![selected, crate::events::AGM_PROGRESS], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(index)?
        {
            owners.insert(owner.map_err(index)?);
        }
    }
    selected_related_paths(db, source, &encode(&owners)?, true)
}

/// Native Query only renders sources inside the scan window. Match those
/// presentation keys to the full committed lineage without reading older logs.
fn native_lineage_aliases(
    db: &Connection,
    namespace: &Namespace,
    paths: &str,
    data: &mut crate::model::QueryData,
) -> rusqlite::Result<()> {
    let mut statement = db.prepare_cached(
        "SELECT DISTINCT active.harness, active.native_id, alias.native_id
         FROM json_each(?1) admitted
         JOIN warehouse_current c ON c.path=admitted.value
         JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
         JOIN dim_session active ON active.source=c.source
         JOIN lineage_member m ON m.source=c.source
         JOIN lineage_member peer ON peer.component=m.component AND peer.source<>m.source
         JOIN warehouse_source peer_source ON peer_source.source=peer.source AND peer_source.origin=s.origin
         JOIN dim_session alias ON alias.source=peer.source AND alias.harness=active.harness",
    )?;
    let mut rows = statement.query(params![paths, namespace.binding()])?;
    let mut aliases = BTreeMap::<_, Option<&str>>::new();
    while let Some(row) = rows.next()? {
        let harness: String = row.get(0)?;
        let Some(harness) = crate::InputRoot::parse(&harness).map(crate::InputRoot::as_str) else {
            continue;
        };
        let active: String = row.get(1)?;
        let alias: String = row.get(2)?;
        let Some(key) = data.native_keys.get(&(harness, active)) else {
            continue;
        };
        let alias = (harness, alias);
        if data.native_keys.contains_key(&alias) {
            continue;
        }
        aliases
            .entry(alias)
            .and_modify(|prior| {
                if *prior != Some(key.as_str()) {
                    *prior = None;
                }
            })
            .or_insert(Some(key.as_str()));
    }
    let aliases: Vec<_> = aliases
        .into_iter()
        .filter_map(|(identity, key)| Some((identity, key?.to_owned())))
        .collect();
    data.native_keys.extend(aliases);
    Ok(())
}

/// Native query wire fields still use the shared session model. The warehouse
/// selects and qualifies its native Index inputs before any event rows load.
pub(crate) fn native_query(
    options: &crate::Options,
    namespace: &Namespace,
    now: i64,
    focus: Option<&str>,
    activity_only: bool,
) -> std::io::Result<crate::model::QueryData> {
    use crate::events::EventCache;
    use crate::model::{catalog_source_in_window, scan_cutoff};

    let cutoff = scan_cutoff(options, now);
    let ids = focus.map(native_query_ids).unwrap_or_default();
    let encoded_ids = encode(&ids).map_err(std::io::Error::other)?;
    let mut prior_paths = BTreeSet::new();
    if let Some(key) = focus {
        let path = EventCache::path(&options.cache);
        if path.try_exists()? {
            let source = Source::focused(namespace.clone(), &path, options, None)
                .map_err(std::io::Error::other)?;
            if let Ok(view) = selected::ReadView::open(&source) {
                let tx = view.lock().map_err(std::io::Error::other)?;
                prior_paths.extend(
                    native_query_paths(&tx, &source, key, &encoded_ids, activity_only)
                        .map_err(std::io::Error::other)?,
                );
            }
        }
    }
    let skipped = crate::origin_projection::admit_namespace(
        options,
        namespace,
        cutoff,
        focus.is_some() && !activity_only,
        true,
        |input, path| {
            if focus.is_none() {
                return Ok(true);
            }
            if path.to_str().is_some_and(|path| prior_paths.contains(path)) {
                return Ok(true);
            }
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let matches = |id: &str| ids.contains(id);
            match input.root {
                crate::InputRoot::Claude => {
                    if crate::native_sidecar::agent_path(path) {
                        let parent = path
                            .parent()
                            .and_then(std::path::Path::parent)
                            .and_then(std::path::Path::file_name)
                            .and_then(|name| name.to_str());
                        Ok(matches(name.strip_prefix("agent-").unwrap_or(name))
                            || (!activity_only && parent.is_some_and(matches)))
                    } else {
                        Ok(matches(name))
                    }
                }
                crate::InputRoot::Codex => {
                    let found = std::fs::metadata(path)
                        .and_then(|stat| crate::codex_native_id_prefix(path, stat.len()));
                    Ok(found.ok().flatten().as_deref().is_some_and(matches))
                }
                crate::InputRoot::Copilot => Ok(path
                    .parent()
                    .and_then(std::path::Path::file_name)
                    .and_then(|name| name.to_str())
                    .is_some_and(matches)),
            }
        },
        || crate::selected_read::check().is_err(),
    )?;
    // Admission may have atomically switched the active Index during recovery.
    let path = EventCache::path(&options.cache);
    let source = if focus.is_some() {
        Source::focused(namespace.clone(), &path, options, None)
    } else {
        Source::discover(namespace.clone(), &path, options)
    }
    .map_err(std::io::Error::other)?;
    let view = selected::ReadView::open(&source).map_err(std::io::Error::other)?;
    let db = view.lock().map_err(std::io::Error::other)?;
    let paths = if let Some(key) = focus {
        let paths = native_query_paths(&db, &source, key, &encoded_ids, activity_only)
            .map_err(std::io::Error::other)?;
        encode(&paths).map_err(std::io::Error::other)?
    } else {
        Snapshot::read(&db, &source)
            .map_err(std::io::Error::other)?
            .paths
    };
    let mut records = db
        .prepare(
            "SELECT s.harness,c.path,s.ledger,d.native_id
         FROM json_each(?1) admitted JOIN warehouse_current c ON c.path=admitted.value
         JOIN warehouse_source s ON s.source=c.source AND s.carrier=c.path AND s.origin=?2
         JOIN dim_session d ON d.source=c.source
         ORDER BY s.harness,c.path",
        )
        .map_err(std::io::Error::other)?;
    let mut rows = records
        .query(params![paths, namespace.binding()])
        .map_err(std::io::Error::other)?;
    let mut sources = Vec::new();
    let mut ids = BTreeSet::new();
    let mut usage = BTreeMap::new();
    while let Some(row) = rows.next().map_err(std::io::Error::other)? {
        let harness: String = row.get(0).map_err(std::io::Error::other)?;
        let path: String = row.get(1).map_err(std::io::Error::other)?;
        let ledger: Ledger =
            serde_json::from_str(&row.get::<_, String>(2).map_err(std::io::Error::other)?)
                .map_err(std::io::Error::other)?;
        let native_id: String = row.get(3).map_err(std::io::Error::other)?;
        let member = ledger.source(std::path::Path::new(&path), &native_id);
        if !catalog_source_in_window(&member, cutoff) {
            continue;
        }
        if harness == "codex"
            && crate::events::store::warehouse_has_codex_usage(&db, &path, &ledger)
                .map_err(std::io::Error::other)?
        {
            let steps = crate::events::store::warehouse_codex_activity(&db, &path, &ledger)
                .map_err(std::io::Error::other)?;
            usage.insert(member.path.clone(), steps);
        }
        ids.insert(native_id);
        sources.push((harness, member));
    }
    let members: Vec<_> = sources.iter().map(|(_, member)| member.clone()).collect();
    let mut cache =
        EventCache::from_selected(&db, &members, &ids).map_err(std::io::Error::other)?;
    let mut data =
        crate::model::query_warehouse(options, &mut cache, sources, skipped, &usage, now, focus)?;
    if focus.is_none() && cutoff.is_some() {
        native_lineage_aliases(&db, namespace, &paths, &mut data).map_err(std::io::Error::other)?;
    }
    source.validate_authority().map_err(std::io::Error::other)?;
    Ok(data)
}

pub(super) fn read_list(
    sources: &[Source<'_>],
    workspace: &str,
    request: &Request,
    cursor: Option<&Continuation>,
    workspace_read: bool,
) -> Result<Page, ReadError> {
    debug_assert_eq!(request.scope, CatalogReadScope::Current);
    read_pages(
        sources,
        workspace,
        request,
        cursor,
        workspace_read,
        |row| &row.position,
        |view, source, boundary| {
            let (candidates, count, paths, repos, coverage, revision) = {
                let tx = view.lock()?;
                #[cfg(test)]
                let phase = crate::catalog::baseline_start();
                let snapshot = Snapshot::read(&tx, source)?;
                #[cfg(test)]
                crate::catalog::baseline_record(phase, |sample| &mut sample.runtime_and_custody);
                #[cfg(test)]
                AFTER_LIST_SNAPSHOT.with(|hook| {
                    if let Some(hook) = hook.borrow_mut().take() {
                        hook();
                    }
                });
                #[cfg(test)]
                let phase = crate::catalog::baseline_start();
                let (candidates, count) = snapshot.candidates(request, boundary)?;
                #[cfg(test)]
                crate::catalog::baseline_record(phase, |sample| &mut sample.selection);
                (
                    candidates,
                    count,
                    snapshot.paths,
                    snapshot.repos,
                    snapshot.coverage,
                    snapshot.revision,
                )
            };
            #[cfg(test)]
            let phase = crate::catalog::baseline_start();
            let (mut rows, scanned, next) = select_list_rows(view, source, request, candidates)?;
            #[cfg(test)]
            crate::catalog::baseline_record(phase, |sample| &mut sample.title_and_presentation);
            let tx = view.lock()?;
            let snapshot = Snapshot {
                transaction: &tx,
                source,
                paths,
                repos,
                coverage,
                revision,
            };
            #[cfg(test)]
            let phase = crate::catalog::baseline_start();
            snapshot.enrich(&mut rows, false, view)?;
            #[cfg(test)]
            crate::catalog::baseline_record(phase, |sample| &mut sample.runtime_and_custody);
            Ok((
                (rows, count, scanned, next),
                snapshot.coverage,
                snapshot.revision,
            ))
        },
    )
}

fn select_list_rows(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    request: &Request,
    candidates: Vec<Row>,
) -> Result<(Vec<Row>, usize, Option<Boundary>), ReadError> {
    use crate::catalog_search::{MAX_CANDIDATES, MAX_SCAN_BYTES};
    let (mut rows, mut examined, mut bytes) = (Vec::new(), 0, 0);
    for mut row in candidates {
        if rows.len() == request.limit || examined == MAX_CANDIDATES || bytes >= MAX_SCAN_BYTES {
            return Ok((
                rows,
                examined,
                Some(Boundary {
                    position: row.position,
                    inclusive: true,
                }),
            ));
        }
        row.resolve_title(view, source)?;
        let fields = [
            Some(row.title.as_str()),
            Some(row.position.key.as_str()),
            row.repo.as_deref(),
            row.branch.as_deref(),
            Some(row.model.as_str()),
            Some(row.harness.as_str()),
        ];
        examined += 1;
        bytes += fields
            .iter()
            .flatten()
            .map(|field| field.len())
            .sum::<usize>();
        if request.q.as_ref().is_none_or(|q| {
            fields
                .into_iter()
                .flatten()
                .any(|field| field.to_lowercase().contains(q))
        }) {
            rows.push(row);
        }
    }
    Ok((rows, examined, None))
}

struct RetainedRow {
    position: Position,
    metadata: serde_json::Value,
    identity: crate::CatalogSessionIdentity,
}

fn read_retained_list(
    sources: &[Source<'_>],
    workspace: &str,
    request: &Request,
    cursor: Option<&Continuation>,
    workspace_read: bool,
) -> Result<Page<RetainedRow>, ReadError> {
    debug_assert_eq!(request.scope, CatalogReadScope::RetainedHistory);
    read_pages(
        sources,
        workspace,
        request,
        cursor,
        workspace_read,
        |row| &row.position,
        |view, source, boundary| {
            let tx = view.lock()?;
            retained_rows(&tx, source, request, boundary)
        },
    )
}

fn retained_rows(
    tx: &Connection,
    source: &Source<'_>,
    request: &Request,
    boundary: Option<&Boundary>,
) -> Result<SourcePage<RetainedRow>, ReadError> {
    use crate::catalog_search::{MAX_CANDIDATES, MAX_SCAN_BYTES};
    crate::events::store::require_current(tx).map_err(index)?;
    crate::storage::sqlite::qualify(tx, &source.namespace).map_err(index)?;
    let machine = source.namespace.as_str();
    let origin = source.namespace.binding();
    let incarnation: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='incarnation'",
            [],
            |row| row.get(0),
        )
        .map_err(index)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM warehouse_identity WHERE scope='retained_history' AND source_key=?1 AND retained=1",
        [origin], |row| row.get(0),
    ).map_err(index)?;
    // A frozen binding is written at most once per key and lasts for this Index incarnation.
    let coverage = Coverage {
        committed: usize::try_from(count).map_err(index)?,
        ..Default::default()
    };
    let revision = format!(
        "{:x}",
        Sha256::digest(encode(&(origin, incarnation, count))?)
    );
    let (last, key, inclusive) = boundary.map_or((None, None, false), |b| {
        (
            Some(b.position.last),
            Some(b.position.key.as_str()),
            machine > b.position.source.as_str() || (machine == b.position.source && b.inclusive),
        )
    });
    let limit = request.q.as_ref().map_or(request.limit, |_| MAX_CANDIDATES);
    let mut statement = tx
        .prepare_cached(include_str!("events/store/warehouse_retained_list.sql"))
        .map_err(index)?;
    let mut records = statement
        .query(params![
            origin,
            request.harness,
            request.repo,
            request.model,
            last,
            key,
            inclusive,
            (limit + 1) as i64
        ])
        .map_err(index)?;
    let (mut selected, mut total, mut examined, mut bytes, mut next) = (Vec::new(), 0, 0, 0, None);
    while let Some(record) = records.next().map_err(index)? {
        let Some(key) = record.get::<_, Option<String>>(0).map_err(index)? else {
            total = usize::try_from(record.get::<_, i64>(3).map_err(index)?).map_err(index)?;
            continue;
        };
        if next.is_some() {
            continue;
        }
        let position = Position {
            key,
            source: machine.to_owned(),
            last: record.get(1).map_err(index)?,
        };
        if selected.len() == request.limit || examined == limit || bytes >= MAX_SCAN_BYTES {
            next = Some(Boundary {
                position,
                inclusive: true,
            });
            continue;
        }
        let saved: String = record.get(2).map_err(index)?;
        let metadata: serde_json::Value = serde_json::from_str(&saved).map_err(index)?;
        let fields =
            ["name", "key", "repo", "branch", "model", "harness"].map(|key| metadata[key].as_str());
        examined += 1;
        bytes += fields
            .iter()
            .flatten()
            .map(|field| field.len())
            .sum::<usize>();
        if request.q.as_ref().is_some_and(|q| {
            !fields
                .into_iter()
                .flatten()
                .any(|field| field.to_lowercase().contains(q))
        }) {
            continue;
        }
        let (metadata, identity) = retained_metadata_in_snapshot(tx, source, &position.key)?;
        selected.push(RetainedRow {
            position,
            metadata,
            identity,
        });
    }
    Ok(((selected, total, examined, next), coverage, revision))
}

fn read_pages<T>(
    sources: &[Source<'_>],
    workspace: &str,
    request: &Request,
    cursor: Option<&Continuation>,
    workspace_read: bool,
    position: fn(&T) -> &Position,
    read: impl Fn(
        &selected::ReadView<'_>,
        &Source<'_>,
        Option<&Boundary>,
    ) -> Result<SourcePage<T>, ReadError>,
) -> Result<Page<T>, ReadError> {
    let mut ordered: Vec<_> = sources.iter().collect();
    ordered.sort_by(|a, b| a.namespace.binding().cmp(b.namespace.binding()));
    if ordered.len() > 64
        || ordered
            .windows(2)
            .any(|pair| pair[0].namespace.binding() == pair[1].namespace.binding())
        || request.sid.is_some()
        || request.cursor.is_some()
    {
        return Err(ReadError::Scope);
    }
    let query = format!(
        "{:x}",
        Sha256::digest(encode(&(
            workspace,
            request.scope,
            &request.harness,
            &request.repo,
            &request.model,
            &request.q,
            ordered
                .iter()
                .map(|source| source.namespace.as_str())
                .collect::<Vec<_>>(),
        ))?),
    );
    if cursor.is_some_and(|cursor| cursor.query != query) {
        return Err(ReadError::Scope);
    }
    let mut digest = Sha256::new();
    let (mut rows, mut total, mut examined, mut coverage, mut frontier) =
        (Vec::new(), 0, 0, Coverage::default(), None::<Boundary>);
    let allow_pending = workspace_read || request.scope == CatalogReadScope::Current;
    for source in ordered {
        let db = match Connection::open_with_flags(
            source.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(db) => db,
            Err(error) => {
                if allow_pending
                    && std::fs::metadata(source.path)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                {
                    digest.update(encode(&(source.namespace.binding(), "pending"))?);
                    coverage.pending += source.inventory.len().max(1);
                    source.validate_authority()?;
                    continue;
                }
                return Err(index(error));
            }
        };
        list_functions(&db).map_err(index)?;
        db.execute_batch("BEGIN DEFERRED").map_err(index)?;
        if allow_pending && !crate::events::store::current_or_fresh(&db).map_err(index)? {
            digest.update(encode(&(source.namespace.binding(), "pending"))?);
            coverage.pending += source.inventory.len().max(1);
            source.validate_authority()?;
            continue;
        }
        let view = selected::ReadView::from_connection(db, source);
        let ((local, count, scanned, local_next), source_coverage, revision) =
            read(&view, source, cursor.map(|cursor| &cursor.boundary))?;
        digest.update(revision);
        source.validate_authority()?;
        rows.extend(local);
        rows.sort_by(|a, b| position(a).cmp(position(b)));
        if rows.len() > request.limit {
            let position = position(&rows[request.limit]).clone();
            if frontier.as_ref().is_none_or(|old| position < old.position) {
                frontier = Some(Boundary {
                    position,
                    inclusive: true,
                });
            }
            rows.truncate(request.limit);
        }
        if let Some(next) = local_next
            && frontier
                .as_ref()
                .is_none_or(|old| next.position < old.position)
        {
            frontier = Some(next);
        }
        total += count;
        examined += scanned;
        coverage.committed += source_coverage.committed;
        coverage.pending += source_coverage.pending;
        coverage.unavailable += source_coverage.unavailable;
    }
    let revision = format!("{:x}", digest.finalize());
    if cursor.is_some_and(|cursor| cursor.revision != revision) {
        return Err(ReadError::Restart);
    }
    if let Some(next) = &frontier {
        rows.retain(|row| position(row) < &next.position);
    }
    let next = frontier.map(|mut boundary| {
        if request.q.is_none()
            && let Some(last) = rows.last()
        {
            boundary = Boundary {
                position: position(last).clone(),
                inclusive: false,
            };
        }
        Continuation {
            revision: revision.clone(),
            query,
            boundary,
        }
    });
    Ok(Page {
        rows,
        total,
        coverage,
        examined,
        next,
        generation: revision,
    })
}

#[cfg(test)]
pub(super) fn identity(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
) -> Result<Option<Identity>, ReadError> {
    let mut db =
        Connection::open_with_flags(source.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    if scope == CatalogReadScope::RetainedHistory {
        return warehouse::retained_identity(&tx, &source.namespace, key)
            .map(|identity| Some((identity.owner, identity.harness, identity.native_id)))
            .map_err(retained_error);
    }
    if source.focused.is_some() {
        crate::events::store::require_current(&tx).map_err(index)?;
        crate::storage::sqlite::qualify(&tx, &source.namespace).map_err(index)?;
        let Some(identity) = current_identity(&tx, source, key, None)? else {
            return Ok(None);
        };
        return Ok(focused_members(&tx, source, &identity)?.map(|_| identity));
    }
    let snapshot = Snapshot::read(&tx, source)?;
    current_identity(&tx, source, key, Some(&snapshot.paths))
}

fn observation(
    harness: &str,
    path: &str,
    native_id: String,
    ledger: Ledger,
    state: &str,
) -> crate::CatalogSourceObservation {
    crate::CatalogSourceObservation {
        source: crate::SessionSourceRef {
            root: harness.to_owned(),
            path: path.to_owned(),
            native_id,
            offset: ledger.offset,
            prefix_sha256: ledger.prefix,
            tail_sha256: ledger.tail,
        },
        state: state.to_owned(),
    }
}

fn focused_members(
    db: &Connection,
    source: &Source<'_>,
    (owner, harness, _): &Identity,
) -> Result<Option<Vec<crate::CatalogSourceObservation>>, ReadError> {
    let mut members = db
        .prepare_cached(
            "SELECT s.carrier,s.ledger,d.native_id,f.path IS NOT NULL FROM lineage_member root
         CROSS JOIN lineage_member m ON m.component=root.component
         LEFT JOIN warehouse_source s ON s.source=m.source AND s.origin=?2 AND s.harness=?3
         LEFT JOIN dim_session d ON d.source=m.source
         LEFT JOIN warehouse_current c ON c.source=s.source AND c.path=s.carrier
         LEFT JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
         WHERE root.source=?1 ORDER BY m.source",
        )
        .map_err(index)?;
    let mut rows = members
        .query(params![owner, source.namespace.binding(), harness])
        .map_err(index)?;
    let mut references = Vec::new();
    while let Some(row) = rows.next().map_err(index)? {
        if !row.get::<_, bool>(3).map_err(index)? {
            return Ok(None);
        }
        let carrier: String = row.get(0).map_err(index)?;
        let ledger: Ledger =
            serde_json::from_str(&row.get::<_, String>(1).map_err(index)?).map_err(index)?;
        let Some(path) = selected::relative(source.options, harness, &carrier) else {
            return Ok(None);
        };
        let state = match source.availability(harness, &carrier)? {
            Availability::Expected(Some(stat))
                if selected::stable_or_grown(&ledger.stat, &stat) =>
            {
                "cached"
            }
            Availability::Expected(None) if source.current_provider.is_some() => "unavailable",
            _ => return Ok(None),
        };
        references.push(observation(
            harness,
            path,
            row.get(2).map_err(index)?,
            ledger,
            state,
        ));
    }
    source.validate_authority()?;
    Ok((!references.is_empty()).then_some(references))
}

pub(crate) fn host_source(options: &crate::Options, key: &str) -> crate::OverviewSource {
    crate::OverviewSource {
        key: key.into(),
        label: key.into(),
        options: options.clone(),
        up: true,
    }
}

pub(super) fn host_source_proof(
    options: &crate::Options,
    source_key: &str,
    harness: &str,
    relative: &str,
) -> Result<Option<crate::SessionSourceProof>, crate::CatalogIdentityError> {
    use crate::CatalogIdentityError as Error;
    let namespace = Namespace::new(source_key).map_err(|_| Error::InvalidArguments)?;
    let root = crate::InputRoot::parse(harness).ok_or(Error::InvalidArguments)?;
    if relative.len() > 4096 || !crate::is_input_path(harness, relative) {
        return Err(Error::InvalidArguments);
    }
    let carrier = root.home(options).join(relative);
    let carrier = carrier.to_str().ok_or(Error::InvalidArguments)?;
    if crate::index_rebuild::status(&options.cache).is_some() {
        return Err(Error::Unavailable);
    }
    let path = crate::events::EventCache::path(&options.cache);
    let source =
        Source::focused(namespace, &path, options, None).map_err(|_| Error::Unavailable)?;
    let recorded = source
        .focused
        .as_ref()
        .and_then(|authority| authority.recorded.as_ref());
    if recorded.is_some_and(|facts| {
        !facts.facts_known() || (harness == "codex" && !facts.selection_known())
    }) {
        return Err(Error::Unavailable);
    }
    let read = || -> Result<_, ReadError> {
        let view = selected::ReadView::open(&source)?;
        let tx = view.lock()?;
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT s.ledger,d.native_id FROM warehouse_current c
             CROSS JOIN warehouse_source s ON s.source=c.source
             CROSS JOIN dim_session d ON d.source=s.source AND d.harness=s.harness
             CROSS JOIN lineage_member m ON m.source=s.source
             WHERE c.path=?1 AND s.origin=?2 AND s.harness=?3 AND s.carrier=c.path",
                params![carrier, source.namespace.binding(), harness],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(index)?;
        let Some((encoded, native_id)) = row else {
            return Ok(None);
        };
        let ledger: Ledger = serde_json::from_str(&encoded).map_err(index)?;
        if crate::events::store::committed_ledger(&tx, carrier)
            .map_err(index)?
            .is_none_or(|(committed, _)| committed != ledger)
        {
            return Err(ReadError::Restart);
        }
        let Availability::Expected(Some(stat)) = source.availability(harness, carrier)? else {
            return Err(ReadError::Restart);
        };
        if ledger.stat != stat {
            return Err(ReadError::Restart);
        }
        source.validate_authority()?;
        let state = if ledger.stat.size == ledger.offset {
            "cached"
        } else {
            "incomplete"
        };
        Ok(Some(crate::SessionSourceProof {
            dev: ledger.stat.dev,
            ino: ledger.stat.ino,
            size: ledger.stat.size,
            modified_ns: ledger.stat.modified_ns,
            changed_ns: ledger.stat.changed_ns,
            freshness: crate::CatalogFreshness {
                state: state.into(),
            },
            source: observation(harness, relative, native_id, ledger, state).source,
        }))
    };
    read().map_err(|_| Error::Unavailable)
}

pub(super) fn host_identity(
    options: &crate::Options,
    source_key: &str,
    scope: CatalogReadScope,
    key: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, crate::CatalogIdentityError> {
    use crate::CatalogIdentityError as Error;
    if !crate::session_route::identifier(key) {
        return Err(Error::InvalidArguments);
    }
    let namespace = Namespace::new(source_key).map_err(|_| Error::InvalidArguments)?;
    if crate::index_rebuild::status(&options.cache).is_some() {
        return Err(Error::Unavailable);
    }
    let path = crate::events::EventCache::path(&options.cache);
    let source =
        Source::focused(namespace, &path, options, None).map_err(|_| Error::Unavailable)?;
    focused_identity(&source, scope, key).map_err(|error| match error {
        ReadError::Restart => Error::ScopeChanged,
        ReadError::RetainedUnverified => Error::RetainedUnverified,
        _ => Error::Unavailable,
    })
}

pub(super) fn focused_identity(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    crate::events::store::require_current(&tx).map_err(index)?;
    crate::storage::sqlite::qualify(&tx, &source.namespace).map_err(index)?;
    let identity = focused_identity_in(&tx, source, scope, key)?;
    source.validate_authority()?;
    Ok(identity)
}

fn focused_identity_in(
    tx: &Connection,
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, ReadError> {
    let (harness, references, identity_witness) = match scope {
        CatalogReadScope::Current => {
            let Some(identity) = current_identity(tx, source, key, None)? else {
                return Ok(None);
            };
            let Some(references) = focused_members(tx, source, &identity)? else {
                return Ok(None);
            };
            let witness = current_identity_witness(tx, source, key, &identity, &references)?;
            (identity.1, references, witness)
        }
        CatalogReadScope::RetainedHistory => {
            let identity =
                warehouse::retained_identity(tx, &source.namespace, key).map_err(retained_error)?;
            let references = retained_references(tx, source, &identity)?;
            (identity.harness, references, identity.witness)
        }
    };
    Ok(Some(present_identity(
        source,
        scope,
        key,
        harness,
        references,
        identity_witness,
    )))
}

fn focused_locate(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
) -> Result<Vec<crate::session_route::Candidate>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    crate::events::store::require_current(&tx).map_err(index)?;
    crate::storage::sqlite::qualify(&tx, &source.namespace).map_err(index)?;
    let candidates = match focused_identity_in(&tx, source, scope, key)? {
        Some(identity) => vec![identity.into()],
        None => match key.strip_prefix("unsent:") {
            Some(target)
                if !target.is_empty() && current_identity(&tx, source, key, None)?.is_none() =>
            {
                failed_send_candidates(&tx, source, target)?
            }
            _ => Vec::new(),
        },
    };
    source.validate_authority()?;
    Ok(candidates)
}

fn failed_send_candidates(
    tx: &Connection,
    source: &Source<'_>,
    target: &str,
) -> Result<Vec<crate::session_route::Candidate>, ReadError> {
    let mut statement = tx.prepare_cached(
        "WITH senders AS MATERIALIZED (
           SELECT DISTINCT m.owner FROM fact_relay_send s
           JOIN warehouse_current c ON c.source=s.source
           JOIN warehouse_source w ON w.source=s.source AND w.origin=?2 AND w.carrier=c.path
           JOIN warehouse_prefix p ON p.source=s.source AND NOT p.superseded
           JOIN files f ON f.path=c.path AND f.resume_at=p.pos
           JOIN bridge_session_owner m ON m.source=s.source
           JOIN fact_relay_result r ON r.source=s.source AND r.call_id=s.call_id
             AND (r.call_pos=s.pos AND r.call_block=s.block OR r.call_pos IS NULL AND
               (SELECT count(*) FROM fact_relay_send other
                WHERE other.source=s.source AND other.call_id=s.call_id)=1)
           WHERE s.target=?1 AND r.failed AND (s.pos,s.block)<(r.pos,r.block) AND r.pos<p.pos
             AND NOT EXISTS (SELECT 1 FROM fact_relay_receipt receipt
               JOIN warehouse_current rc ON rc.source=receipt.source
               JOIN warehouse_source rw ON rw.source=rc.source AND rw.origin=?2 AND rw.carrier=rc.path
               JOIN warehouse_prefix rp ON rp.source=receipt.source AND NOT rp.superseded
               WHERE receipt.correlation=r.correlation AND receipt.pos<rp.pos)
         )
         SELECT k.id,k.harness FROM senders s JOIN catalog_keys k ON k.owner=s.owner
         ORDER BY k.id LIMIT 3",
    ).map_err(index)?;
    let rows = statement
        .query_map(params![target, source.namespace.binding()], |row| {
            Ok(crate::session_route::Candidate {
                source_key: source.namespace.to_string(),
                catalog_key: row.get(0)?,
                harness: row.get(1)?,
            })
        })
        .map_err(index)?;
    rows.collect::<rusqlite::Result<_>>().map_err(index)
}

fn retained_references(
    tx: &Connection,
    source: &Source<'_>,
    identity: &warehouse::RetainedIdentity,
) -> Result<Vec<crate::CatalogSourceObservation>, ReadError> {
    let mut members = tx
        .prepare_cached(
            "SELECT receipt FROM warehouse_identity_member WHERE identity=?1 ORDER BY ordinal",
        )
        .map_err(index)?;
    let mut rows = members.query([identity.binding]).map_err(index)?;
    let mut references = Vec::new();
    while let Some(row) = rows.next().map_err(index)? {
        let receipt: crate::model::summary::CatalogSource =
            serde_json::from_str(&row.get::<_, String>(0).map_err(index)?).map_err(index)?;
        let path = receipt
            .path
            .to_str()
            .and_then(|path| selected::relative(source.options, &identity.harness, path))
            .ok_or(ReadError::RetainedUnverified)?;
        references.push(crate::CatalogSourceObservation {
            source: crate::SessionSourceRef {
                root: identity.harness.clone(),
                path: path.to_owned(),
                native_id: receipt.native_id,
                offset: receipt.offset,
                prefix_sha256: receipt.prefix_sha256,
                tail_sha256: receipt.tail_sha256,
            },
            state: "unavailable".into(),
        });
    }
    source.validate_authority()?;
    Ok(references)
}

fn focused_retained_metadata(
    source: &Source<'_>,
    key: &str,
) -> Result<(serde_json::Value, crate::CatalogSessionIdentity), ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    retained_metadata_in_snapshot(&tx, source, key)
}

fn retained_metadata_in_snapshot(
    tx: &Connection,
    source: &Source<'_>,
    key: &str,
) -> Result<(serde_json::Value, crate::CatalogSessionIdentity), ReadError> {
    let identity =
        warehouse::retained_identity(tx, &source.namespace, key).map_err(retained_error)?;
    let refs = retained_references(tx, source, &identity)?;
    let metadata = serde_json::from_str(&identity.metadata).map_err(index)?;
    Ok((
        metadata,
        present_identity(
            source,
            CatalogReadScope::RetainedHistory,
            key,
            identity.harness,
            refs,
            identity.witness,
        ),
    ))
}

fn present_identity(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    harness: String,
    references: Vec<crate::CatalogSourceObservation>,
    identity_witness: String,
) -> crate::CatalogSessionIdentity {
    let recorded = source
        .focused
        .as_ref()
        .and_then(|authority| authority.recorded.as_ref());
    let facts_known =
        source.options.facts.is_none() || recorded.is_some_and(|facts| facts.facts_known());
    let selection_known =
        source.options.facts.is_none() || recorded.is_some_and(|facts| facts.selection_known());
    let provisional = scope == CatalogReadScope::Current && harness == "codex" && !selection_known;
    let freshness = |state: &str| crate::catalog::CatalogFreshness {
        state: state.into(),
    };
    let native_selection = (harness == "codex").then(|| {
        freshness(if scope == CatalogReadScope::RetainedHistory {
            "retired"
        } else if selection_known {
            "cached"
        } else {
            "unavailable"
        })
    });
    let native_ids: BTreeSet<_> = references
        .iter()
        .map(|reference| reference.source.native_id.clone())
        .collect();
    let machine_label = if source.options.facts.is_none() {
        Some(crate::facts::MachineFacts::Local.hostname(source.options))
    } else {
        recorded.and_then(|facts| facts.hostname())
    };
    let state = if references
        .iter()
        .all(|reference| reference.state == "unavailable")
    {
        "unavailable"
    } else if provisional
        || references
            .iter()
            .any(|reference| reference.state == "unavailable")
    {
        "incomplete"
    } else {
        "cached"
    };
    crate::CatalogSessionIdentity {
        source_key: source.namespace.as_str().to_owned(),
        read_scope: scope,
        catalog_key: key.to_owned(),
        harness,
        native_id: (native_ids.len() == 1 && !provisional)
            .then(|| native_ids.first().expect("one native ID").clone()),
        owner_qualification: if provisional {
            crate::catalog::CatalogOwnerQualification::Provisional
        } else {
            crate::catalog::CatalogOwnerQualification::Qualified
        },
        native_ids: native_ids.into_iter().collect(),
        source_refs: references,
        machine_label,
        generation: identity_witness,
        observed_at: None,
        freshness: freshness(state),
        facts_observation: freshness(if facts_known { "cached" } else { "unavailable" }),
        native_selection,
    }
}

fn current_identity_witness(
    tx: &Connection,
    source: &Source<'_>,
    key: &str,
    identity: &Identity,
    references: &[crate::CatalogSourceObservation],
) -> Result<String, ReadError> {
    let incarnation: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='incarnation'",
            [],
            |row| row.get(0),
        )
        .map_err(index)?;
    let digest = Sha256::digest(encode(&(
        source.namespace.binding(),
        key,
        identity,
        incarnation,
        references
            .iter()
            .map(|reference| &reference.source)
            .collect::<Vec<_>>(),
    ))?);
    Ok(format!("{digest:x}"))
}

pub(crate) fn host_resolve_native(
    options: &crate::Options,
    source_key: &str,
    harness: &str,
    native_id: &str,
    scope: CatalogReadScope,
) -> Result<crate::CatalogNativeResolution, crate::CatalogIdentityError> {
    use crate::{CatalogIdentityError as Error, CatalogNativeResolutionState as State};
    if !crate::native_resolution::valid_resolution_arguments(source_key, harness, native_id) {
        return Err(Error::InvalidArguments);
    }
    if crate::index_rebuild::status(&options.cache).is_some() {
        return Err(Error::Unavailable);
    }
    let path = crate::events::EventCache::path(&options.cache);
    let namespace = Namespace::new(source_key).map_err(|_| Error::InvalidArguments)?;
    let source =
        Source::focused(namespace, &path, options, None).map_err(|_| Error::Unavailable)?;
    let matches =
        resolve_native(&source, scope, harness, native_id).map_err(|error| match error {
            ReadError::Restart => Error::ScopeChanged,
            ReadError::RetainedUnverified => Error::RetainedUnverified,
            _ => Error::Unavailable,
        })?;
    let (state, catalog_key, generation) = match matches.as_slice() {
        [] => (State::Pending, None, None),
        [identity] => {
            let state = match identity.owner_qualification {
                crate::catalog::CatalogOwnerQualification::Qualified => State::Resolved,
                crate::catalog::CatalogOwnerQualification::Provisional => State::Provisional,
            };
            (
                state,
                Some(identity.catalog_key.clone()),
                Some(identity.generation.clone()),
            )
        }
        _ => (State::Ambiguous, None, None),
    };
    Ok(crate::CatalogNativeResolution {
        state,
        source_key: source_key.into(),
        harness: harness.into(),
        native_id: native_id.into(),
        read_scope: scope,
        catalog_key,
        generation,
    })
}

pub(super) fn resolve_native(
    source: &Source<'_>,
    scope: CatalogReadScope,
    harness: &str,
    native_id: &str,
) -> Result<Vec<crate::CatalogSessionIdentity>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    crate::events::store::require_current(&tx).map_err(index)?;
    crate::storage::sqlite::qualify(&tx, &source.namespace).map_err(index)?;
    if scope == CatalogReadScope::RetainedHistory {
        return resolve_retained_native(&tx, source, harness, native_id);
    }
    let mut aliases = tx
        .prepare_cached(include_str!("events/store/warehouse_native_key.sql"))
        .map_err(index)?;
    let mut rows = aliases.query(params![harness, native_id]).map_err(index)?;
    let mut matches = Vec::new();
    while let Some(row) = rows.next().map_err(index)? {
        let identity = (
            row.get(0).map_err(index)?,
            row.get(1).map_err(index)?,
            row.get(2).map_err(index)?,
        );
        if let Some(references) = focused_members(&tx, source, &identity)? {
            let key: String = row.get(3).map_err(index)?;
            let witness = current_identity_witness(&tx, source, &key, &identity, &references)?;
            matches.push(present_identity(
                source, scope, &key, identity.1, references, witness,
            ));
            if matches.len() == 2 {
                break;
            }
        }
    }
    source.validate_authority()?;
    Ok(matches)
}

fn resolve_retained_native(
    tx: &Connection,
    source: &Source<'_>,
    harness: &str,
    native_id: &str,
) -> Result<Vec<crate::CatalogSessionIdentity>, ReadError> {
    let mut aliases = tx
        .prepare_cached(
            "SELECT DISTINCT i.catalog_key FROM warehouse_identity_member m
         CROSS JOIN warehouse_identity i ON i.id=m.identity
         WHERE json_extract(m.receipt,'$.native_id')=?1 AND i.source_key=?2
           AND i.harness=?3 AND i.scope='retained_history' AND i.retained=1
         ORDER BY i.catalog_key",
        )
        .map_err(index)?;
    let mut rows = aliases
        .query(params![native_id, source.namespace.binding(), harness])
        .map_err(index)?;
    let mut matches = Vec::new();
    while let Some(row) = rows.next().map_err(index)? {
        let key: String = row.get(0).map_err(index)?;
        match retained_metadata_in_snapshot(tx, source, &key) {
            Ok((_, identity)) if identity.native_ids.iter().any(|id| id == native_id) => {
                matches.push(identity);
                if matches.len() == 2 {
                    break;
                }
            }
            Ok(_) | Err(ReadError::RetainedUnverified) => (),
            Err(error) => return Err(error),
        }
    }
    source.validate_authority()?;
    Ok(matches)
}

#[derive(Debug)]
pub(super) struct FamilyPrice {
    pub(super) usd: Option<f64>,
    pub(super) shared: crate::events::session_activity::Measured<f64>,
}

/// Read inclusive and shared pricing for a selected current family. Keep
/// this on the selected adapter so listing a page does not walk every subtree.
#[cfg(test)]
pub(super) fn family_price(
    source: &Source<'_>,
    key: &str,
) -> Result<Option<FamilyPrice>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    let snapshot = Snapshot::read(&tx, source)?;
    let Some((owner, _, _)) = current_identity(&tx, source, key, Some(&snapshot.paths))? else {
        return Ok(None);
    };
    let price = selected_family_price(&tx, source, owner)?;
    source.validate_authority()?;
    Ok(Some(price))
}

fn selected_family_price(
    tx: &Connection,
    source: &Source<'_>,
    owner: i64,
) -> Result<FamilyPrice, ReadError> {
    let mut graph = parents::Parents::new(tx)?;
    let mut edges = BTreeMap::new();
    let mut pending = vec![owner];
    while let Some(parent) = pending.pop() {
        if let std::collections::btree_map::Entry::Vacant(entry) = edges.entry(parent) {
            let children = graph.owner_children(parent)?;
            pending.extend(children.iter().copied());
            entry.insert(children);
        }
    }
    let owners = encode(&edges.keys().collect::<Vec<_>>())?;
    let paths = encode(&selected_related_paths(tx, source, &owners, true)?)?;
    let mut members = tx
        .prepare_cached(include_str!("events/store/warehouse_selected_family.sql"))
        .map_err(index)?;
    let mut rows = members
        .query(params![owners, paths, source.namespace.binding()])
        .map_err(index)?;
    let mut family = BTreeMap::new();
    while let Some(row) = rows.next().map_err(index)? {
        if !row.get::<_, bool>(2).map_err(index)? {
            return Err(ReadError::Restart);
        }
        family.insert(
            row.get::<_, i64>(0).map_err(index)?,
            row.get::<_, String>(1).map_err(index)?,
        );
    }
    let mut shared = crate::events::session_activity::Measured::default();
    let mut remaining = edges
        .iter()
        .map(|(&id, children)| (id, children.len()))
        .collect::<BTreeMap<_, _>>();
    let mut parents = BTreeMap::<i64, Vec<i64>>::new();
    for (&parent, children) in &edges {
        for &child in children {
            parents.entry(child).or_default().push(parent);
        }
    }
    let mut leaves = remaining
        .iter()
        .filter_map(|(&id, &count)| (count == 0).then_some(id))
        .collect::<Vec<_>>();
    let keys: BTreeMap<_, _> = relative_keys(tx, &owners)?
        .into_iter()
        .map(|(owner, key, _)| (owner, key))
        .collect();
    // The native family fold starts with Catalog-key-ordered leaves. Children
    // finish in order of subtree height and last key at that height.
    let mut priority = BTreeMap::<i64, (usize, &str)>::new();
    while let Some(node) = leaves.pop() {
        let order = priority.get(&node).copied().unwrap_or((0, &keys[&node]));
        priority.insert(node, order);
        for &parent in parents.get(&node).into_iter().flatten() {
            let candidate = (order.0 + 1, order.1);
            let rank = priority.entry(parent).or_insert(candidate);
            *rank = (*rank).max(candidate);
            let count = remaining.get_mut(&parent).expect("selected family parent");
            *count -= 1;
            if *count == 0 {
                leaves.push(parent);
            }
        }
    }
    if remaining.values().any(|count| *count != 0) {
        return Ok(FamilyPrice { usd: None, shared });
    }
    // An owner's own observation wins before its children. The first visit
    // also gives the winning branch when two paths reach the same owner.
    let mut seen = BTreeSet::new();
    let mut fold = Vec::new();
    let mut pending = vec![owner];
    while let Some(node) = pending.pop() {
        if seen.insert(node) {
            fold.push(node);
            let mut children: Vec<_> = edges[&node].iter().copied().collect();
            children.sort_by_key(|child| std::cmp::Reverse(priority[child]));
            pending.extend(children);
        }
    }
    let fold = encode(&fold)?;
    let table = crate::pricing::snapshot();
    let mut priced = Snapshot::priced_usage(tx, &paths, &owners, &table)?;
    let usd = family
        .into_iter()
        .map(|(owner, harness)| {
            if harness == "copilot" {
                return None;
            }
            match priced.remove(&owner) {
                Some(usage) if usage.shared => None,
                Some(usage) => usage.pricing.finish().usd,
                None => Some(0.0),
            }
        })
        .sum();

    let mut shared_usage = tx
        .prepare_cached(include_str!(
            "events/store/warehouse_selected_family_shared.sql"
        ))
        .map_err(index)?;
    let mut steps = shared_usage.query(params![fold, paths]).map_err(index)?;
    while let Some(step) = steps.next().map_err(index)? {
        let billing = step.get::<_, Option<String>>(1).map_err(index)?;
        let usd = billing
            .map(|billing| {
                let mut pricing = crate::pricing::CostBuild::with_table(table.clone());
                pricing.message(&crate::events::MessageUsage {
                    model: step.get(0).map_err(index)?,
                    billing: serde_json::from_str(&billing).map_err(index)?,
                    ..Default::default()
                });
                Ok::<_, ReadError>(pricing.finish().usd)
            })
            .transpose()?
            .flatten();
        if let Some(usd) = usd {
            shared.known += usd;
        } else {
            shared.unknown_steps += 1;
        }
    }
    Ok(FamilyPrice { usd, shared })
}

fn retained_error(error: warehouse::RetainedIdentityError) -> ReadError {
    match error {
        warehouse::RetainedIdentityError::Unverified => ReadError::RetainedUnverified,
        warehouse::RetainedIdentityError::Index(error) => index(error),
    }
}

fn current_identity(
    db: &Connection,
    source: &Source<'_>,
    key: &str,
    paths: Option<&str>,
) -> Result<Option<Identity>, ReadError> {
    let mut exact = db.prepare_cached(EXACT).map_err(index)?;
    let mut matches = exact
        .query(params![key, paths, source.namespace.binding()])
        .map_err(index)?;
    let found = matches
        .next()
        .map_err(index)?
        .map(|r| Ok::<_, rusqlite::Error>((r.get(0)?, r.get(1)?, r.get(2)?)))
        .transpose()
        .map_err(index)?;
    if matches.next().map_err(index)?.is_some() {
        return Err(index("ambiguous warehouse Catalog key"));
    }
    Ok(found)
}

#[cfg(test)]
#[path = "catalog_warehouse/tests.rs"]
mod tests;

#[path = "catalog_warehouse/selected.rs"]
mod selected;

#[path = "catalog_warehouse/http.rs"]
pub(crate) mod http;

#[path = "catalog_warehouse/trace.rs"]
mod trace;
