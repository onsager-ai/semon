//! Selected warehouse positions and origin-local byte proofs.
use super::{
    Availability, CatalogReadScope, Ledger, ReadError, Source, Stat, current_identity, encode,
    index, warehouse,
};
pub(super) use crate::session_route::NativeRecordAddress as Address;
use crate::{
    Options, SessionSourceRange, SessionSourceReader, SessionSourceRef,
    events::store::chunk_proofs, native_evidence::RuntimeObservations,
};
use rusqlite::{OptionalExtension, named_params, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::OnceCell,
    collections::BTreeMap,
    io,
    path::Path,
    sync::{Arc, Mutex},
};

#[path = "causal.rs"]
pub(super) mod causal;
#[path = "render.rs"]
pub(super) mod render;
#[path = "retained.rs"]
mod retained;
pub(super) use retained::{
    PeerBinding, retained_activity_anchor, retained_addressed_preview, retained_annotations,
    retained_preview_turns, retained_trace_window, retained_turn,
};
#[cfg(test)]
pub(super) use retained::{
    retained_addressed_window, retained_latest_page, retained_page, retained_peer_launch_call,
    retained_peer_record_at, retained_peer_relay_address, retained_peer_window, retained_record_at,
};

const MEMBER_PAGE: i64 = 64;
// SQLite tags gap rows independently of any native record's `type` field.
const GAP: &str = "\0";

pub(super) const PAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy)]
enum BodyMode {
    Complete,
    Gap,
    Transcript,
}

impl BodyMode {
    fn transcript(kind: Option<&str>) -> Self {
        if kind == Some(GAP) {
            Self::Gap
        } else {
            Self::Transcript
        }
    }
}
/// The committed database view and byte allowance shared by selected reads
/// within one request. Release the SQL guard before any native proof read.
pub(super) struct ReadView<'a> {
    db: Arc<chunk_proofs::Snapshot>,
    options: &'a Options,
    runtime: OnceCell<RuntimeObservations>,
    names: OnceCell<BTreeMap<String, String>>,
    pub(super) archive: super::ArchiveReader<'a>,
    body_bytes: Mutex<usize>,
    transcript: bool,
    part: Option<render::Part>,
    proof_bytes: Mutex<usize>,
}

impl<'a> ReadView<'a> {
    pub(super) fn open(source: &Source<'a>) -> Result<Self, ReadError> {
        Self::open_with_budget(source, PAGE_BYTES)
    }

    pub(super) fn open_transcript(
        source: &Source<'a>,
        part: Option<render::Part>,
    ) -> Result<Self, ReadError> {
        let bytes = if matches!(part, Some(render::Part::Attachment(_))) {
            crate::transcript_range::ATTACHMENT_SOURCE_BYTES
        } else {
            crate::transcript_range::PAGE_SOURCE_BYTES
        };
        let mut view = Self::open_with_budget(source, bytes)?;
        view.transcript = true;
        view.part = part;
        Ok(view)
    }

    fn open_with_budget(source: &Source<'a>, bytes: usize) -> Result<Self, ReadError> {
        let db = crate::events::store::focused_connection(source.path).map_err(index)?;
        db.execute_batch("BEGIN DEFERRED").map_err(index)?;
        crate::events::store::require_current(&db).map_err(index)?;
        crate::storage::sqlite::qualify(&db, &source.namespace).map_err(index)?;
        Ok(Self::with_budget(db, source, bytes))
    }

    pub(super) fn from_connection(db: rusqlite::Connection, source: &Source<'a>) -> Self {
        Self::with_budget(db, source, PAGE_BYTES)
    }

    fn with_budget(db: rusqlite::Connection, source: &Source<'a>, bytes: usize) -> Self {
        Self {
            db: chunk_proofs::Snapshot::new(db),
            options: source.options,
            runtime: OnceCell::new(),
            names: OnceCell::new(),
            archive: super::ArchiveReader::with_budget(source.options, bytes),
            body_bytes: Mutex::new(bytes),
            transcript: false,
            part: None,
            proof_bytes: Mutex::new(bytes),
        }
    }

    pub(super) fn runtime(&self) -> &RuntimeObservations {
        self.runtime
            .get_or_init(|| RuntimeObservations::new(self.options))
    }

    pub(super) fn codex_name(&self, id: &str) -> Result<Option<&str>, ReadError> {
        if self.names.get().is_none() {
            let db = self.lock()?;
            let names = crate::events::store::catalog_names_json(&db)
                .map_err(index)?
                .map(|encoded| serde_json::from_str(&encoded))
                .transpose()
                .map_err(index)?
                .unwrap_or_default();
            let _ = self.names.set(names);
        }
        Ok(self
            .names
            .get()
            .and_then(|names| names.get(id))
            .map(String::as_str))
    }

    #[allow(clippy::too_many_arguments)]
    fn read_record(
        &self,
        reader: &dyn SessionSourceReader,
        source: i64,
        reference: &SessionSourceRef,
        offset: u64,
        images: Option<&crate::attachments::PromptMetadata>,
        mode: BodyMode,
        budget: &mut usize,
        fields: &mut Fields,
    ) -> Result<serde_json::Value, io::Error> {
        if matches!(mode, BodyMode::Gap) {
            return crate::source_reader::read_record_bytes_bounded(
                reader,
                reference,
                offset,
                crate::tx::LINE_MAX as usize,
                budget,
            )
            .map(|_| serde_json::Value::Null);
        }
        let image = match self.part {
            Some(render::Part::Attachment(image)) => Some(image),
            _ => None,
        };
        let recipe: Option<String> = if self.transcript && !matches!(mode, BodyMode::Complete) {
            self.db
                .lock()
                .map_err(|_| io::Error::other("Selected Index lock poisoned"))?
                .prepare_cached(
                    "SELECT recipe FROM fact_native_fields WHERE source=?1 AND pos=?2 AND block=0",
                )
                .map_err(io::Error::other)?
                .query_row(
                    params![source, i64::try_from(offset).map_err(io::Error::other)?],
                    |row| row.get(0),
                )
                .optional()
                .map_err(io::Error::other)?
        } else {
            None
        };
        if let Some(recipe) = recipe {
            let recipe: crate::native_field::SparseRecord =
                serde_json::from_str(&recipe).map_err(io::Error::other)?;
            let (preview, chunk) = match self.part {
                Some(render::Part::Text(chunk)) => (crate::model::MSG_MAX, Some(chunk)),
                Some(render::Part::Output) => (64 * 1024, None),
                _ => (crate::model::MSG_MAX, None),
            };
            let omitted = images
                .map(|images| images.omitted_ranges(image))
                .transpose()?
                .into_iter()
                .flatten();
            let (value, selected) =
                recipe.read(reader, reference, offset, (preview, chunk), omitted, budget)?;
            fields.insert((reference.path.clone(), offset), selected);
            return Ok(value);
        }
        if let Some(images) = images {
            return images.selected_record(reference, offset, image, reader, budget);
        }
        crate::source_reader::read_record_bounded(
            reader,
            reference,
            offset,
            crate::tx::LINE_MAX as usize,
            budget,
        )
    }

    pub(super) fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, ReadError> {
        self.db.lock().map_err(index)
    }
}

const MEMBERS: &str = "SELECT m.source,s.incarnation,s.carrier,s.ledger,d.native_id,c.path,
    f.resume_at,coalesce(r.sidecar_revision,0),d.lineage
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
// committed prefix. The native offset and block name the call. Warehouse
// occurrence distinguishes facts emitted for that line and is not shared with
// fact_record. Refuse a block with more than one call fact.
const TOOL_RESULTS: &str = "WITH call_at AS MATERIALIZED (
    SELECT min(call_id) AS call_id,min(pos) AS pos,min(block) AS block
    FROM fact_tool_call WHERE source=?1 AND pos=?2 AND block=?3
    HAVING count(*)=1 AND count(call_id)=1
), unique_call AS (
    SELECT c.call_id,c.pos,c.block FROM call_at c
    CROSS JOIN fact_tool_call all_calls ON all_calls.source=?1 AND all_calls.call_id=c.call_id
      AND all_calls.pos<?4
    GROUP BY c.call_id,c.pos,c.block HAVING count(*)=1
)
SELECT coalesce(r.ts,-9223372036854775808),r.pos,r.block,r.occurrence,r.kind,
       r.ts,r.failed,r.flags,r.correlation
FROM unique_call c CROSS JOIN fact_tool_result r ON r.source=?1 AND r.call_id=c.call_id
  AND ((r.call_pos IS NULL AND r.kind<>'copilot-execution')
       OR (r.call_pos=c.pos AND r.call_block=c.block))
WHERE r.pos<?4 AND r.kind<>'yield_poll' ORDER BY 1,r.pos,r.block,r.occurrence LIMIT ?5";

#[cfg(test)]
const EXACT_RELAY_RECEIPTS: &str =
    "SELECT r.source,c.path,s.ledger,r.pos,r.block,r.occurrence,r.ts,s.harness
    FROM fact_relay_receipt r INDEXED BY relay_receipt_correlation
    CROSS JOIN warehouse_source s ON s.source=r.source AND s.origin=?1
    CROSS JOIN warehouse_current c ON c.source=r.source AND c.path=s.carrier
    WHERE r.correlation=?2 ORDER BY r.source,r.pos,r.block,r.occurrence";

#[derive(Clone, Debug)]
#[cfg(test)]
pub(super) struct RelayReceipt {
    pub(super) path: String,
    pub(super) offset: u64,
    pub(super) block: i64,
    pub(super) at: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct Position {
    time: i64,
    source: i64,
    offset: i64,
    block: i64,
    occurrence: i64,
}

impl Position {
    const END: Self = Self {
        time: i64::MAX,
        source: i64::MAX,
        offset: i64::MAX,
        block: i64::MAX,
        occurrence: i64::MAX,
    };
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Prefix {
    source: i64,
    incarnation: i64,
    offset: u64,
    digest: [u8; 32],
    tail: [u8; 32],
    provider: Option<ProviderPin>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ProviderPin {
    generation: [u8; 32],
    length: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    direction: Direction,
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

#[derive(Debug)]
pub(super) struct Record {
    pub(super) source: i64,
    pub(super) prompt_anchor: Option<String>,
    pub(super) entry_anchor: Option<String>,
    pub(super) operation: Option<causal::Operation>,
    pub(super) causal: causal::Context,
    #[cfg(test)]
    pub(super) relay_receipts: Vec<RelayReceipt>,
    pub(super) address: Address,
    pub(super) reference: SessionSourceRef,
    pub(super) kind: Option<String>,
    pub(super) at: Option<i64>,
    pub(super) native: serde_json::Value,
    pub(super) images: Option<Vec<serde_json::Value>>,
}

fn prompt_images(
    view: &ReadView<'_>,
    source: i64,
    offset: u64,
    kind: Option<&str>,
) -> Result<Option<crate::attachments::PromptMetadata>, ReadError> {
    if !matches!(kind, Some("user" | "response_item" | "attachment")) {
        return Ok(None);
    }
    let row: Option<String> = view
        .lock()?
        .prepare_cached(
            "SELECT recipe FROM fact_prompt_images WHERE source=?1 AND pos=?2 AND block=0",
        )
        .map_err(index)?
        .query_row(
            params![source, i64::try_from(offset).map_err(index)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(index)?;
    row.as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(index)
}

#[cfg(test)]
fn relay_receipts(
    db: &rusqlite::Connection,
    source: &Source<'_>,
    result: Position,
    view: i64,
) -> Result<Vec<RelayReceipt>, ReadError> {
    let correlation: Option<Option<String>> = db
        .query_row(
            "SELECT correlation FROM fact_relay_result
         WHERE source=?1 AND pos=?2 AND block=?3 AND occurrence=?4 AND NOT failed
         ORDER BY kind LIMIT 1",
            params![
                result.source,
                result.offset,
                result.block,
                result.occurrence
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(index)?;
    let Some(correlation) = correlation.flatten() else {
        return Ok(Vec::new());
    };
    let mut receipts = db.prepare_cached(EXACT_RELAY_RECEIPTS).map_err(index)?;
    let mut prefix = db.prepare_cached(COMMITTED_PREFIX).map_err(index)?;
    let mut rows = receipts
        .query(params![source.namespace.binding(), correlation])
        .map_err(index)?;
    let mut qualified = Vec::new();
    while let Some(row) = rows.next().map_err(index)? {
        let id: i64 = row.get(0).map_err(index)?;
        let path: String = row.get(1).map_err(index)?;
        let offset = u64::try_from(row.get::<_, i64>(3).map_err(index)?).map_err(index)?;
        let ledger: String = row.get(2).map_err(index)?;
        let current = decode(&ledger)?;
        let harness: String = row.get(7).map_err(index)?;
        let Availability::Expected(Some(stat)) = source.availability(&harness, &path)? else {
            continue;
        };
        if !stable_or_grown(&current.stat, &stat)
            || !raw_stat(Path::new(&path)).is_ok_and(|actual| stable_or_grown(&stat, &actual))
        {
            continue;
        }
        let Some(receipt) = prefix
            .query_row(params![id, view], |r| r.get::<_, String>(0))
            .optional()
            .map_err(index)?
        else {
            continue;
        };
        let receipt = decode(&receipt)?;
        if offset < receipt.offset {
            qualified.push(RelayReceipt {
                path,
                offset,
                block: row.get(4).map_err(index)?,
                at: row.get(6).map_err(index)?,
            });
            if qualified.len() > 100 {
                return Err(index("Selected relay receipt budget exhausted"));
            }
        }
    }
    Ok(qualified)
}

fn prompt_anchor(
    query: &mut rusqlite::Statement<'_>,
    key: &str,
    reference: &SessionSourceRef,
    position: Position,
) -> Result<Option<String>, ReadError> {
    let kind: Option<String> = query
        .query_row(
            params![position.source, position.offset, position.block],
            |row| row.get(0),
        )
        .optional()
        .map_err(index)?;
    let offset = u64::try_from(position.offset).map_err(index)?;
    Ok(kind.map(|kind| causal::prompt_id(key, &reference.native_id, offset, &kind)))
}

const PROMPT_AT: &str = "SELECT kind FROM fact_prompt WHERE source=?1 AND pos=?2 AND block=?3 ORDER BY occurrence LIMIT 1";

#[cfg(test)]
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

pub(super) type Fields = BTreeMap<(String, u64), Vec<crate::native_field::SelectedField>>;

#[derive(Debug)]
pub(super) struct Component {
    pub(super) binding: String,
    pub(super) identity: crate::CatalogSessionIdentity,
    pub(super) harness: String,
    pub(super) native_id: String,
    pub(super) title: Option<String>,
    pub(super) model: Option<String>,
    pub(super) metadata: Option<super::Row>,
    pub(super) retained_metadata: Option<serde_json::Value>,
    #[cfg(test)]
    pub(super) member_count: usize,
    pub(super) latest_turn: Option<causal::TurnStart>,
    pub(super) runtime_status: Option<&'static str>,
    pub(super) records: Vec<Record>,
    pub(super) reply_records: BTreeMap<(String, u64), serde_json::Value>,
    pub(super) fields: Fields,
    pub(super) next: Option<Cursor>,
    pub(super) annotations: Option<AnnotationPage>,
    pub(super) home_detail: Option<HomeDetail>,
    home_origins: BTreeMap<Position, serde_json::Value>,
    pub(super) home_activity: Option<serde_json::Value>,
}

impl Component {
    pub(super) fn home_origin(&self, record: &Record) -> Option<&serde_json::Value> {
        self.home_origins
            .get(&record.causal.turn.as_ref()?.position)
    }

    pub(super) fn errors_generation(&self) -> String {
        if self.identity.read_scope == CatalogReadScope::RetainedHistory {
            return self.identity.generation.clone();
        }
        current_errors_generation(&self.identity.generation, self.runtime_status)
    }
}

#[derive(Debug, Serialize)]
pub(super) struct TranscriptObservation {
    pub(super) generation: String,
    pub(super) errors_generation: String,
    pub(super) total: i64,
}

pub(super) fn current_errors_generation(generation: &str, status: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(generation.as_bytes());
    digest.update(status.unwrap_or("off").as_bytes());
    format!("{:x}", digest.finalize())
}

/// Count committed entry positions before display rendering. The same UUID
/// suppression as transcript paging excludes records copied by continuation.
pub(super) fn entry_count(
    db: &rusqlite::Connection,
    source: &Source<'_>,
    (owner, harness, _): &super::Identity,
) -> Result<i64, ReadError> {
    db.query_row(
        "WITH members AS MATERIALIZED (
          SELECT m.source,CAST(json_extract(s.ledger,'$.offset') AS INTEGER) AS prefix_end
          FROM lineage_member root JOIN lineage_member m ON m.component=root.component
          JOIN warehouse_source s ON s.source=m.source AND s.origin=?2 AND s.harness=?3
          WHERE root.source=?1
        ), predecessors AS MATERIALIZED (
          SELECT target.source AS target, prior.source, prior.prefix_end
          FROM members target JOIN dim_session d ON d.source=target.source AND d.harness='claude'
          JOIN fact_relation_observation e ON e.target=d.native_id AND e.kind='continued-in'
          JOIN members prior ON prior.source=e.source AND e.pos<prior.prefix_end
          JOIN dim_session original ON original.source=prior.source AND original.lineage
        )
        SELECT count(*) FROM members m JOIN fact_record r ON r.source=m.source
        WHERE r.pos>=0 AND r.pos<m.prefix_end AND NOT EXISTS (
          SELECT 1 FROM predecessors p JOIN fact_record old ON old.source=p.source
            AND old.uuid=r.uuid AND old.pos<p.prefix_end WHERE p.target=m.source
        )",
        params![owner, source.namespace.binding(), harness],
        |row| row.get(0),
    )
    .map_err(index)
}

#[derive(Debug)]
pub(super) struct HomeDetail {
    pub(super) text: serde_json::Value,
    pub(super) from: Option<String>,
    pub(super) from_name: Option<String>,
    pub(super) from_reason: Option<&'static str>,
}

impl HomeDetail {
    fn from_turn(turn: &causal::TurnStart, native: &serde_json::Value) -> Self {
        let kinds: &[_] = if turn.id.starts_with('r') {
            &["received_cross", "received_agent", "received_down"]
        } else {
            &["prompt"]
        };
        let text = kinds
            .iter()
            .map(|kind| {
                crate::model::overview_text(
                    &serde_json::json!({"kind":kind,"block":turn.address.block}),
                    native,
                    None,
                )
            })
            .find(serde_json::Value::is_string)
            .unwrap_or_default();
        Self {
            text,
            from: (!turn.handoff).then(|| "you".into()),
            from_name: None,
            from_reason: turn.id.starts_with('r').then_some("not linked yet"),
        }
    }

    fn set_parent(&mut self, parent: &str, metadata: &serde_json::Value) {
        self.from = Some(parent.to_owned());
        self.from_reason = None;
        self.from_name = metadata.pointer("/family/ancestors").and_then(|ancestors| {
            ancestors.as_array()?.iter().find_map(|row| {
                (row["key"] == parent)
                    .then(|| row["name"].as_str())
                    .flatten()
                    .map(str::to_owned)
            })
        });
    }
}

struct Member {
    carrier: String,
    current: Ledger,
    reference: SessionSourceRef,
    remote: bool,
    changed_local: Option<Stat>,
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

pub(super) fn stable_or_grown(prior: &Stat, now: &Stat) -> bool {
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

pub(super) fn relative<'a>(options: &Options, harness: &str, path: &'a str) -> Option<&'a str> {
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

struct PositionSearch<'a> {
    source: i64,
    start: i64,
    end: i64,
    predecessors: &'a str,
    entry: Option<(i64, i64)>,
    frontier: (i64, i64, i64, i64),
    capacity: usize,
}

fn member_positions(
    query: &mut rusqlite::Statement<'_>,
    search: PositionSearch<'_>,
    mut keep: impl FnMut((Position, Option<String>)),
) -> Result<(), ReadError> {
    let (entry_block, entry_occurrence) = search.entry.unzip();
    let mut frontier = search.frontier;
    let mut accepted = 0;
    while accepted < search.capacity {
        let mut positions = query.query(named_params! {
            ":source": search.source, ":start": search.start, ":end": search.end,
            ":predecessors": search.predecessors, ":entry_block": entry_block,
            ":entry_occurrence": entry_occurrence, ":time": frontier.0, ":pos": frontier.1,
            ":block": frontier.2, ":occurrence": frontier.3, ":limit": (search.capacity - accepted) as i64,
        }).map_err(index)?;
        let mut examined = 0;
        while let Some(row) = positions.next().map_err(index)? {
            examined += 1;
            let (position, kind) = position(row, search.source).map_err(index)?;
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
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Direction {
    Forward,
    Reverse,
}

impl Direction {
    fn statement(self) -> &'static str {
        match self {
            Self::Forward => warehouse::DETAIL,
            Self::Reverse => include_str!("../events/store/warehouse_detail_reverse.sql"),
        }
    }

    fn frontier(self, after: Option<Position>, source: i64) -> (i64, i64, i64, i64) {
        let (start, end) = match self {
            Self::Forward => (i64::MIN, i64::MAX),
            Self::Reverse => (i64::MAX, i64::MIN),
        };
        let Some(after) = after else {
            return (start, start, start, start);
        };
        let (offset, block, occurrence) = match source.cmp(&after.source) {
            std::cmp::Ordering::Less => match self {
                Self::Forward => (end, end, end),
                Self::Reverse => (start, start, start),
            },
            std::cmp::Ordering::Equal => (after.offset, after.block, after.occurrence),
            std::cmp::Ordering::Greater => match self {
                Self::Forward => (start, start, start),
                Self::Reverse => (end, end, end),
            },
        };
        (after.time, offset, block, occurrence)
    }

    fn discard_extra<V>(self, positions: &mut BTreeMap<Position, V>) {
        match self {
            Self::Forward => positions.pop_last(),
            Self::Reverse => positions.pop_first(),
        };
    }

    fn continuation<V>(self, positions: &BTreeMap<Position, V>) -> Option<Position> {
        match self {
            Self::Forward => positions.last_key_value(),
            Self::Reverse => positions.first_key_value(),
        }
        .map(|(position, _)| *position)
    }
}

fn address_frontier(
    db: &rusqlite::Connection,
    direction: Direction,
    source: i64,
    address: &Address,
) -> Result<Option<(i64, i64, i64, i64)>, ReadError> {
    let offset = i64::try_from(address.offset).map_err(index)?;
    let time: Option<i64> = db
        .query_row(
            "SELECT coalesce(ts,-9223372036854775808) FROM fact_record
             WHERE source=?1 AND pos=?2 AND block=?3 AND occurrence=?4",
            params![source, offset, address.block, address.occurrence],
            |row| row.get(0),
        )
        .optional()
        .map_err(index)?;
    let (_, offset, block, occurrence) = direction.frontier(None, source);
    Ok(time.map(|time| (time, offset, block, occurrence)))
}

pub(super) fn prompt_title(
    view: &ReadView<'_>,
    source: &Source<'_>,
    carrier: String,
    ledger: &str,
    native_id: String,
    offset: i64,
) -> Result<Option<String>, ReadError> {
    let indexed_source: Option<i64> = view
        .lock()?
        .query_row(
            "SELECT w.source FROM warehouse_current w JOIN warehouse_source s ON s.source=w.source
         WHERE w.path=?1 AND s.origin=?2",
            params![carrier, source.namespace.binding()],
            |row| row.get(0),
        )
        .optional()
        .map_err(index)?;
    let images = indexed_source
        .map(|id| {
            prompt_images(
                view,
                id,
                u64::try_from(offset).map_err(index)?,
                Some("response_item"),
            )
        })
        .transpose()?
        .flatten();
    let mut budget = view.body_bytes.lock().map_err(index)?;
    let allowance = (*budget).min(256 * 1024);
    let mut bytes_left = allowance;
    let native = qualified_record(
        view.db.clone(),
        source,
        RecordProof {
            harness: "codex",
            carrier,
            ledger,
            native_id,
            offset,
            images: images.as_ref(),
        },
        &mut bytes_left,
        &view.proof_bytes,
    );
    *budget -= allowance - bytes_left;
    Ok(crate::model::prompt_text(&native?)
        .filter(|text| !text.trim().is_empty())
        .map(|text| crate::model::one_line(&text, 80)))
}

struct RecordProof<'a> {
    harness: &'a str,
    carrier: String,
    ledger: &'a str,
    native_id: String,
    offset: i64,
    images: Option<&'a crate::attachments::PromptMetadata>,
}

fn qualified_record(
    db: Arc<chunk_proofs::Snapshot>,
    source: &Source<'_>,
    proof: RecordProof<'_>,
    bytes_left: &mut usize,
    proof_budget: &Mutex<usize>,
) -> Result<serde_json::Value, ReadError> {
    let RecordProof {
        harness,
        carrier,
        ledger,
        native_id,
        offset,
        images,
    } = proof;
    let current = decode(ledger)?;
    let path = relative(source.options, harness, &carrier).ok_or(ReadError::Restart)?;
    let reference = SessionSourceRef {
        root: harness.into(),
        path: path.into(),
        native_id,
        offset: current.offset,
        prefix_sha256: current.prefix,
        tail_sha256: current.tail,
    };
    if !matches!(source.availability(harness, &carrier)?,
        Availability::Expected(Some(stat)) if stable_or_grown(&current.stat, &stat))
    {
        return Err(ReadError::Restart);
    }
    let mut reader = PrefixReader::open(
        db,
        source.path,
        Arc::new(Member {
            carrier,
            current,
            reference,
            remote: false,
            changed_local: None,
        }),
        proof_budget,
    )?;
    let offset = u64::try_from(offset).map_err(index)?;
    let native = if let Some(images) = images {
        images.selected_record(&reader.member.reference, offset, None, &reader, bytes_left)
    } else {
        crate::source_reader::read_record_bounded(
            &reader,
            &reader.member.reference,
            offset,
            *bytes_left,
            bytes_left,
        )
    }
    .map_err(|error| {
        if *bytes_left == 0 {
            ReadError::SourceBudget
        } else {
            body_error(error)
        }
    })?;
    reader.finish().map_err(body_error)?;
    Ok(native)
}

#[cfg(test)]
pub(super) fn launch_call(
    view: &ReadView<'_>,
    source: &Source<'_>,
    child: &str,
) -> Result<Option<crate::ToolCall>, ReadError> {
    let db = view.lock()?;
    let snapshot = source
        .focused
        .is_none()
        .then(|| super::Snapshot::read(&db, source))
        .transpose()?;
    let paths = snapshot.as_ref().map(|snapshot| snapshot.paths.as_str());
    let Some((owner, harness, native_id)) = current_identity(&db, source, child, paths)? else {
        return Ok(None);
    };
    if harness != "codex" {
        return Ok(None);
    }
    let marker_parent = super::parents::Parents::new(&db)?.parent(owner)?;
    let Some(marker_parent) = marker_parent.filter(|parent| parent.method == "marker") else {
        return Ok(None);
    };
    type MarkerRow = (String, String, i64, Option<String>, Option<i64>, i64);
    let marker: Option<MarkerRow> = db
        .query_row(
            "SELECT s.carrier,s.ledger,p.pos,f.cwd,d.first_ts,b.owner
         FROM fact_parent_observation p
         CROSS JOIN warehouse_source s ON s.source=p.source
         CROSS JOIN dim_session d ON d.source=p.source
         CROSS JOIN files f ON f.path=s.carrier
         CROSS JOIN dim_session parent ON parent.source=?2 AND parent.harness='claude'
          AND parent.native_id=p.parent_id AND parent.harness=p.parent_harness
         CROSS JOIN bridge_session_owner b ON b.source=parent.source
         WHERE p.source=?1 AND p.method='marker' AND p.call_id IS NULL
         ORDER BY p.pos DESC,p.block DESC,p.occurrence DESC LIMIT 1",
            params![owner, marker_parent.source],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(index)?;
    let Some((carrier, ledger, offset, cwd, start, parent)) = marker else {
        return Ok(None);
    };
    let calls = {
        let mut statement = db
            .prepare_cached(
                "SELECT s.carrier,s.ledger,d.native_id,c.pos,c.block,c.call_id,c.tool
             FROM lineage_member root CROSS JOIN lineage_member m ON m.component=root.component
             CROSS JOIN warehouse_source s ON s.source=m.source
             CROSS JOIN warehouse_current w ON w.source=s.source AND w.path=s.carrier
             CROSS JOIN dim_session d ON d.source=m.source
             CROSS JOIN fact_claude_call c ON c.source=m.source AND c.tool IN ('Bash','Skill')
             WHERE root.source=?1 AND c.pos<json_extract(s.ledger,'$.offset')
               AND (?2 IS NULL OR s.carrier IN (SELECT value FROM json_each(?2)))
             ORDER BY d.first_ts,d.continued DESC,d.native_id,c.pos,c.block
             LIMIT ?3",
            )
            .map_err(index)?;
        statement
            .query_map(
                params![parent, paths, crate::handoff::MAX_CALLS as i64 + 1],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, u32>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                },
            )
            .map_err(index)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(index)?
    };
    if calls.len() > crate::handoff::MAX_CALLS {
        return Err(index("Selected launch candidate budget exhausted"));
    }
    drop(db);
    let mut shared = view.body_bytes.lock().map_err(index)?;
    let allowed = crate::handoff::MAX_INPUT_BYTES.min(*shared);
    let mut bytes_left = allowed;
    let result = (|| {
        let native = qualified_record(
            view.db.clone(),
            source,
            RecordProof {
                harness: "codex",
                carrier,
                ledger: &ledger,
                native_id,
                offset,
                images: None,
            },
            &mut bytes_left,
            &view.proof_bytes,
        )?;
        let Some(prompt) = crate::user_text(&native)
            .and_then(crate::parse_marker)
            .and_then(|marker| marker.handoff)
        else {
            return Ok(None);
        };
        let mut matcher = crate::handoff::Matcher::new(&prompt, cwd.as_deref(), start);
        for (carrier, ledger, native_id, offset, block, id, name) in calls {
            let native = qualified_record(
                view.db.clone(),
                source,
                RecordProof {
                    harness: "claude",
                    carrier,
                    ledger: &ledger,
                    native_id,
                    offset,
                    images: None,
                },
                &mut bytes_left,
                &view.proof_bytes,
            )?;
            let Some(input) = crate::model::tool_input(&native, block as usize) else {
                continue;
            };
            if let Some(tool) = matcher.consider(&crate::handoff::Call {
                tool: crate::ToolCall { id, name },
                time: crate::events::record_time(&native),
                input: input.to_string(),
            }) {
                return Ok(Some(tool));
            }
        }
        Ok(matcher.finish())
    })();
    *shared -= allowed - bytes_left;
    source.validate_authority()?;
    result
}

#[cfg(test)]
pub(super) fn page(
    options: &Options,
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<Option<Component>, ReadError> {
    read(
        options,
        source,
        scope,
        key,
        Target::Page(limit, Direction::Forward),
        cursor,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn transcript_window(
    view: &ReadView<'_>,
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    limit: usize,
    cursor: Option<&Cursor>,
    address: Option<&crate::session_route::NativeEntryAddress>,
    retained_provider: Option<&dyn SessionSourceReader>,
    peer: Option<PeerBinding<'_>>,
) -> Result<Option<Component>, ReadError> {
    let target = address.map_or(Target::Page(limit, Direction::Reverse), |address| {
        Target::Address(&address.record, Some(&address.binding), None)
    });
    match scope {
        CatalogReadScope::Current => {
            read_current(view, source.options, source, scope, key, target, cursor)
        }
        CatalogReadScope::RetainedHistory => retained::read_frozen(
            view,
            source,
            key,
            target,
            cursor,
            retained_provider.unwrap_or(&view.archive),
            peer.map(|binding| (binding, retained_provider.is_none())),
        )
        .map(Some),
    }
}

#[cfg(test)]
pub(super) fn latest_page(
    options: &Options,
    source: &Source<'_>,
    key: &str,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<Option<Component>, ReadError> {
    read(
        options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Page(limit, Direction::Reverse),
        cursor,
    )
}

#[cfg(test)]
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
        Target::Address(address, None, None),
        cursor,
    )
    .map(|component| component.and_then(|component| component.records.into_iter().next()))
}

pub(super) fn addressed_preview(
    source: &Source<'_>,
    key: &str,
    address: &crate::session_route::NativeEntryAddress,
    generation: &str,
) -> Result<Option<Component>, ReadError> {
    read(
        source.options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Address(&address.record, Some(&address.binding), Some(generation)),
        None,
    )
}

#[derive(Clone, Copy)]
pub(super) struct AnnotationRequest<'a> {
    pub(super) kind: &'a str,
    pub(super) first: i64,
    pub(super) limit: i64,
}

#[derive(Debug)]
pub(super) struct AnnotationPage {
    pub(super) total: i64,
    pub(super) addresses: Vec<crate::session_route::NativeEntryAddress>,
}

type AnnotationMembers = BTreeMap<i64, (SessionSourceRef, i64, String)>;

#[derive(Default)]
struct PendingRuntime {
    include: bool,
    work: bool,
    boundary: Option<(i64, i64, i64)>,
}

impl PendingRuntime {
    fn retained(harness: &str) -> Self {
        Self {
            include: harness != "copilot",
            ..Self::default()
        }
    }
}

pub(super) fn live_pending(harness: &str, status: Option<&str>) -> bool {
    harness != "copilot" && matches!(status, Some("work" | "wait"))
}

impl PendingRuntime {
    fn current(harness: &str, status: Option<&str>, turn: Option<&causal::TurnStart>) -> Self {
        let mut runtime = Self::retained(harness);
        runtime.work = status == Some("work");
        if live_pending(harness, status) {
            runtime.boundary = turn.map(|turn| {
                let at = turn.position;
                (at.time, at.source, at.offset)
            });
        }
        runtime
    }
}

pub(super) fn pending_boundary(
    view: &ReadView<'_>,
    source: &Source<'_>,
    key: &str,
) -> Result<Option<causal::TurnStart>, ReadError> {
    let selected = read_current(
        view,
        source.options,
        source,
        CatalogReadScope::Current,
        key,
        Target::PendingBoundary,
        None,
    )?
    .ok_or(ReadError::Restart)?;
    Ok(selected.latest_turn)
}

pub(super) fn pending_counts_runtime(
    row: &super::Row,
    turn: Option<&causal::TurnStart>,
) -> Result<(bool, bool, String), ReadError> {
    let runtime = PendingRuntime::current(&row.harness, row.status, turn);
    Ok((runtime.include, runtime.work, encode(&runtime.boundary)?))
}

fn annotation_page(
    db: &rusqlite::Connection,
    source: &Source<'_>,
    key: &str,
    binding: &str,
    request: AnnotationRequest<'_>,
    members: &AnnotationMembers,
    runtime: PendingRuntime,
) -> Result<AnnotationPage, ReadError> {
    let members_json = members
        .iter()
        .map(|(id, (reference, revision, predecessors))| {
            (*id, reference.offset, revision, predecessors)
        })
        .collect::<Vec<_>>();
    let mut statement = db
        .prepare_cached(include_str!("annotation_page.sql"))
        .map_err(index)?;
    let mut rows = statement
        .query(params![
            encode(&members_json)?,
            request.kind,
            crate::events::ACK,
            crate::events::UNKNOWN,
            request.limit,
            request.first,
            runtime.include,
            runtime.work,
            encode(&runtime.boundary)?
        ])
        .map_err(index)?;
    let mut result = AnnotationPage {
        total: 0,
        addresses: Vec::new(),
    };
    while let Some(row) = rows.next().map_err(index)? {
        result.total = row.get(0).map_err(index)?;
        let Some(id) = row.get::<_, Option<i64>>(1).map_err(index)? else {
            break;
        };
        let (reference, _, _) = &members[&id];
        let offset = u64::try_from(row.get::<_, i64>(2).map_err(index)?).map_err(index)?;
        let event: Option<String> = row.get(5).map_err(index)?;
        let block = row.get(6).map_err(index)?;
        let signal = row
            .get::<_, Option<String>>(7)
            .map_err(index)?
            .map(|kind| serde_json::from_value(serde_json::Value::String(kind)).map_err(index))
            .transpose()?;
        let kind = if signal.is_some() { "signal" } else { "tool" };
        result
            .addresses
            .push(crate::session_route::NativeEntryAddress {
                record: Address {
                    path: reference.path.clone(),
                    offset,
                    block: row.get(3).map_err(index)?,
                    occurrence: row.get(4).map_err(index)?,
                },
                binding: binding.to_owned(),
                block,
                entry_id: crate::transcript_range::native_entry_id(
                    source.namespace.as_str(),
                    key,
                    reference,
                    (offset, block),
                    kind,
                    event
                        .as_deref()
                        .filter(|event| !event.is_empty() && event.len() <= 4096),
                    signal,
                ),
            });
    }
    Ok(result)
}

pub(super) fn annotations(
    source: &Source<'_>,
    key: &str,
    request: AnnotationRequest<'_>,
) -> Result<Option<Component>, ReadError> {
    read(
        source.options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Annotations(request),
        None,
    )
}

struct ActiveTool {
    position: Position,
    tool: String,
    since: i64,
    block: i64,
    cwd: Option<String>,
}

fn active_tool_candidate(
    db: &rusqlite::Connection,
    component: i64,
    harness: &str,
) -> Result<Option<ActiveTool>, ReadError> {
    let active = db
        .prepare_cached(include_str!("active_tool.sql"))
        .map_err(index)?
        .query_row(params![component, harness], |row| {
            let Some(since) = row.get(6)? else {
                return Ok(None);
            };
            Ok(Some(ActiveTool {
                position: Position {
                    time: row.get(0)?,
                    source: row.get(1)?,
                    offset: row.get(2)?,
                    block: row.get(3)?,
                    occurrence: row.get(4)?,
                },
                tool: row.get(5)?,
                since,
                block: row.get(7)?,
                cwd: row.get(8)?,
            }))
        })
        .optional()
        .map_err(index)?;
    Ok(active.flatten())
}

pub(super) fn active_tool(
    db: &rusqlite::Connection,
    source: &Source<'_>,
    owner: i64,
    key: &str,
    harness: &str,
) -> Result<Option<(String, i64)>, ReadError> {
    let (component, view) = db
        .query_row(
            "SELECT component,(SELECT max(revision) FROM warehouse_prefix)
             FROM lineage_member WHERE source=?1",
            [owner],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(index)?;
    let Some(active) = active_tool_candidate(db, component, harness)? else {
        return Ok(None);
    };
    let at = active.position;
    let end = Position::END;
    let turns = causal::turns(
        db,
        source,
        key,
        harness,
        component,
        view,
        [at, end].into_iter(),
    )?;
    Ok(turns[&at]
        .as_ref()
        .filter(|turn| turns[&end].as_ref() == Some(*turn))
        .map(|_| (active.tool, active.since)))
}

pub(super) fn turn(
    source: &Source<'_>,
    key: &str,
    turn: &str,
) -> Result<Option<Component>, ReadError> {
    read(
        source.options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Turns(&[turn], None),
        None,
    )
}

pub(super) fn trace_turn(
    view: &ReadView<'_>,
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    turn: &str,
    retained: &dyn SessionSourceReader,
) -> Result<Option<Component>, ReadError> {
    let turns = [turn];
    let target = Target::SenderAnchor(&turns);
    match scope {
        CatalogReadScope::Current => {
            read_current(view, source.options, source, scope, key, target, None)
        }
        CatalogReadScope::RetainedHistory => {
            retained::read_frozen(view, source, key, target, None, retained, None).map(Some)
        }
    }
}

pub(super) fn sender_window(
    view: &ReadView<'_>,
    source: &Source<'_>,
    component: &Component,
    turn: &causal::TurnStart,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Option<Component>, ReadError> {
    let key = &component.identity.catalog_key;
    let target = Target::SenderWindow(turn.position);
    match component.identity.read_scope {
        CatalogReadScope::Current => read_current(
            view,
            source.options,
            source,
            CatalogReadScope::Current,
            key,
            target,
            None,
        ),
        CatalogReadScope::RetainedHistory => retained::read_frozen(
            view,
            source,
            key,
            target,
            None,
            provider.unwrap_or(&view.archive),
            None,
        )
        .map(Some),
    }
}

pub(super) fn trace_record(
    view: &ReadView<'_>,
    source: &Source<'_>,
    key: &str,
    address: &Address,
) -> Result<Option<Component>, ReadError> {
    read_current(
        view,
        source.options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Address(address, None, None),
        None,
    )
}

pub(super) fn preview_turns(
    source: &Source<'_>,
    key: &str,
    turns: &[&str],
    home: HomeFields<'_>,
) -> Result<Option<Component>, ReadError> {
    read(
        source.options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Turns(turns, Some(home)),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn member_turn_positions(
    db: &rusqlite::Connection,
    positions: &mut rusqlite::Statement<'_>,
    key: &str,
    id: i64,
    reference: &SessionSourceRef,
    predecessors: &str,
    turns: &[&str],
    mut keep: impl FnMut((Position, Option<String>)),
) -> Result<(), ReadError> {
    if turns.iter().any(|turn| is_question_id(turn)) {
        let mut calls = db
            .prepare_cached(
                "SELECT c.pos,c.call_id,r.occurrence FROM fact_tool_call c
             JOIN fact_record r ON r.source=c.source AND r.pos=c.pos AND r.block=0 AND NOT r.gap
             WHERE c.source=?1 AND c.pos<?2 AND c.tool IN (
               'AskUserQuestion','request_user_input','request_user_input_async',
               'functions.request_user_input','functions.request_user_input_async')
             ORDER BY c.pos DESC,c.block DESC LIMIT 128",
            )
            .map_err(index)?;
        let mut rows = calls
            .query(params![id, i64::try_from(reference.offset).map_err(index)?])
            .map_err(index)?;
        while let Some(row) = rows.next().map_err(index)? {
            let offset: i64 = row.get(0).map_err(index)?;
            let call: Option<String> = row.get(1).map_err(index)?;
            let event = crate::model::question_id(
                &reference.native_id,
                u64::try_from(offset).map_err(index)?,
                call.as_deref(),
            );
            if !turns.contains(&event.as_str()) {
                continue;
            }
            let occurrence = row.get(2).map_err(index)?;
            member_positions(
                positions,
                PositionSearch {
                    source: id,
                    start: offset,
                    end: offset + 1,
                    predecessors,
                    entry: Some((0, occurrence)),
                    frontier: Direction::Forward.frontier(None, id),
                    capacity: 1,
                },
                &mut keep,
            )?;
        }
    }
    if turns.iter().any(|turn| is_question_id(turn)) {
        let mut results = db
            .prepare_cached(
                "SELECT a.pos,a.block,r.occurrence FROM fact_subagent_event a
             JOIN fact_record r ON r.source=a.source AND r.pos=a.pos AND r.block=0 AND NOT r.gap
             WHERE a.source=?1 AND a.pos<?2 AND a.kind='assistant'
             ORDER BY a.pos DESC,a.block DESC LIMIT 32",
            )
            .map_err(index)?;
        let mut rows = results
            .query(params![id, i64::try_from(reference.offset).map_err(index)?])
            .map_err(index)?;
        while let Some(row) = rows.next().map_err(index)? {
            let offset: i64 = row.get(0).map_err(index)?;
            let block: u32 = row.get(1).map_err(index)?;
            let event = crate::model::result_id(
                &reference.native_id,
                u64::try_from(offset).map_err(index)?,
                block,
            );
            if !turns.contains(&event.as_str()) {
                continue;
            }
            member_positions(
                positions,
                PositionSearch {
                    source: id,
                    start: offset,
                    end: offset + 1,
                    predecessors,
                    entry: Some((0, row.get(2).map_err(index)?)),
                    frontier: Direction::Forward.frontier(None, id),
                    capacity: 1,
                },
                &mut keep,
            )?;
        }
    }
    for turn in turns {
        turn_candidates(db, key, id, reference.offset, reference, turn, |address| {
            let Some(frontier) = address_frontier(db, Direction::Forward, id, address)? else {
                return Ok(());
            };
            let start = i64::try_from(address.offset).map_err(index)?;
            member_positions(
                positions,
                PositionSearch {
                    source: id,
                    start,
                    end: start + 1,
                    predecessors,
                    entry: Some((address.block, address.occurrence)),
                    frontier,
                    capacity: 1,
                },
                &mut keep,
            )
        })?;
    }
    Ok(())
}

fn home_event_origin<'a>(
    target: &Target<'_>,
    context: &'a causal::Context,
    reference: &SessionSourceRef,
    offset: u64,
    native: &serde_json::Value,
) -> Option<&'a causal::TurnStart> {
    let Target::Turns(requested, Some(_)) = target else {
        return None;
    };
    context.turn.as_ref().filter(|_| {
        context.calls.iter().any(|call| {
            is_question_tool(&call.tool)
                && requested.contains(
                    &crate::model::question_id(&reference.native_id, offset, Some(&call.id))
                        .as_str(),
                )
        }) || requested
            .iter()
            .any(|id| home_result(context, reference, offset, native, id).is_some())
    })
}

pub(super) fn home_result(
    context: &causal::Context,
    reference: &SessionSourceRef,
    offset: u64,
    native: &serde_json::Value,
    id: &str,
) -> Option<serde_json::Value> {
    let turn = context.turn.as_ref()?;
    if turn.handoff || !(crate::model::is_ask(&turn.id) || reference.root == "codex") {
        return None;
    }
    if !matches!(
        native.get("type").and_then(serde_json::Value::as_str),
        Some("assistant" | "response_item")
    ) || (reference.root == "codex"
        && native
            .pointer("/payload/role")
            .and_then(serde_json::Value::as_str)
            != Some("assistant"))
    {
        return None;
    }
    let blocks = native
        .pointer("/message/content")
        .and_then(serde_json::Value::as_array)
        .map_or(1, Vec::len);
    (0..blocks).find_map(|block| {
        let block = u32::try_from(block).ok()?;
        if crate::model::result_id(&reference.native_id, offset, block) != id {
            return None;
        }
        let text = crate::model::overview_text(
            &serde_json::json!({"kind":"assistant","block":block}),
            native,
            None,
        );
        text.is_string().then_some(text)
    })
}

pub(super) fn is_question_id(id: &str) -> bool {
    id.strip_prefix('q')
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

pub(super) fn is_question_tool(tool: &str) -> bool {
    matches!(
        tool,
        "AskUserQuestion"
            | "request_user_input"
            | "request_user_input_async"
            | "functions.request_user_input"
            | "functions.request_user_input_async"
    )
}

fn turn_candidates(
    db: &rusqlite::Connection,
    key: &str,
    source: i64,
    end: u64,
    reference: &SessionSourceRef,
    turn: &str,
    mut accept: impl FnMut(&Address) -> Result<(), ReadError>,
) -> Result<(), ReadError> {
    let offset = turn
        .strip_prefix(key)
        .and_then(|suffix| suffix.strip_prefix(':'))
        .and_then(|suffix| suffix.strip_prefix(&reference.native_id))
        .and_then(|suffix| suffix.strip_prefix(':'))
        .and_then(|suffix| suffix.parse::<i64>().ok());
    let launch = turn == crate::model::spawn_turn_id(&reference.root, key);
    let ask = crate::model::is_ask(turn);
    let relay = turn
        .strip_prefix('r')
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if offset.is_none() && !launch && !ask && !relay {
        return Ok(());
    }
    let mut lookup = db
        .prepare_cached(include_str!("turn_anchor.sql"))
        .map_err(index)?;
    let mut rows = lookup
        .query(params![
            source,
            i64::try_from(end).map_err(index)?,
            offset,
            launch,
            ask,
            relay,
            crate::events::AGM_PROGRESS
        ])
        .map_err(index)?;
    while let Some(row) = rows.next().map_err(index)? {
        let offset = u64::try_from(row.get::<_, i64>(0).map_err(index)?).map_err(index)?;
        let kind = row.get::<_, String>(1).map_err(index)?;
        let correlation = row.get::<_, Option<String>>(2).map_err(index)?;
        if !launch
            && turn
                != if kind == "relay" {
                    crate::model::incoming_relay_turn_id(
                        correlation.as_deref(),
                        &reference.native_id,
                        offset,
                    )
                } else {
                    causal::prompt_id(key, &reference.native_id, offset, &kind)
                }
        {
            continue;
        }
        accept(&Address {
            path: reference.path.clone(),
            offset,
            block: 0,
            occurrence: row.get(3).map_err(index)?,
        })?;
    }
    Ok(())
}

#[cfg(test)]
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

/// Native provenance supplied by a selected transcript or activity step.
#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) enum Evidence<'a> {
    Event(&'a str),
    Call(&'a str),
}

#[cfg(test)]
pub(super) fn evidence_at(
    options: &Options,
    source: &Source<'_>,
    key: &str,
    evidence: Evidence<'_>,
    cursor: Option<&Cursor>,
) -> Result<Option<Record>, ReadError> {
    read(
        options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Evidence(evidence),
        cursor,
    )
    .map(|component| component.and_then(|component| component.records.into_iter().next()))
}

pub(super) fn activity_anchor(
    source: &Source<'_>,
    key: &str,
    anchor: &crate::transcript_range::StepAnchor,
) -> Result<Option<Component>, ReadError> {
    read(
        source.options,
        source,
        CatalogReadScope::Current,
        key,
        Target::Activity(anchor),
        None,
    )
}

struct ActivityEvidence {
    sources: BTreeMap<std::path::PathBuf, crate::model::summary::CatalogSource>,
    events: String,
    usage: Option<crate::events::session_activity::SourcePosition>,
    call: Option<crate::events::session_activity::SourcePosition>,
}

impl ActivityEvidence {
    fn matches_member(&mut self, mut selected: crate::model::summary::CatalogSource) -> bool {
        let Some(saved) = self.sources.remove(&selected.path) else {
            return false;
        };
        selected.native_role.clone_from(&saved.native_role);
        selected == saved
    }

    fn candidate(
        &self,
        query: &mut rusqlite::Statement<'_>,
        id: i64,
        end: u64,
        carrier: &str,
        predecessors: &str,
    ) -> Result<Option<(Position, Option<String>)>, ReadError> {
        let offset = |position: Option<&crate::events::session_activity::SourcePosition>| {
            position
                .filter(|position| position.source == carrier)
                .map(|position| i64::try_from(position.offset).map_err(index))
                .transpose()
        };
        query
            .query_row(
                named_params! {
                    ":source": id, ":end": i64::try_from(end).map_err(index)?,
                    ":events": self.events, ":usage": offset(self.usage.as_ref())?,
                    ":call": offset(self.call.as_ref())?, ":predecessors": predecessors,
                },
                |row| position(row, id),
            )
            .optional()
            .map_err(index)
    }
}

fn activity_evidence(
    db: &rusqlite::Connection,
    key: &str,
    anchor: &crate::transcript_range::StepAnchor,
) -> Result<Option<ActivityEvidence>, ReadError> {
    use crate::events::session_activity::Usage;
    let encoded: Option<String> = db
        .query_row(
            "SELECT metadata FROM session_activity_sources WHERE session_key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(index)?;
    let Some(encoded) = encoded else {
        return Ok(None);
    };
    let row: crate::model::summary::CatalogRow = serde_json::from_str(&encoded).map_err(index)?;
    let Some((step, usage)) = crate::events::store::session_activity::step_at(
        db,
        &row,
        &anchor.generation,
        anchor.ordinal,
    )
    .map_err(index)?
    else {
        return Ok(None);
    };
    if step.id != anchor.id {
        return Ok(None);
    }
    let (events, usage, call) = match usage {
        Usage::Claude(usage) if usage.record_ids.is_empty() => {
            ("[]".into(), Some(step.position), None)
        }
        Usage::Claude(usage) if usage.record_ids.len() <= 256 => {
            (encode(&usage.record_ids)?, None, None)
        }
        Usage::Claude(_) => return Ok(None),
        Usage::Codex(_) => ("[]".into(), None, step.call_position),
    };
    let sources = row
        .sources
        .into_iter()
        .map(|source| (source.path.clone(), source))
        .collect();
    Ok(Some(ActivityEvidence {
        sources,
        events,
        usage,
        call,
    }))
}

#[cfg(test)]
const EVIDENCE: &str = include_str!("evidence.sql");

#[derive(Clone, Copy)]
pub(super) struct HomeFields<'a> {
    pub(super) expected: &'a str,
    pub(super) detail: bool,
    pub(super) activity: bool,
}

enum Target<'a> {
    Page(usize, Direction),
    Address(&'a Address, Option<&'a str>, Option<&'a str>),
    #[cfg(test)]
    Evidence(Evidence<'a>),
    Turns(&'a [&'a str], Option<HomeFields<'a>>),
    SenderAnchor(&'a [&'a str]),
    SenderWindow(Position),
    Annotations(AnnotationRequest<'a>),
    PendingBoundary,
    Activity(&'a crate::transcript_range::StepAnchor),
    #[cfg_attr(not(test), allow(dead_code))]
    ToolResults(&'a Address),
}

impl Target<'_> {
    fn expected_generation(&self) -> Option<&str> {
        match *self {
            Self::Turns(_, Some(home)) => Some(home.expected),
            Self::Address(_, _, generation) => generation,
            _ => None,
        }
    }

    fn capacity(&self) -> Result<usize, ReadError> {
        match *self {
            Self::Page(n @ 1..=100, _) => Ok(n + 1),
            Self::Page(..) => Err(index("Selected pages require a limit of 1–100")),
            Self::Address(..) | Self::Activity(_) => Ok(1),
            #[cfg(test)]
            Self::Evidence(_) => Ok(1),
            Self::Turns(..) | Self::SenderAnchor(_) | Self::SenderWindow(_) => Ok(33),
            Self::Annotations(_) | Self::PendingBoundary => Ok(0),
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
    let view = ReadView::open(source)?;
    read_current(&view, options, source, scope, key, target, cursor)
}

fn read_current(
    read_view: &ReadView<'_>,
    options: &Options,
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    target: Target<'_>,
    cursor: Option<&Cursor>,
) -> Result<Option<Component>, ReadError> {
    let direction = match target {
        Target::Page(_, direction) => {
            if cursor.is_some_and(|cursor| cursor.direction != direction) {
                return Err(ReadError::Restart);
            }
            direction
        }
        Target::Address(_, Some(_), _) => Direction::Reverse,
        _ => Direction::Forward,
    };
    let capacity = target.capacity()?;
    let provider = source
        .current_provider
        .map(|provider| crate::source_reader::BudgetedReader::new(provider, 1024));
    let capacity = if provider.is_some()
        && matches!(target, Target::Page(..))
        && (source.focused.is_some()
            || source
                .inventory
                .values()
                .any(|state| *state == Availability::Expected(None)))
    {
        // Reauthorizing every prior provider source and reading up to 32 new
        // sources fits the request reader's 64-source budget.
        capacity.min(33)
    } else {
        capacity
    };
    let transaction = read_view.lock()?;
    let db = &*transaction;
    if scope == CatalogReadScope::RetainedHistory {
        // This slice can qualify retained identity via `identity`, but only
        // opens current member carriers for body I/O.
        let retained = warehouse::retained_identity(db, &source.namespace, key)
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
    let paths = source
        .focused
        .is_none()
        .then(|| {
            encode(
                &source
                    .inventory
                    .iter()
                    .filter_map(|(path, state)| match state {
                        Availability::Expected(Some(_)) => Some(path),
                        Availability::Expected(None) if provider.is_some() => Some(path),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .transpose()?;
    let Some((owner, harness, native_id)) = current_identity(db, source, key, paths.as_deref())?
    else {
        return if cursor.is_some() {
            Err(ReadError::Restart)
        } else {
            Ok(None)
        };
    };
    let mut activity = if let Target::Activity(anchor) = target {
        let Some(evidence) = activity_evidence(db, key, anchor)? else {
            return Ok(None);
        };
        Some(evidence)
    } else {
        None
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
    let mut witness = Sha256::new_with_prefix(encode(&(owner, component))?);
    let mut candidates = BTreeMap::new();
    let mut home_members = BTreeMap::new();
    let mut needs_local_proof = false;
    let include_home_detail = matches!(
        target,
        Target::Turns(_, Some(HomeFields { detail: true, .. }))
    );
    let include_home_activity = matches!(
        target,
        Target::Turns(_, Some(HomeFields { activity: true, .. }))
    );
    let mut annotation_members = BTreeMap::new();
    let mut references = Vec::new();
    #[cfg(test)]
    let mut evidence_matches = 0;
    let mut prior_provider = Vec::new();
    let mut confirmed = 0;
    let mut member_after = 0;
    {
        let mut members = db.prepare_cached(MEMBERS).map_err(index)?;
        let mut detail = db.prepare_cached(direction.statement()).map_err(index)?;
        #[cfg(test)]
        let mut evidence = db.prepare_cached(EVIDENCE).map_err(index)?;
        let mut activity_query = db
            .prepare_cached(include_str!("activity_evidence.sql"))
            .map_err(index)?;
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
                let (mut remote, changed_local) = match source.availability(&harness, &carrier)? {
                    Availability::Expected(Some(observed))
                        if stable_or_grown(&current.stat, &observed)
                            && raw_stat(Path::new(&carrier))
                                .is_ok_and(|actual| stable_or_grown(&observed, &actual)) =>
                    {
                        (false, None)
                    }
                    Availability::Expected(None)
                        if provider.is_some()
                            && (source.provider_only() || local_missing(&carrier)) =>
                    {
                        (true, None)
                    }
                    Availability::Expected(Some(observed))
                        if matches!(
                            source.mode,
                            crate::SessionSourceReadMode::LocalOrPinnedProvider
                        ) && raw_stat(Path::new(&carrier))
                            .is_ok_and(|actual| actual == observed) =>
                    {
                        (false, Some(observed))
                    }
                    _ => return Err(ReadError::Restart),
                };
                if current_path.as_deref() != Some(&carrier)
                    || resume != Some(i64::try_from(current.offset).map_err(index)?)
                    || committed.offset > current.offset
                    || (committed.offset == current.offset
                        && (committed.prefix != current.prefix || committed.tail != current.tail))
                    || !committed.stat.is_prefix_of(&current.stat)
                {
                    return Err(ReadError::Restart);
                }
                needs_local_proof |= changed_local.is_some();
                witness.update(encode(&(
                    id,
                    incarnation,
                    &carrier,
                    sidecar,
                    &member_id,
                    remote,
                    changed_local,
                ))?);
                let mut witness_prefix = Prefix {
                    source: id,
                    incarnation,
                    offset: committed.offset,
                    digest: committed.prefix,
                    tail: committed.tail,
                    provider: None,
                };
                let reference = SessionSourceRef {
                    root: harness.clone(),
                    path: path.to_owned(),
                    native_id: member_id,
                    offset: committed.offset,
                    prefix_sha256: committed.prefix,
                    tail_sha256: committed.tail,
                };
                references.push(crate::CatalogSourceObservation {
                    source: reference.clone(),
                    state: if remote { "unavailable" } else { "cached" }.into(),
                });
                if let Some(prior) =
                    cursor.and_then(|cursor| cursor.touched.iter().find(|prior| prior.source == id))
                {
                    witness_prefix.provider.clone_from(&prior.provider);
                    if prior != &witness_prefix
                        || (changed_local.is_none() && remote != prior.provider.is_some())
                    {
                        return Err(ReadError::Restart);
                    }
                    if let Some(pin) = &prior.provider {
                        remote = true;
                        prior_provider.push((
                            id,
                            carrier.clone(),
                            changed_local,
                            reference.clone(),
                            pin,
                        ));
                    }
                    confirmed += 1;
                }
                if activity.as_mut().is_some_and(|activity| {
                    !activity
                        .matches_member(committed.source(Path::new(&carrier), &reference.native_id))
                }) {
                    return Ok(None);
                }
                let member = Arc::new(Member {
                    carrier,
                    current,
                    reference,
                    remote,
                    changed_local,
                });
                let mut keep = |(position, kind)| {
                    candidates.insert(position, (kind, member.clone(), witness_prefix.clone()));
                    if candidates.len() > capacity {
                        direction.discard_extra(&mut candidates);
                    }
                };
                let (start, end, entry_block, entry_occurrence) = match target {
                    Target::Page(..) | Target::SenderWindow(_) => (0, committed.offset, None, None),
                    Target::Address(wanted, ..)
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
                    Target::Address(..) => continue,
                    Target::PendingBoundary => continue,
                    Target::Annotations(_) => {
                        let predecessors = continuation_bounds(
                            &mut continuations,
                            &mut prefix,
                            id,
                            component,
                            view,
                        )?;
                        annotation_members
                            .insert(id, (member.reference.clone(), view, predecessors));
                        continue;
                    }
                    Target::Turns(turns, _) | Target::SenderAnchor(turns) => {
                        if include_home_detail
                            || include_home_activity
                            || (matches!(target, Target::Turns(_, Some(_)))
                                && turns.iter().any(|turn| is_question_id(turn)))
                        {
                            home_members.insert(id, member.clone());
                        }
                        let predecessors = continuation_bounds(
                            &mut continuations,
                            &mut prefix,
                            id,
                            component,
                            view,
                        )?;
                        member_turn_positions(
                            db,
                            &mut detail,
                            key,
                            id,
                            &member.reference,
                            &predecessors,
                            turns,
                            &mut keep,
                        )?;
                        continue;
                    }
                    Target::Activity(_) => {
                        let activity = activity
                            .as_ref()
                            .expect("activity target qualified before paging");
                        let predecessors = continuation_bounds(
                            &mut continuations,
                            &mut prefix,
                            id,
                            component,
                            view,
                        )?;
                        if let Some(found) = activity.candidate(
                            &mut activity_query,
                            id,
                            committed.offset,
                            &member.carrier,
                            &predecessors,
                        )? {
                            keep(found);
                        }
                        continue;
                    }
                    #[cfg(test)]
                    Target::Evidence(wanted) => {
                        if evidence_matches < 2 {
                            let predecessors = continuation_bounds(
                                &mut continuations,
                                &mut prefix,
                                id,
                                component,
                                view,
                            )?;
                            let (event, call) = match wanted {
                                Evidence::Event(id) => (Some(id), None),
                                Evidence::Call(id) => (None, Some(id)),
                            };
                            let mut rows = evidence.query(named_params! {
                                ":source": id, ":end": i64::try_from(committed.offset).map_err(index)?,
                                ":event": event, ":call": call, ":predecessors": predecessors,
                            }).map_err(index)?;
                            while let Some(row) = rows.next().map_err(index)? {
                                evidence_matches += 1;
                                keep(position(row, id).map_err(index)?);
                            }
                        }
                        continue;
                    }
                    Target::ToolResults(call) => {
                        if member.reference.path == call.path && call.offset < committed.offset {
                            let mut positions = results
                                .query(params![
                                    id,
                                    i64::try_from(call.offset).map_err(index)?,
                                    call.block,
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
                let frontier = if let Target::Address(address, ..) = target {
                    let Some(frontier) = address_frontier(db, direction, id, address)? else {
                        continue;
                    };
                    frontier
                } else {
                    direction.frontier(
                        match target {
                            Target::SenderWindow(after) => Some(after),
                            _ => cursor.map(|cursor| cursor.after),
                        },
                        id,
                    )
                };
                let predecessors =
                    continuation_bounds(&mut continuations, &mut prefix, id, component, view)?;
                member_positions(
                    &mut detail,
                    PositionSearch {
                        source: id,
                        start,
                        end,
                        predecessors: &predecessors,
                        entry: entry_block.zip(entry_occurrence),
                        frontier,
                        capacity,
                    },
                    &mut keep,
                )?;
            }
            if count < MEMBER_PAGE {
                break;
            }
        }
    }
    if activity.is_some_and(|activity| !activity.sources.is_empty()) {
        return Ok(None);
    }
    let members = format!("{:x}", witness.finalize());
    let binding = component_binding(&selection, &members)?;
    let runtime_status = if direction == Direction::Reverse
        || include_home_activity
        || matches!(target, Target::Annotations(_))
    {
        super::selected_runtime_status(db, source, owner, read_view)?
    } else {
        None
    };
    let need_pending_turn = matches!(target, Target::PendingBoundary)
        || (live_pending(&harness, runtime_status)
            && (matches!(
                target,
                Target::Page(_, Direction::Reverse) | Target::Address(_, Some(_), None)
            ) || matches!(target, Target::Annotations(request) if request.kind == "errors")));
    if let Target::Address(_, Some(expected), _) = target
        && expected != binding
    {
        return Err(ReadError::Restart);
    }
    if cursor.is_some_and(|cursor| cursor.members != members || cursor.touched.len() != confirmed) {
        return Err(ReadError::Restart);
    }
    let owner_identity = (owner, harness.clone(), native_id);
    let identity_witness =
        super::current_identity_witness(db, source, key, &owner_identity, &references)?;
    if target
        .expected_generation()
        .is_some_and(|expected| expected != identity_witness)
    {
        return Err(ReadError::Restart);
    }
    let mut provider_pins = BTreeMap::new();
    for (id, carrier, changed_local, reference, expected) in prior_provider {
        let observed = pin_provider(provider.as_ref().ok_or(ReadError::Restart)?, &reference)?;
        if expected != &observed {
            return Err(ReadError::Restart);
        }
        provider_pins.insert(id, (carrier, changed_local, observed));
    }
    if matches!(target, Target::ToolResults(_)) && candidates.len() == capacity {
        return Err(index("Selected tool result budget exhausted"));
    }
    #[cfg(test)]
    if evidence_matches > 1 {
        candidates.clear();
    }
    let more = matches!(target, Target::Page(..)) && candidates.len() == capacity;
    if more {
        direction.discard_extra(&mut candidates);
    }
    let after = direction.continuation(&candidates);
    let mut anchors = BTreeMap::new();
    {
        let mut prompts = db.prepare_cached(PROMPT_AT).map_err(index)?;
        if !matches!(target, Target::ToolResults(_)) {
            for (position, (_, member, _)) in &candidates {
                if let Some(anchor) =
                    prompt_anchor(&mut prompts, key, &member.reference, *position)?
                {
                    anchors.insert(*position, anchor);
                }
            }
        }
    }
    let mut causal = BTreeMap::new();
    if !matches!(target, Target::ToolResults(_)) {
        for (position, (_, member, _)) in &candidates {
            causal.insert(
                *position,
                causal::Inputs::load(
                    db,
                    &member.reference,
                    *position,
                    view,
                    direction == Direction::Reverse && harness == "claude",
                )?,
            );
        }
    }
    #[cfg(test)]
    let mut relay_context = if matches!(target, Target::ToolResults(_)) {
        candidates
            .keys()
            .map(|position| Ok((*position, relay_receipts(db, source, *position, view)?)))
            .collect::<Result<BTreeMap<_, _>, ReadError>>()?
    } else {
        BTreeMap::new()
    };
    let home_activity = if include_home_activity && runtime_status == Some("work") {
        active_tool_candidate(db, component, &harness)?
    } else {
        None
    };
    let need_latest_turn =
        need_pending_turn || !causal.is_empty() || include_home_detail || home_activity.is_some();
    drop(transaction);
    let mut readers = BTreeMap::new();
    let mut fields = Fields::new();
    let mut bytes_left = read_view.body_bytes.lock().map_err(index)?;
    let mut read_body = |id,
                         member: &Arc<Member>,
                         offset,
                         images: Option<&crate::attachments::PromptMetadata>,
                         mode| {
        if !member.remote
            && !provider_pins.contains_key(&id)
            && let std::collections::btree_map::Entry::Vacant(entry) = readers.entry(id)
        {
            let local = PrefixReader::open(
                read_view.db.clone(),
                source.path,
                member.clone(),
                &read_view.proof_bytes,
            )
            .and_then(|mut reader| {
                if member.changed_local.is_some() {
                    reader
                        .read_range(&member.reference, None, offset, 1)
                        .map_err(body_error)?;
                    reader.finish().map_err(body_error)?;
                }
                Ok(reader)
            });
            match local {
                Ok(reader) => {
                    entry.insert(reader);
                }
                Err(ReadError::Restart)
                    if provider.is_some()
                        && member.changed_local.is_some()
                        && raw_stat(Path::new(&member.carrier)).ok() == member.changed_local => {}
                Err(error) => return Err(error),
            }
        }
        let (reader, pin): (&dyn SessionSourceReader, Option<ProviderPin>) =
            if let Some(reader) = readers.get(&id) {
                (reader, None)
            } else {
                let provider = provider.as_ref().ok_or(ReadError::Restart)?;
                let (_, _, pin) = match provider_pins.entry(id) {
                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::btree_map::Entry::Vacant(entry) => entry.insert((
                        member.carrier.clone(),
                        member.changed_local,
                        pin_provider(provider, &member.reference)?,
                    )),
                };
                (provider, Some(pin.clone()))
            };
        let native = read_view
            .read_record(
                reader,
                id,
                &member.reference,
                offset,
                images,
                mode,
                &mut bytes_left,
                &mut fields,
            )
            .map_err(|error| {
                if *bytes_left == 0 {
                    ReadError::SourceBudget
                } else {
                    body_error(error)
                }
            })?;
        Ok::<_, ReadError>((native, pin))
    };
    let mut turns = causal::turns(
        &*read_view.lock()?,
        source,
        key,
        &harness,
        component,
        view,
        causal
            .keys()
            .copied()
            .chain(need_latest_turn.then_some(Position::END))
            .chain(home_activity.as_ref().map(|activity| activity.position)),
    )?;
    let latest_turn = turns.remove(&Position::END).flatten();
    let annotations = if let Target::Annotations(request) = target {
        Some(annotation_page(
            &*read_view.lock()?,
            source,
            key,
            &binding,
            request,
            &annotation_members,
            if request.kind == "errors" {
                PendingRuntime::current(&harness, runtime_status, latest_turn.as_ref())
            } else {
                PendingRuntime::default()
            },
        )?)
    } else {
        None
    };
    let mut home_detail = if let Some(turn) = latest_turn.as_ref().filter(|_| include_home_detail) {
        let (id, member) = home_members
            .iter()
            .find(|(_, member)| member.reference.path == turn.address.path)
            .ok_or(ReadError::Restart)?;
        let native = read_body(
            *id,
            member,
            turn.address.offset,
            prompt_images(read_view, *id, turn.address.offset, Some("user"))?.as_ref(),
            BodyMode::Complete,
        )?
        .0;
        Some(HomeDetail::from_turn(turn, &native))
    } else {
        None
    };
    let home_activity = if let Some(active) = home_activity
        .filter(|active| latest_turn.is_some() && turns.get(&active.position) == Some(&latest_turn))
    {
        let member = home_members
            .get(&active.position.source)
            .ok_or(ReadError::Restart)?;
        let offset = u64::try_from(active.position.offset).map_err(index)?;
        let (native, _) = read_body(
            active.position.source,
            member,
            offset,
            None,
            BodyMode::Complete,
        )?;
        let recipe = serde_json::json!({"kind":"activity","block":active.block,"tool":active.tool,"cwd":active.cwd});
        let text = crate::model::overview_text(&recipe, &native, None);
        Some(serde_json::json!([
            active.tool,
            text.as_str().unwrap_or_default(),
            0,
            active.since
        ]))
    } else {
        None
    };
    let mut records = Vec::with_capacity(candidates.len());
    let mut reply_records = BTreeMap::new();
    let mut home_origins = BTreeMap::new();
    let mut touched = BTreeMap::new();
    for (position, (kind, member, mut prefix)) in candidates {
        let offset = u64::try_from(position.offset).map_err(index)?;
        let inputs = causal.remove(&position);
        let images = prompt_images(read_view, position.source, offset, kind.as_deref())?;
        let mode = if inputs
            .as_ref()
            .is_some_and(causal::Inputs::has_background_finish)
        {
            BodyMode::Complete
        } else {
            BodyMode::transcript(kind.as_deref())
        };
        let (native, pin) = read_body(position.source, &member, offset, images.as_ref(), mode)?;
        prefix.provider = pin;
        touched.entry(position.source).or_insert(prefix);
        let reference = &member.reference;
        let native_id = crate::tx::native_event_id(&native);
        let turn = turns.remove(&position).flatten();
        let starts_handoff = turn.as_ref().is_some_and(|turn| {
            turn.handoff
                && turn.address.path == reference.path
                && turn.address.offset == offset
                && turn.address.block == position.block
                && turn.address.occurrence == position.occurrence
        });
        if starts_handoff && let Some(turn) = &turn {
            anchors.insert(position, turn.id.clone());
        }
        let operation = inputs.as_ref().and_then(causal::Inputs::operation);
        let entry_kind = match kind.as_deref() {
            _ if starts_handoff && turn.as_ref().is_some_and(|turn| !turn.head) => Some("h"),
            Some("event_msg") if operation.is_some() => Some("tool"),
            Some("user" | "user.message") => anchors.get(&position).map(|anchor| {
                if crate::model::is_ask(anchor) {
                    "h"
                } else {
                    "u"
                }
            }),
            Some("assistant" | "assistant.message")
                if crate::model::assistant_text(&native, 0).is_some() =>
            {
                Some("a")
            }
            Some("response_item") => match native
                .pointer("/payload/role")
                .and_then(serde_json::Value::as_str)
            {
                Some("user") if anchors.contains_key(&position) => Some("u"),
                Some("assistant")
                    if native
                        .pointer("/payload/type")
                        .and_then(serde_json::Value::as_str)
                        == Some("message") =>
                {
                    Some("a")
                }
                _ => None,
            },
            _ => None,
        };
        let entry_anchor = entry_kind.map(|kind| {
            crate::transcript_range::native_entry_id(
                source.namespace.as_str(),
                key,
                reference,
                (offset, 0),
                kind,
                native_id.as_deref(),
                None,
            )
        });
        let causal = inputs.map_or_else(causal::Context::default, |inputs| {
            inputs.render(source, key, reference, native_id.as_deref(), turn)
        });
        if let Some(turn) = home_event_origin(&target, &causal, reference, offset, &native)
            && let std::collections::btree_map::Entry::Vacant(entry) =
                home_origins.entry(turn.position)
        {
            let id = turn.position.source;
            let member = home_members.get(&id).ok_or(ReadError::Restart)?;
            let images = prompt_images(read_view, id, turn.address.offset, Some("user"))?;
            let (prompt, _) = read_body(
                id,
                member,
                turn.address.offset,
                images.as_ref(),
                BodyMode::Complete,
            )?;
            entry.insert(HomeDetail::from_turn(turn, &prompt).text);
        }
        if direction == Direction::Reverse
            && let Some(operation) = operation
            && let std::collections::btree_map::Entry::Vacant(entry) =
                reply_records.entry((reference.path.clone(), operation.parent))
        {
            let (body, _) = read_body(
                position.source,
                &member,
                operation.parent,
                None,
                BodyMode::Complete,
            )?;
            entry.insert(body);
        }
        if direction == Direction::Reverse || matches!(target, Target::Turns(_, Some(_))) {
            for reply_offset in causal
                .calls
                .iter()
                .filter(|call| direction == Direction::Reverse || call.tool == "AskUserQuestion")
                .flat_map(causal::CallSlot::display_offsets)
            {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    reply_records.entry((reference.path.clone(), reply_offset))
                {
                    let (body, _) = read_body(
                        position.source,
                        &member,
                        reply_offset,
                        None,
                        BodyMode::Transcript,
                    )?;
                    entry.insert(body);
                }
            }
        }
        for offset in causal.background_bodies() {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                reply_records.entry((reference.path.clone(), offset))
            {
                let (body, _) =
                    read_body(position.source, &member, offset, None, BodyMode::Complete)?;
                entry.insert(body);
            }
        }
        records.push(Record {
            source: position.source,
            prompt_anchor: anchors.remove(&position),
            entry_anchor,
            operation,
            causal,
            at: (position.time != i64::MIN).then_some(position.time),
            #[cfg(test)]
            relay_receipts: relay_context.remove(&position).unwrap_or_default(),
            address: Address {
                path: reference.path.clone(),
                offset,
                block: position.block,
                occurrence: position.occurrence,
            },
            reference: reference.clone(),
            kind,
            native,
            images: images.map(|images| images.refs),
        });
    }
    #[cfg(test)]
    AFTER_BODY.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook(*bytes_left);
        }
    });
    for reader in readers.values_mut() {
        reader.finish().map_err(body_error)?;
    }
    if !source.provider_only()
        && provider_pins
            .values()
            .any(|(carrier, observed, _)| match observed {
                Some(observed) => raw_stat(Path::new(carrier)).ok().as_ref() != Some(observed),
                None => !local_missing(carrier),
            })
    {
        return Err(ReadError::Restart);
    }
    drop(bytes_left);
    source.validate_authority()?;
    // An earlier page may use historical prefix receipts with current row facts.
    // The browser keeps the metadata from its latest or addressed selection.
    let include_metadata = cursor.is_none()
        && matches!(
            target,
            Target::Page(_, Direction::Reverse) | Target::Address(_, Some(_), None)
        );
    let launch_detail = home_detail.is_some()
        && latest_turn
            .as_ref()
            .is_some_and(|turn| turn.id == crate::model::spawn_turn_id(&harness, key));
    // Qualifying a page's blocks from a changed carrier does not make that
    // carrier eligible for the component's complete metadata projection.
    let metadata = (!needs_local_proof && (include_metadata || launch_detail))
        .then(|| {
            super::selected_metadata(
                read_view,
                source,
                key,
                owner,
                &owner_identity.1,
                &references,
                include_metadata && source.current_provider.is_none(),
                Some(latest_turn.as_ref()),
            )
        })
        .transpose()?;
    if launch_detail
        && let Some(detail) = &mut home_detail
        && let Some(row) = &metadata
        && let (Some(parent), Some(details)) = (&row.parent, &row.details)
    {
        detail.set_parent(parent, details);
    }
    let runtime_status = metadata.as_ref().map_or(runtime_status, |row| row.status);
    let (_, harness, native_id) = owner_identity;
    let identity = super::present_identity(
        source,
        scope,
        key,
        harness.clone(),
        references,
        identity_witness,
    );
    let next = after
        .filter(|_| more || matches!(target, Target::Address(_, Some(_), _)))
        .map(|after| Cursor {
            direction,
            selection,
            members,
            view,
            opened_at,
            after,
            touched: touched.into_values().collect(),
        });
    Ok(Some(Component {
        binding,
        #[cfg(test)]
        member_count: identity.source_refs.len(),
        identity,
        harness,
        native_id,
        title,
        model,
        metadata: metadata.filter(|_| include_metadata),
        retained_metadata: None,
        latest_turn,
        runtime_status,
        records,
        reply_records,
        fields,
        next,
        annotations,
        home_detail,
        home_origins,
        home_activity,
    }))
}

fn component_binding(selection: &str, members: &str) -> Result<String, ReadError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(encode(&(selection, members))?)
    ))
}

fn local_missing(carrier: &str) -> bool {
    raw_stat(Path::new(carrier)).is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
}

fn pin_provider(
    provider: &dyn SessionSourceReader,
    reference: &SessionSourceRef,
) -> Result<ProviderPin, ReadError> {
    pin_provider_with(provider, reference, body_error)
}

fn pin_provider_with(
    provider: &dyn SessionSourceReader,
    reference: &SessionSourceRef,
    map_error: impl FnOnce(io::Error) -> ReadError,
) -> Result<ProviderPin, ReadError> {
    let part = provider
        .read_range(reference, None, 0, 1)
        .map_err(map_error)?;
    if part.bytes.len() != 1 || part.length < reference.offset {
        return Err(ReadError::Restart);
    }
    Ok(ProviderPin {
        generation: Sha256::digest(part.generation.as_bytes()).into(),
        length: part.length,
    })
}

fn body_error(error: io::Error) -> ReadError {
    if error.kind() == io::ErrorKind::WouldBlock {
        ReadError::SourceBudget
    } else {
        ReadError::Restart
    }
}

#[cfg(test)]
type TestHook = std::cell::RefCell<Option<Box<dyn FnOnce(usize)>>>;

#[cfg(test)]
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
        if member.changed_local.map_or_else(
            || !stable_or_grown(&member.current.stat, &observed),
            |expected| expected != observed || observed.size < member.current.offset,
        ) {
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
        if self.member.changed_local.is_some() && current != io.observed
            || !stable_or_grown(&io.observed, &current)
        {
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
