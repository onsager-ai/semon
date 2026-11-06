use crate::{
    CatalogReadScope, Input, InputRoot,
    storage::{Namespace, postgres::PgProjectionStore},
};

#[cfg(target_os = "linux")]
struct SqliteOpenWatch(std::os::fd::OwnedFd);
#[cfg(target_os = "linux")]
impl SqliteOpenWatch {
    fn new(options: &crate::Options) -> Self {
        use std::os::fd::{AsRawFd, FromRawFd};
        let path = crate::events::EventCache::path(&options.cache);
        // A real readable SQLite sentinel detects hidden read-only opens too.
        // It is test-owned and separate from the explicit comparison backend.
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('unchanged')").unwrap();
        drop(db);
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0);
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert!(unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), libc::IN_OPEN | libc::IN_MODIFY | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) } >= 0);
        Self(fd)
    }
    fn assert_untouched(self, options: &crate::Options) {
        use std::os::fd::AsRawFd;
        let mut events = [0u8; 4096];
        let read = unsafe { libc::read(self.0.as_raw_fd(), events.as_mut_ptr().cast(), events.len()) };
        assert_eq!(read, -1, "PostgreSQL path accessed the SQLite sentinel");
        assert_eq!(std::io::Error::last_os_error().kind(), std::io::ErrorKind::WouldBlock);
        drop(self);
        std::fs::remove_file(crate::events::EventCache::path(&options.cache)).unwrap();
    }
}

struct NativeReader {
    path: std::path::PathBuf,
    source: crate::model::summary::CatalogSource,
    proof: [u8; 32],
}
impl crate::SessionSourceReader for NativeReader {
    fn read_range(
        &self,
        reference: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> std::io::Result<crate::SessionSourceRange> {
        use sha2::{Digest, Sha256};
        let invalid =
            || std::io::Error::new(std::io::ErrorKind::InvalidData, "selected fixture changed");
        let generation = format!("{:x}", Sha256::digest(self.source.prefix_sha256));
        let bytes = std::fs::read(&self.path)?;
        if reference.offset != self.source.offset
            || reference.prefix_sha256 != self.source.prefix_sha256
            || expected.is_some_and(|expected| expected != generation)
            || (bytes.len() as u64) < reference.offset
            || <[u8; 32]>::from(Sha256::digest(&bytes[..reference.offset as usize])) != self.proof
            || offset >= reference.offset
            || max > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(invalid());
        }
        let end = (offset as usize + max).min(reference.offset as usize);
        Ok(crate::SessionSourceRange {
            generation,
            length: reference.offset,
            offset,
            bytes: std::sync::Arc::from(&bytes[offset as usize..end]),
            cached: true,
        })
    }
}

async fn semantic_parity(dsn: &str) {
    let root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&root.0, "pg-parity");
    let sqlite = crate::storage::conformance::source(&root.0, "sqlite-parity");
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/compatibility");
    let path = options.codex_home.join("sessions/2026/09/24/rollout-parser-cases.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(fixtures.join("codex-native-a9b0815/parser-cases.jsonl"), &path).unwrap();
    let input = Input { root: InputRoot::Codex, path: "sessions/2026/09/24/rollout-parser-cases.jsonl".into() };
    let namespace = Namespace::new("shared-semantic-parity").unwrap();
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone()).await.unwrap();
    store.publish_sources(&namespace, &options, std::slice::from_ref(&input)).await.unwrap().unwrap();
    crate::storage::conformance::produce(&Namespace::new("sqlite-parity").unwrap(), &sqlite).unwrap();
    let local = crate::session_catalog_page(&sqlite, "sqlite-parity", "limit=10");
    assert_eq!(local.status, 200);
    let remote = store.catalog_reply(&namespace, &options, "limit=10").await.unwrap();
    assert_eq!(remote.status, 200);
    let local: serde_json::Value = serde_json::from_slice(&local.body).unwrap();
    let remote: serde_json::Value = serde_json::from_slice(&remote.body).unwrap();
    for field in ["key", "name", "harness", "native_ids", "repo", "model", "tokens", "source_refs"] {
        assert_eq!(remote["items"][0][field], local["items"][0][field], "shared producer {field}");
    }
    let key = remote["items"][0]["key"].as_str().unwrap();
    for query in ["q=parser&harness=codex&limit=1", "q=parser&harness=claude&limit=1", "no_repo=1", "repo=fixture", "model=absent"] {
        let a = crate::session_catalog_page(&sqlite, "sqlite-parity", query);
        let b = store.catalog_reply(&namespace, &options, query).await.unwrap();
        assert_eq!(a.status, b.status, "{query}");
        let a: serde_json::Value = serde_json::from_slice(&a.body).unwrap();
        let b: serde_json::Value = serde_json::from_slice(&b.body).unwrap();
        let keys = |value: &serde_json::Value| value["items"].as_array().unwrap().iter().map(|row| row["key"].clone()).collect::<Vec<_>>();
        assert_eq!(keys(&a), keys(&b), "{query}");
    }
    let tx = store.begin(&namespace).await.unwrap();
    let row = tx.catalog_row(&namespace, key, CatalogReadScope::Current).await.unwrap().unwrap();
    let source = row.sources[0].clone();
    let tag = tx.ledger(&namespace, path.to_str().unwrap()).await.unwrap().unwrap();
    let proof = tx.proof_block(&namespace, path.to_str().unwrap(), tag, 0).await.unwrap().unwrap();
    tx.commit(&namespace).await.unwrap();
    let reader = NativeReader { path, source, proof };
    let remote = store.transcript_reply(&namespace, &options, key, CatalogReadScope::Current, None, 0, 10, &reader).await.unwrap();
    let query = format!("sid={}&after=0&limit=10", crate::viewer::percent_encode(key));
    let local = crate::session_transcript_range(&sqlite, "sqlite-parity", &query, None);
    assert_eq!(remote.status, local.status);
    let remote: serde_json::Value = serde_json::from_slice(&remote.body).unwrap();
    let local: serde_json::Value = serde_json::from_slice(&local.body).unwrap();
    let texts = |value: &serde_json::Value| value["entries"].as_array().unwrap().iter().map(|entry| (entry["k"].clone(), entry["text"].clone(), entry["out"].clone(), entry["cmd"].clone())).collect::<Vec<_>>();
    assert_eq!(texts(&remote), texts(&local));
    assert!(!crate::events::EventCache::path(&options.cache).exists());
}

pub(super) async fn lifecycle(dsn: &str) {
    let root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&root.0, "pg-producer");
    let path =
        crate::storage::conformance::native(&root.0, "pg-produced", "synthetic producer prompt");
    let namespace = Namespace::new("native-producer").unwrap();
    let inputs = vec![Input {
        root: InputRoot::Claude,
        path: "projects/project/pg-produced.jsonl".into(),
    }];
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    #[cfg(target_os = "linux")]
    let sqlite_watch = SqliteOpenWatch::new(&options);
    let start = std::time::Instant::now();
    let first = store
        .publish_sources(&namespace, &options, &inputs)
        .await
        .unwrap()
        .unwrap();
    let served = store
        .catalog_reply(&namespace, &options, "limit=10")
        .await
        .unwrap();
    assert_eq!(served.status, 200);
    let served: serde_json::Value = serde_json::from_slice(&served.body).unwrap();
    assert_eq!(served["items"].as_array().unwrap().len(), 1);
    assert_eq!(served["items"][0]["harness"], "claude");
    let tx = store.begin(&namespace).await.unwrap();
    let page = tx
        .catalog_page(&namespace, CatalogReadScope::Current, None, 10)
        .await
        .unwrap();
    eprintln!(
        "postgres-shared-producer-catalog-first-page-us={}",
        start.elapsed().as_micros()
    );
    assert_eq!(page.generation, first);
    assert_eq!(page.rows.len(), 1);
    let row = &page.rows[0];
    let key = row.key.clone();
    assert_eq!(row.harness, "claude");
    assert_eq!(row.native_ids, ["pg-produced"]);
    let (recipe, total, slots) = tx
        .recipe_page(&namespace, &key, CatalogReadScope::Current, None, 0, 10)
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(slots.len(), 1);
    assert!(matches!(slots[0].recipe, crate::slot_projection::Recipe::U));
    assert_eq!(slots[0].file.as_ref().unwrap().path, path);
    let source = row.sources[0].clone();
    let proof = tx
        .proof_block(
            &namespace,
            path.to_str().unwrap(),
            tx.ledger(&namespace, path.to_str().unwrap())
                .await
                .unwrap()
                .unwrap(),
            0,
        )
        .await
        .unwrap()
        .unwrap();
    tx.commit(&namespace).await.unwrap();
    let reader = NativeReader {
        path: path.clone(),
        source,
        proof,
    };
    let selected = store
        .transcript_reply(
            &namespace,
            &options,
            &key,
            CatalogReadScope::Current,
            None,
            0,
            10,
            &reader,
        )
        .await
        .unwrap();
    assert_eq!(selected.status, 200);
    let selected: serde_json::Value = serde_json::from_slice(&selected.body).unwrap();
    assert_eq!(selected["entries"][0]["text"], "synthetic producer prompt");
    assert_eq!(selected["entries"][0]["freshness"]["state"], "cached");
    assert!(selected["entries"][0]["entry_id"].as_str().is_some());
    // Restart and append use the same native producer and persisted parser base.
    drop(store);
    std::io::Write::write_all(&mut std::fs::OpenOptions::new().append(true).open(&path).unwrap(), format!("{}\n", serde_json::json!({"type":"assistant","sessionId":"pg-produced","uuid":"answer","timestamp":"2026-10-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"synthetic answer"}]}})).as_bytes()).unwrap();
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let next = store
        .publish_sources(&namespace, &options, &inputs)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first, next);
    let tx = store.begin(&namespace).await.unwrap();
    for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
        let (generation, total, slots) = tx
            .recipe_page(&namespace, &key, scope, None, 0, 10)
            .await
            .unwrap();
        assert_ne!(generation, recipe);
        assert_eq!(total, 2);
        assert_eq!(slots.len(), 2);
    }
    tx.commit(&namespace).await.unwrap();
    let foreign = Namespace::new("foreign-native-producer").unwrap();
    assert!(
        store
            .publish_sources(&foreign, &options, &inputs)
            .await
            .is_err()
    );
    let mut other = PgProjectionStore::new(super::connect(dsn).await, foreign.clone())
        .await
        .unwrap();
    let tx = other.begin(&foreign).await.unwrap();
    assert!(
        tx.catalog_row(&foreign, &key, CatalogReadScope::Current)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&foreign).await.unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    let intent = tx.begin_raw_write(&namespace, &inputs[0]).await.unwrap();
    tx.commit(&namespace).await.unwrap();
    assert!(store.publish_sources(&namespace, &options, &inputs).await.is_err());
    assert_eq!(store.catalog_reply(&namespace, &options, "limit=10").await.unwrap().status, 503);
    let tx = store.begin(&namespace).await.unwrap();
    assert!(tx.complete_raw_write(&namespace, intent, &crate::PgRawGeneration { generation: "b".repeat(32), length: std::fs::metadata(&path).unwrap().len(), sha256: [8; 32] }).await.unwrap());
    tx.commit(&namespace).await.unwrap();
    assert_eq!(store.catalog_reply(&namespace, &options, "limit=10").await.unwrap().status, 200);
    #[cfg(target_os = "linux")]
    sqlite_watch.assert_untouched(&options);
    assert!(!crate::events::EventCache::path(&options.cache).exists());
    assert!(!options.cache.with_extension("sqlite3-wal").exists());
    assert!(!options.cache.with_extension("sqlite3-shm").exists());
    semantic_parity(dsn).await;
}
