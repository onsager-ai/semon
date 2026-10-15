//! Qualified Original event primitives. This exposes only fully folded source
//! metadata; source header/usage ownership remains a separate native witness.
//! Work measures the selected primitive statements (including all counts).
//! Job/source/publication fence SQL is the controller's relationship-work scope,
//! not hidden preparation of the selected source transcript.

use super::*;
use crate::events::native_event::NativeEvent;
use crate::model::native_stage::SourceTime;

#[derive(Clone, Debug)]
pub(crate) struct OriginalEventFacts {
    pub(crate) regular: usize,
    pub(crate) extras: usize,
    pub(crate) signals: usize,
    pub(crate) seed: TimeSeed,
}
pub(super) fn source_ready(
    db: &Connection,
    job: &Job,
    witness: &SourceWitness,
    repair: &Repair,
) -> io::Result<()> {
    if repair.cursor.phase != Phase::Complete
        || !job_current(db, job).map_err(io::Error::other)?
        || !check_source(db, witness).map_err(io::Error::other)?
        || !repair_current(db, repair).map_err(io::Error::other)?
        || !admitted_original(db, job, witness).map_err(io::Error::other)?
    {
        return Err(io::Error::other(
            "Original source event metadata is incomplete or changed",
        ));
    }
    Ok(())
}
fn time_at(
    db: &Connection,
    original: &[u8; 32],
    sequence: usize,
) -> io::Result<(SourceTime, Work)> {
    let mut query = db.prepare("SELECT time FROM native_original_positions WHERE original=?1 AND extra=0 AND time IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1").map_err(io::Error::other)?;
    let time: Option<i64> = query
        .query_row(
            params![original.as_slice(), seq_u64(sequence as u64)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    let work = Work {
        decoded_rows: usize::from(time.is_some()),
        metadata_bytes: usize::from(time.is_some()) * 8,
        vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
    };
    Ok((
        time.map_or(SourceTime::BeforeFirst, SourceTime::Observed),
        work,
    ))
}
fn time_before(
    db: &Connection,
    original: &[u8; 32],
    offset: u64,
) -> io::Result<(SourceTime, Work)> {
    let mut query = db.prepare("SELECT sequence FROM native_original_positions WHERE original=?1 AND extra=0 AND offset<=?2 ORDER BY offset DESC,sequence DESC LIMIT 1").map_err(io::Error::other)?;
    let sequence: Option<i64> = query
        .query_row(
            params![original.as_slice(), offset.to_be_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(io::Error::other)?;
    let mut work = Work {
        decoded_rows: usize::from(sequence.is_some()),
        metadata_bytes: usize::from(sequence.is_some()) * 8,
        vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
    };
    let sequence = usize::try_from(sequence.unwrap_or(0)).map_err(io::Error::other)?;
    let (time, read) = time_at(db, original, sequence)?;
    add_work(&mut work, read)?;
    Ok((time, work))
}
impl NativeStore {
    pub(crate) fn original_event_facts(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
    ) -> io::Result<(OriginalEventFacts, Work)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original source runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        source_ready(&tx, job, witness, repair)?;
        let mut regular_query=tx.prepare("SELECT coalesce((SELECT event_count FROM native_original_event_state WHERE original=?1),0)").map_err(io::Error::other)?;
        let regular: i64 = regular_query
            .query_row(params![repair.original.as_slice()], |row| row.get(0))
            .map_err(io::Error::other)?;
        let regular_vm = regular_query.get_status(rusqlite::StatementStatus::VmStep);
        let mut query=tx.prepare("SELECT channel,count FROM native_original_channel_state WHERE original=?1 ORDER BY channel LIMIT 2").map_err(io::Error::other)?;
        let mut extras = 0;
        let mut signals = 0;
        let mut work = Work {
            decoded_rows: 1,
            metadata_bytes: 8,
            vm_steps: regular_vm,
        };
        let mut rows = query
            .query(params![repair.original.as_slice()])
            .map_err(io::Error::other)?;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let channel: String = row.get(0).map_err(io::Error::other)?;
            let count = usize::try_from(row.get::<_, i64>(1).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            match channel.as_str() {
                "extras" => extras = count,
                "signals" => signals = count,
                _ => return Err(io::Error::other("unknown Original native channel")),
            }
            work.decoded_rows += 1;
            work.metadata_bytes += channel.len() + 8;
        }
        drop(rows);
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        let mut first=tx.prepare("SELECT sequence,time FROM native_original_positions WHERE original=?1 AND extra=0 AND time IS NOT NULL ORDER BY sequence LIMIT 1").map_err(io::Error::other)?;
        let seed: Option<(i64, i64)> = first
            .query_row(params![repair.original.as_slice()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += first.get_status(rusqlite::StatementStatus::VmStep);
        work.decoded_rows += usize::from(seed.is_some());
        work.metadata_bytes += usize::from(seed.is_some()) * 16;
        Ok((
            OriginalEventFacts {
                regular: usize::try_from(regular).map_err(io::Error::other)?,
                extras,
                signals,
                seed: TimeSeed {
                    first_seq: seed
                        .map(|seed| usize::try_from(seed.0))
                        .transpose()
                        .map_err(io::Error::other)?,
                    first_ms: seed.map(|seed| seed.1),
                },
            },
            work,
        ))
    }
    pub(crate) fn original_source_time_at(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        regular_seq: usize,
    ) -> io::Result<(SourceTime, Work)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original source runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        source_ready(&tx, job, witness, repair)?;
        time_at(&tx, &repair.original, regular_seq)
    }
    pub(crate) fn original_source_time_before_offset(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        offset: u64,
    ) -> io::Result<(SourceTime, Work)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original source runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        source_ready(&tx, job, witness, repair)?;
        time_before(&tx, &repair.original, offset)
    }
    pub(crate) fn original_last_operation(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        parent: usize,
    ) -> io::Result<(Option<(EventAddress, SourceTime, (u64, u32))>, Work)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original source runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        source_ready(&tx, job, witness, repair)?;
        let mut query=tx.prepare("SELECT extra,sequence,offset,block,time FROM native_original_positions WHERE original=?1 AND parent=?2 AND child=1 ORDER BY offset DESC,block DESC,extra DESC,sequence LIMIT 1").map_err(io::Error::other)?;
        let row: Option<(bool, i64, Vec<u8>, i64, Option<i64>)> = query
            .query_row(
                params![repair.original.as_slice(), seq_u64(parent as u64)?],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(io::Error::other)?;
        let mut work = Work {
            decoded_rows: usize::from(row.is_some()),
            metadata_bytes: usize::from(row.is_some()) * 40,
            vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        };
        let Some((extra, sequence, offset, block, time)) = row else {
            return Ok((None, work));
        };
        let offset = u64::from_be_bytes(
            offset
                .try_into()
                .map_err(|_| io::Error::other("Original offset shape changed"))?,
        );
        let time = match time {
            Some(time) => SourceTime::Observed(time),
            None => {
                let (time, read) = time_before(&tx, &repair.original, offset)?;
                add_work(&mut work, read)?;
                time
            }
        };
        Ok((
            Some((
                EventAddress {
                    extra,
                    seq: usize::try_from(sequence).map_err(io::Error::other)?,
                },
                time,
                (offset, u32::try_from(block).map_err(io::Error::other)?),
            )),
            work,
        ))
    }
    pub(crate) fn original_event_at(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        address: EventAddress,
    ) -> io::Result<(Option<NativeEvent<OriginalText>>, Work)> {
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original source runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        source_ready(&tx, job, witness, repair)?;
        if i64::try_from(address.seq).is_err() {
            return Ok((None, Work::default()));
        }
        let table = if address.extra {
            "native_original_extras"
        } else {
            "native_original_events"
        };
        let mut query=tx.prepare(&format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM {table} WHERE original=?1 AND sequence=?2")).map_err(io::Error::other)?;
        let row: Option<(Option<String>, i64)> = query
            .query_row(
                params![repair.original.as_slice(), seq_u64(address.seq as u64)?],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(io::Error::other)?;
        let work = Work {
            decoded_rows: usize::from(row.is_some()),
            metadata_bytes: row
                .as_ref()
                .map_or(0, |row| usize::try_from(row.1).unwrap_or(BYTES + 1)),
            vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        };
        let event = row
            .map(|(text, _)| {
                let event: NativeEvent<OriginalText> =
                    serde_json::from_str(text.as_deref().ok_or_else(|| {
                        io::Error::other("Original event exceeds descriptor residency")
                    })?)
                    .map_err(io::Error::other)?;
                if event
                    .text_refs()
                    .into_iter()
                    .flatten()
                    .any(|text| text.original() != repair.original)
                {
                    return Err(io::Error::other(
                        "Original event contains another source's text",
                    ));
                }
                Ok(event)
            })
            .transpose()?;
        Ok((event, work))
    }
    pub(crate) fn original_signals_page(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        after: Option<usize>,
        limit: usize,
    ) -> io::Result<Page<crate::events::Signal, usize>> {
        if limit == 0 || limit > ROWS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Original page limit must be 1..=64",
            ));
        }
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original source runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        source_ready(&tx, job, witness, repair)?;
        let mut query=tx.prepare("SELECT sequence,CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_signals WHERE original=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3").map_err(io::Error::other)?;
        let mut rows = query
            .query(params![
                repair.original.as_slice(),
                after
                    .map(|after| seq_u64(after as u64))
                    .transpose()?
                    .unwrap_or(-1),
                i64::try_from(limit).map_err(io::Error::other)?
            ])
            .map_err(io::Error::other)?;
        let mut values = Vec::new();
        let mut cursor = after;
        let mut work = Work::default();
        let mut stopped = false;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let sequence = usize::try_from(row.get::<_, i64>(0).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            let size = usize::try_from(row.get::<_, i64>(2).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if size > STATE_BYTES {
                return Err(io::Error::other(
                    "Original signal exceeds descriptor residency",
                ));
            }
            if work.metadata_bytes + size > BYTES {
                stopped = true;
                break;
            }
            let text: Option<String> = row.get(1).map_err(io::Error::other)?;
            let signal: crate::events::Signal =
                serde_json::from_str(text.as_deref().ok_or_else(|| {
                    io::Error::other("Original signal exceeds descriptor residency")
                })?)
                .map_err(io::Error::other)?;
            values.push(signal);
            cursor = Some(sequence);
            work.decoded_rows += 1;
            work.metadata_bytes += size;
        }
        drop(rows);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        Ok(Page {
            complete: !stopped && values.len() < limit,
            rows: values,
            after: cursor,
            work,
        })
    }
    pub(crate) fn original_events_page(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        extra: bool,
        after: Option<usize>,
        limit: usize,
    ) -> io::Result<Page<NativeEvent<OriginalText>, usize>> {
        if limit == 0 || limit > ROWS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Original page limit must be 1..=64",
            ));
        }
        if !self.runtime_current(&job.spec)? {
            return Err(io::Error::other("Original source runtime changed"));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        source_ready(&tx, job, witness, repair)?;
        let table = if extra {
            "native_original_extras"
        } else {
            "native_original_events"
        };
        let mut query=tx.prepare(&format!("SELECT sequence,CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM {table} WHERE original=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3")).map_err(io::Error::other)?;
        let mut rows = query
            .query(params![
                repair.original.as_slice(),
                after
                    .map(|after| seq_u64(after as u64))
                    .transpose()?
                    .unwrap_or(-1),
                i64::try_from(limit).map_err(io::Error::other)?
            ])
            .map_err(io::Error::other)?;
        let mut values = Vec::new();
        let mut cursor = after;
        let mut work = Work::default();
        let mut stopped = false;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let sequence = usize::try_from(row.get::<_, i64>(0).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            let size = usize::try_from(row.get::<_, i64>(2).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if size > STATE_BYTES {
                return Err(io::Error::other(
                    "Original event exceeds descriptor residency",
                ));
            }
            if work.metadata_bytes + size > BYTES {
                stopped = true;
                break;
            }
            let text: Option<String> = row.get(1).map_err(io::Error::other)?;
            let event: NativeEvent<OriginalText> =
                serde_json::from_str(text.as_deref().ok_or_else(|| {
                    io::Error::other("Original event exceeds descriptor residency")
                })?)
                .map_err(io::Error::other)?;
            if event
                .text_refs()
                .into_iter()
                .flatten()
                .any(|text| text.original() != repair.original)
            {
                return Err(io::Error::other(
                    "Original page contains another source's text",
                ));
            }
            values.push(event);
            cursor = Some(sequence);
            work.decoded_rows += 1;
            work.metadata_bytes += size;
        }
        drop(rows);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        Ok(Page {
            complete: !stopped && values.len() < limit,
            rows: values,
            after: cursor,
            work,
        })
    }
}
