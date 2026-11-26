use super::{Index, sql_error};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    time::{Duration, Instant},
};

#[derive(Deserialize, Serialize)]
pub(crate) struct Candidate {
    pub(crate) owner: i64,
    pub(crate) catalog_key: String,
    session_uid: String,
    card_posted_at: Option<i64>,
    last_message_at: Option<i64>,
    #[serde(flatten)]
    fields: Fields,
}

const FIELD_CHARS: usize = 300;

/// The allowlisted card projection carried in full pushed facts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamCard {
    session_uid: String,
    #[serde(deserialize_with = "bounded_text")]
    title: String,
    state: Option<State>,
    last_message_at: Option<i64>,
    card_posted_at: Option<i64>,
    #[serde(deserialize_with = "bounded_text")]
    stream: String,
    #[serde(deserialize_with = "bounded_text")]
    lane_goal: String,
    #[serde(deserialize_with = "bounded_text")]
    number: String,
    #[serde(deserialize_with = "bounded_text")]
    now: String,
    #[serde(deserialize_with = "bounded_text")]
    next_owner_decision: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Active,
    Idle,
    Ended,
}

fn bounded_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.chars().nth(FIELD_CHARS).is_some() {
        return Err(de::Error::custom("stream card text exceeds 300 characters"));
    }
    Ok(value)
}

impl StreamCard {
    /// Apply the sender's text redaction to the allowlisted text values.
    pub fn redact(&mut self, mut redact: impl FnMut(&mut [u8])) {
        for text in [
            &mut self.title,
            &mut self.stream,
            &mut self.lane_goal,
            &mut self.number,
            &mut self.now,
            &mut self.next_owner_decision,
        ] {
            let mut bytes = std::mem::take(text).into_bytes();
            redact(&mut bytes);
            *text = String::from_utf8(bytes).expect("push redaction preserves UTF-8");
        }
    }

    pub(crate) fn with_host(self, host: &str, offline: bool) -> Value {
        let mut row = json!(self);
        row["host"] = json!(host);
        if offline {
            row["state"] = json!("offline");
        }
        row
    }
}

/// Read the same qualified cards as the local query for an opted-in push.
/// An origin that has not published analytics has no cards to send yet.
pub fn collect(options: &crate::Options) -> Result<Vec<StreamCard>, Value> {
    super::admit(if options.facts.is_some() {
        super::Access::Receiver
    } else {
        super::Access::Origin
    })?;
    let deadline = Instant::now() + Duration::from_millis(super::TIMEOUT_MS);
    let path = crate::analytics_index_path(&options.cache);
    let index = match super::open(&path, deadline) {
        Ok(index) => index,
        Err(error) if error["error"]["code"] == "not_materialized" => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let cards = read(&index, deadline)?;
    drop(index);
    qualify(options, cards)
}

pub(crate) fn qualify(
    options: &crate::Options,
    cards: Vec<Candidate>,
) -> Result<Vec<StreamCard>, Value> {
    if cards.is_empty() {
        return Ok(Vec::new());
    }
    crate::catalog::warehouse::qualify_stream_cards(options, cards).map_err(|_| {
        json!({"error":{"code":"store_unavailable","message":"Stream card evidence unavailable"}})
    })
}

#[derive(Deserialize, Serialize)]
struct Fields {
    stream: String,
    lane_goal: String,
    number: String,
    now: String,
    next_owner_decision: String,
}

fn parse(text: &str) -> Option<Fields> {
    let mut lines = text.lines();
    let mut field = |label| {
        let value = lines.next()?.strip_prefix(label)?.trim();
        (!value.is_empty()).then(|| capped_text(value))
    };
    let fields = Fields {
        stream: field("Stream:")?,
        lane_goal: field("Lane goal:")?,
        number: field("Number:")?,
        now: field("Now:")?,
        next_owner_decision: field("Next owner decision:")?,
    };
    lines.next().is_none().then_some(fields)
}

fn capped_text(value: &str) -> String {
    value
        .trim()
        .chars()
        .take(FIELD_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

impl Candidate {
    pub(crate) fn finish(self, title: &str, state: Option<State>) -> StreamCard {
        StreamCard {
            session_uid: self.session_uid,
            title: capped_text(title),
            state,
            last_message_at: self.last_message_at,
            card_posted_at: self.card_posted_at,
            stream: self.fields.stream,
            lane_goal: self.fields.lane_goal,
            number: self.fields.number,
            now: self.fields.now,
            next_owner_decision: self.fields.next_owner_decision,
        }
    }
}

pub(super) fn read(index: &Index, deadline: Instant) -> Result<Vec<Candidate>, Value> {
    let db = &index.connection;
    let fail = |error| sql_error(error, deadline, false);
    // Match each body to a committed local source and the source's current
    // lineage owner. In particular, a copied body cannot grant root status
    // to a Claude sidecar or a Codex spawn with a missing parent log.
    let mut candidates = db.prepare(include_str!("stream_cards.sql")).map_err(fail)?;
    let mut rows = candidates.query([]).map_err(fail)?;
    let mut last_message = db
        .prepare(
            "SELECT max(b.ts) FROM lineage_member root
             JOIN lineage_member m USING(component)
             JOIN warehouse_current c ON c.source=m.source
             JOIN warehouse_source s ON s.source=c.source AND s.origin='local' AND s.carrier=c.path
             JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
             JOIN body_source bs ON bs.source_path=c.path
             JOIN body b USING(body_uid)
             WHERE root.source=?1 AND b.part IN ('user','assistant')",
        )
        .map_err(fail)?;
    let mut latest = BTreeMap::new();
    while let Some(row) = rows.next().map_err(fail)? {
        let owner: i64 = row.get(0).map_err(fail)?;
        let Entry::Vacant(entry) = latest.entry(owner) else {
            continue;
        };
        let text: String = row.get(5).map_err(fail)?;
        let Some(fields) = parse(&text) else {
            continue;
        };
        let harness: String = row.get(2).map_err(fail)?;
        let native_id: String = row.get(3).map_err(fail)?;
        let session_uid = format!(
            "{}/{}/{}",
            index.namespace,
            harness,
            super::encode_component(&native_id)
        );
        entry.insert(Candidate {
            owner,
            catalog_key: row.get(1).map_err(fail)?,
            session_uid,
            card_posted_at: row.get(4).map_err(fail)?,
            last_message_at: last_message
                .query_row([owner], |row| row.get(0))
                .map_err(fail)?,
            fields,
        });
    }
    let mut cards = latest.into_values().collect::<Vec<_>>();
    cards.sort_unstable_by(|a, b| a.session_uid.cmp(&b.session_uid));
    Ok(cards)
}
