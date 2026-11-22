//! Viewer transport over source-qualified warehouse reads.
use super::selected::render::{EntryAddress, Expansion, Part};
use super::{CatalogReadScope, Continuation, ReadError, Request, Row, Source, index, read_list};
use crate::{OverviewSource, viewer::ViewerReply};
use serde_json::{Value, json};

fn error(status: u16, code: &str, message: &str, retryable: bool) -> ViewerReply {
    crate::catalog::error(status, code, message, retryable)
}

fn invalid() -> ViewerReply {
    error(400, "invalid_arguments", "Invalid Catalog query.", false)
}

fn read_error(error_value: ReadError) -> ViewerReply {
    match error_value {
        ReadError::Scope => error(
            400,
            "cursor_scope_mismatch",
            "Discard the cursor when changing workspace, source or filters.",
            false,
        ),
        ReadError::Restart => error(
            409,
            "stale_cursor",
            "The committed session view changed. Restart at the first page.",
            true,
        ),
        ReadError::RetainedUnverified => error(
            409,
            "retained_identity_unverified",
            "No verified original warehouse identity is available for this retained key.",
            false,
        ),
        ReadError::PeerUnavailable(reason) => {
            error(409, "retained_peer_unavailable", &reason, false)
        }
        ReadError::Index(_) => error(
            503,
            "catalog_unavailable",
            "The committed session Index is unavailable.",
            true,
        ),
    }
}

fn parse_list(query: &str, union: bool) -> Result<(Request, Option<Continuation>), ViewerReply> {
    if query.len() > 64 * 1024 {
        return Err(invalid());
    }
    let mut clean = Vec::new();
    let mut cursor = None;
    for part in query.split('&').filter(|part| !part.is_empty()) {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        match name {
            "cursor" if cursor.is_none() => {
                let decoded = crate::viewer::decoded(value).ok_or_else(invalid)?;
                if decoded.len() > 8192 {
                    return Err(invalid());
                }
                let parsed: Continuation = serde_json::from_str(&decoded).map_err(|_| invalid())?;
                if !crate::session_route::identifier(&parsed.boundary.position.key)
                    || !crate::session_route::identifier(&parsed.boundary.position.source)
                    || [&parsed.revision, &parsed.query].into_iter().any(|digest| {
                        digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                {
                    return Err(invalid());
                }
                cursor = Some(parsed);
            }
            "scope" | "limit" | "harness" | "repo" | "no_repo" | "model" | "q" => clean.push(part),
            "machine" if !union => (),
            _ => return Err(invalid()),
        }
    }
    let request = Request::parse(&clean.join("&"))?;
    Ok((request, cursor))
}

fn list_item(
    row: Row,
    label: Option<&str>,
    annotations: &crate::annotations::Annotations,
) -> Value {
    let [
        input,
        cached,
        output,
        shared_input,
        shared_cached,
        shared_output,
    ] = row.usage;
    let million = |tokens: i64| (tokens as f64 / 1000.0).round() / 1000.0;
    let input = input.saturating_add(shared_input);
    let cached = cached.saturating_add(shared_cached);
    let tokens = [
        million(input.saturating_sub(cached).max(0)),
        million(cached),
        million(output.saturating_add(shared_output)),
    ];
    let annotation = row
        .native_ids
        .get(&row.position.key)
        .or_else(|| {
            (row.native_ids.len() == 1)
                .then(|| row.native_ids.first())
                .flatten()
        })
        .and_then(|id| annotations.get(&row.harness, id));
    let mut item = json!({
        "lifecycle":"current",
        "key":row.position.key,"name":row.title,"harness":row.harness,
        "kind":row.kind,"presentation_kind":row.presentation_kind,
        "native_ids":row.native_ids,"parent":row.parent,"parent_source":row.parent_source,
        "repo":row.repo,"branch":row.branch,"model":row.model,"effort":null,
        "start":row.start,"last":(row.position.last != 0).then_some(row.position.last),
        "tokens":tokens,"cost":row.cost,"source_refs":row.source_refs,
        "freshness":{"state":"cached"},"annotation":annotation,
        "status":{"state":row.status,"observation":if row.status.is_some() { "current" } else { "unavailable" }},
    });
    if let Some(label) = label {
        item["machine"] = json!({"key":row.position.source,"label":label});
    }
    item
}

/// The caller obtains these sources from the host's authorized inventory.
/// A focused request uses one source. Workspace reads merge committed source pages.
pub(super) fn list(
    configured: &[OverviewSource],
    workspace: &str,
    query: &str,
    union: bool,
) -> ViewerReply {
    match read(configured, workspace, query, union) {
        Ok(reply) | Err(reply) => reply,
    }
}

fn read(
    configured: &[OverviewSource],
    workspace: &str,
    query: &str,
    union: bool,
) -> Result<ViewerReply, ViewerReply> {
    let (request, cursor) = parse_list(query, union)?;
    if configured.len() > 64 {
        return Err(error(
            503,
            "catalog_capacity",
            "Select a machine for this read.",
            true,
        ));
    }
    if !union && configured.len() != 1 {
        return Err(invalid());
    }
    if request.scope == CatalogReadScope::RetainedHistory {
        return Err(read_error(ReadError::RetainedUnverified));
    }
    for source in configured {
        if let Some(reply) = crate::index_rebuild::history_reply(&source.options) {
            return Err(reply);
        }
    }
    let paths: Vec<_> = configured
        .iter()
        .map(|source| crate::events::EventCache::path(&source.options.cache))
        .collect();
    let sources = configured
        .iter()
        .zip(&paths)
        .map(|(source, path)| {
            let namespace = crate::storage::Namespace::new(&source.key).map_err(index)?;
            Source::discover(namespace, path, &source.options)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    let page = read_list(&sources, workspace, &request, cursor.as_ref()).map_err(read_error)?;
    let annotations = configured
        .iter()
        .map(|source| {
            crate::annotations::Annotations::read(&source.options.annotations)
                .map(|annotations| (source.key.as_str(), (source.label.as_str(), annotations)))
        })
        .collect::<std::io::Result<std::collections::BTreeMap<_, _>>>()
        .map_err(|err| read_error(index(err)))?;
    let items: Vec<_> = page
        .rows
        .into_iter()
        .map(|row| {
            let (label, annotations) = &annotations[row.position.source.as_str()];
            list_item(row, union.then_some(*label), annotations)
        })
        .collect();
    let next = page
        .next
        .map(|next| super::encode(&next))
        .transpose()
        .map_err(read_error)?;
    let (machine, label) = if union {
        ("", "All machines")
    } else {
        (configured[0].key.as_str(), configured[0].label.as_str())
    };
    let partial = page.coverage.pending != 0 || page.coverage.unavailable != 0;
    let scale = if union { 64 } else { 1 };
    Ok(crate::catalog::reply(
        200,
        json!({
            "api":1,"machine":machine,"read_scope":request.scope,"generation":page.generation,
            "observed_at":null,"machine_info":{"key":machine,"label":label,"freshness":"cached"},
            "capabilities":{"pagination":true,"metadata_search":true,"filters":["harness","repo","no_repo","model","q"],
                "order":if union { "last_desc_key_asc_machine_asc" } else { "last_desc_key_asc" },
                "full_text_search":false,"selected_session_lookup":!union,"runtime_status":true,"global_union":union},
            "annotations":{"files":annotations.values().map(|(_, value)| &value.status).collect::<Vec<_>>()},
            "completeness":{"state":if partial { "partial" } else { "complete" }},
            "freshness":if partial { "updating" } else { "cached" },
            "coverage":{"committed":page.coverage.committed,"pending":page.coverage.pending,"unavailable":page.coverage.unavailable},
            "total":page.total,"items":items,"next_cursor":next,
            "search":request.q.as_ref().map(|_| json!({
                "semantics":"unicode_lowercase_substring","fields":["name","key","repo","branch","model","harness"],
                "partial":next.is_some(),"candidates":page.examined,"index_complete":!partial,
                "candidate_budget":scale * crate::catalog_search::MAX_CANDIDATES,
                "byte_budget":scale * crate::catalog_search::MAX_SCAN_BYTES,
            })),
        }),
    ))
}

impl EntryAddress {
    fn parse(value: &str) -> Result<Self, ViewerReply> {
        let address: Self = serde_json::from_str(value).map_err(|_| invalid())?;
        if !crate::session_route::identifier(&address.record.path)
            || address.record.offset > i64::MAX as u64
            || address.record.block < 0
            || address.record.occurrence < 0
            || address.binding.len() != 64
            || !address.binding.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !crate::session_route::identifier(&address.entry_id)
        {
            return Err(invalid());
        }
        Ok(address)
    }
}

pub(super) fn transcript(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    match read_transcript(
        configured,
        query,
        current_provider,
        retained_provider,
        false,
    ) {
        Ok(reply) | Err(reply) => reply,
    }
}

pub(super) fn entry(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    match read_transcript(configured, query, current_provider, retained_provider, true) {
        Ok(reply) | Err(reply) => reply,
    }
}

fn read_transcript(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
    expanded: bool,
) -> Result<ViewerReply, ViewerReply> {
    use super::selected;
    if query.len() > 128 * 1024 {
        return Err(invalid());
    }
    let mut arguments = std::collections::BTreeMap::new();
    for field in query.split('&').filter(|field| !field.is_empty()) {
        let (name, value) = field.split_once('=').ok_or_else(invalid)?;
        if !matches!(
            name,
            "machine"
                | "sid"
                | "scope"
                | "limit"
                | "cursor"
                | "address"
                | "part"
                | "image"
                | "field_chunk"
        ) || arguments
            .insert(name, crate::viewer::decoded(value).ok_or_else(invalid)?)
            .is_some()
        {
            return Err(invalid());
        }
    }
    let argument = |name| arguments.get(name).map(String::as_str);
    if argument("machine").is_some_and(|machine| machine != configured.key) {
        return Err(invalid());
    }
    let key = argument("sid")
        .filter(|key| crate::session_route::identifier(key))
        .ok_or_else(invalid)?;
    let scope = match argument("scope") {
        None | Some("current") => CatalogReadScope::Current,
        Some("retained_history") => CatalogReadScope::RetainedHistory,
        Some(_) => return Err(invalid()),
    };
    let limit = argument("limit")
        .map_or(Ok(32), str::parse::<usize>)
        .ok()
        .filter(|limit| (1..=32).contains(limit))
        .ok_or_else(invalid)?;
    let cursor: Option<selected::Cursor> = argument("cursor")
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| invalid())?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.touched_count() > 32)
    {
        return Err(invalid());
    }
    let address = argument("address").map(EntryAddress::parse).transpose()?;
    if address.is_some() && (cursor.is_some() || argument("limit").is_some()) {
        return Err(invalid());
    }
    let part = match (
        expanded,
        argument("part"),
        argument("image"),
        argument("field_chunk"),
    ) {
        (false, None, None, None) => None,
        (true, Some("output"), None, None) => Some(Part::Output),
        (true, Some("script"), None, None) => Some(Part::Script),
        (true, Some("attachment"), Some(image), None) => {
            Some(Part::Attachment(image.parse().map_err(|_| invalid())?))
        }
        (true, None, None, Some(chunk)) => Some(Part::Text(chunk.parse().map_err(|_| invalid())?)),
        _ => return Err(invalid()),
    };
    if part.is_some() && address.is_none() {
        return Err(invalid());
    }
    if let Some(reply) = crate::index_rebuild::history_reply(&configured.options) {
        return Err(reply);
    }
    let path = crate::events::EventCache::path(&configured.options.cache);
    let namespace = crate::storage::Namespace::new(&configured.key)
        .map_err(|error| read_error(index(error)))?;
    let source = Source::focused(namespace, &path, &configured.options, current_provider)
        .map_err(read_error)?;
    let component = match scope {
        CatalogReadScope::Current => match &address {
            None => selected::latest_page(source.options, &source, key, limit, cursor.as_ref()),
            Some(address) => {
                selected::addressed_window(&source, key, &address.record, &address.binding)
            }
        },
        CatalogReadScope::RetainedHistory => {
            let Some(provider) = retained_provider else {
                // Read the binding before reporting unavailable custody so an
                // unverified original receives the same refusal on every route.
                super::focused_identity(&source, scope, key).map_err(read_error)?;
                return Err(error(
                    503,
                    "retained_source_unavailable",
                    "No authorized source reader is available for this retained session.",
                    false,
                ));
            };
            match &address {
                None => {
                    selected::retained_latest_page(&source, key, limit, cursor.as_ref(), provider)
                }
                Some(address) => selected::retained_addressed_window(
                    &source,
                    key,
                    &address.record,
                    &address.binding,
                    provider,
                ),
            }
            .map(Some)
        }
    }
    .map_err(read_error)?;
    let Some(component) = component else {
        return Err(error(
            404,
            "session_not_found",
            "Session is unavailable.",
            false,
        ));
    };
    let entries =
        selected::render::entries(&component, &configured.key, key, crate::model::now_ms());
    if let Some(address) = &address
        && !entries.iter().any(|entry| {
            entry["entry_id"] == address.entry_id && entry["address"]["block"] == address.block
        })
    {
        return Err(error(
            404,
            "entry_not_found",
            "This native transcript entry is unavailable.",
            false,
        ));
    }
    if let (Some(address), Some(part)) = (&address, part) {
        return match selected::render::expand(&component, &configured.key, key, address, part) {
            Some(Expansion::Json(entry)) => Ok(crate::catalog::reply(
                200,
                json!({
                    "api":1,"projection":7,"source_key":configured.key,"catalog_key":key,
                    "read_scope":scope,"binding":component.binding,"entry":entry,
                }),
            )),
            Some(Expansion::Image(content_type, body)) => Ok(ViewerReply {
                status: 200,
                content_type,
                body,
                etag: None,
                extra_headers: Vec::new(),
            }),
            None if matches!(part, Part::Attachment(_)) => Err(error(
                404,
                "attachment_unavailable",
                "This recorded image is unavailable.",
                false,
            )),
            None => Err(error(
                422,
                "field_unsupported",
                "This native entry has no qualified expansion for the requested field.",
                false,
            )),
        };
    }
    let earlier = component
        .next
        .as_ref()
        .map(super::encode)
        .transpose()
        .map_err(read_error)?;
    Ok(crate::catalog::reply(
        200,
        json!({
            "api":1, "projection":7,
            "source_key":configured.key, "catalog_key":key, "read_scope":scope,
            "binding":component.binding, "earlier_cursor":earlier, "entries":entries,
            "position":if address.is_some() { "address" } else if cursor.is_some() { "earlier" } else { "latest" },
            "selected":{"harness":component.harness,"native_id":component.native_id,"title":component.title,"model":component.model},
        }),
    ))
}
