//! Native point observations published with their actual Catalog owner. These
//! facts are not inferred from output recipes and grant no source authority.
use super::*;
use crate::model::relation_stage::{Location, PointPublication, Relation};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_point_publications (
 publication INTEGER PRIMARY KEY, catalog_generation TEXT NOT NULL,
 relations INTEGER NOT NULL, postings INTEGER NOT NULL, complete INTEGER NOT NULL CHECK(complete=1)
) STRICT;
CREATE TABLE IF NOT EXISTS native_point_owners (
 owner TEXT PRIMARY KEY, publication INTEGER NOT NULL, source_generation TEXT NOT NULL, fingerprint TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_point_owner_publication ON native_point_owners(publication,owner);
CREATE TABLE IF NOT EXISTS native_point_sources (
 publication INTEGER NOT NULL,path TEXT NOT NULL,metadata TEXT NOT NULL,
 PRIMARY KEY(publication,path)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_point_relations (
 publication INTEGER NOT NULL,relation_id TEXT NOT NULL,native_order INTEGER NOT NULL,metadata TEXT NOT NULL,
 PRIMARY KEY(publication,relation_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_point_postings (
 publication INTEGER NOT NULL,path TEXT NOT NULL,seq INTEGER NOT NULL,
 phase INTEGER NOT NULL CHECK(phase IN (0,1,3)),ordinal INTEGER NOT NULL,
 relation_id TEXT NOT NULL,metadata TEXT,
 PRIMARY KEY(publication,path,seq,phase,ordinal,relation_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_point_heads (
 publication INTEGER NOT NULL,owner TEXT NOT NULL,relation_id TEXT NOT NULL,
 PRIMARY KEY(publication,owner,relation_id)
) STRICT, WITHOUT ROWID;
";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicationRef {
    pub(crate) publication: i64,
    pub(crate) owner: String,
    pub(crate) source_generation: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Posting {
    /// 0: native placed member; 1: after; 3: background completion.
    pub(crate) phase: u8,
    pub(crate) ordinal: usize,
    pub(crate) relation: String,
    pub(crate) background_call: Option<Location>,
    pub(crate) outgoing: Option<bool>,
}

pub(crate) fn header(db: &Connection, row: &CatalogRow) -> rusqlite::Result<Option<String>> {
    db.query_row("SELECT json_array(o.publication,o.source_generation) FROM native_point_owners o JOIN native_point_publications p USING(publication) WHERE o.owner=?1 AND p.complete=1 AND octet_length(o.source_generation)<=65536",[crate::query_projection::identity(row)],|row|row.get(0)).optional()
}

fn reference(db: &Connection, row: &CatalogRow) -> rusqlite::Result<Option<PublicationRef>> {
    let source_generation = crate::retention::source_generation(&row.sources)
        .map_err(|_| invalid("native point source generation cannot serialize"))?;
    db.query_row("SELECT o.publication FROM native_point_owners o JOIN native_point_publications p USING(publication) WHERE o.owner=?1 AND o.source_generation=?2 AND p.complete=1",params![crate::query_projection::identity(row),source_generation],|r|r.get(0)).optional().map(|value|value.map(|publication|PublicationRef { publication,owner:crate::query_projection::identity(row),source_generation }))
}

fn current(db: &Connection, expected: &PublicationRef) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM native_point_owners o JOIN native_point_publications p USING(publication) WHERE o.owner=?1 AND o.publication=?2 AND o.source_generation=?3 AND p.complete=1)",params![expected.owner,expected.publication,expected.source_generation],|r|r.get(0))
}

fn posting(
    db: &Connection,
    publication: i64,
    at: &Location,
    phase: u8,
    ordinal: usize,
    relation: &str,
    metadata: Option<&str>,
) -> rusqlite::Result<()> {
    let seq = at
        .seq
        .ok_or_else(|| invalid("native point publication needs an absolute event address"))?;
    let source: Option<String> = db
        .query_row(
            "SELECT metadata FROM native_point_sources WHERE publication=?1 AND path=?2",
            params![publication, at.source.path.to_string_lossy()],
            |r| r.get(0),
        )
        .optional()?;
    if source.as_deref() != Some(&bounded_json(&at.source, BYTES)?) {
        return Err(invalid(
            "native point placement source generation disagrees",
        ));
    }
    db.execute(
        "INSERT INTO native_point_postings VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            publication,
            at.source.path.to_string_lossy(),
            sequence_sql(seq)?,
            phase,
            sequence_sql(ordinal)?,
            relation,
            metadata
        ],
    )?;
    Ok(())
}

fn owner_fingerprint(row: &CatalogRow, points: &PointPublication) -> rusqlite::Result<String> {
    let mut digest = Sha256::new();
    let mut part = |text: &str| {
        digest.update((text.len() as u64).to_be_bytes());
        digest.update(text.as_bytes());
    };
    part(
        &crate::retention::source_generation(&row.sources)
            .map_err(|_| invalid("native point source generation cannot serialize"))?,
    );
    let mut relations: Vec<_> = points
        .relations
        .values()
        .filter(|relation| {
            relation.from == row.key || relation.to.as_deref() == Some(row.key.as_str())
        })
        .collect();
    relations.sort_by_key(|relation| relation.native_order);
    for relation in relations {
        // Only relative native order in this owner's actual incident set
        // matters. Unrelated handoffs may shift global Builder indices.
        let mut scalar = relation.scalar();
        scalar.native_order = None;
        part(&bounded_json(&scalar, BYTES)?);
        for place in &relation.placements {
            part(&bounded_json(place, BYTES)?);
        }
        for after in &relation.after {
            part(&bounded_json(after, BYTES)?);
        }
        for head in &relation.heads {
            part(head);
        }
    }
    for (end, call) in points.background_ends.iter().filter(|(end, _)| {
        row.sources
            .iter()
            .any(|source| source.path == end.source.path)
    }) {
        part(&bounded_json(&(end, call), BYTES)?);
    }
    drop(part);
    Ok(format!("{:x}", digest.finalize()))
}

/// Called only inside the already fenced native Catalog publication transaction.
/// This mirrors its complete native point owner while the bounded replacement
/// producer is introduced; it makes no bounded-append claim for the old caller.
pub(in crate::events::store) fn publish(
    db: &Connection,
    generation: &str,
    rows: &[CatalogRow],
    points: &PointPublication,
) -> rusqlite::Result<()> {
    if !private_current(db)? {
        return Err(invalid("native point private schema changed"));
    }
    let mut changed = Vec::new();
    for row in rows {
        let identity = crate::query_projection::identity(row);
        let fingerprint = owner_fingerprint(row, points)?;
        let unchanged:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM native_point_owners o JOIN native_point_publications p USING(publication) WHERE o.owner=?1 AND o.fingerprint=?2 AND p.complete=1)",params![identity,fingerprint],|row|row.get(0))?;
        if !unchanged {
            changed.push((row, fingerprint));
        }
    }
    if changed.is_empty() {
        return Ok(());
    }
    let incident = |relation: &Relation| {
        changed.iter().any(|(row, _)| {
            relation.from == row.key || relation.to.as_deref() == Some(row.key.as_str())
        })
    };
    let needed = |path: &Path| {
        changed
            .iter()
            .any(|(row, _)| row.sources.iter().any(|source| source.path == path))
    };
    let previous: Option<String> = db
        .query_row(
            "SELECT value FROM meta WHERE key='native_point_epoch'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let previous = previous
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| invalid("invalid native point epoch"))
        })
        .transpose()?
        .unwrap_or(0);
    let publication = previous
        .checked_add(1)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("native point epoch overflow"))?;
    db.execute("INSERT INTO meta VALUES('native_point_epoch',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[publication.to_string()])?;
    for row in rows {
        for source in &row.sources {
            let text = bounded_json(source, BYTES)?;
            let path = source.path.to_string_lossy();
            let old: Option<String> = db
                .query_row(
                    "SELECT metadata FROM native_point_sources WHERE publication=?1 AND path=?2",
                    params![publication, path],
                    |r| r.get(0),
                )
                .optional()?;
            if old.as_ref().is_some_and(|old| old != &text) {
                return Err(invalid("native point source generations disagree"));
            }
            db.execute(
                "INSERT OR IGNORE INTO native_point_sources VALUES(?1,?2,?3)",
                params![publication, path, text],
            )?;
        }
    }
    let mut postings = 0usize;
    let mut relation_count = 0usize;
    for relation in points
        .relations
        .values()
        .filter(|relation| incident(relation))
    {
        relation_count += 1;
        let native_order = relation
            .native_order
            .ok_or_else(|| invalid("native handoff emission order is absent"))?;
        // Per-point postings own locations and multiplicity. A shared relation
        // cell must not contain an event-history-sized placement vector.
        let scalar = relation.scalar();
        let text = bounded_json(&scalar, BYTES)?;
        db.execute(
            "INSERT INTO native_point_relations VALUES(?1,?2,?3,?4)",
            params![publication, relation.id, sequence_sql(native_order)?, text],
        )?;
        for place in &relation.placements {
            posting(
                db,
                publication,
                &place.at,
                0,
                place
                    .ordinal
                    .ok_or_else(|| invalid("native placement order is absent"))?,
                &relation.id,
                Some(if place.outgoing { "true" } else { "false" }),
            )?;
            postings = postings
                .checked_add(1)
                .ok_or_else(|| invalid("native point count overflow"))?;
        }
        for at in &relation.after {
            posting(db, publication, at, 1, 0, &relation.id, None)?;
            postings = postings
                .checked_add(1)
                .ok_or_else(|| invalid("native point count overflow"))?;
        }
        for owner in &relation.heads {
            if !rows.iter().any(|row| row.key == *owner) {
                return Err(invalid("native head owner is outside publication"));
            }
            if !changed.iter().any(|(row, _)| row.key == *owner) {
                continue;
            }
            db.execute(
                "INSERT INTO native_point_heads VALUES(?1,?2,?3)",
                params![publication, owner, relation.id],
            )?;
        }
    }
    for (end, call) in points
        .background_ends
        .iter()
        .filter(|(end, _)| needed(&end.source.path))
    {
        posting(
            db,
            publication,
            end,
            3,
            0,
            "",
            Some(&bounded_json(call, BYTES)?),
        )?;
        postings = postings
            .checked_add(1)
            .ok_or_else(|| invalid("native point count overflow"))?;
    }
    db.execute(
        "INSERT INTO native_point_publications VALUES(?1,?2,?3,?4,1)",
        params![
            publication,
            generation,
            sequence_sql(relation_count)?,
            sequence_sql(postings)?
        ],
    )?;
    for (row, fingerprint) in changed {
        let sources = crate::retention::source_generation(&row.sources)
            .map_err(|_| invalid("native point source generation cannot serialize"))?;
        db.execute("INSERT INTO native_point_owners VALUES(?1,?2,?3,?4) ON CONFLICT(owner) DO UPDATE SET publication=excluded.publication,source_generation=excluded.source_generation,fingerprint=excluded.fingerprint",params![crate::query_projection::identity(row),publication,sources,fingerprint])?;
    }
    Ok(())
}

impl NativeStore {
    pub(crate) fn point_publication(&self, row: &CatalogRow) -> io::Result<Option<PublicationRef>> {
        reference(&self.connection, row).map_err(io::Error::other)
    }
    pub(crate) fn point_publication_current(
        &self,
        publication: &PublicationRef,
    ) -> io::Result<bool> {
        current(&self.connection, publication).map_err(io::Error::other)
    }
    pub(crate) fn native_point_head(
        &self,
        publication: &PublicationRef,
        owner: &str,
    ) -> io::Result<Option<String>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !current(&tx, publication).map_err(io::Error::other)? {
            return Err(io::Error::other("native point owner publication changed"));
        }
        let mut statement=tx.prepare("SELECT CASE WHEN octet_length(relation_id)<=1048576 THEN relation_id END FROM native_point_heads WHERE publication=?1 AND owner=?2 LIMIT 2").map_err(io::Error::other)?;
        let mut rows = statement
            .query(params![publication.publication, owner])
            .map_err(io::Error::other)?;
        let result = rows
            .next()
            .map_err(io::Error::other)?
            .map(|row| row.get::<_, String>(0).map_err(io::Error::other))
            .transpose()?;
        if rows.next().map_err(io::Error::other)?.is_some() {
            return Err(io::Error::other("native point owner has multiple heads"));
        }
        drop(rows);
        drop(statement);
        tx.commit().map_err(io::Error::other)?;
        if !current(&self.connection, publication).map_err(io::Error::other)? {
            return Err(io::Error::other("native point owner publication changed"));
        }
        Ok(result)
    }
    pub(crate) fn native_point_relation(
        &self,
        publication: &PublicationRef,
        id: &str,
    ) -> io::Result<Option<Relation>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !current(&tx, publication).map_err(io::Error::other)? {
            return Err(io::Error::other("native point owner publication changed"));
        }
        let text:Option<Option<String>>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM native_point_relations WHERE publication=?1 AND relation_id=?2",params![publication.publication,id],|r|r.get(0)).optional().map_err(io::Error::other)?;
        let value = text
            .map(|text| {
                serde_json::from_str(&text.ok_or_else(|| {
                    io::Error::other("native point relation exceeds metadata budget")
                })?)
                .map_err(io::Error::other)
            })
            .transpose()?;
        tx.commit().map_err(io::Error::other)?;
        if !current(&self.connection, publication).map_err(io::Error::other)? {
            return Err(io::Error::other("native point owner publication changed"));
        }
        Ok(value)
    }
    pub(crate) fn native_point_postings(
        &self,
        publication: &PublicationRef,
        witness: &SourceWitness,
        seq: usize,
        after: Option<(u8, usize, &str)>,
    ) -> io::Result<Page<Posting, (u8, usize, String)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !current(&tx, publication).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "native point source or publication changed",
            ));
        }
        let source:Option<String>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM native_point_sources WHERE publication=?1 AND path=?2",params![publication.publication,witness.source.path.to_string_lossy()],|r|r.get(0)).optional().map_err(io::Error::other)?;
        if source.as_deref()
            != Some(&bounded_json(&witness.source, BYTES).map_err(io::Error::other)?)
        {
            return Err(io::Error::other(
                "native point Original source generation disagrees",
            ));
        }
        let (phase, ordinal, id) = after.unwrap_or((0, 0, ""));
        let inclusive = after.is_none();
        let mut q=tx.prepare("SELECT phase,ordinal,relation_id,CASE WHEN COALESCE(octet_length(metadata),0)+octet_length(relation_id)+32<=1048576 THEN metadata END,COALESCE(octet_length(metadata),0)+octet_length(relation_id)+32 FROM native_point_postings WHERE publication=?1 AND path=?2 AND seq=?3 AND ((phase,ordinal,relation_id)>(?4,?5,?6) OR ?7 AND (phase,ordinal,relation_id)=(?4,?5,?6)) ORDER BY phase,ordinal,relation_id LIMIT 64").map_err(io::Error::other)?;
        let mut values = q
            .query(params![
                publication.publication,
                witness.source.path.to_string_lossy(),
                sequence_sql(seq).map_err(io::Error::other)?,
                phase,
                sequence_sql(ordinal).map_err(io::Error::other)?,
                id,
                inclusive
            ])
            .map_err(io::Error::other)?;
        let mut rows = Vec::new();
        let mut work = Work::default();
        let mut end = after.map(|(p, o, id)| (p, o, id.to_owned()));
        while let Some(row) = values.next().map_err(io::Error::other)? {
            let bytes = usize::try_from(row.get::<_, i64>(4).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native point posting exceeds metadata budget",
                ));
            }
            if work.metadata_bytes + bytes > BYTES {
                break;
            }
            let phase = row.get(0).map_err(io::Error::other)?;
            let ordinal = usize::try_from(row.get::<_, i64>(1).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            let relation: String = row.get(2).map_err(io::Error::other)?;
            let metadata: Option<String> = row.get(3).map_err(io::Error::other)?;
            let outgoing = if phase == 0 {
                Some(
                    serde_json::from_str::<bool>(metadata.as_deref().ok_or_else(|| {
                        io::Error::other("native point placement direction is absent")
                    })?)
                    .map_err(io::Error::other)?,
                )
            } else {
                None
            };
            let background_call = if phase == 3 {
                Some(
                    serde_json::from_str(
                        metadata
                            .as_deref()
                            .ok_or_else(|| io::Error::other("native background call is absent"))?,
                    )
                    .map_err(io::Error::other)?,
                )
            } else {
                None
            };
            end = Some((phase, ordinal, relation.clone()));
            rows.push(Posting {
                phase,
                ordinal,
                relation,
                background_call,
                outgoing,
            });
            work.decoded_rows += 1;
            work.metadata_bytes += bytes;
        }
        drop(values);
        work.vm_steps = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let (phase, ordinal, id) = end
            .as_ref()
            .map(|(p, o, id)| (*p, *o, id.as_str()))
            .unwrap_or((0, 0, ""));
        let remains:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_point_postings WHERE publication=?1 AND path=?2 AND seq=?3 AND (phase,ordinal,relation_id)>(?4,?5,?6))",params![publication.publication,witness.source.path.to_string_lossy(),sequence_sql(seq).map_err(io::Error::other)?,phase,sequence_sql(ordinal).map_err(io::Error::other)?,id],|r|r.get(0)).map_err(io::Error::other)?;
        tx.commit().map_err(io::Error::other)?;
        if !current(&self.connection, publication).map_err(io::Error::other)?
            || !check_source(&self.connection, witness).map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "native point source or publication changed",
            ));
        }
        Ok(Page {
            rows,
            after: end,
            complete: !remains,
            work,
        })
    }
}
