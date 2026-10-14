//! Durable native record scheduling. Each invocation performs one existing
//! producer action; foreground readers never iterate this state machine.
//! Completion is a receipt of completed native actions, not a second reducer.

use super::*;
use crate::events::native_event::NativeEvent;
use crate::events::native_harness::Admission;

const CONTROL_BYTES: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Stage {
    Admission,
    Time {
        identity: OriginalText,
    },
    Activity {
        identity: OriginalText,
        time: Option<i64>,
    },
    Metadata {
        identity: OriginalText,
        time: Option<i64>,
    },
    Snapshot {
        identity: OriginalText,
        time: Option<i64>,
    },
    Body {
        identity: OriginalText,
        time: Option<i64>,
    },
    Gap,
    Seal,
}
impl Stage {
    fn valid(&self, original: [u8; 32]) -> bool {
        match self {
            Self::Time { identity }
            | Self::Activity { identity, .. }
            | Self::Metadata { identity, .. }
            | Self::Snapshot { identity, .. }
            | Self::Body { identity, .. } => identity.original() == original,
            _ => true,
        }
    }
}

#[derive(Debug)]
pub(crate) struct CopilotRecordStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) sealed: bool,
    /// A cumulative snapshot must finish at its native owner before this
    /// record can proceed. A body/header completion never bypasses it.
    pub(crate) needs_snapshot: bool,
}
fn result(outcome: Outcome, work: RepairWork) -> CopilotRecordStep {
    CopilotRecordStep {
        outcome,
        work,
        sealed: false,
        needs_snapshot: false,
    }
}
fn add(total: &mut RepairWork, work: RepairWork) {
    total.range_requests += work.range_requests;
    total.native_bytes += work.native_bytes;
    total.metadata_rows += work.metadata_rows;
    total.metadata_bytes += work.metadata_bytes;
    total.cleanup_vm_steps += work.cleanup_vm_steps;
}
fn bounded(work: &RepairWork) -> io::Result<()> {
    if work.metadata_rows > ROWS
        || work.metadata_bytes > BYTES
        || work.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX
    {
        return Err(io::Error::other(
            "native record controller exceeded reserved action work",
        ));
    }
    Ok(())
}

impl NativeStore {
    /// Only the immutable, admitted Original source root selects a native
    /// harness. This entry cannot reinterpret a Current or aggregate header.
    pub(crate) fn original_copilot_record_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CopilotRecordStep> {
        let mut work = RepairWork::default();
        if repair.source.root != "copilot"
            || reference(records, witness)? != repair.source
            || !self.runtime_current(&job.spec)?
        {
            return Ok(result(Outcome::Conflict, work));
        }
        if cancelled() {
            return Ok(result(Outcome::Deferred, work));
        }
        let status = record_status(&self.connection, repair).map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += status.as_ref().map_or(0, String::len);
        if status.as_deref() == Some("gap") {
            let step = self.original_gap_step(job, witness, repair, records, cancelled)?;
            let sealed = step.outcome == Outcome::Written;
            add(&mut work, step.work);
            bounded(&work)?;
            return Ok(CopilotRecordStep {
                outcome: step.outcome,
                work,
                sealed,
                needs_snapshot: false,
            });
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let row: Option<(i64, Option<String>, i64)> = self.connection.query_row(
            "SELECT revision,CASE WHEN octet_length(cursor)<=4096 THEN cursor END,octet_length(cursor) FROM native_original_harness WHERE original=?1 AND record_offset=?2",
            params![repair.original.as_slice(), seq_u64(repair.cursor.record_at)?],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, stage) = match row {
            Some((revision, cursor, size)) => {
                work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
                (
                    Some(revision),
                    serde_json::from_str::<Stage>(cursor.as_deref().ok_or_else(|| {
                        io::Error::other("native controller cursor exceeds residency")
                    })?)
                    .map_err(io::Error::other)?,
                )
            }
            None => (None, Stage::Admission),
        };
        if !stage.valid(repair.original) {
            return Err(io::Error::other(
                "native controller identity changed Original",
            ));
        }
        let next = match &stage {
            Stage::Admission => {
                let step =
                    self.original_copilot_admission_step(job, witness, repair, records, cancelled)?;
                add(&mut work, step.work);
                if step.outcome != Outcome::Written {
                    return Ok(result(step.outcome, work));
                }
                match step.admission {
                    None => return Ok(result(Outcome::Written, work)),
                    Some(Admission::Gap) => Stage::Gap,
                    Some(Admission::BeforeStart | Admission::Duplicate) => Stage::Seal,
                    Some(Admission::Accepted) => Stage::Time {
                        identity: step.identity.ok_or_else(|| {
                            io::Error::other("admitted native record lacks identity")
                        })?,
                    },
                }
            }
            Stage::Time { identity } => {
                let step =
                    self.original_record_time_step(job, witness, repair, records, cancelled)?;
                add(&mut work, step.work);
                if step.outcome != Outcome::Written || !step.complete {
                    return Ok(result(step.outcome, work));
                }
                Stage::Activity {
                    identity: identity.clone(),
                    time: step.millis,
                }
            }
            Stage::Activity { identity, time } => {
                let step =
                    self.original_activity_step(job, witness, repair, *time, records, cancelled)?;
                add(&mut work, step.work);
                if step.outcome != Outcome::Written {
                    return Ok(result(step.outcome, work));
                }
                Stage::Metadata {
                    identity: identity.clone(),
                    time: *time,
                }
            }
            Stage::Metadata { identity, time } => {
                let step = self.original_copilot_metadata_step(
                    job, witness, repair, *time, records, cancelled,
                )?;
                add(&mut work, step.work);
                if step.outcome != Outcome::Written || !step.complete {
                    return Ok(result(step.outcome, work));
                }
                if step.snapshot_required {
                    Stage::Snapshot {
                        identity: identity.clone(),
                        time: *time,
                    }
                } else {
                    Stage::Body {
                        identity: identity.clone(),
                        time: *time,
                    }
                }
            }
            Stage::Snapshot { .. } => {
                // The exact object-key iterator/snapshot owner must consume
                // this continuation. Never seal it as an empty snapshot.
                verify_member_anchor(records, repair, &mut work)?;
                records.finish()?;
                if !self.runtime_current(&job.spec)? {
                    return Ok(result(Outcome::Conflict, work));
                }
                check_record_view(&self.connection, job, witness, repair)
                    .map_err(io::Error::other)?;
                bounded(&work)?;
                let mut step = result(Outcome::Deferred, work);
                step.needs_snapshot = true;
                return Ok(step);
            }
            Stage::Body { identity, time } => {
                let base = NativeEvent {
                    o: repair.cursor.record_at,
                    t: *time,
                    u: Some(identity.clone()),
                    ..Default::default()
                };
                let step = self
                    .original_copilot_body_step(job, witness, repair, &base, records, cancelled)?;
                add(&mut work, step.work);
                if step.outcome != Outcome::Written || !step.complete {
                    return Ok(result(step.outcome, work));
                }
                Stage::Seal
            }
            Stage::Gap => Stage::Seal,
            Stage::Seal => {
                verify_member_anchor(records, repair, &mut work)?;
                records.finish()?;
                let read_work = std::cell::RefCell::new(RepairWork::default());
                let completed = |db: &Connection, original: [u8; 32], offset: u64| {
                    let cursor: Option<String> = db.query_row(
                        "SELECT CASE WHEN octet_length(cursor)<=4096 THEN cursor END FROM native_original_harness WHERE original=?1 AND record_offset=?2 AND revision=?3",
                        params![original.as_slice(), i64::try_from(offset).map_err(|_| rusqlite::Error::InvalidQuery)?, revision],
                        |row| row.get(0),
                    ).optional()?.flatten();
                    read_work.borrow_mut().metadata_rows += 1;
                    read_work.borrow_mut().metadata_bytes += cursor.as_ref().map_or(0, String::len);
                    Ok(cursor
                        .as_deref()
                        .and_then(|state| serde_json::from_str::<Stage>(state).ok())
                        .is_some_and(|stage| matches!(stage, Stage::Seal)))
                };
                let step = self.finish_original_record_with_work(
                    job, witness, repair, &completed, cancelled,
                )?;
                add(&mut work, read_work.into_inner());
                add(&mut work, step.work);
                bounded(&work)?;
                return Ok(CopilotRecordStep {
                    sealed: step.outcome == Outcome::Written,
                    outcome: step.outcome,
                    work,
                    needs_snapshot: false,
                });
            }
        };
        if !next.valid(repair.original) {
            return Err(io::Error::other(
                "native controller identity changed Original",
            ));
        }
        // Existing native primitives have already reauthorized their I/O. The
        // no-I/O Gap transition still needs its own Records anchor.
        if work.range_requests == 0 {
            verify_member_anchor(records, repair, &mut work)?;
        }
        records.finish()?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let current: Option<i64> = tx.query_row("SELECT revision FROM native_original_harness WHERE original=?1 AND record_offset=?2",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?],|row|row.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        if current != revision {
            return Ok(result(Outcome::Conflict, work));
        }
        if matches!(stage, Stage::Gap) {
            let mut events = native_events::Events::new(&tx, repair.original)?;
            crate::events::native_producer::push(
                &mut events,
                NativeEvent::<OriginalText> {
                    k: crate::events::Kind::Gap,
                    o: repair.cursor.record_at,
                    ..Default::default()
                },
            )?;
            add(&mut work, events.work());
        }
        let state = bounded_json(&next, CONTROL_BYTES).map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += state.len();
        bounded(&work)?;
        tx.execute("INSERT INTO native_original_harness VALUES(?1,?2,0,?3) ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1,cursor=excluded.cursor",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work));
        }
        tx.commit().map_err(io::Error::other)?;
        let mut step = result(Outcome::Written, work);
        step.needs_snapshot = matches!(next, Stage::Snapshot { .. });
        Ok(step)
    }
}
