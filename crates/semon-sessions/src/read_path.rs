//! Descriptor-bound reads of input files. A path's directories are opened
//! one at a time without following links; replacing any of them afterwards
//! cannot redirect the read. The last open is nonblocking so a FIFO swapped
//! in after a scan is rejected by fstat rather than waiting for a writer.

use std::{
    fs::File,
    io,
    path::{Component, Path},
};

#[cfg(unix)]
pub(crate) fn open_input(path: &Path) -> io::Result<File> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    // macOS exposes these two system directories through root-owned aliases.
    // Expand only their fixed system targets, never a link inside an input root.
    #[cfg(target_os = "macos")]
    let expanded = if let Ok(tail) = path.strip_prefix("/tmp") {
        Path::new("/private/tmp").join(tail)
    } else if let Ok(tail) = path.strip_prefix("/var") {
        Path::new("/private/var").join(tail)
    } else {
        path.to_owned()
    };
    #[cfg(target_os = "macos")]
    let path = expanded.as_path();

    let mut directory = File::open(if path.is_absolute() { "/" } else { "." })?;
    let components: Vec<_> = path
        .components()
        .filter(|part| !matches!(part, Component::RootDir | Component::CurDir))
        .collect();
    if components.is_empty() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    for (index, component) in components.iter().enumerate() {
        let name = match component {
            Component::Normal(name) => *name,
            Component::ParentDir => std::ffi::OsStr::new(".."),
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        };
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let last = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if last { 0 } else { libc::O_DIRECTORY };
        // SAFETY: directory owns a live descriptor, name is NUL-terminated,
        // and no mode argument is needed because O_CREAT is not used.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returns a new descriptor owned here.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if last {
            return if metadata.is_file() {
                Ok(file)
            } else {
                Err(io::ErrorKind::InvalidInput.into())
            };
        }
        if !metadata.is_dir() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        directory = file;
    }
    unreachable!("a nonempty path has a final component")
}

#[cfg(not(unix))]
pub(crate) fn open_input(path: &Path) -> io::Result<File> {
    // No descriptor-relative no-follow implementation on this platform yet.
    // Refuse links in every component before opening, then check the handle.
    let mut prefix = std::path::PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        if std::fs::symlink_metadata(&prefix)?.file_type().is_symlink() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Read,
        os::unix::fs::symlink,
        time::{Duration, Instant},
    };

    #[test]
    fn no_input_read_follows_a_link_at_any_depth_or_blocks_on_a_fifo() {
        let root = std::env::temp_dir().join(format!("semon-read-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("machine/claude/projects")).unwrap();
        fs::write(root.join("outside"), b"private").unwrap();
        let log = root.join("machine/claude/projects/session.jsonl");
        fs::write(&log, b"inside").unwrap();
        let mut bound = open_input(&log).unwrap();
        fs::rename(root.join("machine"), root.join("old-machine")).unwrap();
        symlink(root.join("old-machine"), root.join("machine")).unwrap();
        assert!(
            open_input(&log).is_err(),
            "a swapped parent is never followed"
        );
        let mut text = String::new();
        bound.read_to_string(&mut text).unwrap();
        assert_eq!(
            text, "inside",
            "the already opened descriptor remains bound"
        );
        fs::remove_file(root.join("machine")).unwrap();
        fs::rename(root.join("old-machine"), root.join("machine")).unwrap();
        fs::remove_file(&log).unwrap();
        symlink(root.join("outside"), &log).unwrap();
        assert!(open_input(&log).is_err());
        fs::remove_file(&log).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&log)
                .status()
                .unwrap()
                .success()
        );
        let started = Instant::now();
        assert!(open_input(&log).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        fs::remove_dir_all(root).unwrap();
    }
}
