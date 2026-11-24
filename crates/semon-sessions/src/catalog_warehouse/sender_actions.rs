//! Relationship cards describe only calls and outcomes in the selected sender.
use super::selected;
use crate::events::{DENIED, SEND_FAILED, UNKNOWN};
use serde_json::{Value, json};

pub(super) struct Action<'a> {
    pub(super) id: String,
    pub(super) turn: &'a str,
    pub(super) address: &'a selected::Address,
    pub(super) block: u32,
    kind: &'static str,
    label: Option<String>,
    at: i64,
    done: Option<i64>,
    status: &'static str,
}

impl Action<'_> {
    pub(super) fn edge(&self, key: &str) -> Value {
        json!({"id":self.id,"kind":self.kind,"from":key,"to":null,"at":self.at,"done":self.done,
            "status":self.status,"declined":false,"source_turn":self.turn,
            "target_label":self.label,"reason":"not linked yet"})
    }
}

pub(super) fn recorded<'a>(
    component: &'a selected::Component,
    key: &'a str,
) -> impl Iterator<Item = Action<'a>> {
    component.records.iter().flat_map(move |record| {
        record.causal.calls.iter().filter_map(move |call| {
            let (kind, label_fields): (_, &[_]) = match call.tool.as_str() {
                "SendMessage"
                | "send_message"
                | "followup_task"
                | "collaboration.send_message"
                | "collaboration.followup_task" => ("relay", &["to", "target", "target_id"]),
                "Agent" | "Task" | "spawn_agent" | "collaboration.spawn_agent" => {
                    ("spawn", &["name", "task_name", "description"])
                }
                _ => return None,
            };
            let turn = &record.causal.turn.as_ref()?.id;
            let at = record.at?;
            let label =
                crate::model::input_string(&record.native, call.block as usize, label_fields)
                    .filter(|label| !label.trim().is_empty())
                    .map(|label| {
                        label
                            .chars()
                            .filter(|ch| !ch.is_control())
                            .take(96)
                            .collect()
                    });
            let reply = call.display_reply();
            let status = match reply {
                Some(reply) if reply.e || reply.f & (DENIED | SEND_FAILED) != 0 => "err",
                Some(reply) if reply.f & UNKNOWN == 0 => "done",
                _ => "off",
            };
            Some(Action {
                id: crate::model::legacy_relay_turn_id(&format!(
                    "sender:{key}:{}:{}:{}:{}",
                    record.address.path,
                    record.address.offset,
                    record.address.occurrence,
                    call.block
                )),
                turn,
                address: &record.address,
                block: call.block,
                kind,
                label,
                at,
                done: reply.and_then(|reply| reply.t),
                status,
            })
        })
    })
}
