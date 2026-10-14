//! Native signal-tag classification resumes in the same derived metadata namespace as
//! its qualified record. Source/record/job/runtime proofs are rechecked after
//! all provider I/O, including cached Some/None results.

use super::*;
use crate::events::native_harness::tag::Cursor as Tag;
use crate::json_string::native_identity::Stream;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    stream: Stream,
    tag: Tag,
    end: u64,
    complete: bool,
    digest: [u8; 32],
}
#[derive(Debug)]
pub(crate) struct TagStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) complete: bool,
    pub(crate) tag: Option<String>,
}
fn result(outcome: Outcome, work: RepairWork, cursor: Option<&Cursor>) -> TagStep {
    TagStep {
        outcome,
        work,
        complete: cursor.is_some_and(|c| c.complete),
        tag: cursor
            .filter(|c| c.complete)
            .and_then(|c| c.tag.result(&c.digest)),
    }
}
impl NativeStore {
    pub(crate) fn original_tag_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        ordinal: u64,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<TagStep> {
        self.original_tag_step_with_budget(
            job,
            witness,
            repair,
            ordinal,
            records,
            cancelled,
            crate::SESSION_SOURCE_CHUNK_MAX,
            ROWS,
        )
    }
    pub(crate) fn original_tag_step_with_budget(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        ordinal: u64,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
        native_allowance: usize,
        metadata_rows_allowance: usize,
    ) -> io::Result<TagStep> {
        let mut work = RepairWork::default();
        let maximum = native_allowance.min(crate::SESSION_SOURCE_CHUNK_MAX);
        if cancelled() || maximum == 0 || metadata_rows_allowance < 4 {
            return Ok(result(Outcome::Deferred, work, None));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let existing: Option<(i64,Option<String>,i64)> = self.connection.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_tag WHERE original=?1 AND record_offset=?2 AND ordinal=?3", params![repair.original.as_slice(), seq_u64(repair.cursor.record_at)?,seq_u64(ordinal)?], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, mut cursor) = if let Some((revision, state, size)) = existing {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            (
                Some(revision),
                serde_json::from_str::<Cursor>(
                    state
                        .as_deref()
                        .ok_or_else(|| io::Error::other("native tag cursor exceeds residency"))?,
                )
                .map_err(io::Error::other)?,
            )
        } else {
            let row:(Option<String>,i64,i64,Option<Vec<u8>>)=self.connection.query_row("SELECT CASE WHEN octet_length(f.metadata)<=65536 THEN f.metadata END,octet_length(f.metadata),r.end_offset,CASE WHEN octet_length(f.digest)=32 THEN f.digest END FROM native_original_fields f JOIN native_original_records r ON r.original=f.original AND r.record_offset=f.record_offset WHERE f.original=?1 AND f.record_offset=?2 AND f.ordinal=?3 AND f.digest IS NOT NULL AND r.status IN('lexed','folded')",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(ordinal)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += usize::try_from(row.1).map_err(io::Error::other)? + 32;
            let field: OriginalField = serde_json::from_str(
                row.0
                    .as_deref()
                    .ok_or_else(|| io::Error::other("native tag field exceeds residency"))?,
            )
            .map_err(io::Error::other)?;
            let record_end = u64::try_from(row.2).map_err(io::Error::other)?;
            if field.ordinal != ordinal
                || field.span.start > field.span.end
                || repair
                    .cursor
                    .record_at
                    .checked_add(field.span.end)
                    .is_none_or(|at| at >= record_end)
                || record_end > repair.source.offset
            {
                return Err(io::Error::other("native tag field escaped Original"));
            }
            (
                None,
                Cursor {
                    stream: Stream::new(field.span.start, field.span.end)?,
                    tag: Tag::default(),
                    end: field.span.end,
                    complete: false,
                    digest: row
                        .3
                        .ok_or_else(|| io::Error::other("native tag digest is unqualified"))?
                        .try_into()
                        .map_err(|_| io::Error::other("native tag digest changed"))?,
                },
            )
        };
        let cached = cursor.complete;
        if !cursor.complete {
            let remaining = cursor
                .end
                .checked_sub(cursor.stream.at)
                .ok_or_else(|| io::Error::other("native tag escaped field"))?;
            if remaining != 0 {
                if maximum < 12 {
                    return Ok(result(Outcome::Deferred, work, None));
                }
                let at = repair
                    .cursor
                    .record_at
                    .checked_add(cursor.stream.at)
                    .ok_or_else(|| io::Error::other("native tag offset overflow"))?;
                let count =
                    usize::try_from(remaining.min(maximum as u64)).map_err(io::Error::other)?;
                let pin = repair
                    .cursor
                    .pin
                    .as_ref()
                    .ok_or_else(|| io::Error::other("native tag lacks Original pin"))?;
                let part = records.read_range(&repair.source, Some(&pin.generation), at, count)?;
                work.range_requests += 1;
                work.native_bytes += part.bytes.len();
                if part.offset != at
                    || part.bytes.is_empty()
                    || part.bytes.len() > count
                    || part.generation != pin.generation
                    || part.length != pin.length
                {
                    return Err(io::Error::other("native tag provider changed Original"));
                }
                let decoded = cursor.stream.step(&part.bytes, 32 * 1024)?;
                cursor.tag.push(&decoded.bytes);
            }
            cursor.complete = cursor.stream.complete() || cursor.tag.rejected();
        }
        if work.range_requests == 0 {
            verify_member_anchor(records, repair, &mut work)?;
        }
        records.finish()?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let current:Option<i64>=tx.query_row("SELECT revision FROM native_original_tag WHERE original=?1 AND record_offset=?2 AND ordinal=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(ordinal)?],|r|r.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        if current != revision {
            return Ok(result(Outcome::Conflict, work, None));
        }
        if !cached {
            let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += state.len();
            tx.execute("INSERT INTO native_original_tag VALUES(?1,?2,?3,0,?4) ON CONFLICT(original,record_offset,ordinal) DO UPDATE SET revision=revision+1,cursor=excluded.cursor",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(ordinal)?,state]).map_err(io::Error::other)?;
        }
        if work.native_bytes > maximum
            || work.metadata_rows > metadata_rows_allowance
            || work.metadata_bytes > BYTES
        {
            return Err(io::Error::other("native tag exceeded reserved work"));
        }
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(result(
            if cursor.complete {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            Some(&cursor),
        ))
    }
}
