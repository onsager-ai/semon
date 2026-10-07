//! Authorized immutable originals feed the shared native producer. Descriptors
//! are provenance, never credentials or a grant of native control authority.
use crate::SessionSourceRange;
use std::io;

/// Host-authorized logical original. `generation` is the lowercase hexadecimal
/// SHA-256 of all `length` logical bytes, verified by the host independently.
/// The producer also hashes the returned logical stream before publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySourceDescriptor {
    pub root: String,
    pub path: String,
    pub generation: String,
    pub length: u64,
}

/// Borrowed, operation-owned authorization and immutable byte transport. Each
/// range revalidates its host's custody/authentication, deadline and cancellation.
/// Never retain this reader or request credentials in a shared model or store.
/// Each result must echo the descriptor generation, total length and offset;
/// `max_bytes` never exceeds SESSION_SOURCE_CHUNK_MAX (128 KiB).
pub trait HistorySourceReader: Send + Sync {
    fn read_range(
        &self,
        source: &HistorySourceDescriptor,
        offset: u64,
        max_bytes: usize,
    ) -> io::Result<SessionSourceRange>;
}

/// Read-only source projection evidence returned after atomic CAS publication.
#[derive(Clone, Debug)]
pub struct HistorySourcePublication {
    pub catalog_keys: Vec<String>,
    pub source_refs: Vec<crate::SessionSourceRef>,
    pub recipe_version: u32,
    pub projections: Vec<HistoryProjectionEvidence>,
}
#[derive(Clone, Debug)]
pub struct HistoryProjectionEvidence {
    pub key: String,
    pub generation: String,
    pub source_generation: String,
}

/// Backfill one authorized immutable original through the shared native producer.
/// Native records and bytes are operation-local only; persistent data contains
/// metadata/recipes/source references. Current native selection is never granted.
/// This initial contract supports at most64MiB/4096records/events and standard
/// Claude top sessions or Codex rollouts; relationship context remains incomplete.
/// Conflicting current owners or newer catalog generations return WouldBlock.
pub fn publish_immutable_history_source(
    options: &crate::Options,
    descriptor: &HistorySourceDescriptor,
    reader: &dyn HistorySourceReader,
) -> io::Result<HistorySourcePublication> {
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    const MAX_SOURCE: u64 = 64 * 1024 * 1024;
    if descriptor.length == 0
        || descriptor.length > MAX_SOURCE
        || descriptor.generation.len() != 64
        || !descriptor
            .generation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !crate::is_input_path(&descriptor.root, &descriptor.path)
        || !descriptor.path.ends_with(".jsonl")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "immutable original descriptor is outside producer limits",
        ));
    }
    let root = crate::inputs::InputRoot::parse(&descriptor.root)
        .ok_or_else(|| io::Error::other("native original root"))?;
    if !matches!(
        root,
        crate::inputs::InputRoot::Claude | crate::inputs::InputRoot::Codex
    ) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native original harness unsupported",
        ));
    }
    let input = crate::inputs::Input {
        root,
        path: descriptor.path.clone(),
    };
    let mut cache =
        crate::events::EventCache::open_scoped(&options.cache).map_err(io::Error::other)?;
    let expected = cache.session_catalog_generation();
    let mut bytes = Vec::with_capacity(
        usize::try_from(descriptor.length)
            .map_err(|_| io::Error::other("native original length"))?,
    );
    let mut hash = Sha256::new();
    while (bytes.len() as u64) < descriptor.length {
        let offset = bytes.len() as u64;
        let max = usize::try_from(descriptor.length - offset)
            .unwrap_or(crate::SESSION_SOURCE_CHUNK_MAX)
            .min(crate::SESSION_SOURCE_CHUNK_MAX);
        let range = reader.read_range(descriptor, offset, max)?;
        if range.generation != descriptor.generation
            || range.length != descriptor.length
            || range.offset != offset
            || range.bytes.is_empty()
            || range.bytes.len() > max
        {
            return Err(io::Error::other(
                "immutable original range identity or length mismatch",
            ));
        }
        hash.update(&range.bytes);
        bytes.extend_from_slice(&range.bytes);
    }
    let generation: [u8; 32] = hash.finalize().into();
    if generation
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        != descriptor.generation
    {
        return Err(io::Error::other("immutable original full SHA mismatch"));
    }
    // Reauthorize at completion even when all bytes came from a host cache.
    let end = reader.read_range(descriptor, descriptor.length, 1)?;
    if end.generation != descriptor.generation
        || end.length != descriptor.length
        || end.offset != descriptor.length
        || !end.bytes.is_empty()
    {
        return Err(io::Error::other(
            "immutable original completion identity mismatch",
        ));
    }
    let consumed = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "immutable original has no complete native record",
            )
        })?;
    let prefix = Sha256::digest(&bytes[..consumed]).into();
    let tail = Sha256::digest(&bytes[consumed.saturating_sub(4096)..consumed]).into();
    let immutable = crate::model::ImmutableSource {
        generation,
        length: descriptor.length,
        consumed: consumed as u64,
        prefix,
        tail,
    };
    let mut prepared =
        crate::model::prepare_immutable_source(options, &input, Arc::from(bytes), immutable)?;
    prepared.base_generation = expected;
    let mut evidence = Vec::new();
    for projection in &prepared.transcripts {
        let row = prepared
            .rows
            .iter()
            .find(|row| row.key == projection.key)
            .ok_or_else(|| io::Error::other("native original projection membership"))?;
        let source_generation =
            crate::retention::source_generation(&row.sources).map_err(io::Error::other)?;
        let texts: Vec<_> = projection
            .slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<_, _>>()
            .map_err(io::Error::other)?;
        let encoded = serde_json::to_string(&(
            crate::slot_projection::VERSION,
            &source_generation,
            &texts,
            true,
        ))
        .map_err(io::Error::other)?;
        evidence.push(HistoryProjectionEvidence {
            key: row.key.clone(),
            generation: format!("{:x}", Sha256::digest(encoded.as_bytes())),
            source_generation,
        });
    }
    // Native preparation and recipe mapping are bounded but may outlive a host
    // deadline/grant. Reauthorize again immediately before the CAS transaction.
    let end = reader.read_range(descriptor, descriptor.length, 1)?;
    if end.generation != descriptor.generation
        || end.length != descriptor.length
        || end.offset != descriptor.length
        || !end.bytes.is_empty()
    {
        return Err(io::Error::other(
            "immutable original publication authorization changed",
        ));
    }
    if cache.publish_immutable_history(&prepared)? != crate::events::Outcome::Written {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "immutable original publication conflicted; resynchronize before retry",
        ));
    }
    Ok(HistorySourcePublication {
        catalog_keys: prepared.rows.iter().map(|row| row.key.clone()).collect(),
        source_refs: prepared
            .rows
            .iter()
            .flat_map(|row| &row.sources)
            .map(|source| crate::SessionSourceRef {
                root: descriptor.root.clone(),
                path: descriptor.path.clone(),
                native_id: source.native_id.clone(),
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
                immutable_generation: source.immutable_generation.clone(),
            })
            .collect(),
        recipe_version: crate::slot_projection::VERSION,
        projections: evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionSourceReadMode, SessionSourceReader, SessionSourceRef};
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Original {
        bytes: Arc<[u8]>,
        generation: String,
    }
    impl Original {
        fn new(bytes: Vec<u8>) -> Self {
            Self {
                generation: format!("{:x}", Sha256::digest(&bytes)),
                bytes: Arc::from(bytes),
            }
        }
        fn range(&self, offset: u64, max: usize) -> io::Result<SessionSourceRange> {
            let start = usize::try_from(offset).map_err(io::Error::other)?;
            let end = start.saturating_add(max).min(self.bytes.len());
            Ok(SessionSourceRange {
                generation: self.generation.clone(),
                length: self.bytes.len() as u64,
                offset,
                bytes: Arc::from(
                    self.bytes
                        .get(start..end)
                        .ok_or_else(|| io::Error::other("range"))?,
                ),
                cached: true,
            })
        }
    }
    impl HistorySourceReader for Original {
        fn read_range(
            &self,
            _: &HistorySourceDescriptor,
            offset: u64,
            max: usize,
        ) -> io::Result<SessionSourceRange> {
            self.range(offset, max)
        }
    }
    impl SessionSourceReader for Original {
        fn read_range(
            &self,
            source: &SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            max: usize,
        ) -> io::Result<SessionSourceRange> {
            assert_eq!(
                source.immutable_generation.as_deref(),
                Some(self.generation.as_str())
            );
            assert_eq!(expected, Some(self.generation.as_str()));
            self.range(offset, max)
        }
    }
    struct Fixture {
        root: std::path::PathBuf,
        options: crate::Options,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-immutable-native-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let options = crate::Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                proc_root: root.join("proc"),
                cache: root.join("index.json"),
                all: true,
                since: std::time::Duration::from_secs(86400),
                session: None,
                facts: Some(root.join("missing-facts.json")),
                scan_window: false,
            };
            Self { root, options }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn claude_original() -> Original {
        let mut bytes = Vec::new();
        for value in [
            serde_json::json!({"type":"user","sessionId":"native-original","uuid":"original-u","timestamp":"2026-10-01T12:00:00Z","message":{"role":"user","content":"original archived question"}}),
            serde_json::json!({"type":"assistant","sessionId":"native-original","uuid":"original-a","timestamp":"2026-10-01T12:00:01Z","message":{"role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"original archived answer"}]}}),
        ] {
            bytes.extend_from_slice(serde_json::to_string(&value).unwrap().as_bytes());
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(b"{\"type\":\"user\",\"incomplete\":");
        Original::new(bytes)
    }
    #[test]
    fn verified_immutable_native_original_publishes_history_without_local_or_current_authority() {
        let fixture = Fixture::new();
        let original = claude_original();
        let descriptor = HistorySourceDescriptor {
            root: "claude".into(),
            path: "projects/project/native-original.jsonl".into(),
            generation: original.generation.clone(),
            length: original.bytes.len() as u64,
        };
        let publication =
            publish_immutable_history_source(&fixture.options, &descriptor, &original).unwrap();
        assert_eq!(publication.catalog_keys, vec!["native-original"]);
        assert!(publication.source_refs[0].offset < descriptor.length);
        assert_eq!(
            publication.source_refs[0].prefix_sha256,
            Sha256::digest(&original.bytes[..publication.source_refs[0].offset as usize])
                .as_slice()
        );
        let identity =
            crate::session_catalog_history_identity(&fixture.options, "machine", "native-original")
                .unwrap()
                .unwrap();
        assert_eq!(identity.native_id, None);
        assert_eq!(
            identity.owner_qualification,
            crate::CatalogOwnerQualification::Provisional
        );
        let current = crate::session_catalog_page(&fixture.options, "machine", "limit=60");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&current.body).unwrap()["items"],
            serde_json::json!([])
        );
        let reply = crate::session_transcript_range_with_mode(
            &fixture.options,
            "machine",
            "sid=native-original&scope=retained_history&limit=60",
            Some(&original),
            SessionSourceReadMode::LocalThenProvider,
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["entries"][0]["text"], "original archived question");
        assert_eq!(body["entries"][1]["text"], "original archived answer");
        assert_eq!(
            body["projection"]["generation"],
            publication.projections[0].generation
        );
        assert!(!descriptor.path.is_empty());
        assert!(!fixture.options.claude_home.join(&descriptor.path).exists());
        let connection =
            rusqlite::Connection::open(crate::events::EventCache::path(&fixture.options.cache))
                .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let metadata: String = connection
            .query_row(
                "SELECT metadata FROM session_catalog WHERE session_key='native-original'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!metadata.contains("original archived question"));
        assert!(!metadata.contains("original archived answer"));
        drop(connection);
        assert!(publish_immutable_history_source(&fixture.options, &descriptor, &original).is_ok());
        let mut cache = crate::events::EventCache::open(&fixture.options.cache);
        crate::model::build(
            &fixture.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            crate::model::now_ms(),
        )
        .unwrap();
        let reply = crate::session_transcript_range_with_mode(
            &fixture.options,
            "machine",
            "sid=native-original&scope=retained_history&limit=60",
            Some(&original),
            SessionSourceReadMode::LocalThenProvider,
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
    }
    #[test]
    fn immutable_original_sha_mismatch_never_publishes_a_projection() {
        let fixture = Fixture::new();
        let mut original = claude_original();
        original.generation = "00".repeat(32);
        let descriptor = HistorySourceDescriptor {
            root: "claude".into(),
            path: "projects/project/native-original.jsonl".into(),
            generation: original.generation.clone(),
            length: original.bytes.len() as u64,
        };
        assert!(
            publish_immutable_history_source(&fixture.options, &descriptor, &original).is_err()
        );
        let connection =
            rusqlite::Connection::open(crate::events::EventCache::path(&fixture.options.cache))
                .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM session_catalog", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn codex_immutable_native_tool_original_preserves_bounded_field_recipe() {
        let fixture = Fixture::new();
        let records = [
            serde_json::json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":"original-codex","cwd":"/synthetic/project"}}),
            serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"function_call","name":"exec_command","call_id":"native-tool","arguments":"{\"cmd\":\"printf fixture\"}"}}),
            serde_json::json!({"type":"response_item","timestamp":"2026-10-01T00:00:02Z","payload":{"type":"function_call_output","call_id":"native-tool","output":"original codex output"}}),
        ];
        let bytes = records
            .iter()
            .map(|record| format!("{record}\n"))
            .collect::<String>()
            .into_bytes();
        let original = Original::new(bytes);
        let descriptor = HistorySourceDescriptor {
            root: "codex".into(),
            path: "sessions/2026/10/01/rollout-original-codex.jsonl".into(),
            generation: original.generation.clone(),
            length: original.bytes.len() as u64,
        };
        let publication =
            publish_immutable_history_source(&fixture.options, &descriptor, &original).unwrap();
        assert_eq!(publication.source_refs[0].native_id, "original-codex");
        let query = format!(
            "sid={}&scope=retained_history&limit=60",
            publication.catalog_keys[0]
        );
        let reply = crate::session_transcript_range_with_mode(
            &fixture.options,
            "machine",
            &query,
            Some(&original),
            SessionSourceReadMode::LocalThenProvider,
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert!(
            body["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["out"] == "original codex output"),
            "{body}"
        );
    }
    #[test]
    fn conflicting_same_path_observation_preserves_immutable_original_history() {
        let fixture = Fixture::new();
        let original = claude_original();
        let descriptor = HistorySourceDescriptor {
            root: "claude".into(),
            path: "projects/project/native-original.jsonl".into(),
            generation: original.generation.clone(),
            length: original.bytes.len() as u64,
        };
        publish_immutable_history_source(&fixture.options, &descriptor, &original).unwrap();
        let path = fixture.options.claude_home.join(&descriptor.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path,format!("{}\n",serde_json::json!({"type":"user","sessionId":"native-original","uuid":"different-user","timestamp":"2026-10-02T12:00:00Z","message":{"role":"user","content":"a different current original"}}))).unwrap();
        let mut cache = crate::events::EventCache::open(&fixture.options.cache);
        crate::model::build(
            &fixture.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            crate::model::now_ms(),
        )
        .unwrap();
        let reply = crate::session_transcript_range_with_mode(
            &fixture.options,
            "machine",
            "sid=native-original&scope=retained_history&limit=60",
            Some(&original),
            SessionSourceReadMode::LocalThenProvider,
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(body["entries"][0]["text"], "original archived question");
        let connection =
            rusqlite::Connection::open(crate::events::EventCache::path(&fixture.options.cache))
                .unwrap();
        assert!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM session_catalog_invalidations",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap()
                > 0
        );
    }
    #[test]
    fn expired_authorization_after_native_preparation_never_publishes_history() {
        struct Revoked {
            original: Original,
            ends: std::sync::atomic::AtomicUsize,
        }
        impl HistorySourceReader for Revoked {
            fn read_range(
                &self,
                source: &HistorySourceDescriptor,
                offset: u64,
                max: usize,
            ) -> io::Result<SessionSourceRange> {
                if offset == source.length && self.ends.fetch_add(1, Ordering::Relaxed) == 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "publication grant expired",
                    ));
                }
                self.original.range(offset, max)
            }
        }
        let fixture = Fixture::new();
        let original = claude_original();
        let descriptor = HistorySourceDescriptor {
            root: "claude".into(),
            path: "projects/project/native-original.jsonl".into(),
            generation: original.generation.clone(),
            length: original.bytes.len() as u64,
        };
        let revoked = Revoked {
            original,
            ends: std::sync::atomic::AtomicUsize::new(0),
        };
        assert_eq!(
            publish_immutable_history_source(&fixture.options, &descriptor, &revoked)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(revoked.ends.load(Ordering::Relaxed), 2);
        let connection =
            rusqlite::Connection::open(crate::events::EventCache::path(&fixture.options.cache))
                .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM session_catalog", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
