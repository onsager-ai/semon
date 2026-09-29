//! A receiver's bearer tokens: `DIR/tokens`, mode 0600 in a 0700 directory,
//! one line per machine, `NAME SHA256(token)`. Only the hash is kept; the
//! token itself is shown once, when it is made. A request's token decides
//! its machine.
//!
//! [`Tokens`] answers lookups from that file and reads it again whenever it
//! changes on disk, so a revoked token stops working without a restart.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use sha2::{Digest, Sha256};

use crate::mirror::is_machine_name;

/// The token file, in the receiver's directory.
pub const TOKENS_FILE: &str = "tokens";

const HEADER: &str = "# semon receive: one machine per line, NAME SHA256(token) in hex. Edit with `semon receive token`.\n";

/// The longest token a request may present.
const TOKEN_MAX: usize = 512;

/// One machine's token, by its hash.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    hash: [u8; 32],
}

/// `DIR/tokens`.
pub fn tokens_path(dir: &Path) -> PathBuf {
    dir.join(TOKENS_FILE)
}

/// Makes a token for machine `name`, stores its hash in `DIR/tokens`, and
/// returns the token (32 random bytes, base64url without padding). A name
/// that already has a token is refused: revoke it first.
pub fn add_token(dir: &Path, name: &str) -> Result<String, String> {
    if !is_machine_name(name) {
        return Err(format!(
            "{name:?} is not a machine name: use 1 to 63 of a-z, 0-9 and -"
        ));
    }
    let mut entries = read_entries(&tokens_path(dir))?;
    if entries.iter().any(|entry| entry.name == name) {
        return Err(format!(
            "{name} already has a token; revoke it first (semon receive token revoke {name} --dir {})",
            dir.display()
        ));
    }
    let mut secret = [0u8; 32];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut secret)
        .map_err(|_| "the system's random source failed".to_owned())?;
    let token = base64url(&secret);
    entries.push(Entry {
        name: name.to_owned(),
        hash: hash(&token),
    });
    write_entries(dir, &entries)?;
    Ok(token)
}

/// Removes machine `name`'s token.
pub fn revoke_token(dir: &Path, name: &str) -> Result<(), String> {
    let mut entries = read_entries(&tokens_path(dir))?;
    let before = entries.len();
    entries.retain(|entry| entry.name != name);
    if entries.len() == before {
        return Err(format!(
            "{name} has no token in {}",
            tokens_path(dir).display()
        ));
    }
    write_entries(dir, &entries)
}

/// The machines that have a token, in the file's order.
pub fn token_names(dir: &Path) -> Result<Vec<String>, String> {
    Ok(read_entries(&tokens_path(dir))?
        .into_iter()
        .map(|entry| entry.name)
        .collect())
}

/// Refuses a token file anyone but its owner can read or write.
pub fn check_tokens_mode(dir: &Path) -> Result<(), String> {
    let path = tokens_path(dir);
    let meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!(
                "{} is mode {mode:03o}; the token file must be 0600 (chmod 600 it)",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    Ok(())
}

/// What the token file looked like when it was last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64),
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            inode: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino())
            },
        }
    }
}

struct Loaded {
    stamp: Option<Stamp>,
    entries: Vec<Entry>,
}

/// The live token set of a receiver's directory.
pub struct Tokens {
    path: PathBuf,
    loaded: Mutex<Loaded>,
}

impl Tokens {
    /// Reads `DIR/tokens` (a missing file is no tokens).
    pub fn open(dir: &Path) -> Result<Self, String> {
        let path = tokens_path(dir);
        let stamp = fs::metadata(&path).ok().map(|meta| Stamp::of(&meta));
        let entries = read_entries(&path)?;
        Ok(Self {
            path,
            loaded: Mutex::new(Loaded { stamp, entries }),
        })
    }

    /// How many machines have a token now.
    pub fn len(&self) -> usize {
        self.refresh().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The machine whose token `token` is, if any. Every stored hash is
    /// compared, each in constant time.
    pub fn machine(&self, token: &str) -> Option<String> {
        if token.is_empty() || token.len() > TOKEN_MAX {
            return None;
        }
        let presented = hash(token);
        let loaded = self.refresh();
        let mut found = None;
        for entry in &loaded.entries {
            if constant_time_eq(&entry.hash, &presented) {
                found = Some(entry.name.clone());
            }
        }
        found
    }

    /// The machine an `Authorization` header's bearer token names, if any.
    pub fn authorize(&self, header: Option<&str>) -> Option<String> {
        let header = header?.trim();
        let (scheme, token) = header.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("bearer") {
            return None;
        }
        self.machine(token.trim())
    }

    /// The entries, read again first when the file changed since. A file
    /// that can't be read or parsed then holds no tokens until it changes
    /// again, so a broken edit locks everyone out rather than in.
    fn refresh(&self) -> std::sync::MutexGuard<'_, Loaded> {
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        let stamp = fs::metadata(&self.path).ok().map(|meta| Stamp::of(&meta));
        if stamp != loaded.stamp {
            loaded.stamp = stamp;
            loaded.entries = match read_entries(&self.path) {
                Ok(entries) => entries,
                Err(error) => {
                    eprintln!("semon receive: {error}; no token is accepted until it is fixed");
                    Vec::new()
                }
            };
        }
        loaded
    }
}

fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let difference = left
        .iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b));
    std::hint::black_box(difference) == 0
}

fn read_entries(path: &Path) -> Result<Vec<Entry>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut entries: Vec<Entry> = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || format!("{}:{}: expected NAME SHA256", path.display(), number + 1);
        let mut fields = line.split_whitespace();
        let (Some(name), Some(digest), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err(bad());
        };
        let hash: [u8; 32] = hex::decode(digest)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(bad)?;
        if !is_machine_name(name) || entries.iter().any(|entry| entry.name == name) {
            return Err(bad());
        }
        entries.push(Entry {
            name: name.to_owned(),
            hash,
        });
    }
    Ok(entries)
}

/// Writes the token file, 0600, atomically, with `dir` made 0700.
fn write_entries(dir: &Path, entries: &[Entry]) -> Result<(), String> {
    let mut text = String::from(HEADER);
    for entry in entries {
        text.push_str(&format!("{} {}\n", entry.name, hex::encode(entry.hash)));
    }
    let path = tokens_path(dir);
    crate::write_private(&path, text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// base64url without padding (RFC 4648 §5).
fn base64url(bytes: &[u8]) -> String {
    crate::wire::base64_encode(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "semon-tokens-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_token_is_shown_once_stored_hashed_and_names_its_machine() {
        let dir = temp_dir("add");
        let token = add_token(&dir, "laptop").unwrap();
        assert_eq!(token.len(), 43, "32 bytes, base64url without padding");
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
        let stored = fs::read_to_string(tokens_path(&dir)).unwrap();
        assert!(!stored.contains(&token), "only the hash is stored");
        assert!(stored.contains(&hex::encode(hash(&token))));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&tokens_path(&dir)), 0o600);
            assert_eq!(mode(&dir), 0o700);
        }
        let other = add_token(&dir, "desk").unwrap();
        assert_ne!(token, other);
        assert!(add_token(&dir, "laptop").is_err(), "one token per machine");
        let long = "x".repeat(64);
        for bad in ["", "Laptop", "a_b", "a/b", "..", long.as_str()] {
            assert!(add_token(&dir, bad).is_err(), "{bad:?}");
        }
        assert_eq!(token_names(&dir).unwrap(), ["laptop", "desk"]);

        let tokens = Tokens::open(&dir).unwrap();
        assert_eq!(tokens.machine(&token).as_deref(), Some("laptop"));
        assert_eq!(
            tokens
                .authorize(Some(&format!("Bearer {other}")))
                .as_deref(),
            Some("desk")
        );
        assert_eq!(
            tokens
                .authorize(Some(&format!("bearer {other}")))
                .as_deref(),
            Some("desk")
        );
        assert_eq!(tokens.authorize(Some(&format!("Basic {other}"))), None);
        assert_eq!(tokens.authorize(Some("Bearer")), None);
        assert_eq!(tokens.authorize(None), None);
        assert_eq!(tokens.machine("not-a-token"), None);
        assert_eq!(tokens.machine(""), None);

        // Revoking takes effect for a running receiver, without a restart.
        revoke_token(&dir, "laptop").unwrap();
        assert_eq!(tokens.machine(&token), None);
        assert_eq!(tokens.machine(&other).as_deref(), Some("desk"));
        assert!(revoke_token(&dir, "laptop").is_err());
        assert_eq!(token_names(&dir).unwrap(), ["desk"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_token_file_accepts_nobody() {
        let dir = temp_dir("broken");
        let token = add_token(&dir, "laptop").unwrap();
        let tokens = Tokens::open(&dir).unwrap();
        assert_eq!(tokens.len(), 1);
        fs::write(tokens_path(&dir), "laptop not-hex\n").unwrap();
        assert_eq!(tokens.machine(&token), None);
        assert!(Tokens::open(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_token_file_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        add_token(&dir, "laptop").unwrap();
        check_tokens_mode(&dir).unwrap();
        fs::set_permissions(tokens_path(&dir), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check_tokens_mode(&dir).unwrap_err().contains("0600"));
        let _ = fs::remove_dir_all(&dir);
    }
}
