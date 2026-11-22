//! Fixture adapter for the committed warehouse. The host supplies authorized
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

const LIST: &str = include_str!("events/store/warehouse_list.sql");
const EXACT: &str = include_str!("events/store/warehouse_exact.sql");
const USAGE: &str = include_str!("events/store/warehouse_list_usage.sql");
const PAGE_MEMBERS: &str = "SELECT p.value,d.harness,d.native_id,s.carrier,s.ledger
    FROM json_each(?1) p
    CROSS JOIN lineage_member root ON root.source=p.value
    CROSS JOIN lineage_member m ON m.component=root.component
    CROSS JOIN dim_session d ON d.source=m.source
    CROSS JOIN warehouse_source s ON s.source=m.source AND s.origin=?2 AND s.harness=d.harness
    ORDER BY p.value,m.source";
const PARENTS: &str = "SELECT p.value,k.id,e.method FROM json_each(?1) p
    JOIN parent_selection e ON e.child=p.value JOIN bridge_session_owner b ON b.source=e.parent
    JOIN catalog_keys k ON k.owner=b.owner
    JOIN warehouse_current c ON c.source=b.owner
    JOIN json_each(?2) authorized ON authorized.value=c.path
    ORDER BY p.value,k.id DESC";
const STATUS: &str = "SELECT p.value,d.harness,d.native_id,s.carrier,s.ledger,d.lineage,m.source
    FROM json_each(?1) p
    CROSS JOIN lineage_member root ON root.source=p.value
    CROSS JOIN lineage_member m ON m.component=root.component
    CROSS JOIN dim_session d ON d.source=m.source
    CROSS JOIN warehouse_source s ON s.source=m.source
    ORDER BY p.value,m.source";
const RECEIPTS: &str =
    "SELECT paths.key,paths.value,s.source,s.ledger,m.sidecar_revision,m.eof_witness,a.evidence
    FROM json_each(?1) paths
    LEFT JOIN warehouse_current c ON paths.value AND c.path=paths.key
    LEFT JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
    LEFT JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
    LEFT JOIN maintenance_cursor m ON m.source=s.source AND f.file_id IS NOT NULL
    LEFT JOIN session_source_mutations a ON a.source_path=c.path
    ORDER BY paths.key";

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
            current_provider,
            focused: Some(FocusedAuthority { recorded }),
        })
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
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Availability::Unavailable
                }
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                    Availability::Unavailable
                }
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
        let Some(path) = selected::relative(self.options, harness, carrier) else {
            return Ok(Availability::Unavailable);
        };
        if harness == "codex"
            && let Some(recorded) = &authority.recorded
            && recorded
                .codex_current(path)
                .map_err(|_| ReadError::Restart)?
                == Some(false)
        {
            return Ok(Availability::Unavailable);
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
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.kind() == std::io::ErrorKind::InvalidInput =>
            {
                Ok(Availability::Unavailable)
            }
            Err(error) => Err(index(error)),
        }
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
pub(super) struct Row {
    position: Position,
    owner: i64,
    pub(super) harness: String,
    pub(super) title: String,
    pub(super) model: String,
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
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Coverage {
    pub(super) committed: usize,
    pub(super) pending: usize,
    pub(super) unavailable: usize,
}

#[derive(Debug)]
pub(super) struct Page {
    pub(super) rows: Vec<Row>,
    pub(super) total: usize,
    pub(super) coverage: Coverage,
    pub(super) examined: usize,
    pub(super) next: Option<Continuation>,
    generation: String,
}

#[derive(Debug)]
pub(super) enum ReadError {
    Index(String),
    Scope,
    Restart,
    RetainedUnverified,
    PeerUnavailable(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(error) => f.write_str(error),
            Self::Scope => f.write_str("Discard the cursor when changing workspace, source or filters."),
            Self::Restart => f.write_str("The committed session view changed. Restart at the first page."),
            Self::PeerUnavailable(reason) => f.write_str(reason),
            Self::RetainedUnverified => f.write_str("retained_identity_unverified: no verified original warehouse identity is available for this key."),
        }
    }
}

fn index(error: impl std::fmt::Display) -> ReadError {
    ReadError::Index(error.to_string())
}

fn encode(value: &impl serde::Serialize) -> Result<String, ReadError> {
    serde_json::to_string(value).map_err(index)
}

type LocalPage = (Vec<Row>, usize, usize, Option<Boundary>);
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
                if committed.stat == observed
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
        let mut repos = source.repos.clone();
        if source.focused.is_some() && source.options.facts.is_none() {
            let home = crate::facts::env_home();
            let mut directories = transaction
                .prepare_cached(
                    "SELECT DISTINCT f.cwd FROM json_each(?1) path
                     CROSS JOIN files f ON f.path=path.value WHERE f.cwd IS NOT NULL",
                )
                .map_err(index)?;
            let mut rows = directories.query([&paths]).map_err(index)?;
            while let Some(row) = rows.next().map_err(index)? {
                let cwd: String = row.get(0).map_err(index)?;
                if let Some(repo) =
                    crate::model::repo_of(&cwd, home.as_deref().map(std::path::Path::new))
                {
                    repos.insert(cwd, repo);
                }
            }
        }
        digest.update(encode(&repos)?);
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

    fn rows(&self, request: &Request, boundary: Option<&Boundary>) -> Result<LocalPage, ReadError> {
        use crate::catalog_search::{MAX_CANDIDATES, MAX_SCAN_BYTES};
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
        let (mut selected, mut examined, mut bytes, mut next, mut total) =
            (Vec::new(), 0, 0, None, 0);
        while let Some(record) = records.next().map_err(index)? {
            let Some(key) = record.get::<_, Option<String>>(0).map_err(index)? else {
                total = usize::try_from(record.get::<_, i64>(8).map_err(index)?).map_err(index)?;
                continue;
            };
            if next.is_some() {
                continue;
            }
            let mut row = Row {
                position: Position {
                    last: record.get(3).map_err(index)?,
                    key,
                    source: machine.to_owned(),
                },
                owner: record.get(1).map_err(index)?,
                harness: record.get(2).map_err(index)?,
                title: record.get(4).map_err(index)?,
                model: record.get(5).map_err(index)?,
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
            };
            if selected.len() == request.limit || examined == limit || bytes >= MAX_SCAN_BYTES {
                next = Some(Boundary {
                    position: row.position,
                    inclusive: true,
                });
                continue;
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
                row.title = "Approval review".into();
            }
            if row.harness == "codex" && record.get::<_, bool>(14).map_err(index)? {
                let task: Option<String> = record.get(13).map_err(index)?;
                let prompt: Option<i64> = record.get(9).map_err(index)?;
                if let Some(offset) = prompt {
                    let path: String = record.get(10).map_err(index)?;
                    let ledger: String = record.get(11).map_err(index)?;
                    let native_id: String = record.get(12).map_err(index)?;
                    row.title =
                        selected::prompt_title(self.source, path, &ledger, native_id, offset)?
                            .unwrap_or_else(|| "Codex subagent".into());
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
                selected.push(row);
            }
        }
        Ok((selected, total, examined, next))
    }

    fn runtime_status(&self, owners: &[i64]) -> Result<BTreeMap<i64, &'static str>, ReadError> {
        use crate::facts::source_authority::CurrentSources;
        let options = self.source.options;
        let observed = crate::native_evidence::RuntimeObservations::new(options);
        let authority = options
            .facts
            .as_deref()
            .and_then(|path| CurrentSources::open(path).ok());
        if options.facts.is_some() && !authority.as_ref().is_some_and(CurrentSources::facts_known) {
            return Ok(BTreeMap::new());
        }
        let mut children = self
            .transaction
            .prepare_cached(include_str!("events/store/warehouse_child_runtime.sql"))
            .map_err(index)?;
        let children = children
            .query_map(params![encode(&owners)?, self.paths], |row| {
                Ok(ChildRuntime {
                    child: row.get(0)?,
                    native_id: row.get(1)?,
                    parent: row.get(2)?,
                    call: row.get(3)?,
                    root: row.get(4)?,
                    qualified: row.get(5)?,
                })
            })
            .map_err(index)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(index)?;
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
                let phase: Option<String> = phases
                    .query_row(params![id, native_id], |r| r.get(0))
                    .optional()
                    .map_err(index)?;
                let live = process.is_some();
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
                let state =
                    child_runtime_state(&mut evidence, &child, alive.contains(&child.root))?;
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
        &self,
        owners: &str,
        table: &std::sync::Arc<crate::pricing::EffectiveTable>,
    ) -> Result<BTreeMap<i64, Usage>, ReadError> {
        let mut usage = self.transaction.prepare_cached(USAGE).map_err(index)?;
        let mut facts = usage.query(params![owners, self.paths]).map_err(index)?;
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

    fn enrich(&self, rows: &mut [Row]) -> Result<(), ReadError> {
        if rows.is_empty() {
            return Ok(());
        }
        let owners: Vec<_> = rows.iter().map(|row| row.owner).collect();
        let status = self.runtime_status(&owners)?;
        let owners = encode(&owners)?;
        let table = crate::pricing::snapshot();
        let mut priced = self.priced_usage(&owners, &table)?;
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
        let mut parents = self.transaction.prepare_cached(PARENTS).map_err(index)?;
        let mut parents = parents
            .query_map(params![owners, self.paths], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
                ))
            })
            .map_err(index)?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .map_err(index)?;
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

pub(super) fn read_list(
    sources: &[Source<'_>],
    workspace: &str,
    request: &Request,
    cursor: Option<&Continuation>,
) -> Result<Page, ReadError> {
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
    for source in ordered {
        let mut db =
            Connection::open_with_flags(source.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(index)?;
        list_functions(&db).map_err(index)?;
        let tx = db.transaction().map_err(index)?;
        let snapshot = Snapshot::read(&tx, source)?;
        digest.update(&snapshot.revision);
        let (mut local, count, scanned, local_next) =
            snapshot.rows(request, cursor.map(|cursor| &cursor.boundary))?;
        snapshot.enrich(&mut local)?;
        source.validate_authority()?;
        rows.extend(local);
        rows.sort_by(|a, b| a.position.cmp(&b.position));
        if rows.len() > request.limit {
            let position = rows[request.limit].position.clone();
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
        coverage.committed += snapshot.coverage.committed;
        coverage.pending += snapshot.coverage.pending;
        coverage.unavailable += snapshot.coverage.unavailable;
    }
    let revision = format!("{:x}", digest.finalize());
    if cursor.is_some_and(|cursor| cursor.revision != revision) {
        return Err(ReadError::Restart);
    }
    if let Some(next) = &frontier {
        rows.retain(|row| row.position < next.position);
    }
    let next = frontier.map(|mut boundary| {
        if request.q.is_none()
            && let Some(last) = rows.last()
        {
            boundary = Boundary {
                position: last.position.clone(),
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

pub(super) fn focused_identity(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    crate::events::store::require_current(&tx).map_err(index)?;
    crate::storage::sqlite::qualify(&tx, &source.namespace).map_err(index)?;
    let (harness, references, identity_witness) = match scope {
        CatalogReadScope::Current => {
            let Some(identity) = current_identity(&tx, source, key, None)? else {
                return Ok(None);
            };
            let Some(references) = focused_members(&tx, source, &identity)? else {
                return Ok(None);
            };
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
                &identity,
                incarnation,
                &references,
            ))?);
            (identity.1, references, format!("{digest:x}"))
        }
        CatalogReadScope::RetainedHistory => {
            let identity = warehouse::retained_identity(&tx, &source.namespace, key)
                .map_err(retained_error)?;
            let mut members = tx.prepare_cached("SELECT receipt FROM warehouse_identity_member WHERE identity=?1 ORDER BY ordinal").map_err(index)?;
            let mut rows = members.query([identity.binding]).map_err(index)?;
            let mut references = Vec::new();
            while let Some(row) = rows.next().map_err(index)? {
                let receipt: crate::model::summary::CatalogSource =
                    serde_json::from_str(&row.get::<_, String>(0).map_err(index)?)
                        .map_err(index)?;
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
            (identity.harness, references, identity.witness)
        }
    };
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
    source.validate_authority()?;
    Ok(Some(crate::CatalogSessionIdentity {
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
    }))
}

pub(super) fn resolve_current_native(
    source: &Source<'_>,
    harness: &str,
    native_id: &str,
) -> Result<Vec<String>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    crate::events::store::require_current(&tx).map_err(index)?;
    crate::storage::sqlite::qualify(&tx, &source.namespace).map_err(index)?;
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
        if focused_members(&tx, source, &identity)?.is_some() {
            matches.push(row.get(3).map_err(index)?);
            if matches.len() == 2 {
                break;
            }
        }
    }
    Ok(matches)
}

#[derive(Debug)]
pub(super) struct FamilyPrice {
    pub(super) usd: Option<f64>,
    pub(super) shared: crate::events::session_activity::Measured<f64>,
}

/// Read inclusive and shared pricing for a selected current family. Keep
/// this on the selected adapter so listing a page does not walk every subtree.
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
    let mut members = tx
        .prepare_cached(include_str!("events/store/warehouse_selected_family.sql"))
        .map_err(index)?;
    let mut rows = members
        .query(params![owner, snapshot.paths, source.namespace.binding()])
        .map_err(index)?;
    let mut family = BTreeMap::new();
    let mut cycle = false;
    while let Some(row) = rows.next().map_err(index)? {
        if !row.get::<_, bool>(3).map_err(index)? {
            return Err(ReadError::Restart);
        }
        cycle = row.get(2).map_err(index)?;
        family.insert(
            row.get::<_, i64>(0).map_err(index)?,
            row.get::<_, String>(1).map_err(index)?,
        );
    }
    let mut shared = crate::events::session_activity::Measured::default();
    if cycle {
        return Ok(Some(FamilyPrice { usd: None, shared }));
    }
    let owners = encode(&family.keys().collect::<Vec<_>>())?;
    let table = crate::pricing::snapshot();
    let mut priced = snapshot.priced_usage(&owners, &table)?;
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
    let mut steps = shared_usage
        .query(params![owner, owners, snapshot.paths])
        .map_err(index)?;
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
    Ok(Some(FamilyPrice { usd, shared }))
}

fn retained_error(error: warehouse::RetainedIdentityError) -> ReadError {
    if error.code() == "retained_identity_unverified" {
        ReadError::RetainedUnverified
    } else {
        index(error)
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

#[path = "catalog_warehouse/tests.rs"]
mod tests;

#[path = "catalog_warehouse/selected.rs"]
mod selected;

#[path = "catalog_warehouse/http.rs"]
mod http;
