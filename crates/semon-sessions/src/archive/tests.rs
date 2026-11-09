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
    let index: Index = read_json(&archive.dir.join(INDEX)).unwrap();
    assert_eq!(index.sources.len(), 2);
    assert!(index.sources.values().all(|id| is_id(id)));
    let (a, b) = (
        &index.sources["claude/a.jsonl"],
        &index.sources["claude/b.jsonl"],
    );
    assert_ne!(a, b);
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
    // Another instance finds the source once the index is flushed.
    archive.flush().unwrap();
    let other = Archive::new(archive.dir.clone());
    assert_eq!(
        other.chains("native").unwrap(),
        std::slice::from_ref(&chain)
    );
    // The index may be behind the archive (it was rebuilt): nothing changes.
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
    manifest.chains[0].resume = None;
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    assert!(archive.chains("native").is_ok());
    manifest.chains[0].length = 0;
    publish(path.parent().unwrap(), MANIFEST, &manifest).unwrap();
    assert!(archive.chains("native").is_err());
}
