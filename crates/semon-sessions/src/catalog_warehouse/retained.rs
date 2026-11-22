//! Retained record reads through the certified warehouse binding.
use super::*;

const COPIES: &str = "SELECT e.source,min(e.pos),m.value FROM fact_relation_observation e
     CROSS JOIN json_each(?1) m ON CAST(m.key AS INTEGER)=e.source
     WHERE e.target=?2 AND e.kind='continued-in' AND e.source<>?3 GROUP BY e.source";

fn certified_parent_relay(
    tx: &rusqlite::Connection,
    source: &Source<'_>,
    identity: &warehouse::RetainedIdentity,
    parents: &[(i64, SessionSourceRef)],
    call: Option<(&Address, u32)>,
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
    let child = Path::new(path);
    let storage_parent = child.parent().and_then(Path::parent);
    let native_parent = parents.iter().any(|(_, parent)| {
        parent.root == "claude"
            && storage_parent == Some(Path::new(&parent.path).with_extension("").as_path())
    });
    if !crate::native_sidecar::agent_path(child)
        || !native_parent
        || child
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.strip_prefix("agent-"))
            != Some(identity.native_id.as_str())
    {
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
    parent_relay::Plan::collect_certified(tx, identity.owner, reference, call, parents)
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
        |(address, binding)| Target::Address(address, Some(binding)),
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

/// Retained members and their prefix witnesses come from the certified binding.
/// The provider separately authorizes each native source reference on demand.
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
        Target::Address(address, Some(binding)),
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

fn read_frozen_page(
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
        Target::Address(address, Some(binding)) => (Direction::Reverse, Some((address, binding))),
        _ => unreachable!("retained transcript target"),
    };
    let limit = target.capacity()?.min(33);
    let include_home_detail = matches!(
        target,
        Target::Turns(_, Some(HomeFields { detail: true, .. }))
    );
    // Retained requests may touch both the previous page and this one. Stay
    // within the existing 64-source provider budget across the two sets.
    let db = crate::events::store::focused_connection(source.path).map_err(index)?;
    db.execute_batch("BEGIN DEFERRED").map_err(index)?;
    let db = chunk_proofs::Snapshot::new(db);
    let tx = db.lock().map_err(index)?;
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
    let mut parent_relay = None;
    let origin_owner = identity.owner;
    let mut launch_peers = identity
        .observed_peers()
        .map_err(index)?
        .into_iter()
        .filter(|(participant, _)| participant != key)
        .map(|(_, peer)| (peer.harness, peer.native_id))
        .collect::<Vec<_>>();
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
            parent_relay = certified_parent_relay(&tx, source, &identity, &parents, None)?;
        }
        launch_peers.push((identity.harness, identity.native_id));
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
        let mut members = tx.prepare_cached(
            "SELECT source,revision,receipt FROM warehouse_identity_member WHERE identity=?1 ORDER BY ordinal"
        ).map_err(index)?;
        let mut rows = members.query([identity.binding]).map_err(index)?;
        let mut frozen = Vec::new();
        while let Some(row) = rows.next().map_err(index)? {
            frozen.push(FrozenMember {
                id: row.get(0).map_err(index)?,
                revision: row.get(1).map_err(index)?,
                receipt: serde_json::from_str(&row.get::<_, String>(2).map_err(index)?)
                    .map_err(index)?,
            });
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
    if let Target::Turns(_, Some(home)) = target
        && home.expected != witness
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
    let launch_peers = encode(&launch_peers)?;
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
    let mut carriers = BTreeMap::new();
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
            let state = if live {
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
            turn_members.push((id, reference.clone(), predecessors.clone()));
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
            harness != "copilot",
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
    let local_budget = Mutex::new(PAGE_BYTES);
    let peer_reader = FrozenPeerReader {
        index_path: source.path,
        db: db.clone(),
        carriers,
        provider,
        budget: &local_budget,
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
    let mut bytes_left = PAGE_BYTES;
    let mut touched = BTreeMap::new();
    let mut reply_records = BTreeMap::new();
    let mut records = Vec::with_capacity(candidates.len());
    let mut read = |reference: &SessionSourceRef, offset| {
        crate::source_reader::read_record_bounded(
            &reader,
            reference,
            offset,
            crate::tx::LINE_MAX as usize,
            &mut bytes_left,
        )
        .map_err(|error| retained_body_error(error, cursor.is_some()))
    };
    let legacy = if let Some(plan) = parent_relay.filter(|_| !candidates.is_empty()) {
        match plan.resolve(origin_owner, |_, reference, offset| {
            let native = read(reference, offset)?;
            pin_provider_with(&reader, reference, |error| {
                retained_body_error(error, cursor.is_some())
            })?;
            Ok(native)
        }) {
            Ok(resolved) => encode(&resolved.incoming)?,
            Err(ReadError::RetainedUnavailable) => "{}".into(),
            Err(error) => return Err(error),
        }
    } else {
        "{}".into()
    };
    let mut turns = if candidates.is_empty() && !include_home_detail {
        BTreeMap::new()
    } else {
        let positions = candidates
            .keys()
            .copied()
            .chain(include_home_detail.then_some(Position::END));
        let tx = db.lock().map_err(index)?;
        let mut search = causal::TurnSearch::new(&tx, positions)?;
        for (id, reference, predecessors) in &turn_members {
            search.consider(
                key,
                &harness,
                causal::BoundaryMember {
                    id: *id,
                    end: i64::try_from(reference.offset).map_err(index)?,
                    predecessors,
                    launch_at: if harness == "codex" {
                        certified_launch_at(
                            &tx,
                            *id,
                            i64::try_from(reference.offset).map_err(index)?,
                            &launch_peers,
                        )?
                    } else {
                        None
                    },
                    native_id: &reference.native_id,
                    path: &reference.path,
                },
                &legacy,
            )?;
        }
        search.finish()
    };
    let home_detail = match turns.remove(&Position::END).flatten() {
        Some(turn) => {
            let (_, reference, _) = turn_members
                .iter()
                .find(|(_, reference, _)| reference.path == turn.address.path)
                .ok_or(ReadError::RetainedUnverified)?;
            let native = read(reference, turn.address.offset)?;
            pin_provider_with(&reader, reference, |error| {
                retained_body_error(error, false)
            })?;
            Some(HomeDetail::from_turn(&turn, &native))
        }
        None => None,
    };
    for (position, (kind, reference, prefix, revision)) in candidates {
        let offset = u64::try_from(position.offset).map_err(index)?;
        let native = read(&reference, offset)?;
        let event_id = crate::tx::native_event_id(&native);
        let turn = turns.remove(&position).flatten();
        let address = Address {
            path: reference.path.clone(),
            offset,
            block: position.block,
            occurrence: position.occurrence,
        };
        let (prompt_anchor, inputs) = {
            let tx = db.lock().map_err(index)?;
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
        if direction == Direction::Reverse {
            if let Some(operation) = operation
                && let std::collections::btree_map::Entry::Vacant(entry) =
                    reply_records.entry((reference.path.clone(), operation.parent))
            {
                entry.insert(read(&reference, operation.parent)?);
            }
            for reply in causal
                .calls
                .iter()
                .filter_map(causal::CallSlot::display_reply)
            {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    reply_records.entry((reference.path.clone(), reply.o))
                {
                    entry.insert(read(&reference, reply.o)?);
                }
            }
        }
        touched
            .entry(position.source)
            .or_insert((prefix, reference.clone()));
        records.push(Record {
            prompt_anchor,
            entry_anchor: None,
            operation,
            causal,
            relay_receipts: Vec::new(),
            reference: (*reference).clone(),
            kind,
            at: (position.time != i64::MIN).then_some(position.time),
            native,
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
        member_count,
        legacy_completion: None,
        records,
        reply_records,
        next,
        annotations,
        home_detail,
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
    let Some((receipt, kind)) = tx.query_row(
        "SELECT m.receipt,r.kind FROM warehouse_identity_member m
         CROSS JOIN fact_record r ON r.source=m.source AND r.pos=?3 AND r.block=?4 AND r.occurrence=?5
         WHERE m.identity=?1 AND json_extract(m.receipt,'$.path')=?2
          AND r.pos>=0 AND r.pos<json_extract(m.receipt,'$.offset') AND NOT r.gap",
        params![identity.binding, full_path.to_str(), at, address.block, address.occurrence],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    ).optional().map_err(index)? else {
        return Ok(None);
    };
    let receipt: crate::model::summary::CatalogSource =
        serde_json::from_str(&receipt).map_err(index)?;
    let reference = certified_reference(identity.harness, receipt, address);
    let record = read_certified_record(reference, address, kind, provider)?;
    source.validate_authority()?;
    Ok(record)
}

pub(in super::super) fn retained_peer_record_at(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    address: &Address,
    provider: Option<&dyn SessionSourceReader>,
) -> Result<Option<Record>, ReadError> {
    read_retained_peer(source, trace, witness, peer, provider, |_, _, _, _| {
        Ok(address.clone())
    })
}

pub(in super::super) fn retained_peer_relay_address(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    (send, block): (&Address, u32),
    provider: Option<&dyn SessionSourceReader>,
) -> Result<crate::session_route::NativeEntryAddress, ReadError> {
    let unavailable = || {
        ReadError::PeerUnavailable(
        "No exact relay delivery and rendered receipt could be proven in the certified prefixes.".into(),
    )
    };
    let mut binding = String::new();
    let record = read_retained_peer(source, trace, witness, peer, provider, |db, (identity, observed), parents, read| {
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
        let root = crate::InputRoot::parse(&identity.harness).ok_or_else(unavailable)?;
        if !crate::is_input_path(&identity.harness, &send.path) {
            return Err(unavailable());
        }
        let path = root.home(source.options).join(&send.path);
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
                identity.binding,
                path.to_str(),
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
        let plan = if correlated.is_none() {
            Some(certified_parent_relay(&tx, source, identity, parents, Some((send, block)))?
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
        binding = component_binding(&encode(&(origin, peer))?, witness)?;
        Ok(address)
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
    Ok(crate::session_route::NativeEntryAddress {
        record: record.address,
        binding,
        block: 0,
        entry_id,
    })
}

type CertifiedRecordRead<'a> =
    dyn FnMut(&SessionSourceRef, u64) -> Result<serde_json::Value, ReadError> + 'a;

fn read_retained_peer(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    provider: Option<&dyn SessionSourceReader>,
    select: impl FnOnce(
        &Arc<chunk_proofs::Snapshot>,
        (&warehouse::RetainedIdentity, &warehouse::ObservedPeer),
        &[(i64, SessionSourceRef)],
        &mut CertifiedRecordRead<'_>,
    ) -> Result<Address, ReadError>,
) -> Result<Option<Record>, ReadError> {
    let db = crate::events::store::focused_connection(source.path).map_err(index)?;
    db.execute_batch("BEGIN DEFERRED").map_err(index)?;
    let db = chunk_proofs::Snapshot::new(db);
    let tx = db.lock().map_err(index)?;
    let identity = warehouse::retained_identity(&tx, &source.namespace, trace)
        .map_err(|error| ReadError::PeerUnavailable(retained_peer_trace_error(error)))?;
    let (observed, current) =
        warehouse::retained_peer(&tx, &source.namespace, &identity, witness, peer)
            .map_err(|error| ReadError::PeerUnavailable(error.to_string()))?;
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
    let archive = super::super::ArchiveReader::new(source.options);
    let local_budget = Mutex::new(PAGE_BYTES);
    let peer_reader = FrozenPeerReader {
        index_path: source.path,
        db: db.clone(),
        carriers,
        provider: provider.unwrap_or(&archive),
        budget: &local_budget,
        readers: Mutex::new(BTreeMap::new()),
    };
    let reader = crate::source_reader::BudgetedReader::new(&peer_reader, 1024);
    let mut bytes_left = PAGE_BYTES;
    let mut read = |reference: &SessionSourceRef, offset| {
        let native = crate::source_reader::read_record_bounded(
            &reader,
            reference,
            offset,
            crate::tx::LINE_MAX as usize,
            &mut bytes_left,
        )
        .map_err(|error| retained_body_error(error, false))?;
        pin_provider_with(&reader, reference, |error| {
            retained_body_error(error, false)
        })?;
        Ok(native)
    };
    let result = (|| {
        let address = select(&db, (&identity, &observed), &parents, &mut read)?;
        let Some((id, reference)) = parents.into_iter().find(|(_, reference)| reference.path == address.path) else {
            return Ok(None);
        };
        if address.offset >= reference.offset {
            return Ok(None);
        }
        let at = i64::try_from(address.offset).map_err(index)?;
        let kind = db.lock().map_err(index)?.query_row(
            "SELECT kind FROM fact_record WHERE source=?1 AND pos=?2 AND block=?3 AND occurrence=?4 AND NOT gap",
            params![id, at, address.block, address.occurrence],
            |row| row.get::<_, Option<String>>(0),
        ).optional().map_err(index)?;
        let Some(kind) = kind else {
            return Ok(None);
        };
        let native = read(&reference, address.offset)?;
        peer_reader.finish().map_err(|error| retained_body_error(error, false))?;
        Ok(Some(Record {
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

fn read_certified_record(
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
