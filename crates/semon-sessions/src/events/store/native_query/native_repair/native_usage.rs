//! One qualified native Copilot modelMetrics report. Object key semantics and
//! snapshot replacement remain at their native publication owner.

use super::*;
use crate::events::native_harness::copilot_usage as usage;
use std::cell::RefCell;

#[derive(Debug)]
pub(crate) struct UsageRowStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) complete: bool,
    pub(crate) row: Option<usage::Row>,
}
fn result(
    outcome: Outcome,
    work: RepairWork,
    cursor: Option<&usage::Cursor<OriginalNode>>,
) -> UsageRowStep {
    let result = cursor.and_then(|c| c.result());
    UsageRowStep {
        outcome,
        work,
        complete: result.is_some(),
        row: result.flatten(),
    }
}
impl NativeStore {
    pub(crate) fn original_copilot_usage_row_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        report: &OriginalNode,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<UsageRowStep> {
        let mut work = RepairWork::default();
        if repair.source.root != "copilot" {
            return Ok(result(Outcome::Conflict, work, None));
        }
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let existing:Option<(i64,Option<String>,i64)>=self.connection.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_copilot_usage_row WHERE original=?1 AND record_offset=?2 AND report_address=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(report.address)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, mut cursor) = if let Some((revision, state, size)) = existing {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            (
                Some(revision),
                serde_json::from_str::<usage::Cursor<OriginalNode>>(state.as_deref().ok_or_else(
                    || io::Error::other("native Copilot usage row cursor exceeds residency"),
                )?)
                .map_err(io::Error::other)?,
            )
        } else {
            (None, usage::Cursor::default())
        };
        if !cursor.valid() {
            return Err(io::Error::other("native usage cursor phase is invalid"));
        }
        let root = self
            .original_node(job, witness, repair, report.address)?
            .ok_or_else(|| io::Error::other("native usage report lacks a qualified descriptor"))?;
        if json(&root).map_err(io::Error::other)? != json(report).map_err(io::Error::other)? {
            return Err(io::Error::other("native usage report changed Original"));
        }
        work.metadata_rows += 1;
        work.metadata_bytes += json(&root).map_err(io::Error::other)?.len();
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
            usage::step(&mut record, &cursor)
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
        let current:Option<i64>=tx.query_row("SELECT revision FROM native_original_copilot_usage_row WHERE original=?1 AND record_offset=?2 AND report_address=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(report.address)?],|r|r.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        if current != revision {
            return Ok(result(Outcome::Conflict, work, None));
        }
        cursor = prepared;
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += state.len();
        if work.metadata_rows > ROWS
            || work.metadata_bytes > BYTES
            || work.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(io::Error::other(
                "native Copilot usage row exceeded its reserved action work",
            ));
        }
        tx.execute("INSERT INTO native_original_copilot_usage_row VALUES(?1,?2,?3,0,?4) ON CONFLICT(original,record_offset,report_address) DO UPDATE SET revision=revision+1,cursor=excluded.cursor",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(report.address)?,state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(result(Outcome::Written, work, Some(&cursor)))
    }
}
