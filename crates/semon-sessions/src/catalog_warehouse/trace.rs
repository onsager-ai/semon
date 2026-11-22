//! Assemble selected Trace evidence from one committed warehouse view.
use super::{CatalogReadScope, ReadError, Source, selected};
use crate::SessionSourceReader;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) fn read(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    root: &str,
    retained_provider: Option<&dyn SessionSourceReader>,
    address: Option<&crate::session_route::NativeEntryAddress>,
    origin: Option<selected::PeerBinding<'_>>,
) -> Result<Option<Value>, ReadError> {
    let view = selected::ReadView::open(source)?;
    let provider =
        retained_provider.map(|reader| crate::source_reader::BudgetedReader::new(reader, 1024));
    let provider = provider
        .as_ref()
        .map(|reader| reader as &dyn SessionSourceReader);
    let component = if let Some(address) = address {
        selected::transcript_window(
            &view,
            source,
            scope,
            key,
            1,
            None,
            Some(address),
            provider,
            origin,
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
    let turn = record
        .causal
        .turn
        .as_ref()
        .expect("qualified turn boundary");
    let source_key = source.namespace.as_str();
    let root_address = selected::render::turn_address(&component, source_key, key, root);
    if address.is_some_and(|address| root_address.as_ref() != Some(&json!(address))) {
        return Err(ReadError::Restart);
    }
    let mut evidence = vec![match root_address {
        Some(address) => json!({"catalog_key": key, "turn": root, "address": address,
            "peer":origin.map(|peer| json!({"trace":peer.trace,"witness":peer.witness}))}),
        None => {
            json!({"catalog_key": key, "turn": root, "reason": "The selected turn has no rendered transcript entry."})
        }
    }];
    let mut participants = serde_json::Map::new();
    let mut handoffs = Vec::new();
    let mut start = None;
    if crate::model::is_ask(root)
        && let Some(at) = record.at
    {
        start = Some(root.to_owned());
        handoffs.push(json!({
            "id":root,"kind":"ask","from":"you","to":key,"status":"done","declined":false,
            "at":at,"target_turn":root,"brief_session":key,
        }));
    } else if scope == CatalogReadScope::RetainedHistory
        && root == crate::model::spawn_turn_id(&component.harness, key)
        && let Some(at) = record.at
        && let Some(parent) = component
            .retained_metadata
            .as_ref()
            .and_then(|metadata| metadata["parent"].as_str())
            .filter(|parent| *parent != key && *parent != "you")
    {
        let peer = selected::PeerBinding {
            trace: key,
            witness: &component.identity.generation,
        };
        participants.insert(parent.to_owned(), json!(parent));
        evidence.push(launch_evidence(&view, source, parent, peer, provider)?);
        start = Some(root.to_owned());
        handoffs.push(json!({
            "id":root,"kind":"spawn","from":parent,"to":key,"status":"off","declined":false,
            "at":at,"target_turn":root,"brief_session":key,
        }));
    }
    let current = if scope == CatalogReadScope::Current {
        let (owner, harness, _) = {
            let tx = view.lock()?;
            super::current_identity(&tx, source, key, None)?.ok_or(ReadError::Restart)?
        };
        Some(super::selected_metadata(
            &view,
            source,
            key,
            owner,
            &harness,
            &component.identity.source_refs,
            false,
        )?)
    } else {
        None
    };
    if root == crate::model::spawn_turn_id(&component.harness, key)
        && let Some(at) = record.at
        && let Some(parent) = current
            .as_ref()
            .and_then(|row| row.parent.as_deref())
            .filter(|parent| *parent != key && *parent != "you")
    {
        participants.insert(parent.to_owned(), json!(parent));
        evidence.push(current_launch_evidence(&view, source, key, parent)?);
        start = Some(root.to_owned());
        handoffs.push(json!({
            "id":root,"kind":"spawn","from":parent,"to":key,"status":"off","declined":false,
            "at":at,"target_turn":root,"brief_session":key,
        }));
    }
    let metadata = component.retained_metadata.as_ref();
    let name = current
        .as_ref()
        .map(|row| row.title.as_str())
        .or_else(|| metadata.and_then(|row| row["name"].as_str()))
        .or(component.title.as_deref())
        .unwrap_or(key);
    let model = current
        .as_ref()
        .map(|row| row.model.as_str())
        .or_else(|| metadata.and_then(|row| row["model"].as_str()))
        .or(component.model.as_deref())
        .unwrap_or("");
    let first = current
        .as_ref()
        .and_then(|row| row.start)
        .or_else(|| metadata.and_then(|row| row["start"].as_i64()))
        .or(record.at)
        .unwrap_or_default();
    let last = current
        .as_ref()
        .map(|row| row.position.last)
        .filter(|last| *last != 0)
        .or_else(|| metadata.and_then(|row| row["last"].as_i64()))
        .unwrap_or(first);
    let tokens = current
        .as_ref()
        .map(|row| json!(row.token_millions()))
        .or_else(|| metadata.and_then(|row| row.get("tokens")).cloned())
        .unwrap_or_else(|| json!([0, 0, 0]));
    let cost = current
        .as_ref()
        .and_then(|row| row.cost.as_ref())
        .map(|cost| json!(cost))
        .or_else(|| metadata.and_then(|row| row.get("cost")).cloned())
        .unwrap_or_else(|| json!({"usd":null,"unpriced_models":[]}));
    let status = current.as_ref().and_then(|row| row.status);
    let mut sessions = json!({(key):{
            "name":name,"harness":component.harness,"model":model,"state":status.unwrap_or("off"),"live_observation":status.is_some(),"start":first,"last":last,
            "tokens":tokens,"cost":cost,"busy":[],"wait_edges":[],"wait_edges_truncated":false,
    }});
    let (end, last_turn) = recorded_end(&view, source, &component, record)?;
    let mut turns = vec![
        json!({"id":root,"sid":key,"start":start,"at":record.at,"u":!turn.handoff,
        "sent":[],"end":end,"last":last_turn}),
    ];
    let mut previews = json!({(key):component.identity.generation});
    let root_identity = component.identity.clone();
    let peer = origin.or_else(|| {
        (scope == CatalogReadScope::RetainedHistory).then_some(selected::PeerBinding {
            trace: key,
            witness: &root_identity.generation,
        })
    });
    let mut ids = BTreeSet::from([root.to_owned()]);
    let mut reported = BTreeSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut frontier = Vec::new();
    let mut reads = 0;
    let root_address = record.address.clone();
    let mut next = Some((0, component, root_address));
    while let Some((index, component, address)) = next {
        let Some(record) = component
            .records
            .iter()
            .find(|record| record.address == address)
        else {
            return Err(ReadError::Restart);
        };
        let from = &component.identity.catalog_key;
        let from_turn = record
            .causal
            .turn
            .as_ref()
            .ok_or(ReadError::Restart)?
            .id
            .as_str();
        let mut refusals = Vec::new();
        let relays = match peer {
            Some(peer) => retained_relays(
                &view,
                source,
                &component,
                peer,
                record,
                provider,
                &mut refusals,
            )?,
            None => current_relays(&view, source, &component, from, record, &mut refusals)?,
        };
        reads += 1;
        let mut sent = Vec::new();
        for receipt in relays {
            let Relay {
                key: target,
                harness,
                turn,
                address,
                sent_at,
                received_at,
                generation,
                window,
            } = receipt;
            if target == from.as_str() || target == "you" || !ids.insert(turn.clone()) {
                continue;
            }
            participants.remove(&target);
            previews[target.as_str()] = json!(generation);
            if sessions.get(&target).is_none() {
                sessions[target.as_str()] = json!({
                    "name":target,"harness":harness,"model":"","state":"off","start":received_at,"last":received_at,
                    "tokens":[0,0,0],"cost":{"usd":null,"unpriced_models":[]},
                    "busy":[],"wait_edges":[],"wait_edges_truncated":false,
                });
            }
            handoffs.push(json!({
                "id":turn,"kind":"relay","from":from,"to":target,"at":sent_at,"status":"done",
                "declined":false,"target_turn":turn,"source_turn":from_turn,"brief_session":target,
            }));
            let (end, last) = if let Some((window, native)) = &window {
                let receiving = window
                    .records
                    .iter()
                    .find(|record| record.address == *native)
                    .ok_or(ReadError::Restart)?;
                recorded_end(&view, source, window, receiving)?
            } else {
                (json!({"st":"off","why":"unavailable"}), false)
            };
            if let Some((window, native)) = window {
                queue.push_back((turns.len(), window, native));
            }
            turns.push(json!({
                "id":turn,"sid":target,"start":turn,"at":received_at,"u":false,"sent":[],
                "end":end,"last":last,
            }));
            evidence.push(json!({
                "catalog_key":target,"turn":turn,"address":address,
                "peer":peer.filter(|peer| target != peer.trace).map(|peer| json!({"trace":peer.trace,"witness":peer.witness})),
            }));
            sent.push(turn);
        }
        for (id, target, at, reason) in refusals {
            if target == from.as_str() || target == "you" || !ids.insert(id.clone()) {
                continue;
            }
            if sessions.get(&target).is_none() {
                participants
                    .entry(target.clone())
                    .or_insert_with(|| json!(target));
            }
            if reported.insert(target.clone())
                && !evidence
                    .iter()
                    .any(|row| row["catalog_key"] == target && row.get("turn").is_none())
            {
                evidence.push(json!({"catalog_key":target,"reason":reason.chars().take(256).collect::<String>()}));
            }
            handoffs.push(
                json!({"id":id,"kind":"relay","from":from,"to":target,"at":at,
                "status":"off","declined":false,"source_turn":from_turn}),
            );
            sent.push(id);
        }
        turns[index]["sent"] = json!(sent);
        // Each expanded turn considers at most sixteen relay candidates. Stop
        // after four turns so the response stays below the Viewer's row bound.
        if reads == 4 {
            frontier.extend(queue.drain(..).map(|(index, _, _)| {
                json!({
                    "catalog_key":turns[index]["sid"],"turn":turns[index]["id"],
                })
            }));
            break;
        }
        next = queue.pop_front();
    }
    let previews_incomplete = !reported.is_empty();
    Ok(Some(json!({
        "api":1,"projection":7,"source_key":source_key,"catalog_key":key,"root":root,
        "generation":root_identity.generation,"identity":root_identity,
        "sessions":sessions,"participants":participants,"handoffs":handoffs,"turns":turns,
        "frontier":frontier,"evidence":evidence,"previews":previews,
        "incomplete":true,"previews_incomplete":previews_incomplete,
    })))
}

pub(super) fn recorded_end(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    root: &selected::Record,
) -> Result<(Value, bool), ReadError> {
    let unavailable = || json!({"st":"off","why":"unavailable"});
    // A Codex terminal fact names the same native turn as a reducer-qualified
    // start. Stop at the next human turn so a later task cannot close this one.
    if component.harness != "codex"
        || root.causal.turn.as_ref().is_none_or(|turn| turn.handoff)
        || root.prompt_anchor.is_none()
    {
        return Ok((unavailable(), false));
    }
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
    let last = next.is_none() && component.member_count == 1;
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
            }) && super::selected_runtime_status(&tx, source, source_id)? == Some("work")
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

struct Relay {
    key: String,
    harness: String,
    turn: String,
    address: Value,
    sent_at: i64,
    received_at: i64,
    generation: String,
    window: Option<(selected::Component, selected::Address)>,
}

struct Send {
    address: selected::Address,
    block: u32,
    recipient: String,
    at: i64,
}

fn relay_sends(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    root: &selected::Record,
    retained: Option<&str>,
) -> Result<Vec<Send>, ReadError> {
    let Some(harness) = crate::InputRoot::parse(&component.harness) else {
        return Ok(Vec::new());
    };
    let tx = view.lock()?;
    let mut query = tx
        .prepare_cached(
            "WITH member AS (
           SELECT CAST(m.key AS INTEGER) FROM json_each(?1) m
           WHERE json_extract(m.value,'$[0]')=?2 AND json_extract(m.value,'$[1]')=?5
           UNION ALL
           SELECT w.source FROM warehouse_current w
           CROSS JOIN warehouse_source s ON s.source=w.source AND s.origin=?3
           WHERE ?1 IS NULL AND w.path=?2
         )
         SELECT s.pos,r.occurrence,s.block,s.target,s.ts FROM member m
         CROSS JOIN fact_relay_send s ON s.source=m.source AND s.pos>=?4 AND s.pos<?5
         CROSS JOIN fact_record r ON r.source=s.source AND r.pos=s.pos AND r.block=0 AND NOT r.gap
         WHERE s.ts>=?6 ORDER BY s.pos,s.block,s.occurrence LIMIT 16",
        )
        .map_err(super::index)?;
    let mut sends = Vec::new();
    for member in &component.identity.source_refs {
        let reference = &member.source;
        let same = reference.path == root.address.path;
        let start = if same {
            i64::try_from(root.address.offset).map_err(super::index)?
        } else {
            0
        };
        let since = if same {
            i64::MIN
        } else {
            root.at.unwrap_or(i64::MIN)
        };
        let carrier = harness.home(source.options).join(&reference.path);
        let mut rows = query
            .query(params![
                retained,
                carrier.to_str(),
                source.namespace.binding(),
                start,
                i64::try_from(reference.offset).map_err(super::index)?,
                since
            ])
            .map_err(super::index)?;
        while let Some(row) = rows.next().map_err(super::index)? {
            sends.push(Send {
                address: selected::Address {
                    path: reference.path.clone(),
                    offset: u64::try_from(row.get::<_, i64>(0).map_err(super::index)?)
                        .map_err(super::index)?,
                    block: 0,
                    occurrence: row.get(1).map_err(super::index)?,
                },
                block: row.get(2).map_err(super::index)?,
                recipient: row.get(3).map_err(super::index)?,
                at: row.get(4).map_err(super::index)?,
            });
        }
        sends.sort_by(|a, b| {
            (a.at, &a.address.path, a.address.offset, a.block).cmp(&(
                b.at,
                &b.address.path,
                b.address.offset,
                b.block,
            ))
        });
        sends.truncate(16);
    }
    Ok(sends)
}

fn retained_relays(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    peer: selected::PeerBinding<'_>,
    root: &selected::Record,
    provider: Option<&dyn SessionSourceReader>,
    refusals: &mut Vec<(String, String, i64, String)>,
) -> Result<Vec<Relay>, ReadError> {
    let sender = component.identity.catalog_key.as_str();
    let (bounds, identity, observed) = {
        let tx = view.lock()?;
        let identity =
            crate::events::store::warehouse::retained_identity(&tx, &source.namespace, peer.trace)
                .map_err(super::retained_error)?;
        let bounds = selected::retained_member_bounds(&tx, source, &identity, peer, sender, None)?;
        let observed = identity.observed_peers().map_err(super::index)?;
        (bounds, identity, observed)
    };
    let sends = relay_sends(view, source, component, root, Some(&bounds))?;
    let mut relays = Vec::new();
    let mut attempts = 0;
    for Send {
        address: send,
        block,
        recipient,
        at: sent_at,
    } in sends
    {
        let window =
            match selected::retained_peer_trace_window(view, source, sender, peer, &send, provider)
            {
                Ok(window) => window,
                Err(
                    error @ (ReadError::RetainedPreparing
                    | ReadError::RetainedUnavailable
                    | ReadError::PeerUnavailable(_)),
                ) => {
                    if crate::session_route::identifier(&recipient) {
                        refusals.push((
                            refused_relay_id(&send, block),
                            recipient,
                            sent_at,
                            error.to_string(),
                        ));
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
        if !window
            .records
            .iter()
            .any(|record| record.address == send && record.causal.turn == root.causal.turn)
        {
            continue;
        }
        let parent = component
            .retained_metadata
            .as_ref()
            .and_then(|row| row["parent"].as_str());
        let mut found = None;
        let mut candidates = observed
            .iter()
            .map(|(key, observation)| (key.as_str(), observation.native_id.as_str()))
            .chain(std::iter::once((peer.trace, identity.native_id.as_str())))
            .filter(|(key, native_id)| {
                *key != sender
                    && if recipient == "main" {
                        parent == Some(*key)
                    } else {
                        recipient == *native_id || recipient == *key
                    }
            })
            .peekable();
        let target = candidates.peek().map(|(key, _)| *key);
        let mut reason = "No certified peer receipt could be matched to this relay.".to_string();
        for (key, _) in candidates {
            if attempts == 32 {
                reason =
                    "The certified peer receipt could not be checked within this Trace request."
                        .into();
                found = None;
                break;
            }
            attempts += 1;
            let (address, turn) = match selected::retained_relay_in_view(
                view,
                source,
                peer,
                sender,
                key,
                (&send, block),
                provider,
            ) {
                Ok(receipt) => receipt,
                Err(
                    error @ (ReadError::PeerUnavailable(_)
                    | ReadError::RetainedPreparing
                    | ReadError::RetainedUnavailable),
                ) => {
                    reason = error.to_string();
                    continue;
                }
                Err(error) => return Err(error),
            };
            let window = match selected::retained_peer_trace_window(
                view,
                source,
                key,
                peer,
                &address.record,
                provider,
            ) {
                Ok(window) => window,
                Err(
                    error @ (ReadError::PeerUnavailable(_)
                    | ReadError::RetainedPreparing
                    | ReadError::RetainedUnavailable
                    | ReadError::Restart),
                ) => {
                    reason = error.to_string();
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some(received_at) = window
                .records
                .iter()
                .find(|record| record.address == address.record)
                .and_then(|record| record.at)
            else {
                continue;
            };
            if selected::render::turn_address(&window, source.namespace.as_str(), key, &turn)
                != Some(json!(address))
            {
                reason = "The certified receipt has no matching rendered entry.".into();
                continue;
            }
            if found.is_some() {
                reason = "More than one certified peer receipt matches this relay.".into();
                found = None;
                break;
            }
            found = Some(Relay {
                key: key.into(),
                harness: window.harness.clone(),
                turn,
                address: json!(address),
                sent_at,
                received_at,
                generation: window.identity.generation.clone(),
                window: Some((window, address.record)),
            });
        }
        if let Some(accepted) = found {
            relays.push(accepted);
        } else {
            let target = target.or_else(|| (recipient == "main").then_some(parent).flatten());
            let label = target.unwrap_or(&recipient);
            if crate::session_route::identifier(label) {
                if target.is_none() {
                    reason = "The recipient is absent from the original certified peer set.".into();
                }
                refusals.push((
                    refused_relay_id(&send, block),
                    label.into(),
                    sent_at,
                    reason,
                ));
            }
        }
    }
    Ok(relays)
}

fn current_relays(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    key: &str,
    root: &selected::Record,
    refusals: &mut Vec<(String, String, i64, String)>,
) -> Result<Vec<Relay>, ReadError> {
    let sends = relay_sends(view, source, component, root, None)?;
    let mut out = Vec::new();
    for Send {
        address: mut call,
        block,
        at: sent_at,
        recipient,
    } in sends
    {
        let Some(window) = selected::trace_record(view, source, key, &call)? else {
            continue;
        };
        if !window
            .records
            .iter()
            .any(|record| record.address == call && record.causal.turn == root.causal.turn)
        {
            continue;
        }
        call.block = i64::from(block);
        let mut reason = "No receipt could be matched to this relay in the selected prefixes.";
        let mut read = || -> Result<Option<Relay>, ReadError> {
            let Some(results) = selected::trace_results(view, source, key, &call)? else {
                return Ok(None);
            };
            let receipts = results
                .records
                .into_iter()
                .flat_map(|record| record.relay_receipts)
                .map(|receipt| (receipt.path, receipt.offset, receipt.block))
                .collect::<BTreeSet<_>>();
            let (carrier, offset, _) = match receipts.len() {
                0 => return Ok(None),
                1 => receipts.into_iter().next().expect("one receipt"),
                _ => {
                    reason = "More than one receipt matches this relay in the selected prefixes.";
                    return Ok(None);
                }
            };
            reason = "The matched relay receipt has no qualified transcript entry.";
            let hit = {
                let tx = view.lock()?;
                let mut query = tx.prepare_cached(
                "SELECT b.owner,s.harness,r.occurrence FROM warehouse_current w
                 CROSS JOIN warehouse_source s ON s.source=w.source AND s.origin=?2
                 CROSS JOIN bridge_session_owner b ON b.source=w.source
                 CROSS JOIN fact_record r ON r.source=w.source AND r.pos=?3 AND r.block=0 AND NOT r.gap
                 WHERE w.path=?1 AND r.pos<json_extract(s.ledger,'$.offset') LIMIT 2",
            ).map_err(super::index)?;
                let mut rows = query
                    .query(params![
                        carrier,
                        source.namespace.binding(),
                        i64::try_from(offset).map_err(super::index)?
                    ])
                    .map_err(super::index)?;
                let Some(row) = rows.next().map_err(super::index)? else {
                    return Ok(None);
                };
                let owner: i64 = row.get(0).map_err(super::index)?;
                let harness: String = row.get(1).map_err(super::index)?;
                let occurrence = row.get(2).map_err(super::index)?;
                if rows.next().map_err(super::index)?.is_some() {
                    return Ok(None);
                }
                let Some((_, target, _)) =
                    super::relative_keys(&tx, &super::encode(&[owner])?)?.pop()
                else {
                    return Ok(None);
                };
                (target, harness, occurrence)
            };
            let (target, harness, occurrence) = hit;
            if target == key {
                return Ok(None);
            }
            let Some(path) = selected::relative(source.options, &harness, &carrier) else {
                return Ok(None);
            };
            let native = selected::Address {
                path: path.into(),
                offset,
                block: 0,
                occurrence,
            };
            let Some(window) = selected::trace_record(view, source, &target, &native)? else {
                return Ok(None);
            };
            let Some(record) = window
                .records
                .iter()
                .find(|record| record.address == native)
            else {
                return Ok(None);
            };
            let Some(turn) =
                record.causal.turn.as_ref().filter(|turn| {
                    turn.handoff && turn.address == native && turn.id.starts_with('r')
                })
            else {
                return Ok(None);
            };
            let Some(address) = selected::render::turn_address(
                &window,
                source.namespace.as_str(),
                &target,
                &turn.id,
            ) else {
                return Ok(None);
            };
            let Some(received_at) = record.at else {
                return Ok(None);
            };
            Ok(Some(Relay {
                key: target,
                harness,
                turn: turn.id.clone(),
                address,
                sent_at,
                received_at,
                generation: window.identity.generation.clone(),
                window: Some((window, native)),
            }))
        };
        match read()? {
            Some(relay) => out.push(relay),
            None if crate::session_route::identifier(&recipient) => {
                refusals.push((
                    refused_relay_id(&call, block),
                    recipient,
                    sent_at,
                    reason.into(),
                ));
            }
            None => (),
        }
    }
    Ok(out)
}

fn refused_relay_id(address: &selected::Address, block: u32) -> String {
    crate::model::legacy_relay_turn_id(&format!(
        "unavailable:{}:{}:{block}",
        address.path, address.offset
    ))
}

fn current_launch_evidence(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    child_key: &str,
    parent: &str,
) -> Result<Value, ReadError> {
    let locate = || -> Result<Option<(selected::Address, u32)>, ReadError> {
        let tx = view.lock()?;
        let Some((child, _, _)) = super::current_identity(&tx, source, child_key, None)? else {
            return Ok(None);
        };
        let Some((parent, harness, _)) = super::current_identity(&tx, source, parent, None)? else {
            return Ok(None);
        };
        let mut graph = super::parents::Parents::new(&tx)?;
        let Some(edge) = graph.parent(child)? else {
            return Ok(None);
        };
        if !matches!(edge.method.as_str(), "native" | "marker")
            || graph.owner(edge.source)? != Some(parent)
        {
            return Ok(None);
        }
        let marker = edge.method == "marker";
        let call = edge.call;
        drop(graph);
        drop(tx);
        let call = match call {
            Some(call) => call,
            None if marker => match selected::launch_call(view, source, child_key)? {
                Some(call) => call.id,
                None => return Ok(None),
            },
            None => return Ok(None),
        };
        let tx = view.lock()?;
        let mut query = tx.prepare_cached(
            "SELECT s.carrier,c.pos,r.occurrence,c.block
             FROM lineage_member root CROSS JOIN lineage_member m USING(component)
             CROSS JOIN warehouse_source s ON s.source=m.source AND s.origin=?2
             CROSS JOIN warehouse_current w ON w.source=s.source AND w.path=s.carrier
             CROSS JOIN fact_tool_call c ON c.source=s.source AND c.call_id=?3
             CROSS JOIN fact_record r ON r.source=s.source AND r.pos=c.pos AND r.block=0 AND NOT r.gap
             WHERE root.source=?1 AND c.pos<json_extract(s.ledger,'$.offset') LIMIT 2",
        ).map_err(super::index)?;
        let mut rows = query
            .query(params![parent, source.namespace.binding(), call])
            .map_err(super::index)?;
        let Some(row) = rows.next().map_err(super::index)? else {
            return Ok(None);
        };
        let carrier: String = row.get(0).map_err(super::index)?;
        let Some(path) = selected::relative(source.options, &harness, &carrier) else {
            return Ok(None);
        };
        let address = selected::Address {
            path: path.to_owned(),
            offset: u64::try_from(row.get::<_, i64>(1).map_err(super::index)?)
                .map_err(super::index)?,
            block: 0,
            occurrence: row.get(2).map_err(super::index)?,
        };
        let block: u32 = row.get(3).map_err(super::index)?;
        Ok(rows
            .next()
            .map_err(super::index)?
            .is_none()
            .then_some((address, block)))
    };
    let unavailable = || json!({"catalog_key":parent,"reason":"No exact launch call could be proven in the selected prefixes."});
    let (address, block) = match locate() {
        Ok(Some(located)) => located,
        Ok(None) | Err(ReadError::Restart) => return Ok(unavailable()),
        Err(error) => return Err(error),
    };
    let window = match selected::trace_record(view, source, parent, &address) {
        Ok(Some(window)) => window,
        Ok(None) | Err(ReadError::Restart) => return Ok(unavailable()),
        Err(error) => return Err(error),
    };
    Ok(rendered_call(&window, source, parent, block).map_or_else(
        unavailable,
        |address| json!({"catalog_key":parent,"address":address}),
    ))
}

fn rendered_call(
    window: &selected::Component,
    source: &Source<'_>,
    key: &str,
    block: u32,
) -> Option<Value> {
    selected::render::entries(window, source.namespace.as_str(), key, 0)
        .into_iter()
        .find(|entry| entry["k"] == "tool" && entry["address"]["block"] == block)
        .and_then(|mut entry| entry.as_object_mut()?.remove("address"))
}

fn launch_evidence(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    parent: &str,
    peer: selected::PeerBinding<'_>,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Value, ReadError> {
    let read = || -> Result<Value, ReadError> {
        let (address, block, _) = selected::retained_peer_launch_in_view(
            view,
            source,
            peer.trace,
            peer.witness,
            parent,
            provider,
        )?;
        let window =
            selected::retained_peer_trace_window(view, source, parent, peer, &address, provider)?;
        rendered_call(&window, source, parent, block).ok_or_else(|| {
            ReadError::PeerUnavailable("The launch call has no rendered transcript entry.".into())
        })
    };
    let reason = match read() {
        Ok(address) => {
            return Ok(
                json!({"catalog_key":parent,"address":address,"peer":{"trace":peer.trace,"witness":peer.witness}}),
            );
        }
        Err(ReadError::PeerUnavailable(reason)) => reason,
        Err(ReadError::RetainedPreparing) => "Preparing recorded history.".into(),
        Err(
            ReadError::RetainedUnavailable | ReadError::RetainedUnverified | ReadError::Restart,
        ) => "The certified launch source could not be verified.".into(),
        Err(error) => return Err(error),
    };
    Ok(json!({"catalog_key":parent,"reason":reason.chars().take(256).collect::<String>()}))
}
