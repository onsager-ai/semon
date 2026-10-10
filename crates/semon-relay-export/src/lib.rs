//! Complete byte custody for explicitly selected historical Relay inputs.
//! No receiver, transport, enrollment, decryption, restoration or deletion runs.
use std::{
    collections::BTreeMap,
    error::Error,
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub const WARNING: &str = "private export may contain age/signing keys, credentials, \
    ciphertext, plaintext session evidence, machine paths and pending deletion state.";
pub const LEGACY_SOURCE: &str = "0648a99f977971fb177a5dda7a445c4941d64b57";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Receiver,
    Config,
    Sender,
    RecoveryKey,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Receiver => "receiver",
            Self::Config => "config",
            Self::Sender => "sender",
            Self::RecoveryKey => "recovery-key",
        }
    }
}

pub struct Input {
    pub kind: Kind,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    directory: bool,
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    generation: (u64, u64, i64, i64, u32),
}

fn stamp(metadata: &Metadata) -> Result<Stamp> {
    if !metadata.is_file() && !metadata.is_dir() {
        return Err("custody inputs must contain only real directories and regular files".into());
    }
    Ok(Stamp {
        directory: metadata.is_dir(),
        length: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        generation: (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.mode(),
        ),
    })
}

fn inventory(root: &Path) -> Result<BTreeMap<PathBuf, Stamp>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let path = under(root, &relative);
        let entry = stamp(&fs::symlink_metadata(&path)?)?;
        if entry.directory {
            for child in fs::read_dir(&path)? {
                pending.push(relative.join(child?.file_name()));
            }
        }
        entries.insert(relative, entry);
    }
    Ok(entries)
}

fn under(root: &Path, relative: &Path) -> PathBuf {
    if relative.as_os_str().is_empty() {
        root.to_owned()
    } else {
        root.join(relative)
    }
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

fn source_file(path: &Path, expected: &Stamp) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let input = options.open(path)?;
    if stamp(&input.metadata()?)? != *expected || expected.directory {
        return Err("source file generation changed; pause writers and retry".into());
    }
    Ok(input)
}

fn hash(input: &mut File, mut output: Option<&mut File>) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut bytes = [0_u8; 64 * 1024];
    loop {
        let length = input.read(&mut bytes)?;
        if length == 0 {
            break;
        }
        digest.update(&bytes[..length]);
        if let Some(output) = &mut output {
            output.write_all(&bytes[..length])?;
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

struct Source {
    kind: Kind,
    root: PathBuf,
    target: PathBuf,
    id: String,
    before: BTreeMap<PathBuf, Stamp>,
    hashes: BTreeMap<PathBuf, String>,
}

/// Publish a checksummed custody artifact, not a decryption or recovery claim.
/// Operators must quiesce writers. Original file handles are read-only; source
/// inventory/generation/hash changes reject publication. No existing target is
/// reused; failures retain private output. A failed command is never a verified
/// export, even if publication reached the final directory durability checks.
pub fn export(inputs: &[Input], destination: &Path) -> Result<()> {
    export_with_hook(inputs, destination, || {})
}

/// Read only a complete version-1 custody artifact, checking every declared
/// byte and the exact tree before and after the callback. The callback receives
/// paths inside the artifact, never the historical `source` paths in its JSON.
/// Keep the artifact immutable; write derived output outside it.
pub fn with_verified_inputs<T>(root: &Path, read: impl FnOnce(&[Input]) -> Result<T>) -> Result<T> {
    let supplied: PathBuf = root.components().collect();
    let supplied_stamp = stamp(&fs::symlink_metadata(&supplied)?)?;
    if !supplied_stamp.directory {
        return Err("custody artifact must be a real directory".into());
    }
    let root = fs::canonicalize(supplied)?;
    let before = inventory(&root)?;
    if before.get(Path::new("")) != Some(&supplied_stamp) {
        return Err("custody artifact generation changed during resolution".into());
    }
    let manifest_path = Path::new("manifest.json");
    let manifest_stamp = before
        .get(manifest_path)
        .ok_or("custody manifest is missing")?;
    if manifest_stamp.directory || manifest_stamp.length > 64 * 1024 * 1024 {
        return Err("custody manifest is not a supported regular file (maximum 64 MiB)".into());
    }
    let mut manifest_file = source_file(&root.join(manifest_path), manifest_stamp)?;
    let mut bytes = Vec::new();
    manifest_file.read_to_end(&mut bytes)?;
    let manifest: Value = serde_json::from_slice(&bytes)?;
    if manifest["format"] != "semon.relay-custody-export"
        || manifest["version"] != 1
        || manifest["legacy_source"] != LEGACY_SOURCE
        || manifest["decryption_verified"] != false
    {
        return Err(
            "unsupported custody artifact format/version/source or decryption claim".into(),
        );
    }
    let declarations = manifest["inputs"]
        .as_array()
        .ok_or("custody inputs are missing")?;
    if declarations.is_empty() {
        return Err("custody artifact declares no inputs".into());
    }
    let mut expected = BTreeMap::from([
        (PathBuf::new(), (true, None, None)),
        (PathBuf::from("inputs"), (true, None, None)),
        (
            manifest_path.to_owned(),
            (
                false,
                Some(manifest_stamp.length),
                Some(format!("{:x}", Sha256::digest(&bytes))),
            ),
        ),
    ]);
    let mut inputs = Vec::new();
    for (index, declaration) in declarations.iter().enumerate() {
        let kind = match declaration["kind"].as_str() {
            Some("receiver") => Kind::Receiver,
            Some("config") => Kind::Config,
            Some("sender") => Kind::Sender,
            Some("recovery-key") => Kind::RecoveryKey,
            _ => return Err("unsupported custody input role".into()),
        };
        let id = format!("{}-{index}", kind.name());
        let artifact = format!("inputs/{id}");
        if declaration["id"] != id || declaration["artifact"] != artifact {
            return Err("custody input identity/path does not match its declared role".into());
        }
        let base = PathBuf::from(artifact);
        let entries = declaration["entries"]
            .as_array()
            .ok_or("custody entries missing")?;
        for entry in entries {
            let relative = entry["path"].as_str().ok_or("custody entry path missing")?;
            let relative = Path::new(relative);
            if relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err("unsafe custody entry path".into());
            }
            let directory = match entry["kind"].as_str() {
                Some("directory") => true,
                Some("file") => false,
                _ => return Err("unsupported custody entry type".into()),
            };
            let (length, digest) = if directory {
                if !entry["bytes"].is_null() || !entry["sha256"].is_null() {
                    return Err("directory has unsupported custody byte/hash fields".into());
                }
                (None, None)
            } else {
                (
                    Some(entry["bytes"].as_u64().ok_or("custody length missing")?),
                    Some(
                        entry["sha256"]
                            .as_str()
                            .ok_or("custody digest missing")?
                            .to_owned(),
                    ),
                )
            };
            if expected
                .insert(under(&base, relative), (directory, length, digest))
                .is_some()
            {
                return Err("duplicate custody path declaration".into());
            }
        }
        if expected.get(&base).map(|entry| entry.0) != Some(kind != Kind::RecoveryKey) {
            return Err("custody input root has the wrong declared type".into());
        }
        inputs.push(Input {
            kind,
            path: root.join(base),
        });
    }
    let verify = || -> Result<()> {
        if inventory(&root)? != before || expected.len() != before.len() {
            return Err("custody tree changed or contains undeclared/missing entries".into());
        }
        for (relative, (directory, length, digest)) in &expected {
            let actual = before
                .get(relative)
                .ok_or("declared custody entry missing")?;
            if actual.directory != *directory
                || length.is_some_and(|length| length != actual.length)
            {
                return Err("custody entry type/length mismatch".into());
            }
            if let Some(digest) = digest {
                let mut file = source_file(&root.join(relative), actual)?;
                if hash(&mut file, None)? != *digest {
                    return Err("custody entry checksum mismatch".into());
                }
            }
        }
        if inventory(&root)? != before {
            return Err("custody generation changed during verification".into());
        }
        Ok(())
    };
    verify()?;
    let result = read(&inputs);
    verify()?;
    result
}

fn export_with_hook(
    inputs: &[Input],
    destination: &Path,
    before_verify: impl FnOnce(),
) -> Result<()> {
    if inputs.is_empty() {
        return Err("at least one explicit custody input is required".into());
    }
    let name = destination
        .file_name()
        .ok_or("destination needs a new directory name")?;
    let parent = fs::canonicalize(
        destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    if !parent.is_dir() {
        return Err("destination parent must be an existing directory".into());
    }
    let destination = parent.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err("destination already exists; choose a new directory".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut sources = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let supplied: PathBuf = input.path.components().collect();
        let original = stamp(&fs::symlink_metadata(&supplied)?)?;
        if original.directory == (input.kind == Kind::RecoveryKey) {
            return Err(
                "recovery-key inputs must be files; other custody inputs must be directories"
                    .into(),
            );
        }
        let root = fs::canonicalize(&supplied)?;
        root.to_str()
            .ok_or("non-UTF-8 custody input roots are unsupported; originals remain untouched")?;
        if original != stamp(&fs::symlink_metadata(&root)?)? {
            return Err("source generation changed during resolution".into());
        }
        if destination.starts_with(&root) {
            return Err("destination must be outside every input tree".into());
        }
        let id = format!("{}-{index}", input.kind.name());
        let target = destination.join("inputs").join(&id);
        sources.push(Source {
            kind: input.kind,
            before: inventory(&root)?,
            root,
            target,
            id,
            hashes: BTreeMap::new(),
        });
    }
    private_directory(&destination)?;
    private_directory(&destination.join("inputs"))?;
    for source in &mut sources {
        for (relative, entry) in &source.before {
            let target = under(&source.target, relative);
            if entry.directory {
                private_directory(&target)?;
            } else {
                let mut input = source_file(&under(&source.root, relative), entry)?;
                let mut output = private_file(&target)?;
                let digest = hash(&mut input, Some(&mut output))?;
                output.sync_all()?;
                // Re-read the actual destination, not just the write buffer.
                if hash(&mut File::open(&target)?, None)? != digest
                    || output.metadata()?.len() != entry.length
                    || stamp(&input.metadata()?)? != *entry
                {
                    return Err("copied bytes or source generation changed".into());
                }
                source.hashes.insert(relative.clone(), digest);
            }
        }
    }
    before_verify();
    for source in &sources {
        if inventory(&source.root)? != source.before {
            return Err("source tree changed; pause writers and retry to a new destination".into());
        }
        for (relative, digest) in &source.hashes {
            let mut input = source_file(&under(&source.root, relative), &source.before[relative])?;
            if hash(&mut input, None)? != *digest {
                return Err("source bytes changed; pause writers and retry".into());
            }
        }
    }
    // Fence changes during the complete verification pass, including additions,
    // removals, substitutions and directory/permission changes.
    for source in &sources {
        if inventory(&source.root)? != source.before {
            return Err("source generation changed during verification".into());
        }
    }
    let mut declarations = Vec::new();
    for source in &sources {
        let mut entries = Vec::new();
        for (relative, entry) in &source.before {
            let path = relative
                .to_str()
                .ok_or("non-UTF-8 custody paths are unsupported; originals remain untouched")?;
            entries.push(json!({
                "path": path,
                "kind": if entry.directory { "directory" } else { "file" },
                "bytes": if entry.directory { Value::Null } else { json!(entry.length) },
                "sha256": source.hashes.get(relative),
            }));
        }
        // Persist directory entries as well as file bytes before the completion
        // marker; originals' modes never change.
        for (relative, entry) in source.before.iter().rev() {
            if entry.directory {
                File::open(source.target.join(relative))?.sync_all()?;
            }
        }
        declarations.push(json!({
            "id": source.id,
            "kind": source.kind.name(),
            "source": source.root,
            "artifact": format!("inputs/{}", source.id),
            "entries": entries,
        }));
    }
    File::open(destination.join("inputs"))?.sync_all()?;
    let manifest = json!({
        "format": "semon.relay-custody-export",
        "version": 1,
        "scope": "all files and directories in the explicitly declared inputs",
        "may_include_private_keys": true,
        "decryption_verified": false,
        "legacy_source": LEGACY_SOURCE,
        "inputs": declarations,
    });
    let partial = destination.join("manifest.json.partial");
    let mut output = private_file(&partial)?;
    serde_json::to_writer_pretty(&mut output, &manifest)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    fs::rename(partial, destination.join("manifest.json"))?;
    File::open(&destination)?.sync_all()?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "semon-relay-custody-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            private_directory(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn verified_artifact_reads_never_use_source_paths_and_reject_tampering_or_mutation() {
        let root = Scratch::new();
        let source = root.0.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("record"), b"private evidence").unwrap();
        let out = root.0.join("artifact");
        export(
            &[Input {
                kind: Kind::Config,
                path: source.clone(),
            }],
            &out,
        )
        .unwrap();
        let baseline = inventory(&out).unwrap();
        with_verified_inputs(&out, |inputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].path, out.join("inputs/config-0"));
            assert_ne!(inputs[0].path, source);
            assert_eq!(
                fs::read(inputs[0].path.join("record"))?,
                b"private evidence"
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(baseline, inventory(&out).unwrap());
        assert!(
            with_verified_inputs(&out, |inputs| {
                fs::write(inputs[0].path.join("record"), b"changed evidence")?;
                Ok(())
            })
            .is_err()
        );
        let mut invoked = false;
        assert!(
            with_verified_inputs(&out, |_| {
                invoked = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!invoked);

        for change in ["path", "duplicate", "role", "version", "extra", "missing"] {
            let artifact = root.0.join(change);
            export(
                &[Input {
                    kind: Kind::Config,
                    path: source.clone(),
                }],
                &artifact,
            )
            .unwrap();
            let manifest_path = artifact.join("manifest.json");
            let mut manifest: Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
            match change {
                "path" => {
                    manifest["inputs"][0]["entries"][1]["path"] = json!("../../source/record")
                }
                "duplicate" => {
                    let duplicate = manifest["inputs"][0]["entries"][1].clone();
                    manifest["inputs"][0]["entries"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                "role" => manifest["inputs"][0]["artifact"] = json!(source),
                "version" => manifest["version"] = json!(999),
                "extra" => fs::write(artifact.join("undeclared"), b"extra").unwrap(),
                "missing" => fs::remove_file(artifact.join("inputs/config-0/record")).unwrap(),
                _ => unreachable!(),
            }
            fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            let mut invoked = false;
            assert!(
                with_verified_inputs(&artifact, |_| {
                    invoked = true;
                    Ok(())
                })
                .is_err(),
                "{change}"
            );
            assert!(!invoked, "{change}");
        }
        assert_eq!(
            fs::read(source.join("record")).unwrap(),
            b"private evidence"
        );
    }

    #[test]
    fn preserves_every_declared_input_including_retained_and_unknown_files() {
        let root = Scratch::new();
        let receiver = root.0.join("receiver");
        let config = root.0.join("config");
        let sender = root.0.join("sender");
        for directory in [&receiver, &config, &sender] {
            fs::create_dir(directory).unwrap();
        }
        for name in [
            "sessions/generation-0/epoch-0/frames/00000.json",
            "sessions/generation-1/epoch-1/frames/00001.deleted",
            "orphans/session/stream/epoch-0/generation-2/frames/00000.json",
            "envelopes/session.age",
            "snapshots/root/manifests/first",
            "snapshots/root/manifests/fork",
            "snapshots/root/blobs/body",
            "unknown/future-version.bin",
        ] {
            let path = receiver.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name.as_bytes()).unwrap();
        }
        fs::create_dir(receiver.join("empty-retained-directory")).unwrap();
        fs::write(
            config.join("identity.age"),
            b"synthetic-private-key\x00\xff",
        )
        .unwrap();
        fs::create_dir(config.join("snapshot-outbox")).unwrap();
        fs::write(
            config.join("snapshot-outbox/pending.json"),
            "synthetic pending",
        )
        .unwrap();
        fs::write(sender.join("relay.json"), "{\"version\":3}").unwrap();
        fs::create_dir(sender.join("relay.json.forget")).unwrap();
        fs::write(sender.join("relay.json.forget/pending.json"), "pending").unwrap();
        let recovery = root.0.join("recovery.age");
        fs::write(&recovery, b"synthetic historical recovery key").unwrap();
        let out = root.0.join("export");
        let inputs = vec![
            Input {
                kind: Kind::Receiver,
                path: receiver.clone(),
            },
            Input {
                kind: Kind::Config,
                path: config,
            },
            Input {
                kind: Kind::Sender,
                path: sender,
            },
            Input {
                kind: Kind::RecoveryKey,
                path: recovery,
            },
        ];
        let before: Vec<_> = inputs
            .iter()
            .map(|input| inventory(&input.path).unwrap())
            .collect();
        export(&inputs, &out).unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["format"], "semon.relay-custody-export");
        assert_eq!(manifest["version"], 1);
        assert_eq!(manifest["decryption_verified"], false);
        assert_eq!(manifest["inputs"].as_array().unwrap().len(), 4);
        for (index, declaration) in manifest["inputs"].as_array().unwrap().iter().enumerate() {
            let copied = out.join(declaration["artifact"].as_str().unwrap());
            assert_eq!(inventory(&inputs[index].path).unwrap(), before[index]);
            assert_eq!(inventory(&copied).unwrap().len(), before[index].len());
            for (relative, entry) in &before[index] {
                if !entry.directory {
                    assert_eq!(
                        fs::read(under(&inputs[index].path, relative)).unwrap(),
                        fs::read(under(&copied, relative)).unwrap()
                    );
                }
            }
        }
        assert!(
            out.join("inputs/receiver-0/empty-retained-directory")
                .is_dir()
        );
        assert_eq!(
            fs::read(out.join("inputs/recovery-key-3")).unwrap(),
            b"synthetic historical recovery key"
        );
    }

    #[test]
    fn mutation_replacement_membership_and_permission_changes_refuse_completion() {
        for action in ["rewrite", "replace", "add", "remove", "permissions"] {
            let root = Scratch::new();
            let source = root.0.join("input");
            fs::create_dir(&source).unwrap();
            let file = source.join("frame.json");
            fs::write(&file, b"old generation").unwrap();
            let out = root.0.join("export");
            let result = export_with_hook(
                &[Input {
                    kind: Kind::Receiver,
                    path: source.clone(),
                }],
                &out,
                || match action {
                    "rewrite" => fs::write(&file, b"new generation").unwrap(),
                    "replace" => {
                        fs::rename(&file, source.join("old.json")).unwrap();
                        fs::write(&file, b"old generation").unwrap();
                    }
                    "add" => fs::write(source.join("new.json"), b"new file").unwrap(),
                    "remove" => fs::remove_file(&file).unwrap(),
                    "permissions" => {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
                        }
                        #[cfg(not(unix))]
                        fs::write(&file, b"new generation").unwrap();
                    }
                    _ => unreachable!(),
                },
            );
            assert!(result.is_err(), "{action}");
            assert!(!out.join("manifest.json").exists(), "{action}");
            assert!(source.exists());
            assert!(out.join("inputs/receiver-0/frame.json").exists());
        }
    }

    #[test]
    fn refuses_existing_contained_missing_and_mismatched_inputs() {
        let root = Scratch::new();
        let source = root.0.join("input");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("frame"), "synthetic").unwrap();
        let inputs = [Input {
            kind: Kind::Receiver,
            path: source.clone(),
        }];
        assert!(export(&inputs, &source.join("nested-export")).is_err());
        assert!(!source.join("nested-export").exists());
        let out = root.0.join("existing");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("original"), "unchanged").unwrap();
        assert!(export(&inputs, &out).is_err());
        assert_eq!(
            fs::read_to_string(out.join("original")).unwrap(),
            "unchanged"
        );
        assert!(export(&[], &root.0.join("empty")).is_err());
        assert!(
            export(
                &[Input {
                    kind: Kind::RecoveryKey,
                    path: source
                }],
                &root.0.join("wrong-kind")
            )
            .is_err()
        );
        assert!(
            export(
                &[Input {
                    kind: Kind::Config,
                    path: root.0.join("absent")
                }],
                &root.0.join("missing")
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_special_files_are_refused_without_following_or_blocking() {
        use std::os::unix::fs::symlink;
        let root = Scratch::new();
        let source = root.0.join("input");
        fs::create_dir(&source).unwrap();
        let private = root.0.join("private");
        fs::write(&private, "must not be followed").unwrap();
        symlink(&private, source.join("link")).unwrap();
        let inputs = [Input {
            kind: Kind::Receiver,
            path: source.clone(),
        }];
        let out = root.0.join("export");
        assert!(export(&inputs, &out).is_err());
        assert!(!out.exists());
        fs::remove_file(source.join("link")).unwrap();
        symlink(&source, root.0.join("root-link")).unwrap();
        assert!(
            export(
                &[Input {
                    kind: Kind::Receiver,
                    path: root.0.join("root-link/")
                }],
                &out
            )
            .is_err()
        );
        assert!(!out.exists());
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let fifo = CString::new(source.join("pipe").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(export(&inputs, &out).is_err());
        assert!(!out.exists());
        assert_eq!(fs::read_to_string(private).unwrap(), "must not be followed");
    }

    #[cfg(unix)]
    #[test]
    fn private_outputs_preserve_original_modes_and_non_utf8_paths_fail_explicitly() {
        use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
        let root = Scratch::new();
        let source = root.0.join("input");
        fs::create_dir(&source).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();
        let file = source.join("identity.age");
        fs::write(&file, "synthetic").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        let before = inventory(&source).unwrap();
        let out = root.0.join("export");
        export(
            &[Input {
                kind: Kind::Config,
                path: source.clone(),
            }],
            &out,
        )
        .unwrap();
        assert_eq!(before, inventory(&source).unwrap());
        for path in [&out, &out.join("inputs"), &out.join("inputs/config-0")] {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o700);
        }
        for path in [
            out.join("manifest.json"),
            out.join("inputs/config-0/identity.age"),
        ] {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
        let strange = source.join(std::ffi::OsString::from_vec(vec![0xff]));
        fs::write(&strange, "unknown private bytes").unwrap();
        let failed = root.0.join("unsupported");
        assert!(
            export(
                &[Input {
                    kind: Kind::Config,
                    path: source
                }],
                &failed
            )
            .is_err()
        );
        assert!(!failed.join("manifest.json").exists());
        assert_eq!(
            fs::read_to_string(strange).unwrap(),
            "unknown private bytes"
        );
        let non_utf8_root = root.0.join(std::ffi::OsString::from_vec(vec![0xff]));
        fs::create_dir(&non_utf8_root).unwrap();
        fs::write(non_utf8_root.join("identity.age"), "private root bytes").unwrap();
        let before = inventory(&non_utf8_root).unwrap();
        let target = root.0.join("unsupported-root");
        let error = export(
            &[Input {
                kind: Kind::Config,
                path: non_utf8_root.clone(),
            }],
            &target,
        )
        .unwrap_err();
        assert!(error.to_string().contains("non-UTF-8 custody input roots"));
        assert!(!target.exists());
        assert_eq!(inventory(&non_utf8_root).unwrap(), before);
        assert_eq!(
            fs::read_to_string(non_utf8_root.join("identity.age")).unwrap(),
            "private root bytes"
        );
    }
}
