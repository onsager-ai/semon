//! Durable native Original record descriptors for rebuilding private Query facts.
//! This is derived metadata, not a migration of native logs or Trace history.
//! Only the Event Index schema owner installs these tables. Source bytes remain
//! in the authorized Records/provider path and never enter these tables.

use super::*;
use crate::SessionSourceReader;
use crate::json_string::native_identity::{Compare, Comparison, Fingerprint, Receipt};
use crate::json_string::native_number::{
    Cursor as NumberCursor, Qualification as NumberQualification,
};
use crate::json_string::native_record::{
    Container, Cursor as RecordCursor, Literal, Location, Span, Token,
};

mod native_admission;
mod native_classes;
mod native_compare;
mod native_copilot;
mod native_events;
mod native_harness;
mod native_record_source;
mod native_retirement;
mod native_source;
mod native_tag;
mod native_time;
pub(crate) type IdentityStep = native_classes::IdentityStep;
pub(crate) type OriginalTextRepresentative = native_classes::Representative;
pub(super) use native_compare::SCHEMA as COMPARE_SCHEMA;
pub(crate) type ComparisonSource<'a, 'r> = native_compare::ComparisonSource<'a, 'r>;
pub(crate) type TextComparisonStep = native_compare::TextComparisonStep;
pub(crate) type BodyStep = native_copilot::BodyStep;
pub(crate) type AdmissionStep = native_admission::AdmissionStep;
pub(crate) use native_events::Text as OriginalText;
pub(crate) use native_harness::LiteralStep;
pub(crate) use native_source::OriginalEventFacts;
pub(crate) type TimestampStep = native_time::TimestampStep;
pub(crate) type TagStep = native_tag::TagStep;

pub(crate) const REPAIR_FORMAT: u32 = 3;
// The pre-format namespace was published with parser 24. Its key must stay
// frozen even when the complete native producer later advances CACHE_VERSION.
const PREVIOUS_REPAIR_PARSER: u32 = 24;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_original_classes (
 original BLOB NOT NULL, class INTEGER NOT NULL, record_offset INTEGER NOT NULL,
 ordinal INTEGER NOT NULL, digest BLOB NOT NULL, decoded_bytes INTEGER NOT NULL,
 PRIMARY KEY(original,class)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_class_candidates
 ON native_original_classes(original,digest,decoded_bytes,class DESC);
CREATE TABLE IF NOT EXISTS native_original_identities (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,ordinal)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_channel_state (
 original BLOB NOT NULL, channel TEXT NOT NULL, count INTEGER NOT NULL,
 PRIMARY KEY(original,channel)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_extras (
 original BLOB NOT NULL, sequence INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(original,sequence)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_signals (
 original BLOB NOT NULL, sequence INTEGER NOT NULL, kind TEXT NOT NULL,
 annotates INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(original,sequence)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_signal_kind
 ON native_original_signals(original,kind,sequence);
CREATE INDEX IF NOT EXISTS native_original_signal_observation
 ON native_original_signals(original,sequence) WHERE annotates=0;
CREATE TABLE IF NOT EXISTS native_original_positions (
 original BLOB NOT NULL, extra INTEGER NOT NULL, sequence INTEGER NOT NULL,
 offset BLOB NOT NULL, block INTEGER NOT NULL, time INTEGER,
 child INTEGER NOT NULL, parent INTEGER,
 PRIMARY KEY(original,extra,sequence)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_regular_time
 ON native_original_positions(original,sequence) WHERE extra=0 AND time IS NOT NULL;
CREATE INDEX IF NOT EXISTS native_original_regular_offset
 ON native_original_positions(original,offset,sequence) WHERE extra=0;
CREATE INDEX IF NOT EXISTS native_original_child_order
 ON native_original_positions(original,parent,offset DESC,block DESC,extra DESC,sequence)
 WHERE child=1;
CREATE TABLE IF NOT EXISTS native_original_literal (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 literal TEXT NOT NULL, revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,ordinal,literal)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_timestamp (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,ordinal)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_tag (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,ordinal)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_copilot_body (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,record_offset)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_copilot_admission (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,record_offset)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_header (
 original BLOB NOT NULL, field TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(original,field)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_copilot_started (
 original BLOB PRIMARY KEY
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_copilot_seen (
 original BLOB NOT NULL, identity INTEGER NOT NULL,
 PRIMARY KEY(original,identity)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_event_state (
 original BLOB PRIMARY KEY, event_count INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_events (
 original BLOB NOT NULL, sequence INTEGER NOT NULL, is_tool INTEGER NOT NULL,
 identity INTEGER, code_mode INTEGER NOT NULL, code_mode_open INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(original,sequence)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_event_identity
 ON native_original_events(original,identity,sequence) WHERE is_tool=1;
CREATE TABLE IF NOT EXISTS native_original_pending (
 original BLOB NOT NULL, identity INTEGER NOT NULL, sequence INTEGER NOT NULL,
 code_mode_open INTEGER NOT NULL, PRIMARY KEY(original,identity)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_pending_open
 ON native_original_pending(original,identity,sequence) WHERE code_mode_open=1;
CREATE TABLE IF NOT EXISTS native_original_tool_ids (
 original BLOB NOT NULL, identity INTEGER NOT NULL, sequence INTEGER NOT NULL,
 plain INTEGER NOT NULL, PRIMARY KEY(original,identity,sequence)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_plain_tool_ids
 ON native_original_tool_ids(original,identity,plain,sequence);
CREATE TABLE IF NOT EXISTS native_original_cleanup (
 original BLOB PRIMARY KEY, obsolete BLOB NOT NULL, table_index INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_retirements (
 original BLOB NOT NULL, obsolete BLOB NOT NULL, table_index INTEGER NOT NULL,
 PRIMARY KEY(original,obsolete)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_repairs (
 original BLOB PRIMARY KEY, parser INTEGER NOT NULL, source TEXT NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_tokens (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 metadata TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,ordinal)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_records (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, end_offset INTEGER NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('lexed','gap','folded')),
 PRIMARY KEY(original,record_offset)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_members (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, parent_offset INTEGER NOT NULL,
 name TEXT NOT NULL, revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,parent_offset,name)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_numbers (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, address INTEGER NOT NULL,
 start_offset INTEGER NOT NULL, end_offset INTEGER NOT NULL, cursor TEXT NOT NULL, receipt TEXT,
 PRIMARY KEY(original,record_offset,address)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_number_pending
 ON native_original_numbers(original,record_offset,address) WHERE receipt IS NULL;
CREATE TABLE IF NOT EXISTS native_original_nodes (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, address INTEGER NOT NULL,
 parent_offset INTEGER, key_start INTEGER, element_index INTEGER,
 metadata TEXT NOT NULL, end_offset INTEGER,
 PRIMARY KEY(original,record_offset,address)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_node_parent
 ON native_original_nodes(original,record_offset,parent_offset,address);
CREATE UNIQUE INDEX IF NOT EXISTS native_original_node_key
 ON native_original_nodes(original,record_offset,parent_offset,key_start) WHERE key_start IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS native_original_node_element
 ON native_original_nodes(original,record_offset,parent_offset,element_index) WHERE element_index IS NOT NULL;
CREATE TABLE IF NOT EXISTS native_original_fields (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 start_offset INTEGER NOT NULL, end_offset INTEGER NOT NULL, parent_offset INTEGER, is_key INTEGER NOT NULL, metadata TEXT NOT NULL,
 cursor TEXT NOT NULL, decoded_bytes INTEGER, digest BLOB,
 PRIMARY KEY(original,record_offset,ordinal)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_field_candidate
 ON native_original_fields(original,record_offset,parent_offset,is_key,digest,decoded_bytes,ordinal DESC);
CREATE INDEX IF NOT EXISTS native_original_field_pending
 ON native_original_fields(original,record_offset,ordinal) WHERE digest IS NULL;
";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    generation: String,
    length: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Lexing,
    Discarding,
    ClearGap,
    FoldRecord,
    Complete,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    source_at: u64,
    record_at: u64,
    ordinal: u64,
    record: RecordCursor,
    pin: Option<Pin>,
    phase: Phase,
    fields_after: Option<u64>,
    fields_registered: bool,
    node_stack: Vec<(u64, Container)>,
    native_invalid: bool,
}
impl Cursor {
    fn new() -> Self {
        Self {
            source_at: 0,
            record_at: 0,
            ordinal: 0,
            record: RecordCursor::new(),
            pin: None,
            phase: Phase::Lexing,
            fields_after: None,
            fields_registered: false,
            node_stack: Vec::new(),
            native_invalid: false,
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Repair {
    pub(crate) original: [u8; 32],
    pub(crate) source: crate::SessionSourceRef,
    pub(crate) revision: i64,
    cursor: Cursor,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct RepairWork {
    pub(crate) range_requests: usize,
    pub(crate) native_bytes: usize,
    pub(crate) metadata_rows: usize,
    pub(crate) metadata_bytes: usize,
    /// Actual statement VM work of obsolete-format key selection and deletion.
    /// Other producer phases retain their own instrumentation obligations.
    pub(crate) cleanup_vm_steps: i32,
    /// Peak owned primary-key payload during retirement, excluding SQLite's
    /// page cache and fixed transaction/statement storage.
    pub(crate) cleanup_key_bytes: usize,
}
#[derive(Debug)]
pub(crate) struct RepairStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
}

/// A qualified lexical field descriptor. A receipt only narrows indexed
/// candidates; exact native equality still requires Compare and Records fences.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OriginalField {
    pub(crate) ordinal: u64,
    pub(crate) span: Span,
    pub(crate) key: bool,
    pub(crate) location: Location,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OriginalNode {
    pub(crate) ordinal: u64,
    pub(crate) address: u64,
    pub(crate) location: Location,
    pub(crate) value: OriginalValue,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum OriginalValue {
    Object,
    Array,
    String(Span),
    Number(Span),
    Literal { span: Span, kind: Literal },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberCursor {
    before: Option<u64>,
    candidate: Option<OriginalField>,
    compare: Option<Compare>,
    complete: bool,
    key: Option<OriginalField>,
}
impl MemberCursor {
    fn new() -> Self {
        Self {
            before: None,
            candidate: None,
            compare: None,
            complete: false,
            key: None,
        }
    }
}
#[derive(Debug)]
pub(crate) struct MemberStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) complete: bool,
    pub(crate) node: Option<OriginalNode>,
}

fn reference(
    records: &crate::query_source::Records<'_>,
    witness: &SourceWitness,
) -> io::Result<crate::SessionSourceRef> {
    let reference = records
        .source_ref(&witness.source.path)
        .ok_or_else(|| io::Error::other("original native source authority unavailable"))?;
    let source = &witness.source;
    if reference.native_id != source.native_id
        || reference.offset != source.offset
        || reference.prefix_sha256 != source.prefix_sha256
        || reference.tail_sha256 != source.tail_sha256
    {
        return Err(io::Error::other("original native source proof changed"));
    }
    Ok(reference.clone())
}
fn namespace(source: &crate::SessionSourceRef) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(b"native-original-derived\0");
    digest.update(REPAIR_FORMAT.to_be_bytes());
    digest.update(super::super::super::CACHE_VERSION.to_be_bytes());
    let policy = crate::json_string::native_number::PRODUCER_POLICY.as_bytes();
    digest.update((policy.len() as u32).to_be_bytes());
    digest.update(policy);
    digest.update(serde_json::to_vec(source).map_err(io::Error::other)?);
    Ok(digest.finalize().into())
}
fn previous_namespace(source: &crate::SessionSourceRef) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    digest.update(b"native-original-derived-v1\0");
    digest.update(PREVIOUS_REPAIR_PARSER.to_be_bytes());
    digest.update(serde_json::to_vec(source).map_err(io::Error::other)?);
    Ok(digest.finalize().into())
}

fn format_two_namespace(source: &crate::SessionSourceRef) -> io::Result<[u8; 32]> {
    // Format 2 predated the numeric feature-policy binding. Freeze its parser
    // and key instead of attempting to deserialize its obsolete continuations.
    let mut digest = Sha256::new();
    digest.update(b"native-original-derived\0");
    digest.update(2_u32.to_be_bytes());
    digest.update(PREVIOUS_REPAIR_PARSER.to_be_bytes());
    digest.update(serde_json::to_vec(source).map_err(io::Error::other)?);
    Ok(digest.finalize().into())
}

fn repair_current(db: &Connection, repair: &Repair) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT revision=?2 AND parser=?3 FROM native_original_repairs WHERE original=?1",
        params![
            repair.original.as_slice(),
            repair.revision,
            super::super::super::CACHE_VERSION
        ],
        |row| row.get(0),
    )
    .optional()
    .map(|value| value == Some(true))
}
fn record_status(db: &Connection, repair: &Repair) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT status FROM native_original_records WHERE original=?1 AND record_offset=?2",
        params![
            repair.original.as_slice(),
            i64::try_from(repair.cursor.record_at).map_err(|_| rusqlite::Error::InvalidQuery)?
        ],
        |row| row.get(0),
    )
    .optional()
}
fn fields_ready(db: &Connection, repair: &Repair) -> rusqlite::Result<bool> {
    let status = record_status(db, repair)?;
    if status.as_deref() == Some("gap") {
        return Ok(true);
    }
    if status.as_deref() != Some("lexed")
        || !repair.cursor.fields_registered
        || repair.cursor.native_invalid
    {
        return Ok(false);
    }
    db.query_row("SELECT NOT EXISTS(SELECT 1 FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND digest IS NULL) AND NOT EXISTS(SELECT 1 FROM native_original_numbers WHERE original=?1 AND record_offset=?2 AND receipt IS NULL)",params![repair.original.as_slice(),i64::try_from(repair.cursor.record_at).map_err(|_|rusqlite::Error::InvalidQuery)?],|row|row.get(0))
}

fn check_record_view(
    db: &Connection,
    job: &Job,
    witness: &SourceWitness,
    repair: &Repair,
) -> rusqlite::Result<()> {
    if !job_current(db, job)?
        || !check_source(db, witness)?
        || !repair_current(db, repair)?
        || !admitted_original(db, job, witness)?
        || repair.cursor.phase != Phase::FoldRecord
        || !fields_ready(db, repair)?
        || record_status(db, repair)?.as_deref() != Some("lexed")
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

fn admitted_original(
    db: &Connection,
    job: &Job,
    witness: &SourceWitness,
) -> rusqlite::Result<bool> {
    let expected = json(&witness.source)?;
    db.query_row(
        "SELECT metadata=?3 FROM native_query_sources WHERE job=?1 AND path=?2",
        params![job.id, witness.source.path.to_string_lossy(), expected],
        |row| row.get(0),
    )
    .optional()
    .map(|value| value == Some(true))
}

impl NativeStore {
    /// One native Gap transition, followed by the same record cursor seal used
    /// by the complete record reducer. This never qualifies the publication:
    /// other retained records and source/owner completion still remain pending.
    pub(crate) fn original_gap_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<RepairStep> {
        let mut work = RepairWork::default();
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        if reference(records, witness)? != repair.source || repair.cursor.phase != Phase::FoldRecord
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        if record_status(&self.connection, repair)
            .map_err(io::Error::other)?
            .as_deref()
            != Some("gap")
        {
            return Err(io::Error::other(
                "native Gap reducer requires a qualified Gap record",
            ));
        }
        verify_member_anchor(records, repair, &mut work)?;
        records.finish()?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
            || record_status(&tx, repair)
                .map_err(io::Error::other)?
                .as_deref()
                != Some("gap")
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let mut events = native_events::Events::new(&tx, repair.original)?;
        crate::events::native_producer::push(
            &mut events,
            crate::events::native_event::NativeEvent::<native_events::Text> {
                k: crate::events::Kind::Gap,
                o: repair.cursor.record_at,
                ..Default::default()
            },
        )?;
        let event_work = events.work();
        work.metadata_rows += event_work.metadata_rows + 2;
        work.metadata_bytes += event_work.metadata_bytes;
        let cursor = next_record_cursor(repair);
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_bytes += state.len();
        if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
            return Err(io::Error::other("native Gap fold exceeds metadata work"));
        }
        if tx.execute("UPDATE native_original_records SET status='folded' WHERE original=?1 AND record_offset=?2 AND end_offset=?3 AND status='gap'",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(repair.cursor.source_at)?]).map_err(io::Error::other)?!=1 {
            return Ok(RepairStep { outcome:Outcome::Conflict,work });
        }
        tx.execute(
            "UPDATE native_original_repairs SET cursor=?2,revision=revision+1 WHERE original=?1",
            params![repair.original.as_slice(), state],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(RepairStep {
            outcome: Outcome::Written,
            work,
        })
    }

    /// Called by the bounded lexical worker before qualifying new-format facts.
    /// The fixed native schema owner retires only proven obsolete namespaces.
    pub(crate) fn original_cleanup_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<RepairStep> {
        native_retirement::advance(self, job, witness, repair, records, cancelled)
    }

    fn original_number_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<RepairStep> {
        let mut work = RepairWork::default();
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        if !repair.cursor.fields_registered {
            return Err(io::Error::other(
                "native number qualification lacks record nodes",
            ));
        }
        let mut numeric = None;
        let mut invalid = repair.cursor.native_invalid;
        if !invalid {
            let pending:Option<(i64,i64,i64,Option<String>,i64)>=self.connection.query_row("SELECT address,start_offset,end_offset,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_numbers WHERE original=?1 AND record_offset=?2 AND receipt IS NULL ORDER BY address LIMIT 1",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(io::Error::other)?;
            let Some((address, start, end, state, size)) = pending else {
                return Ok(RepairStep {
                    outcome: Outcome::Written,
                    work,
                });
            };
            work.metadata_rows = 1;
            work.metadata_bytes = usize::try_from(size).map_err(io::Error::other)?;
            let start = u64::try_from(start).map_err(io::Error::other)?;
            let end = u64::try_from(end).map_err(io::Error::other)?;
            let mut cursor: NumberCursor =
                serde_json::from_str(state.as_deref().ok_or_else(|| {
                    io::Error::other("native number cursor exceeds metadata budget")
                })?)
                .map_err(io::Error::other)?;
            if start > end || cursor.at < start || cursor.at > end || end > record_length(repair)? {
                return Err(io::Error::other(
                    "native number descriptor is outside its record",
                ));
            }
            let remaining = end - cursor.at;
            if remaining > 0 {
                let at = repair
                    .cursor
                    .record_at
                    .checked_add(cursor.at)
                    .ok_or_else(|| io::Error::other("native number offset overflow"))?;
                let maximum =
                    usize::try_from(remaining.min(crate::SESSION_SOURCE_CHUNK_MAX as u64))
                        .map_err(io::Error::other)?;
                let part = records.read_range(
                    &repair.source,
                    repair
                        .cursor
                        .pin
                        .as_ref()
                        .map(|pin| pin.generation.as_str()),
                    at,
                    maximum,
                )?;
                work.range_requests = 1;
                work.native_bytes = part.bytes.len();
                let pin = repair.cursor.pin.as_ref().ok_or_else(|| {
                    io::Error::other("native number lacks Original consumed-prefix pin")
                })?;
                if part.offset != at
                    || part.bytes.is_empty()
                    || part.bytes.len() > maximum
                    || part.generation != pin.generation
                    || part.length != pin.length
                {
                    return Err(io::Error::other("native number source generation changed"));
                }
                cursor.step(&part.bytes, crate::SESSION_SOURCE_CHUNK_MAX)?;
            } else {
                cursor.step(&[], 1)?;
            }
            invalid = cursor.result == Some(NumberQualification::Invalid);
            let text = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
            let receipt = cursor
                .result
                .map(|value| bounded_json(&value, STATE_BYTES))
                .transpose()
                .map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += text.len() + receipt.as_ref().map_or(0, String::len);
            numeric = Some((address, text, receipt));
        }
        if work.range_requests == 0 {
            verify_member_anchor(records, repair, &mut work)?
        }
        records.finish()?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        work.metadata_rows += 1 + usize::from(invalid);
        work.metadata_bytes += 64;
        if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
            return Err(io::Error::other(
                "native number step exceeds metadata budget",
            ));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        if let Some((address, cursor, receipt)) = numeric {
            tx.execute("UPDATE native_original_numbers SET cursor=?4,receipt=?5 WHERE original=?1 AND record_offset=?2 AND address=?3 AND receipt IS NULL",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,address,cursor,receipt]).map_err(io::Error::other)?;
        }
        if invalid {
            tx.execute("UPDATE native_original_records SET status='gap' WHERE original=?1 AND record_offset=?2 AND status='lexed'",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?]).map_err(io::Error::other)?;
        }
        tx.execute(
            "UPDATE native_original_repairs SET revision=revision+1 WHERE original=?1",
            params![repair.original.as_slice()],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(RepairStep {
            outcome: Outcome::Deferred,
            work,
        })
    }

    pub(crate) fn original_number(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        address: u64,
    ) -> io::Result<Option<crate::json_string::native_number::NativeNumber>> {
        self.original_number_with_size(job, witness, repair, address)
            .map(|(number, _)| number)
    }
    pub(super) fn original_number_with_size(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        address: u64,
    ) -> io::Result<(
        Option<crate::json_string::native_number::NativeNumber>,
        usize,
    )> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native number runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let receipt:Option<(Option<String>,Option<i64>)>=tx.query_row("SELECT CASE WHEN octet_length(receipt)<=65536 THEN receipt END,octet_length(receipt) FROM native_original_numbers WHERE original=?1 AND record_offset=?2 AND address=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(address)?],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(io::Error::other)?;
        let size = receipt
            .as_ref()
            .and_then(|r| r.1)
            .map(usize::try_from)
            .transpose()
            .map_err(io::Error::other)?
            .unwrap_or(0);
        let number =
            receipt
                .map(|receipt| {
                    match serde_json::from_str(receipt.0.as_deref().ok_or_else(|| {
                        io::Error::other("native number receipt is not qualified")
                    })?)
                    .map_err(io::Error::other)?
                    {
                        NumberQualification::Finite(number) => Ok(number),
                        NumberQualification::Invalid => {
                            Err(io::Error::other("native number belongs to a gap record"))
                        }
                    }
                })
                .transpose()?;
        Ok((number, size))
    }

    pub(crate) fn original_root(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
    ) -> io::Result<Option<OriginalNode>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("original root runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let address:Option<i64>=tx.query_row("SELECT address FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND parent_offset IS NULL ORDER BY address LIMIT 1",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|row|row.get(0)).optional().map_err(io::Error::other)?;
        drop(tx);
        address
            .map(|address| {
                self.original_node(
                    job,
                    witness,
                    repair,
                    u64::try_from(address).map_err(io::Error::other)?,
                )
            })
            .transpose()
            .map(Option::flatten)
    }

    /// One bounded native member selection step. name is an existing producer
    /// field literal, never arbitrary source text. Newest candidate keys must
    /// match complete canonical native bytes before they own a logical field;
    /// rejected digest collisions advance durably to the previous candidate.
    pub(crate) fn original_member_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        parent: u64,
        name: &'static str,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<MemberStep> {
        self.original_member_step_with_budget(
            job,
            witness,
            repair,
            parent,
            name,
            records,
            cancelled,
            crate::SESSION_SOURCE_CHUNK_MAX,
            ROWS,
        )
    }

    pub(crate) fn original_member_step_with_budget(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        parent: u64,
        name: &'static str,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
        native_allowance: usize,
        metadata_rows_allowance: usize,
    ) -> io::Result<MemberStep> {
        // This is a producer API literal bound, not a bound on supported keys.
        // Arbitrary native keys remain complete descriptor/hash candidates.
        if name.len() > 128
            || name
                .bytes()
                .any(|byte| byte < 0x20 || byte == b'"' || byte == b'\\' || byte >= 0x80)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native member name must be a bounded producer literal",
            ));
        }
        let mut work = RepairWork::default();
        if cancelled() || native_allowance == 0 || metadata_rows_allowance < 8 {
            return Ok(MemberStep {
                outcome: Outcome::Deferred,
                work,
                complete: false,
                node: None,
            });
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(MemberStep {
                outcome: Outcome::Conflict,
                work,
                complete: false,
                node: None,
            });
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let existing:Option<(i64,Option<String>,i64)>=self.connection.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_members WHERE original=?1 AND record_offset=?2 AND parent_offset=?3 AND name=?4",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(parent)?,name],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, mut cursor) = if let Some((revision, text, size)) = existing {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            (
                Some(revision),
                serde_json::from_str::<MemberCursor>(text.as_deref().ok_or_else(|| {
                    io::Error::other("native member cursor exceeds metadata budget")
                })?)
                .map_err(io::Error::other)?,
            )
        } else {
            (None, MemberCursor::new())
        };
        if cursor.complete {
            verify_member_anchor(records, repair, &mut work)?;
            records.finish()?;
            if !self.runtime_current(&job.spec)? {
                return Ok(MemberStep {
                    outcome: Outcome::Conflict,
                    work,
                    complete: false,
                    node: None,
                });
            }
            check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
            let node = cursor
                .key
                .as_ref()
                .map(|key| self.original_value_by_key(job, witness, repair, key))
                .transpose()?
                .flatten();
            if cursor.key.is_some() {
                work.metadata_rows += 2;
                work.metadata_bytes += 8 + node
                    .as_ref()
                    .map(json)
                    .transpose()
                    .map_err(io::Error::other)?
                    .map_or(0, |s| s.len());
            }
            if cursor.key.is_some() && node.is_none() {
                return Err(io::Error::other(
                    "native member value descriptor is missing",
                ));
            }
            return Ok(MemberStep {
                outcome: Outcome::Written,
                work,
                complete: true,
                node,
            });
        }
        if cursor.candidate.is_none() {
            let receipt = Receipt {
                decoded_bytes: name.len() as u64,
                sha256: Sha256::digest(name.as_bytes()).into(),
            };
            let candidate:Option<(Option<String>,i64)>=self.connection.query_row("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND parent_offset=?3 AND is_key=1 AND digest=?4 AND decoded_bytes=?5 AND ordinal<?6 ORDER BY ordinal DESC LIMIT 1",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(parent)?,receipt.sha256.as_slice(),seq_u64(receipt.decoded_bytes)?,cursor.before.map(seq_u64).transpose()?.unwrap_or(i64::MAX)],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(io::Error::other)?;
            work.metadata_rows += 1;
            if let Some((text, size)) = candidate {
                work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
                let field: OriginalField =
                    serde_json::from_str(text.as_deref().ok_or_else(|| {
                        io::Error::other("native member candidate exceeds metadata budget")
                    })?)
                    .map_err(io::Error::other)?;
                if !field.key
                    || field.location.parent != Some(parent)
                    || field.span.end > record_length(repair)?
                {
                    return Err(io::Error::other(
                        "native member candidate descriptor disagrees",
                    ));
                }
                cursor.compare = Some(Compare::new(
                    (field.span.start, field.span.end),
                    (0, name.len() as u64),
                )?);
                cursor.candidate = Some(field);
            } else {
                cursor.complete = true
            }
        }
        if let Some(compare) = cursor.compare.as_mut() {
            let remaining = compare.left.clone();
            let field = cursor
                .candidate
                .as_ref()
                .ok_or_else(|| io::Error::other("native member comparison lacks a candidate"))?;
            let left = field
                .span
                .end
                .checked_sub(remaining.at)
                .ok_or_else(|| io::Error::other("native member comparison exceeds field"))?;
            let mut native = None;
            if left > 0 {
                if native_allowance < 12 {
                    return Ok(MemberStep {
                        outcome: Outcome::Deferred,
                        work,
                        complete: false,
                        node: None,
                    });
                }
                let at = repair
                    .cursor
                    .record_at
                    .checked_add(remaining.at)
                    .ok_or_else(|| io::Error::other("native member source offset overflow"))?;
                let maximum = usize::try_from(
                    left.min(native_allowance.min(crate::SESSION_SOURCE_CHUNK_MAX) as u64),
                )
                .map_err(io::Error::other)?;
                let part = records.read_range(
                    &repair.source,
                    repair
                        .cursor
                        .pin
                        .as_ref()
                        .map(|pin| pin.generation.as_str()),
                    at,
                    maximum,
                )?;
                work.range_requests += 1;
                work.native_bytes += part.bytes.len();
                let pin = repair.cursor.pin.as_ref().ok_or_else(|| {
                    io::Error::other("native member lacks Original consumed-prefix pin")
                })?;
                if part.offset != at
                    || part.bytes.is_empty()
                    || part.bytes.len() > maximum
                    || part.generation != pin.generation
                    || part.length != pin.length
                {
                    return Err(io::Error::other("native member source generation changed"));
                }
                native = Some(part);
            }
            let right = usize::try_from(compare.right.at).map_err(io::Error::other)?;
            compare.step(
                native.as_ref().map_or(&[][..], |part| part.bytes.as_ref()),
                name.as_bytes()
                    .get(right..)
                    .ok_or_else(|| io::Error::other("native member literal cursor changed"))?,
                1024,
            )?;
            if let Some(result) = compare.result {
                cursor.before = Some(field.ordinal);
                if result == Comparison::Equal {
                    cursor.key = Some(field.clone());
                    cursor.complete = true
                }
                cursor.candidate = None;
                cursor.compare = None;
            }
        }
        if work.range_requests == 0 {
            verify_member_anchor(records, repair, &mut work)?
        }
        records.finish()?;
        if cancelled() {
            return Ok(MemberStep {
                outcome: Outcome::Deferred,
                work,
                complete: false,
                node: None,
            });
        }
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += state.len();
        if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
            return Err(io::Error::other(
                "native member step exceeds metadata budget",
            ));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(MemberStep {
                outcome: Outcome::Conflict,
                work,
                complete: false,
                node: None,
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let current:Option<i64>=tx.query_row("SELECT revision FROM native_original_members WHERE original=?1 AND record_offset=?2 AND parent_offset=?3 AND name=?4",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(parent)?,name],|row|row.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        if current != revision {
            return Ok(MemberStep {
                outcome: Outcome::Conflict,
                work,
                complete: false,
                node: None,
            });
        }
        tx.execute("INSERT INTO native_original_members VALUES(?1,?2,?3,?4,0,?5) ON CONFLICT(original,record_offset,parent_offset,name) DO UPDATE SET revision=revision+1,cursor=excluded.cursor",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(parent)?,name,state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(MemberStep {
                outcome: Outcome::Deferred,
                work,
                complete: false,
                node: None,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        let node = cursor
            .key
            .as_ref()
            .map(|key| self.original_value_by_key(job, witness, repair, key))
            .transpose()?
            .flatten();
        if cursor.key.is_some() {
            work.metadata_rows += 2;
            work.metadata_bytes += 8 + node
                .as_ref()
                .map(json)
                .transpose()
                .map_err(io::Error::other)?
                .map_or(0, |s| s.len());
        }
        if work.metadata_rows > metadata_rows_allowance
            || work.native_bytes > native_allowance.min(crate::SESSION_SOURCE_CHUNK_MAX)
            || work.metadata_bytes > BYTES
        {
            return Err(io::Error::other("native member exceeded reserved work"));
        }
        if cursor.key.is_some() && node.is_none() {
            return Err(io::Error::other(
                "native member value descriptor is missing",
            ));
        }
        Ok(MemberStep {
            outcome: if cursor.complete {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            complete: cursor.complete,
            node,
        })
    }

    /// Point access by physical field address. Selecting a logical object key
    /// first requires the native last-key exact-comparison protocol; this API
    /// does not convert candidate hashes into semantic ownership.
    pub(crate) fn original_node(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        address: u64,
    ) -> io::Result<Option<OriginalNode>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("original node runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let value:Option<(Option<String>,Option<i64>)>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END,end_offset FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND address=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(address)?],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(io::Error::other)?;
        let length = record_length(repair)?;
        value
            .map(|(text, end)| {
                let node: OriginalNode =
                    serde_json::from_str(text.as_deref().ok_or_else(|| {
                        io::Error::other("original node exceeds metadata budget")
                    })?)
                    .map_err(io::Error::other)?;
                if node.address != address
                    || end.is_none()
                    || end.is_some_and(|end| end < 0 || end as u64 > length)
                {
                    return Err(io::Error::other(
                        "original node is not a complete descriptor",
                    ));
                }
                Ok(node)
            })
            .transpose()
    }

    /// A value selected by one already-compared physical key descriptor.
    pub(crate) fn original_value_by_key(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        key: &OriginalField,
    ) -> io::Result<Option<OriginalNode>> {
        if !key.key {
            return Err(io::Error::other(
                "original value key is not a key descriptor",
            ));
        }
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("original node runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let address:Option<i64>=tx.query_row("SELECT address FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND parent_offset IS ?3 AND key_start=?4",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,key.location.parent.map(seq_u64).transpose()?,seq_u64(key.span.start)?],|row|row.get(0)).optional().map_err(io::Error::other)?;
        drop(tx);
        address
            .map(|address| {
                self.original_node(
                    job,
                    witness,
                    repair,
                    u64::try_from(address).map_err(io::Error::other)?,
                )
            })
            .transpose()
            .map(Option::flatten)
    }

    /// Indexed array traversal; this never collects a complete native array.
    pub(crate) fn original_array_page(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        parent: u64,
        after: Option<u64>,
    ) -> io::Result<Page<OriginalNode, u64>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("original array runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let mut query=tx.prepare("SELECT element_index,octet_length(metadata),CASE WHEN octet_length(metadata)<=65536 THEN metadata END,end_offset FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND parent_offset=?3 AND element_index IS NOT NULL AND element_index>?4 ORDER BY element_index LIMIT 64").map_err(io::Error::other)?;
        let mut values = query
            .query(params![
                repair.original.as_slice(),
                seq_u64(repair.cursor.record_at)?,
                seq_u64(parent)?,
                after.map(seq_u64).transpose()?.unwrap_or(-1)
            ])
            .map_err(io::Error::other)?;
        let length = record_length(repair)?;
        let mut rows = Vec::new();
        let mut cursor = after;
        let mut work = Work::default();
        let mut budget_stopped = false;
        while let Some(row) = values.next().map_err(io::Error::other)? {
            let index = u64::try_from(row.get::<_, i64>(0).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            let bytes = usize::try_from(row.get::<_, i64>(1).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > STATE_BYTES {
                return Err(io::Error::other(
                    "original array node exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                budget_stopped = true;
                break;
            }
            let node: OriginalNode =
                serde_json::from_str(&row.get::<_, String>(2).map_err(io::Error::other)?)
                    .map_err(io::Error::other)?;
            let end: Option<i64> = row.get(3).map_err(io::Error::other)?;
            if node.location.parent != Some(parent)
                || node.location.index != Some(index)
                || end.is_none()
                || end.is_some_and(|end| end < 0 || end as u64 > length)
            {
                return Err(io::Error::other("original array node descriptor disagrees"));
            }
            rows.push(node);
            cursor = Some(index);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(values);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        let complete = !budget_stopped && rows.len() < ROWS;
        Ok(Page {
            rows,
            after: cursor,
            complete,
            work,
        })
    }

    /// Candidate key descriptors, newest first to preserve the complete native
    /// Value's duplicate-key ownership. Receipt matches require exact decoded
    /// Compare before any key or identifier is selected. Both cursor and rows
    /// remain private until the native harness producer qualifies the record.
    pub(crate) fn original_field_candidates(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        parent: Option<u64>,
        key: bool,
        receipt: Receipt,
        before: Option<u64>,
    ) -> io::Result<Page<OriginalField, u64>> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("original field runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
            || repair.cursor.phase != Phase::FoldRecord
            || !fields_ready(&tx, repair).map_err(io::Error::other)?
            || record_status(&tx, repair)
                .map_err(io::Error::other)?
                .as_deref()
                != Some("lexed")
        {
            return Err(io::Error::other("original field generation changed"));
        }
        let mut query=tx.prepare("SELECT ordinal,octet_length(metadata),CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND parent_offset IS ?3 AND is_key=?4 AND digest=?5 AND decoded_bytes=?6 AND ordinal<?7 ORDER BY ordinal DESC LIMIT 64").map_err(io::Error::other)?;
        let mut values = query
            .query(params![
                repair.original.as_slice(),
                seq_u64(repair.cursor.record_at)?,
                parent.map(seq_u64).transpose()?,
                key,
                receipt.sha256.as_slice(),
                seq_u64(receipt.decoded_bytes)?,
                before.map(seq_u64).transpose()?.unwrap_or(i64::MAX)
            ])
            .map_err(io::Error::other)?;
        let mut rows = Vec::new();
        let mut after = before;
        let mut work = Work::default();
        let mut budget_stopped = false;
        while let Some(row) = values.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(1).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > STATE_BYTES {
                return Err(io::Error::other(
                    "original field descriptor exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                budget_stopped = true;
                break;
            }
            let ordinal = u64::try_from(row.get::<_, i64>(0).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            let field: OriginalField =
                serde_json::from_str(&row.get::<_, String>(2).map_err(io::Error::other)?)
                    .map_err(io::Error::other)?;
            if field.ordinal != ordinal || field.key != key || field.location.parent != parent {
                return Err(io::Error::other(
                    "original field indexed descriptor disagrees",
                ));
            }
            rows.push(field);
            after = Some(ordinal);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(values);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        // A full page is deliberately provisional; one empty bounded seek
        // seals its end without a whole-record COUNT or OFFSET.
        let complete = !budget_stopped && rows.len() < ROWS;
        Ok(Page {
            rows,
            after,
            complete,
            work,
        })
    }

    /// Register at most 31 descriptor tokens or advance one native identity.
    /// Every arbitrary-length string/key stays in retained native evidence;
    /// the derived row holds only a descriptor and content-free continuation.
    /// A caller reopens/reloads the repair after each committed step.
    pub(crate) fn original_fields_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<RepairStep> {
        let mut work = RepairWork::default();
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        if reference(records, witness)? != repair.source
            || !self.runtime_current(&job.spec)?
            || !job_current(&self.connection, job).map_err(io::Error::other)?
            || !check_source(&self.connection, witness).map_err(io::Error::other)?
            || !repair_current(&self.connection, repair).map_err(io::Error::other)?
            || !admitted_original(&self.connection, job, witness).map_err(io::Error::other)?
            || repair.cursor.phase != Phase::FoldRecord
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        if record_status(&self.connection, repair)
            .map_err(io::Error::other)?
            .as_deref()
            == Some("gap")
        {
            return Ok(RepairStep {
                outcome: Outcome::Written,
                work,
            });
        }
        if repair.cursor.fields_registered && repair.cursor.native_invalid {
            return self.original_number_step(job, witness, repair, records, cancelled);
        }
        if !repair.cursor.fields_registered {
            let page = self.original_tokens_page_limit(
                job,
                witness,
                repair,
                repair.cursor.fields_after,
                (ROWS - 1) / 3,
            )?;
            work.metadata_rows = page.work.decoded_rows;
            work.metadata_bytes = page.work.metadata_bytes;
            let mut fields = Vec::new();
            let mut nodes = Vec::new();
            let mut closes = Vec::new();
            let mut numbers = Vec::new();
            let mut cursor = repair.cursor.clone();
            for (ordinal, token) in page.rows {
                let node = match token {
                    Token::Begin { at, kind, location } => {
                        cursor.node_stack.push((at, kind));
                        if cursor.node_stack.len() >= 128
                            || (location.parent.is_none() && kind != Container::Object)
                        {
                            cursor.native_invalid = true
                        }
                        Some(OriginalNode {
                            ordinal,
                            address: at,
                            location,
                            value: match kind {
                                Container::Object => OriginalValue::Object,
                                Container::Array => OriginalValue::Array,
                            },
                        })
                    }
                    Token::End { at, kind } => {
                        let (address, opened) = cursor.node_stack.pop().ok_or_else(|| {
                            io::Error::other("original descriptor container close has no owner")
                        })?;
                        if opened != kind {
                            return Err(io::Error::other(
                                "original descriptor container kind changed",
                            ));
                        }
                        work.metadata_rows += 1;
                        work.metadata_bytes += 64;
                        closes.push((address, at));
                        None
                    }
                    Token::String {
                        span,
                        location,
                        key: false,
                    } => Some(OriginalNode {
                        ordinal,
                        address: span.start,
                        location,
                        value: OriginalValue::String(span),
                    }),
                    Token::Number { span, location } => Some(OriginalNode {
                        ordinal,
                        address: span.start,
                        location,
                        value: OriginalValue::Number(span),
                    }),
                    Token::Literal {
                        span,
                        location,
                        kind,
                    } => Some(OriginalNode {
                        ordinal,
                        address: span.start,
                        location,
                        value: OriginalValue::Literal { span, kind },
                    }),
                    _ => None,
                };
                if let Some(node) = node {
                    if node.location.parent.is_none()
                        && !matches!(node.value, OriginalValue::Object)
                    {
                        cursor.native_invalid = true
                    }
                    if let OriginalValue::Number(span) = &node.value {
                        let number = NumberCursor::new(span.start, span.end)?;
                        let state = bounded_json(&number, STATE_BYTES).map_err(io::Error::other)?;
                        work.metadata_rows += 1;
                        work.metadata_bytes += state.len() + 64;
                        numbers.push((node.address, *span, state));
                    }
                    let text = bounded_json(&node, STATE_BYTES).map_err(io::Error::other)?;
                    work.metadata_rows += 1;
                    work.metadata_bytes += text.len() + 64;
                    nodes.push((node, text));
                }
                if let Token::String {
                    span,
                    location,
                    key,
                } = token
                {
                    let field = OriginalField {
                        ordinal,
                        span,
                        key,
                        location,
                    };
                    let fingerprint = Fingerprint::new(span.start, span.end)?;
                    let text = bounded_json(&field, STATE_BYTES).map_err(io::Error::other)?;
                    let state =
                        bounded_json(&fingerprint, STATE_BYTES).map_err(io::Error::other)?;
                    work.metadata_rows += 1;
                    work.metadata_bytes += text.len() + state.len() + 64;
                    fields.push((field, text, state));
                }
            }
            if page.complete && !cursor.node_stack.is_empty() {
                return Err(io::Error::other(
                    "original descriptor record has open containers",
                ));
            }
            cursor.fields_after = page.after;
            cursor.fields_registered = page.complete;
            let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += state.len();
            if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
                return Err(io::Error::other(
                    "original field registration exceeds metadata budget",
                ));
            }
            if cancelled() {
                return Ok(RepairStep {
                    outcome: Outcome::Deferred,
                    work,
                });
            }
            if !self.runtime_current(&job.spec)? {
                return Ok(RepairStep {
                    outcome: Outcome::Conflict,
                    work,
                });
            }
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(io::Error::other)?;
            if !job_current(&tx, job).map_err(io::Error::other)?
                || !check_source(&tx, witness).map_err(io::Error::other)?
                || !repair_current(&tx, repair).map_err(io::Error::other)?
                || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
            {
                return Ok(RepairStep {
                    outcome: Outcome::Conflict,
                    work,
                });
            }
            for (node, text) in nodes {
                let end = match &node.value {
                    OriginalValue::Object | OriginalValue::Array => None,
                    OriginalValue::String(span) => {
                        Some(span.end.checked_add(1).ok_or_else(|| {
                            io::Error::other("original native string offset overflow")
                        })?)
                    }
                    OriginalValue::Number(span) | OriginalValue::Literal { span, .. } => {
                        Some(span.end)
                    }
                };
                tx.execute(
                    "INSERT INTO native_original_nodes VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        repair.original.as_slice(),
                        seq_u64(repair.cursor.record_at)?,
                        seq_u64(node.address)?,
                        node.location.parent.map(seq_u64).transpose()?,
                        node.location
                            .key
                            .map(|span| seq_u64(span.start))
                            .transpose()?,
                        node.location.index.map(seq_u64).transpose()?,
                        text,
                        end.map(seq_u64).transpose()?
                    ],
                )
                .map_err(io::Error::other)?;
            }
            for (address, end) in closes {
                let changed=tx.execute("UPDATE native_original_nodes SET end_offset=?4 WHERE original=?1 AND record_offset=?2 AND address=?3 AND end_offset IS NULL",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(address)?,seq_u64(end)?]).map_err(io::Error::other)?;
                if changed != 1 {
                    return Err(io::Error::other(
                        "original descriptor container close disagrees",
                    ));
                }
            }
            for (address, span, state) in numbers {
                tx.execute(
                    "INSERT INTO native_original_numbers VALUES(?1,?2,?3,?4,?5,?6,NULL)",
                    params![
                        repair.original.as_slice(),
                        seq_u64(repair.cursor.record_at)?,
                        seq_u64(address)?,
                        seq_u64(span.start)?,
                        seq_u64(span.end)?,
                        state
                    ],
                )
                .map_err(io::Error::other)?;
            }
            for (field, text, state) in fields {
                tx.execute("INSERT INTO native_original_fields VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,NULL)",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(field.ordinal)?,seq_u64(field.span.start)?,seq_u64(field.span.end)?,field.location.parent.map(seq_u64).transpose()?,field.key,text,state]).map_err(io::Error::other)?;
            }
            tx.execute("UPDATE native_original_repairs SET cursor=?2,revision=revision+1 WHERE original=?1",params![repair.original.as_slice(),state]).map_err(io::Error::other)?;
            if cancelled() {
                return Ok(RepairStep {
                    outcome: Outcome::Deferred,
                    work,
                });
            }
            tx.commit().map_err(io::Error::other)?;
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        let pending:Option<(i64,i64,i64,Option<String>,i64)>=self.connection.query_row("SELECT ordinal,start_offset,end_offset,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND digest IS NULL ORDER BY ordinal LIMIT 1",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(io::Error::other)?;
        let Some((ordinal, start, end, state, size)) = pending else {
            return self.original_number_step(job, witness, repair, records, cancelled);
        };
        work.metadata_rows = 1;
        work.metadata_bytes = usize::try_from(size).map_err(io::Error::other)?;
        let mut fingerprint: Fingerprint =
            serde_json::from_str(state.as_deref().ok_or_else(|| {
                io::Error::other("original field cursor exceeds metadata budget")
            })?)
            .map_err(io::Error::other)?;
        let start = u64::try_from(start).map_err(io::Error::other)?;
        let end = u64::try_from(end).map_err(io::Error::other)?;
        if start > end
            || fingerprint.stream.at < start
            || fingerprint.stream.at > end
            || end > record_length(repair)?
        {
            return Err(io::Error::other(
                "original field descriptor is outside its qualified record",
            ));
        }
        let remaining = end - fingerprint.stream.at;
        if remaining > 0 {
            let at = repair
                .cursor
                .record_at
                .checked_add(fingerprint.stream.at)
                .ok_or_else(|| io::Error::other("original field offset overflow"))?;
            let maximum = usize::try_from(remaining.min(crate::SESSION_SOURCE_CHUNK_MAX as u64))
                .map_err(io::Error::other)?;
            let part = records.read_range(
                &repair.source,
                repair
                    .cursor
                    .pin
                    .as_ref()
                    .map(|pin| pin.generation.as_str()),
                at,
                maximum,
            )?;
            work.range_requests = 1;
            work.native_bytes = part.bytes.len();
            let pin = repair
                .cursor
                .pin
                .as_ref()
                .ok_or_else(|| io::Error::other("original record lacks consumed-prefix pin"))?;
            if part.offset != at
                || part.bytes.is_empty()
                || part.bytes.len() > maximum
                || part.generation != pin.generation
                || part.length != pin.length
            {
                return Err(io::Error::other("original field native generation changed"));
            }
            fingerprint.step(&part.bytes, 64 * 1024)?;
        } else {
            fingerprint.step(&[], 64)?;
        }
        records.finish()?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        let state = bounded_json(&fingerprint, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_rows += 2;
        work.metadata_bytes += state.len() + 64;
        if work.metadata_bytes > BYTES {
            return Err(io::Error::other("original field metadata budget exceeded"));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let receipt = fingerprint.receipt();
        tx.execute("UPDATE native_original_fields SET cursor=?4,decoded_bytes=?5,digest=?6 WHERE original=?1 AND record_offset=?2 AND ordinal=?3 AND digest IS NULL",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,ordinal,state,receipt.map(|receipt|seq_u64(receipt.decoded_bytes)).transpose()?,receipt.map(|receipt|receipt.sha256.to_vec())]).map_err(io::Error::other)?;
        tx.execute(
            "UPDATE native_original_repairs SET revision=revision+1 WHERE original=?1",
            params![repair.original.as_slice()],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(RepairStep {
            outcome: Outcome::Deferred,
            work,
        })
    }

    /// The caller has admitted Original plus separate Current observation and
    /// all native publication/relationship owners. Records owns configured-root
    /// authority and independently qualified consumed-prefix byte proofs.
    pub(crate) fn begin_original_repair(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Option<Repair>> {
        let source = reference(records, witness)?;
        let original = namespace(&source)?;
        if cancelled() || !self.runtime_current(&job.spec)? {
            return Ok(None);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
        {
            return Ok(None);
        }
        let text = json(&source).map_err(io::Error::other)?;
        native_retirement::enqueue(&tx, original, &source).map_err(io::Error::other)?;
        let cursor = bounded_json(&Cursor::new(), STATE_BYTES).map_err(io::Error::other)?;
        tx.execute(
            "INSERT OR IGNORE INTO native_original_repairs VALUES(?1,?2,?3,0,?4)",
            params![
                original.as_slice(),
                super::super::super::CACHE_VERSION,
                text,
                cursor
            ],
        )
        .map_err(io::Error::other)?;
        let actual: (i64, Option<String>, bool) = tx.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,source=?2 AND parser=?3 FROM native_original_repairs WHERE original=?1",params![original.as_slice(),text,super::super::super::CACHE_VERSION],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(io::Error::other)?;
        if !actual.2 {
            return Err(io::Error::other(
                "original derived source identity disagrees",
            ));
        }
        let cursor: Cursor =
            serde_json::from_str(actual.1.as_deref().ok_or_else(|| {
                io::Error::other("original repair cursor exceeds metadata budget")
            })?)
            .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(None);
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(Some(Repair {
            original,
            source,
            revision: actual.0,
            cursor,
        }))
    }

    pub(crate) fn reload_original_repair(
        &self,
        job: &Job,
        witness: &SourceWitness,
        records: &crate::query_source::Records<'_>,
    ) -> io::Result<Option<Repair>> {
        let source = reference(records, witness)?;
        let original = namespace(&source)?;
        if !self.runtime_current(&job.spec)? {
            return Ok(None);
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
        {
            return Ok(None);
        }
        let value: Option<(i64,Option<String>,bool)> = tx.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,source=?2 AND parser=?3 FROM native_original_repairs WHERE original=?1",params![original.as_slice(),json(&source).map_err(io::Error::other)?,super::super::super::CACHE_VERSION],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(io::Error::other)?;
        value
            .map(|(revision, cursor, same)| {
                if !same {
                    return Err(io::Error::other(
                        "original derived source identity disagrees",
                    ));
                }
                let cursor = serde_json::from_str(cursor.as_deref().ok_or_else(|| {
                    io::Error::other("original repair cursor exceeds metadata budget")
                })?)
                .map_err(io::Error::other)?;
                Ok(Repair {
                    original,
                    source,
                    revision,
                    cursor,
                })
            })
            .transpose()
    }

    /// The shared native harness reducer reads only one qualified record's
    /// descriptor page. It never receives a complete source Index or Value.
    pub(crate) fn original_tokens_page(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        after: Option<u64>,
    ) -> io::Result<Page<(u64, Token), u64>> {
        self.original_tokens_page_limit(job, witness, repair, after, ROWS)
    }

    fn original_tokens_page_limit(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        after: Option<u64>,
        limit: usize,
    ) -> io::Result<Page<(u64, Token), u64>> {
        assert!(limit > 0 && limit <= ROWS);
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("original native producer runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
            || repair.cursor.phase != Phase::FoldRecord
        {
            return Err(io::Error::other("original descriptor generation changed"));
        }
        let ready:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_original_records WHERE original=?1 AND record_offset=?2 AND end_offset=?3 AND status IN('lexed','gap'))",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(repair.cursor.source_at)?],|row|row.get(0)).map_err(io::Error::other)?;
        if !ready {
            return Err(io::Error::other("original record descriptors incomplete"));
        }
        let mut q=tx.prepare("SELECT ordinal,octet_length(metadata),CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM native_original_tokens WHERE original=?1 AND record_offset=?2 AND ordinal>?3 ORDER BY ordinal LIMIT ?4").map_err(io::Error::other)?;
        let mut values = q
            .query(params![
                repair.original.as_slice(),
                seq_u64(repair.cursor.record_at)?,
                after.map(seq_u64).transpose()?.unwrap_or(-1),
                limit as i64
            ])
            .map_err(io::Error::other)?;
        let mut rows = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(value) = values.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(value.get::<_, i64>(1).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "original descriptor exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes + STATE_BYTES + 64 > BYTES {
                break;
            }
            let ordinal = u64::try_from(value.get::<_, i64>(0).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            let text: String = value.get(2).map_err(io::Error::other)?;
            rows.push((
                ordinal,
                serde_json::from_str(&text).map_err(io::Error::other)?,
            ));
            end = Some(ordinal);
            work.metadata_bytes += bytes;
            work.decoded_rows += 1;
        }
        drop(values);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        let complete = end.map_or(repair.cursor.ordinal == 0, |ordinal| {
            ordinal + 1 == repair.cursor.ordinal
        });
        Ok(Page {
            rows,
            after: end,
            complete,
            work,
        })
    }

    /// This seal is a point validation of the existing native producer's
    /// durable record completion, not a callback that prepares history. Native
    /// facts already committed in bounded steps must agree with the record and
    /// Original identity before the next record is admitted.
    pub(crate) fn finish_original_record(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        completed: &dyn Fn(&Connection, [u8; 32], u64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Outcome> {
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(Outcome::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
            || repair.cursor.phase != Phase::FoldRecord
            || !fields_ready(&tx, repair).map_err(io::Error::other)?
            || !completed(&tx, repair.original, repair.cursor.record_at)
                .map_err(io::Error::other)?
        {
            return Ok(Outcome::Conflict);
        }
        let changed=tx.execute("UPDATE native_original_records SET status='folded' WHERE original=?1 AND record_offset=?2 AND end_offset=?3 AND status IN('lexed','gap')",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(repair.cursor.source_at)?]).map_err(io::Error::other)?;
        if changed != 1 {
            return Ok(Outcome::Conflict);
        }
        let cursor = next_record_cursor(repair);
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        tx.execute(
            "UPDATE native_original_repairs SET cursor=?2,revision=revision+1 WHERE original=?1",
            params![repair.original.as_slice(), state],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Outcome::Deferred);
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(Outcome::Written)
    }

    /// One attempt requests at most one authorized 128KiB Records range and
    /// commits at most 64 metadata rows. Records independently bounds and verifies
    /// its underlying proof/provider fragments; those actual calls belong to
    /// its existing counters, not range_requests. Use request-owned Records for
    /// each attempt so final proof rechecks cover bounded touched context.
    /// Completed records wait for the shared native producer; partial records
    /// have no qualified output and no complete Index/transcript preparation.
    pub(crate) fn original_lex_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<RepairStep> {
        let mut work = RepairWork::default();
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        if reference(records, witness)? != repair.source
            || !self.runtime_current(&job.spec)?
            || !job_current(&self.connection, job).map_err(io::Error::other)?
            || !check_source(&self.connection, witness).map_err(io::Error::other)?
            || !repair_current(&self.connection, repair).map_err(io::Error::other)?
            || !admitted_original(&self.connection, job, witness).map_err(io::Error::other)?
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let obsolete_pending = native_retirement::pending(&self.connection, repair.original)
            .map_err(io::Error::other)?;
        if obsolete_pending {
            return self.original_cleanup_step(job, witness, repair, records, cancelled);
        }
        if matches!(repair.cursor.phase, Phase::FoldRecord | Phase::Complete) {
            return Ok(RepairStep {
                outcome: Outcome::Written,
                work,
            });
        }
        if repair.cursor.phase == Phase::ClearGap {
            return self.original_clear_gap(job, witness, repair, cancelled);
        }
        let mut cursor = repair.cursor.clone();
        let left = repair
            .source
            .offset
            .checked_sub(cursor.source_at)
            .ok_or_else(|| io::Error::other("original repair cursor exceeds consumed prefix"))?;
        let mut tokens = Vec::new();
        let mut seal = None;
        if left == 0 {
            // A consumed prefix ends at a complete native newline. Any remaining
            // open record indicates stale/corrupt derived cursor state.
            if cursor.record_at != cursor.source_at {
                return Err(io::Error::other(
                    "original repair ended inside a native record",
                ));
            }
            cursor.phase = Phase::Complete;
        } else {
            let maximum = usize::try_from(left.min(crate::SESSION_SOURCE_CHUNK_MAX as u64))
                .map_err(io::Error::other)?;
            let part = records.read_range(
                &repair.source,
                cursor.pin.as_ref().map(|pin| pin.generation.as_str()),
                cursor.source_at,
                maximum,
            )?;
            work.range_requests = 1;
            work.native_bytes = part.bytes.len();
            if part.offset != cursor.source_at
                || part.bytes.is_empty()
                || part.bytes.len() > maximum
                || part.generation.is_empty()
                || part.generation.len() > 4096
                || part.length < repair.source.offset
                || part
                    .offset
                    .checked_add(part.bytes.len() as u64)
                    .is_none_or(|end| end > part.length)
                || cursor.pin.as_ref().is_some_and(|pin| {
                    pin.generation != part.generation || pin.length != part.length
                })
            {
                return Err(io::Error::other("original native range generation changed"));
            }
            cursor.pin.get_or_insert(Pin {
                generation: part.generation,
                length: part.length,
            });
            if cancelled() {
                return Ok(RepairStep {
                    outcome: Outcome::Deferred,
                    work,
                });
            }
            let newline = part.bytes.iter().position(|byte| *byte == b'\n');
            let end = newline.map_or(part.bytes.len(), |at| at + 1);
            let bytes = &part.bytes[..end];
            if cursor.phase == Phase::Discarding {
                cursor.source_at = cursor
                    .source_at
                    .checked_add(end as u64)
                    .ok_or_else(|| io::Error::other("original repair offset overflow"))?;
                if newline.is_some() {
                    cursor.phase = Phase::ClearGap;
                }
            } else {
                match cursor.record.step(
                    bytes,
                    newline.is_some(),
                    BYTES,
                    ROWS - 2,
                    crate::json_string::JSON_STRING_CHECKPOINT_BYTES as usize,
                    |token| {
                        tokens.push(token);
                        Ok(())
                    },
                ) {
                    Ok(step) => {
                        cursor.source_at = cursor
                            .source_at
                            .checked_add(step.consumed as u64)
                            .ok_or_else(|| io::Error::other("original repair offset overflow"))?;
                        if newline.is_some() && step.consumed == end && step.complete {
                            cursor.phase = Phase::FoldRecord;
                            seal = Some("lexed");
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::InvalidData | io::ErrorKind::Unsupported
                        ) =>
                    {
                        // Keep malformed native gap behavior. Stage no tokens
                        // from the failed step, then clean earlier partial rows
                        // through bounded keyset deletion before publishing Gap.
                        tokens.clear();
                        cursor.source_at = repair
                            .cursor
                            .source_at
                            .checked_add(end as u64)
                            .ok_or_else(|| io::Error::other("original repair offset overflow"))?;
                        cursor.phase = if newline.is_some() {
                            Phase::ClearGap
                        } else {
                            Phase::Discarding
                        };
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        records.finish()?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        let texts: Vec<_> = tokens
            .iter()
            .map(|token| bounded_json(token, BYTES).map_err(io::Error::other))
            .collect::<io::Result<_>>()?;
        let first_ordinal = cursor.ordinal;
        cursor.ordinal = cursor
            .ordinal
            .checked_add(texts.len() as u64)
            .ok_or_else(|| io::Error::other("original descriptor ordinal overflow"))?;
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_rows = 1 + texts.len() + usize::from(seal.is_some());
        work.metadata_bytes = state.len()
            + texts.iter().map(|text| text.len() + 64).sum::<usize>()
            + usize::from(seal.is_some()) * 64;
        if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
            return Err(io::Error::other(
                "original repair step exceeds metadata budget",
            ));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let mut put = tx
            .prepare_cached("INSERT INTO native_original_tokens VALUES(?1,?2,?3,?4)")
            .map_err(io::Error::other)?;
        for (index, text) in texts.into_iter().enumerate() {
            put.execute(params![
                repair.original.as_slice(),
                seq_u64(cursor.record_at)?,
                seq_u64(first_ordinal + index as u64)?,
                text
            ])
            .map_err(io::Error::other)?;
        }
        drop(put);
        if let Some(status) = seal {
            tx.execute(
                "INSERT INTO native_original_records VALUES(?1,?2,?3,?4)",
                params![
                    repair.original.as_slice(),
                    seq_u64(cursor.record_at)?,
                    seq_u64(cursor.source_at)?,
                    status
                ],
            )
            .map_err(io::Error::other)?;
        }
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        tx.execute(
            "UPDATE native_original_repairs SET cursor=?2,revision=revision+1 WHERE original=?1",
            params![repair.original.as_slice(), state],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(RepairStep {
            outcome: Outcome::Deferred,
            work,
        })
    }

    fn original_clear_gap(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<RepairStep> {
        let mut work = RepairWork::default();
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
        {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        let mut query=tx.prepare("SELECT ordinal,octet_length(metadata) FROM native_original_tokens WHERE original=?1 AND record_offset=?2 ORDER BY ordinal LIMIT 62").map_err(io::Error::other)?;
        let mut rows = query
            .query(params![
                repair.original.as_slice(),
                seq_u64(repair.cursor.record_at)?
            ])
            .map_err(io::Error::other)?;
        let mut ordinals = Vec::new();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(1).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if work.metadata_bytes + bytes + STATE_BYTES + 64 > BYTES {
                break;
            }
            work.metadata_bytes += bytes;
            ordinals.push(row.get::<_, i64>(0).map_err(io::Error::other)?);
        }
        drop(rows);
        drop(query);
        for ordinal in &ordinals {
            tx.execute("DELETE FROM native_original_tokens WHERE original=?1 AND record_offset=?2 AND ordinal=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,ordinal]).map_err(io::Error::other)?;
        }
        work.metadata_rows = 1 + ordinals.len();
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_original_tokens WHERE original=?1 AND record_offset=?2)",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|row|row.get(0)).map_err(io::Error::other)?;
        let mut cursor = repair.cursor.clone();
        if !remains {
            cursor.phase = Phase::FoldRecord;
            cursor.ordinal = 0;
            tx.execute(
                "INSERT INTO native_original_records VALUES(?1,?2,?3,'gap')",
                params![
                    repair.original.as_slice(),
                    seq_u64(cursor.record_at)?,
                    seq_u64(cursor.source_at)?
                ],
            )
            .map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += 64;
        }
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_bytes += state.len();
        if work.metadata_bytes > BYTES {
            return Err(io::Error::other(
                "original repair cleanup exceeds metadata budget",
            ));
        }
        tx.execute(
            "UPDATE native_original_repairs SET cursor=?2,revision=revision+1 WHERE original=?1",
            params![repair.original.as_slice(), state],
        )
        .map_err(io::Error::other)?;
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(RepairStep {
            outcome: Outcome::Deferred,
            work,
        })
    }
}
fn verify_member_anchor(
    records: &crate::query_source::Records<'_>,
    repair: &Repair,
    work: &mut RepairWork,
) -> io::Result<()> {
    let pin = repair
        .cursor
        .pin
        .as_ref()
        .ok_or_else(|| io::Error::other("native member lacks Original consumed-prefix pin"))?;
    let part = records.read_range(
        &repair.source,
        Some(&pin.generation),
        repair.cursor.record_at,
        1,
    )?;
    work.range_requests += 1;
    work.native_bytes += part.bytes.len();
    if part.offset != repair.cursor.record_at
        || part.bytes.len() != 1
        || part.generation != pin.generation
        || part.length != pin.length
    {
        return Err(io::Error::other("native member source authority changed"));
    }
    Ok(())
}

fn record_length(repair: &Repair) -> io::Result<u64> {
    repair
        .cursor
        .source_at
        .checked_sub(repair.cursor.record_at)
        .ok_or_else(|| io::Error::other("original descriptor record offsets disagree"))
}

fn seq_u64(value: u64) -> io::Result<i64> {
    i64::try_from(value).map_err(io::Error::other)
}

fn next_record_cursor(repair: &Repair) -> Cursor {
    let mut cursor = repair.cursor.clone();
    cursor.record_at = cursor.source_at;
    cursor.ordinal = 0;
    cursor.record = RecordCursor::new();
    cursor.fields_after = None;
    cursor.fields_registered = false;
    cursor.node_stack.clear();
    cursor.native_invalid = false;
    cursor.phase = if cursor.source_at == repair.source.offset {
        Phase::Complete
    } else {
        Phase::Lexing
    };
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Arc;

    struct Fixture {
        root: PathBuf,
        options: crate::Options,
        store: NativeStore,
        source: SourceWitness,
        sources: crate::query_source::Sources,
        job: Job,
    }
    impl Fixture {
        fn qualified_record(&mut self) -> Repair {
            let mut repair = self
                .store
                .begin_original_repair(&self.job, &self.source, &self.sources.reader(), &|| false)
                .unwrap()
                .unwrap();
            while repair.cursor.phase != Phase::FoldRecord {
                self.store
                    .original_lex_step(
                        &self.job,
                        &self.source,
                        &repair,
                        &self.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                self.store = NativeStore::open(&self.options).unwrap();
                repair = self
                    .store
                    .reload_original_repair(&self.job, &self.source, &self.sources.reader())
                    .unwrap()
                    .unwrap();
            }
            loop {
                let step = self
                    .store
                    .original_fields_step(
                        &self.job,
                        &self.source,
                        &repair,
                        &self.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                self.store = NativeStore::open(&self.options).unwrap();
                repair = self
                    .store
                    .reload_original_repair(&self.job, &self.source, &self.sources.reader())
                    .unwrap()
                    .unwrap();
                if step.outcome == Outcome::Written {
                    return repair;
                }
            }
        }
        fn member(&mut self, repair: &Repair, name: &'static str) -> OriginalNode {
            let root = self
                .store
                .original_root(&self.job, &self.source, repair)
                .unwrap()
                .unwrap();
            loop {
                let step = self
                    .store
                    .original_member_step(
                        &self.job,
                        &self.source,
                        repair,
                        root.address,
                        name,
                        &self.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                if step.complete {
                    return step.node.unwrap();
                }
                self.store = NativeStore::open(&self.options).unwrap();
            }
        }
        fn identity(&mut self, repair: &Repair, ordinal: u64) -> OriginalText {
            loop {
                let step = self.identity_once(repair, ordinal, &|| false);
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.range_requests <= 2);
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                assert!(step.class_sql_vm_steps <= 4096);
                assert!(step.serialized_cursor_bytes <= STATE_BYTES);
                if let Some(text) = step.text {
                    return text;
                }
            }
        }
        fn identity_once(
            &mut self,
            repair: &Repair,
            ordinal: u64,
            cancelled: &dyn Fn() -> bool,
        ) -> IdentityStep {
            let step = self
                .store
                .original_identity_step(
                    &self.job,
                    &self.source,
                    repair,
                    ordinal,
                    &self.sources.reader(),
                    cancelled,
                )
                .unwrap();
            assert!(step.work.metadata_rows <= ROWS);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.work.range_requests <= 2);
            assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(step.class_sql_vm_steps <= 4096);
            assert!(step.serialized_cursor_bytes <= STATE_BYTES);
            assert!(step.fence_checks <= 16);
            assert!(step.records_finish_calls <= 1);
            self.store = NativeStore::open(&self.options).unwrap();
            step
        }
        fn identity_with_provider(
            &mut self,
            repair: &Repair,
            ordinal: u64,
            provider: &dyn crate::SessionSourceReader,
        ) -> io::Result<IdentityStep> {
            let result = {
                let records = self
                    .sources
                    .reader_with(Some((provider, crate::SessionSourceReadMode::ProviderOnly)));
                self.store.original_identity_step(
                    &self.job,
                    &self.source,
                    repair,
                    ordinal,
                    &records,
                    &|| false,
                )
            };
            if let Ok(step) = &result {
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.range_requests <= 2);
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                assert!(step.class_sql_vm_steps <= 4096);
                assert!(step.serialized_cursor_bytes <= STATE_BYTES);
                assert!(step.fence_checks <= 16);
                assert!(step.records_finish_calls <= 1);
            }
            self.store = NativeStore::open(&self.options).unwrap();
            result
        }
        fn new(bytes: &[u8]) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-original-repair-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let options = crate::Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                cache: root.join("cache"),
                ..crate::Options::default()
            };
            let path = options.claude_home.join("projects/p/chosen.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            let capture = crate::source_chunks::capture(
                &mut crate::sealed::LogFile::open(&path).unwrap(),
                bytes.len() as u64,
                0,
            )
            .unwrap();
            let ledger = Ledger {
                stat: Stat::of(&path).unwrap(),
                offset: bytes.len() as u64,
                prefix: capture.prefix,
                tail: capture.tail,
            };
            let chunks = Arc::new(capture.chunks);
            let original = ledger.source(&path, "chosen");
            let sources = crate::query_source::Sources::capture(
                &options,
                std::iter::once((original.clone(), Some(chunks.clone()))),
            )
            .unwrap();
            let mut index = FileIndex {
                source_chunks: Some(chunks),
                ..FileIndex::default()
            };
            let mut offset = 0;
            for line in bytes.split_inclusive(|byte| *byte == b'\n') {
                if let Ok(value) = serde_json::from_slice::<Value>(line)
                    && value.is_object()
                {
                    super::super::super::super::claude(&mut index, &value, offset);
                } else {
                    super::super::super::super::gap(&mut index, offset);
                }
                offset += line.len() as u64;
            }
            let row:CatalogRow=serde_json::from_value(serde_json::json!({"key":"chosen","name":"chosen","harness":"claude","kind":"session","native_ids":["chosen"],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":null,"effort":null,"start":null,"last":null,"tokens":[0.0,0.0,0.0],"cost":crate::pricing::Cost::default(),"sources":[original]})).unwrap();
            let (mut writer, _) = SqliteStore::attempt_scoped(
                &super::super::super::super::EventCache::path(&options.cache),
                None,
                true,
            )
            .unwrap_or_else(|_| panic!("native repair fixture store must open"));
            assert_eq!(
                writer
                    .write_one(path.to_str().unwrap(), None, None, &ledger, &index)
                    .unwrap(),
                Outcome::Written
            );
            // Production registration belongs to the Event Index owner. This
            // isolated fixture installs only this derived schema in that store.
            writer.connection.execute_batch(SCHEMA).unwrap();
            writer.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,0,?2,?3)",params![row.key,row.harness,json(&row).unwrap()]).unwrap();
            let mut store = NativeStore {
                connection: writer.connection,
                options: options.clone(),
            };
            let source = store.source_witness(&row.sources[0]).unwrap();
            let spec = JobSpec {
                view_key: "original-repair:chosen".into(),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&options).unwrap(),
            };
            let job = store.begin(&spec).unwrap();
            let owner = store.owner_witness(&row).unwrap();
            assert_eq!(
                store
                    .admit(
                        &job,
                        std::slice::from_ref(&source),
                        &[owner],
                        "original-admitted"
                    )
                    .unwrap(),
                Outcome::Written
            );
            let job = store.reload(&spec.view_key).unwrap().unwrap();
            Self {
                root,
                options,
                store,
                source,
                sources,
                job,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct InterleavingReader {
        source: crate::SessionSourceRef,
        bytes: std::sync::Arc<[u8]>,
        calls: std::sync::atomic::AtomicUsize,
        bytes_read: std::sync::atomic::AtomicUsize,
        change_generation_after: Option<usize>,
        on_read: Box<dyn Fn(usize) + Send + Sync>,
    }
    impl InterleavingReader {
        fn new(
            source: crate::SessionSourceRef,
            bytes: &[u8],
            change_generation_after: Option<usize>,
            on_read: impl Fn(usize) + Send + Sync + 'static,
        ) -> Self {
            Self {
                source,
                bytes: std::sync::Arc::from(bytes),
                calls: std::sync::atomic::AtomicUsize::new(0),
                bytes_read: std::sync::atomic::AtomicUsize::new(0),
                change_generation_after,
                on_read: Box::new(on_read),
            }
        }
    }
    impl crate::SessionSourceReader for InterleavingReader {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            _expected_generation: Option<&str>,
            offset: u64,
            max_bytes: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            if source != &self.source {
                return Err(io::Error::other("test source reference changed"));
            }
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let start = usize::try_from(offset).map_err(io::Error::other)?;
            if start >= self.bytes.len() || max_bytes == 0 {
                return Err(io::Error::other("test provider range is outside source"));
            }
            let end = start.saturating_add(max_bytes).min(self.bytes.len());
            let bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(&self.bytes[start..end]);
            (self.on_read)(call);
            self.bytes_read
                .fetch_add(bytes.len(), std::sync::atomic::Ordering::SeqCst);
            let generation = if self
                .change_generation_after
                .is_some_and(|first| call > first)
            {
                "test-generation-2"
            } else {
                "test-generation-1"
            };
            Ok(crate::SessionSourceRange {
                generation: generation.to_owned(),
                length: self.bytes.len() as u64,
                offset,
                bytes,
                cached: false,
            })
        }
    }

    fn identity_cursor(fixture: &Fixture, repair: &Repair, ordinal: u64) -> Option<(i64, Value)> {
        let row: Option<(i64, String)> = fixture
            .store
            .connection
            .query_row(
                "SELECT revision,cursor FROM native_original_identities WHERE original=?1 AND record_offset=?2 AND ordinal=?3",
                params![
                    repair.original.as_slice(),
                    repair.cursor.record_at as i64,
                    ordinal as i64
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap();
        row.map(|(revision, state)| (revision, serde_json::from_str(&state).unwrap()))
    }

    fn force_class_fingerprint(fixture: &Fixture, repair: &Repair, class: u64, ordinal: u64) {
        let (digest, decoded_bytes): (Vec<u8>, i64) = fixture
            .store
            .connection
            .query_row(
                "SELECT digest,decoded_bytes FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND ordinal=?3",
                params![
                    repair.original.as_slice(),
                    repair.cursor.record_at as i64,
                    ordinal as i64
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_original_classes SET digest=?3,decoded_bytes=?4 WHERE original=?1 AND class=?2",
                params![
                    repair.original.as_slice(),
                    class as i64,
                    digest,
                    decoded_bytes
                ],
            )
            .unwrap();
    }

    #[test]
    fn exact_original_classes_match_independent_native_strings_and_refuse_hash_collisions() {
        for size in [0, 128, 1024, 8192, 100_000] {
            let prefix = "α".repeat(size + 1);
            let first = format!("{prefix}a");
            let different = format!("{prefix}b");
            let encoded = serde_json::to_string(&first).unwrap();
            let escaped = encoded.replace('α', "\\u03b1");
            let bytes = format!(
                "{{\"a\":{encoded},\"b\":{escaped},\"c\":{}}}\n",
                serde_json::to_string(&different).unwrap()
            );
            let oracle: Value = serde_json::from_str(&bytes).unwrap();
            assert!(oracle["a"] == oracle["b"]);
            assert!(oracle["a"] != oracle["c"]);
            let mut fixture = Fixture::new(bytes.as_bytes());
            let reads = super::super::super::super::FILE_READS.with(Cell::get);
            let repair = fixture.qualified_record();
            let a = fixture.member(&repair, "a");
            let b = fixture.member(&repair, "b");
            let c = fixture.member(&repair, "c");
            let calls = Cell::new(0);
            let cancelled = || {
                calls.set(calls.get() + 1);
                calls.get() == 3
            };
            let cancelled_step = fixture
                .store
                .original_identity_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    a.ordinal,
                    &fixture.sources.reader(),
                    &cancelled,
                )
                .unwrap();
            assert_eq!(cancelled_step.outcome, Outcome::Deferred);
            assert!(cancelled_step.text.is_none());
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0
            );
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            let first = fixture.identity(&repair, a.ordinal);
            let second = fixture.identity(&repair, b.ordinal);
            assert_eq!(first, second);
            assert_eq!(first.original(), repair.original);
            // Force the indexed candidate filter to report a collision. Only
            // complete retained byte comparison may decide semantic equality.
            let (sha256,length):(Vec<u8>,i64)=fixture.store.connection.query_row("SELECT digest,decoded_bytes FROM native_original_fields WHERE original=?1 AND record_offset=0 AND ordinal=?2",params![repair.original.as_slice(),c.ordinal as i64],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
            fixture.store.connection.execute("UPDATE native_original_classes SET digest=?3,decoded_bytes=?4 WHERE original=?1 AND class=?2",params![repair.original.as_slice(),first.identity() as i64,sha256,length]).unwrap();
            let third = fixture.identity(&repair, c.ordinal);
            assert_ne!(first, third);
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                2
            );
            let cached = fixture
                .store
                .original_identity_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    b.ordinal,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert_eq!(cached.text, Some(second));
            assert_eq!(cached.work.range_requests, 1);
            assert_eq!(cached.work.native_bytes, 1);
            assert!(serde_json::to_vec(&third).unwrap().len() < 256);
            assert_eq!(
                super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
        }
    }

    #[test]
    fn native_identity_rechecks_newer_classes_after_a_durable_collision_rejection() {
        let bytes = b"{\"a\":\"wrong\",\"b\":\"equal\",\"c\":\"equal\"}\n";
        let oracle: Value = serde_json::from_slice(bytes).unwrap();
        assert_ne!(oracle["a"], oracle["b"]);
        assert_eq!(oracle["b"], oracle["c"]);

        let mut fixture = Fixture::new(bytes);
        let repair = fixture.qualified_record();
        let a = fixture.member(&repair, "a");
        let b = fixture.member(&repair, "b");
        let c = fixture.member(&repair, "c");
        let class1 = fixture.identity(&repair, a.ordinal);
        assert_eq!(class1.identity(), 1);
        force_class_fingerprint(&fixture, &repair, class1.identity(), b.ordinal);

        let rejected = fixture.identity_once(&repair, b.ordinal, &|| false);
        assert_eq!(rejected.outcome, Outcome::Deferred);
        assert!(rejected.text.is_none());
        let (_, state) = identity_cursor(&fixture, &repair, b.ordinal).unwrap();
        assert_eq!(state["before"], 1);
        assert!(state["candidate"].is_null());
        assert!(state["compare"].is_null());
        assert_eq!(state["complete"], false);

        // This allocation runs through the production interner. Class 1 has
        // the same forced fingerprint, but its native bytes are unequal.
        let class2 = fixture.identity(&repair, c.ordinal);
        assert_eq!(class2.identity(), 2);
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let resumed = fixture.identity(&repair, b.ordinal);
        assert_eq!(resumed, class2);
        assert_eq!(resumed.identity(), 2);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
    }

    #[test]
    fn native_identity_ranges_preserve_long_compare_and_coalesce_later_insertions() {
        let prefix = "q".repeat(40_000);
        let a_value = format!("{prefix}a");
        let b_value = format!("{prefix}b");
        let c_value = format!("{prefix}c");
        let encoded_a = serde_json::to_string(&a_value).unwrap();
        let encoded_b = serde_json::to_string(&b_value).unwrap();
        let encoded_c = serde_json::to_string(&c_value).unwrap();
        let bytes = format!(
            "{{\"a\":{encoded_a},\"b\":{encoded_b},\"c\":{encoded_c},\"d\":{encoded_b}}}\n"
        );
        let oracle: Value = serde_json::from_slice(bytes.as_bytes()).unwrap();
        assert_ne!(oracle["a"], oracle["b"]);
        assert_ne!(oracle["c"], oracle["b"]);
        assert_eq!(oracle["b"], oracle["d"]);

        let mut fixture = Fixture::new(bytes.as_bytes());
        let repair = fixture.qualified_record();
        let a = fixture.member(&repair, "a");
        let b = fixture.member(&repair, "b");
        let c = fixture.member(&repair, "c");
        let d = fixture.member(&repair, "d");
        let reads = super::super::super::super::FILE_READS.with(Cell::get);

        let class1 = fixture.identity(&repair, a.ordinal);
        assert_eq!(class1.identity(), 1);
        force_class_fingerprint(&fixture, &repair, class1.identity(), b.ordinal);

        // B has an incomplete content comparison persisted before class 2 is
        // allocated by C.
        let first_b = fixture.identity_once(&repair, b.ordinal, &|| false);
        assert_eq!(first_b.outcome, Outcome::Deferred);
        let (_, state) = identity_cursor(&fixture, &repair, b.ordinal).unwrap();
        assert_eq!(state["candidate"]["class"], 1);
        assert!(state["compare"]["compared"].as_u64().unwrap() > 0);
        assert!(state["compare"]["result"].is_null());
        assert!(first_b.serialized_cursor_bytes < 4096);

        let class2 = fixture.identity(&repair, c.ordinal);
        assert_eq!(class2.identity(), 2);
        force_class_fingerprint(&fixture, &repair, class2.identity(), b.ordinal);

        // B finishes rejecting class 1, then starts the newer [1,2] range.
        let rejected1 = fixture.identity_once(&repair, b.ordinal, &|| false);
        assert_eq!(rejected1.outcome, Outcome::Deferred);
        let (_, state) = identity_cursor(&fixture, &repair, b.ordinal).unwrap();
        assert_eq!(state["before"], 1);
        assert_eq!(state["range"]["after"], 1);
        assert_eq!(state["range"]["through"], 2);

        // Pause while B is comparing class 2. D exact-compares classes 2 and
        // 1, then allocates equal class 3. B must queue (2,3] and visit it.
        let comparing2 = fixture.identity_once(&repair, b.ordinal, &|| false);
        assert_eq!(comparing2.outcome, Outcome::Deferred);
        let (_, state) = identity_cursor(&fixture, &repair, b.ordinal).unwrap();
        assert_eq!(state["candidate"]["class"], 2);
        assert!(state["compare"]["compared"].as_u64().unwrap() > 0);
        let class3 = fixture.identity(&repair, d.ordinal);
        assert_eq!(class3.identity(), 3);

        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let resumed = fixture.identity(&repair, b.ordinal);
        assert_eq!(resumed, class3);
        assert_eq!(resumed.identity(), 3);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            3
        );
        assert_eq!(
            super::super::super::super::FILE_READS.with(Cell::get),
            reads
        );
    }

    #[test]
    fn native_identity_old_incomplete_cursor_restarts_with_defaulted_search_coverage() {
        let bytes = b"{\"a\":\"wrong\",\"b\":\"equal\",\"c\":\"equal\"}\n";
        let mut fixture = Fixture::new(bytes);
        let repair = fixture.qualified_record();
        let a = fixture.member(&repair, "a");
        let b = fixture.member(&repair, "b");
        let c = fixture.member(&repair, "c");
        let class1 = fixture.identity(&repair, a.ordinal);
        force_class_fingerprint(&fixture, &repair, class1.identity(), b.ordinal);
        let rejected = fixture.identity_once(&repair, b.ordinal, &|| false);
        assert!(rejected.text.is_none());

        let (revision, mut legacy) = identity_cursor(&fixture, &repair, b.ordinal).unwrap();
        for field in ["high_water", "range", "match_class"] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_original_identities SET cursor=?4 WHERE original=?1 AND record_offset=?2 AND ordinal=?3",
                params![
                    repair.original.as_slice(),
                    repair.cursor.record_at as i64,
                    b.ordinal as i64,
                    legacy.to_string()
                ],
            )
            .unwrap();

        let class2 = fixture.identity(&repair, c.ordinal);
        assert_eq!(class2.identity(), 2);
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let resumed = fixture.identity(&repair, b.ordinal);
        assert_eq!(resumed, class2);
        assert_eq!(resumed.identity(), 2);
        assert_eq!(
            identity_cursor(&fixture, &repair, b.ordinal).unwrap().0,
            revision + 1
        );
    }

    #[test]
    fn native_identity_old_completed_cursor_remains_explicitly_pending() {
        let bytes = b"{\"a\":\"same\"}\n";
        let mut fixture = Fixture::new(bytes);
        let repair = fixture.qualified_record();
        let a = fixture.member(&repair, "a");
        let class = fixture.identity(&repair, a.ordinal);
        assert_eq!(class.identity(), 1);

        let (revision, mut legacy) = identity_cursor(&fixture, &repair, a.ordinal).unwrap();
        assert_eq!(legacy["complete"], true);
        for field in ["high_water", "range", "match_class"] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_original_identities SET cursor=?4 WHERE original=?1 AND record_offset=?2 AND ordinal=?3",
                params![
                    repair.original.as_slice(),
                    repair.cursor.record_at as i64,
                    a.ordinal as i64,
                    legacy.to_string()
                ],
            )
            .unwrap();

        let pending = fixture.identity_once(&repair, a.ordinal, &|| false);
        assert_eq!(pending.outcome, Outcome::Deferred);
        assert!(pending.text.is_none());
        assert_eq!(pending.records_finish_calls, 1);
        assert_eq!(
            identity_cursor(&fixture, &repair, a.ordinal).unwrap().0,
            revision
        );
    }

    #[test]
    fn native_identity_cancellation_before_and_after_cursor_commit_reopens_safely() {
        let prefix = "r".repeat(40_000);
        let bytes = format!(
            "{{\"a\":{},\"b\":{}}}\n",
            serde_json::to_string(&format!("{prefix}a")).unwrap(),
            serde_json::to_string(&format!("{prefix}b")).unwrap()
        );
        let mut fixture = Fixture::new(bytes.as_bytes());
        let repair = fixture.qualified_record();
        let a = fixture.member(&repair, "a");
        let b = fixture.member(&repair, "b");
        let class1 = fixture.identity(&repair, a.ordinal);
        force_class_fingerprint(&fixture, &repair, class1.identity(), b.ordinal);

        let calls = Cell::new(0);
        let before_commit = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        let cancelled = fixture.identity_once(&repair, b.ordinal, &before_commit);
        assert_eq!(cancelled.outcome, Outcome::Deferred);
        assert!(identity_cursor(&fixture, &repair, b.ordinal).is_none());

        // A non-cancelled attempt commits an incomplete Compare. Cancellation
        // on the next call must leave that committed cursor byte-for-byte intact.
        let progress = fixture.identity_once(&repair, b.ordinal, &|| false);
        assert_eq!(progress.outcome, Outcome::Deferred);
        let committed = identity_cursor(&fixture, &repair, b.ordinal).unwrap();
        assert!(committed.1["compare"]["compared"].as_u64().unwrap() > 0);
        let after_commit = fixture.identity_once(&repair, b.ordinal, &|| true);
        assert_eq!(after_commit.outcome, Outcome::Deferred);
        assert_eq!(
            identity_cursor(&fixture, &repair, b.ordinal).unwrap(),
            committed
        );

        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let resumed = fixture.identity(&repair, b.ordinal);
        assert_eq!(resumed.identity(), 2);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
    }

    #[test]
    fn native_identity_shares_the_outer_native_budget_after_cached_anchors() {
        let value = "α".repeat(100_000);
        let bytes = format!(
            "{{\"a\":{0},\"b\":{0},\"type\":\"session.start\"}}\n",
            serde_json::to_string(&value).unwrap()
        );
        let mut fixture = Fixture::new(bytes.as_bytes());
        let reads = super::super::super::super::FILE_READS.with(Cell::get);
        let repair = fixture.qualified_record();
        let a = fixture.member(&repair, "a");
        let b = fixture.member(&repair, "b");
        let kind = fixture.member(&repair, "type");
        let expected = fixture.identity(&repair, a.ordinal);
        fixture
            .store
            .original_literal_step(
                &fixture.job,
                &fixture.source,
                &repair,
                kind.ordinal,
                "session.start",
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap();
        for (native, rows) in [(0, ROWS), (1, 11)] {
            let step = fixture
                .store
                .original_identity_step_with_budget(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    b.ordinal,
                    &fixture.sources.reader(),
                    &|| false,
                    native,
                    rows,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            assert_eq!(step.work.native_bytes, 0);
            assert_eq!(step.work.metadata_rows, 0);
        }
        let mut attempts = 0;
        loop {
            attempts += 1;
            assert!(
                attempts < 128,
                "bounded comparison must make durable progress"
            );
            let member = fixture
                .store
                .original_member_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    fixture
                        .store
                        .original_root(&fixture.job, &fixture.source, &repair)
                        .unwrap()
                        .unwrap()
                        .address,
                    "type",
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            let literal = fixture
                .store
                .original_literal_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    kind.ordinal,
                    "session.start",
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            let prior = member.work.native_bytes + literal.work.native_bytes;
            assert_eq!(prior, 2);
            let step = fixture
                .store
                .original_identity_step_with_budget(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    b.ordinal,
                    &fixture.sources.reader(),
                    &|| false,
                    crate::SESSION_SOURCE_CHUNK_MAX - prior,
                    ROWS - 12,
                )
                .unwrap();
            assert!(prior + step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(step.work.metadata_rows <= ROWS - 12);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            if let Some(actual) = step.text {
                assert_eq!(actual, expected);
                break;
            }
        }
        assert!(attempts > 1);
        assert_eq!(
            super::super::super::super::FILE_READS.with(Cell::get),
            reads
        );
    }

    #[test]
    fn native_identity_cached_handle_and_vm_work_stay_bounded_with_class_history() {
        let bytes = b"{\"a\":\"cached\",\"b\":\"other\"}\n";
        for history in [0usize, 128, 1024, 8192] {
            let mut fixture = Fixture::new(bytes);
            let repair = fixture.qualified_record();
            let a = fixture.member(&repair, "a");
            let b = fixture.member(&repair, "b");
            let canonical = fixture.identity(&repair, a.ordinal);
            assert_eq!(canonical.identity(), 1);
            if history > 0 {
                let (digest, decoded_bytes): (Vec<u8>, i64) = fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT digest,decoded_bytes FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND ordinal=?3",
                        params![
                            repair.original.as_slice(),
                            repair.cursor.record_at as i64,
                            b.ordinal as i64
                        ],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                fixture
                    .store
                    .connection
                    .execute(
                        "WITH RECURSIVE prior(class) AS (SELECT 2 UNION ALL SELECT class+1 FROM prior WHERE class<?1) INSERT INTO native_original_classes SELECT ?2,class,?3,?4,?5,?6 FROM prior",
                        params![
                            history as i64 + 1,
                            repair.original.as_slice(),
                            repair.cursor.record_at as i64,
                            b.ordinal as i64,
                            digest,
                            decoded_bytes
                        ],
                    )
                    .unwrap();
            }
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            let cached = fixture.identity_once(&repair, a.ordinal, &|| false);
            assert_eq!(cached.text, Some(canonical));
            assert!(cached.class_sql_vm_steps > 0);
            assert!(cached.class_sql_vm_steps < 256);
            assert!(cached.serialized_cursor_bytes <= STATE_BYTES);
            assert!(cached.work.metadata_rows <= ROWS);
            assert!(cached.work.metadata_bytes <= BYTES);
            assert!(cached.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
        }
    }

    #[test]
    fn native_identity_source_generation_change_during_provider_io_stays_unavailable() {
        let bytes = b"{\"a\":\"target\",\"b\":\"other\"}\n";
        let mut fixture = Fixture::new(bytes);
        let repair = fixture.qualified_record();
        let target = fixture.member(&repair, "a");
        let provider = InterleavingReader::new(repair.source.clone(), bytes, Some(0), |_| {});
        let file_reads = super::super::super::super::FILE_READS.with(Cell::get);

        let result = fixture.identity_with_provider(&repair, target.ordinal, &provider);
        assert!(result.is_err());
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            provider
                .bytes_read
                .load(std::sync::atomic::Ordering::SeqCst),
            bytes.len() * 2
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_identities",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
        assert_eq!(
            super::super::super::super::FILE_READS.with(Cell::get),
            file_reads
        );
    }

    #[test]
    fn native_identity_publication_change_during_provider_io_stays_unavailable() {
        let bytes = b"{\"a\":\"target\",\"b\":\"other\"}\n";
        let mut fixture = Fixture::new(bytes);
        let repair = fixture.qualified_record();
        let target = fixture.member(&repair, "a");
        let cache_path = super::super::super::super::EventCache::path(&fixture.options.cache);
        let job = fixture.job.id;
        let path = fixture.source.source.path.to_string_lossy().into_owned();
        let provider = InterleavingReader::new(repair.source.clone(), bytes, None, move |call| {
            if call == 0 {
                let db = rusqlite::Connection::open(&cache_path).unwrap();
                assert_eq!(
                    db.execute(
                        "UPDATE native_query_sources SET metadata='{}' WHERE job=?1 AND path=?2",
                        params![job, path]
                    )
                    .unwrap(),
                    1
                );
            }
        });

        let result = fixture.identity_with_provider(&repair, target.ordinal, &provider);
        assert!(result.is_err());
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            provider
                .bytes_read
                .load(std::sync::atomic::Ordering::SeqCst),
            bytes.len() * 2
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_identities",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn native_identity_runtime_change_during_provider_io_returns_conflict() {
        let bytes = b"{\"a\":\"target\",\"b\":\"other\"}\n";
        let mut fixture = Fixture::new(bytes);
        let repair = fixture.qualified_record();
        let target = fixture.member(&repair, "a");
        let runtime_path = fixture.options.claude_json.clone();
        let provider = InterleavingReader::new(repair.source.clone(), bytes, None, move |call| {
            if call == 0 {
                std::fs::write(&runtime_path, b"{\"changed\":true}").unwrap();
            }
        });

        let step = fixture
            .identity_with_provider(&repair, target.ordinal, &provider)
            .unwrap();
        assert_eq!(step.outcome, Outcome::Conflict);
        assert_eq!(step.fence_checks, 5);
        assert_eq!(step.records_finish_calls, 1);
        assert_eq!(step.work.range_requests, 1);
        assert_eq!(step.work.native_bytes, 1);
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            provider
                .bytes_read
                .load(std::sync::atomic::Ordering::SeqCst),
            bytes.len() * 2
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_identities",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn native_identity_class_generation_change_during_provider_io_cannot_allocate() {
        let bytes = b"{\"a\":\"target\",\"b\":\"other\"}\n";
        let mut fixture = Fixture::new(bytes);
        let repair = fixture.qualified_record();
        let target = fixture.member(&repair, "a");
        let other = fixture.member(&repair, "b");
        let (digest, decoded_bytes): (Vec<u8>, i64) = fixture
            .store
            .connection
            .query_row(
                "SELECT digest,decoded_bytes FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND ordinal=?3",
                params![
                    repair.original.as_slice(),
                    repair.cursor.record_at as i64,
                    other.ordinal as i64
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let cache_path = super::super::super::super::EventCache::path(&fixture.options.cache);
        let original = repair.original;
        let record = repair.cursor.record_at as i64;
        let ordinal = other.ordinal as i64;
        let provider = InterleavingReader::new(repair.source.clone(), bytes, None, move |call| {
            if call == 0 {
                let db = rusqlite::Connection::open(&cache_path).unwrap();
                db.execute(
                    "INSERT INTO native_original_classes VALUES(?1,1,?2,?3,?4,?5)",
                    params![original.as_slice(), record, ordinal, digest, decoded_bytes],
                )
                .unwrap();
            }
        });

        let step = fixture
            .identity_with_provider(&repair, target.ordinal, &provider)
            .unwrap();
        assert_eq!(step.outcome, Outcome::Conflict);
        assert_eq!(step.fence_checks, 8);
        assert_eq!(step.records_finish_calls, 1);
        assert_eq!(step.work.range_requests, 1);
        assert_eq!(step.work.native_bytes, 1);
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            provider
                .bytes_read
                .load(std::sync::atomic::Ordering::SeqCst),
            bytes.len() * 2
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_identities",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
    }

    fn prepared_record_fixture(bytes: &[u8]) -> (Fixture, Repair) {
        let mut fixture = Fixture::new(bytes);
        let mut repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        while repair.cursor.phase != Phase::FoldRecord {
            fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
        }
        loop {
            let step = fixture
                .store
                .original_fields_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
            if step.outcome == Outcome::Written {
                break;
            }
        }
        (fixture, repair)
    }

    #[test]
    fn original_source_primitives_keep_fixed_pages_and_native_child_ties() {
        use crate::events::native_event::NativeEvent;
        use crate::events::native_harness::ExtraStore;
        use crate::events::native_producer::{SignalStore, ToolStore};
        use crate::events::{Event, Kind, Signal, SignalKind};
        use crate::model::native_stage::SourceTime;
        for count in [0usize, 128, 1024, 8192] {
            let (mut fixture, mut repair) = prepared_record_fixture(b"{}\n");
            let mut reference = vec![
                Event {
                    k: Kind::U,
                    o: 0,
                    ..Default::default()
                },
                Event {
                    k: Kind::A,
                    o: 1,
                    t: Some(11),
                    ..Default::default()
                },
            ];
            reference.extend((0..count).map(|index| Event {
                k: Kind::A,
                o: 2 + index as u64,
                t: Some(11),
                ..Default::default()
            }));
            for event in &reference {
                let tx = fixture.store.connection.transaction().unwrap();
                let mut events = native_events::Events::new(&tx, repair.original).unwrap();
                events
                    .append_event(NativeEvent::<OriginalText> {
                        k: event.k,
                        o: event.o,
                        t: event.t,
                        ..Default::default()
                    })
                    .unwrap();
                tx.commit().unwrap();
            }
            let offset = (1u64 << 63) + 5;
            let regular = Event {
                k: Kind::Tool,
                o: offset,
                b: 2,
                parent: Some(0),
                ..Default::default()
            };
            {
                let tx = fixture.store.connection.transaction().unwrap();
                let mut events = native_events::Events::new(&tx, repair.original).unwrap();
                events
                    .append_event(NativeEvent::<OriginalText> {
                        k: regular.k,
                        o: regular.o,
                        b: regular.b,
                        parent: regular.parent,
                        ..Default::default()
                    })
                    .unwrap();
                events
                    .append_extra(NativeEvent::<OriginalText> {
                        k: Kind::Operation,
                        o: offset,
                        b: 2,
                        parent: Some(0),
                        ..Default::default()
                    })
                    .unwrap();
                events
                    .append_extra(NativeEvent::<OriginalText> {
                        k: Kind::Tool,
                        o: offset,
                        b: 2,
                        parent: Some(0),
                        ..Default::default()
                    })
                    .unwrap();
                events
                    .append_signal(Signal {
                        k: SignalKind::Compact,
                        o: 0,
                        t: None,
                        at: 0,
                        n: None,
                        v: None,
                    })
                    .unwrap();
                tx.commit().unwrap();
            }
            reference.push(regular);
            assert!(
                fixture
                    .store
                    .original_event_facts(&fixture.job, &fixture.source, &repair)
                    .is_err()
            );
            // Simulate the record producer's atomic final seal. Event facts
            // are still separate from native header/usage/Query qualification.
            let cursor = next_record_cursor(&repair);
            assert_eq!(cursor.phase, Phase::Complete);
            fixture
                .store
                .connection
                .execute(
                    "UPDATE native_original_records SET status='folded' WHERE original=?1",
                    params![repair.original.as_slice()],
                )
                .unwrap();
            fixture.store.connection.execute("UPDATE native_original_repairs SET cursor=?2,revision=revision+1 WHERE original=?1",params![repair.original.as_slice(),json(&cursor).unwrap()]).unwrap();
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
            let reads = crate::events::FILE_READS.with(Cell::get);
            let (facts, facts_work) = fixture
                .store
                .original_event_facts(&fixture.job, &fixture.source, &repair)
                .unwrap();
            assert_eq!(
                (facts.regular, facts.extras, facts.signals),
                (reference.len(), 2, 1)
            );
            assert_eq!(
                (facts.seed.first_seq, facts.seed.first_ms),
                (Some(1), Some(11))
            );
            assert_eq!(facts_work.decoded_rows, 4);
            assert!(facts_work.metadata_bytes < 128);
            assert!(facts_work.vm_steps > 0 && facts_work.vm_steps < 256);

            let page = fixture
                .store
                .original_events_page(&fixture.job, &fixture.source, &repair, false, None, 1)
                .unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(page.rows[0].scalar(), reference[0]);
            assert_eq!(page.work.decoded_rows, 1);
            assert!(page.work.metadata_bytes < 4096);
            assert!(page.work.vm_steps < 128);
            let (time, _) = fixture
                .store
                .original_source_time_at(&fixture.job, &fixture.source, &repair, 0)
                .unwrap();
            assert_eq!(time, SourceTime::BeforeFirst);
            let (time, _) = fixture
                .store
                .original_source_time_before_offset(&fixture.job, &fixture.source, &repair, offset)
                .unwrap();
            let regular_seq = reference
                .iter()
                .rposition(|event| event.o <= offset)
                .unwrap_or(0);
            let expected = reference[..=regular_seq]
                .iter()
                .rev()
                .find_map(|event| event.t)
                .map_or(SourceTime::BeforeFirst, SourceTime::Observed);
            assert_eq!(time, expected);
            let (child, work) = fixture
                .store
                .original_last_operation(&fixture.job, &fixture.source, &repair, 0)
                .unwrap();
            // Independent native iteration: extras first, then regular. Equal
            // positions keep the first exact winner, including Tool extras.
            assert_eq!(
                child,
                Some((
                    EventAddress {
                        extra: true,
                        seq: 0
                    },
                    expected,
                    (offset, 2)
                ))
            );
            assert!(work.decoded_rows <= 3);
            assert!(work.vm_steps < 256);
            assert!(
                fixture
                    .store
                    .original_event_at(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        EventAddress {
                            extra: false,
                            seq: usize::MAX
                        }
                    )
                    .unwrap()
                    .0
                    .is_none()
            );
            assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
        }
    }

    #[test]
    fn original_copilot_admission_preserves_missing_start_duplicate_and_header_semantics() {
        use crate::events::native_harness::{Admission, CopilotIdentityStore};
        for size in [0usize, 128, 1024, 8192, 200_000] {
            let version = "v".repeat(size);
            let record = serde_json::json!({"id":"identity","type":"session.start","data":{"copilotVersion":version,"sessionId":"native-session"}});
            let bytes = format!("{record}\n");
            let mut fixture = Fixture::new(bytes.as_bytes());
            let repair = fixture.qualified_record();
            let reads = crate::events::FILE_READS.with(Cell::get);
            let mut attempts = 0;
            let identity = loop {
                attempts += 1;
                assert!(attempts < 128);
                let cancelled = fixture
                    .store
                    .original_copilot_admission_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| true,
                    )
                    .unwrap();
                assert_eq!(cancelled.outcome, Outcome::Deferred);
                assert_eq!(cancelled.work.native_bytes, 0);
                let step = fixture
                    .store
                    .original_copilot_admission_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                if let Some(admission) = step.admission {
                    assert_eq!(admission, Admission::Accepted);
                    break step.identity.unwrap();
                }
            };
            let expected_version=fixture.store.connection.query_row("SELECT metadata FROM native_original_header WHERE original=?1 AND field='copilot_version'",params![repair.original.as_slice()],|r|r.get::<_,String>(0)).unwrap();
            let value: crate::events::native_harness::header::Value<OriginalText> =
                serde_json::from_str(&expected_version).unwrap();
            let crate::events::native_harness::header::Value::Native(version_ref) = value else {
                panic!("native version remains an exact field reference")
            };
            let representative = fixture
                .store
                .original_text_representative(&fixture.job, &fixture.source, &repair, &version_ref)
                .unwrap();
            assert_eq!(representative.receipt.decoded_bytes, version.len() as u64);
            let mut events =
                native_events::Events::new(&fixture.store.connection, repair.original).unwrap();
            assert!(events.copilot_started().unwrap());
            assert!(events.copilot_record_seen(&identity).unwrap());
            let cached = fixture
                .store
                .original_copilot_admission_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert_eq!(cached.admission, Some(Admission::Accepted));
            assert_eq!(cached.identity, Some(identity));
            assert_eq!(cached.work.native_bytes, 1);
            assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
        }
        for (record, expected) in [
            (serde_json::json!({"type":"session.start"}), Admission::Gap),
            (
                serde_json::json!({"id":"id","type":"user.message"}),
                Admission::BeforeStart,
            ),
        ] {
            let mut fixture = Fixture::new(format!("{record}\n").as_bytes());
            let repair = fixture.qualified_record();
            loop {
                let step = fixture
                    .store
                    .original_copilot_admission_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                if let Some(admission) = step.admission {
                    assert_eq!(admission, expected);
                    break;
                }
            }
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM native_original_copilot_seen",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM native_original_header", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn original_copilot_body_resumes_every_duplicate_reset_without_complete_indexes() {
        use crate::events::native_event::{NativeEvent, NativeReply};
        use crate::events::native_producer::{self, ToolStore};
        for history in [0usize, 128, 1024, 8192] {
            let call_id = "λ".repeat(1024);
            let record = serde_json::json!({"id":"record-id","timestamp":"2026-10-15T01:02:03Z","type":"assistant.message","data":{"messageId":"message-id","content":" ","toolRequests":[{"toolCallId":call_id,"name":"bash"}]}});
            let bytes = format!("{}\n", record);
            let mut fixture = Fixture::new(bytes.as_bytes());
            let reads = crate::events::FILE_READS.with(Cell::get);
            let repair = fixture.qualified_record();
            let uid_node = fixture.member(&repair, "id");
            let uid = fixture.identity(&repair, uid_node.ordinal);
            let data = fixture.member(&repair, "data");
            let tools = fixture
                .store
                .original_member_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    data.address,
                    "toolRequests",
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .node
                .unwrap();
            let request = fixture
                .store
                .original_array_page(&fixture.job, &fixture.source, &repair, tools.address, None)
                .unwrap()
                .rows
                .into_iter()
                .next()
                .unwrap();
            let mut get = |name| {
                fixture
                    .store
                    .original_member_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        request.address,
                        name,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap()
                    .node
                    .unwrap()
            };
            let id_node = get("toolCallId");
            let name_node = get("name");
            let message_node = fixture
                .store
                .original_member_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    data.address,
                    "messageId",
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .node
                .unwrap();
            let message = fixture.identity(&repair, message_node.ordinal);
            let id = fixture.identity(&repair, id_node.ordinal);
            let name = fixture.identity(&repair, name_node.ordinal);
            let base = NativeEvent {
                o: repair.cursor.record_at,
                t: crate::events::record_time(&record),
                u: Some(uid),
                ..Default::default()
            };
            let mut oracle = crate::events::FileIndex {
                copilot: Some(Default::default()),
                ..Default::default()
            };
            for sequence in 0..history {
                let event = NativeEvent {
                    k: crate::events::Kind::Tool,
                    o: 0,
                    b: sequence as u32,
                    id: Some(id.clone()),
                    n: Some(name.clone()),
                    r: Some(NativeReply {
                        e: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                let mut native =
                    native_events::Events::new(&fixture.store.connection, repair.original).unwrap();
                native_producer::tool_event(&mut native, event.clone()).unwrap();
                native_producer::complete(native_producer::tool_event(
                    &mut oracle,
                    NativeEvent {
                        id: Some(call_id.clone()),
                        n: Some("bash".to_owned()),
                        ..event.scalar().into()
                    },
                ));
            }
            crate::events::copilot(&mut oracle, &record, repair.cursor.record_at);
            let mut attempts = 0;
            loop {
                attempts += 1;
                assert!(
                    attempts < history + 128,
                    "every bounded phase must make progress"
                );
                let step = fixture
                    .store
                    .original_copilot_body_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &base,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                if step.complete {
                    break;
                }
            }
            assert!(attempts >= history);
            let native =
                native_events::Events::read_only(&fixture.store.connection, repair.original);
            assert_eq!(native.event_count().unwrap(), oracle.events.len());
            for (sequence, expected) in oracle.events.iter().enumerate() {
                let native =
                    native_events::Events::read_only(&fixture.store.connection, repair.original);
                let event = native.event(sequence).unwrap().unwrap();
                assert_eq!(
                    serde_json::to_value(event.scalar()).unwrap(),
                    serde_json::to_value(NativeEvent::from(expected.clone()).scalar()).unwrap()
                );
                assert_eq!(
                    event.id,
                    expected.id.as_ref().map(|text| if text == &call_id {
                        id.clone()
                    } else {
                        assert_eq!(text, "message-id");
                        message.clone()
                    })
                );
                assert_eq!(
                    event.n,
                    expected.n.as_ref().map(|text| {
                        assert_eq!(text, "bash");
                        name.clone()
                    })
                );
                assert_eq!(
                    event.u,
                    expected.u.as_ref().map(|text| {
                        assert_eq!(text, "record-id");
                        base.u.clone().unwrap()
                    })
                );
            }
            assert!(
                fixture
                    .store
                    .original_event_facts(&fixture.job, &fixture.source, &repair)
                    .is_err(),
                "a body cannot certify missing header/admission/source work"
            );
            assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
        }
    }

    #[test]
    fn original_signal_tags_stream_the_whole_safe_value_and_resume_after_cancellation() {
        for size in [0usize, 128, 1024, 8192, 200_000] {
            for suffix in ["", "λ", "\n"] {
                let value = format!("{}{}", "model-".repeat(size), suffix);
                let bytes = format!("{{\"value\":{}}}\n", serde_json::to_string(&value).unwrap());
                let mut fixture = Fixture::new(bytes.as_bytes());
                let repair = fixture.qualified_record();
                let node = fixture.member(&repair, "value");
                let reads = crate::events::FILE_READS.with(Cell::get);
                let mut attempts = 0;
                loop {
                    attempts += 1;
                    assert!(attempts < 256);
                    let cancelled = fixture
                        .store
                        .original_tag_step(
                            &fixture.job,
                            &fixture.source,
                            &repair,
                            node.ordinal,
                            &fixture.sources.reader(),
                            &|| true,
                        )
                        .unwrap();
                    assert_eq!(cancelled.work.native_bytes, 0);
                    let step = fixture
                        .store
                        .original_tag_step_with_budget(
                            &fixture.job,
                            &fixture.source,
                            &repair,
                            node.ordinal,
                            &fixture.sources.reader(),
                            &|| false,
                            crate::SESSION_SOURCE_CHUNK_MAX - 2,
                            4,
                        )
                        .unwrap();
                    assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX - 2);
                    assert!(step.work.metadata_rows <= 4);
                    assert!(step.work.metadata_bytes < 16 * 1024);
                    fixture.store = NativeStore::open(&fixture.options).unwrap();
                    if step.complete {
                        assert_eq!(step.tag, crate::events::bounded(&value));
                        break;
                    }
                }
                let cached = fixture
                    .store
                    .original_tag_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        node.ordinal,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(cached.work.native_bytes, 1);
                assert_eq!(cached.tag, crate::events::bounded(&value));
                assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
            }
        }
    }

    #[test]
    fn original_timestamp_is_bounded_resumable_and_equals_the_native_producer() {
        for size in [0usize, 128, 1024, 8192, 200_000] {
            let value = format!("2026-10-15T-1:-2:-3.12{}+01:-1ignored", "λ".repeat(size));
            let bytes = format!(
                "{{\"timestamp\":{},\"padding\":{}}}\n",
                serde_json::to_string(&value)
                    .unwrap()
                    .replace('λ', "\\u03bb"),
                serde_json::to_string(&"x".repeat(size)).unwrap()
            );
            let mut fixture = Fixture::new(bytes.as_bytes());
            let reads = crate::events::FILE_READS.with(Cell::get);
            let repair = fixture.qualified_record();
            let node = fixture.member(&repair, "timestamp");
            let cancelled = fixture
                .store
                .original_timestamp_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    node.ordinal,
                    &fixture.sources.reader(),
                    &|| true,
                )
                .unwrap();
            assert_eq!(cancelled.outcome, Outcome::Deferred);
            assert_eq!(cancelled.work.native_bytes, 0);
            let mut attempts = 0;
            loop {
                attempts += 1;
                assert!(attempts < 256);
                let step = fixture
                    .store
                    .original_timestamp_step_with_budget(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        node.ordinal,
                        &fixture.sources.reader(),
                        &|| false,
                        crate::SESSION_SOURCE_CHUNK_MAX - 2,
                        4,
                    )
                    .unwrap();
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX - 2);
                assert!(step.work.metadata_rows <= 4);
                assert!(step.work.metadata_bytes < 16 * 1024);
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                if step.complete {
                    assert_eq!(step.outcome, Outcome::Written);
                    assert_eq!(step.millis, crate::events::parse_ms(&value));
                    break;
                }
            }
            let cached = fixture
                .store
                .original_timestamp_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    node.ordinal,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert!(cached.complete);
            assert_eq!(cached.work.native_bytes, 1);
            assert_eq!(cached.work.range_requests, 1);
            assert!(cached.work.metadata_rows <= 2);
            assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
            let mut changed = fixture.source.clone();
            changed.claim = Some(changed.claim.unwrap_or(0) + 1);
            assert!(
                fixture
                    .store
                    .original_timestamp_step(
                        &fixture.job,
                        &changed,
                        &repair,
                        node.ordinal,
                        &fixture.sources.reader(),
                        &|| false
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn native_literal_classification_matches_serde_without_complete_index_loads() {
        for count in [0usize, 128, 1024, 8192] {
            for kind in ["session.start", "session.spart"] {
                let bytes = format!(
                    "{{\"type\":\"{}\",\"body\":\"{}\"}}\n",
                    kind.replace('e', "\\u0065"),
                    "x".repeat(count)
                )
                .into_bytes();
                let native: Value = serde_json::from_slice(&bytes).unwrap();
                let expected = native["type"].as_str() == Some("session.start");
                let (mut fixture, repair) = prepared_record_fixture(&bytes);
                let root = fixture
                    .store
                    .original_root(&fixture.job, &fixture.source, &repair)
                    .unwrap()
                    .unwrap();
                let selected = fixture
                    .store
                    .original_member_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        root.address,
                        "type",
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(selected.complete);
                let ordinal = selected.node.unwrap().ordinal;
                // Force the hash candidate to match: only exact native bytes
                // can admit a semantic type, including escaped literal text.
                fixture.store.connection.execute("UPDATE native_original_fields SET digest=?3 WHERE original=?1 AND record_offset=?2 AND ordinal=?4", params![repair.original.as_slice(), seq_u64(repair.cursor.record_at).unwrap(), Sha256::digest(b"session.start").as_slice(), seq_u64(ordinal).unwrap()]).unwrap();
                let reads = crate::events::FILE_READS.with(Cell::get);
                let calls = Cell::new(0);
                let cancel = || {
                    calls.set(calls.get() + 1);
                    calls.get() == 3
                };
                let step = fixture
                    .store
                    .original_literal_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        ordinal,
                        "session.start",
                        &fixture.sources.reader(),
                        &cancel,
                    )
                    .unwrap();
                assert_eq!(step.outcome, Outcome::Deferred);
                let partial: i64 = fixture
                    .store
                    .connection
                    .query_row("SELECT count(*) FROM native_original_literal", [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(partial, 0);
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                let step = fixture
                    .store
                    .original_literal_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        ordinal,
                        "session.start",
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(step.outcome, Outcome::Written);
                assert_eq!(step.matches, Some(expected));
                assert_eq!(step.work.range_requests, 1);
                assert!(step.work.native_bytes < 128);
                assert!(step.work.metadata_rows <= 5);
                assert!(step.work.metadata_bytes < 4096);
                assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                let cached = fixture
                    .store
                    .original_literal_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        ordinal,
                        "session.start",
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(cached.matches, Some(expected));
                assert_eq!(
                    (cached.work.range_requests, cached.work.native_bytes),
                    (1, 1)
                );
                // Even a cached false remains tied to the admitted Current
                // observation. Mutating the ledger must refuse the answer.
                fixture
                    .store
                    .connection
                    .execute(
                        "UPDATE files SET resume_at=resume_at+1 WHERE path=?1",
                        params![fixture.source.source.path.to_string_lossy()],
                    )
                    .unwrap();
                assert!(
                    fixture
                        .store
                        .original_literal_step(
                            &fixture.job,
                            &fixture.source,
                            &repair,
                            ordinal,
                            "session.start",
                            &fixture.sources.reader(),
                            &|| false
                        )
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn native_gap_fold_is_durable_coalesced_and_bounded_with_growing_history() {
        use crate::events::native_event::NativeEvent;
        use crate::events::native_producer::ToolStore;
        for size in [0usize, 128, 1024, 8192] {
            let mut fixture = Fixture::new(b"[]\nnull\n");
            let mut repair = fixture
                .store
                .begin_original_repair(
                    &fixture.job,
                    &fixture.source,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            // Already-derived history can grow without becoming resident in
            // the next native record transition. Only the last row is read.
            {
                let tx = fixture.store.connection.transaction().unwrap();
                tx.execute(
                    "INSERT INTO native_original_event_state VALUES(?1,?2)",
                    params![repair.original.as_slice(), size as i64],
                )
                .unwrap();
                let text = serde_json::to_string(&NativeEvent::<native_events::Text> {
                    k: crate::events::Kind::U,
                    ..Default::default()
                })
                .unwrap();
                for sequence in 0..size {
                    tx.execute(
                        "INSERT INTO native_original_events VALUES(?1,?2,0,NULL,0,0,?3)",
                        params![repair.original.as_slice(), sequence as i64, text],
                    )
                    .unwrap();
                }
                tx.commit().unwrap();
            }
            let reads = super::super::super::super::FILE_READS.with(Cell::get);
            let mut gap_steps = 0;
            while repair.cursor.phase != Phase::Complete {
                if repair.cursor.phase == Phase::FoldRecord {
                    let prepared = fixture
                        .store
                        .original_fields_step(
                            &fixture.job,
                            &fixture.source,
                            &repair,
                            &fixture.sources.reader(),
                            &|| false,
                        )
                        .unwrap();
                    repair = fixture
                        .store
                        .reload_original_repair(
                            &fixture.job,
                            &fixture.source,
                            &fixture.sources.reader(),
                        )
                        .unwrap()
                        .unwrap();
                    if prepared.outcome == Outcome::Written {
                        let before = repair.revision;
                        let calls = Cell::new(0);
                        let cancelled = || {
                            calls.set(calls.get() + 1);
                            calls.get() == 3
                        };
                        let step = fixture
                            .store
                            .original_gap_step(
                                &fixture.job,
                                &fixture.source,
                                &repair,
                                &fixture.sources.reader(),
                                &cancelled,
                            )
                            .unwrap();
                        assert_eq!(step.outcome, Outcome::Deferred);
                        fixture.store = NativeStore::open(&fixture.options).unwrap();
                        repair = fixture
                            .store
                            .reload_original_repair(
                                &fixture.job,
                                &fixture.source,
                                &fixture.sources.reader(),
                            )
                            .unwrap()
                            .unwrap();
                        assert_eq!(repair.revision, before);
                        let step = fixture
                            .store
                            .original_gap_step(
                                &fixture.job,
                                &fixture.source,
                                &repair,
                                &fixture.sources.reader(),
                                &|| false,
                            )
                            .unwrap();
                        assert_eq!(step.outcome, Outcome::Written);
                        assert!(step.work.metadata_rows <= ROWS);
                        assert!(step.work.metadata_bytes < 4096);
                        assert_eq!(step.work.range_requests, 1);
                        assert!(step.work.native_bytes <= 1);
                        gap_steps += 1;
                    }
                } else {
                    fixture
                        .store
                        .original_lex_step(
                            &fixture.job,
                            &fixture.source,
                            &repair,
                            &fixture.sources.reader(),
                            &|| false,
                        )
                        .unwrap();
                }
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
            }
            assert_eq!(gap_steps, 2);
            let events =
                native_events::Events::new(&fixture.store.connection, repair.original).unwrap();
            assert_eq!(events.event_count().unwrap(), size + 1);
            let last = events.event(size).unwrap().unwrap();
            assert_eq!(
                last.scalar(),
                Event {
                    k: crate::events::Kind::Gap,
                    o: 0,
                    ..Default::default()
                }
            );
            // Independent complete native parsing of both non-object records
            // gives one Gap at the first absolute offset, never one per line.
            let mut complete = FileIndex::default();
            for (offset, line) in [(0, b"[]".as_slice()), (3, b"null".as_slice())] {
                let value: Value = serde_json::from_slice(line).unwrap();
                assert!(!value.is_object());
                super::super::super::super::gap(&mut complete, offset);
            }
            assert_eq!(complete.events, vec![last.scalar()]);
            assert_eq!(
                super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
        }
    }

    #[test]
    fn native_invalid_intermediate_record_is_unreadable_after_cancel_and_reopen() {
        for bytes in [
            b"[]\n".to_vec(),
            format!("{}{{}}{}\n", "{\"x\":".repeat(127), "}".repeat(127)).into_bytes(),
        ] {
            let mut fixture = Fixture::new(&bytes);
            let mut repair = fixture
                .store
                .begin_original_repair(
                    &fixture.job,
                    &fixture.source,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            while repair.cursor.phase != Phase::FoldRecord {
                fixture
                    .store
                    .original_lex_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
            }
            while !repair.cursor.fields_registered {
                fixture
                    .store
                    .original_fields_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
            }
            assert!(repair.cursor.native_invalid);
            assert_eq!(
                record_status(&fixture.store.connection, &repair)
                    .unwrap()
                    .as_deref(),
                Some("lexed")
            );
            assert!(!fields_ready(&fixture.store.connection, &repair).unwrap());
            assert!(
                fixture
                    .store
                    .original_root(&fixture.job, &fixture.source, &repair)
                    .is_err()
            );
            assert_eq!(
                fixture
                    .store
                    .finish_original_record(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &|_, _, _| panic!("invalid record reached native completion"),
                        &|| false
                    )
                    .unwrap(),
                Outcome::Conflict
            );
            let calls = Cell::new(0);
            let cancelled = || {
                calls.set(calls.get() + 1);
                calls.get() == 4
            };
            let step = fixture
                .store
                .original_fields_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &cancelled,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
            assert!(!fields_ready(&fixture.store.connection, &repair).unwrap());
            assert!(
                fixture
                    .store
                    .original_root(&fixture.job, &fixture.source, &repair)
                    .is_err()
            );
            fixture
                .store
                .original_fields_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
            assert_eq!(
                record_status(&fixture.store.connection, &repair)
                    .unwrap()
                    .as_deref(),
                Some("gap")
            );
            assert!(fields_ready(&fixture.store.connection, &repair).unwrap());
            assert!(
                fixture
                    .store
                    .original_root(&fixture.job, &fixture.source, &repair)
                    .is_err()
            );
        }
    }

    #[test]
    fn obsolete_cursor_restarts_and_cleanup_resumes_in_bounded_lex_steps() {
        for size in [0, 128, 1024, 8192] {
            let mut fixture = Fixture::new(b"{}\n");
            let reference = reference(&fixture.sources.reader(), &fixture.source).unwrap();
            let old = previous_namespace(&reference).unwrap();
            let current = namespace(&reference).unwrap();
            assert_ne!(old, current);
            // This is the earlier fa56540d cursor shape, without the required
            // native_invalid member. It must never be decoded during restart.
            let mut cursor = serde_json::to_value(Cursor::new()).unwrap();
            cursor.as_object_mut().unwrap().remove("native_invalid");
            assert!(serde_json::from_value::<Cursor>(cursor.clone()).is_err());
            fixture
                .store
                .connection
                .execute(
                    "INSERT INTO native_original_repairs VALUES(?1,?2,?3,7,?4)",
                    params![
                        old.as_slice(),
                        PREVIOUS_REPAIR_PARSER,
                        json(&reference).unwrap(),
                        cursor.to_string()
                    ],
                )
                .unwrap();
            {
                let tx = fixture.store.connection.transaction().unwrap();
                for ordinal in 0..size + 1 {
                    tx.execute(
                        "INSERT INTO native_original_tokens VALUES(?1,0,?2,'{}')",
                        params![old.as_slice(), ordinal],
                    )
                    .unwrap();
                }
                tx.execute("INSERT INTO native_original_fields VALUES(?1,0,0,0,1,NULL,0,'{}','{}',NULL,NULL)",params![old.as_slice()]).unwrap();
                tx.execute(
                    "INSERT INTO native_original_nodes VALUES(?1,0,0,NULL,NULL,NULL,'{}',1)",
                    params![old.as_slice()],
                )
                .unwrap();
                tx.execute(
                    "INSERT INTO native_original_numbers VALUES(?1,0,0,0,1,'{}',NULL)",
                    params![old.as_slice()],
                )
                .unwrap();
                tx.execute(
                    "INSERT INTO native_original_members VALUES(?1,0,0,'type',0,'{}')",
                    params![old.as_slice()],
                )
                .unwrap();
                tx.execute(
                    "INSERT INTO native_original_records VALUES(?1,0,3,'lexed')",
                    params![old.as_slice()],
                )
                .unwrap();
                tx.commit().unwrap();
            }
            let reads = super::super::super::super::FILE_READS.with(Cell::get);
            let mut repair = fixture
                .store
                .begin_original_repair(
                    &fixture.job,
                    &fixture.source,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            assert_eq!(repair.original, current);
            assert_eq!(repair.revision, 0);
            assert_eq!(repair.cursor.source_at, 0);
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT cursor FROM native_original_repairs WHERE original=?1",
                        params![old.as_slice()],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                cursor.to_string()
            );
            let calls = Cell::new(0);
            let cancelled = || {
                calls.set(calls.get() + 1);
                calls.get() == 3
            };
            let cancelled_step = fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &cancelled,
                )
                .unwrap();
            assert_eq!(cancelled_step.outcome, Outcome::Deferred);
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT count(*) FROM native_original_tokens WHERE original=?1",
                        params![old.as_slice()],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                i64::from(size) + 1
            );
            let mut attempts = 0;
            loop {
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
                let pending: i64 = fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT table_index FROM native_original_cleanup WHERE original=?1",
                        params![current.as_slice()],
                        |row| row.get(0),
                    )
                    .unwrap();
                if pending == 7 {
                    break;
                }
                let step = fixture
                    .store
                    .original_lex_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(step.outcome, Outcome::Deferred);
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.cleanup_vm_steps < 4096);
                assert_eq!(step.work.native_bytes, 0);
                assert_eq!(step.work.range_requests, 0);
                assert_eq!(repair.cursor.source_at, 0);
                attempts += 1;
                assert!(attempts < size / 30 + 16);
            }
            for table in [
                "native_original_tokens",
                "native_original_fields",
                "native_original_nodes",
                "native_original_numbers",
                "native_original_members",
                "native_original_records",
                "native_original_repairs",
            ] {
                assert_eq!(
                    fixture
                        .store
                        .connection
                        .query_row(
                            &format!("SELECT count(*) FROM {table} WHERE original=?1"),
                            params![old.as_slice()],
                            |row| row.get::<_, i64>(0)
                        )
                        .unwrap(),
                    0
                );
            }
            assert_eq!(fs::read(&fixture.source.source.path).unwrap(), b"{}\n");
            assert!(check_source(&fixture.store.connection, &fixture.source).unwrap());
            assert!(job_current(&fixture.store.connection, &fixture.job).unwrap());
            assert_eq!(
                super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
            fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
            assert_eq!(repair.cursor.phase, Phase::FoldRecord);
        }
    }

    fn seed_obsolete_policy(fixture: &mut Fixture, old: [u8; 32], events: i64) {
        let source = reference(&fixture.sources.reader(), &fixture.source).unwrap();
        let tx = fixture.store.connection.transaction().unwrap();
        tx.execute(
            "INSERT INTO native_original_repairs VALUES(?1,24,?2,7,'{unknown old cursor')",
            params![old.as_slice(), json(&source).unwrap()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_original_event_state VALUES(?1,?2)",
            params![old.as_slice(), events],
        )
        .unwrap();
        for sequence in 0..events {
            tx.execute(
                "INSERT INTO native_original_events VALUES(?1,?2,1,1,0,0,'{unknown old event')",
                params![old.as_slice(), sequence],
            )
            .unwrap();
        }
        tx.execute(
            "INSERT INTO native_original_pending VALUES(?1,1,0,0)",
            params![old.as_slice()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_original_tool_ids VALUES(?1,1,0,1)",
            params![old.as_slice()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_original_classes VALUES(?1,1,0,0,zeroblob(32),4)",
            params![old.as_slice()],
        )
        .unwrap();
        tx.execute("INSERT INTO native_original_identities VALUES(?1,0,0,0,'{\"complete\":true,\"class\":1}')",params![old.as_slice()]).unwrap();
        tx.execute(
            "INSERT INTO native_original_tokens VALUES(?1,0,0,'{unknown token')",
            params![old.as_slice()],
        )
        .unwrap();
        tx.execute("INSERT INTO native_original_fields VALUES(?1,0,0,0,1,NULL,0,'{unknown field','{unknown fingerprint',NULL,NULL)",params![old.as_slice()]).unwrap();
        tx.execute(
            "INSERT INTO native_original_nodes VALUES(?1,0,0,NULL,NULL,NULL,'{unknown node',1)",
            params![old.as_slice()],
        )
        .unwrap();
        tx.execute("INSERT INTO native_original_numbers VALUES(?1,0,0,0,1,'{unknown number cursor','{\"Finite\":1.0}')",params![old.as_slice()]).unwrap();
        tx.execute(
            "INSERT INTO native_original_members VALUES(?1,0,0,'n',0,'{unknown member cursor')",
            params![old.as_slice()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_original_records VALUES(?1,0,?2,'folded')",
            params![old.as_slice(), source.offset as i64],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO native_original_cleanup VALUES(?1,?2,3)",
            params![
                old.as_slice(),
                previous_namespace(&source).unwrap().as_slice()
            ],
        )
        .unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn numeric_policy_restart_retires_both_formats_and_requalifies_exact_native_facts() {
        let bytes = b"{\"n\":1.000000000000000111022302462515654042363166809082031251,\"a\":\"same\",\"b\":\"same\"}\n";
        let expected: Value = serde_json::from_slice(bytes).unwrap();
        assert_ne!(expected["n"].as_f64().unwrap().to_bits(), 1.0_f64.to_bits());
        let mut fixture = Fixture::new(bytes);
        let source = reference(&fixture.sources.reader(), &fixture.source).unwrap();
        let obsolete = [
            previous_namespace(&source).unwrap(),
            format_two_namespace(&source).unwrap(),
        ];
        let current = namespace(&source).unwrap();
        assert_ne!(current, obsolete[0]);
        assert_ne!(current, obsolete[1]);
        for old in obsolete {
            seed_obsolete_policy(&mut fixture, old, 31);
        }
        let unknown = [0x66_u8; 32];
        fixture.store.connection.execute("INSERT INTO native_original_repairs VALUES(?1,24,'unknown external source',0,'unknown external cursor')",params![unknown.as_slice()]).unwrap();
        let reads = super::super::super::super::FILE_READS.with(Cell::get);
        let repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(repair.original, current);
        assert_eq!(repair.cursor.source_at, 0);
        let calls = Cell::new(0);
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        let cancelled_step = fixture
            .store
            .original_lex_step(
                &fixture.job,
                &fixture.source,
                &repair,
                &fixture.sources.reader(),
                &cancelled,
            )
            .unwrap();
        assert_eq!(cancelled_step.outcome, Outcome::Deferred);
        for old in obsolete {
            let count: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_events WHERE original=?1",
                    params![old.as_slice()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 31);
        }
        let mut attempts = 0;
        while native_retirement::pending(&fixture.store.connection, current).unwrap() {
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            let resumed = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
            assert_eq!(resumed.cursor.source_at, 0);
            let step = fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &resumed,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            assert!(step.work.metadata_rows <= ROWS);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.work.cleanup_vm_steps < 4096);
            assert!(step.work.cleanup_key_bytes <= STATE_BYTES);
            assert_eq!((step.work.range_requests, step.work.native_bytes), (0, 0));
            attempts += 1;
            assert!(attempts < 64);
        }
        for old in obsolete {
            for table in [
                "native_original_repairs",
                "native_original_event_state",
                "native_original_events",
                "native_original_pending",
                "native_original_tool_ids",
                "native_original_classes",
                "native_original_identities",
                "native_original_tokens",
                "native_original_fields",
                "native_original_nodes",
                "native_original_numbers",
                "native_original_members",
                "native_original_records",
                "native_original_cleanup",
            ] {
                let count: i64 = fixture
                    .store
                    .connection
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE original=?1"),
                        params![old.as_slice()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(count, 0, "unretired {table}");
            }
        }
        let qualified = fixture.qualified_record();
        let n = fixture.member(&qualified, "n");
        let number = fixture
            .store
            .original_number(&fixture.job, &fixture.source, &qualified, n.address)
            .unwrap()
            .unwrap();
        assert_eq!(
            number.number().as_f64().unwrap().to_bits(),
            expected["n"].as_f64().unwrap().to_bits()
        );
        let a = fixture.member(&qualified, "a");
        let b = fixture.member(&qualified, "b");
        let first = fixture.identity(&qualified, a.ordinal);
        let second = fixture.identity(&qualified, b.ordinal);
        assert_eq!(first, second);
        assert_eq!(first.identity(), 1);
        let before = json(&qualified.cursor).unwrap();
        let again = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(json(&again.cursor).unwrap(), before);
        assert!(!native_retirement::pending(&fixture.store.connection, current).unwrap());
        assert_eq!(fs::read(&fixture.source.source.path).unwrap(), bytes);
        assert!(job_current(&fixture.store.connection, &fixture.job).unwrap());
        assert!(check_source(&fixture.store.connection, &fixture.source).unwrap());
        let unknown_count:i64=fixture.store.connection.query_row("SELECT count(*) FROM native_original_repairs WHERE original=?1 AND cursor='unknown external cursor'",params![unknown.as_slice()],|row|row.get(0)).unwrap();
        assert_eq!(unknown_count, 1);
        assert_eq!(
            super::super::super::super::FILE_READS.with(Cell::get),
            reads
        );
    }

    #[test]
    fn policy_retirement_step_holds_keys_and_work_fixed_as_obsolete_history_grows() {
        let mut baseline = None;
        for history in [0, 128, 1024, 8192] {
            let mut fixture = Fixture::new(b"{}\n");
            let source = reference(&fixture.sources.reader(), &fixture.source).unwrap();
            let old = format_two_namespace(&source).unwrap();
            seed_obsolete_policy(&mut fixture, old, history + 30);
            if history == 0 {
                // Interrupted previous cleanup removed the repair root first.
                // Its remaining consumers still require called retirement.
                fixture
                    .store
                    .connection
                    .execute(
                        "DELETE FROM native_original_repairs WHERE original=?1",
                        params![old.as_slice()],
                    )
                    .unwrap();
            }
            let reads = super::super::super::super::FILE_READS.with(Cell::get);
            let repair = fixture
                .store
                .begin_original_repair(
                    &fixture.job,
                    &fixture.source,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            let step = fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            assert_eq!(step.work.metadata_rows, 64);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.work.cleanup_vm_steps < 4096);
            assert!(step.work.cleanup_key_bytes <= STATE_BYTES);
            assert_eq!((step.work.range_requests, step.work.native_bytes), (0, 0));
            let fixed = (
                step.work.metadata_rows,
                step.work.metadata_bytes,
                step.work.cleanup_key_bytes,
            );
            assert_eq!(*baseline.get_or_insert(fixed), fixed);
            assert_eq!(repair.cursor.source_at, 0);
            let remaining: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_events WHERE original=?1",
                    params![old.as_slice()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(remaining, history);
            assert_eq!(
                super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
        }
    }

    #[test]
    fn policy_retirement_rechecks_source_owner_runtime_and_provider_after_evidence_io() {
        for change in ["source", "owner", "runtime", "provider"] {
            let bytes = b"{}\n";
            let mut fixture = Fixture::new(bytes);
            let source = reference(&fixture.sources.reader(), &fixture.source).unwrap();
            let old = format_two_namespace(&source).unwrap();
            seed_obsolete_policy(&mut fixture, old, 31);
            let repair = fixture
                .store
                .begin_original_repair(
                    &fixture.job,
                    &fixture.source,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            let cache = super::super::super::super::EventCache::path(&fixture.options.cache);
            let path = fixture.source.source.path.to_string_lossy().into_owned();
            let runtime = fixture.options.claude_json.clone();
            let provider = InterleavingReader::new(
                source,
                bytes,
                (change == "provider").then_some(0),
                move |call| {
                    if call != 1 {
                        return;
                    }
                    if change == "runtime" {
                        fs::write(&runtime, b"{\"changed\":true}").unwrap();
                    } else if change != "provider" {
                        let db = Connection::open(&cache).unwrap();
                        if change == "source" {
                            db.execute("INSERT INTO session_catalog_invalidations(source_path,revision,reason,observed_at) VALUES(?1,1,'changed',0) ON CONFLICT(source_path) DO UPDATE SET revision=revision+1",[&path]).unwrap();
                        } else {
                            db.execute("UPDATE session_catalog SET metadata=metadata WHERE session_key='chosen'",[]).unwrap();
                        }
                    }
                },
            );
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            records.read_range(&repair.source, None, 0, 1).unwrap();
            assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            let result = fixture.store.original_lex_step(
                &fixture.job,
                &fixture.source,
                &repair,
                &records,
                &|| false,
            );
            if change == "provider" {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().outcome, Outcome::Conflict);
            }
            assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
            assert_eq!(
                provider
                    .bytes_read
                    .load(std::sync::atomic::Ordering::SeqCst),
                bytes.len() * 2
            );
            let remaining: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_events WHERE original=?1",
                    params![old.as_slice()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(remaining, 31);
        }
    }

    #[test]
    fn policy_restart_keeps_retained_provider_evidence_and_refuses_unadmitted_authority() {
        let bytes = b"{\"n\":1.25}\n";
        let mut fixture = Fixture::new(bytes);
        let source = reference(&fixture.sources.reader(), &fixture.source).unwrap();
        let old = format_two_namespace(&source).unwrap();
        seed_obsolete_policy(&mut fixture, old, 31);
        let mut repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        let denied =
            crate::query_source::Sources::capture(&fixture.options, std::iter::empty()).unwrap();
        assert!(
            fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &denied.reader(),
                    &|| false
                )
                .is_err()
        );
        let remaining: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_events WHERE original=?1",
                params![old.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 31);
        // Only this test-owned live path disappears. The independently retained
        // immutable provider bytes and the original identity/custody stay bound.
        fs::remove_file(&fixture.source.source.path).unwrap();
        let provider = InterleavingReader::new(source.clone(), bytes, None, |_| {});
        let reads = super::super::super::super::FILE_READS.with(Cell::get);
        while native_retirement::pending(&fixture.store.connection, repair.original).unwrap() {
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            let step = fixture
                .store
                .original_lex_step(&fixture.job, &fixture.source, &repair, &records, &|| false)
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            assert_eq!((step.work.native_bytes, step.work.range_requests), (0, 0));
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &records)
                .unwrap()
                .unwrap();
        }
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        while repair.cursor.phase != Phase::FoldRecord {
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            let step = fixture
                .store
                .original_lex_step(&fixture.job, &fixture.source, &repair, &records, &|| false)
                .unwrap();
            assert!(step.work.metadata_rows <= ROWS);
            assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &records)
                .unwrap()
                .unwrap();
        }
        loop {
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            let step = fixture
                .store
                .original_fields_step(&fixture.job, &fixture.source, &repair, &records, &|| false)
                .unwrap();
            assert!(step.work.metadata_rows <= ROWS);
            assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &records)
                .unwrap()
                .unwrap();
            if step.outcome == Outcome::Written {
                break;
            }
        }
        let root = fixture
            .store
            .original_root(&fixture.job, &fixture.source, &repair)
            .unwrap()
            .unwrap();
        let number_node = loop {
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            let step = fixture
                .store
                .original_member_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    root.address,
                    "n",
                    &records,
                    &|| false,
                )
                .unwrap();
            if step.complete {
                break step.node.unwrap();
            }
        };
        let number = fixture
            .store
            .original_number(&fixture.job, &fixture.source, &repair, number_node.address)
            .unwrap()
            .unwrap();
        assert_eq!(number.number().as_f64(), Some(1.25));
        assert_eq!(repair.source, source);
        assert_eq!(provider.bytes.as_ref(), bytes);
        assert!(provider.calls.load(std::sync::atomic::Ordering::SeqCst) > 0);
        assert!(check_source(&fixture.store.connection, &fixture.source).unwrap());
        assert!(job_current(&fixture.store.connection, &fixture.job).unwrap());
        assert_eq!(
            super::super::super::super::FILE_READS.with(Cell::get),
            reads
        );
    }

    #[test]
    fn original_large_record_resumes_without_source_index_load_or_body_residency() {
        let record = serde_json::json!({"type":"assistant","uuid":"one","message":{"role":"assistant","content":[{"type":"text","text":"native evidence ".repeat(100_000)}]}});
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.push(b'\n');
        let expected = crate::json_string::index_json_string_spans(&bytes).unwrap();
        let mut fixture = Fixture::new(&bytes);
        let index_reads = super::super::super::super::FILE_READS.with(Cell::get);
        let mut repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        let mut steps = 0;
        while repair.cursor.phase != Phase::FoldRecord {
            let result = fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert_eq!(result.outcome, Outcome::Deferred);
            assert!(result.work.range_requests <= 1);
            assert!(result.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(result.work.metadata_rows <= ROWS);
            assert!(result.work.metadata_bytes <= BYTES);
            let saved = serde_json::to_vec(&repair.cursor).unwrap();
            assert!(saved.len() < 4096);
            assert!(
                !String::from_utf8(saved)
                    .unwrap()
                    .contains("native evidence")
            );
            steps += 1;
            // A fresh storage connection and fresh proof reader resume each
            // committed step; no warm whole-history object is available.
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
        }
        assert!(steps > 1);
        let mut actual = Vec::new();
        let mut after = None;
        loop {
            let page = fixture
                .store
                .original_tokens_page(&fixture.job, &fixture.source, &repair, after)
                .unwrap();
            assert!(page.work.decoded_rows <= ROWS);
            for (_, token) in page.rows {
                if let Token::String {
                    span, key: false, ..
                } = token
                {
                    actual.push((span.start, span.end));
                }
            }
            if page.complete {
                break;
            }
            after = page.after;
        }
        assert_eq!(
            actual,
            expected
                .into_iter()
                .map(|span| (span.start, span.end))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            super::super::super::super::FILE_READS.with(Cell::get),
            index_reads
        );
        let mut field_steps = 0;
        loop {
            let result = fixture
                .store
                .original_fields_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert!(result.work.range_requests <= 1);
            assert!(result.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(result.work.metadata_rows <= ROWS);
            assert!(result.work.metadata_bytes <= BYTES);
            if result.outcome == Outcome::Written {
                break;
            }
            field_steps += 1;
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
        }
        assert!(field_steps > 1);
        let mut fields=fixture.store.connection.prepare("SELECT metadata,decoded_bytes,digest,cursor FROM native_original_fields ORDER BY ordinal").unwrap();
        let mut rows = fields.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let field: OriginalField =
                serde_json::from_str(&row.get::<_, String>(0).unwrap()).unwrap();
            let raw = &bytes[field.span.start as usize..field.span.end as usize];
            let mut quoted = Vec::with_capacity(raw.len() + 2);
            quoted.push(b'\"');
            quoted.extend_from_slice(raw);
            quoted.push(b'\"');
            let native: String = serde_json::from_slice(&quoted).unwrap();
            assert_eq!(row.get::<_, i64>(1).unwrap(), native.len() as i64);
            assert_eq!(
                row.get::<_, Vec<u8>>(2).unwrap(),
                Sha256::digest(native.as_bytes()).to_vec()
            );
            let cursor = row.get::<_, String>(3).unwrap();
            assert!(cursor.len() < 1024);
            assert!(!cursor.contains("native evidence"));
        }
        assert_eq!(
            super::super::super::super::FILE_READS.with(Cell::get),
            index_reads
        );
    }

    #[test]
    fn original_field_registration_cancel_rolls_back_descriptors_and_cursor() {
        let mut fixture =
            Fixture::new(b"{\"type\":\"assistant\",\"message\":{\"content\":\"text\"}}\n");
        let mut repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        while repair.cursor.phase != Phase::FoldRecord {
            fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
        }
        let calls = Cell::new(0);
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        let result = fixture
            .store
            .original_fields_step(
                &fixture.job,
                &fixture.source,
                &repair,
                &fixture.sources.reader(),
                &cancelled,
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Deferred);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_fields", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let resumed = fixture
            .store
            .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
            .unwrap()
            .unwrap();
        assert_eq!(resumed.revision, repair.revision);
        assert_eq!(resumed.cursor.fields_after, None);
    }

    #[test]
    fn original_record_view_keeps_duplicate_keys_and_pages_array_nodes() {
        for count in [0, 128, 1024, 8192] {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"{\"type\":\"user\",\"text\":\"first\",\"text\":\"last\",\"message\":{\"content\":[");
            for index in 0..count {
                if index > 0 {
                    bytes.push(b',')
                }
                bytes.extend_from_slice(b"{\"type\":\"text\",\"text\":\"body\"}");
            }
            bytes.extend_from_slice(b"]}}\n");
            let oracle: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(oracle["text"], "last");
            let mut fixture = Fixture::new(&bytes);
            let reads = super::super::super::super::FILE_READS.with(Cell::get);
            let mut repair = fixture
                .store
                .begin_original_repair(
                    &fixture.job,
                    &fixture.source,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            while repair.cursor.phase != Phase::FoldRecord {
                let step = fixture
                    .store
                    .original_lex_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
            }
            loop {
                let step = fixture
                    .store
                    .original_fields_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                if step.outcome == Outcome::Written {
                    break;
                }
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
            }
            let receipt = Receipt {
                decoded_bytes: 4,
                sha256: Sha256::digest(b"text").into(),
            };
            let candidates = fixture
                .store
                .original_field_candidates(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    Some(0),
                    true,
                    receipt,
                    None,
                )
                .unwrap();
            assert!(candidates.complete);
            assert_eq!(candidates.rows.len(), 2);
            assert!(candidates.rows[0].span.start > candidates.rows[1].span.start);
            for key in &candidates.rows {
                let raw = &bytes[key.span.start as usize..key.span.end as usize];
                let mut compare = crate::json_string::native_identity::Compare::new(
                    (0, raw.len() as u64),
                    (0, 4),
                )
                .unwrap();
                compare.step(raw, b"text", 64).unwrap();
                assert_eq!(
                    compare.result,
                    Some(crate::json_string::native_identity::Comparison::Equal)
                );
            }
            let node = fixture
                .store
                .original_value_by_key(&fixture.job, &fixture.source, &repair, &candidates.rows[0])
                .unwrap()
                .unwrap();
            let OriginalValue::String(span) = node.value else {
                panic!("native last text field must be a string")
            };
            assert_eq!(&bytes[span.start as usize..span.end as usize], b"last");
            let array:i64=fixture.store.connection.query_row("SELECT address FROM native_original_nodes WHERE json_extract(metadata,'$.value')='Array'",[],|row|row.get(0)).unwrap();
            let mut after = None;
            let mut observed = 0;
            loop {
                let page = fixture
                    .store
                    .original_array_page(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        array as u64,
                        after,
                    )
                    .unwrap();
                assert!(page.rows.len() <= ROWS);
                assert!(page.work.decoded_rows <= ROWS);
                assert!(page.work.metadata_bytes <= BYTES);
                for node in &page.rows {
                    assert!(matches!(node.value, OriginalValue::Object));
                }
                observed += page.rows.len();
                if page.complete {
                    break;
                }
                after = page.after;
            }
            assert_eq!(observed, count);
            assert_eq!(
                super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
        }
    }

    #[test]
    fn original_member_resumes_false_digest_candidate_and_selects_native_last_key() {
        let bytes = b"  {\"text\":\"first\",\"t\\u0065xt\":\"last\",\"fext\":\"collision\"}\n";
        let oracle: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(oracle["text"], "last");
        let mut fixture = Fixture::new(bytes);
        let mut repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        while repair.cursor.phase != Phase::FoldRecord {
            fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
        }
        loop {
            let step = fixture
                .store
                .original_fields_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            if step.outcome == Outcome::Written {
                break;
            }
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
        }
        let root = fixture
            .store
            .original_root(&fixture.job, &fixture.source, &repair)
            .unwrap()
            .unwrap();
        assert_eq!(root.address, 2);
        assert!(matches!(root.value, OriginalValue::Object));
        // Simulate a cryptographic candidate collision. The actual field
        // bytes still differ and must be rejected by the native comparison.
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_original_fields SET digest=?1 WHERE is_key=1 AND digest=?2",
                params![
                    Sha256::digest(b"text").as_slice(),
                    Sha256::digest(b"fext").as_slice()
                ],
            )
            .unwrap();
        let step = fixture
            .store
            .original_member_step(
                &fixture.job,
                &fixture.source,
                &repair,
                root.address,
                "text",
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap();
        assert_eq!(step.outcome, Outcome::Deferred);
        assert!(!step.complete);
        assert!(step.node.is_none());
        assert!(step.work.range_requests <= 1);
        assert!(step.work.metadata_rows <= ROWS);
        assert!(step.work.metadata_bytes <= BYTES);
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let step = fixture
            .store
            .original_member_step(
                &fixture.job,
                &fixture.source,
                &repair,
                root.address,
                "text",
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap();
        assert_eq!(step.outcome, Outcome::Written);
        assert!(step.complete);
        let OriginalValue::String(span) = step.node.unwrap().value else {
            panic!("last native member must be a string")
        };
        assert_eq!(&bytes[span.start as usize..span.end as usize], b"last");
        let cached = fixture
            .store
            .original_member_step(
                &fixture.job,
                &fixture.source,
                &repair,
                root.address,
                "text",
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap();
        assert!(cached.complete);
        assert_eq!(cached.work.range_requests, 1);
        let missing = fixture
            .store
            .original_member_step(
                &fixture.job,
                &fixture.source,
                &repair,
                root.address,
                "missing",
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap();
        assert!(missing.complete);
        assert!(missing.node.is_none());
        assert_eq!(missing.work.range_requests, 1);
    }

    #[test]
    fn original_ignored_numbers_and_recursion_keep_complete_native_gap_semantics() {
        let mut cases = vec![
            format!(
                "{{\"type\":\"assistant\",\"ignored\":1{}e-1000000}}\n",
                "0".repeat(1_000_000)
            ),
            format!(
                "{{\"type\":\"assistant\",\"ignored\":0.{}1}}\n",
                "0".repeat(1_000_000)
            ),
            "{\"type\":\"assistant\",\"ignored\":1e309}\n".into(),
            "{\"type\":\"assistant\",\"ignored\":1e-2147483648}\n".into(),
            "{\"type\":\"assistant\",\"ignored\":-0}\n".into(),
        ];
        for depth in [0, 126, 127, 128] {
            cases.push(format!(
                "{{\"type\":\"assistant\",\"ignored\":{}0{}}}\n",
                "[".repeat(depth),
                "]".repeat(depth)
            ));
        }
        for text in cases {
            let oracle = serde_json::from_str::<Value>(&text);
            let mut fixture = Fixture::new(text.as_bytes());
            let reads = super::super::super::super::FILE_READS.with(Cell::get);
            let mut repair = fixture
                .store
                .begin_original_repair(
                    &fixture.job,
                    &fixture.source,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            while repair.cursor.phase != Phase::FoldRecord {
                let step = fixture
                    .store
                    .original_lex_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
            }
            loop {
                let step = fixture
                    .store
                    .original_fields_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                if step.outcome == Outcome::Written {
                    break;
                }
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                repair = fixture
                    .store
                    .reload_original_repair(
                        &fixture.job,
                        &fixture.source,
                        &fixture.sources.reader(),
                    )
                    .unwrap()
                    .unwrap();
            }
            let status = record_status(&fixture.store.connection, &repair).unwrap();
            assert_eq!(status.as_deref() == Some("lexed"), oracle.is_ok());
            if let Ok(value) = oracle
                && value["ignored"].is_number()
            {
                let root = fixture
                    .store
                    .original_root(&fixture.job, &fixture.source, &repair)
                    .unwrap()
                    .unwrap();
                let selected = fixture
                    .store
                    .original_member_step(
                        &fixture.job,
                        &fixture.source,
                        &repair,
                        root.address,
                        "ignored",
                        &fixture.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(selected.complete);
                let node = selected.node.unwrap();
                let native = fixture
                    .store
                    .original_number(&fixture.job, &fixture.source, &repair, node.address)
                    .unwrap()
                    .unwrap()
                    .number();
                let expected = value["ignored"].as_number().unwrap();
                assert_eq!(native.as_i64(), expected.as_i64());
                assert_eq!(native.as_u64(), expected.as_u64());
                assert_eq!(
                    native.as_f64().map(f64::to_bits),
                    expected.as_f64().map(f64::to_bits)
                );
            }
            assert_eq!(
                super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
        }
    }

    #[test]
    fn original_cancel_before_commit_and_publication_change_leave_no_qualified_partial_record() {
        let mut fixture =
            Fixture::new(b"{\"type\":\"assistant\",\"message\":{\"content\":\"text\"}}\n");
        let repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        let calls = Cell::new(0);
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() == 4
        };
        let result = fixture
            .store
            .original_lex_step(
                &fixture.job,
                &fixture.source,
                &repair,
                &fixture.sources.reader(),
                &cancelled,
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Deferred);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_tokens", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let resumed = fixture
            .store
            .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
            .unwrap()
            .unwrap();
        assert_eq!(resumed.revision, repair.revision);
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_catalog SET metadata='{}' WHERE session_key='chosen'",
                [],
            )
            .unwrap();
        let result = fixture
            .store
            .original_lex_step(
                &fixture.job,
                &fixture.source,
                &repair,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Conflict);
        assert_eq!(result.work.range_requests, 0);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_records", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn malformed_record_cleanup_is_bounded_and_keeps_native_gap_explicit() {
        let mut record = String::from("[");
        for index in 0..1024 {
            if index != 0 {
                record.push(',');
            }
            record.push_str("\"text\"");
        }
        record.push_str(",]\n");
        let mut fixture = Fixture::new(record.as_bytes());
        let mut repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.source,
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        let mut cleanup_steps = 0;
        while repair.cursor.phase != Phase::FoldRecord {
            if repair.cursor.phase == Phase::ClearGap {
                cleanup_steps += 1;
            }
            let result = fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.source,
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert!(result.work.metadata_rows <= ROWS);
            assert!(result.work.metadata_bytes <= BYTES);
            repair = fixture
                .store
                .reload_original_repair(&fixture.job, &fixture.source, &fixture.sources.reader())
                .unwrap()
                .unwrap();
        }
        assert!(cleanup_steps > 1);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT status FROM native_original_records", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "gap"
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_tokens", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(!repair.cursor.phase.eq(&Phase::Complete));
    }
}
