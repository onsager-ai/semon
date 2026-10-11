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
            failed: Mutex::new(false),
        }
    }
}

/// The verification cache belongs to one read, never the shared query cohort.
pub(crate) struct Records<'a> {
    sources: &'a Sources,
    checked: Mutex<BTreeMap<PathBuf, Stat>>,
    checked_chunks: Mutex<BTreeMap<PathBuf, BTreeSet<usize>>>,
    failed: Mutex<bool>,
}

impl Records<'_> {
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
        Ok(())
    }
    fn range(
        &self,
        source: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
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
        for index in first..=last {
            let part = proof.chunks.read(&mut file, index)?;
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
        let result = self.range(source, expected, offset, max);
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
}

impl Read for RangeFile<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let max = buffer
            .len()
            .min(crate::SESSION_SOURCE_CHUNK_MAX)
            .min(usize::try_from(self.proof.source.offset - self.position).unwrap_or(usize::MAX));
        if max == 0 {
            return Ok(0);
        }
        let part = self.records.read_range(
            &self.proof.source,
            Some(&self.proof.generation),
            self.position,
            max,
        )?;
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
    use sha2::{Digest, Sha256};
    use std::fs;

    #[test]
    fn a_fixed_range_and_completion_do_not_hash_unrelated_source_bytes() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root =
            std::env::temp_dir().join(format!("semon-native-range-budget-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let fixture = Fixture(root);
        let path = fixture.0.join("lead.jsonl");
        for length in [1024 * 1024, 16 * 1024 * 1024] {
            let bytes = format!(
                "{}\n",
                serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"x".repeat(length)}]}})
            );
            fs::write(&path, &bytes).unwrap();
            let captured = crate::source_chunks::capture(
                &mut LogFile::open(&path).unwrap(),
                bytes.len() as u64,
                0,
            )
            .unwrap();
            let stat = Stat::from_metadata(&fs::metadata(&path).unwrap()).unwrap();
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
                path.clone(),
            );
            sources.proofs.insert(
                path.clone(),
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
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let fixture_path = std::env::temp_dir().join(format!(
            "semon-native-range-generation-{}",
            std::process::id()
        ));
        fs::create_dir(&fixture_path).unwrap();
        let fixture = Fixture(fixture_path);
        let path = fixture.0.join("lead.jsonl");
        let original = b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"old evidence\"}]}}\n";
        let replacement = b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"new evidence\"}]}}\n";
        assert_eq!(original.len(), replacement.len());
        let digest: [u8; 32] = Sha256::digest(original).into();
        fs::write(&path, original).unwrap();
        let before = fs::metadata(&path).unwrap();
        let stat = Stat::from_metadata(&before).unwrap();
        let reference = SessionSourceRef {
            root: "claude".into(),
            path: "lead.jsonl".into(),
            native_id: "lead".into(),
            offset: original.len() as u64,
            prefix_sha256: digest,
            tail_sha256: digest,
        };
        let generation = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut sources = Sources::default();
        sources.paths.insert(
            (reference.root.clone(), reference.path.clone()),
            path.clone(),
        );
        sources.proofs.insert(
            path.clone(),
            Proof {
                source: reference,
                ledger: Ledger {
                    stat,
                    offset: original.len() as u64,
                    prefix: digest,
                    tail: digest,
                },
                generation,
                chunks: Arc::new(crate::source_chunks::Chunks {
                    offset: original.len() as u64,
                    hashes: vec![digest],
                }),
            },
        );
        let records = sources.reader();
        assert_eq!(
            records.derive(&path, 0).unwrap().unwrap()["message"]["content"][0]["text"],
            "old evidence"
        );
        fs::write(&path, replacement).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(before.modified().unwrap())
            .unwrap();
        let after = Stat::from_metadata(&fs::metadata(&path).unwrap()).unwrap();
        assert_eq!(stat, after, "every captured stamp field must match");
        let proof = sources.proofs.get(&path).unwrap();
        assert!(
            !proof
                .ledger
                .resumes(&after, &mut LogFile::open(&path).unwrap())
        );
        assert!(
            records.finish().is_err(),
            "completion must recheck used bytes"
        );
        let result = records.derive(&path, 0);
        let finished = records.finish();
        assert!(
            result.is_err() && finished.is_err(),
            "captured old-generation proof admitted: {result:?}; finish={finished:?}"
        );
    }
}
