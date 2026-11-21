//! Parent-addressed Claude relay matching. Native bodies stay in the request.
use super::*;

struct Send {
    pos: i64,
    block: i64,
    sent: Option<i64>,
    last: Option<i64>,
}

struct Receipt {
    source: i64,
    member: Arc<Member>,
    address: RelayReceipt,
    skip: usize,
}

pub(super) struct Plan {
    sends: Vec<Send>,
    receipts: Vec<Receipt>,
    child: String,
    call_offset: u64,
    call_block: i64,
}

impl Plan {
    pub(super) fn collect(
        db: &rusqlite::Connection,
        source: &Source<'_>,
        child: i64,
        call: &Address,
        view: i64,
    ) -> Result<Option<Self>, ReadError> {
        use crate::events::{AGM_FINAL, AGM_UP, PIN};
        let binding: Option<(String, i64, Option<String>)> = db
            .query_row(
                "SELECT d.native_id,b.owner,p.call_id FROM dim_session d
             JOIN parent_selection p ON p.child=d.source AND p.method='native'
             JOIN bridge_session_owner b ON b.source=p.parent
             WHERE d.source=?1 AND d.harness='claude' AND NOT d.lineage",
                [child],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(index)?;
        let Some((native_id, parent, spawn)) = binding else {
            return Ok(None);
        };
        let mut prefix = db.prepare_cached(COMMITTED_PREFIX).map_err(index)?;
        let child_end =
            i64::try_from(committed_prefix(&mut prefix, child, view)?.offset).map_err(index)?;
        let mut sends = db
            .prepare_cached(include_str!("parent_relay.sql"))
            .map_err(index)?;
        let sends = sends
            .query_map(params![child, child_end, PIN], |row| {
                Ok(Send {
                    pos: row.get(0)?,
                    block: row.get(1)?,
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
        if !sends
            .iter()
            .any(|send| send.pos as u64 == call.offset && send.block == call.block)
        {
            return Ok(None);
        }
        let mut members = db
            .prepare_cached(
                "SELECT m.source,s.carrier,s.ledger,d.native_id
            FROM lineage_member root JOIN lineage_member m USING(component)
            JOIN warehouse_source s ON s.source=m.source AND s.origin=?2
            JOIN warehouse_current c ON c.source=s.source AND c.path=s.carrier
            JOIN dim_session d ON d.source=m.source WHERE root.source=?1 ORDER BY m.source",
            )
            .map_err(index)?;
        let mut members = members
            .query(params![parent, source.namespace.binding()])
            .map_err(index)?;
        let mut scan = db
            .prepare_cached(
                "SELECT pos,block,occurrence,ts FROM fact_subagent_event
            WHERE source=?1 AND pos<?2 AND kind='agm' AND flags & ?3<>0 AND flags & ?4=0
              AND (id IN (?5,?6) OR name IN (?5,?6)) ORDER BY pos,block,occurrence LIMIT 129",
            )
            .map_err(index)?;
        let mut receipts = Vec::new();
        while let Some(row) = members.next().map_err(index)? {
            let id: i64 = row.get(0).map_err(index)?;
            let carrier: String = row.get(1).map_err(index)?;
            let current = decode(&row.get::<_, String>(2).map_err(index)?)?;
            match source.inventory.get(&carrier) {
                Some(Availability::Expected(Some(stat))) => {
                    if !stable_or_grown(&current.stat, stat)
                        || !raw_stat(Path::new(&carrier))
                            .is_ok_and(|actual| stable_or_grown(stat, &actual))
                    {
                        return Err(ReadError::Restart);
                    }
                }
                Some(Availability::Expected(None))
                    if source.current_provider.is_some() && local_missing(&carrier) => {}
                _ => return Ok(None),
            }
            let committed = committed_prefix(&mut prefix, id, view)?;
            if committed.offset > current.offset || !committed.stat.is_prefix_of(&current.stat) {
                return Err(ReadError::Restart);
            }
            let path = relative(source.options, "claude", &carrier)
                .ok_or(ReadError::Restart)?
                .to_owned();
            let member = Arc::new(Member {
                carrier,
                current,
                reference: SessionSourceRef {
                    root: "claude".into(),
                    path,
                    native_id: row.get(3).map_err(index)?,
                    offset: committed.offset,
                    prefix_sha256: committed.prefix,
                    tail_sha256: committed.tail,
                },
            });
            let mut rows = scan
                .query(params![
                    id,
                    i64::try_from(committed.offset).map_err(index)?,
                    AGM_UP,
                    AGM_FINAL,
                    native_id,
                    spawn
                ])
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
                    address: RelayReceipt {
                        path: member.carrier.clone(),
                        offset,
                        block: row.get(1).map_err(index)?,
                        at: row.get(3).map_err(index)?,
                    },
                });
            }
        }
        receipts.sort_by_key(|receipt| (receipt.address.at.is_none(), receipt.address.at));
        Ok(Some(Self {
            sends,
            receipts,
            child: native_id,
            call_offset: call.offset,
            call_block: call.block,
        }))
    }

    pub(super) fn resolve(
        self,
        child_source: i64,
        child: &Arc<Member>,
        mut read: impl FnMut(i64, &Arc<Member>, u64) -> Result<serde_json::Value, ReadError>,
    ) -> Result<Option<RelayReceipt>, ReadError> {
        if self.receipts.is_empty() {
            return Ok(None);
        }
        let mut messages = Vec::new();
        for send in &self.sends {
            let record = read(child_source, child, u64::try_from(send.pos).map_err(index)?)?;
            messages.push(crate::model::input_string(
                &record,
                usize::try_from(send.block).map_err(index)?,
                &["message", "content"],
            ));
        }
        let mut bodies = Vec::new();
        for receipt in &self.receipts {
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
        let (paired, _) = crate::model::pair_up(&windows, &messages, &times, &bodies);
        let Some(index) = self
            .sends
            .iter()
            .position(|send| send.pos as u64 == self.call_offset && send.block == self.call_block)
        else {
            return Ok(None);
        };
        Ok(paired[index]
            .and_then(|match_index| self.receipts.into_iter().nth(match_index))
            .map(|receipt| receipt.address))
    }
}
