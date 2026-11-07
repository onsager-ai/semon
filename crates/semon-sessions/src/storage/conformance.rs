//! Port conformance for the SQLite adapter: namespace collision, refusal
//! without partial success, capability separation, reopen, version stamp and
//! projection tags. Slice 3 runs the same cases against PostgreSQL.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

#[cfg(feature = "postgres")]
#[path = "postgres_tests.rs"]
mod postgres;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

use super::{
    Namespace, ProjectionTag, SessionStore, StorageError, TagCheck, sqlite::SqliteSessionStore,
};
use crate::{Options, events::EventCache};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root() -> Root {
    let path = std::env::temp_dir().join(format!(
        "semon-storage-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(path.join("proc")).unwrap();
    fs::write(path.join("proc/locks"), "").unwrap();
    Root(path)
}

/// One source placed on its own index under `root/name`. Every source of a
/// test observes the same native home, so their native paths collide.
fn source(root: &Path, name: &str) -> Options {
    fs::create_dir_all(root.join(name)).unwrap();
    let options = Options {
        claude_home: root.join("claude"),
        claude_json: root.join(".claude.json"),
        codex_home: root.join("codex"),
        copilot_home: root.join("copilot"),
        proc_root: root.join("proc"),
        cache: root.join(name).join("index.json"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: Some(root.join(name).join("facts.json")),
        scan_window: false,
    };
    crate::write_facts(
        options.facts.as_ref().unwrap(),
        &crate::Facts {
            version: crate::FACTS_VERSION,
            hostname: format!("synthetic-{name}"),
            repos: BTreeMap::from([("/synthetic/project".into(), Some("project".into()))]),
            ..crate::Facts::default()
        },
    )
    .unwrap();
    options
}

fn native(root: &Path, id: &str, prompt: &str) -> PathBuf {
    let path = root.join(format!("claude/projects/project/{id}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(
            "{}\n",
            json!({"type":"user","sessionId":id,"uuid":id,"parentUuid":null,"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":prompt}})
        ),
    )
    .unwrap();
    path
}

/// The ordinary producer publishes the Catalog from Raw.
fn publish(options: &Options) {
    let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
    let mut work = crate::viewer::lock(&evidence.storage.work);
    while !evidence
        .refresh_catalog(&mut work, crate::model::now_ms())
        .unwrap()
    {}
    drop(work);
    evidence.retire();
}

/// The production producer of a host-configured source: it binds the
/// database to the source's namespace and refuses one bound to another.
fn produce(namespace: &Namespace, options: &Options) -> std::io::Result<()> {
    let evidence =
        crate::native_evidence::NativeEvidence::for_source(namespace.clone(), options.clone());
    let mut work = crate::viewer::lock(&evidence.storage.work);
    let result = loop {
        match evidence.refresh_catalog(&mut work, crate::model::now_ms()) {
            Ok(true) => break Ok(()),
            Ok(false) => {}
            Err(error) => break Err(error),
        }
    };
    drop(work);
    evidence.retire();
    result
}

fn bound(options: &Options) -> Option<String> {
    database(options)
        .query_row(
            "SELECT value FROM meta WHERE key='storage_namespace'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn request(query: &str) -> crate::catalog::Request {
    crate::catalog::Request::parse(query).unwrap()
}

fn read(
    store: &SqliteSessionStore,
    bound: &Namespace,
    requested: &Namespace,
) -> Result<(u16, Value), StorageError> {
    let reply = store
        .reader(bound)?
        .page(requested, &request("limit=10"), None)?;
    Ok((reply.status, serde_json::from_slice(&reply.body).unwrap()))
}

/// The served status, generation and item count of one page read.
fn summary(read: Result<(u16, Value), StorageError>) -> (u16, Value, usize) {
    let (status, body) = read.unwrap();
    let items = body["items"].as_array().map_or(0, Vec::len);
    (status, body["generation"].clone(), items)
}

fn namespace(name: &str) -> Namespace {
    Namespace::new(name).unwrap()
}

fn database(options: &Options) -> Connection {
    Connection::open(EventCache::path(&options.cache)).unwrap()
}

fn observe(store: &SqliteSessionStore, namespace: &Namespace) {
    store
        .index_writer(namespace)
        .unwrap()
        .observe(namespace)
        .unwrap();
}

#[test]
fn storage_invalid_host_keys_return_errors_before_opening_an_index() {
    let root = root();
    for key in ["invalid\0source".to_owned(), "x".repeat(4097)] {
        let options = source(&root.0, if key.contains('\0') { "nul" } else { "long" });
        let evidence =
            crate::native_evidence::NativeEvidence::for_source_key(&key, options.clone());
        let error = match evidence.open_index() {
            Ok(_) => panic!("invalid namespace opened an index"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        let observer = crate::SessionCatalogObserver::new(&key, options.clone());
        assert_eq!(
            observer.demand().unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        observer.close();
        assert!(!EventCache::path(&options.cache).exists());
        assert!(crate::collect(&options, &key).is_err());
        let mut facts = crate::FactsSource::new(&options, &key);
        assert!(facts.facts().is_err());
        evidence.retire();
    }
}

#[test]
fn storage_custody_writers_and_proofs_refuse_another_namespace_without_changes() {
    let root = root();
    native(&root.0, "selected", "synthetic prompt");
    let options = source(&root.0, "custody");
    let owner = namespace("owner");
    produce(&owner, &options).unwrap();
    let before = database(&options)
        .query_row(
            "SELECT COUNT(*) FROM session_catalog_invalidations",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let input = crate::inputs(&options).unwrap().remove(0);
    assert!(crate::SessionSourceWrite::begin(&options, "other", &input, None, 0).is_err());
    assert!(crate::SessionFactsWrite::begin(&options, "other", &crate::Facts::default()).is_err());
    assert!(crate::collect(&options, "other").is_err());
    assert!(crate::FactsSource::new(&options, "other").facts().is_err());
    assert!(crate::session_source_proof(&options, "other", "claude", &input.path).is_err());
    let proof = crate::session_source_proof(&options, "owner", "claude", &input.path)
        .unwrap()
        .unwrap();
    let ready = crate::source_projection_ready(&options, "claude", &input.path)
        .unwrap()
        .unwrap();
    assert!(crate::retain_source_event_index(&options, "other", &proof, &ready).is_err());
    assert!(!crate::source_event_index_retained(&options, &proof, &ready).unwrap());
    assert_eq!(bound(&options).as_deref(), Some("owner"));
    assert_eq!(
        database(&options)
            .query_row(
                "SELECT COUNT(*) FROM session_catalog_invalidations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        before
    );
}

#[test]
fn storage_sources_observing_one_native_path_keep_their_own_namespaces() {
    let root = root();
    let path = native(&root.0, "shared", "synthetic prompt");
    let (a, b) = (source(&root.0, "a"), source(&root.0, "b"));
    let (na, nb) = (namespace("machine-a"), namespace("machine-b"));
    let store = SqliteSessionStore::new([(na.clone(), a.clone()), (nb.clone(), b.clone())]);
    for namespace in [&na, &nb] {
        observe(&store, namespace);
    }
    publish(&a);
    publish(&b);
    for namespace in [&na, &nb] {
        let (status, body) = read(&store, namespace, namespace).unwrap();
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["machine"], namespace.as_str());
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{body}");
        assert_eq!(items[0]["native_ids"], json!(["shared"]));
    }
    // One native path, one Raw identity, two separately bound databases.
    let path = path.to_str().unwrap();
    let tag = |namespace: &Namespace| {
        store
            .index_writer(namespace)
            .unwrap()
            .ledger_tag(namespace, path)
            .unwrap()
            .unwrap()
    };
    assert_eq!(tag(&na), tag(&nb));
    for (options, expected) in [(&a, &na), (&b, &nb)] {
        let bound: String = database(options)
            .query_row(
                "SELECT value FROM meta WHERE key='storage_namespace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bound, expected.as_str());
    }
    // A placement that points one namespace at the other's database is
    // refused by that database's binding, for every capability.
    let swapped = SqliteSessionStore::new([(na.clone(), b.clone())]);
    assert!(matches!(
        read(&swapped, &na, &na),
        Err(StorageError::WrongNamespace { .. })
    ));
    assert!(matches!(
        swapped.index_writer(&na),
        Err(StorageError::WrongNamespace { .. })
    ));
    assert!(matches!(
        swapped.publisher(&na),
        Err(StorageError::WrongNamespace { .. })
    ));
    assert!(matches!(
        store.reader(&namespace("machine-c")),
        Err(StorageError::Unplaced(_))
    ));
}

#[test]
fn storage_wrong_namespace_is_refused_without_partial_success() {
    let root = root();
    native(&root.0, "selected", "synthetic prompt");
    let (a, b) = (source(&root.0, "a"), source(&root.0, "b"));
    let (na, nb) = (namespace("machine-a"), namespace("machine-b"));
    let store = SqliteSessionStore::new([(na.clone(), a.clone()), (nb.clone(), b)]);
    observe(&store, &na);
    publish(&a);
    let before = summary(read(&store, &na, &na));
    assert_eq!((before.0, before.2), (200, 1));

    let reader = store.reader(&na).unwrap();
    assert_eq!(reader.namespace(), &na);
    assert!(matches!(
        reader.page(&nb, &request("limit=10"), None),
        Err(StorageError::WrongNamespace { .. })
    ));
    let mut writer = store.index_writer(&na).unwrap();
    assert_eq!(writer.namespace(), &na);
    assert!(matches!(
        writer.observe(&nb),
        Err(StorageError::WrongNamespace { .. })
    ));
    assert!(matches!(
        writer.ledger_tag(&nb, "unused"),
        Err(StorageError::WrongNamespace { .. })
    ));
    drop(writer);
    let mut publisher = store.publisher(&na).unwrap();
    assert_eq!(publisher.namespace(), &na);
    let generation = publisher.generation(&na).unwrap();
    assert!(generation.is_some());
    assert!(matches!(
        publisher.generation(&nb),
        Err(StorageError::WrongNamespace { .. })
    ));
    let empty = crate::slot_projection::Publication {
        rows: &[],
        transcripts: None,
    };
    assert!(matches!(
        publisher.publish(&nb, &empty, generation.as_deref()),
        Err(StorageError::WrongNamespace { .. })
    ));
    assert_eq!(publisher.generation(&na).unwrap(), generation);
    drop(publisher);
    // The refused operations left the namespace's database as it was, and
    // the other namespace's database was never created.
    assert_eq!(summary(read(&store, &na, &na)), before);
    assert!(!EventCache::path(&root.0.join("b/index.json")).exists());
}

#[test]
fn storage_capabilities_are_separate_and_bindings_survive_reopen() {
    let root = root();
    native(&root.0, "selected", "synthetic prompt");
    let a = source(&root.0, "a");
    let na = namespace("machine-a");
    let store = SqliteSessionStore::new([(na.clone(), a.clone())]);
    // A publisher never creates or binds a database.
    assert!(matches!(
        store.publisher(&na),
        Err(StorageError::Unbound(_))
    ));
    assert!(!EventCache::path(&a.cache).exists());
    // The ordinary producer creates an unbound, stamped database: readers
    // may read it, but publication still needs an index writer's binding.
    publish(&a);
    let (status, body) = read(&store, &na, &na).unwrap();
    assert_eq!(status, 200, "{body}");
    let stamp: String = database(&a)
        .query_row(
            "SELECT value FROM meta WHERE key='storage_stamp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stamp, super::sqlite::stamp());
    assert!(matches!(
        store.publisher(&na),
        Err(StorageError::Unbound(_))
    ));
    observe(&store, &na);
    assert!(store.publisher(&na).is_ok());
    drop(store);

    // Reopening keeps the binding: the same namespace reads and writes, and
    // another namespace placed on this database is still refused.
    let reopened = SqliteSessionStore::new([(na.clone(), a.clone())]);
    assert_eq!(read(&reopened, &na, &na).unwrap().0, 200);
    assert!(reopened.index_writer(&na).is_ok());
    assert!(reopened.publisher(&na).is_ok());
    let other = namespace("machine-b");
    let misplaced = SqliteSessionStore::new([(other.clone(), a.clone())]);
    assert!(matches!(
        read(&misplaced, &other, &other),
        Err(StorageError::WrongNamespace { .. })
    ));
    assert!(matches!(
        misplaced.index_writer(&other),
        Err(StorageError::WrongNamespace { .. })
    ));

    // A database without this binary's stamp is refused, not reinterpreted.
    let connection = database(&a);
    connection
        .execute(
            "UPDATE meta SET value='semon-session-store/0' WHERE key='storage_stamp'",
            [],
        )
        .unwrap();
    assert!(matches!(
        read(&reopened, &na, &na),
        Err(StorageError::Unsupported(_))
    ));
    assert!(reopened.index_writer(&na).is_err());
    assert!(reopened.publisher(&na).is_err());
    connection
        .execute(
            "UPDATE meta SET value=?1 WHERE key='storage_stamp'",
            [super::sqlite::stamp()],
        )
        .unwrap();
    assert_eq!(read(&reopened, &na, &na).unwrap().0, 200);
}

#[test]
fn storage_projection_tag_mismatches_are_refused_and_rebuilt_from_raw() {
    let root = root();
    let path = native(&root.0, "selected", "synthetic prompt");
    let a = source(&root.0, "a");
    let na = namespace("machine-a");
    let store = SqliteSessionStore::new([(na.clone(), a.clone())]);
    observe(&store, &na);
    publish(&a);
    let served = summary(read(&store, &na, &na));
    assert_eq!((served.0, served.2), (200, 1));
    let connection = database(&a);
    let key = path.to_str().unwrap();
    let stale = |change: &str, restore: &str| {
        connection.execute(change, [key]).unwrap();
        let (status, body) = read(&store, &na, &na).unwrap();
        assert_eq!(
            (status, &body["error"]["code"]),
            (503, &json!("projection_stale")),
            "{change}"
        );
        assert_eq!(body["error"]["retryable"], true);
        connection.execute(restore, [key]).unwrap();
        assert_eq!(summary(read(&store, &na, &na)), served, "{restore}");
    };
    let prefix: Vec<u8> = connection
        .query_row(
            "SELECT head_sha256 FROM files WHERE path=?1",
            [key],
            |row| row.get(0),
        )
        .unwrap();
    // An altered consumed prefix at the published boundary.
    connection
        .execute(
            "UPDATE files SET head_sha256=zeroblob(32) WHERE path=?1",
            [key],
        )
        .unwrap();
    let (status, body) = read(&store, &na, &na).unwrap();
    assert_eq!(
        (status, &body["error"]["code"]),
        (503, &json!("projection_stale"))
    );
    connection
        .execute(
            "UPDATE files SET head_sha256=?2 WHERE path=?1",
            rusqlite::params![key, prefix],
        )
        .unwrap();
    assert_eq!(summary(read(&store, &na, &na)), served);
    // A ledger shorter than the publication, at the same or another Raw
    // identity.
    stale(
        "UPDATE files SET ino=ino+1,resume_at=resume_at-1 WHERE path=?1",
        "UPDATE files SET ino=ino-1,resume_at=resume_at+1 WHERE path=?1",
    );
    // A content-identical copy at another Raw identity is the same prefix.
    connection
        .execute("UPDATE files SET ino=ino+1 WHERE path=?1", [key])
        .unwrap();
    assert_eq!(summary(read(&store, &na, &na)), served);
    connection
        .execute("UPDATE files SET ino=ino-1 WHERE path=?1", [key])
        .unwrap();
    stale(
        "UPDATE files SET resume_at=resume_at-1 WHERE path=?1",
        "UPDATE files SET resume_at=resume_at+1 WHERE path=?1",
    );
    // An append-only ledger advance keeps the published prefix readable.
    connection
        .execute(
            "UPDATE files SET resume_at=resume_at+1 WHERE path=?1",
            [key],
        )
        .unwrap();
    assert_eq!(read(&store, &na, &na).unwrap().0, 200);
    connection
        .execute(
            "UPDATE files SET resume_at=resume_at-1 WHERE path=?1",
            [key],
        )
        .unwrap();
    // A unit from another projector version is refused with its database.
    connection
        .execute("UPDATE meta SET value='0' WHERE key='catalog_version'", [])
        .unwrap();
    assert!(!matches!(read(&store, &na, &na), Ok((200, _))));
    connection
        .execute(
            "UPDATE meta SET value=?1 WHERE key='catalog_version'",
            [crate::model::summary::CATALOG_VERSION.to_string()],
        )
        .unwrap();
    assert_eq!(summary(read(&store, &na, &na)), served);
    drop(connection);

    // Rewriting Raw in place changes the consumed prefix; the producer
    // rebuilds the unit from Raw and its new tag is served.
    let before = store
        .index_writer(&na)
        .unwrap()
        .ledger_tag(&na, key)
        .unwrap()
        .unwrap();
    let path = native(&root.0, "selected", "synthetic answer");
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    publish(&a);
    let after = store
        .index_writer(&na)
        .unwrap()
        .ledger_tag(&na, key)
        .unwrap()
        .unwrap();
    assert_ne!(before.prefix_sha256, after.prefix_sha256);
    let (status, body) = read(&store, &na, &na).unwrap();
    assert_eq!(status, 200, "{body}");
}

#[test]
fn storage_projection_tag_check_classifies_raw_prefix_and_projector() {
    let tag = ProjectionTag {
        dev: 1,
        ino: 2,
        offset: 10,
        prefix_sha256: [3; 32],
        tail_sha256: [4; 32],
        epoch: 7,
        projector: crate::model::summary::CATALOG_VERSION,
    };
    assert_eq!(tag.check(&tag), TagCheck::Current);
    let advanced = ProjectionTag {
        offset: 20,
        prefix_sha256: [5; 32],
        tail_sha256: [6; 32],
        ..tag
    };
    assert_eq!(tag.check(&advanced), TagCheck::Advanced);
    let copied = ProjectionTag { ino: 9, ..advanced };
    assert_eq!(tag.check(&copied), TagCheck::Relocated);
    // Past the boundary at the same Raw identity, a replaced ledger is no
    // append of the published prefix.
    assert_eq!(
        tag.check(&ProjectionTag {
            epoch: 8,
            ..advanced
        }),
        TagCheck::Mismatch("ledger_epoch")
    );
    assert_eq!(
        tag.check(&ProjectionTag { epoch: 8, ..tag }),
        TagCheck::Current
    );
    assert_eq!(
        tag.check(&ProjectionTag { dev: 9, ..tag }),
        TagCheck::Current
    );
    for (committed, reason) in [
        (ProjectionTag { offset: 9, ..tag }, "consumed_prefix"),
        (
            ProjectionTag {
                ino: 9,
                offset: 9,
                ..tag
            },
            "consumed_prefix",
        ),
        (
            ProjectionTag {
                prefix_sha256: [0; 32],
                ..tag
            },
            "consumed_prefix",
        ),
        (
            ProjectionTag {
                tail_sha256: [0; 32],
                ..tag
            },
            "consumed_prefix",
        ),
        (
            ProjectionTag {
                projector: tag.projector + 1,
                ..tag
            },
            "projector_version",
        ),
    ] {
        assert_eq!(tag.check(&committed), TagCheck::Mismatch(reason));
    }
    assert!(Namespace::new(&"n".repeat(4097)).is_err());
    assert!(Namespace::new("a\0b").is_err());
}

#[test]
fn storage_production_producer_binds_its_namespace_and_refuses_another() {
    let root = root();
    native(&root.0, "selected", "synthetic prompt");
    let (a, local) = (source(&root.0, "a"), source(&root.0, "local"));
    let na = namespace("machine-a");
    produce(&na, &a).unwrap();
    assert_eq!(bound(&a).as_deref(), Some("machine-a"));
    let store = SqliteSessionStore::new([(na.clone(), a.clone())]);
    let served = summary(read(&store, &na, &na));
    assert_eq!((served.0, served.2), (200, 1));
    let files = |options: &Options| -> i64 {
        database(options)
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap()
    };
    let indexed = files(&a);

    // Another source configured on this database is refused before any
    // index or Catalog write.
    let error = produce(&namespace("machine-b"), &a).unwrap_err();
    assert!(error.to_string().contains("another namespace"), "{error}");
    assert_eq!(bound(&a).as_deref(), Some("machine-a"));
    assert_eq!(files(&a), indexed);
    assert_eq!(summary(read(&store, &na, &na)), served);

    // The local source's empty key is bound as `local`; both spellings read
    // it and another namespace placed on it is refused.
    produce(&Namespace::local(), &local).unwrap();
    assert_eq!(bound(&local).as_deref(), Some("local"));
    let named = namespace("local");
    let locals = SqliteSessionStore::new([
        (Namespace::local(), local.clone()),
        (named.clone(), local.clone()),
    ]);
    assert_eq!(
        read(&locals, &Namespace::local(), &Namespace::local())
            .unwrap()
            .0,
        200
    );
    assert_eq!(read(&locals, &named, &named).unwrap().0, 200);
    let misplaced = SqliteSessionStore::new([(na.clone(), local.clone())]);
    assert!(matches!(
        read(&misplaced, &na, &na),
        Err(StorageError::WrongNamespace { .. })
    ));
    assert!(produce(&na, &local).is_err());
    assert_eq!(bound(&local).as_deref(), Some("local"));
}

#[test]
fn storage_same_inode_rewrite_past_the_published_boundary_is_refused_then_rebuilt() {
    use std::io::Write;
    let root = root();
    let path = native(&root.0, "selected", "synthetic prompt");
    let a = source(&root.0, "a");
    let na = namespace("machine-a");
    let store = SqliteSessionStore::new([(na.clone(), a.clone())]);
    produce(&na, &a).unwrap();
    assert_eq!(read(&store, &na, &na).unwrap().0, 200);
    let key = path.to_str().unwrap();
    let tag = || {
        store
            .index_writer(&na)
            .unwrap()
            .ledger_tag(&na, key)
            .unwrap()
            .unwrap()
    };
    let recorded = || -> i64 {
        database(&a)
            .query_row(
                "SELECT ledger_epoch FROM session_catalog_sources WHERE source_path=?1 AND lifecycle='current'",
                [key],
                |row| row.get(0),
            )
            .unwrap()
    };
    let published = tag();
    assert_eq!(recorded(), published.epoch);
    let record = |uuid: &str, prompt: &str| json!({"type":"user","sessionId":"selected","uuid":uuid,"parentUuid":"selected","cwd":"/synthetic/project","timestamp":"2026-10-01T00:01:00Z","message":{"role":"user","content":prompt}});

    // An append keeps the ledger epoch: the published prefix stays readable.
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", record("appended", "synthetic append")).unwrap();
    drop(file);
    observe(&store, &na);
    let appended = tag();
    assert!(appended.offset > published.offset);
    assert_eq!(appended.epoch, published.epoch);
    assert_eq!(published.check(&appended), TagCheck::Advanced);
    assert_eq!(read(&store, &na, &na).unwrap().0, 200);

    // Rewriting the same inode in place with other bytes that reach past the
    // published boundary is a replaced ledger, refused by its epoch alone.
    fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n",
            json!({"type":"user","sessionId":"selected","uuid":"selected","parentUuid":null,"cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"rewritten prompt"}}),
            record("rewritten-1", "rewritten answer"),
            record("rewritten-2", "rewritten again"),
        ),
    )
    .unwrap();
    observe(&store, &na);
    let rewritten = tag();
    assert_eq!(
        (rewritten.dev, rewritten.ino),
        (published.dev, published.ino)
    );
    assert!(rewritten.offset > published.offset);
    assert_ne!(rewritten.epoch, published.epoch);
    assert_eq!(
        published.check(&rewritten),
        TagCheck::Mismatch("ledger_epoch")
    );
    let (status, body) = read(&store, &na, &na).unwrap();
    assert_eq!(
        (status, &body["error"]["code"]),
        (503, &json!("projection_stale")),
        "{body}"
    );

    // The producer rebuilds the unit from Raw at the new epoch.
    produce(&na, &a).unwrap();
    assert_eq!(recorded(), tag().epoch);
    let (status, body) = read(&store, &na, &na).unwrap();
    assert_eq!(status, 200, "{body}");
}

#[test]
fn storage_withdrawal_during_a_failed_publication_keeps_history_in_custody() {
    let root = root();
    native(&root.0, "kept", "kept prompt");
    let withdrawn = native(&root.0, "withdrawn", "withdrawn prompt");
    let a = source(&root.0, "a");
    let na = namespace("machine-a");
    let store = SqliteSessionStore::new([(na.clone(), a.clone())]);
    produce(&na, &a).unwrap();
    let page = |query: &str| -> Value {
        let reply = store
            .reader(&na)
            .unwrap()
            .page(&na, &request(query), None)
            .unwrap();
        assert_eq!(reply.status, 200);
        serde_json::from_slice(&reply.body).unwrap()
    };
    let native_ids = |page: &Value| -> Vec<Value> {
        let mut ids: Vec<_> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["native_ids"].clone())
            .collect();
        ids.sort_by_key(Value::to_string);
        ids
    };
    assert_eq!(
        native_ids(&page("limit=10")),
        [json!(["kept"]), json!(["withdrawn"])]
    );
    let connection = database(&a);
    let catalog = || -> Vec<(String, String, String)> {
        connection
            .prepare(
                "SELECT session_key,lifecycle,metadata FROM session_catalog ORDER BY session_key",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let published = catalog();

    // Raw is withdrawn while a publication fails partway through its
    // transaction: nothing it wrote survives and the History is unchanged.
    fs::remove_file(&withdrawn).unwrap();
    connection
        .execute_batch("CREATE TRIGGER interrupt_publication BEFORE UPDATE ON session_catalog BEGIN SELECT RAISE(ABORT,'injected publication failure'); END;")
        .unwrap();
    let evidence = crate::native_evidence::NativeEvidence::for_source(na.clone(), a.clone());
    evidence.request_metadata_audit();
    let mut work = crate::viewer::lock(&evidence.storage.work);
    let failed = (0..64).any(|_| {
        evidence
            .refresh_catalog(&mut work, crate::model::now_ms())
            .is_err()
    });
    drop(work);
    evidence.retire();
    assert!(failed, "the injected publication failure was never reached");
    assert_eq!(catalog(), published);

    // The next publication hands the withdrawn session's History to custody:
    // it leaves Current and stays readable as retained History.
    connection
        .execute_batch("DROP TRIGGER interrupt_publication")
        .unwrap();
    produce(&na, &a).unwrap();
    assert_eq!(native_ids(&page("limit=10")), [json!(["kept"])]);
    let history = page("scope=retained_history&limit=10");
    assert_eq!(
        native_ids(&history),
        [json!(["kept"]), json!(["withdrawn"])]
    );
    let retained = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["native_ids"] == json!(["withdrawn"]))
        .unwrap();
    assert_eq!(retained["lifecycle"], "retained", "{retained}");
    assert_eq!(bound(&a).as_deref(), Some("machine-a"));
}

#[test]
fn admitted_local_custody_keeps_sqlite_default_and_refuses_retired_sources() {
    let root = root();
    let options = source(&root.0, "local-custody");
    let path = native(&root.0, "local-write", "synthetic custody prompt");
    let sources = crate::SessionSources::with_machines(vec![(String::new(), options.clone())]);
    let input = crate::Input {
        root: crate::InputRoot::Claude,
        path: "projects/project/local-write.jsonl".into(),
    };
    let length = fs::metadata(path).unwrap().len();
    crate::SessionSourceWrite::begin_in(&sources, "", &input, None, length)
        .unwrap()
        .complete(length)
        .unwrap();
    assert_eq!(bound(&options).as_deref(), Some("local"));
    assert!(crate::SessionSourceWrite::begin_in(&sources, "unknown", &input, None, 0).is_err());
    sources.retire();
    assert!(crate::SessionSourceWrite::begin_in(&sources, "local", &input, None, 0).is_err());
}
