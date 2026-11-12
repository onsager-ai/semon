use super::*;
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{fs::MetadataExt, process::CommandExt},
    },
    path::{Component, Path},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|_| Failure::Storage)
}
fn name(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| Failure::Storage)
}
fn open(dir: &File, value: &str, flags: i32) -> Result<File> {
    let value = name(value)?;
    // SAFETY: live directory fd and NUL-terminated fixed/validated component.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            value.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(Failure::Storage);
    }
    // SAFETY: openat returned a newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn directory(dir: &File, value: &str) -> Result<File> {
    let c = name(value)?;
    // SAFETY: valid descriptor and component; existing paths are checked by openat.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), 0o700) } != 0
        && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
    {
        return Err(Failure::Storage);
    }
    open(dir, value, libc::O_RDONLY | libc::O_DIRECTORY)
}
fn unlink(dir: &File, value: &str) -> Result<()> {
    let value = name(value)?;
    // SAFETY: unlink only one fixed store-owned staging entry, without following it.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), value.as_ptr(), 0) } != 0 {
        return Err(Failure::Storage);
    }
    io(dir.sync_all())
}
fn read(mut file: File) -> Result<Vec<u8>> {
    if !io(file.metadata())?.is_file() {
        return Err(Failure::Storage);
    }
    let mut bytes = Vec::new();
    io(Read::by_ref(&mut file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Failure::Limit);
    }
    Ok(bytes)
}

/// One exclusive writer per authorization scope. Limits include pending uploads.
/// Linux descriptor-relative operations refuse symlinks in every root component.
pub struct ObjectStore {
    dir: File,
    _lock: crate::store::LockGuard,
    scope: Scope,
    key: String,
    quota: u64,
    max_objects: usize,
}
pub struct StagedObject {
    identity: Identity,
    key: String,
    // Pin the inode even after restart unlinks the old stage. Its identity
    // cannot be recycled for a replacement while this token remains alive.
    file: File,
}
/// Unforgeable in-process validation receipt. It grants no head activation.
pub struct ValidatedObject {
    identity: Identity,
    scope: Scope,
}
impl ValidatedObject {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
}
impl ObjectStore {
    pub fn open(root: &Path, scope: Scope, quota: u64) -> Result<Self> {
        Self::open_with_ceiling(root, scope, quota, 10_000)
    }
    fn open_with_ceiling(
        root: &Path,
        scope: Scope,
        quota: u64,
        max_objects: usize,
    ) -> Result<Self> {
        scope.check()?;
        if !root.is_absolute() || quota < MAX_BYTES {
            return Err(Failure::Invalid);
        }
        let mut parent = io(File::open("/"))?;
        for component in root.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(value) => {
                    parent = open(
                        &parent,
                        value.to_str().ok_or(Failure::Invalid)?,
                        libc::O_RDONLY | libc::O_DIRECTORY,
                    )?;
                }
                _ => return Err(Failure::Invalid),
            }
        }
        let base = directory(&parent, "asi-objects-v1")?;
        let key = hex::encode(Sha256::digest(
            serde_json::to_vec(&scope).map_err(|_| Failure::Invalid)?,
        ));
        let dir = directory(&base, &key)?;
        io(parent.sync_all())?;
        io(base.sync_all())?;
        let lock = open(&dir, "lock", libc::O_RDWR | libc::O_CREAT)?;
        if !io(lock.metadata())?.is_file() {
            return Err(Failure::Storage);
        }
        // Never unlink the lock inode; explicitly unlock even if a forked child
        // temporarily retains a duplicate of this open file description.
        let lock = crate::store::LockGuard::acquire(lock, true).map_err(|_| Failure::Storage)?;
        let store = Self {
            dir,
            _lock: lock,
            scope,
            key,
            quota,
            max_objects,
        };
        // Only the reserved incomplete stage is disposable. Unknown entries remain.
        match open(&store.dir, "stage", libc::O_RDONLY) {
            Ok(file) => {
                if !io(file.metadata())?.is_file() {
                    return Err(Failure::Storage);
                }
                unlink(&store.dir, "stage")?;
            }
            Err(_) => {
                let path = format!("/proc/self/fd/{}/stage", store.dir.as_raw_fd());
                if fs::symlink_metadata(path).is_ok() {
                    return Err(Failure::Storage);
                }
            }
        }
        store.used()?;
        Ok(store)
    }
    fn used(&self) -> Result<u64> {
        self.inventory().map(|(bytes, _)| bytes)
    }
    fn inventory(&self) -> Result<(u64, usize)> {
        let mut total = 0u64;
        let mut entries = 0usize;
        let mut objects = 0usize;
        for entry in io(fs::read_dir(format!(
            "/proc/self/fd/{}",
            self.dir.as_raw_fd()
        )))? {
            entries += 1;
            if entries > self.max_objects + 2 {
                return Err(Failure::Limit);
            }
            let entry = io(entry)?;
            let value = entry.file_name();
            let value = value.to_str().ok_or(Failure::Storage)?;
            if value == "lock" {
                continue;
            }
            if value != "stage" {
                if !hash(value) {
                    return Err(Failure::Storage);
                }
                objects += 1;
                if objects > self.max_objects {
                    return Err(Failure::Limit);
                }
            }
            let file = open(&self.dir, value, libc::O_RDONLY)?;
            let meta = io(file.metadata())?;
            if !meta.is_file() || meta.nlink() != 1 {
                return Err(Failure::Storage);
            }
            total = total.checked_add(meta.len()).ok_or(Failure::Limit)?;
        }
        if total > self.quota {
            return Err(Failure::Limit);
        }
        Ok((total, objects))
    }
    /// Upload never checks another scope's digest inventory. One pending stage.
    pub fn stage(&mut self, expected: &Identity, input: &mut impl Read) -> Result<StagedObject> {
        if !hash(&expected.sha256) || expected.bytes > MAX_BYTES {
            return Err(Failure::Invalid);
        }
        let (used, objects) = self.inventory()?;
        let existing = open(&self.dir, &expected.sha256, libc::O_RDONLY).is_ok();
        if (!existing && objects >= self.max_objects)
            || used
                .checked_add(expected.bytes)
                .is_none_or(|n| n > self.quota)
        {
            return Err(Failure::Limit);
        }
        let mut file = open(
            &self.dir,
            "stage",
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        let result = (|| {
            let mut remaining = input.take(expected.bytes + 1);
            let mut digest = Sha256::new();
            let mut count = 0;
            let mut buffer = [0u8; 8192];
            loop {
                let n = io(remaining.read(&mut buffer))?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                if count > expected.bytes {
                    return Err(Failure::Invalid);
                }
                io(file.write_all(&buffer[..n]))?;
                digest.update(&buffer[..n]);
            }
            if count != expected.bytes || hex::encode(digest.finalize()) != expected.sha256 {
                return Err(Failure::Invalid);
            }
            io(file.sync_all())?;
            io(self.dir.sync_all())?;
            Ok(StagedObject {
                identity: expected.clone(),
                key: self.key.clone(),
                file: io(file.try_clone())?,
            })
        })();
        if result.is_err() {
            let _ = unlink(&self.dir, "stage");
        }
        result
    }
    /// Explicitly cancel an upload without validation. A stale/foreign token
    /// cannot remove a replacement stage, even after this store was reopened.
    pub fn discard(&mut self, staged: StagedObject) -> Result<()> {
        self.discard_matching(&staged)
    }
    fn discard_matching(&self, staged: &StagedObject) -> Result<()> {
        if staged.key != self.key {
            return Err(Failure::Invalid);
        }
        let file = open(&self.dir, "stage", libc::O_RDONLY)?;
        let meta = io(file.metadata())?;
        let pinned = io(staged.file.metadata())?;
        if meta.dev() != pinned.dev() || meta.ino() != pinned.ino() {
            return Err(Failure::Invalid);
        }
        unlink(&self.dir, "stage")
    }
    /// Failed validation discards its stage immediately.
    pub fn validate(&mut self, staged: StagedObject, executable: &Path) -> Result<ValidatedObject> {
        self.validate_with(staged, |bytes, namespace| run(executable, bytes, namespace))
    }
    fn validate_with(
        &mut self,
        staged: StagedObject,
        worker: impl FnOnce(&[u8], &str) -> Result<()>,
    ) -> Result<ValidatedObject> {
        if staged.key != self.key {
            return Err(Failure::Invalid);
        }
        let result = self.install(&staged, worker);
        if result.is_err() {
            self.discard_matching(&staged)?;
        }
        result
    }
    fn install(
        &self,
        staged: &StagedObject,
        worker: impl FnOnce(&[u8], &str) -> Result<()>,
    ) -> Result<ValidatedObject> {
        let file = open(&self.dir, "stage", libc::O_RDONLY)?;
        let meta = io(file.metadata())?;
        let pinned = io(staged.file.metadata())?;
        if meta.dev() != pinned.dev() || meta.ino() != pinned.ino() {
            return Err(Failure::Invalid);
        }
        let bytes = read(file)?;
        if identity(&bytes) != staged.identity {
            return Err(Failure::Invalid);
        }
        worker(&bytes, &self.scope.namespace)?;
        let source = name("stage")?;
        let target = name(&staged.identity.sha256)?;
        // SAFETY: descriptor-relative atomic no-overwrite publication. Both paths
        // are fixed or digest-only, inside the exclusively locked scope directory.
        let installed = unsafe {
            libc::renameat2(
                self.dir.as_raw_fd(),
                source.as_ptr(),
                self.dir.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if installed != 0 {
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
                || read(open(&self.dir, &staged.identity.sha256, libc::O_RDONLY)?)? != bytes
            {
                return Err(Failure::Storage);
            }
            unlink(&self.dir, "stage")?;
        }
        io(self.dir.sync_all())?;
        Ok(ValidatedObject {
            identity: staged.identity.clone(),
            scope: self.scope.clone(),
        })
    }
    /// Scoped digest lookup revalidates after restart before issuing a token.
    pub fn lookup(&self, expected: &Identity, executable: &Path) -> Result<ValidatedObject> {
        if !hash(&expected.sha256) {
            return Err(Failure::Invalid);
        }
        let bytes = read(open(&self.dir, &expected.sha256, libc::O_RDONLY)?)?;
        if identity(&bytes) != *expected {
            return Err(Failure::Invalid);
        }
        run(executable, &bytes, &self.scope.namespace)?;
        Ok(ValidatedObject {
            identity: expected.clone(),
            scope: self.scope.clone(),
        })
    }
    pub fn read(&self, validated: &ValidatedObject) -> Result<Vec<u8>> {
        if validated.scope != self.scope {
            return Err(Failure::Invalid);
        }
        let bytes = read(open(&self.dir, &validated.identity.sha256, libc::O_RDONLY)?)?;
        if identity(&bytes) != validated.identity {
            return Err(Failure::Invalid);
        }
        Ok(bytes)
    }
}

pub(super) fn worker_limits() -> Result<()> {
    let memory = libc::rlimit {
        rlim_cur: 256 * 1024 * 1024,
        rlim_max: 256 * 1024 * 1024,
    };
    let cpu = libc::rlimit {
        rlim_cur: 2,
        rlim_max: 2,
    };
    let files = libc::rlimit {
        rlim_cur: 16,
        rlim_max: 16,
    };
    let processes = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: initialized limits; modifies only the disposable worker.
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &memory) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &files) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &processes) } != 0
        || unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
    {
        return Err(Failure::Worker);
    }
    Ok(())
}
fn run(executable: &Path, bytes: &[u8], namespace: &str) -> Result<()> {
    run_with_deadline(executable, bytes, namespace, Duration::from_secs(3))
}
fn run_with_deadline(
    executable: &Path,
    bytes: &[u8],
    namespace: &str,
    timeout: Duration,
) -> Result<()> {
    // Zero is the injected already-expired deadline used by deterministic tests.
    if timeout.is_zero() {
        return Err(Failure::Worker);
    }
    let mut command = Command::new(executable);
    command
        .args(["asi", "object-worker"])
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: only libc syscalls in the post-fork closure; no allocation or locks.
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0
                || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| Failure::Worker)?;
    let mut stdin = child.stdin.take().ok_or(Failure::Worker)?;
    let stdout = child.stdout.take().ok_or(Failure::Worker)?;
    let mut input = namespace.as_bytes().to_vec();
    input.push(b'\n');
    input.extend_from_slice(bytes);
    let (send, recv) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = (|| {
            stdin.write_all(&input)?;
            drop(stdin);
            let mut response = Vec::new();
            stdout.take(2).read_to_end(&mut response)?;
            Ok::<_, std::io::Error>(response)
        })();
        let _ = send.send(result);
    });
    let result = recv.recv_timeout(timeout);
    // Always kill before wait: EOF does not prove the worker has exited. A valid
    // response is the complete parser result; no post-response work is permitted.
    // SAFETY: child is unreaped, with a private process group established pre-exec.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = thread.join();
    match result {
        Ok(Ok(response)) if response == b"1" => Ok(()),
        _ => Err(Failure::Worker),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> Scope {
        Scope {
            tenant: "tenant".into(),
            destination: "receiver".into(),
            publisher: "publisher".into(),
            namespace: "origin".into(),
        }
    }
    #[test]
    fn scope_lock_releases_while_inherited_descriptor_remains_open() {
        let root = tempfile::tempdir().unwrap();
        crate::store::INHERITED_LOCKS.with(|capture| *capture.borrow_mut() = Some(Vec::new()));
        let store = ObjectStore::open(root.path(), scope(), MAX_BYTES).unwrap();
        let inherited =
            crate::store::INHERITED_LOCKS.with(|capture| capture.borrow_mut().take().unwrap());
        assert_eq!(inherited.len(), 1);
        assert!(ObjectStore::open(root.path(), scope(), MAX_BYTES).is_err());
        drop(store);
        let reopened = ObjectStore::open(root.path(), scope(), MAX_BYTES).unwrap();
        drop(reopened);
        drop(inherited);
    }
    #[test]
    fn stages_quota_recovery_scope_and_immutable_install() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"fixture";
        let id = identity(bytes);
        let mut store = ObjectStore::open(root.path(), scope(), MAX_BYTES).unwrap();
        let staged = store.stage(&id, &mut &bytes[..]).unwrap();
        assert!(store.stage(&id, &mut &bytes[..]).is_err());
        let token = store.validate_with(staged, |_, _| Ok(())).unwrap();
        assert_eq!(store.read(&token).unwrap(), bytes);
        let staged = store.stage(&id, &mut &bytes[..]).unwrap();
        store.validate_with(staged, |_, _| Ok(())).unwrap();
        assert!(store.stage(&id, &mut &b"changed"[..]).is_err());
        let mut foreign = scope();
        foreign.tenant = "other".into();
        let foreign = ObjectStore::open(root.path(), foreign, MAX_BYTES).unwrap();
        assert!(foreign.read(&token).is_err());
        assert!(foreign.lookup(&id, Path::new("/missing")).is_err());
        let staged = store.stage(&id, &mut &bytes[..]).unwrap();
        assert!(
            store
                .validate_with(staged, |_, _| Err(Failure::Worker))
                .is_err()
        );
        assert_eq!(store.used().unwrap(), bytes.len() as u64);
        let pending = store.stage(&id, &mut &bytes[..]).unwrap();
        store.discard(pending).unwrap();
        assert_eq!(store.used().unwrap(), bytes.len() as u64);
        let _crashed = store.stage(&id, &mut &bytes[..]).unwrap();
        drop(store);
        let mut store = ObjectStore::open(root.path(), scope(), MAX_BYTES).unwrap();
        assert_eq!(store.used().unwrap(), bytes.len() as u64);
        assert!(store.stage(&id, &mut &bytes[..]).is_ok());
    }
    #[test]
    fn stale_tokens_pin_unlinked_inodes_and_preserve_replacement_stage() {
        for validate_old in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut store = ObjectStore::open(root.path(), scope(), MAX_BYTES).unwrap();
            let id = identity(b"same bytes");
            let old = store.stage(&id, &mut &b"same bytes"[..]).unwrap();
            let old_inode = old.file.metadata().unwrap().ino();
            drop(store);
            let mut store = ObjectStore::open(root.path(), scope(), MAX_BYTES).unwrap();
            assert_eq!(old.file.metadata().unwrap().nlink(), 0);
            let replacement = store.stage(&id, &mut &b"same bytes"[..]).unwrap();
            assert_ne!(replacement.file.metadata().unwrap().ino(), old_inode);
            let result = if validate_old {
                store
                    .validate_with(old, |_, _| panic!("stale token reached worker"))
                    .map(|_| ())
            } else {
                store.discard(old)
            };
            assert_eq!(result, Err(Failure::Invalid));
            assert_eq!(store.used().unwrap(), id.bytes);
            let validated = store.validate_with(replacement, |_, _| Ok(())).unwrap();
            assert_eq!(store.read(&validated).unwrap(), b"same bytes");
        }
    }
    #[test]
    fn inventory_reserves_install_slot_and_reopens_at_ceiling() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ObjectStore::open_with_ceiling(root.path(), scope(), MAX_BYTES, 2).unwrap();
        for mut bytes in [b"one".as_slice(), b"two".as_slice()] {
            let staged = store.stage(&identity(bytes), &mut bytes).unwrap();
            store.validate_with(staged, |_, _| Ok(())).unwrap();
        }
        assert_eq!(store.inventory().unwrap().1, 2);
        assert!(matches!(
            store.stage(&identity(b"three"), &mut &b"three"[..]),
            Err(Failure::Limit)
        ));
        let staged = store.stage(&identity(b"one"), &mut &b"one"[..]).unwrap();
        store.validate_with(staged, |_, _| Ok(())).unwrap();
        drop(store);
        let store = ObjectStore::open_with_ceiling(root.path(), scope(), MAX_BYTES, 2).unwrap();
        assert_eq!(store.inventory().unwrap().1, 2);
        let mut rogue = open(
            &store.dir,
            &identity(b"three").sha256,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )
        .unwrap();
        rogue.write_all(b"three").unwrap();
        assert_eq!(store.inventory(), Err(Failure::Limit));
    }
    #[test]
    fn worker_death_and_injected_deadline_refuse() {
        assert_eq!(
            run(Path::new("/bin/false"), b"{}", "origin"),
            Err(Failure::Worker)
        );
        assert_eq!(
            run_with_deadline(Path::new("/missing"), b"{}", "origin", Duration::ZERO),
            Err(Failure::Worker)
        );
    }
    #[test]
    fn traversal_symlinks_and_pending_quota_refuse() {
        let root = tempfile::tempdir().unwrap();
        let mut invalid = scope();
        invalid.publisher = "../escape".into();
        assert!(ObjectStore::open(root.path(), invalid, MAX_BYTES).is_err());
        let link = root.path().join("link");
        std::os::unix::fs::symlink(root.path(), &link).unwrap();
        assert!(ObjectStore::open(&link, scope(), MAX_BYTES).is_err());
        let mut store = ObjectStore::open(root.path(), scope(), MAX_BYTES).unwrap();
        let large = vec![0; MAX_BYTES as usize];
        let id = identity(&large);
        let staged = store.stage(&id, &mut large.as_slice()).unwrap();
        assert_eq!(store.used().unwrap(), MAX_BYTES);
        assert!(matches!(
            store.stage(&identity(b"x"), &mut &b"x"[..]),
            Err(Failure::Limit)
        ));
        assert!(
            store
                .validate_with(staged, |_, _| Err(Failure::Worker))
                .is_err()
        );
    }
}
