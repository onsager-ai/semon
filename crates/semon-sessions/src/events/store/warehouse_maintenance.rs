//! Maintain only the committed suffix and the graph components it touches.

use std::{collections::BTreeSet, path::Path};

use rusqlite::{OptionalExtension, Transaction, params};

use super::put;

#[derive(Debug, PartialEq, Eq)]
struct Session {
    id: String,
    parent: Option<String>,
    nickname: Option<String>,
    first: Option<i64>,
    continued: bool,
    lineage: bool,
}

fn dimension(tx: &Transaction<'_>, source: i64) -> rusqlite::Result<Option<Session>> {
    tx.prepare_cached(
        "SELECT native_id,parent_id,nickname,first_ts,continued,lineage FROM dim_session WHERE source=?1",
    )?.query_row([source], |r| Ok(Session {
        id:r.get(0)?, parent:r.get(1)?, nickname:r.get(2)?,
        first:r.get(3)?, continued:r.get(4)?, lineage:r.get(5)?,
    })).optional()
}

type SpawnKeys = BTreeSet<(String, String)>;

pub(super) fn withdraw(tx: &Transaction<'_>, source: i64) -> rusqlite::Result<()> {
    let mut spawn = SpawnKeys::new();
    retire(tx, source, &mut spawn)?;
    refresh_spawns(tx, spawn)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn maintain(
    tx: &Transaction<'_>,
    source: i64,
    retired: Option<i64>,
    before: i64,
    after: i64,
    path: &Path,
    harness: &str,
    header_kind: Option<&str>,
) -> rusqlite::Result<()> {
    let mut spawn = SpawnKeys::new();
    if let Some(retired) = retired {
        retire(tx, retired, &mut spawn)?;
    }
    let old = dimension(tx, source)?;
    let lineage = harness == "claude" && !crate::native_sidecar::agent_path(path);
    let (identity, identity_pos) = match harness {
        "claude" => (Some("physical"), -1),
        "copilot" => (Some("session-id"), 0),
        _ => (header_kind, -1),
    };
    if harness == "codex" && before == 0 && identity.is_none() {
        // A provisional header may disappear as its first line is finished.
        retire(tx, source, &mut spawn)?;
        return refresh_spawns(tx, spawn);
    }
    if let Some(kind) = identity
        && (old.is_none() || harness == "codex")
    {
        put(tx,
            "INSERT INTO dim_session(source,harness,native_id,parent_id,nickname,agent_path,continued,lineage)
             SELECT source,?2,native_id,parent_id,nickname,agent_path,0,?4
             FROM fact_session_observation WHERE source=?1 AND pos=?5 AND block=0 AND kind=?3
             ON CONFLICT(source) DO UPDATE SET native_id=excluded.native_id,parent_id=excluded.parent_id,
               nickname=excluded.nickname,agent_path=excluded.agent_path
             WHERE dim_session.native_id IS NOT excluded.native_id OR dim_session.parent_id IS NOT excluded.parent_id
                OR dim_session.nickname IS NOT excluded.nickname OR dim_session.agent_path IS NOT excluded.agent_path",
            params![source,harness,kind,lineage,identity_pos],
        )?;
    }
    if old.is_none() && dimension(tx, source)?.is_none() {
        return refresh_spawns(tx, spawn);
    }
    // A formerly unidentified Codex source may receive an identity at the
    // first complete header. Its dimension starts at the beginning.
    let before = if old.is_none() { 0 } else { before };
    if before < after {
        put(tx,
            "WITH records AS (
                SELECT min(ts) first_ts,max(ts) last_ts
                FROM fact_record WHERE source=?1 AND pos>=?2
             ), title AS (
                SELECT pos,present FROM fact_label_observation WHERE source=?1 AND pos>=?2 AND kind='title'
                ORDER BY pos DESC,block DESC,occurrence DESC LIMIT 1
             ), model AS (
                SELECT pos,value FROM fact_label_observation WHERE source=?1 AND pos>=?2 AND kind='model'
                ORDER BY pos DESC,block DESC,occurrence DESC LIMIT 1
             )
             UPDATE dim_session SET
                first_ts=(SELECT coalesce(min(dim_session.first_ts,first_ts),dim_session.first_ts,first_ts) FROM records),
                last_ts=(SELECT coalesce(max(dim_session.last_ts,last_ts),dim_session.last_ts,last_ts) FROM records),
                title_pos=CASE WHEN EXISTS(SELECT 1 FROM title) THEN (SELECT pos FROM title WHERE present) ELSE title_pos END,
                model=coalesce((SELECT value FROM model),model),
                model_pos=coalesce((SELECT pos FROM model),model_pos),
                prompt_pos=coalesce(prompt_pos,(SELECT min(pos) FROM fact_prompt WHERE source=?1 AND pos>=?2)),
                continued=continued OR EXISTS(SELECT 1 FROM fact_relation_observation
                                                WHERE source=?1 AND pos>=?2 AND kind='continued-in')
             WHERE source=?1",
             params![source,before],
        )?;
    }
    let session = dimension(tx, source)?.ok_or(rusqlite::Error::InvalidQuery)?;
    if old.is_none() {
        admit_member(tx, source, &session)?;
    } else if old
        .as_ref()
        .is_some_and(|old| (old.first, old.continued) != (session.first, session.continued))
    {
        // Suffix maintenance only improves a source's rank (a smaller first
        // timestamp or a continuation). Compare only it and the incumbent.
        let component = component(tx, source)?;
        let group = group(tx, component)?;
        let owner = preferred(tx, group.owner, source)?;
        if owner != group.owner {
            put(
                tx,
                "UPDATE lineage_group SET owner=?2 WHERE id=?1",
                params![component, owner],
            )?;
        }
    }
    if lineage && before < after {
        add_edges(tx, source, before)?;
    }
    if harness == "codex" {
        if old.as_ref() != Some(&session)
            && old.as_ref().is_none_or(|old| {
                (&old.id, &old.parent, &old.nickname)
                    != (&session.id, &session.parent, &session.nickname)
            })
        {
            if let Some(old) = &old {
                spawn_session_keys(tx, source, old, &mut spawn)?;
            }
            spawn_session_keys(tx, source, &session, &mut spawn)?;
        }
        if before < after {
            let mut names = tx.prepare_cached(
                "SELECT DISTINCT task_name FROM fact_tool_call WHERE source=?1 AND pos>=?2
                 AND tool IN ('spawn_agent','collaboration.spawn_agent') AND task_name IS NOT NULL",
            )?;
            for row in names.query_map(params![source, before], |r| r.get::<_, String>(0))? {
                spawn.insert((session.id.clone(), row?));
            }
        }
    }
    refresh_spawns(tx, spawn)
}

fn spawn_session_keys(
    tx: &Transaction<'_>,
    source: i64,
    session: &Session,
    keys: &mut SpawnKeys,
) -> rusqlite::Result<()> {
    if let (Some(parent), Some(name)) = (&session.parent, &session.nickname) {
        keys.insert((parent.clone(), name.clone()));
    }
    let mut children = tx.prepare_cached(
        "SELECT DISTINCT nickname FROM dim_session WHERE harness='codex' AND parent_id=?1 AND nickname IS NOT NULL",
    )?;
    for row in children.query_map([&session.id], |r| r.get::<_, String>(0))? {
        keys.insert((session.id.clone(), row?));
    }
    // A retiring or renamed child no longer participates in name lookups.
    // Remove its own placement while its old identity is available.
    put(
        tx,
        "DELETE FROM bridge_spawn_placement WHERE child=?1 OR parent_source=?2",
        params![session.id, source],
    )
}

fn refresh_spawns(tx: &Transaction<'_>, keys: SpawnKeys) -> rusqlite::Result<()> {
    for (id, name) in keys {
        put(tx,
            "DELETE FROM bridge_spawn_placement WHERE child IN (
                SELECT native_id FROM dim_session WHERE harness='codex' AND parent_id=?1 AND nickname=?2
             )", params![id,name],
        )?;
        put(tx,
            "WITH parent AS (
                 SELECT min(source) source FROM dim_session WHERE harness='codex' AND native_id=?1
                 HAVING count(*)=1
             ), child AS (
                 SELECT min(native_id) id FROM dim_session WHERE harness='codex' AND parent_id=?1 AND nickname=?2
                 HAVING count(*)=1
             ), call AS (
                 SELECT min(c.source) source,min(c.pos) pos,min(c.ts) ts FROM parent p
                 CROSS JOIN fact_tool_call c ON c.source=p.source
                   AND c.tool IN ('spawn_agent','collaboration.spawn_agent') AND c.task_name=?2
                 HAVING count(*)=1
             )
             INSERT INTO bridge_spawn_placement(child,parent_source,pos,ts)
             SELECT child.id,call.source,call.pos,call.ts FROM child,call WHERE true
             ON CONFLICT(child) DO UPDATE SET parent_source=excluded.parent_source,pos=excluded.pos,ts=excluded.ts",
            params![id,name],
        )?;
    }
    Ok(())
}

fn component(tx: &Transaction<'_>, source: i64) -> rusqlite::Result<i64> {
    tx.prepare_cached("SELECT component FROM lineage_member WHERE source=?1")?
        .query_row([source], |r| r.get(0))
}

fn node(tx: &Transaction<'_>, name: &str) -> rusqlite::Result<Option<i64>> {
    tx.prepare_cached("SELECT component FROM lineage_node WHERE node=?1")?
        .query_row([name], |r| r.get(0))
        .optional()
}

fn admit_member(tx: &Transaction<'_>, source: i64, session: &Session) -> rusqlite::Result<()> {
    put(
        tx,
        "INSERT INTO lineage_group(id,weight,minimum,owner) VALUES(?1,1,?1,?1)",
        [source],
    )?;
    put(
        tx,
        "INSERT INTO lineage_member(source,component) VALUES(?1,?1)",
        [source],
    )?;
    if session.lineage {
        connect(tx, source, &session.id)?;
    }
    Ok(())
}

fn add_edges(tx: &Transaction<'_>, source: i64, before: i64) -> rusqlite::Result<()> {
    let mut edges = tx.prepare_cached(
        "SELECT target FROM fact_relation_observation WHERE source=?1 AND pos>=?2
         AND kind IN ('session_id','continued-in','bridge')",
    )?;
    for target in edges.query_map(params![source, before], |r| r.get::<_, String>(0))? {
        let current = component(tx, source)?;
        connect(tx, current, &target?)?;
    }
    Ok(())
}

struct Group {
    weight: i64,
    minimum: i64,
    owner: i64,
}

fn group(tx: &Transaction<'_>, id: i64) -> rusqlite::Result<Group> {
    tx.prepare_cached("SELECT weight,minimum,owner FROM lineage_group WHERE id=?1")?
        .query_row([id], |r| {
            Ok(Group {
                weight: r.get(0)?,
                minimum: r.get(1)?,
                owner: r.get(2)?,
            })
        })
}

fn preferred(tx: &Transaction<'_>, a: i64, b: i64) -> rusqlite::Result<i64> {
    if a == b {
        return Ok(a);
    }
    tx.prepare_cached(
        "SELECT source FROM dim_session WHERE source IN (?1,?2)
         ORDER BY first_ts IS NULL,first_ts,continued DESC,native_id,source LIMIT 1",
    )?
    .query_row(params![a, b], |r| r.get(0))
}

fn connect(tx: &Transaction<'_>, current: i64, name: &str) -> rusqlite::Result<()> {
    let Some(other) = node(tx, name)? else {
        put(
            tx,
            "INSERT INTO lineage_node(node,component) VALUES(?1,?2)",
            params![name, current],
        )?;
        return put(
            tx,
            "UPDATE lineage_group SET weight=weight+1 WHERE id=?1",
            [current],
        );
    };
    if current == other {
        return Ok(());
    }
    let a = group(tx, current)?;
    let b = group(tx, other)?;
    let (large, small) = if a.weight >= b.weight {
        (current, other)
    } else {
        (other, current)
    };
    let owner = preferred(tx, a.owner, b.owner)?;
    put(
        tx,
        "UPDATE lineage_member SET component=?1 WHERE component=?2",
        params![large, small],
    )?;
    put(
        tx,
        "UPDATE lineage_node SET component=?1 WHERE component=?2",
        params![large, small],
    )?;
    put(tx, "DELETE FROM lineage_group WHERE id=?1", [small])?;
    put(
        tx,
        "UPDATE lineage_group SET weight=?2,minimum=?3,owner=?4 WHERE id=?1",
        params![large, a.weight + b.weight, a.minimum.min(b.minimum), owner],
    )
}

fn retire(tx: &Transaction<'_>, source: i64, spawn: &mut SpawnKeys) -> rusqlite::Result<()> {
    let Some(session) = dimension(tx, source)? else {
        return Ok(());
    };
    super::identity::retire(tx, source)?;
    let old = component(tx, source)?;
    if !session.lineage {
        // Agent and Codex rows are singleton lineages. Codex parent and spawn
        // evidence can still be revised independently of the lineage graph.
        let harness: String = tx
            .prepare_cached("SELECT harness FROM dim_session WHERE source=?1")?
            .query_row([source], |r| r.get(0))?;
        if harness == "codex" {
            spawn_session_keys(tx, source, &session, spawn)?;
        }
    }
    let members: Vec<i64> = tx
        .prepare_cached(
            "SELECT source FROM lineage_member WHERE component=?1 AND source<>?2 ORDER BY source",
        )?
        .query_map(params![old, source], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    put(tx, "DELETE FROM lineage_member WHERE component=?1", [old])?;
    put(tx, "DELETE FROM lineage_node WHERE component=?1", [old])?;
    put(tx, "DELETE FROM lineage_group WHERE id=?1", [old])?;
    put(tx, "DELETE FROM dim_session WHERE source=?1", [source])?;
    // All endpoints touched by these old active edges already belonged to the
    // old component. New evidence is applied after this replay can split it.
    for &member in &members {
        let session = dimension(tx, member)?.ok_or(rusqlite::Error::InvalidQuery)?;
        admit_member(tx, member, &session)?;
    }
    for member in members {
        add_edges(tx, member, 0)?;
    }
    Ok(())
}
