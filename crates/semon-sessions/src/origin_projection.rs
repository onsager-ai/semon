//! SQLite implementation of `semon_asi`'s admitted input contract.
//!
//! This boundary intentionally reads the activity page and StepFacts readers
//! rather than their backing rows.  Those readers are where current-format,
//! source cohort, rules, and pricing admission is enforced.

use std::{collections::BTreeMap, path::Path};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use semon_asi::input::{
    Activity, AdmittedBatch, AdmittedSession, AnalyticsInput, BillingUsage, Compaction,
    ContextBand, Coverage, Model, ParentMethod, Price, PriceUnit, RateKind, Reconciliation,
    RecordedCost, RecordedVersions, Repo, Role, Session, SessionChange, Source, SourceWitness,
    Status, Step, ToolCall, Trigger, UsageOwner, WasteKind,
};

use crate::{
    events::session_activity as native_activity,
    facts::RepoId,
    model::summary::{CatalogLifecycle, CatalogRow},
};

/// A read-only input over one native SQLite index and one machine namespace.
///
/// The namespace is supplied by the owner that configured the index.  It is
/// not inferred from paths or host state.
pub struct SqliteAnalyticsInput {
    namespace: String,
    connection: Connection,
}

impl SqliteAnalyticsInput {
    pub fn open(index: &Path, namespace: impl Into<String>) -> rusqlite::Result<Self> {
        let connection = Connection::open_with_flags(
            index,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
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

    fn read_admitted(&mut self, checkpoint: Option<&str>) -> rusqlite::Result<AdmittedBatch> {
        let namespace = self.namespace.clone();
        crate::events::store::read_silver_changes(
            &mut self.connection,
            checkpoint
                .map(decode_checkpoint)
                .transpose()?
                .map_or(0, |(_, sequence)| sequence),
            |tx, incarnation, changes| {
                let previous = checkpoint.map(decode_checkpoint).transpose()?;
                let full = previous.is_none_or(|(old, _)| old != incarnation);
                let mut changed = BTreeMap::<String, String>::new();
                if full {
                    let mut statement = tx.prepare(
                        "SELECT session_key FROM session_catalog WHERE lifecycle='current' \
                         UNION SELECT session_key FROM session_activity_projections \
                         ORDER BY session_key",
                    )?;
                    for key in statement.query_map([], |row| row.get::<_, String>(0))? {
                        changed.insert(key?, "changed".into());
                    }
                } else {
                    for (_, key, kind) in changes {
                        changed.insert(key.clone(), kind.clone());
                    }
                }

                let mut result = Vec::with_capacity(changed.len());
                for (key, kind) in changed {
                    let identity = session_identity(tx, &key)?;
                    let uid = session_uid(&namespace, &identity);
                    if kind != "changed" && kind != "removed" {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    let Some(row) = catalog_row(tx, &key)? else {
                        if kind == "removed" {
                            result.push(SessionChange::Removed { session_uid: uid });
                        } else {
                            result.push(SessionChange::Pending { session_uid: uid });
                        }
                        continue;
                    };
                    // A Catalog row is still a live identity.  A `removed`
                    // journal entry may only delete an identity that is no
                    // longer in Catalog; while replacement activity is pending,
                    // retain the previous admitted facts.
                    if kind == "removed" {
                        result.push(SessionChange::Pending { session_uid: uid });
                        continue;
                    }
                    if row.lifecycle != CatalogLifecycle::Current {
                        result.push(SessionChange::SourceMissing {
                            session_uid: uid,
                            retained_at_origin: true,
                        });
                        continue;
                    }
                    match crate::events::store::session_activity::source_missing(tx, &key)? {
                        Some(true) => {
                            result.push(SessionChange::SourceMissing {
                                session_uid: uid,
                                retained_at_origin: true,
                            });
                            continue;
                        }
                        None => {
                            result.push(SessionChange::Pending { session_uid: uid });
                            continue;
                        }
                        Some(false) => {}
                    }
                    let price = crate::pricing::snapshot();
                    let Some(header) = crate::events::store::session_activity::page(
                        tx,
                        &row,
                        None,
                        &price.fingerprint,
                        0,
                        0,
                    )?
                    else {
                        result.push(SessionChange::Pending { session_uid: uid });
                        continue;
                    };
                    let Some(facts) = crate::events::store::session_activity::step_facts(
                        tx,
                        &row,
                        &header.generation,
                    )?
                    else {
                        result.push(SessionChange::Pending { session_uid: uid });
                        continue;
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
                    let witness = source_witness(tx, &key)?;
                    let prices = prices(tx, &key, &header.generation, &price.fingerprint)?;
                    result.push(SessionChange::SourceRestored { session_uid: uid });
                    result.push(SessionChange::Admitted(admitted(
                        tx,
                        &namespace,
                        AdmittedEvidence {
                            row,
                            witness,
                            price_version: price.fingerprint.clone(),
                            generation: &header.generation,
                            steps,
                            facts,
                            prices,
                        },
                    )?));
                }
                let sequence = tx.query_row(
                    "SELECT COALESCE(MAX(seq),0) FROM silver_change",
                    [],
                    |row| row.get(0),
                )?;
                Ok(AdmittedBatch {
                    namespace,
                    checkpoint: encode_checkpoint(incarnation, sequence),
                    reconciliation: if full {
                        Reconciliation::ReplaceNamespace
                    } else {
                        Reconciliation::Incremental
                    },
                    changes: result,
                })
            },
        )
    }
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

fn decode_checkpoint(value: &str) -> rusqlite::Result<(String, i64)> {
    let Some((incarnation, sequence)) = value.rsplit_once(':') else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    if incarnation.len() != 32
        || !incarnation.bytes().all(|byte| byte.is_ascii_hexdigit())
        || sequence
            .parse::<i64>()
            .ok()
            .filter(|sequence| *sequence >= 0)
            .is_none()
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok((
        incarnation.into(),
        sequence.parse().expect("validated sequence"),
    ))
}

fn encode_checkpoint(incarnation: &str, sequence: i64) -> String {
    format!("{incarnation}:{sequence}")
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

fn admitted(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    evidence: AdmittedEvidence<'_>,
) -> rusqlite::Result<AdmittedSession> {
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
    let projected_steps = steps
        .iter()
        .map(|step| {
            let fact = fact_by_id
                .get(&step.id)
                .ok_or(rusqlite::Error::InvalidQuery)?;
            step_input(&session_uid, &row, step, fact, &price_version)
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
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
    let source_generation = crate::retention::source_generation(&witness.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let compactions = read_compactions(tx, &session_uid, &row, generation, &price_version)?;
    let usage_complete = projected_steps
        .iter()
        .all(|step| !step.billing.split_unknown);
    Ok(AdmittedSession {
        session_uid: session_uid.clone(),
        source_witness: SourceWitness {
            generation: source_generation,
            sources: witness
                .sources
                .iter()
                .map(|source| Source {
                    native_id: source.native_id.clone(),
                    local_path: source.path.to_string_lossy().into_owned(),
                    consumed_offset: source.offset,
                    prefix_sha256: hex_bytes(&source.prefix_sha256),
                })
                .collect(),
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
    })
}

fn read_compactions(
    tx: &rusqlite::Transaction<'_>,
    session_uid: &str,
    row: &CatalogRow,
    generation: &str,
    price: &str,
) -> rusqlite::Result<Vec<Compaction>> {
    // The page reader establishes the source/rules/price admission for the
    // same generation before this companion-table read occurs.
    if crate::events::store::session_activity::page(tx, row, Some(generation), price, 0, 0)?
        .is_none()
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut statement = tx.prepare(
        "SELECT ordinal,metadata FROM session_activity_compactions \
         WHERE session_key=?1 AND generation=?2 ORDER BY ordinal",
    )?;
    let mut records = statement.query(params![row.key, generation])?;
    let mut compactions = Vec::new();
    while let Some(record) = records.next()? {
        let ordinal: i64 = record.get(0)?;
        if ordinal != compactions.len() as i64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let metadata: String = record.get(1)?;
        let compact: native_activity::Compaction =
            serde_json::from_str(&metadata).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let native_record = compact
            .native_record
            .as_ref()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let anchor = match native_record.id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => id.to_owned(),
            None => record_anchor(row, &compact.position, native_record.ordinal)?,
        };
        compactions.push(Compaction {
            compaction_uid: format!("{session_uid}/c/{}", encode_component(&anchor)),
            step_uid: compact.step_id.as_ref().map(|id| step_uid(session_uid, id)),
            timestamp: compact.timestamp,
        });
    }
    Ok(compactions)
}

fn session_input(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    _uid: &str,
    row: &CatalogRow,
    repo: Repo,
) -> rusqlite::Result<Session> {
    let parent_session_uid = row
        .parent
        .as_deref()
        .filter(|parent| *parent != "you")
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
            "codex-run" if parent_session_uid.is_some() => Role::Worker,
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
            local_path: row.repo.clone(),
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
            local_path: row.repo.clone(),
            display_name: "local".into(),
        }),
        Some(RepoId::Unknown) | None => Ok(Repo {
            key: "unknown".into(),
            repo_id: None,
            local_path: row.repo.clone(),
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
) -> rusqlite::Result<Step> {
    let step_uid = step_uid(session_uid, &step.id);
    let turn = match &fact.turn_id {
        Some(turn_id) => turn_id.clone(),
        None => record_anchor(
            row,
            &step.position,
            fact.record_ordinal.ok_or(rusqlite::Error::InvalidQuery)?,
        )?,
    };
    Ok(Step {
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
    })
}

/// A fallback record identity is portable native evidence, never a local
/// physical path.  The catalog binds the physical position to exactly one
/// source; the exported anchor uses that native log's filename stem and the
/// source-record ordinal persisted by ingest, never its byte offset.
fn record_anchor(
    row: &CatalogRow,
    position: &native_activity::SourcePosition,
    record_ordinal: u64,
) -> rusqlite::Result<String> {
    let source_path = Path::new(&position.source);
    let matching = row
        .sources
        .iter()
        .filter(|source| source.path == source_path)
        .collect::<Vec<_>>();
    let [source] = matching.as_slice() else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let stem = source
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| {
            (1..=128).contains(&stem.len())
                && stem
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .ok_or(rusqlite::Error::InvalidQuery)?;
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
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(format!("@{stem}:{record_ordinal}"))
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

fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
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
        assert!(
            native_session_identity(&CatalogRow {
                native_ids: vec!["one".into(), "two".into()],
                ..child
            })
            .is_err()
        );
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
            record_anchor(&row, &position, 3).is_err(),
            "duplicate stems are ambiguous"
        );
        row.sources.pop();
        row.sources.push(row.sources[0].clone());
        assert!(
            record_anchor(&row, &position, 3).is_err(),
            "duplicate contributors are ambiguous"
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
            .is_err()
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
