//! Receipt-bound repair of a missing current tree projection from verified source
//! ranges. Native usage facts are never converted or replaced by this worker.
use super::*;
use std::io::{BufRead, BufReader, Read};

const RECORDS: usize = 512;
const BYTES: u64 = 1024 * 1024;
const RECORD_MAX: u64 = 32 * 1024 * 1024;

pub(in crate::events::store) fn pending(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_tree_repairs WHERE state IN ('ready','running'))",
        [],
        |r| r.get(0),
    )
}

struct Verified {
    file: crate::sealed::LogFile,
    proof: chunk_proofs::Focused,
    position: u64,
    consumed: u64,
    block: Vec<u8>,
    block_index: Option<usize>,
}
impl Read for Verified {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.position == self.consumed || output.is_empty() {
            return Ok(0);
        }
        let index = (self.position / crate::source_chunks::SIZE as u64) as usize;
        if self.block_index != Some(index) {
            self.block = self.proof.read(&mut self.file, index)?;
            self.block_index = Some(index);
        }
        let start = (self.position % crate::source_chunks::SIZE as u64) as usize;
        let n = output.len().min(self.block.len() - start);
        output[..n].copy_from_slice(&self.block[start..start + n]);
        self.position += n as u64;
        Ok(n)
    }
}

#[derive(Default)]
struct Batch {
    index: FileIndex,
    cursor: u64,
    rows: usize,
    bytes: u64,
}

fn prepare(store: &SqliteStore, path: &str, ledger: &Ledger, resume: Resume) -> io::Result<Batch> {
    let Resume {
        cursor,
        model,
        tree,
        projection_id,
        harness,
    } = resume;
    let sql_error = io::Error::other;
    let mut batch = Batch {
        cursor,
        index: FileIndex {
            native_usage_observations: Some(Vec::new()),
            last_model: model,
            tree_summary: Some(
                tree.map(|encoded| from_json(0, &encoded))
                    .transpose()
                    .map_err(sql_error)?
                    .unwrap_or_default(),
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    tree_state::restore_labels(
        &store.connection,
        projection_id,
        batch.index.tree_summary.as_mut().expect("tree initialized"),
    )
    .map_err(sql_error)?;
    if harness == "claude"
        && batch
            .index
            .tree_summary
            .as_ref()
            .is_some_and(|tree| tree.usage_total.is_none())
    {
        batch
            .index
            .tree_summary
            .as_mut()
            .expect("tree initialized")
            .usage_total = Some(Tokens::default());
    }
    let file = crate::sealed::LogFile::open(Path::new(path))?;
    if Stat::from_metadata(&file.metadata()?)? != ledger.stat {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    let db = Connection::open_with_flags(&store.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(sql_error)?;
    let proof =
        chunk_proofs::Focused::with_db(chunk_proofs::Snapshot::new(db), &store.path, path, ledger)?;
    let mut reader = BufReader::new(Verified {
        file,
        proof,
        position: cursor,
        consumed: ledger.offset,
        block: Vec::new(),
        block_index: None,
    });
    while batch.cursor < ledger.offset && batch.rows < RECORDS && batch.bytes < BYTES {
        crate::selected_read::checkpoint("native_tree_repair")?;
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(RECORD_MAX + 1)
            .read_until(b'\n', &mut bytes)?;
        if bytes.len() as u64 > RECORD_MAX || !bytes.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native usage repair record budget",
            ));
        }
        let record = serde_json::from_slice::<Value>(&bytes).ok();
        let observation = (harness == "codex")
            .then(|| {
                record.as_ref().and_then(|r| {
                    Observation::capture(r, &bytes, batch.cursor, batch.index.last_model.as_deref())
                })
            })
            .flatten();
        let known = if let Some(observation) = &observation {
            store.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_tree_codex_work WHERE projection_id=?1 AND identity=?2)",
                params![projection_id, observation.identity],
                |r| r.get::<_, bool>(0),
            ).map_err(sql_error)?
        } else {
            false
        };
        if harness == "claude"
            && let Some(message) = record.as_ref().and_then(|r| r.get("message"))
            && message.get("usage").is_some()
            && let Some(message_id) = crate::field(message, "id")
        {
            let tree = batch.index.tree_summary.as_mut().expect("tree initialized");
            if !tree.usage_by_id.contains_key(message_id) {
                let encoded:Option<String>=store.connection.query_row("SELECT tokens FROM native_tree_usage_work WHERE projection_id=?1 AND message_id=?2",params![projection_id,message_id],|r|r.get(0)).optional().map_err(sql_error)?;
                if let Some(encoded) = encoded {
                    tree.usage_by_id.insert(
                        message_id.into(),
                        from_json(0, &encoded).map_err(sql_error)?,
                    );
                }
            }
        }
        batch
            .index
            .tree_summary
            .as_mut()
            .expect("tree initialized")
            .observe_counted(record.as_ref(), &harness, !known);
        if harness == "codex"
            && let Some(record) = &record
        {
            batch
                .index
                .tree_summary
                .as_mut()
                .expect("tree initialized")
                .codex_turn
                .observe(record, batch.cursor);
        }
        if let Some(observation) = observation {
            batch
                .index
                .native_usage_observations
                .as_mut()
                .expect("batch initialized")
                .push(observation);
        }
        batch.cursor += bytes.len() as u64;
        batch.bytes += bytes.len() as u64;
        batch.rows += 1;
    }
    if Stat::of(Path::new(path))? != ledger.stat {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    Ok(batch)
}

/// One round-robin source slice; completed repairs release the cached index.
type PendingSource = (
    String,
    String,
    Option<i64>,
    u64,
    Option<String>,
    Option<String>,
    Option<i64>,
);
struct Resume {
    cursor: u64,
    model: Option<String>,
    tree: Option<String>,
    projection_id: i64,
    harness: String,
}

struct Checkpoint<'a> {
    path: &'a str,
    receipt: &'a str,
    cursor: u64,
    projection_id: Option<i64>,
}

// Progress writes are writers too: neither a stale schema nor a superseded
// repair slice may publish a failure, reset or completion over its successor.
fn progress(
    connection: &mut Connection,
    expected: Checkpoint<'_>,
    update: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !current(&tx)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let actual: (String, i64, Option<i64>) = tx.query_row(
        "SELECT receipt,cursor,projection_id FROM native_tree_repairs WHERE source_path=?1",
        [expected.path],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let cursor = i64::try_from(expected.cursor).map_err(|_| rusqlite::Error::InvalidQuery)?;
    if actual != (expected.receipt.into(), cursor, expected.projection_id) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    update(&tx)?;
    tx.commit()
}

pub(in crate::events::store) fn advance(
    store: &mut SqliteStore,
    options: &crate::Options,
) -> io::Result<(bool, Option<String>)> {
    let sql_error = io::Error::other;
    let selected: Option<PendingSource> = store.connection.query_row("SELECT source_path,receipt,source_id,cursor,last_model,tree_root,projection_id FROM native_tree_repairs WHERE state IN ('ready','running') ORDER BY last_attempt,source_path LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,u64::try_from(r.get::<_,i64>(3)?).map_err(|e|rusqlite::Error::FromSqlConversionFailure(3,rusqlite::types::Type::Integer,Box::new(e)))?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(sql_error)?;
    let Some((path, receipt, id, cursor, model, tree, projection_id)) = selected else {
        return Ok((pending(&store.connection).map_err(sql_error)?, None));
    };
    let _source_lock = crate::source_change::SourceLock::acquire(options, &path, false)?;
    let Some((_, ledger)) = ledger_row(&store.connection, &path).map_err(sql_error)? else {
        progress(
            &mut store.connection,
            Checkpoint {
                path: &path,
                receipt: &receipt,
                cursor,
                projection_id,
            },
            |tx| {
                tx.execute(
                    "UPDATE native_tree_repairs SET state='blocked_source' WHERE source_path=?1",
                    [&path],
                )?;
                Ok(())
            },
        )
        .map_err(sql_error)?;
        return Ok((pending(&store.connection).map_err(sql_error)?, None));
    };
    if header_ledger(&ledger) != receipt {
        let same_incarnation = source(&store.connection, &path)
            .map_err(sql_error)?
            .is_some_and(|(current, _)| Some(current) == id)
            && ledger.offset >= cursor;
        progress(&mut store.connection, Checkpoint {path:&path,receipt:&receipt,cursor,projection_id}, |tx| {
            if ledger_row(tx,&path)?.map(|(_,l)|l).as_ref()!=Some(&ledger) {return Err(rusqlite::Error::InvalidQuery);}
            if same_incarnation {
                tx.execute("UPDATE native_tree_repairs SET receipt=?2,state='running' WHERE source_path=?1",params![path,header_ledger(&ledger)])?;
            } else {
                tx.execute("UPDATE native_tree_repairs SET receipt=?2,source_id=NULL,projection_id=NULL,cursor=0,last_model=NULL,tree_root=NULL,state='ready' WHERE source_path=?1",params![path,header_ledger(&ledger)])?;
            }
            Ok(())
        }).map_err(sql_error)?;
        return Ok((true, None));
    }
    if tree_state::ready(&store.connection, &path, &ledger).map_err(sql_error)? {
        progress(
            &mut store.connection,
            Checkpoint {
                path: &path,
                receipt: &receipt,
                cursor,
                projection_id,
            },
            |tx| {
                tx.execute(
                    "UPDATE native_tree_repairs SET state='done' WHERE source_path=?1",
                    [&path],
                )?;
                Ok(())
            },
        )
        .map_err(sql_error)?;
        return Ok((pending(&store.connection).map_err(sql_error)?, None));
    }
    let input = crate::native_evidence::catalog_input(Path::new(&path), options);
    if input.is_none() {
        progress(
            &mut store.connection,
            Checkpoint {
                path: &path,
                receipt: &receipt,
                cursor,
                projection_id,
            },
            |tx| {
                tx.execute(
                    "UPDATE native_tree_repairs SET state='blocked_source' WHERE source_path=?1",
                    [&path],
                )?;
                Ok(())
            },
        )
        .map_err(sql_error)?;
        return Ok((pending(&store.connection).map_err(sql_error)?, None));
    }
    let (id, projection_id) = {
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if !current(&tx).map_err(sql_error)?
            || ledger_row(&tx, &path)
                .map_err(sql_error)?
                .map(|(_, l)| l)
                .as_ref()
                != Some(&ledger)
        {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let claimed:(Option<i64>,i64,Option<i64>,String)=tx.query_row("SELECT source_id,cursor,projection_id,receipt FROM native_tree_repairs WHERE source_path=?1",[&path],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(sql_error)?;
        if claimed != (id, cursor as i64, projection_id, receipt.clone()) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let id = match id {
            Some(id) => id,
            None => {
                source(&tx, &path)
                    .map_err(sql_error)?
                    .ok_or_else(|| io::Error::other("missing current source generation"))?
                    .0
            }
        };
        let projection_id = match projection_id {
            Some(id) => id,
            None => tree_state::new_generation(&tx, id, &ledger).map_err(sql_error)?,
        };
        tx.execute("UPDATE native_tree_repairs SET source_id=?2,projection_id=?3,state='running' WHERE source_path=?1",params![path,id,projection_id]).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        (id, projection_id)
    };
    let batch = match prepare(
        store,
        &path,
        &ledger,
        Resume {
            cursor,
            model,
            tree,
            projection_id,
            harness: input.expect("input checked").root.as_str().into(),
        },
    ) {
        Ok(batch) => batch,
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
        Err(error) => {
            let reason = if error.kind() == io::ErrorKind::NotFound {
                "blocked_source"
            } else if error.kind() == io::ErrorKind::WouldBlock {
                "blocked_receipt"
            } else {
                "blocked_evidence"
            };
            progress(
                &mut store.connection,
                Checkpoint {
                    path: &path,
                    receipt: &receipt,
                    cursor,
                    projection_id: Some(projection_id),
                },
                |tx| {
                    tx.execute(
                        "UPDATE native_tree_repairs SET state=?2 WHERE source_path=?1",
                        params![path, reason],
                    )?;
                    Ok(())
                },
            )
            .map_err(sql_error)?;
            return Ok((pending(&store.connection).map_err(sql_error)?, None));
        }
    };
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    if !current(&tx).map_err(sql_error)?
        || ledger_row(&tx, &path)
            .map_err(sql_error)?
            .map(|(_, l)| l)
            .as_ref()
            != Some(&ledger)
        || Stat::of(Path::new(&path)).ok().as_ref() != Some(&ledger.stat)
    {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    let actual: (i64,i64,i64) = tx.query_row("SELECT source_id,cursor,projection_id FROM native_tree_repairs WHERE source_path=?1 AND receipt=?2",params![path,receipt],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(sql_error)?;
    if actual != (id, cursor as i64, projection_id) {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    if let Some(observations) = &batch.index.native_usage_observations {
        for observation in observations {
            tx.prepare_cached(
                "INSERT INTO native_tree_codex_work VALUES(?1,?2) ON CONFLICT DO NOTHING",
            )
            .map_err(sql_error)?
            .execute(params![projection_id, observation.identity])
            .map_err(sql_error)?;
        }
    }
    let tree = batch.index.tree_summary.as_ref().expect("tree initialized");
    tree_state::write_values(&tx, projection_id, tree).map_err(sql_error)?;
    for (message, tokens) in &tree.usage_by_id {
        tx.prepare_cached("INSERT INTO native_tree_usage_work VALUES(?1,?2,?3) ON CONFLICT(projection_id,message_id) DO UPDATE SET tokens=excluded.tokens").map_err(sql_error)?.execute(params![projection_id,message,json(tokens).map_err(sql_error)?]).map_err(sql_error)?;
    }
    let tree_root = tree.compact(true);
    tx.execute(
        "UPDATE native_tree_generations SET work_pending=1 WHERE projection_id=?1",
        [projection_id],
    )
    .map_err(sql_error)?;
    let complete = batch.cursor == ledger.offset;
    tx.execute(
        metadata_increment(&tx).map_err(sql_error)?,
        ["native_tree_repair_clock"],
    )
    .map_err(sql_error)?;
    tx.execute(configured::metadata_sql(&tx,
        "UPDATE native_tree_repairs SET cursor=?2,last_model=?3,tree_root=?4,state=?5,last_attempt=CAST((SELECT value FROM meta WHERE key='native_tree_repair_clock') AS INTEGER) WHERE source_path=?1",
        "UPDATE native_tree_repairs SET cursor=?2,last_model=?3,tree_root=?4,state=?5,last_attempt=CAST((SELECT value FROM meta WHERE namespace='' AND key='native_tree_repair_clock') AS INTEGER) WHERE source_path=?1").map_err(sql_error)?,params![path,batch.cursor as i64,batch.index.last_model,json(&tree_root).map_err(sql_error)?,if complete {"done"} else {"running"}]).map_err(sql_error)?;
    if complete {
        tree_state::bind_generation(&tx, &path, &ledger, projection_id).map_err(sql_error)?;
        write_tree_summary(&tx, &path, &ledger, Some(&tree_root), true).map_err(sql_error)?;
    }
    crate::selected_read::checkpoint("native_tree_repair_commit")?;
    tx.commit().map_err(sql_error)?;
    Ok((
        pending(&store.connection).map_err(sql_error)?,
        complete.then_some(path),
    ))
}
