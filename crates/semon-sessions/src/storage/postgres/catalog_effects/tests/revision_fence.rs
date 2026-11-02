use super::*;
use std::{cell::RefCell, fs, io::Write, rc::Rc, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
struct FenceState {
    claims: Vec<(Vec<u8>, i64, String, i64)>,
    continuations: Vec<(Vec<u8>, Vec<u8>)>,
    head: Option<String>,
}

fn state(
    pool: &crate::storage::postgres::PgPool,
    namespace: &crate::storage::LogicalSourceNamespace,
    control: &PgCancellation,
) -> Result<FenceState, crate::storage::StorageError> {
    let namespace_bytes = namespace.as_str().as_bytes().to_vec();
    pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
        let claims: Vec<(Vec<u8>, i64, String, i64)> = tx
            .driver
            .query(
                "SELECT source_path,revision,reason,observed_at FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 ORDER BY source_path",
                &[&namespace_bytes],
            )
            .map_err(crate::storage::postgres::sqlstate::error)?
            .iter()
            .map(|row| {
                Ok((
                    row.try_get(0).map_err(|_| crate::storage::StorageError::Unavailable)?,
                    row.try_get(1).map_err(|_| crate::storage::StorageError::Unavailable)?,
                    row.try_get(2).map_err(|_| crate::storage::StorageError::Unavailable)?,
                    row.try_get(3).map_err(|_| crate::storage::StorageError::Unavailable)?,
                ))
            })
            .collect::<Result<Vec<_>, crate::storage::StorageError>>()?;
        let continuations: Vec<(Vec<u8>, Vec<u8>)> = tx
            .driver
            .query(
                "SELECT source_path,evidence FROM semon_pg_native_source_continuations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 ORDER BY source_path",
                &[&namespace_bytes],
            )
            .map_err(crate::storage::postgres::sqlstate::error)?
            .iter()
            .map(|row| {
                Ok((
                    row.try_get(0).map_err(|_| crate::storage::StorageError::Unavailable)?,
                    row.try_get(1).map_err(|_| crate::storage::StorageError::Unavailable)?,
                ))
            })
            .collect::<Result<Vec<_>, crate::storage::StorageError>>()?;
        let head: Option<String> = tx
            .driver
            .query_one(
                "SELECT catalog_generation FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
                &[&namespace_bytes],
            )
            .map_err(crate::storage::postgres::sqlstate::error)?
            .try_get(0)
            .map_err(|_| crate::storage::StorageError::Unavailable)?;
        Ok(FenceState {
            claims,
            continuations,
            head,
        })
    })
}

#[test]
fn pg_full_catalog_revision_fence_counts_owned_scan_and_refuses_external_advance_atomically() {
    let (pool, _) = fixture(|_| {});
    let namespace = ns("catalog-revision-fence");
    let control = PgCancellation::default();
    let host_source = crate::storage::HostSource::new(namespace.clone(), "c".repeat(64)).unwrap();
    let config = host_source
        .store_config(crate::storage::StoreBackend::Postgres {
            pool: pool.clone(),
            control: control.clone(),
        })
        .unwrap();
    config.create_native_store().unwrap();
    let root = std::env::temp_dir().join(format!(
        "pg-catalog-revision-fence-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let options = Options {
        source: host_source.with_store(config).unwrap(),
        claude_home: root.join("claude"),
        claude_json: root.join("claude.json"),
        codex_home: root.join("codex"),
        copilot_home: root.join("copilot"),
        proc_root: root.join("proc"),
        cache: root.join("index.json"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    fs::create_dir_all(&options.proc_root).unwrap();
    fs::write(options.proc_root.join("locks"), "").unwrap();
    let path = options
        .claude_home
        .join("projects/project/revision-fence.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let lines = [
        serde_json::json!({"type":"user","sessionId":"revision-fence","uuid":"revision-fence-u1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:00Z","message":{"role":"user","content":"sanitized prompt"}}),
        serde_json::json!({"type":"assistant","sessionId":"revision-fence","uuid":"revision-fence-a0","parentUuid":"revision-fence-u1","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:01Z","message":{"id":"revision-fence-a0","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized answer"}]}}),
    ];
    fs::write(
        &path,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let path_text = path.to_string_lossy().into_owned();
    let input = crate::inputs::Input {
        root: crate::inputs::InputRoot::Claude,
        path: "projects/project/revision-fence.jsonl".into(),
    };

    let mut cache =
        crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
    cache.begin_scan().unwrap();
    let baseline = cache.full_catalog_start().unwrap();
    assert!(baseline.pending_paths.is_empty());
    let mut dirty = false;
    let mut texts = crate::model::Texts::default();
    crate::model::observe_catalog(
        &options,
        &mut cache,
        &mut dirty,
        &mut texts,
        crate::model::now_ms(),
    )
    .unwrap();
    let owned_delta = cache.source_revision_delta();
    assert!(
        owned_delta > 0,
        "the full scan must commit a native source write"
    );
    let expected = baseline
        .revision
        .checked_add(i64::try_from(owned_delta).unwrap())
        .unwrap();
    assert_eq!(cache.full_catalog_start().unwrap().revision, expected);
    assert!(cache.session_catalog_generation().is_some());

    let ledger = cache
        .stored_ledger_batch(std::slice::from_ref(&path_text))
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    let from = ledger.stat.size;
    let append = format!(
        "{}\n",
        serde_json::json!({"type":"assistant","sessionId":"revision-fence","uuid":"revision-fence-a1","parentUuid":"revision-fence-a0","cwd":"/synthetic/project","timestamp":"2026-10-01T00:00:02Z","message":{"id":"revision-fence-a1","role":"assistant","model":"claude-test","content":[{"type":"text","text":"sanitized append"}]}})
    );
    let generation = crate::source_change::stat_generation(&ledger.stat).unwrap();
    let writer =
        crate::SessionSourceWrite::begin(&options, &input, Some(&generation), from).unwrap();
    let mut raw = fs::OpenOptions::new().append(true).open(&path).unwrap();
    raw.write_all(append.as_bytes()).unwrap();
    raw.sync_all().unwrap();
    drop(raw);
    writer.complete(from + append.len() as u64).unwrap();

    cache.begin_scan().unwrap();
    let baseline = cache.full_catalog_start().unwrap();
    assert!(baseline.pending_paths.contains(&path_text));
    let hook_pool = pool.clone();
    let hook_namespace = namespace.clone();
    let hook_control = control.clone();
    let hook_options = options.clone();
    let hook_input = input.clone();
    let committed_state = Rc::new(RefCell::new(None));
    let captured_state = Rc::clone(&committed_state);
    crate::model::AFTER_SCAN.with(|hook| {
        assert!(hook.borrow().is_none());
        *hook.borrow_mut() = Some(Box::new(move || {
            crate::invalidate_session_sources(&hook_options, std::slice::from_ref(&hook_input))
                .unwrap();
            let snapshot = state(&hook_pool, &hook_namespace, &hook_control).unwrap();
            assert!(
                snapshot
                    .continuations
                    .iter()
                    .any(|(source_path, _)| source_path.as_slice() == path_text.as_bytes())
            );
            assert!(
                snapshot
                    .claims
                    .iter()
                    .any(|(source_path, _, _, _)| source_path.as_slice() == path_text.as_bytes())
            );
            assert!(snapshot.head.is_some());
            *captured_state.borrow_mut() = Some(snapshot);
        }));
    });
    let mut dirty = false;
    let mut texts = crate::model::Texts::default();
    let error = crate::model::observe_catalog(
        &options,
        &mut cache,
        &mut dirty,
        &mut texts,
        crate::model::now_ms(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert!(crate::model::AFTER_SCAN.with(|hook| hook.borrow().is_none()));
    let owned_delta = cache.source_revision_delta();
    assert!(
        owned_delta > 0,
        "the second full scan must commit its append"
    );
    let expected = baseline
        .revision
        .checked_add(i64::try_from(owned_delta).unwrap())
        .unwrap();
    assert_eq!(cache.full_catalog_start().unwrap().revision, expected + 1);
    let committed_state = committed_state.borrow().clone().unwrap();
    assert_eq!(state(&pool, &namespace, &control).unwrap(), committed_state);

    drop(cache);
    fs::remove_dir_all(root).unwrap();
}
