use super::*;

#[test]
fn retained_child_fields_use_exact_current_parent_after_original_history_is_preserved() {
    use crate::model::summary::CatalogRow;
    use crate::storage::StorageError;
    use std::io::Write;

    let home = Home::new("retained-endpoint-context");
    home.top(
        "lead",
        &[
            human("lead", ts(1, 0), "Delegate the parser review"),
            assistant(
                "lead",
                ts(1, 1),
                vec![tool(
                    "spawn-child",
                    "Agent",
                    json!({"description":"child","prompt":"Review the parser"}),
                )],
            ),
        ],
    );
    home.agent(
        "lead",
        "child",
        "spawn-child",
        &[
            user("child", ts(1, 1), "Review the parser"),
            assistant("child", ts(1, 2), vec![text("Parser reviewed")]),
        ],
    );
    let publish = || {
        let mut cache =
            crate::events::EventCache::open_scoped(&home.options.cache, &home.options.source)
                .unwrap();
        crate::model::observe_catalog(
            &home.options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            NOW,
        )
        .unwrap();
    };
    let load = |key: &str, history: bool| -> CatalogRow {
        let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
        let db = binding.connection().unwrap();
        let table = if history {
            "session_history_catalog"
        } else {
            "session_catalog"
        };
        let text: String = db
            .query_row(
                &format!("SELECT metadata FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1"),
                [key],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    publish();
    let original_parent = load("lead", false);
    let parent_path = &original_parent.sources[0].path;
    let mut parent = fs::OpenOptions::new()
        .append(true)
        .open(parent_path)
        .unwrap();
    writeln!(
        parent,
        "{}",
        assistant("lead", ts(1, 3), vec![text("Continued parent observation")])
    )
    .unwrap();
    drop(parent);
    publish();
    let continued_parent = load("lead", false);
    assert_ne!(continued_parent.sources, original_parent.sources);
    let history_parent = load("lead", true);
    assert_eq!(history_parent.sources, original_parent.sources);
    for (candidate, history) in [(&history_parent, true), (&continued_parent, false)] {
        let binding = crate::storage::local_navigation_binding(&home.options, "").unwrap();
        let db = home.semantic_publisher_connection();
        let tx = db.unchecked_transaction().unwrap();
        let current = binding.runtime_binding_at(&tx).unwrap();
        let query_observation_matches = candidate.query.as_ref().is_some_and(|saved| {
            crate::model::summary::query_observation_matches(
                saved,
                &candidate.sources,
                false,
                None,
            )
            .unwrap_or(false)
        });
        assert!(
            query_observation_matches,
            "retained context Query observation must match its source generation"
        );
        let sources_are_allowlisted = !candidate.sources.is_empty()
            && candidate.sources.len() <= 64
            && candidate.sources.iter().all(|source| {
                crate::catalog::source_relative(&home.options, &candidate.harness, source)
                    .is_some()
            });
        assert!(
            sources_are_allowlisted,
            "retained context sources must stay within the candidate harness allowlist"
        );
        let owner = if history {
            crate::storage::catalog::CatalogObservationOwner::History
        } else {
            crate::storage::catalog::CatalogObservationOwner::Catalog
        };
        let physical_owner_is_bound =
            crate::storage::sqlite::proof_generations::observation_binding_for_physical_owner(
                &tx, &current, candidate, owner,
            )
            .is_ok();
        assert!(
            physical_owner_is_bound,
            "retained context must bind the candidate's physical Catalog or History owner"
        );
        let history_header_is_qualified = !history
            || crate::history_projection::qualified(&tx, candidate)
                .is_ok_and(|qualified| qualified);
        assert!(
            history_header_is_qualified,
            "retained context History candidate must have a qualified History header"
        );
        crate::storage::sqlite::query::qualify_context_candidate(
            &tx,
            &current,
            &home.options,
            candidate,
            history,
        )
        .unwrap();
        let (own, opposite, reference) = if history {
            ("history", "catalog", "history")
        } else {
            ("catalog", "history", "current")
        };
        tx.execute(
            "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key=?1 AND owner_kind=?2",
            rusqlite::params![candidate.key, opposite],
        )
        .unwrap();
        assert_eq!(
            tx.execute(
                "INSERT INTO storage_catalog_native_observation_grants(namespace,session_key,owner_kind,metadata_digest,admission,stream,direct_capture) SELECT namespace,session_key,?3,metadata_digest,admission,stream,direct_capture FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key=?1 AND owner_kind=?2",
                rusqlite::params![candidate.key, own, opposite],
            )
            .unwrap(),
            1
        );
        tx.execute(
            "DELETE FROM storage_catalog_native_observation_grants WHERE namespace=semon_namespace() AND session_key=?1 AND owner_kind=?2",
            rusqlite::params![candidate.key, own],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM storage_proof_refs WHERE namespace=semon_namespace() AND session_key=?1 AND kind=?2",
            rusqlite::params![candidate.key, reference],
        )
        .unwrap();
        // The opposite grant genuinely authenticates this exact metadata.
        crate::storage::sqlite::proof_generations::observation_binding(&tx, &current, candidate)
            .unwrap();
        assert!(matches!(
            crate::storage::sqlite::query::qualify_context_candidate(
                &tx,
                &current,
                &home.options,
                candidate,
                history,
            ),
            Err(StorageError::ScopeChanged)
        ));
        tx.rollback().unwrap();
    }
    let child = load("child", false);
    let source = &child.sources[0];
    let archive = SelectedArchive {
        source: crate::SessionSourceRef {
            root: "claude".into(),
            path: source
                .path
                .strip_prefix(&home.options.claude_home)
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
            native_id: source.native_id.clone(),
            offset: source.offset,
            prefix_sha256: source.prefix_sha256,
            tail_sha256: source.tail_sha256,
        },
        bytes: Arc::from(fs::read(&source.path).unwrap()),
        calls: std::sync::atomic::AtomicUsize::new(0),
        read_bytes: AtomicU64::new(0),
        fault: 0,
    };
    fs::remove_file(&source.path).unwrap();
    publish();
    let retained = load("child", true);
    assert_eq!(load("lead", true).sources, original_parent.sources);
    assert_eq!(load("lead", false).sources, continued_parent.sources);
    let recipes = retained.query.as_ref().unwrap()["selected"]["edges"]
        .as_array()
        .unwrap();
    assert!(recipes.iter().any(|edge| {
        edge["query_content"]
            .as_object()
            .into_iter()
            .flatten()
            .any(|(_, recipe)| {
                serde_json::from_value::<crate::model::summary::CatalogSource>(
                    recipe["source"].clone(),
                )
                .is_ok_and(|source| continued_parent.sources.contains(&source))
            })
    }));
    let mut query = Query::from_sources(admitted(home.options.clone()));
    let fields = query
        .call_selected_at(
            "local",
            "child",
            "get_session",
            &json!({"id":"child"}),
            Some(&archive),
            crate::SessionSourceReadMode::LocalThenProvider,
            NOW,
        )
        .unwrap();
    assert_eq!(fields["session"]["id"], "child");
    assert!(
        fields["session"]["handoffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|handoff| {
                handoff["from"] == "lead"
                    && handoff["to"] == "child"
                    && handoff["brief"] == "Review the parser"
            })
    );
    assert!(archive.calls.load(Ordering::Relaxed) > 0);

    // The saved dependency cannot borrow a different parent generation, nor
    // can a path-keyed reader admit two generations under one path.
    for candidate in [None, Some(original_parent.clone())] {
        let result = crate::storage::query::selected_edge_context(
            &retained,
            true,
            |key, _history| Ok(candidate.clone().filter(|row| row.key == key)),
            |_row, _history| Ok(()),
        );
        assert!(matches!(result, Err(StorageError::ScopeChanged)));
    }
    let mut conflicting = continued_parent.clone();
    let mut conflicting_child = retained.sources[0].clone();
    conflicting_child.offset += 1;
    conflicting.sources.push(conflicting_child);
    let result = crate::storage::query::selected_edge_context(
        &retained,
        true,
        |key, history| Ok((key == "lead" && !history).then(|| conflicting.clone())),
        |_row, _history| Ok(()),
    );
    assert!(matches!(result, Err(StorageError::ScopeChanged)));
}
