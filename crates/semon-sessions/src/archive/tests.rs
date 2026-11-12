use std::{
    io::Cursor,
    sync::atomic::{AtomicU64, Ordering},
};

use super::*;

struct Clock(Option<u64>);

impl Clock {
    fn at(now: u64) -> Self {
        Self(TEST_NOW.with(|clock| clock.replace(Some(now))))
    }
    fn set(&self, now: u64) {
        TEST_NOW.with(|clock| clock.set(Some(now)));
    }
}
impl Drop for Clock {
    fn drop(&mut self) {
        TEST_NOW.with(|clock| clock.set(self.0));
    }
}

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
fn discovery_without_registered_sources_does_not_create_archive_state() {
    let (_scratch, archive) = archive();
    let options = crate::Options::default();
    assert!(archive.missing_sources(&options).unwrap().is_empty());
    assert!(!archive.dir.exists());
    fs::create_dir_all(&archive.dir).unwrap();
    assert!(archive.missing_sources(&options).unwrap().is_empty());
    assert_eq!(fs::read_dir(&archive.dir).unwrap().count(), 0);
}

#[test]
fn retention_is_explicit_and_uses_admission_age_and_exact_dependencies() {
    let _clock = Clock::at(200_000);
    let (scratch, archive) = archive();
    let native = scratch.0.join("native.jsonl");
    fs::write(&native, b"native bytes remain").unwrap();
    let expired = preserve(&archive, "source", b"old unreferenced\n", "none").unwrap();
    let seal = preserve(&archive, "source", b"sealed custody\n", "none").unwrap();
    let index = preserve(&archive, "source", b"indexed custody\n", "none").unwrap();
    let boundary = preserve(&archive, "source", b"exact boundary\n", "none").unwrap();
    let recent = preserve(&archive, "source", b"recent\n", "none").unwrap();
    let path = manifest_path(&archive, "source");
    let mut manifest = read_manifest(path.parent().unwrap()).unwrap();
    for chain in &mut manifest.chains {
        chain.admitted_at = if chain.chain_id == recent.chain_id {
            200_000
        } else if chain.chain_id == boundary.chain_id {
            113_600
        } else {
            1
        };
    }
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    archive
        .protect("source", &seal.chain_id, "seal:original".into())
        .unwrap();
    archive
        .protect("source", &index.chain_id, "index:original".into())
        .unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(archive.prune_at(None, 200_000).unwrap(), 0);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(archive.prune_at(Some(1), 200_000).unwrap(), 1);
    let remaining = archive.chains("source").unwrap();
    assert!(
        !remaining
            .iter()
            .any(|chain| chain.chain_id == expired.chain_id)
    );
    for chain in [&seal, &index, &boundary, &recent] {
        assert!(remaining.iter().any(|held| held.chain_id == chain.chain_id));
    }
    assert_eq!(fs::read(native).unwrap(), b"native bytes remain");
    assert_eq!(archive.prune_at(Some(1), 200_000).unwrap(), 0);
    archive.verify("source").unwrap();
}

#[test]
fn missing_native_binding_is_exact_verified_and_retention_protected() {
    let (scratch, archive) = archive();
    let options = crate::Options {
        claude_home: scratch.0.join("claude"),
        cache: scratch.0.join("index.json"),
        facts: None,
        ..crate::Options::default()
    };
    let path = options.claude_home.join("projects/project/native.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = b"{\"type\":\"user\"}\n";
    fs::write(&path, bytes).unwrap();
    let native = NativeSource {
        home: options.claude_home.clone(),
        root: "claude".into(),
        path: "projects/project/native.jsonl".into(),
        native_id: "native".into(),
        ledger: crate::events::Ledger {
            stat: crate::events::Stat::of(&path).unwrap(),
            offset: bytes.len() as u64,
            prefix: digest(bytes),
            tail: digest(bytes),
        },
        sidecar: None,
        chain_id: String::new(),
    };
    assert!(
        archive
            .admit_native(&mut crate::sealed::LogFile::open(&path).unwrap(), &native)
            .unwrap()
    );
    let manifest = manifest_path(&archive, &path.to_string_lossy());
    let before = crate::events::Stat::of(&manifest).unwrap();
    let _clock = Clock::at(
        read_json::<ArchiveClock>(&archive.dir.join(CLOCK))
            .unwrap()
            .high_water,
    );
    FAIL_BEFORE_RENAME.with(|fail| fail.set(true));
    assert!(
        archive
            .admit_native(&mut crate::sealed::LogFile::open(&path).unwrap(), &native)
            .unwrap()
    );
    assert_eq!(crate::events::Stat::of(&manifest).unwrap(), before);
    FAIL_BEFORE_RENAME
        .with(|fail| assert!(fail.replace(false), "unchanged admission must not publish"));
    assert!(archive.missing_sources(&options).unwrap().is_empty());
    fs::remove_file(&path).unwrap();
    let missing = archive.missing_sources(&options).unwrap();
    assert_eq!(missing.len(), 1);
    let wrong_roots = crate::Options {
        claude_home: scratch.0.join("other"),
        ..options.clone()
    };
    assert!(archive.missing_sources(&wrong_roots).unwrap().is_empty());
    assert_eq!(archive.prune_at(Some(0), u64::MAX).unwrap(), 0);
    archive
        .with_native(&missing[0], |file| {
            let mut read = Vec::new();
            file.read_to_end(&mut read)?;
            assert_eq!(read, bytes);
            Ok(())
        })
        .unwrap();
    fs::write(&path, bytes).unwrap();
    assert!(
        archive.open_native(&missing[0]).is_err(),
        "present native path wins"
    );
    fs::remove_file(&path).unwrap();
    let chain = archive.chains(&path.to_string_lossy()).unwrap().remove(0);
    fs::write(
        chain_file(&archive, &path.to_string_lossy(), &chain),
        b"damaged",
    )
    .unwrap();
    assert!(archive.open_native(&missing[0]).is_err());
}

#[test]
fn clock_rollback_clamps_admission_and_refuses_retention_without_deletion() {
    let clock = Clock::at(2_000_000);
    let (_scratch, archive) = archive();
    let first = preserve(&archive, "first", b"first\n", "none").unwrap();
    clock.set(1_000_000);
    let fresh = preserve(&archive, "fresh", b"fresh\n", "none").unwrap();
    assert_eq!(fresh.admitted_at, first.admitted_at);
    let state = fs::read(archive.dir.join(CLOCK)).unwrap();
    let restarted = Archive::new(archive.dir.clone());
    let error = restarted.prune(Some(7)).unwrap_err();
    assert!(error.to_string().contains("clock moved backward"));
    assert_eq!(fs::read(archive.dir.join(CLOCK)).unwrap(), state);
    assert_eq!(archive.chains("first").unwrap(), [first]);
    assert_eq!(
        archive.chains("fresh").unwrap(),
        std::slice::from_ref(&fresh)
    );
    clock.set(2_003_600);
    assert_eq!(archive.prune(Some(7)).unwrap(), 0);
    assert_eq!(archive.chains("fresh").unwrap(), [fresh]);
    let recorded: ArchiveClock = read_json(&archive.dir.join(CLOCK)).unwrap();
    assert_eq!(recorded.high_water, 2_003_600);
    clock.set(2_003_599);
    assert!(archive.prune(Some(0)).is_err());
}

#[cfg(unix)]
#[test]
fn source_journal_and_lock_symlinks_are_refused_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let (scratch, archive) = archive();
    let target = scratch.0.join("victim");
    let bytes = b"victim must remain unchanged\n";
    fs::write(&target, bytes).unwrap();
    fs::create_dir_all(&archive.dir).unwrap();
    symlink(&target, archive.dir.join(SOURCES)).unwrap();
    assert!(
        archive
            .preserve("source", &mut Cursor::new(b"data\n"), "none")
            .is_err()
    );
    assert_eq!(fs::read(&target).unwrap(), bytes);
    fs::remove_file(archive.dir.join(SOURCES)).unwrap();
    preserve(&archive, "existing", b"existing\n", "none").unwrap();
    let journal = archive.dir.join(SOURCES);
    let swap = journal.clone();
    let victim = target.clone();
    BEFORE_REGISTER_OPEN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            fs::remove_file(&swap).unwrap();
            symlink(victim, swap).unwrap();
        }))
    });
    assert!(
        archive
            .preserve("new", &mut Cursor::new(b"new\n"), "none")
            .is_err()
    );
    assert_eq!(fs::read(&target).unwrap(), bytes);
    fs::remove_file(journal).unwrap();
    fs::remove_file(archive.dir.join(LOCK)).unwrap();
    symlink(&target, archive.dir.join(LOCK)).unwrap();
    assert!(archive.chains("existing").is_err());
    assert_eq!(fs::read(target).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn chain_extension_refuses_swapped_symlinks_regular_files_and_fifos() {
    use std::{
        os::unix::fs::symlink,
        time::{Duration, Instant},
    };
    for replacement in ["symlink", "regular", "fifo"] {
        let (scratch, archive) = archive();
        assert!(admit(&archive, b"first\n", 6));
        let held = archive.chains("native").unwrap().remove(0);
        let path = chain_file(&archive, "native", &held);
        let victim = scratch.0.join("victim");
        let bytes = b"victim must remain unchanged\n";
        fs::write(&victim, bytes).unwrap();
        let swap = path.clone();
        let target = victim.clone();
        BEFORE_EXTEND_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&swap, swap.with_extension("kept")).unwrap();
                match replacement {
                    "symlink" => symlink(target, swap).unwrap(),
                    "regular" => {
                        fs::hard_link(target, swap).unwrap();
                    }
                    "fifo" => assert!(
                        std::process::Command::new("mkfifo")
                            .arg(swap)
                            .status()
                            .unwrap()
                            .success()
                    ),
                    _ => unreachable!(),
                }
            }))
        });
        let started = Instant::now();
        assert!(
            archive
                .admit(
                    "native",
                    &mut Cursor::new(b"first\nsecond\n"),
                    13,
                    &digest(b"first\nsecond\n")
                )
                .is_err(),
            "{replacement}"
        );
        assert!(started.elapsed() < Duration::from_secs(5), "{replacement}");
        BEFORE_EXTEND_OPEN.with(|hook| assert!(hook.borrow().is_none()));
        assert_eq!(fs::read(&victim).unwrap(), bytes, "{replacement}");
        assert_eq!(archive.chains("native").unwrap(), [held]);
    }
}

#[test]
fn bounded_native_checkpoints_extend_custody_without_advertising_incomplete_sources() {
    let (scratch, archive) = archive();
    let options = crate::Options {
        claude_home: scratch.0.join("claude"),
        cache: scratch.0.join("index.json"),
        facts: None,
        ..crate::Options::default()
    };
    let path = options.claude_home.join("projects/project/native.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = b"first\nsecond\nthird\n";
    fs::write(&path, bytes).unwrap();
    let physical = crate::events::Stat::of(&path).unwrap();
    for offset in [6, 13, bytes.len()] {
        let prefix = &bytes[..offset];
        let native = NativeSource {
            home: options.claude_home.clone(),
            root: "claude".into(),
            path: "projects/project/native.jsonl".into(),
            native_id: "native".into(),
            ledger: crate::events::Ledger {
                stat: crate::events::Stat {
                    size: offset as u64,
                    ..physical
                },
                offset: offset as u64,
                prefix: digest(prefix),
                tail: digest(prefix),
            },
            sidecar: None,
            chain_id: String::new(),
        };
        assert!(
            archive
                .admit_native(&mut crate::sealed::LogFile::open(&path).unwrap(), &native)
                .unwrap()
        );
        let chains = archive.chains(&path.to_string_lossy()).unwrap();
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].length, offset as u64);
        let manifest = read_manifest(
            manifest_path(&archive, &path.to_string_lossy())
                .parent()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.native.is_some(), offset == bytes.len());
    }
}

#[test]
fn chain_admission_time_survives_extension_and_reuse() {
    let (_scratch, archive) = archive();
    assert!(admit(&archive, b"first\n", 6));
    let path = manifest_path(&archive, "native");
    let mut manifest = read_manifest(path.parent().unwrap()).unwrap();
    manifest.chains[0].admitted_at = 17;
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    assert!(admit(&archive, b"first\nsecond\n", 13));
    assert!(admit(&archive, b"first\nsecond\n", 13));
    assert_eq!(archive.chains("native").unwrap()[0].admitted_at, 17);
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
    let _clock = Clock::at(2_000_000);
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
    let _clock = Clock::at(2_000_000);
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

#[test]
fn archive_ahead_bounded_rebuild_compares_each_prefix_byte_once() {
    let (_scratch, archive) = archive();
    let log = lines(400_000, 1);
    assert!(admit(&archive, &log, log.len()));
    let reopened = Archive::new(&archive.dir);
    let before = VERIFIED_BYTES.with(std::cell::Cell::get);
    for offset in [40_000, 80_000, 120_000, 160_000, 200_000] {
        assert!(admit(&reopened, &log, offset));
        assert_eq!(
            VERIFIED_BYTES.with(std::cell::Cell::get) - before,
            log.len() as u64 + offset as u64
        );
    }
    assert_eq!(archive.chains("native").unwrap().len(), 1);
}

#[test]
fn archive_lock_unlocks_while_inherited_descriptor_stays_open() {
    let (_scratch, archive) = archive();
    INHERITED_LOCKS.with(|capture| *capture.borrow_mut() = Some(Vec::new()));
    let guard = archive.lock(true).unwrap();
    let inherited = INHERITED_LOCKS.with(|capture| capture.borrow_mut().take().unwrap());
    assert_eq!(inherited.len(), 1);

    // A dup'd descriptor shares the open file description, so closing the
    // guard alone leaves flock held; explicit LOCK_UN releases it for all clones.
    drop(guard);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(archive.dir.join(LOCK))
        .unwrap();
    use std::os::fd::AsRawFd;
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(
        locked, 0,
        "archive lock remained held by inherited descriptor"
    );
    drop(file);
    drop(inherited);
}

#[test]
fn unchanged_shorter_prefix_reuses_its_archived_comparison() {
    let (_scratch, archive) = archive();
    let log = lines(200_000, 1);
    assert!(admit(&archive, &log, log.len()));
    let reopened = Archive::new(&archive.dir);
    let before = VERIFIED_BYTES.with(std::cell::Cell::get);
    for _ in 0..4 {
        assert!(admit(&reopened, &log[..40_000], 40_000));
        assert_eq!(VERIFIED_BYTES.with(std::cell::Cell::get) - before, 240_000);
    }
    assert_eq!(archive.chains("native").unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn planted_archive_temporary_symlinks_never_truncate_the_target() {
    use std::os::unix::fs::symlink;
    let (_scratch, archive) = archive();
    let held = preserve(&archive, "s", b"keep these bytes", "1").unwrap();
    let target = chain_file(&archive, "s", &held);
    let before = fs::read(&target).unwrap();
    let dir = target.parent().unwrap();
    let planted = dir.join("manifest.json.tmp");
    symlink(&target, &planted).unwrap();
    assert!(create_file(&planted).is_err());
    preserve(&archive, "s", b"other bytes", "1").unwrap();
    assert_eq!(fs::read(&target).unwrap(), before);
    assert!(
        fs::symlink_metadata(planted)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

fn prefix_resources(archive: &Archive) -> (usize, usize) {
    let known = archive.known();
    let progress: Vec<_> = known
        .verified
        .values()
        .filter_map(|held| held.prefix.as_ref())
        .collect();
    assert_eq!(
        known.active_prefixes,
        progress.len(),
        "capacity count matches live decoder objects"
    );
    (
        progress.len(),
        progress.iter().map(|prefix| prefix.digests.len()).sum(),
    )
}

#[test]
fn many_archive_ahead_sources_bound_live_decoders_and_release_on_catch_up() {
    let (_scratch, archive) = archive();
    let log = lines(4096, 1);
    let sources: Vec<_> = (0..MAX_PREFIX_READERS * 2)
        .map(|n| format!("native-{n}"))
        .collect();
    for source in &sources {
        archive
            .admit(
                source,
                &mut Cursor::new(&log),
                log.len() as u64,
                &digest(&log),
            )
            .unwrap();
    }
    for (n, source) in sources.iter().enumerate() {
        archive
            .admit(source, &mut Cursor::new(&log), 1024, &digest(&log[..1024]))
            .unwrap();
        let (readers, digests) = prefix_resources(&archive);
        assert_eq!(readers, (n + 1).min(MAX_PREFIX_READERS));
        assert_eq!(digests, readers);
        assert_eq!(
            archive.chains(source).unwrap().len(),
            if n < MAX_PREFIX_READERS { 1 } else { 2 }
        );
    }
    // Capacity pressure must not evict active sources and restart their scans.
    let before = VERIFIED_BYTES.with(std::cell::Cell::get);
    for source in &sources[..MAX_PREFIX_READERS] {
        archive
            .admit(source, &mut Cursor::new(&log), 2048, &digest(&log[..2048]))
            .unwrap();
    }
    assert_eq!(
        VERIFIED_BYTES.with(std::cell::Cell::get) - before,
        MAX_PREFIX_READERS as u64 * 1024
    );
    for source in &sources {
        archive
            .admit(
                source,
                &mut Cursor::new(&log),
                log.len() as u64,
                &digest(&log),
            )
            .unwrap();
    }
    assert_eq!(
        prefix_resources(&archive),
        (0, 0),
        "catch-up drops decoder objects and their open files"
    );
}

#[test]
fn prefix_digest_capacity_starts_a_chain_without_rescanning() {
    let (_scratch, archive) = archive();
    let log = lines(MAX_PREFIX_DIGESTS * 4, 1);
    assert!(admit(&archive, &log, log.len()));
    for offset in 1..=MAX_PREFIX_DIGESTS {
        assert!(admit(&archive, &log, offset));
    }
    assert_eq!(prefix_resources(&archive), (1, MAX_PREFIX_DIGESTS));
    let before = VERIFIED_BYTES.with(std::cell::Cell::get);
    assert!(admit(&archive, &log, MAX_PREFIX_DIGESTS + 1));
    assert_eq!(prefix_resources(&archive), (0, 0));
    assert_eq!(archive.chains("native").unwrap().len(), 2);
    assert!(admit(&archive, &log, MAX_PREFIX_DIGESTS + 2));
    assert_eq!(
        VERIFIED_BYTES.with(std::cell::Cell::get),
        before,
        "overflow writes native bytes, never replays the archived prefix"
    );
}

#[test]
fn damaged_and_superseded_chains_release_prefix_decoders() {
    let (_scratch, archive) = archive();
    let log = lines(4096, 1);
    assert!(admit(&archive, &log, log.len()));
    assert!(admit(&archive, &log, 1024));
    let old = archive.chains("native").unwrap().remove(0);
    let path = chain_file(&archive, "native", &old);
    let damaged = b"damaged evidence";
    fs::write(&path, damaged).unwrap();
    assert!(admit(&archive, &log, 1024));
    assert_eq!(prefix_resources(&archive), (0, 0));
    assert_eq!(fs::read(&path).unwrap(), damaged);
    assert_eq!(archive.chains("native").unwrap().len(), 2);

    assert!(admit(&archive, &log, log.len()));
    assert!(admit(&archive, &log, 2048));
    assert_eq!(prefix_resources(&archive).0, 1);
    let other = Archive::new(&archive.dir);
    preserve(&other, "native", &log, "new-version").unwrap();
    // Observing another writer's successor releases superseded progress.
    assert!(admit(&archive, &log, log.len()));
    assert_eq!(prefix_resources(&archive), (0, 0));
}

#[test]
fn prefix_capacity_accounting_ignores_completed_history_and_cached_comparisons() {
    let (_scratch, archive) = archive();
    let log = lines(512, 1);
    for n in 0..128 {
        let source = format!("completed-{n}");
        archive
            .admit(
                &source,
                &mut Cursor::new(&log),
                log.len() as u64,
                &digest(&log),
            )
            .unwrap();
        archive
            .admit(&source, &mut Cursor::new(&log), 128, &digest(&log[..128]))
            .unwrap();
        archive
            .admit(
                &source,
                &mut Cursor::new(&log),
                log.len() as u64,
                &digest(&log),
            )
            .unwrap();
    }
    assert_eq!(prefix_resources(&archive), (0, 0));
    assert!(archive.known().verified.len() >= 128);
    assert!(admit(&archive, &log, log.len()));
    let work = PREFIX_CAPACITY_WORK.with(std::cell::Cell::get);
    assert!(admit(&archive, &log, 128));
    assert_eq!(
        PREFIX_CAPACITY_WORK.with(std::cell::Cell::get) - work,
        1,
        "one constant-time capacity lookup, regardless of history"
    );
    let work = PREFIX_CAPACITY_WORK.with(std::cell::Cell::get);
    let bytes = VERIFIED_BYTES.with(std::cell::Cell::get);
    for _ in 0..128 {
        assert!(admit(&archive, &log, 128));
    }
    assert_eq!(
        PREFIX_CAPACITY_WORK.with(std::cell::Cell::get),
        work,
        "cached comparisons perform no capacity accounting"
    );
    assert_eq!(VERIFIED_BYTES.with(std::cell::Cell::get), bytes);
    assert_eq!(prefix_resources(&archive), (1, 1));
    assert!(admit(&archive, &log, log.len()));
    assert_eq!(prefix_resources(&archive), (0, 0));
}

#[test]
fn a_failed_prefix_decoder_open_does_not_reserve_capacity() {
    let (_scratch, archive) = archive();
    let log = lines(512, 1);
    assert!(admit(&archive, &log, log.len()));
    let chain = archive.chains("native").unwrap().remove(0);
    let path = chain_file(&archive, "native", &chain);
    fs::remove_file(&path).unwrap();
    // Exercise the open-failure boundary after verification, without allocating a slot.
    assert!(
        archive
            .matches_prefix(path.parent().unwrap(), &chain, 128, &digest(&log[..128]))
            .is_err()
    );
    assert_eq!(prefix_resources(&archive), (0, 0));
    assert!(admit(&archive, &log, 128));
    assert_eq!(prefix_resources(&archive), (0, 0));
}
