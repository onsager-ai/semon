//! Paged content-free byte proofs, owned by the existing native ledger CAS.
use super::*;
use crate::source_carrier::FailureStage;
use crate::source_chunks::Chunks;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};

const PAGE: usize = 1024;
mod published;

/// Own the selected read transaction until the last lazy proof reader drops.
/// Connections are never returned to a pool with an unfinished transaction.
pub(crate) struct Snapshot(Mutex<Connection>);

impl Snapshot {
    pub(crate) fn new(db: Connection) -> Arc<Self> {
        Arc::new(Self(Mutex::new(db)))
    }

    fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, Connection>> {
        self.0.lock()
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let db = self.0.get_mut().unwrap_or_else(|error| error.into_inner());
        if !db.is_autocommit() {
            // Closing the owned connection also rolls back if this fails.
            let _ = db.execute_batch("ROLLBACK");
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    parser_version: u32,
    ledger: String,
    count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    consumed_offset: Option<u64>,
    fingerprint: [u8; 32],
    pages_root: [u8; 32],
    #[serde(default)]
    resume: Option<crate::source_chunks::PrefixState>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    version: u32,
    index: usize,
    hashes: Vec<[u8; 32]>,
    sha256: String,
}

impl Page {
    fn canonical(&self) -> rusqlite::Result<String> {
        // The witness covers only the page payload, never its own digest.
        #[derive(Serialize)]
        struct Payload<'a> {
            version: u32,
            index: usize,
            hashes: &'a [[u8; 32]],
        }
        json(&Payload {
            version: self.version,
            index: self.index,
            hashes: &self.hashes,
        })
    }
}

fn prefix(path: &str) -> String {
    // Fixed ASCII keys make exact namespace cleanup independent of path glob
    // characters; host paths and raw source bytes never enter proof values.
    format!(
        "native_source_chunks:{:x}:",
        Sha256::digest(path.as_bytes())
    )
}

#[derive(Default)]
struct HeaderMismatchLimiter {
    last: BTreeMap<String, HeaderMismatchEntry>,
    next_order: u64,
}

#[derive(PartialEq, Eq)]
struct HeaderMismatchEvent {
    catalog: String,
    proof: String,
    stage: &'static str,
    raw_collected: bool,
}

impl HeaderMismatchEvent {
    fn covers(&self, next: &Self) -> bool {
        self.catalog == next.catalog
            && self.proof == next.proof
            && self.stage == next.stage
            && (self.raw_collected || !next.raw_collected)
    }
}

struct HeaderMismatchEntry {
    event: HeaderMismatchEvent,
    order: u64,
}

impl HeaderMismatchLimiter {
    fn allow(
        &mut self,
        source: &str,
        catalog: &str,
        proof: &str,
        stage: &'static str,
        raw_collected: bool,
    ) -> bool {
        let event = HeaderMismatchEvent {
            catalog: catalog.to_owned(),
            proof: proof.to_owned(),
            stage,
            raw_collected,
        };
        if self
            .last
            .get(source)
            .is_some_and(|prior| prior.event.covers(&event))
        {
            return false;
        }
        if self.last.len() >= 256 && !self.last.contains_key(source) {
            let oldest = self
                .last
                .iter()
                .min_by_key(|(_, entry)| entry.order)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.last.remove(&oldest);
            }
        }
        self.next_order = self.next_order.wrapping_add(1);
        self.last.insert(
            source.to_owned(),
            HeaderMismatchEntry {
                event,
                order: self.next_order,
            },
        );
        true
    }
}

#[derive(Default)]
pub(crate) struct HeaderOpenDiagnostic {
    pub(super) proof_ledger_digest: Option<String>,
    pub(super) index_ledger_digest: Option<String>,
    pub(super) proof_offset: Option<u64>,
    pub(super) index_offset: Option<u64>,
    pub(super) parser_version: Option<u32>,
}

fn fresh_raw_size(path: &str) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

pub(crate) fn log_header_rejection(
    path: &str,
    stage: FailureStage,
    ledger: &Ledger,
    diagnostic: &HeaderOpenDiagnostic,
    raw_before: Option<u64>,
    raw_after: Option<u64>,
) {
    static LAST: OnceLock<Mutex<HeaderMismatchLimiter>> = OnceLock::new();
    let source_hash = crate::source_carrier::source_hash(path);
    let catalog_hash = crate::source_carrier::ledger_digest(ledger);
    let proof_hash = diagnostic
        .proof_ledger_digest
        .as_deref()
        .unwrap_or("missing");
    let allowed = LAST
        .get_or_init(|| Mutex::new(HeaderMismatchLimiter::default()))
        .lock()
        .is_ok_and(|mut limiter| {
            limiter.allow(
                &source_hash,
                &catalog_hash,
                proof_hash,
                stage.name(),
                raw_before.is_some(),
            )
        });
    if allowed {
        let raw = match raw_before {
            Some(before) => format!("raw_before={before} raw_after={raw_after:?}"),
            None => "raw=not_collected".to_owned(),
        };
        let line = format!(
            "semon native proof: source_hash={} stage={} ledger_digest_domain=ledger-v1 catalog_ledger_digest={} proof_ledger_digest={} index_ledger_digest={} catalog_offset={} proof_offset={:?} index_offset={:?} proof_parser={:?} {raw} sample=independent_statements",
            &source_hash[..12],
            stage.name(),
            &catalog_hash[..12],
            diagnostic
                .proof_ledger_digest
                .as_deref()
                .map_or("missing", |hash| &hash[..12]),
            diagnostic
                .index_ledger_digest
                .as_deref()
                .map_or("missing", |hash| &hash[..12]),
            ledger.offset,
            diagnostic.proof_offset,
            diagnostic.index_offset,
            diagnostic.parser_version
        );
        eprintln!("{line}");
        #[cfg(test)]
        HEADER_DIAGNOSTICS.with(|lines| lines.borrow_mut().push(line));
    }
}

#[cfg(test)]
pub(crate) fn legacy_change_token_for_test(cache: &Path, path: &str, ledger: &Ledger) {
    let db = Connection::open(cache).unwrap();
    let key = format!("{}header", prefix(path));
    let encoded: String = db
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r.get(0))
        .unwrap();
    let mut header: Header = serde_json::from_str(&encoded).unwrap();
    assert_eq!(header.ledger, header_ledger(ledger));
    let mut legacy = ledger.clone();
    legacy.stat.changed_ns = None;
    header.ledger = header_ledger(&legacy);
    db.execute(
        "UPDATE meta SET value=?2 WHERE key=?1",
        params![key, json(&header).unwrap()],
    )
    .unwrap();
}

pub(super) fn digest(connection: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    let encoded: Option<Option<String>> = connection
        .query_row(
            native_metadata_value(connection)?,
            [format!("{}header", prefix(path))],
            |row| row.get(0),
        )
        .optional()?;
    Ok(encoded
        .flatten()
        .map(|value| format!("{:x}", Sha256::digest(value.as_bytes()))))
}

pub(super) fn read(
    connection: &Connection,
    path: &str,
    ledger: &Ledger,
) -> rusqlite::Result<Option<Arc<Chunks>>> {
    let prefix = prefix(path);
    let header: Option<Option<String>> = connection
        .query_row(native_metadata_value(connection)?, [format!("{prefix}header")], |row| {
            crate::selected_read::row(row, 512)?;
            row.get(0)
        })
        .optional()?;
    let Some(header) = header
        .flatten()
        .and_then(|text| serde_json::from_str::<Header>(&text).ok())
    else {
        return Ok(None);
    };
    if header.version != 1
        || header.parser_version != CACHE_VERSION
        || header.ledger != header_ledger(ledger)
        || usize::try_from(ledger.offset.div_ceil(crate::source_chunks::SIZE as u64))
            != Ok(header.count)
    {
        return Ok(None);
    }
    let mut statement = connection.prepare(
        configured::metadata_sql(connection, "SELECT key, CASE WHEN length(CAST(value AS BLOB))<=?3 THEN value ELSE NULL END FROM meta WHERE key>=?1 AND key<?2 ORDER BY key", "SELECT key, CASE WHEN length(CAST(value AS BLOB))<=?3 THEN value ELSE NULL END FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2 ORDER BY key")?
    )?;
    let mut rows = statement.query(params![
        &prefix,
        format!("{prefix}~"),
        NATIVE_METADATA_MAX as i64
    ])?;
    let mut hashes = Vec::new();
    let mut page = 0;
    while let Some(row) = rows.next()? {
        crate::selected_read::row(row, PAGE * 32)?;
        let key: String = row.get(0)?;
        if key == format!("{prefix}header") {
            continue;
        }
        if key != format!("{prefix}{page:010}") {
            return Ok(None);
        }
        let Some(encoded) = row.get::<_, Option<String>>(1)? else {
            return Ok(None);
        };
        let Some(part) = serde_json::from_str::<Page>(&encoded).ok() else {
            return Ok(None);
        };
        let remaining = header.count.saturating_sub(hashes.len());
        if part.version != 1
            || part.index != page
            || part.hashes.len() != remaining.min(PAGE)
            || remaining == 0
            || part.sha256 != page_witness(&part.canonical()?)
        {
            return Ok(None);
        }
        hashes.extend(part.hashes);
        page += 1;
    }
    let chunks = Chunks {
        offset: ledger.offset,
        hashes,
        resume: header.resume,
    };
    if !chunks.valid(ledger.offset) || chunks.fingerprint() != header.fingerprint {
        return Ok(None);
    }
    Ok(Some(Arc::new(chunks)))
}

// Warm Tree metadata needs the ledger-bound proof root, not all historical
// block hashes. Focused readers verify the selected pages against this root.
pub(super) fn header_ready(c: &Connection, path: &str, ledger: &Ledger) -> rusqlite::Result<bool> {
    let encoded: Option<Option<String>> = c
        .query_row(
            native_metadata_value(c)?,
            [format!("{}header", prefix(path))],
            |r| {
                crate::selected_read::row(r, 512)?;
                r.get(0)
            },
        )
        .optional()?;
    let Some(header) = encoded
        .flatten()
        .and_then(|s| serde_json::from_str::<Header>(&s).ok())
    else {
        return Ok(false);
    };
    Ok(header.version == 1
        && header.parser_version == CACHE_VERSION
        && header.ledger == header_ledger(ledger)
        && usize::try_from(ledger.offset.div_ceil(crate::source_chunks::SIZE as u64))
            == Ok(header.count))
}

/// Request-owned proof pager. The producer binds each page to the exact header
/// and page digest in its ledger transaction. No source bytes or reader survive
/// the request, and completion reads the same witnesses again.
pub(crate) struct Focused {
    db: Arc<Snapshot>,
    cache_path: PathBuf,
    prefix: String,
    header: String,
    ledger: Ledger,
    path: String,
    count: usize,
    pages_root: [u8; 32],
    cached: std::sync::Mutex<Option<(usize, Page)>>,
}

#[cfg(test)]
thread_local! {
    pub(crate) static FOCUSED_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static FOCUSED_METADATA_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static HEADER_DIAGNOSTICS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn proof_db_error(missing: FailureStage, error: rusqlite::Error) -> io::Error {
    let stage = if matches!(error, rusqlite::Error::QueryReturnedNoRows) {
        missing
    } else {
        FailureStage::ProofOpenDb
    };
    stage.wrap(io::Error::other(error))
}

impl Focused {
    #[cfg(test)]
    pub(crate) fn open(cache: &Path, path: &str, ledger: &Ledger) -> io::Result<Self> {
        Self::open_with_raw(cache, path, ledger, None)
    }

    pub(crate) fn open_with_raw(
        cache: &Path,
        path: &str,
        ledger: &Ledger,
        raw_before: Option<u64>,
    ) -> io::Result<Self> {
        let mut diagnostic = HeaderOpenDiagnostic::default();
        let result = (|| {
            let db = super::focused_connection(cache)
                .map_err(|error| FailureStage::ProofOpenDb.wrap(error))?;
            db.busy_timeout(Duration::from_millis(100))
                .map_err(|error| FailureStage::ProofOpenDb.wrap(io::Error::other(error)))?;
            Self::with_db_inner(Snapshot::new(db), cache, path, ledger, &mut diagnostic)
        })();
        if let Err(error) = &result
            && let Some(stage) = FailureStage::of(error)
        {
            crate::source_carrier::note_diagnostic_interest(cache, path, ledger);
            let raw_after = raw_before.and_then(|_| fresh_raw_size(path));
            log_header_rejection(path, stage, ledger, &diagnostic, raw_before, raw_after);
        }
        result
    }

    pub(crate) fn with_db(
        db: Arc<Snapshot>,
        cache: &Path,
        path: &str,
        ledger: &Ledger,
    ) -> io::Result<Self> {
        let mut diagnostic = HeaderOpenDiagnostic::default();
        let result = Self::with_db_inner(db, cache, path, ledger, &mut diagnostic);
        if let Err(error) = &result
            && let Some(stage) = FailureStage::of(error)
        {
            crate::source_carrier::note_diagnostic_interest(cache, path, ledger);
            log_header_rejection(path, stage, ledger, &diagnostic, None, None);
        }
        result
    }

    fn with_db_inner(
        db: Arc<Snapshot>,
        cache: &Path,
        path: &str,
        ledger: &Ledger,
        diagnostic: &mut HeaderOpenDiagnostic,
    ) -> io::Result<Self> {
        let connection = db.lock().map_err(|_| {
            FailureStage::ProofOpenDb.wrap(io::Error::other("Native proof reader unavailable"))
        })?;
        // Keep the proof header, pages and witnesses in one request-owned WAL
        // snapshot. Ingestion can advance or collect an old publication while
        // this reader continues verifying its original consumed prefix.
        if connection.is_autocommit() {
            connection
                .execute_batch("BEGIN DEFERRED")
                .map_err(|error| FailureStage::ProofOpenDb.wrap(io::Error::other(error)))?;
        }
        if !super::current(&connection)
            .map_err(|error| FailureStage::ProofOpenDb.wrap(io::Error::other(error)))?
        {
            return Err(
                FailureStage::HeaderStore.wrap(io::Error::other("Native proof store changed"))
            );
        }
        super::require_current(&connection)
            .map_err(|error| FailureStage::ProofOpenDb.wrap(error))?;
        if let Some((_, index)) = super::ledger_row(&connection, path).ok().flatten() {
            diagnostic.index_ledger_digest = Some(crate::source_carrier::ledger_digest(&index));
            diagnostic.index_offset = Some(index.offset);
        }
        let metadata_value = native_metadata_value(&connection).map_err(|error| proof_db_error(FailureStage::ProofOpenDb, error))?;
        let mut prefix = prefix(path);
        let live: Option<String> = connection
            .query_row(metadata_value, [format!("{prefix}header")], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| FailureStage::ProofOpenDb.wrap(io::Error::other(error)))?
            .flatten();
        // Retained headers belong to immutable published generations. Prefer
        // an exact current live header; otherwise select this ledger's retained
        // generation, which still has to pass the current header checks.
        let live_matches = live.as_deref().is_some_and(|text| {
            serde_json::from_str::<Header>(text).is_ok_and(|header| {
                header.version == 1
                    && header.parser_version == CACHE_VERSION
                    && header.ledger == header_ledger(ledger)
            })
        });
        let retained = published::prefix(path, &crate::source_carrier::ledger_digest(ledger));
        let retained_header: Option<String> = connection
            .query_row(
                metadata_value,
                [format!("{retained}header")],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| FailureStage::ProofOpenDb.wrap(io::Error::other(error)))?
            .flatten();
        let encoded: String = if live_matches {
            live.expect("matching live header")
        } else if let Some(encoded) = retained_header {
            prefix = retained;
            encoded
        } else {
            live.ok_or_else(|| {
                FailureStage::HeaderMissing
                    .wrap(io::Error::other("Native proof header unavailable"))
            })?
        };
        let header: Header = serde_json::from_str(&encoded).map_err(|error| {
            FailureStage::HeaderVersion
                .wrap_preserving_kind(io::Error::new(io::ErrorKind::InvalidData, error))
        })?;
        diagnostic.proof_ledger_digest =
            Some(crate::source_carrier::ledger_digest_value(&header.ledger));
        diagnostic.proof_offset = header.consumed_offset;
        diagnostic.parser_version = Some(header.parser_version);
        let stage = if header.version != 1 {
            Some(FailureStage::HeaderVersion)
        } else if header.parser_version != CACHE_VERSION {
            Some(FailureStage::HeaderParser)
        } else if header.ledger != header_ledger(ledger) {
            Some(FailureStage::HeaderLedger)
        } else if usize::try_from(ledger.offset.div_ceil(crate::source_chunks::SIZE as u64))
            != Ok(header.count)
        {
            Some(FailureStage::HeaderCount)
        } else {
            None
        };
        if let Some(stage) = stage {
            return Err(stage.wrap_preserving_kind(io::Error::new(
                io::ErrorKind::InvalidData,
                "Native proof header changed",
            )));
        }
        #[cfg(test)]
        FOCUSED_METADATA_BYTES.with(|n| n.set(n.get() + encoded.len()));
        drop(connection);
        Ok(Self {
            db,
            cache_path: cache.to_owned(),
            prefix,
            header: encoded,
            ledger: ledger.clone(),
            path: path.to_owned(),
            count: header.count,
            pages_root: header.pages_root,
            cached: std::sync::Mutex::new(None),
        })
    }

    fn hash(&self, index: usize) -> io::Result<[u8; 32]> {
        let result = self.hash_inner(index);
        if result
            .as_ref()
            .is_err_and(|error| FailureStage::of(error).is_some())
        {
            crate::source_carrier::note_diagnostic_interest(
                &self.cache_path,
                &self.path,
                &self.ledger,
            );
        }
        result
    }

    fn hash_inner(&self, index: usize) -> io::Result<[u8; 32]> {
        if index >= self.count {
            return Err(
                FailureStage::PageMissing.wrap(io::Error::other("Native proof block unavailable"))
            );
        }
        let db = self.db.lock().map_err(|_| {
            FailureStage::ProofOpenDb.wrap(io::Error::other("Native proof reader unavailable"))
        })?;
        let tx = &*db;
        let metadata_value = native_metadata_value(tx).map_err(|error| proof_db_error(FailureStage::ProofOpenDb, error))?;
        let header: String = tx
            .query_row(
                metadata_value,
                [format!("{}header", self.prefix)],
                |row| row.get(0),
            )
            .map_err(|error| proof_db_error(FailureStage::HeaderMissing, error))?;
        if header != self.header {
            return Err(FailureStage::ProofChanged
                .wrap(io::Error::other("Native proof publication changed")));
        }
        let page = index / PAGE;
        let mut cache = self.cached.lock().map_err(|_| {
            FailureStage::ProofOpenDb.wrap(io::Error::other("Native proof cache unavailable"))
        })?;
        if let Some((cached_page, part)) = cache.as_ref()
            && *cached_page == page
        {
            let witness: Option<String> = tx
                .query_row(
                    configured::metadata_sql(tx, "SELECT json_extract(value,'$.sha256') FROM meta WHERE key=?1", "SELECT json_extract(value,'$.sha256') FROM meta WHERE namespace=semon_namespace() AND key=?1").map_err(|error| proof_db_error(FailureStage::ProofOpenDb, error))?,
                    [format!("{}{page:010}", self.prefix)],
                    |r| r.get(0),
                )
                .map_err(|error| proof_db_error(FailureStage::PageMissing, error))?;
            if witness.as_deref() != Some(part.sha256.as_str()) {
                return Err(
                    FailureStage::PageHash.wrap(io::Error::other("Native proof page changed"))
                );
            }
            #[cfg(test)]
            FOCUSED_METADATA_BYTES
                .with(|n| n.set(n.get() + header.len() + witness.as_ref().map_or(0, String::len)));
            return Ok(part.hashes[index % PAGE]);
        }
        let encoded: String = tx
            .query_row(
                metadata_value,
                [format!("{}{page:010}", self.prefix)],
                |row| row.get(0),
            )
            .map_err(|error| proof_db_error(FailureStage::PageMissing, error))?;
        let part: Page = serde_json::from_str(&encoded).map_err(|error| {
            FailureStage::PageHash
                .wrap_preserving_kind(io::Error::new(io::ErrorKind::InvalidData, error))
        })?;
        if part.version != 1
            || part.index != page
            || part.hashes.len() != (self.count - page * PAGE).min(PAGE)
        {
            return Err(
                FailureStage::PageMissing.wrap_preserving_kind(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Native proof page is incomplete",
                )),
            );
        }
        let canonical = part
            .canonical()
            .map_err(|error| FailureStage::PageHash.wrap(io::Error::other(error)))?;
        if part.sha256 != page_witness(&canonical) {
            return Err(FailureStage::PageHash.wrap(io::Error::other("Native proof page changed")));
        }
        {
            let mut node: [u8; 32] = Sha256::digest(canonical.as_bytes()).into();
            let mut at = page;
            let mut width = self.count.div_ceil(PAGE);
            let mut level = 0;
            while width > 1 {
                let sibling_at = at ^ 1;
                let sibling = if sibling_at >= width {
                    node
                } else {
                    let encoded: String = tx
                        .query_row(
                            metadata_value,
                            [format!(
                                "native_proof_tree:{}:{level:03}:{sibling_at:010}",
                                self.prefix
                            )],
                            |r| r.get(0),
                        )
                        .map_err(|error| proof_db_error(FailureStage::PageMissing, error))?;
                    #[cfg(test)]
                    FOCUSED_METADATA_BYTES.with(|n| n.set(n.get() + encoded.len()));
                    serde_json::from_str::<[u8; 32]>(&encoded)
                        .map_err(|error| FailureStage::PageHash.wrap(io::Error::other(error)))?
                };
                node = if at.is_multiple_of(2) {
                    tree_parent(&node, &sibling)
                } else {
                    tree_parent(&sibling, &node)
                };
                at /= 2;
                width = width.div_ceil(2);
                level += 1;
            }
            if node != self.pages_root {
                return Err(
                    FailureStage::PageHash.wrap(io::Error::other("Native proof root changed"))
                );
            }
        }
        #[cfg(test)]
        {
            FOCUSED_HASHES.with(|n| n.set(n.get() + part.hashes.len()));
            FOCUSED_METADATA_BYTES.with(|n| n.set(n.get() + header.len() + encoded.len()));
        }
        let hash = part.hashes[index % PAGE];
        *cache = Some((page, part));
        Ok(hash)
    }

    pub(crate) fn clear_cache(&self) -> io::Result<()> {
        *self.cached.lock().map_err(|_| {
            FailureStage::ProofOpenDb.wrap(io::Error::other("Native proof cache unavailable"))
        })? = None;
        Ok(())
    }

    pub(crate) fn verify(&self, index: usize, bytes: &[u8]) -> io::Result<()> {
        let hash = self.hash(index)?;
        let start = (index as u64)
            .checked_mul(crate::source_chunks::SIZE as u64)
            .filter(|start| *start < self.ledger.offset)
            .ok_or_else(|| {
                FailureStage::PageMissing.wrap(io::Error::other("Native proof block unavailable"))
            })?;
        let length = (self.ledger.offset - start).min(crate::source_chunks::SIZE as u64) as usize;
        #[cfg(test)]
        crate::events::SOURCE_HASHED_BYTES.with(|n| n.set(n.get() + bytes.len() as u64));
        if bytes.len() != length || <[u8; 32]>::from(Sha256::digest(bytes)) != hash {
            return Err(FailureStage::PageHash.wrap(io::Error::other(
                "Native source block generation unavailable",
            )));
        }
        Ok(())
    }

    pub(crate) fn read(
        &self,
        file: &mut (impl io::Read + io::Seek),
        index: usize,
    ) -> io::Result<Vec<u8>> {
        use std::io::SeekFrom;
        let start = (index as u64)
            .checked_mul(crate::source_chunks::SIZE as u64)
            .filter(|start| *start < self.ledger.offset)
            .ok_or_else(|| {
                FailureStage::PageMissing.wrap(io::Error::other("Native proof block unavailable"))
            })?;
        let mut bytes =
            vec![0; (self.ledger.offset - start).min(crate::source_chunks::SIZE as u64) as usize];
        file.seek(SeekFrom::Start(start))
            .map_err(|error| FailureStage::PageHash.wrap_preserving_kind(error))?;
        file.read_exact(&mut bytes)
            .map_err(|error| FailureStage::PageHash.wrap_preserving_kind(error))?;
        #[cfg(test)]
        {
            crate::source_chunks::READ_BYTES.with(|n| n.set(n.get() + bytes.len() as u64));
            crate::source_chunks::READ_BLOCKS.with(|blocks| {
                blocks.borrow_mut().insert(index);
            });
        }
        self.verify(index, &bytes)?;
        Ok(bytes)
    }
}

fn tree_parent(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(left);
    hash.update(right);
    hash.finalize().into()
}
fn page_witness(page: &str) -> String {
    format!("{:x}", Sha256::digest(page.as_bytes()))
}

pub(super) fn clear(transaction: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    published::preserve(transaction, path)?;
    let prefix = prefix(path);
    transaction.execute(
        configured::metadata_sql(transaction, "DELETE FROM meta WHERE key>=?1 AND key<?2", "DELETE FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2")?,
        params![&prefix, format!("{prefix}~")],
    )?;
    let namespace = format!("native_proof_tree:{prefix}:");
    transaction.execute(
        configured::metadata_sql(transaction, "DELETE FROM meta WHERE key>=?1 AND key<?2", "DELETE FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2")?,
        params![&namespace, format!("{namespace}~")],
    )?;
    Ok(())
}

pub(super) fn write(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    chunks: Option<&Chunks>,
) -> rusqlite::Result<()> {
    write_measured(
        transaction,
        path,
        ledger,
        chunks,
        &mut crate::native_timing::Measurements::default(),
    )
}

pub(super) fn write_measured(
    transaction: &Transaction<'_>,
    path: &str,
    ledger: &Ledger,
    chunks: Option<&Chunks>,
    measurements: &mut crate::native_timing::Measurements,
) -> rusqlite::Result<()> {
    let started = measurements.start();
    let chunks = chunks.filter(|chunks| chunks.valid(ledger.offset));
    measurements.record("chunks_validation", started, 1);
    let Some(chunks) = chunks else {
        let started = measurements.start();
        let result = clear(transaction, path);
        measurements.record("chunks_clear", started, 1);
        return result;
    };
    let started = measurements.start();
    let prefix = prefix(path);
    published::preserve(transaction, path)?;
    measurements.record("chunks_prefix", started, 1);
    let started = measurements.start();
    let mut put = transaction.prepare_cached(configured::metadata_sql(transaction, "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value", "INSERT INTO meta(namespace,key,value) VALUES(semon_meta_scope(?1),?1,?2) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value")?)?;
    measurements.record("chunks_prepare", started, 1);
    let mut leaves = Vec::new();
    for (index, hashes) in chunks.hashes.chunks(PAGE).enumerate() {
        let started = measurements.start();
        let key = format!("{prefix}{index:010}");
        let mut value = Page {
            version: 1,
            index,
            hashes: hashes.to_vec(),
            sha256: String::new(),
        };
        let encoded = value.canonical()?;
        measurements.record("chunks_page_serialization", started, 1);
        measurements.count("chunks_page_bytes", encoded.len());
        measurements.count("chunks_hashes", hashes.len());
        let started = measurements.start();
        leaves.push(<[u8; 32]>::from(Sha256::digest(encoded.as_bytes())));
        value.sha256 = page_witness(&encoded);
        let encoded = json(&value)?;
        put.execute(params![key, encoded])?;
        measurements.record("chunks_page_sql", started, 1);
    }
    let mut nodes = leaves;
    let mut level = 0;
    let pages_root = loop {
        for (at, node) in nodes.iter().enumerate() {
            put.execute(params![
                format!("native_proof_tree:{prefix}:{level:03}:{at:010}"),
                json(node)?
            ])?;
        }
        let namespace = format!("native_proof_tree:{prefix}:{level:03}:");
        transaction.execute(
            configured::metadata_sql(transaction, "DELETE FROM meta WHERE key>=?1 AND key<?2", "DELETE FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2")?,
            params![
                format!("{namespace}{:010}", nodes.len()),
                format!("{namespace}~")
            ],
        )?;
        if nodes.len() <= 1 {
            break nodes
                .first()
                .copied()
                .unwrap_or_else(|| Sha256::digest([]).into());
        }
        nodes = nodes
            .chunks(2)
            .map(|pair| tree_parent(&pair[0], pair.get(1).unwrap_or(&pair[0])))
            .collect();
        level += 1;
    };
    transaction.execute(
        configured::metadata_sql(transaction, "DELETE FROM meta WHERE key>=?1 AND key<?2", "DELETE FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2")?,
        params![
            format!("native_proof_tree:{prefix}:{:03}:", level + 1),
            format!("native_proof_tree:{prefix}:~")
        ],
    )?;
    let started = measurements.start();
    measurements.cleanup(
        transaction,
        &prefix,
        || chunks.hashes.len().div_ceil(PAGE),
        &crate::native_timing::CHUNKS_CLEANUP,
    )?;
    measurements.record("chunks_cleanup_sql", started, 1);
    let key = format!("{prefix}header");
    let header = Header {
        version: 1,
        parser_version: CACHE_VERSION,
        ledger: {
            let started = measurements.start();
            let value = header_ledger(ledger);
            measurements.record("chunks_header_ledger", started, 1);
            value
        },
        count: chunks.hashes.len(),
        consumed_offset: Some(ledger.offset),
        fingerprint: {
            let started = measurements.start();
            let value = chunks.fingerprint();
            measurements.record("chunks_header_fingerprint", started, 1);
            value
        },
        pages_root,
        resume: chunks.resume.clone(),
    };
    let started = measurements.start();
    let encoded = json(&header)?;
    measurements.record("chunks_header_serialization", started, 1);
    measurements.count("chunks_header_bytes", encoded.len());
    let started = measurements.start();
    put.execute(params![key, &encoded])?;
    measurements.record("chunks_header_sql", started, 1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_proof_metadata_is_refused_without_mutation() {
        for (is_header, field, replacement, stage) in [
            (true, "pages_root", None, FailureStage::HeaderVersion),
            (
                true,
                "pages_root",
                Some(serde_json::Value::Null),
                FailureStage::HeaderVersion,
            ),
            (
                true,
                "version",
                Some(serde_json::json!(0)),
                FailureStage::HeaderVersion,
            ),
            (
                true,
                "version",
                Some(serde_json::json!(2)),
                FailureStage::HeaderVersion,
            ),
            (false, "sha256", None, FailureStage::PageHash),
            (
                false,
                "sha256",
                Some(serde_json::Value::Null),
                FailureStage::PageHash,
            ),
            (
                false,
                "version",
                Some(serde_json::json!(0)),
                FailureStage::PageMissing,
            ),
            (
                false,
                "version",
                Some(serde_json::json!(2)),
                FailureStage::PageMissing,
            ),
        ] {
            // Every case owns a fresh empty state root, never user data.
            let root = cleanup_root();
            let path = root.join("index.sqlite3");
            let mut store = cleanup_store(&root, "index.sqlite3");
            let bytes = vec![b'x'; crate::source_chunks::SIZE];
            let (ledger, mut index) = cleanup_proof(1, 0);
            index.source_chunks = Some(Arc::new(Chunks {
                offset: ledger.offset,
                hashes: vec![Sha256::digest(&bytes).into()],
                resume: None,
            }));
            let tx = store.connection.transaction().unwrap();
            write(&tx, "original", &ledger, index.source_chunks.as_deref()).unwrap();
            tx.commit().unwrap();
            let key = format!(
                "{}{}",
                prefix("original"),
                if is_header { "header" } else { "0000000000" }
            );
            let encoded: String = store
                .connection
                .query_row(NATIVE_METADATA_VALUE, [&key], |row| row.get(0))
                .unwrap();
            let mut value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            if is_header {
                assert_eq!(value["consumed_offset"], serde_json::json!(ledger.offset));
            }
            if let Some(replacement) = replacement {
                value[field] = replacement;
            } else {
                value.as_object_mut().unwrap().remove(field);
            }
            store
                .connection
                .execute(
                    "UPDATE meta SET value=?2 WHERE key=?1",
                    params![key, value.to_string()],
                )
                .unwrap();
            store
                .connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            let before = proof_metadata(&store.connection);
            let before_db = fs::read(&path).unwrap();
            let wal = root.join("index.sqlite3-wal");
            let before_wal = fs::read(&wal).unwrap();
            assert_eq!(
                header_ready(&store.connection, "original", &ledger).unwrap(),
                !is_header
            );
            assert!(
                read(&store.connection, "original", &ledger)
                    .unwrap()
                    .is_none()
            );
            let error = Focused::open(&path, "original", &ledger)
                .and_then(|reader| reader.verify(0, &bytes))
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(FailureStage::of(&error), Some(stage));
            assert_eq!(proof_metadata(&store.connection), before);
            assert_eq!(fs::read(&path).unwrap(), before_db);
            assert_eq!(fs::read(&wal).unwrap(), before_wal);
            let unexpected_proof_metadata: i64 = store
                .connection
                .query_row(
                    "SELECT count(*) FROM meta WHERE key GLOB 'native_proof_*' AND key NOT GLOB 'native_proof_tree:*'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(unexpected_proof_metadata, 0);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn proof_metadata(connection: &Connection) -> Vec<(String, Option<String>)> {
        connection
            .prepare("SELECT key,value FROM meta ORDER BY key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn header_mismatch_logging_is_limited_to_source_transitions() {
        let mut limiter = HeaderMismatchLimiter::default();
        assert!(limiter.allow("source", "catalog-l1", "proof-l2", "header_ledger", false));
        assert!(!limiter.allow("source", "catalog-l1", "proof-l2", "header_ledger", false));
        assert!(limiter.allow("source", "catalog-l1", "proof-l2", "header_ledger", true));
        assert!(!limiter.allow("source", "catalog-l1", "proof-l2", "header_ledger", false));
        assert!(!limiter.allow("source", "catalog-l1", "proof-l2", "header_ledger", true));
        assert!(limiter.allow("source", "catalog-l1", "proof-l3", "header_ledger", false));
        assert!(limiter.allow("source", "catalog-l1", "proof-l3", "header_parser", false));
        assert!(limiter.allow(
            "other-source",
            "catalog-l1",
            "proof-l3",
            "header_parser",
            false
        ));
    }

    #[test]
    fn header_mismatch_limiter_evicts_oldest_insertion_not_smallest_hash() {
        let mut limiter = HeaderMismatchLimiter::default();
        for index in 0..256 {
            let source = format!("source-{index:03}");
            assert!(limiter.allow(&source, "catalog", "proof", "header_ledger", false));
        }
        assert!(limiter.allow("source-new", "catalog", "proof", "header_ledger", false));
        assert!(!limiter.last.contains_key("source-000"));
        assert!(limiter.last.contains_key("source-255"));
        assert!(limiter.last.contains_key("source-new"));
    }

    #[test]
    fn focused_byte_io_failures_preserve_original_kind_and_add_safe_stage() {
        struct FailingIo {
            seek: bool,
        }
        impl io::Read for FailingIo {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "private read cause",
                ))
            }
        }
        impl io::Seek for FailingIo {
            fn seek(&mut self, _: io::SeekFrom) -> io::Result<u64> {
                if self.seek {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "private seek cause",
                    ))
                } else {
                    Ok(0)
                }
            }
        }
        let root = cleanup_root();
        let cache = root.join("index.sqlite3");
        let mut store = cleanup_store(&root, "index.sqlite3");
        let (ledger, index) = cleanup_proof(1, 0);
        let tx = store.connection.transaction().unwrap();
        write(&tx, "synthetic", &ledger, index.source_chunks.as_deref()).unwrap();
        tx.commit().unwrap();
        let focused = Focused::open(&cache, "synthetic", &ledger).unwrap();
        for (seek, expected) in [
            (true, io::ErrorKind::PermissionDenied),
            (false, io::ErrorKind::UnexpectedEof),
        ] {
            let error = focused.read(&mut FailingIo { seek }, 0).unwrap_err();
            assert_eq!(error.kind(), expected);
            assert_eq!(FailureStage::of(&error), Some(FailureStage::PageHash));
            assert!(!error.to_string().contains("private"));
        }
        drop(focused);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn focused_proof_work_is_independent_of_the_complete_vector_and_rechecks_pages() {
        let bytes = vec![b'x'; crate::source_chunks::SIZE];
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        for count in [1, 1024, 1025, 8192, 8193] {
            let root = cleanup_root();
            let path = root.join("index.sqlite3");
            let mut store = cleanup_store(&root, "index.sqlite3");
            let (ledger, mut index) = cleanup_proof(count, 0);
            index.source_chunks = Some(Arc::new(Chunks {
                offset: ledger.offset,
                hashes: vec![hash; count],
                resume: None,
            }));
            let tx = store.connection.transaction().unwrap();
            write(&tx, "original", &ledger, index.source_chunks.as_deref()).unwrap();
            tx.commit().unwrap();
            FOCUSED_HASHES.with(|n| n.set(0));
            FOCUSED_METADATA_BYTES.with(|n| n.set(0));
            let index_loads = crate::events::FILE_READS.with(std::cell::Cell::get);
            let focused = Focused::open(&path, "original", &ledger).unwrap();
            focused.verify(count - 1, &bytes).unwrap();
            focused.verify(count - 1, &bytes).unwrap();
            focused.clear_cache().unwrap();
            focused.verify(count - 1, &bytes).unwrap();
            let hashes = FOCUSED_HASHES.with(std::cell::Cell::get);
            let metadata = FOCUSED_METADATA_BYTES.with(std::cell::Cell::get);
            assert_eq!(
                crate::events::FILE_READS.with(std::cell::Cell::get),
                index_loads
            );
            assert!(hashes <= 2 * PAGE, "decoded full proof vector: {hashes}");
            assert!(metadata < 300000, "decoded proof metadata: {metadata}");
            eprintln!(
                "focused_proofs total_hashes={count} decoded_hashes={hashes} metadata_bytes={metadata} complete_index_loads=0"
            );
            let key = format!("{}{:010}", prefix("original"), (count - 1) / PAGE);
            store
                .connection
                .execute(
                    "UPDATE meta SET value=json_set(value,'$.hashes[0][0]',0) WHERE key=?1",
                    [&key],
                )
                .unwrap();
            // Even a self-consistent page checksum cannot change the proof
            // bound by the independently captured header root.
            let encoded: String = store
                .connection
                .query_row(NATIVE_METADATA_VALUE, [&key], |r| r.get(0))
                .unwrap();
            let mut part: Page = serde_json::from_str(&encoded).unwrap();
            let canonical = part.canonical().unwrap();
            part.sha256 = page_witness(&canonical);
            store
                .connection
                .execute(
                    "UPDATE meta SET value=?2 WHERE key=?1",
                    params![key, json(&part).unwrap()],
                )
                .unwrap();
            focused.clear_cache().unwrap();
            // This request retains the original committed witnesses. A new
            // request sees the modified proof and must reject its false root.
            focused.verify(count - 1, &bytes).unwrap();
            assert!(
                Focused::open(&path, "original", &ledger)
                    .unwrap()
                    .verify(count - 1, &bytes)
                    .is_err()
            );
            drop(focused);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn proof_reader_refuses_malformed_contract_without_metadata_writes() {
        let root = cleanup_root();
        let path = root.join("index.sqlite3");
        let mut store = cleanup_store(&root, "index.sqlite3");
        let (ledger, index) = cleanup_proof(1, 0);
        let tx = store.connection.transaction().unwrap();
        write(&tx, "original", &ledger, index.source_chunks.as_deref()).unwrap();
        tx.commit().unwrap();
        store
            .connection
            .execute("DROP TABLE reported_runs", [])
            .unwrap();
        let metadata = |connection: &Connection| {
            connection
                .prepare("SELECT key,value FROM meta ORDER BY key")
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let before = metadata(&store.connection);
        assert!(Focused::open(&path, "original", &ledger).is_err());
        assert_eq!(metadata(&store.connection), before);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn cleanup_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-chunks-cleanup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn published_metadata(ledger: &Ledger) -> String {
        serde_json::json!({
            "key":"owner","name":"Synthetic owner","harness":"codex","kind":"Codex",
            "native_ids":["owner"],"model":"unknown","tokens":[0,0,0],
            "cost":{"pricing_fingerprint":"","usd":null,"unpriced_models":[],
                "split_unknown_messages":0,"by_model":{},"by_day":{}},
            "sources":[{"path":"original","native_id":"owner", "dev":ledger.stat.dev,
                "ino":ledger.stat.ino,"size":ledger.stat.size,"modified_ns":ledger.stat.modified_ns,
                "changed_ns":ledger.stat.changed_ns,"offset":ledger.offset,
                "prefix_sha256":ledger.prefix,"tail_sha256":ledger.tail}]
        })
        .to_string()
    }

    fn proof_catalog_tables(connection: &Connection) {
        // Low-level proof tests use an in-memory meta table. Keep the tables
        // queried by published::preserve present, as they are after a real
        // Index open/migration, while leaving the reference set empty.
        connection.execute_batch(SCHEMA).unwrap();
        connection
            .execute_batch(crate::history_projection::SCHEMA)
            .unwrap();
    }

    #[test]
    fn oversized_published_owner_refuses_mutation_without_collecting_its_proofs() {
        let root = cleanup_root();
        let path = root.join("index.sqlite3");
        let mut store = cleanup_store(&root, "index.sqlite3");
        let (old, index) = cleanup_proof(1, 0);
        let tx = store.connection.transaction().unwrap();
        write(&tx, "original", &old, index.source_chunks.as_deref()).unwrap();
        tx.execute(
            "INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES('owner',0,'codex',?1)",
            [published_metadata(&old)],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES('owner','codex','owner','original')",
            [],
        )
        .unwrap();
        tx.commit().unwrap();
        let mut next = old.clone();
        next.stat.modified_ns += 1;
        let tx = store.connection.transaction().unwrap();
        write(&tx, "original", &next, index.source_chunks.as_deref()).unwrap();
        tx.commit().unwrap();
        let mut owner: serde_json::Value = serde_json::from_str(&published_metadata(&old)).unwrap();
        owner["name"] = serde_json::Value::String("x".repeat(1024 * 1024));
        store
            .connection
            .execute(
                "UPDATE session_catalog SET metadata=?1 WHERE session_key='owner'",
                [owner.to_string()],
            )
            .unwrap();
        let key = format!("{}header", prefix("original"));
        let before: String = store
            .connection
            .query_row(NATIVE_METADATA_VALUE, [&key], |row| row.get(0))
            .unwrap();
        next.stat.modified_ns += 1;
        let tx = store.connection.transaction().unwrap();
        assert!(write(&tx, "original", &next, index.source_chunks.as_deref()).is_err());
        drop(tx);
        let after: String = store
            .connection
            .query_row(NATIVE_METADATA_VALUE, [&key], |row| row.get(0))
            .unwrap();
        assert_eq!(
            before, after,
            "failed preservation must not advance the live proof"
        );
        assert!(Focused::open(&path, "original", &old).is_ok());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn published_proofs_follow_catalog_references_and_survive_gc_until_last_reader_drop() {
        let root = cleanup_root();
        let path = root.join("index.sqlite3");
        let mut store = cleanup_store(&root, "index.sqlite3");
        let bytes = vec![b'x'; crate::source_chunks::SIZE];
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let (old, mut index) = cleanup_proof(PAGE + 1, 0);
        index.source_chunks = Some(Arc::new(Chunks {
            offset: old.offset,
            hashes: vec![hash; PAGE + 1],
            resume: None,
        }));
        let tx = store.connection.transaction().unwrap();
        write(&tx, "original", &old, index.source_chunks.as_deref()).unwrap();
        tx.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES('owner',0,'codex',?1)", [published_metadata(&old)]).unwrap();
        tx.execute("INSERT INTO session_catalog_sources(session_key,harness,native_id,source_path) VALUES('owner','codex','owner','original')", []).unwrap();
        tx.execute(
            "INSERT INTO session_history_catalog(session_key,metadata) VALUES('owner',?1)",
            [published_metadata(&old)],
        )
        .unwrap();
        tx.commit().unwrap();
        let live_reader = Focused::open(&path, "original", &old).unwrap();
        let owned_snapshot = Arc::downgrade(&live_reader.db);
        let mut latest = old.clone();
        // Index leads an unchanged Catalog through several observations.
        for at in 1..=4 {
            latest.stat.modified_ns += 1;
            let tx = store.connection.transaction().unwrap();
            write(&tx, "original", &latest, index.source_chunks.as_deref()).unwrap();
            tx.commit().unwrap();
            let count: i64 = store
                .connection
                .query_row(
                    "SELECT count(*) FROM meta WHERE key LIKE 'native_published_chunks:%:header'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "unbounded retained versions at mutation {at}");
            Focused::open(&path, "original", &old)
                .unwrap()
                .verify(PAGE, &bytes)
                .unwrap();
        }
        let retained_reader = Focused::open(&path, "original", &old).unwrap();
        assert!(
            retained_reader
                .prefix
                .starts_with("native_published_chunks:")
        );
        // Both current and history references count. Publishing a different
        // Current alone must not collect the original retained observation.
        store
            .connection
            .execute(
                "UPDATE session_catalog SET metadata=?1 WHERE session_key='owner'",
                [published_metadata(&latest)],
            )
            .unwrap();
        let tx = store.connection.transaction().unwrap();
        write(&tx, "original", &latest, index.source_chunks.as_deref()).unwrap();
        tx.commit().unwrap();
        Focused::open(&path, "original", &old)
            .unwrap()
            .verify(PAGE, &bytes)
            .unwrap();
        store
            .connection
            .execute(
                "DELETE FROM session_history_catalog WHERE session_key='owner'",
                [],
            )
            .unwrap();
        latest.stat.modified_ns += 1;
        let tx = store.connection.transaction().unwrap();
        write(&tx, "original", &latest, index.source_chunks.as_deref()).unwrap();
        tx.commit().unwrap();
        assert!(Focused::open(&path, "original", &old).is_err());
        // Uncached current pages remain verifiable in each pinned snapshot
        // after the original publication has been collected.
        live_reader.verify(PAGE, &bytes).unwrap();
        retained_reader.verify(PAGE, &bytes).unwrap();
        let count: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'native_published_chunks:%:header'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        store
            .connection
            .busy_timeout(Duration::from_millis(10))
            .unwrap();
        let checkpoint = |db: &Connection| {
            db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
        };
        assert_eq!(checkpoint(&store.connection), 1, "request must pin WAL");
        drop(live_reader);
        assert!(owned_snapshot.upgrade().is_none());
        drop(retained_reader);
        assert_eq!(
            checkpoint(&store.connection),
            0,
            "last drop must release WAL"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parser_reset_does_not_prefer_obsolete_retained_proofs_over_rebuilt_live_proofs() {
        let root = cleanup_root();
        let path = root.join("index.sqlite3");
        let mut store = cleanup_store(&root, "index.sqlite3");
        let (ledger, index) = cleanup_proof(1, 0);
        let retained =
            published::prefix("original", &crate::source_carrier::ledger_digest(&ledger));
        let tx = store.connection.transaction().unwrap();
        write(&tx, "original", &ledger, index.source_chunks.as_deref()).unwrap();
        let live = format!("{}header", prefix("original"));
        tx.execute("INSERT INTO meta(key,value) SELECT ?1,json_set(value,'$.parser_version',?2) FROM meta WHERE key=?3", params![format!("{retained}header"),CACHE_VERSION-1,live]).unwrap();
        tx.commit().unwrap();
        let reader = Focused::open(&path, "original", &ledger).unwrap();
        assert_eq!(reader.prefix, prefix("original"));
        drop(reader);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn cleanup_store(root: &Path, name: &str) -> SqliteStore {
        match SqliteStore::attempt(&root.join(name)) {
            Ok((store, _)) => store,
            Err(error) => panic!("real Index schema: {error}"),
        }
    }

    fn cleanup_proof(count: usize, byte: u8) -> (Ledger, FileIndex) {
        let offset = count as u64 * crate::source_chunks::SIZE as u64;
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: offset,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let index = FileIndex {
            source_chunks: Some(Arc::new(Chunks {
                offset,
                hashes: vec![[byte; 32]; count],
                resume: None,
            })),
            ..FileIndex::default()
        };
        (ledger, index)
    }

    fn cleanup_publish(
        store: &mut SqliteStore,
        path: &str,
        ledger: &Ledger,
        index: &FileIndex,
        enabled: bool,
    ) -> rusqlite::Result<crate::native_timing::Measurements> {
        cleanup_publish_with(store, path, ledger, index, enabled, false)
    }

    fn cleanup_publish_with(
        store: &mut SqliteStore,
        path: &str,
        ledger: &Ledger,
        index: &FileIndex,
        enabled: bool,
        uncached_reference: bool,
    ) -> rusqlite::Result<crate::native_timing::Measurements> {
        use crate::native_timing::{Measurements, cleanup_tests as timing};
        let write = |tx: &Transaction<'_>, measurements: &mut Measurements| {
            write_file(
                tx,
                path,
                ledger,
                &super::super::super::DirtyRows::default(),
                index,
                measurements,
            )
        };
        if uncached_reference {
            assert!(!enabled);
            timing::publish_uncached_reference(&mut store.connection, write)
        } else {
            timing::publish(&mut store.connection, enabled, write)
        }
    }

    #[test]
    fn cleanup_diagnostics_preserve_parent_writer_proofs_and_exact_state() {
        use crate::native_timing::{CHUNKS_CLEANUP, cleanup_tests as timing};
        let _recording = timing::recording();
        let root = cleanup_root();
        // Clone one closed real initializer so the random stream epoch and all
        // authority metadata begin byte-identical rather than being normalized.
        drop(cleanup_store(&root, "seed.sqlite3"));
        for name in ["ordinary.sqlite3", "measured.sqlite3", "reference.sqlite3"] {
            fs::copy(root.join("seed.sqlite3"), root.join(name)).unwrap();
        }
        let mut ordinary = cleanup_store(&root, "ordinary.sqlite3");
        let mut measured = cleanup_store(&root, "measured.sqlite3");
        let mut reference = cleanup_store(&root, "reference.sqlite3");
        let path = "/fixture/parent[1]*.jsonl";
        let other = "/fixture/other-prefix.jsonl";
        let (ledger, index) = cleanup_proof(PAGE + 1, 1);
        for (store, uncached_reference) in [
            (&mut ordinary, false),
            (&mut measured, false),
            (&mut reference, true),
        ] {
            for path in [path, other] {
                cleanup_publish_with(store, path, &ledger, &index, false, uncached_reference)
                    .unwrap();
            }
            store.connection.execute_batch(&format!(
                "CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER preserve_header BEFORE DELETE ON meta WHEN OLD.key='{}header' BEGIN SELECT RAISE(ABORT,'chunk-header-exemption'); END;",
                prefix(path)
            )).unwrap();
        }
        assert_eq!(
            timing::snapshot(&ordinary.connection),
            timing::snapshot(&measured.connection)
        );
        for (count, changed) in [(PAGE + 1, 5), (PAGE, 6), (1, 5), (0, 6)] {
            for store in [&ordinary, &measured, &reference] {
                for suffix in [
                    "-1",
                    "00000000x0",
                    "0000000000bad",
                    "0000000002",
                    "header:extra",
                ] {
                    store
                        .connection
                        .execute(
                            "INSERT INTO meta VALUES(?1,'invalid')",
                            [format!("{}{suffix}", prefix(path))],
                        )
                        .unwrap();
                }
            }
            let (current, index) = cleanup_proof(count, 9);
            timing::reset_records();
            let disabled = cleanup_publish(&mut ordinary, path, &current, &index, false).unwrap();
            timing::assert_disabled(&disabled);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            timing::reset_records();
            let old =
                cleanup_publish_with(&mut reference, path, &current, &index, false, true).unwrap();
            timing::assert_uncached_reference(&old);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            let sample = cleanup_publish(&mut measured, path, &current, &index, true).unwrap();
            timing::assert_sample(&sample, &CHUNKS_CLEANUP, changed);
            assert_eq!(timing::status_reads(), 2);
            assert_eq!(timing::records().len(), 1);
            assert_eq!(
                timing::snapshot(&ordinary.connection),
                timing::snapshot(&measured.connection)
            );
            assert_eq!(
                timing::snapshot(&reference.connection),
                timing::snapshot(&measured.connection)
            );
            assert_eq!(
                digest(&reference.connection, path).unwrap(),
                digest(&measured.connection, path).unwrap()
            );
            for store in [&ordinary, &measured, &reference] {
                assert_eq!(
                    read(&store.connection, path, &current).unwrap(),
                    index.source_chunks
                );
                assert_eq!(
                    read(&store.connection, other, &ledger).unwrap(),
                    cleanup_proof(PAGE + 1, 1).1.source_chunks
                );
                assert_eq!(
                    ledger_row(&store.connection, path).unwrap().unwrap().1,
                    current
                );
                assert!(digest(&store.connection, path).unwrap().is_some());
            }
        }
        drop((ordinary, measured, reference));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_cleanup_rebinds_alternating_prefixes_and_reprepares_after_ddl() {
        use crate::native_timing::{CHUNKS_CLEANUP, cleanup_tests as timing};
        let _recording = timing::recording();
        let root = cleanup_root();
        drop(cleanup_store(&root, "seed.sqlite3"));
        for name in ["ordinary.sqlite3", "measured.sqlite3", "reference.sqlite3"] {
            fs::copy(root.join("seed.sqlite3"), root.join(name)).unwrap();
        }
        let mut ordinary = cleanup_store(&root, "ordinary.sqlite3");
        let mut measured = cleanup_store(&root, "measured.sqlite3");
        let mut reference = cleanup_store(&root, "reference.sqlite3");
        let paths = [
            "/fixture/alternating-a[1]*.jsonl",
            "/fixture/alternating-b.jsonl",
        ];
        let unrelated = "/fixture/malformed-other-prefix.jsonl";
        let (seed_ledger, seed_index) = cleanup_proof(PAGE + 1, 1);
        for (store, uncached) in [
            (&mut ordinary, false),
            (&mut measured, false),
            (&mut reference, true),
        ] {
            for path in paths.into_iter().chain([unrelated]) {
                cleanup_publish_with(store, path, &seed_ledger, &seed_index, false, uncached)
                    .unwrap();
            }
            store
                .connection
                .execute(
                    "INSERT INTO meta VALUES(?1,'untouched-malformed')",
                    [format!("{}bad", prefix(unrelated))],
                )
                .unwrap();
            // The real parent has returned the cleanup statement to its cache.
            // Actual main DDL and TEMP triggers now invalidate that warm SQL.
            store.connection.execute_batch(&format!(
                "CREATE INDEX cleanup_meta_value ON meta(value); CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER cleanup_audit AFTER DELETE ON meta BEGIN INSERT INTO audit VALUES(OLD.key); END; CREATE TEMP TRIGGER preserve_headers BEFORE DELETE ON meta WHEN OLD.key IN ('{}header','{}header') BEGIN SELECT RAISE(ABORT,'alternating-header-exemption'); END;",
                prefix(paths[0]), prefix(paths[1])
            )).unwrap();
        }
        let mut expected = [
            (seed_ledger.clone(), seed_index.clone()),
            (seed_ledger.clone(), seed_index.clone()),
        ];
        let mut audit_before = 0;
        let mut previous_digest = [
            digest(&reference.connection, paths[0]).unwrap(),
            digest(&reference.connection, paths[1]).unwrap(),
        ];
        for (step, (selected, count)) in [
            (0, PAGE + 1),
            (1, PAGE * 2 + 1),
            (0, 0),
            (1, 1),
            (0, PAGE + 1),
            (1, 0),
            (0, 1),
        ]
        .into_iter()
        .enumerate()
        {
            let path = paths[selected];
            for store in [&ordinary, &measured, &reference] {
                for suffix in ["bad", "0000000009"] {
                    store
                        .connection
                        .execute(
                            "INSERT INTO meta VALUES(?1,'invalid')",
                            [format!("{}{suffix}", prefix(path))],
                        )
                        .unwrap();
                }
                if step == 3 {
                    // Replace both main and TEMP schema objects after further
                    // reuse; the new trigger must run on the cached statement.
                    store.connection.execute_batch("DROP INDEX cleanup_meta_value; CREATE INDEX cleanup_meta_key_value ON meta(key,value); DROP TRIGGER temp.cleanup_audit; CREATE TEMP TRIGGER cleanup_audit AFTER DELETE ON meta BEGIN INSERT INTO audit VALUES('reprepared:' || OLD.key); END;").unwrap();
                }
            }
            let (current, index) = cleanup_proof(count, (step + 2) as u8);
            timing::reset_records();
            let old =
                cleanup_publish_with(&mut reference, path, &current, &index, false, true).unwrap();
            timing::assert_uncached_reference(&old);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            let disabled = cleanup_publish(&mut ordinary, path, &current, &index, false).unwrap();
            timing::assert_disabled(&disabled);
            assert_eq!(timing::status_reads(), 0);
            assert!(timing::records().is_empty());
            let sample = cleanup_publish(&mut measured, path, &current, &index, true).unwrap();
            assert_eq!(timing::status_reads(), 2);
            assert_eq!(timing::records().len(), 1);
            // Every operation deletes the two injected malformed/obsolete keys;
            // page shrink can add deletes witnessed by the real trigger.
            let audit_rows: i64 = measured
                .connection
                .query_row("SELECT count(*) FROM temp.audit WHERE value GLOB 'native_source_chunks:*' OR value GLOB 'reprepared:native_source_chunks:*'", [], |row| row.get(0))
                .unwrap();
            let audit_rows = usize::try_from(audit_rows).unwrap();
            assert!(audit_rows >= (step + 1) * 2);
            if step >= 3 {
                let recompiled_rows: i64 = measured
                    .connection
                    .query_row(
                        "SELECT count(*) FROM temp.audit WHERE value LIKE 'reprepared:%'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let recompiled_rows = usize::try_from(recompiled_rows).unwrap();
                assert!(recompiled_rows >= (step - 2) * 2);
            }
            timing::assert_sample(&sample, &CHUNKS_CLEANUP, audit_rows - audit_before);
            audit_before = audit_rows;
            for store in [&ordinary, &measured] {
                assert_eq!(
                    timing::snapshot(&store.connection),
                    timing::snapshot(&reference.connection)
                );
            }
            let old_ledger = expected[selected].0.clone();
            expected[selected] = (current.clone(), index);
            let current_digest = digest(&reference.connection, path).unwrap();
            assert!(current_digest.is_some());
            assert_ne!(current_digest, previous_digest[selected]);
            previous_digest[selected] = current_digest;
            for store in [&ordinary, &measured, &reference] {
                for (path, (ledger, index)) in paths.into_iter().zip(&expected) {
                    assert_eq!(
                        read(&store.connection, path, ledger).unwrap(),
                        index.source_chunks
                    );
                    assert_eq!(
                        ledger_row(&store.connection, path).unwrap().unwrap().1,
                        *ledger
                    );
                    assert_eq!(
                        digest(&store.connection, path).unwrap(),
                        digest(&reference.connection, path).unwrap()
                    );
                }
                if old_ledger != current {
                    assert!(
                        read(&store.connection, path, &old_ledger)
                            .unwrap()
                            .is_none()
                    );
                }
                assert!(
                    read(&store.connection, unrelated, &seed_ledger)
                        .unwrap()
                        .is_none()
                );
                let untouched: String = store
                    .connection
                    .query_row(
                        "SELECT value FROM meta WHERE key=?1",
                        [format!("{}bad", prefix(unrelated))],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(untouched, "untouched-malformed");
                assert_eq!(
                    ledger_row(&store.connection, unrelated).unwrap().unwrap().1,
                    seed_ledger
                );
            }
        }
        drop((ordinary, measured, reference));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_diagnostic_errors_roll_back_parent_pages_and_never_emit() {
        use crate::native_timing::cleanup_tests as timing;
        let _recording = timing::recording();
        let root = cleanup_root();
        let path = "/fixture/chunk-rollback.jsonl";
        let p = prefix(path);
        let (old, old_index) = cleanup_proof(PAGE + 1, 1);
        let (new, new_index) = cleanup_proof(1, 9);
        for (label, event, key) in [
            (
                "chunk-cleanup-after-page",
                "DELETE",
                format!("{p}0000000001"),
            ),
            ("chunk-header-after-page", "INSERT", format!("{p}header")),
        ] {
            let mut expected_error = None;
            for (enabled, uncached_reference) in [(false, false), (true, false), (false, true)] {
                let mut store = cleanup_store(
                    &root,
                    &format!("{label}-{enabled}-{uncached_reference}.sqlite3"),
                );
                cleanup_publish_with(
                    &mut store,
                    path,
                    &old,
                    &old_index,
                    false,
                    uncached_reference,
                )
                .unwrap();
                store.connection.execute_batch(&format!(
                    "CREATE TEMP TABLE audit(value TEXT); CREATE TEMP TRIGGER prior_page AFTER UPDATE ON meta WHEN NEW.key='{p}0000000000' BEGIN INSERT INTO audit VALUES('page-written'); END; CREATE TEMP TRIGGER cleanup_failure BEFORE {event} ON meta WHEN {}.key='{key}' AND EXISTS(SELECT 1 FROM audit) BEGIN SELECT RAISE(ABORT,'{label}'); END;",
                    if event == "DELETE" { "OLD" } else { "NEW" }
                )).unwrap();
                // Reuse after ABORT must reset the cached statement without
                // retaining partial pages, TEMP side effects or parameters.
                for _ in 0..2 {
                    let before = timing::snapshot(&store.connection);
                    timing::reset_records();
                    let error = match cleanup_publish_with(
                        &mut store,
                        path,
                        &new,
                        &new_index,
                        enabled,
                        uncached_reference,
                    ) {
                        Err(error) => error,
                        Ok(_) => panic!("named failure was not reached"),
                    };
                    assert!(error.to_string().contains(label), "{error}");
                    let actual_error = error.to_string();
                    if let Some(expected) = &expected_error {
                        assert_eq!(&actual_error, expected);
                    } else {
                        expected_error = Some(actual_error);
                    }
                    assert_eq!(timing::snapshot(&store.connection), before);
                    assert!(timing::records().is_empty());
                    assert_eq!(timing::status_reads(), if enabled { 2 } else { 0 });
                    assert_eq!(
                        read(&store.connection, path, &old).unwrap(),
                        old_index.source_chunks
                    );
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paged_proofs_round_trip_and_refuse_corruption_or_a_different_ledger() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            .unwrap();
        proof_catalog_tables(&connection);
        let path = "/fixture/parent[1]*.jsonl";
        let chunks = Chunks {
            offset: crate::source_chunks::SIZE as u64 * 1024 + 19,
            hashes: vec![[42; 32]; 1025],
            resume: None,
        };
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: chunks.offset,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset: chunks.offset,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let transaction = connection.transaction().unwrap();
        write(&transaction, path, &ledger, Some(&chunks)).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            read(&connection, path, &ledger).unwrap().unwrap().as_ref(),
            &chunks
        );
        let max: i64 = connection
            .query_row(
                "SELECT max(length(CAST(value AS BLOB))) FROM meta",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(max < NATIVE_METADATA_MAX as i64);
        let mut changed = ledger.clone();
        changed.stat.modified_ns += 1;
        assert!(read(&connection, path, &changed).unwrap().is_none());
        let key = format!("{}0000000000", prefix(path));
        let encoded: String = connection
            .query_row("SELECT value FROM meta WHERE key=?1", [&key], |row| {
                row.get(0)
            })
            .unwrap();
        let mut page: Page = serde_json::from_str(&encoded).unwrap();
        page.hashes[0][0] ^= 1;
        connection
            .execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                params![&key, json(&page).unwrap()],
            )
            .unwrap();
        assert!(read(&connection, path, &ledger).unwrap().is_none());
        let transaction = connection.transaction().unwrap();
        write(&transaction, path, &ledger, Some(&chunks)).unwrap();
        transaction.commit().unwrap();
        assert!(read(&connection, path, &ledger).unwrap().is_some());
        connection
            .execute(
                "DELETE FROM meta WHERE key=?1",
                [format!("{}0000000001", prefix(path))],
            )
            .unwrap();
        assert!(read(&connection, path, &ledger).unwrap().is_none());
    }

    #[test]
    fn shrinking_or_removing_a_proof_cleans_only_its_exact_namespace() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            .unwrap();
        proof_catalog_tables(&connection);
        connection
            .execute("INSERT INTO meta VALUES ('reported_runs','retained')", [])
            .unwrap();
        let chunks = Chunks {
            offset: crate::source_chunks::SIZE as u64 * 1025,
            hashes: vec![[1; 32]; 1025],
            resume: None,
        };
        let mut ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: chunks.offset,
                modified_ns: 3,
                changed_ns: Some(4),
            },
            offset: chunks.offset,
            prefix: [5; 32],
            tail: [6; 32],
        };
        let transaction = connection.transaction().unwrap();
        for path in ["one[?]", "two*"] {
            write(&transaction, path, &ledger, Some(&chunks)).unwrap();
        }
        transaction.commit().unwrap();
        for suffix in [
            "-1",
            "00000000x0",
            "0000000000bad",
            "0000000002",
            "header:extra",
        ] {
            connection
                .execute(
                    "INSERT INTO meta VALUES (?1,'invalid')",
                    [format!("{}{suffix}", prefix("one[?]"))],
                )
                .unwrap();
        }
        assert!(read(&connection, "one[?]", &ledger).unwrap().is_none());
        let short = Chunks {
            offset: 19,
            hashes: vec![[2; 32]],
            resume: None,
        };
        ledger.offset = short.offset;
        ledger.stat.size = short.offset;
        let transaction = connection.transaction().unwrap();
        write(&transaction, "one[?]", &ledger, Some(&short)).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            read(&connection, "one[?]", &ledger)
                .unwrap()
                .unwrap()
                .as_ref(),
            &short
        );
        let transaction = connection.transaction().unwrap();
        clear(&transaction, "one[?]").unwrap();
        transaction.commit().unwrap();
        assert!(read(&connection, "one[?]", &ledger).unwrap().is_none());
        let count: i64 = connection
            .query_row("SELECT count(*) FROM meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 7,
            "the other proof's two pages/header, three proof-tree nodes and observed runs survive"
        );
    }
}
