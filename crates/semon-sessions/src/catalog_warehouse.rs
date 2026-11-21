//! Fixture adapter for the committed warehouse. The host supplies authorized
//! inventory and repository observations. Each source is read independently.
use super::{CatalogReadScope, Request};
use crate::{
    events::{Ledger, Stat, store::warehouse},
    source_change::AppendEvidence,
    storage::Namespace,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const LIST: &str = include_str!("events/store/warehouse_list.sql");
const EXACT: &str = include_str!("events/store/warehouse_exact.sql");
const USAGE: &str = include_str!("events/store/warehouse_list_usage.sql");
const PARENTS: &str = "SELECT p.value,k.id FROM json_each(?1) p
    JOIN parent_selection e ON e.child=p.value JOIN bridge_session_owner b ON b.source=e.parent
    JOIN catalog_keys k ON k.owner=b.owner
    JOIN warehouse_current c ON c.source=b.owner
    JOIN json_each(?2) authorized ON authorized.value=c.path
    ORDER BY p.value,k.id DESC";
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Debug)]
struct Boundary {
    position: Position,
    inclusive: bool,
}

#[derive(Debug)]
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
    pub(super) usage: [i64; 6],
    pub(super) cost: Option<crate::pricing::Cost>,
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
}

#[derive(Debug)]
pub(super) enum ReadError {
    Index(String),
    Scope,
    Restart,
    RetainedUnverified,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(error) => f.write_str(error),
            Self::Scope => f.write_str("Discard the cursor when changing workspace, source or filters."),
            Self::Restart => f.write_str("The committed session view changed. Restart at the first page."),
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
    coverage: Coverage,
    revision: String,
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
            "codex-response" => self.pricing.codex(&crate::events::CodexUsageEvent {
                model: model.unwrap_or_else(|| "unknown".into()),
                tokens: billing(row)?,
                timestamp: row.get(7).map_err(index)?,
            }),
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
        let mut digest = Sha256::new_with_prefix(encode(&(origin, incarnation, &source.repos))?);
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
        Ok(Self {
            transaction,
            source,
            paths,
            coverage,
            revision: format!("{:x}", digest.finalize()),
        })
    }

    fn rows(&self, request: &Request, boundary: Option<&Boundary>) -> Result<LocalPage, ReadError> {
        use crate::catalog_search::{MAX_CANDIDATES, MAX_SCAN_BYTES};
        let repo = encode(&self.source.repos)?;
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
                usage: [0; 6],
                cost: None,
            };
            if selected.len() == request.limit || examined == limit || bytes >= MAX_SCAN_BYTES {
                next = Some(Boundary {
                    position: row.position,
                    inclusive: true,
                });
                continue;
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

    fn enrich(&self, rows: &mut [Row]) -> Result<(), ReadError> {
        if rows.is_empty() {
            return Ok(());
        }
        let owners = encode(&rows.iter().map(|row| row.owner).collect::<Vec<_>>())?;
        let table = crate::pricing::snapshot();
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
        let mut parents = self.transaction.prepare_cached(PARENTS).map_err(index)?;
        let mut parents = parents
            .query_map(params![owners, self.paths], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
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
            row.parent = parents.remove(&row.owner);
        }
        Ok(())
    }
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
    let query = encode(&(
        workspace,
        request.scope,
        &request.harness,
        &request.repo,
        &request.model,
        &request.q,
    ))?;
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
            revision,
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
    let snapshot = Snapshot::read(&tx, source)?;
    current_identity(&tx, source, key, &snapshot.paths)
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
    paths: &str,
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
