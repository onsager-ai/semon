//! Indexed native-to-catalog navigation. This grants no native control authority.
use crate::{
    CatalogIdentityError, CatalogReadScope, Options,
    events::EventCache,
    model::summary::{CATALOG_VERSION, CatalogRow},
    viewer::ViewerReply,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogNativeResolutionState {
    Resolved,
    Provisional,
    Pending,
    Ambiguous,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogNativeResolution {
    pub state: CatalogNativeResolutionState,
    pub source_key: String,
    pub harness: String,
    pub native_id: String,
    pub read_scope: CatalogReadScope,
    pub catalog_key: Option<String>,
    pub generation: Option<String>,
}

/// The host must authorize `source_key` and supply precisely its configured
/// options. A resolved read alias is not evidence of runtime/control readiness.
pub fn session_catalog_resolve_native(
    options: &Options,
    source_key: &str,
    harness: &str,
    native_id: &str,
    scope: CatalogReadScope,
) -> Result<CatalogNativeResolution, CatalogIdentityError> {
    if source_key.is_empty()
        || source_key.len() > 4096
        || native_id.is_empty()
        || native_id.len() > 4096
        || native_id.chars().any(char::is_control)
        || !matches!(harness, "claude" | "codex" | "copilot")
    {
        return Err(CatalogIdentityError::InvalidArguments);
    }
    let mut result = CatalogNativeResolution {
        state: CatalogNativeResolutionState::Unavailable,
        source_key: source_key.into(),
        harness: harness.into(),
        native_id: native_id.into(),
        read_scope: scope,
        catalog_key: None,
        generation: None,
    };
    let Ok(mut connection) = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Ok(result);
    };
    connection
        .busy_timeout(Duration::from_millis(100))
        .map_err(|_| CatalogIdentityError::Unavailable)?;
    resolve(&mut connection, &mut result, Some(options))
        .map_err(|_| CatalogIdentityError::Unavailable)?;
    Ok(result)
}

fn resolve(
    connection: &mut Connection,
    result: &mut CatalogNativeResolution,
    options: Option<&Options>,
) -> rusqlite::Result<()> {
    result.state = CatalogNativeResolutionState::Unavailable;
    result.catalog_key = None;
    result.generation = None;
    let tx = connection.transaction()?;
    let meta = |key: &str| {
        tx.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()
    };
    if meta("catalog_version")?.as_deref() != Some(CATALOG_VERSION.to_string().as_str()) {
        return Ok(());
    }
    let Some(generation) = meta("catalog_generation")? else {
        return Ok(());
    };
    if generation.len() != 64 || !generation.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    let complete = match meta("catalog_completeness")?.as_deref() {
        None | Some("complete") => true,
        Some("partial") => false,
        _ => return Ok(()),
    };
    result.generation = Some(generation);
    // Current navigation uses current native bindings. History additionally
    // resolves qualified retained bindings; ambiguity is never guessed away.
    let current_sql = "SELECT DISTINCT session_key FROM session_catalog_sources INDEXED BY session_catalog_current_native WHERE lifecycle='current' AND harness=?1 AND native_id=?2 ORDER BY session_key LIMIT 2";
    let lookup = |sql: &str| -> rusqlite::Result<Vec<String>> {
        tx.prepare(sql)?
            .query_map((&result.harness, &result.native_id), |row| row.get(0))?
            .collect()
    };
    let mut keys = lookup(current_sql)?;
    let current_owner = !keys.is_empty();
    if !current_owner && result.read_scope == CatalogReadScope::RetainedHistory {
        keys = lookup(
            "SELECT DISTINCT session_key FROM session_catalog_sources INDEXED BY session_catalog_native WHERE harness=?1 AND native_id=?2 ORDER BY session_key LIMIT 2",
        )?;
    }
    match keys.as_slice() {
        [] => result.state = CatalogNativeResolutionState::Pending,
        [key] => {
            let current_metadata = "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM session_catalog WHERE session_key=?1";
            let history_metadata = "SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM session_history_catalog WHERE session_key=?1";
            let read_metadata = |sql: &str| -> rusqlite::Result<Option<Option<String>>> {
                tx.query_row(sql, [key], |row| row.get(0)).optional()
            };
            // An active parsed owner may precede materialized History. Its
            // current metadata already proves this exact native membership.
            // Retained-only aliases prefer the richer History provenance.
            let encoded = if current_owner {
                read_metadata(current_metadata)?
            } else {
                match read_metadata(history_metadata)? {
                    None => read_metadata(current_metadata)?,
                    present => present,
                }
            };
            if let Some(Some(encoded)) = encoded {
                let row: CatalogRow =
                    serde_json::from_str(&encoded).map_err(|_| rusqlite::Error::InvalidQuery)?;
                if row.key == *key
                    && row.harness == result.harness
                    && row.sources.len() <= 64
                    && row.sources.iter().any(|s| {
                        if s.native_id != result.native_id {
                            return false;
                        }
                        let Some(options) = options else {
                            return true;
                        };
                        let home = match result.harness.as_str() {
                            "claude" => &options.claude_home,
                            "codex" => &options.codex_home,
                            "copilot" => &options.copilot_home,
                            _ => return false,
                        };
                        s.path
                            .strip_prefix(home)
                            .ok()
                            .and_then(|p| p.to_str())
                            .is_some_and(|path| crate::is_input_path(&result.harness, path))
                    })
                {
                    result.catalog_key = Some(key.clone());
                    result.state = if complete {
                        CatalogNativeResolutionState::Resolved
                    } else {
                        CatalogNativeResolutionState::Provisional
                    };
                }
            }
        }
        _ => result.state = CatalogNativeResolutionState::Ambiguous,
    }
    tx.commit()
}

pub(crate) fn page(options: &Options, key: &str, query: &str) -> ViewerReply {
    use crate::viewer::{decoded, query_value};
    let value = |name| query_value(query, name).and_then(decoded);
    let scope = match value("scope").as_deref() {
        None | Some("current") => CatalogReadScope::Current,
        Some("retained_history") => CatalogReadScope::RetainedHistory,
        _ => return crate::catalog::error(400, "invalid_arguments", "Unknown read scope.", false),
    };
    let reply = match (value("harness"), value("native_id")) {
        (Some(harness), Some(native)) => {
            session_catalog_resolve_native(options, key, &harness, &native, scope)
        }
        _ => Err(CatalogIdentityError::InvalidArguments),
    };
    match reply {
        Ok(resolution) => ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: serde_json::json!({"api":1,"resolution":resolution})
                .to_string()
                .into_bytes(),
            etag: None,
        },
        Err(CatalogIdentityError::InvalidArguments) => crate::catalog::error(
            400,
            "invalid_arguments",
            "Explicit harness and native_id are required.",
            false,
        ),
        Err(_) => crate::catalog::error(
            503,
            "catalog_unavailable",
            "Native identity observation is unavailable.",
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_alias_is_exact_bounded_and_preserves_ambiguity_and_provisional_reads() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT);CREATE TABLE session_catalog_sources(session_key TEXT,harness TEXT,native_id TEXT,lifecycle TEXT);CREATE INDEX session_catalog_native ON session_catalog_sources(harness,native_id,session_key);CREATE INDEX session_catalog_current_native ON session_catalog_sources(lifecycle,harness,native_id,session_key);CREATE TABLE session_catalog(session_key TEXT PRIMARY KEY,metadata TEXT);CREATE TABLE session_history_catalog(session_key TEXT PRIMARY KEY,metadata TEXT);").unwrap();
        db.execute(
            "INSERT INTO meta VALUES('catalog_version',?1)",
            [CATALOG_VERSION.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO meta VALUES('catalog_generation',?1)",
            ["a".repeat(64)],
        )
        .unwrap();
        let metadata=json!({"key":"canonical","name":"example","harness":"codex","kind":"session","native_ids":["native"],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":"","effort":null,"start":null,"last":null,"tokens":[0,0,0],"cost":{"usd":null,"unpriced_models":[],"split_unknown_messages":0,"by_model":{},"by_day":{}},"sources":[{"path":"missing.jsonl","native_id":"native","dev":1,"ino":1,"size":0,"modified_ns":0,"offset":0,"prefix_sha256":vec![0;32],"tail_sha256":vec![0;32]}]}).to_string();
        db.execute(
            "INSERT INTO session_catalog VALUES('canonical',?1)",
            [&metadata],
        )
        .unwrap();
        db.execute(
            "INSERT INTO session_history_catalog VALUES('canonical',?1)",
            [&metadata],
        )
        .unwrap();
        db.execute(
            "INSERT INTO session_catalog_sources VALUES('canonical','codex','native','current')",
            [],
        )
        .unwrap();
        let mut r = CatalogNativeResolution {
            state: CatalogNativeResolutionState::Unavailable,
            source_key: "exact-source".into(),
            harness: "codex".into(),
            native_id: "native".into(),
            read_scope: CatalogReadScope::Current,
            catalog_key: None,
            generation: None,
        };
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Resolved);
        assert_eq!(r.catalog_key.as_deref(), Some("canonical"));
        db.execute("DELETE FROM session_history_catalog", [])
            .unwrap();
        r.read_scope = CatalogReadScope::RetainedHistory;
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Resolved);
        assert_eq!(r.catalog_key.as_deref(), Some("canonical"));
        db.execute(
            "INSERT INTO session_catalog_sources VALUES('old-owner','codex','native','retained')",
            [],
        )
        .unwrap();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Resolved);
        db.execute(
            "DELETE FROM session_catalog_sources WHERE session_key='old-owner'",
            [],
        )
        .unwrap();
        r.read_scope = CatalogReadScope::Current;
        db.execute(
            "INSERT INTO meta VALUES('catalog_completeness','partial')",
            [],
        )
        .unwrap();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Provisional);
        db.execute(
            "INSERT INTO session_catalog_sources VALUES('other','codex','native','current')",
            [],
        )
        .unwrap();
        r.catalog_key = None;
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Ambiguous);
        assert_eq!(r.catalog_key, None);
        r.harness = "claude".into();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Pending);
        db.execute(
            "DELETE FROM session_catalog_sources WHERE session_key='other'",
            [],
        )
        .unwrap();
        db.execute(
            "UPDATE session_catalog_sources SET lifecycle='retained'",
            [],
        )
        .unwrap();
        r.harness = "codex".into();
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Pending);
        r.read_scope = CatalogReadScope::RetainedHistory;
        resolve(&mut db, &mut r, None).unwrap();
        assert_eq!(r.state, CatalogNativeResolutionState::Provisional);
        let plan:String=db.query_row("EXPLAIN QUERY PLAN SELECT DISTINCT session_key FROM session_catalog_sources INDEXED BY session_catalog_current_native WHERE lifecycle='current' AND harness='codex' AND native_id='native' ORDER BY session_key LIMIT 2",[],|row|row.get(3)).unwrap();
        assert!(plan.contains("COVERING INDEX"), "{plan}");
    }
}
