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
    let mut launch_return = None;
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
        let (launch, synchronous) = launch_evidence(&view, source, parent, peer, provider)?;
        let synchronous_evidence = synchronous.as_ref().map(|_| return_at_launch(&launch));
        evidence.push(launch);
        launch_return = synchronous;
        let mut reason = None;
        match selected::retained_spawn_return(&view, source, peer, parent, provider) {
            Ok(Some(completion)) => {
                launch_return = Some(explicit_return(
                    &view,
                    source,
                    key,
                    parent,
                    &completion,
                    Some(peer),
                    provider,
                    &mut evidence,
                )?);
            }
            Ok(None) => evidence.extend(synchronous_evidence),
            Err(ReadError::PeerUnavailable(message)) => reason = Some(message),
            Err(ReadError::SourceBudget) => {
                reason = Some("The selected return read budget was exhausted.".into())
            }
            Err(ReadError::RetainedPreparing) => {
                reason = Some("Preparing recorded history.".into())
            }
            Err(
                ReadError::RetainedUnavailable | ReadError::RetainedUnverified | ReadError::Restart,
            ) => {
                reason = Some("The certified return source could not be verified.".into());
            }
            Err(error) => return Err(error),
        }
        start = Some(root.to_owned());
        let returned = launch_return.as_ref();
        handoffs.push(json!({
            "id":root,"kind":"spawn","from":parent,"to":key,
            "status":returned.map_or("off", LaunchReturn::status),"declined":false,
            "at":at,"done":returned.and_then(|result| result.at),
            "target_turn":root,"brief_session":key,
            "result":returned.and_then(|result| result.preview.as_deref()),
        }));
        if let Some(reason) = reason {
            evidence.push(
                json!({"catalog_key":parent,"role":"return","reason":reason.chars().take(256).collect::<String>()}),
            );
        }
    }
    let current = current_metadata(&view, source, &component)?;
    if root == crate::model::spawn_turn_id(&component.harness, key)
        && let Some(at) = record.at
        && let Some(parent) = current
            .as_ref()
            .and_then(|row| row.parent.as_deref())
            .filter(|parent| *parent != key && *parent != "you")
    {
        participants.insert(parent.to_owned(), json!(parent));
        let (launch, synchronous) = current_launch_evidence(&view, source, key, parent)?;
        let synchronous_evidence = synchronous.as_ref().map(|_| return_at_launch(&launch));
        evidence.push(launch);
        start = Some(root.to_owned());
        launch_return = if let Some(completion) = &component.trace_completion {
            Some(explicit_return(
                &view,
                source,
                key,
                parent,
                completion,
                None,
                provider,
                &mut evidence,
            )?)
        } else {
            evidence.extend(synchronous_evidence);
            synchronous
        };
        let returned = launch_return.as_ref();
        let resumed = returned.is_some_and(|result| result.resumed);
        let status = if resumed
            && current
                .as_ref()
                .is_some_and(|row| row.status == Some("work"))
        {
            "work"
        } else if resumed {
            "done"
        } else {
            returned.map_or("off", LaunchReturn::status)
        };
        handoffs.push(json!({
            "id":root,"kind":"spawn","from":parent,"to":key,
            "status":status,"declined":false,
            "at":at,"done":returned.and_then(|result| result.at),
            "target_turn":root,"brief_session":key,
            "result":returned.and_then(|result| result.preview.as_deref()),
        }));
    }
    let mut sessions = json!({(key):session_summary(&component, current.as_ref(), record.at)});
    let mut previews = json!({(key):component.identity.generation});
    let root_identity = component.identity.clone();
    let peer = origin.or_else(|| {
        (scope == CatalogReadScope::RetainedHistory).then_some(selected::PeerBinding {
            trace: key,
            witness: &root_identity.generation,
        })
    });
    let (end, last_turn) = if let Some(completion) = &launch_return {
        (
            json!({"st":completion.status(),"why":if completion.failed {"handoff_failed"} else {"returned"},"at":completion.at}),
            component.latest_turn.as_ref() == Some(turn),
        )
    } else {
        trace_end(&view, source, &component, record, peer, provider)?
    };
    let mut turns = vec![
        json!({"id":root,"sid":key,"start":start,"at":record.at,"u":!turn.handoff,
        "sent":[],"end":end,"last":last_turn}),
    ];
    let mut ids = BTreeSet::from([root.to_owned()]);
    let mut reported = BTreeSet::new();
    let mut qualified_participants = BTreeSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut frontier = Vec::new();
    let mut reads = 0;
    let mut relay_limit_reached = false;
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
        let (relays, limited) = match peer {
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
        relay_limit_reached |= limited;
        reads += 1;
        let mut sent = Vec::new();
        for receipt in relays {
            let Relay {
                key: target,
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
                let current = current_metadata(&view, source, &window.0)?;
                sessions[target.as_str()] =
                    session_summary(&window.0, current.as_ref(), Some(received_at));
            }
            handoffs.push(json!({
                "id":turn,"kind":"relay","from":from,"to":target,"at":sent_at,"status":"done",
                "declined":false,"target_turn":turn,"source_turn":from_turn,"brief_session":target,
            }));
            let (window, native) = window;
            let receiving = window
                .records
                .iter()
                .find(|record| record.address == native)
                .ok_or(ReadError::Restart)?;
            let (end, last) = trace_end(&view, source, &window, receiving, peer, provider)?;
            queue.push_back((turns.len(), window, native));
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
        for RelayRefusal {
            id,
            target,
            at,
            reason,
            qualified,
        } in refusals
        {
            if target == from.as_str() || target == "you" || !ids.insert(id.clone()) {
                continue;
            }
            if sessions.get(&target).is_none() {
                participants
                    .entry(target.clone())
                    .or_insert_with(|| json!(target));
            }
            if qualified {
                qualified_participants.insert(target.clone());
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
    let mut participant_summaries = serde_json::Map::new();
    for target in &qualified_participants {
        if !participants.contains_key(target) {
            continue;
        }
        match passive_summary(&view, source, target, peer) {
            Ok(Some(summary)) => {
                participants.insert(target.clone(), summary["name"].clone());
                participant_summaries.insert(target.clone(), summary);
            }
            Ok(None)
            | Err(
                ReadError::Restart
                | ReadError::SourceBudget
                | ReadError::PeerUnavailable(_)
                | ReadError::RetainedUnverified,
            ) => {}
            Err(error) => return Err(error),
        }
    }
    let previews_incomplete = !reported.is_empty();
    let incomplete = relay_limit_reached
        || !frontier.is_empty()
        || evidence.iter().any(|row| row.get("reason").is_some())
        || turns.iter().any(|turn| turn["end"]["why"] == "unavailable");
    Ok(Some(json!({
        "api":1,"projection":7,"source_key":source_key,"catalog_key":key,"root":root,
        "generation":root_identity.generation,"identity":root_identity,
        "sessions":sessions,"participants":participants,"participant_summaries":participant_summaries,
        "handoffs":handoffs,"turns":turns,
        "frontier":frontier,"evidence":evidence,"previews":previews,
        "incomplete":incomplete,"previews_incomplete":previews_incomplete,
    })))
}

pub(super) fn passive_summary(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    key: &str,
    peer: Option<selected::PeerBinding<'_>>,
) -> Result<Option<Value>, ReadError> {
    use crate::events::store::warehouse;
    let (identity, references) = {
        let tx = view.lock()?;
        let observed = if let Some(peer) = peer {
            let original = warehouse::retained_identity(&tx, &source.namespace, peer.trace)
                .map_err(super::retained_error)?;
            if original.witness != peer.witness {
                return Err(ReadError::Restart);
            }
            let (observed, current) =
                warehouse::retained_peer(&tx, &source.namespace, &original, peer.witness, key)
                    .map_err(|err| ReadError::PeerUnavailable(err.to_string()))?;
            if !current {
                let (metadata, identity) = super::retained_metadata_in_snapshot(&tx, source, key)?;
                return Ok(exact_observed_prefix(source, &observed, &identity.source_refs).then(|| json!({
                    "name": metadata["name"], "harness": identity.harness,
                    "kind": metadata["presentation_kind"].as_str().or(metadata["kind"].as_str()).unwrap_or(""),
                    "model": metadata["model"], "tokens": metadata["tokens"],
                    "cost": {"usd":metadata["cost"]["usd"],"unpriced_models":metadata["cost"]["unpriced_models"]},
                    "observed_extent": {"scope":"whole_session","last":metadata["last"],
                        "status":{"state":null,"observation":"unavailable"},
                        "recorded_activity":{"state":"unavailable"}},
                })));
            }
            Some(observed)
        } else {
            None
        };
        let Some(identity) = super::current_identity(&tx, source, key, None)? else {
            return Ok(None);
        };
        let Some(references) = super::focused_members(&tx, source, &identity)? else {
            return Ok(None);
        };
        if observed.as_ref().is_some_and(|peer| {
            identity.0 != peer.owner
                || identity.1 != peer.harness
                || identity.2 != peer.native_id
                || !exact_observed_prefix(source, peer, &references)
        }) {
            return Ok(None);
        }
        (identity, references)
    };
    let row = super::selected_metadata(
        view,
        source,
        key,
        identity.0,
        &identity.1,
        &references,
        false,
        Some(None),
    )?;
    let status = if peer.is_some() {
        json!({"state":null,"observation":"unavailable"})
    } else {
        row.recorded_status()
    };
    Ok(Some(json!({
        "name": row.title, "harness": row.harness,
        "kind": row.presentation_kind.unwrap_or(row.kind),
        "model": row.model, "tokens": row.token_millions(),
        "cost": {"usd":row.cost.as_ref().and_then(|cost| cost.usd),
            "unpriced_models":row.cost.as_ref().map_or(&[][..], |cost| cost.unpriced_models.as_slice())},
        "observed_extent": {"scope":"whole_session","last":(row.position.last != 0).then_some(row.position.last),
            "status":status,"recorded_activity":{"state":"unavailable"}},
    })))
}

fn exact_observed_prefix(
    source: &Source<'_>,
    peer: &crate::events::store::warehouse::ObservedPeer,
    references: &[crate::CatalogSourceObservation],
) -> bool {
    references.len() == peer.members.len()
        && peer.members.iter().all(|member| {
            let receipt = &member.receipt;
            let path = receipt
                .path
                .to_str()
                .and_then(|path| selected::relative(source.options, &peer.harness, path));
            references.iter().any(|reference| {
                let reference = &reference.source;
                path == Some(reference.path.as_str())
                    && receipt.native_id == reference.native_id
                    && receipt.offset == reference.offset
                    && receipt.prefix_sha256 == reference.prefix_sha256
                    && receipt.tail_sha256 == reference.tail_sha256
            })
        })
}

fn current_metadata(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
) -> Result<Option<super::Row>, ReadError> {
    if component.identity.read_scope != CatalogReadScope::Current {
        return Ok(None);
    }
    let key = &component.identity.catalog_key;
    let (owner, harness, _) = {
        let tx = view.lock()?;
        super::current_identity(&tx, source, key, None)?.ok_or(ReadError::Restart)?
    };
    super::selected_metadata(
        view,
        source,
        key,
        owner,
        &harness,
        &component.identity.source_refs,
        false,
        Some(component.latest_turn.as_ref()),
    )
    .map(Some)
}

fn session_summary(
    component: &selected::Component,
    current: Option<&super::Row>,
    at: Option<i64>,
) -> Value {
    let key = component.identity.catalog_key.as_str();
    let metadata = component.retained_metadata.as_ref();
    let name = current
        .map(|row| row.title.as_str())
        .or_else(|| metadata.and_then(|row| row["name"].as_str()))
        .or(component.title.as_deref())
        .unwrap_or(key);
    let model = current
        .map(|row| row.model.as_str())
        .or_else(|| metadata.and_then(|row| row["model"].as_str()))
        .or(component.model.as_deref())
        .unwrap_or("");
    let first = current
        .and_then(|row| row.start)
        .or_else(|| metadata.and_then(|row| row["start"].as_i64()))
        .or(at)
        .unwrap_or_default();
    let last = current
        .map(|row| row.position.last)
        .filter(|last| *last != 0)
        .or_else(|| metadata.and_then(|row| row["last"].as_i64()))
        .unwrap_or(first);
    let tokens = current
        .map(|row| json!(row.token_millions()))
        .or_else(|| metadata.and_then(|row| row.get("tokens")).cloned())
        .unwrap_or_else(|| json!([0, 0, 0]));
    let cost = current
        .and_then(|row| row.cost.as_ref())
        .map(|cost| json!(cost))
        .or_else(|| metadata.and_then(|row| row.get("cost")).cloned())
        .unwrap_or_else(|| json!({"usd":null,"unpriced_models":[]}));
    let status = current.and_then(|row| row.status);
    json!({
            "name":name,"harness":component.harness,"model":model,"state":status.unwrap_or("off"),"live_observation":status.is_some(),"start":first,"last":last,
            "tokens":tokens,"cost":cost,"busy":[],"wait_edges":[],"wait_edges_truncated":false,
    })
}

fn trace_end(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    root: &selected::Record,
    peer: Option<selected::PeerBinding<'_>>,
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
        let retained = peer
            .map(|peer| {
                let identity = crate::events::store::warehouse::retained_identity(
                    &tx,
                    &source.namespace,
                    peer.trace,
                )
                .map_err(super::retained_error)?;
                selected::retained_member_bounds(
                    &tx,
                    source,
                    &identity,
                    peer,
                    &component.identity.catalog_key,
                    None,
                )
            })
            .transpose()?;
        let harness = crate::InputRoot::parse(&component.harness).ok_or(ReadError::Restart)?;
        let order = if copilot { "ASC" } else { "DESC" };
        let sql = format!(
            "WITH member(source) AS (
               SELECT CAST(m.key AS INTEGER) FROM json_each(?1) m
               WHERE json_extract(m.value,'$[0]')=?2 AND json_extract(m.value,'$[1]')=?5
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
                    retained.as_deref(),
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
        let window = match peer {
            Some(peer) => {
                selected::retained_peer_trace_window(view, source, key, peer, &address, provider)
                    .map(Some)
            }
            None => selected::trace_record(view, source, key, &address),
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
    if root.causal.turn.as_ref().is_some_and(|turn| {
        turn.id == crate::model::spawn_turn_id(&component.harness, &component.identity.catalog_key)
    }) && let Some(receipt) = &component.legacy_completion
    {
        return Ok((json!({"st":"done","why":"returned","at":receipt.at}), last));
    }
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
    turn: String,
    address: Value,
    sent_at: i64,
    received_at: i64,
    generation: String,
    window: (selected::Component, selected::Address),
}

struct RelayRefusal {
    id: String,
    target: String,
    at: i64,
    reason: String,
    qualified: bool,
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
) -> Result<(Vec<Send>, bool), ReadError> {
    let Some(harness) = crate::InputRoot::parse(&component.harness) else {
        return Ok((Vec::new(), false));
    };
    let tx = view.lock()?;
    let mut query = tx
        .prepare_cached(
            "WITH member(source) AS (
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
         WHERE s.ts>=?6 ORDER BY s.pos,s.block,s.occurrence LIMIT 17",
        )
        .map_err(super::index)?;
    let mut sends = Vec::new();
    let mut limited = false;
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
        limited |= sends.len() > 16;
        sends.truncate(16);
    }
    Ok((sends, limited))
}

fn retained_relays(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    peer: selected::PeerBinding<'_>,
    root: &selected::Record,
    provider: Option<&dyn SessionSourceReader>,
    refusals: &mut Vec<RelayRefusal>,
) -> Result<(Vec<Relay>, bool), ReadError> {
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
    let (sends, limited) = relay_sends(view, source, component, root, Some(&bounds))?;
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
                    | ReadError::PeerUnavailable(_)
                    | ReadError::SourceBudget),
                ) => {
                    if crate::session_route::identifier(&recipient) {
                        refusals.push(RelayRefusal {
                            id: refused_relay_id(&send, block),
                            target: recipient,
                            at: sent_at,
                            reason: error.to_string(),
                            qualified: false,
                        });
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
        let qualified_target = target.filter(|_| candidates.clone().nth(1).is_none());
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
                    | ReadError::RetainedUnavailable
                    | ReadError::SourceBudget),
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
                    | ReadError::Restart
                    | ReadError::SourceBudget),
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
                turn,
                address: json!(address),
                sent_at,
                received_at,
                generation: window.identity.generation.clone(),
                window: (window, address.record),
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
                refusals.push(RelayRefusal {
                    id: refused_relay_id(&send, block),
                    target: label.into(),
                    at: sent_at,
                    reason,
                    qualified: qualified_target.is_some(),
                });
            }
        }
    }
    Ok((relays, limited))
}

fn current_relays(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    key: &str,
    root: &selected::Record,
    refusals: &mut Vec<RelayRefusal>,
) -> Result<(Vec<Relay>, bool), ReadError> {
    let (sends, limited) = relay_sends(view, source, component, root, None)?;
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
        let mut qualified_target = None;
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
            qualified_target = Some(target.clone());
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
                turn: turn.id.clone(),
                address,
                sent_at,
                received_at,
                generation: window.identity.generation.clone(),
                window: (window, native),
            }))
        };
        match read()? {
            Some(relay) => out.push(relay),
            None if qualified_target.is_some() || crate::session_route::identifier(&recipient) => {
                let qualified = qualified_target.is_some();
                refusals.push(RelayRefusal {
                    id: refused_relay_id(&call, block),
                    target: qualified_target.unwrap_or(recipient),
                    at: sent_at,
                    reason: reason.into(),
                    qualified,
                });
            }
            None => (),
        }
    }
    Ok((out, limited))
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
) -> Result<(Value, Option<LaunchReturn>), ReadError> {
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
        Ok(None) | Err(ReadError::Restart) => return Ok((unavailable(), None)),
        Err(error) => return Err(error),
    };
    let window = match selected::trace_record(view, source, parent, &address) {
        Ok(Some(window)) => window,
        Ok(None) | Err(ReadError::Restart) => return Ok((unavailable(), None)),
        Err(error) => return Err(error),
    };
    let Some(rendered) = rendered_call(&window, source, parent, &address, block) else {
        return Ok((unavailable(), None));
    };
    Ok((
        json!({"catalog_key":parent,"address":rendered}),
        synchronous_return(&window, &address, block),
    ))
}

fn rendered_call(
    window: &selected::Component,
    source: &Source<'_>,
    key: &str,
    record: &selected::Address,
    block: u32,
) -> Option<Value> {
    let record = json!(record);
    selected::render::entries(window, source.namespace.as_str(), key, 0)
        .into_iter()
        .find(|entry| {
            entry["k"] == "tool"
                && entry["address"]["record"] == record
                && entry["address"]["block"] == block
        })
        .and_then(|mut entry| entry.as_object_mut()?.remove("address"))
}

struct LaunchReturn {
    at: Option<i64>,
    failed: bool,
    resumed: bool,
    preview: Option<String>,
}

impl LaunchReturn {
    fn status(&self) -> &'static str {
        if self.failed { "err" } else { "done" }
    }
}

fn synchronous_return(
    window: &selected::Component,
    address: &selected::Address,
    block: u32,
) -> Option<LaunchReturn> {
    let record = window
        .records
        .iter()
        .find(|record| record.address == *address)?;
    let call = record
        .causal
        .calls
        .iter()
        .find(|call| call.block == block)?;
    if window.harness != "claude" || !matches!(call.tool.as_str(), "Agent" | "Task") {
        return None;
    }
    let reply = call.display_reply()?;
    if reply.f & (crate::events::ASYNC | crate::events::UNKNOWN | crate::events::DENIED) != 0 {
        return None;
    }
    let native = window.reply_records.get(&(address.path.clone(), reply.o))?;
    Some(LaunchReturn {
        at: reply.t,
        failed: reply.e,
        resumed: false,
        preview: crate::model::tool_result_text(native, reply.b as usize)
            .map(|text| crate::model::cap(&text, crate::model::MSG_MAX)),
    })
}

fn return_at_launch(launch: &Value) -> Value {
    let mut evidence = launch.clone();
    evidence["role"] = json!("return");
    evidence
}

#[allow(clippy::too_many_arguments)]
fn explicit_return(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    child: &str,
    parent: &str,
    completion: &selected::TraceCompletion,
    peer: Option<selected::PeerBinding<'_>>,
    provider: Option<&dyn SessionSourceReader>,
    evidence: &mut Vec<Value>,
) -> Result<LaunchReturn, ReadError> {
    let key = if completion.in_child { child } else { parent };
    let address = &completion.address;
    let window = if let Some(peer) = peer {
        selected::retained_peer_trace_window(view, source, key, peer, address, provider).map(Some)
    } else {
        selected::trace_record(view, source, key, address)
    };
    let mut reason = "The verified return has no available exact transcript entry.".to_owned();
    let entry = match window {
        Ok(Some(window)) => {
            let record = json!(address);
            let mut entries = selected::render::entries(&window, source.namespace.as_str(), key, 0)
                .into_iter()
                .filter(|entry| {
                    entry["address"]["record"] == record
                        && entry["address"]["block"] == completion.block
                });
            let first = entries.next();
            first
                .filter(|_| entries.next().is_none())
                .map(|entry| entry["address"].clone())
        }
        Ok(None) => None,
        Err(
            error @ (ReadError::Restart
            | ReadError::PeerUnavailable(_)
            | ReadError::RetainedPreparing
            | ReadError::RetainedUnavailable
            | ReadError::RetainedUnverified
            | ReadError::SourceBudget),
        ) => {
            reason = error.to_string();
            None
        }
        Err(error) => return Err(error),
    };
    evidence.push(if let Some(address) = entry {
        json!({"catalog_key":key,"role":"return","address":address,
            "peer":peer.filter(|peer| key != peer.trace).map(|peer| json!({"trace":peer.trace,"witness":peer.witness}))})
    } else {
        json!({"catalog_key":key,"role":"return","reason":reason.chars().take(256).collect::<String>()})
    });
    Ok(LaunchReturn {
        at: completion.at,
        failed: completion.failed,
        resumed: completion.resumed,
        preview: completion.preview.clone(),
    })
}

fn launch_evidence(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    parent: &str,
    peer: selected::PeerBinding<'_>,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<(Value, Option<LaunchReturn>), ReadError> {
    let read = || -> Result<(Value, Option<LaunchReturn>), ReadError> {
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
        let entry = rendered_call(&window, source, parent, &address, block).ok_or_else(|| {
            ReadError::PeerUnavailable("The launch call has no rendered transcript entry.".into())
        })?;
        Ok((entry, synchronous_return(&window, &address, block)))
    };
    let reason = match read() {
        Ok((address, completion)) => {
            return Ok((
                json!({"catalog_key":parent,"address":address,"peer":{"trace":peer.trace,"witness":peer.witness}}),
                completion,
            ));
        }
        Err(ReadError::PeerUnavailable(reason)) => reason,
        Err(ReadError::RetainedPreparing) => "Preparing recorded history.".into(),
        Err(ReadError::SourceBudget) => "The selected launch read budget was exhausted.".into(),
        Err(
            ReadError::RetainedUnavailable | ReadError::RetainedUnverified | ReadError::Restart,
        ) => "The certified launch source could not be verified.".into(),
        Err(error) => return Err(error),
    };
    Ok((
        json!({"catalog_key":parent,"reason":reason.chars().take(256).collect::<String>()}),
        None,
    ))
}
