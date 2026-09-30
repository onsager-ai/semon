//! Sealed logs: a log's older bytes kept as zstd segments beside it, with
//! the sealed range punched out of the plain file.
//!
//! A receiver that mirrors logs may seal them to save space ([`seal`]).
//! Nothing about a sealed log changes for a reader that goes through
//! [`LogFile`], which every log read in this crate does. For a log `F`:
//! - `F` keeps its path, inode, length and modified time, so every byte
//!   offset, stat-keyed cache and ledger stays valid. A sealed range is a
//!   hole in it: no blocks, and it reads back as zeros.
//! - `F.seal/manifest.json` is its [`Manifest`]: the segments, contiguous
//!   from offset 0, each ending on a line boundary, and the inode it was
//!   sealed from.
//! - `F.seal/<generation>-<start>.zst` is one segment: the bytes
//!   `[start, end)` of `F` in the zstd seekable format (frames of
//!   [`FRAME_BYTES`], then the seek table in a skippable frame), so plain
//!   `zstd -d` reads it back.
//!
//! [`LogFile`] reads a sealed range from its segment whenever the manifest
//! is *bound*: recorded from this very inode. A NUL byte read from the plain
//! file is only a hint that a seal happened after the file was opened: the
//! manifest is read again and, if it changed, the read is retried. A manifest
//! recorded from another inode (the log was copied, or replaced by a new
//! file) is *unbound*: the plain file is read, and only its NUL bytes inside
//! a sealed range are filled from the segments. JSON never holds a raw NUL,
//! so that fill touches nothing a live log wrote.
//!
//! Only the log's one writer may seal it, holding whatever lock orders its
//! writes: [`seal`] punches holes and restores the modified time.

use std::{
    collections::VecDeque,
    fs,
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The plain bytes in one frame of a segment.
pub const FRAME_BYTES: usize = 1 << 20;
/// What a segment's frame may claim at most when read back.
const MAX_FRAME_BYTES: u32 = 8 << 20;
/// zstd's level for new segments: fast, and about 6x on session logs.
const LEVEL: i32 = 3;
const SKIPPABLE_MAGIC: u32 = 0x184D_2A5E;
const SEEKABLE_MAGIC: u32 = 0x8F92_EAB1;
/// The seek table's footer: the frame count, a descriptor byte, the magic.
const FOOTER: u64 = 9;
/// Holes are punched in whole blocks of this size.
const BLOCK: u64 = 4096;
/// Blocks a plain file may hold past its unsealed tail (extent metadata)
/// before [`Status::needs_punch`] says its sealed range still holds data.
const PUNCH_SLACK: u64 = 4 * BLOCK;
/// Decompressed frames kept for all readers of this process.
const CACHE_FRAMES: usize = 16;
/// The largest manifest read.
const MANIFEST_MAX: u64 = 16 << 20;
const MANIFEST: &str = "manifest.json";
const VERSION: u32 = 1;

/// A sealed log's manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// Names this manifest's segment files; a new one starts when the log
    /// was replaced by other content.
    pub generation: String,
    /// The log's inode when the manifest was last written (0 off Unix).
    pub ino: u64,
    /// The log's length and modified time (ns since the epoch) when last
    /// sealed: a seal interrupted after its punch restores that time.
    pub sealed_len: u64,
    pub sealed_mtime_ns: u64,
    pub segments: Vec<Segment>,
}

/// One segment: the log's bytes `[start, end)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    /// The segment file's size.
    pub bytes: u64,
    /// SHA-256 of the plain bytes, and of the segment file, lowercase hex.
    pub plain_sha256: String,
    pub zst_sha256: String,
}

impl Manifest {
    /// Where the sealed range ends: 0 with no segment.
    pub fn end(&self) -> u64 {
        self.segments.last().map_or(0, |segment| segment.end)
    }

    fn valid(&self) -> bool {
        let mut expected = 0;
        self.version == VERSION
            && is_generation(&self.generation)
            && self.segments.iter().all(|segment| {
                let fits = segment.start == expected
                    && segment.end > segment.start
                    && is_sha256_hex(&segment.plain_sha256)
                    && is_sha256_hex(&segment.zst_sha256);
                expected = segment.end;
                fits
            })
    }
}

impl Segment {
    /// The segment's file name in the seal directory.
    pub fn file_name(&self, generation: &str) -> String {
        format!("{generation}-{:020}.zst", self.start)
    }
}

fn is_generation(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

/// `log`'s seal directory: `<log>.seal`.
pub fn seal_dir(log: &Path) -> PathBuf {
    let mut name = log.as_os_str().to_owned();
    name.push(".seal");
    PathBuf::from(name)
}

/// Whether a directory entry's name is a log's seal directory, for walkers
/// that count or copy a mirror's logs.
pub fn is_seal_dir_name(name: &str) -> bool {
    name.len() > ".jsonl.seal".len() && name.ends_with(".jsonl.seal")
}

/// A file as last stat'ed: enough to tell a changed or replaced file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    modified_ns: i128,
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        let modified_ns =
            meta.modified()
                .ok()
                .map_or(0, |time| match time.duration_since(UNIX_EPOCH) {
                    Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
                    Err(before) => {
                        -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX)
                    }
                });
        let (dev, ino) = identity(meta);
        Self {
            dev,
            ino,
            len: meta.len(),
            modified_ns,
        }
    }
}

#[cfg(unix)]
fn identity(meta: &fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(_meta: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// Bytes a file holds on disk.
#[cfg(unix)]
fn allocated(meta: &fs::Metadata) -> u64 {
    meta.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated(meta: &fs::Metadata) -> u64 {
    meta.len()
}

fn mtime_ns(meta: &fs::Metadata) -> io::Result<u64> {
    let since = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Ok(u64::try_from(since.as_nanos()).unwrap_or(u64::MAX))
}

fn read_at(file: &fs::File, buf: &mut [u8], at: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_at(buf, at)
    }
    #[cfg(not(unix))]
    {
        let mut file = file;
        file.seek(SeekFrom::Start(at))?;
        file.read(buf)
    }
}

fn read_exact_at(file: &fs::File, buf: &mut [u8], at: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, at)
    }
    #[cfg(not(unix))]
    {
        let mut file = file;
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(buf)
    }
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// The manifest in `dir` and its stamp: `None` when there is none. One that
/// doesn't parse or doesn't hold together is an error.
fn read_manifest(dir: &Path) -> io::Result<Option<(Manifest, Stamp)>> {
    let file = match fs::File::open(dir.join(MANIFEST)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let meta = file.metadata()?;
    if meta.len() > MANIFEST_MAX {
        return Err(invalid("oversized seal manifest"));
    }
    let mut bytes = Vec::new();
    file.take(MANIFEST_MAX + 1).read_to_end(&mut bytes)?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| invalid("unreadable seal manifest"))?;
    if !manifest.valid() {
        return Err(invalid("inconsistent seal manifest"));
    }
    Ok(Some((manifest, Stamp::of(&meta))))
}

/// Writes `manifest` to a temporary file, syncs it and renames it over the
/// old one.
fn write_manifest(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    let temporary = dir.join(format!("{MANIFEST}.tmp"));
    let bytes = serde_json::to_vec(manifest).map_err(io::Error::other)?;
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, dir.join(MANIFEST))?;
    sync_dir(dir)
}

// ---- Segments ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Frame {
    /// Where the frame's plain bytes start, from the segment's start.
    plain_at: u64,
    plain_len: u32,
    file_at: u64,
    file_len: u32,
}

impl Frame {
    fn plain_end(&self) -> u64 {
        self.plain_at + u64::from(self.plain_len)
    }
}

/// A segment's seek table.
#[derive(Debug)]
struct Table {
    frames: Vec<Frame>,
    plain: u64,
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Reads and checks a segment file's seek table: every frame's sizes are
/// bounded and the frames exactly fill the file before the table.
fn read_table(file: &fs::File) -> io::Result<Table> {
    let len = file.metadata()?.len();
    if len < 8 + FOOTER {
        return Err(invalid("segment too short"));
    }
    let mut footer = [0; FOOTER as usize];
    read_exact_at(file, &mut footer, len - FOOTER)?;
    let count = u64::from(u32_at(&footer, 0));
    let descriptor = footer[4];
    if u32_at(&footer, 5) != SEEKABLE_MAGIC || descriptor & 0x7C != 0 {
        return Err(invalid("not a seekable zstd segment"));
    }
    let entry: u64 = if descriptor & 0x80 == 0 { 8 } else { 12 };
    let table_len = count
        .checked_mul(entry)
        .and_then(|entries| entries.checked_add(8 + FOOTER))
        .filter(|table_len| *table_len <= len)
        .ok_or_else(|| invalid("segment seek table overruns the file"))?;
    let mut bytes = vec![0; usize::try_from(table_len).map_err(|_| invalid("seek table"))?];
    read_exact_at(file, &mut bytes, len - table_len)?;
    if u32_at(&bytes, 0) != SKIPPABLE_MAGIC || u64::from(u32_at(&bytes, 4)) != table_len - 8 {
        return Err(invalid("segment seek table header"));
    }
    let mut frames = Vec::new();
    let (mut plain, mut at) = (0u64, 0u64);
    for index in 0..count as usize {
        let entry_at = 8 + index * entry as usize;
        let file_len = u32_at(&bytes, entry_at);
        let plain_len = u32_at(&bytes, entry_at + 4);
        if plain_len == 0
            || plain_len > MAX_FRAME_BYTES
            || file_len == 0
            || file_len > 2 * MAX_FRAME_BYTES
        {
            return Err(invalid("segment frame size"));
        }
        frames.push(Frame {
            plain_at: plain,
            plain_len,
            file_at: at,
            file_len,
        });
        plain += u64::from(plain_len);
        at += u64::from(file_len);
    }
    if at != len - table_len {
        return Err(invalid("segment frames don't fill the file"));
    }
    Ok(Table { frames, plain })
}

/// The seek table that ends a segment: a skippable frame of entries
/// (compressed size, plain size), then the footer.
fn seek_table(entries: &[(u32, u32)]) -> io::Result<Vec<u8>> {
    let count = u32::try_from(entries.len()).map_err(|_| invalid("too many frames"))?;
    let size = u32::try_from(entries.len() * 8 + FOOTER as usize)
        .map_err(|_| invalid("too many frames"))?;
    let mut table = Vec::with_capacity(8 + size as usize);
    table.extend_from_slice(&SKIPPABLE_MAGIC.to_le_bytes());
    table.extend_from_slice(&size.to_le_bytes());
    for (compressed, plain) in entries {
        table.extend_from_slice(&compressed.to_le_bytes());
        table.extend_from_slice(&plain.to_le_bytes());
    }
    table.extend_from_slice(&count.to_le_bytes());
    table.push(0);
    table.extend_from_slice(&SEEKABLE_MAGIC.to_le_bytes());
    Ok(table)
}

struct OpenSegment {
    file: fs::File,
    stamp: Stamp,
    table: Table,
}

fn open_segment(dir: &Path, generation: &str, segment: &Segment) -> io::Result<OpenSegment> {
    let file = fs::File::open(dir.join(segment.file_name(generation)))?;
    let stamp = Stamp::of(&file.metadata()?);
    let table = read_table(&file)?;
    if table.plain != segment.end - segment.start {
        return Err(invalid("segment length differs from its manifest"));
    }
    Ok(OpenSegment { file, stamp, table })
}

type FrameKey = (Stamp, usize);

/// Decompressed frames, most recent last, shared by every reader.
static FRAMES: Mutex<VecDeque<(FrameKey, Arc<[u8]>)>> = Mutex::new(VecDeque::new());

fn decompress(segment: &OpenSegment, index: usize) -> io::Result<Vec<u8>> {
    let frame = segment.table.frames[index];
    let mut compressed = vec![0; frame.file_len as usize];
    read_exact_at(&segment.file, &mut compressed, frame.file_at)?;
    let plain = zstd::bulk::decompress(&compressed, frame.plain_len as usize)?;
    if plain.len() != frame.plain_len as usize {
        return Err(invalid("segment frame decompressed short"));
    }
    Ok(plain)
}

/// Frame `index` of `segment`, from the shared cache or decompressed.
fn frame_bytes(segment: &OpenSegment, index: usize) -> io::Result<Arc<[u8]>> {
    let key = (segment.stamp, index);
    let cached = FRAMES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(seen, _)| *seen == key)
        .map(|(_, bytes)| Arc::clone(bytes));
    if let Some(bytes) = cached {
        return Ok(bytes);
    }
    let bytes: Arc<[u8]> = decompress(segment, index)?.into();
    let mut frames = FRAMES.lock().unwrap_or_else(PoisonError::into_inner);
    if frames.len() >= CACHE_FRAMES {
        frames.pop_front();
    }
    frames.push_back((key, Arc::clone(&bytes)));
    Ok(bytes)
}

// ---- Reading ----------------------------------------------------------------------------

struct Loaded {
    manifest: Manifest,
    bound: bool,
    segments: Vec<Option<OpenSegment>>,
}

/// A log opened for reading: [`Read`] and [`Seek`] over its bytes, sealed
/// or not, at the offsets of the plain file. See the module docs.
pub struct LogFile {
    file: fs::File,
    dir: PathBuf,
    ino: u64,
    pos: u64,
    seal: Option<Loaded>,
    /// The manifest's stamp when last read: `None` when there was none.
    seen: Option<Stamp>,
    /// The last frame read: segment, frame, bytes.
    frame: Option<(usize, usize, Arc<[u8]>)>,
}

impl LogFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let ino = identity(&file.metadata()?).1;
        let mut log = Self {
            file,
            dir: seal_dir(path),
            ino,
            pos: 0,
            seal: None,
            seen: None,
            frame: None,
        };
        log.reload();
        Ok(log)
    }

    /// The plain file's metadata: the log's length, inode and modified time.
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        self.file.metadata()
    }

    /// Reads the manifest again if it changed since last read; returns
    /// whether it did. A manifest that can't be read counts as none: its
    /// sealed range then reads as the plain file does.
    fn reload(&mut self) -> bool {
        let stamp = fs::metadata(self.dir.join(MANIFEST))
            .ok()
            .map(|meta| Stamp::of(&meta));
        if stamp == self.seen && (stamp.is_none() || self.seal.is_some()) {
            return false;
        }
        let read = read_manifest(&self.dir).ok().flatten();
        self.seen = read.as_ref().map(|(_, stamp)| *stamp);
        self.frame = None;
        self.seal = read.map(|(manifest, _)| Loaded {
            bound: manifest.ino == self.ino,
            segments: std::iter::repeat_with(|| None)
                .take(manifest.segments.len())
                .collect(),
            manifest,
        });
        true
    }

    /// The segment and frame holding `at`, which the sealed range holds,
    /// with that frame's bytes and where they start in the log.
    fn frame_at(&mut self, at: u64) -> io::Result<(Arc<[u8]>, u64)> {
        let seal = self.seal.as_mut().ok_or_else(|| invalid("not sealed"))?;
        let index = seal
            .manifest
            .segments
            .partition_point(|segment| segment.end <= at);
        let segment = seal
            .manifest
            .segments
            .get(index)
            .ok_or_else(|| invalid("past the sealed range"))?;
        if seal.segments[index].is_none() {
            seal.segments[index] =
                Some(open_segment(&self.dir, &seal.manifest.generation, segment)?);
        }
        let open = seal.segments[index].as_ref().expect("opened above");
        let within = at - segment.start;
        let frame_index = open
            .table
            .frames
            .partition_point(|frame| frame.plain_end() <= within);
        let frame = open
            .table
            .frames
            .get(frame_index)
            .ok_or_else(|| invalid("past the segment"))?;
        let frame_start = segment.start + frame.plain_at;
        if let Some((seen_segment, seen_frame, bytes)) = &self.frame
            && (*seen_segment, *seen_frame) == (index, frame_index)
        {
            return Ok((Arc::clone(bytes), frame_start));
        }
        let bytes = frame_bytes(open, frame_index)?;
        self.frame = Some((index, frame_index, Arc::clone(&bytes)));
        Ok((bytes, frame_start))
    }

    /// Where the bound sealed range ends, if `self.pos` is inside it.
    fn bound_end(&self) -> Option<u64> {
        self.seal
            .as_ref()
            .filter(|seal| seal.bound)
            .map(|seal| seal.manifest.end())
            .filter(|end| self.pos < *end)
    }

    /// Fills the NUL bytes of `buf`, read at `self.pos`, that an unbound
    /// manifest's sealed range covers.
    fn fill_unbound(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let Some(end) = self
            .seal
            .as_ref()
            .filter(|seal| !seal.bound)
            .map(|seal| seal.manifest.end())
        else {
            return Ok(());
        };
        let mut index = 0;
        while let Some(zero) = buf[index..].iter().position(|byte| *byte == 0) {
            index += zero;
            let at = self.pos + index as u64;
            if at >= end {
                break;
            }
            let (bytes, frame_start) = self.frame_at(at)?;
            let frame_end = frame_start + bytes.len() as u64;
            let stop = buf.len().min((frame_end.min(end) - self.pos) as usize);
            for (offset, byte) in buf.iter_mut().enumerate().take(stop).skip(index) {
                if *byte == 0 {
                    *byte = bytes[(self.pos + offset as u64 - frame_start) as usize];
                }
            }
            index = stop;
        }
        Ok(())
    }
}

impl Read for LogFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut reloaded = false;
        loop {
            if self.bound_end().is_some() {
                let (bytes, frame_start) = self.frame_at(self.pos)?;
                let from = (self.pos - frame_start) as usize;
                let count = buf.len().min(bytes.len() - from);
                buf[..count].copy_from_slice(&bytes[from..from + count]);
                self.pos += count as u64;
                return Ok(count);
            }
            let count = read_at(&self.file, buf, self.pos)?;
            if count > 0 && buf[..count].contains(&0) {
                // A seal may have punched these bytes after the manifest
                // was last read.
                if !reloaded && self.reload() {
                    reloaded = true;
                    continue;
                }
                // Best effort: a segment that can't be read leaves the
                // plain bytes.
                let _ = self.fill_unbound(&mut buf[..count]);
            }
            self.pos += count as u64;
            return Ok(count);
        }
    }
}

impl Seek for LogFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let next = match to {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.file.metadata()?.len().checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        self.pos = next
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

// ---- Sealing ----------------------------------------------------------------------------

/// What [`status`] finds about a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The log's length: what it holds, sealed or not.
    pub len: u64,
    /// Bytes the plain file holds on disk.
    pub allocated: u64,
    pub modified: SystemTime,
    /// Where the sealed range ends: 0 when nothing is sealed.
    pub sealed_end: u64,
    /// The segment files' bytes, per the manifest.
    pub segment_bytes: u64,
    /// Whether the manifest was recorded from this inode.
    pub bound: bool,
    /// The sealed range still holds blocks: a seal was interrupted before
    /// its punch, or the log was copied without its holes.
    pub needs_punch: bool,
}

impl Status {
    /// Bytes past the sealed range.
    pub fn unsealed(&self) -> u64 {
        self.len.saturating_sub(self.sealed_end)
    }
}

/// Where punching `[0, sealed_end)` stops, and what the plain file should
/// hold on disk once it has: the rest, in whole blocks.
fn punch_plan(len: u64, sealed_end: u64) -> (u64, u64) {
    let punch_to = sealed_end / BLOCK * BLOCK;
    (punch_to, (len - punch_to).div_ceil(BLOCK) * BLOCK)
}

/// A log's size on disk and how far it is sealed. Reads the manifest, not
/// the segments. A manifest that can't be read is an error.
pub fn status(log: &Path) -> io::Result<Status> {
    let meta = fs::metadata(log)?;
    let manifest = read_manifest(&seal_dir(log))?.map(|(manifest, _)| manifest);
    let sealed_end = manifest.as_ref().map_or(0, Manifest::end).min(meta.len());
    let (punch_to, plain) = punch_plan(meta.len(), sealed_end);
    Ok(Status {
        len: meta.len(),
        allocated: allocated(&meta),
        modified: meta.modified()?,
        sealed_end,
        segment_bytes: manifest.as_ref().map_or(0, |manifest| {
            manifest.segments.iter().map(|segment| segment.bytes).sum()
        }),
        bound: manifest
            .as_ref()
            .is_some_and(|manifest| manifest.ino == identity(&meta).1),
        needs_punch: punch_to > 0 && allocated(&meta) > plain + PUNCH_SLACK,
    })
}

/// What [`seal`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sealed {
    /// The segment written, if any.
    pub added: Option<Segment>,
    /// The manifest belonged to other content (the log was replaced) and
    /// was removed with its segments.
    pub retired: bool,
    /// The manifest was recorded from another inode holding the same
    /// bytes (the log was copied) and now names this one.
    pub rebound: bool,
    /// Holes were punched.
    pub punched: bool,
}

#[cfg(test)]
thread_local! {
    /// Stops [`seal`] right before its punch, as a crash there would.
    pub(crate) static CRASH_BEFORE_PUNCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn crash_before_punch() -> bool {
    CRASH_BEFORE_PUNCH.with(std::cell::Cell::get)
}

#[cfg(not(test))]
fn crash_before_punch() -> bool {
    false
}

/// Seals `log`: writes its bytes past the sealed range, up to the last
/// complete line, as a new segment when there are at least `min_new` of
/// them; then punches the whole sealed range out of the plain file and
/// restores its modified time. Also finishes an interrupted seal, rebinds a
/// copied log's manifest, and retires a replaced log's.
///
/// The caller is the log's only writer and holds its write lock throughout.
/// Nothing is punched before its segment and the manifest naming it are
/// verified and durable. Punching needs Linux and a filesystem that
/// supports it; elsewhere the error is `Unsupported`, after the segment is
/// written.
pub fn seal(log: &Path, min_new: u64) -> io::Result<Sealed> {
    let file = fs::OpenOptions::new().read(true).write(true).open(log)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let ino = identity(&meta).1;
    let len = meta.len();
    let modified = mtime_ns(&meta)?;
    let dir = seal_dir(log);
    remove_temporaries(&dir)?;
    let mut done = Sealed::default();
    let mut manifest = read_manifest(&dir)?.map(|(manifest, _)| manifest);
    if let Some(found) = &manifest
        && found.ino != ino
    {
        if agrees(&file, len, &dir, found)? {
            done.rebound = true;
        } else {
            retire(&dir, found)?;
            manifest = None;
            done.retired = true;
        }
    }
    let start = manifest.as_ref().map_or(0, Manifest::end);
    if len > start {
        let end = line_end(&file, start, len)?;
        if end - start >= min_new.max(1) {
            let generation = manifest
                .as_ref()
                .map_or_else(|| new_generation(log), |found| found.generation.clone());
            let segment = write_segment(&file, &dir, &generation, start, end)?;
            manifest
                .get_or_insert_with(|| Manifest {
                    version: VERSION,
                    generation,
                    ino,
                    sealed_len: len,
                    sealed_mtime_ns: modified,
                    segments: Vec::new(),
                })
                .segments
                .push(segment.clone());
            done.added = Some(segment);
        }
    }
    let Some(manifest) = manifest.as_mut() else {
        return Ok(done);
    };
    // The time to leave on the log: its own, unless an earlier seal of the
    // same bytes punched and stopped before restoring it.
    let target = if done.added.is_none() && !done.rebound && manifest.sealed_len == len {
        manifest.sealed_mtime_ns
    } else {
        modified
    };
    if done.added.is_some() || done.rebound {
        manifest.ino = ino;
        manifest.sealed_len = len;
        manifest.sealed_mtime_ns = target;
        write_manifest(&dir, manifest)?;
    }
    if crash_before_punch() {
        return Ok(done);
    }
    let (punch_to, plain) = punch_plan(len, manifest.end());
    if punch_to > 0 && allocated(&meta) > plain + PUNCH_SLACK {
        punch(&file, 0, punch_to)?;
        done.punched = true;
    }
    if done.punched || target != modified {
        let time = UNIX_EPOCH + Duration::from_nanos(target);
        file.set_modified(time)?;
    }
    Ok(done)
}

/// Removes what an interrupted seal left in `log`'s seal directory: its
/// temporary files and, when the manifest reads, segment files it doesn't
/// name. Retires a manifest recorded from another inode whose bytes the
/// log no longer holds (a replace interrupted before its cleanup). Returns
/// how many files went. Never touches the log, nor a segment while the
/// manifest can't be read.
pub fn tidy(log: &Path) -> io::Result<usize> {
    let dir = seal_dir(log);
    let mut removed = remove_temporaries(&dir)?;
    let Some((manifest, _)) = read_manifest(&dir)? else {
        return Ok(removed);
    };
    if let Ok(file) = fs::File::open(log) {
        let meta = file.metadata()?;
        if identity(&meta).1 != manifest.ino && !agrees(&file, meta.len(), &dir, &manifest)? {
            retire(&dir, &manifest)?;
            return Ok(removed + manifest.segments.len() + 1);
        }
    }
    let named: Vec<String> = manifest
        .segments
        .iter()
        .map(|segment| segment.file_name(&manifest.generation))
        .collect();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".zst") && !named.contains(&name) && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn remove_temporaries(dir: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().ends_with(".tmp") && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Removes a manifest and then its segments.
fn retire(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    fs::remove_file(dir.join(MANIFEST))?;
    sync_dir(dir)?;
    for segment in &manifest.segments {
        match fs::remove_file(dir.join(segment.file_name(&manifest.generation))) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// Whether the plain file agrees with every segment wherever it holds a
/// byte other than NUL: the manifest describes this content. A segment that
/// can't be read is an error, never a disagreement.
fn agrees(file: &fs::File, len: u64, dir: &Path, manifest: &Manifest) -> io::Result<bool> {
    if manifest.end() > len {
        return Ok(false);
    }
    let mut plain = Vec::new();
    for segment in &manifest.segments {
        let open = open_segment(dir, &manifest.generation, segment)?;
        for (index, frame) in open.table.frames.iter().enumerate() {
            let sealed = decompress(&open, index)?;
            plain.resize(sealed.len(), 0);
            read_exact_at(file, &mut plain, segment.start + frame.plain_at)?;
            if plain
                .iter()
                .zip(&sealed)
                .any(|(held, expected)| *held != 0 && held != expected)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// The offset just past the last newline in `[start, len)`, or `start`.
fn line_end(file: &fs::File, start: u64, len: u64) -> io::Result<u64> {
    const CHUNK: u64 = 64 * 1024;
    let mut buffer = vec![0; CHUNK as usize];
    let mut end = len;
    while end > start {
        let from = end.saturating_sub(CHUNK).max(start);
        let chunk = &mut buffer[..(end - from) as usize];
        read_exact_at(file, chunk, from)?;
        if let Some(newline) = chunk.iter().rposition(|byte| *byte == b'\n') {
            return Ok(from + newline as u64 + 1);
        }
        end = from;
    }
    Ok(start)
}

fn new_generation(log: &Path) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut hash = Sha256::new();
    hash.update(now.as_nanos().to_le_bytes());
    hash.update(std::process::id().to_le_bytes());
    hash.update(log.as_os_str().as_encoded_bytes());
    format!("{}-{}", now.as_millis(), hex(&hash.finalize()[..4]))
}

/// Writes `[start, end)` of `file` as a segment: to a temporary file,
/// synced, read back and checked against the bytes' hash, then renamed
/// into place and the directory synced.
fn write_segment(
    file: &fs::File,
    dir: &Path,
    generation: &str,
    start: u64,
    end: u64,
) -> io::Result<Segment> {
    fs::create_dir_all(dir)?;
    let name = Segment {
        start,
        end,
        bytes: 0,
        plain_sha256: String::new(),
        zst_sha256: String::new(),
    }
    .file_name(generation);
    let temporary = dir.join(format!("{name}.tmp"));
    let written = (|| -> io::Result<Segment> {
        let mut out = BufWriter::new(fs::File::create(&temporary)?);
        let (mut plain_hash, mut zst_hash) = (Sha256::new(), Sha256::new());
        let mut entries = Vec::new();
        let mut chunk = vec![0; FRAME_BYTES];
        let mut at = start;
        while at < end {
            let count = (end - at).min(FRAME_BYTES as u64) as usize;
            read_exact_at(file, &mut chunk[..count], at)?;
            plain_hash.update(&chunk[..count]);
            let compressed = zstd::bulk::compress(&chunk[..count], LEVEL)?;
            out.write_all(&compressed)?;
            zst_hash.update(&compressed);
            let compressed_len =
                u32::try_from(compressed.len()).map_err(|_| invalid("frame too large"))?;
            entries.push((compressed_len, count as u32));
            at += count as u64;
        }
        let table = seek_table(&entries)?;
        out.write_all(&table)?;
        zst_hash.update(&table);
        let out = out.into_inner().map_err(io::IntoInnerError::into_error)?;
        out.sync_all()?;
        let bytes = out.metadata()?.len();
        drop(out);
        let plain_sha256 = hex(&plain_hash.finalize());
        verify(&temporary, end - start, &plain_sha256)?;
        Ok(Segment {
            start,
            end,
            bytes,
            plain_sha256,
            zst_sha256: hex(&zst_hash.finalize()),
        })
    })();
    let segment = match written {
        Ok(segment) => segment,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    fs::rename(&temporary, dir.join(&name))?;
    sync_dir(dir)?;
    Ok(segment)
}

/// Reads a segment file back whole and checks it holds `plain` bytes with
/// that hash.
fn verify(path: &Path, plain: u64, plain_sha256: &str) -> io::Result<()> {
    let file = fs::File::open(path)?;
    let open = OpenSegment {
        stamp: Stamp::of(&file.metadata()?),
        table: read_table(&file)?,
        file,
    };
    if open.table.plain != plain {
        return Err(invalid("segment length differs after writing"));
    }
    let mut hash = Sha256::new();
    for index in 0..open.table.frames.len() {
        hash.update(decompress(&open, index)?);
    }
    if hex(&hash.finalize()) != plain_sha256 {
        return Err(invalid("segment bytes differ after writing"));
    }
    Ok(())
}

/// Punches `[offset, offset + len)` out of `file`, keeping its size.
#[cfg(target_os = "linux")]
fn punch(file: &fs::File, offset: u64, len: u64) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let (Ok(offset), Ok(len)) = (libc::off_t::try_from(offset), libc::off_t::try_from(len)) else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    // SAFETY: fallocate takes a descriptor and integers and touches no
    // memory of ours; `file` keeps the descriptor open for the call.
    let result = unsafe {
        libc::fallocate(
            file.as_raw_fd(),
            libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
            offset,
            len,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EOPNOTSUPP) {
        return Err(io::Error::new(io::ErrorKind::Unsupported, error));
    }
    Err(error)
}

#[cfg(not(target_os = "linux"))]
fn punch(_file: &fs::File, _offset: u64, _len: u64) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// Whether holes can be punched in files under `dir`: tried on a small
/// temporary file there, removed after. `Ok(false)` when the filesystem or
/// platform doesn't support it.
pub fn punch_supported(dir: &Path) -> io::Result<bool> {
    let path = dir.join(format!(".punch-probe-{}.tmp", std::process::id()));
    let probe = (|| -> io::Result<bool> {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.write_all(&[b'x'; 3 * BLOCK as usize])?;
        file.sync_all()?;
        match punch(&file, BLOCK, BLOCK) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::Unsupported => return Ok(false),
            Err(error) => return Err(error),
        }
        let mut middle = [1; BLOCK as usize];
        read_exact_at(&file, &mut middle, BLOCK)?;
        Ok(file.metadata()?.len() == 3 * BLOCK && middle.iter().all(|byte| *byte == 0))
    })();
    let _ = fs::remove_file(&path);
    probe
}

#[cfg(test)]
mod tests;
