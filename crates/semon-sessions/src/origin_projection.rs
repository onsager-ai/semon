//! SQLite implementation of `semon_asi`'s admitted input contract.
//!
//! This boundary intentionally reads the activity page and StepFacts readers
//! rather than their backing rows.  Those readers are where current-format,
//! source cohort, rules, and pricing admission is enforced.

use std::{collections::BTreeMap, path::Path};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use semon_asi::input::{
    Activity, AdmittedBatch, AdmittedSession, AnalyticsInput, BillingUsage, Compaction,
    ContextBand, Coverage, InputBudget, Model, ParentMethod, Price, PriceUnit, RateKind,
    Reconciliation, RecordedCost, RecordedVersions, Repo, Role, Session, SessionChange, Source,
    SourceWitness, Status, Step, ToolCall, Trigger, UNKNOWN_REPO_KEY, UsageOwner, WasteKind,
    encode_component,
};

use crate::{
    events::session_activity as native_activity,
    facts::RepoId,
    model::summary::{CatalogLifecycle, CatalogRow},
};

/// Generations of the exact contributing session set, independent of cohort IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CohortGenerations {
    pub activity_generation: String,
    pub transcript_generation: String,
    pub chains: BTreeMap<String, ChainEvidence>,
}

/// Stable archive identity; prefix and byte verification remain caller-owned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainEvidence {
    pub chain_id: String,
}

/// Closed, path-free refusals from a cohort evidence read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CohortReadError {
    Unavailable,
    Unsupported,
    Malformed,
    NotIndexed,
    Stale,
}

impl std::fmt::Display for CohortReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "cohort index unavailable",
            Self::Unsupported => "unsupported cohort index binding",
            Self::Malformed => "malformed cohort evidence",
            Self::NotIndexed => "cohort member not indexed",
            Self::Stale => "stale cohort index evidence",
        })
    }
}
impl std::error::Error for CohortReadError {}

/// Pin index evidence only. The caller retains authorization and before/after
/// source checks around body reads. This never initializes or repairs an index.
pub fn read_cohort_generations(
    index: &Path,
    expected: &crate::analytics::Binding,
    cohorts: &[semon_asi::egress::AdmittedCohort],
) -> Result<CohortGenerations, CohortReadError> {
    use CohortReadError::{Malformed, NotIndexed, Stale, Unavailable, Unsupported};
    let mut members = BTreeMap::new();
    if cohorts.is_empty() {
        return Err(Malformed);
    }
    for cohort in cohorts {
        if cohort.session_uid.is_empty() || cohort.witnesses.is_empty() {
            return Err(Malformed);
        }
        let mut sources = std::collections::BTreeSet::new();
        for (uid, witness) in &cohort.witnesses {
            if uid.is_empty() || witness.generation.is_empty() || witness.sources.is_empty() {
                return Err(Malformed);
            }
            for source in &witness.sources {
                if source.native_id.is_empty()
                    || source.local_path.is_empty()
                    || source.prefix_sha256.len() != 64
                    || !source
                        .prefix_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(Malformed);
                }
                sources.insert(source.native_id.clone());
            }
            if let Some(previous) = members.insert(uid, witness)
                && canonical_witness(previous.clone()) != canonical_witness(witness.clone())
            {
                return Err(Malformed);
            }
        }
        if sources != cohort.source_ids || !cohort.witnesses.contains_key(&cohort.session_uid) {
            return Err(Malformed);
        }
    }
    let mut connection = Connection::open_with_flags(
        index,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| Unavailable)?;
    let tx = connection.transaction().map_err(|_| Unavailable)?;
    let incarnation = crate::events::store::qualified_analytics_incarnation(&tx).map_err(
        |error| match error.kind {
            crate::analytics::ErrorKind::UnsupportedState
            | crate::analytics::ErrorKind::BindingMismatch => Unsupported,
            _ => Unavailable,
        },
    )?;
    let identity = crate::analytics_namespace::read_identity(index.parent().ok_or(Unsupported)?)
        .map_err(|_| Unsupported)?
        .ok_or(Unavailable)?;
    if identity.binding(incarnation) != *expected {
        return Err(Unsupported);
    }
    let coherent: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta a JOIN meta b ON a.value=b.value WHERE a.key='catalog_generation' AND b.key='slot_projection_catalog_generation')",
        [], |row| row.get(0),
    ).map_err(cohort_sql_error)?;
    if !coherent {
        return Err(Stale);
    }
    let price = crate::pricing::snapshot();
    let mut activity = Vec::new();
    let mut transcript = Vec::new();
    let mut chains = BTreeMap::new();
    for (uid, pinned) in members {
        let mut candidates = std::collections::BTreeSet::new();
        for source in &pinned.sources {
            let mut statement = tx
                .prepare(
                    "SELECT DISTINCT session_key FROM session_catalog_sources WHERE native_id=?1",
                )
                .map_err(cohort_sql_error)?;
            for key in statement
                .query_map([&source.native_id], |row| row.get::<_, String>(0))
                .map_err(cohort_sql_error)?
            {
                let key = key.map_err(cohort_sql_error)?;
                let identity = session_identity(&tx, &key).map_err(cohort_sql_error)?;
                if session_uid(&expected.namespace, &identity) == *uid {
                    candidates.insert(key);
                }
            }
        }
        if candidates.len() > 1 {
            return Err(Malformed);
        }
        let key = candidates.into_iter().next().ok_or(NotIndexed)?;
        let row = catalog_row(&tx, &key)
            .map_err(cohort_sql_error)?
            .ok_or(NotIndexed)?;
        if row.key != key {
            return Err(Malformed);
        }
        if row.lifecycle != CatalogLifecycle::Current {
            return Err(NotIndexed);
        }
        let pending: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_catalog_sources s JOIN session_catalog_invalidations i ON i.source_path=s.source_path WHERE s.session_key=?1)",
            [&key], |row| row.get(0),
        ).map_err(cohort_sql_error)?;
        if pending {
            return Err(Stale);
        }
        match crate::events::store::session_activity::source_missing(&tx, &key)
            .map_err(cohort_sql_error)?
        {
            Some(true) => return Err(NotIndexed),
            None => return Err(Stale),
            Some(false) => {}
        }
        let header =
            crate::events::store::session_activity::page(&tx, &row, None, &price.fingerprint, 0, 0)
                .map_err(cohort_sql_error)?
                .ok_or(Stale)?;
        let witness = source_witness(&tx, &key).map_err(cohort_sql_error)?;
        if witness.key != key
            || native_session_identity(&witness).map_err(cohort_sql_error)?
                != native_session_identity(&row).map_err(cohort_sql_error)?
            || canonical_witness(indexed_source_witness(&witness).map_err(cohort_sql_error)?)
                != canonical_witness(pinned.clone())
        {
            return Err(Stale);
        }
        let sources = crate::retention::source_generation(&row.sources).map_err(|_| Stale)?;
        if sources != pinned.generation {
            return Err(Stale);
        }
        let slot: Option<String> = tx.query_row(
            "SELECT generation FROM session_slot_projections WHERE session_key=?1 AND source_generation=?2 AND version=?3",
            params![key, sources, crate::slot_projection::VERSION], |row| row.get(0),
        ).optional().map_err(cohort_sql_error)?;
        let slot = slot.filter(|value| !value.is_empty()).ok_or(Stale)?;
        if header.generation.is_empty() {
            return Err(Stale);
        }
        for source in &pinned.sources {
            let evidence = ChainEvidence {
                chain_id: crate::events::store::cohort_chain(&tx, source)?,
            };
            if let Some(previous) = chains.insert(source.native_id.clone(), evidence.clone())
                && previous != evidence
            {
                return Err(Stale);
            }
        }
        activity.push((uid.as_str(), header.generation));
        transcript.push((
            uid.as_str(),
            crate::transcript_range::entry_generation(&slot),
        ));
    }
    let generations = CohortGenerations {
        activity_generation: cohort_digest("cohort-activity-v1", expected, &activity)?,
        transcript_generation: cohort_digest("cohort-transcript-v1", expected, &transcript)?,
        chains,
    };
    tx.commit().map_err(cohort_sql_error)?;
    Ok(generations)
}

fn cohort_sql_error(error: rusqlite::Error) -> CohortReadError {
    match error {
        rusqlite::Error::SqliteFailure(_, _) => CohortReadError::Unavailable,
        _ => CohortReadError::Stale,
    }
}

fn cohort_digest(
    domain: &str,
    binding: &crate::analytics::Binding,
    records: &[(&str, String)],
) -> Result<String, CohortReadError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(
        domain,
        &binding.namespace,
        &binding.index_incarnation,
        records,
    ))
    .map_err(|_| CohortReadError::Malformed)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn canonical_witness(mut witness: SourceWitness) -> SourceWitness {
    witness.sources.sort_by(|a, b| {
        (
            &a.native_id,
            &a.local_path,
            a.consumed_offset,
            &a.prefix_sha256,
        )
            .cmp(&(
                &b.native_id,
                &b.local_path,
                b.consumed_offset,
                &b.prefix_sha256,
            ))
    });
    witness
}

fn indexed_source_witness(row: &CatalogRow) -> rusqlite::Result<SourceWitness> {
    Ok(SourceWitness {
        generation: crate::retention::source_generation(&row.sources)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        sources: row
            .sources
            .iter()
            .map(|source| Source {
                native_id: source.native_id.clone(),
                local_path: source.path.to_string_lossy().into_owned(),
                consumed_offset: source.offset,
                prefix_sha256: hex_bytes(&source.prefix_sha256),
            })
            .collect(),
    })
}

/// A read-only input over one native SQLite index and one machine namespace.
///
/// The namespace is supplied by the owner that configured the index.  It is
/// not inferred from paths or host state.
///
/// The silver journal is append-only within an index incarnation. Any pruning
/// or resequencing must change the incarnation so readers restart inventory.
/// Sequence numbers may have gaps; replay does not require contiguity.
/// Each replay drain captures a fixed journal endpoint (after inventory, for a
/// cold read), so ongoing source writes cannot keep that drain open forever.
pub struct SqliteAnalyticsInput {
    namespace: String,
    connection: Connection,
}

impl SqliteAnalyticsInput {
    /// A replay endpoint in committed state predates a newly captured writer
    /// chain. Finish it before starting a fresh drain for that request. The
    /// checkpoint format remains adapter-owned; actual reads validate anchors
    /// and restart inventory if the index incarnation changed.
    pub(crate) fn resumes_replay(checkpoint: Option<&str>) -> rusqlite::Result<bool> {
        Ok(checkpoint
            .map(decode_checkpoint)
            .transpose()?
            .is_some_and(|checkpoint| matches!(checkpoint.cursor, Cursor::Replay { .. })))
    }

    pub fn open(index: &Path, namespace: impl Into<String>) -> rusqlite::Result<Self> {
        let connection = Connection::open_with_flags(
            index,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        if !crate::events::store::current(&connection)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(Self {
            namespace: namespace.into(),
            connection,
        })
    }
}

impl AnalyticsInput for SqliteAnalyticsInput {
    type Error = rusqlite::Error;

    fn read_admitted(
        &mut self,
        checkpoint: Option<&str>,
        budget: InputBudget,
    ) -> rusqlite::Result<AdmittedBatch> {
        if budget.max_sessions == 0 || budget.max_steps == 0 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let previous = checkpoint.map(decode_checkpoint).transpose()?;
        let namespace = self.namespace.clone();
        if previous
            .as_ref()
            .is_some_and(|previous| previous.namespace != namespace)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        crate::events::store::read_silver_snapshot(
            &mut self.connection,
            |tx, incarnation, current| {
                let previous = match previous {
                    Some(previous) if previous.incarnation == incarnation => {
                        Some(validate_cursor(tx, previous.cursor, current)?)
                    }
                    _ => None,
                };
                let (snapshot, after_key, through_key, after_sequence, endpoint) = match previous {
                    Some(Cursor::Inventory {
                        snapshot,
                        after,
                        through,
                    }) => (Some(snapshot), Some(after), Some(through), 0, current),
                    Some(Cursor::Replay { after, through }) => (None, None, None, after, through),
                    Some(Cursor::Incremental { after }) => (None, None, None, after, current),
                    None => (Some(current), None, maximum_inventory_key(tx)?, 0, current),
                };
                let limit = i64::try_from(budget.max_sessions)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let selected = if snapshot.is_some() {
                    if through_key.is_some() {
                        inventory_keys(tx, after_key.as_deref(), through_key.as_deref(), limit)?
                            .into_iter()
                            .map(|key| (key, 0, "changed".to_owned()))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    journal_page(tx, after_sequence, endpoint, limit)?
                };
                let mut changes = Vec::new();
                let mut cursor_key = after_key;
                let mut cursor_sequence = after_sequence;
                let mut steps = 0usize;
                for (key, sequence, kind) in selected {
                    let mut next = read_session(tx, &namespace, &key, &kind)?;
                    for change in &mut next {
                        if let SessionChange::Admitted(session) = change {
                            session.derivation.index_incarnation = Some(incarnation.to_owned());
                        }
                    }
                    let next_steps = next
                        .iter()
                        .map(|change| match change {
                            SessionChange::Admitted(session) => session.steps.len(),
                            _ => 0,
                        })
                        .sum::<usize>();
                    if !changes.is_empty() && next_steps > budget.max_steps.saturating_sub(steps) {
                        break;
                    }
                    changes.extend(next);
                    cursor_key = Some(key);
                    cursor_sequence = sequence;
                    steps = steps.saturating_add(next_steps);
                    if steps >= budget.max_steps {
                        break;
                    }
                }
                let finished_inventory = snapshot.is_some()
                    && (through_key.is_none()
                        || inventory_keys(tx, cursor_key.as_deref(), through_key.as_deref(), 1)?
                            .is_empty());
                let reconciliation = if finished_inventory {
                    Reconciliation::ReplaceNamespace {
                        session_uids: inventory_uids(tx, &namespace)?,
                    }
                } else {
                    Reconciliation::Incremental
                };
                let phase = match snapshot {
                    Some(snapshot) if !finished_inventory => Cursor::Inventory {
                        snapshot,
                        through: through_key.ok_or(rusqlite::Error::InvalidQuery)?,
                        after: cursor_key.ok_or(rusqlite::Error::InvalidQuery)?,
                    },
                    Some(snapshot) => replay_cursor(snapshot, endpoint),
                    None => replay_cursor(cursor_sequence, endpoint),
                };
                let complete = matches!(&phase, Cursor::Incremental { .. });
                Ok(AdmittedBatch {
                    checkpoint: encode_checkpoint(&Checkpoint {
                        namespace: namespace.clone(),
                        incarnation: incarnation.into(),
                        cursor: phase,
                    })?,
                    namespace,
                    complete,
                    reconciliation,
                    changes,
                })
            },
        )
    }
}

fn maximum_inventory_key(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<Option<String>> {
    tx.query_row(
        "SELECT session_key FROM session_catalog WHERE lifecycle='current' \
         UNION SELECT session_key FROM session_activity_projections ORDER BY session_key DESC LIMIT 1",
        [], |row| row.get(0),
    ).optional()
}

fn inventory_keys(
    tx: &rusqlite::Transaction<'_>,
    after: Option<&str>,
    through: Option<&str>,
    limit: i64,
) -> rusqlite::Result<Vec<String>> {
    // Put range predicates in both UNION arms so SQLite can merge ordered
    // index scans and stop at LIMIT instead of materializing the inventory.
    let lower = if after.is_some() {
        " AND session_key>?1"
    } else {
        ""
    };
    let upper = if through.is_some() {
        " AND session_key<=?2"
    } else {
        ""
    };
    let sql = format!(
        "SELECT session_key FROM session_catalog WHERE lifecycle='current'{lower}{upper} \
         UNION SELECT session_key FROM session_activity_projections WHERE 1{lower}{upper} \
         ORDER BY session_key LIMIT ?3",
    );
    let mut statement = tx.prepare(&sql)?;
    statement
        .query_map(params![after, through, limit], |row| row.get(0))?
        .collect()
}

fn journal_page(
    tx: &rusqlite::Transaction<'_>,
    after: i64,
    through: i64,
    limit: i64,
) -> rusqlite::Result<Vec<(String, i64, String)>> {
    let mut statement = tx.prepare(
        "SELECT seq,session_key,kind FROM silver_change \
         WHERE seq>?1 AND seq<=?2 ORDER BY seq LIMIT ?3",
    )?;
    let mut selected = BTreeMap::new();
    for entry in statement.query_map(params![after, through, limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (sequence, key, kind) = entry?;
        selected.insert(key, (sequence, kind));
    }
    let mut ordered: Vec<_> = selected
        .into_iter()
        .map(|(key, (sequence, kind))| (key, sequence, kind))
        .collect();
    ordered.sort_by_key(|(_, sequence, _)| *sequence);
    Ok(ordered)
}

fn inventory_uids(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let mut statement = tx.prepare(
        "SELECT session_key FROM session_catalog WHERE lifecycle='current' \
         UNION SELECT session_key FROM session_activity_projections",
    )?;
    let mut uids = std::collections::BTreeSet::new();
    for key in statement.query_map([], |row| row.get::<_, String>(0))? {
        // Catalog metadata supplies never-admitted Pending identities; the
        // native identity ledger supplies removed/retained activity identities.
        // Neither path reads StepFacts or native source bodies here.
        uids.insert(session_uid(namespace, &session_identity(tx, &key?)?));
    }
    Ok(uids)
}

fn read_session(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    key: &str,
    kind: &str,
) -> rusqlite::Result<Vec<SessionChange>> {
    let mut result = Vec::new();
    let identity = session_identity(tx, key)?;
    let uid = session_uid(namespace, &identity);
    if kind != "changed" && kind != "removed" {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let Some(row) = catalog_row(tx, key)? else {
        if kind == "removed" {
            result.push(SessionChange::Removed { session_uid: uid });
        } else {
            result.push(SessionChange::Pending { session_uid: uid });
        }
        return Ok(result);
    };
    // A Catalog row is still a live identity.  A `removed`
    // journal entry may only delete an identity that is no
    // longer in Catalog; while replacement activity is pending,
    // retain the previous admitted facts.
    if kind == "removed" {
        result.push(SessionChange::Pending { session_uid: uid });
        return Ok(result);
    }
    if row.lifecycle != CatalogLifecycle::Current {
        result.push(SessionChange::SourceMissing {
            session_uid: uid,
            retained_at_origin: true,
        });
        return Ok(result);
    }
    match crate::events::store::session_activity::source_missing(tx, key)? {
        Some(true) => {
            result.push(SessionChange::SourceMissing {
                session_uid: uid,
                retained_at_origin: true,
            });
            return Ok(result);
        }
        None => {
            result.push(SessionChange::Pending { session_uid: uid });
            return Ok(result);
        }
        Some(false) => {}
    }
    let price = crate::pricing::snapshot();
    let Some(header) =
        crate::events::store::session_activity::page(tx, &row, None, &price.fingerprint, 0, 0)?
    else {
        result.push(SessionChange::Pending { session_uid: uid });
        return Ok(result);
    };
    let Some(facts) =
        crate::events::store::session_activity::step_facts(tx, &row, &header.generation)?
    else {
        result.push(SessionChange::Pending { session_uid: uid });
        return Ok(result);
    };
    let steps = read_steps(
        tx,
        &row,
        &header.generation,
        &price.fingerprint,
        header.total,
    )?;
    if steps.len() != facts.len() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let witness = source_witness(tx, key)?;
    let prices = prices(tx, key, &header.generation, &price.fingerprint)?;
    let Some(session) = admitted(
        tx,
        namespace,
        AdmittedEvidence {
            row,
            witness,
            price_version: price.fingerprint.clone(),
            generation: &header.generation,
            steps,
            facts,
            prices,
        },
    )?
    else {
        // An unrepresentable anchor withholds this whole
        // generation, retaining prior facts without blocking
        // unrelated sessions or claiming source restoration.
        result.push(SessionChange::Pending { session_uid: uid });
        return Ok(result);
    };
    result.push(SessionChange::SourceRestored { session_uid: uid });
    result.push(SessionChange::Admitted(Box::new(session)));
    Ok(result)
}

fn prices(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    generation: &str,
    fingerprint: &str,
) -> rusqlite::Result<Vec<Price>> {
    let snapshot: String = tx.query_row(
        "SELECT pricing_snapshot FROM session_activity_projections \
         WHERE session_key=?1 AND generation=?2 AND pricing_fingerprint=?3",
        params![key, generation, fingerprint],
        |row| row.get(0),
    )?;
    let rates = crate::pricing::runtime::recorded_rates(&snapshot)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    rates
        .into_iter()
        .map(|rate| {
            if rate.fingerprint != fingerprint {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let rate_kind = match rate.kind {
                "input" => RateKind::Input,
                "output" => RateKind::Output,
                "cache_read" => RateKind::CacheRead,
                "cache_write_5m" => RateKind::CacheWrite5m,
                "cache_write_1h" => RateKind::CacheWrite1h,
                "web_search_request" => RateKind::WebSearchRequest,
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            let unit = match rate.unit {
                "mtok" => PriceUnit::Mtok,
                "request" => PriceUnit::Request,
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(Price {
                price_version: rate.fingerprint,
                provider: rate.provider,
                model: rate.model,
                rate_kind,
                tier: rate.tier,
                speed: rate.speed,
                service_tier: None,
                threshold_tokens: rate.threshold_tokens,
                unit,
                usd_per_unit: rate.usd_per_unit,
                valid_from: rate.learned_at,
                valid_to: None,
                learned_at: rate.learned_at,
            })
        })
        .collect()
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    namespace: String,
    incarnation: String,
    cursor: Cursor,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum Cursor {
    Inventory {
        snapshot: i64,
        through: String,
        after: String,
    },
    Incremental {
        after: i64,
    },
    Replay {
        after: i64,
        through: i64,
    },
}

fn replay_cursor(after: i64, through: i64) -> Cursor {
    if after == through {
        Cursor::Incremental { after }
    } else {
        Cursor::Replay { after, through }
    }
}

fn decode_checkpoint(value: &str) -> rusqlite::Result<Checkpoint> {
    let checkpoint: Checkpoint =
        serde_json::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery)?;
    if checkpoint.incarnation.len() != 32
        || !checkpoint
            .incarnation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !match &checkpoint.cursor {
            Cursor::Inventory {
                snapshot,
                through,
                after,
            } => *snapshot >= 0 && after < through,
            Cursor::Incremental { after } => *after >= 0,
            Cursor::Replay { after, through } => *after >= 0 && after < through,
        }
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(checkpoint)
}

fn validate_cursor(
    tx: &rusqlite::Transaction<'_>,
    cursor: Cursor,
    current: i64,
) -> rusqlite::Result<Cursor> {
    let (sequence, endpoint) = match &cursor {
        Cursor::Inventory {
            snapshot,
            through,
            after,
        } => {
            for key in [after, through] {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1 \
                     UNION ALL SELECT 1 FROM session_activity_projections WHERE session_key=?1 \
                     UNION ALL SELECT 1 FROM silver_session_identity WHERE session_key=?1)",
                    [key],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(rusqlite::Error::InvalidQuery);
                }
            }
            (*snapshot, *snapshot)
        }
        Cursor::Incremental { after } => (*after, *after),
        Cursor::Replay { after, through } => (*after, *through),
    };
    for anchor in [sequence, endpoint] {
        if anchor > current
            || (anchor != 0
                && !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM silver_change WHERE seq=?1)",
                    [anchor],
                    |row| row.get::<_, bool>(0),
                )?)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(cursor)
}

fn encode_checkpoint(checkpoint: &Checkpoint) -> rusqlite::Result<String> {
    serde_json::to_string(checkpoint).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn catalog_row(tx: &rusqlite::Transaction<'_>, key: &str) -> rusqlite::Result<Option<CatalogRow>> {
    let row: Option<CatalogRow> = tx
        .query_row(
            "SELECT metadata FROM session_catalog WHERE session_key=?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|metadata| serde_json::from_str(&metadata).map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()?;
    let Some(mut row) = row else {
        return Ok(None);
    };
    row.repo_id = tx
        .query_row(
            "SELECT repo_id FROM silver_session_state WHERE session_key=?1",
            [key],
            |record| record.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .map(|encoded| serde_json::from_str(&encoded).map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()?;
    Ok(Some(row))
}

fn session_identity(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
) -> rusqlite::Result<(String, String)> {
    if let Some(row) = catalog_row(tx, key)? {
        return native_session_identity(&row);
    }
    tx.query_row(
        "SELECT harness,native_session_id FROM silver_session_identity WHERE session_key=?1",
        [key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

/// Catalog keys disambiguate source-local rows; they are not native session
/// identifiers. A current logical session has one native identity even when
/// several source files contribute to it. Refuse an ambiguous catalog row
/// rather than deriving an identifier from a key prefix.
fn native_session_identity(row: &CatalogRow) -> rusqlite::Result<(String, String)> {
    let native_session_id = row.owner_native_id().ok_or(rusqlite::Error::InvalidQuery)?;
    Ok((row.harness.clone(), native_session_id.into()))
}

fn source_witness(tx: &rusqlite::Transaction<'_>, key: &str) -> rusqlite::Result<CatalogRow> {
    let metadata: String = tx.query_row(
        "SELECT metadata FROM session_activity_sources WHERE session_key=?1",
        [key],
        |row| row.get(0),
    )?;
    serde_json::from_str(&metadata).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn read_steps(
    tx: &rusqlite::Transaction<'_>,
    row: &CatalogRow,
    generation: &str,
    price: &str,
    total: u64,
) -> rusqlite::Result<Vec<native_activity::ActivityStep>> {
    let mut steps = Vec::with_capacity(total as usize);
    while steps.len() < total as usize {
        let page = crate::events::store::session_activity::page(
            tx,
            row,
            Some(generation),
            price,
            steps.len() as u64,
            256,
        )?
        .ok_or(rusqlite::Error::InvalidQuery)?;
        if page.steps.is_empty() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        steps.extend(page.steps);
    }
    Ok(steps)
}

struct AdmittedEvidence<'a> {
    row: CatalogRow,
    witness: CatalogRow,
    price_version: String,
    generation: &'a str,
    steps: Vec<native_activity::ActivityStep>,
    facts: Vec<(
        u64,
        native_activity::ActivityStepId,
        native_activity::StepFacts,
    )>,
    prices: Vec<Price>,
}

/// `None` withholds only a generation whose anchors cannot be represented.
/// Invalid persisted evidence and database failures remain errors.
fn admitted(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    evidence: AdmittedEvidence<'_>,
) -> rusqlite::Result<Option<AdmittedSession>> {
    let AdmittedEvidence {
        row,
        witness,
        price_version,
        generation,
        steps,
        facts,
        prices,
    } = evidence;
    if row.key != witness.key || steps.len() != facts.len() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if facts
        .iter()
        .enumerate()
        .any(|(expected, (ordinal, id, _))| {
            *ordinal != expected as u64 || id != &steps[expected].id
        })
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let session_uid = session_uid(namespace, &native_session_identity(&row)?);
    let fact_by_id: BTreeMap<_, _> = facts.into_iter().map(|(_, id, fact)| (id, fact)).collect();
    if fact_by_id.len() != steps.len() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let repo = repo(tx, &row)?;
    let mut projected_steps = Vec::with_capacity(steps.len());
    for step in &steps {
        let fact = fact_by_id
            .get(&step.id)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let Some(projected) = step_input(&session_uid, &row, step, fact, &price_version)? else {
            return Ok(None);
        };
        projected_steps.push(projected);
    }
    let exclusive = projected_steps
        .iter()
        .filter(|step| step.usage_owner == UsageOwner::Exclusive)
        .count() as u64;
    let unknown = projected_steps
        .iter()
        .filter(|step| step.timestamp.is_none() || step.billing.split_unknown)
        .count() as u64;
    let unclassified = projected_steps
        .iter()
        .filter(|step| step.activity.is_none())
        .count() as u64;
    let Some(compactions) = read_compactions(tx, &session_uid, &row, generation, &price_version)?
    else {
        return Ok(None);
    };
    let usage_complete = projected_steps
        .iter()
        .all(|step| !step.billing.split_unknown);
    Ok(Some(AdmittedSession {
        session_uid: session_uid.clone(),
        source_witness: indexed_source_witness(&witness)?,
        derivation: semon_asi::input::Derivation {
            activity_generation: generation.to_owned(),
            producer_version: env!("CARGO_PKG_VERSION").to_owned(),
            parser_version: Some(crate::events::CACHE_VERSION),
            index_incarnation: None,
            computed_at: None,
        },
        versions: RecordedVersions {
            transform: semon_asi::ANALYTICS_TRANSFORM_VERSION,
            rules: native_activity::ACTIVITY_RULES_VERSION,
            price: price_version,
            taxonomy: None,
            coverage: Coverage {
                session_count: 1,
                step_count: projected_steps.len() as u64,
                exclusive_step_count: exclusive,
                unknown_step_count: unknown,
                unclassified_step_count: unclassified,
            },
        },
        session: session_input(tx, namespace, &session_uid, &row, repo)?,
        steps: projected_steps,
        compactions,
        usage_complete,
        prices,
    }))
}

fn read_compactions(
    tx: &rusqlite::Transaction<'_>,
    session_uid: &str,
    row: &CatalogRow,
    generation: &str,
    price: &str,
) -> rusqlite::Result<Option<Vec<Compaction>>> {
    // The page reader establishes the source/rules/price admission for the
    // same generation before this companion-table read occurs.
    if crate::events::store::session_activity::page(tx, row, Some(generation), price, 0, 0)?
        .is_none()
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let declared: i64 = tx.query_row(
        "SELECT compactions FROM session_activity_projections \
         WHERE session_key=?1 AND generation=?2",
        params![row.key, generation],
        |record| record.get(0),
    )?;
    let mut statement = tx.prepare(
        "SELECT ordinal,metadata FROM session_activity_compactions \
         WHERE session_key=?1 AND generation=?2 ORDER BY ordinal",
    )?;
    let mut records = statement.query(params![row.key, generation])?;
    let mut native_compactions = Vec::new();
    while let Some(record) = records.next()? {
        let ordinal: i64 = record.get(0)?;
        if ordinal != native_compactions.len() as i64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let metadata: String = record.get(1)?;
        let compact: native_activity::Compaction =
            serde_json::from_str(&metadata).map_err(|_| rusqlite::Error::InvalidQuery)?;
        native_compactions.push(compact);
    }
    // Validate physical rows before collapsing repeated native identities.
    // Contiguous surviving ordinals alone cannot detect a truncated tail.
    if native_compactions.len() as i64 != declared {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut compactions: Vec<Compaction> = Vec::new();
    let mut identities: BTreeMap<String, usize> = BTreeMap::new();
    for compact in native_compactions {
        let native_record = compact
            .native_record
            .as_ref()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let Some(anchor) = (match native_record.id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => Some(id.to_owned()),
            None => record_anchor(row, &compact.position, native_record.ordinal)?,
        }) else {
            return Ok(None);
        };
        let projected = Compaction {
            compaction_uid: format!("{session_uid}/c/{}", encode_component(&anchor)),
            step_uid: compact.step_id.as_ref().map(|id| step_uid(session_uid, id)),
            timestamp: compact.timestamp,
        };
        if let Some(&index) = identities.get(&projected.compaction_uid) {
            let existing = &mut compactions[index];
            if existing
                .step_uid
                .as_ref()
                .zip(projected.step_uid.as_ref())
                .is_some_and(|(first, next)| first != next)
                || existing
                    .timestamp
                    .zip(projected.timestamp)
                    .is_some_and(|(first, next)| first != next)
            {
                return Ok(None);
            }
            // Missing evidence is compatible with a known value in either
            // contributor order; retain the first persisted ordinal position.
            existing.step_uid = existing.step_uid.take().or(projected.step_uid);
            existing.timestamp = existing.timestamp.or(projected.timestamp);
        } else {
            // Retain the first occurrence in persisted ordinal order.
            identities.insert(projected.compaction_uid.clone(), compactions.len());
            compactions.push(projected);
        }
    }
    Ok(Some(compactions))
}

fn session_input(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    _uid: &str,
    row: &CatalogRow,
    repo: Repo,
) -> rusqlite::Result<Session> {
    // Native Catalog links have no parent_source. The observed-link producers
    // set ancestry, environment or marker (Semon-Parent in prompt text). None
    // proves native lineage or workspace/time inference; unknown sources are
    // likewise omitted rather than mislabelled under the ASI parent methods.
    let parent = match row.parent_source.as_deref() {
        None => row.parent.as_deref().filter(|parent| *parent != "you"),
        Some(_) => None,
    };
    let parent_session_uid = parent
        .map(|parent| {
            session_identity(tx, parent).map(|identity| session_uid(namespace, &identity))
        })
        .transpose()?;
    let parent_method = if parent_session_uid.is_some() {
        ParentMethod::Native
    } else {
        ParentMethod::None
    };
    Ok(Session {
        machine: namespace.into(),
        harness: row.harness.clone(),
        native_session_id: native_session_identity(row)?.1,
        role: match row.kind.as_str() {
            "subagent" => Role::Worker,
            "codex-run" if parent_method == ParentMethod::Native => Role::Worker,
            "session" | "codex-run" => Role::Solo,
            _ => Role::Unknown,
        },
        parent_session_uid,
        parent_method,
        lane_root_uid: None,
        repo,
        branch: row.branch.clone(),
        // Catalog's last observation is not an end-of-session proof.
        status: Status::Unknown,
        started_at: row.start,
        ended_at: None,
        // Catalog cost is Semon's recorded price estimate, not an admitted
        // harness-reported run cost.
        reported_usd: None,
    })
}

fn repo(tx: &rusqlite::Transaction<'_>, row: &CatalogRow) -> rusqlite::Result<Repo> {
    match row.repo_id.as_ref() {
        Some(RepoId::Remote(id)) => Ok(Repo {
            key: id.clone(),
            repo_id: Some(id.clone()),
            local_path: None,
            // A remote identity can contain path separators, which are valid
            // in its Key-class field but forbidden by the display-name
            // contract. Keep the display label deliberately non-identifying.
            display_name: "remote repository".into(),
        }),
        Some(RepoId::Path(path)) => Ok(Repo {
            // Bind the existing native path identity to the origin-owned,
            // opaque local key. The raw path remains in the typed path field
            // and never becomes a Key-class value.
            key: tx.query_row(
                "SELECT repo_key FROM silver_repo_identity WHERE repo_path=?1",
                [path],
                |record| record.get(0),
            )?,
            repo_id: None,
            local_path: Some(path.clone()),
            display_name: "local".into(),
        }),
        Some(RepoId::Unknown) | None => Ok(Repo {
            key: UNKNOWN_REPO_KEY.into(),
            repo_id: None,
            local_path: None,
            display_name: "unknown".into(),
        }),
    }
}

fn step_input(
    session_uid: &str,
    row: &CatalogRow,
    step: &native_activity::ActivityStep,
    fact: &native_activity::StepFacts,
    price_version: &str,
) -> rusqlite::Result<Option<Step>> {
    let step_uid = step_uid(session_uid, &step.id);
    // Preserve the native initiating turn when available. Claude's persisted
    // step identity is already its native message ID, so a step without an
    // initiating UUID need not fall back to a physical source's record ordinal.
    let native_id = fact
        .turn_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .or(match &step.id {
            native_activity::ActivityStepId::Claude(message) => Some(message.as_str()),
            native_activity::ActivityStepId::Codex(_) => None,
        });
    let Some(turn) = (match native_id.filter(|id| !id.is_empty()) {
        Some(id) => Some(id.to_owned()),
        None => {
            let Some(ordinal) = fact.record_ordinal else {
                return Ok(None);
            };
            record_anchor(row, &step.position, ordinal)?
        }
    }) else {
        return Ok(None);
    };
    Ok(Some(Step {
        step_uid: step_uid.clone(),
        turn_uid: format!("{session_uid}/t/{}", encode_component(&turn)),
        model: Model {
            key: step.model.clone().unwrap_or_else(|| "unknown".into()),
            name: step.model.clone().unwrap_or_else(|| "unknown".into()),
            provider: None,
        },
        timestamp: step.timestamp,
        billing: BillingUsage {
            input_tokens: fact.billing.input,
            output_tokens: fact.billing.output,
            cache_read_tokens: fact.billing.cache_read,
            cache_write_5m_tokens: fact.billing.cache_write_5m,
            cache_write_1h_tokens: fact.billing.cache_write_1h,
            reasoning_tokens: None,
            web_search_requests: fact.billing.web_search_requests,
            speed: fact.billing.speed.clone(),
            service_tier: fact.billing.service_tier.clone(),
            prompt_size_tokens: fact.billing.prompt_size,
            split_unknown: fact.billing.split_unknown,
        },
        usage_owner: match fact.usage_owner {
            native_activity::UsageOwner::Exclusive => UsageOwner::Exclusive,
            native_activity::UsageOwner::Copied => UsageOwner::Copied,
        },
        context_tokens: step.context_tokens,
        context_band: match step.band {
            native_activity::ContextBand::Under50k => ContextBand::Under50k,
            native_activity::ContextBand::From50kTo150k => ContextBand::From50kTo150k,
            native_activity::ContextBand::Over150k => ContextBand::Over150k,
        },
        model_ms: fact.model_ms,
        tool_ms: fact.tool_ms,
        cost: RecordedCost {
            usd_est: step.list_cost_usd,
            price_version: price_version.into(),
            components: [
                ("input", step.fresh_input_usd),
                ("cache_read", step.cache_read_usd),
                ("cache_write", step.cache_write_usd),
                ("output", step.output_usd),
            ]
            .into_iter()
            .filter_map(|(kind, value)| value.map(|value| (kind.into(), value)))
            .collect(),
        },
        activity: step.activity.map(activity),
        waste_kind: step.waste.map(waste),
        trigger: step.trigger.map(trigger),
        ok: (!fact.tool_calls.is_empty() && fact.tool_calls.iter().all(|call| call.ok.is_some()))
            .then(|| fact.tool_calls.iter().all(|call| call.ok == Some(true))),
        tool_calls: fact.tool_calls.iter().map(tool_call).collect(),
    }))
}

/// A fallback record identity is portable native evidence, never a local
/// physical path.  The catalog binds the physical position to exactly one
/// source; the exported anchor uses that native log's filename stem and the
/// source-record ordinal persisted by ingest, never its byte offset.
/// An unrepresentable stem returns `None`; a broken position/source binding
/// remains an integrity error.
fn record_anchor(
    row: &CatalogRow,
    position: &native_activity::SourcePosition,
    record_ordinal: u64,
) -> rusqlite::Result<Option<String>> {
    let source_path = Path::new(&position.source);
    let matching = row
        .sources
        .iter()
        .filter(|source| source.path == source_path)
        .collect::<Vec<_>>();
    let [source] = matching.as_slice() else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let Some(stem) = source
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| {
            (1..=128).contains(&stem.len())
                && stem
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    else {
        return Ok(None);
    };
    // The ASI record-anchor contract names the file stem.  Two physical
    // contributors with the same stem cannot be represented distinctly by
    // that contract, so refuse rather than smuggle a path into a key.
    if row
        .sources
        .iter()
        .filter(|candidate| {
            candidate.path.file_stem().and_then(|value| value.to_str()) == Some(stem)
        })
        .count()
        != 1
    {
        return Ok(None);
    }
    Ok(Some(format!("@{stem}:{record_ordinal}")))
}

fn tool_call(call: &native_activity::ToolCallFact) -> ToolCall {
    ToolCall {
        ord: call.ord,
        tool: call.tool.clone(),
        target_path: call.target_path.clone(),
        command_tag: Some(format!("{:?}", call.command_tag).to_lowercase()),
        ok: call.ok,
        exit_code: call.exit_code,
        tool_ms: call.tool_ms,
    }
}

fn activity(activity: native_activity::classify::Activity) -> Activity {
    match activity {
        native_activity::classify::Activity::Coordinate => Activity::Coordinate,
        native_activity::classify::Activity::Wait => Activity::Wait,
        native_activity::classify::Activity::Integrate => Activity::Integrate,
        native_activity::classify::Activity::Verify => Activity::Verify,
        native_activity::classify::Activity::Recover => Activity::Recover,
        native_activity::classify::Activity::Implement => Activity::Implement,
        native_activity::classify::Activity::Explore => Activity::Explore,
        native_activity::classify::Activity::Setup => Activity::Setup,
        native_activity::classify::Activity::Report => Activity::Report,
        native_activity::classify::Activity::Think => Activity::Think,
    }
}

fn waste(kind: native_activity::classify::WasteKind) -> WasteKind {
    use native_activity::classify::WasteKind as Native;
    match kind {
        Native::OrchestrationWait | Native::ChecksPending | Native::Sleep => WasteKind::Wait,
        Native::PathNotFound
        | Native::MalformedCommand
        | Native::ToolInputRejected
        | Native::Timeout
        | Native::ExternalFailure
        | Native::OtherFailure
        | Native::MergeConflict => WasteKind::FailedCall,
        Native::Retry | Native::FlakyRerun => WasteKind::Retry,
        Native::RedundantRead => WasteKind::RedundantRead,
        Native::Refill => WasteKind::ContextRefill,
    }
}

fn trigger(trigger: native_activity::Trigger) -> Trigger {
    use native_activity::Trigger as Native;
    match trigger {
        Native::ToolError => Trigger::ToolError,
        Native::EmptyResult => Trigger::EmptyResult,
        Native::TestOrCiFailure => Trigger::TestOrCiFailure,
        Native::ExternalError => Trigger::ExternalError,
        Native::Compaction => Trigger::Compaction,
        Native::Polling => Trigger::Polling,
        Native::WorkerResult => Trigger::WorkerResult,
        Native::NoneVisible => Trigger::NoneVisible,
    }
}

fn step_uid(session_uid: &str, id: &native_activity::ActivityStepId) -> String {
    match id {
        native_activity::ActivityStepId::Claude(message) => {
            format!("{session_uid}/s/claude/{}", encode_component(message))
        }
        native_activity::ActivityStepId::Codex([thread, session, turn, response]) => format!(
            "{session_uid}/s/codex/{}/{}/{}/{}",
            encode_component(thread),
            encode_component(session),
            encode_component(turn),
            encode_component(response),
        ),
    }
}

fn session_uid(namespace: &str, identity: &(String, String)) -> String {
    format!(
        "{}/{}/{}",
        encode_component(namespace),
        encode_component(&identity.0),
        encode_component(&identity.1),
    )
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};
    use serde_json::json;

    use super::*;

    fn transaction(connection: &mut Connection) -> rusqlite::Transaction<'_> {
        connection.transaction().unwrap()
    }

    fn catalog_row(
        key: &str,
        harness: &str,
        native_id: &str,
        parent: Option<&str>,
        kind: &str,
    ) -> CatalogRow {
        serde_json::from_value(json!({
            "key": key,
            "name": "synthetic",
            "harness": harness,
            "kind": kind,
            "native_ids": [native_id],
            "parent": parent,
            "parent_source": null,
            "repo": null,
            "branch": null,
            "model": "",
            "effort": null,
            "start": null,
            "last": null,
            "tokens": [0.0, 0.0, 0.0],
            "cost": crate::pricing::Cost::default(),
            "sources": []
        }))
        .unwrap()
    }

    #[test]
    fn repository_dimensions_depend_on_identity_not_checkout() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE silver_repo_identity(\
                    repo_path TEXT PRIMARY KEY, repo_key TEXT NOT NULL UNIQUE\
                 ) STRICT;\
                 INSERT INTO silver_repo_identity VALUES('/srv/base.git','local-base');\
                 INSERT INTO silver_repo_identity VALUES('file:///srv/base.git','local-file');",
            )
            .unwrap();
        let tx = transaction(&mut connection);
        for (identity, key, repo_id, local_path, display_name) in [
            (
                Some(RepoId::Path("/srv/base.git".into())),
                "local-base",
                None,
                Some("/srv/base.git"),
                "local",
            ),
            (
                Some(RepoId::Path("file:///srv/base.git".into())),
                "local-file",
                None,
                Some("file:///srv/base.git"),
                "local",
            ),
            (
                Some(RepoId::Remote("example.test/team/base".into())),
                "example.test/team/base",
                Some("example.test/team/base"),
                None,
                "remote repository",
            ),
            (
                Some(RepoId::Unknown),
                UNKNOWN_REPO_KEY,
                None,
                None,
                "unknown",
            ),
            (None, UNKNOWN_REPO_KEY, None, None, "unknown"),
        ] {
            let mut one = catalog_row("one", "codex", "one", None, "session");
            one.repo = Some("/work/one".into());
            one.repo_id = identity.clone();
            let mut two = catalog_row("two", "codex", "two", None, "session");
            two.repo = Some("/work/two".into());
            two.repo_id = identity;
            let expected = Repo {
                key: key.into(),
                repo_id: repo_id.map(str::to_owned),
                local_path: local_path.map(str::to_owned),
                display_name: display_name.into(),
            };
            assert_eq!(repo(&tx, &one).unwrap(), expected);
            assert_eq!(repo(&tx, &two).unwrap(), expected);
        }
    }

    #[test]
    fn recorded_price_snapshot_is_required_to_match_the_published_header() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session_activity_projections(\
                    session_key TEXT PRIMARY KEY, generation TEXT NOT NULL,\
                    pricing_fingerprint TEXT NOT NULL, pricing_snapshot TEXT NOT NULL\
                 ) STRICT;",
            )
            .unwrap();
        let table = crate::pricing::snapshot();
        let snapshot = crate::pricing::runtime::recorded_snapshot(&table);
        connection
            .execute(
                "INSERT INTO session_activity_projections VALUES(?1,?2,?3,?4)",
                params!["synthetic", "g1", table.fingerprint, snapshot],
            )
            .unwrap();

        let recorded_prices = {
            let tx = transaction(&mut connection);
            prices(&tx, "synthetic", "g1", &table.fingerprint).unwrap()
        };
        assert!(!recorded_prices.is_empty());
        assert!(
            recorded_prices
                .iter()
                .all(|price| price.price_version == table.fingerprint)
        );

        connection
            .execute(
                "UPDATE session_activity_projections SET pricing_fingerprint='partial'",
                [],
            )
            .unwrap();
        let partial = {
            let tx = transaction(&mut connection);
            prices(&tx, "synthetic", "g1", "partial")
        };
        assert!(partial.is_err());
    }

    #[test]
    fn removal_identity_uses_the_persisted_native_identity() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY, metadata TEXT NOT NULL) STRICT;\
                 CREATE TABLE silver_session_identity(\
                    session_key TEXT PRIMARY KEY, harness TEXT NOT NULL, native_session_id TEXT NOT NULL\
                 ) STRICT;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO silver_session_identity VALUES(?1,?2,?3)",
                params!["retired-key", "codex", "native/session"],
            )
            .unwrap();
        let identity = {
            let tx = transaction(&mut connection);
            session_identity(&tx, "retired-key").unwrap()
        };
        assert_eq!(identity, ("codex".into(), "native/session".into()));
        assert_eq!(
            session_uid("machine", &identity),
            "machine/codex/native%2Fsession"
        );
    }

    #[test]
    fn catalog_collisions_use_native_identity_and_recorded_codex_parent() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY, metadata TEXT NOT NULL) STRICT;\
                 CREATE TABLE silver_session_identity(\
                    session_key TEXT PRIMARY KEY, harness TEXT NOT NULL, native_session_id TEXT NOT NULL\
                 ) STRICT;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO silver_session_identity VALUES(?1,?2,?3)",
                params!["parent-catalog-key", "codex", "parent-native"],
            )
            .unwrap();
        let child = catalog_row(
            ":codex:same",
            "codex",
            "same",
            Some("parent-catalog-key"),
            "codex-run",
        );
        let session = {
            let tx = transaction(&mut connection);
            assert_eq!(
                native_session_identity(&child).unwrap(),
                ("codex".into(), "same".into())
            );
            session_input(
                &tx,
                "machine",
                "machine/codex/same",
                &child,
                Repo {
                    key: "unknown".into(),
                    repo_id: None,
                    local_path: None,
                    display_name: "unknown".into(),
                },
            )
            .unwrap()
        };
        assert_eq!(session.native_session_id, "same");
        assert_eq!(
            session.parent_session_uid.as_deref(),
            Some("machine/codex/parent-native")
        );
        assert_eq!(session.role, Role::Worker);
        assert_eq!(session.parent_method, ParentMethod::Native);
        assert!(
            native_session_identity(&CatalogRow {
                native_ids: vec!["one".into(), "two".into()],
                ..child
            })
            .is_err()
        );
    }

    #[test]
    fn parent_methods_preserve_native_lineage_without_promoting_observed_links() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY, metadata TEXT NOT NULL) STRICT;\
             CREATE TABLE silver_session_identity(\
                session_key TEXT PRIMARY KEY, harness TEXT NOT NULL, native_session_id TEXT NOT NULL\
             ) STRICT;\
             INSERT INTO silver_session_identity VALUES('parent-key','codex','parent-native');",
        ).unwrap();
        for (source, parent, method, role) in [
            (None, Some("parent-key"), ParentMethod::Native, Role::Worker),
            (
                Some("ancestry"),
                Some("parent-key"),
                ParentMethod::None,
                Role::Solo,
            ),
            (
                Some("environment"),
                Some("parent-key"),
                ParentMethod::None,
                Role::Solo,
            ),
            (
                Some("marker"),
                Some("parent-key"),
                ParentMethod::None,
                Role::Solo,
            ),
            (
                Some("unknown"),
                Some("parent-key"),
                ParentMethod::None,
                Role::Solo,
            ),
            (None, None, ParentMethod::None, Role::Solo),
            (None, Some("you"), ParentMethod::None, Role::Solo),
        ] {
            let mut row = catalog_row("child", "codex", "child", parent, "codex-run");
            row.parent_source = source.map(str::to_owned);
            let tx = transaction(&mut connection);
            let session = session_input(
                &tx,
                "machine",
                "machine/codex/child",
                &row,
                Repo {
                    key: "unknown".into(),
                    repo_id: None,
                    local_path: None,
                    display_name: "unknown".into(),
                },
            )
            .unwrap();
            assert_eq!(session.parent_method, method, "{source:?}, {parent:?}");
            assert_eq!(session.role, role, "{source:?}, {parent:?}");
            assert_eq!(
                session.parent_session_uid.as_deref(),
                (method == ParentMethod::Native).then_some("machine/codex/parent-native"),
                "unsupported parent evidence is not exported as native lineage"
            );

            // A native subagent kind establishes Worker independently of the
            // parent's provenance; only codex-run promotion depends on it.
            row.kind = "subagent".into();
            assert_eq!(
                session_input(&tx, "machine", "machine/codex/child", &row, session.repo)
                    .unwrap()
                    .role,
                Role::Worker
            );
        }
    }

    #[test]
    fn claude_lineage_owner_identity_and_record_anchors_are_portable() {
        let mut row = catalog_row("root", "claude", "root", None, "session");
        row.native_ids = vec!["cleared".into(), "root".into()];
        assert_eq!(
            native_session_identity(&row).unwrap(),
            ("claude".into(), "root".into()),
            "the recorded lineage owner, not native-ID order, names the session"
        );

        let source = |path: &str, native_id: &str| crate::model::summary::CatalogSource {
            path: path.into(),
            native_id: native_id.into(),
            dev: 1,
            ino: 2,
            size: 3,
            modified_ns: 4,
            changed_ns: None,
            offset: 5,
            prefix_sha256: [0; 32],
            tail_sha256: [0; 32],
            native_role: None,
        };
        row.sources = vec![source("/private/first-root/record.jsonl", "root")];
        let first = record_anchor(
            &row,
            &native_activity::SourcePosition {
                source: "/private/first-root/record.jsonl".into(),
                offset: 900,
            },
            3,
        )
        .unwrap()
        .unwrap();
        row.sources = vec![source("/different/parent/record.jsonl", "root")];
        let moved = record_anchor(
            &row,
            &native_activity::SourcePosition {
                source: "/different/parent/record.jsonl".into(),
                offset: 900,
            },
            3,
        )
        .unwrap()
        .unwrap();
        assert_eq!(first, "@record:3");
        assert_eq!(first, moved, "source relocation must not change an ASI key");
        let first_compaction_uid = format!("session/c/{}", encode_component(&first));
        let moved_compaction_uid = format!("session/c/{}", encode_component(&moved));
        assert_eq!(first_compaction_uid, moved_compaction_uid);
        assert!(!first_compaction_uid.contains("private"));
        assert!(!first_compaction_uid.contains("%2F"));

        row.sources = vec![
            source("/different/parent/record.jsonl", "root"),
            source("/different/parent/cleared.jsonl", "cleared"),
        ];
        let cleared = record_anchor(
            &row,
            &native_activity::SourcePosition {
                source: "/different/parent/cleared.jsonl".into(),
                offset: 900,
            },
            3,
        )
        .unwrap()
        .unwrap();
        assert_eq!(cleared, "@cleared:3");
        assert_ne!(
            first, cleared,
            "distinct native logs retain distinct anchors"
        );

        let position = native_activity::SourcePosition {
            source: "/different/parent/record.jsonl".into(),
            offset: 900,
        };
        row.sources.push(source("/another/record.jsonl", "root"));
        assert!(
            record_anchor(&row, &position, 3).unwrap().is_none(),
            "duplicate stems are ambiguous"
        );
        row.sources.pop();
        row.sources.push(row.sources[0].clone());
        assert!(
            record_anchor(&row, &position, 3).is_err(),
            "duplicate contributors are ambiguous"
        );
        row.sources.pop();
        assert!(
            record_anchor(
                &row,
                &native_activity::SourcePosition {
                    source: "/unwitnessed/record.jsonl".into(),
                    offset: 900,
                },
                3,
            )
            .is_err(),
            "a broken source binding remains an integrity error"
        );
        row.sources = vec![source("/different/parent/invalid stem.jsonl", "root")];
        assert!(
            record_anchor(
                &row,
                &native_activity::SourcePosition {
                    source: "/different/parent/invalid stem.jsonl".into(),
                    offset: 900
                },
                3
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn computed_catalog_cost_is_not_reported_spend() {
        let mut row = catalog_row("priced", "claude", "priced", None, "session");
        row.cost.usd = Some(12.34);
        let mut connection = Connection::open_in_memory().unwrap();
        let session = {
            let tx = transaction(&mut connection);
            session_input(
                &tx,
                "machine",
                "machine/claude/priced",
                &row,
                Repo {
                    key: "unknown".into(),
                    repo_id: None,
                    local_path: None,
                    display_name: "unknown".into(),
                },
            )
            .unwrap()
        };
        assert_eq!(session.reported_usd, None);
    }
}
