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
struct Cursor {
    before: Option<u64>,
    candidate: Option<Candidate>,
    compare: Option<Compare>,
    complete: bool,
    class: Option<u64>,
}
impl Cursor {
    fn new() -> Self {
        Self {
            before: None,
            candidate: None,
            compare: None,
            complete: false,
            class: None,
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct IdentityStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) text: Option<OriginalText>,
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

fn class_generation(db: &Connection, original: &[u8; 32]) -> io::Result<i64> {
    db.query_row(
        "SELECT coalesce(max(class),0) FROM native_original_classes WHERE original=?1",
        params![original.as_slice()],
        |row| row.get(0),
    )
    .map_err(io::Error::other)
}

fn field_at(
    db: &Connection,
    repair: &Repair,
    record: u64,
    ordinal: u64,
    work: &mut RepairWork,
) -> io::Result<(OriginalField, Receipt)> {
    let row:(Option<String>,i64,Option<Vec<u8>>,Option<i64>,i64)=db.query_row("SELECT CASE WHEN octet_length(f.metadata)<=65536 THEN f.metadata END,octet_length(f.metadata),CASE WHEN octet_length(f.digest)=32 THEN f.digest END,f.decoded_bytes,r.end_offset FROM native_original_fields f JOIN native_original_records r ON r.original=f.original AND r.record_offset=f.record_offset WHERE f.original=?1 AND f.record_offset=?2 AND f.ordinal=?3 AND r.status IN('lexed','folded')",params![repair.original.as_slice(),seq_u64(record)?,seq_u64(ordinal)?],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).map_err(io::Error::other)?;
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
) -> io::Result<Option<crate::SessionSourceRange>> {
    if at > end || end > repair.source.offset {
        return Err(io::Error::other(
            "native identity comparison escaped Original evidence",
        ));
    }
    if at == end {
        return Ok(None);
    }
    let maximum = usize::try_from((end - at).min(64 * 1024)).map_err(io::Error::other)?;
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
        let (record,ordinal):(i64,i64)=tx.query_row("SELECT record_offset,ordinal FROM native_original_classes WHERE original=?1 AND class=?2",params![repair.original.as_slice(),seq_u64(text.identity())?],|row|Ok((row.get(0)?,row.get(1)?))).map_err(io::Error::other)?;
        let record = u64::try_from(record).map_err(io::Error::other)?;
        let (field, receipt) = field_at(
            &tx,
            repair,
            record,
            u64::try_from(ordinal).map_err(io::Error::other)?,
            &mut RepairWork::default(),
        )?;
        Ok(Representative {
            original: repair.original,
            source: repair.source.clone(),
            class: text.identity(),
            record,
            field,
            receipt,
        })
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
        let mut work = RepairWork::default();
        if cancelled() {
            return Ok(IdentityStep {
                outcome: Outcome::Deferred,
                work,
                text: None,
            });
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(IdentityStep {
                outcome: Outcome::Conflict,
                work,
                text: None,
            });
        }
        check_record_view(&self.connection, job, witness, repair).map_err(io::Error::other)?;
        let generation = class_generation(&self.connection, &repair.original)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        let (field, receipt) = field_at(
            &self.connection,
            repair,
            repair.cursor.record_at,
            ordinal,
            &mut work,
        )?;
        let existing:Option<(i64,Option<String>,i64)>=self.connection.query_row("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_identities WHERE original=?1 AND record_offset=?2 AND ordinal=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(ordinal)?],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(io::Error::other)?;
        let (revision, mut cursor) =
            if let Some((revision, state, size)) = existing {
                work.metadata_rows += 1;
                work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
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
        let mut allocate = false;
        if !cursor.complete && cursor.candidate.is_none() {
            let candidate:Option<(i64,i64,i64)>=self.connection.query_row("SELECT class,record_offset,ordinal FROM native_original_classes WHERE original=?1 AND digest=?2 AND decoded_bytes=?3 AND class<?4 ORDER BY class DESC LIMIT 1",params![repair.original.as_slice(),receipt.sha256.as_slice(),seq_u64(receipt.decoded_bytes)?,cursor.before.map(seq_u64).transpose()?.unwrap_or(i64::MAX)],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += 24;
            if let Some((class, record, ordinal)) = candidate {
                let record = u64::try_from(record).map_err(io::Error::other)?;
                let (candidate, _) = field_at(
                    &self.connection,
                    repair,
                    record,
                    u64::try_from(ordinal).map_err(io::Error::other)?,
                    &mut work,
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
            } else {
                allocate = true;
                cursor.complete = true
            }
        }
        if let Some(compare) = cursor.compare.as_mut() {
            let candidate = cursor
                .candidate
                .as_ref()
                .ok_or_else(|| io::Error::other("native identity comparison lacks candidate"))?;
            let left = part(
                records,
                repair,
                compare.left.at,
                absolute(repair.cursor.record_at, &field)?.1,
                &mut work,
            )?;
            let right = part(
                records,
                repair,
                compare.right.at,
                absolute(candidate.record, &candidate.field)?.1,
                &mut work,
            )?;
            compare.step(
                left.as_ref().map_or(&[][..], |part| part.bytes.as_ref()),
                right.as_ref().map_or(&[][..], |part| part.bytes.as_ref()),
                32 * 1024,
            )?;
            if let Some(result) = compare.result {
                cursor.before = Some(candidate.class);
                if result == Comparison::Equal {
                    cursor.complete = true;
                    cursor.class = Some(candidate.class)
                }
                cursor.candidate = None;
                cursor.compare = None;
            }
        }
        if work.range_requests == 0 {
            verify_member_anchor(records, repair, &mut work)?
        }
        records.finish()?;
        if cancelled() {
            return Ok(IdentityStep {
                outcome: Outcome::Deferred,
                work,
                text: None,
            });
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(IdentityStep {
                outcome: Outcome::Conflict,
                work,
                text: None,
            });
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
        let current:Option<i64>=tx.query_row("SELECT revision FROM native_original_identities WHERE original=?1 AND record_offset=?2 AND ordinal=?3",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(ordinal)?],|row|row.get(0)).optional().map_err(io::Error::other)?;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        work.metadata_rows += 1;
        work.metadata_bytes += 8;
        if current != revision || class_generation(&tx, &repair.original)? != generation {
            return Ok(IdentityStep {
                outcome: Outcome::Conflict,
                work,
                text: None,
            });
        }
        if allocate {
            let class = generation
                .checked_add(1)
                .ok_or_else(|| io::Error::other("native identity class sequence overflow"))?;
            tx.execute(
                "INSERT INTO native_original_classes VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    repair.original.as_slice(),
                    class,
                    seq_u64(repair.cursor.record_at)?,
                    seq_u64(field.ordinal)?,
                    receipt.sha256.as_slice(),
                    seq_u64(receipt.decoded_bytes)?
                ],
            )
            .map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += 64;
            cursor.class = Some(u64::try_from(class).map_err(io::Error::other)?);
        }
        let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
        if !cached {
            tx.execute("INSERT INTO native_original_identities VALUES(?1,?2,?3,0,?4) ON CONFLICT(original,record_offset,ordinal) DO UPDATE SET cursor=excluded.cursor,revision=revision+1",params![repair.original.as_slice(),seq_u64(repair.cursor.record_at)?,seq_u64(ordinal)?,state]).map_err(io::Error::other)?;
            work.metadata_rows += 1;
            work.metadata_bytes += state.len();
        }
        if work.metadata_rows > ROWS
            || work.metadata_bytes > BYTES
            || work.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(io::Error::other(
                "native identity step exceeds bounded work",
            ));
        }
        let text = if cursor.complete {
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
            return Ok(IdentityStep {
                outcome: Outcome::Deferred,
                work,
                text: None,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(IdentityStep {
            outcome: if text.is_some() {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            text,
        })
    }
}
