//! Read dispatch and stop evidence for the Worker roster in a lane report.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;

use crate::{
    events::store,
    storage::{Namespace, sqlite},
};

#[derive(Serialize)]
pub struct WorkerTask {
    pub dispatch_call_id: Option<String>,
    pub disposition: String,
    pub pr_url: Option<String>,
    pub head_sha: Option<String>,
    pub checks_state: Option<String>,
    pub harness_failed: bool,
}

impl Default for WorkerTask {
    fn default() -> Self {
        Self {
            dispatch_call_id: None,
            disposition: "unknown".into(),
            pr_url: None,
            head_sha: None,
            checks_state: None,
            harness_failed: false,
        }
    }
}

/// The caller supplies machine-qualified Worker/parent pairs from a local ASI
/// roster. All native dispatch and terminal evidence is read in one snapshot.
pub fn worker_tasks(index: &Path, pairs: &[(&str, &str)]) -> rusqlite::Result<Vec<WorkerTask>> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let namespace =
        crate::read_analytics_namespace(index.parent().ok_or(rusqlite::Error::InvalidQuery)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?
            .ok_or(rusqlite::Error::InvalidQuery)?;
    let db = Connection::open_with_flags(
        index,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.execute_batch("BEGIN DEFERRED")?;
    store::require_current_sql(&db)?;
    sqlite::qualify(
        &db,
        &Namespace::new("local").map_err(|_| rusqlite::Error::InvalidQuery)?,
    )
    .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let mut source = db.prepare_cached(
        "SELECT min(d.source) FROM dim_session d
         JOIN warehouse_current current USING(source)
         JOIN bridge_session_owner o ON o.source=d.source AND o.owner=d.source
         WHERE d.harness=?1 AND d.native_id=?2 HAVING count(*)=1",
    )?;
    let mut claude = db.prepare_cached(include_str!("worker_outcome_claude.sql"))?;
    let mut codex = db.prepare_cached(include_str!("worker_outcome_codex.sql"))?;
    let identity = |uid: &str| {
        let (origin, rest) = uid.split_once('/')?;
        let (harness, id) = rest.split_once('/')?;
        if origin != namespace || id.contains('/') {
            return None;
        }
        Some((harness.to_owned(), crate::session_route::decode(id).ok()?))
    };
    pairs
        .iter()
        .map(|(worker, parent)| {
            let (Some((harness, worker)), Some((parent_harness, parent))) =
                (identity(worker), identity(parent))
            else {
                return Ok(WorkerTask::default());
            };
            if harness != parent_harness {
                return Ok(WorkerTask::default());
            }
            let lookup = |statement: &mut rusqlite::CachedStatement<'_>, id: &str| {
                statement
                    .query_row(params![harness, id], |row| row.get::<_, i64>(0))
                    .optional()
            };
            let (Some(child), Some(parent)) =
                (lookup(&mut source, &worker)?, lookup(&mut source, &parent)?)
            else {
                return Ok(WorkerTask::default());
            };
            let statement = match harness.as_str() {
                "claude" => &mut claude,
                "codex" => &mut codex,
                _ => return Ok(WorkerTask::default()),
            };
            Ok(statement
                .query_row(params![child, parent], |row| {
                    Ok(WorkerTask {
                        dispatch_call_id: row.get(0)?,
                        disposition: row
                            .get::<_, Option<String>>(1)?
                            .unwrap_or_else(|| "unknown".into()),
                        pr_url: row.get(2)?,
                        head_sha: row.get(3)?,
                        checks_state: row.get(4)?,
                        harness_failed: row.get(5)?,
                    })
                })
                .optional()?
                .unwrap_or_default())
        })
        .collect()
}
