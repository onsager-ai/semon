//! Allowlisted Claude subagent metadata with an independent source observation.
use crate::{events::Stat, field};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentMetadata {
    pub(crate) tool_use_id: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) branch: Option<String>,
}

impl AgentMetadata {
    fn from_record(record: &Value) -> Self {
        Self {
            tool_use_id: field(record, "toolUseId").map(str::to_owned),
            label: field(record, "description")
                .or_else(|| field(record, "agentType"))
                .map(str::to_owned),
            model: field(record, "model").map(str::to_owned),
            cwd: field(record, "worktreePath").map(str::to_owned),
            branch: field(record, "worktreeBranch").map(str::to_owned),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Observation {
    Absent,
    Present { stat: String, sha256: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaudeSidecar {
    observation: Observation,
    pub(crate) metadata: AgentMetadata,
}

fn fingerprint(stat: &Stat) -> String {
    let mut digest = Sha256::new();
    for value in [stat.dev, stat.ino, stat.size] {
        digest.update(value.to_be_bytes());
    }
    digest.update(stat.modified_ns.to_be_bytes());
    digest.update([u8::from(stat.changed_ns.is_some())]);
    if let Some(changed) = stat.changed_ns {
        digest.update(changed.to_be_bytes());
    }
    format!("{:x}", digest.finalize())
}

pub(crate) fn agent_path(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "subagents")
        && path
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("agent-"))
        && path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
}

fn sidecar_path(path: &Path) -> PathBuf {
    path.with_extension("meta.json")
}

impl ClaudeSidecar {
    pub(crate) fn valid(&self) -> bool {
        let hash =
            |text: &str| text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
        match &self.observation {
            Observation::Absent => self.metadata == AgentMetadata::default(),
            Observation::Present { stat, sha256 } => hash(stat) && hash(sha256),
        }
    }

    /// Safely open every ancestor and the sidecar. A known absence is reusable;
    /// an unreadable, oversized or linked input is ignored as before, but cannot
    /// become a persisted proof or authorize eviction. The raw JSON is discarded.
    pub(crate) fn observe(path: &Path, previous: Option<&Self>) -> io::Result<Option<Self>> {
        let path = sidecar_path(path);
        let mut file = match crate::open_input(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Some(Self {
                    observation: Observation::Absent,
                    metadata: AgentMetadata::default(),
                }));
            }
            Err(_) => return Ok(None),
        };
        let stat = match file
            .metadata()
            .and_then(|metadata| Stat::from_metadata(&metadata))
        {
            Ok(stat) => stat,
            Err(_) => return Ok(None),
        };
        let stamp = fingerprint(&stat);
        if stat.changed_ns.is_some()
            && let Some(previous) = previous
            && matches!(&previous.observation, Observation::Present { stat, .. } if stat == &stamp)
        {
            return Ok(Some(previous.clone()));
        }
        if stat.size > crate::RECORD_MAX {
            crate::warn_record_too_large(&path);
            return Ok(None);
        }
        let mut bytes = Vec::new();
        #[cfg(test)]
        crate::events::CLAUDE_SIDECAR_READS.with(|reads| reads.set(reads.get() + 1));
        if (&mut file)
            .take(crate::RECORD_MAX + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Ok(None);
        }
        if bytes.len() as u64 > crate::RECORD_MAX {
            crate::warn_record_too_large(&path);
            return Ok(None);
        }
        if file
            .metadata()
            .and_then(|metadata| Stat::from_metadata(&metadata))
            .ok()
            != Some(stat)
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Claude sidecar changed while indexing",
            ));
        }
        let metadata = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(Value::is_object)
            .as_ref()
            .map(AgentMetadata::from_record)
            .unwrap_or_default();
        Ok(Some(Self {
            observation: Observation::Present {
                stat: stamp,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            },
            metadata,
        }))
    }

    /// Archive custody verifies the whole bounded metadata file, including on
    /// platforms whose physical timestamps cannot distinguish a quick rewrite.
    pub(crate) fn verified(&self, path: &Path) -> bool {
        Self::observe(path, None).is_ok_and(|observed| observed.as_ref() == Some(self))
            && self.current(path)
    }

    /// Recheck the independent physical observation without following links.
    /// Platforms without change time compare the full bounded sidecar instead.
    pub(crate) fn current(&self, path: &Path) -> bool {
        match crate::open_input(&sidecar_path(path)) {
            Ok(file) => match &self.observation {
                Observation::Present { stat: expected, .. } => {
                    let Ok(stat) = file
                        .metadata()
                        .and_then(|metadata| Stat::from_metadata(&metadata))
                    else {
                        return false;
                    };
                    if &fingerprint(&stat) != expected {
                        return false;
                    }
                    if stat.changed_ns.is_some() {
                        return true;
                    }
                    Self::observe(path, None).is_ok_and(|observed| observed.as_ref() == Some(self))
                }
                Observation::Absent => false,
            },
            Err(error) => {
                error.kind() == io::ErrorKind::NotFound
                    && matches!(self.observation, Observation::Absent)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Query, SessionSources};
    use rusqlite::{Connection, OptionalExtension};
    use serde_json::json;
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const NOW: i64 = 1_790_208_000_000;
    const RELATIVE: &str = "projects/project/lead/subagents/agent-sub.jsonl";
    struct Home {
        root: PathBuf,
        path: PathBuf,
        meta: PathBuf,
        options: Options,
    }
    impl Home {
        fn new(metadata: Option<Value>) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-native-sidecar-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let options = Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                facts: None,
                ..Options::default()
            };
            let path = options.claude_home.join(RELATIVE);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let parent = json!({"type":"user", "timestamp":"2026-09-24T00:00:00Z", "sessionId":"lead", "message":{"role":"user","content":"parent evidence"}});
            fs::write(
                options.claude_home.join("projects/project/lead.jsonl"),
                format!("{parent}\n"),
            )
            .unwrap();
            let record = json!({"type":"user", "timestamp":"2026-09-24T00:00:01Z", "sessionId":"lead", "agentId":"sub", "isSidechain":true,
                "message":{"role":"user","content":"PRIVATE-NATIVE-BODY"}});
            fs::write(&path, format!("{record}\n")).unwrap();
            let meta = sidecar_path(&path);
            if let Some(metadata) = metadata {
                fs::write(&meta, metadata.to_string()).unwrap();
            }
            Self {
                root,
                path,
                meta,
                options,
            }
        }
        fn query(&self) -> Value {
            let sources = Arc::new(SessionSources::with_machines(vec![(
                String::new(),
                self.options.clone(),
            )]));
            let result = Query::from_sources(sources.clone())
                .call_at("list_sessions", &json!({}), NOW)
                .unwrap();
            sources.close();
            result
        }
        fn session(query: &Value) -> &Value {
            query["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|session| session["id"] == "sub")
                .unwrap()
        }
        fn connection(&self) -> Connection {
            Connection::open(crate::events::EventCache::path(&self.options.cache)).unwrap()
        }
        fn key(&self) -> String {
            format!("native_claude_sidecar:{}", self.path.display())
        }
        fn encoded(&self) -> Option<String> {
            self.connection()
                .query_row("SELECT value FROM meta WHERE key=?1", [self.key()], |row| {
                    row.get(0)
                })
                .optional()
                .unwrap()
        }
        fn record(&self) -> Value {
            serde_json::from_str(&self.encoded().unwrap()).unwrap()
        }
        fn proof(&self) -> crate::SessionSourceProof {
            crate::session_source_proof(&self.options, "claude", RELATIVE)
                .unwrap()
                .unwrap()
        }
        fn ready(&self) -> crate::SourceProjectionReady {
            crate::source_projection_ready(&self.options, "claude", RELATIVE)
                .unwrap()
                .unwrap()
        }
        fn refuse_eviction(&self) {
            assert_eq!(
                crate::retain_source_event_index(&self.options, &self.proof(), &self.ready()),
                Err(crate::CatalogIdentityError::ScopeChanged)
            );
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn metadata() -> Value {
        json!({"toolUseId":"call", "description":"Checker", "agentType":"general-purpose", "model":"claude-opus-5-5", "worktreePath":"/workspace/project", "worktreeBranch":"feature/native", "ignored_secret":"PRIVATE-SIDECAR-CONTROL"})
    }
    fn parsed() -> u64 {
        crate::events::PARSED.with(std::cell::Cell::get)
    }
    fn reads() -> u64 {
        crate::events::CLAUDE_SIDECAR_READS.with(std::cell::Cell::get)
    }

    #[test]
    fn claude_sidecar_cold_query_reuses_allowlisted_metadata_and_native_identity() {
        let home = Home::new(Some(metadata()));
        let originals = (fs::read(&home.path).unwrap(), fs::read(&home.meta).unwrap());
        let before = reads();
        let expected = home.query();
        assert_eq!(reads(), before + 1);
        let session = Home::session(&expected);
        assert_eq!(session["parent"], "lead");
        assert_eq!(session["kind"], "subagent");
        assert_eq!(session["name"], "Checker");
        // Native display hides a branch when this synthetic cwd has no repository.
        assert_eq!(session["branch"], Value::Null);
        assert_eq!(
            home.record()["sidecar"]["metadata"]["branch"],
            "feature/native"
        );
        let record = home.record();
        assert_eq!(
            record["sidecar"]["metadata"],
            serde_json::to_value(AgentMetadata::from_record(&metadata())).unwrap()
        );
        assert!(!home.encoded().unwrap().contains("PRIVATE-SIDECAR-CONTROL"));
        assert!(!home.encoded().unwrap().contains("PRIVATE-NATIVE-BODY"));
        let lines = parsed();
        assert_eq!(home.query(), expected);
        assert_eq!(reads(), before + 1);
        assert_eq!(parsed(), lines);
        assert_eq!(
            (fs::read(&home.path).unwrap(), fs::read(&home.meta).unwrap()),
            originals
        );
        assert_eq!(
            home.connection()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            23
        );
    }

    #[test]
    fn claude_sidecar_rewrite_creation_removal_and_malformed_json_do_not_replay_log() {
        let home = Home::new(Some(metadata()));
        let expected = home.query();
        let lines = parsed();
        let old = home.encoded();
        let mut changed = metadata();
        changed["description"] = json!("Renamed");
        changed["worktreeBranch"] = json!("feature/changed");
        fs::write(&home.meta, changed.to_string()).unwrap();
        let result = home.query();
        assert_eq!(Home::session(&result)["name"], "Renamed");
        assert_eq!(
            home.record()["sidecar"]["metadata"]["branch"],
            "feature/changed"
        );
        assert_ne!(home.encoded(), old);
        assert_eq!(parsed(), lines);
        fs::remove_file(&home.meta).unwrap();
        let absent = home.query();
        assert_eq!(Home::session(&absent)["parent"], "lead");
        assert_eq!(Home::session(&absent)["branch"], Value::Null);
        assert_eq!(home.record()["sidecar"]["observation"], "Absent");
        fs::write(&home.meta, metadata().to_string()).unwrap();
        assert_eq!(home.query(), expected);
        fs::write(&home.meta, "malformed").unwrap();
        assert_eq!(home.query(), absent);
        assert!(home.record()["sidecar"]["observation"]["Present"].is_object());
        assert_eq!(parsed(), lines);
    }

    #[test]
    fn claude_sidecar_missing_corrupt_or_mixed_index_metadata_repairs_without_event_replay() {
        let home = Home::new(Some(metadata()));
        let expected = home.query();
        let original = home.encoded().unwrap();
        let lines = parsed();
        for mutation in 0..5 {
            if mutation == 0 {
                home.connection()
                    .execute("DELETE FROM meta WHERE key=?1", [home.key()])
                    .unwrap();
            } else {
                let mut record: Value = serde_json::from_str(&original).unwrap();
                match mutation {
                    1 => record["version"] = json!(99),
                    2 => record["ledger"] = json!("mixed-generation"),
                    3 => record["sidecar"]["metadata"]["credential_authority"] = json!(true),
                    _ => record["sidecar"]["observation"]["Present"]["stat"] = json!("bad"),
                }
                home.connection()
                    .execute(
                        "UPDATE meta SET value=?2 WHERE key=?1",
                        rusqlite::params![home.key(), record.to_string()],
                    )
                    .unwrap();
            }
            home.refuse_eviction();
            let before = reads();
            assert_eq!(home.query(), expected);
            assert_eq!(reads(), before + 1);
            assert_eq!(parsed(), lines);
            assert_eq!(home.encoded().as_deref(), Some(original.as_str()));
        }
    }

    #[test]
    fn claude_sidecar_changes_during_indexing_and_publication_are_fenced() {
        let home = Home::new(Some(metadata()));
        let path = home.meta.clone();
        crate::events::set_claude_sidecar_hook(move || {
            let mut meta = metadata();
            meta["description"] = json!("After capture");
            fs::write(path, meta.to_string()).unwrap();
        });
        let result = home.query();
        assert_eq!(Home::session(&result)["name"], "After capture");
        assert_eq!(
            home.record()["sidecar"]["metadata"]["label"],
            "After capture"
        );
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        let generation = cache.session_catalog_generation();
        let path = home.meta.clone();
        crate::model::AFTER_SCAN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(path, metadata().to_string()).unwrap()
            }))
        });
        assert!(
            crate::model::observe_catalog(
                &home.options,
                &mut cache,
                &mut false,
                &mut crate::model::Texts::default(),
                NOW
            )
            .is_err()
        );
        assert_eq!(cache.session_catalog_generation(), generation);
        home.refuse_eviction();
        assert_eq!(Home::session(&home.query())["name"], "Checker");
        let sources = Arc::new(SessionSources::with_machines(vec![(
            String::new(),
            home.options.clone(),
        )]));
        let mut query = Query::from_sources(sources.clone());
        let path = home.meta.clone();
        crate::model::AFTER_QUERY_DERIVATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let mut metadata = metadata();
                metadata["description"] = json!("After derivation");
                fs::write(path, metadata.to_string()).unwrap();
            }))
        });
        assert_eq!(
            query
                .call_at("list_sessions", &json!({}), NOW)
                .unwrap_err()
                .code,
            "io"
        );
        sources.close();
        assert_eq!(Home::session(&home.query())["name"], "After derivation");
    }

    #[cfg(unix)]
    #[test]
    fn claude_sidecar_links_and_oversized_inputs_keep_log_reads_but_cannot_qualify_eviction() {
        use std::os::unix::fs::symlink;
        let home = Home::new(Some(metadata()));
        home.query();
        let lines = parsed();
        fs::rename(&home.meta, home.root.join("private-target")).unwrap();
        symlink(home.root.join("private-target"), &home.meta).unwrap();
        let before = reads();
        let ignored = home.query();
        assert_eq!(reads(), before);
        assert_eq!(parsed(), lines);
        assert!(home.encoded().is_none());
        home.refuse_eviction();
        assert_eq!(Home::session(&ignored)["parent"], "lead");
        fs::remove_file(&home.meta).unwrap();
        fs::write(&home.meta, vec![b' '; crate::RECORD_MAX as usize + 1]).unwrap();
        assert_eq!(home.query(), ignored);
        assert!(home.encoded().is_none());
        home.refuse_eviction();
        assert_eq!(parsed(), lines);
        // The same old sidecar behind a linked ancestor cannot restore its proof.
        fs::remove_file(&home.meta).unwrap();
        fs::rename(home.root.join("private-target"), &home.meta).unwrap();
        let dir = home.path.parent().unwrap();
        let moved = dir.with_file_name("moved");
        fs::rename(dir, &moved).unwrap();
        symlink(&moved, dir).unwrap();
        assert!(ClaudeSidecar::observe(&home.path, None).unwrap().is_none());
    }

    #[test]
    fn claude_sidecar_repeated_races_keep_native_session_without_a_portable_proof() {
        fn arm(path: PathBuf, remaining: usize) {
            crate::events::set_claude_sidecar_hook(move || {
                let mut metadata = metadata();
                metadata["description"] = json!(format!("Generation {remaining}"));
                std::thread::sleep(std::time::Duration::from_millis(2));
                fs::write(&path, metadata.to_string()).unwrap();
                if remaining > 1 {
                    arm(path, remaining - 1);
                }
            });
        }
        let home = Home::new(Some(metadata()));
        arm(home.meta.clone(), 3);
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
        assert_eq!(home.proof().source.native_id, "sub");
        assert_eq!(home.ready().source.native_id, "sub");
        assert!(home.encoded().is_none());
        home.refuse_eviction();
        assert_eq!(Home::session(&home.query())["name"], "Generation 1");
    }

    #[test]
    fn claude_sidecar_encoded_limit_preserves_full_native_values_without_eviction() {
        let label = "x".repeat(crate::RECORD_MAX as usize - 32);
        let home = Home::new(Some(json!({"description":label})));
        assert!(fs::metadata(&home.meta).unwrap().len() < crate::RECORD_MAX);
        let query = home.query();
        // Preserve the native display limit while retaining the entire parsed value.
        assert_eq!(
            Home::session(&query)["name"],
            crate::model::one_line(&label, 80)
        );
        assert!(home.encoded().is_none());
        home.refuse_eviction();
        let lines = parsed();
        assert_eq!(home.query(), query);
        assert_eq!(parsed(), lines);
        assert_eq!(
            ClaudeSidecar::observe(&home.path, None)
                .unwrap()
                .unwrap()
                .metadata
                .label
                .as_deref(),
            Some(label.as_str())
        );
    }

    #[test]
    fn claude_sidecar_archive_marker_binds_current_metadata_and_preserves_prior_rows() {
        let home = Home::new(Some(metadata()));
        home.query();
        let proof = home.proof();
        let ready = home.ready();
        crate::retain_source_event_index(&home.options, &proof, &ready).unwrap();
        assert!(crate::source_event_index_retained(&home.options, &proof, &ready).unwrap());
        let key = format!("archive_event_index:{}", home.path.display());
        let encoded: String = home
            .connection()
            .query_row("SELECT value FROM meta WHERE key=?1", [&key], |row| {
                row.get(0)
            })
            .unwrap();
        let mut marker: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(marker["version"], 6);
        fs::write(&home.meta, metadata().to_string() + " ").unwrap();
        assert!(!crate::source_event_index_retained(&home.options, &proof, &ready).unwrap());
        home.refuse_eviction();
        home.query();
        let ready = home.ready();
        crate::retain_source_event_index(&home.options, &proof, &ready).unwrap();
        let mut record = home.encoded().unwrap();
        record.push(' ');
        home.connection()
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                rusqlite::params![home.key(), record],
            )
            .unwrap();
        assert!(!crate::source_event_index_retained(&home.options, &proof, &ready).unwrap());
        // Prior v1 markers retain already evicted rows even without sidecar proof.
        marker["version"] = json!(1);
        marker
            .as_object_mut()
            .unwrap()
            .remove("source_chunks_sha256");
        marker
            .as_object_mut()
            .unwrap()
            .remove("parent_marker_sha256");
        marker
            .as_object_mut()
            .unwrap()
            .remove("launch_calls_sha256");
        marker
            .as_object_mut()
            .unwrap()
            .remove("native_header_sha256");
        home.connection()
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                rusqlite::params![key, marker.to_string()],
            )
            .unwrap();
        home.connection()
            .execute("DELETE FROM meta WHERE key=?1", [home.key()])
            .unwrap();
        fs::remove_file(&home.path).unwrap();
        let mut cache = crate::events::EventCache::open_scoped(&home.options.cache).unwrap();
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
        assert_eq!(
            home.connection()
                .query_row(
                    "SELECT count(*) FROM files WHERE path=?1",
                    [home.path.to_str().unwrap()],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert!(!crate::source_event_index_retained(&home.options, &proof, &ready).unwrap());
    }
}
