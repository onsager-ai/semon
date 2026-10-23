//! Private, restartable transport views of native archives. No writes to homes.
//!
//! Only a fully decoded, unchanged source becomes visible to the existing
//! complete-line/redaction sender. Both encoded and decoded work are bounded
//! per scheduling turn, including concatenated and skippable Zstd frames.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use zstd::stream::raw::{DParameter, Decoder, Operation};

const SLICE: usize = 1 << 20;
const BUFFER: usize = 64 << 10;
const MAX_JOBS: usize = 4;
// Logical cache usage, including unfinished output. This is independent of
// receiver quota accounting, which still charges every redacted logical byte.
const MAX_BYTES: u64 = 20 << 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Witness {
    stat: super::FileStat,
    changed_ns: Option<i128>,
}

impl Witness {
    fn of(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        #[cfg(unix)]
        let changed_ns = {
            use std::os::unix::fs::MetadataExt;
            Some(i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()))
        };
        #[cfg(not(unix))]
        let changed_ns = None;
        Ok(Self {
            stat: super::FileStat::from_metadata(&metadata),
            changed_ns,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Complete {
    source: Witness,
    output: Witness,
}

struct Job {
    source: File,
    witness: Witness,
    decoder: Decoder<'static>,
    output: File,
    temporary: PathBuf,
    encoded: u64,
    decoded: u64,
    input: Vec<u8>,
    at: usize,
    boundary: bool,
}

struct Comparison {
    archive: Option<File>,
    plain: Option<File>,
    archive_witness: Witness,
    plain_witness: Witness,
    at: u64,
    complete: bool,
}

impl Drop for Job {
    fn drop(&mut self) {
        // Removing a named temporary never follows a link. A completed job
        // has already renamed it; failed/stopped jobs leave no usable view.
        let _ = fs::remove_file(&self.temporary);
    }
}

pub(crate) struct Cache {
    dir: PathBuf,
    jobs: BTreeMap<PathBuf, Job>,
    used: u64,
    initialized: bool,
    comparisons: BTreeMap<PathBuf, Comparison>,
    failures: BTreeMap<PathBuf, (Witness, String)>,
}

impl Cache {
    pub(crate) fn new(state: &Path) -> Self {
        let mut name = state.as_os_str().to_owned();
        name.push(".archives");
        Self {
            dir: PathBuf::from(name),
            jobs: BTreeMap::new(),
            used: 0,
            initialized: false,
            comparisons: BTreeMap::new(),
            failures: BTreeMap::new(),
        }
    }

    pub(crate) fn retain(&mut self, sources: &std::collections::BTreeSet<PathBuf>) {
        self.jobs.retain(|source, job| {
            let keep = sources.contains(source);
            if !keep {
                self.used = self.used.saturating_sub(job.decoded);
            }
            keep
        });
        self.comparisons
            .retain(|source, _| sources.contains(source));
        self.failures.retain(|source, _| sources.contains(source));
    }

    pub(crate) fn active(&self, source: &Path) -> bool {
        self.jobs.contains_key(source)
            || self
                .comparisons
                .get(source)
                .is_some_and(|comparison| !comparison.complete)
    }

    #[cfg(test)]
    pub(crate) fn jobs_len_for_test(&self) -> usize {
        self.jobs.len()
    }

    fn initialize(&mut self) -> io::Result<()> {
        if self.initialized {
            return Ok(());
        }
        super::create_private_dir(&self.dir)?;
        let metadata = fs::symlink_metadata(&self.dir)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("archive cache is not a private directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(invalid("archive cache must have mode 0700"));
            }
        }
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|ext| ext == "tmp") {
                fs::remove_file(entry.path())?;
            } else if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                self.used = self.used.saturating_add(entry.metadata()?.len());
            }
        }
        self.initialized = true;
        Ok(())
    }

    /// One bounded preparation turn. None means yield to other sources.
    pub(crate) fn step(
        &mut self,
        source: &Path,
        plain: Option<&Path>,
    ) -> io::Result<Option<PathBuf>> {
        let result = self.step_inner(source, plain);
        if result.is_err() {
            self.comparisons.remove(source);
        }
        result
    }

    fn step_inner(&mut self, source: &Path, plain: Option<&Path>) -> io::Result<Option<PathBuf>> {
        let Some(view) = self.prepare(source)? else {
            return Ok(None);
        };
        let Some(plain) = plain else {
            self.comparisons.remove(source);
            return Ok(Some(view));
        };
        let archive_file = semon_sessions::open_read_only_input(&view)?;
        let plain_file = semon_sessions::open_read_only_input(plain)?;
        let archive_witness = Witness::of(&archive_file)?;
        let plain_witness = Witness::of(&plain_file)?;
        let changed = self.comparisons.get(source).is_none_or(|comparison| {
            comparison.archive_witness != archive_witness
                || comparison.plain_witness != plain_witness
        });
        if changed {
            self.comparisons.remove(source);
            if self
                .comparisons
                .values()
                .filter(|comparison| !comparison.complete)
                .count()
                >= MAX_JOBS
            {
                return Ok(None);
            }
            self.comparisons.insert(
                source.to_owned(),
                Comparison {
                    archive: Some(archive_file),
                    plain: Some(plain_file),
                    archive_witness,
                    plain_witness,
                    at: 0,
                    complete: false,
                },
            );
        }
        let comparison = self.comparisons.get_mut(source).expect("inserted");
        if !comparison.complete {
            let common = comparison
                .archive_witness
                .stat
                .len
                .min(comparison.plain_witness.stat.len);
            let count = (common - comparison.at).min(SLICE as u64) as usize;
            let mut left = vec![0; count];
            let mut right = vec![0; count];
            let read = comparison
                .archive
                .as_mut()
                .expect("active comparison")
                .read_exact(&mut left)
                .and_then(|()| {
                    comparison
                        .plain
                        .as_mut()
                        .expect("active comparison")
                        .read_exact(&mut right)
                });
            if let Err(error) = read {
                self.comparisons.remove(source);
                return Err(error);
            }
            if left != right {
                self.comparisons.remove(source);
                return Err(invalid(
                    "plain and compressed log disagree; refusing ambiguous source identity",
                ));
            }
            comparison.at += count as u64;
            if comparison.at < common {
                return Ok(None);
            }
            if Witness::of(comparison.archive.as_ref().expect("active comparison"))?
                != comparison.archive_witness
                || Witness::of(comparison.plain.as_ref().expect("active comparison"))?
                    != comparison.plain_witness
                || Witness::of(&semon_sessions::open_read_only_input(&view)?)?
                    != comparison.archive_witness
                || Witness::of(&semon_sessions::open_read_only_input(plain)?)?
                    != comparison.plain_witness
            {
                self.comparisons.remove(source);
                return Err(invalid(
                    "log changed while comparing archive and plain sources",
                ));
            }
            comparison.complete = true;
            comparison.archive = None;
            comparison.plain = None;
        }
        // Never discard a longer history just because a plain name exists.
        // Equality and append containment are proven over every common byte.
        Ok(Some(
            if comparison.plain_witness.stat.len >= comparison.archive_witness.stat.len {
                plain.to_owned()
            } else {
                view
            },
        ))
    }

    fn prepare(&mut self, source: &Path) -> io::Result<Option<PathBuf>> {
        self.initialize()?;
        let key = super::sha256_hex(source.as_os_str().as_encoded_bytes());
        let view = self.dir.join(format!("{key}.jsonl"));
        let marker = self.dir.join(format!("{key}.json"));
        let opened = semon_sessions::open_read_only_input(source)?;
        let witness = Witness::of(&opened)?;
        if let Some((failed, message)) = self.failures.get(source)
            && *failed == witness
        {
            return Err(invalid(message));
        }
        self.failures.remove(source);
        if witness.changed_ns.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native archive transport requires filesystem change-time witnesses",
            ));
        }
        if let Ok(file) = semon_sessions::open_read_only_input(&marker)
            && file.metadata()?.len() <= 16 << 10
            && let Ok(complete) = serde_json::from_reader::<_, Complete>(file)
            && complete.source == witness
            && let Ok(output) = semon_sessions::open_read_only_input(&view)
            && Witness::of(&output)? == complete.output
        {
            return Ok(Some(view));
        }
        if self
            .jobs
            .get(source)
            .is_some_and(|job| job.witness != witness)
        {
            let job = self.jobs.remove(source).expect("present");
            self.used = self.used.saturating_sub(job.decoded);
        }
        if !self.jobs.contains_key(source) {
            if self.jobs.len() >= MAX_JOBS {
                return Ok(None);
            }
            // Invalidate the completion proof before replacing an old view.
            remove(&marker)?;
            if let Ok(metadata) = fs::symlink_metadata(&view) {
                remove(&view)?;
                self.used = self.used.saturating_sub(metadata.len());
            }
            let temporary = self.dir.join(format!("{key}.tmp"));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut decoder = Decoder::new()?;
            decoder.set_parameter(DParameter::WindowLogMax(23))?;
            let output = options.open(&temporary)?;
            self.jobs.insert(
                source.to_owned(),
                Job {
                    source: opened,
                    witness,
                    decoder,
                    output,
                    temporary,
                    encoded: 0,
                    decoded: 0,
                    input: Vec::new(),
                    at: 0,
                    boundary: false,
                },
            );
        }
        let job = self.jobs.get_mut(source).expect("inserted");
        let before = job.decoded;
        let result = job.step(MAX_BYTES.saturating_sub(self.used));
        self.used = self.used.saturating_add(job.decoded - before);
        match result {
            Ok(false) => Ok(None),
            Ok(true) => {
                let job = self.jobs.remove(source).expect("present");
                let publish = || -> io::Result<()> {
                    if Witness::of(&job.source)? != job.witness
                        || Witness::of(&semon_sessions::open_read_only_input(source)?)?
                            != job.witness
                    {
                        return Err(invalid("archive changed during decompression"));
                    }
                    job.output.sync_all()?;
                    fs::rename(&job.temporary, &view)?;
                    let complete = Complete {
                        source: job.witness.clone(),
                        output: Witness::of(&semon_sessions::open_read_only_input(&view)?)?,
                    };
                    super::write_private(&marker, &serde_json::to_vec(&complete)?)?;
                    File::open(&self.dir)?.sync_all()
                };
                if let Err(error) = publish() {
                    remove(&marker)?;
                    remove(&view)?;
                    self.used = self.used.saturating_sub(job.decoded);
                    return Err(error);
                }
                Ok(Some(view))
            }
            Err(error) => {
                let job = self.jobs.remove(source).expect("present");
                self.used = self.used.saturating_sub(job.decoded);
                if error.kind() == io::ErrorKind::InvalidData {
                    self.failures
                        .insert(source.to_owned(), (job.witness.clone(), error.to_string()));
                }
                Err(error)
            }
        }
    }
}

impl Job {
    fn step(&mut self, available: u64) -> io::Result<bool> {
        let mut output = vec![0; BUFFER];
        let mut read = 0;
        let before = self.decoded;
        for _ in 0..256 {
            if self.at == self.input.len() {
                if self.encoded == self.witness.stat.len && self.boundary {
                    return Ok(true);
                }
                if read >= SLICE {
                    return Ok(false);
                }
                self.input.resize(
                    BUFFER.min((self.witness.stat.len - self.encoded) as usize),
                    0,
                );
                let count = self.source.read(&mut self.input)?;
                if count == 0 && self.encoded < self.witness.stat.len {
                    return Err(invalid("archive was truncated during decompression"));
                }
                self.input.truncate(count);
                self.at = 0;
                self.encoded += count as u64;
                read += count;
            }
            let status = self
                .decoder
                .run_on_buffers(&self.input[self.at..], &mut output)
                .map_err(|error| invalid(&format!("invalid Zstd archive: {error}")))?;
            self.at += status.bytes_read;
            self.boundary = status.remaining == 0;
            if status.bytes_read == 0 && status.bytes_written == 0 {
                return Err(invalid("truncated Zstd archive"));
            }
            let growth = self.decoded - before + status.bytes_written as u64;
            if growth > available {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "archive cache exceeds its 20 GiB logical byte limit",
                ));
            }
            self.output.write_all(&output[..status.bytes_written])?;
            self.decoded += status.bytes_written as u64;
            if self.decoded - before >= SLICE as u64 {
                return Ok(false);
            }
        }
        Ok(false)
    }
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "semon-archives-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn cache(&self) -> Cache {
            Cache::new(&self.0.join("push.json"))
        }
        fn source(&self, bytes: &[u8]) -> PathBuf {
            let path = self.0.join("history.jsonl.zst");
            fs::write(&path, bytes).unwrap();
            path
        }
        fn finish(
            &self,
            cache: &mut Cache,
            source: &Path,
            plain: Option<&Path>,
        ) -> io::Result<PathBuf> {
            for _ in 0..1024 {
                if let Some(path) = cache.step(source, plain)? {
                    return Ok(path);
                }
            }
            panic!("archive preparation failed to make progress");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn decode_is_sliced_and_reused_across_restart_without_changing_source() {
        let fixture = Fixture::new();
        let raw = b"{\"message\":\"sanitized history\"}\n".repeat(120_000);
        let encoded = zstd::stream::encode_all(raw.as_slice(), 3).unwrap();
        let source = fixture.source(&encoded);
        let original = Witness::of(&File::open(&source).unwrap()).unwrap();
        let mut cache = fixture.cache();
        assert!(cache.step(&source, None).unwrap().is_none());
        assert!(cache.jobs[&source].decoded <= SLICE as u64);
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(fs::read(&view).unwrap(), raw);
        let before = Witness::of(&File::open(&view).unwrap()).unwrap();
        drop(cache);
        let mut restarted = fixture.cache();
        assert_eq!(restarted.step(&source, None).unwrap(), Some(view.clone()));
        assert!(
            restarted.jobs.is_empty(),
            "valid cache must not decompress on restart"
        );
        assert_eq!(Witness::of(&File::open(&view).unwrap()).unwrap(), before);
        assert_eq!(
            Witness::of(&File::open(&source).unwrap()).unwrap(),
            original
        );
        assert_eq!(fs::read(source).unwrap(), encoded);
    }

    #[test]
    fn unchanged_corruption_does_not_repeat_decompression_and_a_repaired_source_retries() {
        let fixture = Fixture::new();
        let mut encoded = zstd::stream::encode_all(b"{}\n".as_slice(), 3).unwrap();
        encoded.pop();
        let source = fixture.source(&encoded);
        let mut cache = fixture.cache();
        assert!(fixture.finish(&mut cache, &source, None).is_err());
        assert!(cache.failures.contains_key(&source));
        assert!(cache.step(&source, None).is_err());
        assert!(cache.jobs.is_empty());
        fs::write(
            &source,
            zstd::stream::encode_all(b"repaired\n".as_slice(), 3).unwrap(),
        )
        .unwrap();
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(fs::read(view).unwrap(), b"repaired\n");
    }

    #[test]
    fn concatenated_frames_partial_records_and_replacement_are_exact() {
        let fixture = Fixture::new();
        let mut encoded = zstd::stream::encode_all(b"first\n".as_slice(), 3).unwrap();
        encoded.extend(zstd::stream::encode_all(b"partial".as_slice(), 3).unwrap());
        let source = fixture.source(&encoded);
        let mut cache = fixture.cache();
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(fs::read(&view).unwrap(), b"first\npartial");
        // Changes beyond a shared head cannot reuse a completed cache proof.
        let new = zstd::stream::encode_all(b"first\nreplacement\n".as_slice(), 3).unwrap();
        fs::write(&source, new).unwrap();
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(fs::read(view).unwrap(), b"first\nreplacement\n");
    }

    #[test]
    fn corrupt_truncated_and_trailing_garbage_never_publish_a_view() {
        let fixture = Fixture::new();
        let encoded = zstd::stream::encode_all(b"{}\n{}\n".as_slice(), 3).unwrap();
        for cut in 0..encoded.len() {
            let source = fixture.source(&encoded[..cut]);
            let mut cache = fixture.cache();
            assert!(
                fixture.finish(&mut cache, &source, None).is_err(),
                "cut {cut}"
            );
            assert!(cache.jobs.is_empty());
            assert!(!fs::read_dir(&cache.dir).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "jsonl")
            }));
        }
        let mut corrupt = encoded;
        corrupt.extend_from_slice(b"invalid frame");
        let source = fixture.source(&corrupt);
        assert!(fixture.finish(&mut fixture.cache(), &source, None).is_err());
    }

    #[test]
    fn skippable_frames_consume_the_encoded_work_budget() {
        let fixture = Fixture::new();
        let mut encoded = 0x184D2A50_u32.to_le_bytes().to_vec();
        encoded.extend_from_slice(&(3 * SLICE as u32).to_le_bytes());
        encoded.resize(3 * SLICE + 8, 0);
        encoded.extend(zstd::stream::encode_all(b"{}\n".as_slice(), 3).unwrap());
        let source = fixture.source(&encoded);
        let mut cache = fixture.cache();
        assert!(cache.step(&source, None).unwrap().is_none());
        assert!(cache.jobs[&source].encoded <= SLICE as u64);
        assert_eq!(cache.jobs[&source].decoded, 0);
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(fs::read(view).unwrap(), b"{}\n");
    }

    #[test]
    fn output_quota_and_decoder_window_are_enforced_before_publication() {
        let fixture = Fixture::new();
        let raw = b"{}\n".repeat(1024);
        let source = fixture.source(&zstd::stream::encode_all(raw.as_slice(), 3).unwrap());
        let mut cache = fixture.cache();
        cache.initialize().unwrap();
        cache.used = MAX_BYTES - 32;
        assert!(
            cache
                .step(&source, None)
                .unwrap_err()
                .to_string()
                .contains("20 GiB")
        );
        assert!(cache.jobs.is_empty());
        assert_eq!(cache.used, MAX_BYTES - 32);
        // Valid frame: no declared content size, 16 MiB window, final raw
        // block containing three bytes. Window admission precedes output.
        let large_window = [
            0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x70, 0x19, 0, 0, b'{', b'}', b'\n',
        ];
        assert_eq!(
            zstd::stream::decode_all(large_window.as_slice()).unwrap(),
            b"{}\n"
        );
        fs::write(&source, large_window).unwrap();
        assert!(
            fixture.finish(&mut fixture.cache(), &source, None).is_err(),
            "a frame with a window above 8 MiB must be rejected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn archives_and_cache_views_never_follow_symbolic_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let source = fixture.source(&zstd::stream::encode_all(b"{}\n".as_slice(), 3).unwrap());
        let link = fixture.0.join("linked.jsonl.zst");
        symlink(&source, &link).unwrap();
        assert!(fixture.finish(&mut fixture.cache(), &link, None).is_err());
        let mut cache = fixture.cache();
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(
            fs::metadata(&view).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let outside = fixture.0.join("outside");
        fs::write(&outside, b"unrelated private bytes").unwrap();
        fs::remove_file(&view).unwrap();
        symlink(&outside, &view).unwrap();
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(fs::read(&view).unwrap(), b"{}\n");
        assert_eq!(fs::read(&outside).unwrap(), b"unrelated private bytes");
    }

    #[test]
    fn coexisting_encodings_select_only_verified_longer_content_and_release_descriptors() {
        let fixture = Fixture::new();
        let source =
            fixture.source(&zstd::stream::encode_all(b"first\nsecond\n".as_slice(), 3).unwrap());
        let plain = fixture.0.join("history.jsonl");
        fs::write(&plain, b"first\n").unwrap();
        let mut cache = fixture.cache();
        let view = fixture.finish(&mut cache, &source, Some(&plain)).unwrap();
        assert_ne!(view, plain);
        assert_eq!(fs::read(view).unwrap(), b"first\nsecond\n");
        assert!(cache.comparisons[&source].archive.is_none());
        fs::write(&plain, b"first\nsecond\nthird\n").unwrap();
        assert_eq!(
            fixture.finish(&mut cache, &source, Some(&plain)).unwrap(),
            plain
        );
        fs::write(&plain, b"DIFFERENT SESSION\n").unwrap();
        assert!(fixture.finish(&mut cache, &source, Some(&plain)).is_err());
    }

    #[test]
    fn changed_views_are_rebuilt_and_cancelled_jobs_release_their_budget() {
        let fixture = Fixture::new();
        let raw = b"{}\n".repeat(SLICE);
        let source = fixture.source(&zstd::stream::encode_all(raw.as_slice(), 3).unwrap());
        let mut cache = fixture.cache();
        assert!(cache.step(&source, None).unwrap().is_none());
        cache.retain(&Default::default());
        assert_eq!(cache.used, 0);
        assert!(cache.jobs.is_empty());
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        fs::write(&view, b"forged bytes\n").unwrap();
        let view = fixture.finish(&mut cache, &source, None).unwrap();
        assert_eq!(fs::read(view).unwrap(), raw);
    }
}
