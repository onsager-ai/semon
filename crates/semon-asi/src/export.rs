//! Verified local analytics snapshot export and guarded reset.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use arrow::array::StringArray;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Error, Manifest, Result,
    parquet_io::visit_batches,
    schema::SESSION_COLUMN,
    store::{LockGuard, read_live_map, read_manifest},
};

const ANALYTICS_DIR: &str = "analytics";
const LOCK_FILE: &str = "analytics.lock";
const MANIFEST_FILE: &str = "manifest.json";
const RECEIPT_FILE: &str = "export.json";
const RECEIPT_VERSION: u32 = 1;

/// One copied export file, recorded with its content digest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExportFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The final export record. Its presence means the bundle was copied and verified.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExportReceipt {
    pub format_version: u32,
    pub source_namespace: String,
    pub manifest_generation: u64,
    pub files: Vec<ExportFile>,
}

/// A completed export's stable identity and counts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportSummary {
    pub generation: u64,
    pub sessions: u64,
    pub files: u64,
}

/// The published generation removed by reset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetSummary {
    pub generation: u64,
    pub removed: Vec<String>,
}

/// Copy one current, immutable local snapshot. `destination` must be empty or absent.
///
/// The caller owns admission of the state directory and namespace file. This function
/// does not create or recover the source store; it only opens its existing lock shared.
pub fn export_snapshot(
    root: &Path,
    namespace_file: &Path,
    namespace: &str,
    destination: &Path,
) -> Result<ExportSummary> {
    let (destination, created_destination) = prepare_destination(root, destination)?;
    let copied = (|| {
        let lock = open_lock(root, false)?;
        let manifest = validate_snapshot(root)?;
        require_regular_file(namespace_file, "analytics namespace")?;
        let source_namespace = fs::read_to_string(namespace_file)?;
        if source_namespace.trim_end_matches('\n') != namespace {
            return Err(Error::invalid(
                "analytics namespace changed before export lock was acquired",
            ));
        }
        require_namespace_ownership(root, &manifest, namespace)?;
        let files = snapshot_files(root, &manifest)?;
        let mut receipt_files = Vec::with_capacity(files.len() + 1);
        let (bytes, sha256) =
            copy_and_hash(namespace_file, &destination.join("analytics-namespace"))?;
        receipt_files.push(ExportFile {
            path: "analytics-namespace".to_owned(),
            bytes,
            sha256,
        });
        for relative in &files {
            let source = root.join(relative);
            let target = destination.join(relative);
            let (bytes, sha256) = copy_and_hash(&source, &target)?;
            receipt_files.push(ExportFile {
                path: relative.to_string_lossy().into_owned(),
                bytes,
                sha256,
            });
        }
        let sessions = u64::try_from(read_live_map(root, &manifest)?.len())
            .map_err(|_| Error::invalid("live session count does not fit in u64"))?;
        let file_count = u64::try_from(manifest.files.len())
            .map_err(|_| Error::invalid("manifest file count does not fit in u64"))?;
        drop(lock);

        let receipt = ExportReceipt {
            format_version: RECEIPT_VERSION,
            source_namespace: namespace.to_owned(),
            manifest_generation: manifest.generation,
            files: receipt_files,
        };
        verify_receipt(&destination, &receipt)?;
        write_receipt(&destination, &receipt)?;
        Ok(ExportSummary {
            generation: manifest.generation,
            sessions,
            files: file_count,
        })
    })();
    if copied.is_err() {
        cleanup_destination(&destination, created_destination);
    }
    copied
}

/// Load and verify a completed export bundle. The receipt is trusted only after every
/// named file's size and digest, manifest, and live map agree.
pub fn verify_export(bundle: &Path) -> Result<ExportReceipt> {
    require_directory(bundle, "export bundle")?;
    let receipt_path = bundle.join(RECEIPT_FILE);
    require_regular_file(&receipt_path, "export receipt")?;
    let receipt: ExportReceipt = serde_json::from_slice(&fs::read(&receipt_path)?)?;
    if receipt.format_version != RECEIPT_VERSION {
        return Err(Error::invalid(format!(
            "export receipt format {} is not {RECEIPT_VERSION}",
            receipt.format_version
        )));
    }
    verify_receipt(bundle, &receipt)?;
    Ok(receipt)
}

/// Remove the local store after its caller has validated ownership and confirmation.
/// The exclusive writer lock covers namespace removal and store invalidation. The manifest
/// is removed before every other store entry so a partial failure is never readable as a
/// valid snapshot. The lock file and store root remain so another writer cannot acquire a
/// replacement lock inode before this operation has released ownership.
/// `retire_namespace_handles` is `None` to keep namespace identity and mappings;
/// `Some(state_dir/analytics-egress-handles.sqlite)` retires both under this lock.
pub fn reset_snapshot(
    root: &Path,
    namespace_file: &Path,
    expected_namespace: &str,
    verified_export: Option<&Path>,
    expected_generation: u64,
    retire_namespace_handles: Option<&Path>,
) -> Result<ResetSummary> {
    reset_snapshot_with(
        root,
        namespace_file,
        expected_namespace,
        verified_export,
        expected_generation,
        retire_namespace_handles,
        |_| Ok(()),
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ResetStage {
    HandlesRetired,
    Completed,
}

pub(crate) fn reset_snapshot_with<F: FnMut(ResetStage) -> Result<()>>(
    root: &Path,
    namespace_file: &Path,
    expected_namespace: &str,
    verified_export: Option<&Path>,
    expected_generation: u64,
    retire_namespace_handles: Option<&Path>,
    mut on_locked: F,
) -> Result<ResetSummary> {
    let _lock = open_lock(root, true)?;
    require_regular_file(namespace_file, "analytics namespace")?;
    let namespace = fs::read_to_string(namespace_file)?;
    if namespace.trim_end_matches('\n') != expected_namespace {
        return Err(Error::invalid(
            "analytics namespace changed before reset lock was acquired",
        ));
    }
    let manifest = validate_snapshot(root)?;
    require_namespace_ownership(root, &manifest, expected_namespace)?;
    if let Some(bundle) = verified_export {
        reject_overlapping_bundle(root, bundle)?;
        let receipt = verify_export(bundle)?;
        if receipt.source_namespace != expected_namespace
            || receipt.manifest_generation != manifest.generation
        {
            return Err(Error::invalid(
                "verified export does not match the current namespace and manifest generation",
            ));
        }
        verify_current_export(root, namespace_file, &manifest, &receipt)?;
    }
    if manifest.generation != expected_generation {
        return Err(Error::invalid(
            "published manifest changed after verified export; export again before reset",
        ));
    }
    if let Some(handles) = retire_namespace_handles {
        let parent = namespace_file
            .parent()
            .ok_or_else(|| Error::invalid("namespace has no state directory"))?;
        if handles != parent.join(crate::egress::HANDLES_FILE) {
            return Err(Error::invalid("handle registry must be beside namespace"));
        }
        match fs::symlink_metadata(handles) {
            Ok(_) => require_regular_file(handles, "egress handle registry")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let analytics = root.join(ANALYTICS_DIR);
    let manifest_path = analytics.join(MANIFEST_FILE);
    fs::remove_file(&manifest_path)?;
    let mut removed = vec![
        PathBuf::from(ANALYTICS_DIR)
            .join(MANIFEST_FILE)
            .to_string_lossy()
            .into_owned(),
    ];
    for entry in fs::read_dir(&analytics)? {
        let entry = entry?;
        let name = entry.file_name();
        let relative = PathBuf::from(ANALYTICS_DIR).join(&name);
        fs::remove_dir_all(entry.path())?;
        removed.push(relative.to_string_lossy().into_owned());
    }
    fs::remove_dir(&analytics)?;
    removed.push(ANALYTICS_DIR.to_owned());
    if let Some(handles) = retire_namespace_handles {
        match fs::remove_file(handles) {
            Ok(()) => removed.push(crate::egress::HANDLES_FILE.to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let state = File::open(namespace_file.parent().expect("validated namespace parent"))?;
        state.sync_all()?;
        on_locked(ResetStage::HandlesRetired)?;
        fs::remove_file(namespace_file)?;
        removed.push("analytics-namespace".to_owned());
        state.sync_all()?;
    }
    on_locked(ResetStage::Completed)?;
    Ok(ResetSummary {
        generation: manifest.generation,
        removed,
    })
}

pub(crate) fn require_namespace_ownership(
    root: &Path,
    manifest: &Manifest,
    namespace: &str,
) -> Result<()> {
    let Some(live_map) = &manifest.live_map else {
        return Ok(());
    };
    let prefix = format!("{namespace}/");
    let path = root.join(ANALYTICS_DIR).join(live_map);
    visit_batches(&path, Some(&[SESSION_COLUMN]), |batch| {
        let uids = batch
            .column_by_name(SESSION_COLUMN)
            .ok_or_else(|| Error::invalid("published live map lacks session_uid"))?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| Error::invalid("published live session_uid is not text"))?;
        for row in 0..batch.num_rows() {
            if !uids.value(row).starts_with(&prefix) {
                return Err(Error::invalid(
                    "published live sessions do not belong to the analytics namespace",
                ));
            }
        }
        Ok(())
    })
}

fn reject_overlapping_bundle(root: &Path, bundle: &Path) -> Result<()> {
    let root = canonical_directory(root, "analytics store")?;
    let bundle = canonical_directory(bundle, "export bundle")?;
    if bundle.starts_with(&root) || root.starts_with(&bundle) {
        return Err(Error::invalid(
            "verified export must be outside the analytics store",
        ));
    }
    Ok(())
}

fn verify_current_export(
    root: &Path,
    namespace_file: &Path,
    manifest: &Manifest,
    receipt: &ExportReceipt,
) -> Result<()> {
    let mut current = BTreeMap::new();
    for relative in
        std::iter::once(PathBuf::from("analytics-namespace")).chain(snapshot_files(root, manifest)?)
    {
        let path = if relative == Path::new("analytics-namespace") {
            namespace_file.to_path_buf()
        } else {
            root.join(&relative)
        };
        let (bytes, sha256) = hash_file(&path)?;
        current.insert(relative.to_string_lossy().into_owned(), (bytes, sha256));
    }
    let receipt_files: BTreeMap<&str, (u64, &str)> = receipt
        .files
        .iter()
        .map(|file| (file.path.as_str(), (file.bytes, file.sha256.as_str())))
        .collect();
    if current.len() != receipt_files.len()
        || current.iter().any(|(path, file)| {
            receipt_files
                .get(path.as_str())
                .is_none_or(|receipt_file| file.0 != receipt_file.0 || file.1 != receipt_file.1)
        })
    {
        return Err(Error::invalid(
            "verified export does not match the current published snapshot",
        ));
    }
    Ok(())
}

fn prepare_destination(root: &Path, destination: &Path) -> Result<(PathBuf, bool)> {
    let source = canonical_directory(root, "analytics store")?;
    let parent = destination.parent().ok_or_else(|| {
        Error::invalid(format!(
            "export destination has no parent: {}",
            destination.display()
        ))
    })?;
    let parent = canonical_directory(parent, "export destination parent")?;
    let destination = parent.join(destination.file_name().ok_or_else(|| {
        Error::invalid(format!(
            "invalid export destination: {}",
            destination.display()
        ))
    })?);
    if destination.starts_with(&source) {
        return Err(Error::invalid(
            "export destination is inside the source store",
        ));
    }
    let ancestor = nearest_existing_ancestor(&destination)?;
    if ancestor.starts_with(&source) {
        return Err(Error::invalid(
            "export destination is inside the source store",
        ));
    }
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Error::invalid(format!(
            "export destination is a symlink: {}",
            destination.display()
        ))),
        Ok(metadata) if !metadata.file_type().is_dir() => Err(Error::invalid(format!(
            "export destination is not a directory: {}",
            destination.display()
        ))),
        Ok(_) => {
            if fs::read_dir(&destination)?.next().is_some() {
                return Err(Error::invalid(format!(
                    "export destination is not empty: {}",
                    destination.display()
                )));
            }
            Ok((destination, false))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&destination)?;
            Ok((destination, true))
        }
        Err(error) => Err(error.into()),
    }
}

fn nearest_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut current = path;
    while !current.exists() {
        current = current.parent().ok_or_else(|| {
            Error::invalid(format!(
                "export destination has no existing ancestor: {}",
                path.display()
            ))
        })?;
    }
    canonical_directory(current, "export destination ancestor")
}

fn cleanup_destination(destination: &Path, created_destination: bool) {
    if created_destination {
        let _ = fs::remove_dir_all(destination);
        return;
    }
    let _ = fs::remove_file(destination.join("analytics-namespace"));
    let _ = fs::remove_dir_all(destination.join(ANALYTICS_DIR));
    let _ = fs::remove_file(destination.join(RECEIPT_FILE));
}

pub(crate) fn open_lock(root: &Path, exclusive: bool) -> Result<LockGuard> {
    require_directory(root, "analytics store")?;
    let path = root.join(LOCK_FILE);
    require_regular_file(&path, "analytics lock")?;
    let lock = OpenOptions::new().read(true).write(exclusive).open(&path)?;
    LockGuard::acquire(lock, exclusive)
}

pub(crate) fn validate_snapshot(root: &Path) -> Result<Manifest> {
    require_directory(root, "analytics store")?;
    let analytics = root.join(ANALYTICS_DIR);
    require_directory(&analytics, "analytics directory")?;
    let manifest = read_manifest(root)?.ok_or_else(|| Error::invalid("no published manifest"))?;
    let paths = snapshot_files(root, &manifest)?;
    for path in paths {
        require_regular_file(&root.join(path), "published analytics file")?;
    }
    let _live = read_live_map(root, &manifest)?;
    crate::provenance::read(root, &manifest)?;
    Ok(manifest)
}

fn snapshot_files(root: &Path, manifest: &Manifest) -> Result<Vec<PathBuf>> {
    let analytics = canonical_directory(&root.join(ANALYTICS_DIR), "analytics directory")?;
    let mut files = BTreeSet::from([PathBuf::from(ANALYTICS_DIR).join(MANIFEST_FILE)]);
    if let Some(live_map) = &manifest.live_map {
        files.insert(analytics_relative_path(&analytics, live_map, "live")?);
    }
    for entry in &manifest.files {
        files.insert(analytics_relative_path(
            &analytics,
            &entry.path,
            &entry.table,
        )?);
    }
    Ok(files.into_iter().collect())
}

fn analytics_relative_path(
    analytics: &Path,
    relative: &str,
    expected_parent: &str,
) -> Result<PathBuf> {
    let relative = Path::new(relative);
    let mut components = relative.components();
    let parent = components.next();
    let file = components.next();
    if !matches!(parent, Some(Component::Normal(parent)) if parent.to_str() == Some(expected_parent))
        || !matches!(file, Some(Component::Normal(_)))
        || components.next().is_some()
        || relative
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("parquet")
    {
        return Err(Error::invalid(format!(
            "published {expected_parent} path is invalid: {}",
            relative.display()
        )));
    }
    let parent_path = analytics.join(expected_parent);
    require_directory(&parent_path, "published table directory")?;
    let path = parent_path.join(file.expect("validated file component"));
    require_regular_file(&path, "published analytics file")?;
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(analytics) {
        return Err(Error::invalid(
            "published analytics file escapes analytics directory",
        ));
    }
    Ok(PathBuf::from(ANALYTICS_DIR).join(relative))
}

fn verify_receipt(bundle: &Path, receipt: &ExportReceipt) -> Result<()> {
    let manifest = validate_snapshot(bundle)?;
    let expected: BTreeSet<String> = std::iter::once(PathBuf::from("analytics-namespace"))
        .chain(snapshot_files(bundle, &manifest)?)
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let recorded: BTreeSet<&str> = receipt
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    if expected.len() != receipt.files.len()
        || expected
            .iter()
            .any(|path| !recorded.contains(path.as_str()))
    {
        return Err(Error::invalid(
            "export receipt files do not match the published snapshot",
        ));
    }
    if receipt.manifest_generation != manifest.generation {
        return Err(Error::invalid(
            "export receipt generation does not match manifest",
        ));
    }
    let namespace = fs::read_to_string(bundle.join("analytics-namespace"))?;
    if namespace.trim_end_matches('\n') != receipt.source_namespace {
        return Err(Error::invalid(
            "export receipt namespace does not match bundle",
        ));
    }
    for file in &receipt.files {
        let path = checked_bundle_path(bundle, &file.path)?;
        let (bytes, sha256) = hash_file(&path)?;
        if bytes != file.bytes || sha256 != file.sha256 {
            return Err(Error::invalid(format!(
                "export file verification failed: {}",
                file.path
            )));
        }
    }
    Ok(())
}

fn checked_bundle_path(bundle: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::invalid(format!(
            "export receipt path is invalid: {}",
            relative.display()
        )));
    }
    let path = bundle.join(relative);
    require_regular_file(&path, "export file")?;
    let bundle = canonical_directory(bundle, "export bundle")?;
    if !path.canonicalize()?.starts_with(bundle) {
        return Err(Error::invalid("export file escapes bundle"));
    }
    Ok(path)
}

fn copy_and_hash(source: &Path, target: &Path) -> Result<(u64, String)> {
    let parent = target
        .parent()
        .ok_or_else(|| Error::invalid("export target has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut source = File::open(source)?;
    let mut target = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        target.write_all(&buffer[..read])?;
        hash.update(&buffer[..read]);
        bytes = bytes
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| Error::invalid("copy length does not fit in u64"))?,
            )
            .ok_or_else(|| Error::invalid("export file is too large"))?;
    }
    target.sync_all()?;
    Ok((bytes, hex::encode(hash.finalize())))
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        bytes = bytes
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| Error::invalid("hash length does not fit in u64"))?,
            )
            .ok_or_else(|| Error::invalid("export file is too large"))?;
    }
    Ok((bytes, hex::encode(hash.finalize())))
}

fn write_receipt(destination: &Path, receipt: &ExportReceipt) -> Result<()> {
    let path = destination.join(RECEIPT_FILE);
    let bytes = serde_json::to_vec_pretty(receipt)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(destination)?.sync_all()?;
    Ok(())
}

fn canonical_directory(path: &Path, kind: &str) -> Result<PathBuf> {
    require_directory(path, kind)?;
    Ok(path.canonicalize()?)
}

fn require_directory(path: &Path, kind: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(Error::invalid(format!(
            "{kind} is not a real directory: {}",
            path.display()
        ))),
        Err(error) => Err(error.into()),
    }
}

fn require_regular_file(path: &Path, kind: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(Error::invalid(format!(
            "{kind} is not a regular file: {}",
            path.display()
        ))),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Store,
        testutil::{TestClock, update},
    };

    // Capture dup'd open descriptions at real acquisition sites, just as fork
    // would inherit them. Keep them alive through the immediate reopen assertion.
    fn inherited_during<T>(operation: impl FnOnce() -> T) -> (T, Vec<File>) {
        crate::store::INHERITED_LOCKS.with(|capture| *capture.borrow_mut() = Some(Vec::new()));
        let result = operation();
        let files =
            crate::store::INHERITED_LOCKS.with(|capture| capture.borrow_mut().take().unwrap());
        assert!(!files.is_empty());
        (result, files)
    }
    #[test]
    fn real_lock_owners_release_inherited_descriptors_on_success_and_error() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("store");
        let namespace = temporary.path().join("namespace");
        fs::write(&namespace, "origin-test\n").unwrap();
        let (_, writer_fds) = inherited_during(|| {
            let mut store = Store::open(&root).unwrap();
            store
                .publish(&update(&[("origin-test/claude/session", 1)], "a", 0))
                .unwrap();
            assert!(matches!(Store::open(&root), Err(Error::WriterBusy)));
            drop(store);
        });
        drop(Store::open(&root).unwrap());
        let bundle = temporary.path().join("bundle");
        let (result, export_fds) =
            inherited_during(|| export_snapshot(&root, &namespace, "origin-test", &bundle));
        result.unwrap();
        drop(Store::open(&root).unwrap());
        let (result, export_error_fds) = inherited_during(|| {
            export_snapshot(
                &root,
                &namespace,
                "wrong",
                &temporary.path().join("bad-bundle"),
            )
        });
        assert!(result.is_err());
        drop(Store::open(&root).unwrap());
        let (result, error_fds) =
            inherited_during(|| reset_snapshot(&root, &namespace, "wrong", None, 1, None));
        assert!(result.is_err());
        drop(Store::open(&root).unwrap());
        let (result, reset_fds) =
            inherited_during(|| reset_snapshot(&root, &namespace, "origin-test", None, 1, None));
        result.unwrap();
        drop(Store::open(&root).unwrap());
        drop((
            writer_fds,
            export_fds,
            export_error_fds,
            error_fds,
            reset_fds,
        ));
    }
    #[test]
    fn custody_export_hashes_provenance_and_refuses_tampering() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("store");
        let namespace_file = temporary.path().join("namespace");
        fs::write(&namespace_file, "origin-test\n").unwrap();
        let mut store = Store::open_with_clock(&root, TestClock::new(0)).unwrap();
        store
            .publish(&update(&[("origin-test/claude/session", 1)], "a", 0))
            .unwrap();
        drop(store);
        let bundle = temporary.path().join("bundle");
        export_snapshot(&root, &namespace_file, "origin-test", &bundle).unwrap();
        let receipt = verify_export(&bundle).unwrap();
        let provenance = receipt
            .files
            .iter()
            .find(|file| file.path.starts_with("analytics/provenance/"))
            .unwrap();
        assert!(provenance.bytes > 0);
        let manifest = read_manifest(&root).unwrap().unwrap();
        verify_current_export(&root, &namespace_file, &manifest, &receipt).unwrap();
        fs::write(bundle.join(&provenance.path), "tampered").unwrap();
        assert!(verify_export(&bundle).is_err());
        fs::write(root.join(&provenance.path), "tampered").unwrap();
        assert!(verify_current_export(&root, &namespace_file, &manifest, &receipt).is_err());
    }

    #[test]
    fn reset_removes_new_namespace_before_releasing_writer_lock() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path();
        let root = state.join("analytics-store");
        let namespace_file = state.join("analytics-namespace");
        let namespace = "origin-test";
        fs::write(&namespace_file, format!("{namespace}\n")).unwrap();
        let clock = TestClock::new(0);
        let mut store = Store::open_with_clock(&root, clock).unwrap();
        store
            .publish(&update(&[("origin-test/claude/session", 1)], "test", 0))
            .unwrap();
        drop(store);

        let handles_path = state.join(crate::egress::HANDLES_FILE);
        drop(crate::egress::Handles::open(state, namespace).unwrap());
        let summary = reset_snapshot_with(
            &root,
            &namespace_file,
            namespace,
            None,
            1,
            Some(&handles_path),
            |stage| {
                assert_eq!(namespace_file.exists(), stage == ResetStage::HandlesRetired);
                assert!(!handles_path.exists());
                let lock = OpenOptions::new()
                    .write(true)
                    .open(root.join(LOCK_FILE))
                    .unwrap();
                assert!(matches!(
                    lock.try_lock(),
                    Err(std::fs::TryLockError::WouldBlock)
                ));
                Ok(())
            },
        )
        .unwrap();

        assert!(
            summary
                .removed
                .iter()
                .any(|path| path == "analytics-namespace")
        );
        assert!(root.join(LOCK_FILE).is_file());
        assert!(!root.join(ANALYTICS_DIR).exists());
    }
}
