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
            insert(
                tx,
                ns,
                &generation,
                &row(&child, Some("fanout-root")),
                None,
            )?;
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

    let family = super::super::with_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            selected_family(snapshot, "family-root", &mut |_| {
                serde_json::json!({"state":"current"})
            })
            .map_err(|_| StorageError::Busy)
        },
    )
    .unwrap();
    assert_eq!(family["state"], "complete");
    let family_keys = family["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(family_keys, vec!["family-child", "family-leaf"]);

    let fanout = super::super::with_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            selected_family(snapshot, "fanout-root", &mut |_| {
                serde_json::json!({"state":"current"})
            })
            .map_err(|_| StorageError::Busy)
        },
    )
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

    let cycle = super::super::with_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            selected_family(snapshot, "cycle-a", &mut |_| {
                serde_json::json!({"state":"current"})
            })
            .map_err(|_| StorageError::Busy)
        },
    )
    .unwrap();
    assert_eq!(cycle["state"], "incomplete");
    assert!(cycle["runs"].as_array().unwrap().is_empty());

    let collision = super::super::with_generation(
        &pool,
        &namespace,
        &control,
        &generation,
        |snapshot| {
            crate::storage::selected::ContextLookup::descendants(snapshot, "collision-root")
                .map_err(|_| StorageError::ScopeChanged)
        },
    );
    assert_eq!(collision, Err(StorageError::ScopeChanged));
}
