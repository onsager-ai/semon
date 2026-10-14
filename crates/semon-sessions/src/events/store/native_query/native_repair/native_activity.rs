//! Native activity for every admitted record, including records without any
//! transcript event. One timestamp touches at most two indexed busy intervals.
use super::*;
use crate::events::native_harness::activity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OriginalActivityFacts {
    pub(crate) first: Option<i64>,
    pub(crate) last: Option<i64>,
}

impl NativeStore {
    pub(crate) fn original_activity_facts(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
    ) -> io::Result<(OriginalActivityFacts, Work)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native activity runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        native_source::source_ready(&tx, job, witness, repair)?;
        let mut query = tx
            .prepare("SELECT first_ms,last_ms FROM native_original_activity WHERE original=?1")
            .map_err(io::Error::other)?;
        let bounds: Option<(i64, i64)> = query
            .query_row(params![repair.original.as_slice()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        let work = Work {
            decoded_rows: usize::from(bounds.is_some()),
            metadata_bytes: usize::from(bounds.is_some()) * 16,
            vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        };
        Ok((
            OriginalActivityFacts {
                first: bounds.map(|b| b.0),
                last: bounds.map(|b| b.1),
            },
            work,
        ))
    }
    pub(crate) fn original_busy_page(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        after: Option<i64>,
        limit: usize,
    ) -> io::Result<Page<(i64, i64), i64>> {
        if limit == 0 || limit > ROWS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native busy page must be 1..=64",
            ));
        }
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("native activity runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        native_source::source_ready(&tx, job, witness, repair)?;
        let sql = if after.is_some() {
            "SELECT start_ms,end_ms FROM native_original_busy WHERE original=?1 AND start_ms>?2 ORDER BY start_ms LIMIT ?3"
        } else {
            "SELECT start_ms,end_ms FROM native_original_busy WHERE original=?1 AND start_ms>=?2 ORDER BY start_ms LIMIT ?3"
        };
        let mut query = tx.prepare(sql).map_err(io::Error::other)?;
        let rows = query
            .query_map(
                params![
                    repair.original.as_slice(),
                    after.unwrap_or(i64::MIN),
                    limit as i64
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(io::Error::other)?
            .collect::<rusqlite::Result<Vec<(i64, i64)>>>()
            .map_err(io::Error::other)?;
        let work = Work {
            decoded_rows: rows.len(),
            metadata_bytes: rows.len() * 16,
            vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        };
        let complete = rows.len() < limit;
        let after = rows.last().map(|row| row.0).or(after);
        Ok(Page {
            rows,
            after,
            complete,
            work,
        })
    }
    pub(crate) fn original_activity_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        time: Option<i64>,
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
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(RepairStep {
                outcome: Outcome::Conflict,
                work,
            });
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
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
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        if let Some(time) = time {
            let left:Option<(i64,i64)>=tx.query_row("SELECT start_ms,end_ms FROM native_original_busy WHERE original=?1 AND start_ms<=?2 ORDER BY start_ms DESC LIMIT 1",params![repair.original.as_slice(),time],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(io::Error::other)?;
            let right:Option<(i64,i64)>=tx.query_row("SELECT start_ms,end_ms FROM native_original_busy WHERE original=?1 AND start_ms>?2 ORDER BY start_ms LIMIT 1",params![repair.original.as_slice(),time],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(io::Error::other)?;
            work.metadata_rows += 2;
            work.metadata_bytes += 32;
            let change = activity::cluster(left, right, time);
            if change.changed {
                for (start, _) in change.left.into_iter().chain(change.right) {
                    tx.execute(
                        "DELETE FROM native_original_busy WHERE original=?1 AND start_ms=?2",
                        params![repair.original.as_slice(), start],
                    )
                    .map_err(io::Error::other)?;
                    work.metadata_rows += 1;
                    work.metadata_bytes += 8;
                }
                tx.execute(
                    "INSERT INTO native_original_busy VALUES(?1,?2,?3)",
                    params![
                        repair.original.as_slice(),
                        change.interval.0,
                        change.interval.1
                    ],
                )
                .map_err(io::Error::other)?;
                work.metadata_rows += 1;
                work.metadata_bytes += 16;
            }
            let bounds: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT first_ms,last_ms FROM native_original_activity WHERE original=?1",
                    params![repair.original.as_slice()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(io::Error::other)?;
            let (first, last) = activity::bounds(bounds.map(|b| b.0), bounds.map(|b| b.1), time);
            tx.execute("INSERT INTO native_original_activity VALUES(?1,?2,?3) ON CONFLICT(original) DO UPDATE SET first_ms=excluded.first_ms,last_ms=excluded.last_ms",params![repair.original.as_slice(),first,last]).map_err(io::Error::other)?;
            work.metadata_rows += 2;
            work.metadata_bytes += 32;
        }
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
}
