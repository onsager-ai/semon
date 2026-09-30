//! [`StateLock`]: one pusher per state file.

use std::{
    fs::{self, File, TryLockError},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

/// An exclusive OS advisory lock on `<state>.lock`, beside a push's state
/// file (`flock` on Unix, `LockFileEx` on Windows, through `std`). A push
/// holds it for as long as it runs, so a hand-started `semon push` and an
/// embedding app never write the same state, or send to the same receiver
/// from it, at once.
///
/// Work a stop leaves running (a request to the receiver, a facts
/// collection) holds it too, so it is released only when the last of that
/// work ends: a request within its 120 s timeout, a facts collection when
/// its scan does (it has no timeout). Until then a new
/// push gets a distinct "still finishing" error rather than overlapping
/// with it.
///
/// It is released when dropped, and by the OS when the process ends in any
/// way. The lock file itself stays: removing it could let two pushers lock
/// two different files of the same name. It names its holder's process and
/// host, for the next pusher's error.
#[derive(Debug)]
pub struct StateLock {
    file: File,
    path: PathBuf,
}

/// What the lock file says about its holder.
#[derive(Default)]
struct Holder {
    pid: Option<u32>,
    host: Option<String>,
    finishing: bool,
}

impl Holder {
    fn read(path: &Path) -> Self {
        let mut holder = Self::default();
        // Best-effort: Windows refuses to read a locked range.
        let Ok(text) = fs::read_to_string(path) else {
            return holder;
        };
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("pid", pid)) => holder.pid = pid.trim().parse().ok(),
                Some(("host", host)) => holder.host = Some(host.trim().to_owned()),
                _ if line.trim() == "finishing" => holder.finishing = true,
                _ => {}
            }
        }
        holder
    }

    fn describe(&self) -> String {
        match (self.pid, &self.host) {
            (Some(pid), Some(host)) if !host.is_empty() => format!(", process {pid} on {host}"),
            (Some(pid), _) => format!(", process {pid}"),
            _ => String::new(),
        }
    }
}

impl StateLock {
    /// The lock file for a state file.
    pub fn path_for(state: &Path) -> PathBuf {
        let mut name = state.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    }

    /// Takes the lock for `state`, or fails at once when another pusher,
    /// in this process or another, holds it, or when a stopped push is
    /// still finishing work it had started.
    pub fn acquire(state: &Path) -> crate::Result<Self> {
        let path = Self::path_for(state);
        let failed = |error: io::Error| format!("{}: {error}", path.display());
        if let Some(parent) = path.parent() {
            crate::create_private_dir(parent).map_err(failed)?;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(failed)?;
        match file.try_lock() {
            Ok(()) => {
                let lock = Self { file, path };
                lock.describe_holder(false);
                Ok(lock)
            }
            Err(TryLockError::WouldBlock) => {
                let holder = Holder::read(&path);
                if holder.finishing {
                    Err(format!(
                        "a stopped push with the state {} is still finishing a request or a \
                         facts collection it had started (it holds {}{}); try again shortly",
                        state.display(),
                        path.display(),
                        holder.describe()
                    ))
                } else {
                    Err(format!(
                        "another push is already running with the state {} (it holds {}{}); \
                         stop it first",
                        state.display(),
                        path.display(),
                        holder.describe()
                    ))
                }
            }
            Err(TryLockError::Error(error)) => Err(failed(error)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes who holds the lock, and whether its push has returned with
    /// work still running. Best-effort: it only shapes the next pusher's
    /// error.
    fn describe_holder(&self, finishing: bool) {
        let mut text = format!("pid {}\nhost {}\n", std::process::id(), hostname());
        if finishing {
            text.push_str("finishing\n");
        }
        let write = || -> io::Result<()> {
            let mut file = &self.file;
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(text.as_bytes())
        };
        let _ = write();
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        // Closing the file releases the lock too; this only makes it
        // explicit.
        let _ = self.file.unlock();
    }
}

/// A push's own hold on its [`StateLock`]. Work that must outlive a stop
/// takes a [`Hold::share`]; when the push drops its hold while such work
/// still runs, the lock file says the push is finishing.
pub(crate) struct Hold(Arc<StateLock>);

impl Hold {
    pub(crate) fn new(lock: StateLock) -> Self {
        Self(Arc::new(lock))
    }

    pub(crate) fn share(&self) -> Arc<StateLock> {
        Arc::clone(&self.0)
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) > 1 {
            self.0.describe_holder(true);
        }
    }
}

/// This machine's name, for the lock file: best-effort, one line.
fn hostname() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .find_map(|path| fs::read_to_string(path).ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .chars()
        .take(253)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("semon-push-lock-{}-{name}", std::process::id()))
            .join("push/state.json")
    }

    #[test]
    fn a_second_lock_fails_at_once_and_a_dropped_one_is_free_again() {
        let state = state_path("second");
        let first = StateLock::acquire(&state).unwrap();
        assert_eq!(first.path(), StateLock::path_for(&state));
        let error = StateLock::acquire(&state).unwrap_err();
        assert!(error.contains("already running"), "{error}");
        #[cfg(unix)]
        {
            assert!(
                error.contains(&format!("process {}", std::process::id())),
                "{error}"
            );
            let host = hostname();
            if !host.is_empty() {
                assert!(error.contains(&format!(" on {host})")), "{error}");
            }
        }
        drop(first);
        let again = StateLock::acquire(&state).unwrap();
        drop(again);
        let _ = fs::remove_dir_all(state.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn a_hold_shared_with_running_work_keeps_the_lock_and_says_so() {
        let state = state_path("hold");
        let hold = Hold::new(StateLock::acquire(&state).unwrap());
        let work = hold.share();
        drop(hold);
        let error = StateLock::acquire(&state).unwrap_err();
        #[cfg(unix)]
        assert!(error.contains("still finishing"), "{error}");
        drop(work);
        drop(StateLock::acquire(&state).unwrap());
        let _ = fs::remove_dir_all(state.parent().unwrap().parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_file_and_a_new_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let state = state_path("private");
        let lock = StateLock::acquire(&state).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(lock.path()), 0o600);
        assert_eq!(mode(state.parent().unwrap()), 0o700);
        drop(lock);
        let _ = fs::remove_dir_all(state.parent().unwrap().parent().unwrap());
    }
}
