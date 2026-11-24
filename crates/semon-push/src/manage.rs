//! Receiver-local snapshot validation and immutable storage.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(crate) const ROUTE: &str = "/v1/manage/snapshot";
pub(crate) const LIMIT: usize = 256 * 1024;
const DIRECTORY: &str = "manage-snapshots";

macro_rules! object {
    ($name:ident { $($field:tt)* }) => {
        #[derive(Deserialize)]
        #[allow(dead_code, reason = "display fields are consumed by the frontend")]
        #[serde(deny_unknown_fields)]
        struct $name { $($field)* }
    };
}

macro_rules! variants {
    ($name:ident { $($item:ident),+ }) => {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        enum $name { $($item),+ }
    };
}

variants!(Activity {
    Running,
    Idle,
    Finished,
    Unknown
});
variants!(Health { Green, Amber, Red });
variants!(State {
    Active,
    Parked,
    Done
});
variants!(Unit { Usd, KiroCredits });
variants!(Gate { Ok, Gated });

// Display-only fields still pass through their typed deserializers. Only fields
// used by cross-field validation are read here.
object!(Snapshot {
    version: u32,
    generated_at: Time,
    chief_pm_checked_at: Time,
    decisions_waiting: Vec<Waiting>,
    decisions_today: Vec<Decision>,
    budgets: Vec<Budget>,
    lanes: Vec<Lane>,
    hosts: Vec<Host>,
});
object!(Waiting {
    id: Text,
    lane: Text,
    question: Text,
    recommendation: Text,
    waiting_since: Time,
});
object!(Decision {
    id: Text,
    lane: Text,
    summary: Text,
    decided_at: Time,
    link: Text
});
object!(Budget {
    id: Text, label: Text, spent: Amount, cap: Amount, unit: Unit,
    reset_at: Nullable<Time>, run_out_at: Nullable<Time>,
    #[serde(default)] breakdown: Vec<BudgetDetail>,
});
object!(BudgetDetail {
    #[serde(default, deserialize_with = "optional_text")] lane: Option<Text>,
    #[serde(default, deserialize_with = "optional_text")] agent: Option<Text>,
    #[serde(default, deserialize_with = "optional_text")] model: Option<Text>,
    spent: Amount,
});
object!(Lane {
    status: LaneStatus, alerts: Vec<Text>, health: Health, activity: Activity,
    last_update_at: Time, last_seen_at: Time, agents: Vec<Agent>,
});
object!(Agent { id: Text, activity: Activity, last_seen_at: Nullable<Time> });
object!(LaneStatus {
    #[serde(rename = "Lane")]
    lane: Text,
    #[serde(rename = "State")]
    state: State,
    #[serde(rename = "Goal")]
    goal: Text,
    #[serde(rename = "Next")]
    next: Text,
    #[serde(rename = "Blockers")]
    blockers: Text,
    #[serde(rename = "Decisions")]
    decisions: Text,
    #[serde(rename = "Spend")]
    spend: Text,
    #[serde(rename = "Links")]
    links: Text,
    #[serde(rename = "State-of-work")]
    state_of_work: Text,
});
object!(Host {
    host: Text,
    temperature_c: f64,
    gate: Gate,
    observed_at: Time
});

#[derive(Deserialize)]
struct Nullable<T>(Option<T>);
struct Text(String);
struct Amount;
struct Time(DateTime<Utc>);

fn optional_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Text>, D::Error> {
    Text::deserialize(d).map(Some)
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if value.is_empty() {
            return Err(de::Error::custom("invalid snapshot text"));
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if f64::deserialize(d)? < 0.0 {
            return Err(de::Error::custom("invalid amount"));
        }
        Ok(Self)
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if !value.ends_with('Z')
            || value.as_bytes().get(10) != Some(&b'T')
            || value.as_bytes().get(17..19) == Some(b"60")
        {
            return Err(de::Error::custom("expected UTC timestamp"));
        }
        DateTime::parse_from_rfc3339(&value)
            .map(|time| Self(time.to_utc()))
            .map_err(|_| de::Error::custom("invalid UTC timestamp"))
    }
}

fn looks_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("private key-----") {
        return true;
    }
    let mut words = lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    if words.any(|word| {
        word == "bearer"
            || [
                "ghp_",
                "gho_",
                "ghu_",
                "ghs_",
                "ghr_",
                "github_pat_",
                "sk-",
                "xoxb-",
                "xoxp-",
            ]
            .iter()
            .any(|prefix| {
                word.strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.len() >= 8)
            })
    }) {
        return true;
    }
    if [
        "api_key",
        "api-key",
        "apikey",
        "access_token",
        "access-token",
        "token",
        "password",
        "secret",
        "credential",
        "key",
    ]
    .iter()
    .any(|name| {
        lower.match_indices(name).any(|(at, _)| {
            (at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric())
                && lower[at + name.len()..]
                    .trim_start()
                    .starts_with(['=', ':'])
        })
    }) {
        return true;
    }
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .any(|word| {
            if word.len() < 32 {
                return false;
            }
            let hex = word.bytes().all(|b| b.is_ascii_hexdigit());
            // The lane handover uses Git SHA-1 references. Longer hex secrets, including
            // receiver bearers, are screened even inside a reference or URL.
            if hex && word.len() == 40 {
                let start = word.as_ptr() as usize - text.as_ptr() as usize;
                if text[..start]
                    .strip_suffix('@')
                    .and_then(|before| before.rsplit_once('#'))
                    .is_some_and(|(_, number)| {
                        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
                    })
                {
                    return false;
                }
            }
            let mut counts = [0usize; 128];
            for byte in word.bytes() {
                counts[byte as usize] += 1;
            }
            let entropy: f64 = counts
                .into_iter()
                .filter(|&n| n > 0)
                .map(|n| {
                    let p = n as f64 / word.len() as f64;
                    -p * p.log2()
                })
                .sum();
            entropy >= if hex { 3.25 } else { 4.2 }
        })
}

fn contains_secret(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            let mut current = text.clone();
            for _ in 0..4 {
                if looks_secret(&current) {
                    return true;
                }
                let plus_decoded = current.replace('+', " ");
                let decoded = percent_encoding::percent_decode_str(&plus_decoded)
                    .decode_utf8_lossy()
                    .into_owned();
                if decoded == current {
                    return false;
                }
                current = decoded;
            }
            looks_secret(&current)
        }
        Value::Array(items) => items.iter().any(contains_secret),
        Value::Object(fields) => fields.values().any(contains_secret),
        _ => false,
    }
}

#[derive(Debug)]
pub(crate) struct Invalid;

pub(crate) fn validate(body: &[u8], now: DateTime<Utc>) -> Result<(DateTime<Utc>, Value), Invalid> {
    if body.len() > LIMIT {
        return Err(Invalid);
    }
    let snapshot: Snapshot = serde_json::from_slice(body).map_err(|_| Invalid)?;
    let value: Value = serde_json::from_slice(body).map_err(|_| Invalid)?;
    fn object_shapes(value: &Value) -> bool {
        match value {
            Value::Object(fields) => fields.iter().all(|(name, value)| match value {
                Value::Array(items) => {
                    matches!(
                        name.as_str(),
                        "decisions_waiting"
                            | "decisions_today"
                            | "budgets"
                            | "breakdown"
                            | "lanes"
                            | "alerts"
                            | "agents"
                            | "hosts"
                    ) && items
                        .iter()
                        .all(|item| !item.is_array() && object_shapes(item))
                }
                _ => object_shapes(value),
            }),
            Value::Array(_) => false,
            _ => true,
        }
    }
    if !value.is_object() || !object_shapes(&value) || contains_secret(&value) {
        return Err(Invalid);
    }
    let generation = snapshot.generated_at.0;
    if snapshot.version != 1
        || generation > now + chrono::TimeDelta::minutes(5)
        || snapshot.chief_pm_checked_at.0 > generation
    {
        return Err(Invalid);
    }
    let mut decisions = HashSet::new();
    for decision in &snapshot.decisions_waiting {
        let Waiting {
            id,
            lane: _,
            question: _,
            recommendation: _,
            waiting_since,
        } = decision;
        if waiting_since.0 > generation || !decisions.insert(&id.0) {
            return Err(Invalid);
        }
    }
    for decision in &snapshot.decisions_today {
        let Decision {
            id,
            lane: _,
            summary: _,
            decided_at,
            link,
        } = decision;
        let url = reqwest::Url::parse(&link.0).map_err(|_| Invalid)?;
        let authority = link.0.strip_prefix("https://").ok_or(Invalid)?;
        if decided_at.0 > generation
            || !decisions.insert(&id.0)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || authority
                .split(['/', '?', '#'])
                .next()
                .is_some_and(|name| name.contains('@'))
            || link
                .0
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        {
            return Err(Invalid);
        }
    }
    let mut lanes = HashSet::new();
    let mut agents = HashMap::new();
    for lane in &snapshot.lanes {
        let Lane {
            status,
            alerts: _,
            health: _,
            activity: _,
            last_update_at,
            last_seen_at,
            agents: lane_agents,
        } = lane;
        let LaneStatus {
            lane,
            state: _,
            goal: _,
            next,
            blockers: _,
            decisions: _,
            spend: _,
            links: _,
            state_of_work: _,
        } = status;
        let (milestone, due) = next.0.rsplit_once(" · due ").ok_or(Invalid)?;
        if milestone.is_empty()
            || milestone.contains(['\n', '\r'])
            || due.len() != 17
            || !due.ends_with('Z')
            || chrono::NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%MZ").is_err()
            || last_seen_at.0 < last_update_at.0
            || last_seen_at.0 > generation
            || !lanes.insert(lane.0.as_str())
        {
            return Err(Invalid);
        }
        for agent in lane_agents {
            let Agent {
                id,
                activity: _,
                last_seen_at,
            } = agent;
            if last_seen_at
                .0
                .as_ref()
                .is_some_and(|time| time.0 > generation)
                || agents.insert(id.0.as_str(), lane.0.as_str()).is_some()
            {
                return Err(Invalid);
            }
        }
    }
    let mut budgets = HashSet::new();
    for budget in &snapshot.budgets {
        let Budget {
            id,
            label: _,
            spent: _,
            cap: _,
            unit: _,
            reset_at: _,
            run_out_at: _,
            breakdown,
        } = budget;
        if !budgets.insert(&id.0) {
            return Err(Invalid);
        }
        for detail in breakdown {
            let BudgetDetail {
                lane,
                agent,
                model: _,
                spent: _,
            } = detail;
            let lane = lane.as_ref().map(|text| text.0.as_str());
            let agent = agent.as_ref().map(|text| text.0.as_str());
            if (lane.is_none() && agent.is_none())
                || lane.is_some_and(|name| !lanes.contains(name))
                || agent.is_some_and(|id| {
                    agents
                        .get(id)
                        .is_none_or(|owner| lane.is_some_and(|name| name != *owner))
                })
            {
                return Err(Invalid);
            }
        }
    }
    let mut hosts = HashSet::new();
    for observation in &snapshot.hosts {
        let Host {
            host,
            temperature_c: _,
            gate: _,
            observed_at,
        } = observation;
        if observed_at.0 > generation || !hosts.insert(&host.0) {
            return Err(Invalid);
        }
    }
    // The typed decoder has already rejected duplicate/unknown fields and all
    // invalid values. Keep the original UTC strings and optional field presence.
    Ok((generation, value))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    received_at: String,
    credential_id: String,
    snapshot: Value,
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .mode(0o600);
    options
}

fn directory(dir: &Path) -> io::Result<PathBuf> {
    let path = dir.join(DIRECTORY);
    let meta = path.symlink_metadata()?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(path)
}

fn records(dir: &Path) -> io::Result<impl Iterator<Item = io::Result<(u64, String)>>> {
    Ok(fs::read_dir(dir)?.filter_map(|entry| match entry {
        Ok(entry) => {
            let name = entry.file_name().into_string().ok()?;
            let (stamp, sequence) = name.strip_suffix(".json")?.rsplit_once('-')?;
            DateTime::parse_from_rfc3339(stamp).ok()?;
            Some(Ok((sequence.parse().ok()?, name)))
        }
        Err(error) => Some(Err(error)),
    }))
}

pub(crate) fn append(
    root: &Path,
    credential_id: String,
    received_at: DateTime<Utc>,
    generation: DateTime<Utc>,
    snapshot: Value,
) -> io::Result<()> {
    match fs::DirBuilder::new()
        .mode(0o700)
        .create(root.join(DIRECTORY))
    {
        Ok(()) => File::open(root)?.sync_all()?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let dir = directory(root)?;
    let directory_file = private_options().read(true).open(&dir)?;
    // Readers and writers share this lock so no reader sees an entry before its
    // rename and directory sync have committed. The lock outlives each operation.
    directory_file.lock()?;
    // Another writer may have created the directory but not yet synced its parent.
    File::open(root)?.sync_all()?;
    let mut previous = 0;
    for record in records(&dir)? {
        previous = previous.max(record?.0);
    }
    let sequence = previous.checked_add(1).ok_or(io::ErrorKind::InvalidData)?;
    let stamp = generation.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let destination = dir.join(format!("{stamp}-{sequence:020}.json"));
    let temporary = dir.join(".pending");
    let mut file = private_options()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    let record = Record {
        received_at: received_at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        credential_id,
        snapshot,
    };
    serde_json::to_writer(&mut file, &record)?;
    file.flush()?;
    file.sync_all()?;
    fs::rename(&temporary, &destination)?;
    if let Err(error) = directory_file.sync_all() {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn latest(root: &Path) -> io::Result<Option<Value>> {
    let dir = match directory(root) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let lock = private_options().read(true).open(&dir)?;
    lock.lock_shared()?;
    let mut latest = None;
    for record in records(&dir)? {
        let (_, name) = record?;
        if latest.as_ref().is_none_or(|known| &name > known) {
            latest = Some(name);
        }
    }
    let Some(name) = latest else { return Ok(None) };
    let file = private_options().read(true).open(dir.join(name))?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.permissions().mode() & 0o077 != 0
        || meta.len() > (2 * LIMIT + 1024) as u64
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let record: Record = serde_json::from_reader(file.take((2 * LIMIT + 1024) as u64))?;
    Ok(Some(record.snapshot))
}

pub(crate) fn now() -> DateTime<Utc> {
    SystemTime::now().into()
}
