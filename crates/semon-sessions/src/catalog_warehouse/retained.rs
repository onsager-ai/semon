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
    let limit = Target::Page(limit).capacity()?.min(33);
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
    let now = crate::model::now_ms();
    let opened_at = cursor.map_or(now, |cursor| cursor.opened_at);
    if cursor
        .is_some_and(|cursor| cursor.selection != selection || cursor.members != identity.witness)
        || opened_at > now
        || opened_at < now.saturating_sub(warehouse::PREFIX_WINDOW_MS)
    {
        return Err(ReadError::Restart);
    }
    let reader = crate::source_reader::BudgetedReader::new(provider, 1024);
    let mut confirmed = 0;
    let mut candidates = BTreeMap::new();
    let mut member_count = 0;
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
        let mut positions = tx.prepare_cached(warehouse::DETAIL).map_err(index)?;
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
                let prefix = Prefix {
                    source: id,
                    incarnation: row.get(2).map_err(index)?,
                    offset: receipt.offset,
                    digest: receipt.prefix_sha256,
                    tail: receipt.tail_sha256,
                };
                if let Some(prior) =
                    cursor.and_then(|cursor| cursor.touched.iter().find(|prior| prior.source == id))
                {
                    if prior != &prefix
                        || (reference.offset > 0
                            && reader
                                .read_range(&reference, None, 0, 1)
                                .map_or(true, |part| {
                                    part.bytes.len() != 1 || part.length < reference.offset
                                }))
                    {
                        return Err(ReadError::Restart);
                    }
                    confirmed += 1;
                }
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
                        start: 0,
                        end: i64::try_from(receipt.offset).map_err(index)?,
                        predecessors: &encode(&predecessor_bounds)?,
                        entry: None,
                        frontier: member_frontier(cursor.map(|cursor| cursor.after), id),
                        capacity: limit,
                    },
                    |(position, kind)| {
                        candidates.insert(position, (kind, reference.clone(), prefix.clone()));
                        if candidates.len() > limit {
                            candidates.pop_last();
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
    let more = candidates.len() == limit;
    if more {
        candidates.pop_last();
    }
    let after = candidates.last_key_value().map(|(position, _)| *position);
    let mut bytes_left = PAGE_BYTES;
    let mut touched = BTreeMap::new();
    let mut records = Vec::with_capacity(candidates.len());
    let mut prompts = tx.prepare_cached(PROMPT_AT).map_err(index)?;
    for (position, (kind, reference, prefix)) in candidates {
        let offset = u64::try_from(position.offset).map_err(index)?;
        let native = crate::source_reader::read_record_bounded(
            &reader,
            &reference,
            offset,
            crate::tx::LINE_MAX as usize,
            &mut bytes_left,
        )
        .map_err(|_| ReadError::Restart)?;
        touched.entry(position.source).or_insert(prefix);
        records.push(Record {
            prompt_anchor: prompt_anchor(&mut prompts, key, &reference, position)?,
            entry_anchor: None,
            causal: causal::Context::default(),
            relay_receipts: Vec::new(),
            kind,
            native,
            address: Address {
                path: reference.path.clone(),
                offset,
                block: position.block,
                occurrence: position.occurrence,
            },
        });
    }
    let next = after.filter(|_| more).map(|after| Cursor {
        selection,
        members: identity.witness,
        view: 0,
        opened_at,
        after,
        touched: touched.into_values().collect(),
    });
    Ok(Component {
        harness: identity.harness,
        native_id: identity.native_id,
        title: None,
        model: None,
        member_count,
        records,
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
    let reference = SessionSourceRef {
        root: identity.harness,
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
        causal: causal::Context::default(),
        relay_receipts: Vec::new(),
        address: address.clone(),
        kind,
        native,
    }))
}
