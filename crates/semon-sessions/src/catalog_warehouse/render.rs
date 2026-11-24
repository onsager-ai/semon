//! Render a selected window using only records already verified by the warehouse reader.
use super::{Component, Record};
use crate::{
    events::{SignalKind, UNKNOWN},
    model::{self, Background, BgEnd, Shown, SignalData, Slot, SlotFile, SlotKind},
    session_route::NativeEntryAddress,
    tx::{self, Lines, RenderContext},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Clone, Copy)]
pub(in super::super) enum Part {
    Text(usize),
    Output,
    Script,
    Attachment(usize),
}

pub(in super::super) enum Expansion {
    Json(Value),
    Image(&'static str, Vec<u8>),
}

pub(in super::super) fn entries(
    component: &Component,
    source: &str,
    key: &str,
    now: i64,
) -> Vec<Value> {
    render(component, source, key, now, |entry, _, _, _| Some(entry))
}

pub(in super::super) fn turn_address(
    component: &Component,
    source: &str,
    key: &str,
    turn: &str,
) -> Option<Value> {
    render(
        component,
        source,
        key,
        model::now_ms(),
        |mut entry, slot, _, _| {
            if !slot.first || slot.turn.as_deref() != Some(turn) {
                return None;
            }
            let secondary = !matches!(entry["k"].as_str(), Some("u" | "h"));
            let address = entry.as_object_mut()?.remove("address")?;
            Some((secondary, address))
        },
    )
    .into_iter()
    .min_by_key(|(order, _)| *order)
    .map(|(_, address)| address)
}

pub(in super::super) fn expand(
    component: &Component,
    source: &str,
    key: &str,
    address: &NativeEntryAddress,
    part: Part,
) -> Option<Expansion> {
    let exact = json!(address);
    render(
        component,
        source,
        key,
        model::now_ms(),
        |mut entry, slot, lines, field| {
            if entry["address"] != exact {
                return None;
            }
            match part {
                Part::Attachment(index) => {
                    let image = entry["img"].as_array()?.get(index)?;
                    let (content_type, bytes) = tx::selected_image(lines, slot, image)?;
                    return Some(Expansion::Image(content_type, bytes));
                }
                Part::Output => {
                    let output = tx::selected_output(lines, slot)?;
                    crate::transcript_range::copy_selected_output_fields(&mut entry, &output);
                }
                Part::Script => {
                    let (script, truncated) = tx::selected_script(lines, slot)?;
                    entry["scriptText"] = json!(script);
                    entry["scriptTruncated"] = json!(truncated);
                }
                Part::Text(chunk) => {
                    if let Some(field) = field {
                        let next = (chunk.checked_add(1)? < field.chunks).then_some(chunk + 1);
                        return Some(Expansion::Json(json!({
                            "entry_id":address.entry_id,"address":address,
                            "field":{"name":field.kind.name(),"chunk":chunk,"next":next,"complete":next.is_none()},
                            "text":field.text.as_ref()?,
                        })));
                    }
                    let (name, text) = tx::selected_chunk_text(lines, slot)?;
                    let mut chunks = text_chunks(&text).skip(chunk);
                    let text = chunks.next()?;
                    let next = chunks.next().map(|_| chunk + 1);
                    entry = json!({
                        "entry_id":address.entry_id,"address":address,
                        "field":{"name":name,"chunk":chunk,"next":next,"complete":next.is_none()},
                        "text":text,
                    });
                }
            }
            Some(Expansion::Json(entry))
        },
    )
    .into_iter()
    .next()
}

fn text_chunks(mut text: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if text.is_empty() {
            return None;
        }
        let mut end = text
            .len()
            .min(crate::json_string::JSON_STRING_CHUNK_DECODED_MAX);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let (chunk, rest) = text.split_at(end);
        text = rest;
        Some(chunk)
    })
}

fn render<T>(
    component: &Component,
    source: &str,
    key: &str,
    now: i64,
    mut finish: impl FnMut(
        Value,
        &Slot,
        &mut Lines<'_>,
        Option<&crate::native_field::SelectedField>,
    ) -> Option<T>,
) -> Vec<T> {
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
        if record.native.is_null() {
            add(SlotKind::Gap, 0, None);
        } else if let Some(anchor) = &record.prompt_anchor {
            let kind = if model::is_ask(anchor)
                || record.causal.turn.as_ref().is_some_and(|turn| {
                    turn.handoff && !turn.head && turn.address == record.address
                }) {
                SlotKind::H(anchor.clone())
            } else {
                SlotKind::U
            };
            add(kind, 0, record.entry_anchor.as_deref());
        } else if record.kind.as_deref() == Some("response_item")
            && crate::field(&record.native["payload"], "type") == Some("message")
            && crate::field(&record.native["payload"], "role") == Some("user")
            && let Some(tag) = crate::events::harness_tag(&crate::events::codex_message_text(
                &record.native["payload"],
            ))
        {
            add(SlotKind::Harness(tag.to_owned()), 0, None);
        } else {
            for block in display_blocks(record) {
                add(block.0, block.1, None);
            }
        }
        if let Some(operation) = record.operation
            && let Some(kind) = record
                .native
                .pointer("/payload/item/type")
                .and_then(Value::as_str)
        {
            add(
                SlotKind::Operation {
                    kind: kind.to_owned(),
                    ok: operation.ok,
                    script_offset: Some(operation.parent),
                },
                0,
                record.entry_anchor.as_deref(),
            );
        }
        for finish in &record.causal.background_ends {
            add(
                SlotKind::BgEnd {
                    call: finish.id.clone(),
                    status: finish.end.status.clone(),
                    source: (file.clone(), finish.launch, finish.block),
                    start: finish.start,
                },
                finish.end.occurrence,
                None,
            );
        }
        for call in &record.causal.calls {
            let reply = call.display_reply().cloned();
            let shown = match reply.as_ref() {
                Some(reply) if reply.f & UNKNOWN != 0 => Shown::Unknown,
                Some(reply) if reply.e => Shown::Err,
                Some(_) => Shown::Ok,
                None if component.harness == "copilot" || !call.qualified => Shown::Unknown,
                None if call.yield_started.is_some() => {
                    if call.yield_open && component.runtime_status == Some("work") {
                        Shown::Live
                    } else {
                        Shown::Unfinished
                    }
                }
                None if matches!(component.runtime_status, Some("work" | "wait"))
                    && component.latest_turn.is_some()
                    && component.latest_turn == record.causal.turn =>
                {
                    Shown::Live
                }
                None => Shown::Unfinished,
            };
            let kind = if let Some(first) = call.yield_started {
                SlotKind::Yielded {
                    shown,
                    first: Some(first),
                    polls: call.polls.clone(),
                    cut: call.polls_cut,
                    done: reply,
                }
            } else {
                SlotKind::Tool {
                    shown,
                    name: call.tool.clone(),
                    result_source: call.tool == "exec"
                        && record
                            .native
                            .pointer("/payload/type")
                            .and_then(Value::as_str)
                            == Some("custom_tool_call")
                        && record
                            .native
                            .pointer("/payload/input")
                            .and_then(Value::as_str)
                            .is_some_and(crate::events::result_source),
                    reply,
                    item: None,
                    bg: (shown == Shown::Ok && call.background).then(|| Background {
                        tid: call.id.clone(),
                        live: false,
                        end: call.background_end.as_ref().map(|end| BgEnd {
                            file: file.clone(),
                            offset: end.offset,
                            t: end.at,
                            status: end.status.clone(),
                            failed: end.failed,
                        }),
                    }),
                }
            };
            add(kind, call.block, None);
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
            entry.as_object_mut()?.remove("slot");
            let (record, id) = addresses[index];
            if matches!(slot.kind, SlotKind::U | SlotKind::H(_))
                && let Some(mut images) = record.images.clone()
            {
                for image in &mut images {
                    image["o"] = json!(slot.offset);
                }
                entry["img"] = Value::Array(images);
            }
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
            entry["address"] = json!(NativeEntryAddress {
                record: record.address.clone(),
                block: slot.block,
                binding: component.binding.clone(),
                entry_id: entry_id.clone(),
            });
            entry["freshness"] = json!({"state":"cached"});
            entry["provenance"] = json!({"source":record.reference,"offset":slot.offset,"block":slot.block,"native_event_id":event_id});
            entry["observed_at"] = json!(slot.t);
            if let Some(turn) = &slot.turn {
                entry["turn"] = json!(turn);
            }
            entry["entry_id"] = json!(entry_id);
            let field = field_at(component, record, slot);
            let has_clipped_output = entry["more"].as_array()
                .is_some_and(|parts| parts.iter().any(|part| part == "out"));
            let may_have_clipped_text = entry["text"].as_str().is_some_and(|text| text.len() >= model::MSG_MAX);
            if let Some(field) = field {
                entry["field"] = json!({"name":field.kind.name(),"chunks":field.chunks,"complete":false});
                entry["clipped"] = json!(true);
            } else if (has_clipped_output || may_have_clipped_text)
                && let Some((name, text)) = tx::selected_chunk_text(&mut lines, slot)
                && (text.len() > model::MSG_MAX || has_clipped_output)
            {
                entry["field"] = json!({"name":name,"chunks":text_chunks(&text).count(),"complete":false});
                entry["clipped"] = json!(true);
            }
            finish(entry, slot, &mut lines, field)
        })
        .collect()
}

fn field_at<'a>(
    component: &'a Component,
    record: &Record,
    slot: &Slot,
) -> Option<&'a crate::native_field::SelectedField> {
    let (offset, block, name) = match &slot.kind {
        SlotKind::U | SlotKind::A | SlotKind::Think => (slot.offset, slot.block, "text"),
        SlotKind::H(id) if model::is_ask(id) => (slot.offset, slot.block, "text"),
        SlotKind::Tool {
            reply: Some(reply), ..
        }
        | SlotKind::Yielded {
            done: Some(reply), ..
        } => (reply.o, reply.b, "out"),
        SlotKind::Operation { kind, .. } if kind == "CommandExecution" => {
            (slot.offset, slot.block, "out")
        }
        _ => return None,
    };
    component
        .fields
        .get(&(record.address.path.clone(), offset))?
        .iter()
        .find(|field| field.block == block && field.kind.name() == name)
}

pub(in super::super) fn has_assistant_text(record: &Record) -> bool {
    display_blocks(record).any(|(kind, _)| matches!(kind, SlotKind::A))
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
