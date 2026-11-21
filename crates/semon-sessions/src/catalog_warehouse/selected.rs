//! Selected warehouse positions and origin-local byte proofs for comparison tests.
use super::{
    Availability, CatalogReadScope, Ledger, ReadError, Source, Stat, current_identity, encode,
    index, warehouse,
};
use crate::{
    Options, SessionSourceRange, SessionSourceReader, SessionSourceRef, events::store::chunk_proofs,
};
use rusqlite::{OptionalExtension, named_params, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io,
    path::Path,
    sync::{Arc, Mutex},
};

const MEMBER_PAGE: i64 = 64;
const PAGE_BYTES: usize = 8 * 1024 * 1024;
const MEMBERS: &str = "SELECT m.source,s.incarnation,s.carrier,s.ledger,d.native_id,c.path,
    f.resume_at,coalesce(r.sidecar_revision,0)
    FROM lineage_member m
    CROSS JOIN warehouse_source s ON s.source=m.source AND s.origin=?2 AND s.harness=?3
    CROSS JOIN dim_session d ON d.source=m.source
    LEFT JOIN warehouse_current c ON c.source=m.source AND c.path=s.carrier
    LEFT JOIN files f ON f.path=c.path
    LEFT JOIN maintenance_cursor r ON r.source=m.source
    WHERE m.component=?1 AND m.source>?4 ORDER BY m.source LIMIT ?5";
const COMMITTED_PREFIX: &str = "SELECT ledger FROM warehouse_prefix
    WHERE source=?1 AND revision<=?2 ORDER BY revision DESC LIMIT 1";
const CONTINUATIONS: &str = "SELECT e.source,min(e.pos) FROM dim_session target
    CROSS JOIN fact_relation_observation e ON e.target=target.native_id AND e.kind='continued-in'
    CROSS JOIN lineage_member m ON m.source=e.source AND m.component=?2
    CROSS JOIN dim_session original ON original.source=e.source AND original.lineage
    WHERE target.source=?1 AND target.harness='claude' GROUP BY e.source";

// Select a unique call and its native result positions within this source's
// committed prefix. Result order remains stable across equal timestamps.
const TOOL_RESULTS: &str = "WITH calls AS MATERIALIZED (
    SELECT call_id,pos,block FROM fact_tool_call WHERE source=?1 AND pos=?2 AND block=?3
      AND occurrence=?4 AND call_id IS NOT NULL
), unique_call AS (
    SELECT c.call_id,c.pos,c.block FROM calls c
    CROSS JOIN fact_tool_call all_calls ON all_calls.source=?1 AND all_calls.call_id=c.call_id
      AND all_calls.pos<?5
    GROUP BY c.call_id,c.pos,c.block HAVING count(*)=1
)
SELECT coalesce(r.ts,-9223372036854775808),r.pos,r.block,r.occurrence,r.kind
FROM unique_call c CROSS JOIN fact_tool_result r ON r.source=?1 AND r.call_id=c.call_id
  AND ((r.call_pos IS NULL AND r.kind<>'copilot-execution')
       OR (r.call_pos=c.pos AND r.call_block=c.block))
WHERE r.pos<?5 ORDER BY 1,r.pos,r.block,r.occurrence LIMIT ?6";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct Position {
    time: i64,
    source: i64,
    offset: i64,
    block: i64,
    occurrence: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Prefix {
    source: i64,
    incarnation: i64,
    offset: u64,
    digest: [u8; 32],
    tail: [u8; 32],
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    selection: String,
    members: String,
    view: i64,
    opened_at: i64,
    after: Position,
    touched: Vec<Prefix>,
}

impl Cursor {
    pub(super) fn touched_count(&self) -> usize {
        self.touched.len()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Address {
    pub(super) path: String,
    pub(super) offset: u64,
    pub(super) block: i64,
    pub(super) occurrence: i64,
}

#[derive(Debug)]
pub(super) struct Record {
    pub(super) address: Address,
    pub(super) kind: Option<String>,
    pub(super) native: serde_json::Value,
}

impl Record {
    pub(super) fn text(&self) -> Option<String> {
        let kind = self.kind.as_deref()?;
        match (
            kind,
            self.native
                .pointer("/payload/role")
                .and_then(serde_json::Value::as_str),
        ) {
            ("user" | "user.message", _) | ("response_item", Some("user")) => {
                crate::model::prompt_text(&self.native)
            }
            ("assistant" | "assistant.message", _) | ("response_item", Some("assistant")) => {
                crate::model::assistant_text(
                    &self.native,
                    usize::try_from(self.address.block).ok()?,
                )
            }
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(super) struct Component {
    pub(super) harness: String,
    pub(super) native_id: String,
    pub(super) title: Option<String>,
    pub(super) model: Option<String>,
    pub(super) member_count: usize,
    pub(super) records: Vec<Record>,
    pub(super) next: Option<Cursor>,
}

struct Member {
    carrier: String,
    current: Ledger,
    reference: SessionSourceRef,
}

fn decode(ledger: &str) -> Result<Ledger, ReadError> {
    serde_json::from_str(ledger).map_err(index)
}

fn committed_prefix(
    query: &mut rusqlite::Statement<'_>,
    source: i64,
    view: i64,
) -> Result<Ledger, ReadError> {
    let encoded: String = query
        .query_row(params![source, view], |r| r.get(0))
        .optional()
        .map_err(index)?
        .ok_or(ReadError::Restart)?;
    decode(&encoded)
}

fn continuation_bounds(
    continuations: &mut rusqlite::Statement<'_>,
    prefix: &mut rusqlite::Statement<'_>,
    source: i64,
    component: i64,
    view: i64,
) -> Result<String, ReadError> {
    let mut bounds = BTreeMap::new();
    let mut sources = continuations
        .query(params![source, component])
        .map_err(index)?;
    while let Some(row) = sources.next().map_err(index)? {
        let id = row.get(0).map_err(index)?;
        let edge = row.get::<_, i64>(1).map_err(index)?;
        let receipt = committed_prefix(prefix, id, view)?;
        if i64::try_from(receipt.offset).map_err(index)? > edge {
            bounds.insert(id, receipt.offset);
        }
    }
    encode(&bounds)
}

fn stable_or_grown(prior: &Stat, now: &Stat) -> bool {
    prior == now
        || (prior.is_prefix_of(now)
            && prior.size < now.size
            && prior.changed_ns.is_some()
            && now.changed_ns.is_some())
}

fn raw_stat(path: &Path) -> io::Result<Stat> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(changed());
    }
    Stat::from_metadata(&metadata)
}

fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Selected source or committed prefix changed",
    )
}

fn relative<'a>(options: &Options, harness: &str, path: &'a str) -> Option<&'a str> {
    let home = match harness {
        "claude" => &options.claude_home,
        "codex" => &options.codex_home,
        "copilot" => &options.copilot_home,
        _ => return None,
    };
    let relative = Path::new(path).strip_prefix(home).ok()?.to_str()?;
    crate::is_input_path(harness, relative).then_some(relative)
}

fn position(row: &rusqlite::Row<'_>, source: i64) -> rusqlite::Result<(Position, Option<String>)> {
    Ok((
        Position {
            time: row.get(0)?,
            source,
            offset: row.get(1)?,
            block: row.get(2)?,
            occurrence: row.get(3)?,
        },
        row.get(4)?,
    ))
}

pub(super) fn prompt_title(
    source: &Source<'_>,
    carrier: String,
    ledger: &str,
    native_id: String,
    offset: i64,
) -> Result<Option<String>, ReadError> {
    let db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let db = chunk_proofs::Snapshot::new(db);
    let current = decode(ledger)?;
    let path = relative(source.options, "codex", &carrier).ok_or(ReadError::Restart)?;
    let reference = SessionSourceRef {
        root: "codex".into(),
        path: path.into(),
        native_id,
        offset: current.offset,
        prefix_sha256: current.prefix,
        tail_sha256: current.tail,
    };
    let budget = Mutex::new(PAGE_BYTES);
    let mut reader = PrefixReader::open(
        db,
        source.path,
        Arc::new(Member {
            carrier,
            current,
            reference,
        }),
        &budget,
    )?;
    let mut bytes_left = 256 * 1024;
    let native = crate::source_reader::read_record_bounded(
        &reader,
        &reader.member.reference,
        u64::try_from(offset).map_err(index)?,
        bytes_left,
        &mut bytes_left,
    )
    .map_err(|_| ReadError::Restart)?;
    reader.finish().map_err(|_| ReadError::Restart)?;
    Ok(crate::model::prompt_text(&native)
        .filter(|text| !text.trim().is_empty())
        .map(|text| crate::model::one_line(&text, 80)))
}

pub(super) fn page(
    options: &Options,
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<Option<Component>, ReadError> {
    read(options, source, scope, key, Target::Page(limit), cursor)
}

pub(super) fn record_at(
    options: &Options,
    source: &Source<'_>,
    key: &str,
    address: &Address,
    cursor: Option<&Cursor>,
) -> Result<Option<Record>, ReadError> {
    read(
        options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Address(address),
        cursor,
    )
    .map(|component| component.and_then(|component| component.records.into_iter().next()))
}

/// The host provides an independently authorized archive reader. The warehouse
/// binding supplies the original source reference and exact record position.
pub(super) fn retained_record_at(
    options: &Options,
    source: &Source<'_>,
    key: &str,
    address: &Address,
    provider: &dyn SessionSourceReader,
) -> Result<Option<Record>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    let identity =
        warehouse::retained_identity(&tx, &source.namespace, key).map_err(super::retained_error)?;
    let home = match identity.harness.as_str() {
        "claude" => &options.claude_home,
        "codex" => &options.codex_home,
        "copilot" => &options.copilot_home,
        _ => return Err(ReadError::RetainedUnverified),
    };
    if !crate::is_input_path(&identity.harness, &address.path) {
        return Ok(None);
    }
    let full_path = home.join(&address.path);
    let at = i64::try_from(address.offset).map_err(index)?;
    let Some((receipt, kind)) = tx.query_row(
        "SELECT m.receipt,r.kind FROM warehouse_identity_member m
         CROSS JOIN fact_record r ON r.source=m.source AND r.pos=?3 AND r.block=?4 AND r.occurrence=?5
         WHERE m.identity=?1 AND json_extract(m.receipt,'$.path')=?2
          AND r.pos>=0 AND r.pos<json_extract(m.receipt,'$.offset') AND NOT r.gap",
        params![identity.binding, full_path.to_str(), at, address.block, address.occurrence],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    ).optional().map_err(index)? else {
        return Ok(None);
    };
    let receipt: crate::model::summary::CatalogSource =
        serde_json::from_str(&receipt).map_err(index)?;
    let reference = SessionSourceRef {
        root: identity.harness,
        path: address.path.clone(),
        native_id: receipt.native_id,
        offset: receipt.offset,
        prefix_sha256: receipt.prefix_sha256,
        tail_sha256: receipt.tail_sha256,
    };
    let reader = crate::source_reader::BudgetedReader::new(provider, 1024);
    let mut bytes_left = PAGE_BYTES;
    let native = crate::source_reader::read_record_bounded(
        &reader,
        &reference,
        address.offset,
        crate::tx::LINE_MAX as usize,
        &mut bytes_left,
    )
    .map_err(|_| ReadError::Restart)?;
    Ok(Some(Record {
        address: address.clone(),
        kind,
        native,
    }))
}

pub(super) fn tool_results(
    options: &Options,
    source: &Source<'_>,
    key: &str,
    call: &Address,
    cursor: Option<&Cursor>,
) -> Result<Option<Vec<Record>>, ReadError> {
    read(
        options,
        source,
        CatalogReadScope::Current,
        key,
        Target::ToolResults(call),
        cursor,
    )
    .map(|component| component.map(|component| component.records))
}

enum Target<'a> {
    Page(usize),
    Address(&'a Address),
    ToolResults(&'a Address),
}

impl Target<'_> {
    fn capacity(&self) -> Result<usize, ReadError> {
        match *self {
            Self::Page(n @ 1..=100) => Ok(n + 1),
            Self::Page(_) => Err(index("Selected pages require a limit of 1–100")),
            Self::Address(_) => Ok(1),
            Self::ToolResults(_) => Ok(101),
        }
    }
}

fn read(
    options: &Options,
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    target: Target<'_>,
    cursor: Option<&Cursor>,
) -> Result<Option<Component>, ReadError> {
    let capacity = target.capacity()?;
    let db = crate::events::store::focused_connection(source.path).map_err(index)?;
    db.execute_batch("BEGIN DEFERRED").map_err(index)?;
    crate::events::store::require_current(&db).map_err(index)?;
    crate::storage::sqlite::qualify(&db, &source.namespace).map_err(index)?;
    if scope == CatalogReadScope::RetainedHistory {
        // This slice can qualify retained identity via `identity`, but only
        // opens current member carriers for body I/O.
        let retained = warehouse::retained_identity(&db, &source.namespace, key)
            .map_err(super::retained_error)?;
        return Err(index(format!(
            "Retained component {} body reads are unavailable",
            retained.owner
        )));
    }
    let incarnation: String = db
        .query_row("SELECT value FROM meta WHERE key='incarnation'", [], |r| {
            r.get(0)
        })
        .map_err(index)?;
    let selection = encode(&(source.namespace.as_str(), scope, key, incarnation))?;
    if cursor.is_some_and(|cursor| cursor.selection != selection) {
        return Err(ReadError::Restart);
    }
    let paths = encode(
        &source
            .inventory
            .iter()
            .filter_map(|(path, state)| {
                matches!(state, Availability::Expected(Some(_))).then_some(path)
            })
            .collect::<Vec<_>>(),
    )?;
    let Some((owner, harness, native_id)) = current_identity(&db, source, key, &paths)? else {
        return if cursor.is_some() {
            Err(ReadError::Restart)
        } else {
            Ok(None)
        };
    };
    let (component, title, model): (i64, Option<String>, Option<String>) = db
        .query_row(
            "SELECT m.component,coalesce(f.title,f.agent_name),d.model
         FROM lineage_member m JOIN dim_session d USING(source)
         JOIN warehouse_current c USING(source) JOIN files f USING(path) WHERE m.source=?1",
            [owner],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(index)?;
    let (latest, pruned_through): (i64, i64) = db
        .query_row(
            "SELECT coalesce(max(revision),0),coalesce((SELECT CAST(value AS INTEGER) FROM meta
             WHERE key='warehouse_prefix_pruned_through'),0) FROM warehouse_prefix",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(index)?;
    let view = cursor.map_or(latest, |cursor| cursor.view);
    let now = crate::model::now_ms();
    let opened_at = cursor.map_or(now, |cursor| cursor.opened_at);
    if view <= pruned_through
        || view > latest
        || opened_at > now
        || opened_at < now.saturating_sub(warehouse::PREFIX_WINDOW_MS)
    {
        return Err(ReadError::Restart);
    }
    let minimum = Position {
        time: i64::MIN,
        source: 0,
        offset: 0,
        block: 0,
        occurrence: 0,
    };
    let after = match target {
        Target::Page(_) => cursor.map_or(minimum, |cursor| cursor.after),
        Target::Address(_) | Target::ToolResults(_) => minimum,
    };
    let mut witness = Sha256::new_with_prefix(encode(&(owner, component))?);
    let mut candidates = BTreeMap::new();
    let mut member_count = 0;
    let mut confirmed = 0;
    let mut member_after = 0;
    {
        let mut members = db.prepare_cached(MEMBERS).map_err(index)?;
        let mut detail = db.prepare_cached(warehouse::DETAIL).map_err(index)?;
        let mut prefix = db.prepare_cached(COMMITTED_PREFIX).map_err(index)?;
        let mut continuations = db.prepare_cached(CONTINUATIONS).map_err(index)?;
        let mut results = db.prepare_cached(TOOL_RESULTS).map_err(index)?;
        loop {
            let mut page = members
                .query(params![
                    component,
                    source.namespace.binding(),
                    harness,
                    member_after,
                    MEMBER_PAGE
                ])
                .map_err(index)?;
            let mut count = 0;
            while let Some(row) = page.next().map_err(index)? {
                count += 1;
                member_count += 1;
                let id: i64 = row.get(0).map_err(index)?;
                member_after = id;
                let incarnation: i64 = row.get(1).map_err(index)?;
                let carrier: String = row.get(2).map_err(index)?;
                let current = decode(&row.get::<_, String>(3).map_err(index)?)?;
                let member_id: String = row.get(4).map_err(index)?;
                let current_path: Option<String> = row.get(5).map_err(index)?;
                let resume: Option<i64> = row.get(6).map_err(index)?;
                let committed = committed_prefix(&mut prefix, id, view)?;
                let sidecar: i64 = row.get(7).map_err(index)?;
                let path = relative(options, &harness, &carrier).ok_or(ReadError::Restart)?;
                let Some(Availability::Expected(Some(observed))) = source.inventory.get(&carrier)
                else {
                    return Err(ReadError::Restart);
                };
                if current_path.as_deref() != Some(&carrier)
                    || resume != Some(i64::try_from(current.offset).map_err(index)?)
                    || committed.offset > current.offset
                    || (committed.offset == current.offset
                        && (committed.prefix != current.prefix || committed.tail != current.tail))
                    || !committed.stat.is_prefix_of(&current.stat)
                    || !stable_or_grown(&current.stat, observed)
                    || !raw_stat(Path::new(&carrier))
                        .is_ok_and(|actual| stable_or_grown(observed, &actual))
                {
                    return Err(ReadError::Restart);
                }
                witness.update(encode(&(id, incarnation, &carrier, sidecar, &member_id))?);
                let witness_prefix = Prefix {
                    source: id,
                    incarnation,
                    offset: committed.offset,
                    digest: committed.prefix,
                    tail: committed.tail,
                };
                if let Some(prior) =
                    cursor.and_then(|cursor| cursor.touched.iter().find(|prior| prior.source == id))
                {
                    if prior != &witness_prefix {
                        return Err(ReadError::Restart);
                    }
                    confirmed += 1;
                }
                let reference = SessionSourceRef {
                    root: harness.clone(),
                    path: path.to_owned(),
                    native_id: member_id,
                    offset: committed.offset,
                    prefix_sha256: committed.prefix,
                    tail_sha256: committed.tail,
                };
                let member = Arc::new(Member {
                    carrier,
                    current,
                    reference,
                });
                let mut keep = |(position, kind)| {
                    candidates.insert(position, (kind, member.clone(), witness_prefix.clone()));
                    if candidates.len() > capacity {
                        candidates.pop_last();
                    }
                };
                let (start, end, entry_block, entry_occurrence) = match target {
                    Target::Page(_) => (0, committed.offset, None, None),
                    Target::Address(wanted)
                        if member.reference.path == wanted.path
                            && wanted.offset < committed.offset =>
                    {
                        (
                            wanted.offset,
                            wanted.offset + 1,
                            Some(wanted.block),
                            Some(wanted.occurrence),
                        )
                    }
                    Target::Address(_) => continue,
                    Target::ToolResults(call) => {
                        if member.reference.path == call.path && call.offset < committed.offset {
                            let mut positions = results
                                .query(params![
                                    id,
                                    i64::try_from(call.offset).map_err(index)?,
                                    call.block,
                                    call.occurrence,
                                    i64::try_from(committed.offset).map_err(index)?,
                                    capacity as i64
                                ])
                                .map_err(index)?;
                            while let Some(row) = positions.next().map_err(index)? {
                                keep(position(row, id).map_err(index)?);
                            }
                        }
                        continue;
                    }
                };
                let start = i64::try_from(start).map_err(index)?;
                let end = i64::try_from(end).map_err(index)?;
                let (time, tie) = if entry_block.is_some() {
                    let Some(time) = db
                        .query_row(
                            "SELECT coalesce(ts,-9223372036854775808) FROM fact_record
                         WHERE source=?1 AND pos=?2 AND block=?3 AND occurrence=?4 AND NOT gap",
                            params![id, start, entry_block, entry_occurrence],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(index)?
                    else {
                        continue;
                    };
                    (time, (i64::MIN, i64::MIN, i64::MIN))
                } else {
                    (
                        after.time,
                        match id.cmp(&after.source) {
                            std::cmp::Ordering::Less => (i64::MAX, i64::MAX, i64::MAX),
                            std::cmp::Ordering::Equal => {
                                (after.offset, after.block, after.occurrence)
                            }
                            std::cmp::Ordering::Greater => (i64::MIN, i64::MIN, i64::MIN),
                        },
                    )
                };
                let predecessors =
                    continuation_bounds(&mut continuations, &mut prefix, id, component, view)?;
                let mut frontier = (time, tie.0, tie.1, tie.2);
                let mut accepted = 0;
                while accepted < capacity {
                    let mut positions = detail.query(named_params! {
                        ":source": id, ":start": start, ":end": end, ":predecessors": predecessors,
                        ":entry_block": entry_block, ":entry_occurrence": entry_occurrence,
                        ":time": frontier.0, ":pos": frontier.1, ":block": frontier.2,
                        ":occurrence": frontier.3, ":limit": (capacity - accepted) as i64,
                    }).map_err(index)?;
                    let mut examined = 0;
                    while let Some(row) = positions.next().map_err(index)? {
                        examined += 1;
                        let (position, kind) = position(row, id).map_err(index)?;
                        frontier = (
                            position.time,
                            position.offset,
                            position.block,
                            position.occurrence,
                        );
                        if !row.get::<_, bool>(5).map_err(index)? {
                            keep((position, kind));
                            accepted += 1;
                        }
                    }
                    if examined == 0 {
                        break;
                    }
                }
            }
            if count < MEMBER_PAGE {
                break;
            }
        }
    }
    let members = format!("{:x}", witness.finalize());
    if cursor.is_some_and(|cursor| cursor.members != members || cursor.touched.len() != confirmed) {
        return Err(ReadError::Restart);
    }
    if matches!(target, Target::ToolResults(_)) && candidates.len() == capacity {
        return Err(index("Selected tool result budget exhausted"));
    }
    let more = matches!(target, Target::Page(_)) && candidates.len() == capacity;
    if more {
        candidates.pop_last();
    }
    let after = candidates.last_key_value().map(|(position, _)| *position);
    let db = chunk_proofs::Snapshot::new(db);
    let proof_bytes = Mutex::new(PAGE_BYTES);
    let mut readers = BTreeMap::new();
    let mut bytes_left = PAGE_BYTES;
    let mut records = Vec::with_capacity(candidates.len());
    let mut touched = BTreeMap::new();
    for (position, (kind, member, prefix)) in candidates {
        touched.entry(position.source).or_insert(prefix);
        let reader =
            match readers.entry(position.source) {
                std::collections::btree_map::Entry::Occupied(reader) => reader.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                    PrefixReader::open(db.clone(), source.path, member, &proof_bytes)?,
                ),
            };
        let offset = u64::try_from(position.offset).map_err(index)?;
        let native = crate::source_reader::read_record_bounded(
            reader,
            &reader.member.reference,
            offset,
            crate::tx::LINE_MAX as usize,
            &mut bytes_left,
        )
        .map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock || bytes_left == 0 {
                index("Selected page source budget exhausted")
            } else {
                ReadError::Restart
            }
        })?;
        records.push(Record {
            address: Address {
                path: reader.member.reference.path.clone(),
                offset,
                block: position.block,
                occurrence: position.occurrence,
            },
            kind,
            native,
        });
    }
    AFTER_BODY.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    for reader in readers.values_mut() {
        reader.finish().map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                index("Selected page source budget exhausted")
            } else {
                ReadError::Restart
            }
        })?;
    }
    let next = after.filter(|_| more).map(|after| Cursor {
        selection,
        members,
        view,
        opened_at,
        after,
        touched: touched.into_values().collect(),
    });
    Ok(Some(Component {
        harness,
        native_id,
        title,
        model,
        member_count,
        records,
        next,
    }))
}

type TestHook = std::cell::RefCell<Option<Box<dyn FnOnce()>>>;

thread_local! {
    pub(super) static AFTER_BODY: TestHook = std::cell::RefCell::new(None);
}

struct OpenFile {
    file: crate::sealed::LogFile,
    observed: Stat,
    blocks: BTreeMap<usize, Vec<u8>>,
}

struct PrefixReader<'a> {
    member: Arc<Member>,
    generation: String,
    proof: chunk_proofs::Focused,
    io: Mutex<OpenFile>,
    budget: &'a Mutex<usize>,
}

impl<'a> PrefixReader<'a> {
    fn open(
        db: Arc<chunk_proofs::Snapshot>,
        cache: &Path,
        member: Arc<Member>,
        budget: &'a Mutex<usize>,
    ) -> Result<Self, ReadError> {
        let file = crate::sealed::LogFile::open(Path::new(&member.carrier))
            .map_err(|_| ReadError::Restart)?;
        let observed = Stat::from_metadata(&file.metadata().map_err(|_| ReadError::Restart)?)
            .map_err(|_| ReadError::Restart)?;
        if !stable_or_grown(&member.current.stat, &observed) {
            return Err(ReadError::Restart);
        }
        let proof = chunk_proofs::Focused::with_db(db, cache, &member.carrier, &member.current)
            .map_err(|_| ReadError::Restart)?;
        let generation = member
            .reference
            .prefix_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self {
            member,
            generation,
            proof,
            io: Mutex::new(OpenFile {
                file,
                observed,
                blocks: BTreeMap::new(),
            }),
            budget,
        })
    }

    fn charge(budget: &Mutex<usize>, size: usize) -> io::Result<()> {
        let mut left = budget.lock().map_err(|_| changed())?;
        *left = left
            .checked_sub(size)
            .ok_or_else(|| io::Error::from(io::ErrorKind::WouldBlock))?;
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        let io = self.io.get_mut().map_err(|_| changed())?;
        let path = Path::new(&self.member.carrier);
        let current = raw_stat(path)?;
        if !stable_or_grown(&io.observed, &current) {
            return Err(changed());
        }
        if current != io.observed {
            for (&index, bytes) in &io.blocks {
                Self::charge(self.budget, bytes.len())?;
                if self.proof.read(&mut io.file, index)? != *bytes {
                    return Err(changed());
                }
            }
        }
        crate::source_carrier::recheck(path, &io.file, &current)?;
        Ok(())
    }
}

impl SessionSourceReader for PrefixReader<'_> {
    fn read_range(
        &self,
        source: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<SessionSourceRange> {
        let end = offset.checked_add(max as u64).ok_or_else(changed)?;
        if source != &self.member.reference
            || expected.is_some_and(|expected| expected != self.generation)
            || max == 0
            || max > crate::SESSION_SOURCE_CHUNK_MAX
            || end > source.offset
        {
            return Err(changed());
        }
        let mut io = self.io.lock().map_err(|_| changed())?;
        let OpenFile { file, blocks, .. } = &mut *io;
        let chunk = crate::source_chunks::SIZE as u64;
        let mut bytes = Vec::with_capacity(max);
        for block in offset / chunk..=(end - 1) / chunk {
            let start = block * chunk;
            let block = usize::try_from(block).map_err(io::Error::other)?;
            if let std::collections::btree_map::Entry::Vacant(entry) = blocks.entry(block) {
                let charge = (self.member.current.offset - start).min(chunk) as usize;
                Self::charge(self.budget, charge)?;
                entry.insert(self.proof.read(file, block)?);
            }
            let data = &blocks[&block];
            let lo = offset.saturating_sub(start) as usize;
            let hi = (end - start).min(data.len() as u64) as usize;
            bytes.extend_from_slice(&data[lo..hi]);
        }
        Ok(SessionSourceRange {
            generation: self.generation.clone(),
            length: source.offset,
            offset,
            bytes: bytes.into(),
            cached: true,
        })
    }
}
