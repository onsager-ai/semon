//! Bounded retirement of rebuildable Original metadata, owned by NativeRepair.
//! Namespace keys are derived from the exact admitted source; stored cursors
//! and markers never supply table names, source authority, or replacement facts.
use super::*;

const BATCH: usize = 30;
// Consumers precede their classes/lexical evidence and the repair root. The
// optional entries belong to this same private schema owner, but may not yet
// exist in a supported preceding private schema. No source-controlled names.
const TABLES: &[(&str, &str, bool)] = &[
    ("native_original_events", "sequence", false),
    ("native_original_pending", "identity", false),
    ("native_original_tool_ids", "identity,sequence", false),
    ("native_original_event_state", "original", false),
    ("native_original_extras", "sequence", true),
    ("native_original_signals", "sequence", true),
    ("native_original_channel_state", "channel", true),
    ("native_original_positions", "extra,sequence", true),
    ("native_original_copilot_started", "original", true),
    ("native_original_copilot_seen", "identity", true),
    ("native_original_harness", "record_offset", true),
    ("native_original_claude_usage_step", "record_offset", true),
    (
        "native_original_claude_usage_records",
        "identity,uuid",
        true,
    ),
    ("native_original_claude_usage", "identity", true),
    (
        "native_original_literal",
        "record_offset,ordinal,literal",
        true,
    ),
    ("native_original_timestamp", "record_offset,ordinal", true),
    ("native_original_copilot_body", "record_offset", true),
    ("native_original_tag", "record_offset,ordinal", true),
    ("native_original_copilot_admission", "record_offset", true),
    ("native_original_header", "field", true),
    ("native_original_activity", "original", true),
    ("native_original_busy", "start_ms", true),
    ("native_original_record_time", "record_offset", true),
    ("native_original_copilot_metadata", "record_offset", true),
    (
        "native_original_copilot_usage_row",
        "record_offset,report_address",
        true,
    ),
    (
        "native_original_text_boundaries",
        "view_id,decoded_at",
        true,
    ),
    ("native_original_class_literals", "class,literal", true),
    ("native_original_class_values", "class", true),
    ("native_original_view_identities", "view_id", true),
    ("native_original_text_parts", "view_id,page", true),
    ("native_original_text_views", "view_id", true),
    ("native_original_text_policy", "original", true),
    (
        "native_original_members",
        "record_offset,parent_offset,name",
        false,
    ),
    ("native_original_identities", "record_offset,ordinal", false),
    ("native_original_classes", "class", false),
    ("native_original_tokens", "record_offset,ordinal", false),
    ("native_original_fields", "record_offset,ordinal", false),
    ("native_original_text_epochs", "record_offset", true),
    ("native_original_nodes", "record_offset,address", false),
    ("native_original_numbers", "record_offset,address", false),
    ("native_original_records", "record_offset", false),
    ("native_original_cleanup", "original", false),
    ("native_original_repairs", "original", false),
];

pub(super) fn enqueue(
    db: &Connection,
    original: [u8; 32],
    source: &crate::SessionSourceRef,
) -> io::Result<()> {
    let (optional_names, exists, _) = optional_tables(db)?;
    let probes = TABLES
        .iter()
        .filter(|(name, _, optional)| {
            !optional
                || exists[optional_names
                    .iter()
                    .position(|value| value == name)
                    .unwrap()]
        })
        .map(|(name, _, _)| format!("SELECT 1 FROM {name} WHERE original=?1"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let has_rows = format!("SELECT EXISTS({probes})");
    for obsolete in [format_two_namespace(source)?, previous_namespace(source)?] {
        // Enqueue even when the obsolete repair root has already gone: an
        // interrupted old cleanup can leave dependent derived rows behind.
        // Completed markers remain durable, so repeated foreground interest
        // neither resets cleanup nor reparses already-qualified new metadata.
        if db
            .query_row(&has_rows, params![obsolete.as_slice()], |row| {
                row.get::<_, bool>(0)
            })
            .map_err(io::Error::other)?
            || native_compare::has_rows(db, obsolete.as_slice())?
        {
            db.execute(
                "INSERT OR IGNORE INTO native_original_retirements VALUES(?1,?2,0)",
                params![original.as_slice(), obsolete.as_slice()],
            )
            .map_err(io::Error::other)?;
        }
    }
    // Retain the preceding lexical worker's progress observation. The durable
    // queue is authoritative; this compatibility marker is never deserialized.
    let obsolete = previous_namespace(source)?;
    db.execute("INSERT OR IGNORE INTO native_original_cleanup SELECT ?1,?2,0 WHERE EXISTS(SELECT 1 FROM native_original_repairs WHERE original=?2)",params![original.as_slice(),obsolete.as_slice()]).map_err(io::Error::other)?;
    Ok(())
}

fn optional_tables(db: &Connection) -> io::Result<(Vec<&'static str>, Vec<bool>, i32)> {
    let names: Vec<_> = TABLES
        .iter()
        .filter_map(|(name, _, optional)| optional.then_some(*name))
        .collect();
    let sql = format!(
        "SELECT {}",
        names
            .iter()
            .map(|name| format!("EXISTS(SELECT 1 FROM pragma_table_info('{name}'))"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut query = db.prepare(&sql).map_err(io::Error::other)?;
    let exists: Vec<bool> = query
        .query_row([], |row| (0..names.len()).map(|i| row.get(i)).collect())
        .map_err(io::Error::other)?;
    Ok((
        names,
        exists,
        query.get_status(rusqlite::StatementStatus::VmStep),
    ))
}

pub(super) fn pending(db: &Connection, original: [u8; 32]) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_original_retirements WHERE original=?1 AND table_index<?2)",
        params![original.as_slice(), TABLES.len() as i64],
        |row| row.get(0),
    )
}

pub(super) fn advance(
    store: &mut NativeStore,
    job: &Job,
    witness: &SourceWitness,
    repair: &Repair,
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
    if reference(records, witness)? != repair.source {
        return Ok(RepairStep {
            outcome: Outcome::Conflict,
            work,
        });
    }
    // Cleanup produces no native content and does not request provider bytes.
    // Finish any evidence touched by the caller before opening the transaction;
    // all current source/publication/owner/runtime fences follow that I/O.
    records.finish()?;
    if !store.runtime_current(&job.spec)? {
        return Ok(RepairStep {
            outcome: Outcome::Conflict,
            work,
        });
    }
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io::Error::other)?;
    if !job_current(&tx, job).map_err(io::Error::other)?
        || !check_source(&tx, witness).map_err(io::Error::other)?
        || !repair_current(&tx, repair).map_err(io::Error::other)?
        || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
    {
        return Ok(RepairStep {
            outcome: Outcome::Conflict,
            work,
        });
    }
    let previous = previous_namespace(&repair.source)?;
    let old_two = format_two_namespace(&repair.source)?;
    let mut marker = tx.prepare("SELECT obsolete,table_index FROM native_original_retirements WHERE original=?1 AND table_index<?2 ORDER BY obsolete=?3,obsolete LIMIT 1").map_err(io::Error::other)?;
    let value: Option<(Vec<u8>, i64)> = marker
        .query_row(
            params![
                repair.original.as_slice(),
                TABLES.len() as i64,
                previous.as_slice()
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(io::Error::other)?;
    work.cleanup_vm_steps += marker.get_status(rusqlite::StatementStatus::VmStep);
    drop(marker);
    let Some((obsolete, at)) = value else {
        return Ok(RepairStep {
            outcome: Outcome::Written,
            work,
        });
    };
    work.metadata_rows += 1;
    work.metadata_bytes += obsolete.len() + 8;
    if (obsolete.as_slice() != previous.as_slice() && obsolete.as_slice() != old_two.as_slice())
        || obsolete.as_slice() == repair.original.as_slice()
        || at < 0
    {
        return Err(io::Error::other(
            "obsolete Original repair identity disagrees",
        ));
    }
    let mut index = usize::try_from(at).map_err(io::Error::other)?;
    // One fixed, content-free schema row keeps optional-table discovery bounded
    // and leaves room for 30 key reads/deletes inside the 64-row work limit.
    let (optional_names, exists, vm_steps) = optional_tables(&tx)?;
    work.cleanup_vm_steps += vm_steps;
    work.metadata_rows += 1;
    work.metadata_bytes += optional_names.len() * 8;
    // Pair receipts depend on either Original endpoint. Retire a bounded batch
    // through both endpoint indexes before discarding either side's classes or
    // lexical evidence. Committed deletions are the durable progress marker.
    if native_compare::retire(&tx, &obsolete, &mut work)? {
        if cancelled() {
            return Ok(RepairStep {
                outcome: Outcome::Deferred,
                work,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        return Ok(RepairStep {
            outcome: Outcome::Deferred,
            work,
        });
    }
    while index < TABLES.len() {
        let (table, columns, optional) = TABLES[index];
        if optional
            && !exists[optional_names
                .iter()
                .position(|name| *name == table)
                .expect("fixed optional retirement table")]
        {
            index += 1;
            continue;
        }
        let sql = format!(
            "SELECT {columns} FROM {table} WHERE original=?1 ORDER BY {columns} LIMIT {BATCH}"
        );
        let mut query = tx.prepare(&sql).map_err(io::Error::other)?;
        let width = columns.split(',').count();
        let mut rows = query
            .query(params![obsolete.as_slice()])
            .map_err(io::Error::other)?;
        let mut keys = Vec::new();
        let mut key_bytes = 0;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let mut key = Vec::with_capacity(width);
            for column in 0..width {
                let value = row.get_ref(column).map_err(io::Error::other)?;
                let bytes = match value {
                    rusqlite::types::ValueRef::Integer(_) => 8,
                    rusqlite::types::ValueRef::Text(bytes)
                    | rusqlite::types::ValueRef::Blob(bytes) => bytes.len(),
                    _ => return Err(io::Error::other("obsolete Original key has invalid type")),
                };
                if bytes > 128 || work.metadata_bytes + bytes * 2 > BYTES {
                    return Err(io::Error::other(
                        "obsolete Original key exceeds derived budget",
                    ));
                }
                // Selection and deletion each visit this bounded key; body and
                // cursor columns are never loaded, even for malformed old JSON.
                work.metadata_bytes += bytes * 2;
                key_bytes += bytes + std::mem::size_of::<rusqlite::types::Value>();
                key.push(rusqlite::types::Value::try_from(value).map_err(io::Error::other)?);
            }
            keys.push(key);
            work.metadata_rows += 1;
        }
        work.cleanup_key_bytes = work.cleanup_key_bytes.max(key_bytes);
        drop(rows);
        work.cleanup_vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        drop(query);
        if keys.is_empty() {
            index += 1;
            continue;
        }
        let predicates = columns
            .split(',')
            .enumerate()
            .map(|(i, column)| format!("{column}=?{}", i + 2))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut remove = tx
            .prepare(&format!(
                "DELETE FROM {table} WHERE original=?1 AND {predicates}"
            ))
            .map_err(io::Error::other)?;
        for key in &keys {
            let mut args = Vec::with_capacity(key.len() + 1);
            args.push(rusqlite::types::Value::Blob(obsolete.clone()));
            args.extend(key.iter().cloned());
            let argument_bytes: usize = args
                .iter()
                .map(|value| {
                    std::mem::size_of::<rusqlite::types::Value>()
                        + match value {
                            rusqlite::types::Value::Integer(_) => 8,
                            rusqlite::types::Value::Text(text) => text.len(),
                            rusqlite::types::Value::Blob(bytes) => bytes.len(),
                            _ => 0,
                        }
                })
                .sum();
            work.cleanup_key_bytes = work.cleanup_key_bytes.max(key_bytes + argument_bytes);
            remove
                .execute(rusqlite::params_from_iter(args))
                .map_err(io::Error::other)?;
            work.metadata_rows += 1;
        }
        work.cleanup_vm_steps += remove.get_status(rusqlite::StatementStatus::VmStep);
        drop(remove);
        if keys.len() < BATCH {
            index += 1;
        }
        // One nonempty table per durable step; remaining empty tables are cheap
        // but never multiply the selected keys or held native/metadata payloads.
        break;
    }
    let mut checkpoint = tx
        .prepare("UPDATE native_original_retirements SET table_index=?3 WHERE original=?1 AND obsolete=?2")
        .map_err(io::Error::other)?;
    checkpoint
        .execute(params![
            repair.original.as_slice(),
            obsolete.as_slice(),
            index as i64
        ])
        .map_err(io::Error::other)?;
    work.cleanup_vm_steps += checkpoint.get_status(rusqlite::StatementStatus::VmStep);
    drop(checkpoint);
    work.metadata_rows += 1;
    work.metadata_bytes += 72;
    if index == TABLES.len() && obsolete.as_slice() == previous.as_slice() {
        let mut compatibility = tx.prepare("UPDATE native_original_cleanup SET table_index=7 WHERE original=?1 AND obsolete=?2").map_err(io::Error::other)?;
        work.metadata_rows += compatibility
            .execute(params![repair.original.as_slice(), previous.as_slice()])
            .map_err(io::Error::other)?;
        work.cleanup_vm_steps += compatibility.get_status(rusqlite::StatementStatus::VmStep);
        work.metadata_bytes += 72;
    }
    if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
        return Err(io::Error::other(
            "Original retirement exceeds metadata work",
        ));
    }
    if cancelled() {
        return Ok(RepairStep {
            outcome: Outcome::Deferred,
            work,
        });
    }
    tx.commit().map_err(io::Error::other)?;
    Ok(RepairStep {
        outcome: Outcome::Deferred,
        work,
    })
}
