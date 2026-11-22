//! Prefix-qualified native causal inputs for the selected-record adapter.
use super::{
    Address, Availability, COMMITTED_PREFIX, CONTINUATIONS, Direction, MEMBER_PAGE, MEMBERS,
    Position, ReadError, SessionSourceRef, Source, TOOL_RESULTS, committed_prefix,
    continuation_bounds, decode, index, position, raw_stat, relative, stable_or_grown,
};
use rusqlite::{Connection, OptionalExtension, named_params, params};
use std::{collections::BTreeMap, path::Path};

const BOUNDARY: &str = include_str!("causal_boundary.sql");
const PRECEDING_TIME: &str = "SELECT ts FROM fact_record WHERE source=?1 AND pos<?2
    AND ts IS NOT NULL AND NOT gap ORDER BY pos DESC LIMIT 1";
const SIGNALS_AT: &str = "SELECT payload FROM warehouse_signal WHERE source=?1 AND pos=?2
    AND revision<=?3 AND (retired_revision IS NULL OR retired_revision>?3) ORDER BY seq LIMIT 129";
const CALLS_AT: &str = "SELECT c.block,c.tool,c.call_id,EXISTS (
        SELECT 1 FROM fact_operation o INDEXED BY operation_parent
        WHERE o.source=c.source AND o.call_pos=c.pos AND o.call_block=c.block AND o.pos<?3
    ) FROM fact_tool_call c WHERE c.source=?1 AND c.pos=?2
    ORDER BY c.block,c.occurrence LIMIT 129";
const OPERATION_AT: &str = "SELECT call_pos,ok FROM fact_operation
    WHERE source=?1 AND pos=?2 AND block=?3 ORDER BY occurrence LIMIT 1";

#[derive(Debug, Default)]
pub(in super::super) struct Context {
    pub(in super::super) turn: Option<TurnStart>,
    pub(in super::super) signals: Vec<SignalSlot>,
    pub(in super::super) calls: Vec<CallSlot>,
}

#[derive(Debug, PartialEq, Eq)]
pub(in super::super) struct TurnStart {
    pub(in super::super) id: String,
    pub(in super::super) address: Address,
    pub(in super::super) handoff: bool,
    pub(super) head: bool,
}

struct Boundary {
    position: Position,
    start: Option<TurnStart>,
}

/// Resolve the same component boundary for a page and for a direct evidence
/// read. All member receipts are from the selected view. Keep just one winning
/// boundary per requested position while walking members in keyset batches.
#[allow(clippy::too_many_arguments)]
pub(super) fn turns(
    db: &Connection,
    source: &Source<'_>,
    key: &str,
    harness: &str,
    component: i64,
    view: i64,
    targets: impl Iterator<Item = Position>,
    legacy: &str,
) -> Result<BTreeMap<Position, Option<TurnStart>>, ReadError> {
    let mut search = TurnSearch::new(db, targets)?;
    if search.winners.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut members = db.prepare_cached(MEMBERS).map_err(index)?;
    let mut prefix = db.prepare_cached(COMMITTED_PREFIX).map_err(index)?;
    let mut continuations = db.prepare_cached(CONTINUATIONS).map_err(index)?;
    let mut launch = db
        .prepare_cached(include_str!("launch_boundary.sql"))
        .map_err(index)?;
    let mut graph = super::super::parents::Parents::new(db)?;
    let mut after = 0;
    loop {
        let mut rows = members
            .query(params![
                component,
                source.namespace.binding(),
                harness,
                after,
                MEMBER_PAGE
            ])
            .map_err(index)?;
        let mut count = 0;
        while let Some(row) = rows.next().map_err(index)? {
            count += 1;
            let id = row.get(0).map_err(index)?;
            after = id;
            let carrier: String = row.get(2).map_err(index)?;
            let native_id: String = row.get(4).map_err(index)?;
            let end =
                i64::try_from(committed_prefix(&mut prefix, id, view)?.offset).map_err(index)?;
            let predecessors =
                continuation_bounds(&mut continuations, &mut prefix, id, component, view)?;
            let claude_child = harness == "claude" && !row.get::<_, bool>(8).map_err(index)?;
            let native_parent = if claude_child {
                graph
                    .parent(id)?
                    .filter(|edge| edge.method == "native")
                    .map(|edge| edge.source)
            } else {
                None
            };
            let launch_at = if harness == "codex" || native_parent.is_some() {
                launch
                    .query_row(
                        params![id, end, source.namespace.binding(), native_parent],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<i64>>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, String>(4)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(index)?
            } else {
                None
            };
            let launch_at = if let Some((carrier, ledger, at, parent, parent_harness)) = launch_at {
                let current = decode(&ledger)?;
                if matches!(source.availability(&parent_harness, &carrier)?, Availability::Expected(Some(stat))
                    if stable_or_grown(&current.stat, &stat) && raw_stat(Path::new(&carrier))
                        .is_ok_and(|actual| stable_or_grown(&stat, &actual)))
                    && prefix
                        .query_row(params![parent, view], |row| row.get::<_, String>(0))
                        .optional()
                        .map_err(index)?
                        .is_some()
                {
                    Some(at)
                } else {
                    None
                }
            } else {
                None
            };
            let path = relative(source.options, harness, &carrier).ok_or(ReadError::Restart)?;
            search.consider(
                key,
                harness,
                BoundaryMember {
                    id,
                    end,
                    predecessors: &predecessors,
                    launch_at,
                    native_id: &native_id,
                    path,
                },
                legacy,
            )?;
        }
        if count < MEMBER_PAGE {
            break;
        }
    }
    Ok(search.finish())
}

pub(super) struct BoundaryMember<'a> {
    pub(super) id: i64,
    pub(super) end: i64,
    pub(super) predecessors: &'a str,
    pub(super) launch_at: Option<Option<i64>>,
    pub(super) native_id: &'a str,
    pub(super) path: &'a str,
}

pub(super) struct TurnSearch<'a> {
    boundary: rusqlite::CachedStatement<'a>,
    winners: BTreeMap<Position, (Position, Option<Boundary>)>,
}

impl<'a> TurnSearch<'a> {
    pub(super) fn new(
        db: &'a Connection,
        targets: impl Iterator<Item = Position>,
    ) -> Result<Self, ReadError> {
        let mut previous = db.prepare_cached(PRECEDING_TIME).map_err(index)?;
        let mut winners: BTreeMap<_, (_, Option<Boundary>)> = BTreeMap::new();
        for target in targets {
            let mut ordered = target;
            if ordered.time == i64::MIN {
                ordered.time = previous
                    .query_row(params![target.source, target.offset], |row| row.get(0))
                    .optional()
                    .map_err(index)?
                    .unwrap_or(i64::MIN);
            }
            winners.insert(target, (ordered, None));
        }
        Ok(Self {
            boundary: db.prepare_cached(BOUNDARY).map_err(index)?,
            winners,
        })
    }

    pub(super) fn consider(
        &mut self,
        key: &str,
        harness: &str,
        member: BoundaryMember<'_>,
        legacy: &str,
    ) -> Result<(), ReadError> {
        let BoundaryMember {
            id,
            end,
            predecessors,
            launch_at,
            native_id,
            path,
        } = member;
        for (at, winner) in self.winners.values_mut() {
            let (time, pos, _, _) = Direction::Forward.frontier(Some(*at), id);
            let matched = self.boundary
                .query_row(
                    named_params! {
                        ":source": id, ":end": end, ":predecessors": predecessors,
                        ":time": time, ":pos": pos, ":head": launch_at == Some(None), ":progress": crate::events::AGM_PROGRESS, ":legacy": legacy,
                    },
                    |row| {
                        Ok((
                            Position {
                                time: row.get(0)?,
                                source: id,
                                offset: row.get(1)?,
                                block: row.get(2)?,
                                occurrence: row.get(3)?,
                            },
                            row.get::<_, Option<String>>(4)?,
                            row.get(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                        ))
                    },
                )
                .optional()
                .map_err(index)?;
            let Some((position, kind, occurrence, correlation, legacy_id)) = matched else {
                continue;
            };
            if winner
                .as_ref()
                .is_some_and(|current| current.position >= position)
            {
                continue;
            }
            let start = if let Some(kind) = kind {
                let offset = u64::try_from(position.offset).map_err(index)?;
                let launch = kind == "launch"
                    || (kind == "user" && launch_at == Some(Some(position.offset)));
                let relay = kind == "relay";
                let id = if launch {
                    crate::model::spawn_turn_id(harness, key)
                } else if let Some(legacy_id) = legacy_id {
                    legacy_id
                } else if relay {
                    crate::model::incoming_relay_turn_id(correlation.as_deref(), native_id, offset)
                } else {
                    prompt_id(key, native_id, offset, &kind)
                };
                Some(TurnStart {
                    id,
                    handoff: launch || relay,
                    head: kind == "launch",
                    address: Address {
                        path: path.to_owned(),
                        offset,
                        block: 0,
                        occurrence,
                    },
                })
            } else {
                None
            };
            *winner = Some(Boundary { position, start });
        }
        Ok(())
    }

    pub(super) fn finish(self) -> BTreeMap<Position, Option<TurnStart>> {
        self.winners
            .into_iter()
            .map(|(target, (_, winner))| (target, winner.and_then(|boundary| boundary.start)))
            .collect()
    }
}

pub(super) fn prompt_id(key: &str, native_id: &str, offset: u64, kind: &str) -> String {
    if kind == "ask" {
        crate::model::ask_id(native_id, offset)
    } else {
        crate::model::native_turn_id(key, native_id, offset)
    }
}

#[derive(Debug)]
pub(in super::super) struct SignalSlot {
    pub(in super::super) id: String,
    pub(in super::super) signal: crate::events::Signal,
}

#[derive(Debug)]
pub(in super::super) struct CallSlot {
    pub(in super::super) id: String,
    pub(in super::super) tool: String,
    pub(in super::super) block: u32,
    pub(in super::super) replies: Vec<crate::events::Reply>,
}

impl CallSlot {
    pub(super) fn display_reply(&self) -> Option<&crate::events::Reply> {
        self.replies
            .iter()
            .rev()
            .find(|reply| reply.f & crate::events::ACK == 0)
    }
}

#[derive(Clone, Copy, Debug)]
pub(in super::super) struct Operation {
    pub(in super::super) parent: u64,
    pub(super) ok: Option<bool>,
}

pub(super) struct Inputs {
    signals: Vec<crate::events::Signal>,
    calls: Vec<CallSlot>,
    operation: Option<Operation>,
}

impl Inputs {
    pub(super) fn operation(&self) -> Option<Operation> {
        self.operation
    }

    pub(super) fn load(
        db: &Connection,
        reference: &SessionSourceRef,
        at: Position,
        revision: i64,
    ) -> Result<Self, ReadError> {
        let mut signal_query = db.prepare_cached(SIGNALS_AT).map_err(index)?;
        let mut signal_rows = signal_query
            .query(params![at.source, at.offset, revision])
            .map_err(index)?;
        let mut signals = Vec::new();
        while let Some(row) = signal_rows.next().map_err(index)? {
            let text: String = row.get(0).map_err(index)?;
            signals.push(serde_json::from_str(&text).map_err(index)?);
        }
        if signals.len() > 128 {
            return Err(index("Selected signal budget exhausted"));
        }
        let mut calls_query = db.prepare_cached(CALLS_AT).map_err(index)?;
        let end = i64::try_from(reference.offset).map_err(index)?;
        let mut call_rows = calls_query
            .query(params![at.source, at.offset, end])
            .map_err(index)?;
        let mut result_query = db.prepare_cached(TOOL_RESULTS).map_err(index)?;
        let mut calls = Vec::new();
        let mut examined = 0;
        while let Some(row) = call_rows.next().map_err(index)? {
            if examined == 128 {
                return Err(index("Selected native call budget exhausted"));
            }
            examined += 1;
            let block: u32 = row.get(0).map_err(index)?;
            let tool: Option<String> = row.get(1).map_err(index)?;
            let id: Option<String> = row.get(2).map_err(index)?;
            let (Some(tool), Some(id)) = (tool, id) else {
                continue;
            };
            let mut result_rows = result_query
                .query(params![at.source, at.offset, block, end, 129,])
                .map_err(index)?;
            let mut replies = Vec::new();
            while let Some(row) = result_rows.next().map_err(index)? {
                if replies.len() == 128 {
                    return Err(index("Selected reply budget exhausted"));
                }
                let (reply, _) = position(row, at.source).map_err(index)?;
                replies.push(crate::events::Reply {
                    o: u64::try_from(reply.offset).map_err(index)?,
                    b: u32::try_from(reply.block).map_err(index)?,
                    t: row.get(5).map_err(index)?,
                    e: row.get(6).map_err(index)?,
                    f: row.get(7).map_err(index)?,
                    m: row.get(8).map_err(index)?,
                });
            }
            let call = CallSlot {
                id,
                tool,
                block,
                replies,
            };
            let has_operations: bool = row.get(3).map_err(index)?;
            if !has_operations
                || call
                    .display_reply()
                    .is_none_or(|reply| reply.e && reply.f & crate::events::UNKNOWN == 0)
            {
                calls.push(call);
            }
        }
        let operation = db
            .prepare_cached(OPERATION_AT)
            .map_err(index)?
            .query_row(params![at.source, at.offset, at.block], |row| {
                Ok(Operation {
                    parent: row.get(0)?,
                    ok: row.get(1)?,
                })
            })
            .optional()
            .map_err(index)?;
        Ok(Self {
            signals,
            calls,
            operation,
        })
    }

    pub(super) fn render(
        self,
        source: &Source<'_>,
        key: &str,
        reference: &SessionSourceRef,
        event_id: Option<&str>,
        turn: Option<TurnStart>,
    ) -> Context {
        let signals = self
            .signals
            .into_iter()
            .map(|signal| {
                let id = crate::transcript_range::native_entry_id(
                    source.namespace.as_str(),
                    key,
                    reference,
                    (signal.o, 0),
                    "signal",
                    event_id,
                    Some(signal.k),
                );
                SignalSlot { id, signal }
            })
            .collect();
        Context {
            turn,
            signals,
            calls: self.calls,
        }
    }
}
