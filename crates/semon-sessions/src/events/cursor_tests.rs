//! The same restart/cursor lifecycle runs against both configured adapters.
use super::*;
use crate::storage::native_events::NativeEventStore;
use std::io::Write;

pub(crate) fn lifecycle(root: &Path, mut open: impl FnMut() -> EventCache) {
    let path = root.join("cursor.jsonl");
    let record = |id: &str| {
        serde_json::to_string(&serde_json::json!({"type":"user","uuid":id,
            "message":{"role":"user","content":id}}))
        .unwrap()
    };
    let first = format!("{}\n", record("first"));
    let second = record("second");
    fs::write(&path, format!("{first}{second}{}", " ".repeat(4096))).unwrap();
    let mut scan = |operation: u64| {
        let mut cache = open();
        cache
            .ingest(&format!("{operation:064x}"), &path, "claude")
            .unwrap();
        let index = scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        let ledger = cache
            .stored_ledger_batch(&[path.to_string_lossy().into_owned()])
            .unwrap()
            .remove(0)
            .unwrap();
        (ledger, index)
    };
    let (initial, index) = scan(1);
    assert_eq!(initial.offset, first.len() as u64);
    assert_eq!(index.events.len(), 1, "partial JSON is never committed");
    drop(index);
    // A shrink can remain above the committed cursor: it still resets.
    fs::write(&path, format!("{first}{second}")).unwrap();
    let before = PARSED.with(std::cell::Cell::get);
    let (shrunk, index) = scan(2);
    assert_eq!(shrunk.offset, initial.offset);
    assert_eq!(
        PARSED.with(std::cell::Cell::get) - before,
        1,
        "shrink rereads from zero"
    );
    assert_eq!(index.events.len(), 1);
    drop(index);
    let suffix = "\nmalformed native line\n";
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(suffix.as_bytes())
        .unwrap();
    let (complete, index) = scan(3);
    assert_eq!(complete.offset, fs::metadata(&path).unwrap().len());
    assert_eq!(index.events.len(), 3);
    assert_eq!(index.tree_summary.as_ref().unwrap().malformed_lines, 1);
    assert_eq!(
        index
            .events
            .iter()
            .filter(|event| event.k == Kind::Gap)
            .count(),
        1
    );
    drop(index);
    let (_, index) = scan(4);
    assert_eq!(index.tree_summary.as_ref().unwrap().malformed_lines, 1);
    assert_eq!(
        index.events.len(),
        3,
        "restart must not duplicate events or anomalies"
    );
    drop(index);
    let third = format!("{}\n", record("third"));
    let replacement = root.join("rotated.jsonl");
    fs::write(&replacement, format!("{first}{second}{suffix}{third}")).unwrap();
    fs::rename(&replacement, &path).unwrap();
    let before = PARSED.with(std::cell::Cell::get);
    let (rotated, index) = scan(5);
    assert_ne!(rotated.stat.ino, complete.stat.ino);
    assert_eq!(
        PARSED.with(std::cell::Cell::get) - before,
        4,
        "new inode rereads every complete line"
    );
    assert_eq!(
        index.events.len(),
        4,
        "reread replaces the per-file projection"
    );
    assert_eq!(
        index
            .events
            .iter()
            .filter(|event| event.k == Kind::Gap)
            .count(),
        1
    );
    drop(index);
    fs::write(&path, format!("{first}{third}")).unwrap();
    let (_, index) = scan(6);
    assert_eq!(index.events.len(), 2);
    assert!(index.events.iter().all(|event| event.k != Kind::Gap));
}

#[test]
fn sqlite_native_cursor_newlines_anomalies_shrink_and_inode_restart() {
    let root = std::env::temp_dir().join(format!("cursor520-sqlite-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    lifecycle(&root, || {
        EventCache::open(&root.join("cache.json"), &crate::Options::default().source).unwrap()
    });
    fs::remove_dir_all(root).unwrap();
}
