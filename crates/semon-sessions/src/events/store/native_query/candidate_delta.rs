//! Writer-owned candidate interests. Capture before replacing native rows:
//! point changes retain both observations; large suffixes/cuts remain ranges.
//! An interest is work, never a qualified candidate publication.
use super::*;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_candidate_changes (
 change_id INTEGER PRIMARY KEY,
 path TEXT NOT NULL, before_generation TEXT, generation TEXT,
 reset INTEGER NOT NULL CHECK(reset IN (0,1)),
 events INTEGER NOT NULL, extras INTEGER NOT NULL, signals INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS native_candidate_change_path
 ON native_candidate_changes(path,change_id);
CREATE TABLE IF NOT EXISTS native_candidate_interests (
 change_id INTEGER NOT NULL, channel INTEGER NOT NULL CHECK(channel BETWEEN 0 AND 3),
 first INTEGER NOT NULL, last INTEGER,
 before_fact TEXT, after_fact TEXT,
 PRIMARY KEY(change_id,channel,first),
 CHECK(first>=0 AND (last IS NULL OR last>=first))
) STRICT, WITHOUT ROWID;
";

#[derive(Clone, Debug)]
pub(crate) struct CandidateChange {
    pub(crate) id: i64,
    pub(crate) before_generation: Option<String>,
    pub(crate) generation: Option<String>,
    pub(crate) reset: bool,
    pub(crate) counts: (usize, usize, usize),
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateInterest {
    /// 0: regular events, 1: extras, 2: signals, 3: native link delta.
    pub(crate) channel: u8,
    pub(crate) first: usize,
    /// Inclusive point when equal to first; exclusive range end otherwise.
    /// None is the old candidate generation's complete remaining suffix.
    pub(crate) last: Option<usize>,
    pub(crate) before: Option<Value>,
    pub(crate) after: Option<Value>,
}

fn insert_interest(
    tx: &Connection,
    change: i64,
    channel: u8,
    first: usize,
    last: Option<usize>,
    before: Option<&str>,
    after: Option<&str>,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO native_candidate_interests VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            change,
            channel,
            sequence_sql(first)?,
            last.map(sequence_sql).transpose()?,
            before,
            after
        ],
    )?;
    Ok(())
}

/// Complete replacement is a pair of old/new range interests, represented by
/// one reset plus current counts. The previous immutable payload/root owns old
/// parent, placement, identity and annotation facts until worker retirement.
pub(in crate::events::store) fn capture(
    tx: &Transaction<'_>,
    path: &str,
    expected: Option<&Ledger>,
    ledger: Option<&Ledger>,
    index: Option<&FileIndex>,
    changes: Option<&super::super::super::DirtyRows>,
) -> rusqlite::Result<()> {
    if !private_current(tx)? {
        return Err(invalid("native candidate producer schema changed"));
    }
    let counts = index.map_or((0, 0, 0), |index| {
        (index.events.len(), index.extras.len(), index.signals.len())
    });
    let reset = changes.is_none();
    // A persistent scalar prevents ABA after old interests have been retired.
    // It is unrelated-history independent and belongs to this writer's tx.
    let previous: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='native_candidate_revision'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let previous = previous
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| invalid("invalid native candidate revision"))
        })
        .transpose()?
        .unwrap_or(0);
    let id = previous
        .checked_add(1)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("native candidate revision overflow"))?;
    tx.execute("INSERT INTO meta VALUES('native_candidate_revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[id.to_string()])?;
    tx.execute(
        "INSERT INTO native_candidate_changes(change_id,path,before_generation,generation,reset,events,extras,signals) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id,path,expected.map(source_fact_generation).transpose()?,ledger.map(source_fact_generation).transpose()?,reset,sequence_sql(counts.0)?,sequence_sql(counts.1)?,sequence_sql(counts.2)?],
    )?;
    let (Some(index), Some(changes)) = (index, changes) else {
        // No old-row enumeration, even for a replacement or million-row cut.
        for channel in 0..3 {
            insert_interest(tx, id, channel, 0, None, None, None)?;
        }
        return Ok(());
    };
    let file: Option<i64> = tx
        .query_row("SELECT file_id FROM files WHERE path=?1", [path], |row| {
            row.get(0)
        })
        .optional()?;
    for (channel, before, touched, events) in [
        (0, changes.before_events, &changes.events, &index.events),
        (1, changes.before_extras, &changes.extras, &index.extras),
    ] {
        // Appended rows are one durable current-generation range, not another
        // copy of every appended Event. Only overwritten points need old facts.
        for &seq in touched.range(..before.min(events.len())) {
            let old = file
                .map(|file| event_fact(tx, file, channel == 1, seq))
                .transpose()?
                .flatten();
            let new = json(&events[seq])?;
            insert_interest(tx, id, channel, seq, Some(seq), old.as_deref(), Some(&new))?;
        }
        suffix(tx, id, channel, before, events.len())?;
    }
    for &seq in changes
        .signals
        .range(..changes.before_signals.min(index.signals.len()))
    {
        let old = file
            .map(|file| signal_fact(tx, file, seq))
            .transpose()?
            .flatten();
        let new = encode_signal(&index.signals[seq])?;
        insert_interest(tx, id, 2, seq, Some(seq), old.as_deref(), Some(&new))?;
    }
    suffix(tx, id, 2, changes.before_signals, index.signals.len())?;
    for (ordinal, (kind, native_id)) in changes.links.iter().enumerate() {
        let old: bool = match file {
            Some(file) => tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_source_links WHERE file_id=?1 AND kind=?2 AND native_id=?3)",
                params![file,kind,native_id], |row| row.get(0),
            )?,
            None => false,
        };
        let new = match kind.as_str() {
            "session" => index.links.session_ids.contains(native_id),
            "continued" => index.links.continued_in.contains(native_id),
            "bridge" => index.links.bridges.contains(native_id),
            _ => return Err(invalid("unsupported native candidate link")),
        };
        let old = json(&(kind, native_id, old))?;
        let new = json(&(kind, native_id, new))?;
        insert_interest(tx, id, 3, ordinal, Some(ordinal), Some(&old), Some(&new))?;
    }
    Ok(())
}

fn suffix(
    tx: &Connection,
    id: i64,
    channel: u8,
    before: usize,
    after: usize,
) -> rusqlite::Result<()> {
    if before < after {
        insert_interest(tx, id, channel, before, Some(after), None, None)?;
    } else if after < before {
        insert_interest(tx, id, channel, after, None, None, None)?;
    }
    Ok(())
}

fn event_fact(
    tx: &Connection,
    file: i64,
    extra: bool,
    seq: usize,
) -> rusqlite::Result<Option<String>> {
    let event = tx
        .query_row(
            &format!("SELECT {EVENT_COLUMNS} FROM events WHERE file_id=?1 AND extra=?2 AND seq=?3"),
            params![file, extra, sequence_sql(seq)?],
            read_event,
        )
        .optional()?;
    event.map(|(_, _, event)| json(&event)).transpose()
}

fn signal_fact(tx: &Connection, file: i64, seq: usize) -> rusqlite::Result<Option<String>> {
    let found = tx
        .query_row(
            "SELECT k,o,t,at,n,v FROM signals WHERE file_id=?1 AND seq=?2",
            params![file, sequence_sql(seq)?],
            |row| {
                Ok(Signal {
                    k: untag(0, row.get(0)?)?,
                    o: uint(row.get(1)?),
                    t: row.get(2)?,
                    at: row.get(3)?,
                    n: row.get(4)?,
                    v: row.get::<_, Option<i64>>(5)?.map(uint),
                })
            },
        )
        .optional()?;
    found.map(|signal| encode_signal(&signal)).transpose()
}

fn encode_signal(signal: &Signal) -> rusqlite::Result<String> {
    json(
        &serde_json::json!({"k":signal.k,"o":signal.o,"t":signal.t,"at":signal.at,"n":signal.n,"v":signal.v}),
    )
}

impl NativeStore {
    /// Immutable interests from all intermediate writes are visible under the
    /// current captured source observation. Retaining earlier changes matters
    /// when a second append arrives before the previous worker finishes.
    pub(crate) fn candidate_changes_page(
        &self,
        witness: &SourceWitness,
        after: Option<i64>,
    ) -> io::Result<Page<CandidateChange, i64>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(io::Error::other("native candidate source changed"));
        }
        let mut query = tx.prepare(
            "SELECT change_id,CASE WHEN octet_length(before_generation)<=1048576 THEN before_generation END,CASE WHEN octet_length(generation)<=1048576 THEN generation END,reset,events,extras,signals,COALESCE(octet_length(before_generation),0)+COALESCE(octet_length(generation),0)+64 FROM native_candidate_changes WHERE path=?1 AND change_id>?2 ORDER BY change_id LIMIT 64",
        ).map_err(io::Error::other)?;
        let mut rows = query
            .query(params![
                witness.source.path.to_string_lossy(),
                after.unwrap_or(0)
            ])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes: usize = row.get(7).map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native candidate delta exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let id = row.get(0).map_err(io::Error::other)?;
            output.push(CandidateChange {
                id,
                before_generation: row.get(1).map_err(io::Error::other)?,
                generation: row.get(2).map_err(io::Error::other)?,
                reset: row.get(3).map_err(io::Error::other)?,
                counts: (
                    seq_of(row.get(4).map_err(io::Error::other)?),
                    seq_of(row.get(5).map_err(io::Error::other)?),
                    seq_of(row.get(6).map_err(io::Error::other)?),
                ),
            });
            end = Some(id);
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        drop(query);
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_candidate_changes WHERE path=?1 AND change_id>?2)",params![witness.source.path.to_string_lossy(),end.unwrap_or(0)],|row|row.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }

    pub(crate) fn candidate_interests_page(
        &self,
        witness: &SourceWitness,
        change: i64,
        after: Option<(u8, usize)>,
    ) -> io::Result<Page<CandidateInterest, (u8, usize)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(io::Error::other("native candidate source changed"));
        }
        let admitted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_candidate_changes WHERE change_id=?1 AND path=?2)",params![change,witness.source.path.to_string_lossy()],|row|row.get(0)).map_err(io::Error::other)?;
        if !admitted {
            return Err(io::Error::other(
                "native candidate change does not belong to source",
            ));
        }
        let start = after.map_or((-1, -1), |(channel, seq)| {
            (i64::from(channel), seq_int(seq))
        });
        let mut query=tx.prepare("SELECT channel,first,last,CASE WHEN COALESCE(octet_length(before_fact),0)+COALESCE(octet_length(after_fact),0)+64<=1048576 THEN before_fact END,CASE WHEN COALESCE(octet_length(before_fact),0)+COALESCE(octet_length(after_fact),0)+64<=1048576 THEN after_fact END,COALESCE(octet_length(before_fact),0)+COALESCE(octet_length(after_fact),0)+64 FROM native_candidate_interests WHERE change_id=?1 AND (channel,first)>(?2,?3) ORDER BY channel,first LIMIT 64").map_err(io::Error::other)?;
        let mut rows = query
            .query(params![change, start.0, start.1])
            .map_err(io::Error::other)?;
        let mut output = Vec::new();
        let mut end = after;
        let mut work = Work::default();
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let bytes: usize = row.get(5).map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native candidate point exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let channel: u8 = row.get(0).map_err(io::Error::other)?;
            let first = seq_of(row.get(1).map_err(io::Error::other)?);
            let before: Option<String> = row.get(3).map_err(io::Error::other)?;
            let after: Option<String> = row.get(4).map_err(io::Error::other)?;
            output.push(CandidateInterest {
                channel,
                first,
                last: row
                    .get::<_, Option<i64>>(2)
                    .map_err(io::Error::other)?
                    .map(seq_of),
                before: before
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(io::Error::other)?,
                after: after
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(io::Error::other)?,
            });
            end = Some((channel, first));
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(rows);
        work.vm_steps = query.get_status(rusqlite::StatementStatus::VmStep);
        drop(query);
        let start = end.map_or((-1, -1), |(channel, seq)| {
            (i64::from(channel), seq_int(seq))
        });
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_candidate_interests WHERE change_id=?1 AND (channel,first)>(?2,?3))",params![change,start.0,start.1],|row|row.get(0)).map_err(io::Error::other)?;
        Ok(Page {
            rows: output,
            after: end,
            complete: !remains,
            work,
        })
    }
}
