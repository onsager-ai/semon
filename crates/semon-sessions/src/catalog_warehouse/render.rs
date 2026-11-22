//! Render a selected window using only records already verified by the warehouse reader.
use super::{Component, Record};
use crate::{
    events::{SignalKind, UNKNOWN},
    model::{self, Shown, SignalData, Slot, SlotFile, SlotKind},
    tx::{self, Lines, RenderContext},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub(in super::super) fn entries(
    component: &Component,
    source: &str,
    key: &str,
    now: i64,
) -> Vec<Value> {
    let context = RenderContext {
        home: None,
        harness: Some(&component.harness),
        bounded: true,
    };
    let mut files = BTreeMap::new();
    let mut bodies: BTreeMap<_, _> = component
        .reply_records
        .iter()
        .map(|((path, offset), value)| ((PathBuf::from(path), *offset), value))
        .collect();
    let mut slots = Vec::new();
    let mut addresses = Vec::new();
    for record in &component.records {
        let file = Arc::clone(
            files
                .entry(record.address.path.as_str())
                .or_insert_with(|| {
                    Arc::new(SlotFile {
                        path: PathBuf::from(&record.address.path),
                        cwd: None,
                        prompts: Default::default(),
                    })
                }),
        );
        bodies.insert((file.path.clone(), record.address.offset), &record.native);
        let mut record_slots = Vec::new();
        let mut add = |kind, block, entry_id| {
            let mut slot = Slot::new(
                kind,
                Some(file.clone()),
                record.address.offset,
                block,
                record.at,
            );
            slot.turn = record.causal.turn.as_ref().map(|turn| turn.id.clone());
            slot.first = record
                .causal
                .turn
                .as_ref()
                .is_some_and(|turn| turn.address == record.address);
            record_slots.push((slot, entry_id));
        };
        if let Some(anchor) = &record.prompt_anchor {
            let kind = if model::is_ask(anchor)
                || record.causal.turn.as_ref().is_some_and(|turn| {
                    turn.handoff && !turn.head && turn.address == record.address
                }) {
                SlotKind::H(anchor.clone())
            } else {
                SlotKind::U
            };
            add(kind, 0, record.entry_anchor.as_deref());
        } else {
            for block in display_blocks(record) {
                add(block.0, block.1, None);
            }
        }
        for call in &record.causal.calls {
            let reply = call.display_reply().cloned();
            let shown = match reply.as_ref() {
                Some(reply) if reply.f & UNKNOWN != 0 => Shown::Unknown,
                Some(reply) if reply.e => Shown::Err,
                Some(_) => Shown::Ok,
                None => Shown::Unknown,
            };
            add(
                SlotKind::Tool {
                    shown,
                    name: call.tool.clone(),
                    result_source: false,
                    reply,
                    item: None,
                    bg: None,
                },
                call.block,
                None,
            );
        }
        record_slots.sort_by_key(|(slot, _)| slot.block);
        for (slot, entry_id) in record_slots {
            addresses.push((record, entry_id));
            slots.push(slot);
        }
        for signal in &record.causal.signals {
            let mut slot = Slot::new(
                SlotKind::Signal(SignalData {
                    kind: signal.signal.k,
                    tag: signal.signal.n.clone(),
                    previous: None,
                    value: signal
                        .signal
                        .v
                        .filter(|_| signal.signal.k == SignalKind::Compact),
                    tool: None,
                }),
                Some(file.clone()),
                signal.signal.o,
                0,
                signal.signal.t,
            );
            slot.turn = record.causal.turn.as_ref().map(|turn| turn.id.clone());
            addresses.push((record, Some(signal.id.as_str())));
            slots.push(slot);
        }
    }
    let mut read = |path: &std::path::Path, offset| {
        Ok((
            bodies
                .get(&(path.to_owned(), offset))
                .map(|value| (**value).clone()),
            0,
        ))
    };
    let mut lines = Lines::provider(&mut read);
    slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            let mut entry = tx::render(&context, &mut lines, &slots, slot, index, now)?;
            let (record, id) = addresses[index];
            let event_id = tx::native_event_id(&record.native);
            let entry_id = id.map_or_else(
                || crate::transcript_range::native_entry_id(
                    source,
                    key,
                    &record.reference,
                    (slot.offset, slot.block),
                    entry["k"].as_str().expect("native renderer emits a kind"),
                    event_id.as_deref(),
                    None,
                ),
                str::to_owned,
            );
            entry["address"] = json!({"record":record.address,"block":slot.block});
            entry["provenance"] = json!({"source":record.reference,"offset":slot.offset,"block":slot.block,"native_event_id":event_id});
            entry["observed_at"] = json!(slot.t);
            if let Some(turn) = &slot.turn {
                entry["turn"] = json!(turn);
            }
            entry["entry_id"] = json!(entry_id);
            Some(entry)
        })
        .collect()
}

fn display_blocks(record: &Record) -> impl Iterator<Item = (SlotKind, u32)> + '_ {
    let count = record
        .native
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map_or(1, Vec::len);
    let assistant = matches!(
        record.kind.as_deref(),
        Some("assistant" | "assistant.message")
    ) || (record.kind.as_deref() == Some("response_item")
        && record
            .native
            .pointer("/payload/role")
            .and_then(Value::as_str)
            == Some("assistant"));
    (0..count).filter_map(move |block| {
        let kind = if tx::think_text(&record.native, block).is_some() {
            SlotKind::Think
        } else if assistant
            && model::assistant_text(&record.native, block).is_some_and(|text| !text.is_empty())
        {
            SlotKind::A
        } else {
            return None;
        };
        Some((kind, u32::try_from(block).ok()?))
    })
}
