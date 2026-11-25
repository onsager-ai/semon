use std::collections::BTreeSet;

use semon_asi::query::{self, Request};
use semon_sessions::Query;
use serde_json::{Map, Value, json};

use super::{AsiMcp, asi_error, asi_store_root};

const SQL: &str = concat!(
    "WITH RECURSIVE root_session AS (
        SELECT session_uid, machine_key FROM dim_session WHERE session_uid = ?
    ), family(session_uid) AS (
        SELECT session_uid FROM root_session
        UNION
        SELECT d.session_uid FROM family f JOIN dim_session d ON d.parent_session_uid = f.session_uid
        JOIN root_session r ON d.machine_key = r.machine_key
    ), steps AS (
        SELECT s.* FROM fact_step s JOIN family f USING (session_uid)
    ), compactions AS (
        SELECT c.session_uid, c.compaction_uid, epoch_ms(c.ts) AS ts_ms, c.trigger, c.pre_tokens
        FROM fact_compaction c JOIN family f USING (session_uid)
    ),\n",
    include_str!("compaction_context.sql"),
    include_str!("lane_health.sql"),
);

fn native_identity(uid: &str) -> Option<&str> {
    let (origin, identity) = uid.split_once('/')?;
    let (harness, id) = identity.split_once('/')?;
    (!origin.is_empty() && !harness.is_empty() && !id.is_empty() && !id.contains('/'))
        .then_some(identity)
}

pub(super) fn report(
    extension: &AsiMcp,
    native: &Query,
    arguments: &Value,
) -> Result<Value, Value> {
    if extension.remote {
        return Err(asi_error(
            "capability_unavailable",
            "Lane health requires a local origin",
            "origin",
        ));
    }
    let uid = arguments
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("root_session_uid"))
        .and_then(Value::as_str)
        .filter(|uid| native_identity(uid).is_some())
        .ok_or_else(|| {
            asi_error(
                "invalid_argument",
                "A full ASI root session UID is required",
                "root_session_uid",
            )
        })?;
    let store = asi_store_root(&extension.index)
        .ok_or_else(|| asi_error("store_unavailable", "Analytics unavailable", "store"))?;
    let mut request = Request::new(SQL);
    request.params.push(json!(uid));
    let result = query::run(&extension.executable, &store, &request)
        .map_err(|failure| serde_json::to_value(failure).expect("query failure serializes"))?;
    if result.truncated {
        return Err(asi_error(
            "resource_limit",
            "Lane health report exceeds the query response limit",
            "report",
        ));
    }
    let row = result.rows.into_iter().next().ok_or_else(|| {
        asi_error(
            "unknown_session",
            "Root session is absent from the analytics snapshot",
            "root_session_uid",
        )
    })?;
    let mut report: Map<_, _> = result
        .columns
        .into_iter()
        .map(|column| column.name)
        .zip(row)
        .collect();
    let members = report
        .get_mut("members")
        .and_then(Value::as_array_mut)
        .expect("lane health query returns a member list");
    let identities: BTreeSet<_> = members
        .iter()
        .filter_map(|member| native_identity(member["session_uid"].as_str()?))
        .collect();
    let mut runtime = match native.lane_runtime(&identities) {
        Ok(value) => value,
        Err(error) => {
            json!({"now":null,"observed_at":null,"window_start":null,"error_code":error.code})
        }
    };
    let sessions = runtime
        .as_object_mut()
        .and_then(|value| value.remove("sessions"));
    for member in members.iter_mut() {
        let live = member["session_uid"]
            .as_str()
            .and_then(native_identity)
            .and_then(|identity| sessions.as_ref()?.get(identity));
        member["live"] = live.cloned().unwrap_or(Value::Null);
    }
    let pairs: Vec<_> = members
        .iter()
        .filter_map(|member| {
            let worker = member["session_uid"].as_str()?;
            (worker != uid && member["role"] == "worker")
                .then_some((worker, member["parent_session_uid"].as_str()?))
        })
        .collect();
    let outcomes = semon_sessions::origin_projection::worker_tasks(&extension.index, &pairs)
        .map_err(|_| {
            asi_error(
                "store_unavailable",
                "Worker outcome evidence unavailable",
                "index",
            )
        })?;
    let mut counts = json!({"completed":0, "blocked":0, "cancelled":0, "unknown":0,
        "stops_without_blocker":0, "harness_failed":0});
    let mut tasks = Vec::with_capacity(outcomes.len());
    for ((worker, parent), outcome) in pairs.into_iter().zip(outcomes) {
        let count = &mut counts[&outcome.disposition];
        *count = json!(count.as_u64().expect("known disposition") + 1);
        if outcome.harness_failed {
            counts["harness_failed"] = json!(counts["harness_failed"].as_u64().unwrap() + 1);
        }
        let mut task = json!(outcome);
        task["session_uid"] = json!(worker);
        task["parent_session_uid"] = json!(parent);
        tasks.push(task);
    }
    counts["stops_without_blocker"] = counts["completed"].clone();
    report.insert("worker_tasks".into(), json!(tasks));
    report.insert("worker_stops".into(), counts);
    report.insert("runtime".into(), runtime);
    report.insert("snapshot".into(), json!(result.snapshot));
    report.insert(
        "asi_schema_version".into(),
        json!(result.asi_schema_version),
    );
    report.insert("data_notice".into(), json!(result.data_notice));
    Ok(Value::Object(report))
}
