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
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct Proof {
    source: SessionSourceRef,
    ledger: Ledger,
    generation: String,
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
        sources: impl Iterator<Item = CatalogSource>,
    ) -> io::Result<Self> {
        let mut captured = Self::default();
        for source in sources {
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
            failed: Mutex::new(false),
        }
    }
}

/// The verification cache belongs to one read, never the shared query cohort.
pub(crate) struct Records<'a> {
    sources: &'a Sources,
    checked: Mutex<BTreeMap<PathBuf, Stat>>,
    failed: Mutex<bool>,
}

impl Records<'_> {
    /// Derivation preserves the shared producer's 64 MiB native-record limit.
    /// Its short text cap is applied by Texts after the unchanged extractor.
    pub(crate) fn derive(&self, path: &Path, offset: u64) -> io::Result<Option<serde_json::Value>> {
        let result = (|| {
            let proof = self.sources.proofs.get(path).ok_or_else(unavailable)?;
            if offset >= proof.source.offset {
                return Err(unavailable());
            }
            let mut reader = BufReader::new(RangeFile {
                records: self,
                proof,
                position: offset,
            })
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
        if proof.source.offset > 0 {
            self.read_range(&proof.source, Some(&proof.generation), 0, 1)?;
        } else {
            let mut file = LogFile::open(path)?;
            let before = Stat::from_metadata(&file.metadata()?)?;
            if !proof.ledger.resumes(&before, &mut file)
                || Stat::from_metadata(&file.metadata()?)? != before
            {
                return Err(unavailable());
            }
            self.checked
                .lock()
                .map_err(|_| unavailable())?
                .insert(path.to_owned(), before);
        }
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
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; max];
        file.read_exact(&mut bytes)?;
        if Stat::from_metadata(&file.metadata()?)? != before
            || (before.changed_ns.is_none() && !proof.ledger.resumes(&before, &mut file))
        {
            return Err(unavailable());
        }
        checked.insert(path.clone(), before);
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
