//! Original record actions consume the existing qualified lexical descriptors.
//! Literal classification shares the native identity decoder/comparator; no
//! digest becomes a semantic tag, and no native String enters durable state.

use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiteralCursor {
    compare: Compare,
    result: Option<bool>,
}
#[derive(Clone, Debug)]
pub(crate) struct LiteralStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) matches: Option<bool>,
}
fn literal_result(outcome: Outcome, work: RepairWork, matches: Option<bool>) -> LiteralStep {
    LiteralStep {
        outcome,
        work,
        matches,
    }
}

impl NativeStore {
    /// Compare one qualified string field with an existing producer literal.
    /// One worker attempt performs at most one 128-KiB native range
    /// and persists only the existing content-free Compare cursor. The caller
    /// may advance a semantic action only after Written with Some(matches).
    pub(crate) fn original_literal_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        ordinal: u64,
        literal: &'static str,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<LiteralStep> {
        self.original_field_literal_step(
            job,
            witness,
            repair,
            repair.cursor.record_at,
            ordinal,
            literal,
            records,
            cancelled,
        )
    }

    /// Names on prior requests remain exact text classes in the same Original.
    /// Their qualified representative may belong to a previous folded record;
    /// its field address and native bytes never become a current-record ordinal.
    pub(crate) fn original_text_literal_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        text: &OriginalText,
        literal: &'static str,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<LiteralStep> {
        let representative = self.original_text_representative(job, witness, repair, text)?;
        if representative.original != repair.original || representative.source != repair.source {
            return Err(io::Error::other(
                "native literal text belongs to a different Original",
            ));
        }
        self.original_field_literal_step(
            job,
            witness,
            repair,
            representative.record,
            representative.field.ordinal,
            literal,
            records,
            cancelled,
        )
    }

    fn original_field_literal_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        field_record_at: u64,
        ordinal: u64,
        literal: &'static str,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<LiteralStep> {
        // This restricts caller-supplied parser literals, not supported source
        // strings. All source text lengths/escapes remain descriptor-backed.
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
        if cancelled() {
            return Ok(literal_result(Outcome::Deferred, work, None));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(literal_result(Outcome::Conflict, work, None));
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let existing: Option<(i64, Option<String>, i64)> = self.connection.query_row(
            "SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_literal WHERE original=?1 AND record_offset=?2 AND ordinal=?3 AND literal=?4",
            params![repair.original.as_slice(), seq_u64(field_record_at)?, seq_u64(ordinal)?, literal],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        let (revision, mut cursor) = if let Some((revision, text, size)) = existing {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            (
                Some(revision),
                serde_json::from_str::<LiteralCursor>(
                    text.as_deref().ok_or_else(|| {
                        io::Error::other("native literal cursor exceeds residency")
                    })?,
                )
                .map_err(io::Error::other)?,
            )
        } else {
            let row: (Option<String>, i64, Option<i64>, i64) = self.connection.query_row(
                "SELECT CASE WHEN octet_length(f.metadata)<=65536 THEN f.metadata END,octet_length(f.metadata),f.decoded_bytes,r.end_offset FROM native_original_fields f JOIN native_original_records r ON r.original=f.original AND r.record_offset=f.record_offset WHERE f.original=?1 AND f.record_offset=?2 AND f.ordinal=?3 AND f.digest IS NOT NULL AND r.status IN('lexed','folded')",
                params![repair.original.as_slice(), seq_u64(field_record_at)?, seq_u64(ordinal)?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += usize::try_from(row.1).map_err(io::Error::other)?;
            let field: OriginalField = serde_json::from_str(
                row.0
                    .as_deref()
                    .ok_or_else(|| io::Error::other("native literal field exceeds residency"))?,
            )
            .map_err(io::Error::other)?;
            if field.ordinal != ordinal
                || field_record_at
                    .checked_add(field.span.end)
                    .is_none_or(|at| at >= u64::try_from(row.3).unwrap_or(0))
                || u64::try_from(row.3).map_err(io::Error::other)? > repair.source.offset
            {
                return Err(io::Error::other(
                    "native literal field escaped Original record",
                ));
            }
            // A qualified decoded length can reject a candidate. A same-length
            // value still requires exact native byte comparison, even if its
            // stored hash was forced to the literal's hash by a test fixture.
            let length = row
                .2
                .ok_or_else(|| io::Error::other("native literal length is unqualified"))?;
            (
                None,
                LiteralCursor {
                    compare: Compare::new(
                        (field.span.start, field.span.end),
                        (0, literal.len() as u64),
                    )?,
                    result: (u64::try_from(length).map_err(io::Error::other)?
                        != literal.len() as u64)
                        .then_some(false),
                },
            )
        };
        if revision.is_some() && cursor.result.is_some() {
            verify_member_anchor(records, repair, &mut work)?;
            records.finish()?;
            if cancelled() {
                return Ok(literal_result(Outcome::Deferred, work, None));
            }
            if !self.runtime_current(&job.spec)? {
                return Ok(literal_result(Outcome::Conflict, work, None));
            }
            check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
            return Ok(literal_result(Outcome::Written, work, cursor.result));
        }
        if cursor.result.is_none() {
            let left = &cursor.compare.left;
            // Stream's end is authoritative; use the descriptor's exact span,
            // rather than the remaining record, to bound source callbacks.
            let field: (i64, i64) = self.connection.query_row(
                "SELECT start_offset,end_offset FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND ordinal=?3",
                params![repair.original.as_slice(), seq_u64(field_record_at)?, seq_u64(ordinal)?],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += 16;
            let end = u64::try_from(field.1).map_err(io::Error::other)?;
            if left.at < u64::try_from(field.0).map_err(io::Error::other)?
                || left.at > end
                || field_record_at
                    .checked_add(end)
                    .is_none_or(|at| at >= repair.source.offset)
            {
                return Err(io::Error::other("native literal comparison escaped field"));
            }
            let count = end
                .checked_sub(left.at)
                .ok_or_else(|| io::Error::other("native literal comparison exceeds field"))?;
            let mut native = None;
            if count > 0 {
                let at = field_record_at
                    .checked_add(left.at)
                    .ok_or_else(|| io::Error::other("native literal source offset overflow"))?;
                let maximum = usize::try_from(count.min(crate::SESSION_SOURCE_CHUNK_MAX as u64))
                    .map_err(io::Error::other)?;
                let pin = repair
                    .cursor
                    .pin
                    .as_ref()
                    .ok_or_else(|| io::Error::other("native literal lacks Original pin"))?;
                let part =
                    records.read_range(&repair.source, Some(&pin.generation), at, maximum)?;
                work.range_requests += 1;
                work.native_bytes += part.bytes.len();
                if part.offset != at
                    || part.bytes.is_empty()
                    || part.bytes.len() > maximum
                    || part.generation != pin.generation
                    || part.length != pin.length
                {
                    return Err(io::Error::other("native literal provider changed Original"));
                }
                native = Some(part);
            }
            let right = usize::try_from(cursor.compare.right.at).map_err(io::Error::other)?;
            cursor.compare.step(
                native.as_ref().map_or(&[], |part| part.bytes.as_ref()),
                literal
                    .as_bytes()
                    .get(right..)
                    .ok_or_else(|| io::Error::other("native literal cursor exceeds literal"))?,
                1024,
            )?;
            cursor.result = cursor
                .compare
                .result
                .map(|result| result == Comparison::Equal);
        }
        if work.range_requests == 0 {
            verify_member_anchor(records, repair, &mut work)?;
        }
        records.finish()?;
        if cancelled() {
            return Ok(literal_result(Outcome::Deferred, work, None));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(literal_result(Outcome::Conflict, work, None));
        }
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        work.metadata_rows += 2;
        work.metadata_bytes += state.len();
        if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
            return Err(io::Error::other(
                "native literal classification exceeds step work",
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let current: Option<i64> = tx.query_row("SELECT revision FROM native_original_literal WHERE original=?1 AND record_offset=?2 AND ordinal=?3 AND literal=?4", params![repair.original.as_slice(), seq_u64(field_record_at)?, seq_u64(ordinal)?, literal], |row| row.get(0)).optional().map_err(io::Error::other)?;
        if current != revision {
            return Ok(literal_result(Outcome::Conflict, work, None));
        }
        tx.execute("INSERT INTO native_original_literal VALUES(?1,?2,?3,?4,0,?5) ON CONFLICT(original,record_offset,ordinal,literal) DO UPDATE SET revision=revision+1,cursor=excluded.cursor", params![repair.original.as_slice(), seq_u64(field_record_at)?, seq_u64(ordinal)?, literal, state]).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(literal_result(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(literal_result(
            if cursor.result.is_some() {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            cursor.result,
        ))
    }
}
