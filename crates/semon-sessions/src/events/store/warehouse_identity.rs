//! The warehouse retains the first certified component behind a retired key.
//! Certification records a complete recipe generation and the source receipts
//! it covered. The old Catalog remains a producer until its own cutover.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{
    model::summary::{CatalogLifecycle, CatalogRow},
    slot_projection::{Projection, VERSION},
};

use super::{Ledger, json, put};

pub(in crate::events::store) fn certify(
    tx: &Transaction<'_>,
    row: &CatalogRow,
    projection: &Projection,
    encoded_slots: &[String],
    recipe_generation: &str,
    recipe_source_generation: &str,
) -> rusqlite::Result<()> {
    if row.lifecycle != CatalogLifecycle::Current || row.sources.is_empty() {
        return Ok(());
    }
    let Some(source_key): Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='storage_namespace'",
            [],
            |record| record.get(0),
        )
        .optional()?
    else {
        return Ok(());
    };
    let previous: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM warehouse_identity WHERE scope='retained_history' AND source_key=?1
         AND catalog_key=?2 AND retained=0",
        )?
        .query_row(params![source_key, row.key], |record| record.get(0))
        .optional()?;
    if let Some(previous) = previous {
        put(
            tx,
            "DELETE FROM warehouse_identity_member WHERE identity=?1",
            [previous],
        )?;
        put(tx, "DELETE FROM warehouse_identity WHERE id=?1", [previous])?;
    }
    let paths: BTreeSet<_> = row.sources.iter().map(|source| &source.path).collect();
    if paths.len() != row.sources.len()
        || encoded_slots.iter().any(|slot| slot.len() > 1_048_576)
        || projection
            .slots
            .iter()
            .any(|slot| slot.source_paths().iter().any(|path| !paths.contains(path)))
    {
        return Ok(());
    }
    let mut cohort = None;
    let mut members = Vec::with_capacity(row.sources.len());
    let mut source_query = tx.prepare_cached(
        "SELECT c.source,m.component,g.owner,d.native_id,s.ledger FROM warehouse_current c
         JOIN warehouse_source s ON s.source=c.source
         JOIN lineage_member m ON m.source=c.source JOIN lineage_group g ON g.id=m.component
         JOIN dim_session d ON d.source=g.owner
         WHERE c.path=?1 AND s.harness=?2 AND s.origin=?3",
    )?;
    for source in &row.sources {
        let Some((id, component, owner, native_id, ledger)) = source_query
            .query_row(
                params![
                    source.path.to_string_lossy().as_ref(),
                    row.harness,
                    source_key
                ],
                |record| {
                    Ok((
                        record.get::<_, i64>(0)?,
                        record.get::<_, i64>(1)?,
                        record.get::<_, i64>(2)?,
                        record.get::<_, String>(3)?,
                        record.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
        else {
            return Ok(());
        };
        let ledger: Ledger = serde_json::from_str(&ledger).map_err(data)?;
        if ledger.stat.dev != source.dev
            || ledger.stat.ino != source.ino
            || ledger.stat.size != source.size
            || ledger.stat.modified_ns != source.modified_ns
            || ledger.stat.changed_ns != source.changed_ns
            || ledger.offset != source.offset
            || ledger.prefix != source.prefix_sha256
            || ledger.tail != source.tail_sha256
        {
            return Ok(());
        }
        let binding = (component, owner, native_id);
        if cohort.as_ref().is_some_and(|current| *current != binding) {
            return Ok(());
        }
        cohort = Some(binding);
        members.push((id, json(source)?));
    }
    let Some((component, owner, native_id)) = cohort else {
        return Ok(());
    };
    let member_count: i64 = tx
        .prepare_cached("SELECT count(*) FROM lineage_member WHERE component=?1")?
        .query_row([component], |record| record.get(0))?;
    if usize::try_from(member_count).ok() != Some(members.len()) {
        return Ok(());
    }
    let source_generation = crate::retention::source_generation(&row.sources).map_err(data)?;
    if source_generation != recipe_source_generation {
        return Ok(());
    }
    let identity: i64 = tx.prepare_cached(
        "INSERT INTO warehouse_identity(scope,source_key,catalog_key,retained,owner,harness,native_id,
          member_count,recipe_version,recipe_generation,recipe_source_generation,source_generation,complete)
         VALUES('retained_history',?1,?2,0,?3,?4,?5,?6,?7,?8,?9,?10,1) RETURNING id",
    )?.query_row(params![source_key, row.key, owner, row.harness, native_id, member_count, VERSION,
        recipe_generation, recipe_source_generation, source_generation], |record| record.get(0))?;
    for (ordinal, (source, receipt)) in members.into_iter().enumerate() {
        put(
            tx,
            "INSERT INTO warehouse_identity_member VALUES(?1,?2,?3,?4)",
            params![identity, source, ordinal as i64, receipt],
        )?;
    }
    Ok(())
}

fn data(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

pub(super) fn retire(tx: &Transaction<'_>, source: i64) -> rusqlite::Result<()> {
    // The first qualified original wins. Later incarnations with the same
    // key can be current, but cannot replace the original retained binding.
    put(
        tx,
        "UPDATE OR IGNORE warehouse_identity SET retained=1 WHERE retained=0 AND id IN
         (SELECT identity FROM warehouse_identity_member WHERE source=?1)",
        [source],
    )?;
    tx.prepare_cached(
        "DELETE FROM warehouse_identity WHERE retained=0 AND id IN
         (SELECT identity FROM warehouse_identity_member WHERE source=?1) RETURNING id",
    )?
    .query_map([source], |record| record.get::<_, i64>(0))?
    .collect::<rusqlite::Result<Vec<_>>>()?
    .into_iter()
    .try_for_each(|identity| {
        put(
            tx,
            "DELETE FROM warehouse_identity_member WHERE identity=?1",
            [identity],
        )
    })
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RetainedIdentity {
    pub(crate) owner: i64,
    pub(crate) harness: String,
    pub(crate) native_id: String,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum RetainedIdentityError {
    Unverified,
    Index(String),
}

#[cfg(test)]
impl RetainedIdentityError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Unverified => "retained_identity_unverified",
            Self::Index(_) => "catalog_unavailable",
        }
    }
}

#[cfg(test)]
impl std::fmt::Display for RetainedIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unverified => f.write_str(
                "No verified original warehouse identity is available for this retained key.",
            ),
            Self::Index(error) => f.write_str(error),
        }
    }
}

#[cfg(test)]
pub(crate) fn retained_identity(
    tx: &Transaction<'_>,
    namespace: &crate::storage::Namespace,
    key: &str,
) -> Result<RetainedIdentity, RetainedIdentityError> {
    use RetainedIdentityError::{Index, Unverified};
    crate::events::store::require_current(tx).map_err(|error| Index(error.to_string()))?;
    crate::storage::sqlite::qualify(tx, namespace).map_err(|error| Index(error.to_string()))?;
    read_retained(tx, namespace.binding(), key)
        .map_err(|error| Index(error.to_string()))?
        .ok_or(Unverified)
}

#[cfg(test)]
fn read_retained(
    tx: &Transaction<'_>,
    source_key: &str,
    key: &str,
) -> rusqlite::Result<Option<RetainedIdentity>> {
    use sha2::{Digest, Sha256};

    let binding = tx.prepare_cached(
        "SELECT i.id,i.owner,i.harness,i.native_id,i.member_count,i.source_generation
         FROM warehouse_identity i
         JOIN warehouse_source s ON s.source=i.owner AND s.origin=i.source_key AND s.harness=i.harness
         JOIN warehouse_identity_member m ON m.identity=i.id AND m.source=i.owner
         WHERE i.scope='retained_history' AND i.source_key=?1 AND i.catalog_key=?2 AND i.retained=1
         AND i.complete=1 AND i.recipe_version=?3 AND length(i.recipe_generation)=64
         AND i.recipe_source_generation=i.source_generation",
    )?.query_row(params![source_key, key, VERSION], |row| {
        Ok((row.get::<_, i64>(0)?, RetainedIdentity { owner: row.get(1)?, harness: row.get(2)?, native_id: row.get(3)? }, row.get::<_,i64>(4)?, row.get::<_,String>(5)?))
    }).optional()?;
    let Some((id, identity, member_count, generation)) = binding else {
        return Ok(None);
    };
    let mut digest = Sha256::new_with_prefix(b"[");
    let mut statement = tx.prepare_cached(
        "SELECT m.ordinal,m.receipt FROM warehouse_identity_member m
         JOIN warehouse_source s ON s.source=m.source AND s.origin=?2 AND s.harness=?3
         WHERE m.identity=?1 ORDER BY m.ordinal",
    )?;
    let mut members = statement.query(params![id, source_key, identity.harness])?;
    let mut count = 0;
    while let Some(member) = members.next()? {
        if member.get::<_, i64>(0)? != count {
            return Ok(None);
        }
        if count > 0 {
            digest.update(b",");
        }
        digest.update(member.get::<_, String>(1)?.as_bytes());
        count += 1;
    }
    digest.update(b"]");
    Ok(
        (count == member_count && format!("{:x}", digest.finalize()) == generation)
            .then_some(identity),
    )
}
