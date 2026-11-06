use super::super::{
    Namespace, ProjectionTag, StorageError, TagCheck,
    postgres::{PgProjectionStore, schema},
};

async fn connect(dsn: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(dsn, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        connection.await.unwrap();
    });
    client
}

/// Runs only in the service-container workflow. Missing DSNs fail explicitly.
#[tokio::test]
async fn pg_projection_rls_epochs_rollback_and_no_sqlite_files() {
    let admin =
        connect(&std::env::var("SEMON_PG_ADMIN_TEST_DSN").expect("CI PostgreSQL admin DSN")).await;
    admin.batch_execute(&schema()).await.unwrap();
    admin.batch_execute("GRANT USAGE ON SCHEMA semon_projection TO semon_app; GRANT SELECT,INSERT,UPDATE,DELETE ON semon_projection.namespaces,semon_projection.ledgers,semon_projection.units TO semon_app; GRANT SELECT ON semon_projection.metadata TO semon_app").await.unwrap();
    let dsn = std::env::var("SEMON_PG_TEST_DSN").expect("CI restricted PostgreSQL DSN");
    let a = Namespace::new("machine-a").unwrap();
    let b = Namespace::new("machine-b").unwrap();
    let mut store_a = PgProjectionStore::new(connect(&dsn).await, a.clone())
        .await
        .unwrap();
    let mut store_b = PgProjectionStore::new(connect(&dsn).await, b.clone())
        .await
        .unwrap();
    assert!(matches!(
        store_a.begin(&b).await,
        Err(StorageError::WrongNamespace { .. })
    ));
    let tag = ProjectionTag {
        dev: 1,
        ino: 2,
        offset: 10,
        prefix_sha256: [3; 32],
        tail_sha256: [4; 32],
        epoch: 0,
        projector: crate::model::summary::CATALOG_VERSION,
    };
    let start = std::time::Instant::now();
    let tx = store_a.begin(&a).await.unwrap();
    let tag = tx
        .write_ledger(&a, "same/native/path", None, tag, false)
        .await
        .unwrap()
        .unwrap();
    tx.publish_tag(&a, "session", "same/native/path", tag)
        .await
        .unwrap();
    tx.commit(&a).await.unwrap();
    eprintln!(
        "postgres-projection-cold-transaction-us={}",
        start.elapsed().as_micros()
    );
    let tx = store_b.begin(&b).await.unwrap();
    assert_eq!(tx.ledger(&b, "same/native/path").await.unwrap(), None);
    let other = tx
        .write_ledger(
            &b,
            "same/native/path",
            None,
            ProjectionTag {
                prefix_sha256: [9; 32],
                ..tag
            },
            false,
        )
        .await
        .unwrap()
        .unwrap();
    tx.commit(&b).await.unwrap();
    assert_ne!(tag.prefix_sha256, other.prefix_sha256);
    let tx = store_a.begin(&a).await.unwrap();
    assert_eq!(
        tx.check_tag(&a, "session", "same/native/path")
            .await
            .unwrap(),
        Some(TagCheck::Current)
    );
    let next = tx
        .write_ledger(
            &a,
            "same/native/path",
            Some(tag),
            ProjectionTag { offset: 20, ..tag },
            true,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.epoch, tag.epoch);
    assert_eq!(
        tx.check_tag(&a, "session", "same/native/path")
            .await
            .unwrap(),
        Some(TagCheck::Advanced)
    );
    assert!(
        tx.write_ledger(&a, "same/native/path", Some(tag), tag, false)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit(&a).await.unwrap();
    let tx = store_a.begin(&a).await.unwrap();
    let rewritten = tx
        .write_ledger(
            &a,
            "same/native/path",
            Some(next),
            ProjectionTag { offset: 30, ..next },
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(rewritten.epoch > next.epoch);
    assert_eq!(
        tx.check_tag(&a, "session", "same/native/path")
            .await
            .unwrap(),
        Some(TagCheck::Mismatch("ledger_epoch"))
    );
    // Dropping an uncommitted transaction restores both ledger and epoch.
    drop(tx);
    let tx = store_a.begin(&a).await.unwrap();
    assert_eq!(tx.ledger(&a, "same/native/path").await.unwrap(), Some(next));
    tx.commit(&a).await.unwrap();
    // Test the actual SQL policy directly, without the Rust namespace check.
    let mut raw = connect(&dsn).await;
    let tx = raw.transaction().await.unwrap();
    tx.query_one("SELECT set_config('semon.namespace','machine-a',true)", &[])
        .await
        .unwrap();
    let visible: i64 = tx
        .query_one(
            "SELECT count(*) FROM semon_projection.ledgers WHERE namespace='machine-b'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(visible, 0);
    assert_eq!(
        tx.execute(
            "UPDATE semon_projection.ledgers SET consumed=0 WHERE namespace='machine-b'",
            &[]
        )
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        tx.execute(
            "DELETE FROM semon_projection.ledgers WHERE namespace='machine-b'",
            &[]
        )
        .await
        .unwrap(),
        0
    );
    let refused = tx
        .execute(
            "INSERT INTO semon_projection.namespaces(namespace) VALUES('foreign')",
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.code(),
        Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)
    );
    tx.rollback().await.unwrap();
    let leaked: i64 = raw
        .query_one("SELECT count(*) FROM semon_projection.ledgers", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        leaked, 0,
        "transaction-local namespace must not leak on reuse"
    );
    assert!(
        PgProjectionStore::new(
            connect(&std::env::var("SEMON_PG_ADMIN_TEST_DSN").unwrap()).await,
            a.clone()
        )
        .await
        .is_err()
    );
    let tx = store_a.begin(&a).await.unwrap();
    assert!(tx.write_ledger(&a, "wrong-version", None, ProjectionTag { projector: 0, ..tag }, false).await.is_err());
    tx.commit(&a).await.unwrap();
    admin.batch_execute("UPDATE semon_projection.units SET projector=0; UPDATE semon_projection.ledgers SET projector=0").await.unwrap();
    let tx = store_a.begin(&a).await.unwrap();
    assert_eq!(tx.check_tag(&a, "session", "same/native/path").await.unwrap(), Some(TagCheck::Mismatch("projector_version")));
    tx.commit(&a).await.unwrap();
    admin
        .execute(
            "UPDATE semon_projection.metadata SET stamp='unsupported'",
            &[],
        )
        .await
        .unwrap();
    assert!(matches!(
        store_a.begin(&a).await,
        Err(StorageError::Unsupported(_))
    ));
    let root = std::env::var("SEMON_PG_EMPTY_ROOT").expect("CI empty working directory");
    assert!(
        std::fs::read_dir(root).unwrap().next().is_none(),
        "projection API must create no local session files"
    );
}
