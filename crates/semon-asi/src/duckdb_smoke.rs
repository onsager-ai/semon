//! DuckDB reads a generated store through a live-row view and sees only current rows.

use duckdb::{Connection, params};

use crate::schema::FACT_STEP;
use crate::store::Store;
use crate::testutil::{TestClock, update};

#[test]
fn a_live_row_view_shows_only_current_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open_with_clock(dir.path(), TestClock::new(0)).unwrap();
    store.publish(&update(&[("m1/claude/a", 2), ("m1/claude/b", 2)], "old", 0)).unwrap();
    let mut next = update(&[("m1/claude/a", 3)], "new", 0);
    next.removed_sessions.push("m1/claude/b".to_owned());
    store.publish(&next).unwrap();

    let analytics = store.analytics_dir();
    let live = analytics.join(store.manifest().live_map.as_deref().unwrap());
    let files: Vec<String> = store
        .manifest()
        .files_of(FACT_STEP.name)
        .map(|file| format!("'{}'", analytics.join(&file.path).display()))
        .collect();
    assert_eq!(files.len(), 2);

    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(&format!(
        "CREATE VIEW live AS SELECT * FROM read_parquet('{live}');
         CREATE VIEW fact_step_files AS SELECT * FROM read_parquet([{files}]);
         CREATE VIEW fact_step AS
             SELECT f.* FROM fact_step_files f
             SEMI JOIN live l ON f.session_uid = l.session_uid AND f._gen = l._gen;",
        live = live.display(),
        files = files.join(", "),
    ))
    .unwrap();

    let count = |sql: &str| -> i64 { db.query_row(sql, params![], |row| row.get(0)).unwrap() };
    assert_eq!(count("SELECT count(*) FROM fact_step_files"), 7);
    assert_eq!(count("SELECT count(*) FROM fact_step"), 3);
    assert_eq!(count("SELECT count(DISTINCT session_uid) FROM fact_step"), 1);
    assert_eq!(count("SELECT count(*) FROM fact_step WHERE _gen = 2"), 3);
    assert_eq!(count("SELECT count(*) FROM fact_step WHERE step_uid LIKE '%:old'"), 0);
}
