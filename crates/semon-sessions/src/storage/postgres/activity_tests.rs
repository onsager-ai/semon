use crate::{
    Input, InputRoot,
    storage::{Namespace, postgres::PgProjectionStore},
};
use serde_json::{Value, json};

pub(super) async fn lifecycle(dsn: &str) {
    use std::io::Write;
    let root = crate::storage::conformance::root();
    let options = crate::storage::conformance::source(&root.0, "pg-activity");
    let sqlite = crate::storage::conformance::source(&root.0, "sqlite-activity");
    let path = options
        .codex_home
        .join("sessions/2026/09/24/rollout-native.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let payload = json!({"thread_id":"native","session_id":"native","turn_id":"turn","response_id":"response","usage":{"input_tokens":31,"output_tokens":7,"total_tokens":38,"cached_input_tokens":3,"cache_write_input_tokens":2,"reasoning_output_tokens":0}});
    let line = |kind: &str, payload: Value| {
        json!({"type":kind,"timestamp":"2026-09-24T00:01:00Z","payload":payload}).to_string()
    };
    let accepted = line("token_usage_record", payload.clone());
    let mut conflict = payload;
    conflict["usage"]["input_tokens"] = json!(99);
    let lines = [
        line(
            "session_meta",
            json!({"id":"native","cwd":"/synthetic/project"}),
        ),
        line("turn_context", json!({"model":"gpt-test"})),
        accepted.clone(),
        accepted.clone(),
        line("token_usage_record", conflict),
    ];
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    let namespace = Namespace::new("activity-machine").unwrap();
    let input = Input {
        root: InputRoot::Codex,
        path: "sessions/2026/09/24/rollout-native.jsonl".into(),
    };
    #[cfg(target_os = "linux")]
    let watch = super::producer_tests::SqliteOpenWatch::new(&options);
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    store
        .publish_sources(&namespace, &options, std::slice::from_ref(&input))
        .await
        .unwrap()
        .unwrap();
    let first = store
        .activity_query(
            &namespace,
            &options,
            "native",
            &json!({"id":"native"}),
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
            crate::model::now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["tokens"]["input"], 28);
    assert_eq!(first["items"][0]["tokens"]["cache_read"], 3);
    assert!(first["items"][0].get("position").is_none());
    crate::storage::conformance::produce(&Namespace::new("sqlite-activity").unwrap(), &sqlite)
        .unwrap();
    let mut query = crate::Query::new(sqlite.clone());
    let local = query
        .call("get_session_activity", &json!({"id":"native"}))
        .unwrap();
    assert_eq!(first["items"], local["items"]);
    drop(store);
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let restarted = store
        .activity_query(
            &namespace,
            &options,
            "native",
            &json!({"id":"native"}),
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
            crate::model::now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(first["generation"], restarted["generation"]);
    assert_eq!(first["items"], restarted["items"]);
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
        "{accepted}"
    )
    .unwrap();
    store
        .publish_sources(&namespace, &options, std::slice::from_ref(&input))
        .await
        .unwrap()
        .unwrap();
    let appended = store
        .activity_query(
            &namespace,
            &options,
            "native",
            &json!({"id":"native"}),
            None,
            crate::SessionSourceReadMode::LocalOrPinnedProvider,
            crate::model::now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(first["items"], appended["items"]);
    assert_ne!(first["generation"], appended["generation"]);
    assert!(
        store
            .activity_query(
                &namespace,
                &options,
                "native",
                &json!({"id":"native","generation":first["generation"]}),
                None,
                crate::SessionSourceReadMode::LocalOrPinnedProvider,
                crate::model::now_ms()
            )
            .await
            .is_err()
    );
    let mut client = super::connect(dsn).await;
    let tx = client.transaction().await.unwrap();
    tx.query_one(
        "SELECT set_config('semon.namespace',$1,true)",
        &[&namespace.binding()],
    )
    .await
    .unwrap();
    assert_eq!(
        tx.query_one(
            "SELECT count(*) FROM semon_projection.records WHERE namespace=$1 AND kind=3",
            &[&namespace.binding()]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        4
    );
    for table in ["session_activity"] {
        assert_eq!(tx.query_one(&format!("SELECT count(*) FROM semon_projection.{table} WHERE namespace='foreign-runtime'"), &[]).await.unwrap().get::<_,i64>(0), 0);
    }
    tx.commit().await.unwrap();
    #[cfg(target_os = "linux")]
    watch.assert_untouched(&options);
}
