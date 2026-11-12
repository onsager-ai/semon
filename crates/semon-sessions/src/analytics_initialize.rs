//! Explicit producer admission, with no source refresh or publication.
use crate::{analytics_namespace, analytics_writer::Owner, events::store};
use std::{fs, path::Path};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub namespace: String,
    pub origin_incarnation: String,
    pub index_incarnation: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposition {
    Created,
    Existing,
}
impl Disposition {
    pub fn message(self) -> &'static str {
        match self {
            Self::Created => "Local analytics initialized.",
            Self::Existing => "Using existing local analytics.",
        }
    }
}

/// Ownership must be transferred directly to the writer, without reacquiring it.
pub struct Initialized {
    pub binding: Binding,
    pub disposition: Disposition,
    pub owner: Owner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    UnsupportedState,
    InconsistentState,
    WriterBusy,
    BindingMismatch,
    Io,
    RandomnessUnavailable,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Preflight,
    Identity,
    Index,
    Store,
    Ownership,
    Complete,
}
#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub stage: Stage,
}
impl Error {
    pub(crate) fn new(kind: ErrorKind, stage: Stage) -> Self {
        Self { kind, stage }
    }
    pub(crate) fn store(error: semon_asi::Error, stage: Stage) -> Self {
        let kind = match error {
            semon_asi::Error::WriterBusy => ErrorKind::WriterBusy,
            semon_asi::Error::Io(_) => ErrorKind::Io,
            semon_asi::Error::Invalid(ref message)
                if message.starts_with("analytics schema version") =>
            {
                ErrorKind::UnsupportedState
            }
            semon_asi::Error::Invalid(ref message)
                if message == "store is not owned by the local namespace" =>
            {
                ErrorKind::BindingMismatch
            }
            _ => ErrorKind::InconsistentState,
        };
        Self::new(kind, stage)
    }
    pub fn guidance(&self) -> &'static str {
        match self.kind {
            ErrorKind::WriterBusy => {
                "Another process is writing local analytics. Wait for it to finish, or stop the Semon analytics writer you started, then retry."
            }
            ErrorKind::UnsupportedState | ErrorKind::InconsistentState => {
                "Local analytics state is unsupported or inconsistent; existing data was preserved. See the analytics export/reset recovery guidance (docs/design/asi-store.md#generations-publication-and-versions) before retrying. Unsupported or ambiguous state needs the matching older binary for custody export first. Do not delete the state directory."
            }
            ErrorKind::BindingMismatch => {
                "Local analytics do not match the requested enrollment; existing data was preserved. Re-run connect against the matching setup, or see the analytics export/reset recovery guidance (docs/design/asi-store.md#generations-publication-and-versions) when ownership is unclear."
            }
            ErrorKind::Io => {
                "Local analytics setup could not complete; nothing was deleted. Check the state directory's permissions and free space, then retry."
            }
            ErrorKind::RandomnessUnavailable => {
                "Local analytics setup could not complete; nothing was deleted. Check system randomness, then retry."
            }
            ErrorKind::Cancelled => {
                "Local analytics setup was stopped before completion; no sharing was granted and existing data was preserved. Retry connect when ready."
            }
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.guidance())
    }
}
impl std::error::Error for Error {}

/// Explicit local producer intent. `index` is the native index path, not its cache stem.
/// Reads and qualifies existing bytes before lock creation, then revalidates under
/// ownership. Cancellation never grants sharing or publishes a snapshot.
pub fn initialize(
    index: &Path,
    expected: Option<&Binding>,
    cancelled: impl Fn() -> bool,
) -> Result<Initialized, Error> {
    let state = index
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| Error::new(ErrorKind::InconsistentState, Stage::Preflight))?;
    let root = state.join("analytics-store");
    check_cancelled(&cancelled, Stage::Preflight)?;
    qualify(index, state, expected)?;
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(state)
        .map_err(|_| Error::new(ErrorKind::Io, Stage::Ownership))?;
    let owner =
        Owner::acquire_qualified(&root).map_err(|error| Error::store(error, Stage::Ownership))?;
    check_cancelled(&cancelled, Stage::Ownership)?;
    let (identity, incarnation) = qualify(index, state, expected)?;
    let created = identity.is_none() || incarnation.is_none() || !root.join("analytics").exists();
    // Install identity first: a crash before cold index creation leaves an
    // explicit partial binding that the next admission refuses.
    let identity = match identity {
        Some(identity) => identity,
        None => analytics_namespace::create(state)?,
    };
    // The native index owns cold schema/incarnation creation.
    let incarnation = match incarnation {
        Some(incarnation) => incarnation,
        None => store::initialize_analytics_index(index)
            .map_err(|_| Error::new(ErrorKind::Io, Stage::Index))?,
    };
    check_cancelled(&cancelled, Stage::Index)?;
    fs::create_dir_all(root.join("analytics"))
        .map_err(|_| Error::new(ErrorKind::Io, Stage::Store))?;
    check_cancelled(&cancelled, Stage::Complete)?;
    Ok(Initialized {
        binding: identity.binding(incarnation),
        disposition: if created {
            Disposition::Created
        } else {
            Disposition::Existing
        },
        owner,
    })
}

fn check_cancelled(cancelled: &impl Fn() -> bool, stage: Stage) -> Result<(), Error> {
    if cancelled() {
        Err(Error::new(ErrorKind::Cancelled, stage))
    } else {
        Ok(())
    }
}

fn qualify(
    index: &Path,
    state: &Path,
    expected: Option<&Binding>,
) -> Result<(Option<analytics_namespace::Identity>, Option<String>), Error> {
    safe_ancestors(state)?;
    let root = state.join("analytics-store");
    directory(&root)?;
    directory(&root.join("analytics"))?;
    regular(&root.join("analytics.lock"))?;
    regular(&root.join("analytics/manifest.json"))?;
    regular(index)?;
    // A leftover namespace write is unproven, even if the index is current.
    if state.exists() {
        for entry in fs::read_dir(state).map_err(|_| Error::new(ErrorKind::Io, Stage::Preflight))? {
            let entry = entry.map_err(|_| Error::new(ErrorKind::Io, Stage::Preflight))?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".analytics-namespace-")
            {
                return Err(Error::new(ErrorKind::InconsistentState, Stage::Identity));
            }
        }
    }
    let identity = analytics_namespace::read_identity(state)?;
    let incarnation = if index.exists() {
        Some(store::analytics_index_incarnation(index)?)
    } else {
        None
    };
    if identity.is_some() && incarnation.is_none() {
        return Err(Error::new(ErrorKind::InconsistentState, Stage::Index));
    }
    if let Some(expected) = expected {
        let actual = identity
            .as_ref()
            .zip(incarnation.as_ref())
            .map(|(identity, incarnation)| identity.binding(incarnation.clone()));
        if actual.as_ref() != Some(expected) {
            return Err(Error::new(ErrorKind::BindingMismatch, Stage::Preflight));
        }
    }
    semon_asi::qualify_local_store(
        &root,
        identity
            .as_ref()
            .map(|identity| identity.namespace.as_str()),
    )
    .map_err(|error| Error::store(error, Stage::Store))?;
    Ok((identity, incarnation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "semon-initialize-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn failure(index: &Path, expected: Option<&Binding>) -> Error {
        initialize(index, expected, || false)
            .err()
            .expect("must refuse")
    }
    #[test]
    fn fresh_repeat_and_continuous_writer_handoff_without_publication() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        let first = initialize(&index, None, || false).unwrap();
        assert_eq!(first.disposition, Disposition::Created);
        let binding = first.binding.clone();
        assert!(
            !root
                .join("analytics-store/analytics/manifest.json")
                .exists()
        );
        assert_eq!(failure(&index, None).kind, ErrorKind::WriterBusy);
        let config = crate::AnalyticsWriterConfig::new(
            &binding.namespace,
            &index,
            root.join("analytics-store"),
        );
        let writer = first.owner.start(config).unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::WriterBusy);
        writer.stop_and_join();
        let second = initialize(&index, Some(&binding), || false).unwrap();
        assert_eq!(second.binding, binding);
        assert_eq!(second.disposition, Disposition::Existing);
        assert!(
            !root
                .join("analytics-store/analytics/manifest.json")
                .exists()
        );
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn adopts_existing_index_and_refuses_expected_binding_before_mutation() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        let index = root.join("sessions-index.sqlite3");
        let incarnation = store::initialize_analytics_index(&index).unwrap();
        let before = fs::read(&index).unwrap();
        let expected = Binding {
            namespace: "wrong".into(),
            origin_incarnation: "wrong".into(),
            index_incarnation: incarnation.clone(),
        };
        assert_eq!(
            failure(&index, Some(&expected)).kind,
            ErrorKind::BindingMismatch
        );
        assert_eq!(fs::read(&index).unwrap(), before);
        assert!(!root.join("analytics-store").exists());
        let initialized = initialize(&index, None, || false).unwrap();
        assert_eq!(initialized.binding.index_incarnation, incarnation);
        drop(initialized);
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn custom_state_is_private_and_existing_directory_mode_is_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let parent = root();
        fs::create_dir_all(&parent).unwrap();
        // Explicit parent mode avoids changing the process-wide umask while
        // other tests create files. A 0700 creation mode cannot gain group/world
        // permissions under umask 022 (or any stricter ambient mask).
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        let state = parent.join("custom-state");
        let initialized = initialize(&state.join("index.sqlite3"), None, || false).unwrap();
        assert_eq!(
            fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        drop(initialized);
        let existing = parent.join("existing-state");
        fs::create_dir(&existing).unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();
        let initialized = initialize(&existing.join("index.sqlite3"), None, || false).unwrap();
        assert_eq!(
            fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
            0o755
        );
        drop(initialized);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn publication_bytes_and_identity_survive_repeat_initialization() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        let initialized = initialize(&index, None, || false).unwrap();
        let binding = initialized.binding.clone();
        let mut config = crate::AnalyticsWriterConfig::new(
            &binding.namespace,
            &index,
            root.join("analytics-store"),
        );
        config.interval = std::time::Duration::ZERO;
        let writer = initialized.owner.start(config).unwrap();
        assert_eq!(
            writer.publish_and_wait(),
            crate::AnalyticsWriterStatus::Idle
        );
        writer.stop_and_join();
        let manifest = root.join("analytics-store/analytics/manifest.json");
        let before = fs::read(&manifest).unwrap();
        let initialized = initialize(&index, None, || false).unwrap();
        assert_eq!(initialized.binding, binding);
        assert_eq!(fs::read(&manifest).unwrap(), before);
        drop(initialized);
        // Producer identity stays stable when the native generation changes.
        let db = rusqlite::Connection::open(&index).unwrap();
        db.execute(
            "UPDATE meta SET value=?1 WHERE key='incarnation'",
            ["b".repeat(32)],
        )
        .unwrap();
        drop(db);
        let rebuilt = initialize(&index, None, || false).unwrap();
        assert_eq!(
            rebuilt.binding.origin_incarnation,
            binding.origin_incarnation
        );
        assert_ne!(rebuilt.binding.index_incarnation, binding.index_incarnation);
        drop(rebuilt);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn partial_old_corrupt_and_cancelled_states_are_preserved() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        let error = initialize(&index, None, || true).err().unwrap();
        assert_eq!(error.kind, ErrorKind::Cancelled);
        assert!(!root.exists());
        fs::create_dir_all(&root).unwrap();
        let identity = root.join("analytics-namespace");
        fs::write(&identity, "origin-old\n").unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::UnsupportedState);
        assert_eq!(fs::read(&identity).unwrap(), b"origin-old\n");
        fs::write(&identity, b"partial").unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::InconsistentState);
        assert_eq!(fs::read(&identity).unwrap(), b"partial");
        fs::remove_file(&identity).unwrap();
        analytics_namespace::create(&root).unwrap();
        let before = fs::read(&identity).unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::InconsistentState);
        assert_eq!(fs::read(&identity).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn simultaneous_initializers_exclude_each_other() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        std::thread::scope(|scope| {
            let (ready, wait) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel();
            let index_ref = &index;
            scope.spawn(move || {
                let initialized = initialize(index_ref, None, || false).unwrap();
                ready.send(()).unwrap();
                released.recv().unwrap();
                drop(initialized);
            });
            wait.recv().unwrap();
            assert_eq!(failure(&index, None).kind, ErrorKind::WriterBusy);
            release.send(()).unwrap();
        });
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn reset_retains_index_and_ordinary_identity_but_new_namespace_rebinds() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        let initialized = initialize(&index, None, || false).unwrap();
        let binding = initialized.binding.clone();
        let mut config = crate::AnalyticsWriterConfig::new(
            &binding.namespace,
            &index,
            root.join("analytics-store"),
        );
        config.interval = std::time::Duration::ZERO;
        let writer = initialized.owner.start(config).unwrap();
        assert_eq!(
            writer.publish_and_wait(),
            crate::AnalyticsWriterStatus::Idle
        );
        // Reset and another initializer both use this same writer lock.
        assert!(
            semon_asi::reset_snapshot(
                &root.join("analytics-store"),
                &root.join("analytics-namespace"),
                &binding.namespace,
                None,
                1,
                None
            )
            .is_err()
        );
        writer.stop_and_join();
        semon_asi::reset_snapshot(
            &root.join("analytics-store"),
            &root.join("analytics-namespace"),
            &binding.namespace,
            None,
            1,
            None,
        )
        .unwrap();
        let retained = initialize(&index, Some(&binding), || false).unwrap();
        assert_eq!(retained.binding, binding);
        assert!(
            !root
                .join("analytics-store/analytics/manifest.json")
                .exists()
        );
        drop(retained);
        // Re-publish so the existing explicit reset API can qualify custody.
        let initialized = initialize(&index, None, || false).unwrap();
        let mut config = crate::AnalyticsWriterConfig::new(
            &binding.namespace,
            &index,
            root.join("analytics-store"),
        );
        config.interval = std::time::Duration::ZERO;
        let writer = initialized.owner.start(config).unwrap();
        assert_eq!(
            writer.publish_and_wait(),
            crate::AnalyticsWriterStatus::Idle
        );
        writer.stop_and_join();
        let handles = root.join(semon_asi::egress::HANDLES_FILE);
        semon_asi::reset_snapshot(
            &root.join("analytics-store"),
            &root.join("analytics-namespace"),
            &binding.namespace,
            None,
            1,
            Some(&handles),
        )
        .unwrap();
        assert_eq!(
            failure(&index, Some(&binding)).kind,
            ErrorKind::BindingMismatch
        );
        let fresh = initialize(&index, None, || false).unwrap();
        assert_ne!(fresh.binding.namespace, binding.namespace);
        assert_ne!(fresh.binding.origin_incarnation, binding.origin_incarnation);
        assert_eq!(fresh.binding.index_incarnation, binding.index_incarnation);
        drop(fresh);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_origin_mismatch_refuses_without_recovery() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        let initialized = initialize(&index, None, || false).unwrap();
        let mut config = crate::AnalyticsWriterConfig::new(
            &initialized.binding.namespace,
            &index,
            root.join("analytics-store"),
        );
        config.interval = std::time::Duration::ZERO;
        let writer = initialized.owner.start(config).unwrap();
        assert_eq!(
            writer.publish_and_wait(),
            crate::AnalyticsWriterStatus::Idle
        );
        writer.stop_and_join();
        let path = root.join("analytics-namespace");
        let mut identity: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        identity["origin_incarnation"] =
            serde_json::Value::String(format!("producer-{}", "f".repeat(64)));
        let bytes = serde_json::to_vec(&identity).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::BindingMismatch);
        assert_eq!(fs::read(path).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsupported_manifest_and_orphan_generation_preserve_bytes() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        let initialized = initialize(&index, None, || false).unwrap();
        let binding = initialized.binding.clone();
        drop(initialized);
        let analytics = root.join("analytics-store/analytics");
        let orphan = analytics.join("manifest.json.tmp");
        fs::write(&orphan, b"unproven generation").unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::InconsistentState);
        assert_eq!(fs::read(&orphan).unwrap(), b"unproven generation");
        fs::remove_file(&orphan).unwrap();
        let mut manifest = semon_asi::Manifest::empty();
        manifest.schema_version += 1;
        manifest
            .origin_incarnations
            .insert(binding.namespace, binding.origin_incarnation);
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let path = analytics.join("manifest.json");
        fs::write(&path, &bytes).unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::UnsupportedState);
        assert_eq!(fs::read(path).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_namespace_and_foreign_index_are_preserved() {
        let root = root();
        let index = root.join("sessions-index.sqlite3");
        let initialized = initialize(&index, None, || false).unwrap();
        drop(initialized);
        let identity = root.join("analytics-namespace");
        let target = root.join("identity-target");
        fs::rename(&identity, &target).unwrap();
        let before = fs::read(&target).unwrap();
        std::os::unix::fs::symlink(&target, &identity).unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::InconsistentState);
        assert_eq!(fs::read(&target).unwrap(), before);
        fs::remove_file(&identity).unwrap();
        fs::rename(&target, &identity).unwrap();
        let db = rusqlite::Connection::open(&index).unwrap();
        db.execute(
            "INSERT INTO meta(key,value) VALUES('storage_namespace','foreign')",
            [],
        )
        .unwrap();
        drop(db);
        let before = fs::read(&index).unwrap();
        assert_eq!(failure(&index, None).kind, ErrorKind::BindingMismatch);
        assert_eq!(fs::read(index).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn randomness_and_io_refusals_preserve_existing_bytes() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        let sentinel = root.join("sentinel");
        fs::write(&sentinel, b"retained").unwrap();
        let index = root.join("sessions-index.sqlite3");
        analytics_namespace::REFUSE_RANDOMNESS.with(|refuse| refuse.set(true));
        let error = failure(&index, None);
        analytics_namespace::REFUSE_RANDOMNESS.with(|refuse| refuse.set(false));
        assert_eq!(error.kind, ErrorKind::RandomnessUnavailable);
        assert_eq!(fs::read(&sentinel).unwrap(), b"retained");
        assert!(!root.join("analytics-namespace").exists());
        assert!(!index.exists());
        fs::remove_dir_all(&root).unwrap();
        // An overlong path deterministically returns I/O failure without permissions
        // or timing assumptions, including when tests run as an administrator.
        let long = root.join("x".repeat(4096)).join("index.sqlite3");
        assert_eq!(failure(&long, None).kind, ErrorKind::Io);
        assert!(!root.exists());
    }

    #[test]
    fn every_refusal_has_bounded_actionable_guidance() {
        for kind in [
            ErrorKind::UnsupportedState,
            ErrorKind::InconsistentState,
            ErrorKind::WriterBusy,
            ErrorKind::BindingMismatch,
            ErrorKind::Io,
            ErrorKind::RandomnessUnavailable,
            ErrorKind::Cancelled,
        ] {
            let error = Error::new(kind, Stage::Preflight);
            assert!(
                error.guidance().contains("retry")
                    || error.guidance().contains("Retry")
                    || error.guidance().contains("connect")
            );
            assert!(!error.guidance().contains("/home/"));
            assert!(error.guidance().len() < 600);
        }
    }
}

fn safe_ancestors(path: &Path) -> Result<(), Error> {
    for ancestor in path.ancestors() {
        directory(ancestor)?;
    }
    Ok(())
}
fn directory(path: &Path) -> Result<(), Error> {
    shape(path, true)
}
fn regular(path: &Path) -> Result<(), Error> {
    shape(path, false)
}
fn shape(path: &Path, directory: bool) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if (directory && metadata.is_dir()) || (!directory && metadata.is_file()) => {
            Ok(())
        }
        Ok(_) => Err(Error::new(ErrorKind::InconsistentState, Stage::Preflight)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Error::new(ErrorKind::Io, Stage::Preflight)),
    }
}
