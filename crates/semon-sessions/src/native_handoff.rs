//! Bounded launch-call locations; inputs are read from native evidence on demand.
use crate::{events::Ledger, handoff, native_record::Record, sealed::LogFile};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    ops::Range,
    path::Path,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Index {
    pub(crate) complete: VecDeque<CallRef>,
    pub(crate) eof: VecDeque<CallRef>,
    pub(crate) eof_record: Option<Record>,
    pub(crate) complete_bytes: usize,
    pub(crate) eof_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallRef {
    record: Record,
    fields: [Record; 3],
    time: Option<i64>,
    budget: usize,
}

fn cap(calls: &mut VecDeque<CallRef>, bytes: &mut usize) {
    while calls.len() > handoff::MAX_CALLS || *bytes > handoff::MAX_INPUT_BYTES {
        *bytes -= calls
            .pop_front()
            .expect("over-budget calls are nonempty")
            .budget;
    }
}

impl Index {
    pub(crate) fn begin_append(&mut self) {
        self.eof.clear();
        self.eof_record = None;
        self.eof_bytes = 0;
    }

    pub(crate) fn observe(
        &mut self,
        bytes: &[u8],
        offset: u64,
        complete: bool,
        value: &Value,
    ) -> io::Result<()> {
        if bytes.len() > handoff::MAX_INPUT_BYTES
            || ![b"Semon-Handoff".as_slice(), b"codex".as_slice()]
                .iter()
                .any(|needle| bytes.windows(needle.len()).any(|part| part == *needle))
        {
            return Ok(());
        }
        let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
            return Ok(());
        };
        let Some(message) = members(bytes, 0..bytes.len())?.remove("message") else {
            return Ok(());
        };
        let Some(content) = members(bytes, message)?.remove("content") else {
            return Ok(());
        };
        let ranges = elements(bytes, content)?;
        let record = Record::capture(bytes, offset);
        let calls = if complete {
            &mut self.complete
        } else {
            &mut self.eof
        };
        let input_bytes = if complete {
            &mut self.complete_bytes
        } else {
            &mut self.eof_bytes
        };
        for (block, range) in blocks.iter().zip(ranges) {
            if crate::field(block, "type") != Some("tool_use") {
                continue;
            }
            let Some(name @ ("Bash" | "Skill")) = crate::field(block, "name") else {
                continue;
            };
            let (Some(id), Some(input)) = (crate::field(block, "id"), block.get("input")) else {
                continue;
            };
            let budget = input
                .to_string()
                .len()
                .saturating_add(id.len())
                .saturating_add(name.len());
            if budget > handoff::MAX_INPUT_BYTES {
                continue;
            }
            let fields = members(bytes, range)?;
            let mut locations = Vec::new();
            for key in ["id", "name", "input"] {
                let range = fields.get(key).ok_or_else(invalid)?;
                locations.push(Record::capture(
                    &bytes[range.clone()],
                    offset + range.start as u64,
                ));
            }
            calls.push_back(CallRef {
                record: record.clone(),
                fields: locations.try_into().expect("three named fields"),
                time: crate::events::record_time(value),
                budget,
            });
            *input_bytes += budget;
            cap(calls, input_bytes);
        }
        if !complete && !calls.is_empty() {
            self.eof_record = Some(Record::capture(bytes, offset));
        }
        Ok(())
    }

    pub(crate) fn repair(file: &mut LogFile, consumed: u64) -> io::Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(file.take(consumed));
        let mut index = Self::default();
        let mut offset = 0;
        let mut bytes = Vec::new();
        let mut discarding = false;
        loop {
            let read = reader
                .by_ref()
                .take(handoff::MAX_INPUT_BYTES as u64 + 1)
                .read_until(b'\n', &mut bytes)?;
            if read == 0 {
                break;
            }
            let complete = bytes.last() == Some(&b'\n');
            if discarding || bytes.len() > handoff::MAX_INPUT_BYTES {
                discarding = !complete;
            } else if complete && let Ok(value) = serde_json::from_slice(&bytes) {
                index.observe(&bytes, offset, true, &value)?;
            }
            offset += bytes.len() as u64;
            bytes.clear();
            if discarding {
                bytes.shrink_to(8192);
            }
        }
        Ok(index)
    }

    pub(crate) fn calls(&self) -> VecDeque<&CallRef> {
        let mut calls: VecDeque<_> = self.complete.iter().chain(&self.eof).collect();
        let mut bytes: usize = calls.iter().map(|call| call.budget).sum();
        while calls.len() > handoff::MAX_CALLS || bytes > handoff::MAX_INPUT_BYTES {
            bytes -= calls
                .pop_front()
                .expect("over-budget calls are nonempty")
                .budget;
        }
        calls
    }

    pub(crate) fn valid(&self, ledger: &Ledger) -> bool {
        let valid = |calls: &VecDeque<CallRef>, eof: bool| {
            calls.len() <= handoff::MAX_CALLS
                && calls
                    .iter()
                    .try_fold(0usize, |bytes, call| bytes.checked_add(call.budget))
                    .is_some_and(|bytes| bytes <= handoff::MAX_INPUT_BYTES)
                && calls.iter().all(|call| call.valid(ledger, eof))
                && calls
                    .iter()
                    .zip(calls.iter().skip(1))
                    .all(|(a, b)| a.fields[0].offset < b.fields[0].offset)
        };
        valid(&self.complete, false)
            && valid(&self.eof, true)
            && self.complete.iter().map(|call| call.budget).sum::<usize>() == self.complete_bytes
            && self.eof.iter().map(|call| call.budget).sum::<usize>() == self.eof_bytes
            && (if self.eof.is_empty() {
                self.eof_record.is_none()
            } else {
                self.eof_record.as_ref().is_some_and(|record| {
                    record.offset == ledger.offset && record.end() == Some(ledger.stat.size)
                })
            })
    }

    pub(crate) fn verify(&self, file: &mut LogFile) -> io::Result<()> {
        let mut checked = std::collections::BTreeSet::new();
        for call in self.complete.iter().chain(&self.eof) {
            if checked.insert(&call.record) {
                call.record.verify(file)?;
            }
        }
        if let Some(record) = &self.eof_record
            && checked.insert(record)
        {
            record.verify(file)?;
        }
        Ok(())
    }
}

impl CallRef {
    pub(crate) fn record(&self) -> &Record {
        &self.record
    }

    pub(crate) fn valid(&self, ledger: &Ledger, eof: bool) -> bool {
        self.budget > 0
            && self.budget <= handoff::MAX_INPUT_BYTES
            && self
                .record
                .end()
                .and_then(|end| end.checked_sub(self.record.offset))
                .is_some_and(|length| length > 0 && length <= handoff::MAX_INPUT_BYTES as u64)
            && (if eof {
                self.record.offset == ledger.offset && self.record.end() == Some(ledger.stat.size)
            } else {
                self.record.end().is_some_and(|end| end <= ledger.offset)
            })
            && self.fields.iter().all(|field| {
                field.offset >= self.record.offset
                    && field
                        .end()
                        .is_some_and(|end| self.record.end().is_some_and(|limit| end <= limit))
            })
    }

    pub(crate) fn read(
        &self,
        path: &Path,
        records: &crate::query_source::Records<'_>,
    ) -> io::Result<handoff::Call> {
        let id: String = serde_json::from_slice(&records.evidence(path, &self.fields[0])?)?;
        let name: String = serde_json::from_slice(&records.evidence(path, &self.fields[1])?)?;
        let input: Value = serde_json::from_slice(&records.evidence(path, &self.fields[2])?)?;
        let input = input.to_string();
        if !matches!(name.as_str(), "Bash" | "Skill")
            || input
                .len()
                .saturating_add(id.len())
                .saturating_add(name.len())
                != self.budget
        {
            return Err(invalid());
        }
        Ok(handoff::Call {
            tool: crate::ToolCall { id, name },
            time: self.time,
            input,
        })
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid native launch location")
}
fn ws(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).is_some_and(u8::is_ascii_whitespace) {
        *at += 1;
    }
}
fn value_end(bytes: &[u8], at: usize) -> io::Result<usize> {
    let mut values = serde_json::Deserializer::from_slice(&bytes[at..]).into_iter::<IgnoredAny>();
    values.next().ok_or_else(invalid)??;
    Ok(at + values.byte_offset())
}

/// Locate only native object members using serde's parser. Last duplicate keys
/// win exactly as in Value; unrelated keys and values are never persisted.
fn members(bytes: &[u8], range: Range<usize>) -> io::Result<BTreeMap<String, Range<usize>>> {
    let mut at = range.start;
    ws(bytes, &mut at);
    if bytes.get(at) != Some(&b'{') {
        return Err(invalid());
    }
    at += 1;
    let mut fields = BTreeMap::new();
    loop {
        ws(bytes, &mut at);
        if bytes.get(at) == Some(&b'}') {
            break;
        }
        let mut keys = serde_json::Deserializer::from_slice(&bytes[at..]).into_iter::<String>();
        let key = keys.next().ok_or_else(invalid)??;
        at += keys.byte_offset();
        ws(bytes, &mut at);
        if bytes.get(at) != Some(&b':') {
            return Err(invalid());
        }
        at += 1;
        ws(bytes, &mut at);
        let end = value_end(bytes, at)?;
        if matches!(
            key.as_str(),
            "message" | "content" | "id" | "name" | "input"
        ) {
            fields.insert(key, at..end);
        }
        at = end;
        ws(bytes, &mut at);
        if bytes.get(at) == Some(&b',') {
            at += 1;
        } else {
            break;
        }
    }
    if at >= range.end || bytes.get(at) != Some(&b'}') {
        return Err(invalid());
    }
    Ok(fields)
}

fn elements(bytes: &[u8], range: Range<usize>) -> io::Result<Vec<Range<usize>>> {
    let mut at = range.start;
    ws(bytes, &mut at);
    if bytes.get(at) != Some(&b'[') {
        return Err(invalid());
    }
    at += 1;
    let mut values = Vec::new();
    loop {
        ws(bytes, &mut at);
        if bytes.get(at) == Some(&b']') {
            break;
        }
        let end = value_end(bytes, at)?;
        values.push(at..end);
        at = end;
        ws(bytes, &mut at);
        if bytes.get(at) == Some(&b',') {
            at += 1;
        } else {
            break;
        }
    }
    if at >= range.end || bytes.get(at) != Some(&b']') {
        return Err(invalid());
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Stat;
    use serde_json::json;

    fn ledger(size: u64, offset: u64) -> Ledger {
        Ledger {
            stat: Stat {
                dev: 0,
                ino: 0,
                size,
                modified_ns: 0,
                changed_ns: Some(0),
            },
            offset,
            prefix: [0; 32],
            tail: [0; 32],
        }
    }

    #[test]
    fn native_handoff_locations_follow_escaped_and_duplicate_keys_and_preserve_tool_filters() {
        let bytes = br#"{"message":{"content":[]},"mess\u0061ge":{"content":[{"type":"tool_use","id":"old","id":"c\u0061ll","name":"Bash","input":{"command":"ignored"},"input":{"command":"codex < /private/SECRET","\u0000":"Semon-Handoff"}},{"type":"tool_use","id":"ignored","name":"Read","input":"codex"},{"type":"tool_use","id":"skill","name":"Skill","input":{"args":"codex"}}]}}
"#;
        let value: Value = serde_json::from_slice(bytes).unwrap();
        let mut index = Index::default();
        index.observe(bytes, 0, true, &value).unwrap();
        assert!(index.valid(&ledger(bytes.len() as u64, bytes.len() as u64)));
        assert_eq!(index.calls().len(), 2);
        for (location, expected) in index.calls().iter().zip(["call", "skill"]) {
            let field = &location.fields[0];
            let id: String = serde_json::from_slice(
                &bytes[field.offset as usize..field.end().unwrap() as usize],
            )
            .unwrap();
            assert_eq!(id, expected);
        }
        let encoded = serde_json::to_string(&index).unwrap();
        assert!(!encoded.contains("SECRET") && !encoded.contains("private"));
    }

    #[test]
    fn native_handoff_caps_and_provisional_eof_preserve_complete_candidates() {
        let mut index = Index::default();
        let mut offset = 0;
        for id in 0..handoff::MAX_CALLS + 7 {
            let value = json!({"message":{"content":[{"type":"tool_use","id":id.to_string(),"name":"Bash","input":"codex"}]}});
            let bytes = format!("{value}\n");
            index
                .observe(bytes.as_bytes(), offset, true, &value)
                .unwrap();
            offset += bytes.len() as u64;
        }
        assert_eq!(index.complete.len(), handoff::MAX_CALLS);
        assert!(index.valid(&ledger(offset, offset)));
        let first = index.complete.front().unwrap().record.clone();
        let value = json!({"message":{"content":[{"type":"tool_use","id":"eof","name":"Skill","input":"codex"}]}});
        let bytes = value.to_string();
        index
            .observe(bytes.as_bytes(), offset, false, &value)
            .unwrap();
        assert!(index.valid(&ledger(offset + bytes.len() as u64, offset)));
        assert_eq!(index.calls().len(), handoff::MAX_CALLS);
        assert_ne!(index.calls()[0].record, first);
        index.begin_append();
        assert_eq!(index.calls()[0].record, first);
        assert!(index.valid(&ledger(offset, offset)));
        let value = json!({"message":{"content":[{"type":"tool_use","id":"large","name":"Bash","input":format!("codex {}", "x".repeat(handoff::MAX_INPUT_BYTES / 2))}]}});
        let bytes = format!("{value}\n");
        index
            .observe(bytes.as_bytes(), offset, true, &value)
            .unwrap();
        offset += bytes.len() as u64;
        index
            .observe(bytes.as_bytes(), offset, true, &value)
            .unwrap();
        offset += bytes.len() as u64;
        assert_eq!(index.calls().len(), 1);
        assert!(index.valid(&ledger(offset, offset)));
    }
}
