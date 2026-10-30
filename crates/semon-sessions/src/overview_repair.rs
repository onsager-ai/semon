//! Bounded migration of content-free publications from original native sources.
//! Uses the existing producer; never restores files or publishes current owners.
use crate::{
    Options, SessionSourceReadMode, SessionSourceReader, events::EventCache,
    model::summary::CatalogRow,
};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

const SOURCES_MAX: usize = 64;
const BYTES_MAX: u64 = 64 * 1024 * 1024;
const CANDIDATES: &str = "SELECT substr(key,17) FROM main.meta WHERE namespace=semon_namespace() AND key>'overview_needed:'||?2 AND key<'overview_needed;' ORDER BY key LIMIT ?1";
type EvidenceRead<'a> =
    Box<dyn Fn(&Path, &crate::native_record::Record) -> io::Result<Vec<u8>> + 'a>;
type NativeRead<'a> = Box<dyn Fn(&Path, u64) -> io::Result<Value> + 'a>;
type NativeBytes<'a> = Box<dyn Fn(&Path, u64) -> io::Result<Vec<u8>> + 'a>;

// One already-qualified window per admitted source, owned by this preparation.
// Preserve the rest of an 8 KiB read after a newline instead of asking the
// provider again for every short record or every field in that record.
#[derive(Default)]
struct RecordWindows(std::sync::Mutex<BTreeMap<(String, String), RecordWindow>>);
struct RecordWindow {
    source: crate::SessionSourceRef,
    part: crate::SessionSourceRange,
}
struct WindowReader<'a> {
    reader: &'a dyn SessionSourceReader,
    windows: &'a RecordWindows,
}
impl SessionSourceReader for WindowReader<'_> {
    fn read_range(
        &self,
        source: &crate::SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<crate::SessionSourceRange> {
        crate::selected_read::check()?;
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Native recipe window changed");
        if max == 0 || offset >= source.offset {
            return Err(invalid());
        }
        let key = (source.root.clone(), source.path.clone());
        let mut windows = self.windows.0.lock().map_err(|_| invalid())?;
        let prior = windows.get(&key);
        if let Some(prior) = prior {
            if prior.source != *source
                || expected.is_some_and(|value| value != prior.part.generation)
            {
                return Err(invalid());
            }
            if let Some(start) = offset.checked_sub(prior.part.offset)
                && let Ok(start) = usize::try_from(start)
                && start < prior.part.bytes.len()
            {
                let end = start + max.min(prior.part.bytes.len() - start);
                return Ok(crate::SessionSourceRange {
                    generation: prior.part.generation.clone(),
                    length: prior.part.length,
                    offset,
                    bytes: std::sync::Arc::from(&prior.part.bytes[start..end]),
                    cached: true,
                });
            }
        } else if windows.len() >= SOURCES_MAX {
            return Err(invalid());
        }
        let max = max
            .min(8 * 1024)
            .min((source.offset - offset).min(8192) as usize);
        let part = self.reader.read_range(
            source,
            prior
                .map(|prior| prior.part.generation.as_str())
                .or(expected),
            offset,
            max,
        )?;
        if part.offset != offset
            || part.bytes.is_empty()
            || part.bytes.len() > max
            || part.length < source.offset
            || part.generation.is_empty()
            || part.generation.len() > 4096
            || expected.is_some_and(|value| value != part.generation)
            || prior.is_some_and(|prior| {
                prior.part.generation != part.generation || prior.part.length != part.length
            })
        {
            return Err(invalid());
        }
        // Charge retained bytes before storing them. At most 64 * 8 KiB are
        // resident; replacements also share the unchanged preparation budget.
        crate::selected_read::charge(0, part.bytes.len())?;
        windows.insert(
            key,
            RecordWindow {
                source: source.clone(),
                part: part.clone(),
            },
        );
        Ok(part)
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static BEFORE_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(crate) struct NativeRecords<'a> {
    full: BTreeMap<PathBuf, Vec<u8>>,
    selected: Option<&'a EventCache>,
    read: Option<NativeRead<'a>>,
    raw: Option<NativeBytes<'a>>,
    evidence: Option<EvidenceRead<'a>>,
    error: std::cell::RefCell<Option<io::Error>>,
    fields: BTreeMap<(PathBuf, u64, u32), crate::native_field::NativeField>,
}
impl NativeRecords<'_> {
    pub(crate) fn bytes(&self, path: &Path) -> Option<&[u8]> {
        self.full.get(path).map(Vec::as_slice)
    }
    pub(crate) fn index(&self, path: &Path) -> Option<std::sync::Arc<crate::events::FileIndex>> {
        self.selected?.selected_index(path)
    }
    pub(crate) fn field(
        &self,
        path: &Path,
        offset: u64,
        block: u32,
    ) -> Option<crate::native_field::NativeField> {
        self.fields.get(&(path.to_owned(), offset, block)).cloned()
    }
    pub(crate) fn record_bytes(&self, path: &Path, offset: u64) -> Option<Vec<u8>> {
        if let Some(read) = &self.raw {
            return match read(path, offset) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    self.error.replace(Some(error));
                    None
                }
            };
        }
        let rest = self.bytes(path)?.get(usize::try_from(offset).ok()?..)?;
        let end = rest.iter().position(|byte| *byte == b'\n')? + 1;
        (end <= 32 * 1024 * 1024).then(|| rest[..end].to_vec())
    }
    pub(crate) fn record(&self, path: &Path, offset: u64) -> Option<Value> {
        if let Err(error) = crate::selected_read::check() {
            self.error.replace(Some(error));
            return None;
        }
        if let Some(read) = &self.read {
            match read(path, offset) {
                Ok(value) => return Some(value),
                Err(error) => {
                    self.error.replace(Some(error));
                    return None;
                }
            }
        }
        crate::tx::parse_native_record(&self.record_bytes(path, offset)?)
    }
    pub(crate) fn launch(
        &self,
        path: &Path,
        location: &crate::native_handoff::CallRef,
    ) -> io::Result<crate::handoff::Call> {
        crate::selected_read::check()?;
        location.read_with(|proof| {
            if let Some(read) = &self.evidence {
                return read(path, proof);
            }
            let end = proof
                .end()
                .ok_or_else(|| failure("Invalid launch evidence"))?;
            let bytes = self
                .bytes(path)
                .and_then(|bytes| {
                    bytes.get(usize::try_from(proof.offset).ok()?..usize::try_from(end).ok()?)
                })
                .ok_or_else(|| failure("Launch evidence unavailable"))?;
            proof.verify_bytes(bytes)?;
            Ok(bytes.to_vec())
        })
    }
    pub(crate) fn marker(&self, path: &Path) -> Option<crate::Marker> {
        if let Some(index) = self.index(path) {
            for event in &index.events {
                if matches!(event.k, crate::events::Kind::A) {
                    break;
                }
                if !matches!(event.k, crate::events::Kind::U) {
                    continue;
                }
                let value = self.record(path, event.o)?;
                if let Some(marker) = crate::user_text(&value).and_then(crate::parse_marker) {
                    return Some(marker);
                }
            }
            return None;
        }
        for line in self.bytes(path)?.split_inclusive(|byte| *byte == b'\n') {
            let Some(value) = crate::tx::parse_native_record(line) else {
                continue;
            };
            if crate::is_agent_output(&value) {
                return None;
            }
            if let Some(marker) = crate::user_text(&value).and_then(crate::parse_marker) {
                return Some(marker);
            }
        }
        None
    }
}
fn failure(error: impl std::fmt::Display) -> io::Error {
    crate::selected_read::check()
        .err()
        .unwrap_or_else(|| io::Error::other(error.to_string()))
}
/// A selected producer observation, using the same bounded migration as the
/// hosted native reader. It publishes metadata without restoring native files.
pub fn session_overview_observe(
    options: &Options,
    source_key: &str,
    query: &str,
) -> crate::ViewerReply {
    session_overview_observe_with_gate(options, source_key, query, || Ok(()))
}

pub(crate) fn session_overview_observe_with_gate(
    options: &Options,
    source_key: &str,
    query: &str,
    before_read: impl FnOnce() -> Result<(), crate::ViewerReply>,
) -> crate::ViewerReply {
    let key = match parse_session_overview_observe_query(query) {
        Ok(key) => key,
        Err(error) => return selected_observation_error(error),
    };
    if let Err(reply) = before_read() {
        return reply;
    }
    match repair_session_overview(
        options,
        source_key,
        &key,
        None,
        SessionSourceReadMode::LocalThenProvider,
    ) {
        Ok(_) => crate::ViewerReply {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: serde_json::json!({"api":1,"source_key":source_key,"catalog_key":key,"state":"ready"}).to_string().into_bytes(),
            etag: None,
        },
        Err(error) => selected_observation_error(error),
    }
}

fn parse_session_overview_observe_query(query: &str) -> io::Result<String> {
    let mut values = BTreeMap::new();
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid selected observation")
        })?;
        if !matches!(name, "sid" | "scope" | "machine")
            || values
                .insert(
                    name,
                    crate::viewer::decoded(value).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Invalid selected observation")
                    })?,
                )
                .is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid selected observation",
            ));
        }
    }
    if values.get("scope").map(String::as_str) != Some("retained_history") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Select retained history",
        ));
    }
    values
        .get("sid")
        .filter(|key| !key.is_empty() && key.len() <= 4096)
        .cloned()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Select one session"))
}

fn selected_observation_error(error: io::Error) -> crate::ViewerReply {
    crate::catalog::error(
        if error.kind() == io::ErrorKind::InvalidInput {
            400
        } else {
            503
        },
        "selected_observation",
        &error.to_string(),
        error.kind() != io::ErrorKind::InvalidInput,
    )
}
fn connection(options: &Options) -> io::Result<Connection> {
    let binding = crate::storage::local_navigation_binding(options, "").map_err(failure)?;
    let db = binding.connection().map_err(failure)?;
    crate::selected_read::install(&db)?;
    Ok(db)
}

fn transaction<'a>(
    db: &'a mut Connection,
    options: &Options,
) -> io::Result<rusqlite::Transaction<'a>> {
    let tx = db.transaction().map_err(failure)?;
    let binding = crate::storage::local_navigation_binding(options, "").map_err(failure)?;
    binding
        .validate(&tx, &binding.config().namespace)
        .map_err(failure)?;
    Ok(tx)
}

fn writer_connection(
    options: &Options,
) -> io::Result<(
    crate::events::store::configured::NativeOwnership,
    Connection,
)> {
    let config = options.source.config(&options.cache).map_err(failure)?;
    let owner =
        crate::events::store::configured::NativeOwnership::restore(&config).map_err(failure)?;
    let db = crate::storage::sqlite::source_journal::SourceJournal::owned(owner.clone())
        .connection()
        .map_err(failure)?;
    db.create_scalar_function(
        "semon_semantic_publisher",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(failure)?;
    crate::selected_read::install(&db)?;
    db.busy_timeout(Duration::ZERO).map_err(failure)?;
    Ok((owner, db))
}

fn validate_writer(
    tx: &rusqlite::Transaction<'_>,
    owner: &crate::events::store::configured::NativeOwnership,
) -> io::Result<()> {
    let binding = crate::storage::NativeReaderBinding::Semantic(owner.clone());
    binding
        .validate(tx, &owner.config().namespace)
        .map_err(failure)?;
    Ok(())
}

fn catalog(db: &Connection, key: &str) -> io::Result<Option<CatalogRow>> {
    let text = crate::history_projection::catalog(db, key).map_err(failure)?.or(db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key=?1",[key],|row|row.get::<_,String>(0)).optional().map_err(failure)?);
    text.map(|text| {
        crate::selected_read::charge(512, text.len())?;
        serde_json::from_str(&text).map_err(failure)
    })
    .transpose()
}
fn same_catalog_metadata(left: &CatalogRow, right: &CatalogRow) -> io::Result<bool> {
    Ok(serde_json::to_value(left).map_err(failure)? == serde_json::to_value(right).map_err(failure)?)
}
fn ready(db: &Connection, row: &CatalogRow) -> io::Result<bool> {
    let source = crate::retention::source_generation(&row.sources).map_err(failure)?;
    if !projection_ready(db, row)? {
        return Ok(false);
    }
    if row
        .query
        .as_ref()
        .is_none_or(|query| query["version"] != 1 || query["source_generation"] != source)
    {
        return Ok(false);
    }
    db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_overview o JOIN main.session_trace_headers t USING(namespace,session_key) WHERE o.namespace=semon_namespace() AND o.session_key=?1 AND json_extract(o.summary,'$.source_generation')=?2 AND t.source_generation=?2)",rusqlite::params![row.key,source],|row|row.get(0)).map_err(failure)
}

fn projection_ready(db: &Connection, row: &CatalogRow) -> io::Result<bool> {
    if crate::history_projection::catalog(db, &row.key)
        .map_err(failure)?
        .is_some()
    {
        return crate::history_projection::qualified(db, row).map_err(failure);
    }
    let source = crate::retention::source_generation(&row.sources).map_err(failure)?;
    db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_slot_projections WHERE namespace=semon_namespace() AND session_key=?1 AND version=?2 AND source_generation=?3)",rusqlite::params![row.key,crate::slot_projection::VERSION,source],|row|row.get(0)).map_err(failure)
}

/// Demand an ordinary producer upgrade for an exact selected owner. This only
/// schedules work; publication still requires the producer's source proofs.
pub fn request_session_projection_upgrade(options: &Options, key: &str) -> io::Result<bool> {
    options
        .source
        .config(&options.cache)
        .map_err(failure)?
        .request_projection_upgrade(options, key)
}

pub(crate) fn request_sqlite_projection_upgrade(options: &Options, key: &str) -> io::Result<bool> {
    let mut db = connection(options)?;
    let tx = transaction(&mut db, options)?;
    let Some(row) = catalog(&tx, key)? else {
        return Ok(false);
    };
    if projection_ready(&tx, &row)? {
        return Ok(false);
    }
    source_refs(options, std::slice::from_ref(&row))?;
    tx.commit().map_err(failure)?;
    crate::events::store::prioritize_projection_sources(options, &row.sources)?;
    Ok(true)
}

struct Selection {
    rows: Vec<CatalogRow>,
    incomplete: bool,
}
fn selection(db: &Connection, key: &str) -> io::Result<Selection> {
    let target = catalog(db, key)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Selected native query source unavailable",
        )
    })?;
    let mut paths: std::collections::BTreeSet<_> = target
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect();
    if paths.is_empty() || paths.len() > SOURCES_MAX {
        return Err(failure(
            "Selected native migration exceeds its source bound",
        ));
    }
    let parent = target.parent.clone();
    let mut selection = Selection {
        rows: vec![target],
        incomplete: false,
    };
    let mut add = |row: CatalogRow| {
        if selection.rows.iter().any(|prior| prior.key == row.key) {
            return;
        }
        let next: std::collections::BTreeSet<_> = paths
            .iter()
            .cloned()
            .chain(row.sources.iter().map(|source| source.path.clone()))
            .collect();
        if next.len() > SOURCES_MAX {
            selection.incomplete = true;
        } else {
            paths = next;
            selection.rows.push(row);
        }
    };
    if let Some(parent) = parent
        && let Some(row) = catalog(db, &parent)?
    {
        add(row);
    }
    let mut related=db.prepare("SELECT session_key FROM main.session_catalog WHERE namespace=semon_namespace() AND parent_key=?1 ORDER BY session_key LIMIT 65").map_err(failure)?;
    let mut count = 0;
    for sid in related
        .query_map([key], |row| row.get::<_, String>(0))
        .map_err(failure)?
    {
        let sid = sid.map_err(failure)?;
        count += 1;
        if let Some(row) = catalog(db, &sid)? {
            add(row);
        }
    }
    drop(related);
    selection.incomplete |= count == 65;
    Ok(selection)
}

fn query_selection(db: &Connection, key: &str) -> io::Result<Selection> {
    let mut selected = if catalog(db, key)?.is_some() {
        selection(db, key)?
    } else {
        // Far-end and unsent sessions have no native transcript. Resolve their
        // owner through indexed handoff endpoints, never a workspace scan.
        let mut statement = db.prepare("SELECT session_key FROM (SELECT session_key FROM main.session_trace_edges WHERE namespace=semon_namespace() AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?1 UNION SELECT session_key FROM main.session_trace_edges WHERE namespace=semon_namespace() AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?1 UNION SELECT session_key FROM main.session_trace_bookmarks WHERE namespace=semon_namespace() AND target=?2) ORDER BY session_key LIMIT 65").map_err(failure)?;
        let owners = statement
            .query_map(rusqlite::params![key, key.strip_prefix("unsent:")], |row| {
                row.get::<_, String>(0)
            })
            .map_err(failure)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failure)?;
        let mut rows = BTreeMap::new();
        let mut incomplete = owners.len() == 65;
        for owner in owners.into_iter().take(64) {
            let context = selection(db, &owner)?;
            incomplete |= context.incomplete;
            for row in context.rows {
                rows.insert(row.key.clone(), row);
            }
            let paths: std::collections::BTreeSet<_> = rows
                .values()
                .flat_map(|row| &row.sources)
                .map(|source| &source.path)
                .collect();
            if paths.len() > SOURCES_MAX || rows.len() > 64 {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Selected handoff owner closure exceeds its source bound",
                ));
            }
        }
        if rows.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Selected native query evidence is unavailable",
            ));
        }
        Selection {
            rows: rows.into_values().collect(),
            incomplete,
        }
    };
    // Native derivation needs a deterministic closure, including the parents
    // and endpoints discovered on a newly admitted relative. One pass over the
    // original rows can omit a grandparent or a second-hop handoff owner and
    // silently derive a different identity from the same selected source.
    let mut pending: std::collections::BTreeSet<_> =
        selected.rows.iter().map(|row| row.key.clone()).collect();
    let mut visited = std::collections::BTreeSet::new();
    let mut related = db.prepare("SELECT endpoint FROM (SELECT parent_key AS endpoint FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key=?1 UNION SELECT session_key AS endpoint FROM main.session_catalog WHERE namespace=semon_namespace() AND parent_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END AS endpoint FROM main.session_trace_edges WHERE namespace=semon_namespace() AND session_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END AS endpoint FROM main.session_trace_edges WHERE namespace=semon_namespace() AND session_key=?1) WHERE endpoint IS NOT NULL ORDER BY endpoint LIMIT 65").map_err(failure)?;
    let mut aliases = db.prepare("SELECT DISTINCT session_key FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND native_id=?1 ORDER BY session_key LIMIT 65").map_err(failure)?;
    while let Some(owner) = pending.pop_first() {
        if !visited.insert(owner.clone()) {
            continue;
        }
        let mut endpoints: std::collections::BTreeSet<_> = related
            .query_map([&owner], |row| row.get::<_, String>(0))
            .map_err(failure)?
            .collect::<Result<_, _>>()
            .map_err(failure)?;
        selected.incomplete |= endpoints.len() == 65;
        let row = selected
            .rows
            .iter()
            .find(|row| row.key == owner)
            .expect("selected owner is present");
        let mut names: std::collections::BTreeSet<_> = row.native_ids.iter().cloned().collect();
        if !row.native_ids.iter().any(|id| id == &row.key) {
            // Native keys gain a harness qualifier on collisions, then leading
            // colons while a literal native ID occupies that spelling. Those
            // occupants are identity dependencies even without a handoff edge.
            let mut spelling = row.key.as_str();
            names.insert(spelling.to_owned());
            while let Some(next) = spelling.strip_prefix(':') {
                names.insert(next.to_owned());
                spelling = next;
                if names.len() > 128 {
                    break;
                }
            }
        }
        if names.len() > 128 {
            selected.incomplete = true;
            continue;
        }
        for name in names {
            let matches = aliases
                .query_map([name], |row| row.get::<_, String>(0))
                .map_err(failure)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?;
            selected.incomplete |= matches.len() == 65;
            endpoints.extend(matches.into_iter().take(64));
            if endpoints.len() > 64 {
                selected.incomplete = true;
                break;
            }
        }
        for endpoint in endpoints.into_iter().take(64) {
            if selected.rows.iter().any(|row| row.key == endpoint) {
                continue;
            }
            // Far-end stubs have no source owner. The native producer derives
            // them from the admitted owner's recorded handoff evidence.
            let Some(row) = catalog(db, &endpoint)? else {
                continue;
            };
            let paths: std::collections::BTreeSet<_> = selected
                .rows
                .iter()
                .chain(std::iter::once(&row))
                .flat_map(|row| &row.sources)
                .map(|source| &source.path)
                .collect();
            if paths.len() > SOURCES_MAX || selected.rows.len() >= 64 {
                selected.incomplete = true;
                continue;
            }
            pending.insert(row.key.clone());
            selected.rows.push(row);
        }
    }
    // Keep the requested owner first; every context row has stable ordering
    // independent of SQLite iteration, append timing or producer row order.
    if selected.rows.len() > 1 {
        selected.rows[1..].sort_by(|a, b| a.key.cmp(&b.key));
    }
    Ok(selected)
}

/// Server-selected original references for one producer migration. The host
/// checks machine authority and binds these references to its private custody.
pub fn overview_repair_sources(
    options: &Options,
    key: &str,
) -> io::Result<Option<Vec<crate::SessionSourceRef>>> {
    let mut db = connection(options)?;
    let tx = transaction(&mut db, options)?;
    let rows = selection(&tx, key)?.rows;
    if ready(&tx, &rows[0])? {
        return Ok(None);
    }
    Ok(Some(source_refs(options, &rows)?))
}

/// Original native references for one focused Query and its bounded
/// relationship context. References carry evidence, never source authority.
pub fn session_query_sources(
    options: &Options,
    key: &str,
) -> io::Result<Vec<crate::SessionSourceRef>> {
    let mut db = connection(options)?;
    let tx = transaction(&mut db, options)?;
    let selected = query_selection(&tx, key)?;
    if selected.incomplete {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Selected query relationship closure exceeds its source bound",
        ));
    }
    source_refs(options, &selected.rows)
}

fn local_query_selection(db: &Connection, key: &str) -> io::Result<Option<(String, Selection)>> {
    // A provisional discovery cannot prove a global native identity. In
    // particular, an unobserved harness may introduce a canonical-ID collision.
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_completeness' AND value='complete')",
        [],
        |row| row.get::<_, bool>(0),
    ).map_err(failure)? {
        return Ok(None);
    }
    let (key, selected) = match query_selection(db, key) {
        Ok(selected) => (key.to_owned(), selected),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A complete Catalog's namespace can differ from the CLI's file
            // window. Resolve original native IDs through the existing index;
            // query_selection then includes every identity collision owner.
            let mut aliases = db.prepare("SELECT DISTINCT session_key FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND native_id=?1 ORDER BY session_key LIMIT 65").map_err(failure)?;
            let owners = aliases
                .query_map([key], |row| row.get::<_, String>(0))
                .map_err(failure)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?;
            if owners.len() > 64 {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Native query identity closure exceeds its source bound",
                ));
            }
            let Some(owner) = owners.into_iter().next() else {
                return Ok(None);
            };
            let selected = query_selection(db, &owner)?;
            (owner, selected)
        }
        Err(error) => return Err(error),
    };
    if selected.incomplete {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Selected query relationship closure exceeds its source bound",
        ));
    }
    Ok(Some((key, selected)))
}

pub(crate) fn query_context(db: &Connection, key: &str) -> io::Result<Vec<CatalogRow>> {
    let selected = query_selection(db, key)?;
    if selected.incomplete {
        return Err(failure(
            "Selected query relationship closure exceeds its source bound",
        ));
    }
    Ok(selected.rows)
}

/// Whether the existing bounded identity/relationship closure survives a
/// physical file window unchanged. A provisional Catalog cannot prove this.
pub(crate) fn query_window_context_current(
    db: &Connection,
    options: &Options,
    key: &str,
    now: i64,
) -> io::Result<bool> {
    if !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_completeness' AND value='complete')",
        [],
        |row| row.get::<_, bool>(0),
    ).map_err(failure)? {
        return Ok(false);
    }
    let selected = query_selection(db, key)?;
    if selected.incomplete {
        return Ok(false);
    }
    let mut sources = BTreeMap::new();
    for source in selected.rows.iter().flat_map(|row| &row.sources) {
        if let Some(prior) = sources.insert(source.path.clone(), source.clone())
            && prior != *source
        {
            return Err(failure("Conflicting native query source"));
        }
    }
    let cutoff = crate::model::scan_cutoff(options, now);
    let sources: Vec<_> = sources.into_values().collect();
    if sources.iter().any(|source| {
        !crate::model::catalog_source_in_window(source, cutoff)
            || crate::catalog::source_state(source) != "cached"
    }) {
        return Ok(false);
    }
    Ok(EventCache::selected_roles_current(db, &sources).unwrap_or(false))
}

/// Existing current paths for a bounded selected-source producer refresh.
/// Retained/missing sources keep their archive or disappearance semantics.
pub(crate) fn local_query_inputs(
    options: &Options,
    key: &str,
) -> io::Result<Option<Vec<crate::inputs::Input>>> {
    if !EventCache::path(&options.cache).is_file() {
        return Ok(None);
    }
    let mut db = connection(options)?;
    let tx = transaction(&mut db, options)?;
    let Some((_, selected)) = local_query_selection(&tx, key)? else {
        return Ok(None);
    };
    let mut inputs = std::collections::BTreeSet::new();
    for row in selected.rows {
        if row.lifecycle != crate::model::summary::CatalogLifecycle::Current {
            return Ok(None);
        }
        for source in row.sources {
            if !std::fs::symlink_metadata(&source.path).is_ok_and(|metadata| metadata.is_file()) {
                return Ok(None);
            }
            let Some(input) = crate::native_evidence::catalog_input(&source.path, options) else {
                return Ok(None);
            };
            inputs.insert(input);
        }
    }
    Ok((!inputs.is_empty()).then(|| inputs.into_iter().collect()))
}

/// Qualify an ordinary local focused read against existing native evidence.
/// Cold/stale publications still require their native producer. This never
/// discovers unrelated sources or changes source/catalog ownership.
pub(crate) fn local_query_target(
    options: &Options,
    key: &str,
    now: i64,
) -> io::Result<Option<String>> {
    if !EventCache::path(&options.cache).is_file() {
        return Ok(None);
    }
    let mut db = connection(options)?;
    let tx = transaction(&mut db, options)?;
    let Some((key, selected)) = local_query_selection(&tx, key)? else {
        return Ok(None);
    };
    let cutoff = crate::model::scan_cutoff(options, now);
    let mut sources = BTreeMap::new();
    for source in selected.rows.iter().flat_map(|row| &row.sources) {
        if crate::catalog::source_state(source) != "cached" {
            return Ok(None);
        }
        if !crate::model::catalog_source_in_window(source, cutoff) {
            continue;
        }
        if let Some(prior) = sources.insert(source.path.clone(), source.clone())
            && prior != *source
        {
            return Err(failure("Conflicting native query source"));
        }
    }
    let sources: Vec<_> = sources.into_values().collect();
    if sources.is_empty() {
        return Ok(Some(key));
    }
    // Sidecars are independently generation-bound native evidence. A changed
    // sidecar must not lend an old role/name merely because its log is stable.
    let ready = EventCache::selected_roles_current(&tx, &sources).unwrap_or(false);
    Ok(ready.then_some(key))
}

pub(crate) fn source_refs(
    options: &Options,
    rows: &[CatalogRow],
) -> io::Result<Vec<crate::SessionSourceRef>> {
    let mut selected = BTreeMap::new();
    for row in rows {
        let home = match row.harness.as_str() {
            "claude" => &options.claude_home,
            "codex" => &options.codex_home,
            "copilot" => &options.copilot_home,
            _ => return Err(failure("Unsupported producer source")),
        };
        for source in &row.sources {
            let relative = source
                .path
                .strip_prefix(home)
                .ok()
                .and_then(|path| path.to_str())
                .filter(|path| crate::is_input_path(&row.harness, path))
                .ok_or_else(|| failure("Native producer source is outside this machine"))?;
            let reference = crate::SessionSourceRef {
                root: row.harness.clone(),
                path: relative.into(),
                native_id: source.native_id.clone(),
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
            };
            if let Some(prior) = selected.insert(source.path.clone(), reference.clone())
                && prior != reference
            {
                return Err(failure("Conflicting native migration source"));
            }
        }
    }
    if selected.is_empty() || selected.len() > SOURCES_MAX {
        return Err(failure("Native migration source bound exceeded"));
    }
    Ok(selected.into_values().collect())
}

/// Cheap migration candidates. A host authorizes the machine and supplies its
/// request-owned provider; identifiers do not grant native or archive access.
pub fn overview_repair_candidates(options: &Options, limit: usize) -> io::Result<Vec<String>> {
    candidates_after(options, limit, "")
}
pub(crate) fn candidates_after(
    options: &Options,
    limit: usize,
    after: &str,
) -> io::Result<Vec<String>> {
    if after.len() > 4096 || limit == 0 || limit > 8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid overview migration batch",
        ));
    }
    let mut connection = connection(options)?;
    let db = transaction(&mut connection, options)?;
    // Current-format selected candidates share this admitted read snapshot.
    let mut statement = db.prepare(CANDIDATES).map_err(failure)?;
    statement
        .query_map(rusqlite::params![limit as i64, after], |row| {
            row.get::<_, String>(0)
        })
        .map_err(failure)?
        .map(|row| row.map_err(failure))
        .collect()
}

/// Complete one selected original source closure under a catalog generation
/// CAS. Bounds: 64 sources, 64 MiB native input, 1,024 provider requests. Bodies
/// die with this call; only derived Home/Trace and transcript recipes change.
/// The embedding host owns publication/custody fences and authorization checks.
pub fn repair_session_overview(
    options: &Options,
    source_key: &str,
    key: &str,
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
) -> io::Result<bool> {
    repair_session_overview_with_control(
        options,
        source_key,
        key,
        provider,
        mode,
        &crate::SelectedReadControl::default(),
    )
}

/// Selected repair using the host's request-owned deadline/cancellation.
pub fn repair_session_overview_with_control(
    options: &Options,
    source_key: &str,
    key: &str,
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
    control: &crate::SelectedReadControl,
) -> io::Result<bool> {
    let _scope = crate::selected_read::Scope::enter(control);
    crate::selected_read::check()?;
    if source_key.is_empty() || source_key.len() > 4096 || key.is_empty() || key.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid migration selection",
        ));
    }
    let mut db = connection(options)?;
    let tx = transaction(&mut db, options)?;
    let selection = selection(&tx, key)?;
    let rows = selection.rows;
    if ready(&tx, &rows[0])? {
        tx.commit().map_err(failure)?;
        let (owner, mut writable) = writer_connection(options)?;
        let tx = writable
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(failure)?;
        validate_writer(&tx, &owner)?;
        let Some(current) = catalog(&tx, key)? else {
            return Err(publication_changed());
        };
        if !ready(&tx, &current)? {
            return Err(publication_changed());
        }
        tx.execute(
            "DELETE FROM main.meta WHERE namespace=semon_namespace() AND key=?1",
            [format!("overview_needed:{key}")],
        )
        .map_err(failure)?;
        crate::selected_read::checkpoint("selected_publication")?;
        tx.commit().map_err(failure)?;
        return Ok(false);
    }
    let selected_preimage = rows
        .iter()
        .find(|row| row.key == key)
        .cloned()
        .ok_or_else(|| failure("Selected native migration owner unavailable"))?;
    let reader_binding = crate::storage::local_navigation_binding(options, source_key)
        .map_err(failure)?;
    let preimage_binding = reader_binding.runtime_binding_at(&tx).map_err(failure)?;
    crate::storage::sqlite::proof_generations::observation_binding(
        &tx,
        &preimage_binding,
        &selected_preimage,
    )
    .map_err(failure)?;
    let upgrade = !projection_ready(&tx, &rows[0])?;
    let generation: String = tx
        .query_row(
            "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
            [],
            |row| row.get(0),
        )
        .map_err(failure)?;
    let original: BTreeMap<_, _> = rows
        .iter()
        .map(|row| (row.key.clone(), row.sources.clone()))
        .collect();
    let mut producer_options = options.clone();
    producer_options.all = true;
    producer_options.scan_window = false;
    let prepared = with_records(options, &rows, provider, mode, tx, |native| {
        crate::model::prepare_history(&producer_options, &rows, native, crate::model::now_ms())
    })?;
    let transcripts = prepared.transcripts;
    let mut repaired = Vec::new();
    // Related inputs qualify the selected owner's relationships. They cannot
    // replace another owner's publication with this selected, partial closure.
    for mut row in prepared.rows.into_iter().filter(|row| row.key == key) {
        let Some(sources) = original.get(&row.key) else {
            continue;
        };
        let paths: std::collections::BTreeSet<_> = row
            .sources
            .iter()
            .map(|source| (&source.path, &source.native_id))
            .collect();
        if paths
            != sources
                .iter()
                .map(|source| (&source.path, &source.native_id))
                .collect()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Native migration identity changed",
            ));
        }
        row.sources = sources.clone();
        if let Some(query) = row.query.as_mut() {
            query["source_generation"] =
                serde_json::json!(crate::retention::source_generation(sources).map_err(failure)?);
        }
        if selection.incomplete {
            row.query = None;
        }
        if selection.incomplete
            && let Some(trace) = row.trace.as_mut()
        {
            trace.relationships_incomplete();
        }
        row.overview = row
            .overview
            .take()
            .map(|summary| summary.with_sources(sources))
            .transpose()?;
        repaired.push(row);
    }
    if !repaired.iter().any(|row| row.key == key) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Native migration owner unavailable",
        ));
    }
    crate::selected_read::check()?;
    #[cfg(test)]
    if let Some(hook) = BEFORE_PUBLICATION.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let (owner, mut writable) = writer_connection(options)?;
    let tx = writable
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(failure)?;
    validate_writer(&tx, &owner)?;
    let stream = owner
        .config()
        .expected_stream_epoch
        .as_deref()
        .ok_or_else(|| failure(crate::storage::StorageError::ScopeChanged))?;
    let writer_binding = crate::storage::QueryRuntimeBinding {
        namespace: owner.config().namespace.clone(),
        admission: owner.admission(),
        stream_epoch: stream.to_owned(),
    };
    if writer_binding.namespace != preimage_binding.namespace
        || writer_binding.admission != preimage_binding.admission
        || writer_binding.stream_epoch != preimage_binding.stream_epoch
    {
        return Err(publication_changed());
    }
    let current: String = tx
        .query_row(
            "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
            [],
            |row| row.get(0),
        )
        .map_err(failure)?;
    if current != generation {
        return Err(publication_changed());
    }
    for row in &repaired {
        let Some(current) = catalog(&tx, &row.key)? else {
            return Err(publication_changed());
        };
        if current.sources != row.sources
            || !same_catalog_metadata(&current, &selected_preimage)?
        {
            return Err(publication_changed());
        }
    }
    let trace_store = if repaired.iter().any(|row| row.trace.is_some()) {
        Some(
            crate::storage::sqlite::trace::TraceWriter::owned(&tx, &owner)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
                .map_err(failure)?,
        )
    } else {
        None
    };
    for row in &repaired {
        crate::selected_read::check()?;
        if let Some(summary) = &row.overview {
            crate::overview::publish(&tx, &row.key, row.last.unwrap_or(0), summary)
                .map_err(failure)?;
        }
        if let Some(trace) = &row.trace {
            let trace_store = trace_store
                .as_ref()
                .ok_or(rusqlite::Error::InvalidQuery)
                .map_err(failure)?;
            crate::causal_trace::publish_owned(&tx, trace_store, row, trace).map_err(failure)?;
        }
        if let Some(query) = &row.query {
            let owner = if crate::history_projection::catalog(&tx, &row.key)
                .map_err(failure)?
                .is_some()
            {
                crate::storage::sqlite::proof_generations::CatalogObservationOwner::History
            } else {
                crate::storage::sqlite::proof_generations::CatalogObservationOwner::Catalog
            };
            crate::storage::sqlite::proof_generations::publish_repaired_query_observation(
                &tx,
                &writer_binding,
                owner,
                &selected_preimage,
                query,
            )
            .map_err(failure)?;
        }
        if upgrade {
            let projection = transcripts
                .iter()
                .find(|projection| projection.key == row.key)
                .ok_or_else(|| failure("Selected transcript recipes unavailable"))?;
            if selection.incomplete {
                return Err(failure("Selected transcript closure incomplete"));
            }
            publish_projection(&tx, row, projection)?;
        }
        tx.execute(
            "DELETE FROM main.meta WHERE namespace=semon_namespace() AND key=?1",
            [format!("overview_needed:{}", row.key)],
        )
        .map_err(failure)?;
    }
    if upgrade && !ready(&tx, &repaired[0])? {
        return Err(failure("Selected transcript is not ready"));
    }
    crate::selected_read::checkpoint("selected_publication")?;
    tx.commit().map_err(failure)?;
    Ok(true)
}

fn publish_projection(
    tx: &rusqlite::Transaction<'_>,
    row: &CatalogRow,
    projection: &crate::slot_projection::Projection,
) -> io::Result<()> {
    let texts = projection
        .slots
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(failure)?;
    let source = crate::retention::source_generation(&row.sources).map_err(failure)?;
    let generation = crate::slot_projection::generation(&texts, &row.sources).map_err(failure)?;
    let history = crate::history_projection::catalog(tx, &row.key)
        .map_err(failure)?
        .is_some();
    let slots = if history {
        "session_history_slots"
    } else {
        "session_slots"
    };
    if history {
        tx.execute("INSERT OR REPLACE INTO main.session_history_projections(namespace,session_key,generation,source_generation,version,total,incomplete) VALUES(semon_namespace(),?1,?2,?3,?4,?5,0)",rusqlite::params![row.key,generation,source,crate::slot_projection::VERSION,texts.len() as i64]).map_err(failure)?;
    } else {
        tx.execute("INSERT OR REPLACE INTO main.session_slot_projections(namespace,session_key,generation,source_generation,version,total) VALUES(semon_namespace(),?1,?2,?3,?4,?5)",rusqlite::params![row.key,generation,source,crate::slot_projection::VERSION,texts.len() as i64]).map_err(failure)?;
    }
    tx.execute(
        &format!("DELETE FROM main.{slots} WHERE namespace=semon_namespace() AND session_key=?1"),
        [&row.key],
    )
    .map_err(failure)?;
    let mut put = tx
        .prepare(&format!(
            "INSERT INTO main.{slots}(namespace,session_key,slot,metadata) VALUES(semon_namespace(),?1,?2,?3)"
        ))
        .map_err(failure)?;
    for (slot, text) in texts.iter().enumerate() {
        crate::selected_read::check()?;
        put.execute(rusqlite::params![row.key, slot as i64, text])
            .map_err(failure)?;
    }
    Ok(())
}

fn publication_changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "Selected native publication changed; retry observation",
    )
}

fn with_records<T>(
    options: &Options,
    rows: &[CatalogRow],
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
    tx: rusqlite::Transaction<'_>,
    derive: impl FnOnce(&NativeRecords<'_>) -> io::Result<T>,
) -> io::Result<T> {
    crate::selected_read::check()?;
    let mut unique = BTreeMap::new();
    for source in rows.iter().flat_map(|row| &row.sources) {
        if let Some(prior) = unique.insert(source.path.clone(), source.clone())
            && prior != *source
        {
            return Err(failure("Conflicting native migration source"));
        }
    }
    let sources: Vec<_> = unique.into_values().collect();
    if sources.len() > SOURCES_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Selected native migration exceeds its source bound",
        ));
    }
    if sources.is_empty() {
        tx.commit().map_err(failure)?;
        return derive(&NativeRecords {
            full: BTreeMap::new(),
            selected: None,
            read: None,
            raw: None,
            evidence: None,
            error: std::cell::RefCell::new(None),
            fields: BTreeMap::new(),
        });
    }
    // Prefer the exact original event ledgers. Large native bodies need not be
    // copied merely to derive Home/Trace metadata from an existing native index.
    let ids = rows
        .iter()
        .flat_map(|row| row.native_ids.iter().cloned())
        .collect();
    let selected = match EventCache::from_selected(&tx, &sources, &ids) {
        Ok(selected) => Some(selected),
        Err(error) => {
            // A terminal request failure must never switch to body replay.
            crate::selected_read::check()?;
            if matches!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::TooBig | rusqlite::ErrorCode::OperationInterrupted)
            ) {
                return Err(crate::selected_read::limit_error());
            }
            match error {
                rusqlite::Error::InvalidQuery
                | rusqlite::Error::FromSqlConversionFailure(..)
                | rusqlite::Error::InvalidColumnType(..)
                | rusqlite::Error::QueryReturnedNoRows => None,
                error => return Err(failure(error)),
            }
        }
    };
    let mut fields = BTreeMap::new();
    if selected.is_some() {
        let mut field_bytes = 0;
        for row in rows {
            let source = crate::retention::source_generation(&row.sources).map_err(failure)?;
            let projection_version = crate::slot_projection::VERSION;
            let history = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.session_history_projections WHERE namespace=semon_namespace() AND session_key=?1 AND source_generation=?2 AND version=?3)",rusqlite::params![row.key,source,projection_version],|r|r.get::<_,bool>(0)).map_err(failure)?;
            let current = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.session_slot_projections WHERE namespace=semon_namespace() AND session_key=?1 AND source_generation=?2 AND version=?3)",rusqlite::params![row.key,source,projection_version],|r|r.get::<_,bool>(0)).map_err(failure)?;
            if !history && !current {
                continue;
            }
            let table = if history {
                "session_history_slots"
            } else {
                "session_slots"
            };
            let mut statement=tx.prepare(&format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM main.{table} WHERE namespace=semon_namespace() AND session_key=?1 AND json_extract(metadata,'$.field') IS NOT NULL ORDER BY slot LIMIT 4096")).map_err(failure)?;
            for text in statement
                .query_map([&row.key], |r| {
                    crate::selected_read::row(r, 512)?;
                    r.get::<_, Option<String>>(0)
                })
                .map_err(failure)?
            {
                let Some(text) = text.map_err(failure)? else {
                    continue;
                };
                field_bytes += text.len();
                if fields.len() >= 4096 || field_bytes > 4 * 1024 * 1024 {
                    break;
                }
                let slot: crate::slot_projection::SavedSlot =
                    serde_json::from_str(&text).map_err(failure)?;
                if let (Some(file), Some(field)) = (slot.file, slot.field) {
                    fields.insert((file.path, slot.offset, slot.block), field);
                }
            }
        }
    }
    tx.commit().map_err(failure)?;
    let bytes_left = std::cell::RefCell::new(BYTES_MAX as usize);
    let requests = std::cell::Cell::new(1024usize);
    let windows = RecordWindows::default();
    let memo = std::cell::RefCell::new(BTreeMap::<(PathBuf, u64), Value>::new());
    let raw = |path: &Path, offset: u64| -> io::Result<Vec<u8>> {
        let row = rows
            .iter()
            .find(|row| row.sources.iter().any(|source| source.path == path))
            .ok_or_else(|| failure("Unqualified native recipe source"))?;
        crate::transcript_range::with_source_reader(
            options,
            &row.harness,
            &row.sources,
            provider,
            mode,
            |reader, refs| {
                let reader = crate::source_reader::BudgetedReader::new(reader, requests.get());
                let bytes = crate::source_reader::read_record_bytes_bounded(
                    &WindowReader {
                        reader: &reader,
                        windows: &windows,
                    },
                    refs.get(path)
                        .ok_or_else(|| failure("Unknown recipe source"))?,
                    offset,
                    32 * 1024 * 1024,
                    &mut bytes_left.borrow_mut(),
                );
                requests.set(reader.remaining());
                let bytes = bytes?;
                crate::selected_read::charge(0, bytes.len())?;
                Ok(bytes)
            },
        )
    };
    let read = |path: &Path, offset: u64| -> io::Result<Value> {
        let key = (path.to_owned(), offset);
        if let Some(value) = memo.borrow().get(&key) {
            return Ok(value.clone());
        }
        let bytes = raw(path, offset)?;
        let value = crate::tx::parse_native_record(&bytes)
            .ok_or_else(|| failure("Native producer record is malformed"))?;
        if bytes.len() <= 1048576 {
            let mut memo = memo.borrow_mut();
            if memo.len() >= 16 {
                memo.clear();
            }
            memo.insert(key, value.clone());
        }
        Ok(value)
    };
    let evidence = |path: &Path, proof: &crate::native_record::Record| -> io::Result<Vec<u8>> {
        let row = rows
            .iter()
            .find(|row| row.sources.iter().any(|source| source.path == path))
            .ok_or_else(|| failure("Unqualified native launch source"))?;
        crate::transcript_range::with_source_reader(
            options,
            &row.harness,
            &row.sources,
            provider,
            mode,
            |reader, refs| {
                let reader = crate::source_reader::BudgetedReader::new(reader, requests.get());
                let value = crate::source_reader::read_evidence_bounded(
                    &reader,
                    refs.get(path)
                        .ok_or_else(|| failure("Unknown launch source"))?,
                    proof,
                    &mut bytes_left.borrow_mut(),
                );
                requests.set(reader.remaining());
                value
            },
        )
    };
    let mut native = NativeRecords {
        full: BTreeMap::new(),
        selected: selected.as_ref(),
        read: selected
            .as_ref()
            .map(|_| Box::new(read) as Box<dyn Fn(&Path, u64) -> io::Result<Value>>),
        raw: selected.as_ref().map(|_| Box::new(raw) as NativeBytes),
        evidence: selected
            .as_ref()
            .map(|_| Box::new(evidence) as EvidenceRead),
        error: std::cell::RefCell::new(None),
        fields,
    };
    if selected.is_none()
        && sources
            .iter()
            .map(|source| source.offset)
            .try_fold(0u64, u64::checked_add)
            .is_none_or(|total| total > BYTES_MAX)
    {
        return Err(failure(
            "Original native index is unavailable and migration exceeds its read budget",
        ));
    }
    let mut requests_left = 1024usize;
    // Harness is per source row; mixed recorded harness relationships share the
    // same request-owned provider, never an unqualified filesystem fallback.
    for row in rows.iter().filter(|_| selected.is_none()) {
        crate::transcript_range::with_source_reader(
            options,
            &row.harness,
            &row.sources,
            provider,
            mode,
            |reader, refs| {
                let bounded = crate::source_reader::BudgetedReader::new(reader, 1024);
                for (path, source) in refs {
                    crate::selected_read::charge(
                        0,
                        usize::try_from(source.offset).map_err(failure)?,
                    )?;
                    let mut bytes =
                        Vec::with_capacity(usize::try_from(source.offset).map_err(failure)?);
                    let mut observed = None::<String>;
                    let mut length = None;
                    while (bytes.len() as u64) < source.offset {
                        crate::selected_read::check()?;
                        let offset = bytes.len() as u64;
                        let max =
                            crate::SESSION_SOURCE_CHUNK_MAX.min((source.offset - offset) as usize);
                        requests_left = requests_left.checked_sub(1).ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::WouldBlock,
                                "Native migration request budget exhausted",
                            )
                        })?;
                        let part = bounded.read_range(source, observed.as_deref(), offset, max)?;
                        if part.offset != offset
                            || part.bytes.is_empty()
                            || part.bytes.len() > max
                            || part.length < source.offset
                            || part.generation.is_empty()
                            || part.generation.len() > 4096
                            || observed
                                .as_ref()
                                .is_some_and(|value| value != &part.generation)
                            || length.is_some_and(|value| value != part.length)
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Native migration generation changed",
                            ));
                        }
                        observed = Some(part.generation);
                        length = Some(part.length);
                        bytes.extend_from_slice(&part.bytes);
                    }
                    native.full.insert(path.clone(), bytes);
                }
                Ok(())
            },
        )?;
    }
    crate::selected_read::checkpoint("history_derivation")?;
    let result = derive(&native)?;
    crate::selected_read::check()?;
    if let Some(error) = native.error.take() {
        return Err(error);
    }
    Ok(result)
}

/// Selected native Query evidence uses the same original-generation reader as
/// Home/Trace repair. No provider or credentials escape this request.
pub(crate) fn with_query<T>(
    options: &Options,
    key: &str,
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
    now: i64,
    read: impl FnOnce(crate::model::QueryData) -> T,
) -> io::Result<T> {
    let _scope = crate::selected_read::Scope::ensure();
    crate::selected_read::check()?;
    let mut db = connection(options)?;
    let tx = transaction(&mut db, options)?;
    let selection = query_selection(&tx, key)?;
    if selection.incomplete {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Selected query relationship closure exceeds its source bound",
        ));
    }
    let rows = selection.rows;
    // Filter before index/body adaptation, including its missing-ledger path.
    // Original rows remain the generation footprint and skipped-parent input.
    let native_rows = if let Some(cutoff) = crate::model::scan_cutoff(options, now) {
        std::borrow::Cow::Owned(
            rows.iter()
                .filter_map(|row| {
                    let mut row = row.clone();
                    row.sources.retain(|source| {
                        crate::model::catalog_source_in_window(source, Some(cutoff))
                    });
                    (!row.sources.is_empty()).then_some(row)
                })
                .collect::<Vec<_>>(),
        )
    } else {
        std::borrow::Cow::Borrowed(rows.as_slice())
    };
    let data = with_records(options, &native_rows, provider, mode, tx, |native| {
        crate::model::query_history(options, &rows, native, now)
    })?;
    let result = read(data);
    let tx = transaction(&mut db, options)?;
    let current = query_selection(&tx, key)?;
    let footprint = |rows: &[CatalogRow]| -> BTreeMap<_, _> {
        rows.iter()
            .map(|row| (row.key.clone(), row.sources.clone()))
            .collect()
    };
    if current.incomplete || footprint(&current.rows) != footprint(&rows) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Selected query source or relationship generation changed; resynchronize",
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_record_windows_reuse_qualified_bytes_and_reject_rebinding() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Reader {
            generation: AtomicUsize,
            length: AtomicUsize,
        }
        impl SessionSourceReader for Reader {
            fn read_range(
                &self,
                _source: &crate::SessionSourceRef,
                _expected: Option<&str>,
                offset: u64,
                max: usize,
            ) -> io::Result<crate::SessionSourceRange> {
                Ok(crate::SessionSourceRange {
                    generation: self.generation.load(Ordering::Relaxed).to_string(),
                    length: self.length.load(Ordering::Relaxed) as u64,
                    offset,
                    bytes: Arc::from(vec![b'x'; max]),
                    cached: false,
                })
            }
        }
        let reader = Reader {
            generation: AtomicUsize::new(1),
            length: AtomicUsize::new(24 * 1024),
        };
        let bounded = crate::source_reader::BudgetedReader::new(&reader, 4);
        let windows = RecordWindows::default();
        let windowed = WindowReader {
            reader: &bounded,
            windows: &windows,
        };
        let source = crate::SessionSourceRef {
            root: "claude".into(),
            path: "projects/fixture.jsonl".into(),
            native_id: "fixture".into(),
            offset: 24 * 1024,
            prefix_sha256: [1; 32],
            tail_sha256: [2; 32],
        };
        assert_eq!(
            windowed
                .read_range(&source, None, 0, 8192)
                .unwrap()
                .bytes
                .len(),
            8192
        );
        for offset in 0..1024 {
            let part = windowed
                .read_range(&source, Some("1"), offset, 8192)
                .unwrap();
            assert_eq!(part.offset, offset);
            assert_eq!(part.bytes.len(), 8192 - offset as usize);
        }
        assert_eq!(
            bounded.remaining(),
            3,
            "adjacent records reuse qualified bytes"
        );
        let mut changed = source.clone();
        changed.prefix_sha256 = [3; 32];
        assert!(windowed.read_range(&changed, None, 0, 1).is_err());
        assert!(windowed.read_range(&source, Some("2"), 0, 1).is_err());
        reader.generation.store(2, Ordering::Relaxed);
        assert!(windowed.read_range(&source, None, 8192, 8192).is_err());
        reader.generation.store(1, Ordering::Relaxed);
        reader.length.store(24 * 1024 + 1, Ordering::Relaxed);
        assert!(windowed.read_range(&source, None, 8192, 8192).is_err());
        assert!(
            windowed
                .read_range(&source, None, source.offset, 1)
                .is_err()
        );
        assert_eq!(bounded.remaining(), 1);
    }

    #[test]
    fn migration_candidate_work_is_independent_of_unrelated_history() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); INSERT INTO meta VALUES('overview_needed:selected','1'),('unrelated:initial','1');").unwrap();
        let measure = || {
            let mut statement = db.prepare(CANDIDATES).unwrap();
            let keys = statement
                .query_map(rusqlite::params![8, ""], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (
                keys,
                statement.get_status(rusqlite::StatementStatus::VmStep),
            )
        };
        let before = measure();
        db.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO meta SELECT 'unrelated:'||printf('%05d',x),'1' FROM n;").unwrap();
        assert_eq!(measure(), before);
        assert_eq!(before.0, vec!["selected"]);
        let plan:String=db.query_row("EXPLAIN QUERY PLAN SELECT substr(key,17) FROM meta WHERE key>='overview_needed:' AND key<'overview_needed;' ORDER BY key LIMIT 8",[],|row|row.get(3)).unwrap();
        assert!(plan.contains("SEARCH meta USING COVERING INDEX"), "{plan}");
    }
}
