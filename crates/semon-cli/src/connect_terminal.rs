//! The controlling terminal is the only code/approval input channel.
use semon_push::{connect::Terminal, credentials::Secret};
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
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
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
