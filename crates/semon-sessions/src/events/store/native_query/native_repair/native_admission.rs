//! Durable native Copilot admission. Complete admission only authorizes the
//! subsequent native metadata/body stages; it cannot publish source/Query Ready.

use super::*;
use crate::events::native_harness::Admission;
use crate::events::native_harness::copilot_admission as admission;
use std::cell::RefCell;

#[derive(Debug)]
pub(crate) struct AdmissionStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) admission: Option<Admission>,
    pub(crate) identity: Option<OriginalText>,
}
fn result(
    outcome: Outcome,
    work: RepairWork,
    cursor: Option<&admission::Cursor<OriginalNode, OriginalText>>,
) -> AdmissionStep {
    let completed = cursor.and_then(|cursor| cursor.result());
    AdmissionStep {
        outcome,
        work,
        admission: completed.map(|(admission, _)| admission),
        identity: completed.and_then(|(_, identity)| identity).cloned(),
    }
}
impl NativeStore {
    pub(crate) fn original_copilot_admission_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<AdmissionStep> {
        let mut work = RepairWork::default();
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let existing:Option<(i64,Option<String>,i64)>=self.connection.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_copilot_admission WHERE original=?1 AND record_offset=?2",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, mut cursor) = if let Some((revision, state, size)) = existing {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            (
                Some(revision),
                serde_json::from_str::<admission::Cursor<OriginalNode, OriginalText>>(
                    state.as_deref().ok_or_else(|| {
                        io::Error::other("native Copilot cursor exceeds residency")
                    })?,
                )
                .map_err(io::Error::other)?,
            )
        } else {
            (None, admission::Cursor::default())
        };
        let root = self
            .original_root(job, witness, repair)?
            .ok_or_else(|| io::Error::other("native Copilot admission lacks a qualified record"))?;
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
            let observation = native_record_source::Observation {
                store: &store,
                original: repair.original,
                work: &total,
            };
            admission::prepare(&mut record, &observation, &cursor)
        };
        let mut work = total.into_inner();
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(native_record_source::Error::Deferred) => {
                return Ok(result(Outcome::Deferred, work, None));
            }
            Err(native_record_source::Error::Conflict) => {
                return Ok(result(Outcome::Conflict, work, None));
            }
            Err(native_record_source::Error::Io(error)) => return Err(error),
        };
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
        let current:Option<i64>=tx.query_row("SELECT revision FROM native_original_copilot_admission WHERE original=?1 AND record_offset=?2",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|r|r.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        if current != revision {
            return Ok(result(Outcome::Conflict, work, None));
        }
        let mut events = native_events::Events::new(&tx, repair.original)?;
        admission::apply(&mut events, &mut cursor, prepared)?;
        let event_work = events.work();
        work.metadata_rows += event_work.metadata_rows;
        work.metadata_bytes += event_work.metadata_bytes;
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += state.len();
        if work.metadata_rows > ROWS
            || work.metadata_bytes > BYTES
            || work.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(io::Error::other(
                "native Copilot admission exceeded its reserved action work",
            ));
        }
        tx.execute("INSERT INTO native_original_copilot_admission VALUES(?1,?2,0,?3) ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1,cursor=excluded.cursor",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(result(Outcome::Written, work, Some(&cursor)))
    }
}
