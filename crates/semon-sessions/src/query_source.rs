//! Generation-bound local reads for shared session derivation.
//! Source proofs contain no host credentials or runtime authority.
use crate::{
    Options, SessionSourceRange, SessionSourceReader, SessionSourceRef,
    events::{Ledger, Stat},
    inputs::InputRoot,
    model::summary::CatalogSource,
    sealed::LogFile,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct Proof {
    source: Option<SessionSourceRef>,
    ledger: Ledger,
    generation: String,
    chunks: ProofChunks,
}

enum ProofChunks {
    Complete(Arc<crate::source_chunks::Chunks>),
    Focused(Box<crate::events::store::chunk_proofs::Focused>),
}
impl ProofChunks {
    fn read(&self, file: &mut (impl Read + Seek), index: usize) -> io::Result<Vec<u8>> {
        match self {
            Self::Complete(chunks) => chunks.read(file, index),
            Self::Focused(chunks) => chunks.read(file, index),
        }
    }
    fn verify(&self, index: usize, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Complete(chunks) => chunks.verify(index, bytes),
            Self::Focused(chunks) => chunks.verify(index, bytes),
        }
    }
}

pub(crate) struct Sources {
    proofs: BTreeMap<PathBuf, Proof>,
    paths: BTreeMap<(String, String), PathBuf>,
    diagnostic_scope: PathBuf,
    #[cfg(feature = "test-hooks")]
    cache: PathBuf,
}

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "native query source generation unavailable",
    )
}

fn note_focused_source_interest(scope: &Path, source: &CatalogSource) {
    crate::source_carrier::note_diagnostic_interest(
        scope,
        &source.path.to_string_lossy(),
        &crate::source_carrier::ledger(source),
    );
}

// Cold checkpoints describe a complete prefix of a larger, unchanged physical
// generation. Size is the only synthetic field; both descriptor clocks and
// inode identity still fence the observation. Unknown clocks retain hashing.
fn same_physical_generation(ledger: &Ledger, physical: &Stat) -> bool {
    physical.changed_ns.is_some()
        && (physical == &ledger.stat
            || (ledger.stat.size == ledger.offset
                && physical.size >= ledger.stat.size
                && Stat {
                    size: ledger.stat.size,
                    ..*physical
                } == ledger.stat))
}

impl Sources {
    pub(crate) fn new(options: &Options) -> Self {
        Self {
            proofs: BTreeMap::new(),
            paths: BTreeMap::new(),
            diagnostic_scope: crate::events::EventCache::path(&options.cache),
            #[cfg(feature = "test-hooks")]
            cache: options.cache.clone(),
        }
    }

    pub(crate) fn capture(
        options: &Options,
        sources: impl Iterator<Item = (CatalogSource, Option<Arc<crate::source_chunks::Chunks>>)>,
    ) -> io::Result<Self> {
        Self::capture_inner(
            options,
            sources.map(|(source, chunks)| (source, chunks.map(ProofChunks::Complete))),
            true,
        )
    }

    /// Local native paths can have no lossless public source-reference encoding.
    /// Keep their verified evidence without admitting a lossy alias for requests.
    pub(crate) fn capture_native(
        options: &Options,
        sources: impl Iterator<Item = (CatalogSource, Option<Arc<crate::source_chunks::Chunks>>)>,
    ) -> io::Result<Self> {
        let cache_path = crate::events::EventCache::path(&options.cache);
        let mut db = None;
        let mut proofs = Vec::new();
        for (source, chunks) in sources {
            let chunks = if let Some(chunks) = chunks {
                ProofChunks::Complete(chunks)
            } else {
                if db.is_none() {
                    let binding = crate::storage::local_navigation_binding(options, "")
                        .map_err(io::Error::other)?;
                    let c = crate::events::store::focused_connection(&binding)?;
                    c.busy_timeout(std::time::Duration::from_millis(100))
                        .map_err(io::Error::other)?;
                    db = Some(crate::events::store::chunk_proofs::Snapshot::new(
                        c, binding,
                    ));
                }
                let ledger = Ledger {
                    stat: Stat {
                        dev: source.dev,
                        ino: source.ino,
                        size: source.size,
                        modified_ns: source.modified_ns,
                        changed_ns: source.changed_ns,
                    },
                    offset: source.offset,
                    prefix: source.prefix_sha256,
                    tail: source.tail_sha256,
                };
                let focused = crate::events::store::chunk_proofs::Focused::with_db(
                    db.as_ref().expect("proof connection opened").clone(),
                    &cache_path,
                    // This is the private Index key used by EventCache::tree,
                    // not a public source reference. The captured source below
                    // retains the exact native PathBuf and physical ledger.
                    &source.path.to_string_lossy(),
                    &ledger,
                )?;
                ProofChunks::Focused(Box::new(focused))
            };
            proofs.push((source, Some(chunks)));
        }
        Self::capture_inner(options, proofs.into_iter(), false)
    }

    fn capture_inner(
        options: &Options,
        sources: impl Iterator<Item = (CatalogSource, Option<ProofChunks>)>,
        require_references: bool,
    ) -> io::Result<Self> {
        let mut captured = Self::new(options);
        for (source, chunks) in sources {
            let chunks = chunks
                .filter(|chunks| match chunks {
                    ProofChunks::Complete(chunks) => chunks.valid(source.offset),
                    ProofChunks::Focused(_) => true,
                })
                .ok_or_else(unavailable)?;
            if ![InputRoot::Claude, InputRoot::Codex, InputRoot::Copilot]
                .into_iter()
                .any(|root| source.path.starts_with(root.home(options)))
            {
                return Err(unavailable());
            }
            let reference = [InputRoot::Claude, InputRoot::Codex, InputRoot::Copilot]
                .into_iter()
                .find_map(|root| {
                    let relative = source
                        .path
                        .strip_prefix(root.home(options))
                        .ok()?
                        .to_str()?;
                    // These paths came from native discovery, not a request.
                    // Exact captured references are the only admitted reads;
                    // do not narrow the local CLI's existing discovery rules.
                    Some(SessionSourceRef {
                        root: root.as_str().to_owned(),
                        path: relative.to_owned(),
                        native_id: source.native_id.clone(),
                        offset: source.offset,
                        prefix_sha256: source.prefix_sha256,
                        tail_sha256: source.tail_sha256,
                    })
                });
            if require_references && reference.is_none() {
                return Err(unavailable());
            }
            let generation = source
                .prefix_sha256
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let proof = Proof {
                source: reference,
                ledger: Ledger {
                    stat: Stat {
                        dev: source.dev,
                        ino: source.ino,
                        size: source.size,
                        modified_ns: source.modified_ns,
                        changed_ns: source.changed_ns,
                    },
                    offset: source.offset,
                    prefix: source.prefix_sha256,
                    tail: source.tail_sha256,
                },
                generation,
                chunks,
            };
            if let Some(reference) = &proof.source {
                captured.paths.insert(
                    (reference.root.clone(), reference.path.clone()),
                    source.path.clone(),
                );
            }
            captured.proofs.insert(source.path, proof);
        }
        Ok(captured)
    }

    #[cfg(test)]
    pub(crate) fn capture_focused(
        options: &Options,
        sources: &[CatalogSource],
    ) -> io::Result<Self> {
        if sources.len() > 64 {
            return Err(unavailable());
        }
        let cache_path = crate::events::EventCache::path(&options.cache);
        let db = crate::storage::local_navigation_binding(options, "")
            .map_err(io::Error::other)
            .and_then(|binding| crate::events::store::focused_connection(&binding))
            .map_err(|error| {
                for source in sources.iter().take(16) {
                    note_focused_source_interest(&cache_path, source);
                }
                crate::source_carrier::FailureStage::ProofOpenDb.wrap(error)
            })?;
        db.busy_timeout(std::time::Duration::from_millis(100))
            .map_err(|error| {
                crate::source_carrier::FailureStage::ProofOpenDb.wrap(io::Error::other(error))
            })?;
        Self::capture_focused_snapshot(options, sources, db)
    }

    /// Take ownership of the connection which selected the Catalog recipes.
    /// Its open read transaction pins even proof generations collected between
    /// selection and the first lazy proof read.
    pub(crate) fn capture_focused_snapshot(
        options: &Options,
        sources: &[CatalogSource],
        db: rusqlite::Connection,
    ) -> io::Result<Self> {
        if sources.len() > 64 {
            return Err(unavailable());
        }
        let cache_path = crate::events::EventCache::path(&options.cache);
        let binding = crate::storage::local_navigation_binding(options, "").map_err(|error| {
            crate::source_carrier::FailureStage::ProofOpenDb.wrap(io::Error::other(error))
        })?;
        binding
            .validate(&db, &binding.config().namespace)
            .map_err(|error| {
                crate::source_carrier::FailureStage::ProofOpenDb.wrap(io::Error::other(error))
            })?;
        let db = crate::events::store::chunk_proofs::Snapshot::new(db, binding);
        #[cfg(test)]
        if let Some(hook) = BEFORE_FOCUSED_CAPTURE.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
        let proofs = sources
            .iter()
            .map(|source| {
                let ledger = Ledger {
                    stat: Stat {
                        dev: source.dev,
                        ino: source.ino,
                        size: source.size,
                        modified_ns: source.modified_ns,
                        changed_ns: source.changed_ns,
                    },
                    offset: source.offset,
                    prefix: source.prefix_sha256,
                    tail: source.tail_sha256,
                };
                let proof = crate::events::store::chunk_proofs::Focused::with_db(
                    db.clone(),
                    &cache_path,
                    source.path.to_str().ok_or_else(unavailable)?,
                    &ledger,
                )
                .inspect_err(|error| {
                    if crate::source_carrier::FailureStage::of(error).is_some() {
                        note_focused_source_interest(&cache_path, source);
                    }
                })?;
                Ok((source.clone(), Some(ProofChunks::Focused(Box::new(proof)))))
            })
            .collect::<io::Result<Vec<_>>>()?;
        Self::capture_inner(options, proofs.into_iter(), false)
    }

    /// Private owned generation binding. Coordinates come exclusively from
    /// the admitted store; Options retains only existing Raw-root authority.
    pub(crate) fn capture_bound_snapshot(
        options: &Options,
        sources: &[CatalogSource],
        db: rusqlite::Connection,
        owner: &crate::events::store::configured::NativeOwnership,
    ) -> io::Result<Self> {
        if sources.is_empty() || sources.len() > 64 {
            return Err(unavailable());
        }
        owner.validate_ready(&db).map_err(io::Error::other)?;
        let config = owner.config();
        let database = config.backend.sqlite_database().map_err(io::Error::other)?;
        let db = crate::events::store::chunk_proofs::Snapshot::new(
            db,
            crate::storage::NativeReaderBinding::Semantic(owner.clone()),
        );
        #[cfg(test)]
        if let Some(hook) = BEFORE_FOCUSED_CAPTURE.with(|hook| hook.borrow_mut().take()) {
            hook();
        }
        let mut proofs = Vec::with_capacity(sources.len());
        for source in sources {
            let proof = crate::events::store::chunk_proofs::Focused::with_db(
                db.clone(),
                database,
                source.path.to_str().ok_or_else(unavailable)?,
                &crate::source_carrier::ledger(source),
            )?;
            proofs.push((source.clone(), Some(ProofChunks::Focused(Box::new(proof)))));
        }
        let mut captured = Self::capture_inner(options, proofs.into_iter(), false)?;
        captured.diagnostic_scope = database.clone();
        Ok(captured)
    }

    pub(crate) fn reader(&self) -> Records<'_> {
        self.reader_with(None)
    }

    pub(crate) fn reader_with<'a>(
        &'a self,
        provider: Option<(&'a dyn SessionSourceReader, crate::SessionSourceReadMode)>,
    ) -> Records<'a> {
        self.reader_with_mode(
            provider.map(|value| value.0),
            provider.map_or(crate::SessionSourceReadMode::LocalThenProvider, |value| {
                value.1
            }),
        )
    }

    pub(crate) fn reader_with_mode<'a>(
        &'a self,
        provider: Option<&'a dyn SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
    ) -> Records<'a> {
        Records {
            sources: self,
            provider,
            mode,
            remote: Mutex::default(),
            checked: Mutex::default(),
            checked_chunks: Mutex::default(),
            checked_evidence: Mutex::default(),
            prepared: Mutex::default(),
            failed: Mutex::new(false),
            revision: Mutex::new(None),
            failure_stage: Mutex::new(None),
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static BEFORE_FOCUSED_CAPTURE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// The verification cache belongs to one read, never the shared query cohort.
pub(crate) struct Records<'a> {
    sources: &'a Sources,
    provider: Option<&'a dyn SessionSourceReader>,
    mode: crate::SessionSourceReadMode,
    remote: Mutex<BTreeMap<PathBuf, (String, u64)>>,
    checked: Mutex<BTreeMap<PathBuf, Stat>>,
    checked_chunks: Mutex<BTreeMap<PathBuf, BTreeSet<usize>>>,
    checked_evidence: Mutex<BTreeMap<PathBuf, BTreeSet<crate::native_record::Record>>>,
    // Native reads share at most one handle and verified block per request.
    // Only scalar preparation reuses bytes; ordinary ranges verify the current
    // block on every read. Completion reopens and checks every used source.
    prepared: Mutex<Option<PreparedBlock>>,
    failed: Mutex<bool>,
    revision: Mutex<Option<crate::source_carrier::RevisionReason>>,
    failure_stage: Mutex<Option<crate::source_carrier::FailureStage>>,
}

struct PreparedBlock {
    path: PathBuf,
    index: usize,
    bytes: Arc<[u8]>,
    // One request-owned native handle avoids walking the secure directory chain
    // for every scalar in this block. Completion reopens the named path and
    // verifies every consumed block before any projection can be published.
    file: Option<LogFile>,
}

impl Records<'_> {
    // Preserve the first physical revision failure even when a presentation
    // parser tolerates a missing record. No partial answer may hide this fence.
    fn reject_revision(&self, reason: crate::source_carrier::RevisionReason) -> io::Error {
        if let Ok(mut revision) = self.revision.lock() {
            revision.get_or_insert(reason);
        }
        reason.error()
    }

    fn recheck(&self, path: &Path, file: &LogFile, before: &Stat) -> io::Result<()> {
        crate::source_carrier::recheck(path, file, before).map_err(|_| {
            let reason = file
                .metadata()
                .ok()
                .and_then(|metadata| Stat::from_metadata(&metadata).ok())
                .and_then(|after| crate::source_carrier::mismatch(before, &after))
                .unwrap_or(crate::source_carrier::RevisionReason::Identity);
            self.reject_revision(reason)
        })
    }

    fn proof_failure(&self, path: &Path, error: io::Error) -> io::Error {
        if let Some(stage) = crate::source_carrier::FailureStage::of(&error) {
            if let Ok(mut first) = self.failure_stage.lock() {
                first.get_or_insert(stage);
            }
            if let Some(proof) = self.sources.proofs.get(path) {
                crate::source_carrier::note_diagnostic_interest(
                    &self.sources.diagnostic_scope,
                    &path.to_string_lossy(),
                    &proof.ledger,
                );
            }
        }
        error
    }

    pub(crate) fn query_error(&self, error: impl std::fmt::Display) -> crate::QueryError {
        if let Some(stage) = self.failure_stage.lock().ok().and_then(|stage| *stage) {
            return crate::QueryError::source_failure(stage);
        }
        match self.revision.lock().ok().and_then(|revision| *revision) {
            Some(reason) => crate::QueryError::local_revision(reason),
            None => crate::QueryError::new("io", error.to_string()),
        }
    }

    fn pinned_provider(&self, path: &Path) -> io::Result<bool> {
        Ok(matches!(
            self.mode,
            crate::SessionSourceReadMode::LocalOrPinnedProvider
        ) && self
            .remote
            .lock()
            .map_err(|_| unavailable())?
            .contains_key(path))
    }

    fn may_fallback(&self, path: &Path) -> io::Result<bool> {
        Ok(!matches!(
            self.mode,
            crate::SessionSourceReadMode::LocalOrPinnedProvider
        ) || !self
            .checked
            .lock()
            .map_err(|_| unavailable())?
            .contains_key(path))
    }

    pub(crate) fn source_ref(&self, path: &Path) -> Option<&SessionSourceRef> {
        self.sources.proofs.get(path)?.source.as_ref()
    }
    /// Read only the indexed decisive record. Unlike transcript offsets, a
    /// marker may be valid JSON at EOF before its terminating newline arrives.
    pub(crate) fn marker(
        &self,
        path: &Path,
        index: &crate::native_marker::Index,
    ) -> io::Result<Option<crate::Marker>> {
        let result = (|| {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            if !index.valid(&proof.ledger) {
                return Err(unavailable());
            }
            self.verify(path, proof)?;
            let Some(record) = index.record() else {
                return Ok(None);
            };
            crate::native_marker::decode(&self.evidence(path, record)?)
        })();
        if result.is_err() {
            *self.failed.lock().map_err(|_| unavailable())? = true;
        }
        result
    }

    /// Private indexed evidence can include a valid EOF record; its digest and
    /// source observation are separate from public complete-line range offsets.
    pub(crate) fn evidence(
        &self,
        path: &Path,
        record: &crate::native_record::Record,
    ) -> io::Result<Vec<u8>> {
        self.with_evidence(path, record, |file| record.read(file))
    }

    pub(crate) fn verify_evidence(
        &self,
        path: &Path,
        record: &crate::native_record::Record,
    ) -> io::Result<()> {
        self.with_evidence(path, record, |file| record.verify(file))
    }

    fn with_evidence<T>(
        &self,
        path: &Path,
        record: &crate::native_record::Record,
        read: impl FnOnce(&mut LogFile) -> io::Result<T>,
    ) -> io::Result<T> {
        let result = (|| {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            if record
                .end()
                .is_none_or(|end| end > proof.ledger.stat.size.max(proof.ledger.offset))
            {
                return Err(unavailable());
            }
            self.verify(path, proof)?;
            let mut file = LogFile::open(path)?;
            let before = Stat::from_metadata(&file.metadata()?)?;
            let qualified = matches!(
                self.mode,
                crate::SessionSourceReadMode::LocalOrPinnedProvider
            );
            if (!qualified
                && (before.dev != proof.ledger.stat.dev || before.ino != proof.ledger.stat.ino))
                || before.size < proof.ledger.offset
                || record.end().is_none_or(|end| end > before.size)
                || (qualified
                    && self.checked.lock().map_err(|_| unavailable())?.get(path) != Some(&before))
            {
                return Err(unavailable());
            }
            let value = read(&mut file)?;
            if qualified {
                self.recheck(path, &file, &before)?;
            }
            if Stat::from_metadata(&file.metadata()?)? != before {
                return Err(unavailable());
            }
            self.checked_evidence
                .lock()
                .map_err(|_| unavailable())?
                .entry(path.to_owned())
                .or_default()
                .insert(record.clone());
            Ok(value)
        })();
        if result.is_err() {
            *self.failed.lock().map_err(|_| unavailable())? = true;
        }
        result
    }
    /// Read a native launch's allowlisted fields and fail the entire observation
    /// on either an evidence or decoded-location mismatch.
    pub(crate) fn launch(
        &self,
        path: &Path,
        call: &crate::native_handoff::CallRef,
    ) -> io::Result<crate::handoff::Call> {
        let result = (|| {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            if !call.valid(&proof.ledger, false) && !call.valid(&proof.ledger, true) {
                return Err(unavailable());
            }
            let checked = self
                .checked_evidence
                .lock()
                .map_err(|_| unavailable())?
                .get(path)
                .is_some_and(|records| records.contains(call.record()));
            if !checked {
                self.verify_evidence(path, call.record())?;
            }
            call.read(path, self)
        })();
        if result.is_err() {
            *self.failed.lock().map_err(|_| unavailable())? = true;
        }
        result
    }

    /// Native scalar-recipe preparation keeps its original bounded complete-line
    /// rule while borrowing the same source proof as the other native reads.
    pub(crate) fn record_bytes(
        &self,
        path: &Path,
        offset: u64,
        max: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
        if offset >= proof.ledger.offset {
            return Ok(None);
        }
        let mut reader = BufReader::with_capacity(
            crate::source_chunks::SIZE,
            RangeFile {
                records: self,
                proof,
                path,
                position: offset,
                prepared: true,
            },
        )
        .take(max + 1);
        let mut bytes = Vec::new();
        reader.read_until(b'\n', &mut bytes)?;
        if bytes.len() as u64 > max || bytes.last() != Some(&b'\n') {
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    /// Derivation preserves the shared producer's 64 MiB native-record limit.
    /// Its short text cap is applied by Texts after the unchanged extractor.
    pub(crate) fn derive(&self, path: &Path, offset: u64) -> io::Result<Option<serde_json::Value>> {
        let result = (|| {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            if offset >= proof.ledger.offset {
                return Err(unavailable());
            }
            let mut reader = BufReader::with_capacity(
                crate::source_chunks::SIZE,
                RangeFile {
                    records: self,
                    proof,
                    path,
                    position: offset,
                    prepared: true,
                },
            )
            .take(crate::model::MAX_LINE);
            let mut bytes = Vec::new();
            reader.read_until(b'\n', &mut bytes)?;
            Ok(serde_json::from_slice(&bytes).ok())
        })();
        if result.is_err() {
            *self.failed.lock().map_err(|_| unavailable())? = true;
        }
        result
    }

    /// Metadata and cached derivations also belong to the captured cohort.
    /// Fence every contributing source before deriving, then finish rechecks
    /// these same sources before any query projection can be published.
    pub(crate) fn verify_all(&self) -> io::Result<()> {
        for (path, proof) in &self.sources.proofs {
            self.verify(path, proof)?;
        }
        Ok(())
    }

    fn verify(&self, path: &Path, proof: &Proof) -> io::Result<()> {
        if self.provider.is_some()
            && (matches!(self.mode, crate::SessionSourceReadMode::ProviderOnly)
                || self.pinned_provider(path)?)
        {
            self.provider_block(path, proof, 0)?;
            return Ok(());
        }
        let mut file = match LogFile::open(path) {
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && self.provider.is_some()
                    && self.may_fallback(path)? =>
            {
                self.provider_block(path, proof, 0)?;
                return Ok(());
            }
            result => result?,
        };
        let before = Stat::from_metadata(&file.metadata()?)?;
        let mut checked = self.checked.lock().map_err(|_| unavailable())?;
        if let Some(prior) = checked.get(path)
            && matches!(
                self.mode,
                crate::SessionSourceReadMode::LocalOrPinnedProvider
            )
            && prior != &before
        {
            return Err(self.reject_revision(
                crate::source_carrier::mismatch(prior, &before)
                    .unwrap_or(crate::source_carrier::RevisionReason::MissingChangeToken),
            ));
        }
        let unchanged = before.changed_ns.is_some()
            && (same_physical_generation(&proof.ledger, &before)
                || checked.get(path) == Some(&before));
        if matches!(
            self.mode,
            crate::SessionSourceReadMode::LocalOrPinnedProvider
        ) {
            if before.size < proof.ledger.offset {
                return Err(self.reject_revision(crate::source_carrier::RevisionReason::SizeShrank));
            }
            if !unchanged && proof.ledger.offset != 0 {
                proof
                    .chunks
                    .read(&mut file, 0)
                    .map_err(|error| self.proof_failure(path, error))?;
                self.checked_chunks
                    .lock()
                    .map_err(|_| unavailable())?
                    .entry(path.to_owned())
                    .or_default()
                    .insert(0);
            }
            self.recheck(path, &file, &before)?;
        } else if (!unchanged && !proof.ledger.resumes(&before, &mut file))
            || Stat::from_metadata(&file.metadata()?)? != before
        {
            return Err(unavailable());
        }
        checked.insert(path.to_owned(), before);
        Ok(())
    }
    pub(crate) fn read(
        &self,
        path: &Path,
        offset: u64,
    ) -> io::Result<(Option<serde_json::Value>, u64)> {
        let result = (|| {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            crate::source_reader::read_native_record(
                self,
                proof.source.as_ref().ok_or_else(unavailable)?,
                &proof.generation,
                offset,
            )
        })();
        if result.is_err() {
            *self.failed.lock().map_err(|_| unavailable())? = true;
        }
        result
    }

    pub(crate) fn prompt(
        &self,
        file: &crate::model::SlotFile,
        offset: u64,
    ) -> Option<(serde_json::Value, u64)> {
        let Some(proof) = self.sources.proofs.get(&file.path) else {
            *self.failed.lock().ok()? = true;
            return None;
        };
        file.prompts.record_from(
            &mut RangeFile {
                records: self,
                proof,
                path: &file.path,
                position: offset,
                prepared: false,
            },
            offset,
        )
    }

    /// Recheck touched sources before returning an answer, including native
    /// records served from the pager's short-lived line cache.
    pub(crate) fn finish(&self) -> io::Result<()> {
        if let Some(reason) = *self.revision.lock().map_err(|_| unavailable())? {
            return Err(reason.error());
        }
        if *self.failed.lock().map_err(|_| unavailable())? {
            return Err(unavailable());
        }
        for (path, proof) in &self.sources.proofs {
            if let ProofChunks::Focused(chunks) = &proof.chunks {
                chunks
                    .clear_cache()
                    .map_err(|error| self.proof_failure(path, error))?;
            }
        }
        let paths: Vec<_> = self
            .checked
            .lock()
            .map_err(|_| unavailable())?
            .keys()
            .cloned()
            .collect();
        for path in paths {
            let proof = self.sources.proofs.get(&path).ok_or_else(unavailable)?;
            self.verify(&path, proof)?;
        }
        let touched = self
            .checked_chunks
            .lock()
            .map_err(|_| unavailable())?
            .clone();
        for (path, indices) in &touched {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            let remote = self
                .remote
                .lock()
                .map_err(|_| unavailable())?
                .contains_key(path);
            if remote {
                // Cache reuse still crosses the host's current authority fence.
                for index in indices {
                    self.provider_block(path, proof, *index)?;
                }
                continue;
            }
            let mut file = LogFile::open(path)?;
            let before = Stat::from_metadata(&file.metadata()?)?;
            let qualified = matches!(
                self.mode,
                crate::SessionSourceReadMode::LocalOrPinnedProvider
            );
            if before.size < proof.ledger.offset
                || (!qualified
                    && (before.dev != proof.ledger.stat.dev || before.ino != proof.ledger.stat.ino))
                || (qualified
                    && self.checked.lock().map_err(|_| unavailable())?.get(path) != Some(&before))
            {
                return Err(unavailable());
            }
            for index in indices {
                proof
                    .chunks
                    .read(&mut file, *index)
                    .map_err(|error| self.proof_failure(path, error))?;
            }
            if qualified {
                self.recheck(path, &file, &before)?;
            }
            if Stat::from_metadata(&file.metadata()?)? != before {
                return Err(unavailable());
            }
        }
        for (path, records) in self
            .checked_evidence
            .lock()
            .map_err(|_| unavailable())?
            .iter()
        {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            let mut file = LogFile::open(path)?;
            let before = Stat::from_metadata(&file.metadata()?)?;
            let qualified = matches!(
                self.mode,
                crate::SessionSourceReadMode::LocalOrPinnedProvider
            );
            if (!qualified
                && (before.dev != proof.ledger.stat.dev || before.ino != proof.ledger.stat.ino))
                || (qualified
                    && self.checked.lock().map_err(|_| unavailable())?.get(path) != Some(&before))
            {
                return Err(unavailable());
            }
            for record in records {
                record.verify(&mut file)?;
            }
            if qualified {
                self.recheck(path, &file, &before)?;
            }
            if Stat::from_metadata(&file.metadata()?)? != before {
                return Err(unavailable());
            }
        }
        Ok(())
    }
    fn range(
        &self,
        source: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
        prepare: bool,
    ) -> io::Result<SessionSourceRange> {
        let path = self
            .sources
            .paths
            .get(&(source.root.clone(), source.path.clone()))
            .ok_or_else(unavailable)?;
        let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
        if proof.source.as_ref() != Some(source)
            || expected.is_some_and(|value| value != proof.generation)
        {
            return Err(unavailable());
        }
        self.range_native(path, proof, offset, max, prepare)
    }

    fn range_native(
        &self,
        path: &Path,
        proof: &Proof,
        offset: u64,
        max: usize,
        prepare: bool,
    ) -> io::Result<SessionSourceRange> {
        if max > crate::SESSION_SOURCE_CHUNK_MAX
            || max == 0
            || offset
                .checked_add(max as u64)
                .is_none_or(|end| end > proof.ledger.offset)
        {
            return Err(unavailable());
        }
        if self.provider.is_some()
            && (matches!(self.mode, crate::SessionSourceReadMode::ProviderOnly)
                || self.pinned_provider(path)?)
        {
            return self.provider_range(path, proof, offset, max, prepare);
        }
        let mut prepared = Some(self.prepared.lock().map_err(|_| unavailable())?);
        let cached_file = prepared
            .as_deref_mut()
            .and_then(Option::as_mut)
            .filter(|block| block.path == path)
            .and_then(|block| block.file.take());
        let reused = cached_file.is_some();
        let mut file = match cached_file.map(Ok).unwrap_or_else(|| LogFile::open(path)) {
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && self.provider.is_some()
                    && self.may_fallback(path)? =>
            {
                drop(prepared);
                return self.provider_range(path, proof, offset, max, prepare);
            }
            result => result?,
        };
        let before = Stat::from_metadata(&file.metadata()?)?;
        if reused {
            let named = match std::fs::symlink_metadata(path) {
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && self.provider.is_some()
                        && self.may_fallback(path)? =>
                {
                    drop(prepared);
                    return self.provider_range(path, proof, offset, max, prepare);
                }
                result => result?,
            };
            if !named.is_file() || Stat::from_metadata(&named)? != before {
                return Err(unavailable());
            }
        }
        let qualified = matches!(
            self.mode,
            crate::SessionSourceReadMode::LocalOrPinnedProvider
        );
        if qualified && before.size < proof.ledger.offset {
            return Err(self.reject_revision(crate::source_carrier::RevisionReason::SizeShrank));
        }
        if before.size < proof.ledger.offset
            || (!qualified
                && (before.dev != proof.ledger.stat.dev || before.ino != proof.ledger.stat.ino))
        {
            return Err(unavailable());
        }
        let mut checked = self.checked.lock().map_err(|_| unavailable())?;
        if qualified
            && let Some(prior) = checked.get(path)
            && prior != &before
        {
            return Err(self.reject_revision(
                crate::source_carrier::mismatch(prior, &before)
                    .unwrap_or(crate::source_carrier::RevisionReason::MissingChangeToken),
            ));
        }
        let unchanged = before.changed_ns.is_some()
            && (same_physical_generation(&proof.ledger, &before)
                || checked.get(path) == Some(&before));
        if !qualified && !unchanged && !proof.ledger.resumes(&before, &mut file) {
            return Err(unavailable());
        }
        let first = usize::try_from(offset / crate::source_chunks::SIZE as u64)
            .map_err(|_| unavailable())?;
        let end = offset + max as u64;
        let last = usize::try_from((end - 1) / crate::source_chunks::SIZE as u64)
            .map_err(|_| unavailable())?;
        let mut bytes = Vec::with_capacity(max);
        for index in first..=last {
            let part = if let Some(block) = prepared
                .as_deref()
                .and_then(Option::as_ref)
                .filter(|block| prepare && block.path == path && block.index == index)
            {
                block.bytes.clone()
            } else {
                let part: Arc<[u8]> = Arc::from(
                    proof
                        .chunks
                        .read(&mut file, index)
                        .map_err(|error| self.proof_failure(path, error))?,
                );
                if let Some(prepared) = prepared.as_deref_mut() {
                    *prepared = Some(PreparedBlock {
                        path: path.to_owned(),
                        index,
                        bytes: part.clone(),
                        file: None,
                    });
                }
                part
            };
            let start = index as u64 * crate::source_chunks::SIZE as u64;
            let lo = offset.saturating_sub(start) as usize;
            let hi = (end - start).min(part.len() as u64) as usize;
            bytes.extend_from_slice(&part[lo..hi]);
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::run_cache(&self.sources.cache, "query_source_after_read");
        if qualified {
            self.recheck(path, &file, &before)?;
        }
        if Stat::from_metadata(&file.metadata()?)? != before
            || (!qualified
                && before.changed_ns.is_none()
                && !proof.ledger.resumes(&before, &mut file))
        {
            return Err(unavailable());
        }
        checked.insert(path.to_owned(), before);
        if let Some(block) = prepared.as_deref_mut().and_then(Option::as_mut) {
            block.file = Some(file);
        }
        self.checked_chunks
            .lock()
            .map_err(|_| unavailable())?
            .entry(path.to_owned())
            .or_default()
            .extend(first..=last);
        Ok(SessionSourceRange {
            generation: proof.generation.clone(),
            // The cohort exposes its complete consumed prefix, even if the
            // native writer has appended more bytes since observation.
            length: proof.ledger.offset,
            offset,
            bytes: Arc::from(bytes),
            cached: true,
        })
    }

    fn provider_block(&self, path: &Path, proof: &Proof, index: usize) -> io::Result<Arc<[u8]>> {
        let source = proof.source.as_ref().ok_or_else(unavailable)?;
        let provider = self.provider.ok_or_else(unavailable)?;
        let start = (index as u64)
            .checked_mul(crate::source_chunks::SIZE as u64)
            .filter(|start| *start < proof.ledger.offset)
            .ok_or_else(unavailable)?;
        let len = (proof.ledger.offset - start).min(crate::source_chunks::SIZE as u64) as usize;
        let mut bytes = Vec::with_capacity(len);
        // A provider's independently verified generation can use another name
        // than the native prefix digest. Pin that name and length per request,
        // while every consumed byte still proves the original indexed block.
        while bytes.len() < len {
            let expected = self
                .remote
                .lock()
                .map_err(|_| unavailable())?
                .get(path)
                .cloned();
            let offset = start + bytes.len() as u64;
            let max = len - bytes.len();
            let part = provider.read_range(
                source,
                expected.as_ref().map(|v| v.0.as_str()),
                offset,
                max,
            )?;
            if part.offset != offset
                || part.bytes.is_empty()
                || part.bytes.len() > max
                || part.length < proof.ledger.offset
                || part.generation.is_empty()
                || part.generation.len() > 4096
                || expected
                    .as_ref()
                    .is_some_and(|v| v.0 != part.generation || v.1 != part.length)
            {
                return Err(unavailable());
            }
            let mut remote = self.remote.lock().map_err(|_| unavailable())?;
            if let Some(prior) = remote.get(path)
                && (prior.0 != part.generation || prior.1 != part.length)
            {
                return Err(unavailable());
            }
            remote.insert(path.to_owned(), (part.generation, part.length));
            bytes.extend_from_slice(&part.bytes);
        }
        proof
            .chunks
            .verify(index, &bytes)
            .map_err(|error| self.proof_failure(path, error))?;
        self.checked_chunks
            .lock()
            .map_err(|_| unavailable())?
            .entry(path.to_owned())
            .or_default()
            .insert(index);
        Ok(Arc::from(bytes))
    }

    fn provider_range(
        &self,
        path: &Path,
        proof: &Proof,
        offset: u64,
        max: usize,
        prepare: bool,
    ) -> io::Result<SessionSourceRange> {
        let end = offset + max as u64;
        let first = (offset / crate::source_chunks::SIZE as u64) as usize;
        let last = ((end - 1) / crate::source_chunks::SIZE as u64) as usize;
        let mut bytes = Vec::with_capacity(max);
        let mut prepared = prepare
            .then(|| self.prepared.lock().map_err(|_| unavailable()))
            .transpose()?;
        for index in first..=last {
            let part = if let Some(block) = prepared
                .as_deref()
                .and_then(Option::as_ref)
                .filter(|block| block.path == path && block.index == index)
            {
                block.bytes.clone()
            } else {
                let part = self.provider_block(path, proof, index)?;
                if let Some(prepared) = prepared.as_deref_mut() {
                    *prepared = Some(PreparedBlock {
                        path: path.to_owned(),
                        index,
                        bytes: part.clone(),
                        file: None,
                    });
                }
                part
            };
            let start = index as u64 * crate::source_chunks::SIZE as u64;
            bytes.extend_from_slice(
                &part[offset.saturating_sub(start) as usize
                    ..(end - start).min(part.len() as u64) as usize],
            );
        }
        Ok(SessionSourceRange {
            generation: proof.generation.clone(),
            length: proof.ledger.offset,
            offset,
            bytes: Arc::from(bytes),
            cached: true,
        })
    }
}

impl SessionSourceReader for Records<'_> {
    fn read_range(
        &self,
        source: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<SessionSourceRange> {
        let result = self.range(source, expected, offset, max, false);
        if let Err(error) = &result {
            if let Some(stage) = crate::source_carrier::FailureStage::of(error) {
                self.failure_stage
                    .lock()
                    .map_err(|_| unavailable())?
                    .get_or_insert(stage);
            }
            *self.failed.lock().map_err(|_| unavailable())? = true;
        }
        result
    }
}

/// A native prompt's existing parser can skip cached image strings while all
/// bytes still pass through the generation-bound consumed-prefix reader.
struct RangeFile<'a, 'b> {
    records: &'a Records<'b>,
    proof: &'b Proof,
    path: &'a Path,
    position: u64,
    prepared: bool,
}

impl Read for RangeFile<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut max = buffer
            .len()
            .min(crate::SESSION_SOURCE_CHUNK_MAX)
            .min(usize::try_from(self.proof.ledger.offset - self.position).unwrap_or(usize::MAX));
        if self.prepared {
            max = max.min(
                crate::source_chunks::SIZE
                    - (self.position % crate::source_chunks::SIZE as u64) as usize,
            );
        }
        if max == 0 {
            return Ok(0);
        }
        let result =
            self.records
                .range_native(self.path, self.proof, self.position, max, self.prepared);
        if result.is_err() {
            *self.records.failed.lock().map_err(|_| unavailable())? = true;
        }
        let part = result?;
        buffer[..part.bytes.len()].copy_from_slice(&part.bytes);
        self.position += part.bytes.len() as u64;
        Ok(part.bytes.len())
    }
}

impl Seek for RangeFile<'_, '_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self.proof.ledger.offset.checked_add_signed(offset),
        }
        .filter(|position| *position <= self.proof.ledger.offset);
        let Some(position) = position else {
            *self.records.failed.lock().map_err(|_| unavailable())? = true;
            return Err(unavailable());
        };
        self.position = position;
        Ok(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;

    struct Fixture {
        root: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("semon-native-{name}-{}", std::process::id()));
            fs::create_dir(&root).unwrap();
            Self {
                path: root.join("lead.jsonl"),
                root,
            }
        }
        fn sources(&self) -> (Sources, SessionSourceRef, String) {
            self.sources_at(fs::metadata(&self.path).unwrap().len())
        }
        fn sources_at(&self, consumed: u64) -> (Sources, SessionSourceRef, String) {
            let stat = Stat::from_metadata(&fs::metadata(&self.path).unwrap()).unwrap();
            let captured =
                crate::source_chunks::capture(&mut LogFile::open(&self.path).unwrap(), consumed, 0)
                    .unwrap();
            let reference = SessionSourceRef {
                root: "claude".into(),
                path: "lead.jsonl".into(),
                native_id: "lead".into(),
                offset: captured.chunks.offset,
                prefix_sha256: captured.prefix,
                tail_sha256: captured.tail,
            };
            let generation: String = captured
                .prefix
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let mut sources = Sources::new(&Options {
                cache: self.root.join("index.json"),
                ..Options::default()
            });
            sources.paths.insert(
                (reference.root.clone(), reference.path.clone()),
                self.path.clone(),
            );
            sources.proofs.insert(
                self.path.clone(),
                Proof {
                    source: Some(reference.clone()),
                    ledger: Ledger {
                        stat,
                        offset: reference.offset,
                        prefix: captured.prefix,
                        tail: captured.tail,
                    },
                    generation: generation.clone(),
                    chunks: ProofChunks::Complete(Arc::new(captured.chunks)),
                },
            );
            (sources, reference, generation)
        }
        fn rewrite(&self, bytes: &[u8]) -> Stat {
            let before = fs::metadata(&self.path).unwrap();
            fs::write(&self.path, bytes).unwrap();
            fs::File::options()
                .write(true)
                .open(&self.path)
                .unwrap()
                .set_modified(before.modified().unwrap())
                .unwrap();
            let after = Stat::from_metadata(&fs::metadata(&self.path).unwrap()).unwrap();
            assert_eq!(
                Stat::from_metadata(&before).unwrap(),
                after,
                "every captured stamp field must match"
            );
            after
        }
    }

    fn admitted_writer_connection(options: &Options) -> Connection {
        let config = options.source.config(&options.cache).unwrap();
        crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection()
    }

    #[test]
    fn index_l2_with_selected_l1_and_stable_raw_reports_header_ledger() {
        use std::io::Write;
        let fixture = Fixture::new("index-leading-catalog");
        fs::write(
            &fixture.path,
            b"{\"type\":\"user\",\"message\":{\"content\":\"original\"}}\n",
        )
        .unwrap();
        let options = Options {
            claude_home: fixture.root.clone(),
            cache: fixture.root.join("index.json"),
            ..Options::default()
        };
        let mut cache =
            crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
        crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
        let selected = cache.source(&fixture.path, "lead").unwrap();
        let l1_offset = selected.offset;
        let captured = Sources::capture_focused(&options, std::slice::from_ref(&selected)).unwrap();
        let publish_scope = crate::events::EventCache::path(&options.cache);
        assert_eq!(captured.diagnostic_scope, publish_scope);
        let selected_digest =
            crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(&selected));
        fs::OpenOptions::new()
            .append(true)
            .open(&fixture.path)
            .unwrap()
            .write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"append\"}}\n")
            .unwrap();
        crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
        let l2_offset = cache.source(&fixture.path, "lead").unwrap().offset;
        assert!(l2_offset > l1_offset);
        let before = Stat::from_metadata(&fs::metadata(&fixture.path).unwrap()).unwrap();
        let error = match Sources::capture_focused(&options, &[selected]) {
            Ok(_) => panic!("L2 must expose the pre-repair proof lifetime gap"),
            Err(error) => error,
        };
        assert_eq!(
            crate::source_carrier::FailureStage::of(&error),
            Some(crate::source_carrier::FailureStage::HeaderLedger)
        );
        let reply = crate::QueryError::io(&error).to_json();
        assert_eq!(reply["stage"], "header_ledger");
        let interests = crate::source_carrier::diagnostic_publish_interests(&publish_scope);
        let interest = interests
            .iter()
            .find(|interest| interest.path.as_str() == fixture.path.to_string_lossy().as_ref())
            .expect("focused failure must register an interest for the publish scope");
        assert_eq!(
            interest.scope.as_str(),
            publish_scope.to_string_lossy().as_ref()
        );
        assert_eq!(interest.catalog_ledger_digest, selected_digest);
        assert_eq!(interest.catalog_offset, l1_offset);
        assert_eq!(
            before,
            Stat::from_metadata(&fs::metadata(&fixture.path).unwrap()).unwrap()
        );
    }

    #[test]
    fn focused_proof_store_schema_mismatch_has_its_own_stage() {
        let fixture = Fixture::new("proof-store-schema-stage");
        fs::write(
            &fixture.path,
            b"{\"type\":\"user\",\"message\":{\"content\":\"synthetic\"}}\n",
        )
        .unwrap();
        let options = Options {
            claude_home: fixture.root.clone(),
            cache: fixture.root.join("index.json"),
            ..Options::default()
        };
        let mut cache =
            crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
        crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
        let selected = cache.source(&fixture.path, "lead").unwrap();
        let selected_digest =
            crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(&selected));
        let selected_offset = selected.offset;
        let publish_scope = crate::events::EventCache::path(&options.cache);
        drop(cache);

        crate::events::store::chunk_proofs::HEADER_DIAGNOSTICS
            .with(|lines| lines.borrow_mut().clear());
        let db = Connection::open(crate::events::EventCache::path(&options.cache)).unwrap();
        db.pragma_update(None, "user_version", 0).unwrap();
        drop(db);
        let error = match Sources::capture_focused(&options, &[selected]) {
            Ok(_) => panic!("a schema-mismatched proof store must be refused"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            crate::source_carrier::FailureStage::of(&error),
            Some(crate::source_carrier::FailureStage::ProofOpenDb)
        );
        let logs = crate::events::store::chunk_proofs::HEADER_DIAGNOSTICS
            .with(|lines| lines.borrow().clone());
        assert!(logs.is_empty(), "the proof header was never opened");
        assert!(
            !error
                .to_string()
                .contains(fixture.path.to_string_lossy().as_ref())
        );
        let interests = crate::source_carrier::diagnostic_publish_interests(&publish_scope);
        let interest = interests
            .iter()
            .find(|interest| interest.path.as_str() == fixture.path.to_string_lossy().as_ref())
            .expect("a database-open failure must retain the selected source interest");
        assert_eq!(
            interest.scope.as_str(),
            publish_scope.to_string_lossy().as_ref()
        );
        assert_eq!(interest.catalog_ledger_digest, selected_digest);
        assert_eq!(interest.catalog_offset, selected_offset);
    }

    #[test]
    fn focused_header_parse_rejection_logs_independent_raw_stats() {
        let fixture = Fixture::new("proof-header-parse-diagnostic");
        fs::write(
            &fixture.path,
            b"{\"type\":\"user\",\"message\":{\"content\":\"synthetic\"}}\n",
        )
        .unwrap();
        let options = Options {
            claude_home: fixture.root.clone(),
            cache: fixture.root.join("index.json"),
            ..Options::default()
        };
        let mut cache =
            crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
        crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
        let selected = cache.source(&fixture.path, "lead").unwrap();
        let ledger = crate::source_carrier::ledger(&selected);
        let before = fs::metadata(&fixture.path).unwrap().len();
        drop(cache);

        let db = admitted_writer_connection(&options);
        assert_eq!(
            db.execute(
                "UPDATE main.meta SET value='{' \
                 WHERE namespace=semon_namespace() \
                   AND key GLOB 'native_source_chunks:*:header'",
                [],
            )
            .unwrap(),
            1,
            "corrupt exactly one header in the admitted source namespace"
        );
        drop(db);
        crate::events::store::chunk_proofs::HEADER_DIAGNOSTICS
            .with(|lines| lines.borrow_mut().clear());
        // MCP opens without touching Raw; Web later opens the same selected
        // ledger/cache with Raw observations. That upgrade must not be deduped.
        let mcp_error = match Sources::capture_focused(&options, std::slice::from_ref(&selected)) {
            Ok(_) => panic!("a malformed proof header must be rejected"),
            Err(error) => error,
        };
        assert_eq!(
            crate::source_carrier::FailureStage::of(&mcp_error),
            Some(crate::source_carrier::FailureStage::HeaderVersion)
        );
        let result = crate::events::store::chunk_proofs::Focused::open_with_raw(
            &crate::storage::local_navigation_binding(&options, "").unwrap(),
            &crate::events::EventCache::path(&options.cache),
            fixture.path.to_str().unwrap(),
            &ledger,
            Some(before),
        );
        let error = match result {
            Ok(_) => panic!("a malformed proof header must be rejected"),
            Err(error) => error,
        };
        assert_eq!(
            crate::source_carrier::FailureStage::of(&error),
            Some(crate::source_carrier::FailureStage::HeaderVersion)
        );
        let logs = crate::events::store::chunk_proofs::HEADER_DIAGNOSTICS
            .with(|lines| lines.borrow().clone());
        assert_eq!(logs.len(), 2);
        assert!(logs[0].contains("raw=not_collected"));
        assert!(logs[1].contains("stage=header_version"));
        assert!(logs[1].contains(&format!("raw_before={before}")));
        assert!(logs[1].contains(&format!("raw_after=Some({before})")));
        assert!(logs[1].contains("sample=independent_statements"));
        for line in &logs {
            assert!(!line.contains(fixture.path.to_string_lossy().as_ref()));
        }
        // Neither another MCP open nor another Web open emits a downgrade or
        // a second upgrade for the same failure transition.
        assert!(Sources::capture_focused(&options, std::slice::from_ref(&selected)).is_err());
        assert!(
            crate::events::store::chunk_proofs::Focused::open_with_raw(
                &crate::storage::local_navigation_binding(&options, "").unwrap(),
                &crate::events::EventCache::path(&options.cache),
                fixture.path.to_str().unwrap(),
                &ledger,
                Some(before),
            )
            .is_err()
        );
        assert_eq!(
            crate::events::store::chunk_proofs::HEADER_DIAGNOSTICS
                .with(|lines| lines.borrow().len()),
            2
        );
    }

    #[test]
    fn records_query_error_keeps_unstaged_messages_and_sanitizes_staged_failures() {
        let sources = Sources::new(&Options::default());
        let records = sources.reader();
        assert_eq!(
            records.query_error("original caller message").to_json(),
            serde_json::json!({"error":{"code":"io","message":"original caller message"}})
        );
        let original = crate::source_carrier::FailureStage::PageHash
            .wrap(io::Error::other("private provider cause"));
        let error = records.proof_failure(Path::new("synthetic-source"), original);
        assert_eq!(
            crate::source_carrier::FailureStage::of(&error),
            Some(crate::source_carrier::FailureStage::PageHash)
        );
        assert_eq!(
            records.query_error("private caller message").to_json(),
            serde_json::json!({"error":{"code":"io","message":"Original native transcript proofs are unavailable"},
                "stage":"page_hash"})
        );
    }

    #[test]
    fn local_only_proofs_qualify_changed_carriers_with_bounded_reads() {
        use std::io::Write;
        for mutation in ["append", "copy", "touch", "legacy", "rewrite", "truncate"] {
            let fixture = Fixture::new(&format!("proof-carrier-{mutation}"));
            let original = vec![b'x'; 3 * 1024 * 1024];
            fs::write(&fixture.path, &original).unwrap();
            let (mut sources, reference, generation) = fixture.sources();
            match mutation {
                "append" => fs::OpenOptions::new()
                    .append(true)
                    .open(&fixture.path)
                    .unwrap()
                    .write_all(b"tail")
                    .unwrap(),
                "copy" => {
                    fs::rename(&fixture.path, fixture.path.with_extension("old")).unwrap();
                    fs::write(&fixture.path, &original).unwrap();
                }
                "touch" => fs::File::options()
                    .write(true)
                    .open(&fixture.path)
                    .unwrap()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
                    .unwrap(),
                "legacy" => {
                    sources
                        .proofs
                        .get_mut(&fixture.path)
                        .unwrap()
                        .ledger
                        .stat
                        .changed_ns = None
                }
                "rewrite" => {
                    let mut changed = original.clone();
                    changed[17] = b'y';
                    fs::write(&fixture.path, changed).unwrap();
                }
                "truncate" => fs::write(&fixture.path, b"short").unwrap(),
                _ => unreachable!(),
            }
            let records =
                sources.reader_with_mode(None, crate::SessionSourceReadMode::LocalOrPinnedProvider);
            crate::source_chunks::READ_BYTES.with(|n| n.set(0));
            crate::events::HASHED_BYTES.with(|n| n.set(0));
            let result = records.read_range(&reference, Some(&generation), 17, 1);
            if matches!(mutation, "rewrite" | "truncate") {
                assert!(result.is_err(), "{mutation}");
            } else {
                assert_eq!(&*result.unwrap().bytes, b"x", "{mutation}");
                records.finish().unwrap();
                assert_eq!(
                    crate::source_chunks::READ_BYTES.with(std::cell::Cell::get),
                    2 * crate::source_chunks::SIZE as u64
                );
                // The first accepted carrier remains fixed across chunks.
                fs::OpenOptions::new()
                    .append(true)
                    .open(&fixture.path)
                    .unwrap()
                    .write_all(b"race")
                    .unwrap();
                assert!(
                    records
                        .read_range(&reference, Some(&generation), 18, 1)
                        .is_err()
                );
                assert!(records.finish().is_err());
                let rejected = records
                    .query_error("presentation omitted a failed record")
                    .to_json();
                assert_eq!(rejected["error"]["code"], "stale_projection");
                assert_eq!(rejected["status"], 409);
                assert_eq!(rejected["fence_reason"], "local_source_revision");
                assert_eq!(rejected["local_revision_reason"], "size_grew");
                assert!(rejected.get("entries").is_none());
            }
            assert!(
                records
                    .read_range(&reference, Some("wrong"), 17, 1)
                    .is_err()
            );
            assert_eq!(crate::events::HASHED_BYTES.with(std::cell::Cell::get), 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_native_path_reads_never_admit_a_lossy_public_alias() {
        use std::os::unix::ffi::OsStringExt;
        let mut fixture = Fixture::new("private-path-reference");
        fs::write(&fixture.path, b"{\"content\":\"native evidence\"}\n").unwrap();
        let path = fixture
            .root
            .join(std::ffi::OsString::from_vec(b"lead-\xff.jsonl".to_vec()));
        fs::rename(&fixture.path, &path).unwrap();
        fixture.path = path;
        let (old, mut reference, _) = fixture.sources();
        let proof = old.proofs.get(&fixture.path).unwrap();
        let stat = proof.ledger.stat;
        let source = CatalogSource {
            path: fixture.path.clone(),
            native_id: "lead".into(),
            native_role: None,
            dev: stat.dev,
            ino: stat.ino,
            size: stat.size,
            modified_ns: stat.modified_ns,
            changed_ns: stat.changed_ns,
            offset: proof.ledger.offset,
            prefix_sha256: proof.ledger.prefix,
            tail_sha256: proof.ledger.tail,
        };
        let options = Options {
            source: crate::Options::default().source,
            claude_home: fixture.root.clone(),
            claude_json: fixture.root.join(".claude.json"),
            codex_home: fixture.root.join("codex"),
            copilot_home: fixture.root.join("copilot"),
            proc_root: fixture.root.join("proc"),
            cache: fixture.root.join("index.json"),
            all: true,
            since: std::time::Duration::from_secs(86400),
            session: None,
            facts: None,
            scan_window: false,
        };
        let inputs = || {
            std::iter::once((
                source.clone(),
                Some(match &proof.chunks {
                    ProofChunks::Complete(chunks) => chunks.clone(),
                    ProofChunks::Focused(_) => panic!("fixture requires complete proofs"),
                }),
            ))
        };
        assert!(Sources::capture(&options, inputs()).is_err());
        let sources = Sources::capture_native(&options, inputs()).unwrap();
        assert!(sources.paths.is_empty());
        let reader = sources.reader();
        assert_eq!(
            reader.derive(&fixture.path, 0).unwrap().unwrap()["content"],
            "native evidence"
        );
        reader.finish().unwrap();
        reference.path = fixture
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(reader.read_range(&reference, None, 0, 1).is_err());
        assert!(reader.finish().is_err());
        let mut outside = options;
        outside.claude_home = fixture.root.join("other-root");
        assert!(Sources::capture_native(&outside, inputs()).is_err());
    }

    #[test]
    fn parent_marker_read_and_completion_verify_identical_stat_bytes_including_eof() {
        let _clock = crate::events::fixed_change_time_for_test(0);
        for eof in [false, true] {
            let fixture = Fixture::new(if eof {
                "marker-eof-proof"
            } else {
                "marker-line-proof"
            });
            let record = serde_json::json!({"type":"user","message":{"content":"Semon-Parent: claude:parent\nSemon-Handoff: /private/OLD"}});
            fs::write(
                &fixture.path,
                format!("{record}{}", if eof { "" } else { "\n" }),
            )
            .unwrap();
            let mut cache = crate::events::EventCache::open_scoped(
                &fixture.root.join("index.json"),
                &crate::Options::default().source,
            )
            .unwrap();
            let index =
                crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
            let consumed = cache.revision(&fixture.path).unwrap().2;
            assert_eq!(consumed == 0, eof);
            let (sources, _, _) = fixture.sources_at(consumed);
            let reader = sources.reader();
            let markers = index.marker_index.as_ref().unwrap();
            assert_eq!(
                reader
                    .marker(&fixture.path, markers)
                    .unwrap()
                    .unwrap()
                    .handoff
                    .as_deref(),
                Some("/private/OLD")
            );
            reader.finish().unwrap();
            let replacement = fs::read_to_string(&fixture.path)
                .unwrap()
                .replace("/private/OLD", "/private/NEW");
            fixture.rewrite(replacement.as_bytes());
            assert!(reader.finish().is_err());
            let fresh = sources.reader();
            assert!(fresh.marker(&fixture.path, markers).is_err());
            assert!(fresh.finish().is_err());
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn native_handoff_checks_record_context_and_completion_with_identical_stats_including_eof() {
        let _clock = crate::events::fixed_change_time_for_test(0);
        for eof in [false, true] {
            for (old, new) in [
                ("/private/OLD", "/private/NEW"),
                ("00:00:00Z", "00:00:01Z"),
                ("tool_use", "tool_xse"),
            ] {
                let fixture = Fixture::new("launch-proof");
                let record = serde_json::json!({"timestamp":"2026-09-24T00:00:00Z","message":{"content":[{"type":"tool_use","id":"launch","name":"Bash","input":{"command":"codex < /private/OLD"}}]}});
                fs::write(
                    &fixture.path,
                    format!("{record}{}", if eof { "" } else { "\n" }),
                )
                .unwrap();
                let mut cache = crate::events::EventCache::open_scoped(
                    &fixture.root.join("index.json"),
                    &crate::Options::default().source,
                )
                .unwrap();
                let index =
                    crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false)
                        .unwrap();
                let consumed = cache.revision(&fixture.path).unwrap().2;
                let (sources, _, _) = fixture.sources_at(consumed);
                let reader = sources.reader();
                let calls = index.handoff_index.as_ref().unwrap().calls();
                assert_eq!(calls.len(), 1);
                assert_eq!(
                    reader.launch(&fixture.path, calls[0]).unwrap().tool.id,
                    "launch"
                );
                reader.finish().unwrap();
                let replacement = fs::read_to_string(&fixture.path).unwrap().replace(old, new);
                fixture.rewrite(replacement.as_bytes());
                assert!(reader.finish().is_err());
                let fresh = sources.reader();
                assert!(fresh.launch(&fixture.path, calls[0]).is_err());
                assert!(fresh.finish().is_err());
            }
        }
    }

    #[test]
    fn native_handoff_partial_and_valid_eof_are_reconsidered_and_indexed_once_on_newline() {
        let fixture = Fixture::new("launch-eof-append");
        let value = serde_json::json!({"message":{"content":[{"type":"tool_use","id":"launch","name":"Bash","input":{"command":"codex exec"}}]}});
        let bytes = value.to_string();
        let split = bytes.len() / 2;
        fs::write(&fixture.path, &bytes[..split]).unwrap();
        let mut cache = crate::events::EventCache::open_scoped(
            &fixture.root.join("index.json"),
            &crate::Options::default().source,
        )
        .unwrap();
        let scan = |cache: &mut crate::events::EventCache| {
            crate::events::scan_file(&fixture.path, "claude", cache, &mut false).unwrap()
        };
        assert!(
            scan(&mut cache)
                .handoff_index
                .as_ref()
                .unwrap()
                .calls()
                .is_empty()
        );
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(&fixture.path)
            .unwrap()
            .write_all(&bytes.as_bytes()[split..])
            .unwrap();
        let provisional = scan(&mut cache);
        assert_eq!(cache.revision(&fixture.path).unwrap().2, 0);
        assert_eq!(provisional.handoff_index.as_ref().unwrap().calls().len(), 1);
        let (sources, _, _) = fixture.sources_at(0);
        let reader = sources.reader();
        assert_eq!(
            reader
                .launch(
                    &fixture.path,
                    provisional.handoff_index.as_ref().unwrap().calls()[0]
                )
                .unwrap()
                .tool
                .id,
            "launch"
        );
        reader.finish().unwrap();
        let before = crate::events::PARSED.with(std::cell::Cell::get);
        fs::OpenOptions::new()
            .append(true)
            .open(&fixture.path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        let complete = scan(&mut cache).handoff_index.as_ref().unwrap().clone();
        assert_eq!(complete.complete.len(), 1);
        assert!(complete.eof.is_empty());
        assert_eq!(
            cache.revision(&fixture.path).unwrap().2,
            bytes.len() as u64 + 1
        );
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get) - before, 1);
        assert_eq!(scan(&mut cache).handoff_index.as_ref().unwrap(), &complete);
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get) - before, 1);
    }

    #[test]
    fn native_handoff_metadata_repair_skips_oversized_records_and_keeps_native_offsets() {
        let fixture = Fixture::new("launch-oversized-repair");
        let prefix = "codex".repeat(crate::handoff::MAX_INPUT_BYTES / 5 + 100) + "\n";
        let record = serde_json::json!({"message":{"content":[{"type":"tool_use","id":"launch","name":"Bash","input":{"command":"codex exec"}}]}});
        let original = format!("{prefix}{record}\n");
        fs::write(&fixture.path, &original).unwrap();
        let cache_path = fixture.root.join("index.json");
        let options = Options {
            cache: cache_path.clone(),
            ..Options::default()
        };
        let mut cache =
            crate::events::EventCache::open_scoped(&cache_path, &options.source).unwrap();
        let initial =
            crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
        assert_eq!(initial.handoff_index.as_ref().unwrap().calls().len(), 1);
        let before = crate::events::PARSED.with(std::cell::Cell::get);
        drop(cache);
        let connection = admitted_writer_connection(&options);
        assert_eq!(
            connection
                .execute(
                    "DELETE FROM main.meta \
                     WHERE namespace=semon_namespace() \
                       AND key GLOB 'native_launch_calls:*'",
                    [],
                )
                .unwrap(),
            2,
            "delete the launch page and header in the admitted source namespace"
        );
        let mut cache =
            crate::events::EventCache::open_scoped(&cache_path, &options.source).unwrap();
        let repaired =
            crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
        assert_eq!(repaired.handoff_index, initial.handoff_index);
        assert_eq!(crate::events::PARSED.with(std::cell::Cell::get), before);
        let (sources, _, _) = fixture.sources_at(original.len() as u64);
        let reader = sources.reader();
        assert_eq!(
            reader
                .launch(
                    &fixture.path,
                    repaired.handoff_index.as_ref().unwrap().calls()[0]
                )
                .unwrap()
                .tool
                .id,
            "launch"
        );
        reader.finish().unwrap();
        assert_eq!(fs::read_to_string(&fixture.path).unwrap(), original);
    }

    #[test]
    fn a_fixed_range_and_completion_do_not_hash_unrelated_source_bytes() {
        let _time = crate::events::fixed_change_time_for_test(0);
        let fixture = Fixture::new("range-budget");
        for length in [1024 * 1024, 16 * 1024 * 1024] {
            let bytes = format!(
                "{}\n",
                serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"x".repeat(length)}]}})
            );
            fs::write(&fixture.path, &bytes).unwrap();
            let (sources, reference, generation) = fixture.sources();
            let records = sources.reader();
            let read_before = crate::source_chunks::READ_BYTES.with(std::cell::Cell::get);
            let hash_before = crate::events::HASHED_BYTES.with(std::cell::Cell::get);
            records.verify_all().unwrap();
            let range = records
                .read_range(&reference, Some(&generation), 17, 1)
                .unwrap();
            assert_eq!(&*range.bytes, &bytes.as_bytes()[17..18]);
            records.finish().unwrap();
            assert_eq!(
                crate::source_chunks::READ_BYTES.with(std::cell::Cell::get) - read_before,
                2 * crate::source_chunks::SIZE as u64
            );
            assert_eq!(
                crate::events::HASHED_BYTES.with(std::cell::Cell::get),
                hash_before,
                "normal read must not invoke a complete-prefix hash"
            );
        }
    }

    #[test]
    fn native_ranges_refuse_a_rewrite_with_an_identical_observed_stamp() {
        let _time = crate::events::fixed_change_time_for_test(0);
        let fixture = Fixture::new("range-generation");
        let original = b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"old evidence\"}]}}\n";
        let replacement = b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"new evidence\"}]}}\n";
        assert_eq!(original.len(), replacement.len());
        fs::write(&fixture.path, original).unwrap();
        let (sources, _, _) = fixture.sources();
        let records = sources.reader();
        assert_eq!(
            records.derive(&fixture.path, 0).unwrap().unwrap()["message"]["content"][0]["text"],
            "old evidence"
        );
        let after = fixture.rewrite(replacement);
        let proof = sources.proofs.get(&fixture.path).unwrap();
        assert!(
            !proof
                .ledger
                .resumes(&after, &mut LogFile::open(&fixture.path).unwrap())
        );
        assert!(
            records.finish().is_err(),
            "completion must recheck used bytes"
        );
        let read_request = sources.reader();
        let result = read_request.derive(&fixture.path, 0);
        let finished = read_request.finish();
        assert!(
            result.is_err() && finished.is_err(),
            "captured old-generation proof admitted: {result:?}; finish={finished:?}"
        );
    }

    #[test]
    fn prepared_local_handle_can_fall_back_to_an_authorized_provider() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        struct Provider {
            source: SessionSourceRef,
            bytes: Arc<[u8]>,
            calls: AtomicUsize,
            revoked: AtomicBool,
        }
        impl SessionSourceReader for Provider {
            fn read_range(
                &self,
                source: &SessionSourceRef,
                expected_generation: Option<&str>,
                offset: u64,
                max: usize,
            ) -> io::Result<SessionSourceRange> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                if self.revoked.load(Ordering::Relaxed) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                assert_eq!(source, &self.source);
                assert!(expected_generation.is_none_or(|generation| generation == "archive"));
                let start = offset as usize;
                Ok(SessionSourceRange {
                    generation: "archive".into(),
                    length: self.bytes.len() as u64,
                    offset,
                    bytes: Arc::from(&self.bytes[start..(start + max).min(self.bytes.len())]),
                    cached: true,
                })
            }
        }
        let fixture = Fixture::new("prepared-provider-fallback");
        let line = b"{\"uuid\":\"old\"}\n";
        fs::write(&fixture.path, line).unwrap();
        let (sources, source, _) = fixture.sources();
        let provider = Provider {
            source,
            bytes: Arc::from(line.as_slice()),
            calls: AtomicUsize::new(0),
            revoked: AtomicBool::new(false),
        };
        let records = sources.reader_with(Some((
            &provider,
            crate::SessionSourceReadMode::LocalThenProvider,
        )));
        assert_eq!(
            records.derive(&fixture.path, 0).unwrap().unwrap()["uuid"],
            "old"
        );
        assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
        fs::remove_file(&fixture.path).unwrap();
        assert_eq!(
            records.derive(&fixture.path, 0).unwrap().unwrap()["uuid"],
            "old"
        );
        records.finish().unwrap();
        assert!(provider.calls.load(Ordering::Relaxed) > 0);
        provider.revoked.store(true, Ordering::Relaxed);
        assert!(records.finish().is_err());
    }

    #[test]
    fn prepared_native_handle_refuses_replacement_and_rechecks_parent_custody() {
        for (scalar, replacement) in [
            (true, "file"),
            (true, "parent"),
            (false, "file"),
            (false, "parent"),
        ] {
            let fixture = Fixture::new(&format!("prepared-custody-{scalar}-{replacement}"));
            let line = b"{\"uuid\":\"old\"}\n";
            fs::write(&fixture.path, line).unwrap();
            let (sources, _, _) = fixture.sources();
            let records = sources.reader();
            let read = || {
                if scalar {
                    records.derive(&fixture.path, 0)
                } else {
                    records.read(&fixture.path, 0).map(|(value, _)| value)
                }
            };
            assert_eq!(read().unwrap().unwrap()["uuid"], "old");
            assert!(
                records
                    .prepared
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .file
                    .is_some()
            );
            match replacement {
                "file" => {
                    let next = fixture.root.join("replacement.jsonl");
                    fs::write(&next, line).unwrap();
                    fs::rename(next, &fixture.path).unwrap();
                    assert!(read().is_err());
                    assert!(records.finish().is_err());
                }
                "parent" => {
                    let moved = fixture.root.with_extension("moved");
                    fs::rename(&fixture.root, &moved).unwrap();
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&moved, &fixture.root).unwrap();
                    #[cfg(not(unix))]
                    fs::create_dir(&fixture.root).unwrap();
                    // Even the same inode behind a replaced parent cannot be
                    // published using the previously admitted handle.
                    assert!(records.finish().is_err());
                    #[cfg(unix)]
                    fs::remove_file(&fixture.root).unwrap();
                    #[cfg(not(unix))]
                    fs::remove_dir(&fixture.root).unwrap();
                    fs::rename(moved, &fixture.root).unwrap();
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn scalar_preparation_reuses_one_verified_block_and_completion_refuses_rewrites() {
        let _time = crate::events::fixed_change_time_for_test(0);
        let fixture = Fixture::new("scalar-budget");
        let line = b"{\"uuid\":\"old\"}\n";
        let original = line.repeat(1500);
        assert!(original.len() < crate::source_chunks::SIZE);
        fs::write(&fixture.path, &original).unwrap();
        let (sources, _, _) = fixture.sources();
        let records = sources.reader();
        let read_before = crate::source_chunks::READ_BYTES.with(std::cell::Cell::get);
        let hash_before = crate::events::HASHED_BYTES.with(std::cell::Cell::get);
        for offset in (0..original.len()).step_by(line.len()) {
            assert_eq!(
                records
                    .record_bytes(&fixture.path, offset as u64, 32 * 1024 * 1024)
                    .unwrap()
                    .unwrap(),
                line
            );
            assert_eq!(
                records
                    .derive(&fixture.path, offset as u64)
                    .unwrap()
                    .unwrap()["uuid"],
                "old"
            );
        }
        assert_eq!(
            crate::source_chunks::READ_BYTES.with(std::cell::Cell::get) - read_before,
            original.len() as u64
        );
        assert_eq!(
            records
                .prepared
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .bytes
                .len(),
            original.len()
        );
        records.finish().unwrap();
        assert_eq!(
            crate::source_chunks::READ_BYTES.with(std::cell::Cell::get) - read_before,
            2 * original.len() as u64
        );
        assert_eq!(
            crate::events::HASHED_BYTES.with(std::cell::Cell::get),
            hash_before
        );
        let mut changed = original.clone();
        changed[..line.len()].copy_from_slice(b"{\"uuid\":\"new\"}\n");
        fixture.rewrite(&changed);
        // Preparation can reuse only verified old-generation bytes. It never
        // labels changed bytes with the old proof, and cannot publish on finish.
        assert_eq!(
            records
                .record_bytes(&fixture.path, 0, 32 * 1024 * 1024)
                .unwrap()
                .unwrap(),
            line
        );
        assert_eq!(
            records.derive(&fixture.path, 0).unwrap().unwrap()["uuid"],
            "old"
        );
        assert!(records.finish().is_err());
        fs::remove_file(&fixture.path).unwrap();
        assert!(
            records
                .record_bytes(&fixture.path, 0, 32 * 1024 * 1024)
                .is_err()
        );
    }
}
