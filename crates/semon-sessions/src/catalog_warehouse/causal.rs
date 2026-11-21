//! Prefix-qualified native causal inputs for the selected-record adapter.
use super::{Position, ReadError, SessionSourceRef, Source, TOOL_RESULTS, index, position};
use rusqlite::{Connection, params};

const BOUNDARY: &str = "SELECT pos,kind FROM (
    SELECT pos,kind FROM (SELECT pos,kind FROM fact_prompt WHERE source=?1 AND pos<=?2
      ORDER BY pos DESC,block DESC,occurrence DESC LIMIT 1)
    UNION ALL
    SELECT pos,NULL FROM (SELECT pos FROM fact_record WHERE source=?1 AND pos<=?2 AND gap
      ORDER BY pos DESC LIMIT 1)
) ORDER BY pos DESC LIMIT 1";
const SIGNALS_AT: &str = "SELECT payload FROM warehouse_signal WHERE source=?1 AND pos=?2
    AND revision<=?3 AND (retired_revision IS NULL OR retired_revision>?3) ORDER BY seq LIMIT 129";
const CALLS_AT: &str = "SELECT block,tool,call_id FROM fact_tool_call WHERE source=?1 AND pos=?2
    ORDER BY block,occurrence LIMIT 129";

#[derive(Debug, Default)]
pub(in super::super) struct Context {
    pub(in super::super) source_turn: Option<String>,
    pub(in super::super) signals: Vec<SignalSlot>,
    pub(in super::super) calls: Vec<CallSlot>,
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

pub(super) struct Inputs {
    source_turn: Option<String>,
    signals: Vec<crate::events::Signal>,
    calls: Vec<CallSlot>,
}

impl Inputs {
    pub(super) fn load(
        db: &Connection,
        key: &str,
        reference: &SessionSourceRef,
        at: Position,
        revision: i64,
    ) -> Result<Self, ReadError> {
        use rusqlite::OptionalExtension;

        let boundary: Option<(i64, Option<String>)> = db
            .prepare_cached(BOUNDARY)
            .map_err(index)?
            .query_row(params![at.source, at.offset], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(index)?;
        let source_turn = match boundary {
            Some((pos, Some(kind))) => {
                let offset = u64::try_from(pos).map_err(index)?;
                if kind == "ask" {
                    Some(crate::model::ask_id(&reference.native_id, offset))
                } else {
                    Some(crate::model::native_turn_id(
                        key,
                        &reference.native_id,
                        offset,
                    ))
                }
            }
            _ => None,
        };
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
        let mut call_rows = calls_query
            .query(params![at.source, at.offset])
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
                .query(params![
                    at.source,
                    at.offset,
                    block,
                    i64::try_from(reference.offset).map_err(index)?,
                    129,
                ])
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
            calls.push(CallSlot {
                id,
                tool,
                block,
                replies,
            });
        }
        Ok(Self {
            source_turn,
            signals,
            calls,
        })
    }

    pub(super) fn render(
        self,
        source: &Source<'_>,
        key: &str,
        reference: &SessionSourceRef,
        event_id: Option<&str>,
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
            source_turn: self.source_turn,
            signals,
            calls: self.calls,
        }
    }
}
