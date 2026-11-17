use std::{
    env,
    path::{Path, PathBuf},
};

/// Semon's own state directory: `$XDG_STATE_HOME/semon`, or
/// `~/.local/state/semon`.
pub(super) fn default_state_dir(home: &Path) -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"))
        .join("semon")
}

/// [`default_state_dir`] for this process's `$HOME`.
pub(crate) fn state_dir() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    default_state_dir(&home)
}

pub(super) fn native_homes(
    home: &Path,
    claude_config: Option<&std::ffi::OsStr>,
    codex_config: Option<&std::ffi::OsStr>,
) -> (PathBuf, PathBuf, PathBuf) {
    let claude_config = claude_config.filter(|root| !root.is_empty());
    let (claude, settings) = match claude_config {
        Some(root) => (
            PathBuf::from(root),
            PathBuf::from(root).join(".claude.json"),
        ),
        None => (home.join(".claude"), home.join(".claude.json")),
    };
    let codex = codex_config
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    (claude, settings, codex)
}
