//! Qualified class literal adapter for whole fields and every producer view.
use super::native_values::{Literal, Value, ValueMeter};
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    value: Value,
    literal: Literal,
}
fn same(left: &Value, right: &Value) -> io::Result<bool> {
    Ok(serde_json::to_vec(left).map_err(io::Error::other)?
        == serde_json::to_vec(right).map_err(io::Error::other)?)
}
fn result(
    outcome: Outcome,
    work: RepairWork,
    matches: Option<bool>,
) -> native_harness::LiteralStep {
    native_harness::LiteralStep {
        outcome,
        work,
        matches,
    }
}

// The caller owns related-owner/membership semantics. Always run its callback
// before the native seals: callbacks may themselves replace native evidence.
fn fences(
    db: &Connection,
    options: &crate::Options,
    job: &Job,
    witness: &SourceWitness,
    repair: &Repair,
    closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
) -> io::Result<bool> {
    Ok(closure(db, job.id).map_err(io::Error::other)?
        && crate::native_evidence::query_runtime_generation(options)? == job.spec.runtime
        && job_current(db, job).map_err(io::Error::other)?
        && check_source(db, witness).map_err(io::Error::other)?
        && repair_current(db, repair).map_err(io::Error::other)?
        && admitted_original(db, job, witness).map_err(io::Error::other)?
        && repair.cursor.phase == Phase::FoldRecord
        && fields_ready(db, repair).map_err(io::Error::other)?
        && record_status(db, repair)
            .map_err(io::Error::other)?
            .as_deref()
            == Some("lexed"))
}
impl NativeStore {
    pub(crate) fn original_text_literal_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        text: &OriginalText,
        literal: &'static str,
        records: &crate::query_source::Records<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<native_harness::LiteralStep> {
        self.original_text_literal_step_with_budget(
            job,
            witness,
            repair,
            text,
            literal,
            records,
            closure,
            cancelled,
            crate::SESSION_SOURCE_CHUNK_MAX,
            ROWS,
        )
    }
    pub(crate) fn original_text_literal_step_with_budget(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        text: &OriginalText,
        literal: &'static str,
        records: &crate::query_source::Records<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        native_allowance: usize,
        metadata_allowance: usize,
    ) -> io::Result<native_harness::LiteralStep> {
        if literal.len() > 128
            || literal
                .bytes()
                .any(|b| !(0x20..0x7f).contains(&b) || b == b'"' || b == b'\\')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native classification requires a bounded producer literal",
            ));
        }
        let mut work = RepairWork::default();
        let mut meter = ValueMeter::default();
        if cancelled() || native_allowance < 72 || metadata_allowance < 24 {
            return Ok(result(Outcome::Deferred, work, None));
        }
        if native_allowance > crate::SESSION_SOURCE_CHUNK_MAX || metadata_allowance > ROWS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native literal budget exceeds its owner",
            ));
        }
        if text.original() != repair.original
            || reference(records, witness)? != repair.source
            || !self.runtime_current(&job.spec)?
        {
            return Ok(result(Outcome::Conflict, work, None));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !fences(&tx, &self.options, job, witness, repair, closure)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        let value =
            native_class_values::resolve(&tx, repair, text.identity(), &mut work, &mut meter)?;
        let mut query=tx.prepare("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_class_literals WHERE original=?1 AND class=?2 AND literal=?3").map_err(io::Error::other)?;
        let old: Option<(i64, Option<String>, i64)> = query
            .query_row(
                params![
                    repair.original.as_slice(),
                    seq_u64(text.identity())?,
                    literal
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(io::Error::other)?;
        meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
        work.metadata_rows += 1;
        drop(query);
        let (revision, mut cursor) = if let Some((revision, text, size)) = old {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            let cursor: Cursor = serde_json::from_str(text.as_deref().ok_or_else(|| {
                io::Error::other("native class literal cursor exceeds residency")
            })?)
            .map_err(io::Error::other)?;
            if !same(&cursor.value, &value)? || !cursor.literal.valid(&value, literal)? {
                return Err(io::Error::other("native class literal binding changed"));
            }
            (Some(revision), cursor)
        } else {
            (
                None,
                Cursor {
                    value: value.clone(),
                    literal: Literal::new(&value, literal)?,
                },
            )
        };
        drop(tx);
        let cached = cursor.literal.result.is_some();
        if !cached {
            cursor.literal.step(
                &self.connection,
                repair,
                &value,
                literal,
                records,
                native_allowance,
                metadata_allowance.saturating_sub(work.metadata_rows + 12),
                &mut work,
                &mut meter,
            )?;
        }
        if work.range_requests == 0 {
            native_views::anchor_record(records, repair, value.record(), &mut work)?;
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
        if !fences(&tx, &self.options, job, witness, repair, closure)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        if !value.current(&tx, repair, &mut work, &mut meter)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        // Cached answers take this path too. The final callback precedes the
        // owned value/epoch and cursor-CAS checks, and never follows them.
        if !fences(&tx, &self.options, job, witness, repair, closure)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        let final_value =
            native_class_values::resolve(&tx, repair, text.identity(), &mut work, &mut meter)?;
        if !same(&value, &final_value)? {
            return Ok(result(Outcome::Conflict, work, None));
        }
        let mut query=tx.prepare("SELECT revision FROM native_original_class_literals WHERE original=?1 AND class=?2 AND literal=?3").map_err(io::Error::other)?;
        let current: Option<i64> = query
            .query_row(
                params![
                    repair.original.as_slice(),
                    seq_u64(text.identity())?,
                    literal
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
        drop(query);
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        if current != revision {
            return Ok(result(Outcome::Conflict, work, None));
        }
        if !cached || revision.is_none() {
            let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
            let mut query=tx.prepare("INSERT INTO native_original_class_literals VALUES(?1,?2,?3,0,?4) ON CONFLICT(original,class,literal) DO UPDATE SET revision=revision+1,cursor=excluded.cursor").map_err(io::Error::other)?;
            query
                .execute(params![
                    repair.original.as_slice(),
                    seq_u64(text.identity())?,
                    literal,
                    state
                ])
                .map_err(io::Error::other)?;
            meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
            work.metadata_rows += 1;
            work.metadata_bytes += state.len();
        }
        if work.native_bytes > native_allowance
            || work.metadata_rows > metadata_allowance
            || work.metadata_bytes > BYTES
        {
            return Err(io::Error::other(
                "native class literal exceeded bounded work",
            ));
        }
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(result(
            if cursor.literal.result.is_some() {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            cursor.literal.result,
        ))
    }
}
