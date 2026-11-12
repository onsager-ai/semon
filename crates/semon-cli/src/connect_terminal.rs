//! The controlling terminal is the only code/approval input channel.
use semon_push::{
    connect::Terminal,
    credentials::{Secret, Zeroizing},
};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::fd::AsRawFd,
    path::PathBuf,
};
type Result<T> = std::result::Result<T, String>;
struct Prompt {
    tty: File,
    reader: BufReader<File>,
    sources: Vec<PathBuf>,
}
struct Echo {
    fd: libc::c_int,
    original: libc::termios,
}
impl Drop for Echo {
    fn drop(&mut self) {
        // SAFETY: fd remains open for the lifetime of this scoped guard.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.original);
        }
    }
}
impl Prompt {
    fn line(&mut self) -> Result<String> {
        let mut line = String::new();
        loop {
            let bytes = self
                .reader
                .fill_buf()
                .map_err(|_| "cannot read controlling terminal")?;
            if bytes.is_empty() {
                return Err("controlling terminal closed".into());
            }
            let count = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |index| index + 1);
            if line.len() + count > 1024 {
                return Err("invalid terminal input".into());
            }
            let end = bytes[count - 1] == b'\n';
            line.push_str(
                std::str::from_utf8(&bytes[..count]).map_err(|_| "invalid terminal input")?,
            );
            self.reader.consume(count);
            if end {
                break;
            }
        }
        if line.len() > 1024 || !line.ends_with('\n') {
            return Err("invalid terminal input".into());
        }
        Ok(line.trim().into())
    }
}
impl Terminal for Prompt {
    fn code(&mut self) -> Result<Secret> {
        self.reader = BufReader::new(
            self.tty
                .try_clone()
                .map_err(|_| "cannot clear terminal buffer")?,
        );
        write!(self.tty, "Receiver pairing code: ").map_err(|e| e.to_string())?;
        self.tty.flush().map_err(|e| e.to_string())?;
        let code = read_code(self.tty.as_raw_fd());
        writeln!(self.tty).map_err(|e| e.to_string())?;
        code
    }

    fn approve(&mut self, receiver: &str, receiver_id: &str, machine: &str) -> Result<bool> {
        writeln!(self.tty, "Receiver: {receiver}\nAuthenticated receiver identity: {receiver_id}\nMachine: {machine}\nEnabled kinds: mirror_push\nThis grants the receiver operator custody of redacted native logs, plain source names/paths and machine facts (hostname, processes, locks and repositories). Redaction is best effort. Analytics/body and issuer authority are not enabled.\nSelected current sources:").map_err(|e| e.to_string())?;
        for (index, source) in self.sources.iter().enumerate() {
            writeln!(self.tty, "  {}. {:?}", index + 1, source).map_err(|e| e.to_string())?;
        }
        writeln!(
            self.tty,
            "Select source numbers separated by commas (Enter selects all, none selects no files):"
        )
        .map_err(|e| e.to_string())?;
        self.tty.flush().map_err(|e| e.to_string())?;
        let selection = self.line()?;
        if selection == "none" {
            self.sources.clear();
        } else if !selection.is_empty() {
            let indices = selection
                .split(',')
                .map(|value| {
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "invalid source selection".to_owned())
                })
                .collect::<Result<Vec<_>>>()?;
            if indices
                .iter()
                .any(|index| *index == 0 || *index > self.sources.len())
            {
                return Err("invalid source selection".into());
            }
            self.sources = self
                .sources
                .iter()
                .enumerate()
                .filter(|(index, _)| indices.contains(&(index + 1)))
                .map(|(_, source)| source.clone())
                .collect();
        }
        writeln!(self.tty, "Approved source selection:").map_err(|e| e.to_string())?;
        for source in &self.sources {
            writeln!(self.tty, "  {:?}", source).map_err(|e| e.to_string())?;
        }
        writeln!(self.tty, "Future sources require another explicit consent update.\nEnter yes to approve, anything else to keep sources local: ").map_err(|e| e.to_string())?;
        self.tty.flush().map_err(|e| e.to_string())?;
        Ok(self.line()? == "yes")
    }
    fn sources(&self) -> Vec<PathBuf> {
        self.sources.clone()
    }
}
pub(super) fn run(endpoint: &str, machine: &str) -> Result<()> {
    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty").map_err(|_| "connect requires a controlling terminal; codes cannot be read from argv, environment or stdin")?;
    let options = semon_sessions::Options::default();
    let sources = semon_sessions::inputs(&options)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|input| input.full_path(&options))
        .collect();
    let mut prompt = Prompt {
        reader: BufReader::new(tty.try_clone().map_err(|e| e.to_string())?),
        tty,
        sources,
    };
    let name = semon_push::connect::connect(
        endpoint,
        machine,
        &mut prompt,
        &semon_push::origin_custody::default_directory()?,
        &|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        },
    )?;
    println!("Connected. Receiver name: {name}\nRun semon push --receiver {name} [--watch]");
    Ok(())
}

static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn interrupted(_: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
}
struct Signals(Vec<(libc::c_int, libc::sigaction)>);
impl Drop for Signals {
    fn drop(&mut self) {
        for (signal, action) in &self.0 {
            unsafe {
                libc::sigaction(*signal, action, std::ptr::null_mut());
            }
        }
    }
}
fn read_code(fd: libc::c_int) -> Result<Secret> {
    read_code_ready(fd, || {})
}
fn read_code_ready(fd: libc::c_int, ready: impl FnOnce()) -> Result<Secret> {
    INTERRUPTED.store(false, std::sync::atomic::Ordering::Relaxed);
    let mut signals = Signals(vec![]);
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: initialized action structures and valid signal numbers.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = interrupted as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            let mut old = std::mem::zeroed();
            if libc::sigaction(signal, &action, &mut old) != 0 {
                return Err("cannot protect terminal prompt".into());
            }
            signals.0.push((signal, old));
        }
    }
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr initializes original on success.
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
        return Err("connect requires a controlling terminal".into());
    }
    let original = unsafe { original.assume_init() };
    let mut hidden = original;
    hidden.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ISIG | libc::ICANON);
    hidden.c_cc[libc::VMIN] = 1;
    hidden.c_cc[libc::VTIME] = 0;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
        return Err("cannot disable terminal echo".into());
    }
    let _echo = Echo { fd, original };
    ready();
    let mut bytes = Zeroizing::new([0u8; 128]);
    let mut used = 0;
    loop {
        if INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("pairing interrupted; nothing installed".into());
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one writable pollfd; timeout bounds signal response.
        let ready = unsafe { libc::poll(&mut poll, 1, 100) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err("cannot read terminal".into());
        }
        if ready == 0 {
            continue;
        }
        if used == bytes.len() {
            return Err("pairing code too long".into());
        }
        // SAFETY: read writes one byte within the fixed zeroizing buffer.
        let count = unsafe { libc::read(fd, bytes[used..].as_mut_ptr().cast(), 1) };
        if count != 1 {
            return Err("controlling terminal closed".into());
        }
        match bytes[used] {
            3 | 4 | 26 => return Err("pairing interrupted; nothing installed".into()),
            b'\n' | b'\r' => break,
            8 | 127 if used > 0 => {
                bytes[used] = 0;
                used -= 1;
                bytes[used] = 0;
            }
            _ => used += 1,
        }
    }
    let text = std::str::from_utf8(&bytes[..used]).map_err(|_| "invalid pairing code")?;
    semon_push::pairing::parse_code(text)
        .map_err(|_| "invalid pairing code/check digit; no attempt submitted".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;
    #[test]
    fn pty_ctrl_c_restores_echo_without_echoing_secret() {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let fd = slave.as_raw_fd();
        let mut before = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(unsafe { libc::tcgetattr(fd, before.as_mut_ptr()) }, 0);
        let before = unsafe { before.assume_init() };
        let (ready, started) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || read_code_ready(fd, || ready.send(()).unwrap()));
        started.recv().unwrap();
        let data = b"private-code\x03";
        assert_eq!(
            unsafe { libc::write(master.as_raw_fd(), data.as_ptr().cast(), data.len()) },
            data.len() as isize
        );
        assert!(thread.join().unwrap().is_err());
        let mut after = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(unsafe { libc::tcgetattr(fd, after.as_mut_ptr()) }, 0);
        let after = unsafe { after.assume_init() };
        assert_eq!(after.c_lflag, before.c_lflag);
        assert!(read_code_ready(fd, || interrupted(libc::SIGTERM)).is_err());
        assert!(read_code_ready(fd, || interrupted(libc::SIGHUP)).is_err());
        assert!(
            std::panic::catch_unwind(|| read_code_ready(fd, || panic!("injected prompt panic")))
                .is_err()
        );
        let mut restored = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(unsafe { libc::tcgetattr(fd, restored.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { restored.assume_init() }.c_lflag, before.c_lflag);
        let mut poll = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut poll, 1, 0) },
            0,
            "code must not be echoed"
        );
        drop(slave);
    }
}
