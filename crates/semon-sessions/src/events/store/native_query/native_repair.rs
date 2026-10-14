//! Durable native Original record descriptors for rebuilding private Query facts.
//! This is derived metadata, not a migration of native logs or Trace history.
//! Only the Event Index schema owner installs these tables. Source bytes remain
//! in the authorized Records/provider path and never enter these tables.

use super::*;
use crate::SessionSourceReader;
use crate::json_string::native_identity::{Fingerprint, Receipt};
use crate::json_string::native_record::{Cursor as RecordCursor, Location, Span, Token};

pub(super) const SCHEMA: &str = "
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
}
#[derive(Clone, Debug)]
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
    digest.update(b"native-original-derived-v1\0");
    digest.update(super::super::super::CACHE_VERSION.to_be_bytes());
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
fn fields_ready(db: &Connection, repair: &Repair) -> rusqlite::Result<bool> {
    if !repair.cursor.fields_registered {
        return Ok(false);
    }
    db.query_row("SELECT NOT EXISTS(SELECT 1 FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND digest IS NULL)",params![repair.original.as_slice(),repair.cursor.record_at as i64],|row|row.get(0))
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
        if !repair.cursor.fields_registered {
            let page = self.original_tokens_page_limit(
                job,
                witness,
                repair,
                repair.cursor.fields_after,
                (ROWS - 1) / 2,
            )?;
            work.metadata_rows = page.work.decoded_rows;
            work.metadata_bytes = page.work.metadata_bytes;
            let mut fields = Vec::new();
            for (ordinal, token) in page.rows {
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
            let mut cursor = repair.cursor.clone();
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
            return Ok(RepairStep {
                outcome: Outcome::Written,
                work,
            });
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
            || end > repair.cursor.source_at - repair.cursor.record_at
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
        let mut cursor = repair.cursor.clone();
        cursor.record_at = cursor.source_at;
        cursor.ordinal = 0;
        cursor.record = RecordCursor::new();
        cursor.fields_after = None;
        cursor.fields_registered = false;
        cursor.phase = if cursor.source_at == repair.source.offset {
            Phase::Complete
        } else {
            Phase::Lexing
        };
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
fn seq_u64(value: u64) -> io::Result<i64> {
    i64::try_from(value).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fixture {
        root: PathBuf,
        options: crate::Options,
        store: NativeStore,
        source: SourceWitness,
        sources: crate::query_source::Sources,
        job: Job,
    }
    impl Fixture {
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
            .unwrap();
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
