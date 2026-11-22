//! Origin warehouse storage. Its source keys and maintenance are local to one Index.

#[path = "warehouse_identity.rs"]
pub(super) mod identity;
#[path = "warehouse_maintenance.rs"]
mod maintenance;

#[cfg(test)]
pub(crate) use identity::{
    ObservedPeer, RetainedIdentity, RetainedIdentityError, RetainedPeerError, retained_identity,
    retained_peer,
};

use std::sync::OnceLock;

use rusqlite::{Connection, OptionalExtension};

#[cfg(test)]
thread_local! {
    pub(crate) static SCALE_TIMING: std::cell::Cell<Option<(u128, u128, u64)>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) const DETAIL: &str = include_str!("warehouse_detail.sql");

pub(super) const SCHEMA: &str = include_str!("warehouse.sql");

/// Check the authored DDL, including keys and indexes, on every qualified open.
/// The SQL text in sqlite_schema omits the statement terminator.
pub(super) fn qualify(connection: &Connection) -> rusqlite::Result<bool> {
    // The authored schema uses semicolons only as statement terminators.
    static CONTRACT: OnceLock<(String, Vec<&str>)> = OnceLock::new();
    let (sql, expected) = CONTRACT.get_or_init(|| {
        let expected: Vec<_> = SCHEMA
            .split(';')
            .filter_map(|statement| {
                let sql = statement.get(statement.find("CREATE ")?..)?.trim_end();
                let name = sql.split_ascii_whitespace().nth(2)?.split('(').next()?;
                Some([name, sql])
            })
            .flatten()
            .collect();
        let values = vec!["(?,?)"; expected.len() / 2].join(",");
        (
            format!("SELECT count(*) FROM sqlite_schema WHERE (name,sql) IN (VALUES {values})"),
            expected,
        )
    });
    let matches: i64 = connection
        .prepare_cached(sql)?
        .query_row(rusqlite::params_from_iter(expected), |row| row.get(0))?;
    Ok(matches == (expected.len() / 2) as i64)
}

use std::path::Path;

use rusqlite::{Params, Transaction, params};

use super::{FileIndex, Ledger, int, json};
use crate::{
    events::{
        DENIED, Kind, SEND_FAILED,
        warehouse::{Batch, Fact, Observation, Sidecar},
    },
    native_timing::Measurements,
};

// The same position may be replayed, but an existing row with a different
// payload violates the immutable key's NOT NULL constraint and aborts admission.
// Every column participates in equality, including nullable correlation fields.
macro_rules! fact {
    (@bind $column:ident) => { ",?" };
    ($tx:expr, $table:ident, [source => $source:expr, $($column:ident => $value:expr),+ $(,)?]) => {{
        const SQL: &str = concat!(
            "INSERT INTO ", stringify!($table), "(source", $(",", stringify!($column),)+
            ") VALUES(?", $( fact!(@bind $column), )+
            ") ON CONFLICT DO UPDATE SET source=NULL WHERE NOT (true",
            $(" AND ", stringify!($table), ".", stringify!($column), " IS excluded.", stringify!($column),)+
            ")",
        );
        // The key is supplied first by the enclosing position writer.
        put($tx, SQL, params![$source, $($value),+])
    }};
}

fn put(tx: &Transaction<'_>, sql: &str, values: impl Params) -> rusqlite::Result<()> {
    tx.prepare_cached(sql)?.execute(values)?;
    Ok(())
}

fn source(
    tx: &Transaction<'_>,
    path: &str,
    harness: &str,
    ledger: &str,
    append: bool,
) -> rusqlite::Result<(i64, Option<i64>)> {
    let current = tx.prepare_cached(
        "SELECT s.source,s.harness,s.ledger FROM warehouse_current c JOIN warehouse_source s USING(source) WHERE c.path=?1",
    )?.query_row([path], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()?;
    if let Some((source, old_harness, old_ledger)) = &current
        && old_harness == harness
        && (append || old_ledger == ledger)
    {
        if old_ledger != ledger {
            tx.prepare_cached("UPDATE warehouse_source SET ledger=?1 WHERE source=?2")?
                .execute(params![ledger, source])?;
        }
        return Ok((*source, None));
    }
    let retired = current.map(|(source, _, _)| source);
    let source = tx
        .prepare_cached(
            "INSERT INTO warehouse_source(origin,harness,logical_source,incarnation,carrier,ledger)
         SELECT ?4,
                ?2,?1,coalesce((SELECT max(incarnation) FROM warehouse_source
                                 WHERE logical_source=?1 AND harness=?2),0)+1,?1,?3
         RETURNING source",
        )?
        .query_row(
            params![path, harness, ledger, super::origin_namespace(tx)?],
            |row| row.get(0),
        )?;
    tx.prepare_cached("INSERT INTO warehouse_current(path,source) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET source=excluded.source")?
        .execute(params![path,source])?;
    Ok((source, retired))
}

pub(crate) const PREFIX_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;

fn write_prefix(tx: &Transaction<'_>, source: i64, pos: i64, ledger: &str) -> rusqlite::Result<()> {
    let now = crate::model::now_ms();
    put(
        tx,
        "UPDATE warehouse_prefix SET superseded=1 WHERE source=?1 AND superseded=0",
        [source],
    )?;
    put(
        tx,
        "INSERT INTO warehouse_prefix(source,pos,ledger,committed_at,superseded) VALUES(?1,?2,?3,?4,0)",
        params![source, pos, ledger, now],
    )?;
    let cutoff = now.saturating_sub(PREFIX_WINDOW_MS);
    let mut expired = tx.prepare_cached(
        "DELETE FROM warehouse_prefix WHERE revision IN (
           SELECT revision FROM warehouse_prefix WHERE superseded=1 AND committed_at<?1
           ORDER BY committed_at,revision LIMIT 128
         ) RETURNING revision",
    )?;
    let mut rows = expired.query([cutoff])?;
    let mut floor = 0;
    while let Some(row) = rows.next()? {
        floor = floor.max(row.get::<_, i64>(0)?);
    }
    if floor > 0 {
        put(
            tx,
            "INSERT INTO meta(key,value) VALUES('warehouse_prefix_pruned_through',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value
             WHERE CAST(meta.value AS INTEGER)<CAST(excluded.value AS INTEGER)",
            [floor],
        )?;
    }
    put(tx,
        "DELETE FROM warehouse_signal WHERE (source,seq,revision) IN (
           SELECT s.source,s.seq,s.revision FROM warehouse_signal s
           WHERE s.retired_revision<=CAST((SELECT value FROM meta WHERE key='warehouse_prefix_pruned_through') AS INTEGER)
             AND NOT EXISTS (SELECT 1 FROM warehouse_identity_member m
                 WHERE m.source=s.source AND m.revision>=s.revision AND m.revision<s.retired_revision)
           ORDER BY s.retired_revision LIMIT 128
         )",
        [],
    )?;
    Ok(())
}

pub(super) fn write_signals(
    tx: &Transaction<'_>,
    path: &str,
    index: &FileIndex,
    changes: &super::super::DirtyRows,
) -> rusqlite::Result<()> {
    if changes.signals.is_empty() && changes.before_signals == index.signals.len() {
        return Ok(());
    }
    let bound: Option<(i64, i64)> = tx
        .prepare_cached(
            "SELECT c.source,p.revision FROM warehouse_current c
         CROSS JOIN warehouse_prefix p ON p.source=c.source AND p.superseded=0 WHERE c.path=?1",
        )?
        .query_row([path], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((source, revision)) = bound else {
        return Ok(());
    };
    let mut close = tx.prepare_cached(
        "UPDATE warehouse_signal SET retired_revision=?3
         WHERE source=?1 AND seq=?2 AND retired_revision IS NULL AND revision<?3",
    )?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO warehouse_signal(source,seq,revision,pos,payload) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(source,seq,revision) DO UPDATE SET pos=excluded.pos,payload=excluded.payload",
    )?;
    super::changed(
        changes.before_signals,
        &changes.signals,
        &index.signals,
        |seq, signal| {
            close.execute(params![source, seq as i64, revision])?;
            insert.execute(params![
                source,
                seq as i64,
                revision,
                int(signal.o),
                json(signal)?
            ])?;
            Ok(())
        },
        |from| {
            put(
                tx,
                "UPDATE warehouse_signal SET retired_revision=?3
          WHERE source=?1 AND seq>=?2 AND retired_revision IS NULL",
                params![source, from as i64, revision],
            )
        },
    )?;
    put(
        tx,
        "WITH latest AS MATERIALIZED (
           SELECT lower(trim(json_extract(payload,'$.n'))) AS effort
           FROM warehouse_signal
           WHERE source=?1 AND retired_revision IS NULL AND json_extract(payload,'$.k')='effort'
           ORDER BY pos DESC,seq DESC LIMIT 1
         )
         UPDATE dim_session SET effort=(SELECT effort FROM latest)
         WHERE source=?1 AND effort IS NOT (SELECT effort FROM latest)",
        [source],
    )
}

pub(super) fn write(
    tx: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    index: &FileIndex,
    append: bool,
    previous_sidecar: Option<&crate::native_sidecar::ClaudeSidecar>,
    measurements: &mut Measurements,
) -> rusqlite::Result<()> {
    let Some(batch) = &index.warehouse_observations else {
        return Ok(());
    };
    if batch.harness == "copilot"
        && index
            .copilot
            .as_ref()
            .and_then(|index| index.next_ordinal)
            .is_none()
    {
        return withdraw(tx, path);
    }
    let started = measurements.start();
    #[cfg(test)]
    let fact_started = SCALE_TIMING.with(|timing| timing.get().map(|_| std::time::Instant::now()));
    let encoded_ledger = json(ledger)?;
    let (source, retired) = source(tx, path, batch.harness, &encoded_ledger, append)?;
    let cursor: Option<(i64, i64)> = tx
        .prepare_cached("SELECT pos,sidecar_revision FROM maintenance_cursor WHERE source=?1")?
        .query_row([source], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let (old_offset, sidecar_revision) = cursor.unwrap_or_default();
    if cursor.is_none() || old_offset != int(ledger.offset) {
        write_prefix(tx, source, int(ledger.offset), &encoded_ledger)?;
    }
    let path = Path::new(path);
    let mut header_kind = None;
    if batch.harness == "claude" {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let id = if crate::native_sidecar::agent_path(path) {
                stem.strip_prefix("agent-").unwrap_or(stem)
            } else {
                stem
            };
            session(tx, source, "physical", -1, 0, None, id, None)?;
        }
    } else if let Some(header) = &batch.header {
        let kind = if header.complete {
            "header".to_owned()
        } else {
            let stat = ledger.stat;
            format!(
                "provisional-header:{}:{}:{}:{:?}:{}",
                stat.dev, stat.ino, stat.modified_ns, stat.changed_ns, stat.size
            )
        };
        let metadata =
            serde_json::to_value(&header.metadata).map_err(|_| rusqlite::Error::InvalidQuery)?;
        // Provisional bytes can change repeatedly without advancing the log.
        // The complete header, when present, owns the fixed position.
        session(
            tx,
            source,
            &kind,
            -1,
            0,
            None,
            &header.native_id,
            Some(&metadata),
        )?;
        header_kind = Some(kind);
    }
    let mut record_uuid = None;
    let mut record_offset = None;
    for observation in &batch.observations {
        if record_offset != Some(observation.position.offset) {
            record_offset = Some(observation.position.offset);
            record_uuid = None;
        }
        if let Fact::Record { uuid, .. } = &observation.fact {
            record_uuid = uuid.as_deref();
        }
        write_fact(tx, source, batch, observation, record_uuid)?;
    }
    let eof_witness = if batch.eof.is_empty() {
        None
    } else {
        Some(json(&ledger.stat)?)
    };
    if let Some(witness) = &eof_witness {
        for observation in &batch.eof {
            let (kind, parent_harness, parent_id, call_id, tool) = match &observation.fact {
                Fact::Parent {
                    harness,
                    id,
                    call_id,
                } => (
                    "marker",
                    Some(harness.as_str()),
                    Some(id.as_str()),
                    call_id.as_deref(),
                    None,
                ),
                Fact::LaunchCandidate { call_id, tool } => (
                    "launch",
                    None,
                    None,
                    Some(call_id.as_str()),
                    Some(tool.as_str()),
                ),
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            fact!(tx,fact_eof_observation,[source=>source,pos=>int(observation.position.offset),
                block=>observation.position.block,kind=>kind,occurrence=>observation.discriminator as i64,
                witness=>witness,parent_harness=>parent_harness,parent_id=>parent_id,call_id=>call_id,tool=>tool])?;
        }
    }
    let sidecar_revision = if let Some(sidecar) = &batch.sidecar
        && (sidecar_revision == 0 || previous_sidecar != index.claude_sidecar.as_ref())
    {
        write_sidecar(tx, source, path, sidecar_revision, sidecar)?
    } else {
        sidecar_revision
    };
    measurements.record("warehouse_facts", started, batch.observations.len());
    let started = measurements.start();
    #[cfg(test)]
    let (fact_ns, maintenance_started) = fact_started
        .map(|start| (start.elapsed().as_nanos(), std::time::Instant::now()))
        .unzip();
    maintenance::maintain(
        tx,
        source,
        retired,
        old_offset,
        int(ledger.offset),
        path,
        batch.harness,
        header_kind.as_deref(),
    )?;
    tx.prepare_cached(
        "INSERT INTO maintenance_cursor(source,pos,sidecar_revision,eof_witness) VALUES(?1,?2,?3,?4)
         ON CONFLICT(source) DO UPDATE SET pos=excluded.pos,sidecar_revision=excluded.sidecar_revision,eof_witness=excluded.eof_witness
         WHERE maintenance_cursor.pos<>excluded.pos OR maintenance_cursor.sidecar_revision<>excluded.sidecar_revision
            OR maintenance_cursor.eof_witness IS NOT excluded.eof_witness",
    )?.execute(params![source,int(ledger.offset),sidecar_revision,eof_witness])?;
    measurements.record("warehouse_maintenance", started, 1);
    #[cfg(test)]
    if let Some((fact_ns, maintenance_started)) = fact_ns.zip(maintenance_started) {
        SCALE_TIMING.with(|timing| {
            let (facts, maintenance, admissions) = timing.get().unwrap();
            timing.set(Some((
                facts + fact_ns,
                maintenance + maintenance_started.elapsed().as_nanos(),
                admissions + 1,
            )));
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn session(
    tx: &Transaction<'_>,
    source: i64,
    kind: &str,
    pos: i64,
    occurrence: i64,
    ordinal: Option<i64>,
    native_id: &str,
    meta: Option<&serde_json::Value>,
) -> rusqlite::Result<()> {
    let metadata = meta.map(json).transpose()?;
    let attr = |key| meta?.get(key)?.as_str();
    let parent_id = if meta
        .and_then(|m| m.get("guardian_review"))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        None
    } else {
        attr("parent_thread")
    };
    fact!(tx, fact_session_observation, [source=>source,pos=>pos,block=>0,kind=>kind,
        occurrence=>occurrence,ordinal=>ordinal,native_id=>native_id,parent_id=>parent_id,
        nickname=>attr("nickname"),agent_path=>attr("path"),metadata=>metadata])
}

fn write_sidecar(
    tx: &Transaction<'_>,
    source: i64,
    path: &Path,
    revision: i64,
    sidecar: &Sidecar,
) -> rusqlite::Result<i64> {
    let revision = revision + 1;
    let metadata = match sidecar {
        Sidecar::Absent => None,
        Sidecar::Present(metadata) => Some(metadata),
    };
    let parent = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|s| s.to_str());
    // The Index provides the independently qualified sidecar witness. Its
    // description is not copied into the warehouse.
    fact!(tx, fact_agent_observation, [source=>source, revision=>revision,
        present=>metadata.is_some(),storage_parent=>parent,call_id=>metadata.and_then(|m| m.tool_use_id.as_deref()),
        model=>metadata.and_then(|m| m.model.as_deref()),cwd=>metadata.and_then(|m| m.cwd.as_deref()),
        branch=>metadata.and_then(|m| m.branch.as_deref())])?;
    Ok(revision)
}

fn write_fact(
    tx: &Transaction<'_>,
    source: i64,
    batch: &Batch,
    observation: &Observation,
    record_uuid: Option<&str>,
) -> rusqlite::Result<()> {
    let pos = int(observation.position.offset);
    let block = observation.position.block;
    let occurrence = observation.discriminator as i64;
    let ordinal = observation.ordinal.map(int);
    macro_rules! at {
        ($table:ident, $($name:ident => $value:expr),+ $(,)?) => {
            fact!(tx, $table, [source=>source,pos=>pos,block=>block,occurrence=>occurrence,
                ordinal=>ordinal,$($name=>$value),+])
        };
    }
    match &observation.fact {
        Fact::Record {
            kind,
            uuid,
            message_id,
            parent_id,
            time,
        } => at!(fact_record, kind=>kind,
            uuid=>uuid,message_id=>message_id,parent_id=>parent_id,ts=>time,gap=>false),
        Fact::Gap => {
            // A gap resets the turn at the preceding native time. Assign it
            // at ingest so an append cannot move an earlier gap in a cursor.
            let time: Option<i64> = tx.prepare_cached(
                "SELECT ts FROM fact_record WHERE source=?1 AND pos<?2 AND ts IS NOT NULL AND NOT gap
                 ORDER BY pos DESC LIMIT 1",
            )?.query_row(params![source, pos], |row| row.get(0)).optional()?;
            at!(fact_record, kind=>Option::<&str>::None, uuid=>Option::<&str>::None,
                message_id=>Option::<&str>::None,parent_id=>Option::<&str>::None,ts=>time,gap=>true)
        }
        Fact::SessionId(id) => at!(fact_session_observation, kind=>"session-id", native_id=>id,
            parent_id=>Option::<&str>::None,nickname=>Option::<&str>::None,
            agent_path=>Option::<&str>::None,metadata=>Option::<&str>::None),
        Fact::Title { present } => {
            at!(fact_label_observation, kind=>"title",value=>Option::<&str>::None,present=>present)
        }
        Fact::Model(model) => {
            at!(fact_label_observation, kind=>"model", value=>model, present=>true)
        }
        Fact::Relation { kind, target } => at!(fact_relation_observation,kind=>kind,target=>target),
        Fact::Parent {
            harness,
            id,
            call_id,
        } => at!(fact_parent_observation,method=>"marker",
            parent_harness=>harness,parent_id=>id,call_id=>call_id),
        Fact::LaunchCandidate { call_id, tool } => {
            at!(fact_claude_call,call_id=>call_id,tool=>tool)
        }
        Fact::ClaudeUsage {
            message_id,
            model,
            tokens,
            billing,
        } => {
            at!(fact_step_observation,kind=>"claude",message_id=>message_id,uuid=>record_uuid,
                model=>model,input=>int(tokens.input),cached=>int(tokens.cached_input),output=>int(tokens.output),
                ts=>billing.timestamp,accepted=>true)?;
            at!(fact_step_billing,kind=>"claude",billing=>json(billing)?)
        }
        Fact::CodexUsage {
            identity,
            model,
            tokens,
            model_tokens,
            time,
            accepted,
        } => {
            at!(fact_step_observation,kind=>"codex-response",message_id=>identity,uuid=>record_uuid,
                model=>model,input=>int(tokens.input),cached=>int(tokens.cached_input),output=>int(tokens.output),
                ts=>time,accepted=>accepted)?;
            at!(fact_step_billing,kind=>"codex-response",billing=>json(model_tokens)?)
        }
        Fact::CodexTokenSnapshot {
            model,
            tokens,
            time,
        } => at!(fact_step_observation,
            kind=>"codex-snapshot",message_id=>Option::<&str>::None,uuid=>record_uuid,
            model=>model,input=>int(tokens.input),cached=>int(tokens.cached_input),output=>int(tokens.output),
            ts=>time,accepted=>true),
        Fact::CopilotShutdown {
            event_id,
            time,
            usage,
        } => {
            at!(fact_copilot_shutdown,event_id=>event_id,ts=>time,complete=>usage.is_some())?;
            if let Some(usage) = usage {
                for (model, tokens) in &usage.by_model {
                    at!(fact_copilot_usage,model=>model,input=>int(tokens.input),output=>int(tokens.output),
                        cached=>int(tokens.cache_read),cache_write=>int(tokens.cache_write),
                        reasoning=>int(usage.reasoning_by_model[model]))?;
                }
            }
            Ok(())
        }
        Fact::CodexTurn {
            thread_id,
            turn_id,
            phase,
        } => {
            use crate::codex_lifecycle::TurnPhase;
            let phase = match phase {
                TurnPhase::Active => "active",
                TurnPhase::Completed => "completed",
                TurnPhase::Aborted => "aborted",
                TurnPhase::Errored => "errored",
            };
            at!(fact_codex_turn,thread_id=>thread_id,turn_id=>turn_id,phase=>phase)
        }
        Fact::Compact {
            time,
            trigger,
            pre_tokens,
        } => at!(fact_compaction,ts=>time,
            trigger=>trigger,pre_tokens=>pre_tokens.map(int)),
        Fact::ToolCall {
            id,
            tool,
            flags,
            time,
            target,
            task_name,
        } => {
            at!(fact_tool_call,call_id=>id,tool=>tool,task_name=>task_name,ts=>time,flags=>flags,target=>target)?;
            if batch.harness == "claude"
                && let (Some(id), Some(tool)) = (id, tool)
            {
                at!(fact_claude_call,call_id=>id,tool=>tool)?;
            }
            if batch.harness != "copilot"
                && matches!(
                    tool.as_deref(),
                    Some(
                        "SendMessage"
                            | "send_message"
                            | "followup_task"
                            | "collaboration.send_message"
                            | "collaboration.followup_task"
                    )
                )
            {
                at!(fact_relay_send,call_id=>id,target=>target.as_deref().unwrap_or(""),ts=>time)?;
            }
            Ok(())
        }
        Fact::Operation { parent, tool, ok } => {
            at!(fact_operation,call_pos=>int(parent.offset),call_block=>parent.block,tool=>tool,ok=>ok)
        }
        Fact::ToolResult {
            id,
            call,
            reply,
            denial,
            exit_code,
            target,
            kind,
        } => {
            let call_pos = call.as_ref().map(|call| int(call.offset));
            let call_block = call.as_ref().map(|call| call.block);
            at!(fact_tool_result,kind=>kind,call_id=>id,call_pos=>call_pos,call_block=>call_block,
                ts=>reply.t,failed=>reply.e,flags=>reply.f,correlation=>reply.m,denial=>denial,
                exit_code=>exit_code,target=>target)?;
            if batch.harness == "copilot" {
                return Ok(());
            }
            // Keep unresolved candidates. Read-time qualification will decide
            // whether the scoped result belongs to a relay send.
            at!(fact_relay_result,kind=>kind,call_id=>id,call_pos=>call_pos,call_block=>call_block,
                correlation=>reply.m,ts=>reply.t,failed=>reply.e || reply.f & (DENIED | SEND_FAILED) != 0,flags=>reply.f)
        }
        Fact::Event {
            kind,
            id,
            name,
            flags,
            time,
            peer,
            target,
        } => {
            let peer_pid = peer.as_ref().and_then(|p| p.pid).map(int);
            let peer_start = peer.as_ref().and_then(|p| p.start.as_deref());
            let peer_sock = peer.as_ref().and_then(|p| p.sock.as_deref());
            if *kind == Kind::Xsm {
                at!(fact_relay_receipt,correlation=>id,name=>crate::model::plain_name(name.as_deref().unwrap_or("")),
                    pid=>peer_pid,start=>peer_start,sock=>peer_sock,ts=>time)?;
            }
            let kind = match kind {
                Kind::Agm => "agm",
                Kind::Xsm => "xsm",
                Kind::Tool if batch.harness != "copilot" => "tool",
                Kind::Tn => "tn",
                Kind::Ask | Kind::U => {
                    return at!(fact_prompt,kind=>if *kind == Kind::Ask {"ask"} else {"user"},ts=>time);
                }
                _ => return Ok(()),
            };
            at!(fact_subagent_event,kind=>kind,ts=>time,id=>id,name=>name,flags=>flags,target=>target,
                peer_pid=>peer_pid,peer_start=>peer_start,peer_sock=>peer_sock)
        }
    }
}

pub(super) fn runtime_binding(
    tx: &Transaction<'_>,
    wanted: &crate::events::warehouse::RuntimeSource<'_>,
) -> rusqlite::Result<Option<crate::events::warehouse::RuntimeBinding>> {
    use crate::events::warehouse::RuntimeBinding;
    let namespace = super::origin_namespace(tx)?;
    let found = tx.prepare_cached(
        "SELECT s.source,s.ledger,(SELECT value FROM meta WHERE key='incarnation')
           FROM warehouse_current c JOIN warehouse_source s USING(source)
          WHERE c.path=?1 AND s.origin=?2 AND s.harness=?3
            AND EXISTS(SELECT 1 FROM fact_session_observation f
                        WHERE f.source=s.source AND f.kind IN ('physical','header') AND f.native_id=?4)",
    )?.query_row(params![wanted.path, namespace, wanted.harness, wanted.native_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    }).optional()?;
    let Some((source, ledger, index)) = found else {
        return Ok(None);
    };
    let ledger: Ledger =
        serde_json::from_str(&ledger).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let revision = (
        ledger.stat.dev,
        ledger.stat.ino,
        ledger.offset,
        ledger.prefix,
        ledger.tail,
        ledger.stat.changed_ns,
    );
    Ok((revision == wanted.revision).then_some(RuntimeBinding {
        index,
        namespace,
        source,
    }))
}

pub(super) fn write_runtime(
    tx: &Transaction<'_>,
    source: i64,
    capture: &crate::events::warehouse::RuntimeCapture,
) -> rusqlite::Result<bool> {
    let pid = capture.pid;
    let start = i64::try_from(capture.start).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let last: Option<(i64, i64)> = tx
        .prepare_cached(
            "SELECT capture,start FROM fact_process_observation
          WHERE source=?1 AND pid=?2 ORDER BY capture DESC LIMIT 1",
        )?
        .query_row(params![source, pid], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let clock = match last {
        Some((clock, previous_start)) => {
            if previous_start == start {
                let environment = tx
                    .prepare_cached(
                        "SELECT parent_harness,parent_id,call_id FROM fact_environment_observation
                      WHERE source=?1 AND capture=?2 AND pid=?3 AND start=?4",
                    )?
                    .query_row(params![source, clock, pid, start], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .optional()?;
                let mut query = tx.prepare_cached(
                    "SELECT ancestor_pid FROM fact_ancestor_observation
                      WHERE source=?1 AND capture=?2 AND pid=?3 AND start=?4 ORDER BY ordinal",
                )?;
                let ancestors = query
                    .query_map(params![source, clock, pid, start], |row| {
                        row.get::<_, u32>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if environment == capture.environment && ancestors == capture.ancestors {
                    return Ok(false);
                }
            }
            capture
                .clock
                .max(clock.checked_add(1).ok_or(rusqlite::Error::InvalidQuery)?)
        }
        None => capture.clock,
    };
    fact!(tx, fact_process_observation, [source=>source,capture=>clock,pid=>pid,start=>start])?;
    if let Some((harness, id, call)) = &capture.environment {
        fact!(tx, fact_environment_observation, [source=>source,capture=>clock,pid=>pid,start=>start,
            parent_harness=>harness,parent_id=>id,call_id=>call])?;
    }
    for (ordinal, ancestor) in capture.ancestors.iter().enumerate() {
        fact!(tx, fact_ancestor_observation, [source=>source,capture=>clock,pid=>pid,start=>start,
            ordinal=>ordinal as i64,ancestor_pid=>ancestor])?;
    }
    Ok(true)
}

pub(super) fn withdraw(tx: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let removed = tx
        .prepare_cached("DELETE FROM warehouse_current WHERE path=?1 RETURNING source")?
        .query_row([path], |row| row.get(0))
        .optional()?;
    if let Some(source) = removed {
        maintenance::withdraw(tx, source)?;
    }
    Ok(())
}

pub(super) fn relocate(
    tx: &Transaction<'_>,
    old: &str,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<()> {
    let moved: Option<i64> = tx
        .prepare_cached("UPDATE warehouse_current SET path=?2 WHERE path=?1 RETURNING source")?
        .query_row(params![old, path], |row| row.get(0))
        .optional()?;
    if let Some(source) = moved {
        put(
            tx,
            "UPDATE warehouse_source SET carrier=?2,ledger=?3 WHERE source=?1",
            params![source, path, json(ledger)?],
        )?;
    }
    Ok(())
}
