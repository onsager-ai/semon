//! Retained record reads through the certified warehouse binding.
use super::*;

const COPIES: &str = "SELECT e.source,min(e.pos),m.value FROM fact_relation_observation e
     CROSS JOIN json_each(?1) m ON CAST(m.key AS INTEGER)=e.source
     WHERE e.target=?2 AND e.kind='continued-in' AND e.source<>?3 GROUP BY e.source";

fn claude_storage_parent<'a>(child: &'a Path, native_id: &str) -> Option<&'a Path> {
    if !crate::native_sidecar::agent_path(child)
        || child.file_stem()?.to_str()?.strip_prefix("agent-")? != native_id
    {
        return None;
    }
    child.parent()?.parent()
}

fn certified_parent_relay(
    tx: &rusqlite::Connection,
    source: &Source<'_>,
    identity: &warehouse::RetainedIdentity,
    parents: &[(i64, SessionSourceRef)],
    call: Option<(&Address, u32)>,
    trace: bool,
) -> Result<Option<parent_relay::Plan<SessionSourceRef>>, ReadError> {
    if identity.harness != "claude" {
        return Ok(None);
    }
    let receipt = tx
        .query_row(
            "SELECT receipt FROM warehouse_identity_member WHERE identity=?1 AND source=?2",
            params![identity.binding, identity.owner],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(index)?;
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let receipt: crate::model::summary::CatalogSource =
        serde_json::from_str(&receipt).map_err(index)?;
    let Some(path) = receipt
        .path
        .to_str()
        .and_then(|p| relative(source.options, "claude", p))
    else {
        return Ok(None);
    };
    let Some(storage_parent) = claude_storage_parent(Path::new(path), &identity.native_id) else {
        return Ok(None);
    };
    if !parents.iter().any(|(_, parent)| {
        parent.root == "claude" && storage_parent == Path::new(&parent.path).with_extension("")
    }) {
        return Ok(None);
    }
    if let Some((address, _)) = call {
        if address.path != path || address.offset >= receipt.offset || address.block != 0 {
            return Ok(None);
        }
        let record = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fact_record WHERE source=?1 AND pos=?2
             AND block=0 AND occurrence=?3 AND NOT gap)",
                params![
                    identity.owner,
                    i64::try_from(address.offset).map_err(index)?,
                    address.occurrence
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(index)?;
        if !record {
            return Ok(None);
        }
    }
    let reference = frozen_reference(&identity.harness, path, &receipt);
    parent_relay::Plan::collect_certified(
        tx,
        identity.owner,
        reference,
        receipt.claude_agent_call(),
        call,
        parents,
        trace,
    )
}

fn predecessor_bounds(
    copies: &mut rusqlite::Statement<'_>,
    harness: &str,
    prefixes: &str,
    native_id: &str,
    id: i64,
) -> Result<String, ReadError> {
    let mut bounds = BTreeMap::new();
    if harness == "claude" {
        let mut rows = copies
            .query(params![prefixes, native_id, id])
            .map_err(index)?;
        while let Some(row) = rows.next().map_err(index)? {
            let edge: i64 = row.get(1).map_err(index)?;
            let bound: i64 = row.get(2).map_err(index)?;
            if edge < bound {
                bounds.insert(row.get::<_, i64>(0).map_err(index)?, bound);
            }
        }
    }
    encode(&bounds)
}

#[derive(Clone, Copy)]
pub(in super::super) struct PeerBinding<'a> {
    pub(in super::super) trace: &'a str,
    pub(in super::super) witness: &'a str,
}

struct FrozenMember {
    id: i64,
    revision: i64,
    receipt: crate::model::summary::CatalogSource,
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(in super::super) fn retained_peer_window(
    source: &Source<'_>,
    key: &str,
    peer: PeerBinding<'_>,
    limit: usize,
    cursor: Option<&Cursor>,
    address: Option<(&Address, &str)>,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Component, ReadError> {
    let target = address.map_or(
        Target::Page(limit, Direction::Reverse),
        |(address, binding)| Target::Address(address, Some(binding), None),
    );
    let archive = super::super::ArchiveReader::new(source.options);
    read_frozen_page(
        source,
        key,
        target,
        cursor,
        provider.unwrap_or(&archive),
        Some((peer, provider.is_none())),
    )
}

pub(in super::super) fn retained_peer_trace_window(
    view: &ReadView<'_>,
    source: &Source<'_>,
    key: &str,
    peer: PeerBinding<'_>,
    address: &Address,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Component, ReadError> {
    let incarnation: String = view
        .lock()?
        .query_row(
            "SELECT value FROM meta WHERE key='incarnation'",
            [],
            |row| row.get(0),
        )
        .map_err(index)?;
    let trace = encode(&(
        source.namespace.as_str(),
        CatalogReadScope::RetainedHistory,
        peer.trace,
        incarnation,
    ))?;
    if key == peer.trace {
        let binding = component_binding(&trace, peer.witness)?;
        return retained_trace_send_window(view, source, key, address, &binding, provider);
    }
    let binding = component_binding(&encode(&(trace, key))?, peer.witness)?;
    read_frozen(
        view,
        source,
        key,
        Target::Address(address, Some(&binding), None),
        None,
        provider.unwrap_or(&view.archive),
        Some((peer, provider.is_none())),
    )
}

fn retained_trace_send_window(
    view: &ReadView<'_>,
    source: &Source<'_>,
    key: &str,
    address: &Address,
    binding: &str,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Component, ReadError> {
    read_frozen(
        view,
        source,
        key,
        Target::Address(address, Some(binding), None),
        None,
        provider.unwrap_or(&view.archive),
        None,
    )
}

/// Retained members and their prefix witnesses come from the certified binding.
/// The provider separately authorizes each native source reference on demand.
#[cfg(test)]
pub(in super::super) fn retained_page(
    source: &Source<'_>,
    key: &str,
    limit: usize,
    cursor: Option<&Cursor>,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    read_retained_page(
        source,
        key,
        Target::Page(limit, Direction::Forward),
        cursor,
        provider,
    )
}

#[cfg(test)]
pub(in super::super) fn retained_latest_page(
    source: &Source<'_>,
    key: &str,
    limit: usize,
    cursor: Option<&Cursor>,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    read_retained_page(
        source,
        key,
        Target::Page(limit, Direction::Reverse),
        cursor,
        provider,
    )
}

#[cfg(test)]
pub(in super::super) fn retained_addressed_window(
    source: &Source<'_>,
    key: &str,
    address: &Address,
    binding: &str,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    read_retained_page(
        source,
        key,
        Target::Address(address, Some(binding), None),
        None,
        provider,
    )
}

pub(in super::super) fn retained_annotations(
    source: &Source<'_>,
    key: &str,
    request: AnnotationRequest<'_>,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    read_retained_page(source, key, Target::Annotations(request), None, provider)
}

pub(in super::super) fn retained_turn(
    source: &Source<'_>,
    key: &str,
    turn: &str,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    read_retained_page(source, key, Target::Turns(&[turn], None), None, provider)
}

pub(in super::super) fn retained_preview_turns(
    source: &Source<'_>,
    key: &str,
    turns: &[&str],
    home: HomeFields<'_>,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    read_retained_page(
        source,
        key,
        Target::Turns(turns, Some(home)),
        None,
        provider,
    )
}

pub(in super::super) fn retained_addressed_preview(
    source: &Source<'_>,
    key: &str,
    address: &crate::session_route::NativeEntryAddress,
    generation: &str,
    peer: Option<PeerBinding<'_>>,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Component, ReadError> {
    let archive = super::super::ArchiveReader::new(source.options);
    read_frozen_page(
        source,
        key,
        Target::Address(&address.record, Some(&address.binding), Some(generation)),
        None,
        provider.unwrap_or(&archive),
        peer.map(|peer| (peer, provider.is_none())),
    )
}

pub(in super::super) fn retained_activity_anchor(
    source: &Source<'_>,
    key: &str,
    anchor: &crate::transcript_range::StepAnchor,
    provider: &dyn SessionSourceReader,
) -> Result<Option<Component>, ReadError> {
    match read_retained_page(source, key, Target::Activity(anchor), None, provider) {
        Ok(component) => Ok(Some(component)),
        Err(ReadError::ActivityUnavailable) => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_retained_page(
    source: &Source<'_>,
    key: &str,
    target: Target<'_>,
    cursor: Option<&Cursor>,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    read_frozen_page(source, key, target, cursor, provider, None)
}

fn retained_peer_trace_error(error: warehouse::RetainedIdentityError) -> String {
    warehouse::RetainedPeerError::Trace(error).to_string()
}

fn certified_launch_at(
    tx: &rusqlite::Connection,
    id: i64,
    end: i64,
    peers: &str,
) -> Result<Option<Option<i64>>, ReadError> {
    tx.prepare_cached(include_str!("retained_launch_boundary.sql"))
        .map_err(index)?
        .query_row(params![id, end, peers], |row| row.get(0))
        .optional()
        .map_err(index)
}

fn frozen_members(tx: &rusqlite::Connection, binding: i64) -> Result<Vec<FrozenMember>, ReadError> {
    let mut members = tx.prepare_cached(
        "SELECT source,revision,receipt FROM warehouse_identity_member WHERE identity=?1 ORDER BY ordinal"
    ).map_err(index)?;
    let mut rows = members.query([binding]).map_err(index)?;
    let mut frozen = Vec::new();
    while let Some(row) = rows.next().map_err(index)? {
        frozen.push(FrozenMember {
            id: row.get(0).map_err(index)?,
            revision: row.get(1).map_err(index)?,
            receipt: serde_json::from_str(&row.get::<_, String>(2).map_err(index)?)
                .map_err(index)?,
        });
    }
    Ok(frozen)
}

fn read_frozen_page(
    source: &Source<'_>,
    key: &str,
    target: Target<'_>,
    cursor: Option<&Cursor>,
    provider: &dyn SessionSourceReader,
    peer: Option<(PeerBinding<'_>, bool)>,
) -> Result<Component, ReadError> {
    let view = ReadView::open(source)?;
    read_frozen(&view, source, key, target, cursor, provider, peer)
}

pub(super) fn read_frozen(
    view: &ReadView<'_>,
    source: &Source<'_>,
    key: &str,
    target: Target<'_>,
    cursor: Option<&Cursor>,
    provider: &dyn SessionSourceReader,
    peer: Option<(PeerBinding<'_>, bool)>,
) -> Result<Component, ReadError> {
    let (direction, address) = match target {
        Target::Page(_, direction) => (direction, None),
        Target::Turns(..) | Target::Annotations(_) | Target::Activity(_) => {
            (Direction::Forward, None)
        }
        Target::Address(address, Some(binding), _) => {
            (Direction::Reverse, Some((address, binding)))
        }
        _ => unreachable!("retained transcript target"),
    };
    let limit = target.capacity()?.min(33);
    let include_home_detail = matches!(
        target,
        Target::Turns(_, Some(HomeFields { detail: true, .. }))
    );
    // Retained requests may touch both the previous page and this one. Stay
    // within the existing 64-source provider budget across the two sets.
    let tx = view.lock()?;
    let incarnation: String = tx
        .query_row("SELECT value FROM meta WHERE key='incarnation'", [], |r| {
            r.get(0)
        })
        .map_err(index)?;
    let selection_for = |key| {
        encode(&(
            source.namespace.as_str(),
            CatalogReadScope::RetainedHistory,
            key,
            &incarnation,
        ))
    };
    let trace_key = peer.map_or(key, |(peer, _)| peer.trace);
    let identity = warehouse::retained_identity(&tx, &source.namespace, trace_key).map_err(
        |error| match peer {
            Some(_) => ReadError::PeerUnavailable(retained_peer_trace_error(error)),
            None => super::super::retained_error(error),
        },
    )?;
    let mut legacy_plans = Vec::new();
    let mut carriers = BTreeMap::new();
    let origin_name = serde_json::from_str::<serde_json::Value>(&identity.metadata)
        .map_err(index)?["name"]
        .as_str()
        .map(str::to_owned);
    let observed_peers = identity.observed_peers().map_err(index)?;
    let mut launch_peers = Vec::new();
    let mut launch_parents = std::collections::BTreeSet::new();
    for (participant, observed) in &observed_peers {
        if participant == key {
            continue;
        }
        launch_peers.push((observed.harness.as_str(), observed.native_id.as_str()));
        if observed.harness == "claude" {
            launch_parents.extend(
                observed
                    .members
                    .iter()
                    .map(|m| m.receipt.path.with_extension("")),
            );
        }
    }
    if peer.is_some() {
        launch_peers.push((&identity.harness, &identity.native_id));
        if identity.harness == "claude" {
            launch_parents.extend(
                frozen_members(&tx, identity.binding)?
                    .into_iter()
                    .map(|m| m.receipt.path.with_extension("")),
            );
        }
    }
    let launch_peers = encode(&launch_peers)?;
    let (harness, native_id, witness, selection, metadata, members, live) = if let Some((
        peer,
        use_local,
    )) = peer
    {
        let (observed, current) =
            warehouse::retained_peer(&tx, &source.namespace, &identity, peer.witness, key)
                .map_err(|error| ReadError::PeerUnavailable(error.to_string()))?;
        if observed.harness == "claude" && identity.harness == "claude" {
            let parents = observed
                .members
                .iter()
                .map(|member| {
                    let path = member
                        .receipt
                        .path
                        .to_str()
                        .and_then(|p| relative(source.options, "claude", p))
                        .ok_or(ReadError::RetainedUnverified)?;
                    Ok((
                        member.source,
                        frozen_reference("claude", path, &member.receipt),
                    ))
                })
                .collect::<Result<Vec<_>, ReadError>>()?;
            if let Some(plan) =
                certified_parent_relay(&tx, source, &identity, &parents, None, false)?
            {
                legacy_plans.push((peer.trace.to_owned(), identity.owner, plan));
            }
        }
        let members = observed
            .members
            .into_iter()
            .map(|m| FrozenMember {
                id: m.source,
                revision: m.revision,
                receipt: m.receipt,
            })
            .collect::<Vec<_>>();
        let selection = encode(&(selection_for(peer.trace)?, key))?;
        (
            observed.harness,
            observed.native_id,
            peer.witness.to_owned(),
            selection,
            None,
            members,
            current && use_local,
        )
    } else {
        let frozen = frozen_members(&tx, identity.binding)?;
        if include_home_detail && identity.harness == "claude" {
            let parents = frozen
                .iter()
                .filter_map(|member| {
                    let path = relative(source.options, "claude", member.receipt.path.to_str()?)?;
                    Some((member.id, frozen_reference("claude", path, &member.receipt)))
                })
                .collect::<Vec<_>>();
            for (sender, candidate) in &observed_peers {
                if candidate.harness != "claude" {
                    continue;
                }
                let Some(member) = candidate
                    .members
                    .iter()
                    .find(|member| member.source == candidate.owner)
                else {
                    continue;
                };
                let Some(parent) =
                    claude_storage_parent(&member.receipt.path, &member.receipt.native_id)
                else {
                    continue;
                };
                if !frozen
                    .iter()
                    .any(|member| member.receipt.path.with_extension("") == parent)
                {
                    continue;
                }
                let Ok((_, current)) = warehouse::retained_peer(
                    &tx,
                    &source.namespace,
                    &identity,
                    &identity.witness,
                    sender,
                ) else {
                    continue;
                };
                if legacy_plans.len() == 128 {
                    return Err(ReadError::SourceBudget);
                }
                let carrier = member
                    .receipt
                    .path
                    .to_str()
                    .ok_or(ReadError::RetainedUnverified)?;
                let path = relative(source.options, "claude", carrier)
                    .ok_or(ReadError::RetainedUnverified)?;
                if let Some(plan) = parent_relay::Plan::collect_certified(
                    &tx,
                    member.source,
                    frozen_reference("claude", path, &member.receipt),
                    member.receipt.claude_agent_call(),
                    None,
                    &parents,
                    false,
                )? {
                    if current && !source.provider_only() {
                        let ledger: String = tx
                            .query_row(
                                "SELECT ledger FROM warehouse_source WHERE source=?1",
                                [member.source],
                                |row| row.get(0),
                            )
                            .map_err(index)?;
                        let ledger = decode(&ledger)?;
                        if matches!(source.availability("claude", carrier)?, Availability::Expected(Some(stat))
                                if stable_or_grown(&ledger.stat, &stat))
                        {
                            carriers.insert(path.to_owned(), (carrier.to_owned(), ledger));
                        }
                    }
                    legacy_plans.push((sender.clone(), member.source, plan));
                }
            }
        }
        (
            identity.harness,
            identity.native_id,
            identity.witness,
            selection_for(key)?,
            Some(identity.metadata),
            frozen,
            false,
        )
    };
    if target
        .expected_generation()
        .is_some_and(|expected| expected != witness)
    {
        return Err(ReadError::Restart);
    }
    let binding = component_binding(&selection, &witness)?;
    if address.is_some_and(|(_, expected)| expected != binding) {
        return Err(ReadError::Restart);
    }
    let now = crate::model::now_ms();
    let opened_at = cursor.map_or(now, |cursor| cursor.opened_at);
    if cursor.is_some_and(|cursor| {
        cursor.selection != selection || cursor.members != witness || cursor.direction != direction
    }) || opened_at > now
        || opened_at < now.saturating_sub(warehouse::PREFIX_WINDOW_MS)
    {
        return Err(ReadError::Restart);
    }
    let mut activity = if let Target::Activity(anchor) = target {
        Some(activity_evidence(&tx, key, anchor)?.ok_or(ReadError::ActivityUnavailable)?)
    } else {
        None
    };
    #[cfg(test)]
    let member_count = members.len();
    let prefixes = encode(
        &members
            .iter()
            .map(|m| (m.id, m.receipt.offset))
            .collect::<BTreeMap<_, _>>(),
    )?;
    let mut candidates = BTreeMap::new();
    let mut annotation_members = BTreeMap::new();
    let mut references = Vec::new();
    let mut prior_references = Vec::new();
    let mut turn_members = Vec::new();
    {
        let mut copies = tx.prepare_cached(COPIES).map_err(index)?;
        let mut positions = tx.prepare_cached(direction.statement()).map_err(index)?;
        let mut activity_query = tx
            .prepare_cached(include_str!("activity_evidence.sql"))
            .map_err(index)?;
        let mut ledgers = tx
            .prepare_cached("SELECT incarnation,ledger FROM warehouse_source WHERE source=?1")
            .map_err(index)?;
        for FrozenMember {
            id,
            receipt,
            revision,
        } in members
        {
            let carrier = receipt.path.to_str().ok_or(ReadError::RetainedUnverified)?;
            let path =
                relative(source.options, &harness, carrier).ok_or(ReadError::RetainedUnverified)?;
            if activity
                .as_mut()
                .is_some_and(|activity| !activity.matches_member(receipt.clone()))
            {
                return Err(ReadError::ActivityUnavailable);
            }
            let (incarnation, ledger): (i64, String) = ledgers
                .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(index)?;
            let reference = Arc::new(SessionSourceRef {
                root: harness.clone(),
                path: path.to_owned(),
                native_id: receipt.native_id,
                offset: receipt.offset,
                prefix_sha256: receipt.prefix_sha256,
                tail_sha256: receipt.tail_sha256,
            });
            let state = if live && !source.provider_only() {
                let current = decode(&ledger)?;
                if matches!(source.availability(&harness, carrier)?,
                    Availability::Expected(Some(stat)) if stable_or_grown(&current.stat, &stat))
                {
                    carriers.insert(path.to_owned(), (carrier.to_owned(), current));
                    "cached"
                } else {
                    "unavailable"
                }
            } else {
                "unavailable"
            };
            references.push(crate::CatalogSourceObservation {
                source: (*reference).clone(),
                state: state.into(),
            });
            let prefix = Prefix {
                source: id,
                incarnation,
                offset: receipt.offset,
                digest: receipt.prefix_sha256,
                tail: receipt.tail_sha256,
                provider: None,
            };
            if let Some(prior) =
                cursor.and_then(|cursor| cursor.touched.iter().find(|prior| prior.source == id))
            {
                prior_references.push((prior, prefix.clone(), reference.clone()));
            }
            let predecessors =
                predecessor_bounds(&mut copies, &harness, &prefixes, &reference.native_id, id)?;
            turn_members.push((
                id,
                reference.clone(),
                predecessors.clone(),
                carrier.to_owned(),
            ));
            if let Some(activity) = &activity {
                if let Some((position, kind)) = activity.candidate(
                    &mut activity_query,
                    id,
                    receipt.offset,
                    carrier,
                    &predecessors,
                )? {
                    candidates.insert(position, (kind, reference.clone(), prefix, revision));
                    if candidates.len() > limit {
                        direction.discard_extra(&mut candidates);
                    }
                }
                continue;
            }
            if let Target::Annotations(_) = target {
                annotation_members.insert(id, ((*reference).clone(), revision, predecessors));
                continue;
            }
            if let Target::Turns(turns, _) = target {
                member_turn_positions(
                    &tx,
                    &mut positions,
                    key,
                    id,
                    &reference,
                    &predecessors,
                    turns,
                    |(position, kind)| {
                        candidates.insert(
                            position,
                            (kind, reference.clone(), prefix.clone(), revision),
                        );
                        if candidates.len() > limit {
                            direction.discard_extra(&mut candidates);
                        }
                    },
                )?;
                continue;
            }
            let (start, end, entry, frontier) = if let Some((address, _)) = address {
                if address.path != reference.path || address.offset >= receipt.offset {
                    continue;
                }
                let Some(frontier) = address_frontier(&tx, direction, id, address)? else {
                    continue;
                };
                (
                    address.offset,
                    address.offset + 1,
                    Some((address.block, address.occurrence)),
                    frontier,
                )
            } else {
                (
                    0,
                    receipt.offset,
                    None,
                    direction.frontier(cursor.map(|cursor| cursor.after), id),
                )
            };
            member_positions(
                &mut positions,
                PositionSearch {
                    source: id,
                    start: i64::try_from(start).map_err(index)?,
                    end: i64::try_from(end).map_err(index)?,
                    predecessors: &predecessors,
                    entry,
                    frontier,
                    capacity: limit,
                },
                |(position, kind)| {
                    candidates.insert(
                        position,
                        (kind, reference.clone(), prefix.clone(), revision),
                    );
                    if candidates.len() > limit {
                        direction.discard_extra(&mut candidates);
                    }
                },
            )?;
        }
    }
    if activity.is_some_and(|activity| !activity.sources.is_empty()) {
        return Err(ReadError::ActivityUnavailable);
    }
    let annotations = if let Target::Annotations(request) = target {
        Some(annotation_page(
            &tx,
            source,
            key,
            &binding,
            request,
            &annotation_members,
            PendingRuntime::retained(&harness),
        )?)
    } else {
        None
    };
    let more = matches!(target, Target::Page(..)) && candidates.len() == limit;
    if more {
        direction.discard_extra(&mut candidates);
    }
    let after = direction.continuation(&candidates);
    drop(tx);
    let peer_reader = FrozenPeerReader {
        index_path: source.path,
        db: view.db.clone(),
        carriers,
        provider,
        budget: &view.proof_bytes,
        readers: Mutex::new(BTreeMap::new()),
    };
    let reader = crate::source_reader::BudgetedReader::new(&peer_reader, 1024);
    if cursor.is_some_and(|cursor| cursor.touched.len() != prior_references.len()) {
        return Err(ReadError::Restart);
    }
    for (prior, mut observed, reference) in prior_references {
        observed.provider = Some(pin_provider(&reader, &reference)?);
        if prior != &observed {
            return Err(ReadError::Restart);
        }
    }
    let mut bytes_left = view.body_bytes.lock().map_err(index)?;
    let mut touched = BTreeMap::new();
    let mut reply_records = BTreeMap::new();
    let mut home_origins = BTreeMap::new();
    let mut records = Vec::with_capacity(candidates.len());
    let mut fields = Fields::new();
    let mut read = |id,
                    reference: &SessionSourceRef,
                    offset,
                    images: Option<&crate::attachments::PromptMetadata>,
                    mode| {
        view.read_record(
            &reader,
            id,
            reference,
            offset,
            images,
            mode,
            &mut bytes_left,
            &mut fields,
        )
        .map_err(|error| {
            if *bytes_left == 0 {
                ReadError::SourceBudget
            } else {
                retained_body_error(error, cursor.is_some())
            }
        })
    };
    let mut claims = BTreeMap::new();
    if !candidates.is_empty() || include_home_detail {
        for (sender, child, plan) in legacy_plans {
            match plan.resolve(child, |id, reference, offset| {
                let native = read(id, reference, offset, None, BodyMode::Complete)?;
                pin_provider_with(&reader, reference, |error| {
                    retained_body_error(error, cursor.is_some())
                })?;
                Ok(native)
            }) {
                Ok(resolved) => {
                    for (receipt, turn) in resolved.incoming {
                        claims
                            .entry(receipt)
                            .and_modify(|claim| *claim = None)
                            .or_insert_with(|| Some((turn, sender.clone())));
                    }
                }
                Err(ReadError::RetainedUnavailable | ReadError::RetainedPreparing) => continue,
                Err(error) => return Err(error),
            }
        }
    }
    let mut incoming = BTreeMap::new();
    let mut legacy_senders = BTreeMap::new();
    for (receipt, claim) in claims {
        if let Some((turn, sender)) = claim {
            incoming.insert(receipt, turn.clone());
            legacy_senders
                .entry(turn)
                .and_modify(|sender| *sender = None)
                .or_insert(Some(sender));
        }
    }
    let legacy = encode(&incoming)?;
    let mut turns = if candidates.is_empty() && !include_home_detail {
        BTreeMap::new()
    } else {
        let positions = candidates
            .keys()
            .copied()
            .chain(std::iter::once(Position::END));
        let tx = view.lock()?;
        let mut search = causal::TurnSearch::new(&tx, positions)?;
        for (id, reference, predecessors, carrier) in &turn_members {
            let end = i64::try_from(reference.offset).map_err(index)?;
            let launch_at = if harness == "codex" {
                certified_launch_at(&tx, *id, end, &launch_peers)?
            } else if harness == "claude"
                && claude_storage_parent(Path::new(carrier), &reference.native_id)
                    .is_some_and(|parent| launch_parents.contains(parent))
            {
                Some(tx.query_row(
                    "SELECT min(pos) FROM fact_prompt WHERE source=?1 AND kind='user' AND pos<?2",
                    params![id, end],
                    |row| row.get(0),
                ).map_err(index)?)
            } else {
                None
            };
            search.consider(
                key,
                &harness,
                causal::BoundaryMember {
                    id: *id,
                    end,
                    predecessors,
                    launch_at,
                    native_id: &reference.native_id,
                    path: &reference.path,
                },
                &legacy,
            )?;
        }
        search.finish()
    };
    let latest_turn = turns.remove(&Position::END).flatten();
    let mut home_detail = match latest_turn.as_ref().filter(|_| include_home_detail) {
        Some(turn) => {
            let (id, reference, _, _) = turn_members
                .iter()
                .find(|(_, reference, _, _)| reference.path == turn.address.path)
                .ok_or(ReadError::RetainedUnverified)?;
            let images = prompt_images(view, *id, turn.address.offset, Some("user"))?;
            let native = read(
                *id,
                reference,
                turn.address.offset,
                images.as_ref(),
                BodyMode::Complete,
            )?;
            pin_provider_with(&reader, reference, |error| {
                retained_body_error(error, false)
            })?;
            Some(HomeDetail::from_turn(turn, &native))
        }
        None => None,
    };
    for (position, (kind, reference, prefix, revision)) in candidates {
        let offset = u64::try_from(position.offset).map_err(index)?;
        let images = prompt_images(view, position.source, offset, kind.as_deref())?;
        let native = read(
            position.source,
            &reference,
            offset,
            images.as_ref(),
            BodyMode::transcript(kind.as_deref()),
        )?;
        let event_id = crate::tx::native_event_id(&native);
        let turn = turns.remove(&position).flatten();
        let address = Address {
            path: reference.path.clone(),
            offset,
            block: position.block,
            occurrence: position.occurrence,
        };
        let (prompt_anchor, inputs) = {
            let tx = view.lock()?;
            let prompt_anchor = if let Some(turn) = turn
                .as_ref()
                .filter(|turn| turn.handoff && turn.address == address)
            {
                Some(turn.id.clone())
            } else {
                let mut prompts = tx.prepare_cached(PROMPT_AT).map_err(index)?;
                prompt_anchor(&mut prompts, key, &reference, position)?
            };
            (
                prompt_anchor,
                causal::Inputs::load(&tx, &reference, position, revision)?,
            )
        };
        let operation = inputs.operation();
        let causal = inputs.render(source, key, &reference, event_id.as_deref(), turn);
        if let Some(turn) = home_event_origin(&target, &causal, &reference, offset, &native)
            && let std::collections::btree_map::Entry::Vacant(entry) =
                home_origins.entry(turn.position)
        {
            let id = turn.position.source;
            let (_, reference, _, _) = turn_members
                .iter()
                .find(|(member, ..)| *member == id)
                .ok_or(ReadError::RetainedUnverified)?;
            let images = prompt_images(view, id, turn.address.offset, Some("user"))?;
            let prompt = read(
                id,
                reference,
                turn.address.offset,
                images.as_ref(),
                BodyMode::Complete,
            )?;
            pin_provider_with(&reader, reference, |error| {
                retained_body_error(error, cursor.is_some())
            })?;
            entry.insert(HomeDetail::from_turn(turn, &prompt).text);
        }
        if direction == Direction::Reverse
            && let Some(operation) = operation
            && let std::collections::btree_map::Entry::Vacant(entry) =
                reply_records.entry((reference.path.clone(), operation.parent))
        {
            entry.insert(read(
                position.source,
                &reference,
                operation.parent,
                None,
                BodyMode::Complete,
            )?);
        }
        if direction == Direction::Reverse || matches!(target, Target::Turns(_, Some(_))) {
            for reply_offset in causal
                .calls
                .iter()
                .filter(|call| direction == Direction::Reverse || call.tool == "AskUserQuestion")
                .flat_map(causal::CallSlot::display_offsets)
            {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    reply_records.entry((reference.path.clone(), reply_offset))
                {
                    entry.insert(read(
                        position.source,
                        &reference,
                        reply_offset,
                        None,
                        BodyMode::Transcript,
                    )?);
                }
            }
        }
        touched
            .entry(position.source)
            .or_insert((prefix, reference.clone()));
        records.push(Record {
            source: position.source,
            prompt_anchor,
            entry_anchor: None,
            operation,
            causal,
            relay_receipts: Vec::new(),
            reference: (*reference).clone(),
            kind,
            at: (position.time != i64::MIN).then_some(position.time),
            native,
            images: images.map(|images| images.refs),
            address,
        });
    }
    for (prefix, reference) in touched.values_mut() {
        prefix.provider = Some(pin_provider_with(&reader, reference, |error| {
            retained_body_error(error, cursor.is_some())
        })?);
    }
    peer_reader
        .finish()
        .map_err(|error| retained_body_error(error, cursor.is_some()))?;
    source.validate_authority()?;
    let selection_identity = super::super::present_identity(
        source,
        CatalogReadScope::RetainedHistory,
        key,
        harness.clone(),
        references,
        witness.clone(),
    );
    let next = after
        .filter(|_| more || address.is_some())
        .map(|after| Cursor {
            direction,
            selection,
            members: witness,
            view: 0,
            opened_at,
            after,
            touched: touched.into_values().map(|(prefix, _)| prefix).collect(),
        });
    let metadata: Option<serde_json::Value> = metadata
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(index)?;
    drop(bytes_left);
    if let (Some(detail), Some(turn)) = (&mut home_detail, &latest_turn)
        && turn.id.starts_with('r')
    {
        let binding = PeerBinding {
            trace: trace_key,
            witness: &selection_identity.generation,
        };
        let candidate = if let Some(sender) = legacy_senders.remove(&turn.id) {
            sender
        } else {
            let tx = view.lock()?;
            let original = warehouse::retained_identity(&tx, &source.namespace, trace_key)
                .map_err(super::super::retained_error)?;
            let bounds = encode(
                &observed_peers
                    .iter()
                    .filter(|(name, _)| *name != key)
                    .flat_map(|(_, peer)| {
                        peer.members
                            .iter()
                            .map(|member| (member.source, member.receipt.offset))
                    })
                    .collect::<BTreeMap<_, _>>(),
            )?;
            if let Some((sender, _, _)) = relay_sender(&tx, source, turn, Some(&bounds))? {
                let mut candidates = observed_peers.iter().filter(|(name, observed)| {
                    *name != key
                        && observed
                            .members
                            .iter()
                            .any(|member| member.source == sender)
                });
                match (candidates.next(), candidates.next()) {
                    (Some((name, _)), None)
                        if warehouse::retained_peer(
                            &tx,
                            &source.namespace,
                            &original,
                            binding.witness,
                            name,
                        )
                        .is_ok() =>
                    {
                        Some(name.clone())
                    }
                    _ => None,
                }
            } else {
                None
            }
        };
        if let Some(sender) = candidate {
            let name = if sender == trace_key {
                origin_name
            } else {
                match super::super::trace::passive_summary(view, source, &sender, Some(binding)) {
                    Ok(summary) => {
                        summary.and_then(|summary| summary["name"].as_str().map(str::to_owned))
                    }
                    Err(
                        ReadError::Restart
                        | ReadError::SourceBudget
                        | ReadError::PeerUnavailable(_)
                        | ReadError::RetainedUnverified,
                    ) => None,
                    Err(error) => return Err(error),
                }
            };
            detail.set_sender(sender, name);
        }
    }
    if let Some(detail) = &mut home_detail
        && latest_turn
            .as_ref()
            .is_some_and(|turn| turn.id == crate::model::spawn_turn_id(&harness, key))
        && let Some(row) = &metadata
        && let Some(parent) = row["parent"]
            .as_str()
            .filter(|parent| *parent != "you" && *parent != key)
    {
        detail.set_parent(parent, &row["details"]);
    }
    let title = metadata
        .as_ref()
        .and_then(|m| m["name"].as_str())
        .map(str::to_owned);
    let model = metadata
        .as_ref()
        .and_then(|m| m["model"].as_str())
        .map(str::to_owned);
    Ok(Component {
        binding,
        identity: selection_identity,
        harness,
        native_id,
        title,
        model,
        metadata: None,
        retained_metadata: metadata.filter(|_| cursor.is_none()),
        #[cfg(test)]
        member_count,
        latest_turn,
        runtime_status: None,
        legacy_completion: None,
        trace_completion: None,
        records,
        reply_records,
        fields,
        next,
        annotations,
        home_detail,
        home_origins,
        home_activity: None,
    })
}
struct FrozenPeerReader<'a> {
    index_path: &'a Path,
    db: Arc<chunk_proofs::Snapshot>,
    carriers: BTreeMap<String, (String, Ledger)>,
    provider: &'a dyn SessionSourceReader,
    budget: &'a Mutex<usize>,
    readers: Mutex<BTreeMap<String, PrefixReader<'a>>>,
}

impl FrozenPeerReader<'_> {
    fn finish(&self) -> io::Result<()> {
        for reader in self.readers.lock().map_err(|_| changed())?.values_mut() {
            reader.finish()?;
        }
        Ok(())
    }
}

impl SessionSourceReader for FrozenPeerReader<'_> {
    fn read_range(
        &self,
        reference: &SessionSourceRef,
        generation: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<SessionSourceRange> {
        let Some((carrier, current)) = self.carriers.get(&reference.path) else {
            return self.provider.read_range(reference, generation, offset, max);
        };
        let mut readers = self.readers.lock().map_err(|_| changed())?;
        let reader = match readers.entry(reference.path.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                PrefixReader::open(
                    self.db.clone(),
                    self.index_path,
                    Arc::new(Member {
                        carrier: carrier.clone(),
                        current: current.clone(),
                        reference: reference.clone(),
                        remote: false,
                        changed_local: None,
                    }),
                    self.budget,
                )
                .map_err(|_| changed())?,
            ),
        };
        reader.read_range(reference, generation, offset, max)
    }
}

/// The host provides an independently authorized archive reader. The warehouse
/// binding supplies the original source reference and exact record position.
#[cfg(test)]
pub(in super::super) fn retained_record_at(
    options: &Options,
    source: &Source<'_>,
    key: &str,
    address: &Address,
    provider: &dyn SessionSourceReader,
) -> Result<Option<Record>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    let identity = warehouse::retained_identity(&tx, &source.namespace, key)
        .map_err(super::super::retained_error)?;
    let home = match identity.harness.as_str() {
        "claude" => &options.claude_home,
        "codex" => &options.codex_home,
        "copilot" => &options.copilot_home,
        _ => return Err(ReadError::RetainedUnverified),
    };
    if !crate::is_input_path(&identity.harness, &address.path) {
        return Ok(None);
    }
    let full_path = home.join(&address.path);
    let at = i64::try_from(address.offset).map_err(index)?;
    let Some((id, receipt, kind)) = tx.query_row(
        "SELECT m.source,m.receipt,r.kind FROM warehouse_identity_member m
         CROSS JOIN fact_record r ON r.source=m.source AND r.pos=?3 AND r.block=?4 AND r.occurrence=?5
         WHERE m.identity=?1 AND json_extract(m.receipt,'$.path')=?2
          AND r.pos>=0 AND r.pos<json_extract(m.receipt,'$.offset') AND NOT r.gap",
        params![identity.binding, full_path.to_str(), at, address.block, address.occurrence],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?)),
    ).optional().map_err(index)? else {
        return Ok(None);
    };
    let receipt: crate::model::summary::CatalogSource =
        serde_json::from_str(&receipt).map_err(index)?;
    let reference = certified_reference(identity.harness, receipt, address);
    let record = read_certified_record(id, reference, address, kind, provider)?;
    source.validate_authority()?;
    Ok(record)
}

#[cfg(test)]
pub(in super::super) fn retained_peer_record_at(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    address: &Address,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Option<Record>, ReadError> {
    read_retained_peer(
        source,
        trace,
        witness,
        peer,
        provider,
        PAGE_BYTES,
        |_, _, _, _| Ok(Some(address.clone())),
    )
}

#[cfg(test)]
pub(in super::super) fn retained_peer_relay_address(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    (send, block): (&Address, u32),
    provider: Option<&dyn SessionSourceReader>,
) -> Result<crate::session_route::NativeEntryAddress, ReadError> {
    let view = ReadView::open(source)?;
    retained_relay_in_view(
        &view,
        source,
        PeerBinding { trace, witness },
        trace,
        peer,
        (send, block),
        provider,
    )
    .map(|(address, _)| address)
}

pub(in super::super) fn retained_relay_in_view(
    view: &ReadView<'_>,
    source: &Source<'_>,
    binding: PeerBinding<'_>,
    sender: &str,
    peer: &str,
    (send, block): (&Address, u32),
    provider: Option<&dyn SessionSourceReader>,
) -> Result<(crate::session_route::NativeEntryAddress, String), ReadError> {
    let PeerBinding { trace, witness } = binding;
    let unavailable = || {
        ReadError::PeerUnavailable(
        "No exact relay delivery and rendered receipt could be proven in the certified prefixes.".into(),
    )
    };
    let mut matched = None;
    let record = read_retained_peer_in_view(view, source, PeerBinding { trace, witness }, peer, provider, PAGE_BYTES, |db, (identity, observed), parents, read| {
        let tx = db.lock().map_err(index)?;
        let incarnation: String = tx
            .query_row("SELECT value FROM meta WHERE key='incarnation'", [], |r| {
                r.get(0)
            })
            .map_err(index)?;
        let origin = encode(&(
            source.namespace.as_str(),
            CatalogReadScope::RetainedHistory,
            trace,
            &incarnation,
        ))?;
        let sender_bounds = retained_member_bounds(&tx, source, identity, binding, sender, Some(&send.path))?;
        let bounds = encode(
            &observed
                .members
                .iter()
                .map(|m| (m.source, m.receipt.offset))
                .collect::<BTreeMap<_, _>>(),
        )?;
        let mut query = tx
            .prepare_cached(include_str!("retained_relay.sql"))
            .map_err(index)?;
        let mut rows = query
            .query(params![
                sender_bounds,
                i64::try_from(send.offset).map_err(index)?,
                send.block,
                send.occurrence,
                block,
                bounds
            ])
            .map_err(index)?;
        let correlated = if let Some(row) = rows.next().map_err(index)? {
            let hit = (row.get(0).map_err(index)?, row.get(1).map_err(index)?,
                row.get(2).map_err(index)?, row.get::<_, String>(3).map_err(index)?,
                row.get(4).map_err(index)?);
            if rows.next().map_err(index)?.is_some() {
                return Err(unavailable());
            }
            Some(hit)
        } else {
            None
        };
        drop(rows);
        drop(query);
        let plan = if correlated.is_none() && sender == trace && peer != trace {
            Some(certified_parent_relay(&tx, source, identity, parents, Some((send, block)), false)?
                .ok_or_else(unavailable)?)
        } else {
            None
        };
        drop(tx);
        let mut incoming = BTreeMap::new();
        let (id, offset, occurrence, correlation, time, legacy_turn) = if let Some((id, offset, occurrence, correlation, time)) = correlated {
            (id, offset, occurrence, Some(correlation), time, None)
        } else {
            let matched = plan.ok_or_else(unavailable)?.resolve(identity.owner, |_, reference, offset| read(reference, offset))?;
            let receipt = matched.matched.ok_or_else(unavailable)?;
            let (id, _) = parents.iter().find(|(_, parent)| parent.path == receipt.path).ok_or_else(unavailable)?;
            let offset = i64::try_from(receipt.offset).map_err(index)?;
            let tx = db.lock().map_err(index)?;
            let occurrence = tx.query_row(
                "SELECT occurrence FROM fact_record WHERE source=?1 AND pos=?2 AND block=0 AND NOT gap",
                params![id, offset], |row| row.get(0),
            ).optional().map_err(index)?.ok_or_else(unavailable)?;
            incoming = matched.incoming;
            (*id, offset, occurrence, None, receipt.at.unwrap_or(i64::MIN), matched.matched_legacy_turn)
        };
        let offset_u64 = u64::try_from(offset).map_err(index)?;
        let position = Position {
            source: id,
            time,
            offset,
            block: 0,
            occurrence,
        };
        let tx = db.lock().map_err(index)?;
        let mut search = causal::TurnSearch::new(&tx, std::iter::once(position))?;
        let legacy = encode(&incoming)?;
        let mut copies = tx.prepare_cached(COPIES).map_err(index)?;
        let mut address = None;
        for member in &observed.members {
            let reference = &member.receipt;
            let path = reference
                .path
                .to_str()
                .and_then(|path| relative(source.options, &observed.harness, path))
                .ok_or_else(unavailable)?;
            let predecessors = predecessor_bounds(
                &mut copies,
                &observed.harness,
                &bounds,
                &reference.native_id,
                member.source,
            )?;
            search.consider(
                peer,
                &observed.harness,
                causal::BoundaryMember {
                    id: member.source,
                    end: i64::try_from(reference.offset).map_err(index)?,
                    predecessors: &predecessors,
                    launch_at: None,
                    native_id: &reference.native_id,
                    path,
                },
                &legacy,
            )?;
            if member.source == id {
                address = Some((
                    Address {
                        path: path.into(),
                        offset: offset_u64,
                        block: 0,
                        occurrence,
                    },
                    legacy_turn.clone().unwrap_or_else(|| crate::model::incoming_relay_turn_id(
                            correlation.as_deref(), &reference.native_id, offset_u64,
                        )),
                ));
            }
        }
        let (address, turn) = address.ok_or_else(unavailable)?;
        if !search
            .finish()
            .remove(&position)
            .flatten()
            .is_some_and(|candidate| {
                candidate.handoff
                    && !candidate.head
                    && candidate.address == address
                    && candidate.id == turn
            })
        {
            return Err(unavailable());
        }
        let selection = if peer == trace { origin } else { encode(&(origin, peer))? };
        matched = Some((component_binding(&selection, witness)?, turn));
        Ok(Some(address))
    })?
    .ok_or_else(unavailable)?;
    let entry_id = crate::transcript_range::native_entry_id(
        source.namespace.as_str(),
        peer,
        &record.reference,
        (record.address.offset, 0),
        "h",
        crate::tx::native_event_id(&record.native).as_deref(),
        None,
    );
    let (binding, turn) = matched.ok_or_else(unavailable)?;
    Ok((
        crate::session_route::NativeEntryAddress {
            record: record.address,
            binding,
            block: 0,
            entry_id,
        },
        turn,
    ))
}

/// Locate a native launch call only when the child's verified marker names
/// the certified parent and uniquely identifies a call in its frozen prefix.
#[cfg(test)]
pub(in super::super) fn retained_peer_launch_call(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<(Address, u32, crate::ToolCall), ReadError> {
    let view = ReadView::open(source)?;
    retained_peer_launch_in_view(&view, source, trace, witness, peer, provider)
}

pub(in super::super) fn retained_peer_launch_in_view(
    view: &ReadView<'_>,
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<(Address, u32, crate::ToolCall), ReadError> {
    let unavailable = || {
        ReadError::PeerUnavailable(
            "No exact launch call could be proven in the certified prefixes.".into(),
        )
    };
    if trace == peer {
        return Err(unavailable());
    }
    let mut matched_call = None;
    let record = read_retained_peer_in_view(
        view,
        source,
        PeerBinding { trace, witness },
        peer,
        provider,
        crate::handoff::MAX_INPUT_BYTES,
        |db, (identity, observed), parents, read| {
            let tx = db.lock().map_err(index)?;
            let child_members = frozen_members(&tx, identity.binding)?;
            let child = child_members
                .iter()
                .find(|member| member.id == identity.owner)
                .ok_or_else(unavailable)?;
            let child_carrier = child.receipt.path.to_str().ok_or_else(unavailable)?;
            let child_path = relative(source.options, &identity.harness, child_carrier)
                .ok_or_else(unavailable)?;
            let child_reference = frozen_reference(&identity.harness, child_path, &child.receipt);
            let claude_parent = (identity.harness == "claude" && observed.harness == "claude")
                .then(|| claude_storage_parent(Path::new(child_path), &identity.native_id))
                .flatten()
                .filter(|storage_parent| parents.iter().any(|(_, parent)| {
                    *storage_parent == Path::new(&parent.path).with_extension("")
                }));
            let native_call = claude_parent.and_then(|_| child.receipt.claude_agent_call());
            let marker = if native_call.is_none() {
                let marker_offset: i64 = tx.query_row(
                    "SELECT p.pos FROM fact_parent_observation p
                     JOIN fact_record r ON r.source=p.source AND r.pos=p.pos AND r.block=0 AND NOT r.gap
                     WHERE p.source=?1 AND p.pos<?2 AND p.method='marker'
                     ORDER BY p.pos DESC,p.block DESC,p.occurrence DESC LIMIT 1",
                    params![child.id, i64::try_from(child_reference.offset).map_err(index)?],
                    |row| row.get(0),
                ).optional().map_err(index)?.ok_or_else(unavailable)?;
                drop(tx);
                let native = read(&child_reference, u64::try_from(marker_offset).map_err(index)?)?;
                Some(crate::user_text(&native).and_then(crate::parse_marker)
                    .filter(|marker| marker.harness == observed.harness && marker.parent_id == observed.native_id)
                    .ok_or_else(unavailable)?)
            } else {
                drop(tx);
                None
            };
            let call_id = native_call.or_else(|| marker.as_ref().and_then(|marker| marker.tool_id.as_deref()));
            let prompt = if call_id.is_none() {
                Some(marker.as_ref().and_then(|marker| marker.handoff.as_deref()).ok_or_else(unavailable)?)
            } else {
                None
            };
            let bounds = encode(
                &parents
                    .iter()
                    .map(|(id, reference)| (*id, reference.offset))
                    .collect::<BTreeMap<_, _>>(),
            )?;
            let candidates = certified_launch_calls(db, &bounds, call_id)?;
            if candidates.len() > crate::handoff::MAX_CALLS {
                return Err(unavailable());
            }
            let mut location = None;
            for (id, offset, occurrence, block, call_id, tool) in candidates {
                if native_call.is_some() && !matches!(tool.as_str(), "Agent" | "Task") {
                    continue;
                }
                let (_, reference) = parents
                    .iter()
                    .find(|(candidate, _)| *candidate == id)
                    .ok_or_else(unavailable)?;
                let offset = u64::try_from(offset).map_err(index)?;
                if let Some(path) = prompt {
                    let native = read(reference, offset)?;
                    let Some(input) = crate::model::tool_input(&native, block as usize) else {
                        continue;
                    };
                    if !crate::handoff::matches_segment(&input.to_string(), path) {
                        continue;
                    }
                }
                if location.is_some() {
                    return Err(unavailable());
                }
                matched_call = Some((
                    block,
                    crate::ToolCall {
                        id: call_id,
                        name: tool,
                    },
                ));
                location = Some(Address {
                    path: reference.path.clone(),
                    offset,
                    block: 0,
                    occurrence,
                });
            }
            Ok(location)
        },
    )?
    .ok_or_else(unavailable)?;
    let (block, tool) = matched_call.ok_or_else(unavailable)?;
    Ok((record.address, block, tool))
}

/// Qualify the return against the original child's and observed parent's prefixes.
pub(in super::super) fn retained_spawn_return(
    view: &ReadView<'_>,
    source: &Source<'_>,
    peer: PeerBinding<'_>,
    parent: &str,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Option<TraceCompletion>, ReadError> {
    let mut completion = None;
    read_retained_peer_in_view(
        view,
        source,
        peer,
        parent,
        provider,
        PAGE_BYTES,
        |db, (identity, _), parents, read| {
            let plan = {
                let tx = db.lock().map_err(index)?;
                certified_parent_relay(&tx, source, identity, parents, None, true)?
            };
            if let Some(plan) = plan {
                completion = plan
                    .resolve(identity.owner, |_, reference, offset| {
                        read(reference, offset)
                    })?
                    .trace_completion;
            }
            Ok(None)
        },
    )?;
    Ok(completion)
}

type LaunchCallRow = (i64, i64, i64, u32, String, String);

fn certified_launch_calls(
    db: &chunk_proofs::Snapshot,
    bounds: &str,
    call: Option<&str>,
) -> Result<Vec<LaunchCallRow>, ReadError> {
    let tx = db.lock().map_err(index)?;
    let mut statement = tx
        .prepare_cached(
            "SELECT c.source,c.pos,r.occurrence,c.block,c.call_id,c.tool
         FROM json_each(?1) prefix
         CROSS JOIN fact_tool_call c ON c.source=CAST(prefix.key AS INTEGER) AND c.pos<prefix.value
         CROSS JOIN fact_record r ON r.source=c.source AND r.pos=c.pos AND r.block=0 AND NOT r.gap
         WHERE c.call_id IS NOT NULL AND c.tool IS NOT NULL AND
           ((?2 IS NULL AND c.tool IN ('Bash','Skill')) OR c.call_id=?2)
         ORDER BY c.source,c.pos,c.block,c.occurrence LIMIT ?3",
        )
        .map_err(index)?;
    statement
        .query_map(
            params![bounds, call, crate::handoff::MAX_CALLS as i64 + 1],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(index)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(index)
}

type CertifiedRecordRead<'a> =
    dyn FnMut(&SessionSourceRef, u64) -> Result<serde_json::Value, ReadError> + 'a;

#[cfg(test)]
fn read_retained_peer(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    provider: Option<&dyn SessionSourceReader>,
    bytes: usize,
    select: impl FnOnce(
        &Arc<chunk_proofs::Snapshot>,
        (&warehouse::RetainedIdentity, &warehouse::ObservedPeer),
        &[(i64, SessionSourceRef)],
        &mut CertifiedRecordRead<'_>,
    ) -> Result<Option<Address>, ReadError>,
) -> Result<Option<Record>, ReadError> {
    if peer == trace {
        return Ok(None);
    }
    let view = ReadView::open(source)?;
    read_retained_peer_in_view(
        &view,
        source,
        PeerBinding { trace, witness },
        peer,
        provider,
        bytes,
        select,
    )
}

fn retained_participant(
    tx: &rusqlite::Connection,
    source: &Source<'_>,
    identity: &warehouse::RetainedIdentity,
    binding: PeerBinding<'_>,
    key: &str,
) -> Result<(warehouse::ObservedPeer, bool), ReadError> {
    if key != binding.trace {
        return warehouse::retained_peer(tx, &source.namespace, identity, binding.witness, key)
            .map_err(|error| ReadError::PeerUnavailable(error.to_string()));
    }
    if identity.witness != binding.witness {
        return Err(ReadError::PeerUnavailable(
            "The original Trace witness has changed.".into(),
        ));
    }
    let members = frozen_members(tx, identity.binding)?
        .into_iter()
        .map(|member| warehouse::ObservedMember {
            source: member.id,
            revision: member.revision,
            receipt: member.receipt,
        })
        .collect();
    Ok((
        warehouse::ObservedPeer {
            owner: identity.owner,
            harness: identity.harness.clone(),
            native_id: identity.native_id.clone(),
            members,
        },
        false,
    ))
}

pub(in super::super) fn retained_member_bounds(
    tx: &rusqlite::Connection,
    source: &Source<'_>,
    identity: &warehouse::RetainedIdentity,
    binding: PeerBinding<'_>,
    key: &str,
    path: Option<&str>,
) -> Result<String, ReadError> {
    let (participant, _) = retained_participant(tx, source, identity, binding, key)?;
    encode(
        &participant
            .members
            .iter()
            .filter_map(|member| {
                let carrier = member.receipt.path.to_str()?;
                if path.is_some() && relative(source.options, &participant.harness, carrier) != path
                {
                    return None;
                }
                Some((member.source, (carrier, member.receipt.offset)))
            })
            .collect::<BTreeMap<_, _>>(),
    )
}

fn read_retained_peer_in_view(
    view: &ReadView<'_>,
    source: &Source<'_>,
    PeerBinding { trace, witness }: PeerBinding<'_>,
    peer: &str,
    provider: Option<&dyn SessionSourceReader>,
    bytes: usize,
    select: impl FnOnce(
        &Arc<chunk_proofs::Snapshot>,
        (&warehouse::RetainedIdentity, &warehouse::ObservedPeer),
        &[(i64, SessionSourceRef)],
        &mut CertifiedRecordRead<'_>,
    ) -> Result<Option<Address>, ReadError>,
) -> Result<Option<Record>, ReadError> {
    let tx = view.lock()?;
    let identity = warehouse::retained_identity(&tx, &source.namespace, trace)
        .map_err(|error| ReadError::PeerUnavailable(retained_peer_trace_error(error)))?;
    let (observed, current) =
        retained_participant(&tx, source, &identity, PeerBinding { trace, witness }, peer)?;
    let mut parents = Vec::new();
    let mut carriers = BTreeMap::new();
    for member in &observed.members {
        let carrier = member
            .receipt
            .path
            .to_str()
            .ok_or(ReadError::RetainedUnverified)?;
        let path = relative(source.options, &observed.harness, carrier)
            .ok_or(ReadError::RetainedUnverified)?;
        let reference = frozen_reference(&observed.harness, path, &member.receipt);
        if current
            && provider.is_none()
            && let Availability::Expected(Some(stat)) =
                source.availability(&observed.harness, carrier)?
        {
            let ledger: String = tx
                .query_row(
                    "SELECT ledger FROM warehouse_source WHERE source=?1",
                    [member.source],
                    |row| row.get(0),
                )
                .map_err(index)?;
            let ledger = decode(&ledger)?;
            if stable_or_grown(&ledger.stat, &stat) {
                carriers.insert(path.into(), (carrier.into(), ledger));
            }
        }
        parents.push((member.source, reference));
    }
    drop(tx);
    let peer_reader = FrozenPeerReader {
        index_path: source.path,
        db: view.db.clone(),
        carriers,
        provider: provider.unwrap_or(&view.archive),
        budget: &view.proof_bytes,
        readers: Mutex::new(BTreeMap::new()),
    };
    let reader = crate::source_reader::BudgetedReader::new(&peer_reader, 1024);
    let mut shared_bytes = view.body_bytes.lock().map_err(index)?;
    let mut bytes_left = bytes.min(*shared_bytes);
    let budget_error =
        || ReadError::PeerUnavailable("The selected peer read budget was exhausted.".into());
    let peer_body_error = |error: io::Error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            budget_error()
        } else {
            retained_body_error(error, false)
        }
    };
    let mut read = |reference: &SessionSourceRef, offset| {
        let allowed = bytes_left;
        let result = crate::source_reader::read_record_bounded(
            &reader,
            reference,
            offset,
            crate::tx::LINE_MAX as usize,
            &mut bytes_left,
        );
        *shared_bytes -= allowed - bytes_left;
        let native = result.map_err(|error| {
            if bytes_left == 0 {
                budget_error()
            } else {
                peer_body_error(error)
            }
        })?;
        pin_provider_with(&reader, reference, peer_body_error)?;
        Ok(native)
    };
    let result = (|| {
        let Some(address) = select(&view.db, (&identity, &observed), &parents, &mut read)? else {
            peer_reader.finish().map_err(peer_body_error)?;
            return Ok(None);
        };
        let Some((id, reference)) = parents.into_iter().find(|(_, reference)| reference.path == address.path) else {
            return Ok(None);
        };
        if address.offset >= reference.offset {
            return Ok(None);
        }
        let at = i64::try_from(address.offset).map_err(index)?;
        let kind = view.lock()?.query_row(
            "SELECT kind FROM fact_record WHERE source=?1 AND pos=?2 AND block=?3 AND occurrence=?4 AND NOT gap",
            params![id, at, address.block, address.occurrence],
            |row| row.get::<_, Option<String>>(0),
        ).optional().map_err(index)?;
        let Some(kind) = kind else {
            return Ok(None);
        };
        let native = read(&reference, address.offset)?;
        peer_reader.finish().map_err(peer_body_error)?;
        Ok(Some(Record {
            source: id,
            prompt_anchor: None,
            entry_anchor: None,
            operation: None,
            causal: causal::Context::default(),
            relay_receipts: Vec::new(),
            address,
            reference,
            kind,
            at: crate::events::record_time(&native),
            native,
            images: None,
        }))
    })().map_err(|error| match error {
        ReadError::Restart => ReadError::PeerUnavailable(
            "The source reader could not verify the observed peer prefix.".into(),
        ),
        ReadError::RetainedUnavailable => ReadError::PeerUnavailable(
            "The observed peer prefix is unavailable from its authorized source reader.".into(),
        ),
        error => error,
    })?;
    source.validate_authority()?;
    Ok(result)
}

fn frozen_reference(
    harness: &str,
    path: &str,
    receipt: &crate::model::summary::CatalogSource,
) -> SessionSourceRef {
    SessionSourceRef {
        root: harness.into(),
        path: path.into(),
        native_id: receipt.native_id.clone(),
        offset: receipt.offset,
        prefix_sha256: receipt.prefix_sha256,
        tail_sha256: receipt.tail_sha256,
    }
}

#[cfg(test)]
fn certified_reference(
    harness: String,
    receipt: crate::model::summary::CatalogSource,
    address: &Address,
) -> SessionSourceRef {
    SessionSourceRef {
        root: harness,
        path: address.path.clone(),
        native_id: receipt.native_id,
        offset: receipt.offset,
        prefix_sha256: receipt.prefix_sha256,
        tail_sha256: receipt.tail_sha256,
    }
}

#[cfg(test)]
fn read_certified_record(
    source: i64,
    reference: SessionSourceRef,
    address: &Address,
    kind: Option<String>,
    provider: &dyn SessionSourceReader,
) -> Result<Option<Record>, ReadError> {
    let reader = crate::source_reader::BudgetedReader::new(provider, 1024);
    let mut bytes_left = PAGE_BYTES;
    let native = crate::source_reader::read_record_bounded(
        &reader,
        &reference,
        address.offset,
        crate::tx::LINE_MAX as usize,
        &mut bytes_left,
    )
    .map_err(|error| retained_body_error(error, false))?;
    pin_provider_with(&reader, &reference, |error| {
        retained_body_error(error, false)
    })?;
    Ok(Some(Record {
        source,
        prompt_anchor: None,
        entry_anchor: None,
        operation: None,
        causal: causal::Context::default(),
        relay_receipts: Vec::new(),
        address: address.clone(),
        reference,
        kind,
        at: crate::events::record_time(&native),
        native,
        images: None,
    }))
}

fn retained_body_error(error: io::Error, continuing: bool) -> ReadError {
    if error
        .get_ref()
        .is_some_and(|cause| cause.is::<crate::archive::seekable::Preparing>())
    {
        return ReadError::RetainedPreparing;
    }
    if !continuing
        && matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
        )
    {
        ReadError::RetainedUnavailable
    } else {
        body_error(error)
    }
}
