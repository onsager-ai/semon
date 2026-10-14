//! Shared five-tool search ordering, snippets, line accounting and stop rules.
use super::*;

pub(super) struct Search {
    text: String,
    needle: String,
    since: Option<i64>,
    limit: usize,
    max_bytes: u64,
    rows: BTreeMap<String, Value>,
    sessions: Vec<(String, Vec<&'static str>, usize)>,
    matches: Vec<Value>,
    stopped: Option<&'static str>,
    cache: tx::LineCache,
}
impl Search {
    pub(super) fn new(args: &Value, since: Option<i64>, rows: BTreeMap<String, Value>) -> Self {
        let text = string(args, "text").unwrap_or_default();
        let needle = text.to_ascii_lowercase();
        let rows: BTreeMap<_, _> = rows
            .into_iter()
            .filter(|(_, row)| {
                row["kind"] != "stub"
                    && since.is_none_or(|since| {
                        row["last_activity"]
                            .as_i64()
                            .is_some_and(|last| last >= since)
                    })
            })
            .collect();
        let sessions = rows
            .iter()
            .filter_map(|(id, row)| {
                let fields: Vec<_> = ["name", "repo", "branch"]
                    .into_iter()
                    .filter(|field| {
                        row[*field]
                            .as_str()
                            .is_some_and(|value| find_in(value, &needle).is_some())
                    })
                    .collect();
                (!fields.is_empty()).then(|| (id.clone(), fields, 0))
            })
            .collect();
        Self {
            text,
            needle,
            since,
            limit: integer(args, "limit").unwrap_or(FIND_LIMIT) as usize,
            max_bytes: integer(args, "max_bytes").unwrap_or(FIND_BYTES) as u64,
            rows,
            sessions,
            matches: Vec::new(),
            stopped: None,
            cache: tx::LineCache::default(),
        }
    }
    pub(super) fn order(&self) -> Vec<String> {
        let mut order: Vec<_> = self.rows.keys().cloned().collect();
        order.sort_by(|a, b| {
            self.rows[b]["last_activity"]
                .as_i64()
                .cmp(&self.rows[a]["last_activity"].as_i64())
                .then_with(|| a.cmp(b))
        });
        order
    }
    pub(super) fn since(&self) -> Option<i64> {
        self.since
    }
    pub(super) fn eligible(&self, at: Option<i64>) -> bool {
        self.since
            .is_none_or(|since| at.is_some_and(|at| at >= since))
    }
    pub(super) fn at_capacity(&self) -> bool {
        self.matches.len() >= self.limit || self.cache.bytes >= self.max_bytes
    }
    pub(super) fn stop(&mut self) {
        self.stopped = Some(if self.matches.len() >= self.limit {
            "limit"
        } else {
            "bytes"
        });
    }
    pub(super) fn stopped(&self) -> bool {
        self.stopped.is_some()
    }
    pub(super) fn scan(
        &mut self,
        served: &str,
        transcript: &Transcript,
        data: &crate::model::QueryData,
        handoff_rows: &[Value],
        records: &crate::query_source::Records<'_>,
    ) -> Result<(), QueryError> {
        records
            .verify_all()
            .map_err(|error| QueryError::io(&error))?;
        self.scan_range(served, transcript, 0, data, handoff_rows, records)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn scan_range(
        &mut self,
        served: &str,
        transcript: &Transcript,
        start: usize,
        data: &crate::model::QueryData,
        handoff_rows: &[Value],
        records: &crate::query_source::Records<'_>,
    ) -> Result<(), QueryError> {
        let handoffs: HashMap<&str, &Value> = handoff_rows
            .iter()
            .filter_map(|h| h["id"].as_str().map(|id| (id, h)))
            .collect();
        let mut read = |path: &std::path::Path, offset| records.read(path, offset);
        let mut lines = tx::Lines::provider_with_cache(&mut read, std::mem::take(&mut self.cache));
        for (position, slot) in transcript.slots.iter().enumerate().rev() {
            if self
                .since
                .is_some_and(|since| !slot.t.is_some_and(|t| t >= since))
            {
                continue;
            }
            if self.matches.len() >= self.limit {
                self.stopped = Some("limit");
                break;
            }
            if lines.bytes >= self.max_bytes {
                self.stopped = Some("bytes");
                break;
            }
            let (kind, texts) = match &slot.kind {
                SlotKind::H(id) => {
                    let handoff: Option<&Value> = handoffs.get(id.as_str()).copied();
                    let brief = handoff
                        .and_then(|_| data.ask_briefs.get(id))
                        .map(|brief| brief.read(records))
                        .transpose()
                        .map_err(|error| QueryError::io(&error))?;
                    let mut texts = Vec::new();
                    for part in ["brief", "result"] {
                        let text = if part == "brief" {
                            brief.as_deref()
                        } else {
                            None
                        }
                        .or_else(|| handoff.and_then(|handoff| handoff[part].as_str()));
                        if let Some(text) = text {
                            texts.push((part, text.to_owned()));
                        }
                    }
                    if let Some(answers) = handoff.and_then(|handoff| handoff["answer"].as_array())
                    {
                        let answer: Vec<&str> = answers.iter().filter_map(Value::as_str).collect();
                        texts.push(("answer", answer.join("\n")));
                    }
                    ("h", texts)
                }
                SlotKind::U => ("u", tx::slot_texts(&mut lines, slot)),
                SlotKind::A => ("a", tx::slot_texts(&mut lines, slot)),
                SlotKind::Think => ("think", tx::slot_texts(&mut lines, slot)),
                SlotKind::Tool { .. }
                | SlotKind::Operation { .. }
                | SlotKind::Yielded { .. }
                | SlotKind::Sent { .. } => ("tool", tx::slot_texts(&mut lines, slot)),
                _ => continue,
            };
            let Some((part, text, at)) = texts
                .iter()
                .find_map(|(part, text)| find_in(text, &self.needle).map(|at| (*part, text, at)))
            else {
                continue;
            };
            let mut entry = json!({
                "session": served,
                "position": start + position,
                "turn": slot.turn,
                "kind": kind,
                "part": part,
                "at": slot.t,
                "snippet": snippet(text, at, self.needle.len()),
            });
            match &slot.kind {
                SlotKind::H(id) => entry["handoff"] = json!(id),
                SlotKind::Tool { name, .. } => entry["name"] = json!(name),
                SlotKind::Yielded { .. } => entry["name"] = json!("exec_command"),
                SlotKind::Sent { .. } => entry["name"] = json!("write_stdin"),
                SlotKind::Operation { kind, .. } => {
                    entry["name"] = json!(if kind == "CommandExecution" {
                        "exec_command"
                    } else {
                        "apply_patch"
                    })
                }
                _ => {}
            }
            self.matches.push(entry);
            match self.sessions.iter_mut().find(|(id, _, _)| id == served) {
                Some((_, _, count)) => *count += 1,
                None => self.sessions.push((served.to_owned(), Vec::new(), 1)),
            }
        }
        self.cache = lines.into_cache();
        Ok(())
    }
    pub(super) fn finish(self, now: i64) -> Value {
        let mut sessions: Vec<Value> = self.sessions.into_iter().filter_map(|(id, fields, count)| {
            let row = self.rows.get(&id)?;
            Some(json!({"id":id,"name":row["name"],"repo":row.get("repo").cloned().unwrap_or(Value::Null),
                "last_activity":row["last_activity"],"fields":fields,"entries":count}))
        }).collect();
        sessions.sort_by(newest_first);
        json!({"now":now,"text":self.text,"matches":self.matches,"sessions":sessions,
            "scanned_bytes":self.cache.bytes,"truncated":self.stopped.is_some(),"truncated_by":self.stopped})
    }
}
