//! Generation-bound local reads for native Query/Catalog/source derivation.
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
    source: SessionSourceRef,
    ledger: Ledger,
    generation: String,
    chunks: Arc<crate::source_chunks::Chunks>,
}

#[derive(Default)]
pub(crate) struct Sources {
    proofs: BTreeMap<PathBuf, Proof>,
    paths: BTreeMap<(String, String), PathBuf>,
}

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "native query source generation unavailable",
    )
}

impl Sources {
    pub(crate) fn capture(
        options: &Options,
        sources: impl Iterator<Item = (CatalogSource, Option<Arc<crate::source_chunks::Chunks>>)>,
    ) -> io::Result<Self> {
        let mut captured = Self::default();
        for (source, chunks) in sources {
            let chunks = chunks
                .filter(|chunks| chunks.valid(source.offset))
                .ok_or_else(unavailable)?;
            let (root, path) = [InputRoot::Claude, InputRoot::Codex, InputRoot::Copilot]
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
                    Some((root.as_str().to_owned(), relative.to_owned()))
                })
                .ok_or_else(unavailable)?;
            let reference = SessionSourceRef {
                root: root.clone(),
                path: path.clone(),
                native_id: source.native_id,
                offset: source.offset,
                prefix_sha256: source.prefix_sha256,
                tail_sha256: source.tail_sha256,
            };
            let generation = reference
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
            captured.paths.insert((root, path), source.path.clone());
            captured.proofs.insert(source.path, proof);
        }
        Ok(captured)
    }

    pub(crate) fn reader(&self) -> Records<'_> {
        Records {
            sources: self,
            checked: Mutex::default(),
            checked_chunks: Mutex::default(),
            checked_evidence: Mutex::default(),
            prepared: Mutex::default(),
            failed: Mutex::new(false),
        }
    }
}

/// The verification cache belongs to one read, never the shared query cohort.
pub(crate) struct Records<'a> {
    sources: &'a Sources,
    checked: Mutex<BTreeMap<PathBuf, Stat>>,
    checked_chunks: Mutex<BTreeMap<PathBuf, BTreeSet<usize>>>,
    checked_evidence: Mutex<BTreeMap<PathBuf, BTreeSet<crate::native_record::Record>>>,
    // Scalar preparation visits adjacent short records. Reuse at most one
    // immutable, verified block; completion still checks every used block.
    prepared: Mutex<Option<PreparedBlock>>,
    failed: Mutex<bool>,
}

struct PreparedBlock {
    path: PathBuf,
    index: usize,
    bytes: Arc<[u8]>,
}

impl Records<'_> {
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
                .is_none_or(|end| end > proof.ledger.stat.size.max(proof.source.offset))
            {
                return Err(unavailable());
            }
            self.verify(path, proof)?;
            let mut file = LogFile::open(path)?;
            let before = Stat::from_metadata(&file.metadata()?)?;
            if before.dev != proof.ledger.stat.dev
                || before.ino != proof.ledger.stat.ino
                || record.end().is_none_or(|end| end > before.size)
            {
                return Err(unavailable());
            }
            let value = read(&mut file)?;
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
                self.with_evidence(path, call.record(), |file| call.record().verify(file))?;
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
        if offset >= proof.source.offset {
            return Ok(None);
        }
        let mut reader = BufReader::with_capacity(
            crate::source_chunks::SIZE,
            RangeFile {
                records: self,
                proof,
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
            if offset >= proof.source.offset {
                return Err(unavailable());
            }
            let mut reader = BufReader::with_capacity(
                crate::source_chunks::SIZE,
                RangeFile {
                    records: self,
                    proof,
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
        let mut file = LogFile::open(path)?;
        let before = Stat::from_metadata(&file.metadata()?)?;
        let mut checked = self.checked.lock().map_err(|_| unavailable())?;
        let unchanged = before.changed_ns.is_some()
            && (before == proof.ledger.stat || checked.get(path) == Some(&before));
        if (!unchanged && !proof.ledger.resumes(&before, &mut file))
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
            crate::source_reader::read_native_record(self, &proof.source, &proof.generation, offset)
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
                position: offset,
                prepared: false,
            },
            offset,
        )
    }

    /// Recheck touched sources before returning an answer, including native
    /// records served from the pager's short-lived line cache.
    pub(crate) fn finish(&self) -> io::Result<()> {
        if *self.failed.lock().map_err(|_| unavailable())? {
            return Err(unavailable());
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
        for (path, indices) in self
            .checked_chunks
            .lock()
            .map_err(|_| unavailable())?
            .iter()
        {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            let mut file = LogFile::open(path)?;
            let before = Stat::from_metadata(&file.metadata()?)?;
            if before.dev != proof.ledger.stat.dev
                || before.ino != proof.ledger.stat.ino
                || before.size < proof.source.offset
            {
                return Err(unavailable());
            }
            for index in indices {
                proof.chunks.read(&mut file, *index)?;
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
            if before.dev != proof.ledger.stat.dev || before.ino != proof.ledger.stat.ino {
                return Err(unavailable());
            }
            for record in records {
                record.verify(&mut file)?;
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
        if source != &proof.source
            || expected.is_some_and(|value| value != proof.generation)
            || max > crate::SESSION_SOURCE_CHUNK_MAX
            || max == 0
            || offset
                .checked_add(max as u64)
                .is_none_or(|end| end > source.offset)
        {
            return Err(unavailable());
        }
        let mut file = LogFile::open(path)?;
        let before = Stat::from_metadata(&file.metadata()?)?;
        if before.dev != proof.ledger.stat.dev
            || before.ino != proof.ledger.stat.ino
            || before.size < source.offset
        {
            return Err(unavailable());
        }
        let mut checked = self.checked.lock().map_err(|_| unavailable())?;
        let unchanged = before.changed_ns.is_some()
            && (before == proof.ledger.stat || checked.get(path) == Some(&before));
        if !unchanged && !proof.ledger.resumes(&before, &mut file) {
            return Err(unavailable());
        }
        let first = usize::try_from(offset / crate::source_chunks::SIZE as u64)
            .map_err(|_| unavailable())?;
        let end = offset + max as u64;
        let last = usize::try_from((end - 1) / crate::source_chunks::SIZE as u64)
            .map_err(|_| unavailable())?;
        let mut bytes = Vec::with_capacity(max);
        let mut prepared = prepare
            .then(|| self.prepared.lock().map_err(|_| unavailable()))
            .transpose()?;
        for index in first..=last {
            let part = if let Some(block) = prepared
                .as_deref()
                .and_then(Option::as_ref)
                .filter(|block| block.path == *path && block.index == index)
            {
                block.bytes.clone()
            } else {
                let part: Arc<[u8]> = Arc::from(proof.chunks.read(&mut file, index)?);
                if let Some(prepared) = prepared.as_deref_mut() {
                    *prepared = Some(PreparedBlock {
                        path: path.clone(),
                        index,
                        bytes: part.clone(),
                    });
                }
                part
            };
            let start = index as u64 * crate::source_chunks::SIZE as u64;
            let lo = offset.saturating_sub(start) as usize;
            let hi = (end - start).min(part.len() as u64) as usize;
            bytes.extend_from_slice(&part[lo..hi]);
        }
        if Stat::from_metadata(&file.metadata()?)? != before
            || (before.changed_ns.is_none() && !proof.ledger.resumes(&before, &mut file))
        {
            return Err(unavailable());
        }
        checked.insert(path.clone(), before);
        self.checked_chunks
            .lock()
            .map_err(|_| unavailable())?
            .entry(path.clone())
            .or_default()
            .extend(first..=last);
        Ok(SessionSourceRange {
            generation: proof.generation.clone(),
            // The cohort exposes its complete consumed prefix, even if the
            // native writer has appended more bytes since observation.
            length: source.offset,
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
        if result.is_err() {
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
    position: u64,
    prepared: bool,
}

impl Read for RangeFile<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut max = buffer
            .len()
            .min(crate::SESSION_SOURCE_CHUNK_MAX)
            .min(usize::try_from(self.proof.source.offset - self.position).unwrap_or(usize::MAX));
        if self.prepared {
            max = max.min(
                crate::source_chunks::SIZE
                    - (self.position % crate::source_chunks::SIZE as u64) as usize,
            );
        }
        if max == 0 {
            return Ok(0);
        }
        let part = if self.prepared {
            let result = self.records.range(
                &self.proof.source,
                Some(&self.proof.generation),
                self.position,
                max,
                true,
            );
            if result.is_err() {
                *self.records.failed.lock().map_err(|_| unavailable())? = true;
            }
            result?
        } else {
            self.records.read_range(
                &self.proof.source,
                Some(&self.proof.generation),
                self.position,
                max,
            )?
        };
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
            SeekFrom::End(offset) => self.proof.source.offset.checked_add_signed(offset),
        }
        .filter(|position| *position <= self.proof.source.offset);
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
            let mut sources = Sources::default();
            sources.paths.insert(
                (reference.root.clone(), reference.path.clone()),
                self.path.clone(),
            );
            sources.proofs.insert(
                self.path.clone(),
                Proof {
                    source: reference.clone(),
                    ledger: Ledger {
                        stat,
                        offset: reference.offset,
                        prefix: captured.prefix,
                        tail: captured.tail,
                    },
                    generation: generation.clone(),
                    chunks: Arc::new(captured.chunks),
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
            let mut cache =
                crate::events::EventCache::open_scoped(&fixture.root.join("index.json")).unwrap();
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
                let mut cache =
                    crate::events::EventCache::open_scoped(&fixture.root.join("index.json"))
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
        let mut cache =
            crate::events::EventCache::open_scoped(&fixture.root.join("index.json")).unwrap();
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
        let mut cache = crate::events::EventCache::open_scoped(&cache_path).unwrap();
        let initial =
            crate::events::scan_file(&fixture.path, "claude", &mut cache, &mut false).unwrap();
        assert_eq!(initial.handoff_index.as_ref().unwrap().calls().len(), 1);
        let before = crate::events::PARSED.with(std::cell::Cell::get);
        drop(cache);
        let connection =
            rusqlite::Connection::open(crate::events::EventCache::path(&cache_path)).unwrap();
        connection
            .execute(
                "DELETE FROM meta WHERE key GLOB 'native_launch_calls:*'",
                [],
            )
            .unwrap();
        let mut cache = crate::events::EventCache::open_scoped(&cache_path).unwrap();
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
