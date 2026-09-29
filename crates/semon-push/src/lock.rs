//! [`StateLock`]: one pusher per state file.

use std::{
    fs::{self, File, TryLockError},
    io::{self, Write},
    path::{Path, PathBuf},
};

/// An exclusive OS advisory lock on `<state>.lock`, beside a push's state
/// file (`flock` on Unix, `LockFileEx` on Windows, through `std`). A push
/// holds it for as long as it runs, so a hand-started `semon push` and an
/// embedding app never write the same state, or send to the same receiver
/// from it, at once.
///
/// It is released when dropped, and by the OS when the process ends in any
/// way. The lock file itself stays: removing it could let two pushers lock
/// two different files of the same name.
#[derive(Debug)]
pub struct StateLock {
    file: File,
    path: PathBuf,
}

impl StateLock {
    /// The lock file for a state file.
    pub fn path_for(state: &Path) -> PathBuf {
        let mut name = state.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    }

    /// Takes the lock for `state`, or fails at once when another pusher,
    /// in this process or another, holds it.
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
        let mut file = options.open(&path).map_err(failed)?;
        match file.try_lock() {
            Ok(()) => {
                // Who holds it, for the next pusher's error. Best-effort.
                let _ = file
                    .set_len(0)
                    .and_then(|()| writeln!(file, "{}", std::process::id()));
                Ok(Self { file, path })
            }
            Err(TryLockError::WouldBlock) => {
                let holder = fs::read_to_string(&path)
                    .ok()
                    .and_then(|pid| pid.trim().parse::<u32>().ok())
                    .map(|pid| format!(", process {pid}"))
                    .unwrap_or_default();
                Err(format!(
                    "another push is already running with the state {} (it holds {}{holder}); \
                     stop it first",
                    state.display(),
                    path.display()
                ))
            }
            Err(TryLockError::Error(error)) => Err(failed(error)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        // Closing the file releases the lock too; this only makes it
        // explicit.
        let _ = self.file.unlock();
    }
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
        assert!(
            error.contains(&format!("process {}", std::process::id())),
            "{error}"
        );
        drop(first);
        let again = StateLock::acquire(&state).unwrap();
        drop(again);
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
