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

/// The most of a `.git` file that is read. A real one is a line long.
const GIT_FILE_MAX: u64 = 4096;

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
    let recorded_common = read_metadata(&target.join("commondir"))
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
    if let Some(config) = read_metadata(&common.join("config")) {
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
fn read_metadata(path: &Path) -> Option<String> {
    if path.ancestors().any(|part| {
        fs::symlink_metadata(part)
            .ok()
            .is_some_and(|m| m.file_type().is_symlink())
    }) {
        return None;
    }
    let mut text = String::new();
    crate::events::open_regular(path)?
        .take(GIT_FILE_MAX + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() <= GIT_FILE_MAX as usize).then_some(text)
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
        assert!(read_metadata(&scratch.0.join("storage/config")).is_none());
        std::os::unix::fs::symlink(scratch.0.join("real"), scratch.0.join("alias")).unwrap();
        assert!(read_metadata(&scratch.0.join("alias/config")).is_none());
    }
}
