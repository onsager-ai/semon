//! Bounded migration of content-free publications from original native sources.
//! Uses the existing producer; never restores files or publishes current owners.
use crate::{
    Options, SessionSourceReadMode, SessionSourceReader, events::EventCache,
    model::summary::CatalogRow,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

const SOURCES_MAX: usize = 64;
const BYTES_MAX: u64 = 64 * 1024 * 1024;
const CANDIDATES: &str = "SELECT substr(key,17) FROM meta WHERE key>'overview_needed:'||?2 AND key<'overview_needed;' ORDER BY key LIMIT ?1";
type EvidenceRead<'a> =
    Box<dyn Fn(&Path, &crate::native_record::Record) -> io::Result<Vec<u8>> + 'a>;
type NativeRead<'a> = Box<dyn Fn(&Path, u64) -> io::Result<Value> + 'a>;

pub(crate) struct NativeRecords<'a> {
    full: BTreeMap<PathBuf, Vec<u8>>,
    selected: Option<&'a EventCache>,
    read: Option<NativeRead<'a>>,
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
        let rest = self.bytes(path)?.get(usize::try_from(offset).ok()?..)?;
        let end = rest.iter().position(|byte| *byte == b'\n')? + 1;
        (end <= 32 * 1024 * 1024).then(|| rest[..end].to_vec())
    }
    pub(crate) fn record(&self, path: &Path, offset: u64) -> Option<Value> {
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
    io::Error::other(error.to_string())
}
/// A selected producer observation, using the same bounded migration as the
/// hosted native reader. It publishes metadata without restoring native files.
pub fn session_overview_observe(
    options: &Options,
    source_key: &str,
    query: &str,
) -> crate::ViewerReply {
    let read = || -> io::Result<String> {
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
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "Invalid selected observation",
                            )
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
        let key = values
            .get("sid")
            .filter(|key| !key.is_empty() && key.len() <= 4096)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Select one session"))?;
        repair_session_overview(
            options,
            source_key,
            key,
            None,
            SessionSourceReadMode::LocalThenProvider,
        )?;
        Ok(key.clone())
    };
    match read() {
        Ok(key)=>crate::ViewerReply{status:200,content_type:"application/json; charset=utf-8",body:serde_json::json!({"api":1,"source_key":source_key,"catalog_key":key,"state":"ready"}).to_string().into_bytes(),etag:None},
        Err(error)=>crate::catalog::error(if error.kind()==io::ErrorKind::InvalidInput {400}else{503},"selected_observation",&error.to_string(),error.kind()!=io::ErrorKind::InvalidInput),
    }
}
fn connection(options: &Options) -> io::Result<Connection> {
    let db = Connection::open_with_flags(
        EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(failure)?;
    db.busy_timeout(Duration::from_millis(100))
        .map_err(failure)?;
    Ok(db)
}
fn catalog(db: &Connection, key: &str) -> io::Result<Option<CatalogRow>> {
    let text = crate::history_projection::catalog(db, key).map_err(failure)?.or(db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata ELSE NULL END FROM session_catalog WHERE session_key=?1",[key],|row|row.get::<_,String>(0)).optional().map_err(failure)?);
    text.map(|text| serde_json::from_str(&text).map_err(failure))
        .transpose()
}
fn ready(db: &Connection, row: &CatalogRow) -> io::Result<bool> {
    let has_tables = db.query_row("SELECT count(*)=2 FROM sqlite_schema WHERE name IN ('session_overview','session_trace_headers')",[],|row|row.get::<_,bool>(0)).map_err(failure)?;
    if !has_tables {
        return Ok(false);
    }
    let source = crate::retention::source_generation(&row.sources).map_err(failure)?;
    db.query_row("SELECT EXISTS(SELECT 1 FROM session_overview o JOIN session_trace_headers t USING(session_key) WHERE o.session_key=?1 AND json_extract(o.summary,'$.source_generation')=?2 AND t.source_generation=?2)",rusqlite::params![row.key,source],|row|row.get(0)).map_err(failure)
}

struct Selection {
    rows: Vec<CatalogRow>,
    incomplete: bool,
}
fn selection(db: &Connection, key: &str) -> io::Result<Selection> {
    let target = catalog(db, key)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Selected migration source unavailable",
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
    let mut related=db.prepare("SELECT session_key FROM session_catalog WHERE parent_key=?1 ORDER BY session_key LIMIT 65").map_err(failure)?;
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
        let mut statement = db.prepare("SELECT session_key FROM (SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?1 UNION SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?1 UNION SELECT session_key FROM session_trace_bookmarks WHERE target=?2) ORDER BY session_key LIMIT 65").map_err(failure)?;
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
    let mut related = db.prepare("SELECT endpoint FROM (SELECT parent_key AS endpoint FROM session_catalog WHERE session_key=?1 UNION SELECT session_key AS endpoint FROM session_catalog WHERE parent_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END AS endpoint FROM session_trace_edges WHERE session_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END AS endpoint FROM session_trace_edges WHERE session_key=?1) WHERE endpoint IS NOT NULL ORDER BY endpoint LIMIT 65").map_err(failure)?;
    let mut aliases = db.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE native_id=?1 ORDER BY session_key LIMIT 65").map_err(failure)?;
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
    let tx = db.transaction().map_err(failure)?;
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
    let tx = db.transaction().map_err(failure)?;
    let selected = query_selection(&tx, key)?;
    if selected.incomplete {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Selected query relationship closure exceeds its source bound",
        ));
    }
    source_refs(options, &selected.rows)
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
    let tx = db.transaction().map_err(failure)?;
    let (key, selected) = match query_selection(&tx, key) {
        Ok(selected) => (key.to_owned(), selected),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A complete Catalog's namespace can differ from the CLI's file
            // window. Resolve original native IDs through the existing index;
            // query_selection then includes every identity collision owner.
            let mut aliases = tx.prepare("SELECT DISTINCT session_key FROM session_catalog_sources WHERE native_id=?1 ORDER BY session_key LIMIT 65").map_err(failure)?;
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
            let selected = query_selection(&tx, &owner)?;
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
    let ids = selected
        .rows
        .iter()
        .flat_map(|row| row.native_ids.iter().cloned())
        .collect();
    let cache = match EventCache::from_selected(&tx, &sources, &ids) {
        Ok(cache) => cache,
        Err(_) => return Ok(None),
    };
    // Sidecars are independently generation-bound native evidence. A changed
    // sidecar must not lend an old role/name merely because its log is stable.
    let ready = sources.iter().all(|source| {
        cache.selected_index(&source.path).is_some_and(|index| {
            crate::model::current_query_role(source, &index)
                && index
                    .claude_sidecar
                    .as_ref()
                    .is_none_or(|sidecar| sidecar.current(&source.path))
        })
    });
    Ok(ready.then_some(key))
}

fn source_refs(options: &Options, rows: &[CatalogRow]) -> io::Result<Vec<crate::SessionSourceRef>> {
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
    let db = connection(options)?;
    // Older schemas migrate through their owning producer, never raw SQL here.
    if !db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='session_catalog')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(failure)?
    {
        return Ok(Vec::new());
    }
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
/// die with this call; only the existing Home/Trace metadata tables change.
/// The embedding host owns publication/custody fences and authorization checks.
pub fn repair_session_overview(
    options: &Options,
    source_key: &str,
    key: &str,
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
) -> io::Result<bool> {
    if source_key.is_empty() || source_key.len() > 4096 || key.is_empty() || key.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid migration selection",
        ));
    }
    let mut db = connection(options)?;
    let tx = db.transaction().map_err(failure)?;
    let selection = selection(&tx, key)?;
    let rows = selection.rows;
    if ready(&tx, &rows[0])? {
        tx.commit().map_err(failure)?;
        let mut writable = Connection::open(EventCache::path(&options.cache)).map_err(failure)?;
        writable.busy_timeout(Duration::ZERO).map_err(failure)?;
        let tx = writable
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(failure)?;
        if let Some(current) = catalog(&tx, key)?
            && ready(&tx, &current)?
        {
            tx.execute(
                "DELETE FROM meta WHERE key=?1",
                [format!("overview_needed:{key}")],
            )
            .map_err(failure)?;
        }
        tx.commit().map_err(failure)?;
        return Ok(false);
    }
    let generation: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
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
    // Initialize through the owning schema migration. It preserves independent
    // current/history membership; the publication below does not touch either.
    let _schema = EventCache::open_scoped(&options.cache).map_err(failure)?;
    let mut writable = Connection::open(EventCache::path(&options.cache)).map_err(failure)?;
    writable.busy_timeout(Duration::ZERO).map_err(failure)?;
    let tx = writable
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(failure)?;
    let current: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key='catalog_generation'",
            [],
            |row| row.get(0),
        )
        .map_err(failure)?;
    if current != generation {
        return Ok(false);
    }
    for row in &repaired {
        let Some(current) = catalog(&tx, &row.key)? else {
            return Ok(false);
        };
        if current.sources != row.sources {
            return Ok(false);
        }
    }
    for row in &repaired {
        if let Some(summary) = &row.overview {
            crate::overview::publish(&tx, &row.key, row.last.unwrap_or(0), summary)
                .map_err(failure)?;
        }
        if let Some(trace) = &row.trace {
            crate::causal_trace::publish(&tx, row, trace).map_err(failure)?;
        }
        tx.execute(
            "DELETE FROM meta WHERE key=?1",
            [format!("overview_needed:{}", row.key)],
        )
        .map_err(failure)?;
    }
    tx.commit().map_err(failure)?;
    Ok(true)
}

fn with_records<T>(
    options: &Options,
    rows: &[CatalogRow],
    provider: Option<&dyn SessionSourceReader>,
    mode: SessionSourceReadMode,
    tx: rusqlite::Transaction<'_>,
    derive: impl FnOnce(&NativeRecords<'_>) -> io::Result<T>,
) -> io::Result<T> {
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
    let selected = EventCache::from_selected(&tx, &sources, &ids).ok();
    let mut fields = BTreeMap::new();
    if selected.is_some() {
        let mut field_bytes = 0;
        for row in rows {
            let source = crate::retention::source_generation(&row.sources).map_err(failure)?;
            let history = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_history_projections WHERE session_key=?1 AND source_generation=?2 AND version=5)",rusqlite::params![row.key,source],|r|r.get::<_,bool>(0)).map_err(failure)?;
            let current = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_slot_projections WHERE session_key=?1 AND source_generation=?2 AND version=5)",rusqlite::params![row.key,source],|r|r.get::<_,bool>(0)).map_err(failure)?;
            if !history && !current {
                continue;
            }
            let table = if history {
                "session_history_slots"
            } else {
                "session_slots"
            };
            let mut statement=tx.prepare(&format!("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM {table} WHERE session_key=?1 AND json_extract(metadata,'$.field') IS NOT NULL ORDER BY slot LIMIT 4096")).map_err(failure)?;
            for text in statement
                .query_map([&row.key], |r| r.get::<_, Option<String>>(0))
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
    let memo = std::cell::RefCell::new(BTreeMap::<(PathBuf, u64), Value>::new());
    let read = |path: &Path, offset: u64| -> io::Result<Value> {
        let key = (path.to_owned(), offset);
        if let Some(value) = memo.borrow().get(&key) {
            return Ok(value.clone());
        }
        let row = rows
            .iter()
            .find(|row| row.sources.iter().any(|source| source.path == path))
            .ok_or_else(|| failure("Unqualified native producer source"))?;
        let value = crate::transcript_range::with_source_reader(
            options,
            &row.harness,
            &row.sources,
            provider,
            mode,
            |reader, refs| {
                let reader = crate::source_reader::BudgetedReader::new(reader, requests.get());
                let value = crate::source_reader::read_record_bounded(
                    &reader,
                    refs.get(path)
                        .ok_or_else(|| failure("Unknown producer source"))?,
                    offset,
                    32 * 1024 * 1024,
                    &mut bytes_left.borrow_mut(),
                );
                requests.set(reader.remaining());
                value
            },
        )?;
        if value.to_string().len() <= 1048576 {
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
                    let mut bytes =
                        Vec::with_capacity(usize::try_from(source.offset).map_err(failure)?);
                    let mut observed = None::<String>;
                    let mut length = None;
                    while (bytes.len() as u64) < source.offset {
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
    let result = derive(&native)?;
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
    let mut db = connection(options)?;
    let tx = db.transaction().map_err(failure)?;
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
    let tx = db.transaction().map_err(failure)?;
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
