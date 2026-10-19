//! Bounded merge of existing Catalog pages. No new index, Core or source reader.
use super::*;
use crate::OverviewSource;
use sha2::{Digest, Sha256};

const MAX_SOURCES: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Position {
    last: i64,
    key: String,
    source: String,
}
impl Ord for Position {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .last
            .cmp(&self.last)
            .then_with(|| self.key.cmp(&other.key))
            .then_with(|| self.source.cmp(&other.source))
    }
}
impl PartialOrd for Position {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnionCursor {
    version: u8,
    workspace: String,
    filters: String,
    generation: String,
    after: Position,
}

/// The host supplies authorized source options and a workspace identity. One
/// source database is open at a time; retained memory is at most two pages.
/// Any missing source fails the request. A changed source set/publication resets
/// pagination, matching the focused Catalog's live-write convention.
pub fn session_catalog_union(
    sources: &[OverviewSource],
    workspace: &str,
    query: &str,
) -> ViewerReply {
    match read_union(sources, workspace, query) {
        Ok(reply) | Err(reply) => reply,
    }
}

fn read_union(
    sources: &[OverviewSource],
    workspace: &str,
    query: &str,
) -> Result<ViewerReply, ViewerReply> {
    let invalid = || {
        error(
            400,
            "invalid_arguments",
            "Invalid workspace Catalog query.",
            false,
        )
    };
    if sources.len() > MAX_SOURCES {
        return Err(error(
            503,
            "catalog_capacity",
            "Too many session sources for this read. Select a machine.",
            true,
        ));
    }
    let args: Vec<_> = query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| part.split_once('=').unwrap_or((part, "")))
        .collect();
    if query.len() > 8192
        || args.iter().any(|(name, _)| {
            !matches!(
                *name,
                "scope" | "limit" | "harness" | "repo" | "no_repo" | "model" | "q" | "cursor"
            )
        })
    {
        return Err(invalid());
    }
    let cursors: Vec<_> = args.iter().filter(|(name, _)| *name == "cursor").collect();
    if cursors.len() > 1 {
        return Err(invalid());
    }
    let cursor: Option<UnionCursor> = cursors
        .first()
        .map(|(_, value)| {
            let value = decoded(value).ok_or_else(invalid)?;
            serde_json::from_str(&value).map_err(|_| invalid())
        })
        .transpose()?;
    let clean = query
        .split('&')
        .filter(|part| part.split('=').next() != Some("cursor"))
        .collect::<Vec<_>>()
        .join("&");
    let request = Request::parse(&clean)?;
    let filters = serde_json::to_string(&(
        request.scope,
        &request.harness,
        &request.repo,
        &request.model,
        &request.q,
    ))
    .map_err(|_| invalid())?;
    let filters = format!("{:x}", Sha256::digest(filters.as_bytes()));
    if cursor.as_ref().is_some_and(|c| {
        c.version != 1
            || c.workspace != workspace
            || c.filters != filters
            || c.after.key.is_empty()
            || c.after.key.len() > 4096
            || c.after.source.is_empty()
    }) {
        return Err(error(
            400,
            "cursor_scope_mismatch",
            "Discard the cursor when changing workspace, machine or filters.",
            false,
        ));
    }
    let mut ordered: Vec<_> = sources.iter().collect();
    ordered.sort_by(|a, b| a.key.cmp(&b.key));
    if ordered.windows(2).any(|pair| pair[0].key == pair[1].key) {
        return Err(invalid());
    }
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&(workspace, &filters)).map_err(|_| invalid())?);
    let mut found: Vec<(Position, Value)> = Vec::new();
    let mut frontier: Option<Position> = None;
    let mut partial = false;
    let mut freshness = "cached";
    let mut observed: Option<i64> = None;
    let mut search_partial = false;
    let mut search_complete = true;
    let mut candidates = 0;
    for source in ordered {
        let mut focused = Request::parse(&clean)?;
        // Only the metadata generation is needed to reuse the focused reader's
        // keyset range. Never probe a session through its selected detail API.
        let generation = if cursor.is_some() {
            let unavailable = || {
                error(
                    503,
                    "catalog_unavailable",
                    "A session source Catalog is unavailable. Retry after publication.",
                    true,
                )
            };
            let connection = Connection::open_with_flags(
                EventCache::path(&source.options.cache),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|_| unavailable())?;
            connection
                .busy_timeout(Duration::from_millis(100))
                .map_err(|_| unavailable())?;
            Some(
                connection
                    .query_row(
                        "SELECT value FROM meta WHERE key='catalog_generation'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|_| unavailable())?,
            )
        } else {
            None
        };
        if let Some(c) = &cursor {
            focused.cursor = Some(Cursor {
                inclusive: source.key > c.after.source,
                scope: focused.scope,
                version: 1,
                generation: generation.clone().ok_or_else(invalid)?,
                machine: source.key.clone(),
                harness: focused.harness.clone(),
                repo: focused.repo.clone(),
                model: focused.model.clone(),
                q: focused.q.clone(),
                last: c.after.last,
                key: c.after.key.clone(),
            });
        }
        let result = page(&source.options, &source.key, focused);
        if result.status != 200 {
            return Err(result);
        }
        let mut result: Value = serde_json::from_slice(&result.body).map_err(|_| invalid())?;
        if generation
            .as_ref()
            .is_some_and(|expected| result["generation"] != *expected)
        {
            return Err(error(
                409,
                "stale_cursor",
                "The catalog changed. Refresh the list.",
                true,
            ));
        }
        digest.update(
            serde_json::to_vec(&(
                &source.key,
                result["generation"].as_str().ok_or_else(invalid)?,
            ))
            .map_err(|_| invalid())?,
        );
        partial |= result["completeness"]["state"] == "partial";
        match result["freshness"].as_str() {
            Some("unavailable") => freshness = "unavailable",
            Some("incomplete") if freshness != "unavailable" => freshness = "incomplete",
            Some("stale") if freshness == "cached" => freshness = "stale",
            _ => {}
        }
        if let Some(time) = result["observed_at"].as_i64() {
            observed = Some(observed.map_or(time, |old| old.min(time)));
        }
        if let Some(search) = result.get("search").filter(|s| !s.is_null()) {
            search_partial |= search["partial"] == true;
            search_complete &= search["index_complete"] == true;
            candidates += search["candidates"].as_u64().unwrap_or(0);
        }
        if let Some(next) = result["next_cursor"].as_str() {
            let next: Cursor = serde_json::from_str(next).map_err(|_| invalid())?;
            let position = Position {
                last: next.last,
                key: next.key,
                source: source.key.clone(),
            };
            if frontier.as_ref().is_none_or(|old| position < *old) {
                frontier = Some(position);
            }
        }
        for mut item in result["items"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .drain(..)
        {
            let position = Position {
                last: item["last"].as_i64().unwrap_or(0),
                key: item["key"].as_str().ok_or_else(invalid)?.into(),
                source: source.key.clone(),
            };
            item["machine"] = json!({"key":source.key,"label":source.label});
            found.push((position, item));
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found.truncate(request.limit + 1);
    }
    let generation = format!("{:x}", digest.finalize());
    if cursor.as_ref().is_some_and(|c| c.generation != generation) {
        return Err(error(
            409,
            "stale_cursor",
            "A source or its Catalog changed. Refresh the list.",
            true,
        ));
    }
    // A bounded metadata search may stop before finding a full page. Publish
    // only through the earliest scanned frontier, then resume every source from
    // that same global keyset. Unscanned matches can never become unreachable.
    if let Some(end) = &frontier {
        found.retain(|(position, _)| position <= end);
    }
    let more = found.len() > request.limit;
    found.truncate(request.limit);
    let after = if more {
        found.last().map(|(p, _)| p.clone())
    } else {
        frontier
    };
    let next = after
        .map(|after| {
            serde_json::to_string(&UnionCursor {
                version: 1,
                workspace: workspace.into(),
                filters,
                generation: generation.clone(),
                after,
            })
        })
        .transpose()
        .map_err(|_| invalid())?;
    Ok(reply(
        200,
        json!({
            "api":1,"machine":"","read_scope":request.scope,"generation":generation,"observed_at":observed,
            "machine_info":{"key":"","label":"All machines","freshness":"cached"},
            "capabilities":{"pagination":true,"metadata_search":true,"filters":["harness","repo","no_repo","model","q"],"order":"last_desc_key_asc_machine_asc","full_text_search":false,"selected_session_lookup":false,"runtime_status":true,"global_union":true},
            "completeness":{"state":if partial {"partial"} else {"complete"}},"freshness":if partial {"updating"} else {freshness},
            "items":found.into_iter().map(|(_,item)|item).collect::<Vec<_>>(),"next_cursor":next,
            "search":request.q.as_ref().map(|_|json!({"semantics":"unicode_lowercase_substring","fields":["name","key","repo","branch","model","harness"],"partial":search_partial,"candidates":candidates,"index_complete":search_complete,"candidate_budget":MAX_SOURCES*crate::catalog_search::MAX_CANDIDATES,"byte_budget":MAX_SOURCES*crate::catalog_search::MAX_SCAN_BYTES}))
        }),
    ))
}
