use super::*;
use crate::{
    model::summary::CatalogRow,
    storage::{
        postgres::{
            PgCancellation,
            tests::{fixture, ns},
            txn::{PgTxn, TxnKind},
        },
        selected::selected_family,
    },
};

fn row(key: &str, parent: Option<&str>) -> CatalogRow {
    CatalogRow {
        overview: None,
        trace: None,
        query_recipe: None,
        query: None,
        lifecycle: Default::default(),
        key: key.to_owned(),
        name: key.to_owned(),
        harness: "claude".to_owned(),
        kind: "session".to_owned(),
        presentation_kind: None,
        native_ids: vec![key.to_owned()],
        parent: parent.map(str::to_owned),
        parent_source: None,
        repo: None,
        branch: None,
        model: String::new(),
        effort: None,
        start: Some(0),
        last: Some(0),
        tokens: [0.0; 3],
        cost: Default::default(),
        sources: Vec::new(),
    }
}

fn insert(
    tx: &mut PgTxn<'_>,
    namespace: &[u8],
    generation: &str,
    row: &CatalogRow,
    key_digest: Option<Vec<u8>>,
) -> Result<(), StorageError> {
    let metadata = serde_json::to_vec(row).map_err(|_| StorageError::Unavailable)?;
    let key = row.key.as_bytes();
    let key_digest = key_digest.unwrap_or_else(|| super::digest(key));
    let parent_key = row.parent.as_deref().map(str::as_bytes);
    tx.driver
        .execute(
            "INSERT INTO semon_pg_native_catalog_rows(namespace,generation,key_digest,session_key,parent_key,metadata,last_ms,harness,repo,model) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            &[
                &namespace,
                &generation.as_bytes(),
                &key_digest,
                &key,
                &parent_key,
                &metadata,
                &row.last.unwrap_or(0),
                &row.harness.as_bytes(),
                &row.repo.as_deref().unwrap_or("").as_bytes(),
                &row.model.as_bytes(),
            ],
        )
        .map(|_| ())
        .map_err(|_| StorageError::Unavailable)
}

fn insert_raw_key(
    tx: &mut PgTxn<'_>,
    namespace: &[u8],
    generation: &str,
    key: &[u8],
    parent: Option<&[u8]>,
) -> Result<(), StorageError> {
    tx.driver
        .execute(
            "INSERT INTO semon_pg_native_catalog_rows(namespace,generation,key_digest,session_key,parent_key,metadata,last_ms,harness,repo,model) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            &[
                &namespace,
                &generation.as_bytes(),
                &super::digest(key),
                &key,
                &parent,
                &&b"{}"[..],
                &0i64,
                &&b"claude"[..],
                &&b""[..],
                &&b""[..],
            ],
        )
        .map(|_| ())
        .map_err(|_| StorageError::Unavailable)
}

fn descendants_with_budget(
    pool: &crate::storage::postgres::PgPool,
    namespace: &crate::storage::LogicalSourceNamespace,
    generation: &str,
    key: &str,
    bytes: usize,
) -> Result<Vec<String>, StorageError> {
    let cancellation = PgCancellation::default();
    let selected = crate::selected_read::SelectedReadControl::new(
        std::time::Instant::now() + std::time::Duration::from_secs(20),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .with_limits(131_072, bytes);
    selected
        .scope(|| {
            super::super::with_generation(pool, namespace, &cancellation, generation, |snapshot| {
                crate::storage::selected::ContextLookup::descendants(snapshot, key)
                    .map_err(|_| StorageError::Busy)
            })
            .map_err(std::io::Error::other)
        })
        .map_err(|_| StorageError::Busy)
}

#[test]
fn pg_selected_family_is_scoped_and_bounded_by_its_parent_index() {
    let (pool, _) = fixture(|_| {});
    let namespace = ns("selected-family-scope");
    let foreign_namespace = ns("selected-family-foreign");
    let control = PgCancellation::default();
    let generation = "a".repeat(64);
    let other_generation = "b".repeat(64);
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let ns = namespace.as_str().as_bytes();
        for index in 0..80 {
            let unrelated = format!("unrelated-{index:03}");
            insert(tx, ns, &generation, &row(&unrelated, None), None)?;
        }
        for (key, parent) in [
            ("family-root", None),
            ("family-child", Some("family-root")),
            ("family-leaf", Some("family-child")),
            ("fanout-root", None),
            ("cycle-a", Some("cycle-c")),
            ("cycle-b", Some("cycle-a")),
            ("cycle-c", Some("cycle-b")),
            ("collision-root", None),
        ] {
            insert(tx, ns, &generation, &row(key, parent), None)?;
        }
        for index in 0..105 {
            let child = format!("fanout-child-{index:03}");
            insert(tx, ns, &generation, &row(&child, Some("fanout-root")), None)?;
        }
        let corrupt = row("collision-child", Some("collision-root"));
        insert(
            tx,
            ns,
            &generation,
            &corrupt,
            Some(super::digest(b"different-key")),
        )?;
        insert(
            tx,
            ns,
            &other_generation,
            &row("other-generation-child", Some("family-root")),
            None,
        )?;
        Ok(())
    })
    .unwrap();
    pool.transaction(&foreign_namespace, TxnKind::Write, &control, |tx| {
        insert(
            tx,
            foreign_namespace.as_str().as_bytes(),
            &generation,
            &row("foreign-namespace-child", Some("family-root")),
            None,
        )
    })
    .unwrap();

    let family =
        super::super::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            selected_family(
                snapshot,
                "family-root",
                &mut |_| serde_json::json!({"state":"current"}),
            )
            .map_err(|_| StorageError::Busy)
        })
        .unwrap();
    assert_eq!(family["state"], "complete");
    let family_keys = family["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(family_keys, vec!["family-child", "family-leaf"]);

    let fanout =
        super::super::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            selected_family(
                snapshot,
                "fanout-root",
                &mut |_| serde_json::json!({"state":"current"}),
            )
            .map_err(|_| StorageError::Busy)
        })
        .unwrap();
    assert_eq!(fanout["state"], "incomplete");
    assert_eq!(fanout["runs"].as_array().unwrap().len(), 99);
    let fanout_keys = fanout["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(fanout_keys.windows(2).all(|pair| pair[0] < pair[1]));

    let cycle =
        super::super::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            selected_family(
                snapshot,
                "cycle-a",
                &mut |_| serde_json::json!({"state":"current"}),
            )
            .map_err(|_| StorageError::Busy)
        })
        .unwrap();
    assert_eq!(cycle["state"], "incomplete");
    assert!(cycle["runs"].as_array().unwrap().is_empty());

    let collision =
        super::super::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            crate::storage::selected::ContextLookup::descendants(snapshot, "collision-root")
                .map_err(|_| StorageError::ScopeChanged)
        });
    assert_eq!(collision, Err(StorageError::ScopeChanged));
}

#[test]
fn pg_retained_family_harness_reuse_does_not_report_current_history_status() {
    let (pool, _) = fixture(|_| {});
    let namespace = ns("selected-family-harness-reuse");
    let control = PgCancellation::default();
    let generation = "a".repeat(64);
    let mut parent = row("family-parent", None);
    parent.harness = "codex".into();
    let mut current = row("reused-key", Some("family-parent"));
    current.harness = "codex".into();
    let mut history = row("reused-key", Some("history-parent"));
    history.harness = "claude".into();
    history.name = "old History name".into();
    history.lifecycle = crate::model::summary::CatalogLifecycle::Retained;

    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let ns = namespace.as_str().as_bytes();
        insert(tx, ns, &generation, &parent, None)?;
        insert(tx, ns, &generation, &current, None)?;

        let metadata = serde_json::to_vec(&history).map_err(|_| StorageError::Unavailable)?;
        let key = history.key.as_bytes();
        let parent_key = history.parent.as_deref().map(str::as_bytes);
        tx.driver.execute(
            "INSERT INTO semon_pg_native_catalog_history_headers(namespace,generation,key_digest,session_key,recipe_generation,source_generation,version,total,incomplete,metadata,original_header,last_ms,harness,repo,model,query_activity,query_active,parent_key) VALUES($1,$2,sha256($3::bytea),$3,$2,NULL,1,0,false,$4,NULL,$5,$6,$7,$8,NULL,false,$9)",
            &[
                &ns,
                &generation.as_bytes(),
                &key,
                &metadata,
                &history.last.unwrap_or(0),
                &history.harness.as_bytes(),
                &history.repo.as_deref().unwrap_or("").as_bytes(),
                &history.model.as_bytes(),
                &parent_key,
            ],
        )
        .map_err(|_| StorageError::Unavailable)?;
        tx.driver.execute(
            "INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model,query_activity,query_active,parent_key) VALUES($1,$2,sha256($3::bytea),$3,$2,false,$4,$5,$6,$7,NULL,false,$8)",
            &[
                &ns,
                &generation.as_bytes(),
                &key,
                &history.last.unwrap_or(0),
                &history.harness.as_bytes(),
                &history.repo.as_deref().unwrap_or("").as_bytes(),
                &history.model.as_bytes(),
                &parent_key,
            ],
        )
        .map_err(|_| StorageError::Unavailable)?;
        Ok(())
    })
    .unwrap();

    let family =
        super::super::with_generation(&pool, &namespace, &control, &generation, |snapshot| {
            selected_family(
                snapshot,
                "family-parent",
                &mut |_| serde_json::json!({"state":"current"}),
            )
            .map_err(|_| StorageError::Busy)
        })
        .unwrap();
    assert_eq!(family["state"], "complete");
    assert_eq!(family["runs"].as_array().unwrap().len(), 1);
    let child = &family["runs"].as_array().unwrap()[0];
    assert_eq!(child["key"], "reused-key");
    assert_eq!(child["name"], "old History name");
    assert_eq!(child["parent"], "history-parent");
    assert_eq!(child["harness"], "codex");
    assert_eq!(
        child["status"],
        serde_json::json!({"state":null,"observation":"unavailable"}),
        "a different-harness History row cannot report Current status"
    );
}

#[test]
fn pg_selected_family_bounds_candidate_bytes_and_charges_visited_binds() {
    let (pool, _) = fixture(|_| {});
    let namespace = ns("selected-family-byte-budget");
    let foreign_namespace = ns("selected-family-byte-budget-foreign");
    let control = PgCancellation::default();
    let generation = "a".repeat(64);
    let scoped_parent = "s".repeat(70_000);
    let budget_parent = "budget-root";
    let visited_parent = "v".repeat(140_000);
    pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
        let ns = namespace.as_str().as_bytes();
        insert_raw_key(tx, ns, &generation, scoped_parent.as_bytes(), None)?;
        insert_raw_key(
            tx,
            ns,
            &generation,
            b"scope-child",
            Some(scoped_parent.as_bytes()),
        )?;
        insert_raw_key(tx, ns, &generation, budget_parent.as_bytes(), None)?;
        insert_raw_key(tx, ns, &generation, visited_parent.as_bytes(), None)?;
        for index in 0..4 {
            let child = format!("{index}{}", "c".repeat(70_000));
            insert_raw_key(
                tx,
                ns,
                &generation,
                child.as_bytes(),
                Some(budget_parent.as_bytes()),
            )?;
        }
        Ok(())
    })
    .unwrap();
    pool.transaction(&foreign_namespace, TxnKind::Write, &control, |tx| {
        let ns = foreign_namespace.as_str().as_bytes();
        for index in 0..3 {
            let child = format!("foreign-scope-child-{index}");
            insert_raw_key(
                tx,
                ns,
                &generation,
                child.as_bytes(),
                Some(scoped_parent.as_bytes()),
            )?;
        }
        Ok(())
    })
    .unwrap();

    let scoped = descendants_with_budget(
        &pool,
        &namespace,
        &generation,
        &scoped_parent,
        2 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(scoped, vec!["scope-child".to_owned()]);
    assert_eq!(
        descendants_with_budget(
            &pool,
            &namespace,
            &generation,
            budget_parent,
            2 * 1024 * 1024,
        ),
        Err(StorageError::Busy)
    );
    assert_eq!(
        descendants_with_budget(&pool, &namespace, &generation, &visited_parent, 1024 * 1024),
        Err(StorageError::Busy)
    );
}
