//! Durable Raw write intents in the existing native index database.
use crate::{Input, Options, events::Stat};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppendEvidence {
    pub(crate) generation: String,
    pub(crate) before: Stat,
    pub(crate) after: Stat,
}

impl AppendEvidence {
    pub(crate) fn qualifies(&self, before: &Stat, after: &Stat) -> bool {
        self.generation.len() == 32
            && self.generation.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.before == *before
            && self.after == *after
            && before.dev == after.dev
            && before.ino == after.ino
            && before.size <= after.size
            && before.changed_ns.is_some()
            && after.changed_ns.is_some()
    }
}

// Content-free evidence for one receiver-owned PID input mutation.
// Other process inputs and semantic facts must remain exactly the same.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeInputEvidence {
    pub(crate) unaffected: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppendContinuation {
    pub(crate) proof: String,
    pub(crate) state: crate::source_chunks::PrefixState,
    pub(crate) offset: u64,
    pub(crate) prefix: [u8; 32],
}

/// Exact content-free request retained across Raw/SQL commit domains.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteIntent {
    pub(crate) operation: String,
    pub(crate) root: String,
    pub(crate) path: String,
    pub(crate) generation: Option<String>,
    pub(crate) from: u64,
    pub(crate) before: Option<Stat>,
    pub(crate) before_hash: Option<[u8; 32]>,
    pub(crate) continuation: Option<AppendContinuation>,
    pub(crate) head: Option<String>,
    pub(crate) ledger: Option<crate::events::Ledger>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteCompletion {
    pub(crate) to: u64,
    pub(crate) after: Stat,
    pub(crate) hash: [u8; 32],
    pub(crate) evidence: Option<AppendEvidence>,
    pub(crate) runtime: String,
    pub(crate) facts: Option<String>,
}

#[cfg(test)]
thread_local! { pub(crate) static SOURCE_SQL_LOCK:std::cell::Cell<bool>=const {std::cell::Cell::new(false)}; }
#[cfg(test)]
pub(crate) struct SqlLockScope;
#[cfg(test)]
impl SqlLockScope {
    pub(crate) fn enter() -> Self {
        SOURCE_SQL_LOCK.with(|v| assert!(!v.replace(true)));
        Self
    }
}
#[cfg(test)]
impl Drop for SqlLockScope {
    fn drop(&mut self) {
        SOURCE_SQL_LOCK.with(|v| v.set(false));
    }
}
pub(crate) fn bytes_hash(
    path: &std::path::Path,
    length: u64,
    mut check: impl FnMut() -> io::Result<()>,
) -> io::Result<[u8; 32]> {
    #[cfg(test)]
    SOURCE_SQL_LOCK.with(|v| assert!(!v.get(), "history I/O under SourceState lock"));
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = crate::sealed::LogFile::open(path)?;
    let mut hash = Sha256::new();
    let mut left = length;
    let mut buffer = [0u8; 65536];
    while left > 0 {
        check()?;
        crate::selected_read::check()?;
        let size = (left.min(buffer.len() as u64)) as usize;
        file.read_exact(&mut buffer[..size])?;
        hash.update(&buffer[..size]);
        left -= size as u64;
        #[cfg(test)]
        {
            crate::events::SOURCE_READ_BYTES.with(|n| n.set(n.get() + size as u64));
            crate::events::SOURCE_HASHED_BYTES.with(|n| n.set(n.get() + size as u64));
        }
    }
    Ok(hash.finalize().into())
}

/// A receiver-owned write intent. Begin under the receiver's Raw publication
/// fence before touching bytes, then complete under the same fence after fsync.
/// An interrupted/failed write grants no append trust and remains pending work.
/// The caller must verify that `generation` matches its previous custody proof;
/// pass None for untrusted writes, replacements, or ambiguous generations.
pub struct SessionSourceWrite {
    _lock: SourceLock,
    store: Box<dyn crate::events::IndexStore>,
    intent: WriteIntent,
    options: Options,
    input: Input,
    revision: i64,
    before: Option<Stat>,
    previous: Option<AppendEvidence>,
    generation: Option<String>,
    from: u64,
}

impl SessionSourceWrite {
    pub fn begin(
        options: &Options,
        input: &Input,
        generation: Option<&str>,
        from: u64,
    ) -> io::Result<Self> {
        if !crate::is_input_path(input.root.as_str(), &input.path) {
            return Err(io::Error::other("write intent outside native allowlist"));
        }
        let store = crate::events::store::source_write_store(options)?;
        Self::begin_with_store(options, input, generation, from, store, None)
    }

    pub(crate) fn begin_with_store(
        options: &Options,
        input: &Input,
        generation: Option<&str>,
        from: u64,
        mut store: Box<dyn crate::events::IndexStore>,
        resume: Option<WriteIntent>,
    ) -> io::Result<Self> {
        if !crate::is_input_path(input.root.as_str(), &input.path) {
            return Err(io::Error::other("write intent outside native allowlist"));
        }
        let source_lock = SourceLock::acquire(options, &lock_key(options, input), true)?;
        let resumed = resume.is_some();
        let mut intent = if let Some(intent) = resume {
            intent
        } else {
            let before = match Stat::of(&input.full_path(options)) {
                Ok(stat) => Some(stat),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            let before_hash = None;
            WriteIntent {
                operation: crate::storage::source_observations::SourceOperation::capture(
                    "source-write",
                    "raw",
                    input.path.as_bytes(),
                )
                .id,
                root: input.root.as_str().into(),
                path: input.full_path(options).to_string_lossy().into_owned(),
                generation: generation
                    .filter(|v| {
                        input.path.ends_with(".jsonl")
                            && v.len() == 32
                            && v.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                    .map(str::to_owned),
                from,
                before,
                before_hash,
                continuation: None,
                head: store
                    .session_catalog_generation()
                    .map_err(|e| io::Error::other(e.to_string()))?,
                ledger: store
                    .ledger(input.full_path(options).to_string_lossy().as_ref())
                    .map_err(|e| io::Error::other(e.to_string()))?,
            }
        };
        if intent.path != input.full_path(options).to_string_lossy()
            || intent.root != input.root.as_str()
        {
            return Err(io::Error::other("source request binding changed"));
        }
        if !resumed {
            store
                .source_write_prepare(options, input, &mut intent)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        let (revision, previous) = store
            .source_write_begin(options, input, &intent)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let before = intent.before;
        let generation = intent.generation.clone();
        let from = intent.from;
        Ok(Self {
            _lock: source_lock,
            store,
            intent,
            options: options.clone(),
            input: input.clone(),
            revision,
            before,
            previous,
            generation,
            from,
        })
    }

    // PG fault/replay qualification captures the exact durable request before
    // dropping the receiver. This is not a new request or a conversion path.
    #[cfg(all(test, feature = "postgres"))]
    pub(crate) fn intent(&self) -> WriteIntent {
        self.intent.clone()
    }

    pub fn complete(mut self, to: u64) -> io::Result<()> {
        let after = Stat::of(&self.input.full_path(&self.options))?;
        let evidence = append_plan(
            self.generation,
            self.before,
            self.previous,
            self.from,
            after,
            to,
        );
        let completion = WriteCompletion {
            to,
            after,
            hash: [0; 32],
            evidence,
            runtime: if self.store.source_write_proofs() {
                crate::native_evidence::query_runtime_generation(&self.options)?
            } else {
                String::new()
            },
            facts: self
                .options
                .facts
                .as_ref()
                .filter(|_| self.store.source_write_proofs())
                .map(|path| {
                    use sha2::{Digest, Sha256};
                    let facts = crate::read_facts(path)?;
                    Ok::<_, io::Error>(format!(
                        "{:x}",
                        Sha256::digest(serde_json::to_vec(&facts).map_err(io::Error::other)?)
                    ))
                })
                .transpose()?,
        };
        self.store
            .source_write_complete(
                &self.options,
                &self.input,
                &self.intent,
                self.revision,
                &completion,
            )
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

pub(crate) fn append_plan(
    generation: Option<String>,
    before: Option<Stat>,
    previous: Option<AppendEvidence>,
    from: u64,
    after: Stat,
    to: u64,
) -> Option<AppendEvidence> {
    match (generation, before) {
        (Some(generation), Some(before))
            if before.size == from
                && after.size == to
                && before.dev == after.dev
                && before.ino == after.ino
                && to >= from =>
        {
            let base = previous
                .filter(|previous| previous.generation == generation && previous.after == before)
                .map_or(before, |previous| previous.before);
            Some(AppendEvidence {
                generation,
                before: base,
                after,
            })
        }
        _ => None,
    }
}

pub(crate) fn lock_key(options: &Options, input: &Input) -> String {
    if input.path.ends_with(".meta.json") {
        input
            .full_path(options)
            .with_extension("")
            .with_extension("jsonl")
            .to_string_lossy()
            .into_owned()
    } else if input.root == crate::InputRoot::Claude && input.path.starts_with("sessions/") {
        "facts".into()
    } else {
        input.full_path(options).to_string_lossy().into_owned()
    }
}

/// Private advisory locks complement the SQLite revision fences. A crash drops
/// the OS lock, leaving its durable intent available for conservative replay.
pub(crate) struct SourceLock(#[cfg(unix)] std::fs::File);
impl SourceLock {
    pub(crate) fn acquire(options: &Options, path: &str, exclusive: bool) -> io::Result<Self> {
        Self::acquire_cache(&options.cache, path, exclusive)
    }
    #[cfg(unix)]
    pub(crate) fn acquire_cache(
        cache: &std::path::Path,
        path: &str,
        exclusive: bool,
    ) -> io::Result<Self> {
        use sha2::{Digest, Sha256};
        use std::os::unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            io::AsRawFd,
        };
        let directory = cache.with_extension("source-locks");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(&directory)?;
        if !std::fs::symlink_metadata(&directory)?.file_type().is_dir() {
            return Err(io::Error::other("source lock directory is not regular"));
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join(format!("{:x}", Sha256::digest(path.as_bytes()))))?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("source lock is not regular"));
        }
        // Shared Catalog readers take the facts fence before per-source
        // fences. Writers hold one fence at a time (multi-source relocation
        // sorts its keys), so waiting here preserves the established order.
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
    #[cfg(not(unix))]
    pub(crate) fn acquire_cache(
        cache: &std::path::Path,
        path: &str,
        exclusive: bool,
    ) -> io::Result<Self> {
        let _ = (cache, path);
        if exclusive {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "receiver custody requires advisory source locks",
            ))
        } else {
            Ok(Self())
        }
    }
}
impl Drop for SourceLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// A facts update holds the same durable intent and OS-fence contract as Raw.
/// The producer processes only changed native/runtime dependency selectors.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FactsIntent {
    pub(crate) operation: String,
    pub(crate) path: String,
    pub(crate) head: Option<String>,
    pub(crate) before: crate::Facts,
    pub(crate) wanted: crate::Facts,
    pub(crate) push_driven: bool,
}
pub struct SessionFactsWrite {
    store: Box<dyn crate::events::IndexStore>,
    intent: FactsIntent,
    options: Options,
    _lock: SourceLock,
}
impl SessionFactsWrite {
    pub fn begin(options: &Options, wanted: &crate::Facts) -> io::Result<Self> {
        Self::begin_with_source_journal(options, wanted, true)
    }
    // The reference receiver journals facts, but its Raw transport does not
    // journal every mutation. Keep ordinary metadata discovery for that reader.
    pub(crate) fn begin_observed(options: &Options, wanted: &crate::Facts) -> io::Result<Self> {
        Self::begin_with_source_journal(options, wanted, false)
    }
    fn begin_with_source_journal(
        options: &Options,
        wanted: &crate::Facts,
        push_driven: bool,
    ) -> io::Result<Self> {
        let store = crate::events::store::source_write_store(options)?;
        Self::begin_with_store(options, wanted, push_driven, store, None)
    }
    pub(crate) fn begin_with_store(
        options: &Options,
        wanted: &crate::Facts,
        push_driven: bool,
        mut store: Box<dyn crate::events::IndexStore>,
        resume: Option<FactsIntent>,
    ) -> io::Result<Self> {
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::run(options, "facts_write_wait");
        let source_lock = SourceLock::acquire(options, "facts", true)?;
        let path = options
            .facts
            .as_ref()
            .ok_or_else(|| io::Error::other("Facts path missing"))?;
        let intent = if let Some(intent) = resume {
            intent
        } else {
            let before = match crate::read_facts(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => crate::Facts::default(),
                Err(e) => return Err(e),
            };
            FactsIntent {
                operation: crate::storage::source_observations::SourceOperation::capture(
                    "facts-write",
                    "raw",
                    path.to_string_lossy().as_bytes(),
                )
                .id,
                path: path.to_string_lossy().into_owned(),
                head: store
                    .session_catalog_generation()
                    .map_err(|e| io::Error::other(e.to_string()))?,
                before,
                wanted: wanted.clone(),
                push_driven,
            }
        };
        if intent.path != path.to_string_lossy()
            || intent.wanted != *wanted
            || intent.push_driven != push_driven
        {
            return Err(io::Error::other("Facts request binding changed"));
        }
        store
            .facts_write_begin(options, &intent)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Self {
            store,
            intent,
            options: options.clone(),
            _lock: source_lock,
        })
    }
    #[cfg(all(test, feature = "postgres"))]
    pub(crate) fn intent(&self) -> FactsIntent {
        self.intent.clone()
    }

    pub fn complete(mut self) -> io::Result<()> {
        self.store
            .facts_write_complete(&self.options, &self.intent)
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

/// Journal an allowlisted sidecar or process-record dependency before/after a
/// receiver write. Raw JSONL writers use SessionSourceWrite instead.
pub fn invalidate_session_input(options: &Options, input: &Input) -> io::Result<()> {
    if !crate::is_input_path(input.root.as_str(), &input.path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "input outside native allowlist",
        ));
    }
    if input.path.ends_with(".meta.json") {
        let mut log = input.clone();
        log.path = format!("{}.jsonl", input.path.trim_end_matches(".meta.json"));
        return crate::invalidate_session_sources(options, &[log]);
    }
    if input.root == crate::InputRoot::Claude && input.path.starts_with("sessions/") {
        let value: serde_json::Value = std::fs::read(input.full_path(options))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if let Some(id) = value["sessionId"].as_str() {
            return crate::events::store::enqueue_native(options, "claude", id);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelocationEvidence {
    pub(crate) from_path: String,
    pub(crate) native_id: String,
    pub(crate) generation: String,
    pub(crate) length: u64,
    pub(crate) prefix: String,
    pub(crate) before: Stat,
    pub(crate) after: Stat,
}

/// Verify a proposed Codex relocation against the receiver's existing Raw
/// generation. The caller must also validate its custody generation under its
/// Raw publication fence. An indexed exact prefix avoids reading history.
pub fn verify_codex_relocation(
    options: &Options,
    from: &Input,
    to: &Input,
    native_id: &str,
    length: u64,
    expected: &str,
) -> io::Result<bool> {
    if from.root != crate::InputRoot::Codex
        || to.root != crate::InputRoot::Codex
        || !crate::is_input_path("codex", &from.path)
        || !crate::is_input_path("codex", &to.path)
        || from.path.starts_with("archived_sessions/") == to.path.starts_with("archived_sessions/")
        || length == 0
        || expected.len() != 64
        || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(false);
    }
    let old = from.full_path(options);
    let before = Stat::of(&old)?;
    if before.size < length
        || crate::codex_native_id_prefix(&old, length)?.as_deref() != Some(native_id)
    {
        return Ok(false);
    }
    let matches =
        crate::events::store::verify_relocation_prefix(options, &old, &before, length, expected)?;
    Ok(matches && Stat::of(&old)? == before)
}

/// A native relocation intent fences both paths and survives an interrupted
/// Raw publication in the existing source journal. No source authority is
/// granted here: facts still choose the one current Codex rollout path.
pub struct SessionSourceRelocation {
    options: Options,
    from: Input,
    to: Input,
    native_id: String,
    generation: String,
    length: u64,
    prefix: String,
    before: Stat,
    _locks: Vec<SourceLock>,
}
impl SessionSourceRelocation {
    pub fn begin(
        options: &Options,
        from: &Input,
        to: &Input,
        native_id: &str,
        generation: &str,
        length: u64,
        prefix: &str,
    ) -> io::Result<Self> {
        let mut keys = [lock_key(options, from), lock_key(options, to)];
        keys.sort();
        let locks = keys
            .iter()
            .map(|key| SourceLock::acquire(options, key, true))
            .collect::<io::Result<Vec<_>>>()?;
        if generation.len() != 32
            || !generation.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !verify_codex_relocation(options, from, to, native_id, length, prefix)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native relocation generation is unverified",
            ));
        }
        let before = Stat::of(&from.full_path(options))?;
        crate::invalidate_session_sources(options, &[from.clone(), to.clone()])?;
        Ok(Self {
            options: options.clone(),
            from: from.clone(),
            to: to.clone(),
            native_id: native_id.into(),
            generation: generation.into(),
            length,
            prefix: prefix.into(),
            before,
            _locks: locks,
        })
    }
    pub fn complete(self) -> io::Result<()> {
        if Stat::of(&self.from.full_path(&self.options))? != self.before {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "relocation source changed during copy",
            ));
        }
        let evidence = RelocationEvidence {
            from_path: self
                .from
                .full_path(&self.options)
                .to_string_lossy()
                .into_owned(),
            native_id: self.native_id,
            generation: self.generation,
            length: self.length,
            prefix: self.prefix,
            before: self.before,
            after: Stat::of(&self.to.full_path(&self.options))?,
        };
        crate::events::store::complete_relocation(&self.options, &self.to, &evidence)
    }
}

/// Bounded discovery hints for an already indexed native Codex identity.
/// These paths grant no selection or control authority: a receiver must check
/// its current authoritative inventory and revalidate the native header.
pub fn indexed_codex_sources(options: &Options, native_id: &str) -> io::Result<Vec<Input>> {
    #[cfg(feature = "postgres")]
    if let Some(config) = options.source.selected_store()
        && matches!(
            config.backend,
            crate::storage::StoreBackend::Postgres { .. }
        )
    {
        return crate::storage::postgres::host::indexed_codex_sources(config, options, native_id)
            .map_err(io::Error::other);
    }
    let binding =
        crate::storage::local_navigation_binding(options, "").map_err(io::Error::other)?;
    let db = binding.connection().map_err(io::Error::other)?;
    let tx = db.unchecked_transaction().map_err(io::Error::other)?;
    binding
        .validate(&tx, &binding.config().namespace)
        .map_err(io::Error::other)?;
    let mut statement = tx.prepare("SELECT DISTINCT source_path FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND lifecycle='current' AND harness='codex' AND native_id=?1 LIMIT 2").map_err(io::Error::other)?;
    let paths = statement
        .query_map([native_id], |row| row.get::<_, String>(0))
        .map_err(io::Error::other)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?;
    Ok(paths
        .into_iter()
        .filter_map(|path| {
            let relative = std::path::Path::new(&path)
                .strip_prefix(&options.codex_home)
                .ok()?
                .to_str()?;
            crate::is_input_path("codex", relative).then(|| Input {
                root: crate::InputRoot::Codex,
                path: relative.to_owned(),
            })
        })
        .collect())
}
