//! Assemble selected Trace evidence from one committed warehouse view.
use super::{CatalogReadScope, ReadError, Source, selected};
use crate::SessionSourceReader;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub(super) fn read(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    root: &str,
    retained_provider: Option<&dyn SessionSourceReader>,
    address: Option<&crate::session_route::NativeEntryAddress>,
) -> Result<Option<Value>, ReadError> {
    let view = selected::ReadView::open(source)?;
    let provider =
        retained_provider.map(|reader| crate::source_reader::BudgetedReader::new(reader, 1024));
    let provider = provider
        .as_ref()
        .map(|reader| reader as &dyn SessionSourceReader);
    let component =
        if let Some(address) = address.filter(|_| scope == CatalogReadScope::RetainedHistory) {
            selected::transcript_window(
                &view,
                source,
                scope,
                key,
                1,
                None,
                Some(address),
                provider,
                None,
            )?
        } else {
            selected::trace_turn(
                &view,
                source,
                scope,
                key,
                root,
                provider.unwrap_or(&view.archive),
            )?
        };
    let Some(component) = component else {
        return Ok(None);
    };
    let mut starts = component.records.iter().filter(|record| {
        record
            .causal
            .turn
            .as_ref()
            .is_some_and(|turn| turn.id == root && turn.address == record.address)
    });
    let Some(record) = starts.next() else {
        return Ok(None);
    };
    if starts.next().is_some() {
        return Ok(None);
    }
    let turn = record.causal.turn.as_ref().expect("selected turn boundary");
    let source_key = source.namespace.as_str();
    let rendered = selected::render::turn_address(&component, source_key, key, root);
    if address.is_some_and(|address| rendered.as_ref() != Some(&json!(address))) {
        return Err(ReadError::Restart);
    }
    let evidence = match rendered {
        Some(address) => json!({"catalog_key":key,"turn":root,"address":address}),
        None => {
            json!({"catalog_key":key,"turn":root,"reason":"The selected turn has no rendered transcript entry."})
        }
    };
    let (end, last) = trace_end(&view, source, &component, record, provider)?;
    let mut handoffs = Vec::new();
    let start = if crate::model::is_ask(root)
        && let Some(at) = record.at
    {
        handoffs.push(
            json!({"id":root,"kind":"ask","from":"you","to":key,"status":"done",
            "declined":false,"at":at,"target_turn":root,"brief_session":key}),
        );
        Some(root)
    } else {
        None
    };
    let mut sent = Vec::new();
    let mut limited = false;
    if let Some(window) = selected::sender_window(&view, source, &component, turn, provider)? {
        limited = window.records.len() == 33
            && window.records.iter().all(|record| {
                record
                    .causal
                    .turn
                    .as_ref()
                    .is_some_and(|candidate| candidate.id == root)
            });
        for action in
            super::sender_actions::recorded(&window, key).filter(|action| action.turn == root)
        {
            if sent.len() == 199 {
                limited = true;
                break;
            }
            sent.push(action.id.clone());
            handoffs.push(action.edge(key));
        }
    }
    let generation = &component.identity.generation;
    Ok(Some(json!({
        "api":1,"projection":7,"source_key":source_key,"catalog_key":key,"root":root,
        "generation":generation,"identity":component.identity,
        "sessions":{(key):session_summary(&component,record.at)},
        "participants":{},"participant_summaries":{},"handoffs":handoffs,
        "turns":[{"id":root,"sid":key,"start":start,"at":record.at,"u":!turn.handoff,"sent":sent,"end":end,"last":last}],
        "frontier":[],"evidence":[evidence],"previews":{(key):generation},
        "incomplete":limited || end["why"] == "unavailable","previews_incomplete":false,
    })))
}

fn session_summary(component: &selected::Component, at: Option<i64>) -> Value {
    let metadata = component.retained_metadata.as_ref();
    let name = metadata
        .and_then(|row| row["name"].as_str())
        .or(component.title.as_deref())
        .unwrap_or(&component.identity.catalog_key);
    let model = metadata
        .and_then(|row| row["model"].as_str())
        .or(component.model.as_deref())
        .unwrap_or("");
    let start = metadata
        .and_then(|row| row["start"].as_i64())
        .or(at)
        .unwrap_or_default();
    let last = metadata
        .and_then(|row| row["last"].as_i64())
        .unwrap_or(start);
    let tokens = metadata
        .and_then(|row| row.get("tokens"))
        .cloned()
        .unwrap_or_else(|| json!([0, 0, 0]));
    let cost = metadata
        .and_then(|row| row.get("cost"))
        .cloned()
        .unwrap_or_else(|| json!({"usd":null,"unpriced_models":[]}));
    json!({"name":name,"harness":component.harness,"model":model,"state":"off","live_observation":false,
        "start":start,"last":last,"tokens":tokens,"cost":cost,"busy":[],"wait_edges":[],"wait_edges_truncated":false})
}

fn trace_end(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    root: &selected::Record,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<(Value, bool), ReadError> {
    let (mut end, last) = recorded_end(view, source, component, root)?;
    if end["why"] != "unavailable" || !matches!(component.harness.as_str(), "claude" | "copilot") {
        return Ok((end, last));
    }
    let copilot = component.harness == "copilot";
    let native_turn = root.native.pointer("/data/turnId").and_then(Value::as_str);
    if copilot && native_turn.is_none() {
        end["reason"] = json!("The selected prompt does not identify a native Copilot turn.");
        return Ok((end, last));
    }
    let limit = if copilot { 32 } else { 8 };
    let addresses = {
        let tx = view.lock()?;
        let retained = (component.identity.read_scope == CatalogReadScope::RetainedHistory)
            .then(|| {
                let identity = crate::events::store::warehouse::retained_identity(
                    &tx,
                    &source.namespace,
                    &component.identity.catalog_key,
                )
                .map_err(super::retained_error)?;
                if identity.witness != component.identity.generation {
                    return Err(ReadError::Restart);
                }
                Ok(identity.binding)
            })
            .transpose()?;
        let harness = crate::InputRoot::parse(&component.harness).ok_or(ReadError::Restart)?;
        let order = if copilot { "ASC" } else { "DESC" };
        let sql = format!(
            "WITH member(source) AS (
               SELECT m.source FROM warehouse_identity_member m
               WHERE m.identity=?1 AND json_extract(m.receipt,'$.path')=?2
                 AND json_extract(m.receipt,'$.offset')=?5
               UNION ALL
               SELECT w.source FROM warehouse_current w
               CROSS JOIN warehouse_source s ON s.source=w.source AND s.origin=?3
               WHERE ?1 IS NULL AND w.path=?2
             )
             SELECT r.pos,r.occurrence,r.ts FROM member m
             CROSS JOIN fact_record r ON r.source=m.source
             WHERE r.pos>?4 AND r.pos<min(coalesce((
               SELECT min(p.pos) FROM fact_prompt p WHERE p.source=m.source AND p.pos>?4 AND p.pos<?5
             ),?5),coalesce((
               SELECT min(g.pos) FROM fact_record g WHERE g.source=m.source AND g.pos>?4 AND g.pos<?5 AND g.gap
             ),?5)) AND NOT r.gap AND r.block=0 AND (
               (NOT ?6 AND r.kind='assistant') OR (?6 AND r.kind IN
                 ('assistant.turn_start','assistant.message','abort','assistant.turn_end'))
             ) AND coalesce(r.ts,-9223372036854775808)>=?7
             ORDER BY r.ts {order},r.pos {order},r.occurrence {order} LIMIT ?8"
        );
        let mut lookup = tx.prepare_cached(&sql).map_err(super::index)?;
        let mut addresses = Vec::new();
        for member in &component.identity.source_refs {
            let reference = &member.source;
            let same = reference.path == root.address.path;
            if !same && (copilot || root.at.is_none()) {
                continue;
            }
            let carrier = harness.home(source.options).join(&reference.path);
            let mut rows = lookup
                .query(params![
                    retained,
                    carrier.to_str(),
                    source.namespace.binding(),
                    if same {
                        i64::try_from(root.address.offset).map_err(super::index)?
                    } else {
                        -1
                    },
                    i64::try_from(reference.offset).map_err(super::index)?,
                    copilot,
                    if same {
                        i64::MIN
                    } else {
                        root.at.unwrap_or(i64::MIN)
                    },
                    i64::try_from(limit + 1).map_err(super::index)?,
                ])
                .map_err(super::index)?;
            while let Some(row) = rows.next().map_err(super::index)? {
                addresses.push((
                    row.get::<_, Option<i64>>(2).map_err(super::index)?,
                    selected::Address {
                        path: reference.path.clone(),
                        offset: u64::try_from(row.get::<_, i64>(0).map_err(super::index)?)
                            .map_err(super::index)?,
                        block: 0,
                        occurrence: row.get(1).map_err(super::index)?,
                    },
                ));
            }
            if !copilot {
                addresses.sort_by(|(a_at, a), (b_at, b)| {
                    (b_at, &b.path, b.offset, b.occurrence).cmp(&(
                        a_at,
                        &a.path,
                        a.offset,
                        a.occurrence,
                    ))
                });
            }
            addresses.truncate(limit + 1);
        }
        addresses
    };
    let key = &component.identity.catalog_key;
    let mut started = false;
    let mut interrupted = None;
    let mut reply = None;
    let mut reason = if addresses.len() > limit {
        "The native turn end was outside the Trace candidate limit."
    } else {
        "No terminal record matched this turn in the selected prefix."
    };
    for (_, address) in addresses.into_iter().take(limit) {
        let window = match component.identity.read_scope {
            CatalogReadScope::RetainedHistory => selected::retained_trace_window(
                view,
                source,
                key,
                &component.identity.generation,
                &address,
                provider,
            )
            .map(Some),
            CatalogReadScope::Current => selected::trace_record(view, source, key, &address),
        };
        let window = match window {
            Ok(Some(window)) => window,
            Ok(None)
            | Err(
                ReadError::RetainedPreparing
                | ReadError::RetainedUnavailable
                | ReadError::PeerUnavailable(_),
            ) => {
                reason =
                    "A candidate terminal record could not be verified in the selected prefix.";
                if copilot {
                    break;
                }
                continue;
            }
            Err(ReadError::SourceBudget) => {
                reason =
                    "The native turn end could not be verified within the Trace source budget.";
                break;
            }
            Err(error) => return Err(error),
        };
        let Some(record) = window
            .records
            .iter()
            .find(|record| record.address == address && record.causal.turn == root.causal.turn)
        else {
            continue;
        };
        if !copilot {
            if selected::render::has_assistant_text(record) {
                end = json!({"st":"done","why":"replied","at":record.at});
                break;
            }
            continue;
        }
        let candidate = record
            .native
            .pointer("/data/turnId")
            .and_then(Value::as_str);
        let matches_turn = candidate == native_turn;
        match record.kind.as_deref() {
            Some("assistant.turn_start") if started => break,
            Some("assistant.turn_start") => {
                let interaction = root.native.pointer("/data/interactionId");
                started = matches_turn
                    && interaction
                        .is_none_or(|id| record.native.pointer("/data/interactionId") == Some(id));
            }
            Some("abort") if started && (candidate.is_none() || matches_turn) => {
                interrupted = Some(record.at);
            }
            Some("assistant.message") if started && matches_turn => {
                if selected::render::has_assistant_text(record) {
                    reply = Some(record.at);
                }
            }
            Some("assistant.turn_end") if started && matches_turn => {
                end = if let Some(at) = interrupted {
                    json!({"st":"off","why":"interrupted","at":at})
                } else if let Some(at) = reply {
                    json!({"st":"done","why":"replied","at":at})
                } else {
                    json!({"st":"done","why":"completed","at":record.at})
                };
                break;
            }
            _ => (),
        }
    }
    if end["why"] == "unavailable" {
        end["reason"] = json!(reason);
    }
    Ok((end, last))
}

pub(super) fn recorded_end(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    root: &selected::Record,
) -> Result<(Value, bool), ReadError> {
    let unavailable = || json!({"st":"off","why":"unavailable"});
    let last = root
        .causal
        .turn
        .as_ref()
        .is_some_and(|turn| component.latest_turn.as_ref() == Some(turn));
    if component.harness != "codex"
        || root.causal.turn.as_ref().is_none_or(|turn| turn.handoff)
        || root.prompt_anchor.is_none()
    {
        return Ok((unavailable(), last));
    }
    // A Codex terminal fact names the same native turn as a reducer-qualified
    // start. Stop at the next human turn so a later task cannot close this one.
    let carrier = source.options.codex_home.join(&root.address.path);
    let tx = view.lock()?;
    let (start, limit) = (
        i64::try_from(root.address.offset).map_err(super::index)?,
        i64::try_from(root.reference.offset).map_err(super::index)?,
    );
    let source_id = root.source;
    let next: Option<i64> = tx.query_row(
        "SELECT min(pos) FROM fact_prompt WHERE source=?1 AND pos>?2 AND pos<?3 AND kind IN ('user','ask')",
        params![source_id, start, limit], |row| row.get(0),
    ).map_err(super::index)?;
    let owned_turn = tx
        .query_row(
            "WITH boundary AS (
           SELECT coalesce(max(pos),-1) AS at FROM fact_prompt
           WHERE source=?1 AND pos<?2 AND kind IN ('user','ask')
         ), active AS (
           SELECT t.turn_id,t.pos FROM boundary b
           CROSS JOIN fact_codex_turn t ON t.source=?1 AND t.pos>b.at AND t.pos<?3
             AND t.thread_id=?4 AND t.phase='active'
           WHERE NOT EXISTS (SELECT 1 FROM fact_codex_turn earlier
              WHERE earlier.source=t.source AND earlier.turn_id=t.turn_id
                AND earlier.pos>t.pos AND earlier.pos<?2 AND earlier.phase<>'active')
           ORDER BY (t.pos>?2), CASE WHEN t.pos<=?2 THEN -t.pos ELSE t.pos END LIMIT 1
         ), stop AS (
           SELECT coalesce(min(t.pos),?3) AS pos FROM active a
           CROSS JOIN fact_codex_turn t ON t.source=?1 AND t.pos>a.pos AND t.pos<?3
             AND t.thread_id=?4 AND t.phase='active'
         )
         SELECT a.turn_id,t.phase,r.ts FROM active a CROSS JOIN stop
         LEFT JOIN fact_codex_turn t ON t.source=?1 AND t.turn_id=a.turn_id AND t.thread_id=?4
           AND t.pos>a.pos AND t.pos>=?2 AND t.pos<stop.pos AND t.phase<>'active'
         LEFT JOIN fact_record r ON r.source=t.source AND r.pos=t.pos AND r.block=0 AND NOT r.gap
         WHERE stop.pos>?2
         ORDER BY t.pos LIMIT 1",
            params![
                source_id,
                start,
                next.unwrap_or(limit),
                root.reference.native_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(super::index)?;
    let end = match owned_turn {
        Some((_, Some(phase), at)) => {
            let (state, reason) = match phase.as_str() {
                "completed" => ("done", "completed"),
                "aborted" => ("off", "interrupted"),
                "errored" => ("err", "failed"),
                _ => return Ok((unavailable(), last)),
            };
            json!({"st":state,"why":reason,"at":at})
        }
        Some((native_turn, None, _))
            if last && component.identity.read_scope == CatalogReadScope::Current =>
        {
            let ledger = tx
                .query_row(
                    "SELECT ledger FROM warehouse_source WHERE source=?1",
                    [source_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(super::index)?;
            let ledger = serde_json::from_str(&ledger).map_err(super::index)?;
            let lifecycle = crate::events::store::committed_codex_lifecycle(
                &tx,
                carrier.to_str().ok_or(ReadError::Restart)?,
                &ledger,
                &root.reference.native_id,
            )
            .map_err(super::index)?;
            if lifecycle.is_some_and(|turn| {
                turn.turn_id == native_turn && turn.phase == crate::TurnPhase::Active
            }) && super::selected_runtime_status(&tx, source, source_id, view)? == Some("work")
            {
                json!({"st":"work","why":"working"})
            } else {
                unavailable()
            }
        }
        _ => unavailable(),
    };
    Ok((end, last))
}
