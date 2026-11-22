//! The warehouse retains the first certified component behind a retired key.
//! Certification records a complete recipe generation and the source receipts
//! it covered. The old Catalog remains a producer until its own cutover.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::{
    model::summary::{CatalogLifecycle, CatalogRow, CatalogSource},
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
    let (_, Some(source_key)) = crate::storage::sqlite::binding(tx)? else {
        return Ok(());
    };
    let Some((owner, native_id, observed)) = observe_component(tx, &source_key, row)? else {
        return Ok(());
    };
    let member_count = i64::try_from(observed.len()).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let members = observed
        .iter()
        .map(|member| Ok((member.source, json(&member.receipt)?)))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let previous: Option<(i64, i64)> = tx.prepare_cached(
        "SELECT id,owner FROM warehouse_identity WHERE scope='retained_history' AND source_key=?1
         AND catalog_key=?2 AND retained=0",
    )?.query_row(params![source_key, row.key], |record| Ok((record.get(0)?, record.get(1)?))).optional()?;
    if let Some((previous, previous_owner)) = previous
        && previous_owner != owner
    {
        put(
            tx,
            "UPDATE OR IGNORE warehouse_identity SET retained=1 WHERE id=?1",
            [previous],
        )?;
        discard_candidate(tx, previous)?;
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
    let source_generation = crate::retention::source_generation(&row.sources).map_err(data)?;
    if source_generation != recipe_source_generation {
        return Ok(());
    }
    let trace_peers = json(&observe_trace_peers(tx, &source_key, row)?)?;
    let mut witness = binding_digest(&(
        source_key.as_str(),
        row.key.as_str(),
        owner,
        row.harness.as_str(),
        native_id.as_str(),
        VERSION,
        recipe_generation,
        source_generation.as_str(),
        trace_peers.as_str(),
    ))?;
    for (ordinal, (source, receipt)) in members.iter().enumerate() {
        binding_member(&mut witness, ordinal as i64, *source, receipt)?;
    }
    let witness = format!("{:x}", witness.finalize());
    if let Some((previous, previous_owner)) = previous
        && previous_owner == owner
    {
        discard_candidate(tx, previous)?;
    }
    let identity: i64 = tx.prepare_cached(
        "INSERT INTO warehouse_identity(scope,source_key,catalog_key,retained,owner,harness,native_id,
          member_count,recipe_version,recipe_generation,recipe_source_generation,source_generation,complete,trace_peers,witness)
         VALUES('retained_history',?1,?2,0,?3,?4,?5,?6,?7,?8,?9,?10,1,?11,?12) RETURNING id",
    )?.query_row(params![source_key, row.key, owner, row.harness, native_id, member_count, VERSION,
        recipe_generation, recipe_source_generation, source_generation, trace_peers, witness], |record| record.get(0))?;
    for (ordinal, (source, receipt)) in members.into_iter().enumerate() {
        put(
            tx,
            "INSERT INTO warehouse_identity_member VALUES(?1,?2,?3,?4)",
            params![identity, source, ordinal as i64, receipt],
        )?;
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservedMember {
    pub(crate) source: i64,
    pub(crate) receipt: CatalogSource,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservedPeer {
    pub(crate) owner: i64,
    pub(crate) harness: String,
    pub(crate) native_id: String,
    pub(crate) members: Vec<ObservedMember>,
}

fn observe_component(
    tx: &Transaction<'_>,
    source_key: &str,
    row: &CatalogRow,
) -> rusqlite::Result<Option<(i64, String, Vec<ObservedMember>)>> {
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
            return Ok(None);
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
            return Ok(None);
        }
        let binding = (component, owner, native_id);
        if cohort.as_ref().is_some_and(|current| *current != binding) {
            return Ok(None);
        }
        cohort = Some(binding);
        members.push(ObservedMember {
            source: id,
            receipt: source.clone(),
        });
    }
    let Some((component, owner, native_id)) = cohort else {
        return Ok(None);
    };
    let member_count: i64 = tx
        .prepare_cached("SELECT count(*) FROM lineage_member WHERE component=?1")?
        .query_row([component], |record| record.get(0))?;
    let sources: BTreeSet<_> = members.iter().map(|member| member.source).collect();
    Ok((sources.len() == members.len()
        && usize::try_from(member_count).ok() == Some(members.len()))
    .then_some((owner, native_id, members)))
}

fn observe_trace_peers(
    tx: &Transaction<'_>,
    source_key: &str,
    row: &CatalogRow,
) -> rusqlite::Result<BTreeMap<String, ObservedPeer>> {
    let mut peers = BTreeMap::new();
    let Some(trace) = &row.trace else {
        return Ok(peers);
    };
    let endpoints: BTreeSet<_> = trace
        .endpoints()
        .filter(|key| *key != row.key.as_str())
        .collect();
    let mut catalog = tx.prepare_cached(
        "SELECT metadata FROM session_catalog WHERE session_key=?1 AND lifecycle='current'",
    )?;
    for key in endpoints {
        let Some(encoded) = catalog
            .query_row([key], |record| record.get::<_, String>(0))
            .optional()?
        else {
            continue;
        };
        let peer: CatalogRow = serde_json::from_str(&encoded).map_err(data)?;
        if let Some((owner, native_id, members)) = observe_component(tx, source_key, &peer)? {
            peers.insert(
                key.to_owned(),
                ObservedPeer {
                    owner,
                    harness: peer.harness,
                    native_id,
                    members,
                },
            );
        }
    }
    Ok(peers)
}

fn binding_digest(identity: &impl serde::Serialize) -> rusqlite::Result<Sha256> {
    let mut digest = Sha256::new_with_prefix(b"semon-retained-history-identity-v2\0");
    digest.update(serde_json::to_vec(identity).map_err(data)?);
    Ok(digest)
}

fn binding_member(
    digest: &mut Sha256,
    ordinal: i64,
    source: i64,
    receipt: &str,
) -> rusqlite::Result<()> {
    digest.update(serde_json::to_vec(&(ordinal, source, receipt)).map_err(data)?);
    Ok(())
}

fn discard_candidate(tx: &Transaction<'_>, identity: i64) -> rusqlite::Result<()> {
    put(
        tx,
        "DELETE FROM warehouse_identity_member WHERE identity=?1 AND EXISTS
        (SELECT 1 FROM warehouse_identity WHERE id=?1 AND retained=0)",
        [identity],
    )?;
    put(
        tx,
        "DELETE FROM warehouse_identity WHERE id=?1 AND retained=0",
        [identity],
    )
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
    pub(crate) binding: i64,
    pub(crate) witness: String,
    pub(crate) owner: i64,
    pub(crate) harness: String,
    pub(crate) native_id: String,
    trace_peers: String,
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
    tx: &rusqlite::Connection,
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
    tx: &rusqlite::Connection,
    source_key: &str,
    key: &str,
) -> rusqlite::Result<Option<RetainedIdentity>> {
    let binding = tx.prepare_cached(
        "SELECT i.id,i.owner,i.harness,i.native_id,i.member_count,i.source_generation,i.recipe_generation,i.witness,i.trace_peers
         FROM warehouse_identity i
         JOIN warehouse_source s ON s.source=i.owner AND s.origin=i.source_key AND s.harness=i.harness
         JOIN warehouse_identity_member m ON m.identity=i.id AND m.source=i.owner
         WHERE i.scope='retained_history' AND i.source_key=?1 AND i.catalog_key=?2 AND i.retained=1
         AND i.complete=1 AND i.recipe_version=?3 AND length(i.recipe_generation)=64
         AND i.recipe_source_generation=i.source_generation",
    )?.query_row(params![source_key, key, VERSION], |row| {
        Ok((row.get::<_, i64>(0)?, RetainedIdentity { binding: row.get(0)?, witness: row.get(7)?, owner: row.get(1)?, harness: row.get(2)?, native_id: row.get(3)?, trace_peers: row.get(8)? }, row.get::<_,i64>(4)?, row.get::<_,String>(5)?,row.get::<_,String>(6)?,row.get::<_,String>(7)?))
    }).optional()?;
    let Some((id, identity, member_count, generation, recipe_generation, expected_witness)) =
        binding
    else {
        return Ok(None);
    };
    let mut digest = Sha256::new_with_prefix(b"[");
    let mut witness = binding_digest(&(
        source_key,
        key,
        identity.owner,
        identity.harness.as_str(),
        identity.native_id.as_str(),
        VERSION,
        recipe_generation.as_str(),
        generation.as_str(),
        identity.trace_peers.as_str(),
    ))?;
    let mut statement = tx.prepare_cached(
        "SELECT m.ordinal,m.receipt,m.source FROM warehouse_identity_member m
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
        let receipt: String = member.get(1)?;
        digest.update(receipt.as_bytes());
        binding_member(&mut witness, count, member.get(2)?, &receipt)?;
        count += 1;
    }
    digest.update(b"]");
    Ok((count == member_count
        && format!("{:x}", digest.finalize()) == generation
        && format!("{:x}", witness.finalize()) == expected_witness)
        .then_some(identity))
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum RetainedPeerError {
    Trace(RetainedIdentityError),
    TraceChanged,
    PeerUnobserved,
    CustodyChanged,
    Index(rusqlite::Error),
}

#[cfg(test)]
impl std::fmt::Display for RetainedPeerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trace(error) => write!(formatter, "{error}"),
            Self::TraceChanged => formatter.write_str("The retained trace changed."),
            Self::PeerUnobserved => {
                formatter.write_str("The trace has no verified observation of this peer.")
            }
            Self::CustodyChanged => formatter.write_str(
                "The observed peer incarnation or prefix is no longer in qualifying custody.",
            ),
            Self::Index(error) => write!(formatter, "{error}"),
        }
    }
}

#[cfg(test)]
pub(crate) fn retained_peer(
    tx: &rusqlite::Connection,
    namespace: &crate::storage::Namespace,
    trace: &str,
    witness: &str,
    key: &str,
) -> Result<ObservedPeer, RetainedPeerError> {
    use RetainedPeerError::*;
    let identity = retained_identity(tx, namespace, trace).map_err(Trace)?;
    if identity.witness != witness {
        return Err(TraceChanged);
    }
    let mut peers: BTreeMap<String, ObservedPeer> =
        serde_json::from_str(&identity.trace_peers).map_err(|error| Index(data(error)))?;
    let peer = peers.remove(key).ok_or(PeerUnobserved)?;
    let source_key = namespace.binding();
    if current_peer_covers(tx, source_key, &peer).map_err(Index)?
        || retained_peer_covers(tx, source_key, key, &peer).map_err(Index)?
    {
        return Ok(peer);
    }
    Err(CustodyChanged)
}

#[cfg(test)]
fn current_peer_covers(
    tx: &rusqlite::Connection,
    source_key: &str,
    peer: &ObservedPeer,
) -> rusqlite::Result<bool> {
    let mut statement = tx.prepare_cached(
        "SELECT c.path,s.ledger,g.owner,d.native_id,
         (SELECT count(*) FROM lineage_member p WHERE p.component=g.id)
         FROM warehouse_current c
         JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.harness=?3
         JOIN lineage_member m ON m.source=c.source
         JOIN lineage_group g ON g.id=m.component
         JOIN dim_session d ON d.source=g.owner
         WHERE c.source=?1",
    )?;
    for member in &peer.members {
        let observed = &member.receipt;
        let Some((path, ledger, owner, native_id, count)) = statement
            .query_row(params![member.source, source_key, peer.harness], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, usize>(4)?,
                ))
            })
            .optional()?
        else {
            return Ok(false);
        };
        let current: Ledger = serde_json::from_str(&ledger).map_err(data)?;
        if observed.path.to_str() != Some(path.as_str())
            || owner != peer.owner
            || native_id != peer.native_id
            || count != peer.members.len()
            || !prefix_covers(observed, current.offset, current.prefix, current.tail)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
fn retained_peer_covers(
    tx: &rusqlite::Connection,
    source_key: &str,
    key: &str,
    peer: &ObservedPeer,
) -> rusqlite::Result<bool> {
    let Some(frozen) = read_retained(tx, source_key, key)? else {
        return Ok(false);
    };
    if frozen.owner != peer.owner
        || frozen.harness != peer.harness
        || frozen.native_id != peer.native_id
    {
        return Ok(false);
    }
    let mut statement = tx.prepare_cached(
        "SELECT receipt FROM warehouse_identity_member WHERE identity=?1 AND source=?2",
    )?;
    for member in &peer.members {
        let Some(receipt) = statement
            .query_row(params![frozen.binding, member.source], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
        else {
            return Ok(false);
        };
        let receipt: CatalogSource = serde_json::from_str(&receipt).map_err(data)?;
        if receipt.path != member.receipt.path
            || receipt.native_id != member.receipt.native_id
            || !prefix_covers(
                &member.receipt,
                receipt.offset,
                receipt.prefix_sha256,
                receipt.tail_sha256,
            )
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
fn prefix_covers(observed: &CatalogSource, offset: u64, prefix: [u8; 32], tail: [u8; 32]) -> bool {
    // A source ID cannot survive truncation or replacement. For a later head,
    // this check only establishes custody. The body reader still verifies the
    // original offset and both hashes independently before serving any bytes.
    observed.offset < offset
        || (observed.offset == offset
            && observed.prefix_sha256 == prefix
            && observed.tail_sha256 == tail)
}
