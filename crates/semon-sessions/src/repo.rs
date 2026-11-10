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

/// Git refuses an include chain deeper than ten files.
const CONFIG_INCLUDE_MAX: usize = 10;

/// A single identity lookup reads only a bounded include graph.
const CONFIG_READ_MAX: usize = 32;
const CONFIG_TOTAL_MAX: u64 = 256 * 1024;

/// Conditional config matching must not turn an untrusted pattern into unbounded work.
const WILDMATCH_MAX_STEPS: usize = 1_000;

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
        let (git_dir, common) = if metadata.is_dir() {
            (git.clone(), git)
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
                Some(common) => (target, common),
                // A submodule's or `--separate-git-dir` repository's own gitdir.
                None => (target.clone(), target),
            }
        } else {
            return None;
        };
        let origin = effective_origin(&common, &git_dir)
            .map_or(RepoId::Unknown, |url| classify_origin(&url));
        return Some(origin);
    }
    None
}

/// The effective first fetch URL from the repository-only Git configuration.
/// Includes are processed at their textual position; global and system config
/// are intentionally outside this local identity observation.
fn effective_origin(common: &Path, git_dir: &Path) -> Option<String> {
    let mut state = ConfigState::default();
    let mut stack = Vec::new();
    let mut budget = ConfigBudget::default();
    read_config(
        &common.join("config"),
        git_dir,
        0,
        &mut stack,
        &mut budget,
        &mut state,
    );
    if !state.resource_limited && !state.origin_seen && state.worktree_config {
        let path = if git_dir == common {
            common.join("config.worktree")
        } else {
            git_dir.join("config.worktree")
        };
        read_config(&path, git_dir, 0, &mut stack, &mut budget, &mut state);
    }
    (!state.resource_limited).then_some(state.origin).flatten()
}

#[derive(Default)]
struct ConfigState {
    origin: Option<String>,
    origin_seen: bool,
    resource_limited: bool,
    worktree_config: bool,
}

#[derive(Default)]
struct ConfigBudget {
    files: usize,
    bytes: u64,
    stopped: bool,
}

#[derive(Default)]
enum ConfigSection {
    #[default]
    Other,
    Origin,
    Include(Option<String>),
    Extensions,
}

/// Read one config and its eligible includes. An unreadable include, a loop, or
/// a chain beyond Git's limit is ignored rather than changing the identity.
fn read_config(
    path: &Path,
    git_dir: &Path,
    depth: usize,
    stack: &mut Vec<PathBuf>,
    budget: &mut ConfigBudget,
    state: &mut ConfigState,
) {
    if state.origin_seen || state.resource_limited || budget.stopped {
        return;
    }
    if depth > CONFIG_INCLUDE_MAX || stack.iter().any(|seen| seen == path) {
        state.resource_limited = true;
        return;
    }
    if budget.files == CONFIG_READ_MAX {
        budget.stopped = true;
        state.resource_limited = true;
        return;
    }
    let Some(bytes) = metadata_len(path) else {
        return;
    };
    let Some(total) = budget.bytes.checked_add(bytes) else {
        budget.stopped = true;
        state.resource_limited = true;
        return;
    };
    if bytes > CONFIG_MAX {
        state.resource_limited = true;
        return;
    }
    if total > CONFIG_TOTAL_MAX {
        budget.stopped = true;
        state.resource_limited = true;
        return;
    }
    let Some(config) = read_metadata(path, CONFIG_MAX) else {
        return;
    };
    budget.files += 1;
    budget.bytes = total;
    stack.push(path.to_owned());
    let mut section = ConfigSection::Other;
    for line in logical_lines(config.strip_prefix('\u{feff}').unwrap_or(&config)) {
        let mut body = line.trim_start();
        if let Some(header) = body.strip_prefix('[')
            && let Some((next, rest)) = config_section(header)
        {
            section = next;
            body = rest.trim_start();
        }
        let Some((key, raw_value)) = body.split_once('=') else {
            if matches!(&section, ConfigSection::Extensions)
                && body.trim().eq_ignore_ascii_case("worktreeconfig")
            {
                state.worktree_config = true;
            }
            continue;
        };
        let key = key.trim();
        let Some(value) = config_value(raw_value) else {
            continue;
        };
        match &section {
            ConfigSection::Origin if key.eq_ignore_ascii_case("url") => {
                // `git remote get-url origin` takes the first expanded fetch URL.
                state.origin_seen = true;
                state.origin = (!value.is_empty()).then_some(value);
                break;
            }
            ConfigSection::Include(condition) if key.eq_ignore_ascii_case("path") => {
                match include_matches(condition.as_deref(), git_dir, path) {
                    MatchResult::NoMatch => continue,
                    MatchResult::Exhausted => {
                        state.resource_limited = true;
                        break;
                    }
                    MatchResult::Match => {}
                }
                if let Some(include) = include_path(path, &value) {
                    read_config(&include, git_dir, depth + 1, stack, budget, state);
                    if state.origin_seen || state.resource_limited {
                        break;
                    }
                }
            }
            ConfigSection::Extensions if key.eq_ignore_ascii_case("worktreeconfig") => {
                state.worktree_config = git_bool(&value);
            }
            _ => {}
        }
    }
    stack.pop();
}

fn git_bool(value: &str) -> bool {
    let value = value.trim();
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => true,
        "false" | "no" | "off" | "" => false,
        value => parse_git_integer(value).is_some_and(|value| value != 0),
    }
}

fn parse_git_integer(value: &str) -> Option<i32> {
    let (digits, multiplier) = match value.as_bytes().last()?.to_ascii_lowercase() {
        b'k' => (&value[..value.len() - 1], 1_i64 << 10),
        b'm' => (&value[..value.len() - 1], 1_i64 << 20),
        b'g' => (&value[..value.len() - 1], 1_i64 << 30),
        _ => (value, 1),
    };
    let (sign, digits) = match digits.as_bytes().first() {
        Some(b'+') => (1_i64, &digits[1..]),
        Some(b'-') => (-1_i64, &digits[1..]),
        _ => (1, digits),
    };
    let (radix, digits) = if let Some(digits) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, digits)
    } else if digits.starts_with('0') && digits.len() > 1 {
        (8, digits)
    } else {
        (10, digits)
    };
    let value = i64::from_str_radix(digits, radix).ok()?;
    let value = value.checked_mul(sign)?.checked_mul(multiplier)?;
    i32::try_from(value).ok()
}

/// Parse the section header after its opening bracket and return the section
/// plus text following the closing bracket, which may hold an assignment.
fn config_section(header: &str) -> Option<(ConfigSection, &str)> {
    let name_end = header.find(|c: char| c.is_whitespace() || c == ']')?;
    let (name, tail) = header.split_at(name_end);
    if let Some(rest) = tail.strip_prefix(']') {
        return Some((
            if name.eq_ignore_ascii_case("include") {
                ConfigSection::Include(None)
            } else if name.eq_ignore_ascii_case("extensions") {
                ConfigSection::Extensions
            } else if name.split_once('.').is_some_and(|(section, subsection)| {
                section.eq_ignore_ascii_case("remote") && subsection.eq_ignore_ascii_case("origin")
            }) {
                ConfigSection::Origin
            } else {
                ConfigSection::Other
            },
            rest,
        ));
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
    let section = if name.eq_ignore_ascii_case("remote") && subsection == "origin" {
        ConfigSection::Origin
    } else if name.eq_ignore_ascii_case("includeif") {
        ConfigSection::Include(Some(subsection))
    } else {
        ConfigSection::Other
    };
    Some((section, rest))
}

fn include_path(including: &Path, value: &str) -> Option<PathBuf> {
    let path = if let Some(rest) = value.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else {
        let value = Path::new(value);
        if value.is_absolute() {
            value.to_owned()
        } else {
            including.parent()?.join(value)
        }
    };
    normalize(&path)
}

fn include_matches(condition: Option<&str>, git_dir: &Path, including: &Path) -> MatchResult {
    let Some(condition) = condition else {
        return MatchResult::Match;
    };
    let Some((kind, pattern)) = condition.split_once(':') else {
        return MatchResult::NoMatch;
    };
    match kind.to_ascii_lowercase().as_str() {
        "gitdir" | "gitdir/i" => {
            if pattern.is_empty() {
                return MatchResult::Match;
            }
            let insensitive = kind.eq_ignore_ascii_case("gitdir/i");
            let Some(pattern) = include_pattern(pattern, including) else {
                return MatchResult::NoMatch;
            };
            wildcard_match_result(&pattern, &git_dir.to_string_lossy(), insensitive)
        }
        "onbranch" => branch_name(git_dir).map_or(MatchResult::NoMatch, |branch| {
            wildcard_match_result(&branch_pattern(pattern), &branch, false)
        }),
        _ => MatchResult::NoMatch,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MatchResult {
    Match,
    NoMatch,
    Exhausted,
}

fn include_pattern(pattern: &str, including: &Path) -> Option<String> {
    let trailing_slash = pattern.ends_with('/');
    let pattern = if let Some(rest) = pattern.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else if let Some(rest) = pattern.strip_prefix("./") {
        including.parent()?.join(rest)
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        return Some(with_trailing_glob(format!("**/{pattern}"), trailing_slash));
    };
    let mut pattern = pattern.to_string_lossy().into_owned();
    if trailing_slash && !pattern.ends_with('/') {
        pattern.push('/');
    }
    Some(with_trailing_glob(pattern, trailing_slash))
}

fn branch_pattern(pattern: &str) -> String {
    with_trailing_glob(pattern.to_owned(), pattern.ends_with('/'))
}

fn with_trailing_glob(mut pattern: String, trailing_slash: bool) -> String {
    if trailing_slash {
        pattern.push_str("**");
    }
    pattern
}

fn branch_name(git_dir: &Path) -> Option<String> {
    let head = read_metadata(&git_dir.join("HEAD"), GIT_FILE_MAX)?;
    let reference = head.trim().strip_prefix("ref:")?.trim_start();
    reference
        .strip_prefix("refs/heads/")
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
}

/// Git's `wildmatch.c` pathname rules for conditional config patterns. The
/// bounded state walk fails closed when an adversarial pattern exceeds its work
/// budget; config conditions never justify unbounded CPU.
#[cfg(test)]
fn wildcard_match(pattern: &str, value: &str, insensitive: bool) -> bool {
    wildcard_match_result(pattern, value, insensitive) == MatchResult::Match
}

fn wildcard_match_result(pattern: &str, value: &str, insensitive: bool) -> MatchResult {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pending = vec![(0, 0)];
    let mut seen = std::collections::BTreeSet::new();
    let mut steps = 0;
    while let Some((pattern_index, value_index)) = pending.pop() {
        steps += 1;
        if steps > WILDMATCH_MAX_STEPS {
            return MatchResult::Exhausted;
        }
        if !seen.insert((pattern_index, value_index)) {
            continue;
        }
        let Some(&token) = pattern.get(pattern_index) else {
            if value_index == value.len() {
                return MatchResult::Match;
            }
            continue;
        };
        match token {
            b'\\' => {
                let Some(&literal) = pattern.get(pattern_index + 1) else {
                    continue;
                };
                if value
                    .get(value_index)
                    .is_some_and(|value| byte_eq(*value, literal, insensitive))
                    && !wildmatch_push(&mut pending, (pattern_index + 2, value_index + 1))
                {
                    return MatchResult::Exhausted;
                }
            }
            b'?' => {
                if value.get(value_index).is_some_and(|value| *value != b'/')
                    && !wildmatch_push(&mut pending, (pattern_index + 1, value_index + 1))
                {
                    return MatchResult::Exhausted;
                }
            }
            b'[' => {
                if let Some((end, matches)) = bracket_match(
                    pattern,
                    pattern_index,
                    value.get(value_index).copied(),
                    insensitive,
                ) && matches
                    && !wildmatch_push(&mut pending, (end, value_index + 1))
                {
                    return MatchResult::Exhausted;
                }
            }
            b'*' => {
                let run_end = pattern[pattern_index..]
                    .iter()
                    .position(|byte| *byte != b'*')
                    .map_or(pattern.len(), |offset| pattern_index + offset);
                let prior_is_slash = pattern_index == 0 || pattern[pattern_index - 1] == b'/';
                let next_is_slash = pattern.get(run_end) == Some(&b'/');
                let recursive = run_end - pattern_index >= 2 && prior_is_slash && next_is_slash;
                let terminal_recursive =
                    run_end - pattern_index >= 2 && prior_is_slash && run_end == pattern.len();
                let next = if recursive { run_end + 1 } else { run_end };
                if !wildmatch_push(&mut pending, (next, value_index)) {
                    return MatchResult::Exhausted;
                }
                if recursive {
                    for (offset, byte) in value[value_index..].iter().enumerate() {
                        if *byte == b'/'
                            && !wildmatch_push(&mut pending, (next, value_index + offset + 1))
                        {
                            return MatchResult::Exhausted;
                        }
                    }
                } else if terminal_recursive {
                    return MatchResult::Match;
                } else {
                    let end = value[value_index..]
                        .iter()
                        .position(|byte| *byte == b'/')
                        .map_or(value.len(), |offset| value_index + offset);
                    for offset in 0..end - value_index {
                        if !wildmatch_push(&mut pending, (next, value_index + offset + 1)) {
                            return MatchResult::Exhausted;
                        }
                    }
                }
            }
            literal => {
                if value
                    .get(value_index)
                    .is_some_and(|value| byte_eq(*value, literal, insensitive))
                    && !wildmatch_push(&mut pending, (pattern_index + 1, value_index + 1))
                {
                    return MatchResult::Exhausted;
                }
            }
        }
    }
    MatchResult::NoMatch
}

fn wildmatch_push(pending: &mut Vec<(usize, usize)>, state: (usize, usize)) -> bool {
    if pending.len() == WILDMATCH_MAX_STEPS {
        false
    } else {
        pending.push(state);
        true
    }
}

fn byte_eq(left: u8, right: u8, insensitive: bool) -> bool {
    if insensitive {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

fn bracket_match(
    pattern: &[u8],
    start: usize,
    value: Option<u8>,
    insensitive: bool,
) -> Option<(usize, bool)> {
    let value = value.filter(|value| *value != b'/')?;
    let mut index = start + 1;
    let mut negated = false;
    if matches!(pattern.get(index), Some(b'!') | Some(b'^')) {
        negated = true;
        index += 1;
    }
    let mut matched = false;
    let mut first = true;
    while let Some(&token) = pattern.get(index) {
        if token == b']' && !first {
            return Some((index + 1, matched != negated));
        }
        first = false;
        if pattern.get(index..index + 2) == Some(b"[:") {
            let class_start = index + 2;
            let class_end = pattern[class_start..]
                .windows(2)
                .position(|window| window == b":]")
                .map(|offset| class_start + offset)?;
            let class = std::str::from_utf8(&pattern[class_start..class_end]).ok()?;
            matched |= posix_class(class, value, insensitive)?;
            index = class_end + 2;
            continue;
        }
        let (begin, after_begin) = bracket_token(pattern, index)?;
        index = after_begin;
        if pattern.get(index) == Some(&b'-') && pattern.get(index + 1) != Some(&b']') {
            let (end, after_end) = bracket_token(pattern, index + 1)?;
            index = after_end;
            let (value, begin, end) = if insensitive {
                (
                    value.to_ascii_lowercase(),
                    begin.to_ascii_lowercase(),
                    end.to_ascii_lowercase(),
                )
            } else {
                (value, begin, end)
            };
            matched |= begin <= value && value <= end;
        } else {
            matched |= byte_eq(value, begin, insensitive);
        }
    }
    None
}

fn posix_class(class: &str, value: u8, insensitive: bool) -> Option<bool> {
    let alphabetic = value.is_ascii_alphabetic();
    Some(match class {
        "alnum" => value.is_ascii_alphanumeric(),
        "alpha" => alphabetic,
        "blank" => matches!(value, b' ' | b'\t'),
        "cntrl" => value.is_ascii_control(),
        "digit" => value.is_ascii_digit(),
        "graph" => value.is_ascii_graphic(),
        "lower" => value.is_ascii_lowercase() || insensitive && alphabetic,
        "print" => value.is_ascii_graphic() || value == b' ',
        "punct" => value.is_ascii_punctuation(),
        "space" => value.is_ascii_whitespace(),
        "upper" => value.is_ascii_uppercase() || insensitive && alphabetic,
        "xdigit" => value.is_ascii_hexdigit(),
        _ => return None,
    })
}

fn bracket_token(pattern: &[u8], index: usize) -> Option<(u8, usize)> {
    match pattern.get(index)? {
        b'\\' => Some((*pattern.get(index + 1)?, index + 2)),
        value => Some((*value, index + 1)),
    }
}

/// The `url` of `[remote "origin"]` (also written `[remote.origin]`) in a Git
/// config text; the first one, as `git remote get-url origin` reads it. Section names
/// and keys are case-insensitive, a quoted subsection is exact. A header may
/// be followed by a comment or by an assignment, and a trailing backslash
/// continues a line.
#[cfg(test)]
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
#[cfg(test)]
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

/// Metadata length for a regular file with no symbolic-link ancestor.
fn metadata_len(path: &Path) -> Option<u64> {
    if path.ancestors().any(|part| {
        fs::symlink_metadata(part)
            .ok()
            .is_some_and(|m| m.file_type().is_symlink())
    }) {
        return None;
    }
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata.is_file().then_some(metadata.len())
}

/// Only bounded regular metadata, with no symbolic-link ancestor. Files
/// named by untrusted pointers must not redirect reads through a symlink.
fn read_metadata(path: &Path, max: u64) -> Option<String> {
    metadata_len(path)?;
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
    fn repository_origin_uses_includes_and_worktree_config() {
        let scratch = Scratch::new();
        semon(&scratch);
        let id = |relative: &str| repo_id(&scratch.0.join(relative).to_string_lossy(), None);
        let remote = |host: &str| Some(RepoId::Remote(format!("{host}/team/app")));
        scratch.file("work/semon/.git/worktrees/x/commondir", "../..\n");

        scratch.file("work/semon/.git/config", "[include]\npath = origin.conf\n");
        scratch.file(
            "work/semon/.git/origin.conf",
            "[remote \"origin\"]\nurl = https://included.test/team/app.git\n",
        );
        assert_eq!(id("work/semon"), remote("included.test"));
        scratch.file(
            "work/semon/.git/config",
            "[includeIf \"gitdir:\"]\npath = all.conf\n[remote \"origin\"]\nurl = https://fallback.test/team/app.git\n",
        );
        scratch.file(
            "work/semon/.git/all.conf",
            "[remote \"origin\"]\nurl = https://all.test/team/app.git\n",
        );
        assert_eq!(id("work/semon"), remote("all.test"));
        scratch.file("work/semon/.git/config", "[include]\npath = origin.conf\n");
        scratch.file(
            "work/semon/.git/origin.conf",
            "[remote \"origin\"]\nurl = https://changed.test/team/app.git\n",
        );
        assert_eq!(id("work/semon"), remote("changed.test"));

        let worktree_git = scratch.0.join("work/semon/.git/worktrees/x");
        scratch.file(
            "work/semon/.git/config",
            &format!(
                "[includeIf \"gitdir:{}\"]\npath = worktree.conf\n[includeIf \"gitdir/i:{}\"]\npath = insensitive.conf\n[remote \"origin\"]\nurl = https://common.test/team/app.git\n",
                worktree_git.display(),
                worktree_git.to_string_lossy().to_ascii_uppercase(),
            ),
        );
        scratch.file(
            "work/semon/.git/worktree.conf",
            "[remote \"origin\"]\nurl = https://linked.test/team/app.git\n",
        );
        scratch.file(
            "work/semon/.git/insensitive.conf",
            "[remote \"origin\"]\nurl = https://insensitive.test/team/app.git\n",
        );
        assert_eq!(id("work/semon-wt-x"), remote("linked.test"));
        assert_eq!(id("work/semon"), remote("common.test"));

        scratch.file("work/semon/.git/config", "[extensions]\nworktreeConfig\n");
        scratch.file(
            "work/semon/.git/worktrees/x/config.worktree",
            "[remote \"origin\"]\nurl = https://worktree.test/team/app.git\n",
        );
        assert_eq!(id("work/semon-wt-x"), remote("worktree.test"));
        assert_eq!(id("work/semon"), Some(RepoId::Unknown));
        scratch.file(
            "work/semon/.git/config",
            "[remote \"origin\"]\nurl = https://common.test/team/app.git\n",
        );
        assert_eq!(id("work/semon-wt-x"), remote("common.test"));
    }

    #[test]
    fn worktree_config_accepts_git_booleans() {
        let scratch = Scratch::new();
        semon(&scratch);
        scratch.file("work/semon/.git/worktrees/x/commondir", "../..\n");
        scratch.file(
            "work/semon/.git/worktrees/x/config.worktree",
            "[remote \"origin\"]\nurl = https://worktree.test/team/app.git\n",
        );
        let id = || repo_id(&scratch.0.join("work/semon-wt-x").to_string_lossy(), None);
        for value in ["2", "-1", "01", "1k", "1m", "1g", "0x1", " 1"] {
            scratch.file(
                "work/semon/.git/config",
                &format!("[extensions]\nworktreeConfig = {value}\n"),
            );
            assert_eq!(id(), Some(RepoId::Remote("worktree.test/team/app".into())));
        }
        scratch.file(
            "work/semon/.git/config",
            "[extensions]\nworktreeConfig = 1\nworktreeConfig = 0\n",
        );
        assert_eq!(id(), Some(RepoId::Unknown));
        for value in ["08", "2147483648"] {
            scratch.file(
                "work/semon/.git/config",
                &format!("[extensions]\nworktreeConfig = {value}\n"),
            );
            assert_eq!(id(), Some(RepoId::Unknown));
        }
    }

    #[test]
    fn malformed_config_text_and_missing_includes_are_skipped() {
        let scratch = Scratch::new();
        semon(&scratch);
        let id = || repo_id(&scratch.0.join("work/semon").to_string_lossy(), None);
        scratch.file(
            "work/semon/.git/config",
            "# example = \"unterminated\n; example = \\q\n[core] # example = \"unterminated\n[include]\npath = missing.conf\n[remote \"origin\"]\nurl = https://valid.test/team/app.git\n",
        );
        assert_eq!(id(), Some(RepoId::Remote("valid.test/team/app".into())));

        scratch.file(
            "work/semon/.git/config",
            "[include]\npath = oversized.conf\n[remote \"origin\"]\nurl = https://later.test/team/app.git\n",
        );
        scratch.file(
            "work/semon/.git/oversized.conf",
            &"x".repeat(CONFIG_MAX as usize + 1),
        );
        assert_eq!(id(), Some(RepoId::Unknown));
    }

    #[test]
    fn repository_origin_skips_include_cycles_and_deep_chains() {
        let scratch = Scratch::new();
        semon(&scratch);
        let id = |relative: &str| repo_id(&scratch.0.join(relative).to_string_lossy(), None);
        scratch.file(
            "work/semon/.git/config",
            "[include]\npath = a.conf\n[remote \"origin\"]\nurl = https://base.test/team/app.git\n",
        );
        scratch.file(
            "work/semon/.git/a.conf",
            "[include]\npath = b.conf\n[remote \"origin\"]\nurl = https://a.test/team/app.git\n",
        );
        scratch.file(
            "work/semon/.git/b.conf",
            "[include]\npath = a.conf\n[remote \"origin\"]\nurl = https://b.test/team/app.git\n",
        );
        assert_eq!(id("work/semon"), Some(RepoId::Unknown));

        for index in 0..=CONFIG_INCLUDE_MAX {
            let next = index + 1;
            scratch.file(
                &format!("work/semon/.git/{index}.conf"),
                &format!("[include]\npath = {next}.conf\n"),
            );
        }
        scratch.file(
            "work/semon/.git/config",
            "[include]\npath = 0.conf\n[remote \"origin\"]\nurl = https://base.test/team/app.git\n",
        );
        scratch.file(
            &format!("work/semon/.git/{}.conf", CONFIG_INCLUDE_MAX + 1),
            "[remote \"origin\"]\nurl = https://too-deep.test/team/app.git\n",
        );
        assert_eq!(id("work/semon"), Some(RepoId::Unknown));
        scratch.file(
            "work/semon/.git/config",
            "[broken\n[remote \"origin\"]\nurl = https://base.test/team/app.git\n",
        );
        assert_eq!(
            id("work/semon"),
            Some(RepoId::Remote("base.test/team/app".into()))
        );
        scratch.file(
            "work/semon/.git/config",
            "[remote \"origin\"]\nurl = \"unterminated\n[remote \"origin\"]\nurl = https://base.test/team/app.git\n",
        );
        assert_eq!(
            id("work/semon"),
            Some(RepoId::Remote("base.test/team/app".into()))
        );
    }

    #[test]
    fn onbranch_include_reads_head() {
        let scratch = Scratch::new();
        semon(&scratch);
        scratch.file(
            "work/semon/.git/config",
            "[includeIf \"onbranch:feature/*\"]\npath = branch.conf\n",
        );
        scratch.file(
            "work/semon/.git/HEAD",
            "ref:   refs/heads/feature/origin  \n",
        );
        scratch.file(
            "work/semon/.git/branch.conf",
            "[remote \"origin\"]\nurl = https://branch.test/team/app.git\n",
        );
        assert_eq!(
            repo_id(&scratch.0.join("work/semon").to_string_lossy(), None),
            Some(RepoId::Remote("branch.test/team/app".into()))
        );
    }

    #[test]
    fn conditional_include_patterns_preserve_trailing_slashes_and_components() {
        let scratch = Scratch::new();
        semon(&scratch);
        let repo = scratch.0.join("work/semon");
        scratch.file(
            "work/semon/.git/config",
            &format!(
                "[includeIf \"gitdir:{}/\"]\npath = trailing.conf\n[includeIf \"gitdir:work/*\"]\npath = crosses-component.conf\n",
                repo.display()
            ),
        );
        scratch.file(
            "work/semon/.git/trailing.conf",
            "[remote \"origin\"]\nurl = https://trailing.test/team/app.git\n",
        );
        scratch.file(
            "work/semon/.git/crosses-component.conf",
            "[remote \"origin\"]\nurl = https://crossed.test/team/app.git\n",
        );
        assert_eq!(
            repo_id(&repo.to_string_lossy(), None),
            Some(RepoId::Remote("trailing.test/team/app".into()))
        );
        assert!(!wildcard_match("**/work/*", "/tmp/work/child/.git", false));
        assert!(!wildcard_match("work/*", "/tmp/work/child/.git", false));
        assert!(wildcard_match(
            "/repos/[[:digit:]]/.git",
            "/repos/7/.git",
            false
        ));
        assert!(!wildcard_match(
            "/tmp/project/x/../.git",
            "/tmp/project/.git",
            false
        ));

        scratch.file(
            "work/semon/.git/config",
            "[includeIf \"onbranch:feature/\"]\npath = branch.conf\n",
        );
        scratch.file(
            "work/semon/.git/HEAD",
            "ref: refs/heads/feature/deep/name\n",
        );
        scratch.file(
            "work/semon/.git/branch.conf",
            "[remote \"origin\"]\nurl = https://branch.test/team/app.git\n",
        );
        assert_eq!(
            repo_id(&repo.to_string_lossy(), None),
            Some(RepoId::Remote("branch.test/team/app".into()))
        );
    }

    #[test]
    fn wildmatch_follows_git_pathname_rules_with_a_work_budget() {
        assert!(wildcard_match("**/.git", "/repos/a/.git", false));
        assert!(wildcard_match("/repos/**/.git", "/repos/a/b/.git", false));
        assert!(wildcard_match("/repos/**", "/repos/a/b/.git", false));
        assert!(wildcard_match(
            "/repos/**foo/.git",
            "/repos/foo/.git",
            false
        ));
        assert!(!wildcard_match(
            "/repos/**foo/.git",
            "/repos/a/bfoo/.git",
            false
        ));
        assert!(wildcard_match(
            "/repos/project[0-9]/.git",
            "/repos/project7/.git",
            false
        ));
        assert!(!wildcard_match(
            "/repos/project[0-9]/.git",
            "/repos/projectx/.git",
            false
        ));
        assert!(wildcard_match("[!x]", "a", false));
        assert!(wildcard_match("[^x]", "a", false));
        assert!(wildcard_match("[]]", "]", false));
        assert!(wildcard_match("a\\?", "a?", false));
        assert!(wildcard_match("[[:digit:]]", "7", false));
        assert!(wildcard_match("[[:alpha:]]", "z", false));
        assert!(wildcard_match("[[:alnum:]]", "7", false));
        assert!(wildcard_match("[[:lower:]]", "z", false));
        assert!(wildcard_match("[[:upper:]]", "Z", false));
        assert!(wildcard_match("[[:space:]]", " ", false));
        assert!(wildcard_match("[[:punct:]]", "!", false));
        assert!(wildcard_match("[[:xdigit:]]", "f", false));
        assert!(wildcard_match("[[:blank:]]", "\t", false));
        assert!(wildcard_match("[[:cntrl:]]", "\u{7f}", false));
        assert!(wildcard_match("[[:graph:]]", "!", false));
        assert!(wildcard_match("[[:print:]]", " ", false));
        assert!(wildcard_match("[[:upper:]]", "a", true));
        assert!(wildcard_match("[a-z]", "A", true));
        assert!(!wildcard_match("[[:unknown:]]", "x", false));
        assert!(!wildcard_match("[abc", "a", false));

        let pattern = "*a".repeat(2_000) + "b";
        assert!(!wildcard_match(&pattern, &"a".repeat(2_000), false));
    }

    #[test]
    fn include_if_matcher_budget_refuses_identity() {
        let scratch = Scratch::new();
        let name = "a".repeat(64) + "b";
        scratch.dir(&name);
        scratch.file(
            &format!("{name}/.git/config"),
            &format!(
                "[includeIf \"gitdir:*{}\"]\npath = ignored.conf\n[remote \"origin\"]\nurl = https://later.test/team/app.git\n",
                "a".repeat(64) + "b/.git"
            ),
        );
        let pattern = format!("**/*{}", "a".repeat(64) + "b/.git");
        let git_dir = scratch.0.join(&name).join(".git");
        assert_eq!(
            wildcard_match_result(&pattern, &git_dir.to_string_lossy(), false),
            MatchResult::Exhausted
        );
        assert_eq!(
            repo_id(&scratch.0.join(&name).to_string_lossy(), None),
            Some(RepoId::Unknown)
        );
    }

    #[test]
    fn expanded_config_uses_the_first_origin_url() {
        let scratch = Scratch::new();
        semon(&scratch);
        let id = || repo_id(&scratch.0.join("work/semon").to_string_lossy(), None);
        scratch.file(
            "work/semon/.git/config",
            "[include]\npath = a.conf\n[remote \"origin\"]\nurl = https://local.test/team/app.git\n[include]\npath = a.conf\n",
        );
        scratch.file(
            "work/semon/.git/a.conf",
            "[remote \"origin\"]\nurl = https://included.test/team/app.git\n",
        );
        assert_eq!(id(), Some(RepoId::Remote("included.test/team/app".into())));
        scratch.file(
            "work/semon/.git/config",
            "[remote \"origin\"]\nurl = https://local.test/team/app.git\n[include]\npath = a.conf\n",
        );
        assert_eq!(id(), Some(RepoId::Remote("local.test/team/app".into())));
        scratch.file(
            "work/semon/.git/config",
            "[remote \"origin\"]\nurl =\n[include]\npath = a.conf\n",
        );
        assert_eq!(id(), Some(RepoId::Unknown));
    }

    #[test]
    fn include_read_budget_skips_excess_siblings_and_bytes() {
        let scratch = Scratch::new();
        semon(&scratch);
        let includes = (0..=CONFIG_READ_MAX)
            .map(|index| format!("[include]\npath = {index}.conf\n"))
            .chain(std::iter::once(
                "[remote \"origin\"]\nurl = https://later.test/team/app.git\n".to_owned(),
            ))
            .collect::<String>();
        scratch.file("work/semon/.git/config", &includes);
        for index in 0..=CONFIG_READ_MAX {
            scratch.file(
                &format!("work/semon/.git/{index}.conf"),
                "[core]\nbare = false\n",
            );
        }
        assert_eq!(
            repo_id(&scratch.0.join("work/semon").to_string_lossy(), None),
            Some(RepoId::Unknown)
        );
        let git_dir = scratch.0.join("work/semon/.git");
        let mut state = ConfigState::default();
        let mut stack = Vec::new();
        let mut budget = ConfigBudget::default();
        read_config(
            &git_dir.join("config"),
            &git_dir,
            0,
            &mut stack,
            &mut budget,
            &mut state,
        );
        assert_eq!(budget.files, CONFIG_READ_MAX);
        assert!(budget.stopped);

        let includes = (0..=4)
            .map(|index| format!("[include]\npath = bytes-{index}.conf\n"))
            .collect::<String>();
        scratch.file("work/semon/.git/config", &includes);
        for index in 0..4 {
            scratch.file(
                &format!("work/semon/.git/bytes-{index}.conf"),
                &format!("# {}\n", "x".repeat((CONFIG_MAX - 4096) as usize - 3)),
            );
        }
        scratch.file(
            "work/semon/.git/bytes-4.conf",
            &format!("# {}\n", "x".repeat(CONFIG_MAX as usize - 3)),
        );
        assert_eq!(
            repo_id(&scratch.0.join("work/semon").to_string_lossy(), None),
            Some(RepoId::Unknown)
        );
        let mut state = ConfigState::default();
        let mut stack = Vec::new();
        let mut budget = ConfigBudget::default();
        read_config(
            &git_dir.join("config"),
            &git_dir,
            0,
            &mut stack,
            &mut budget,
            &mut state,
        );
        assert_eq!(budget.files, 5);
        assert!(budget.stopped);
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
