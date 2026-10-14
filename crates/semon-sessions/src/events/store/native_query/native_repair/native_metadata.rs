//! Durable fixed Copilot metadata actions. Snapshot and body prerequisites
//! remain explicit, and no fixed-metadata result can seal source readiness.

use super::*;
use crate::events::native_harness::copilot_metadata as metadata;
use std::cell::RefCell;

#[derive(Debug)]
pub(crate) struct MetadataStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) complete: bool,
    pub(crate) snapshot_required: bool,
}
fn result(
    outcome: Outcome,
    work: RepairWork,
    cursor: Option<&metadata::Cursor<OriginalNode>>,
) -> MetadataStep {
    let result = cursor.and_then(|c| c.result());
    MetadataStep {
        outcome,
        work,
        complete: result.is_some(),
        snapshot_required: result == Some(true),
    }
}
impl NativeStore {
    /// Source-complete native header property. A missing row is qualified
    /// absence only after every source record's native producer seal.
    pub(crate) fn original_header_value(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        field: OriginalHeaderField,
    ) -> io::Result<(Option<OriginalHeaderValue>, Work)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native header runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        native_source::source_ready(&tx, job, witness, repair)?;
        let mut query=tx.prepare("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_header WHERE original=?1 AND field=?2").map_err(io::Error::other)?;
        let row: Option<(Option<String>, i64)> = query
            .query_row(params![repair.original.as_slice(), field.key()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        let bytes = row
            .as_ref()
            .map(|row| usize::try_from(row.1))
            .transpose()
            .map_err(io::Error::other)?
            .unwrap_or(0);
        let value: Option<OriginalHeaderValue> = row
            .map(|row| {
                serde_json::from_str(
                    row.0
                        .as_deref()
                        .ok_or_else(|| io::Error::other("native header exceeds residency"))?,
                )
                .map_err(io::Error::other)
            })
            .transpose()?;
        if let Some(crate::events::native_harness::header::Value::Native(text)) = &value
            && text.original() != repair.original
        {
            return Err(io::Error::other("native header reference changed Original"));
        }
        Ok((
            value,
            Work {
                decoded_rows: usize::from(bytes > 0),
                metadata_bytes: bytes,
                vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
            },
        ))
    }
    pub(crate) fn original_copilot_metadata_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        time: Option<i64>,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<MetadataStep> {
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
        let existing:Option<(i64,Option<String>,i64)>=self.connection.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_copilot_metadata WHERE original=?1 AND record_offset=?2",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, mut cursor) = if let Some((revision, state, size)) = existing {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            (
                Some(revision),
                serde_json::from_str::<metadata::Cursor<OriginalNode>>(
                    state.as_deref().ok_or_else(|| {
                        io::Error::other("native Copilot cursor exceeds residency")
                    })?,
                )
                .map_err(io::Error::other)?,
            )
        } else {
            (None, metadata::Cursor::new(repair.cursor.record_at, time))
        };
        if !cursor.valid() || cursor.offset != repair.cursor.record_at || cursor.time != time {
            return Err(io::Error::other(
                "native Copilot metadata changed record time",
            ));
        }
        let root = self
            .original_root(job, witness, repair)?
            .ok_or_else(|| io::Error::other("native Copilot metadata lacks a qualified record"))?;
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
            metadata::prepare(&mut record, &observation, &cursor)
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
        let current:Option<i64>=tx.query_row("SELECT revision FROM native_original_copilot_metadata WHERE original=?1 AND record_offset=?2",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|r|r.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        if current != revision {
            return Ok(result(Outcome::Conflict, work, None));
        }
        let mut events = native_events::Events::new(&tx, repair.original)?;
        metadata::apply(&mut events, &mut cursor, prepared)?;
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
                "native Copilot metadata exceeded its reserved action work",
            ));
        }
        tx.execute("INSERT INTO native_original_copilot_metadata VALUES(?1,?2,0,?3) ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1,cursor=excluded.cursor",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(result(Outcome::Written, work, Some(&cursor)))
    }
}
