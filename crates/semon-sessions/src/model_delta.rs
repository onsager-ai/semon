//! Opt-in bounded model deltas. Versions outside this core's small history
//! receive a full model, so reconnects and restarts need no special recovery.
use serde_json::{Map, Value, json};
use std::collections::VecDeque;

const MAX_VERSIONS: usize = 8;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_MODEL: usize = 8 * 1024 * 1024;
const EXTRAS: [&str; 3] = ["admin", "account", "nav"];

#[derive(Default)]
pub(crate) struct History {
    models: VecDeque<(String, Vec<u8>)>,
    bytes: usize,
}

impl History {
    pub(crate) fn answer(&mut self, body: &[u8], since: Option<&str>, enabled: bool) -> Vec<u8> {
        let Ok(mut current) = serde_json::from_slice::<Value>(body) else {
            return body.to_vec();
        };
        let Some(version) = current["version"].as_str().map(str::to_owned) else {
            return body.to_vec();
        };
        let mut extras = Map::new();
        for field in EXTRAS {
            if let Some(value) = current.as_object_mut().unwrap().remove(field) {
                extras.insert(field.into(), value);
            }
        }
        let delta = enabled
            .then(|| {
                since.and_then(|since| {
                    self.models
                        .iter()
                        .find(|(version, _)| version == since)
                        .and_then(|(_, old)| serde_json::from_slice(old).ok())
                        .map(|old| diff(&old, &current, since, &extras))
                })
            })
            .flatten();
        if body.len() <= MAX_MODEL && !self.models.iter().any(|(seen, _)| seen == &version) {
            while self.models.len() >= MAX_VERSIONS || self.bytes + body.len() > MAX_BYTES {
                let Some((_, bytes)) = self.models.pop_front() else {
                    break;
                };
                self.bytes -= bytes.len();
            }
            let bytes = serde_json::to_vec(&current).expect("parsed JSON serializes");
            self.bytes += bytes.len();
            self.models.push_back((version, bytes));
        }
        delta
            .and_then(|value| serde_json::to_vec(&value).ok())
            .filter(|delta| delta.len() < body.len())
            .unwrap_or_else(|| body.to_vec())
    }
}

fn indexed(value: &Value, array: bool) -> Map<String, Value> {
    if array {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["id"].as_str().map(|id| (id.into(), row.clone())))
            .collect()
    } else {
        value.as_object().cloned().unwrap_or_default()
    }
}

fn diff(old: &Value, current: &Value, from: &str, extras: &Map<String, Value>) -> Value {
    let mut set = Map::new();
    let mut remove = Vec::new();
    let mut collections = Map::new();
    for (field, value) in current.as_object().unwrap() {
        let array = matches!(field.as_str(), "handoffs" | "turns");
        if array || matches!(field.as_str(), "sessions" | "tx") {
            let before = indexed(&old[field], array);
            let after = indexed(value, array);
            let changed: Map<_, _> = after
                .iter()
                .filter(|(id, row)| before.get(*id) != Some(*row))
                .map(|(id, row)| (id.clone(), row.clone()))
                .collect();
            let deleted: Vec<_> = before
                .keys()
                .filter(|id| !after.contains_key(*id))
                .cloned()
                .collect();
            let mut patch = json!({"set":changed,"remove":deleted});
            if array {
                let order = |value: &Value| {
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|row| row["id"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                };
                if order(&old[field]) != order(value) {
                    patch["order"] = json!(order(value));
                }
            }
            if !changed.is_empty() || !deleted.is_empty() || patch.get("order").is_some() {
                collections.insert(field.clone(), patch);
            }
        } else if old.get(field) != Some(value) {
            set.insert(field.clone(), value.clone());
        }
    }
    for field in old.as_object().unwrap().keys() {
        if current.get(field).is_none() {
            remove.push(field.clone());
        }
    }
    // These fields belong to this request, never to a shared historical model.
    for field in EXTRAS {
        if let Some(value) = extras.get(field) {
            set.insert(field.into(), value.clone());
        } else {
            remove.push(field.into());
        }
    }
    json!({"delta":1,"from":from,"version":current["version"],"set":set,"remove":remove,"collections":collections})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(mut old: Value, delta: &Value) -> Value {
        for field in delta["remove"].as_array().unwrap() {
            old.as_object_mut().unwrap().remove(field.as_str().unwrap());
        }
        for (field, value) in delta["set"].as_object().unwrap() {
            old[field] = value.clone();
        }
        for (field, patch) in delta["collections"].as_object().unwrap() {
            let array = matches!(field.as_str(), "handoffs" | "turns");
            let mut rows = indexed(&old[field], array);
            for id in patch["remove"].as_array().unwrap() {
                rows.remove(id.as_str().unwrap());
            }
            for (id, value) in patch["set"].as_object().unwrap() {
                rows.insert(id.clone(), value.clone());
            }
            old[field] = if array {
                let order = patch.get("order").cloned().unwrap_or_else(|| {
                    json!(
                        old[field]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|row| row["id"].clone())
                            .collect::<Vec<_>>()
                    )
                });
                Value::Array(
                    order
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|id| rows.remove(id.as_str().unwrap()).unwrap())
                        .collect(),
                )
            } else {
                Value::Object(rows)
            };
        }
        old["version"] = delta["version"].clone();
        old
    }

    fn fixture(version: &str) -> Value {
        let sessions: Map<_, _> = (0..50).map(|id| (format!("s{id}"), json!({"name":format!("Session{id}"),"activity":["working", "Bash", 3],"signals":{"question":1}}))).collect();
        json!({"version":version,"now":1,"sessions":sessions,"tx":{"s1":"mark"},"handoffs":[{"id":"h1","status":"wait"},{"id":"h2","status":"done"}],"turns":[{"id":"t1"},{"id":"t2"}],"machines":[{"id":"m1","up":true}],"busy":[1,2]})
    }

    #[test]
    fn delta_round_trip_preserves_deletions_order_signals_and_machine_changes() {
        let old = fixture("v1");
        let mut current = fixture("v2");
        current["sessions"].as_object_mut().unwrap().remove("s2");
        current["sessions"]["s1"]["signals"] = json!({"question":0,"permission":2});
        current["tx"]["s1"] = json!("grown");
        current["handoffs"] = json!([{"id":"h2","status":"done"},{"id":"h3","status":"new"}]);
        current["turns"] = json!([{"id":"t2"},{"id":"t1","end":{"why":"answer"}}]);
        current["machines"][0]["up"] = json!(false);
        current["now"] = json!(2);
        let mut history = History::default();
        let encode = |value: &Value| serde_json::to_vec(value).unwrap();
        history.answer(&encode(&old), None, true);
        let body = history.answer(&encode(&current), Some("v1"), true);
        assert!(body.len() < encode(&current).len() / 2);
        let delta: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(delta["delta"], 1);
        assert_eq!(apply(old.clone(), &delta), current);
        assert_eq!(
            history.answer(&encode(&current), Some("missing"), true),
            encode(&current)
        );
        assert_eq!(
            history.answer(&encode(&current), Some("v1"), false),
            encode(&current)
        );
    }

    #[test]
    fn history_expires_and_never_retains_request_account_values() {
        let mut history = History::default();
        let mut old = fixture("v0");
        old["account"] = json!({"name":"private-account"});
        history.answer(&serde_json::to_vec(&old).unwrap(), None, true);
        assert!(!String::from_utf8_lossy(&history.models[0].1).contains("private-account"));
        for i in 1..=MAX_VERSIONS {
            let current = fixture(&format!("v{i}"));
            history.answer(&serde_json::to_vec(&current).unwrap(), None, true);
        }
        assert_eq!(history.models.len(), MAX_VERSIONS);
        assert!(history.bytes <= MAX_BYTES);
        let current = fixture("next");
        let body = serde_json::to_vec(&current).unwrap();
        assert_eq!(history.answer(&body, Some("v0"), true), body);
        let delta: Value =
            serde_json::from_slice(&history.answer(&body, Some("v8"), true)).unwrap();
        assert!(
            delta["remove"]
                .as_array()
                .unwrap()
                .contains(&json!("account"))
        );
    }
}
