//! Assemble selected Trace evidence from one committed warehouse view.
use super::{CatalogReadScope, ReadError, Source, selected};
use crate::SessionSourceReader;
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) fn read(
    source: &Source<'_>,
    scope: CatalogReadScope,
    key: &str,
    root: &str,
    retained_provider: Option<&dyn SessionSourceReader>,
) -> Result<Option<Value>, ReadError> {
    let view = selected::ReadView::open(source)?;
    let provider =
        retained_provider.map(|reader| crate::source_reader::BudgetedReader::new(reader, 1024));
    let provider = provider
        .as_ref()
        .map(|reader| reader as &dyn SessionSourceReader);
    let Some(component) = selected::trace_turn(
        &view,
        source,
        scope,
        key,
        root,
        provider.unwrap_or(&view.archive),
    )?
    else {
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
    let mut evidence = vec![match root_address {
        Some(address) => json!({"catalog_key": key, "turn": root, "address": address}),
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
        let tx = view.lock()?;
        let (owner, harness, _) =
            super::current_identity(&tx, source, key, None)?.ok_or(ReadError::Restart)?;
        Some(super::selected_metadata_at(
            &tx,
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
    let mut turns = vec![
        json!({"id":root,"sid":key,"start":start,"at":record.at,"u":!turn.handoff,
        "sent":[],"end":{"st":"off","why":"unavailable"},"last":false}),
    ];
    let mut sent = Vec::new();
    let peer = (scope == CatalogReadScope::RetainedHistory).then_some(selected::PeerBinding {
        trace: key,
        witness: &component.identity.generation,
    });
    let relays = match peer {
        Some(peer) => retained_relays(&view, source, &component, peer, record, provider)?,
        None => current_relays(&view, source, &component, key, record)?,
    };
    let mut ids = BTreeSet::from([root.to_owned()]);
    for receipt in relays {
        let Relay {
            key: target,
            harness,
            turn,
            address,
            sent_at,
            received_at,
        } = receipt;
        if !ids.insert(turn.clone()) || target == key || target == "you" {
            continue;
        }
        participants.remove(&target);
        sessions[target.as_str()] = json!({
            "name":target,"harness":harness,"model":"","state":"off","start":received_at,"last":received_at,
            "tokens":[0,0,0],"cost":{"usd":null,"unpriced_models":[]},
            "busy":[],"wait_edges":[],"wait_edges_truncated":false,
        });
        handoffs.push(json!({
            "id":turn,"kind":"relay","from":key,"to":target,"at":sent_at,"status":"done",
            "declined":false,"target_turn":turn,"source_turn":root,
        }));
        turns.push(json!({
            "id":turn,"sid":target,"start":turn,"at":received_at,"u":false,"sent":[],
            "end":{"st":"off","why":"unavailable"},"last":false,
        }));
        evidence.push(json!({
            "catalog_key":target,"turn":turn,"address":address,
            "peer":peer.map(|peer| json!({"trace":peer.trace,"witness":peer.witness})),
        }));
        sent.push(turn);
    }
    let previews_incomplete = !sent.is_empty();
    turns[0]["sent"] = json!(sent);
    Ok(Some(json!({
        "api":1,"projection":7,"source_key":source_key,"catalog_key":key,"root":root,
        "generation":component.identity.generation,"identity":component.identity,
        "sessions":sessions,"participants":participants,"handoffs":handoffs,"turns":turns,
        "frontier":[],"evidence":evidence,"previews":{(key):component.identity.generation},
        "incomplete":true,"previews_incomplete":previews_incomplete,
    })))
}

struct Relay {
    key: String,
    harness: String,
    turn: String,
    address: Value,
    sent_at: i64,
    received_at: i64,
}

fn retained_relays(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    peer: selected::PeerBinding<'_>,
    root: &selected::Record,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Vec<Relay>, ReadError> {
    let tx = view.lock()?;
    let identity =
        crate::events::store::warehouse::retained_identity(&tx, &source.namespace, peer.trace)
            .map_err(super::retained_error)?;
    let observed = identity.observed_peers().map_err(super::index)?;
    let Some(harness) = crate::InputRoot::parse(&component.harness) else {
        return Ok(Vec::new());
    };
    let path = harness.home(source.options).join(&root.address.path);
    let mut query = tx
        .prepare_cached(
            "SELECT s.pos,r.occurrence,s.block,s.target,s.ts FROM warehouse_identity_member m
         CROSS JOIN fact_relay_send s ON s.source=m.source AND s.pos>=?3
         CROSS JOIN fact_record r ON r.source=s.source AND r.pos=s.pos AND r.block=0 AND NOT r.gap
         WHERE m.identity=?1 AND json_extract(m.receipt,'$.path')=?2
           AND s.pos<json_extract(m.receipt,'$.offset') AND s.ts IS NOT NULL
         ORDER BY s.pos,s.block,s.occurrence LIMIT 16",
        )
        .map_err(super::index)?;
    let sends = query
        .query_map(
            params![
                identity.binding,
                path.to_str(),
                i64::try_from(root.address.offset).map_err(super::index)?
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .map_err(super::index)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(super::index)?;
    drop(query);
    drop(tx);
    let mut relays = Vec::new();
    let mut attempts = 0;
    for (offset, occurrence, block, recipient, sent_at) in sends {
        let send = selected::Address {
            path: root.address.path.clone(),
            offset: u64::try_from(offset).map_err(super::index)?,
            block: 0,
            occurrence,
        };
        let window = selected::retained_trace_send_window(
            view,
            source,
            peer.trace,
            &send,
            &component.binding,
            provider,
        )?;
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
        for (key, observation) in observed.iter().filter(|(key, observation)| {
            if recipient == "main" {
                parent == Some(key.as_str())
            } else {
                recipient == observation.native_id || recipient == key.as_str()
            }
        }) {
            if attempts == 32 {
                return Ok(relays);
            }
            attempts += 1;
            let (address, turn) = match selected::retained_peer_relay_in_view(
                view,
                source,
                peer.trace,
                peer.witness,
                key,
                (&send, block),
                provider,
            ) {
                Ok(receipt) => receipt,
                Err(ReadError::PeerUnavailable(_) | ReadError::RetainedPreparing) => continue,
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
                    ReadError::PeerUnavailable(_)
                    | ReadError::RetainedPreparing
                    | ReadError::RetainedUnavailable
                    | ReadError::Restart,
                ) => continue,
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
                continue;
            }
            if found.is_some() {
                found = None;
                break;
            }
            found = Some(Relay {
                key: key.clone(),
                harness: observation.harness.clone(),
                turn,
                address: json!(address),
                sent_at,
                received_at,
            });
        }
        relays.extend(found);
    }
    Ok(relays)
}

fn current_relays(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    key: &str,
    root: &selected::Record,
) -> Result<Vec<Relay>, ReadError> {
    let Some(harness) = crate::InputRoot::parse(&component.harness) else {
        return Ok(Vec::new());
    };
    let carrier = harness.home(source.options).join(&root.address.path);
    let sends = {
        let tx = view.lock()?;
        let mut query = tx.prepare_cached(
            "SELECT s.pos,r.occurrence,s.block,s.ts FROM warehouse_current w
             CROSS JOIN warehouse_source ws ON ws.source=w.source AND ws.origin=?2
             CROSS JOIN fact_relay_send s ON s.source=w.source AND s.pos>=?3
             CROSS JOIN fact_record r ON r.source=s.source AND r.pos=s.pos AND r.block=0 AND NOT r.gap
             WHERE w.path=?1 AND s.pos<?4 AND s.ts IS NOT NULL
             ORDER BY s.pos,s.block,s.occurrence LIMIT 16",
        ).map_err(super::index)?;
        query
            .query_map(
                params![
                    carrier.to_str(),
                    source.namespace.binding(),
                    i64::try_from(root.address.offset).map_err(super::index)?,
                    i64::try_from(root.reference.offset).map_err(super::index)?
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(super::index)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(super::index)?
    };
    let mut out = Vec::new();
    for (offset, occurrence, block, sent_at) in sends {
        let mut call = selected::Address {
            path: root.address.path.clone(),
            offset: u64::try_from(offset).map_err(super::index)?,
            block: 0,
            occurrence,
        };
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
        call.block = block;
        let Some(results) = selected::trace_results(view, source, key, &call)? else {
            continue;
        };
        let receipts = results
            .records
            .into_iter()
            .flat_map(|record| record.relay_receipts)
            .map(|receipt| (receipt.path, receipt.offset))
            .collect::<BTreeSet<_>>();
        if receipts.len() != 1 {
            continue;
        }
        let Some((carrier, offset)) = receipts.into_iter().next() else {
            continue;
        };
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
                continue;
            };
            let owner: i64 = row.get(0).map_err(super::index)?;
            let harness: String = row.get(1).map_err(super::index)?;
            let occurrence = row.get(2).map_err(super::index)?;
            if rows.next().map_err(super::index)?.is_some() {
                continue;
            }
            let Some((_, target, _)) = super::relative_keys(&tx, &super::encode(&[owner])?)?.pop()
            else {
                continue;
            };
            (target, harness, occurrence)
        };
        let (target, harness, occurrence) = hit;
        if target == key {
            continue;
        }
        let Some(path) = selected::relative(source.options, &harness, &carrier) else {
            continue;
        };
        let native = selected::Address {
            path: path.into(),
            offset,
            block: 0,
            occurrence,
        };
        let Some(window) = selected::trace_record(view, source, &target, &native)? else {
            continue;
        };
        let Some(record) = window
            .records
            .iter()
            .find(|record| record.address == native)
        else {
            continue;
        };
        let Some(turn) = record
            .causal
            .turn
            .as_ref()
            .filter(|turn| turn.handoff && turn.address == native && turn.id.starts_with('r'))
        else {
            continue;
        };
        let Some(address) =
            selected::render::turn_address(&window, source.namespace.as_str(), &target, &turn.id)
        else {
            continue;
        };
        let Some(received_at) = record.at else {
            continue;
        };
        out.push(Relay {
            key: target,
            harness,
            turn: turn.id.clone(),
            address,
            sent_at,
            received_at,
        });
    }
    Ok(out)
}

fn current_launch_evidence(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    child: &str,
    parent: &str,
) -> Result<Value, ReadError> {
    let locate = || -> Result<Option<(selected::Address, u32)>, ReadError> {
        let tx = view.lock()?;
        let Some((child, _, _)) = super::current_identity(&tx, source, child, None)? else {
            return Ok(None);
        };
        let Some((parent, harness, _)) = super::current_identity(&tx, source, parent, None)? else {
            return Ok(None);
        };
        let mut graph = super::parents::Parents::new(&tx)?;
        let Some(edge) = graph.parent(child)? else {
            return Ok(None);
        };
        let Some(call) = edge
            .call
            .filter(|_| matches!(edge.method.as_str(), "native" | "marker"))
        else {
            return Ok(None);
        };
        if graph.owner(edge.source)? != Some(parent) {
            return Ok(None);
        }
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
    let Some((address, block)) = locate()? else {
        return Ok(unavailable());
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
