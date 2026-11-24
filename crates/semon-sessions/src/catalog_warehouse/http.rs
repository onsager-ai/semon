//! Viewer transport over source-qualified warehouse reads.
use super::selected::render::{Expansion, Part};
use super::{
    CatalogReadScope, Continuation, ReadError, Request, Row, Source, index, read_list, selected,
};
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
        ReadError::ActivityUnavailable => error(
            409,
            "activity_unavailable",
            "The activity evidence is outside the selected prefixes.",
            false,
        ),
        ReadError::SourceBudget => crate::transcript_range::source_budget_reply(),
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
    let mut machine = false;
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
            "machine" if !union && !machine => {
                if !crate::viewer::decoded(value)
                    .is_some_and(|key| crate::session_route::identifier(&key))
                {
                    return Err(invalid());
                }
                machine = true;
            }
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

pub(crate) fn validate_list(query: &str) -> Result<(), ViewerReply> {
    parse_list(query, false).map(|_| ())
}

pub(crate) fn note_list_search(query: &str, evidence: &crate::native_evidence::NativeEvidence) {
    if let Ok((request, _)) = parse_list(query, false)
        && request.scope == CatalogReadScope::Current
        && let Some(interest) = request.search_interest()
    {
        evidence.note_warehouse_search(interest);
    }
}

fn list_item(
    row: Row,
    label: Option<&str>,
    annotations: &crate::annotations::Annotations,
) -> Value {
    let tokens = row.token_millions();
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
    let title = if row.native_title || row.kind == "subagent" {
        row.title.as_str()
    } else {
        annotation
            .and_then(|annotation| annotation.title.as_deref())
            .unwrap_or(&row.title)
    };
    let mut item = json!({
        "lifecycle":"current",
        "key":row.position.key,"name":title,"harness":row.harness,
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
pub(crate) fn list(
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
    if !union
        && crate::viewer::query_value(query, "machine")
            .and_then(crate::viewer::decoded)
            .is_some_and(|key| configured.first().is_none_or(|source| source.key != key))
    {
        return Err(invalid());
    }
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
                "api": 1, "projection": 7, "machine": configured.key, "read_scope": request.scope, "generation": generation,
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
        CatalogReadScope::Current => {
            read_list(&sources, workspace, &request, cursor.as_ref(), union)
                .map_err(read_error)?
                .map(|row| {
                    let (label, annotations) = &annotations[row.position.source.as_str()];
                    list_item(row, union.then_some(*label), annotations)
                })
        }
        CatalogReadScope::RetainedHistory => {
            super::read_retained_list(&sources, workspace, &request, cursor.as_ref(), union)
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
            "api":1,"projection":7,"machine":machine,"read_scope":request.scope,"generation":page.generation,
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

fn selected_request(source_key: &str, query: &str) -> Result<Request, ViewerReply> {
    let mut arguments = std::collections::BTreeSet::new();
    for field in query.split('&') {
        let (name, value) = field.split_once('=').ok_or_else(invalid)?;
        if !matches!(name, "sid" | "scope" | "machine") || !arguments.insert(name) {
            return Err(invalid());
        }
        let value = crate::viewer::decoded(value).ok_or_else(invalid)?;
        if name == "machine" && value != source_key {
            return Err(invalid());
        }
    }
    let (request, _) = parse_list(query, false)?;
    if request.sid.is_none() {
        return Err(invalid());
    }
    Ok(request)
}

#[cfg(feature = "postgres")]
pub(crate) fn locate_request(
    source_key: &str,
    query: &str,
) -> Result<(String, CatalogReadScope), ViewerReply> {
    let request = selected_request(source_key, query)?;
    Ok((request.sid.expect("validated sid"), request.scope))
}

fn selected_read<T>(
    configured: &OverviewSource,
    query: &str,
    read: impl FnOnce(&Source<'_>, CatalogReadScope, &str) -> Result<T, ReadError>,
) -> Result<T, ViewerReply> {
    let request = selected_request(&configured.key, query)?;
    if let Some(reply) = crate::index_rebuild::history_reply(&configured.options) {
        return Err(reply);
    }
    let path = crate::events::EventCache::path(&configured.options.cache);
    let namespace = crate::storage::Namespace::new(&configured.key)
        .map_err(index)
        .map_err(read_error)?;
    let source =
        Source::focused(namespace, &path, &configured.options, None).map_err(read_error)?;
    read(&source, request.scope, &request.sid.expect("validated sid")).map_err(read_error)
}

fn selected_identity(
    configured: &OverviewSource,
    query: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, ViewerReply> {
    selected_read(configured, query, super::focused_identity)
}

pub(crate) fn identity(configured: &OverviewSource, query: &str) -> ViewerReply {
    match selected_identity(configured, query) {
        Ok(Some(identity)) => crate::catalog::reply(200, json!({"api":1,"identity":identity})),
        Ok(None) => not_found(),
        Err(reply) => reply,
    }
}

pub(crate) fn locate(configured: &OverviewSource, query: &str) -> ViewerReply {
    match selected_read(configured, query, super::focused_locate) {
        Ok(matches) => located(matches),
        Err(reply) => reply,
    }
}

pub(crate) fn located(matches: Vec<crate::session_route::Candidate>) -> ViewerReply {
    crate::catalog::reply(200, json!({"api":1,"matches":matches,"unavailable":0}))
}

pub(crate) fn observe(configured: &OverviewSource, query: &str) -> ViewerReply {
    match selected_request(&configured.key, query) {
        Ok(request) if request.scope == CatalogReadScope::RetainedHistory => (),
        Ok(_) => return invalid(),
        Err(reply) => return reply,
    }
    match selected_identity(configured, query) {
        Ok(Some(identity)) => crate::catalog::reply(
            200,
            json!({
                "api": 1, "source_key": configured.key, "catalog_key": identity.catalog_key, "state": "ready"
            }),
        ),
        Ok(None) => not_found(),
        Err(reply) => reply,
    }
}

pub(crate) fn resolve_native(configured: &OverviewSource, query: &str) -> ViewerReply {
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
            super::host_resolve_native(
                &configured.options,
                &configured.key,
                harness,
                native_id,
                scope,
            )
        },
    )
}

pub(crate) fn annotations(
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
        let request = selected_request(&configured.key, &selected_query.join("&"))?;
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
        let observed = if kind == "errors" {
            component.errors_generation()
        } else {
            component.identity.generation.clone()
        };
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

type AddressedQuery = (
    String,
    Option<NativeEntryAddress>,
    Option<crate::session_route::RetainedPeerAddress>,
);

fn addressed_query(query: &str) -> Result<AddressedQuery, ViewerReply> {
    let mut fields = Vec::new();
    let (mut address, mut peer) = (None, None);
    for field in query.split('&') {
        match field.split_once('=') {
            Some(("address", value)) if address.is_none() => {
                address = Some(
                    NativeEntryAddress::parse(&crate::viewer::decoded(value).ok_or_else(invalid)?)
                        .map_err(|_| invalid())?,
                );
            }
            Some(("peer", value)) if peer.is_none() => {
                peer = Some(
                    crate::session_route::RetainedPeerAddress::parse(
                        &crate::viewer::decoded(value).ok_or_else(invalid)?,
                    )
                    .map_err(|_| invalid())?,
                );
            }
            Some(("address" | "peer", _)) => return Err(invalid()),
            _ => fields.push(field),
        }
    }
    Ok((fields.join("&"), address, peer))
}

pub(crate) fn home(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    home_with_mode(
        configured,
        query,
        current_provider,
        retained_provider,
        crate::SessionSourceReadMode::LocalThenProvider,
    )
}

pub(crate) fn home_with_mode(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
) -> ViewerReply {
    let read = || -> Result<ViewerReply, ViewerReply> {
        let (fields, address, peer) = addressed_query(query)?;
        let (selection, scope, machine) =
            crate::overview::parse_session_home_scope(&fields, true).map_err(|_| invalid())?;
        if peer.is_some() && (address.is_none() || scope != CatalogReadScope::RetainedHistory) {
            return Err(invalid());
        }
        let key = selection.key.as_str();
        if !crate::session_route::identifier(key)
            || machine.is_some_and(|machine| machine != configured.key)
        {
            return Err(invalid());
        }
        if let Some(reply) = crate::index_rebuild::history_reply(&configured.options) {
            return Err(reply);
        }
        let path = crate::events::EventCache::path(&configured.options.cache);
        let namespace = crate::storage::Namespace::new(&configured.key)
            .map_err(index)
            .map_err(read_error)?;
        let source = Source::focused(namespace, &path, &configured.options, current_provider)
            .map_err(read_error)?
            .with_mode(mode);
        let requested = selection
            .turns
            .iter()
            .chain(selection.ids.iter().filter(|id| {
                crate::model::is_ask(id)
                    || selected::is_question_id(id)
                    || id.starts_with(['r', 's'])
            }))
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let home = selected::HomeFields {
            expected: &selection.expected,
            detail: selection
                .session_fields
                .iter()
                .any(|field| field == "detail"),
            activity: selection
                .session_fields
                .iter()
                .any(|field| field == "activity"),
        };
        if address.is_some()
            && (requested.len() != 1
                || !selection.session_fields.is_empty()
                || selection.ids.iter().any(|id| id != requested[0]))
        {
            return Err(invalid());
        }
        let component = match (scope, address.as_ref()) {
            (CatalogReadScope::Current, Some(address)) => {
                selected::addressed_preview(&source, key, address, home.expected)
            }
            (CatalogReadScope::Current, None) => {
                selected::preview_turns(&source, key, &requested, home)
            }
            (CatalogReadScope::RetainedHistory, Some(address)) => {
                selected::retained_addressed_preview(
                    &source,
                    key,
                    address,
                    home.expected,
                    peer.as_ref().map(|peer| selected::PeerBinding {
                        trace: &peer.trace,
                        witness: &peer.witness,
                    }),
                    retained_provider,
                )
                .map(Some)
            }
            (CatalogReadScope::RetainedHistory, None) => {
                let archive = super::ArchiveReader::new(&configured.options);
                selected::retained_preview_turns(
                    &source,
                    key,
                    &requested,
                    home,
                    retained_provider.unwrap_or(&archive),
                )
                .map(Some)
            }
        }
        .map_err(read_error)?
        .ok_or_else(not_found)?;
        if let Some(address) = &address
            && selected::render::turn_address(&component, &configured.key, key, requested[0])
                != Some(json!(address))
        {
            return Err(read_error(ReadError::Restart));
        }
        let previews = requested
            .into_iter()
            .filter_map(|id| {
                let mut candidates = component.records.iter().filter(|record| {
                    record
                        .causal
                        .turn
                        .as_ref()
                        .is_some_and(|turn| turn.id == id && turn.address == record.address)
                });
                let record = candidates.next()?;
                if candidates.next().is_some() {
                    return None;
                }
                let kinds: &[_] = if id.starts_with('r') {
                    &["received_cross", "received_agent", "received_down"]
                } else {
                    &["trace_prompt"]
                };
                kinds.iter().find_map(|kind| {
                    let text = crate::model::overview_text(
                        &json!({"kind":kind,"block":record.address.block}),
                        &record.native,
                        None,
                    );
                    text.is_string().then_some((id, text))
                })
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let turns = selection
            .turns
            .iter()
            .filter_map(|id| Some((id.clone(), previews.get(id.as_str())?.clone())))
            .collect::<serde_json::Map<_, _>>();
        let events = selection
            .ids
            .iter()
            .filter_map(|id| {
                if selected::is_question_id(id) {
                    let mut results = component.records.iter().filter_map(|record| {
                        let brief = selected::home_result(&record.causal, &record.reference, record.address.offset, &record.native, id)?;
                        Some((record, brief))
                    });
                    if let Some((record, brief)) = results.next() {
                        if results.next().is_some() {
                            return None;
                        }
                        return Some((id.clone(), json!({"brief":brief,"answer":null,"origin":component.home_origin(record)})));
                    }
                    let mut calls = component.records.iter().flat_map(|record| {
                        record.causal.calls.iter().filter_map(move |call| {
                            (selected::is_question_tool(&call.tool)
                                && crate::model::question_id(
                                    &record.reference.native_id,
                                    record.address.offset,
                                    Some(&call.id),
                                ) == *id)
                                .then_some((record, call))
                        })
                    });
                    let (record, call) = calls.next()?;
                    if calls.next().is_some() {
                        return None;
                    }
                    let brief = crate::model::overview_text(
                        &json!({"kind":"question","block":call.block}),
                        &record.native,
                        None,
                    );
                    let answer = if call.tool == "AskUserQuestion" {
                        call.replies
                            .iter()
                            .rev()
                            .find(|reply| reply.f & crate::events::ACK == 0)
                            .and_then(|reply| {
                                if reply.e || reply.f & crate::events::DENIED != 0 {
                                    return Some(json!([]));
                                }
                                let body = component
                                    .reply_records
                                    .get(&(record.address.path.clone(), reply.o))?;
                                Some(crate::model::overview_text(
                                    &json!({
                                        "kind":"answer","block":reply.b,"question_block":call.block,
                                        "structured":reply.f & crate::events::ANSWERED != 0,
                                    }),
                                    body,
                                    Some(&record.native),
                                ))
                            })
                    } else {
                        None
                    };
                    let origin = component.home_origin(record);
                    return Some((
                        id.clone(),
                        json!({"brief":brief,"answer":answer,"origin":origin}),
                    ));
                }
                let brief = previews.get(id.as_str())?;
                Some((
                    id.clone(),
                    json!({"brief":brief,"answer":null,"origin":null}),
                ))
            })
            .collect::<serde_json::Map<_, _>>();
        let detail = component.home_detail.as_ref();
        let wait = format!("wait:{}", json!([configured.key, key]));
        let incomplete = selection.turns.iter().any(|id| !turns.contains_key(id))
            || selection
                .ids
                .iter()
                .any(|id| *id != wait && !events.contains_key(id));
        Ok(crate::catalog::reply(
            200,
            json!({
                "api":1,"identity":component.identity,"generation":selection.expected,
                "session":{"detail":detail.map(|detail| &detail.text),"detail_from":detail.and_then(|detail| detail.from.as_deref()),"detail_from_name":detail.and_then(|detail| detail.from_name.as_deref()),"detail_from_reason":detail.and_then(|detail| detail.from_reason),"activity":component.home_activity},
                "events":events,"turns":turns,"incomplete":incomplete,
            }),
        ))
    };
    match read() {
        Ok(reply) | Err(reply) => reply,
    }
}

fn turn_request(
    configured: &OverviewSource,
    query: &str,
) -> Result<(Request, String), ViewerReply> {
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
    let request = selected_request(&configured.key, &selected_query.join("&"))?;
    Ok((request, turn))
}

pub(crate) fn anchor(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    let read = || -> Result<ViewerReply, ViewerReply> {
        let (request, turn) = turn_request(configured, query)?;
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

pub(crate) fn trace(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    let read = || -> Result<ViewerReply, ViewerReply> {
        let (query, address, peer) = addressed_query(query)?;
        let (request, turn) = turn_request(configured, &query)?;
        if peer.is_some() {
            return Err(invalid());
        }
        let key = request.sid.as_deref().ok_or_else(invalid)?;
        if let Some(reply) = crate::index_rebuild::history_reply(&configured.options) {
            return Err(reply);
        }
        let path = crate::events::EventCache::path(&configured.options.cache);
        let namespace = crate::storage::Namespace::new(&configured.key)
            .map_err(index)
            .map_err(read_error)?;
        let current =
            current_provider.map(|reader| crate::source_reader::BudgetedReader::new(reader, 1024));
        let current = current
            .as_ref()
            .map(|reader| reader as &dyn crate::SessionSourceReader);
        let source =
            Source::focused(namespace, &path, &configured.options, current).map_err(read_error)?;
        let value = super::trace::read(
            &source,
            request.scope,
            key,
            &turn,
            retained_provider,
            address.as_ref(),
        )
        .map_err(read_error)?
        .ok_or_else(not_found)?;
        Ok(crate::catalog::reply(200, value))
    };
    match read() {
        Ok(reply) | Err(reply) => reply,
    }
}

pub(crate) fn activity_anchor(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
) -> ViewerReply {
    let read = || -> Result<ViewerReply, ViewerReply> {
        if query.len() > 64 * 1024 {
            return Err(invalid());
        }
        let mut selected = Vec::new();
        let mut evidence = std::collections::BTreeMap::new();
        for field in query.split('&') {
            let (name, value) = field.split_once('=').ok_or_else(invalid)?;
            if matches!(name, "generation" | "ordinal" | "step") {
                let value = crate::viewer::decoded(value).ok_or_else(invalid)?;
                if evidence.insert(name, value).is_some() {
                    return Err(invalid());
                }
            } else {
                selected.push(field);
            }
        }
        let request = selected_request(&configured.key, &selected.join("&"))?;
        let key = request.sid.as_deref().ok_or_else(invalid)?;
        let field = |name| evidence.get(name).map(String::as_str).ok_or_else(invalid);
        let requested = crate::transcript_range::StepAnchor {
            generation: field("generation")?.to_owned(),
            ordinal: field("ordinal")?.parse().map_err(|_| invalid())?,
            id: serde_json::from_str(field("step")?).map_err(|_| invalid())?,
        };
        if requested.generation.is_empty()
            || requested.generation.len() > 4096
            || i64::try_from(requested.ordinal).is_err()
        {
            return Err(invalid());
        }
        let missing =
            |reason| crate::catalog::reply(200, json!({"available":false,"reason":reason}));
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
            CatalogReadScope::Current => super::selected::activity_anchor(&source, key, &requested),
            CatalogReadScope::RetainedHistory => {
                let archive = super::ArchiveReader::new(&configured.options);
                super::selected::retained_activity_anchor(
                    &source,
                    key,
                    &requested,
                    retained_provider.unwrap_or(&archive),
                )
            }
        }
        .map_err(read_error)?;
        let Some(component) = component else {
            return Ok(missing("evidence_unavailable"));
        };
        let Some(record) = component.records.first() else {
            return Ok(missing("evidence_unavailable"));
        };
        let mut entries = super::selected::render::entries(
            &component,
            &configured.key,
            key,
            crate::model::now_ms(),
        )
        .into_iter()
        .filter(|entry| match &record.entry_anchor {
            Some(id) => entry["entry_id"] == id.as_str(),
            None => match &requested.id {
                crate::events::session_activity::ActivityStepId::Claude(_) => {
                    entry["k"] == "a" && entry["address"]["block"] == 0
                }
                crate::events::session_activity::ActivityStepId::Codex(_) => entry["k"] == "tool",
            },
        });
        let Some(entry) = entries.next() else {
            return Ok(missing("entry_unavailable"));
        };
        if entries.next().is_some() {
            return Ok(missing("entry_ambiguous"));
        }
        Ok(crate::catalog::reply(
            200,
            json!({
                "available": true, "projection": 7, "source_key": configured.key,
                "catalog_key": key, "read_scope": request.scope,
                "activity_generation": requested.generation,
                "step": {"ordinal":requested.ordinal,"id":requested.id},
                "address": entry["address"],
            }),
        ))
    };
    match read() {
        Ok(reply) | Err(reply) => reply,
    }
}

pub(crate) fn transcript(
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
        crate::SessionSourceReadMode::LocalThenProvider,
        false,
    ) {
        Ok(reply) | Err(reply) => reply,
    }
}

pub(crate) fn entry(
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
        crate::SessionSourceReadMode::LocalThenProvider,
        true,
    ) {
        Ok(reply) | Err(reply) => reply,
    }
}

pub(crate) fn transcript_with_mode(
    configured: &OverviewSource,
    query: &str,
    provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    expanded: bool,
) -> ViewerReply {
    match read_transcript(configured, query, provider, provider, mode, expanded) {
        Ok(reply) | Err(reply) => reply,
    }
}

fn read_transcript(
    configured: &OverviewSource,
    query: &str,
    current_provider: Option<&dyn crate::SessionSourceReader>,
    retained_provider: Option<&dyn crate::SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    expanded: bool,
) -> Result<ViewerReply, ViewerReply> {
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
        (true, Some("attachment"), Some(image), None) => Some(Part::Attachment(
            image
                .parse::<usize>()
                .ok()
                .filter(|index| *index < 64)
                .ok_or_else(invalid)?,
        )),
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
        .map_err(read_error)?
        .with_mode(mode);
    let view = selected::ReadView::open_transcript(&source, part).map_err(read_error)?;
    let component = selected::transcript_window(
        &view,
        &source,
        scope,
        key,
        limit,
        cursor.as_ref(),
        address.as_ref(),
        retained_provider,
        peer.as_ref().map(|peer| selected::PeerBinding {
            trace: &peer.trace,
            witness: &peer.witness,
        }),
    )
    .map_err(read_error)?;
    let Some(component) = component else {
        return Err(not_found());
    };
    let mut entries =
        selected::render::entries(&component, &configured.key, key, crate::model::now_ms());
    if let (Some(address), Some(part)) = (&address, part) {
        if !entries
            .iter()
            .any(|entry| entry["address"] == json!(address))
        {
            return Err(error(
                404,
                "entry_not_found",
                "This native transcript entry is unavailable.",
                false,
            ));
        }
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
    let context = relationship_context(
        &view,
        &source,
        &component,
        key,
        &mut entries,
        peer.is_none(),
    )
    .map_err(read_error)?;
    if let Some(address) = &address
        && !entries
            .iter()
            .any(|entry| entry["address"] == json!(address))
    {
        return Err(error(
            404,
            "entry_not_found",
            "This native transcript entry is unavailable.",
            false,
        ));
    }
    let earlier = component
        .next
        .as_ref()
        .map(super::encode)
        .transpose()
        .map_err(read_error)?;
    let errors_generation = component.errors_generation();
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
            "errors_generation":errors_generation,
            "identity":component.identity, "session":metadata, "relationship_context":context,
            "position":if address.is_some() { "address" } else if cursor.is_some() { "earlier" } else { "latest" },
            "selected":{"harness":component.harness,"native_id":component.native_id,"title":component.title,"model":component.model},
        }),
    ))
}

fn relationship_context(
    view: &selected::ReadView<'_>,
    source: &Source<'_>,
    component: &selected::Component,
    key: &str,
    entries: &mut Vec<Value>,
    preview: bool,
) -> Result<Value, ReadError> {
    let turn_ids = entries
        .iter()
        .filter_map(|entry| entry["turn"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>();
    let actions = entries
        .iter()
        .filter(|entry| entry["k"] == "h")
        .filter_map(|entry| entry["id"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut turns = std::collections::BTreeMap::new();
    for record in &component.records {
        let Some(turn) = record
            .causal
            .turn
            .as_ref()
            .filter(|turn| turn_ids.contains(turn.id.as_str()))
        else {
            continue;
        };
        let boundary = turn.address == record.address;
        let row = turns.entry(turn.id.as_str());
        if boundary || matches!(&row, std::collections::btree_map::Entry::Vacant(_)) {
            let (end, last) = if boundary && preview {
                super::trace::recorded_end(view, source, component, record)?
            } else {
                (json!({"st":"off","why":"unavailable"}), false)
            };
            *row.or_insert(Value::Null) = json!({
                "id":turn.id,"sid":key,"start":crate::model::is_ask(&turn.id).then_some(&turn.id).filter(|id| actions.contains(id.as_str())),
                "u":!turn.handoff,"at":record.at.filter(|_| boundary),"end":end,"sent":[],"last":last,
            });
        }
    }
    let mut handoffs = component
        .records
        .iter()
        .filter_map(|record| {
            let id = record.prompt_anchor.as_deref()?;
            if !crate::model::is_ask(id) || !actions.contains(id) {
                return None;
            }
            Some(json!({
                "id":id,"kind":"ask","from":"you","to":key,"status":"done","declined":false,
                "at":record.at?,"target_turn":id,"brief_session":preview.then_some(key),
            }))
        })
        .take(200)
        .collect::<Vec<_>>();
    let missing = actions.len() != handoffs.len();
    let previews = if preview && !handoffs.is_empty() {
        json!({(key):&component.identity.generation})
    } else {
        json!({})
    };
    let mut cards = std::collections::BTreeMap::<_, Vec<_>>::new();
    let mut limited = false;
    for action in super::sender_actions::recorded(component, key) {
        let Some(turn) = turns.get_mut(action.turn) else {
            continue;
        };
        if handoffs.len() == 200 {
            limited = true;
            break;
        }
        turn["sent"]
            .as_array_mut()
            .expect("sender action list")
            .push(json!(action.id));
        handoffs.push(action.edge(key));
        cards
            .entry((
                action.address.path.as_str(),
                action.address.offset,
                action.address.occurrence,
                action.block,
            ))
            .or_default()
            .push(action.id);
    }
    *entries = std::mem::take(entries).into_iter().flat_map(|entry| {
        let mut rows = Vec::new();
        if entry["k"] == "tool"
            && let (Some(path), Some(offset), Some(occurrence), Some(block)) = (
                entry["address"]["record"]["path"].as_str(), entry["address"]["record"]["offset"].as_u64(),
                entry["address"]["record"]["occurrence"].as_i64(), entry["address"]["block"].as_u64().and_then(|block| u32::try_from(block).ok()),
            ) && let Some(ids) = cards.get(&(path, offset, occurrence, block)) {
            for id in ids {
                let entry_id = format!("{}:{id}", entry["entry_id"].as_str().expect("rendered entry id"));
                let mut address = entry["address"].clone();
                address["entry_id"] = json!(entry_id);
                rows.push(json!({"k":"h","id":id,"turn":entry["turn"],"entry_id":entry_id,"address":address,
                    "provenance":entry["provenance"],"observed_at":entry["observed_at"],"freshness":entry["freshness"]}));
            }
        }
        rows.push(entry);
        rows
    }).collect();
    Ok(json!({
        "state":if missing || limited {"incomplete"} else {"complete"},
        "turn_ids":turn_ids,"turns":turns.into_values().collect::<Vec<_>>(),"handoffs":handoffs,
        "sessions":{},"previews":previews,
    }))
}

fn read_annotations(
    configured: &OverviewSource,
) -> Result<crate::annotations::Annotations, ViewerReply> {
    crate::annotations::Annotations::read(&configured.options.annotations)
        .map_err(|err| read_error(index(err)))
}
