//! The controlling terminal is the only code/approval input channel.
use semon_push::{connect::Terminal, credentials::Secret};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
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
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}
impl Prompt {
    fn line(&mut self) -> Result<String> {
        let mut line = String::new();
        self.reader
            .by_ref()
            .take(1025)
            .read_line(&mut line)
            .map_err(|_| "cannot read controlling terminal")?;
        if line.len() > 1024 || !line.ends_with('\n') {
            return Err("invalid terminal input".into());
        }
        Ok(line.trim().into())
    }
}
impl Terminal for Prompt {
    fn code(&mut self) -> Result<Secret> {
        let fd = self.tty.as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initializes the provided termios on success.
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err("connect requires a controlling terminal".into());
        }
        // SAFETY: successful tcgetattr initialized the value.
        let original = unsafe { original.assume_init() };
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ECHONL);
        // SAFETY: fd is an open tty and hidden is initialized.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
            return Err("cannot disable terminal echo".into());
        }
        let guard = Echo { fd, original };
        write!(self.tty, "Receiver pairing code: ").map_err(|e| e.to_string())?;
        self.tty.flush().map_err(|e| e.to_string())?;
        let code = self.line().map(Secret::new);
        drop(guard);
        writeln!(self.tty).map_err(|e| e.to_string())?;
        code
    }
    fn approve(&mut self, receiver: &str, receiver_id: &str, machine: &str) -> Result<bool> {
        writeln!(self.tty, "Receiver: {receiver}\nAuthenticated receiver identity: {receiver_id}\nMachine: {machine}\nEnabled kinds: mirror_push\nThis grants the receiver operator custody of redacted native logs, plain source names/paths and machine facts (hostname, processes, locks and repositories). Redaction is best effort. Analytics/body and issuer authority are not enabled.\nSelected current sources:").map_err(|e| e.to_string())?;
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
        &semon_push::origin_custody::default_directory(),
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
