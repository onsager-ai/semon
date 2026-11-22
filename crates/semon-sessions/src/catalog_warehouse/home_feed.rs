//! Metadata for the Home feed. Preview text is hydrated by selected reads.
use super::{ReadError, Snapshot, Source, index, selected};
use crate::overview::{OverviewSource, Saved, Summary};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

const RECENT: &str = concat!(
    include_str!("../events/store/warehouse_list.sql"),
    include_str!("../events/store/warehouse_catalog_row.sql"),
    "\n) SELECT id,owner,harness,last,title,model,repo,branch,NULL,
         prompt_pos,owner_path,owner_ledger,native_id,agent_path,is_child,start,lineage,guardian,effort
       FROM rows WHERE last>=?5 OR id IN (SELECT id FROM rows ORDER BY last DESC,id LIMIT 60)
       ORDER BY last DESC,id",
);

pub(crate) fn read(
    configured: &OverviewSource,
    since: i64,
) -> std::io::Result<(Vec<Saved>, bool, String)> {
    read_source(configured, since).map_err(|err| std::io::Error::other(err.to_string()))
}

fn read_source(
    configured: &OverviewSource,
    since: i64,
) -> Result<(Vec<Saved>, bool, String), ReadError> {
    let path = crate::events::EventCache::path(&configured.options.cache);
    let namespace = crate::storage::Namespace::new(&configured.key).map_err(index)?;
    let source = Source::discover(namespace, &path, &configured.options)?;
    let view = selected::ReadView::open(&source)?;
    let (mut rows, complete, revision) = {
        let tx = view.lock()?;
        let snapshot = Snapshot::read(&tx, &source)?;
        let mut statement = tx.prepare_cached(RECENT).map_err(index)?;
        let mut hits = statement
            .query(params![
                snapshot.paths,
                source.namespace.binding(),
                super::encode(&snapshot.repos)?,
                None::<&str>,
                since
            ])
            .map_err(index)?;
        let mut rows = Vec::new();
        while let Some(hit) = hits.next().map_err(index)? {
            rows.push(super::read_row(
                &source,
                hit,
                hit.get(0).map_err(index)?,
                true,
            )?);
        }
        snapshot.enrich(&mut rows, false)?;
        (
            rows,
            snapshot.coverage.pending == 0 && snapshot.coverage.unavailable == 0,
            snapshot.revision,
        )
    };
    let mut saved = Vec::with_capacity(rows.len());
    for row in &mut rows {
        row.resolve_title(&view, &source)?;
        let key = row.position.key.as_str();
        let state = row.status.filter(|_| configured.up).unwrap_or("off");
        let tx = view.lock()?;
        let owner_id: String = tx
            .query_row(
                "SELECT native_id FROM dim_session WHERE source=?1",
                [row.owner],
                |hit| hit.get(0),
            )
            .map_err(index)?;
        let identity = (row.owner, row.harness.clone(), owner_id);
        let generation =
            super::current_identity_witness(&tx, &source, key, &identity, &row.source_refs)?;
        let events = home_events(&tx, row, since, state)?;
        saved.push(Saved {
            id: json!([configured.key, key]).to_string(),
            key: key.to_owned(), source: configured.key.clone(), content_generation: generation,
            summary: Summary::home_feed(json!({
                "name":row.title,"kind":row.presentation_kind,"harness":row.harness,"model":row.model,
                "repo":row.repo,"branch":row.branch,"parent":row.parent,"state":state,
                "last":row.position.last,"start":row.start,"cost":row.cost.as_ref().and_then(|cost| cost.usd),
                "events":events,
            })),
        });
    }
    source.validate_authority()?;
    Ok((saved, complete, revision))
}

fn home_events(
    db: &Connection,
    row: &super::Row,
    since: i64,
    state: &str,
) -> Result<Vec<Value>, ReadError> {
    let mut statement = db.prepare_cached(
        "SELECT d.native_id,c.pos,c.call_id,c.ts,t.ts,t.failed,t.flags
         FROM lineage_member root JOIN lineage_member m USING(component)
         JOIN dim_session d ON d.source=m.source
         JOIN fact_tool_call c ON c.source=m.source AND c.tool IN
           ('AskUserQuestion','request_user_input','request_user_input_async',
            'functions.request_user_input','functions.request_user_input_async')
         LEFT JOIN fact_tool_result t ON (t.source,t.pos,t.block,t.kind,t.occurrence)=(
           SELECT r.source,r.pos,r.block,r.kind,r.occurrence FROM fact_tool_result r
           WHERE r.source=c.source AND r.call_id=c.call_id AND r.pos>c.pos
             AND ((r.call_pos=c.pos AND r.call_block=c.block) OR (r.call_pos IS NULL AND 1=(
               SELECT count(*) FROM fact_tool_call a WHERE a.source=c.source AND a.call_id=c.call_id)))
             AND ((r.flags & ?3)=0 OR c.tool IN ('request_user_input_async','functions.request_user_input_async'))
           ORDER BY r.pos DESC,r.block DESC,r.occurrence DESC LIMIT 1)
         WHERE root.source=?1 AND (c.ts>=?2 OR t.ts>=?2 OR (?4 AND t.pos IS NULL))
         ORDER BY c.ts DESC,c.source DESC,c.pos DESC,c.block DESC",
    ).map_err(index)?;
    let mut calls = statement
        .query(params![
            row.owner,
            since,
            crate::events::ACK,
            state == "wait"
        ])
        .map_err(index)?;
    let mut events = Vec::new();
    while let Some(call) = calls.next().map_err(index)? {
        let native_id: String = call.get(0).map_err(index)?;
        let offset = u64::try_from(call.get::<_, i64>(1).map_err(index)?).map_err(index)?;
        let call_id: Option<String> = call.get(2).map_err(index)?;
        let at: Option<i64> = call.get(3).map_err(index)?;
        let done: Option<i64> = call.get(4).map_err(index)?;
        let failed: Option<bool> = call.get(5).map_err(index)?;
        let flags: Option<i64> = call.get(6).map_err(index)?;
        let declined = failed == Some(true)
            || flags.is_some_and(|flags| flags & i64::from(crate::events::DENIED) != 0);
        events.push(
            json!({"id":crate::model::question_id(&native_id,offset,call_id.as_deref()),
            "at":at.unwrap_or(row.position.last),"done":done,"ask":"question",
            "status":if failed.is_none() && state == "wait" {"wait"} else {"done"},
            "brief":"","answer":declined.then(Vec::<String>::new),"declined":declined,"turn":null}),
        );
    }
    if row.parent.is_none()
        && ((row.harness == "claude" && state == "idle")
            || (row.harness == "codex" && state != "work"))
    {
        let result = db
            .query_row(
                "SELECT d.native_id,a.pos,a.block,a.ts
             FROM lineage_member root JOIN lineage_member m USING(component)
             JOIN dim_session d ON d.source=m.source
             JOIN fact_subagent_event a ON a.source=m.source AND a.kind='assistant'
             WHERE root.source=?1 AND a.ts>=?2 AND EXISTS (
               SELECT 1 FROM fact_prompt p WHERE p.source=a.source AND p.pos=(
                 SELECT max(z.pos) FROM fact_prompt z WHERE z.source=a.source)
                 AND p.pos<a.pos AND (p.kind='ask' OR d.harness='codex')
               AND NOT EXISTS (SELECT 1 FROM fact_subagent_event e WHERE e.source=a.source
                 AND e.pos>p.pos AND e.pos<=a.pos AND e.kind IN ('agm','xsm')))
             ORDER BY a.ts DESC,a.source DESC,a.pos DESC,a.block DESC LIMIT 1",
                params![row.owner, since],
                |hit| {
                    Ok((
                        hit.get::<_, String>(0)?,
                        hit.get::<_, i64>(1)?,
                        hit.get::<_, u32>(2)?,
                        hit.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(index)?;
        if let Some((native_id, offset, block, at)) = result {
            let offset = u64::try_from(offset).map_err(index)?;
            events.push(
                json!({"id":crate::model::result_id(&native_id,offset,block),"at":at,
                "done":null,"ask":"result","status":"new","brief":"","answer":null,
                "declined":false,"turn":null}),
            );
        }
    }
    Ok(events)
}
