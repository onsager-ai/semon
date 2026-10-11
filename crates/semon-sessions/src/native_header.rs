//! Allowlisted native Codex header metadata, never message bodies or authority.
use crate::model::CodexMeta;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// The first physical record only, matching native discovery. A known absent
/// header still permits the existing filename identity fallback.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodexHeader {
    pub(crate) id: Option<String>,
    pub(crate) metadata: CodexMeta,
}

impl CodexHeader {
    pub(crate) fn from_record(record: &Value) -> Self {
        let Some(metadata) = crate::codex_meta_payload(record) else {
            return Self::default();
        };
        Self {
            id: crate::field(metadata, "id").map(str::to_owned),
            metadata: CodexMeta::from_meta(metadata),
        }
    }

    pub(crate) fn native_id(&self, path: &Path) -> Option<String> {
        self.id
            .clone()
            .or_else(|| crate::codex_id_from_filename(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Query, SessionSources};
    use rusqlite::Connection;
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
    struct Home {
        root: std::path::PathBuf,
        path: std::path::PathBuf,
        options: Options,
    }
    impl Home {
        fn new(records: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-native-header-{}-{}",
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
            let path = options
                .codex_home
                .join("sessions/2026/09/24/rollout-file.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, records).unwrap();
            Self {
                root,
                path,
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
        fn connection(&self) -> Connection {
            Connection::open(crate::events::EventCache::path(&self.options.cache)).unwrap()
        }
        fn header_text(&self) -> String {
            self.connection()
                .query_row(
                    "SELECT value FROM meta WHERE key=?1",
                    [format!("native_codex_header:{}", self.path.display())],
                    |row| row.get(0),
                )
                .unwrap()
        }
        fn proof(&self) -> crate::SessionSourceProof {
            crate::session_source_proof(
                &self.options,
                "codex",
                "sessions/2026/09/24/rollout-file.jsonl",
            )
            .unwrap()
            .unwrap()
        }
        fn ready(&self) -> crate::SourceProjectionReady {
            crate::source_projection_ready(
                &self.options,
                "codex",
                "sessions/2026/09/24/rollout-file.jsonl",
            )
            .unwrap()
            .unwrap()
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn records() -> String {
        let header = json!({"type":"session_meta","timestamp":"2026-09-24T00:00:00Z","payload":{
            "id":"run", "session_id":"native-root", "forked_from_id":"ancestor", "forked_from_ordinal_exclusive":7,
            "history_base":{"thread_id":"ancestor","end_ordinal_exclusive":7,"end_byte_offset":99},
            "parent_thread_id":"parent", "agent_nickname":"Scout", "agent_path":"root.1", "thread_source":"subagent",
            "source":{"subagent":{"other":"guardian"}}, "cwd":"/workspace/project", "git":{"branch":"feature/native"},
            "ignored_secret":"PRIVATE-HEADER-CONTROL"
        }});
        let message = json!({"type":"response_item","timestamp":"2026-09-24T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"PRIVATE-BODY-CONTROL"}]}});
        format!("{header}\n{message}\n")
    }

    #[test]
    fn codex_header_restart_uses_indexed_native_metadata_and_keeps_body_private() {
        let home = Home::new(&records());
        let original = fs::read(&home.path).unwrap();
        let reads = || crate::events::CODEX_HEADER_READS.with(std::cell::Cell::get);
        let before = reads();
        let expected = home.query();
        assert_eq!(reads(), before + 2);
        let header: Value = serde_json::from_str(&home.header_text()).unwrap();
        let legacy = crate::codex_meta(&home.path).unwrap().unwrap();
        assert_eq!(
            header["header"]["metadata"],
            serde_json::to_value(crate::model::CodexMeta::from_meta(&legacy)).unwrap()
        );
        assert_eq!(header["header"]["id"], "run");
        assert!(!home.header_text().contains("PRIVATE-HEADER-CONTROL"));
        assert!(!home.header_text().contains("PRIVATE-BODY-CONTROL"));
        let direct = crate::events::CLAUDE_SIDECAR_READS.with(std::cell::Cell::get);
        assert_eq!(home.query(), expected);
        assert_eq!(
            reads(),
            before + 2,
            "a cold native query reuses the committed header"
        );
        assert_eq!(
            crate::events::CLAUDE_SIDECAR_READS.with(std::cell::Cell::get),
            direct
        );
        assert_eq!(fs::read(&home.path).unwrap(), original);
        assert_eq!(
            home.connection()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            21
        );
    }

    #[test]
    fn codex_header_repairs_only_metadata_for_legacy_corrupt_or_mixed_generations() {
        let home = Home::new(&records());
        let expected = home.query();
        let original = home.header_text();
        let key = format!("native_codex_header:{}", home.path.display());
        for mutation in 0..4 {
            let connection = home.connection();
            if mutation == 0 {
                connection
                    .execute("DELETE FROM meta WHERE key=?1", [&key])
                    .unwrap();
            } else {
                let mut record: Value = serde_json::from_str(&original).unwrap();
                match mutation {
                    1 => record["version"] = json!(99),
                    2 => record["ledger"] = json!("wrong-generation"),
                    _ => record["header"]["metadata"]["credential_authority"] = json!(true),
                }
                connection
                    .execute(
                        "UPDATE meta SET value=?2 WHERE key=?1",
                        rusqlite::params![key, record.to_string()],
                    )
                    .unwrap();
            }
            let proof = home.proof();
            assert!(
                !crate::source_event_index_retained(&home.options, &proof, &home.ready()).unwrap()
            );
            assert_eq!(
                crate::retain_source_event_index(&home.options, &proof, &home.ready()),
                Err(crate::CatalogIdentityError::ScopeChanged)
            );
            let parsed = crate::events::PARSED.with(std::cell::Cell::get);
            let reads = crate::events::CODEX_HEADER_READS.with(std::cell::Cell::get);
            assert_eq!(home.query(), expected);
            assert_eq!(
                crate::events::PARSED.with(std::cell::Cell::get),
                parsed,
                "native event rows do not need replay"
            );
            assert_eq!(
                crate::events::CODEX_HEADER_READS.with(std::cell::Cell::get),
                reads + 2
            );
            assert_eq!(home.header_text(), original);
        }
    }

    #[test]
    fn codex_header_preserves_first_physical_record_and_unterminated_metadata() {
        for text in [
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"unfinished\",\"cwd\":\"/original\"}}",
            "malformed\n{\"type\":\"session_meta\",\"payload\":{\"id\":\"later\"}}\n",
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/no-id\"}}\n",
        ] {
            let home = Home::new(text);
            home.query();
            let header: Value = serde_json::from_str(&home.header_text()).unwrap();
            let legacy = crate::codex_meta(&home.path).unwrap();
            assert_eq!(
                header["header"]["id"],
                json!(legacy.as_ref().and_then(|value| crate::field(value, "id")))
            );
            assert_eq!(
                home.query()["sessions"][0]["id"],
                json!(
                    legacy
                        .as_ref()
                        .and_then(|value| crate::field(value, "id"))
                        .unwrap_or("file")
                )
            );
            if !text.ends_with('\n') {
                assert_eq!(
                    home.connection()
                        .query_row("SELECT resume_at FROM files", [], |row| row
                            .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
            }
        }
    }

    #[test]
    fn codex_header_oversize_stays_complete_and_cannot_qualify_eviction() {
        let records = records();
        let (first, rest) = records.split_once('\n').unwrap();
        let mut first: Value = serde_json::from_str(first).unwrap();
        let cwd = "/".repeat(1024 * 1024 + 1);
        first["payload"]["cwd"] = json!(cwd);
        let home = Home::new(&format!("{first}\n{rest}"));
        let original = fs::read(&home.path).unwrap();
        let expected = home.query();
        let mut cache = crate::events::EventCache::open(&home.options.cache);
        let index = crate::events::scan_file(&home.path, "codex", &mut cache, &mut false).unwrap();
        let metadata =
            serde_json::to_value(&index.codex_header.as_ref().unwrap().metadata).unwrap();
        assert_eq!(metadata["cwd"], cwd, "native metadata is not truncated");
        assert_eq!(
            home.connection()
                .query_row(
                    "SELECT count(*) FROM meta WHERE key GLOB 'native_codex_header:*'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let parsed = crate::events::PARSED.with(std::cell::Cell::get);
        assert_eq!(home.query(), expected);
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), parsed);
        assert_eq!(
            crate::retain_source_event_index(&home.options, &home.proof(), &home.ready()),
            Err(crate::CatalogIdentityError::ScopeChanged)
        );
        assert_eq!(fs::read(&home.path).unwrap(), original);
    }

    #[test]
    fn codex_header_retry_cannot_bind_a_prior_identity_to_a_new_prefix() {
        let home = Home::new(&records());
        let path = home.path.clone();
        crate::events::set_codex_header_hook(move || {
            let before = fs::read_to_string(&path).unwrap();
            fs::write(path, before.replace("\"id\":\"run\"", "\"id\":\"new\"")).unwrap();
        });
        let mut cache = crate::events::EventCache::open(&home.options.cache);
        let index = crate::events::scan_file(&home.path, "codex", &mut cache, &mut false).unwrap();
        assert_eq!(
            index.codex_header.as_ref().unwrap().id.as_deref(),
            Some("new")
        );
        let captured: Value = serde_json::from_str(&home.header_text()).unwrap();
        assert_eq!(captured["header"]["id"], "new");
        assert_eq!(home.query()["sessions"][0]["id"], "new");
    }

    #[test]
    fn codex_header_retention_fences_metadata_and_preserves_prior_archive_markers() {
        let home = Home::new(&records());
        home.query();
        let proof = home.proof();
        let ready = home.ready();
        crate::retain_source_event_index(&home.options, &proof, &ready).unwrap();
        assert!(crate::source_event_index_retained(&home.options, &proof, &ready).unwrap());
        let archive_key = format!("archive_event_index:{}", home.path.display());
        let connection = home.connection();
        let mut marker: Value = serde_json::from_str(
            &connection
                .query_row::<String, _, _>(
                    "SELECT value FROM meta WHERE key=?1",
                    [&archive_key],
                    |row| row.get(0),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(marker["version"], 6);
        connection
            .execute(
                "UPDATE meta SET value=value || ' ' WHERE key=?1",
                [format!("native_codex_header:{}", home.path.display())],
            )
            .unwrap();
        assert!(
            !crate::source_event_index_retained(&home.options, &proof, &ready).unwrap(),
            "even decodable header changes require another verified marker"
        );
        // Earlier markers retain their native index rows even without portable
        // headers. They cannot qualify another eviction in the new binary.
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
        connection
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                rusqlite::params![archive_key, marker.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "DELETE FROM meta WHERE key=?1",
                [format!("native_codex_header:{}", home.path.display())],
            )
            .unwrap();
        fs::remove_file(&home.path).unwrap();
        home.query();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            crate::session_catalog_history_identity(&home.options, "machine", "run")
                .unwrap()
                .unwrap()
                .source_refs[0]
                .source,
            proof.source
        );
        assert!(!crate::source_event_index_retained(&home.options, &proof, &home.ready()).unwrap());
    }
}
