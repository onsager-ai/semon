//! Metadata-only permalink normalization into the canonical native reader.
use crate::{Options, catalog, events::EventCache, viewer::ViewerReply};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::time::Duration;

/// The host authorizes one Source before calling this function. Legacy failed
/// send bookmarks resolve to real originating sessions, never virtual readers.
pub fn session_bookmark(options: &Options, source: &str, query: &str) -> ViewerReply {
    session_bookmark_with_gate(options, source, query, || Ok(()))
}

pub(crate) fn session_bookmark_with_gate(
    options: &Options,
    source: &str,
    query: &str,
    before_read: impl FnOnce() -> Result<(), ViewerReply>,
) -> ViewerReply {
    let sid = match parse_session_bookmark_query(source, query) {
        Ok(sid) => sid,
        Err(reply) => return reply,
    };
    if let Err(reply) = before_read() {
        return reply;
    }
    let selected = format!(
        "sid={}&scope=retained_history&limit=1",
        crate::viewer::percent_encode(&sid)
    );
    let page = crate::session_catalog_page(options, source, &selected);
    let Ok(value) = serde_json::from_slice::<Value>(&page.body) else {
        return page;
    };
    if page.status == 200 {
        let Some(key) = value["items"][0]["key"].as_str() else {
            return page;
        };
        return reply(
            vec![
                json!({"source_key":source,"catalog_key":key,"harness":value["items"][0]["harness"]}),
            ],
            usize::from(value["completeness"]["state"] != "complete"),
        );
    }
    if page.status != 404 {
        return page;
    }
    let readiness = crate::session_catalog_page(options, source, "scope=retained_history&limit=1");
    if readiness.status != 200 {
        return readiness;
    }
    let Ok(ready) = serde_json::from_slice::<Value>(&readiness.body) else {
        return readiness;
    };
    let unavailable = usize::from(ready["completeness"]["state"] != "complete");
    let Some(target) = sid.strip_prefix("unsent:") else {
        return reply(Vec::new(), unavailable);
    };
    let lookup = || -> rusqlite::Result<Vec<String>> {
        let connection = Connection::open_with_flags(
            EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(100))?;
        connection.prepare("SELECT DISTINCT session_key FROM session_trace_bookmarks WHERE target=?1 ORDER BY session_key LIMIT 3")?
            .query_map([target], |row| row.get(0))?.collect()
    };
    let Ok(keys) = lookup() else {
        return reply(Vec::new(), 1);
    };
    let mut matches = Vec::new();
    let mut unavailable = unavailable;
    for key in keys {
        match crate::session_catalog_history_identity(options, source, &key) {
            Ok(Some(identity)) => matches
                .push(json!({"source_key":source,"catalog_key":key,"harness":identity.harness})),
            Ok(None) => {}
            Err(_) => unavailable += 1,
        }
    }
    reply(matches, unavailable)
}

fn parse_session_bookmark_query(source: &str, query: &str) -> Result<String, ViewerReply> {
    let args = query
        .split('&')
        .map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((crate::viewer::decoded(key)?, crate::viewer::decoded(value)?))
        })
        .collect::<Option<Vec<_>>>();
    let Some(args) = args else {
        return Err(catalog::error(
            400,
            "invalid_arguments",
            "Invalid bookmark encoding.",
            false,
        ));
    };
    let sids: Vec<_> = args.iter().filter(|(key, _)| key == "sid").collect();
    if query.len() > 8192
        || sids.len() != 1
        || sids[0].1.is_empty()
        || sids[0].1.len() > 4096
        || sids[0].1.chars().any(char::is_control)
        || args
            .iter()
            .any(|(key, _)| !matches!(key.as_str(), "sid" | "machine"))
        || args.iter().filter(|(key, _)| key == "machine").count() > 1
        || args
            .iter()
            .any(|(key, value)| key == "machine" && value != source)
    {
        return Err(catalog::error(
            400,
            "invalid_arguments",
            "Supply one session bookmark.",
            false,
        ));
    }
    Ok(sids[0].1.clone())
}

fn reply(matches: Vec<Value>, unavailable: usize) -> ViewerReply {
    ViewerReply {
        status: 200,
        content_type: "application/json; charset=utf-8",
        body: json!({"api":1,"matches":matches,"unavailable":unavailable})
            .to_string()
            .into_bytes(),
        etag: None,
    }
}
