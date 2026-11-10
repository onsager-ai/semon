//! SQLite implementation of `semon_asi`'s admitted input contract.
//!
//! This boundary intentionally reads the activity page and StepFacts readers
//! rather than their backing rows.  Those readers are where current-format,
//! source cohort, rules, and pricing admission is enforced.

use std::{collections::BTreeMap, path::Path};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use semon_asi::input::{
    Activity, AdmittedBatch, AdmittedSession, AnalyticsInput, BillingUsage, Compaction,
    ContextBand, Coverage, Model, ParentMethod, RecordedCost, RecordedVersions, Repo, Role,
    Session, SessionChange, Source, SourceWitness, Status, Step, ToolCall, Trigger, UsageOwner,
    WasteKind,
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
                        "SELECT session_key FROM session_activity_projections ORDER BY session_key",
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
                    let uid = session_uid(&namespace, &session_identity(tx, &key)?);
                    if kind == "removed" {
                        result.push(SessionChange::Removed { session_uid: uid });
                        continue;
                    }
                    if kind != "changed" {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    let Some(row) = catalog_row(tx, &key)? else {
                        result.push(SessionChange::SourceMissing { session_uid: uid });
                        continue;
                    };
                    if row.lifecycle != CatalogLifecycle::Current {
                        result.push(SessionChange::SourceMissing { session_uid: uid });
                        continue;
                    }
                    match crate::events::store::session_activity::source_missing(tx, &key)? {
                        Some(true) => {
                            result.push(SessionChange::SourceMissing { session_uid: uid });
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
                    result.push(SessionChange::SourceRestored { session_uid: uid });
                    result.push(SessionChange::Admitted(admitted(
                        tx,
                        &namespace,
                        row,
                        witness,
                        price.fingerprint.clone(),
                        &header.generation,
                        steps,
                        facts,
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
                    changes: result,
                })
            },
        )
    }
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
    let row = tx
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
    Ok(catalog_row(tx, key)?
        .map(|row| (row.harness, row.key))
        .unwrap_or_else(|| ("unknown".into(), key.into())))
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

fn admitted(
    tx: &rusqlite::Transaction<'_>,
    namespace: &str,
    row: CatalogRow,
    witness: CatalogRow,
    price_version: String,
    generation: &str,
    steps: Vec<native_activity::ActivityStep>,
    facts: Vec<(
        u64,
        native_activity::ActivityStepId,
        native_activity::StepFacts,
    )>,
) -> rusqlite::Result<AdmittedSession> {
    if row.key != witness.key || steps.len() != facts.len() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let session_uid = session_uid(namespace, &(row.harness.clone(), row.key.clone()));
    let fact_by_id: BTreeMap<_, _> = facts.into_iter().map(|(_, id, fact)| (id, fact)).collect();
    if fact_by_id.len() != steps.len() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let repo = repo(&row);
    let projected_steps = steps
        .iter()
        .map(|step| {
            let fact = fact_by_id
                .get(&step.id)
                .ok_or(rusqlite::Error::InvalidQuery)?;
            Ok(step_input(&session_uid, step, fact, &price_version))
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let exclusive = projected_steps
        .iter()
        .filter(|step| step.usage_owner == UsageOwner::Exclusive)
        .count() as u64;
    let unknown = projected_steps
        .iter()
        .filter(|step| step.timestamp.is_none())
        .count() as u64;
    let unclassified = projected_steps
        .iter()
        .filter(|step| step.activity.is_none())
        .count() as u64;
    let source_generation = crate::retention::source_generation(&witness.sources)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let compactions = read_compactions(tx, &session_uid, &row, generation, &price_version)?;
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
            transform: crate::events::CACHE_VERSION,
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
        session: session_input(namespace, &session_uid, &row, repo),
        steps: projected_steps,
        compactions,
        // A later current-format index column records the exact table.  Until
        // then the adapter does not synthesize dim_price from live rates.
        prices: Vec::new(),
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
        let anchor = format!("@{}:{}", compact.position.source, compact.position.offset);
        compactions.push(Compaction {
            compaction_uid: format!("{session_uid}/c/{}", encode_component(&anchor)),
            step_uid: compact.step_id.as_ref().map(|id| step_uid(session_uid, id)),
            timestamp: compact.timestamp,
        });
    }
    Ok(compactions)
}

fn session_input(namespace: &str, _uid: &str, row: &CatalogRow, repo: Repo) -> Session {
    let parent_session_uid = row
        .parent
        .as_ref()
        .map(|parent| session_uid(namespace, &(row.harness.clone(), parent.clone())));
    let parent_method = if parent_session_uid.is_some() {
        ParentMethod::Native
    } else {
        ParentMethod::None
    };
    Session {
        machine: namespace.into(),
        harness: row.harness.clone(),
        native_session_id: row.key.clone(),
        role: match row.kind.as_str() {
            "subagent" => Role::Worker,
            "session" | "codex-run" => Role::Solo,
            _ => Role::Unknown,
        },
        parent_session_uid,
        parent_method,
        lane_root_uid: None,
        repo,
        branch: row.branch.clone(),
        status: if row.last.is_some() {
            Status::Ended
        } else {
            Status::Unknown
        },
        started_at: row.start,
        ended_at: row.last,
        reported_usd: row.cost.usd,
    }
}

fn repo(row: &CatalogRow) -> Repo {
    match row.repo_id.as_ref() {
        Some(RepoId::Remote(id)) => Repo {
            key: id.clone(),
            repo_id: Some(id.clone()),
            local_path: row.repo.clone(),
            display_name: id.clone(),
        },
        Some(RepoId::Path(path)) => Repo {
            // The current index does not yet retain the stable random local
            // ID required by dim_repo.  Keep the path in its typed field and
            // make the missing key explicit instead of using the path as one.
            key: "unknown".into(),
            repo_id: None,
            local_path: Some(path.clone()),
            display_name: "unknown".into(),
        },
        Some(RepoId::Unknown) | None => Repo {
            key: "unknown".into(),
            repo_id: None,
            local_path: row.repo.clone(),
            display_name: "unknown".into(),
        },
    }
}

fn step_input(
    session_uid: &str,
    step: &native_activity::ActivityStep,
    fact: &native_activity::StepFacts,
    price_version: &str,
) -> Step {
    let step_uid = step_uid(session_uid, &step.id);
    let turn = fact
        .turn_id
        .clone()
        .unwrap_or_else(|| format!("@{}:{}", step.position.source, step.position.offset));
    Step {
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
            reasoning_tokens: step.tokens.output.saturating_sub(fact.billing.output),
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
        ok: {
            let outcomes: Vec<_> = fact.tool_calls.iter().filter_map(|call| call.ok).collect();
            (!outcomes.is_empty()).then(|| outcomes.into_iter().all(|ok| ok))
        },
        tool_calls: fact.tool_calls.iter().map(tool_call).collect(),
    }
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
