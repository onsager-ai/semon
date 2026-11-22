//! Retained record reads through the certified warehouse binding.
use super::*;

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

fn read_retained_page(
    source: &Source<'_>,
    key: &str,
    target: Target<'_>,
    cursor: Option<&Cursor>,
    provider: &dyn SessionSourceReader,
) -> Result<Component, ReadError> {
    let (direction, address) = match target {
        Target::Page(_, direction) => (direction, None),
        Target::Address(address, Some(binding)) => (Direction::Reverse, Some((address, binding))),
        _ => unreachable!("retained transcript target"),
    };
    let limit = target.capacity()?.min(33);
    // Retained requests may touch both the previous page and this one. Stay
    // within the existing 64-source provider budget across the two sets.
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    let identity = warehouse::retained_identity(&tx, &source.namespace, key)
        .map_err(super::super::retained_error)?;
    let incarnation: String = tx
        .query_row("SELECT value FROM meta WHERE key='incarnation'", [], |r| {
            r.get(0)
        })
        .map_err(index)?;
    let selection = encode(&(
        source.namespace.as_str(),
        CatalogReadScope::RetainedHistory,
        key,
        incarnation,
    ))?;
    let binding = component_binding(&selection, &identity.witness)?;
    if address.is_some_and(|(_, expected)| expected != binding) {
        return Err(ReadError::Restart);
    }
    let now = crate::model::now_ms();
    let opened_at = cursor.map_or(now, |cursor| cursor.opened_at);
    if cursor.is_some_and(|cursor| {
        cursor.selection != selection
            || cursor.members != identity.witness
            || cursor.direction != direction
    }) || opened_at > now
        || opened_at < now.saturating_sub(warehouse::PREFIX_WINDOW_MS)
    {
        return Err(ReadError::Restart);
    }
    let reader = crate::source_reader::BudgetedReader::new(provider, 1024);
    let mut confirmed = 0;
    let mut candidates = BTreeMap::new();
    let mut member_count = 0;
    let mut references = Vec::new();
    let mut ordinal = -1;
    {
        let mut members = tx
            .prepare_cached(
                "SELECT m.ordinal,m.source,s.incarnation,m.receipt FROM warehouse_identity_member m
             CROSS JOIN warehouse_source s ON s.source=m.source
             WHERE m.identity=?1 AND m.ordinal>?2 ORDER BY m.ordinal LIMIT ?3",
            )
            .map_err(index)?;
        let mut copies = tx.prepare_cached(
            "SELECT e.source,min(e.pos),json_extract(m.receipt,'$.offset') FROM fact_relation_observation e
             CROSS JOIN warehouse_identity_member m ON m.identity=?1 AND m.source=e.source
             WHERE e.target=?2 AND e.kind='continued-in' AND e.source<>?3 GROUP BY e.source",
        ).map_err(index)?;
        let mut positions = tx.prepare_cached(direction.statement()).map_err(index)?;
        loop {
            let mut batch = members
                .query(params![identity.binding, ordinal, MEMBER_PAGE])
                .map_err(index)?;
            let mut count = 0;
            while let Some(row) = batch.next().map_err(index)? {
                count += 1;
                member_count += 1;
                ordinal = row.get(0).map_err(index)?;
                let id: i64 = row.get(1).map_err(index)?;
                let receipt: crate::model::summary::CatalogSource =
                    serde_json::from_str(&row.get::<_, String>(3).map_err(index)?)
                        .map_err(index)?;
                let path = receipt
                    .path
                    .to_str()
                    .and_then(|path| relative(source.options, &identity.harness, path))
                    .ok_or(ReadError::RetainedUnverified)?;
                let reference = Arc::new(SessionSourceRef {
                    root: identity.harness.clone(),
                    path: path.into(),
                    native_id: receipt.native_id,
                    offset: receipt.offset,
                    prefix_sha256: receipt.prefix_sha256,
                    tail_sha256: receipt.tail_sha256,
                });
                references.push(crate::CatalogSourceObservation {
                    source: (*reference).clone(),
                    state: "unavailable".into(),
                });
                let prefix = Prefix {
                    source: id,
                    incarnation: row.get(2).map_err(index)?,
                    offset: receipt.offset,
                    digest: receipt.prefix_sha256,
                    tail: receipt.tail_sha256,
                    provider: None,
                };
                if let Some(prior) =
                    cursor.and_then(|cursor| cursor.touched.iter().find(|prior| prior.source == id))
                {
                    let mut observed = prefix.clone();
                    observed.provider = Some(pin_provider(&reader, &reference)?);
                    if prior != &observed {
                        return Err(ReadError::Restart);
                    }
                    confirmed += 1;
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
                let mut predecessor_bounds = BTreeMap::new();
                if identity.harness == "claude" {
                    let mut predecessors = copies
                        .query(params![identity.binding, reference.native_id, id])
                        .map_err(index)?;
                    while let Some(row) = predecessors.next().map_err(index)? {
                        let edge: i64 = row.get(1).map_err(index)?;
                        let bound: i64 = row.get(2).map_err(index)?;
                        if edge < bound {
                            predecessor_bounds.insert(row.get::<_, i64>(0).map_err(index)?, bound);
                        }
                    }
                }
                member_positions(
                    &mut positions,
                    PositionSearch {
                        source: id,
                        start: i64::try_from(start).map_err(index)?,
                        end: i64::try_from(end).map_err(index)?,
                        predecessors: &encode(&predecessor_bounds)?,
                        entry,
                        frontier,
                        capacity: limit,
                    },
                    |(position, kind)| {
                        candidates.insert(position, (kind, reference.clone(), prefix.clone()));
                        if candidates.len() > limit {
                            direction.discard_extra(&mut candidates);
                        }
                    },
                )?;
            }
            if count < MEMBER_PAGE {
                break;
            }
        }
    }
    if cursor.is_some_and(|cursor| cursor.touched.len() != confirmed) {
        return Err(ReadError::Restart);
    }
    let more = address.is_none() && candidates.len() == limit;
    if more {
        direction.discard_extra(&mut candidates);
    }
    let after = direction.continuation(&candidates);
    let mut bytes_left = PAGE_BYTES;
    let mut touched = BTreeMap::new();
    let mut records = Vec::with_capacity(candidates.len());
    let mut prompts = tx.prepare_cached(PROMPT_AT).map_err(index)?;
    for (position, (kind, reference, mut prefix)) in candidates {
        let offset = u64::try_from(position.offset).map_err(index)?;
        let native = crate::source_reader::read_record_bounded(
            &reader,
            &reference,
            offset,
            crate::tx::LINE_MAX as usize,
            &mut bytes_left,
        )
        .map_err(|_| ReadError::Restart)?;
        if let std::collections::btree_map::Entry::Vacant(entry) = touched.entry(position.source) {
            prefix.provider = Some(pin_provider(&reader, &reference)?);
            entry.insert(prefix);
        }
        records.push(Record {
            prompt_anchor: prompt_anchor(&mut prompts, key, &reference, position)?,
            entry_anchor: None,
            operation_parent: None,
            causal: causal::Context::default(),
            relay_receipts: Vec::new(),
            reference: (*reference).clone(),
            kind,
            at: (position.time != i64::MIN).then_some(position.time),
            native,
            address: Address {
                path: reference.path.clone(),
                offset,
                block: position.block,
                occurrence: position.occurrence,
            },
        });
    }
    source.validate_authority()?;
    let selection_identity = super::super::present_identity(
        source,
        CatalogReadScope::RetainedHistory,
        key,
        identity.harness.clone(),
        references,
        identity.witness.clone(),
    );
    let next = after
        .filter(|_| more || address.is_some())
        .map(|after| Cursor {
            direction,
            selection,
            members: identity.witness,
            view: 0,
            opened_at,
            after,
            touched: touched.into_values().collect(),
        });
    Ok(Component {
        binding,
        identity: selection_identity,
        harness: identity.harness,
        native_id: identity.native_id,
        title: None,
        model: None,
        member_count,
        legacy_completion: None,
        records,
        reply_records: BTreeMap::new(),
        next,
    })
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
    read_certified_record(identity.harness, receipt, address, kind, provider)
}

pub(in super::super) fn retained_peer_record_at(
    source: &Source<'_>,
    trace: &str,
    witness: &str,
    peer: &str,
    address: &Address,
    provider: &dyn SessionSourceReader,
) -> Result<Option<Record>, ReadError> {
    let mut db = crate::events::store::focused_connection(source.path).map_err(index)?;
    let tx = db.transaction().map_err(index)?;
    let observed = warehouse::retained_peer(&tx, &source.namespace, trace, witness, peer)
        .map_err(|error| ReadError::PeerUnavailable(error.to_string()))?;
    let Some(member) = observed.members.into_iter().find(|member| {
        member
            .receipt
            .path
            .to_str()
            .and_then(|path| relative(source.options, &observed.harness, path))
            == Some(address.path.as_str())
    }) else {
        return Ok(None);
    };
    if address.offset >= member.receipt.offset {
        return Ok(None);
    }
    let at = i64::try_from(address.offset).map_err(index)?;
    let kind = tx.query_row(
        "SELECT kind FROM fact_record WHERE source=?1 AND pos=?2 AND block=?3 AND occurrence=?4 AND NOT gap",
        params![member.source, at, address.block, address.occurrence],
        |row| row.get::<_, Option<String>>(0),
    ).optional().map_err(index)?;
    let Some(kind) = kind else {
        return Ok(None);
    };
    read_certified_record(observed.harness, member.receipt, address, kind, provider).map_err(
        |error| match error {
            ReadError::Restart => ReadError::PeerUnavailable(
                "The source provider could not verify the observed peer prefix.".into(),
            ),
            error => error,
        },
    )
}

fn read_certified_record(
    harness: String,
    receipt: crate::model::summary::CatalogSource,
    address: &Address,
    kind: Option<String>,
    provider: &dyn SessionSourceReader,
) -> Result<Option<Record>, ReadError> {
    let reference = SessionSourceRef {
        root: harness,
        path: address.path.clone(),
        native_id: receipt.native_id,
        offset: receipt.offset,
        prefix_sha256: receipt.prefix_sha256,
        tail_sha256: receipt.tail_sha256,
    };
    let reader = crate::source_reader::BudgetedReader::new(provider, 1024);
    let mut bytes_left = PAGE_BYTES;
    let native = crate::source_reader::read_record_bounded(
        &reader,
        &reference,
        address.offset,
        crate::tx::LINE_MAX as usize,
        &mut bytes_left,
    )
    .map_err(|_| ReadError::Restart)?;
    Ok(Some(Record {
        prompt_anchor: None,
        entry_anchor: None,
        operation_parent: None,
        causal: causal::Context::default(),
        relay_receipts: Vec::new(),
        address: address.clone(),
        reference,
        kind,
        at: crate::events::record_time(&native),
        native,
    }))
}
