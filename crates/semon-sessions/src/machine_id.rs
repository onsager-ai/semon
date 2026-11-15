//! The origin's persistent machine identity, independent of names and credentials.
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn valid(id: &str) -> bool {
    id.strip_prefix("machine-").is_some_and(|hex| {
        hex.len() == 16
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

pub fn load_or_create() -> io::Result<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::other("machine ID needs XDG_CONFIG_HOME or HOME"))?;
    load_or_create_at(&config.join("semon/machine-id"))
}

fn read(path: &Path) -> io::Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "machine ID {} must be a regular file; left unchanged",
            path.display()
        )));
    }
    use std::io::Read;
    let mut bytes = String::new();
    (&mut file).take(26).read_to_string(&mut bytes)?;
    let id = bytes.strip_suffix('\n').filter(|id| valid(id)).ok_or_else(|| io::Error::other(format!("malformed machine ID at {}; left unchanged; restore a valid machine ID before retrying", path.display())))?;
    Ok(id.into())
}

/// Publish a complete private file without replacing an existing identity, including
/// when two first-use processes race. A hard link atomically publishes the staged inode.
pub fn load_or_create_at(path: &Path) -> io::Result<String> {
    match read(path) {
        Ok(id) => return Ok(id),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("machine ID has no parent"))?;
    fs::create_dir_all(parent)?;
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    let seconds = u32::try_from(seconds)
        .map_err(|_| io::Error::other("machine ID timestamp exceeds 8 hex digits"))?;
    let mut random = [0u8; 4];
    getrandom::fill(&mut random)
        .map_err(|_| io::Error::other("machine ID secure randomness unavailable"))?;
    let id = format!("machine-{seconds:08x}{:08x}", u32::from_be_bytes(random));
    let stage = parent.join(format!(".machine-id-{}-{}", std::process::id(), &id[8..]));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&stage)?;
    let result = (|| {
        writeln!(file, "{id}")?;
        file.sync_all()?;
        match fs::hard_link(&stage, path) {
            Ok(()) => {
                fs::File::open(parent)?.sync_all()?;
                Ok(id)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => read(path),
            Err(e) => Err(e),
        }
    })();
    let _ = fs::remove_file(stage);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn create_load_and_refuse_malformed_without_overwrite() {
        let root = std::env::temp_dir().join(format!(
            "semon-machine-id-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("machine-id");
        let id = load_or_create_at(&path).unwrap();
        assert!(valid(&id));
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{id}\n"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_or_create_at(&path).unwrap(), id);
        fs::write(&path, "malformed\n").unwrap();
        assert!(
            load_or_create_at(&path)
                .unwrap_err()
                .to_string()
                .contains("left unchanged")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "malformed\n");
        fs::remove_dir_all(root).unwrap();
    }
}
