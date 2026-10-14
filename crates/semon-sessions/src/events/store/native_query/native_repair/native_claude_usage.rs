//! One bounded action of the native Claude message-usage producer. Report
//! replacement and UUID union commit with the phase cursor after all fences.
use super::*;
use crate::events::native_harness::claude_usage::{self as usage, Store};
use std::cell::RefCell;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    time: Option<i64>,
    cursor: usage::Cursor<OriginalNode, OriginalText>,
}
#[derive(Debug)]
pub(crate) struct ClaudeUsageStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) complete: bool,
}
fn result(outcome: Outcome, work: RepairWork, complete: bool) -> ClaudeUsageStep {
    ClaudeUsageStep {
        outcome,
        work,
        complete,
    }
}
impl NativeStore {
    pub(crate) fn original_claude_usage_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        time: Option<i64>,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<ClaudeUsageStep> {
        let mut work = RepairWork::default();
        if repair.source.root != "claude" {
            return Ok(result(Outcome::Conflict, work, false));
        }
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, false));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, false));
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let existing:Option<(i64,Option<String>,i64)>=self.connection.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_claude_usage_step WHERE original=?1 AND record_offset=?2",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, mut state) = match existing {
            Some((revision, cursor, size)) => {
                work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
                (
                    Some(revision),
                    serde_json::from_str::<State>(cursor.as_deref().ok_or_else(|| {
                        io::Error::other("native Claude usage cursor exceeds residency")
                    })?)
                    .map_err(io::Error::other)?,
                )
            }
            None => (
                None,
                State {
                    time,
                    cursor: usage::Cursor::default(),
                },
            ),
        };
        if state.time != time || !state.cursor.valid() {
            return Err(io::Error::other(
                "native Claude usage changed record time/phase",
            ));
        }
        let root = self
            .original_root(job, witness, repair)?
            .ok_or_else(|| io::Error::other("native Claude usage lacks qualified record"))?;
        work.metadata_rows += 2;
        work.metadata_bytes += json(&root).map_err(io::Error::other)?.len() + 8;
        let total = RefCell::new(work);
        let prepared = {
            let store = RefCell::new(&mut *self);
            let mut record = native_record_source::DescriptorRecord {
                store: &store,
                job,
                witness,
                repair,
                records,
                cancelled,
                root,
                work: &total,
                validated: RefCell::new(Default::default()),
            };
            usage::step(&mut record, &state.cursor, time)
        };
        let mut work = total.into_inner();
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(native_record_source::Error::Deferred) => {
                return Ok(result(Outcome::Deferred, work, false));
            }
            Err(native_record_source::Error::Conflict) => {
                return Ok(result(Outcome::Conflict, work, false));
            }
            Err(native_record_source::Error::Io(error)) => return Err(error),
        };
        if work.range_requests == 0 {
            verify_member_anchor(records, repair, &mut work)?;
        }
        records.finish()?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, false));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, false));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let current:Option<i64>=tx.query_row("SELECT revision FROM native_original_claude_usage_step WHERE original=?1 AND record_offset=?2",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|r|r.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        if current != revision {
            return Ok(result(Outcome::Conflict, work, false));
        }
        if state.cursor.result().is_none()
            && let Some(Some(report)) = prepared.result()
        {
            let mut events = native_events::Events::read_only(&tx, repair.original);
            events.claude_report(report.clone())?;
            let event_work = events.work();
            work.metadata_rows += event_work.metadata_rows;
            work.metadata_bytes += event_work.metadata_bytes;
        }
        state.cursor = prepared;
        let cursor = bounded_json(&state, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += cursor.len();
        if work.metadata_rows > ROWS
            || work.metadata_bytes > BYTES
            || work.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(io::Error::other(
                "native Claude usage exceeded reserved action work",
            ));
        }
        tx.execute("INSERT INTO native_original_claude_usage_step VALUES(?1,?2,0,?3) ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1,cursor=excluded.cursor",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,cursor]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, false));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(result(
            Outcome::Written,
            work,
            state.cursor.result().is_some(),
        ))
    }
}
