//! Codex 0.162.0 name observations. Never copy native databases into history.
use rusqlite::{Connection, OpenFlags, limits::Limit};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_NAMES: usize = 4096;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeName {
    pub harness: String,
    pub native_id: String,
    pub name: String,
    pub source: String,
    pub updated_at: Option<String>,
    pub observed_at: i64,
    pub freshness: String,
}
impl NativeName {
    pub(crate) fn valid(&self) -> bool {
        self.harness == "codex"
            && id_valid(&self.native_id)
            && name_valid(&self.name)
            && matches!(
                self.source.as_str(),
                "session_index.jsonl" | "state_5.sqlite:threads.name"
            )
            && matches!(self.freshness.as_str(), "current" | "stale")
    }
}
fn id_valid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn name_valid(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 1024 && !name.chars().any(char::is_control)
}
fn observation(id: String, name: String, source: &str, updated_at: Option<String>) -> NativeName {
    NativeName {
        harness: "codex".into(),
        native_id: id,
        name,
        source: source.into(),
        updated_at,
        observed_at: crate::model::now_ms(),
        freshness: "current".into(),
    }
}
pub(super) fn read(home: &Path) -> Vec<NativeName> {
    let mut names = BTreeMap::new();
    // A reverse bounded tail: partial first/final records cannot rename a session.
    if home
        .join("session_index.jsonl")
        .metadata()
        .is_ok_and(|m| m.is_file())
        && let Ok(mut file) = File::open(home.join("session_index.jsonl"))
        && let Ok(metadata) = file.metadata()
    {
        let offset = metadata.len().saturating_sub(MAX_BYTES);
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(offset)).is_ok()
            && file.take(MAX_BYTES).read_to_end(&mut bytes).is_ok()
        {
            let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            let first = if offset == 0 {
                0
            } else {
                bytes
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(complete, |i| i + 1)
            };
            for line in bytes[first..complete]
                .split(|b| *b == b'\n')
                .rev()
                .take(16_384)
            {
                if line.len() > 8192 {
                    continue;
                }
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                    continue;
                };
                let (Some(id), Some(name)) = (value["id"].as_str(), value["thread_name"].as_str())
                else {
                    continue;
                };
                if !id_valid(id) || !name_valid(name.trim()) {
                    continue;
                }
                names.entry(id.to_owned()).or_insert_with(|| {
                    observation(
                        id.into(),
                        name.trim().into(),
                        "session_index.jsonl",
                        value["updated_at"]
                            .as_str()
                            .filter(|s| s.len() <= 64)
                            .map(str::to_owned),
                    )
                });
                if names.len() >= MAX_NAMES {
                    break;
                }
            }
        }
    }
    read_database(home, &mut names);
    names.into_values().collect()
}
fn read_database(home: &Path, names: &mut BTreeMap<String, NativeName>) {
    let path = home.join("state_5.sqlite");
    if !path
        .metadata()
        .is_ok_and(|m| m.is_file() && m.len() <= 256 * 1024 * 1024)
    {
        return;
    }
    // Read-only WAL observation must not create a native sidecar.
    if home.join("state_5.sqlite-wal").exists() && !home.join("state_5.sqlite-shm").exists() {
        return;
    }
    let Ok(conn) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return;
    };
    let _ = conn.busy_timeout(Duration::from_millis(25));
    if conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, 8192).is_err() {
        return;
    }
    let started = Instant::now();
    if conn
        .progress_handler(
            1000,
            Some(move || started.elapsed() > Duration::from_millis(50)),
        )
        .is_err()
    {
        return;
    }
    // Older schemas without name fail closed; title is not a native session name.
    let Ok(mut query) = conn.prepare("SELECT id,name,updated_at FROM threads WHERE name IS NOT NULL AND length(name) BETWEEN 1 AND 1024 ORDER BY updated_at DESC LIMIT 4096") else { return; };
    let Ok(rows) = query.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    }) else {
        return;
    };
    for (id, name, at) in rows.flatten() {
        if names.len() >= MAX_NAMES {
            break;
        }
        if id_valid(&id) && name_valid(name.trim()) {
            names.entry(id.clone()).or_insert_with(|| {
                observation(
                    id,
                    name.trim().into(),
                    "state_5.sqlite:threads.name",
                    Some(at.to_string()),
                )
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    const ID: &str = "00000000-0000-0000-0000-000000000001";
    #[test]
    fn latest_index_name_and_database_fallback_are_read_only() {
        let home = std::env::temp_dir().join(format!("semon-names-{}", std::process::id()));
        fs::create_dir_all(&home).unwrap();
        let db = Connection::open(home.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT,name TEXT,title TEXT,updated_at INTEGER);")
            .unwrap();
        db.execute(
            "INSERT INTO threads VALUES(?1,'database name','prompt title',1)",
            [ID],
        )
        .unwrap();
        drop(db);
        let before = fs::read(home.join("state_5.sqlite")).unwrap();
        assert_eq!(read(&home)[0].name, "database name");
        let row = |name| {
            format!(
                "{{\"id\":\"{ID}\",\"thread_name\":\"{name}\",\"updated_at\":\"2026-10-10T00:00:00Z\"}}\n"
            )
        };
        fs::write(
            home.join("session_index.jsonl"),
            row("old") + &row("renamed") + "{incomplete",
        )
        .unwrap();
        let names = read(&home);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].name, "renamed");
        assert!(names[0].valid());
        assert_eq!(before, fs::read(home.join("state_5.sqlite")).unwrap());
        assert!(!home.join("state_5.sqlite-wal").exists());
        fs::remove_dir_all(home).unwrap();
    }
}
