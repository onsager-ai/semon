//! Viewer transport over source-qualified warehouse reads.
use super::selected::render::{Expansion, Part};
use super::{CatalogReadScope, Continuation, ReadError, Request, Row, Source, index, read_list};
use crate::{OverviewSource, session_route::NativeEntryAddress, viewer::ViewerReply};
use serde_json::{Value, json};

fn error(status: u16, code: &str, message: &str, retryable: bool) -> ViewerReply {
    crate::catalog::error(status, code, message, retryable)
}

fn invalid() -> ViewerReply {
    error(400, "invalid_arguments", "Invalid Catalog query.", false)
}

fn not_found() -> ViewerReply {
    error(404, "session_not_found", "Session is unavailable.", false)
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
        ReadError::RetainedPreparing => error(
            503,
            "retained_source_preparing",
            "Preparing recorded history.",
            true,
        ),
        ReadError::RetainedUnavailable => error(
            503,
            "retained_source_unavailable",
            "The frozen session prefix is unavailable from its authorized source reader.",
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
            "scope" | "limit" | "harness" | "repo" | "no_repo" | "model" | "q" | "sid" => {
                clean.push(part)
            }
            "machine" if !union => (),
            _ => return Err(invalid()),
        }
    }
    let request = Request::parse(&clean.join("&"))?;
    if request
        .sid
        .as_ref()
        .is_some_and(|key| union || cursor.is_some() || !crate::session_route::identifier(key))
    {
        return Err(invalid());
    }
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
    let status = row.recorded_status();
    let mut item = json!({
        "lifecycle":"current",
        "key":row.position.key,"name":row.title,"harness":row.harness,
        "kind":row.kind,"presentation_kind":row.presentation_kind,
        "native_ids":row.native_ids,"parent":row.parent,"parent_source":row.parent_source,
        "repo":row.repo,"branch":row.branch,"model":row.model,"effort":row.effort,
        "start":row.start,"last":(row.position.last != 0).then_some(row.position.last),
        "tokens":tokens,"cost":row.cost,"source_refs":row.source_refs,
        "freshness":{"state":"cached"},"annotation":annotation,
        "status":status,
    });
    if let Some(details) = row.details {
        item["details"] = details;
    }
    if let Some(label) = label {
        item["machine"] = json!({"key":row.position.source,"label":label});
    }
    item
}

fn retained_item(
    mut metadata: Value,
    identity: &crate::CatalogSessionIdentity,
    annotations: &crate::annotations::Annotations,
) -> Value {
    let annotation = identity
        .native_id
        .as_deref()
        .or_else(|| {
            identity
                .native_ids
                .iter()
                .find(|id| **id == identity.catalog_key)
                .map(String::as_str)
        })
        .and_then(|id| annotations.get(&identity.harness, id));
    metadata["lifecycle"] = json!("retained");
    metadata["source_refs"] = json!(identity.source_refs);
    metadata["freshness"] = json!({"state":"unavailable"});
    metadata["facts_observation"] = json!(identity.facts_observation);
    metadata["native_selection"] = json!(identity.native_selection);
    metadata["status"] = json!({"state":null,"observation":"unavailable"});
    metadata["annotation"] = json!(annotation);
    metadata
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
    for source in configured {
        if let Some(reply) = crate::index_rebuild::history_reply(&source.options) {
            return Err(reply);
        }
    }
    if let Some(key) = request.sid.as_deref() {
        let configured = &configured[0];
        let path = crate::events::EventCache::path(&configured.options.cache);
        let namespace = crate::storage::Namespace::new(&configured.key)
            .map_err(index)
            .map_err(read_error)?;
        let source =
            Source::focused(namespace, &path, &configured.options, None).map_err(read_error)?;
        let annotations = read_annotations(configured)?;
        let (item, generation) = match request.scope {
            CatalogReadScope::Current => {
                let (row, generation) = super::focused_metadata(&source, key)
                    .map_err(read_error)?
                    .ok_or_else(not_found)?;
                (list_item(row, None, &annotations), generation)
            }
            CatalogReadScope::RetainedHistory => {
                let (metadata, identity) =
                    super::focused_retained_metadata(&source, key).map_err(read_error)?;
                (
                    retained_item(metadata, &identity, &annotations),
                    identity.generation,
                )
            }
        };
        return Ok(crate::catalog::reply(
            200,
            json!({
                "api": 1, "machine": configured.key, "read_scope": request.scope, "generation": generation,
                "observed_at": null, "machine_info": {"key": configured.key, "label": configured.label, "freshness": "cached"},
                "freshness": "cached", "completeness": {"state": "complete"},
                "capabilities": {"pagination": true, "metadata_search": true, "filters": ["harness", "repo", "no_repo", "model", "q"],
                    "order": "last_desc_key_asc", "full_text_search": false, "selected_session_lookup": true,
                    "runtime_status": true, "global_union": false},
                "annotations": {"files": [&annotations.status]}, "total": 1, "next_cursor": null,
                "items": [item],
            }),
        ));
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
            match request.scope {
                CatalogReadScope::Current => Source::discover(namespace, path, &source.options),
                CatalogReadScope::RetainedHistory => {
                    Source::focused(namespace, path, &source.options, None)
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(read_error)?;
    let annotations = configured
        .iter()
        .map(|source| {
            crate::annotations::Annotations::read(&source.options.annotations)
                .map(|annotations| (source.key.as_str(), (source.label.as_str(), annotations)))
        })
        .collect::<std::io::Result<std::collections::BTreeMap<_, _>>>()
        .map_err(|err| read_error(index(err)))?;
    let page = match request.scope {
        CatalogReadScope::Current => read_list(&sources, workspace, &request, cursor.as_ref())
            .map_err(read_error)?
            .map(|row| {
                let (label, annotations) = &annotations[row.position.source.as_str()];
                list_item(row, union.then_some(*label), annotations)
            }),
        CatalogReadScope::RetainedHistory => {
            super::read_retained_list(&sources, workspace, &request, cursor.as_ref())
                .map_err(read_error)?
                .map(|row| {
                    let (label, annotations) = &annotations[row.position.source.as_str()];
                    let mut item = retained_item(row.metadata, &row.identity, annotations);
                    if union {
                        item["machine"] = json!({"key":row.position.source,"label":label});
                    }
                    item
                })
        }
    };
    let items = page.rows;
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

fn selected_request(configured: &OverviewSource, query: &str) -> Result<Request, ViewerReply> {
    let mut arguments = std::collections::BTreeSet::new();
    for field in query.split('&') {
        let (name, value) = field.split_once('=').ok_or_else(invalid)?;
        if !matches!(name, "sid" | "scope" | "machine") || !arguments.insert(name) {
            return Err(invalid());
        }
        let value = crate::viewer::decoded(value).ok_or_else(invalid)?;
        if name == "machine" && value != configured.key {
            return Err(invalid());
        }
    }
    let (request, _) = parse_list(query, false)?;
    if request.sid.is_none() {
        return Err(invalid());
    }
    Ok(request)
}

fn selected_identity(
    configured: &OverviewSource,
    query: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, ViewerReply> {
    let request = selected_request(configured, query)?;
    if let Some(reply) = crate::index_rebuild::history_reply(&configured.options) {
        return Err(reply);
    }
    let path = crate::events::EventCache::path(&configured.options.cache);
    let namespace = crate::storage::Namespace::new(&configured.key)
        .map_err(index)
        .map_err(read_error)?;
    let source =
        Source::focused(namespace, &path, &configured.options, None).map_err(read_error)?;
    super::focused_identity(&source, request.scope, &request.sid.expect("validated sid"))
        .map_err(read_error)
}

pub(super) fn identity(configured: &OverviewSource, query: &str) -> ViewerReply {
    match selected_identity(configured, query) {
        Ok(Some(identity)) => crate::catalog::reply(200, json!({"api":1,"identity":identity})),
        Ok(None) => not_found(),
        Err(reply) => reply,
    }
}

pub(super) fn locate(configured: &OverviewSource, query: &str) -> ViewerReply {
    match selected_identity(configured, query) {
        Ok(identity) => {
            let matches = identity
                .into_iter()
                .map(|identity| crate::session_route::Candidate {
                    source_key: identity.source_key,
                    catalog_key: identity.catalog_key,
                    harness: identity.harness,
                })
                .collect::<Vec<_>>();
            crate::catalog::reply(200, json!({"api":1,"matches":matches,"unavailable":0}))
        }
        Err(reply) => reply,
    }
}

pub(super) fn resolve_native(configured: &OverviewSource, query: &str) -> ViewerReply {
    use crate::{
        CatalogIdentityError, CatalogNativeResolution, CatalogNativeResolutionState as State,
    };

    if query.len() > 64 * 1024 {
        return invalid();
    }
    let mut seen = std::collections::BTreeSet::new();
    for field in query.split('&').filter(|field| !field.is_empty()) {
        let Some((name, value)) = field.split_once('=') else {
            return invalid();
        };
        if !seen.insert(name) || !matches!(name, "machine" | "harness" | "native_id" | "scope") {
            return invalid();
        }
        let Some(value) = crate::viewer::decoded(value) else {
            return invalid();
        };
        if name == "machine" && value != configured.key {
            return invalid();
        }
    }
    crate::native_resolution::page_with_reader(
        &configured.key,
        query,
        || crate::index_rebuild::history_reply(&configured.options).map_or(Ok(()), Err),
        |harness, native_id, scope| {
            let unavailable = |_| CatalogIdentityError::Unavailable;
            let path = crate::events::EventCache::path(&configured.options.cache);
            let namespace = crate::storage::Namespace::new(&configured.key)
                .map_err(index)
                .map_err(unavailable)?;
            let source = Source::focused(namespace, &path, &configured.options, None)
                .map_err(unavailable)?;
            let matches =
                super::resolve_native(&source, scope, harness, native_id).map_err(unavailable)?;
            let (state, catalog_key, generation) = match matches.as_slice() {
                [] => (State::Pending, None, None),
                [identity] => {
                    let state = match identity.owner_qualification {
                        crate::catalog::CatalogOwnerQualification::Qualified => State::Resolved,
                        crate::catalog::CatalogOwnerQualification::Provisional => {
                            State::Provisional
                        }
                    };
                    (
                        state,
                        Some(identity.catalog_key.clone()),
                        Some(identity.generation.clone()),
                    )
                }
                _ => (State::Ambiguous, None, None),
            };
            Ok(CatalogNativeResolution {
                state,
                source_key: configured.key.clone(),
                harness: harness.into(),
                native_id: native_id.into(),
                read_scope: scope,
                catalog_key,
                generation,
            })
        },
    )
}

pub(super) fn annotations(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    let read = || -> Result<ViewerReply, ViewerReply> {
        if query.len() > 64 * 1024 {
            return Err(invalid());
        }
        let mut selected_query = Vec::new();
        let mut arguments = std::collections::BTreeMap::new();
        for field in query.split('&') {
            let (name, value) = field.split_once('=').ok_or_else(invalid)?;
            if matches!(name, "kind" | "generation" | "after" | "limit") {
                if arguments
                    .insert(name, crate::viewer::decoded(value).ok_or_else(invalid)?)
                    .is_some()
                {
                    return Err(invalid());
                }
            } else {
                selected_query.push(field);
            }
        }
        let argument = |key| arguments.get(key).map(String::as_str);
        let kind = argument("kind")
            .filter(|kind| matches!(*kind, "errors" | "signals"))
            .ok_or_else(invalid)?;
        let first = argument("after")
            .unwrap_or("0")
            .parse::<i64>()
            .ok()
            .filter(|value| (0..=9_007_199_254_740_991).contains(value))
            .ok_or_else(invalid)?;
        let limit = argument("limit")
            .unwrap_or("100")
            .parse::<i64>()
            .ok()
            .filter(|value| (1..=100).contains(value))
            .ok_or_else(invalid)?;
        let generation = argument("generation");
        if generation.is_none() && first != 0
            || generation.is_some_and(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(invalid());
        }
        let request = selected_request(configured, &selected_query.join("&"))?;
        let key = request.sid.as_deref().ok_or_else(invalid)?;
        if let Some(reply) = crate::index_rebuild::history_reply(&configured.options) {
            return Err(reply);
        }
        let path = crate::events::EventCache::path(&configured.options.cache);
        let namespace = crate::storage::Namespace::new(&configured.key)
            .map_err(index)
            .map_err(read_error)?;
        let source = Source::focused(namespace, &path, &configured.options, current_provider)
            .map_err(read_error)?;
        let query = super::selected::AnnotationRequest { kind, first, limit };
        let component = match request.scope {
            CatalogReadScope::Current => super::selected::annotations(&source, key, query),
            CatalogReadScope::RetainedHistory => {
                let archive = super::ArchiveReader::new(&configured.options);
                super::selected::retained_annotations(
                    &source,
                    key,
                    query,
                    retained_provider.unwrap_or(&archive),
                )
                .map(Some)
            }
        }
        .map_err(read_error)?
        .ok_or_else(not_found)?;
        let observed = component.identity.generation;
        if generation.is_some_and(|expected| expected != observed) {
            return Err(read_error(ReadError::Restart));
        }
        let page = component
            .annotations
            .ok_or_else(|| read_error(ReadError::Restart))?;
        if first > page.total {
            return Err(invalid());
        }
        Ok(crate::catalog::reply(
            200,
            json!({
                "api":1,"projection":7,"source_key":configured.key,"catalog_key":key,
                "scope":request.scope,"kind":kind,"generation":observed,"total":page.total,
                "first":first,"entries":page.addresses.into_iter().map(|address| json!({"address":address})).collect::<Vec<_>>(),
            }),
        ))
    };
    match read() {
        Ok(reply) | Err(reply) => reply,
    }
}

pub(super) fn anchor(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    let read = || -> Result<ViewerReply, ViewerReply> {
        let mut selected_query = Vec::new();
        let mut turn = None;
        for field in query.split('&') {
            if let Some(value) = field.strip_prefix("turn=") {
                if turn.is_some() {
                    return Err(invalid());
                }
                turn = Some(
                    crate::viewer::decoded(value)
                        .filter(|value| crate::session_route::identifier(value))
                        .ok_or_else(invalid)?,
                );
            } else {
                selected_query.push(field);
            }
        }
        let turn = turn.ok_or_else(invalid)?;
        let request = selected_request(configured, &selected_query.join("&"))?;
        let key = request.sid.as_deref().ok_or_else(invalid)?;
        if let Some(reply) = crate::index_rebuild::history_reply(&configured.options) {
            return Err(reply);
        }
        let path = crate::events::EventCache::path(&configured.options.cache);
        let namespace = crate::storage::Namespace::new(&configured.key)
            .map_err(index)
            .map_err(read_error)?;
        let source = Source::focused(namespace, &path, &configured.options, current_provider)
            .map_err(read_error)?;
        let component = match request.scope {
            CatalogReadScope::Current => super::selected::turn(&source, key, &turn),
            CatalogReadScope::RetainedHistory => {
                let archive = super::ArchiveReader::new(&configured.options);
                super::selected::retained_turn(
                    &source,
                    key,
                    &turn,
                    retained_provider.unwrap_or(&archive),
                )
                .map(Some)
            }
        }
        .map_err(read_error)?
        .ok_or_else(not_found)?;
        let address =
            super::selected::render::turn_address(&component, &configured.key, key, &turn);
        Ok(crate::catalog::reply(
            200,
            json!({
                "api":1,"projection":7,"source_key":configured.key,"catalog_key":key,
                "read_scope":request.scope,"turn":turn,"address":address,
            }),
        ))
    };
    match read() {
        Ok(reply) | Err(reply) => reply,
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
                | "peer"
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
    let address = argument("address")
        .map(NativeEntryAddress::parse)
        .transpose()
        .map_err(|_| invalid())?;
    if address.is_some() && (cursor.is_some() || argument("limit").is_some()) {
        return Err(invalid());
    }
    let peer = argument("peer")
        .map(crate::session_route::RetainedPeerAddress::parse)
        .transpose()
        .map_err(|_| invalid())?;
    if peer.is_some() && scope != CatalogReadScope::RetainedHistory {
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
    let component = match (scope, peer.as_ref()) {
        (CatalogReadScope::Current, _) => match &address {
            None => selected::latest_page(source.options, &source, key, limit, cursor.as_ref()),
            Some(address) => {
                selected::addressed_window(&source, key, &address.record, &address.binding)
            }
        },
        (CatalogReadScope::RetainedHistory, Some(peer)) => selected::retained_peer_window(
            &source,
            key,
            selected::PeerBinding {
                trace: &peer.trace,
                witness: &peer.witness,
            },
            limit,
            cursor.as_ref(),
            address
                .as_ref()
                .map(|address| (&address.record, address.binding.as_str())),
            retained_provider,
        )
        .map(Some),
        (CatalogReadScope::RetainedHistory, None) => {
            let archive = super::ArchiveReader::new(&configured.options);
            let provider = retained_provider.unwrap_or(&archive);
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
        return Err(not_found());
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
    let metadata = if let Some(row) = component.metadata {
        Some(list_item(row, None, &read_annotations(configured)?))
    } else if let Some(saved) = component.retained_metadata {
        Some(retained_item(
            saved,
            &component.identity,
            &read_annotations(configured)?,
        ))
    } else {
        None
    };
    Ok(crate::catalog::reply(
        200,
        json!({
            "api":1, "projection":7,
            "source_key":configured.key, "catalog_key":key, "read_scope":scope,
            "binding":component.binding, "earlier_cursor":earlier, "entries":entries,
            "identity":component.identity, "session":metadata,
            "position":if address.is_some() { "address" } else if cursor.is_some() { "earlier" } else { "latest" },
            "selected":{"harness":component.harness,"native_id":component.native_id,"title":component.title,"model":component.model},
        }),
    ))
}

fn read_annotations(
    configured: &OverviewSource,
) -> Result<crate::annotations::Annotations, ViewerReply> {
    crate::annotations::Annotations::read(&configured.options.annotations)
        .map_err(|err| read_error(index(err)))
}
