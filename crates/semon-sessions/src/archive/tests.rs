use std::{
    io::Cursor,
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn archive() -> (Scratch, Archive) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "semon-archive-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let archive = Archive::new(root.join(DIR));
    (Scratch(root), archive)
}

/// `len` bytes of short lines that differ with `seed`.
fn lines(len: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut line = 0_u32;
    while out.len() < len {
        out.extend_from_slice(format!("{{\"n\":{line},\"seed\":{seed}}}\n").as_bytes());
        line += 1;
    }
    out.truncate(len);
    out
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Admits the first `offset` bytes of `log` under their true digest.
fn admit(archive: &Archive, log: &[u8], offset: usize) -> bool {
    let prefix = digest(&log[..offset]);
    let mut cursor = Cursor::new(log);
    archive
        .admit("native", &mut cursor, offset as u64, &prefix)
        .unwrap()
}

fn preserve(archive: &Archive, source: &str, bytes: &[u8], version: &str) -> Option<Chain> {
    let mut cursor = Cursor::new(bytes);
    archive.preserve(source, &mut cursor, version).unwrap()
}

fn read_chain(archive: &Archive, source: &str, chain: &Chain) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chain = archive.open_chain(source, chain).unwrap();
    chain.read_to_end(&mut bytes).unwrap();
    bytes
}

fn manifest_path(archive: &Archive, source: &str) -> PathBuf {
    let id = archive.lookup(source).unwrap().unwrap();
    archive.dir.join(id).join(MANIFEST)
}

#[test]
fn a_preserved_chain_holds_the_bytes_and_the_redactor_version() {
    let (_scratch, archive) = archive();
    let bytes = lines(300_000, 1);
    let chain = preserve(&archive, "claude/a.jsonl", &bytes, "7").unwrap();
    assert_eq!(chain.length, bytes.len() as u64);
    assert_eq!(chain.sha256, hex(&digest(&bytes)));
    assert_eq!(chain.redactor_version, "7");
    assert!(is_id(&chain.chain_id));
    assert_eq!(
        archive.chains("claude/a.jsonl").unwrap(),
        std::slice::from_ref(&chain)
    );
    assert_eq!(read_chain(&archive, "claude/a.jsonl", &chain), bytes);
    archive.verify("claude/a.jsonl").unwrap();

    // The same bytes again are held already; nothing is written.
    let again = preserve(&archive, "claude/a.jsonl", &bytes, "7");
    assert_eq!(again, Some(chain));
    assert_eq!(archive.chains("claude/a.jsonl").unwrap().len(), 1);
    // There is nothing to hold in an empty file.
    assert_eq!(preserve(&archive, "claude/a.jsonl", b"", "7"), None);

    // Other bytes are another chain, after the first.
    let other = lines(1_000, 2);
    preserve(&archive, "claude/a.jsonl", &other, "8").unwrap();
    let chains = archive.chains("claude/a.jsonl").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[1].redactor_version, "8");
    // Sources have their own random ids.
    preserve(&archive, "claude/b.jsonl", &other, "8").unwrap();
    let a = archive.lookup("claude/a.jsonl").unwrap().unwrap();
    let b = archive.lookup("claude/b.jsonl").unwrap().unwrap();
    assert!(is_id(&a) && is_id(&b));
    assert_ne!(a, b);
    let log = fs::read_to_string(archive.dir.join(SOURCES)).unwrap();
    assert_eq!(log.lines().count(), 2);
    // Each directory names its source.
    let manifest = read_manifest(&archive.dir.join(&a)).unwrap();
    assert_eq!(manifest.source, "claude/a.jsonl");
}

#[test]
fn a_failure_before_the_rename_leaves_the_last_manifest() {
    let (_scratch, archive) = archive();
    let kept = preserve(&archive, "s", &lines(5_000, 1), "1").unwrap();
    let before = fs::read(manifest_path(&archive, "s")).unwrap();

    FAIL_BEFORE_RENAME.with(|fail| fail.set(true));
    let second = lines(6_000, 2);
    let mut cursor = Cursor::new(&second);
    assert!(archive.preserve("s", &mut cursor, "1").is_err());
    assert_eq!(archive.chains("s").unwrap(), std::slice::from_ref(&kept));
    assert_eq!(fs::read(manifest_path(&archive, "s")).unwrap(), before);
    archive.verify("s").unwrap();

    // The retry succeeds, after the first chain.
    preserve(&archive, "s", &second, "1").unwrap();
    let chains = archive.chains("s").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0], kept);
    archive.verify("s").unwrap();
}

#[test]
fn a_digest_mismatch_is_detected() {
    let (_scratch, archive) = archive();
    let chain = preserve(&archive, "s", &lines(50_000, 1), "1").unwrap();
    archive.verify("s").unwrap();
    // Other bytes in the chain's place, as damage would leave them.
    let id = archive.lookup("s").unwrap().unwrap();
    let file = archive.dir.join(id).join(format!("{}.zst", chain.chain_id));
    fs::write(
        file,
        zstd::bulk::compress(&lines(50_000, 9), LEVEL).unwrap(),
    )
    .unwrap();
    assert!(archive.verify("s").is_err());

    // A manifest that claims other bytes than its chain holds.
    let (_scratch, archive) = self::archive();
    preserve(&archive, "s", &lines(50_000, 1), "1").unwrap();
    let path = manifest_path(&archive, "s");
    let mut manifest: Manifest = read_json(&path).unwrap();
    manifest.chains[0].sha256 = hex(&digest(b"not these"));
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    assert!(archive.verify("s").is_err());
}

#[test]
fn admit_extends_one_chain_with_the_consumed_prefix() {
    let (_scratch, archive) = archive();
    let log = lines(2_500_000, 1);
    // 3001 is not a multiple of 64: the digest continues mid-block.
    for offset in [3_001, 3_001, 900_000, 2_000_001, 2_500_000] {
        assert!(admit(&archive, &log, offset));
        let chains = archive.chains("native").unwrap();
        assert_eq!(chains.len(), 1, "appends extend the chain");
        assert_eq!(chains[0].length, offset as u64);
        assert_eq!(chains[0].redactor_version, NATIVE_REDACTOR);
        archive.verify("native").unwrap();
    }
    let chain = archive.chains("native").unwrap().remove(0);
    assert_eq!(read_chain(&archive, "native", &chain), log);
    // Another instance finds the source at once.
    let other = Archive::new(archive.dir.clone());
    assert_eq!(
        other.chains("native").unwrap(),
        std::slice::from_ref(&chain)
    );
    // The log holds less than the chain: nothing changes.
    assert!(admit(&archive, &log, 5_000));
    assert_eq!(archive.chains("native").unwrap(), [chain]);
}

#[test]
fn a_rewrite_starts_a_new_chain_and_keeps_the_old_one() {
    let (_scratch, archive) = archive();
    let old = lines(40_000, 1);
    assert!(admit(&archive, &old, old.len()));
    // Rewritten from the start, and longer.
    let rewritten = lines(50_000, 2);
    assert!(admit(&archive, &rewritten, rewritten.len()));
    // Rewritten in its last bytes, then grown.
    let mut grown = rewritten.clone();
    let last = grown.len() - 10;
    grown[last..].copy_from_slice(b"0123456789");
    grown.extend_from_slice(&lines(1_000, 3));
    assert!(admit(&archive, &grown, grown.len()));
    // Rewritten shorter than the chain before it.
    let shorter = lines(20_000, 4);
    assert!(admit(&archive, &shorter, shorter.len()));

    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 4);
    for (chain, expected) in chains.iter().zip([&old, &rewritten, &grown, &shorter]) {
        assert_eq!(read_chain(&archive, "native", chain), *expected);
    }
    archive.verify("native").unwrap();
}

#[test]
fn bytes_that_are_not_the_consumed_prefix_are_not_admitted() {
    let (_scratch, archive) = archive();
    let log = lines(30_000, 1);
    let wrong = digest(&lines(30_000, 2));
    // The log changed after the index read it.
    let mut cursor = Cursor::new(&log);
    assert!(
        !archive
            .admit("native", &mut cursor, 30_000, &wrong)
            .unwrap()
    );
    // The log is shorter than what the index consumed.
    let mut cursor = Cursor::new(&log[..100]);
    let all = digest(&log);
    assert!(!archive.admit("native", &mut cursor, 30_000, &all).unwrap());
    assert!(archive.chains("native").unwrap().is_empty());

    assert!(admit(&archive, &log, 10_000));
    let chain = archive.chains("native").unwrap().remove(0);
    // Growth whose digest is not the log's leaves the chain as it was.
    let mut cursor = Cursor::new(&log);
    assert!(
        !archive
            .admit("native", &mut cursor, 30_000, &wrong)
            .unwrap()
    );
    assert_eq!(archive.chains("native").unwrap(), [chain]);
    archive.verify("native").unwrap();
}

#[test]
fn a_failed_extension_is_retried_from_the_last_manifest() {
    let (_scratch, archive) = archive();
    let log = lines(200_000, 1);
    assert!(admit(&archive, &log, 50_000));
    let kept = archive.chains("native").unwrap();

    FAIL_BEFORE_RENAME.with(|fail| fail.set(true));
    let prefix = digest(&log[..150_000]);
    let mut cursor = Cursor::new(&log);
    assert!(
        archive
            .admit("native", &mut cursor, 150_000, &prefix)
            .is_err()
    );
    assert_eq!(archive.chains("native").unwrap(), kept);
    archive.verify("native").unwrap();

    assert!(admit(&archive, &log, 200_000));
    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 1);
    assert_eq!(read_chain(&archive, "native", &chains[0]), log);
    archive.verify("native").unwrap();
}

#[test]
fn identical_bytes_under_another_version_are_another_chain() {
    let (_scratch, archive) = archive();
    let bytes = lines(10_000, 1);
    let first = preserve(&archive, "s", &bytes, "1").unwrap();
    let second = preserve(&archive, "s", &bytes, "2").unwrap();
    assert_ne!(first.chain_id, second.chain_id);
    assert_eq!(second.redactor_version, "2");
    // Each version finds its own chain again.
    assert_eq!(preserve(&archive, "s", &bytes, "1"), Some(first));
    assert_eq!(preserve(&archive, "s", &bytes, "2"), Some(second));
    assert_eq!(archive.chains("s").unwrap().len(), 2);
}

#[test]
fn a_chain_that_does_not_read_back_is_not_reused_and_is_kept() {
    let (_scratch, archive) = archive();
    let bytes = lines(10_000, 1);
    let bad = preserve(&archive, "s", &bytes, "1").unwrap();
    let id = archive.lookup("s").unwrap().unwrap();
    let file = archive.dir.join(id).join(format!("{}.zst", bad.chain_id));

    // Corrupt: other bytes where the chain was. The same bytes come again.
    let other = zstd::bulk::compress(&lines(10_000, 9), LEVEL).unwrap();
    fs::write(&file, &other).unwrap();
    let fresh = preserve(&archive, "s", &bytes, "1").unwrap();
    assert_ne!(fresh.chain_id, bad.chain_id);
    assert_eq!(read_chain(&archive, "s", &fresh), bytes);
    assert_eq!(fs::read(&file).unwrap(), other, "kept as evidence");
    // The intact copy is reused from now on.
    assert_eq!(preserve(&archive, "s", &bytes, "1"), Some(fresh));
    assert_eq!(archive.chains("s").unwrap().len(), 2);

    // Missing: the file is gone. The bytes still have a home.
    let (_scratch, archive) = self::archive();
    let lost = preserve(&archive, "s", &bytes, "1").unwrap();
    let id = archive.lookup("s").unwrap().unwrap();
    let file = archive.dir.join(id).join(format!("{}.zst", lost.chain_id));
    fs::remove_file(file).unwrap();
    let fresh = preserve(&archive, "s", &bytes, "1").unwrap();
    assert_ne!(fresh.chain_id, lost.chain_id);
    assert_eq!(read_chain(&archive, "s", &fresh), bytes);
    assert_eq!(archive.chains("s").unwrap().len(), 2);
}

#[test]
fn a_manifest_that_names_a_path_outside_the_archive_is_refused() {
    let (_scratch, archive) = archive();
    let log = lines(10_000, 1);
    assert!(admit(&archive, &log, 5_000));
    let outside = archive.dir.join("escape.zst");
    fs::write(&outside, b"keep").unwrap();

    let path = manifest_path(&archive, "native");
    let mut manifest: Manifest = read_json(&path).unwrap();
    manifest.chains[0].chain_id = "../escape".into();
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();

    assert!(archive.chains("native").is_err());
    assert!(archive.verify("native").is_err());
    let mut cursor = Cursor::new(&log);
    let prefix = digest(&log);
    let admitted = archive.admit("native", &mut cursor, 10_000, &prefix);
    assert!(admitted.is_err());
    let mut cursor = Cursor::new(&log);
    assert!(archive.preserve("native", &mut cursor, "1").is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"keep");

    // A continuation state past the chain's end is refused too.
    manifest.chains[0].chain_id = "a".repeat(32);
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    assert!(archive.chains("native").is_ok());
    let mut past = manifest.chains[0].resume.clone().unwrap();
    past.at = manifest.chains[0].length.div_ceil(64) * 64 + 64;
    manifest.chains[0].resume = Some(past);
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    assert!(archive.chains("native").is_err());
    // As is a chain of no bytes.
    manifest.chains[0].resume = None;
    manifest.chains[0].length = 0;
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    assert!(archive.chains("native").is_err());
}

fn chain_file(archive: &Archive, source: &str, chain: &Chain) -> PathBuf {
    let id = archive.lookup(source).unwrap().unwrap();
    archive.dir.join(id).join(format!("{}.zst", chain.chain_id))
}

#[test]
fn two_instances_admitting_one_source_share_its_directory() {
    let (_scratch, first) = archive();
    let second = Archive::new(first.dir.clone());
    let log = lines(20_000, 1);
    assert!(admit(&first, &log, 8_000));
    // The second instance has not seen the source; it finds its id.
    assert!(admit(&second, &log, 20_000));
    assert!(admit(&first, &log, 20_000));
    for archive in [&first, &second] {
        let chains = archive.chains("native").unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(read_chain(archive, "native", &chains[0]), log);
    }
    let ids = fs::read_to_string(first.dir.join(SOURCES)).unwrap();
    assert_eq!(ids.lines().count(), 1, "one id for one source");
    let directories = fs::read_dir(&first.dir)
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().path().is_dir())
        .count();
    assert_eq!(directories, 1);
}

#[test]
fn a_restart_after_the_chain_is_published_finds_it() {
    let (_scratch, archive) = archive();
    let log = lines(20_000, 1);
    assert!(admit(&archive, &log, 20_000));
    let chain = archive.chains("native").unwrap().remove(0);
    // A new process: nothing is remembered, the directory is attributed.
    let restarted = Archive::new(archive.dir.clone());
    assert_eq!(
        restarted.chains("native").unwrap(),
        std::slice::from_ref(&chain)
    );
    assert!(admit(&restarted, &log, 20_000));
    assert_eq!(restarted.chains("native").unwrap(), [chain]);
    let id = restarted.lookup("native").unwrap().unwrap();
    let manifest = read_manifest(&restarted.dir.join(id)).unwrap();
    assert_eq!(manifest.source, "native");
}

#[test]
fn a_registration_failure_leaves_nothing_and_is_retried() {
    let (_scratch, archive) = archive();
    let log = lines(20_000, 1);
    FAIL_REGISTER.with(|fail| fail.set(true));
    let prefix = digest(&log);
    let mut cursor = Cursor::new(&log);
    assert!(
        archive
            .admit("native", &mut cursor, 20_000, &prefix)
            .is_err()
    );
    let fresh = Archive::new(archive.dir.clone());
    assert!(fresh.chains("native").unwrap().is_empty());

    // Storage is back: the same, unchanged source is archived and found.
    assert!(admit(&archive, &log, 20_000));
    let fresh = Archive::new(archive.dir.clone());
    assert_eq!(fresh.chains("native").unwrap().len(), 1);
    fresh.verify("native").unwrap();
}

#[test]
fn a_torn_final_log_line_is_ignored_and_cut_off() {
    let (_scratch, archive) = archive();
    let log = lines(20_000, 1);
    assert!(admit(&archive, &log, 20_000));
    let path = archive.dir.join(SOURCES);
    let whole = fs::read(&path).unwrap();
    // A crash in the middle of recording another source.
    let mut torn = whole.clone();
    torn.extend_from_slice(b"{\"source\":\"other\",\"id\":\"0123");
    fs::write(&path, &torn).unwrap();

    let restarted = Archive::new(archive.dir.clone());
    assert_eq!(restarted.chains("native").unwrap().len(), 1);
    assert!(restarted.chains("other").unwrap().is_empty());
    // The next new source is appended after the last whole line.
    preserve(&restarted, "other", &lines(1_000, 2), "1").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(&String::from_utf8(whole).unwrap()));
    assert_eq!(text.lines().count(), 2);
    let again = Archive::new(archive.dir.clone());
    assert_eq!(again.chains("other").unwrap().len(), 1);
    assert_eq!(again.chains("native").unwrap().len(), 1);
}

#[test]
fn a_missing_chain_is_kept_listed_and_a_new_one_is_written() {
    let (_scratch, archive) = archive();
    let log = lines(40_000, 1);
    assert!(admit(&archive, &log, 20_000));
    let lost = archive.chains("native").unwrap().remove(0);
    fs::remove_file(chain_file(&archive, "native", &lost)).unwrap();

    // The same instance must invalidate its cached trust at an equal offset.
    assert!(admit(&archive, &log, 20_000));
    assert!(admit(&archive, &log, 40_000));
    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0], lost, "the damaged entry stays");
    assert_eq!(read_chain(&archive, "native", &chains[1]), log);
    assert!(!chain_file(&archive, "native", &lost).exists());
    assert!(archive.verify("native").is_err(), "the loss stays visible");
}

#[test]
fn a_chain_corrupted_at_the_same_size_is_not_trusted_or_extended() {
    let (_scratch, archive) = archive();
    let log = lines(40_000, 1);
    assert!(admit(&archive, &log, 20_000));
    let bad = archive.chains("native").unwrap().remove(0);
    let file = chain_file(&archive, "native", &bad);
    let mut bytes = fs::read(&file).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(&file, &bytes).unwrap();

    // Equal offset and digest in the same instance must invalidate trust.
    assert!(admit(&archive, &log, 20_000));
    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(read_chain(&archive, "native", &chains[1]), &log[..20_000]);
    // Growth extends the new, verified chain, never the damaged one.
    assert!(admit(&archive, &log, 40_000));
    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0], bad);
    assert_eq!(read_chain(&archive, "native", &chains[1]), log);
    assert_eq!(fs::read(&file).unwrap(), bytes, "kept as found");
}

#[test]
fn growth_after_corruption_does_not_extend_the_damaged_chain() {
    let (_scratch, archive) = archive();
    let log = lines(40_000, 1);
    assert!(admit(&archive, &log, 20_000));
    let bad = archive.chains("native").unwrap().remove(0);
    let file = chain_file(&archive, "native", &bad);
    let mut bytes = fs::read(&file).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(&file, &bytes).unwrap();

    // Growth is the first thing this same instance sees after damage.
    assert!(admit(&archive, &log, 40_000));
    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0], bad, "the manifest entry is unchanged");
    assert_eq!(fs::read(&file).unwrap(), bytes, "no bytes appended");
    assert_eq!(read_chain(&archive, "native", &chains[1]), log);
}

#[test]
fn growth_after_deletion_starts_a_new_chain_in_the_same_instance() {
    let (_scratch, archive) = archive();
    let log = lines(40_000, 1);
    assert!(admit(&archive, &log, 20_000));
    let lost = archive.chains("native").unwrap().remove(0);
    let file = chain_file(&archive, "native", &lost);
    fs::remove_file(&file).unwrap();

    assert!(admit(&archive, &log, 40_000));
    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0], lost);
    assert!(!file.exists());
    assert_eq!(read_chain(&archive, "native", &chains[1]), log);
}

#[test]
fn a_changed_manifest_invalidates_cached_verification() {
    let (_scratch, archive) = archive();
    let log = lines(20_000, 1);
    assert!(admit(&archive, &log, log.len()));
    let path = manifest_path(&archive, "native");
    let mut manifest = read_manifest(path.parent().unwrap()).unwrap();
    let old = manifest.chains[0].clone();
    let file = chain_file(&archive, "native", &old);
    let bytes = fs::read(&file).unwrap();
    let changed = lines(log.len(), 2);
    manifest.chains[0].sha256 = hex(&digest(&changed));
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();

    // The new digest matches the incoming bytes, but not the unchanged file.
    assert!(admit(&archive, &changed, changed.len()));
    let chains = archive.chains("native").unwrap();
    assert_eq!(chains.len(), 2);
    assert_eq!(read_chain(&archive, "native", &chains[1]), changed);
    assert_eq!(fs::read(file).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn a_reader_inside_a_torn_record_blocks_repair() {
    use std::{os::fd::AsRawFd, sync::mpsc, thread, time::Duration};

    let (_scratch, archive) = archive();
    preserve(&archive, "first", b"first", "1").unwrap();
    let journal = archive.dir.join(SOURCES);
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(b"{\"source\":\"torn\",\"id\":\"0123")
        .unwrap();
    let reader = Archive::new(archive.dir.clone());
    assert!(reader.lookup("missing").unwrap().is_none());
    let dir = archive.dir.clone();
    let (paused_tx, paused_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        DURING_JOURNAL_READ.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                paused_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }));
        });
        let found = reader.lookup("torn").unwrap();
        (reader, found)
    });
    paused_rx.recv_timeout(Duration::from_secs(10)).unwrap();

    // This is the exact exclusive flock required by torn-tail repair. It
    // must fail while the reader is paused between bytes of that record.
    let file = fs::OpenOptions::new()
        .append(true)
        .open(dir.join(LOCK))
        .unwrap();
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    let error = io::Error::last_os_error();
    resume_tx.send(()).unwrap();
    let (reader, found) = handle.join().unwrap();
    assert_eq!(locked, -1, "repair acquired the lock during a read");
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(found.is_none(), "a torn record is never a mapping");
    drop(file);

    preserve(&archive, "torn", b"repaired", "1").unwrap();
    let fresh = Archive::new(dir);
    assert_eq!(
        reader.lookup("torn").unwrap(),
        fresh.lookup("torn").unwrap()
    );
    assert_eq!(reader.chains("torn").unwrap().len(), 1);
}
