//! Fixture adapter for the committed warehouse. The host supplies authorized
//! inventory and repository observations. Each source is read independently.
use super::{CatalogReadScope, Request};
use crate::{events::store::warehouse, storage::Namespace};
use rusqlite::{Connection, Transaction, params};
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
    "SELECT paths.key,paths.value,s.source,s.ledger,m.sidecar_revision,m.eof_witness
    FROM json_each(?1) paths
    LEFT JOIN warehouse_current c ON paths.value AND c.path=paths.key
    LEFT JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
    LEFT JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
    LEFT JOIN maintenance_cursor m ON m.source=s.source AND f.file_id IS NOT NULL
    ORDER BY paths.key";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Availability {
    Expected,
    Unavailable,
}

pub(super) struct Source<'a> {
    pub(super) namespace: Namespace,
    pub(super) path: &'a std::path::Path,
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

#[derive(Debug, Eq, PartialEq)]
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
    transaction: &'a Transaction<'a>,
    source: &'a Source<'a>,
    paths: String,
    coverage: Coverage,
    revision: String,
}

impl<'a> Snapshot<'a> {
    fn read(transaction: &'a Transaction<'a>, source: &'a Source<'a>) -> Result<Self, ReadError> {
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
                .map(|(path, availability)| (path, *availability == Availability::Expected))
                .collect::<BTreeMap<_, _>>(),
        )?;
        let mut probe = transaction.prepare_cached(RECEIPTS).map_err(index)?;
        let mut receipts = probe.query(params![inventory, origin]).map_err(index)?;
        while let Some(record) = receipts.next().map_err(index)? {
            let path: String = record.get(0).map_err(index)?;
            let expected: bool = record.get(1).map_err(index)?;
            let sidecar: Option<i64> = record.get(4).map_err(index)?;
            if let Some(sidecar) = sidecar {
                let source_id: i64 = record.get(2).map_err(index)?;
                let ledger: String = record.get(3).map_err(index)?;
                let eof: Option<String> = record.get(5).map_err(index)?;
                digest.update(encode(&(&path, source_id, ledger, sidecar, eof))?);
                coverage.committed += 1;
                authorized.push(path);
            } else {
                digest.update(encode(&(path, expected))?);
                if expected {
                    coverage.pending += 1;
                } else {
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
            let row = Row {
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
            };
            if selected.len() == request.limit || examined == limit || bytes >= MAX_SCAN_BYTES {
                next = Some(Boundary {
                    position: row.position,
                    inclusive: true,
                });
                continue;
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
        let mut usage = self.transaction.prepare_cached(USAGE).map_err(index)?;
        let usage = usage
            .query_map(params![owners, self.paths], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    [
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ],
                ))
            })
            .map_err(index)?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .map_err(index)?;
        let mut parents = self.transaction.prepare_cached(PARENTS).map_err(index)?;
        let mut parents = parents
            .query_map(params![owners, self.paths], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(index)?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .map_err(index)?;
        for row in rows {
            row.usage = usage.get(&row.owner).copied().unwrap_or_default();
            row.parent = parents.remove(&row.owner);
        }
        Ok(())
    }
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
            .map_err(|error| {
                if error.code() == "retained_identity_unverified" {
                    ReadError::RetainedUnverified
                } else {
                    index(error)
                }
            });
    }
    crate::events::store::require_current(&tx).map_err(index)?;
    crate::storage::sqlite::qualify(&tx, &source.namespace).map_err(index)?;
    let paths = encode(
        &source
            .inventory
            .iter()
            .filter_map(|(path, availability)| {
                (*availability == Availability::Expected).then_some(path)
            })
            .collect::<Vec<_>>(),
    )?;
    let mut exact = tx.prepare_cached(EXACT).map_err(index)?;
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
