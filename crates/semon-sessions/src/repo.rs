//! The repository a working directory belongs to.
//!
//! A linked git worktree (`git worktree add`, or a Claude Code worktree under
//! `.claude/worktrees/`) has a `.git` *file* holding
//! `gitdir: <main>/.git/worktrees/<name>`, not a `.git` directory. Its
//! sessions belong to the repository it was made from, so that is the name
//! given, wherever the repo is shown or filtered. Everything here is read from
//! the path text and bounded Git metadata: no git subprocess is run. External
//! common directories are read only through regular metadata files, rejecting
//! symbolic-link ancestors. Unavailable metadata keeps the textual fallback.

use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use crate::facts::RepoId;

/// The most of a `.git` file that is read. A real one is a line long.
const GIT_FILE_MAX: u64 = 4096;

/// The most of a Git `config` that is read for the origin.
const CONFIG_MAX: u64 = 64 * 1024;

/// The repository `cwd` is in, by name: the directory holding the nearest
/// `.git` below `home`. When that `.git` is a linked worktree's file, the
/// directory of the main checkout it names (or, for a bare repository, the
/// bare directory's name without `.git`), even when that checkout is gone.
/// External non-bare common directories use an explicit `core.worktree` when
/// present, otherwise the containing checkout's name: Git need not record the
/// main checkout for `--separate-git-dir`. Unavailable metadata keeps the
/// historical path-text fallback.
/// A `.git` file that names no worktree (a submodule's, or an unreadable
/// one) counts as the directory holding it. With no `.git` on the way up, a
/// path under `.claude/worktrees/` is named by what precedes it.
pub(crate) fn repo_of(cwd: &str, home: Option<&Path>) -> Option<String> {
    let mut path = Some(Path::new(cwd));
    while let Some(current) = path {
        if current == Path::new("/") || home == Some(current) {
            break;
        }
        let git = current.join(".git");
        match fs::symlink_metadata(&git) {
            // A symbolic link is never read: it may lead out of the tree.
            Ok(meta) if meta.is_file() => {
                return worktree_owner(current, &git, home)
                    .or_else(|| claude_owner(cwd))
                    .or_else(|| name(current));
            }
            _ if git.exists() => return name(current),
            _ => {}
        }
        path = current.parent();
    }
    claude_owner(cwd)
}

/// The sanitized `origin` identity of the repository `cwd` is in, read from
/// bounded Git metadata (no git subprocess). `None` when `cwd` is in no
/// repository; a repository without a usable `origin` is `Unknown`.
pub(crate) fn repo_id(cwd: &str, home: Option<&Path>) -> Option<RepoId> {
    for current in Path::new(cwd).ancestors() {
        if current == Path::new("/") || home == Some(current) {
            break;
        }
        let git = current.join(".git");
        let Ok(metadata) = fs::symlink_metadata(&git) else {
            continue;
        };
        let common = if metadata.is_dir() {
            git
        } else if metadata.is_file() {
            let pointer = read_metadata(&git, GIT_FILE_MAX)?;
            let target = pointer
                .lines()
                .find_map(|line| line.strip_prefix("gitdir:"))?
                .trim();
            let target = normalize(&current.join(target))?;
            match read_metadata(&target.join("commondir"), GIT_FILE_MAX)
                .and_then(|value| normalize(&target.join(value.trim())))
            {
                Some(common) => common,
                // A submodule's or `--separate-git-dir` repository's own gitdir.
                None => target,
            }
        } else {
            return None;
        };
        let origin = read_metadata(&common.join("config"), CONFIG_MAX)
            .and_then(|config| origin_url(&config))
            .map_or(RepoId::Unknown, |url| classify_origin(&url));
        return Some(origin);
    }
    None
}

/// The `url` of `[remote "origin"]` (also written `[remote.origin]`) in a Git
/// config text; the first one, as `git remote get-url origin` reads it. Section names
/// and keys are case-insensitive, a quoted subsection is exact. A header may
/// be followed by a comment or by an assignment, and a trailing backslash
/// continues a line.
fn origin_url(config: &str) -> Option<String> {
    let mut origin = false;
    for line in logical_lines(config.strip_prefix('\u{feff}').unwrap_or(config)) {
        let mut body = line.trim_start();
        if let Some(header) = body.strip_prefix('[') {
            let Some((is_origin, rest)) = origin_header(header) else {
                origin = false;
                continue;
            };
            origin = is_origin;
            body = rest.trim_start();
        }
        if origin
            && let Some((key, value)) = body.split_once('=')
            && key.trim().eq_ignore_ascii_case("url")
        {
            return config_value(value).filter(|url| !url.is_empty());
        }
    }
    None
}

/// Join continuations outside comments. Quote state survives a continuation;
/// a backslash in an unquoted comment never consumes the following line.
fn logical_lines(config: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pending = String::new();
    let mut quoted = false;
    for line in config.lines() {
        let mut escaped = false;
        let mut comment = false;
        for c in line.chars() {
            pending.push(c);
            if comment {
                continue;
            }
            if escaped {
                escaped = false;
            } else {
                match c {
                    '\\' => escaped = true,
                    '"' => quoted = !quoted,
                    '#' | ';' if !quoted => comment = true,
                    _ => {}
                }
            }
        }
        if escaped && !comment {
            pending.pop();
        } else {
            lines.push(std::mem::take(&mut pending));
            quoted = false;
        }
    }
    if !pending.is_empty() {
        lines.push(pending);
    }
    lines
}

/// Whether `header` (after the opening bracket) names `remote "origin"` and
/// what follows its closing bracket. `None` for a malformed header.
fn origin_header(header: &str) -> Option<(bool, &str)> {
    let name_end = header.find(|c: char| c.is_whitespace() || c == ']')?;
    let (name, tail) = header.split_at(name_end);
    if let Some(rest) = tail.strip_prefix(']') {
        // `[remote.origin]`: the subsection is lowercased.
        let origin = name.split_once('.').is_some_and(|(section, subsection)| {
            section.eq_ignore_ascii_case("remote") && subsection.eq_ignore_ascii_case("origin")
        });
        return Some((origin, rest));
    }
    let mut chars = tail.trim_start().strip_prefix('"')?.chars();
    let mut subsection = String::new();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => subsection.push(chars.next()?),
            other => subsection.push(other),
        }
    }
    let rest = chars.as_str().trim_start().strip_prefix(']')?;
    Some((
        name.eq_ignore_ascii_case("remote") && subsection == "origin",
        rest,
    ))
}

/// A Git config value: quotes group, `\n`, `\t`, `\b`, `\\` and `\"` escape, an
/// unquoted `#` or `;` starts a comment and unquoted outer whitespace is
/// dropped. `None` for an unterminated quote or an unknown escape.
fn config_value(raw: &str) -> Option<String> {
    let mut value = String::new();
    let mut spaces = String::new();
    let mut quoted = false;
    let mut chars = raw.trim_start().chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                value.push_str(&std::mem::take(&mut spaces));
                quoted = !quoted;
            }
            '\\' => {
                value.push_str(&std::mem::take(&mut spaces));
                value.push(match chars.next()? {
                    'n' => '\n',
                    't' => '\t',
                    'b' => '\u{8}',
                    escaped @ ('\\' | '"') => escaped,
                    _ => return None,
                });
            }
            '#' | ';' if !quoted => break,
            c if c.is_whitespace() && !quoted => spaces.push(c),
            c => {
                value.push_str(&std::mem::take(&mut spaces));
                value.push(c);
            }
        }
    }
    (!quoted).then_some(value)
}

/// A remote URL as a [`RepoId`]: credentials, scheme and `.git` removed, the
/// host lowercased, and the scheme's default port (https 443, http 80, ssh
/// 22, git 9418) dropped; any other port is kept. scp-style
/// `user@host:owner/repo` becomes `host/owner/repo`. `file:` URLs and local
/// paths are [`RepoId::Path`].
fn classify_origin(url: &str) -> RepoId {
    let scheme_url = url.split_once("://").filter(|(scheme, _)| {
        let mut bytes = scheme.bytes();
        bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
    });
    let (default_port, authority, path) = if let Some((scheme, rest)) = scheme_url {
        let default_port = match scheme.to_ascii_lowercase().as_str() {
            "file" => return RepoId::Path(url.to_owned()),
            "https" => Some("443"),
            "http" => Some("80"),
            "ssh" | "git+ssh" | "ssh+git" => Some("22"),
            "git" => Some("9418"),
            _ => None,
        };
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        match rest.split_once('/') {
            Some((authority, path)) => (default_port, authority, path),
            None => return RepoId::Unknown,
        }
    } else if url.starts_with("file:") {
        return RepoId::Path(url.to_owned());
    } else {
        // Git reads `a:b` as scp-style unless a slash comes before the colon.
        match url.split_once(':') {
            Some((host, path)) if !host.contains('/') => (None, host, path),
            _ => return RepoId::Path(url.to_owned()),
        }
    };
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = match host.rsplit_once(':') {
        Some((name, port)) if Some(port) == default_port => name,
        _ => host,
    };
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or(path)
        .trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty()
        || path.is_empty()
        || host.chars().any(|c| c.is_control() || c.is_whitespace())
        || path.chars().any(char::is_control)
    {
        return RepoId::Unknown;
    }
    RepoId::Remote(format!("{}/{path}", host.to_ascii_lowercase()))
}

fn claude_owner(cwd: &str) -> Option<String> {
    let (before, _) = cwd.split_once("/.claude/worktrees/")?;
    name(Path::new(before))
}

fn name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// The main repository named by the linked worktree's `.git` file `git`, in
/// `dir`. `None` when the file isn't a readable worktree pointer.
fn worktree_owner(dir: &Path, git: &Path, home: Option<&Path>) -> Option<String> {
    let mut text = Vec::new();
    crate::events::open_regular(git)?
        .take(GIT_FILE_MAX)
        .read_to_end(&mut text)
        .ok()?;
    let text = String::from_utf8(text).ok()?;
    let target = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    if target.is_empty() {
        return None;
    }
    let target = normalize(&dir.join(target))?;
    // `<common dir>/worktrees/<name>`
    let worktrees = target.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let recorded_common = read_metadata(&target.join("commondir"), GIT_FILE_MAX)
        .and_then(|value| normalize(&target.join(value.trim())));
    let common = recorded_common.as_deref().unwrap_or(worktrees.parent()?);
    let common_name = common.file_name()?.to_string_lossy();
    if common_name == ".git" {
        let owner = common.parent()?;
        return (home != Some(owner)).then(|| name(owner)).flatten();
    }
    // External common directories may instead belong to --separate-git-dir
    // checkouts. Do not label their linked worktrees after an unrelated storage
    // directory when Git explicitly says it is not bare.
    if let Some(config) = read_metadata(&common.join("config"), GIT_FILE_MAX) {
        let mut core = false;
        let mut bare = None;
        let mut checkout = None;
        for line in config.lines().map(str::trim) {
            if line.starts_with('[') {
                core = line.eq_ignore_ascii_case("[core]");
            } else if core && let Some((key, value)) = line.split_once('=') {
                match key.trim().to_ascii_lowercase().as_str() {
                    "bare" => {
                        bare = match value.trim().to_ascii_lowercase().as_str() {
                            "true" => Some(true),
                            "false" => Some(false),
                            _ => None,
                        }
                    }
                    "worktree" => {
                        let value = value.trim();
                        let decoded = if value.starts_with('"') {
                            serde_json::from_str::<String>(value).ok()
                        } else {
                            Some(value.to_owned())
                        };
                        checkout = decoded
                            .filter(|value| {
                                !value.is_empty() && !value.chars().any(char::is_control)
                            })
                            .and_then(|value| normalize(&common.join(value)));
                    }
                    _ => {}
                }
            }
        }
        if let Some(checkout) = checkout {
            return (home != Some(checkout.as_path()))
                .then(|| name(&checkout))
                .flatten();
        }
        if bare == Some(false) {
            return None;
        }
    }
    // A bare repository or historical unavailable metadata: `semon.git`.
    common_name
        .strip_suffix(".git")
        .filter(|stem| !stem.is_empty())
        .map(str::to_owned)
}

/// Only bounded regular metadata, with no symbolic-link ancestor. Files
/// named by untrusted pointers must not redirect reads through a symlink.
fn read_metadata(path: &Path, max: u64) -> Option<String> {
    if path.ancestors().any(|part| {
        fs::symlink_metadata(part)
            .ok()
            .is_some_and(|m| m.file_type().is_symlink())
    }) {
        return None;
    }
    let mut text = String::new();
    crate::events::open_regular(path)?
        .take(max + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= max).then_some(text)
}

/// `path` with `.` and `..` resolved by its text alone (no disk access, so no
/// symbolic link is followed). `None` when a `..` leaves the root.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A scratch directory, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-repo-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).unwrap();
            path
        }

        fn file(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// A `.git` file in `relative` pointing at `target` under this scratch.
        fn pointer(&self, relative: &str, target: &str) {
            self.dir(relative);
            self.file(
                &format!("{relative}/.git"),
                &format!("gitdir: {}\n", self.0.join(target).display()),
            );
        }

        fn repo(&self, relative: &str) -> Option<String> {
            repo_of(&self.0.join(relative).to_string_lossy(), None)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// `work/semon` with a linked worktree `work/semon-wt-x` and a nested
    /// `work/semon/.claude/worktrees/agent-1`.
    fn semon(scratch: &Scratch) {
        scratch.dir("work/semon/.git/worktrees/x");
        scratch.dir("work/semon/.git/worktrees/agent-1");
        scratch.dir("work/semon/src");
        scratch.dir("work/semon-wt-x/crates/deep");
        scratch.pointer("work/semon-wt-x", "work/semon/.git/worktrees/x");
        scratch.pointer(
            "work/semon/.claude/worktrees/agent-1",
            "work/semon/.git/worktrees/agent-1",
        );
    }

    #[test]
    fn a_main_checkout_is_its_own_repo() {
        let scratch = Scratch::new();
        semon(&scratch);
        assert_eq!(scratch.repo("work/semon").as_deref(), Some("semon"));
        assert_eq!(scratch.repo("work/semon/src").as_deref(), Some("semon"));
    }

    #[test]
    fn a_linked_worktree_belongs_to_its_main_checkout() {
        let scratch = Scratch::new();
        semon(&scratch);
        assert_eq!(scratch.repo("work/semon-wt-x").as_deref(), Some("semon"));
    }

    #[test]
    fn a_subdirectory_of_a_worktree_belongs_to_its_main_checkout() {
        let scratch = Scratch::new();
        semon(&scratch);
        assert_eq!(
            scratch.repo("work/semon-wt-x/crates/deep").as_deref(),
            Some("semon")
        );
    }

    #[test]
    fn a_claude_worktree_belongs_to_its_main_checkout() {
        let scratch = Scratch::new();
        semon(&scratch);
        assert_eq!(
            scratch
                .repo("work/semon/.claude/worktrees/agent-1")
                .as_deref(),
            Some("semon")
        );
        // Named by its pointer, not by the directory it sits under.
        scratch.pointer(
            "work/elsewhere/.claude/worktrees/agent-2",
            "work/semon/.git/worktrees/x",
        );
        assert_eq!(
            scratch
                .repo("work/elsewhere/.claude/worktrees/agent-2")
                .as_deref(),
            Some("semon")
        );
    }

    #[test]
    fn a_relative_gitdir_is_resolved_from_the_worktree() {
        let scratch = Scratch::new();
        semon(&scratch);
        scratch.dir("work/semon-wt-rel");
        scratch.file(
            "work/semon-wt-rel/.git",
            "gitdir: ../semon/.git/worktrees/x\n",
        );
        assert_eq!(scratch.repo("work/semon-wt-rel").as_deref(), Some("semon"));
    }

    #[test]
    fn a_worktree_whose_main_checkout_is_gone_keeps_the_repo_name() {
        let scratch = Scratch::new();
        scratch.pointer("work/semon-wt-gone", "missing/semon/.git/worktrees/gone");
        assert_eq!(scratch.repo("work/semon-wt-gone").as_deref(), Some("semon"));
    }

    #[test]
    fn a_bare_common_directory_names_the_repo_without_its_suffix() {
        let scratch = Scratch::new();
        scratch.dir("srv/semon.git/worktrees/x");
        scratch.pointer("work/semon-wt-bare", "srv/semon.git/worktrees/x");
        assert_eq!(scratch.repo("work/semon-wt-bare").as_deref(), Some("semon"));
    }

    #[test]
    fn an_unreadable_or_foreign_git_file_falls_back_to_its_directory() {
        let scratch = Scratch::new();
        // Not a pointer at all, a dangling pointer to no worktree, a
        // submodule's, and an empty one.
        for (dir, text) in [
            ("work/garbage", "not a pointer"),
            ("work/dangling", "gitdir: /nowhere\n"),
            ("work/submodule", "gitdir: ../.git/modules/submodule\n"),
            ("work/empty", "gitdir:\n"),
        ] {
            scratch.dir(dir);
            scratch.file(&format!("{dir}/.git"), text);
            assert_eq!(
                scratch.repo(dir).as_deref(),
                Path::new(dir).file_name().and_then(|name| name.to_str()),
                "{dir}"
            );
        }
    }

    #[test]
    fn a_claude_worktree_with_a_broken_pointer_names_the_owning_repo() {
        let scratch = Scratch::new();
        scratch.file("work/semon/.claude/worktrees/agent-x/.git", "garbage");
        assert_eq!(
            scratch
                .repo("work/semon/.claude/worktrees/agent-x")
                .as_deref(),
            Some("semon")
        );
    }

    #[test]
    fn a_worktree_does_not_name_home_as_its_repository() {
        let scratch = Scratch::new();
        let home = scratch.dir("home/user");
        scratch.pointer("work/linked", "home/user/.git/worktrees/linked");
        assert_eq!(
            repo_of(
                &scratch.0.join("work/linked").to_string_lossy(),
                Some(&home)
            )
            .as_deref(),
            Some("linked")
        );
    }

    #[test]
    fn only_the_start_of_a_git_file_is_read() {
        let scratch = Scratch::new();
        scratch.dir("work/big");
        let mut text = "x".repeat(GIT_FILE_MAX as usize + 10);
        text.push_str("\ngitdir: /a/semon/.git/worktrees/x\n");
        scratch.file("work/big/.git", &text);
        assert_eq!(scratch.repo("work/big").as_deref(), Some("big"));
    }

    #[cfg(unix)]
    #[test]
    fn a_git_file_behind_a_symbolic_link_is_not_read() {
        let scratch = Scratch::new();
        scratch.dir("work/linked");
        scratch.file("outside/pointer", "gitdir: /a/semon/.git/worktrees/x\n");
        std::os::unix::fs::symlink(
            scratch.0.join("outside/pointer"),
            scratch.0.join("work/linked/.git"),
        )
        .unwrap();
        assert_eq!(scratch.repo("work/linked").as_deref(), Some("linked"));
    }

    #[test]
    fn a_path_under_claude_worktrees_with_nothing_on_disk_names_what_precedes_it() {
        assert_eq!(
            repo_of("/nowhere/harbor/.claude/worktrees/agent-9/src", None).as_deref(),
            Some("harbor")
        );
        assert_eq!(repo_of("/nowhere/plain", None), None);
    }
    #[test]
    fn separate_git_directory_worktrees_do_not_inherit_storage_directory_name() {
        use std::process::Command;
        let scratch = Scratch::new();
        let checkout = scratch.0.join("actual-checkout");
        let gitdir = scratch.0.join("unrelated-storage.git");
        let linked = scratch.0.join("linked-checkout");
        let run = |args: &[&std::ffi::OsStr]| {
            let output = Command::new("git").args(args).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&[
            "init".as_ref(),
            "--quiet".as_ref(),
            "--separate-git-dir".as_ref(),
            gitdir.as_os_str(),
            checkout.as_os_str(),
        ]);
        run(&[
            "-C".as_ref(),
            checkout.as_os_str(),
            "-c".as_ref(),
            "user.name=Test".as_ref(),
            "-c".as_ref(),
            "user.email=test@example.invalid".as_ref(),
            "commit".as_ref(),
            "--quiet".as_ref(),
            "--allow-empty".as_ref(),
            "-m".as_ref(),
            "fixture".as_ref(),
        ]);
        run(&[
            "-C".as_ref(),
            checkout.as_os_str(),
            "worktree".as_ref(),
            "add".as_ref(),
            "--quiet".as_ref(),
            "--detach".as_ref(),
            linked.as_os_str(),
        ]);
        assert_eq!(
            repo_of(&checkout.to_string_lossy(), None).as_deref(),
            Some("actual-checkout")
        );
        // Git does not record the main checkout in this configuration, so use
        // the linked checkout, never the unrelated external storage basename.
        assert_eq!(
            repo_of(&linked.to_string_lossy(), None).as_deref(),
            Some("linked-checkout")
        );
        let mut config = fs::read_to_string(gitdir.join("config")).unwrap();
        config.push_str(&format!("\n[core]\nworktree = {}\n", checkout.display()));
        fs::write(gitdir.join("config"), config).unwrap();
        assert_eq!(
            repo_of(&linked.to_string_lossy(), None).as_deref(),
            Some("actual-checkout")
        );
    }
    #[cfg(unix)]
    #[test]
    fn external_metadata_does_not_follow_symlinks() {
        let scratch = Scratch::new();
        scratch.file("real/config", "[core]\nbare = false\n");
        scratch.dir("storage");
        std::os::unix::fs::symlink(
            scratch.0.join("real/config"),
            scratch.0.join("storage/config"),
        )
        .unwrap();
        assert!(read_metadata(&scratch.0.join("storage/config"), GIT_FILE_MAX).is_none());
        std::os::unix::fs::symlink(scratch.0.join("real"), scratch.0.join("alias")).unwrap();
        assert!(read_metadata(&scratch.0.join("alias/config"), GIT_FILE_MAX).is_none());
    }

    #[test]
    fn origin_urls_become_sanitized_remote_ids() {
        for (url, want) in [
            (
                "https://user:secret@GitHub.com/onsager-ai/semon.git",
                "github.com/onsager-ai/semon",
            ),
            (
                "https://github.com:8443/onsager-ai/semon",
                "github.com:8443/onsager-ai/semon",
            ),
            (
                "https://github.com:443/onsager-ai/semon.git",
                "github.com/onsager-ai/semon",
            ),
            (
                "ssh://git@github.com:22/onsager-ai/semon.git",
                "github.com/onsager-ai/semon",
            ),
            (
                "ssh://git@host.example:2222/team/app/",
                "host.example:2222/team/app",
            ),
            (
                "ssh://git@github.com:443/onsager-ai/semon",
                "github.com:443/onsager-ai/semon",
            ),
            (
                "https://github.com:22/onsager-ai/semon",
                "github.com:22/onsager-ai/semon",
            ),
            (
                "http://example.test:80/onsager-ai/semon.git",
                "example.test/onsager-ai/semon",
            ),
            (
                "http://example.test:443/onsager-ai/semon",
                "example.test:443/onsager-ai/semon",
            ),
            (
                "git://example.test:9418/onsager-ai/semon",
                "example.test/onsager-ai/semon",
            ),
            (
                "git://example.test/onsager-ai/semon",
                "example.test/onsager-ai/semon",
            ),
            (
                "git://example.test:9419/onsager-ai/semon",
                "example.test:9419/onsager-ai/semon",
            ),
            (
                "git@GitHub.com:onsager-ai/semon.git",
                "github.com/onsager-ai/semon",
            ),
        ] {
            assert_eq!(
                classify_origin(url),
                RepoId::Remote(want.to_owned()),
                "{url}"
            );
        }
    }

    #[test]
    fn remote_identity_drops_query_fragment_and_encoded_userinfo() {
        for url in [
            "https://a.test/team/app.git?access_token=private",
            "https://a.test/team/app.git#access_token=private",
            "https://user%40mail:secret%3Avalue@a.test/team/app.git?token=private#secret",
            "ssh://user%40mail:secret%2Fvalue@a.test/team/app.git#secret",
            "user%40mail@a.test:team/app.git",
            "user@a.test:team/app.git?access_token=private#secret",
        ] {
            assert_eq!(
                classify_origin(url),
                RepoId::Remote("a.test/team/app".into()),
                "{url}"
            );
        }
    }

    #[test]
    fn local_paths_with_url_like_text_remain_paths() {
        for path in [
            "./backup://private/repo.git",
            "/backup://private/repo.git",
            "dir/https://private/repo.git",
        ] {
            assert_eq!(classify_origin(path), RepoId::Path(path.into()));
        }
        assert_eq!(
            classify_origin("git+ssh://user@a.test/team/app.git"),
            RepoId::Remote("a.test/team/app".into())
        );
        assert_eq!(
            classify_origin("1host://team/app.git"),
            RepoId::Remote("1host/team/app".into()),
            "an invalid scheme follows Git's scp-style rule"
        );
    }

    #[test]
    fn local_origins_are_paths_and_unusable_ones_are_unknown() {
        for url in [
            "file:///srv/git/semon.git",
            "file:/srv/git/semon",
            "/srv/git/semon.git",
            "../semon",
            "./dir:with-colon",
        ] {
            assert_eq!(classify_origin(url), RepoId::Path(url.to_owned()), "{url}");
        }
        for url in ["https://github.com", "https://github.com/", "git@host:"] {
            assert_eq!(classify_origin(url), RepoId::Unknown, "{url}");
        }
    }

    #[test]
    fn a_repository_origin_is_read_from_its_git_config() {
        let scratch = Scratch::new();
        semon(&scratch);
        scratch.file("work/semon/.git/worktrees/x/commondir", "../..\n");
        scratch.file("work/semon/.git/worktrees/agent-1/commondir", "../..\n");
        let id = |relative: &str| repo_id(&scratch.0.join(relative).to_string_lossy(), None);
        assert_eq!(id("work/semon"), Some(RepoId::Unknown), "no config");
        scratch.file(
            "work/semon/.git/config",
            "[core]\n\turl = https://wrong.example/core\n[remote \"upstream\"]\n\turl = https://wrong.example/upstream\n[remote \"origin\"]\n\turl = https://user:token@GitHub.com/onsager-ai/semon.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        );
        let want = || Some(RepoId::Remote("github.com/onsager-ai/semon".into()));
        assert_eq!(id("work/semon"), want());
        assert_eq!(id("work/semon/src"), want());
        assert_eq!(id("work/semon-wt-x/crates/deep"), want(), "linked worktree");
        assert_eq!(id("work/semon/.claude/worktrees/agent-1"), want());
        scratch.file(
            "work/semon/.git/config",
            "[remote \"origin\"]\n\turl = /srv/git/semon.git\n",
        );
        assert_eq!(
            id("work/semon"),
            Some(RepoId::Path("/srv/git/semon.git".into()))
        );
        scratch.file("work/semon/.git/config", "[core]\n\tbare = false\n");
        assert_eq!(id("work/semon"), Some(RepoId::Unknown), "no origin");
        scratch.dir("elsewhere/plain");
        assert_eq!(id("elsewhere/plain"), None, "not in a repository");
    }

    #[test]
    fn scp_style_addresses_have_no_port_to_drop() {
        assert_eq!(
            classify_origin("git@example.test:22/team/app.git"),
            RepoId::Remote("example.test/22/team/app".into())
        );
        assert_eq!(
            classify_origin("example.test:team/app"),
            RepoId::Remote("example.test/team/app".into())
        );
    }

    #[test]
    fn git_config_syntax_is_read_as_git_reads_it() {
        let url = origin_url;
        let some = |value: &str| Some(value.to_owned());
        assert_eq!(
            url("[remote \"origin\"] # the origin\n\turl = https://a.test/x/y # c\n"),
            some("https://a.test/x/y")
        );
        assert_eq!(
            url("[remote \"origin\"]\n\turl = https://a.test/x/y ; c\n"),
            some("https://a.test/x/y")
        );
        assert_eq!(
            url("[REMOTE \"origin\"]\n\tURL = \"https://a.test/x#y\" # c\n"),
            some("https://a.test/x#y")
        );
        assert_eq!(
            url("[remote \"origin\"]\n\turl = \"a b\\\\c\"\n"),
            some("a b\\c")
        );
        assert_eq!(
            url("[remote \"Origin\"]\n\turl = https://a.test/other\n"),
            None,
            "subsections are case-sensitive"
        );
        assert_eq!(url("[remote]\n\turl = https://a.test/x/y\n"), None);
        assert_eq!(url("[remote \"origin\"]\n\turl = \"unterminated\n"), None);
        assert_eq!(
            url("[remote \"origin\"]\n\turl = https://a.test/\\\nx/y\n"),
            some("https://a.test/x/y"),
            "continuation"
        );
        assert_eq!(
            url("[remote \"origin\"] url = https://a.test/x/y # c\n"),
            some("https://a.test/x/y"),
            "assignment on the header line"
        );
        assert_eq!(
            url("[remote.origin]\n\turl = https://a.test/x/y\n"),
            some("https://a.test/x/y")
        );
        assert_eq!(
            url("[Remote.ORIGIN] ; c\n\turl = https://a.test/x/y\n"),
            some("https://a.test/x/y")
        );
        assert_eq!(url("[remote.upstream]\n\turl = https://a.test/x\n"), None);
        assert_eq!(
            url("[remote \"origin\"]\n\turl = https://a.test/first\n\turl = https://a.test/last\n"),
            some("https://a.test/first")
        );
    }

    #[test]
    fn repository_identity_uses_the_first_fetch_url_and_ignores_pushurl() {
        let scratch = Scratch::new();
        semon(&scratch);
        let cwd = scratch.0.join("work/semon");
        let expected = Some(RepoId::Remote("a.test/team/app".into()));
        for secondary in ["https://b.test/secondary.git", "https://c.test/changed.git"] {
            scratch.file(
                "work/semon/.git/config",
                &format!(
                    "[remote \"origin\"]\npushurl = https://push.test/other.git\nurl = https://a.test/team/app.git\n[remote \"origin\"]\nurl = {secondary}\npushurl = https://push.test/changed.git\n"
                ),
            );
            assert_eq!(repo_id(&cwd.to_string_lossy(), None), expected);
        }
        scratch.file(
            "work/semon/.git/config",
            "[remote \"origin\"]\npushurl = https://push.test/only.git\n",
        );
        assert_eq!(repo_id(&cwd.to_string_lossy(), None), Some(RepoId::Unknown));
    }

    #[test]
    fn leading_bom_preserves_first_fetch_url_selection() {
        assert_eq!(
            origin_url("\u{feff}[remote \"origin\"]\npushurl = https://push.test/other.git\nurl = https://a.test/first.git\nurl = https://b.test/second.git\n").as_deref(),
            Some("https://a.test/first.git")
        );
    }

    #[test]
    fn comment_backslashes_do_not_continue_into_the_next_section() {
        for comment in ["#", ";"] {
            let config = format!(
                "[remote \"origin\"]\nurl = https://a.test/x/y {comment} note \\\n[remote \"upstream\"]\nurl = https://b.test/other/repo\n"
            );
            assert_eq!(origin_url(&config).as_deref(), Some("https://a.test/x/y"));
        }
        assert_eq!(
            origin_url("[remote \"origin\"]\nurl = \"https://a.test/x#y\\\n/z\"\n").as_deref(),
            Some("https://a.test/x#y/z"),
            "quoted comment characters still permit continuation"
        );
        assert_eq!(
            origin_url("[remote \"origin\"]\nurl = \"https://a.test/x\\\\\" # note \\\n[remote \"upstream\"]\nurl = https://b.test/other/repo\n").as_deref(),
            Some("https://a.test/x\\"),
            "an escaped backslash does not escape the closing quote"
        );
    }

    #[test]
    fn submodule_and_separate_git_dir_repositories_use_their_own_origin() {
        let scratch = Scratch::new();
        scratch.file(
            "super/.git/config",
            "[remote \"origin\"]\n\turl = https://a.test/team/super\n",
        );
        scratch.file(
            "super/.git/modules/sub/config",
            "[remote \"origin\"]\n\turl = https://a.test/team/sub\n",
        );
        scratch.dir("super/sub");
        scratch.pointer("super/sub", "super/.git/modules/sub");
        scratch.file(
            "storage/app.git/config",
            "[remote \"origin\"]\n\turl = https://a.test/team/app\n",
        );
        scratch.dir("checkout");
        scratch.pointer("checkout", "storage/app.git");
        let id = |relative: &str| repo_id(&scratch.0.join(relative).to_string_lossy(), None);
        assert_eq!(
            id("super"),
            Some(RepoId::Remote("a.test/team/super".into()))
        );
        assert_eq!(
            id("super/sub"),
            Some(RepoId::Remote("a.test/team/sub".into()))
        );
        assert_eq!(
            id("checkout"),
            Some(RepoId::Remote("a.test/team/app".into()))
        );
    }
}
