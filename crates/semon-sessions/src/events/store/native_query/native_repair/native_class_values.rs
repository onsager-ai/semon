//! Typed references in the existing class universe. Missing post-watermark rows
//! never fall back to an ordinal. Legacy whole classes retain their exact rows.
use super::native_values::{Value, ValueMeter};
use super::*;

pub(super) fn policy(
    db: &Connection,
    original: &[u8; 32],
    work: &mut RepairWork,
    meter: &mut ValueMeter,
) -> io::Result<Option<u64>> {
    let mut query = db
        .prepare("SELECT policy,legacy_through FROM native_original_text_policy WHERE original=?1")
        .map_err(io::Error::other)?;
    let row: Option<(i64, i64)> = query
        .query_row(params![original.as_slice()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(io::Error::other)?;
    meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += 1;
    work.metadata_bytes += 16;
    row.map(|(policy, through)| {
        if policy != 1 {
            return Err(io::Error::other("native class value policy changed"));
        }
        u64::try_from(through).map_err(io::Error::other)
    })
    .transpose()
}
/// Called only inside the interner's final immediate transaction, before any
/// new class allocation. Both whole and view writers share this exact seal.
pub(super) fn establish(
    db: &Connection,
    original: &[u8; 32],
    generation: u64,
    work: &mut RepairWork,
    meter: &mut ValueMeter,
) -> io::Result<()> {
    let mut query = db
        .prepare("INSERT OR IGNORE INTO native_original_text_policy VALUES(?1,1,?2,0)")
        .map_err(io::Error::other)?;
    query
        .execute(params![original.as_slice(), seq_u64(generation)?])
        .map_err(io::Error::other)?;
    meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += 1;
    work.metadata_bytes += 48;
    let watermark = policy(db, original, work, meter)?
        .ok_or_else(|| io::Error::other("native class policy missing after seal"))?;
    if watermark > generation {
        return Err(io::Error::other("native class policy exceeds generation"));
    }
    Ok(())
}
pub(super) fn insert(
    db: &Connection,
    original: &[u8; 32],
    class: u64,
    value: &Value,
    work: &mut RepairWork,
    meter: &mut ValueMeter,
) -> io::Result<()> {
    let mut query = db
        .prepare("INSERT INTO native_original_class_values VALUES(?1,?2,?3)")
        .map_err(io::Error::other)?;
    query
        .execute(params![
            original.as_slice(),
            seq_u64(class)?,
            value.view_id().map(seq_u64).transpose()?
        ])
        .map_err(io::Error::other)?;
    meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += 1;
    work.metadata_bytes += 48;
    Ok(())
}
pub(super) fn resolve(
    db: &Connection,
    repair: &Repair,
    class: u64,
    work: &mut RepairWork,
    meter: &mut ValueMeter,
) -> io::Result<Value> {
    resolve_with_backfill(db, repair, class, work, meter).map(|(value, _)| value)
}
pub(super) fn resolve_with_backfill(
    db: &Connection,
    repair: &Repair,
    class: u64,
    work: &mut RepairWork,
    meter: &mut ValueMeter,
) -> io::Result<(Value, bool)> {
    let mut query=db.prepare("SELECT c.record_offset,c.ordinal,CASE WHEN octet_length(c.digest)=32 THEN c.digest END,c.decoded_bytes,v.class,v.view_id,p.policy,p.legacy_through,p.backfilled_through FROM native_original_classes c LEFT JOIN native_original_class_values v ON v.original=c.original AND v.class=c.class LEFT JOIN native_original_text_policy p ON p.original=c.original WHERE c.original=?1 AND c.class=?2").map_err(io::Error::other)?;
    let (record, ordinal, digest, decoded, typed, view, policy, watermark, backfilled): (
        i64,
        i64,
        Vec<u8>,
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = query
        .query_row(
            params![repair.original.as_slice(), seq_u64(class)?],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .map_err(io::Error::other)?;
    meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += 1;
    work.metadata_bytes += 96;
    let record = u64::try_from(record).map_err(io::Error::other)?;
    let receipt = Receipt {
        sha256: digest
            .try_into()
            .map_err(|_| io::Error::other("native class digest width changed"))?,
        decoded_bytes: u64::try_from(decoded).map_err(io::Error::other)?,
    };
    if policy.is_some_and(|policy| policy != 1) || policy.is_some() != watermark.is_some() {
        return Err(io::Error::other("native class reference policy changed"));
    }
    if typed.is_some() && policy.is_none() {
        return Err(io::Error::other(
            "native typed class is missing its policy seal",
        ));
    }
    let watermark = watermark
        .map(u64::try_from)
        .transpose()
        .map_err(io::Error::other)?;
    if typed.is_none() && watermark.is_some_and(|through| class > through) {
        return Err(io::Error::other(
            "native class is missing its typed reference",
        ));
    }
    let value = if let Some(view) = view {
        if typed.is_none() || ordinal != -1 {
            return Err(io::Error::other("native view class legacy ordinal changed"));
        }
        let mut view_meter = native_views::ViewMeter::default();
        let value = native_views::qualified_view_at(
            db,
            repair,
            u64::try_from(view).map_err(io::Error::other)?,
            work,
            &mut view_meter,
        );
        meter.vm += view_meter.vm;
        Value::View(value?)
    } else {
        // A negative sentinel is never converted to a field ordinal.
        let ordinal = u64::try_from(ordinal)
            .map_err(|_| io::Error::other("native whole class has no valid ordinal"))?;
        let (field, proof, vm) =
            native_classes::qualified_field_at(db, repair, record, ordinal, work)?;
        meter.vm += vm;
        Value::Whole {
            record,
            field,
            receipt: proof,
        }
    };
    if value.record() != record || value.receipt() != receipt {
        return Err(io::Error::other(
            "native class reference differs from qualified evidence",
        ));
    }
    let needs_backfill = match (watermark, backfilled) {
        (None, None) => true,
        (Some(through), Some(after)) if after >= 0 && after as u64 <= through => {
            (after as u64) < through
        }
        _ => return Err(io::Error::other("native typed backfill cursor changed")),
    };
    Ok((value, needs_backfill))
}

/// Four existing legacy classes per seal, with a durable indexed high-water.
/// This copies typed references only; retained fields/logs/identity stay intact.
pub(super) fn backfill(
    db: &Connection,
    original: &[u8; 32],
    work: &mut RepairWork,
    meter: &mut ValueMeter,
) -> io::Result<()> {
    let mut query=db.prepare("SELECT policy,legacy_through,backfilled_through FROM native_original_text_policy WHERE original=?1").map_err(io::Error::other)?;
    let (policy, through, after): (i64, i64, i64) = query
        .query_row(params![original.as_slice()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(io::Error::other)?;
    meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += 1;
    work.metadata_bytes += 24;
    if policy != 1 || after < 0 || after > through {
        return Err(io::Error::other("native class backfill cursor changed"));
    }
    if after == through {
        return Ok(());
    }
    let mut query=db.prepare("SELECT class,ordinal FROM native_original_classes WHERE original=?1 AND class>?2 AND class<=?3 ORDER BY class LIMIT 4").map_err(io::Error::other)?;
    let entries: Vec<(i64, i64)> = query
        .query_map(params![original.as_slice(), after, through], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(io::Error::other)?
        .collect::<rusqlite::Result<_>>()
        .map_err(io::Error::other)?;
    meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += entries.len();
    work.metadata_bytes += entries.len() * 16;
    let mut last = through;
    for (class, ordinal) in &entries {
        if *ordinal < 0 {
            return Err(io::Error::other(
                "legacy native class backfill encountered view sentinel",
            ));
        }
        let mut insert = db
            .prepare("INSERT OR IGNORE INTO native_original_class_values VALUES(?1,?2,NULL)")
            .map_err(io::Error::other)?;
        insert
            .execute(params![original.as_slice(), class])
            .map_err(io::Error::other)?;
        meter.vm += insert.get_status(rusqlite::StatementStatus::VmStep);
        work.metadata_rows += 1;
        work.metadata_bytes += 48;
        last = *class;
    }
    let mut query = db
        .prepare("UPDATE native_original_text_policy SET backfilled_through=?2 WHERE original=?1")
        .map_err(io::Error::other)?;
    query
        .execute(params![original.as_slice(), last])
        .map_err(io::Error::other)?;
    meter.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += 1;
    work.metadata_bytes += 8;
    Ok(())
}
