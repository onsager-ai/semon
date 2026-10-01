//! Encrypted, immutable snapshots for explicitly selected mutable memory roots.
//! Paths and contents live only inside age ciphertext. Public blob references
//! permit reachability-based collection without giving the receiver plaintext.
use crate::{Transport, TransportError};
use age::{Decryptor, Encryptor, x25519};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

const FILE_MAX: usize = 8 * 1024 * 1024;
const ROOT_MAX: usize = 24 * 1024 * 1024;
const ENTRY_MAX: usize = 4096;
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn identifier(value: &str) -> io::Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(invalid("expected a lowercase SHA-256 identifier"))
    }
}
fn field<'a>(value: &'a Value, key: &str) -> io::Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| invalid(format!("missing {key}")))
}
fn safe_relative(value: &str) -> io::Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 4096
        || value.contains('\\')
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("unsafe snapshot relative path"));
    }
    Ok(path.into())
}
fn encrypt(bytes: &[u8], recipients: &[x25519::Recipient]) -> io::Result<String> {
    let encryptor = Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
        .map_err(|e| invalid(e.to_string()))?;
    let mut out = Vec::new();
    let mut writer = encryptor.wrap_output(&mut out)?;
    writer.write_all(bytes)?;
    writer.finish()?;
    Ok(hex::encode(out))
}
fn decrypt(value: &str, identity: &x25519::Identity, max: usize) -> io::Result<Vec<u8>> {
    if value.len() > 2 * (max + 8192) {
        return Err(invalid("snapshot ciphertext exceeds limit"));
    }
    let bytes = hex::decode(value).map_err(|e| invalid(e.to_string()))?;
    let mut reader = Decryptor::new(&bytes[..])
        .map_err(|e| invalid(e.to_string()))?
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| invalid(e.to_string()))?;
    let mut out = Vec::new();
    reader
        .by_ref()
        .take((max + 1) as u64)
        .read_to_end(&mut out)?;
    if out.len() > max {
        return Err(invalid("snapshot plaintext exceeds limit"));
    }
    Ok(out)
}
fn directory(path: &Path) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_dir() {
            return Err(invalid("snapshot directory is not a real directory"));
        }
    } else {
        fs::create_dir(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn read_regular(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid("snapshot input is not a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("snapshot input changed type"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((max + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(invalid("snapshot file exceeds limit"));
    }
    Ok(bytes)
}
fn immutable_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("snapshot target lacks parent"))?;
    let temp = parent.join(format!(".tmp-{}", hex::encode(crate::generate_data_key())));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&temp, path) {
            Ok(()) => File::open(parent)?.sync_all(),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if read_regular(path, 2 * (FILE_MAX + 8192))? != bytes {
                    return Err(invalid("immutable snapshot object conflict"));
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    })();
    let _ = fs::remove_file(&temp);
    result
}
fn bound_open(path: &Path) -> io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    #[cfg(target_os = "macos")]
    let absolute = if let Ok(tail) = absolute.strip_prefix("/var") {
        Path::new("/private/var").join(tail)
    } else if let Ok(tail) = absolute.strip_prefix("/tmp") {
        Path::new("/private/tmp").join(tail)
    } else {
        absolute
    };
    let mut file = File::open("/")?;
    let parts = absolute
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .collect::<Vec<_>>();
    for (i, component) in parts.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(invalid("snapshot root cannot contain parent traversal"));
        };
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(name.as_bytes()).map_err(|_| invalid("invalid snapshot path"))?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if i + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        file = unsafe { File::from_raw_fd(fd) };
    }
    Ok(file)
}
/// Metadata and ciphertext retained for one watched root. No file plaintext
/// survives a scan. Recipient or root changes invalidate every cached entry.
#[derive(Default)]
pub struct SnapshotCache {
    scope: Option<(PathBuf, Vec<String>)>,
    files: BTreeMap<String, CachedFile>,
}
#[derive(Clone)]
struct CachedFile {
    stamp: FileStamp,
    blob: String,
    ciphertext: std::sync::Arc<String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    dev: u64,
    ino: u64,
    len: u64,
    mode: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl FileStamp {
    fn of(meta: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mode: meta.mode(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}
#[cfg(test)]
thread_local! {
    static SOURCE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static SOURCE_ENCRYPTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[derive(Default)]
struct ScanBudget {
    bytes: usize,
    nodes: usize,
}
fn bound_walk(
    file: File,
    relative: &Path,
    entries: &mut BTreeMap<String, CachedFile>,
    total: &mut ScanBudget,
    previous: &BTreeMap<String, CachedFile>,
    recipients: &[x25519::Recipient],
    depth: usize,
) -> io::Result<()> {
    use std::ffi::{CStr, CString};
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    use std::os::unix::ffi::OsStrExt;
    total.nodes += 1;
    if total.nodes > ENTRY_MAX * 2 + 1 {
        return Err(invalid("too many snapshot filesystem nodes"));
    }
    if depth > 64 {
        return Err(invalid("snapshot directory depth exceeds limit"));
    }
    let meta = file.metadata()?;
    let stamp = FileStamp::of(&meta);
    if meta.is_dir() {
        let raw = file.try_clone()?.into_raw_fd();
        let dir = unsafe { libc::fdopendir(raw) };
        if dir.is_null() {
            unsafe { libc::close(raw) };
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut names = Vec::new();
            loop {
                let entry = unsafe { libc::readdir(dir) };
                if entry.is_null() {
                    break;
                }
                let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if name != b"." && name != b".." {
                    if names.len() >= ENTRY_MAX {
                        return Err(invalid("too many snapshot directory children"));
                    }
                    names.push(std::ffi::OsString::from(std::ffi::OsStr::from_bytes(name)));
                }
            }
            names.sort();
            for name in names {
                let c =
                    CString::new(name.as_bytes()).map_err(|_| invalid("invalid snapshot child"))?;
                let fd = unsafe {
                    libc::openat(
                        file.as_raw_fd(),
                        c.as_ptr(),
                        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                    )
                };
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                bound_walk(
                    unsafe { File::from_raw_fd(fd) },
                    &relative.join(name),
                    entries,
                    total,
                    previous,
                    recipients,
                    depth + 1,
                )?;
            }
            Ok(())
        })();
        unsafe { libc::closedir(dir) };
        result?;
        if stamp != FileStamp::of(&file.metadata()?) {
            return Err(invalid("snapshot directory changed during scan; retry"));
        }
        Ok(())
    } else if meta.is_file() {
        let relative = relative
            .to_str()
            .ok_or_else(|| invalid("snapshot paths must be UTF-8"))?
            .to_owned();
        safe_relative(&relative)?;
        if entries.len() >= ENTRY_MAX {
            return Err(invalid("too many snapshot entries"));
        }
        if meta.len() > FILE_MAX as u64 {
            return Err(invalid("snapshot file exceeds limit"));
        }
        total.bytes += meta.len() as usize;
        if total.bytes > ROOT_MAX {
            return Err(invalid("snapshot root exceeds limit"));
        }
        let cached = previous
            .get(&relative)
            .filter(|cached| cached.stamp == stamp);
        let entry = if let Some(cached) = cached {
            cached.clone()
        } else {
            let mut bytes = Vec::new();
            let mut input = &file;
            Read::by_ref(&mut input)
                .take((FILE_MAX + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > FILE_MAX || bytes.len() as u64 != meta.len() {
                return Err(invalid("snapshot file changed size during scan; retry"));
            }
            #[cfg(test)]
            SOURCE_BYTES.with(|count| count.set(count.get() + bytes.len()));
            let blob = hash(&bytes);
            let ciphertext = if let Some(same) = previous
                .values()
                .chain(entries.values())
                .find(|old| old.blob == blob)
            {
                same.ciphertext.clone()
            } else {
                #[cfg(test)]
                SOURCE_ENCRYPTIONS.with(|count| count.set(count.get() + 1));
                std::sync::Arc::new(encrypt(&bytes, recipients)?)
            };
            CachedFile {
                stamp,
                blob,
                ciphertext,
            }
        };
        // Validate all stamp fields even on a cache hit; descriptors bind both
        // the opened inode and its complete before/after metadata generation.
        if stamp != FileStamp::of(&file.metadata()?) {
            return Err(invalid("snapshot file changed during scan; retry"));
        }
        entries.insert(relative, entry);
        Ok(())
    } else {
        Err(invalid("snapshot roots cannot contain special files"))
    }
}

/// A scan never persists plaintext. An explicit logical root id survives a move
/// to another checkout; an operator may use `snapshot_root_id` for a new root.
pub fn snapshot_root_id(label: &str) -> String {
    hash(label.as_bytes())
}
#[derive(Clone, Debug)]
pub struct SnapshotPacket {
    pub manifest: Value,
    pub blobs: BTreeMap<String, String>,
}
#[allow(clippy::too_many_arguments)]
pub fn capture_snapshot(
    root: &Path,
    root_id: &str,
    machine: &str,
    parent: Option<&str>,
    wall_ms: u64,
    mono_ms: u64,
    boot_id: &str,
    epoch: Option<u64>,
    recipients: &[x25519::Recipient],
) -> io::Result<SnapshotPacket> {
    SnapshotCache::default().capture(
        root, root_id, machine, parent, wall_ms, mono_ms, boot_id, epoch, None, recipients,
    )
}
impl SnapshotCache {
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        &mut self,
        root: &Path,
        root_id: &str,
        machine: &str,
        parent: Option<&str>,
        wall_ms: u64,
        mono_ms: u64,
        boot_id: &str,
        epoch: Option<u64>,
        session: Option<&str>,
        recipients: &[x25519::Recipient],
    ) -> io::Result<SnapshotPacket> {
        if session.is_some_and(|id| {
            id.is_empty()
                || id.len() > 256
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        }) {
            return Err(invalid("invalid associated session identifier"));
        }
        let scope = (
            root.to_owned(),
            recipients
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        );
        if self.scope.as_ref() != Some(&scope) {
            self.files.clear();
            self.scope = Some(scope);
        }
        identifier(root_id)?;
        if let Some(p) = parent {
            identifier(p)?;
        }
        let mut files = BTreeMap::new();
        let mut total = ScanBudget::default();
        let file = match bound_open(root) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let parent = root
                    .parent()
                    .ok_or_else(|| invalid("missing root lacks parent"))?;
                if !bound_open(parent)?.metadata()?.is_dir() {
                    return Err(invalid("missing root parent is not a directory"));
                }
                None
            }
            Err(error) => return Err(error),
        };
        let relative = if file
            .as_ref()
            .is_some_and(|file| file.metadata().is_ok_and(|meta| meta.is_file()))
        {
            PathBuf::from(
                root.file_name()
                    .ok_or_else(|| invalid("file root lacks name"))?,
            )
        } else {
            PathBuf::new()
        };
        if let Some(file) = file {
            bound_walk(
                file,
                &relative,
                &mut files,
                &mut total,
                &self.files,
                recipients,
                0,
            )?;
        }
        let mut blobs = BTreeMap::new();
        let mut entries = Vec::new();
        for (path, cached) in &files {
            blobs
                .entry(cached.blob.clone())
                .or_insert_with(|| cached.ciphertext.as_ref().clone());
            entries.push(json!({"path":path,"blob":cached.blob,"mode":cached.stamp.mode & 0o777}));
        }
        let clear = json!({"version":1,"root":root_id,"machine":machine,"parent":parent,"taken_at_wall":wall_ms,"taken_at_mono":mono_ms,"boot_id":boot_id,"epoch":epoch,"session":session,"entries":entries});
        let bytes = serde_json::to_vec(&clear)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(invalid("snapshot manifest exceeds limit"));
        }
        let id = hash(&bytes);
        let manifest = json!({"id":id,"root":root_id,"machine":machine,"parent":parent,"taken_at_wall":wall_ms,"blobs":blobs.keys().collect::<Vec<_>>(),"ciphertext":encrypt(&bytes,recipients)?});
        self.files = files;
        Ok(SnapshotPacket { manifest, blobs })
    }
}

/// Durable receiver storage, called under the receiver's mutation lock. Blob
/// and manifest files are immutable. Heads are derived from parents, so an
/// interrupted publish or concurrent siblings cannot lose a fork.
pub(crate) fn handle_snapshot(root: &Path, route: &str, request: &Value) -> io::Result<Value> {
    let base = root.join("snapshots");
    directory(&base)?;
    if route == "/v1/snapshots/roots" {
        let after = request["after"].as_str();
        if let Some(after) = after {
            identifier(after)?;
        }
        let mut roots = BTreeSet::new();
        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if identifier(&id).is_err() || after.is_some_and(|after| id.as_str() <= after) {
                continue;
            }
            roots.insert(id);
            if roots.len() > 101 {
                roots.pop_last();
            }
        }
        let more = roots.len() > 100;
        if more {
            roots.pop_last();
        }
        let next = more.then(|| roots.last().cloned()).flatten();
        return Ok(json!({"roots":roots,"next_after":next}));
    }
    let root_id = field(request, "root")?;
    identifier(root_id)?;
    let base = base.join(root_id);
    directory(&base)?;
    let blobs = base.join("blobs");
    directory(&blobs)?;
    let manifests = base.join("manifests");
    directory(&manifests)?;
    let pruned = base.join("pruned");
    directory(&pruned)?;
    match route {
        "/v1/snapshots/missing" => {
            if let Some(id) = request["manifest_id"].as_str() {
                identifier(id)?;
                if pruned.join(id).exists() {
                    return Err(invalid("snapshot manifest was forgotten; refusing replay"));
                }
            }
            let ids = request["ids"]
                .as_array()
                .ok_or_else(|| invalid("missing blob id list"))?;
            if ids.len() > ENTRY_MAX {
                return Err(invalid("too many blob ids"));
            }
            let mut missing = Vec::new();
            for id in ids {
                let id = id.as_str().ok_or_else(|| invalid("invalid blob id"))?;
                identifier(id)?;
                if !blobs.join(id).is_file() {
                    missing.push(id);
                }
            }
            Ok(json!({"missing":missing}))
        }
        "/v1/snapshots/blob" => {
            let id = field(request, "id")?;
            identifier(id)?;
            let cipher = field(request, "ciphertext")?;
            if cipher.len() > 2 * (FILE_MAX + 8192) || hex::decode(cipher).is_err() {
                return Err(invalid("invalid encrypted snapshot blob"));
            }
            let path = blobs.join(id);
            if path.exists() {
                let old = read_regular(&path, 2 * (FILE_MAX + 8192))?;
                let record: Value = serde_json::from_slice(&old)?;
                if hash(field(&record, "ciphertext")?.as_bytes()) != field(&record, "digest")? {
                    return Err(invalid("stored blob checksum mismatch"));
                }
            } else {
                immutable_write(
                    &path,
                    &serde_json::to_vec(
                        &json!({"ciphertext":cipher,"digest":hash(cipher.as_bytes())}),
                    )?,
                )?;
            }
            Ok(json!({"stored":id}))
        }
        "/v1/snapshots/manifest" => {
            let m = &request["manifest"];
            let cutoff_path = base.join("forget-before");
            if cutoff_path.exists() {
                let cutoff: String = String::from_utf8(read_regular(&cutoff_path, 32)?)
                    .map_err(|_| invalid("invalid root deletion cutoff"))?;
                let cutoff = cutoff
                    .parse::<u64>()
                    .map_err(|_| invalid("invalid root deletion cutoff"))?;
                if m["taken_at_wall"]
                    .as_u64()
                    .is_some_and(|wall| wall < cutoff)
                {
                    return Err(invalid("snapshot predates root deletion; refusing replay"));
                }
            }
            let id = field(m, "id")?;
            identifier(id)?;
            if field(m, "root")? != root_id
                || field(m, "machine")? != field(request, "machine")?
                || !m["taken_at_wall"].is_u64()
            {
                return Err(invalid("snapshot header mismatch"));
            }
            let cipher = field(m, "ciphertext")?;
            if cipher.len() > 2 * (2 * 1024 * 1024 + 8192) || hex::decode(cipher).is_err() {
                return Err(invalid("invalid encrypted snapshot manifest"));
            }
            if let Some(parent) = m["parent"].as_str() {
                identifier(parent)?;
                if !manifests.join(parent).is_file() && !pruned.join(parent).is_file() {
                    return Err(invalid("snapshot parent is missing"));
                }
            }
            let refs = m["blobs"]
                .as_array()
                .ok_or_else(|| invalid("snapshot blob references missing"))?;
            if refs.len() > ENTRY_MAX {
                return Err(invalid("too many blob references"));
            }
            for id in refs {
                let id = id
                    .as_str()
                    .ok_or_else(|| invalid("invalid blob reference"))?;
                identifier(id)?;
                if !blobs.join(id).is_file() {
                    return Err(invalid("snapshot blob is missing"));
                }
            }
            if pruned.join(id).exists() {
                return Err(invalid("snapshot was forgotten; refusing replay"));
            }
            let bytes = serde_json::to_vec(m)?;
            if manifests.join(id).exists() {
                let old = read_regular(&manifests.join(id), 4 * 1024 * 1024)?;
                let old_value: Value = serde_json::from_slice(&old)?;
                for key in ["id", "root", "machine", "parent", "taken_at_wall", "blobs"] {
                    if old_value[key] != m[key] {
                        return Err(invalid("immutable manifest header conflict"));
                    }
                }
                return Ok(json!({"stored":id}));
            }
            immutable_write(&manifests.join(id), &bytes)?;
            Ok(json!({"stored":id}))
        }
        "/v1/snapshots/list" => {
            let mut out = Vec::new();
            for entry in fs::read_dir(manifests)? {
                let entry = entry?;
                if !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| identifier(name).is_ok())
                {
                    continue;
                }
                if pruned.join(entry.file_name()).exists() {
                    continue;
                }
                out.push(serde_json::from_slice::<Value>(&read_regular(
                    &entry.path(),
                    4 * 1024 * 1024,
                )?)?);
            }
            let parents = out
                .iter()
                .filter_map(|m| m["parent"].as_str())
                .collect::<BTreeSet<_>>();
            let heads = out
                .iter()
                .filter(|m| !parents.contains(m["id"].as_str().unwrap_or("")))
                .map(|m| m["id"].clone())
                .collect::<Vec<_>>();
            if heads.len() > ENTRY_MAX {
                return Err(invalid("too many root heads"));
            }
            out.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            let after = request["after"].as_str();
            if let Some(after) = after {
                identifier(after)?;
            }
            let mut page = out
                .into_iter()
                .filter(|m| request["heads_only"] != true || heads.contains(&m["id"]))
                .filter(|m| after.is_none_or(|after| m["id"].as_str().is_some_and(|id| id > after)))
                .take(101)
                .collect::<Vec<_>>();
            let more = page.len() > 100;
            if more {
                page.pop();
            }
            let next = if more {
                page.last().map(|m| m["id"].clone())
            } else {
                None
            };
            for m in &mut page {
                if let Some(object) = m.as_object_mut() {
                    object.remove("ciphertext");
                    object.remove("blobs");
                }
            }
            Ok(json!({"manifests":page,"heads":heads,"next_after":next}))
        }
        "/v1/snapshots/get" => {
            let id = field(request, "id")?;
            identifier(id)?;
            let kind = field(request, "kind")?;
            if kind == "manifest" && pruned.join(id).exists() {
                return Err(invalid("snapshot manifest was forgotten"));
            }
            let path = match kind {
                "blob" => blobs.join(id),
                "manifest" => manifests.join(id),
                _ => return Err(invalid("unknown snapshot object kind")),
            };
            let bytes = read_regular(&path, 2 * (FILE_MAX + 8192))?;
            if kind == "blob" {
                let record: Value = serde_json::from_slice(&bytes)?;
                if hash(field(&record, "ciphertext")?.as_bytes()) != field(&record, "digest")? {
                    return Err(invalid("stored blob checksum mismatch"));
                }
                Ok(json!({"ciphertext":record["ciphertext"]}))
            } else {
                Ok(serde_json::from_slice(&bytes)?)
            }
        }
        "/v1/snapshots/forget" => {
            let before = request["before_wall_ms"].as_u64().unwrap_or(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| invalid(e.to_string()))?
                    .as_millis() as u64,
            );
            let cutoff_path = base.join("forget-before");
            let old = if cutoff_path.exists() {
                String::from_utf8(read_regular(&cutoff_path, 32)?)
                    .map_err(|_| invalid("invalid deletion cutoff"))?
                    .parse::<u64>()
                    .map_err(|_| invalid("invalid deletion cutoff"))?
            } else {
                0
            };
            let cutoff = old.max(before);
            crate::state::write_atomic(&cutoff_path, cutoff.to_string().as_bytes())?;
            File::open(&base)?.sync_all()?;
            let mut removed = 0;
            for entry in fs::read_dir(&manifests)? {
                let entry = entry?;
                if !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| identifier(name).is_ok())
                {
                    continue;
                }
                let path = entry.path();
                let m: Value = serde_json::from_slice(&read_regular(&path, 4 * 1024 * 1024)?)?;
                if m["taken_at_wall"]
                    .as_u64()
                    .is_some_and(|wall| wall < before)
                {
                    immutable_write(&pruned.join(field(&m, "id")?), b"forgotten")?;
                    fs::remove_file(path)?;
                    removed += 1;
                }
            }
            let mut refs = BTreeSet::new();
            for entry in fs::read_dir(&manifests)? {
                let entry = entry?;
                if !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| identifier(name).is_ok())
                {
                    continue;
                }
                let m: Value =
                    serde_json::from_slice(&read_regular(&entry.path(), 4 * 1024 * 1024)?)?;
                for id in m["blobs"]
                    .as_array()
                    .ok_or_else(|| invalid("invalid retained manifest"))?
                {
                    refs.insert(
                        id.as_str()
                            .ok_or_else(|| invalid("invalid retained blob"))?
                            .to_owned(),
                    );
                }
            }
            let mut collected = 0;
            for entry in fs::read_dir(blobs)? {
                let entry = entry?;
                if !refs.contains(&entry.file_name().to_string_lossy().into_owned()) {
                    fs::remove_file(entry.path())?;
                    collected += 1;
                }
            }
            Ok(
                json!({"removed_manifests":removed,"removed_blobs":collected,"retained_referenced_blobs":refs.len(),"warning":"Copies already restored on other machines cannot be removed."}),
            )
        }
        _ => Err(invalid("unknown snapshot route")),
    }
}

pub fn publish_snapshot(
    packet: &SnapshotPacket,
    transport: &dyn Transport,
) -> Result<(), TransportError> {
    let machine = packet.manifest["machine"].clone();
    let root = packet.manifest["root"].clone();
    let response = transport.snapshot_request(
        "/v1/snapshots/missing",
        &json!({"root":root,"machine":machine,"manifest_id":packet.manifest["id"],"ids":packet.blobs.keys().collect::<Vec<_>>()}),
    )?;
    let missing = response["missing"]
        .as_array()
        .ok_or_else(|| TransportError::InvalidAck("invalid missing blob response".into()))?;
    for (id, ciphertext) in &packet.blobs {
        if !missing.contains(&json!(id)) {
            continue;
        }
        transport.snapshot_request(
            "/v1/snapshots/blob",
            &json!({"root":root,"machine":machine,"id":id,"ciphertext":ciphertext}),
        )?;
    }
    transport.snapshot_request(
        "/v1/snapshots/manifest",
        &json!({"root":root,"machine":machine,"manifest":packet.manifest}),
    )?;
    Ok(())
}

/// Restore into a new destination. Conflicting heads are reported with paths
/// whose plaintext content differs, rather than merging or discarding edits.
pub fn restore_snapshot(
    root_id: &str,
    selected: Option<&str>,
    target: &Path,
    machine: &str,
    identity: &x25519::Identity,
    transport: &dyn Transport,
) -> Result<Value, Box<dyn std::error::Error>> {
    restore_snapshot_for_session(
        root_id, selected, target, machine, identity, transport, None,
    )
}
/// Restores an explicitly selected memory root and checks any authenticated
/// session association before publishing a new directory.
pub fn restore_snapshot_for_session(
    root_id: &str,
    selected: Option<&str>,
    target: &Path,
    machine: &str,
    identity: &x25519::Identity,
    transport: &dyn Transport,
    session: Option<&str>,
) -> Result<Value, Box<dyn std::error::Error>> {
    identifier(root_id)?;
    if target.exists() {
        return Err(invalid("snapshot restore target already exists").into());
    }
    let list = list_snapshot_history(root_id, machine, transport)?;
    let heads = list["heads"]
        .as_array()
        .ok_or_else(|| invalid("invalid snapshot heads"))?;
    let manifests = list["manifests"]
        .as_array()
        .ok_or_else(|| invalid("invalid snapshot manifests"))?;
    let chosen = if let Some(id) = selected {
        manifests.iter().find(|m| m["id"] == id)
    } else {
        manifests.iter().rev().find(|m| heads.contains(&m["id"]))
    }
    .ok_or_else(|| invalid("no selected snapshot manifest"))?;
    let chosen_id = chosen["id"].clone();
    let chosen = transport.snapshot_request(
        "/v1/snapshots/get",
        &json!({"root":root_id,"machine":machine,"kind":"manifest","id":chosen["id"]}),
    )?;
    if chosen["id"] != chosen_id {
        return Err(invalid("selected manifest substitution").into());
    }
    let clear = verified_manifest(&chosen, identity)?;
    if let Some(session) = session
        && let Some(associated) = clear["session"].as_str()
        && associated != session
    {
        return Err(invalid("snapshot is associated with a different session").into());
    }
    if field(&clear, "root")? != root_id {
        return Err(invalid("snapshot root authentication failed").into());
    }
    let mut differences = Vec::new();
    let chosen_entries = entry_map(&clear)?;
    for id in heads {
        if *id == chosen["id"] {
            continue;
        }
        let other = manifests
            .iter()
            .find(|m| m["id"] == *id)
            .ok_or_else(|| invalid("head missing"))?;
        let other = transport.snapshot_request(
            "/v1/snapshots/get",
            &json!({"root":root_id,"machine":machine,"kind":"manifest","id":other["id"]}),
        )?;
        if other["id"] != *id {
            return Err(invalid("head manifest substitution").into());
        }
        let other_clear = verified_manifest(&other, identity)?;
        if field(&other_clear, "root")? != root_id {
            return Err(invalid("snapshot head root authentication failed").into());
        }
        let entries = entry_map(&other_clear)?;
        let keys = chosen_entries
            .keys()
            .chain(entries.keys())
            .collect::<BTreeSet<_>>();
        let changed = keys
            .into_iter()
            .filter(|k| chosen_entries.get(*k) != entries.get(*k))
            .collect::<Vec<_>>();
        differences.push(json!({"head":id,"paths":changed}));
    }
    // Authenticate and stage every blob before creating the requested target.
    let parent = target
        .parent()
        .ok_or_else(|| invalid("restore target lacks parent"))?;
    let stage = parent.join(format!(
        ".semon-snapshot-{}-{}",
        std::process::id(),
        hex::encode(crate::generate_data_key())
    ));
    directory(&stage)?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut restored_bytes = 0;
        for (relative, (id, mode)) in &chosen_entries {
            let path = safe_relative(relative)?;
            let response = transport.snapshot_request(
                "/v1/snapshots/get",
                &json!({"root":root_id,"machine":machine,"kind":"blob","id":id}),
            )?;
            let bytes = decrypt(field(&response, "ciphertext")?, identity, FILE_MAX)?;
            restored_bytes += bytes.len();
            if restored_bytes > ROOT_MAX {
                return Err(invalid("restored root exceeds total limit").into());
            }
            if hash(&bytes) != *id {
                return Err(invalid("snapshot blob hash mismatch").into());
            }
            let dest = stage.join(path);
            let mut current = stage.clone();
            for part in dest.parent().unwrap().strip_prefix(&stage)?.components() {
                current.push(part);
                directory(&current)?;
            }
            immutable_write(&dest, &bytes)?;
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(*mode))?;
            }
        }
        // Rename cannot overwrite a non-empty directory, but the explicit
        // preflight is repeated to reject even an empty operator target.
        publish_directory(&stage, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result?;
    Ok(
        json!({"manifest":chosen["id"],"machine":clear["machine"],"epoch":clear["epoch"],"session":clear["session"],"restored_files":chosen_entries.len(),"other_heads":differences,"target":target,"warning":"Review divergent heads before continuing edits."}),
    )
}
fn verified_manifest(wire: &Value, identity: &x25519::Identity) -> io::Result<Value> {
    let bytes = decrypt(field(wire, "ciphertext")?, identity, 2 * 1024 * 1024)?;
    if hash(&bytes) != field(wire, "id")? {
        return Err(invalid("snapshot manifest hash mismatch"));
    }
    let clear: Value = serde_json::from_slice(&bytes)?;
    for key in ["root", "machine", "parent", "taken_at_wall"] {
        if clear[key] != wire[key] {
            return Err(invalid("snapshot public header authentication failed"));
        }
    }
    let refs = entry_map(&clear)?
        .into_values()
        .map(|(id, _)| id)
        .collect::<BTreeSet<_>>();
    let supplied = wire["blobs"]
        .as_array()
        .ok_or_else(|| invalid("missing public blob references"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid("invalid blob id"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if refs != supplied {
        return Err(invalid("snapshot blob references authentication failed"));
    }
    Ok(clear)
}
fn entry_map(clear: &Value) -> io::Result<BTreeMap<String, (String, u32)>> {
    if clear["version"] != 1 {
        return Err(invalid("unsupported snapshot version"));
    }
    let entries = clear["entries"]
        .as_array()
        .ok_or_else(|| invalid("missing manifest entries"))?;
    if entries.len() > ENTRY_MAX {
        return Err(invalid("too many snapshot entries"));
    }
    let mut out = BTreeMap::new();
    for e in entries {
        let path = field(e, "path")?;
        safe_relative(path)?;
        let blob = field(e, "blob")?;
        identifier(blob)?;
        let mode = e["mode"]
            .as_u64()
            .filter(|mode| *mode <= 0o777)
            .ok_or_else(|| invalid("invalid snapshot mode"))? as u32;
        if out
            .insert(path.to_owned(), (blob.to_owned(), mode))
            .is_some()
        {
            return Err(invalid("duplicate snapshot path"));
        }
    }
    Ok(out)
}

fn publish_directory(stage: &Path, target: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let from = CString::new(stage.as_os_str().as_bytes())
            .map_err(|_| invalid("invalid staging path"))?;
        let to = CString::new(target.as_os_str().as_bytes())
            .map_err(|_| invalid("invalid target path"))?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        File::open(
            target
                .parent()
                .ok_or_else(|| invalid("target lacks parent"))?,
        )?
        .sync_all()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (stage, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace directory restore is not available on this platform",
        ))
    }
}

impl SnapshotPacket {
    pub fn to_value(&self) -> Value {
        json!({"manifest":self.manifest,"blobs":self.blobs})
    }
    pub fn from_value(value: &Value) -> io::Result<Self> {
        let manifest = value["manifest"].clone();
        identifier(field(&manifest, "root")?)?;
        identifier(field(&manifest, "id")?)?;
        let values = value["blobs"]
            .as_object()
            .ok_or_else(|| invalid("invalid snapshot outbox"))?;
        if values.len() > ENTRY_MAX {
            return Err(invalid("too many pending blobs"));
        }
        let mut blobs = BTreeMap::new();
        let mut total = 0;
        for (id, value) in values {
            identifier(id)?;
            let cipher = value
                .as_str()
                .ok_or_else(|| invalid("invalid pending ciphertext"))?;
            total += cipher.len();
            if cipher.len() > 2 * (FILE_MAX + 8192) || total > 2 * (ROOT_MAX + ENTRY_MAX * 8192) {
                return Err(invalid("snapshot outbox exceeds limit"));
            }
            blobs.insert(id.clone(), cipher.to_owned());
        }
        Ok(Self { manifest, blobs })
    }
}
/// Authenticated inspection is used to detect unchanged entries without writing
/// decrypted memory or manifest paths to the sender's durable state.
pub fn inspect_snapshot_manifest(wire: &Value, identity: &x25519::Identity) -> io::Result<Value> {
    verified_manifest(wire, identity)
}
pub fn persist_snapshot_packet(path: &Path, packet: &SnapshotPacket) -> io::Result<()> {
    crate::state::write_atomic(path, &serde_json::to_vec(&packet.to_value())?)?;
    File::open(
        path.parent()
            .ok_or_else(|| invalid("outbox lacks parent"))?,
    )?
    .sync_all()
}
pub fn load_snapshot_packet(path: &Path) -> io::Result<SnapshotPacket> {
    SnapshotPacket::from_value(&serde_json::from_slice(&read_regular(
        path,
        64 * 1024 * 1024,
    )?)?)
}

/// Enumerates opaque logical roots in bounded authenticated pages. Contents,
/// relative paths and encrypted session associations are not returned.
pub fn list_snapshot_roots(
    machine: &str,
    transport: &dyn Transport,
) -> Result<Vec<String>, TransportError> {
    let mut roots = Vec::new();
    let mut after = None::<String>;
    loop {
        let page = transport.snapshot_request(
            "/v1/snapshots/roots",
            &json!({"machine":machine,"after":after}),
        )?;
        let values = page["roots"]
            .as_array()
            .ok_or_else(|| TransportError::InvalidAck("invalid snapshot roots".into()))?;
        if values.len() > 100 {
            return Err(TransportError::InvalidAck(
                "snapshot root page exceeds limit".into(),
            ));
        }
        for value in values {
            let id = value
                .as_str()
                .filter(|id| identifier(id).is_ok())
                .ok_or_else(|| TransportError::InvalidAck("invalid snapshot root id".into()))?;
            if roots
                .last()
                .is_some_and(|previous: &String| id <= previous.as_str())
            {
                return Err(TransportError::InvalidAck(
                    "snapshot roots are not advancing".into(),
                ));
            }
            roots.push(id.to_owned());
            if roots.len() > ENTRY_MAX {
                return Err(TransportError::InvalidAck(
                    "snapshot root listing exceeds client limit".into(),
                ));
            }
        }
        match page["next_after"].as_str() {
            None => break,
            Some(next)
                if roots.last().is_some_and(|id| id == next)
                    && after.as_deref().is_none_or(|old| next > old) =>
            {
                after = Some(next.to_owned())
            }
            Some(_) => {
                return Err(TransportError::InvalidAck(
                    "invalid snapshot root cursor".into(),
                ));
            }
        }
    }
    Ok(roots)
}
/// Decrypts one selected snapshot file in memory without publishing any paths.
/// The manifest id, root and content hash are authenticated before returning bytes.
pub fn read_snapshot_file(
    root_id: &str,
    manifest_id: &str,
    path: &str,
    machine: &str,
    identity: &x25519::Identity,
    transport: &dyn Transport,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    identifier(root_id)?;
    identifier(manifest_id)?;
    safe_relative(path)?;
    let wire = transport.snapshot_request(
        "/v1/snapshots/get",
        &json!({"root":root_id,"machine":machine,"kind":"manifest","id":manifest_id}),
    )?;
    if wire["id"] != manifest_id || wire["root"] != root_id {
        return Err(invalid("snapshot manifest substitution").into());
    }
    let clear = verified_manifest(&wire, identity)?;
    let entries = entry_map(&clear)?;
    let Some((blob, _mode)) = entries.get(path) else {
        return Ok(None);
    };
    let wire = transport.snapshot_request(
        "/v1/snapshots/get",
        &json!({"root":root_id,"machine":machine,"kind":"blob","id":blob}),
    )?;
    let bytes = decrypt(field(&wire, "ciphertext")?, identity, FILE_MAX)?;
    if hash(&bytes) != *blob {
        return Err(invalid("snapshot blob hash mismatch").into());
    }
    Ok(Some(bytes))
}

pub fn list_snapshot_history(
    root_id: &str,
    machine: &str,
    transport: &dyn Transport,
) -> Result<Value, TransportError> {
    list_snapshot_metadata(root_id, machine, transport, false)
}
/// Current fork heads only, in bounded pages; old history is not transferred.
pub fn list_snapshot_heads(
    root_id: &str,
    machine: &str,
    transport: &dyn Transport,
) -> Result<Value, TransportError> {
    list_snapshot_metadata(root_id, machine, transport, true)
}
fn list_snapshot_metadata(
    root_id: &str,
    machine: &str,
    transport: &dyn Transport,
    heads_only: bool,
) -> Result<Value, TransportError> {
    let mut after = None::<String>;
    let mut manifests = Vec::new();
    let mut heads = Vec::new();
    loop {
        let page = transport.snapshot_request(
            "/v1/snapshots/list",
            &json!({"root":root_id,"machine":machine,"after":after,"heads_only":heads_only}),
        )?;
        let rows = page["manifests"]
            .as_array()
            .ok_or_else(|| TransportError::InvalidAck("invalid manifest page".into()))?;
        if rows.len() > 100 || (heads_only && manifests.len() + rows.len() > 4096) {
            return Err(TransportError::InvalidResponse(
                "snapshot metadata page exceeds limit".into(),
            ));
        }
        manifests.extend_from_slice(rows);
        if after.is_none() {
            heads = page["heads"]
                .as_array()
                .ok_or_else(|| TransportError::InvalidAck("invalid snapshot heads".into()))?
                .clone();
        }
        let next = page["next_after"].as_str();
        match next {
            None => break,
            Some(next) if after.as_deref().is_none_or(|old| next > old) => {
                after = Some(next.to_owned())
            }
            Some(_) => {
                return Err(TransportError::InvalidAck(
                    "non-advancing snapshot history cursor".into(),
                ));
            }
        }
    }
    manifests.sort_by_key(|m| m["taken_at_wall"].as_u64().unwrap_or(0));
    Ok(json!({"manifests":manifests,"heads":heads}))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "semon-snapshot-{}",
                hex::encode(crate::generate_data_key())
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Local(PathBuf);
    impl Transport for Local {
        fn send(&self, _: &crate::Frame) -> Result<u64, TransportError> {
            unreachable!()
        }
        fn snapshot_request(&self, route: &str, value: &Value) -> Result<Value, TransportError> {
            handle_snapshot(&self.0, route, value)
                .map_err(|e| TransportError::Unavailable(e.to_string()))
        }
    }
    fn capture(
        root: &Path,
        id: &str,
        machine: &str,
        parent: Option<&str>,
        wall: u64,
        identity: &x25519::Identity,
    ) -> SnapshotPacket {
        capture_snapshot(
            root,
            id,
            machine,
            parent,
            wall,
            wall,
            "boot",
            None,
            &[identity.to_public()],
        )
        .unwrap()
    }
    #[test]
    fn encrypted_history_forks_restore_modes_and_collect_only_unreferenced_blobs() {
        use std::os::unix::fs::PermissionsExt;
        let source = Temp::new();
        let store = Temp::new();
        let dest = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("memory");
        let transport = Local(store.0.clone());
        fs::write(source.0.join("MEMORY.md"), "shared secret text").unwrap();
        fs::set_permissions(
            source.0.join("MEMORY.md"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        let a = capture(&source.0, &root, "a", None, 1, &identity);
        publish_snapshot(&a, &transport).unwrap();
        publish_snapshot(&a, &transport).unwrap();
        fs::write(source.0.join("note.md"), "branch A").unwrap();
        let b = capture(
            &source.0,
            &root,
            "a",
            a.manifest["id"].as_str(),
            2,
            &identity,
        );
        publish_snapshot(&b, &transport).unwrap();
        fs::write(source.0.join("note.md"), "branch B").unwrap();
        let c = capture(
            &source.0,
            &root,
            "b",
            a.manifest["id"].as_str(),
            3,
            &identity,
        );
        publish_snapshot(&c, &transport).unwrap();
        let list = transport
            .snapshot_request("/v1/snapshots/list", &json!({"root":root,"machine":"a"}))
            .unwrap();
        assert_eq!(list["heads"].as_array().unwrap().len(), 2);
        let report = restore_snapshot(
            &root,
            None,
            &dest.0.join("restore"),
            "a",
            &identity,
            &transport,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dest.0.join("restore/note.md")).unwrap(),
            "branch B"
        );
        assert_eq!(report["other_heads"][0]["paths"], json!(["note.md"]));
        assert_eq!(
            fs::metadata(dest.0.join("restore/MEMORY.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        let gc = transport
            .snapshot_request(
                "/v1/snapshots/forget",
                &json!({"root":root,"machine":"a","before_wall_ms":3}),
            )
            .unwrap();
        assert_eq!(gc["removed_manifests"], 2);
        assert_eq!(gc["removed_blobs"], 1);
        assert_eq!(gc["retained_referenced_blobs"], 2);
        for dir in ["blobs", "manifests"] {
            for entry in fs::read_dir(store.0.join("snapshots").join(&root).join(dir)).unwrap() {
                let bytes = fs::read(entry.unwrap().path()).unwrap();
                assert!(!String::from_utf8_lossy(&bytes).contains("shared secret text"));
            }
        }
    }
    #[test]
    fn bound_sources_refuse_ancestor_symlinks_and_special_files() {
        use std::os::unix::fs::symlink;
        let source = Temp::new();
        let outside = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("root");
        fs::write(outside.0.join("secret"), "secret").unwrap();
        symlink(&outside.0, source.0.join("escape")).unwrap();
        assert!(
            capture_snapshot(
                &source.0.join("escape"),
                &root,
                "a",
                None,
                1,
                1,
                "boot",
                None,
                &[identity.to_public()]
            )
            .is_err()
        );
        assert!(
            capture_snapshot(
                &source.0,
                &root,
                "a",
                None,
                1,
                1,
                "boot",
                None,
                &[identity.to_public()]
            )
            .is_err()
        );
    }
    #[test]
    fn destination_publication_refuses_a_dangling_link_or_existing_directory() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        let stage = temp.0.join("stage");
        fs::create_dir(&stage).unwrap();
        let target = temp.0.join("target");
        symlink("missing", &target).unwrap();
        assert!(publish_directory(&stage, &target).is_err());
        assert!(stage.is_dir());
        fs::remove_file(&target).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(publish_directory(&stage, &target).is_err());
        assert!(stage.is_dir());
    }
    #[test]
    fn manifest_rejects_unsafe_paths_mode_and_unbound_root() {
        let identity = x25519::Identity::generate();
        let source = Temp::new();
        fs::write(source.0.join("one"), "a").unwrap();
        let root = snapshot_root_id("root");
        let packet = capture(&source.0, &root, "a", None, 1, &identity);
        let clear = verified_manifest(&packet.manifest, &identity).unwrap();
        let mut bad = clear.clone();
        bad["entries"][0]["path"] = json!("../escape");
        assert!(entry_map(&bad).is_err());
        bad = clear;
        bad["entries"][0]["mode"] = json!(0o4777);
        assert!(entry_map(&bad).is_err());
        let mut wire = packet.manifest.clone();
        wire["root"] = json!(snapshot_root_id("other"));
        assert!(verified_manifest(&wire, &identity).is_err());
    }
    #[test]
    fn a_removed_single_file_root_records_an_empty_manifest() {
        let source = Temp::new();
        let store = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("file");
        let transport = Local(store.0.clone());
        let file = source.0.join("CLAUDE.md");
        fs::write(&file, "old memory").unwrap();
        let old = capture(&file, &root, "a", None, 1, &identity);
        publish_snapshot(&old, &transport).unwrap();
        fs::remove_file(&file).unwrap();
        let empty = capture(&file, &root, "a", old.manifest["id"].as_str(), 2, &identity);
        publish_snapshot(&empty, &transport).unwrap();
        assert_eq!(
            inspect_snapshot_manifest(&empty.manifest, &identity).unwrap()["entries"],
            json!([])
        );
        assert_eq!(
            list_snapshot_history(&root, "a", &transport).unwrap()["manifests"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn forgotten_parent_keeps_lineage_but_refuses_replay() {
        let source = Temp::new();
        let store = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("memory");
        let transport = Local(store.0.clone());
        fs::write(source.0.join("one"), "old").unwrap();
        let old = capture(&source.0, &root, "a", None, 1, &identity);
        publish_snapshot(&old, &transport).unwrap();
        transport
            .snapshot_request(
                "/v1/snapshots/forget",
                &json!({"root":root,"machine":"a","before_wall_ms":2}),
            )
            .unwrap();
        assert!(publish_snapshot(&old, &transport).is_err());
        fs::write(source.0.join("one"), "new").unwrap();
        let new = capture(
            &source.0,
            &root,
            "a",
            old.manifest["id"].as_str(),
            2,
            &identity,
        );
        publish_snapshot(&new, &transport).unwrap();
        let list = list_snapshot_history(&root, "a", &transport).unwrap();
        assert_eq!(list["manifests"].as_array().unwrap().len(), 1);
        assert_eq!(list["heads"], json!([new.manifest["id"]]));
    }
    #[test]
    fn explicit_manifest_id_cannot_be_substituted_with_another_valid_snapshot() {
        struct Substitute {
            inner: Local,
            replacement: Value,
        }
        impl Transport for Substitute {
            fn send(&self, _: &crate::Frame) -> Result<u64, TransportError> {
                unreachable!()
            }
            fn snapshot_request(
                &self,
                route: &str,
                value: &Value,
            ) -> Result<Value, TransportError> {
                if route == "/v1/snapshots/get" && value["kind"] == "manifest" {
                    Ok(self.replacement.clone())
                } else {
                    self.inner.snapshot_request(route, value)
                }
            }
        }
        let source = Temp::new();
        let store = Temp::new();
        let dest = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("memory");
        let transport = Local(store.0.clone());
        fs::write(source.0.join("one"), "old").unwrap();
        let old = capture(&source.0, &root, "a", None, 1, &identity);
        publish_snapshot(&old, &transport).unwrap();
        fs::write(source.0.join("one"), "new").unwrap();
        let new = capture(
            &source.0,
            &root,
            "a",
            old.manifest["id"].as_str(),
            2,
            &identity,
        );
        publish_snapshot(&new, &transport).unwrap();
        let malicious = Substitute {
            inner: transport,
            replacement: new.manifest,
        };
        let result = restore_snapshot(
            &root,
            old.manifest["id"].as_str(),
            &dest.0.join("target"),
            "a",
            &identity,
            &malicious,
        );
        assert!(result.unwrap_err().to_string().contains("substitution"));
        assert!(!dest.0.join("target").exists());
    }
    #[test]
    fn a_crashed_unpublished_temp_does_not_poison_history() {
        let source = Temp::new();
        let store = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("memory");
        let transport = Local(store.0.clone());
        let packet = capture(&source.0, &root, "a", None, 1, &identity);
        publish_snapshot(&packet, &transport).unwrap();
        fs::write(
            store
                .0
                .join("snapshots")
                .join(&root)
                .join("manifests/.tmp-crash"),
            "partial json",
        )
        .unwrap();
        assert_eq!(
            list_snapshot_history(&root, "a", &transport).unwrap()["manifests"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        transport
            .snapshot_request(
                "/v1/snapshots/forget",
                &json!({"root":root,"machine":"a","before_wall_ms":2}),
            )
            .unwrap();
    }
    #[test]
    fn history_pages_are_bounded_and_do_not_include_ciphertext() {
        let source = Temp::new();
        let store = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("memory");
        let transport = Local(store.0.clone());
        let mut parent = None::<String>;
        for wall in 0..103 {
            let packet = capture(&source.0, &root, "a", parent.as_deref(), wall, &identity);
            publish_snapshot(&packet, &transport).unwrap();
            parent = packet.manifest["id"].as_str().map(str::to_owned);
        }
        let page = transport
            .snapshot_request("/v1/snapshots/list", &json!({"root":root,"machine":"a"}))
            .unwrap();
        assert_eq!(page["manifests"].as_array().unwrap().len(), 100);
        assert!(page["next_after"].is_string());
        assert!(page["manifests"][0].get("ciphertext").is_none());
        let all = list_snapshot_history(&root, "a", &transport).unwrap();
        assert_eq!(all["manifests"].as_array().unwrap().len(), 103);
        assert_eq!(all["heads"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn immutable_publication_detects_a_conflicting_existing_object() {
        let temp = Temp::new();
        let path = temp.0.join("object");
        immutable_write(&path, b"complete").unwrap();
        immutable_write(&path, b"complete").unwrap();
        assert!(immutable_write(&path, b"changed").is_err());
        assert_eq!(fs::read(path).unwrap(), b"complete");
    }
    #[test]
    fn watched_snapshots_read_and_encrypt_only_changed_generations() {
        let source = Temp::new();
        let root = snapshot_root_id("watched");
        let identity = x25519::Identity::generate();
        let recipients = [identity.to_public()];
        let path = source.0.join("memory.md");
        fs::write(&path, "secret one").unwrap();
        let mut cache = SnapshotCache::default();
        let bytes = SOURCE_BYTES.with(|value| value.get());
        let encryptions = SOURCE_ENCRYPTIONS.with(|value| value.get());
        let first = cache
            .capture(
                &source.0,
                &root,
                "a",
                None,
                1,
                1,
                "boot",
                Some(4),
                Some("session"),
                &recipients,
            )
            .unwrap();
        assert_eq!(SOURCE_BYTES.with(|value| value.get()) - bytes, 10);
        assert_eq!(
            SOURCE_ENCRYPTIONS.with(|value| value.get()) - encryptions,
            1
        );
        let second = cache
            .capture(
                &source.0,
                &root,
                "a",
                first.manifest["id"].as_str(),
                2,
                2,
                "boot",
                Some(4),
                Some("session"),
                &recipients,
            )
            .unwrap();
        assert_eq!(SOURCE_BYTES.with(|value| value.get()) - bytes, 10);
        assert_eq!(
            SOURCE_ENCRYPTIONS.with(|value| value.get()) - encryptions,
            1
        );
        assert_eq!(first.blobs, second.blobs);
        let clear = inspect_snapshot_manifest(&second.manifest, &identity).unwrap();
        assert_eq!(clear["session"], "session");
        assert_eq!(clear["epoch"], 4);
        // Same size and restored mtime still changes ctime; it must be reread.
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, "secret two").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let third = cache
            .capture(
                &source.0,
                &root,
                "a",
                None,
                3,
                3,
                "boot",
                None,
                None,
                &recipients,
            )
            .unwrap();
        assert_ne!(
            third.blobs.keys().collect::<Vec<_>>(),
            first.blobs.keys().collect::<Vec<_>>()
        );
        assert_eq!(SOURCE_BYTES.with(|value| value.get()) - bytes, 20);
        // An inode replacement with identical content is read, but reuses ciphertext.
        let replacement = source.0.join("replacement");
        fs::write(&replacement, "secret two").unwrap();
        fs::rename(&replacement, &path).unwrap();
        cache
            .capture(
                &source.0,
                &root,
                "a",
                None,
                4,
                4,
                "boot",
                None,
                None,
                &recipients,
            )
            .unwrap();
        assert_eq!(SOURCE_BYTES.with(|value| value.get()) - bytes, 30);
        assert_eq!(
            SOURCE_ENCRYPTIONS.with(|value| value.get()) - encryptions,
            2
        );
        // Recipient changes cannot reuse old envelopes.
        let other = x25519::Identity::generate();
        let rotated = cache
            .capture(
                &source.0,
                &root,
                "a",
                None,
                5,
                5,
                "boot",
                None,
                None,
                &[other.to_public()],
            )
            .unwrap();
        assert!(inspect_snapshot_manifest(&rotated.manifest, &other).is_ok());
        assert_eq!(SOURCE_BYTES.with(|value| value.get()) - bytes, 40);
        assert_eq!(
            SOURCE_ENCRYPTIONS.with(|value| value.get()) - encryptions,
            3
        );
        fs::remove_file(&path).unwrap();
        let removed = cache
            .capture(
                &source.0,
                &root,
                "a",
                None,
                6,
                6,
                "boot",
                None,
                None,
                &[other.to_public()],
            )
            .unwrap();
        assert!(cache.files.is_empty());
        assert_eq!(
            inspect_snapshot_manifest(&removed.manifest, &other).unwrap()["entries"],
            json!([])
        );
    }
    #[test]
    fn associated_memory_refuses_another_session_before_writing_destination() {
        let source = Temp::new();
        let store = Temp::new();
        let dest = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("associated");
        fs::write(source.0.join("memory"), "content").unwrap();
        let packet = SnapshotCache::default()
            .capture(
                &source.0,
                &root,
                "a",
                None,
                1,
                1,
                "boot",
                Some(7),
                Some("owner"),
                &[identity.to_public()],
            )
            .unwrap();
        let transport = Local(store.0.clone());
        publish_snapshot(&packet, &transport).unwrap();
        let target = dest.0.join("new");
        assert!(
            restore_snapshot_for_session(
                &root,
                None,
                &target,
                "a",
                &identity,
                &transport,
                Some("other")
            )
            .unwrap_err()
            .to_string()
            .contains("different session")
        );
        assert!(!target.exists());
        let report = restore_snapshot_for_session(
            &root,
            None,
            &target,
            "a",
            &identity,
            &transport,
            Some("owner"),
        )
        .unwrap();
        assert_eq!(report["session"], "owner");
        assert_eq!(report["epoch"], 7);
        assert!(
            restore_snapshot_for_session(
                &root,
                None,
                &target,
                "a",
                &identity,
                &transport,
                Some("owner")
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(target.join("memory")).unwrap(),
            "content"
        );
    }
    #[test]
    fn remote_snapshot_metadata_lists_opaque_roots_and_reads_verified_files_without_restore() {
        let source = Temp::new();
        let store = Temp::new();
        let identity = x25519::Identity::generate();
        let root = snapshot_root_id("sidecars");
        fs::write(
            source.0.join("agent-x.meta.json"),
            "{\"toolUseId\":\"call\"}",
        )
        .unwrap();
        let packet = SnapshotCache::default()
            .capture(
                &source.0,
                &root,
                "a",
                None,
                1,
                1,
                "boot",
                Some(1),
                Some("session"),
                &[identity.to_public()],
            )
            .unwrap();
        let transport = Local(store.0.clone());
        publish_snapshot(&packet, &transport).unwrap();
        assert_eq!(
            list_snapshot_roots("a", &transport).unwrap(),
            vec![root.clone()]
        );
        let manifest = packet.manifest["id"].as_str().unwrap();
        assert_eq!(
            read_snapshot_file(
                &root,
                manifest,
                "agent-x.meta.json",
                "a",
                &identity,
                &transport
            )
            .unwrap()
            .unwrap(),
            b"{\"toolUseId\":\"call\"}"
        );
        assert!(
            read_snapshot_file(&root, manifest, "missing", "a", &identity, &transport)
                .unwrap()
                .is_none()
        );
        assert!(
            read_snapshot_file(&root, manifest, "../escape", "a", &identity, &transport).is_err()
        );
        for index in 0..103 {
            fs::create_dir(
                store
                    .0
                    .join("snapshots")
                    .join(snapshot_root_id(&format!("root-{index}"))),
            )
            .unwrap();
        }
        let page = transport
            .snapshot_request("/v1/snapshots/roots", &json!({"machine":"a"}))
            .unwrap();
        assert_eq!(page["roots"].as_array().unwrap().len(), 100);
        assert!(page["next_after"].is_string());
        let roots = list_snapshot_roots("a", &transport).unwrap();
        assert_eq!(roots.len(), 104);
        assert!(roots.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
