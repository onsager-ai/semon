use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};

use super::Failure;

/// Lock files deliberately remain: unlinking a locked inode breaks cross-process
/// admission. Locks are released by the kernel on exit, including SIGKILL.
pub(super) struct Slot {
    file: File,
}

impl Slot {
    pub(super) fn acquire(root: &Path) -> Result<Self, Failure> {
        for index in 0..4 {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(root.join("analytics").join(format!("query-{index}.lock")))
                .map_err(|_| Failure::unavailable())?;
            // SAFETY: file owns the valid descriptor throughout the lock lifetime.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self { file });
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(Failure::unavailable());
            }
        }
        Err(Failure::new(
            "store_busy",
            "Four queries already running for this store",
            "concurrency",
        ))
    }
}

pub(super) fn configure(command: &mut Command, timeout_ms: u64, slot: &Slot) {
    let lock_fd = slot.file.as_raw_fd();
    #[cfg(target_os = "linux")]
    let parent_pid = std::process::id() as libc::pid_t;
    // SAFETY: the closure only makes libc syscalls after fork. The caller keeps
    // slot alive until the child exits; only the child's descriptor flags change.
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            parent_death_signal(parent_pid)?;
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(lock_fd, libc::F_GETFD);
            if flags == -1 || libc::fcntl(lock_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            limits(timeout_ms)
        });
    }
}

#[cfg(target_os = "linux")]
fn parent_death_signal(parent_pid: libc::pid_t) -> std::io::Result<()> {
    // SAFETY: register before checking the parent so death on either side of
    // prctl is covered. _exit avoids unwinding/allocating in the forked child.
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::getppid() != parent_pid {
            libc::_exit(1);
        }
    }
    Ok(())
}

pub(super) fn kill(child: &mut Child) {
    // SAFETY: configure puts this child in a private process group before exec.
    // Kill the group while its leader remains our unreaped child, preventing PID
    // reuse and closing pipes held by any helper in the same group.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

pub(super) fn limits(timeout_ms: u64) -> std::io::Result<()> {
    let address_space = libc::rlimit {
        rlim_cur: 2 * 1024 * 1024 * 1024,
        rlim_max: 2 * 1024 * 1024 * 1024,
    };
    let seconds = timeout_ms.div_ceil(1000) + 1;
    let cpu = libc::rlimit {
        rlim_cur: seconds as libc::rlim_t,
        rlim_max: (seconds + 1) as libc::rlim_t,
    };
    // SAFETY: both pointers refer to initialized rlimit structs; no owned memory
    // or process-global permission changes are involved.
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &address_space) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
