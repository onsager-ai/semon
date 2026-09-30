use std::io::{BufRead, BufReader};

use super::*;

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "semon-sealed-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

/// About `bytes` of JSON lines, each different, of uneven lengths.
fn jsonl(bytes: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::new();
    let mut line = 0;
    while out.len() < bytes {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let width = 20 + (state >> 33) as usize % 3000;
        let words: String = (0..width)
            .map(|index| {
                (b'a' + ((state >> (index % 50)) as u8).wrapping_add(index as u8) % 26) as char
            })
            .collect();
        out.extend_from_slice(
            format!("{{\"n\":{line},\"seed\":{seed},\"text\":\"{words}\"}}\n").as_bytes(),
        );
        line += 1;
    }
    out
}

fn append(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
}

/// `len` bytes of `path` from `at`, through a [`LogFile`].
fn read_range(path: &Path, at: u64, len: u64) -> Vec<u8> {
    let mut log = LogFile::open(path).unwrap();
    log.seek(SeekFrom::Start(at)).unwrap();
    let mut out = Vec::new();
    log.take(len).read_to_end(&mut out).unwrap();
    out
}

fn read_all(path: &Path) -> Vec<u8> {
    read_range(path, 0, u64::MAX)
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn set_modified(path: &Path, second: u64) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1_790_000_000 + second))
        .unwrap();
}

#[test]
fn seal_directories_are_named_after_their_log() {
    assert_eq!(
        seal_dir(Path::new("/m/claude/projects/p/a.jsonl")),
        Path::new("/m/claude/projects/p/a.jsonl.seal")
    );
    assert!(is_seal_dir_name("a.jsonl.seal"));
    assert!(!is_seal_dir_name(".jsonl.seal"));
    assert!(!is_seal_dir_name("a.jsonl"));
}

#[test]
fn an_unsealed_log_reads_as_its_file() {
    let root = scratch("plain");
    let log = root.join("a.jsonl");
    let bytes = jsonl(100_000, 1);
    fs::write(&log, &bytes).unwrap();
    assert_eq!(read_all(&log), bytes);
    assert_eq!(read_range(&log, 777, 1000), bytes[777..1777]);
    // Past the end: nothing, as a file.
    assert!(read_range(&log, 1 << 30, 10).is_empty());
    let status = status(&log).unwrap();
    assert_eq!(
        (status.sealed_end, status.unsealed()),
        (0, bytes.len() as u64)
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn reads_match_at_any_offset_across_frames_and_segments() {
    let root = scratch("offsets");
    assert!(
        punch_supported(&root).unwrap(),
        "the test filesystem punches holes"
    );
    let log = root.join("a.jsonl");
    let mut bytes = jsonl(2 * FRAME_BYTES + 300_000, 2);
    fs::write(&log, &bytes).unwrap();
    set_modified(&log, 1);
    let before = modified(&log);
    let ino = identity(&fs::metadata(&log).unwrap()).1;

    let first = seal(&log, 1).unwrap();
    let e1 = bytes.len() as u64;
    assert_eq!(
        first.added.as_ref().map(|s| (s.start, s.end)),
        Some((0, e1))
    );
    assert!(first.punched);
    let meta = fs::metadata(&log).unwrap();
    assert_eq!(
        (meta.len(), identity(&meta).1, modified(&log)),
        (e1, ino, before),
        "length, inode and modified time are the log's own"
    );
    assert!(
        allocated(&meta) < e1 / 4,
        "the sealed range holds no blocks"
    );

    // The log grows past the seal, and ends in a partial line.
    let grown = jsonl(FRAME_BYTES + 400_000, 3);
    append(&log, &grown);
    append(&log, b"{\"partial\":");
    bytes.extend_from_slice(&grown);
    let e2 = bytes.len() as u64;
    bytes.extend_from_slice(b"{\"partial\":");
    assert_eq!(read_all(&log), bytes, "sealed, then plain");

    let second = seal(&log, 1).unwrap();
    assert_eq!(
        second.added.as_ref().map(|s| (s.start, s.end)),
        Some((e1, e2))
    );
    let status = status(&log).unwrap();
    assert_eq!(status.sealed_end, e2);
    assert!(status.bound && !status.needs_punch);

    let frame = FRAME_BYTES as u64;
    for at in [
        0,
        1,
        frame - 7,
        frame,
        frame + 1,
        2 * frame - 1,
        e1 - 3,
        e1,
        e1 + 1,
        e1 + frame - 2,
        e2 - 1,
        e2,
        bytes.len() as u64 - 3,
    ] {
        let want = &bytes[at as usize..(at as usize + 10_000).min(bytes.len())];
        assert_eq!(read_range(&log, at, 10_000), want, "at {at}");
    }
    // Line by line, as the builder reads.
    let mut lines = BufReader::new(LogFile::open(&log).unwrap());
    let mut offset = 0;
    let mut line = Vec::new();
    while lines.read_until(b'\n', &mut line).unwrap() > 0 {
        assert_eq!(line, bytes[offset..offset + line.len()]);
        offset += line.len();
        line.clear();
    }
    assert_eq!(offset, bytes.len());
    // Backwards, as a transcript page reads.
    let mut log_file = LogFile::open(&log).unwrap();
    for at in [e2 - 5, e1 + 10, e1 - 10, 5] {
        log_file.seek(SeekFrom::Start(at)).unwrap();
        let mut four = [0; 4];
        log_file.read_exact(&mut four).unwrap();
        assert_eq!(four, bytes[at as usize..at as usize + 4]);
    }
    // A segment is plain zstd: the stock decoder reads it back.
    let manifest = read_manifest(&seal_dir(&log)).unwrap().unwrap().0;
    let path = seal_dir(&log).join(manifest.segments[1].file_name(&manifest.generation));
    let decoded = zstd::stream::decode_all(fs::File::open(path).unwrap()).unwrap();
    assert_eq!(decoded, bytes[e1 as usize..e2 as usize]);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn real_nul_runs_read_back_from_the_segment() {
    let root = scratch("nuls");
    let log = root.join("a.jsonl");
    let mut bytes = jsonl(50_000, 4);
    bytes.extend(std::iter::repeat_n(0, 9_000));
    bytes.extend_from_slice(&jsonl(50_000, 5));
    fs::write(&log, &bytes).unwrap();
    seal(&log, 1).unwrap();
    assert_eq!(read_all(&log), bytes);
    assert_eq!(read_range(&log, 50_100, 100), bytes[50_100..50_200]);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_reader_opened_before_the_seal_reads_through_it() {
    let root = scratch("race");
    let log = root.join("a.jsonl");
    let bytes = jsonl(300_000, 6);
    fs::write(&log, &bytes).unwrap();
    let mut early = LogFile::open(&log).unwrap();
    let mut head = [0; 100];
    early.read_exact(&mut head).unwrap();
    assert!(seal(&log, 1).unwrap().punched);
    let mut rest = Vec::new();
    early.read_to_end(&mut rest).unwrap();
    assert_eq!([&head[..], &rest[..]].concat(), bytes);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn an_interrupted_seal_is_ignored_then_cleaned_up_and_finished() {
    let root = scratch("crash");
    let log = root.join("a.jsonl");
    let bytes = jsonl(400_000, 7);
    fs::write(&log, &bytes).unwrap();
    set_modified(&log, 2);
    let before = modified(&log);
    let dir = seal_dir(&log);

    // Crashed while writing a segment and a manifest: only temporaries.
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("1-00000000000000000000.zst.tmp"), b"half a frame").unwrap();
    fs::write(dir.join("manifest.json.tmp"), b"{\"version\"").unwrap();
    assert_eq!(read_all(&log), bytes, "temporaries are never read");
    assert_eq!(status(&log).unwrap().sealed_end, 0);
    assert_eq!(tidy(&log).unwrap(), 2);
    assert!(fs::read_dir(&dir).unwrap().next().is_none());

    // Crashed after the manifest, before the punch.
    CRASH_BEFORE_PUNCH.with(|crash| crash.set(true));
    let sealed = seal(&log, 1).unwrap();
    CRASH_BEFORE_PUNCH.with(|crash| crash.set(false));
    assert!(sealed.added.is_some() && !sealed.punched);
    let status_now = status(&log).unwrap();
    assert!(status_now.needs_punch);
    assert_eq!(read_all(&log), bytes);
    // A leftover from yet another crash, and a segment no manifest names.
    fs::write(dir.join("x.zst.tmp"), b"junk").unwrap();
    fs::write(dir.join("9-00000000000000000000.zst"), b"junk").unwrap();
    assert_eq!(tidy(&log).unwrap(), 2);
    let finished = seal(&log, u64::MAX).unwrap();
    assert!(finished.added.is_none() && finished.punched);
    assert!(!status(&log).unwrap().needs_punch);
    assert_eq!(modified(&log), before);
    assert_eq!(read_all(&log), bytes);

    // Crashed after a punch, before restoring the time: the next seal
    // restores it.
    set_modified(&log, 99);
    let again = seal(&log, u64::MAX).unwrap();
    assert!(again.added.is_none() && !again.punched);
    assert_eq!(modified(&log), before);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_replaced_log_reads_its_new_bytes_and_retires_the_old_generation() {
    let root = scratch("replace");
    let log = root.join("a.jsonl");
    fs::write(&log, jsonl(200_000, 8)).unwrap();
    seal(&log, 1).unwrap();
    let old = read_manifest(&seal_dir(&log)).unwrap().unwrap().0;

    // As a receiver replaces a file: a new one renamed over it.
    let bytes = jsonl(150_000, 9);
    let next = root.join("a.jsonl.replace.tmp");
    fs::write(&next, &bytes).unwrap();
    fs::rename(&next, &log).unwrap();
    assert_eq!(read_all(&log), bytes);
    assert!(!status(&log).unwrap().bound);

    let sealed = seal(&log, 1).unwrap();
    assert!(sealed.retired && sealed.added.is_some());
    let new = read_manifest(&seal_dir(&log)).unwrap().unwrap().0;
    assert_ne!(new.generation, old.generation);
    assert!(
        !seal_dir(&log)
            .join(old.segments[0].file_name(&old.generation))
            .exists()
    );
    assert_eq!(read_all(&log), bytes);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_copied_log_reads_through_its_holes_and_rebinds() {
    let root = scratch("copy");
    let log = root.join("a.jsonl");
    let bytes = jsonl(300_000, 10);
    fs::write(&log, &bytes).unwrap();
    seal(&log, 1).unwrap();

    // A copy of the volume: the holes come over as zeros, on a new inode.
    let copied = fs::read(&log).unwrap();
    assert!(copied[..4096].iter().all(|byte| *byte == 0));
    let next = root.join("copy.tmp");
    fs::write(&next, &copied).unwrap();
    fs::rename(&next, &log).unwrap();
    assert!(!status(&log).unwrap().bound);
    assert_eq!(read_all(&log), bytes, "zeros filled from the segments");
    assert_eq!(tidy(&log).unwrap(), 0, "a copy is not a replace");

    let sealed = seal(&log, u64::MAX).unwrap();
    assert!(sealed.rebound && !sealed.retired && sealed.punched);
    assert!(status(&log).unwrap().bound);
    assert_eq!(read_all(&log), bytes);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_small_tail_or_a_partial_line_is_not_sealed() {
    let root = scratch("small");
    let log = root.join("a.jsonl");
    fs::write(&log, b"{\"no newline yet\":").unwrap();
    assert_eq!(seal(&log, 1).unwrap(), Sealed::default());
    fs::write(&log, jsonl(10_000, 11)).unwrap();
    assert_eq!(seal(&log, 64 * 1024).unwrap(), Sealed::default());
    assert!(!seal_dir(&log).exists());
    fs::remove_dir_all(root).unwrap();
}
