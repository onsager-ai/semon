//! Selected warehouse positions and origin-local byte proofs for comparison tests.
use super::{
    Availability, CatalogReadScope, Connection, Ledger, ReadError, Source, Stat, current_identity,
    encode, index, warehouse,
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
    f.resume_at,p.ledger,coalesce(r.sidecar_revision,0)
    FROM lineage_member m
    CROSS JOIN warehouse_source s ON s.source=m.source AND s.origin=?2 AND s.harness=?3
    CROSS JOIN dim_session d ON d.source=m.source
    LEFT JOIN warehouse_current c ON c.source=m.source AND c.path=s.carrier
    LEFT JOIN files f ON f.path=c.path
    LEFT JOIN maintenance_cursor r ON r.source=m.source
    LEFT JOIN warehouse_prefix p ON p.revision=(SELECT revision FROM warehouse_prefix
        WHERE source=m.source AND revision<=?4 ORDER BY revision DESC LIMIT 1)
    WHERE m.component=?1 AND m.source>?5 ORDER BY m.source LIMIT ?6";

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
    prefix: Prefix,
}

fn decode(ledger: &str) -> Result<Ledger, ReadError> {
    serde_json::from_str(ledger).map_err(index)
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

enum Target<'a> {
    Page(usize),
    Address(&'a Address),
}

impl Target<'_> {
    fn capacity(&self) -> Result<usize, ReadError> {
        match *self {
            Self::Page(n @ 1..=100) => Ok(n + 1),
            Self::Page(_) => Err(index("Selected pages require a limit of 1–100")),
            Self::Address(_) => Ok(1),
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
    let latest: i64 = db
        .query_row(
            "SELECT coalesce(max(revision),0) FROM warehouse_prefix",
            [],
            |r| r.get(0),
        )
        .map_err(index)?;
    let view = cursor.map_or(latest, |cursor| cursor.view);
    if view <= 0 || view > latest {
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
        Target::Address(_) => minimum,
    };
    let mut witness = Sha256::new_with_prefix(encode(&(owner, component))?);
    let mut candidates = BTreeMap::new();
    let mut member_count = 0;
    let mut confirmed = 0;
    let mut member_after = 0;
    {
        let mut members = db.prepare_cached(MEMBERS).map_err(index)?;
        let mut detail = db.prepare_cached(warehouse::DETAIL).map_err(index)?;
        loop {
            let mut page = members
                .query(params![
                    component,
                    source.namespace.binding(),
                    harness,
                    view,
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
                let resume: Option<u64> = row.get(6).map_err(index)?;
                let Some(prefix) = row.get::<_, Option<String>>(7).map_err(index)? else {
                    return Err(ReadError::Restart);
                };
                let committed = decode(&prefix)?;
                let sidecar: i64 = row.get(8).map_err(index)?;
                let path = relative(options, &harness, &carrier).ok_or(ReadError::Restart)?;
                let Some(Availability::Expected(Some(observed))) = source.inventory.get(&carrier)
                else {
                    return Err(ReadError::Restart);
                };
                if current_path.as_deref() != Some(&carrier)
                    || resume != Some(current.offset)
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
                let prefix = Prefix {
                    source: id,
                    incarnation,
                    offset: committed.offset,
                    digest: committed.prefix,
                    tail: committed.tail,
                };
                if let Some(prior) =
                    cursor.and_then(|cursor| cursor.touched.iter().find(|prior| prior.source == id))
                {
                    if prior != &prefix {
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
                    prefix,
                });
                let mut keep = |(position, kind)| {
                    candidates.insert(position, (kind, member.clone()));
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
                };
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
                let mut positions = detail.query(named_params! {
                    ":source": id, ":start": start, ":end": end, ":view": view, ":component": component,
                    ":entry_block": entry_block, ":entry_occurrence": entry_occurrence,
                    ":time": time, ":pos": tie.0, ":block": tie.1, ":occurrence": tie.2,
                    ":limit": capacity as i64,
                }).map_err(index)?;
                while let Some(row) = positions.next().map_err(index)? {
                    keep(position(row, id).map_err(index)?);
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
    for (position, (kind, member)) in candidates {
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
        after,
        touched: readers
            .values()
            .map(|reader| reader.member.prefix.clone())
            .collect(),
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
            .prefix
            .digest
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
