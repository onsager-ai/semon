//! Parent-addressed Claude relay matching. Native bodies stay in the request.
use super::*;
struct Send {
    pos: i64,
    block: i64,
    id: String,
    sent: Option<i64>,
    last: Option<i64>,
}

struct Receipt<M> {
    source: i64,
    member: M,
    address: RelayReceipt,
    skip: usize,
    legacy: bool,
    occurrence: i64,
}

pub(super) struct Plan<M> {
    sends: Vec<Send>,
    receipts: Vec<Receipt<M>>,
    child: String,
    call: Option<(u64, i64)>,
    member: M,
    explicit_completion: bool,
}

impl Plan<Arc<Member>> {
    pub(super) fn collect(
        db: &rusqlite::Connection,
        source: &Source<'_>,
        child: i64,
        call: Option<&Address>,
        view: i64,
    ) -> Result<Option<Self>, ReadError> {
        use crate::events::AGM_FINAL;
        let native_id: Option<String> = db.query_row(
            "SELECT native_id FROM dim_session WHERE source=?1 AND harness='claude' AND NOT lineage",
            [child], |row| row.get(0),
        ).optional().map_err(index)?;
        let Some(native_id) = native_id else {
            return Ok(None);
        };
        let mut graph = super::super::parents::Parents::new(db)?;
        let Some(edge) = graph.parent(child)?.filter(|edge| edge.method == "native") else {
            return Ok(None);
        };
        let Some(parent) = graph.owner(edge.source)? else {
            return Ok(None);
        };
        let spawn = edge.call;
        let mut prefix = db.prepare_cached(COMMITTED_PREFIX).map_err(index)?;
        let Some(child_ledger) = prefix
            .query_row(params![child, view], |row| row.get::<_, String>(0))
            .optional()
            .map_err(index)?
        else {
            return Ok(None);
        };
        let child_end = i64::try_from(decode(&child_ledger)?.offset).map_err(index)?;
        let sends = collect_sends(db, child, child_end)?;
        if call.is_some_and(|call| {
            !sends
                .iter()
                .any(|send| send.pos as u64 == call.offset && send.block == call.block)
        }) {
            return Ok(None);
        }
        let Some(member) = member_at(db, source, &mut prefix, child, view)? else {
            return Ok(None);
        };
        let mut explicit_completion = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fact_subagent_event WHERE source=?1 AND pos<?2
             AND kind='tool' AND name='SubagentHandback')",
                params![child, child_end],
                |row| row.get::<_, bool>(0),
            )
            .map_err(index)?;
        let mut members = db
            .prepare_cached(
                "SELECT m.source
            FROM lineage_member root JOIN lineage_member m USING(component)
            JOIN warehouse_source s ON s.source=m.source AND s.origin=?2
            JOIN warehouse_current c ON c.source=s.source AND c.path=s.carrier
            JOIN dim_session d ON d.source=m.source WHERE root.source=?1 ORDER BY m.source",
            )
            .map_err(index)?;
        let mut members = members
            .query(params![parent, source.namespace.binding()])
            .map_err(index)?;
        let mut receipts = Vec::new();
        while let Some(row) = members.next().map_err(index)? {
            let id: i64 = row.get(0).map_err(index)?;
            let Some(parent_member) = member_at(db, source, &mut prefix, id, view)? else {
                return Ok(None);
            };
            let end = i64::try_from(parent_member.reference.offset).map_err(index)?;
            if call.is_none() && !explicit_completion {
                explicit_completion = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM fact_subagent_event WHERE source=?1 AND pos<?2
                     AND ((kind='tn' AND id=?3) OR (kind='agm' AND flags & ?4<>0
                       AND (id IN (?3,?5) OR name IN (?3,?5)))))",
                        params![id, end, spawn, AGM_FINAL, native_id],
                        |row| row.get(0),
                    )
                    .map_err(index)?;
            }
            collect_receipts(
                db,
                id,
                end,
                &native_id,
                spawn.as_deref(),
                &parent_member,
                &parent_member.carrier,
                &mut receipts,
            )?;
        }
        receipts.sort_by_key(|receipt| (receipt.address.at.is_none(), receipt.address.at));
        Ok(Some(Self {
            sends,
            receipts,
            child: native_id,
            call: call.map(|call| (call.offset, call.block)),
            member,
            explicit_completion,
        }))
    }
}

impl Plan<SessionSourceRef> {
    pub(super) fn collect_certified(
        db: &rusqlite::Connection,
        child: i64,
        reference: SessionSourceRef,
        spawn: Option<&str>,
        call: Option<(&Address, u32)>,
        parents: &[(i64, SessionSourceRef)],
    ) -> Result<Option<Self>, ReadError> {
        let sends = collect_sends(db, child, i64::try_from(reference.offset).map_err(index)?)?;
        let at = call.map(|(address, block)| (address.offset, i64::from(block)));
        if at.is_some_and(|at| {
            sends
                .iter()
                .filter(|send| (send.pos as u64, send.block) == at)
                .count()
                != 1
        }) {
            return Ok(None);
        }
        let child_end = i64::try_from(reference.offset).map_err(index)?;
        let mut explicit_completion = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fact_subagent_event WHERE source=?1 AND pos<?2
             AND kind='tool' AND name='SubagentHandback')",
                params![child, child_end],
                |row| row.get::<_, bool>(0),
            )
            .map_err(index)?;
        let mut receipts = Vec::new();
        for (id, parent) in parents {
            let end = i64::try_from(parent.offset).map_err(index)?;
            if !explicit_completion {
                explicit_completion = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM fact_subagent_event WHERE source=?1 AND pos<?2
                     AND ((kind='tn' AND id=?3) OR (kind='agm' AND flags & ?4<>0
                       AND (id IN (?3,?5) OR name IN (?3,?5)))))",
                        params![
                            id,
                            end,
                            spawn,
                            crate::events::AGM_FINAL,
                            reference.native_id
                        ],
                        |row| row.get(0),
                    )
                    .map_err(index)?;
            }
            collect_receipts(
                db,
                *id,
                end,
                &reference.native_id,
                spawn,
                parent,
                &parent.path,
                &mut receipts,
            )?;
        }
        receipts.sort_by_key(|receipt| (receipt.address.at.is_none(), receipt.address.at));
        Ok(Some(Self {
            sends,
            receipts,
            child: reference.native_id.clone(),
            call: at,
            member: reference,
            explicit_completion,
        }))
    }
}

impl<M> Plan<M> {
    pub(super) fn resolve(
        self,
        child_source: i64,
        mut read: impl FnMut(i64, &M, u64) -> Result<serde_json::Value, ReadError>,
    ) -> Result<Resolution, ReadError> {
        let mut result = Resolution::default();
        if self.receipts.is_empty()
            || (self.call.is_none() && !self.receipts.iter().any(|receipt| receipt.legacy))
        {
            return Ok(result);
        }
        let mut messages = Vec::new();
        for send in &self.sends {
            let record = read(
                child_source,
                &self.member,
                u64::try_from(send.pos).map_err(index)?,
            )?;
            messages.push(crate::model::input_string(
                &record,
                usize::try_from(send.block).map_err(index)?,
                &["message", "content"],
            ));
        }
        let mut bodies = Vec::new();
        for receipt in &self.receipts {
            if self.sends.is_empty() {
                bodies.push(None);
                continue;
            }
            let record = read(receipt.source, &receipt.member, receipt.address.offset)?;
            bodies.push(crate::model::received_body(
                &record,
                "agent-message",
                Some(&self.child),
                receipt.skip,
            ));
        }
        let windows: Vec<_> = self
            .sends
            .iter()
            .map(|send| (send.sent, send.last))
            .collect();
        let times: Vec<_> = self
            .receipts
            .iter()
            .map(|receipt| receipt.address.at)
            .collect();
        let (matches, claimed) = crate::model::pair_up(&windows, &messages, &times, &bodies);
        let matched = self
            .call
            .and_then(|call| {
                self.sends
                    .iter()
                    .position(|send| (send.pos as u64, send.block) == call)
            })
            .and_then(|send| matches[send]);
        if !self.explicit_completion {
            result.completion = self
                .receipts
                .iter()
                .enumerate()
                .rev()
                .find(|(index, receipt)| receipt.legacy && !claimed[*index])
                .map(|(_, receipt)| receipt.address.clone());
        }
        for (index, receipt) in self.receipts.into_iter().enumerate() {
            if matched == Some(index) {
                result.matched = Some(receipt.address.clone());
            }
            if receipt.legacy
                && claimed[index]
                && let Some(send) = matches.iter().position(|matched| *matched == Some(index))
            {
                let turn = crate::model::legacy_relay_turn_id(&self.sends[send].id);
                if matched == Some(index) {
                    result.matched_legacy_turn = Some(turn.clone());
                }
                result.incoming.insert(
                    format!(
                        "{}:{}:{}:{}",
                        receipt.source,
                        receipt.address.offset,
                        receipt.address.block,
                        receipt.occurrence
                    ),
                    turn,
                );
            }
        }
        Ok(result)
    }
}

fn collect_sends(
    db: &rusqlite::Connection,
    child: i64,
    child_end: i64,
) -> Result<Vec<Send>, ReadError> {
    use crate::events::PIN;
    let mut sends = db
        .prepare_cached(include_str!("parent_relay.sql"))
        .map_err(index)?;
    let sends = sends
        .query_map(params![child, child_end, PIN], |row| {
            Ok(Send {
                pos: row.get(0)?,
                block: row.get(1)?,
                id: row.get(2)?,
                sent: row.get(3)?,
                last: row.get(4)?,
            })
        })
        .map_err(index)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(index)?;
    if sends.len() > 128 {
        return Err(index("Selected parent relay send budget exhausted"));
    }
    Ok(sends)
}

#[allow(clippy::too_many_arguments)]
fn collect_receipts<M: Clone>(
    db: &rusqlite::Connection,
    id: i64,
    end: i64,
    child: &str,
    spawn: Option<&str>,
    member: &M,
    path: &str,
    receipts: &mut Vec<Receipt<M>>,
) -> Result<(), ReadError> {
    use crate::events::{AGM_FINAL, AGM_PROGRESS, AGM_UP};
    let mut scan = db
        .prepare_cached(
            "SELECT e.pos,e.block,e.occurrence,e.ts,e.flags,r.occurrence FROM fact_subagent_event e
        CROSS JOIN fact_record r ON r.source=e.source AND r.pos=e.pos AND r.block=0 AND NOT r.gap
        WHERE e.source=?1 AND e.pos<?2 AND e.kind='agm' AND e.flags & ?3<>0 AND e.flags & ?4=0
          AND (e.id IN (?5,?6) OR e.name IN (?5,?6)) ORDER BY e.pos,e.block,e.occurrence LIMIT 129",
        )
        .map_err(index)?;
    let mut rows = scan
        .query(params![id, end, AGM_UP, AGM_FINAL, child, spawn])
        .map_err(index)?;
    let mut prior = None;
    let mut skip = 0;
    while let Some(row) = rows.next().map_err(index)? {
        if receipts.len() == 128 {
            return Err(index("Selected parent relay receipt budget exhausted"));
        }
        let offset = u64::try_from(row.get::<_, i64>(0).map_err(index)?).map_err(index)?;
        skip = if prior == Some(offset) { skip + 1 } else { 0 };
        prior = Some(offset);
        receipts.push(Receipt {
            source: id,
            member: member.clone(),
            skip,
            legacy: row.get::<_, u8>(4).map_err(index)? & AGM_PROGRESS == 0,
            occurrence: row.get(2).map_err(index)?,
            address: RelayReceipt {
                path: path.to_owned(),
                offset,
                block: row.get(1).map_err(index)?,
                at: row.get(3).map_err(index)?,
            },
        });
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct Resolution {
    pub(super) matched: Option<RelayReceipt>,
    pub(super) matched_legacy_turn: Option<String>,
    pub(super) completion: Option<RelayReceipt>,
    pub(super) incoming: BTreeMap<String, String>,
}

fn member_at(
    db: &rusqlite::Connection,
    source: &Source<'_>,
    prefix: &mut rusqlite::Statement<'_>,
    id: i64,
    view: i64,
) -> Result<Option<Arc<Member>>, ReadError> {
    let (carrier, ledger, native_id): (String, String, String) = db
        .query_row(
            "SELECT s.carrier,s.ledger,d.native_id FROM warehouse_source s
         JOIN warehouse_current c ON c.source=s.source AND c.path=s.carrier
         JOIN dim_session d ON d.source=s.source WHERE s.source=?1 AND s.origin=?2",
            params![id, source.namespace.binding()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(index)?;
    let current = decode(&ledger)?;
    let remote = match source.availability("claude", &carrier)? {
        Availability::Expected(Some(stat)) => {
            if !stable_or_grown(&current.stat, &stat)
                || !raw_stat(Path::new(&carrier))
                    .is_ok_and(|actual| stable_or_grown(&stat, &actual))
            {
                return Err(ReadError::Restart);
            }
            false
        }
        Availability::Expected(None)
            if source.current_provider.is_some() && local_missing(&carrier) =>
        {
            true
        }
        _ => return Ok(None),
    };
    let committed = committed_prefix(prefix, id, view)?;
    if committed.offset > current.offset
        || !committed.stat.is_prefix_of(&current.stat)
        || (committed.offset == current.offset
            && (committed.prefix != current.prefix || committed.tail != current.tail))
    {
        return Err(ReadError::Restart);
    }
    let path = relative(source.options, "claude", &carrier)
        .ok_or(ReadError::Restart)?
        .to_owned();
    Ok(Some(Arc::new(Member {
        carrier,
        current,
        remote,
        changed_local: None,
        reference: SessionSourceRef {
            root: "claude".into(),
            path,
            native_id,
            offset: committed.offset,
            prefix_sha256: committed.prefix,
            tail_sha256: committed.tail,
        },
    })))
}

impl std::fmt::Debug for TraceCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceCompletion")
            .field("address", &self.address)
            .field("block", &self.block)
            .field("at", &self.at)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}
