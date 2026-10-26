//! Durable Raw write intents in the existing native index database.
use crate::{Input, Options, events::Stat};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub(crate) before_hash: Option<[u8;32]>,
    pub(crate) head: Option<String>,
    pub(crate) ledger: Option<crate::events::Ledger>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteCompletion {
    pub(crate) to: u64,
    pub(crate) after: Stat,
    pub(crate) hash: [u8;32],
    pub(crate) evidence: Option<AppendEvidence>,
    pub(crate) runtime: String,
    pub(crate) facts: Option<String>,
}

pub(crate) fn bytes_hash(path: &std::path::Path, length:u64) -> io::Result<[u8;32]> {
    use std::io::Read;
    use sha2::{Digest,Sha256};
    let mut file=crate::sealed::LogFile::open(path)?;
    let mut hash=Sha256::new();let mut left=length;let mut buffer=[0u8;65536];
    while left>0 {
        crate::selected_read::check()?;
        let size=(left.min(buffer.len() as u64)) as usize;
        file.read_exact(&mut buffer[..size])?;hash.update(&buffer[..size]);left-=size as u64;
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
        if !crate::is_input_path(input.root.as_str(),&input.path) {return Err(io::Error::other("write intent outside native allowlist"));}
        let store=crate::events::store::source_write_store(options)?;
        Self::begin_with_store(options,input,generation,from,store,None)
    }

    pub(crate) fn begin_with_store(options:&Options,input:&Input,generation:Option<&str>,from:u64,mut store:Box<dyn crate::events::IndexStore>,resume:Option<WriteIntent>) -> io::Result<Self> {
        if !crate::is_input_path(input.root.as_str(),&input.path) {return Err(io::Error::other("write intent outside native allowlist"));}
        let source_lock=SourceLock::acquire(options,&lock_key(options,input),true)?;
        let intent=if let Some(intent)=resume {intent} else {
            let before=Stat::of(&input.full_path(options)).ok();
            let before_hash=before.as_ref().map(|s|bytes_hash(&input.full_path(options),s.size)).transpose()?;
            WriteIntent {
                operation:crate::storage::source_observations::SourceOperation::capture("source-write","raw",input.path.as_bytes()).id,
                root:input.root.as_str().into(),path:input.full_path(options).to_string_lossy().into_owned(),
                generation:generation.filter(|v|v.len()==32 && v.bytes().all(|b|b.is_ascii_hexdigit())).map(str::to_owned),from,before,before_hash,
                head:store.session_catalog_generation().map_err(|e|io::Error::other(e.to_string()))?,
                ledger:store.ledger(input.full_path(options).to_string_lossy().as_ref()).map_err(|e|io::Error::other(e.to_string()))?,
            }
        };
        if intent.path!=input.full_path(options).to_string_lossy() || intent.root!=input.root.as_str() {return Err(io::Error::other("source request binding changed"));}
        let (revision,previous)=store.source_write_begin(options,input,&intent).map_err(|e|io::Error::other(e.to_string()))?;
        let before=intent.before;let generation=intent.generation.clone();let from=intent.from;
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

    #[cfg(test)]
    pub(crate) fn intent(&self) -> WriteIntent { self.intent.clone() }

    pub fn complete(mut self, to: u64) -> io::Result<()> {
        let after = Stat::of(&self.input.full_path(&self.options))?;
        let evidence = match (self.generation, self.before) {
            (Some(generation), Some(before))
                if before.size == self.from
                    && after.size == to
                    && before.dev == after.dev
                    && before.ino == after.ino
                    && to >= self.from =>
            {
                let base = self
                    .previous
                    .filter(|previous| {
                        previous.generation == generation && previous.after == before
                    })
                    .map_or(before, |previous| previous.before);
                Some(AppendEvidence {
                    generation,
                    before: base,
                    after,
                })
            }
            _ => None,
        };
        let completion=WriteCompletion {
            to,after,hash:bytes_hash(&self.input.full_path(&self.options),to)?,evidence,
            runtime:if self.store.source_write_proofs() {crate::native_evidence::query_runtime_generation(&self.options)?} else {String::new()},
            facts:self.options.facts.as_ref().filter(|_|self.store.source_write_proofs()).map(|path| {
                use sha2::{Digest,Sha256};
                let facts=crate::read_facts(path)?;
                Ok::<_,io::Error>(format!("{:x}",Sha256::digest(serde_json::to_vec(&facts).map_err(io::Error::other)?)))
            }).transpose()?,
        };
        self.store.source_write_complete(&self.options,&self.input,&self.intent,self.revision,&completion).map_err(|e|io::Error::other(e.to_string()))
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
    #[cfg(unix)]
    pub(crate) fn acquire(options: &Options, path: &str, exclusive: bool) -> io::Result<Self> {
        use sha2::{Digest, Sha256};
        use std::os::unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            io::AsRawFd,
        };
        let directory = options.cache.with_extension("source-locks");
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
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH | libc::LOCK_NB
        };
        if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
    #[cfg(not(unix))]
    pub(crate) fn acquire(options: &Options, path: &str, exclusive: bool) -> io::Result<Self> {
        let _ = (options, path);
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
pub struct SessionFactsWrite {
    options: Options,
    before: crate::Facts,
    wanted: crate::Facts,
    push_driven: bool,
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
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::run(options, "facts_write_wait");
        let source_lock = SourceLock::acquire(options, "facts", true)?;
        let before = options
            .facts
            .as_ref()
            .and_then(|path| crate::read_facts(path).ok())
            .unwrap_or_default();
        crate::events::store::enqueue_facts(options, &before, wanted, push_driven)?;
        Ok(Self {
            options: options.clone(),
            before,
            wanted: wanted.clone(),
            push_driven,
            _lock: source_lock,
        })
    }
    pub fn complete(self) -> io::Result<()> {
        // Re-enqueue after publication; claimed revisions can never ACK a newer
        // facts update, including recovery from a lost acknowledgment.
        crate::events::store::enqueue_facts(
            &self.options,
            &self.before,
            &self.wanted,
            self.push_driven,
        )
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
    use rusqlite::{Connection, OpenFlags};
    let connection = Connection::open_with_flags(
        crate::events::EventCache::path(&options.cache),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(io::Error::other)?;
    connection
        .busy_timeout(std::time::Duration::from_millis(100))
        .map_err(io::Error::other)?;
    let mut statement = connection.prepare("SELECT DISTINCT source_path FROM session_catalog_sources WHERE lifecycle='current' AND harness='codex' AND native_id=?1 LIMIT 2").map_err(io::Error::other)?;
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
