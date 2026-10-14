//! Exact, durable native text equivalence classes for one immutable Original.
//! SHA/length only select candidates. A class joins two fields only after the
//! shared decoder compared all bytes and Records rechecked retained evidence.

use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    class: u64,
    record: u64,
    field: OriginalField,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchRange {
    /// Search only classes in `(after, through]`.
    after: u64,
    through: u64,
    /// Descending seek position within this range.
    #[serde(default)]
    before: Option<u64>,
    /// One coalesced contiguous range queued after this one.
    #[serde(default)]
    pending_through: Option<u64>,
}
impl SearchRange {
    fn new(after: u64, through: u64) -> Self {
        Self {
            after,
            through,
            before: None,
            pending_through: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    /// Descending cursor for the original class range. Newer appended ranges
    /// are searched separately, then this exact position is resumed.
    before: Option<u64>,
    candidate: Option<Candidate>,
    compare: Option<Compare>,
    complete: bool,
    class: Option<u64>,
    /// Boundary for the descending base range; `before` tracks its exact
    /// remaining prefix while appended classes live in `range`. Missing on the
    /// old format, so a default triggers bounded requalification only.
    #[serde(default)]
    high_water: Option<u64>,
    /// At most one active newer range and one coalesced pending high-water.
    #[serde(default)]
    range: Option<SearchRange>,
    /// An exact match found below a newer range is held until every appended
    /// candidate range above it has been excluded.
    #[serde(default)]
    match_class: Option<u64>,
}
impl Cursor {
    fn new() -> Self {
        Self {
            before: None,
            candidate: None,
            compare: None,
            complete: false,
            class: None,
            high_water: None,
            range: None,
            match_class: None,
        }
    }
}
#[derive(Debug)]
pub(crate) struct IdentityStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) text: Option<OriginalText>,
    /// VM steps from the class interner's explicit SQLite statements.
    pub(crate) class_sql_vm_steps: i32,
    /// Serialized cursor size, including the bounded exact comparison state.
    pub(crate) serialized_cursor_bytes: usize,
    /// Number of explicit source/runtime/publication and generation fence calls.
    pub(crate) fence_checks: usize,
    /// Records.finish rechecks touched source proofs before cursor commit.
    pub(crate) records_finish_calls: usize,
}

#[derive(Default)]
struct IdentityMeter {
    class_sql_vm_steps: i32,
    serialized_cursor_bytes: usize,
    fence_checks: usize,
    records_finish_calls: usize,
}
impl IdentityMeter {
    fn statement(&mut self, statement: &rusqlite::Statement<'_>) {
        self.class_sql_vm_steps += statement.get_status(rusqlite::StatementStatus::VmStep);
    }
    fn step(&self, outcome: Outcome, work: RepairWork, text: Option<OriginalText>) -> IdentityStep {
        IdentityStep {
            outcome,
            work,
            text,
            class_sql_vm_steps: self.class_sql_vm_steps,
            serialized_cursor_bytes: self.serialized_cursor_bytes,
            fence_checks: self.fence_checks,
            records_finish_calls: self.records_finish_calls,
        }
    }
}

/// Content-free representative. This metadata is not equality authority:
/// callers qualify both native readers, compare bytes and recheck every
/// contributing publication/source after I/O.
#[derive(Clone, Debug)]
pub(crate) struct Representative {
    pub(crate) original: [u8; 32],
    pub(crate) source: crate::SessionSourceRef,
    pub(crate) class: u64,
    pub(crate) record: u64,
    pub(crate) field: OriginalField,
    pub(crate) receipt: Receipt,
}

fn class_generation(
    db: &Connection,
    original: &[u8; 32],
    work: &mut RepairWork,
    meter: &mut IdentityMeter,
) -> io::Result<u64> {
    let mut query = db
        .prepare("SELECT coalesce(max(class),0) FROM native_original_classes WHERE original=?1")
        .map_err(io::Error::other)?;
    let result = query.query_row(params![original.as_slice()], |row| row.get::<_, i64>(0));
    meter.statement(&query);
    let generation = result.map_err(io::Error::other)?;
    work.metadata_rows += 1;
    work.metadata_bytes += 8;
    u64::try_from(generation).map_err(io::Error::other)
}

fn field_at(
    db: &Connection,
    repair: &Repair,
    record: u64,
    ordinal: u64,
    work: &mut RepairWork,
    meter: &mut IdentityMeter,
) -> io::Result<(OriginalField, Receipt)> {
    let mut query = db
        .prepare("SELECT CASE WHEN octet_length(f.metadata)<=65536 THEN f.metadata END,octet_length(f.metadata),CASE WHEN octet_length(f.digest)=32 THEN f.digest END,f.decoded_bytes,r.end_offset FROM native_original_fields f JOIN native_original_records r ON r.original=f.original AND r.record_offset=f.record_offset WHERE f.original=?1 AND f.record_offset=?2 AND f.ordinal=?3 AND r.status IN('lexed','folded')")
        .map_err(io::Error::other)?;
    let result = query.query_row(
        params![
            repair.original.as_slice(),
            seq_u64(record)?,
            seq_u64(ordinal)?
        ],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    );
    meter.statement(&query);
    let row: (Option<String>, i64, Option<Vec<u8>>, Option<i64>, i64) =
        result.map_err(io::Error::other)?;
    work.metadata_rows += 1;
    work.metadata_bytes += usize::try_from(row.1).map_err(io::Error::other)? + 48;
    let field: OriginalField = serde_json::from_str(
        row.0
            .as_deref()
            .ok_or_else(|| io::Error::other("native identity descriptor exceeds residency"))?,
    )
    .map_err(io::Error::other)?;
    let end = u64::try_from(row.4).map_err(io::Error::other)?;
    if field.ordinal != ordinal
        || field.span.start > field.span.end
        || record
            .checked_add(field.span.end)
            .is_none_or(|at| at >= end)
        || end > repair.source.offset
    {
        return Err(io::Error::other(
            "native identity field is outside its Original record",
        ));
    }
    let sha256 = row
        .2
        .ok_or_else(|| io::Error::other("native identity field is not fingerprint-qualified"))?
        .try_into()
        .map_err(|_| io::Error::other("native identity digest length disagrees"))?;
    let decoded_bytes = u64::try_from(
        row.3
            .ok_or_else(|| io::Error::other("native identity length is unqualified"))?,
    )
    .map_err(io::Error::other)?;
    Ok((
        field,
        Receipt {
            decoded_bytes,
            sha256,
        },
    ))
}

pub(super) fn qualified_field_at(
    db: &Connection,
    repair: &Repair,
    record: u64,
    ordinal: u64,
    work: &mut RepairWork,
) -> io::Result<(OriginalField, Receipt, i32)> {
    let mut meter = IdentityMeter::default();
    let (field, receipt) = field_at(db, repair, record, ordinal, work, &mut meter)?;
    Ok((field, receipt, meter.class_sql_vm_steps))
}

/// Load one exact-fingerprint candidate from a bounded class interval. The
/// descending indexed seek preserves the old latest-first behavior without a
/// catalog materialization, COUNT, digest-only equality, or whole scan.
fn candidate_in_range(
    db: &Connection,
    repair: &Repair,
    receipt: &Receipt,
    after: u64,
    through: u64,
    before: Option<u64>,
    work: &mut RepairWork,
    meter: &mut IdentityMeter,
) -> io::Result<Option<(i64, i64, i64)>> {
    let lower = seq_u64(after)?;
    let upper = seq_u64(through)?;
    let result = if let Some(before) = before {
        let mut query = db
            .prepare("SELECT class,record_offset,ordinal FROM native_original_classes WHERE original=?1 AND digest=?2 AND decoded_bytes=?3 AND class>?4 AND class<=?5 AND class<?6 ORDER BY class DESC LIMIT 1")
            .map_err(io::Error::other)?;
        let result = query
            .query_row(
                params![
                    repair.original.as_slice(),
                    receipt.sha256.as_slice(),
                    seq_u64(receipt.decoded_bytes)?,
                    lower,
                    upper,
                    seq_u64(before)?
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional();
        meter.statement(&query);
        result.map_err(io::Error::other)?
    } else {
        let mut query = db
            .prepare("SELECT class,record_offset,ordinal FROM native_original_classes WHERE original=?1 AND digest=?2 AND decoded_bytes=?3 AND class>?4 AND class<=?5 ORDER BY class DESC LIMIT 1")
            .map_err(io::Error::other)?;
        let result = query
            .query_row(
                params![
                    repair.original.as_slice(),
                    receipt.sha256.as_slice(),
                    seq_u64(receipt.decoded_bytes)?,
                    lower,
                    upper
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional();
        meter.statement(&query);
        result.map_err(io::Error::other)?
    };
    work.metadata_rows += 1;
    work.metadata_bytes += 24;
    Ok(result)
}

/// Migrate only incomplete legacy search state. The old before/candidate and
/// Compare remain numeric, exact cursors; newly appended classes get a bounded
/// first range before the old descending position resumes. serde(default) is
/// safe here because these fields describe search coverage only, not source,
/// identity, or equality authority.
fn observe_generation(cursor: &mut Cursor, generation: u64, is_new: bool) -> bool {
    if cursor.complete {
        return true;
    }
    if cursor.high_water.is_none() {
        if is_new {
            cursor.high_water = Some(generation);
        } else {
            let floor = cursor
                .before
                .or_else(|| cursor.candidate.as_ref().map(|candidate| candidate.class))
                .unwrap_or(0);
            cursor.high_water = Some(floor);
            if cursor.candidate.is_none() && cursor.before.is_none() {
                // Class zero is below every allocation and leaves the resumed
                // old interval empty after this full bounded restart.
                cursor.before = Some(0);
            }
            if generation > floor {
                cursor.range = Some(SearchRange::new(floor, generation));
            }
        }
    }
    let high_water = cursor.high_water.unwrap_or(generation);
    if generation < high_water {
        return false;
    }
    if cursor.before.is_some_and(|before| before > high_water) {
        return false;
    }
    if let Some(range) = cursor.range.as_mut() {
        if range.after != high_water
            || range.after >= range.through
            || range
                .before
                .is_some_and(|before| before <= range.after || before > range.through)
            || range
                .pending_through
                .is_some_and(|pending| pending <= range.through || pending > generation)
            || generation < range.through
        {
            return false;
        }
        if generation > range.through {
            range.pending_through = Some(
                range
                    .pending_through
                    .map_or(generation, |pending| pending.max(generation)),
            );
        }
    } else if generation > high_water {
        cursor.range = Some(SearchRange::new(high_water, generation));
    }
    true
}

fn absolute(record: u64, field: &OriginalField) -> io::Result<(u64, u64)> {
    Ok((
        record
            .checked_add(field.span.start)
            .ok_or_else(|| io::Error::other("native identity start overflow"))?,
        record
            .checked_add(field.span.end)
            .ok_or_else(|| io::Error::other("native identity end overflow"))?,
    ))
}

fn part(
    records: &crate::query_source::Records<'_>,
    repair: &Repair,
    at: u64,
    end: u64,
    work: &mut RepairWork,
    allowance: usize,
) -> io::Result<Option<crate::SessionSourceRange>> {
    if at > end || end > repair.source.offset {
        return Err(io::Error::other(
            "native identity comparison escaped Original evidence",
        ));
    }
    if at == end {
        return Ok(None);
    }
    let maximum = usize::try_from((end - at).min(64 * 1024))
        .map_err(io::Error::other)?
        .min(allowance);
    if maximum == 0 {
        return Ok(None);
    }
    let pin = repair
        .cursor
        .pin
        .as_ref()
        .ok_or_else(|| io::Error::other("native identity lacks consumed-prefix pin"))?;
    let part = records.read_range(&repair.source, Some(&pin.generation), at, maximum)?;
    work.range_requests += 1;
    work.native_bytes += part.bytes.len();
    if part.offset != at
        || part.bytes.is_empty()
        || part.bytes.len() > maximum
        || part.generation != pin.generation
        || part.length != pin.length
    {
        return Err(io::Error::other(
            "native identity Original provider changed",
        ));
    }
    Ok(Some(part))
}

/// Shared representative validation with explicit point-read work for consumers.
/// Source/publication/closure fences remain with the owning caller transaction.
pub(super) fn representative_at(
    db: &Connection,
    repair: &Repair,
    text: &OriginalText,
    work: &mut RepairWork,
) -> io::Result<(Representative, i32)> {
    if text.original() != repair.original {
        return Err(io::Error::other(
            "native text representative Original changed",
        ));
    }
    let mut query = db.prepare("SELECT record_offset,ordinal FROM native_original_classes WHERE original=?1 AND class=?2").map_err(io::Error::other)?;
    let (record, ordinal): (i64, i64) = query
        .query_row(
            params![repair.original.as_slice(), seq_u64(text.identity())?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(io::Error::other)?;
    work.metadata_rows += 1;
    work.metadata_bytes += 16;
    let mut meter = IdentityMeter::default();
    meter.statement(&query);
    let record = u64::try_from(record).map_err(io::Error::other)?;
    let (field, receipt) = field_at(
        db,
        repair,
        record,
        u64::try_from(ordinal).map_err(io::Error::other)?,
        work,
        &mut meter,
    )?;
    Ok((
        Representative {
            original: repair.original,
            source: repair.source.clone(),
            class: text.identity(),
            record,
            field,
            receipt,
        },
        meter.class_sql_vm_steps,
    ))
}

impl NativeStore {
    pub(crate) fn original_text_representative(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        text: &OriginalText,
    ) -> io::Result<Representative> {
        if text.original() != repair.original || !self.runtime_current(&job.spec)? {
            return Err(io::Error::other(
                "native text representative Original changed",
            ));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "native text representative dependencies changed",
            ));
        }
        representative_at(&tx, repair, text, &mut RepairWork::default()).map(|value| value.0)
    }

    /// The record reducer asks for one value/key's exact identity handle. A
    /// giant native field may need many calls, each persisting only cursors.
    /// The ordinal identifies retained evidence, never a digest-only claim.
    pub(crate) fn original_identity_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        ordinal: u64,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<IdentityStep> {
        self.original_identity_step_with_budget(
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

    /// The enclosing record action reserves its event/cursor writes and passes
    /// only remaining work. Both comparison inputs and empty/cached anchors
    /// share this allowance; qualification/coverage semantics are unchanged.
    pub(crate) fn original_identity_step_with_budget(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        ordinal: u64,
        records: &crate::query_source::Records<'_>,
        cancelled: &dyn Fn() -> bool,
        native_allowance: usize,
        metadata_rows_allowance: usize,
    ) -> io::Result<IdentityStep> {
        let native_allowance = native_allowance.min(crate::SESSION_SOURCE_CHUNK_MAX);
        let metadata_rows_allowance = metadata_rows_allowance.min(ROWS);
        let mut work = RepairWork::default();
        let mut meter = IdentityMeter::default();
        // At most seven descriptor/candidate reads and four seal writes occur
        // in one identity transition. Reserve their unchanged bounded envelope
        // before executing SQL or provider I/O, rather than rejecting after it.
        if cancelled() || native_allowance == 0 || metadata_rows_allowance < 12 {
            return Ok(meter.step(Outcome::Deferred, work, None));
        }
        meter.fence_checks += 1;
        if reference(records, witness)? != repair.source {
            return Ok(meter.step(Outcome::Conflict, work, None));
        }
        meter.fence_checks += 1;
        if !self.runtime_current(&job.spec)? {
            return Ok(meter.step(Outcome::Conflict, work, None));
        }
        meter.fence_checks += 1;
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let generation =
            class_generation(&self.connection, &repair.original, &mut work, &mut meter)?;
        let (field, receipt) = field_at(
            &self.connection,
            repair,
            repair.cursor.record_at,
            ordinal,
            &mut work,
            &mut meter,
        )?;
        let mut identity_query = self.connection.prepare("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_identities WHERE original=?1 AND record_offset=?2 AND ordinal=?3").map_err(io::Error::other)?;
        let existing_result = identity_query
            .query_row(
                params![
                    repair.original.as_slice(),
                    seq_u64(repair.cursor.record_at)?,
                    seq_u64(ordinal)?
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional();
        meter.statement(&identity_query);
        drop(identity_query);
        let existing: Option<(i64, Option<String>, i64)> =
            existing_result.map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        let is_new = existing.is_none();
        let (revision, mut cursor) =
            if let Some((revision, state, size)) = existing {
                let size = usize::try_from(size).map_err(io::Error::other)?;
                work.metadata_rows += 1;
                work.metadata_bytes += size;
                meter.serialized_cursor_bytes = size;
                (
                    Some(revision),
                    serde_json::from_str::<Cursor>(state.as_deref().ok_or_else(|| {
                        io::Error::other("native identity cursor exceeds residency")
                    })?)
                    .map_err(io::Error::other)?,
                )
            } else {
                (None, Cursor::new())
            };
        let cached = cursor.complete;
        // A completed experimental cursor predating durable coverage has no
        // proof that it exhausted appended classes. Keep it explicitly Pending
        // after the normal source reauthorization instead of promoting it.
        let legacy_complete = cached && cursor.high_water.is_none();
        if !observe_generation(&mut cursor, generation, is_new) {
            return Ok(meter.step(Outcome::Conflict, work, None));
        }
        let mut allocate = false;
        if !cursor.complete && cursor.candidate.is_none() && cursor.compare.is_none() {
            if cursor.range.is_none() && cursor.match_class.is_some() {
                cursor.class = cursor.match_class.take();
                cursor.complete = true;
            } else {
                let candidate = if let Some(range) = cursor.range.as_ref() {
                    candidate_in_range(
                        &self.connection,
                        repair,
                        &receipt,
                        range.after,
                        range.through,
                        range.before,
                        &mut work,
                        &mut meter,
                    )?
                } else if let Some(high_water) = cursor.high_water {
                    if high_water != generation {
                        return Ok(meter.step(Outcome::Conflict, work, None));
                    }
                    candidate_in_range(
                        &self.connection,
                        repair,
                        &receipt,
                        0,
                        high_water,
                        cursor.before,
                        &mut work,
                        &mut meter,
                    )?
                } else {
                    return Err(io::Error::other("native identity lacks class high-water"));
                };
                if let Some((class, record, candidate_ordinal)) = candidate {
                    let record = u64::try_from(record).map_err(io::Error::other)?;
                    let (candidate, _) = field_at(
                        &self.connection,
                        repair,
                        record,
                        u64::try_from(candidate_ordinal).map_err(io::Error::other)?,
                        &mut work,
                        &mut meter,
                    )?;
                    cursor.compare = Some(Compare::new(
                        absolute(repair.cursor.record_at, &field)?,
                        absolute(record, &candidate)?,
                    )?);
                    cursor.candidate = Some(Candidate {
                        class: u64::try_from(class).map_err(io::Error::other)?,
                        record,
                        field: candidate,
                    });
                } else if let Some(finished) = cursor.range.take() {
                    cursor.high_water = Some(finished.through);
                    if let Some(pending) = finished
                        .pending_through
                        .filter(|pending| *pending > finished.through)
                    {
                        cursor.range = Some(SearchRange::new(finished.through, pending));
                    }
                } else {
                    allocate = true;
                    cursor.complete = true;
                }
            }
        }
        if let Some(compare) = cursor.compare.as_mut() {
            let candidate = cursor
                .candidate
                .as_ref()
                .ok_or_else(|| io::Error::other("native identity comparison lacks candidate"))?;
            let in_newer_range = cursor.range.as_ref().is_some_and(|range| {
                candidate.class > range.after && candidate.class <= range.through
            });
            let left_end = absolute(repair.cursor.record_at, &field)?.1;
            let right_end = absolute(candidate.record, &candidate.field)?.1;
            let sides =
                usize::from(compare.left.at < left_end) + usize::from(compare.right.at < right_end);
            // A JSON scalar can occupy twelve native bytes (a surrogate pair).
            // Leave enough for each nonempty side; a small exhausted outer
            // step defers before I/O and retries with a fresh full allowance.
            if native_allowance < 12 * sides || work.metadata_rows + 4 > metadata_rows_allowance {
                return Ok(meter.step(Outcome::Deferred, work, None));
            }
            let left_allowance = if sides == 2 {
                native_allowance / 2
            } else {
                native_allowance
            };
            let left = part(
                records,
                repair,
                compare.left.at,
                left_end,
                &mut work,
                left_allowance,
            )?;
            let remaining = native_allowance.saturating_sub(work.native_bytes);
            let right = part(
                records,
                repair,
                compare.right.at,
                right_end,
                &mut work,
                remaining,
            )?;
            compare.step(
                left.as_ref().map_or(&[][..], |part| part.bytes.as_ref()),
                right.as_ref().map_or(&[][..], |part| part.bytes.as_ref()),
                32 * 1024,
            )?;
            if let Some(result) = compare.result {
                let candidate = cursor
                    .candidate
                    .take()
                    .ok_or_else(|| io::Error::other("native identity candidate disappeared"))?;
                cursor.compare = None;
                if result == Comparison::Equal {
                    if in_newer_range {
                        let finished = cursor
                            .range
                            .take()
                            .ok_or_else(|| io::Error::other("native identity range disappeared"))?;
                        cursor.high_water = Some(finished.through);
                        if let Some(pending) = finished
                            .pending_through
                            .filter(|pending| *pending > finished.through)
                        {
                            cursor.match_class = Some(candidate.class);
                            cursor.range = Some(SearchRange::new(finished.through, pending));
                        } else {
                            cursor.class = Some(candidate.class);
                            cursor.complete = true;
                            cursor.match_class = None;
                        }
                    } else if cursor.range.is_some() {
                        cursor.before = Some(candidate.class);
                        cursor.match_class = Some(candidate.class);
                    } else {
                        cursor.complete = true;
                        cursor.class = Some(candidate.class);
                    }
                } else if in_newer_range {
                    cursor
                        .range
                        .as_mut()
                        .ok_or_else(|| io::Error::other("native identity range disappeared"))?
                        .before = Some(candidate.class);
                } else {
                    cursor.before = Some(candidate.class);
                }
            }
        }
        if work.range_requests == 0 {
            if work.metadata_rows + 4 > metadata_rows_allowance {
                return Ok(meter.step(Outcome::Deferred, work, None));
            }
            meter.fence_checks += 1;
            verify_member_anchor(records, repair, &mut work)?
        }
        meter.records_finish_calls += 1;
        records.finish()?;
        if cancelled() {
            return Ok(meter.step(Outcome::Deferred, work, None));
        }
        meter.fence_checks += 1;
        if !self.runtime_current(&job.spec)? {
            return Ok(meter.step(Outcome::Conflict, work, None));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        meter.fence_checks += 1;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        meter.fence_checks += 2;
        let mut current_query = tx
            .prepare("SELECT revision FROM native_original_identities WHERE original=?1 AND record_offset=?2 AND ordinal=?3")
            .map_err(io::Error::other)?;
        let current_result = current_query
            .query_row(
                params![
                    repair.original.as_slice(),
                    seq_u64(repair.cursor.record_at)?,
                    seq_u64(ordinal)?
                ],
                |row| row.get(0),
            )
            .optional();
        meter.statement(&current_query);
        drop(current_query);
        let current: Option<i64> = current_result.map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        let current_generation = class_generation(&tx, &repair.original, &mut work, &mut meter)?;
        if current != revision || current_generation != generation {
            return Ok(meter.step(Outcome::Conflict, work, None));
        }
        if allocate {
            if cursor.range.is_some()
                || cursor.match_class.is_some()
                || cursor.high_water != Some(generation)
            {
                return Ok(meter.step(Outcome::Conflict, work, None));
            }
            let class = generation
                .checked_add(1)
                .ok_or_else(|| io::Error::other("native identity class sequence overflow"))?;
            let class_id = i64::try_from(class).map_err(io::Error::other)?;
            let mut insert = tx
                .prepare("INSERT INTO native_original_classes VALUES(?1,?2,?3,?4,?5,?6)")
                .map_err(io::Error::other)?;
            let result = insert.execute(params![
                repair.original.as_slice(),
                class_id,
                seq_u64(repair.cursor.record_at)?,
                seq_u64(field.ordinal)?,
                receipt.sha256.as_slice(),
                seq_u64(receipt.decoded_bytes)?
            ]);
            meter.statement(&insert);
            result.map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += 64;
            cursor.class = Some(class);
        }
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        meter.serialized_cursor_bytes = state.len();
        if !cached {
            let mut update = tx
                .prepare("INSERT INTO native_original_identities VALUES(?1,?2,?3,0,?4) ON CONFLICT(original,record_offset,ordinal) DO UPDATE SET cursor=excluded.cursor,revision=revision+1")
                .map_err(io::Error::other)?;
            let result = update.execute(params![
                repair.original.as_slice(),
                seq_u64(repair.cursor.record_at)?,
                seq_u64(ordinal)?,
                state
            ]);
            meter.statement(&update);
            result.map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += meter.serialized_cursor_bytes;
        }
        if work.metadata_rows > metadata_rows_allowance
            || work.metadata_bytes > BYTES
            || work.native_bytes > native_allowance
        {
            return Err(io::Error::other(
                "native identity step exceeds bounded work",
            ));
        }
        let text = if cursor.complete && !legacy_complete {
            let class = cursor
                .class
                .ok_or_else(|| io::Error::other("qualified identity lacks class"))?;
            if class == 0
                || class
                    > u64::try_from(generation).map_err(io::Error::other)? + u64::from(allocate)
            {
                return Err(io::Error::other(
                    "qualified identity class is outside its Original",
                ));
            }
            Some(OriginalText::qualified(repair.original, class))
        } else {
            None
        };
        if cancelled() {
            return Ok(meter.step(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(meter.step(
            if text.is_some() {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            text,
        ))
    }
}
