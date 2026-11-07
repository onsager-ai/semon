use crate::{
    Input, InputRoot, PgRawGeneration,
    storage::{Namespace, postgres::PgProjectionStore},
};

pub(super) async fn lifecycle(dsn: &str) {
    let namespace = Namespace::new("raw-custody").unwrap();
    let input = Input {
        root: InputRoot::Claude,
        path: "projects/project/raw.jsonl".into(),
    };
    let generation = PgRawGeneration {
        generation: "a".repeat(32),
        length: 100,
        sha256: [7; 32],
    };
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    let stale = tx.begin_raw_write(&namespace, &input).await.unwrap();
    assert!(tx.raw_write_pending(&namespace, &input).await.unwrap());
    assert!(
        tx.raw_generation(&namespace, &input)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&namespace).await.unwrap();
    drop(store);
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone())
        .await
        .unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    assert!(tx.raw_write_pending(&namespace, &input).await.unwrap());
    let current = tx.begin_raw_write(&namespace, &input).await.unwrap();
    assert!(
        !tx.complete_raw_write(&namespace, stale, &generation)
            .await
            .unwrap()
    );
    assert!(
        tx.complete_raw_write(&namespace, current, &generation)
            .await
            .unwrap()
    );
    assert_eq!(
        tx.raw_generation(&namespace, &input).await.unwrap(),
        Some(generation.clone())
    );
    assert!(!tx.raw_write_pending(&namespace, &input).await.unwrap());
    tx.commit(&namespace).await.unwrap();
    // An uncommitted intent cannot revoke the completed marker.
    let tx = store.begin(&namespace).await.unwrap();
    let rolled_back = tx.begin_raw_write(&namespace, &input).await.unwrap();
    drop(tx);
    let tx = store.begin(&namespace).await.unwrap();
    assert_eq!(
        tx.raw_generation(&namespace, &input).await.unwrap(),
        Some(generation.clone())
    );
    let pending = tx.begin_raw_write(&namespace, &input).await.unwrap();
    assert!(
        !tx.complete_raw_write(&namespace, rolled_back, &generation)
            .await
            .unwrap()
    );
    assert!(tx.raw_write_pending(&namespace, &input).await.unwrap());
    tx.commit(&namespace).await.unwrap();
    // Facts intents keep completed non-rebuildable usage intact until the
    // exact wanted report completes, and survive a disconnected writer.
    let facts = crate::Facts {
        version: crate::FACTS_VERSION,
        hostname: "completed-machine".into(),
        ..crate::Facts::default()
    };
    let tx = store.begin(&namespace).await.unwrap();
    assert!(tx.write_facts(&namespace, None, 1, &facts).await.unwrap());
    tx.commit(&namespace).await.unwrap();
    let wanted = crate::Facts { hostname: "wanted-machine".into(), ..facts.clone() };
    let tx = store.begin(&namespace).await.unwrap();
    let stale_facts = tx.begin_facts_write(&namespace, &wanted).await.unwrap();
    assert_eq!(tx.reported_facts(&namespace).await.unwrap().unwrap().1, facts);
    assert!(tx.facts_write_pending(&namespace).await.unwrap());
    assert!(!tx.write_facts(&namespace, Some(stale_facts), stale_facts + 1, &facts).await.unwrap());
    tx.commit(&namespace).await.unwrap();
    drop(store);
    let mut store = PgProjectionStore::new(super::connect(dsn).await, namespace.clone()).await.unwrap();
    let tx = store.begin(&namespace).await.unwrap();
    assert!(tx.facts_write_pending(&namespace).await.unwrap());
    assert_eq!(tx.reported_facts(&namespace).await.unwrap().unwrap().1, facts);
    let current_facts = tx.begin_facts_write(&namespace, &wanted).await.unwrap();
    assert!(tx.complete_facts_write(&namespace, stale_facts, &wanted).await.is_err());
    tx.complete_facts_write(&namespace, current_facts, &wanted).await.unwrap();
    assert!(!tx.facts_write_pending(&namespace).await.unwrap());
    assert_eq!(tx.reported_facts(&namespace).await.unwrap().unwrap().1, wanted);
    tx.commit(&namespace).await.unwrap();
    let foreign = Namespace::new("foreign-raw-custody").unwrap();
    let mut other = PgProjectionStore::new(super::connect(dsn).await, foreign.clone())
        .await
        .unwrap();
    let tx = other.begin(&foreign).await.unwrap();
    assert!(tx.raw_generation(&foreign, &input).await.unwrap().is_none());
    assert!(!tx.raw_write_pending(&foreign, &input).await.unwrap());
    assert!(
        tx.complete_raw_write(&foreign, pending, &generation)
            .await
            .is_err()
    );
    tx.commit(&foreign).await.unwrap();
    let mut raw = super::connect(dsn).await;
    let tx = raw.transaction().await.unwrap();
    tx.query_one(
        "SELECT set_config('semon.namespace',$1,true)",
        &[&foreign.binding()],
    )
    .await
    .unwrap();
    assert_eq!(
        tx.query_one("SELECT count(*) FROM semon_projection.raw_sources", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        tx.execute(
            "DELETE FROM semon_projection.raw_sources WHERE namespace=$1",
            &[&namespace.binding()]
        )
        .await
        .unwrap(),
        0
    );
    assert!(tx.execute("INSERT INTO semon_projection.raw_sources(namespace,root,path,revision,pending) VALUES($1,'claude','projects/project/foreign.jsonl',1,true)", &[&namespace.binding()]).await.is_err());
    tx.rollback().await.unwrap();
}
