//! Bounded, transient decoding. Only typed observations leave this module.
use std::{borrow::Cow, collections::BTreeMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{ActivityStepId, SourcePosition};
use crate::events::{FileIndex, Kind, field, record_time};

const MAX_BYTES: usize = 256 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_COMPONENTS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum WasteKind {
    OrchestrationWait,
    PathNotFound,
    MalformedCommand,
    ExternalFailure,
    OtherFailure,
    Refill,
    RedundantRead,
    Sleep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum WasteCategory {
    Orchestration,
    Tooling,
    External,
    Context,
    Unmapped,
}

impl WasteKind {
    pub(crate) fn category(self) -> WasteCategory {
        match self {
            Self::OrchestrationWait => WasteCategory::Orchestration,
            Self::PathNotFound | Self::MalformedCommand => WasteCategory::Tooling,
            Self::ExternalFailure => WasteCategory::External,
            Self::Refill => WasteCategory::Context,
            Self::OtherFailure | Self::RedundantRead | Self::Sleep => WasteCategory::Unmapped,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WaitInterval {
    pub(crate) started_at_ms: i64,
    pub(crate) completed_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Change {
    Commit {
        branch: String,
        sha: String,
    },
    Push {
        repository: String,
        from: String,
        to: String,
    },
    PullRequest {
        repository: String,
        number: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChangeFact {
    pub(crate) identity: String,
    pub(crate) timestamp: Option<i64>,
    pub(crate) step_id: Option<ActivityStepId>,
    pub(crate) position: SourcePosition,
    pub(crate) record_digest: String,
    pub(crate) change: Change,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Command {
    Read(String, ReadTool),
    Search(String, SearchTool),
    Setup,
    Stage,
    BodyWrite,
    Inspect,
    Commit,
    Push,
    PullRequest,
    PrChecks,
    Watch,
    Sleep,
    Mutation,
    Ambiguous,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ReadTool {
    Native,
    Cat,
    Range,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum SearchTool {
    Rg,
    Grep,
    Native,
}

/// No tool inputs or results: fingerprints are SHA-256, commands are allowlisted
/// tags. Both physical records are bound to the enclosing file's consumed ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Observation {
    pub(crate) call_digest: String,
    pub(crate) result_digest: Option<String>,
    pub(crate) step_id: Option<ActivityStepId>,
    commands: Vec<Command>,
    orchestration: bool,
    formatter: bool,
    envelopes: bool,
    completed: bool,
    settled: bool,
    background: bool,
    pub(crate) waste: Option<WasteKind>,
    pub(crate) wait: Option<WaitInterval>,
    pub(crate) empty_search: bool,
    pub(crate) changes: Vec<Change>,
}

#[derive(Clone, Debug)]
pub(crate) struct LocatedObservation {
    pub(crate) position: SourcePosition,
    pub(crate) timestamp: Option<i64>,
    pub(crate) result_offset: Option<u64>,
    pub(crate) observation: Observation,
}

pub(crate) fn unknown_boundary(bytes: &[u8]) -> Observation {
    call("unknown", &Value::Null, None, digest(bytes))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A deliberately small shell grammar. Expansions, redirects, background jobs
/// and unsupported syntax invalidate command attribution and mutation knowledge.
fn components(text: &str) -> Option<Vec<Vec<String>>> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut word_started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        // Single-quoted shell text is literal, including dollars/backticks.
        if quote != Some('\'') && (c == '$' || c == '`') {
            return None;
        }
        if c == '\\' && quote != Some('\'') {
            let next = chars.next()?;
            if next != '\n' {
                word.push(next);
                word_started = true;
            }
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
        } else {
            match c {
                '\'' | '"' => {
                    quote = Some(c);
                    word_started = true;
                }
                '<' | '>' | '(' | ')' | '{' | '}' => return None,
                ';' | '\n' | '&' | '|' => {
                    if (c == '&' || c == '|') && chars.next() != Some(c) {
                        return None;
                    }
                    if word_started {
                        words.push(std::mem::take(&mut word));
                        word_started = false;
                    }
                    if !words.is_empty() {
                        commands.push(std::mem::take(&mut words));
                    }
                    if commands.len() >= MAX_COMPONENTS {
                        return None;
                    }
                }
                c if c.is_whitespace() => {
                    if word_started {
                        words.push(std::mem::take(&mut word));
                        word_started = false;
                    }
                }
                _ => {
                    word.push(c);
                    word_started = true;
                }
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if word_started {
        words.push(word);
    }
    if !words.is_empty() {
        commands.push(words);
    }
    (!commands.is_empty()).then_some(commands)
}

fn command(words: &[String]) -> Command {
    let args: Vec<_> = words.iter().map(String::as_str).collect();
    if let Some(mut rest) = args.strip_prefix(&["git"]) {
        while matches!(rest.first(), Some(&"-c" | &"-C")) {
            let Some(next) = rest.get(2..) else {
                return Command::Unknown;
            };
            rest = next;
        }
        return match rest.first().copied() {
            Some("add") => Command::Stage,
            Some("commit") => Command::Commit,
            Some("push") => Command::Push,
            Some("status" | "diff" | "log" | "show" | "rev-parse") => Command::Inspect,
            Some("checkout" | "switch" | "reset" | "restore" | "apply" | "merge" | "rebase") => {
                Command::Mutation
            }
            _ => Command::Unknown,
        };
    }
    let fingerprint = || digest(&serde_json::to_vec(words).expect("strings serialize"));
    let mut gh = args.as_slice();
    if gh.first() == Some(&"gh") {
        gh = &gh[1..];
        while matches!(gh.first(), Some(&"--repo" | &"-R")) {
            let Some(rest) = gh.get(2..) else {
                return Command::Unknown;
            };
            gh = rest;
        }
        match gh {
            ["pr", "create", ..] => return Command::PullRequest,
            ["pr", "checks", rest @ ..]
                if !rest.iter().any(|arg| {
                    matches!(*arg, "--template" | "--json" | "--jq" | "-q" | "-t")
                        || arg.starts_with("--template=")
                        || arg.starts_with("--json=")
                        || arg.starts_with("--jq=")
                }) =>
            {
                return if rest.contains(&"--watch") {
                    Command::Watch
                } else {
                    Command::PrChecks
                };
            }
            _ => {}
        }
    }
    match args.as_slice() {
        ["cd", path] if path.starts_with('/') => Command::Setup,
        ["gh", "run", "watch", ..] => Command::Watch,
        ["rg", ..]
            if !args
                .iter()
                .any(|arg| *arg == "--pre" || arg.starts_with("--pre=")) =>
        {
            Command::Search(fingerprint(), SearchTool::Rg)
        }
        ["grep", ..] => Command::Search(fingerprint(), SearchTool::Grep),
        ["cat", ..] => Command::Read(
            digest(&serde_json::to_vec(words).expect("strings serialize")),
            ReadTool::Cat,
        ),
        ["head" | "tail" | "nl", ..] => Command::Read(
            digest(&serde_json::to_vec(words).expect("strings serialize")),
            ReadTool::Range,
        ),
        ["sed", "-n", selection, path] if sed_selection(selection) && !path.starts_with('-') => {
            Command::Read(
                digest(&serde_json::to_vec(words).expect("strings serialize")),
                ReadTool::Range,
            )
        }
        ["sleep", ..] => Command::Sleep,
        ["rm" | "mv" | "cp" | "touch" | "mkdir" | "rmdir" | "tee", ..] => Command::Mutation,
        _ => Command::Unknown,
    }
}

/// Unwrap only literal shell invocations. A script remains subject to the same
/// grammar and component/depth bounds; shell expansions never become evidence.
fn shell_commands(text: &str, depth: usize) -> Option<Vec<Command>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let (normalized, body_write) = literal_pr_body(text)?;
    let pipeline = normalized.split(" | ").collect::<Vec<_>>();
    if pipeline.len() > 1 {
        // Only read/search producers followed by bounded text selectors. No
        // shell expansion, arbitrary filter or output-producing side command.
        if pipeline.len() > MAX_COMPONENTS {
            return None;
        }
        let mut first = shell_commands(pipeline[0], depth + 1)?;
        if first.len() != 1 || !matches!(first[0], Command::Read(_, _) | Command::Search(_, _)) {
            return None;
        }
        for stage in &pipeline[1..] {
            let words = components(stage)?;
            let [words] = words.as_slice() else {
                return None;
            };
            let args = words.iter().map(String::as_str).collect::<Vec<_>>();
            if !matches!(args.as_slice(), ["head" | "tail"] | ["nl", "-ba"])
                && !matches!(args.as_slice(), ["head" | "tail", "-n", count] if count.bytes().all(|byte| byte.is_ascii_digit()) && !count.is_empty())
                && !matches!(args.as_slice(), ["sed", "-n", selection] if sed_selection(selection))
            {
                return None;
            }
        }
        let fingerprint = digest(normalized.as_bytes());
        match &mut first[0] {
            Command::Read(key, _) | Command::Search(key, _) => *key = fingerprint,
            _ => unreachable!(),
        }
        return Some(first);
    }
    let parts = components(&normalized)?;
    let mut commands = Vec::new();
    if body_write {
        commands.push(Command::BodyWrite);
    }
    let mut directories = Vec::new();
    for words in parts {
        let args: Vec<_> = words.iter().map(String::as_str).collect();
        let mut kinds = match args.as_slice() {
            [
                "bash" | "/bin/bash" | "sh" | "/bin/sh",
                "-c" | "-lc",
                script,
            ] => shell_commands(script, depth + 1)?,
            ["cd", path] if path.starts_with('/') => {
                directories.push((*path).to_owned());
                vec![Command::Setup]
            }
            _ => vec![command(&words)],
        };
        for kind in &mut kinds {
            if let Command::Read(fingerprint, _) | Command::Search(fingerprint, _) = kind {
                *fingerprint = digest(&serde_json::to_vec(&(&*fingerprint, &directories)).ok()?);
            }
        }
        commands.extend(kinds);
        if commands.len() > MAX_COMPONENTS {
            return None;
        }
    }
    Some(commands)
}

/// Discard only a quoted literal heredoc used as a PR body. Its bytes cannot
/// execute or print a forged change marker. Other redirections stay unknown.
fn literal_pr_body(text: &str) -> Option<(Cow<'_, str>, bool)> {
    if !text.contains("<<") {
        return Some((Cow::Borrowed(text), false));
    }
    if text.len() > MAX_BYTES {
        return None;
    }
    let heredoc_at = text.find("<<")?;
    let header_at = text[..heredoc_at].rfind('\n').map_or(0, |at| at + 1);
    let prefix = &text[..header_at];
    if !prefix.trim().is_empty()
        && !components(prefix)?
            .iter()
            .all(|words| command(words) == Command::Setup)
    {
        return None;
    }
    let mut lines = text[header_at..].split_inclusive('\n');
    let header = lines.next()?.trim_end_matches('\n');
    let (before, after) = header.split_once("<<")?;
    let after = after.trim();
    let substitution = before.contains("--body \"$(cat ");
    let after = if substitution {
        after.strip_suffix(')').unwrap_or(after).trim()
    } else {
        after
    };
    let delimiter = after.strip_prefix('\'')?.strip_suffix('\'')?;
    if !identifier(delimiter) || delimiter.contains('/') {
        return None;
    }
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end_matches(['\r', '\n']) == delimiter {
            closed = true;
            break;
        }
    }
    if !closed {
        return None;
    }
    let rest = lines.collect::<String>();
    if substitution {
        // The only substitution accepted is cat over a quoted literal heredoc,
        // passed directly as --body. No body bytes are executed or printed.
        let rest = rest.trim();
        if rest != "\")" && rest != ")\"" {
            return None;
        }
        let before = before.strip_suffix("$(cat ")?.strip_suffix('"')?;
        let normalized = format!("{prefix}{before}'literal-body'");
        let words = components(&normalized)?;
        if !words
            .iter()
            .all(|words| matches!(command(words), Command::Setup | Command::PullRequest))
        {
            return None;
        }
        return Some((Cow::Owned(normalized), false));
    }
    // cat > literal-file <<'END' followed by a PR using that exact body file.
    if let Some(path) = before.trim().strip_prefix("cat >") {
        let path = components(path.trim())?;
        let [path] = path.as_slice() else {
            return None;
        };
        let [path] = path.as_slice() else {
            return None;
        };
        let commands = components(&rest)?;
        if !commands.iter().any(|words| {
            command(words) == Command::PullRequest
                && words
                    .windows(2)
                    .any(|pair| pair[0] == "--body-file" && pair[1] == *path)
        }) {
            return None;
        }
        return Some((Cow::Owned(format!("{prefix}{rest}")), true));
    }
    // gh pr create --body-file - <<'END' consumes the literal stdin body.
    let words = components(before.trim())?;
    let [words] = words.as_slice() else {
        return None;
    };
    if command(words) != Command::PullRequest
        || !words
            .windows(2)
            .any(|pair| pair[0] == "--body-file" && pair[1] == "-")
        || !rest.trim().is_empty()
    {
        return None;
    }
    Some((Cow::Owned(format!("{prefix}{}", before.trim())), false))
}

fn sed_selection(selection: &str) -> bool {
    let Some(range) = selection.strip_suffix('p') else {
        return false;
    };
    !range.is_empty()
        && range.split(',').count() <= 2
        && range
            .split(',')
            .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
}

fn input_value(input: &Value) -> Option<Cow<'_, Value>> {
    if let Some(text) = input.as_str() {
        if text.len() > MAX_BYTES {
            return None;
        }
        serde_json::from_str(text).ok().map(Cow::Owned)
    } else {
        Some(Cow::Borrowed(input))
    }
}

pub(super) fn call(
    name: &str,
    input: &Value,
    step_id: Option<ActivityStepId>,
    call_digest: String,
) -> Observation {
    let name = name
        .strip_prefix("functions.")
        .or_else(|| name.strip_prefix("tools."))
        .unwrap_or(name);
    let input = input_value(input).unwrap_or(Cow::Owned(Value::Null));
    let background = input.get("run_in_background").and_then(Value::as_bool) == Some(true);
    let mut commands = match name {
        "Bash" | "bash" | "shell" | "exec_command" => input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(Value::as_str)
            .and_then(|text| shell_commands(text, 0))
            .unwrap_or_else(|| vec![Command::Ambiguous]),
        "Read" | "read_file" => {
            let path = input
                .get("file_path")
                .or_else(|| input.get("path"))
                .and_then(Value::as_str);
            let bounded_selection = ["offset", "limit", "pages"].iter().all(|key| {
                input.get(key).is_none_or(|value| {
                    value.is_null()
                        || value.as_u64().is_some()
                        || value.as_str().is_some_and(|text| text.len() <= 128)
                })
            });
            match path.filter(|path| path.len() <= 4096 && bounded_selection) {
                Some(path) => vec![Command::Read(
                    digest(
                        &serde_json::to_vec(&(
                            path.trim_start_matches("./"),
                            input.get("offset"),
                            input.get("limit"),
                            input.get("pages"),
                        ))
                        .expect("read selection serializes"),
                    ),
                    ReadTool::Native,
                )],
                None => vec![Command::Unknown],
            }
        }
        "Grep" | "Glob" => vec![Command::Search(
            digest(&serde_json::to_vec(&(name, &*input)).expect("search selection serializes")),
            SearchTool::Native,
        )],
        "Write" | "Edit" | "apply_patch" => vec![Command::Mutation],
        "Agent" | "Task" | "wait_agent" => Vec::new(),
        _ => vec![Command::Unknown],
    };
    for command in &mut commands {
        if let Command::Read(fingerprint, _) | Command::Search(fingerprint, _) = command {
            let workdir = input
                .get("workdir")
                .or_else(|| input.get("cwd"))
                .and_then(Value::as_str)
                .filter(|path| path.len() <= 4096);
            *fingerprint = digest(
                &serde_json::to_vec(&(&*fingerprint, workdir)).expect("read context serializes"),
            );
        }
    }
    let orchestration =
        matches!(name, "Agent" | "Task" | "wait_agent") || commands.contains(&Command::Watch);
    Observation {
        call_digest,
        result_digest: None,
        step_id,
        commands,
        orchestration,
        formatter: false,
        envelopes: false,
        completed: false,
        settled: false,
        background,
        waste: orchestration.then_some(WasteKind::OrchestrationWait),
        wait: None,
        empty_search: false,
        changes: Vec::new(),
    }
}

#[derive(Default)]
struct ResultEvidence {
    exit: Option<i64>,
    error: bool,
    text: String,
    nested_wait: Option<WaitInterval>,
    is_wait: bool,
    running: bool,
}

/// No arbitrary object traversal: unwrap only recognized result keys/content
/// blocks and nested JSON strings. A shared byte budget prevents fan-out growth.
fn decode(
    value: &Value,
    depth: usize,
    budget: &mut usize,
    result: &mut ResultEvidence,
    formatter: bool,
    envelopes: bool,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    *budget = budget.checked_sub(1)?;
    match value {
        Value::String(text) => {
            *budget = budget.checked_sub(text.len())?;
            if envelopes
                && matches!(text.trim_start().chars().next(), Some('{' | '[' | '"'))
                && let Ok(nested) = serde_json::from_str::<Value>(text)
            {
                return decode(&nested, depth + 1, budget, result, formatter, envelopes);
            }
            // Use the existing strict native formatter recognizer. Output body
            // lines cannot forge an exit status in the header.
            if formatter
                && let Some(frame) = crate::exec_response::decode_exec_response(text)
                && let Some(outcome) = frame.native_outcome()
            {
                if let crate::exec_response::ExecResponseOutcome::Exited(code) = outcome {
                    result.exit = Some(i64::from(code));
                } else if outcome == crate::exec_response::ExecResponseOutcome::SessionOnly {
                    result.running = true;
                }
                result.text.push_str(frame.body);
            } else {
                result.text.push_str(text);
            }
        }
        Value::Array(parts) => {
            if parts.len() > 32 {
                return None;
            }
            for part in parts {
                *budget = budget.checked_sub(1)?;
                if !matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("text" | "input_text" | "output_text")
                ) {
                    return None;
                }
                if part
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(crate::events::successful_script_header)
                {
                    *budget = budget.checked_sub(part.get("text")?.as_str()?.len())?;
                    continue;
                }
                decode(
                    part.get("text")?,
                    depth + 1,
                    budget,
                    result,
                    formatter,
                    envelopes,
                )?;
                result.text.push('\n');
            }
        }
        Value::Object(object) => {
            if !envelopes {
                return None;
            }
            if object.get("type").and_then(Value::as_str) == Some("CollabAgentToolCall")
                && object.get("tool").and_then(Value::as_str) == Some("wait")
            {
                result.is_wait = true;
                if let Some((start, end)) = object
                    .get("started_at_ms")
                    .and_then(Value::as_i64)
                    .zip(object.get("completed_at_ms").and_then(Value::as_i64))
                    && end >= start
                {
                    result.nested_wait = Some(WaitInterval {
                        started_at_ms: start,
                        completed_at_ms: end,
                    });
                }
                return Some(());
            }
            result.exit = object
                .get("exit_code")
                .or_else(|| {
                    object
                        .get("metadata")
                        .and_then(|metadata| metadata.get("exit_code"))
                })
                .and_then(Value::as_i64)
                .or(result.exit);
            result.error |= object.get("is_error").and_then(Value::as_bool) == Some(true);
            result.running |=
                result.exit.is_none() && object.get("session_id").is_some_and(|id| !id.is_null());
            let mut found = false;
            for key in ["output", "stdout", "stderr", "content"] {
                if let Some(value) = object.get(key) {
                    // Once an exec envelope supplies an outcome, its body is
                    // opaque command output. Printed JSON cannot override it.
                    let body_envelopes = result.exit.is_none() && !result.error;
                    decode(
                        value,
                        depth + 1,
                        budget,
                        result,
                        formatter && body_envelopes,
                        envelopes && body_envelopes,
                    )?;
                    result.text.push('\n');
                    found = true;
                }
            }
            if !found && result.exit.is_none() && !result.error {
                return None;
            }
        }
        _ => return None,
    }
    Some(())
}

fn failure(commands: &[Command], result: &ResultEvidence) -> Option<WasteKind> {
    if !result.error && !result.exit.is_some_and(|code| code != 0) {
        return None;
    }
    let text = result.text.to_ascii_lowercase();
    let subtype = if text.contains("no such file or directory")
        || text.contains("path not found")
        || text.contains("file not found")
    {
        WasteKind::PathNotFound
    } else if text.contains("syntax error")
        || text.contains("command not found")
        || text.contains("unrecognized option")
        || text.contains("unknown option")
    {
        WasteKind::MalformedCommand
    } else if [
        "rate limit",
        "too many requests",
        "authentication failed",
        "unauthorized",
        "http 401",
        "http 403",
        "could not resolve host",
        "connection refused",
        "connection reset",
        "network is unreachable",
        "timed out",
    ]
    .iter()
    .any(|needle| text.contains(needle))
        || text.match_indices("http ").any(|(at, _)| {
            text.get(at + 5..at + 8).is_some_and(|code| {
                code.len() == 3 && code.starts_with('5') && code.bytes().all(|c| c.is_ascii_digit())
            })
        })
    {
        WasteKind::ExternalFailure
    } else {
        WasteKind::OtherFailure
    };
    if commands == [Command::Ambiguous] {
        return Some(WasteKind::OtherFailure);
    }
    if commands.len() <= 1 {
        return Some(subtype);
    }
    // A compound's final status alone does not identify the failed component.
    // Accept only a single allowlisted diagnostic prefix and matching command.
    let cat = result.text.lines().any(|line| line.starts_with("cat: "));
    let rg = result.text.lines().any(|line| line.starts_with("rg: "));
    let diagnostic = match (cat, rg) {
        (true, false) => "cat",
        (false, true) => "rg",
        _ => return Some(WasteKind::OtherFailure),
    };
    let matching = commands
        .iter()
        .filter(|kind| match diagnostic {
            "cat" => matches!(kind, Command::Read(_, ReadTool::Cat)),
            "rg" => matches!(kind, Command::Search(_, SearchTool::Rg)),
            _ => false,
        })
        .count();
    Some(if matching == 1 {
        subtype
    } else {
        WasteKind::OtherFailure
    })
}

fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 255
        && text
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._/".contains(&c))
        && !text.starts_with('-')
        && !text.contains("..")
}

fn repository(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("https://github.com/")?
        .trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    (identifier(owner) && identifier(repo) && !repo.contains('/'))
        .then(|| format!("{owner}/{repo}"))
}

fn changes(commands: &[Command], text: &str) -> Vec<Change> {
    let mut facts = Vec::new();
    let mut destination = None;
    // Only setup and uniquely identified change commands may share output.
    // Reads, inspection and unknown programs could print plausible markers.
    if commands.is_empty()
        || commands.iter().any(|command| {
            !matches!(
                command,
                Command::Setup
                    | Command::Stage
                    | Command::BodyWrite
                    | Command::Commit
                    | Command::Push
                    | Command::PullRequest
                    | Command::PrChecks
            )
        })
        || [Command::Commit, Command::Push, Command::PullRequest]
            .iter()
            .any(|kind| commands.iter().filter(|command| *command == kind).count() > 1)
    {
        return facts;
    }
    for line in text.lines().take(4096) {
        if commands.contains(&Command::Commit)
            && let Some(header) = line
                .strip_prefix('[')
                .and_then(|line| line.split_once(']').map(|(header, _)| header))
        {
            let mut fields = header.split_whitespace();
            let branch = fields.next();
            let second = fields.next();
            let sha = if second == Some("(root-commit)") {
                fields.next()
            } else {
                second
            };
            if let (Some(branch), Some(sha), None) = (branch, sha, fields.next())
                && identifier(branch)
                && (7..=64).contains(&sha.len())
                && sha.bytes().all(|c| c.is_ascii_hexdigit())
            {
                facts.push(Change::Commit {
                    branch: branch.into(),
                    sha: sha.to_ascii_lowercase(),
                });
            }
        }
        if commands.contains(&Command::Push) {
            if let Some(url) = line.strip_prefix("To ") {
                destination = repository(url);
                continue;
            }
            if line.starts_with("From ") {
                destination = None;
                continue;
            }
            if let Some(repo) = &destination {
                let words: Vec<_> = line.split_whitespace().collect();
                let update = words.first().is_some_and(|word| {
                    word.split_once("..").is_some_and(|(before, after)| {
                        !before.is_empty()
                            && !after.is_empty()
                            && before.bytes().all(|c| c.is_ascii_hexdigit())
                            && after.bytes().all(|c| c.is_ascii_hexdigit())
                    })
                }) || line.trim_start().starts_with("* [new branch] ")
                    || line.trim_start().starts_with("* [new tag] ");
                if update
                    && let Some(arrow) = words.iter().position(|word| *word == "->")
                    && arrow > 0
                    && arrow + 2 == words.len()
                    && let Some(to) = words.get(arrow + 1)
                {
                    let from = words[arrow - 1];
                    if identifier(from)
                        && identifier(to)
                        && !to.starts_with("origin/")
                        && !to.starts_with("refs/remotes/origin/")
                    {
                        facts.push(Change::Push {
                            repository: repo.clone(),
                            from: from.into(),
                            to: (*to).into(),
                        });
                    }
                }
            }
        }
        if commands.contains(&Command::PullRequest)
            && let Some((url, number)) = line.trim().rsplit_once("/pull/")
            && let Some(repo) = repository(url)
            && let Ok(number) = number.parse::<u64>()
            && number > 0
        {
            facts.push(Change::PullRequest {
                repository: repo,
                number,
            });
        }
        if facts.len() >= 128 {
            break;
        }
    }
    facts
}

pub(super) fn finish(
    observation: &mut Observation,
    output: &Value,
    error: bool,
    start: Option<i64>,
    end: Option<i64>,
    result_digest: String,
) {
    observation.result_digest = Some(result_digest);
    let mut result = ResultEvidence {
        error,
        ..Default::default()
    };
    let mut budget = MAX_BYTES;
    if decode(
        output,
        0,
        &mut budget,
        &mut result,
        observation.formatter,
        observation.envelopes,
    )
    .is_none()
    {
        if error {
            observation.waste = observation.waste.or(Some(WasteKind::OtherFailure));
        }
        return;
    }
    observation.settled = !observation.background && !result.running;
    observation.completed =
        observation.settled && !result.error && result.exit.is_none_or(|code| code == 0);
    if result.is_wait {
        observation.orchestration = true;
    }
    if observation.orchestration {
        observation.waste = Some(WasteKind::OrchestrationWait);
        observation.wait = result.nested_wait.or_else(|| {
            start
                .zip(end)
                .filter(|(start, end)| end >= start)
                .map(|(start, end)| WaitInterval {
                    started_at_ms: start,
                    completed_at_ms: end,
                })
        });
    } else {
        let mut substantive = observation
            .commands
            .iter()
            .filter(|command| **command != Command::Setup);
        let search = substantive.next();
        observation.empty_search = substantive.next().is_none()
            && (matches!(search, Some(Command::Search(_, _)))
                && result.exit == Some(1)
                && result.text.trim().is_empty()
                || matches!(search, Some(Command::Search(_, SearchTool::Native)))
                    && !result.error
                    && result.exit.is_none_or(|code| code == 0)
                    && matches!(result.text.trim(), "No matches found" | "No files found"));
        if !observation.empty_search {
            observation.waste = failure(&observation.commands, &result);
        }
        if observation.waste.is_none() && observation.commands.contains(&Command::Sleep) {
            observation.waste = Some(WasteKind::Sleep);
        }
    }
    if observation.completed {
        observation.changes = changes(&observation.commands, &result.text);
    }
}

/// Called once per complete native record, on the same bytes as the scan ledger.
pub(crate) fn capture(
    index: &mut FileIndex,
    harness: &str,
    record: &Value,
    offset: u64,
    bytes: &[u8],
) {
    let hash = digest(bytes);
    let time = record_time(record);
    if harness == "claude" {
        // Request blobs contain copies of assistant tool blocks, without their
        // native message ownership. They are not additional calls or mutations.
        if !matches!(field(record, "type"), Some("assistant" | "user")) {
            return;
        }
        let Some(items) = record.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for (block, item) in items.iter().enumerate() {
            match field(item, "type") {
                Some("tool_use") => {
                    if let Some(event) = index
                        .events
                        .iter()
                        .enumerate()
                        .rev()
                        .take_while(|(_, event)| event.o == offset)
                        .find(|(_, event)| event.b == block as u32 && event.k == Kind::Tool)
                        .map(|(index, _)| index)
                    {
                        let step = record
                            .pointer("/message/id")
                            .and_then(Value::as_str)
                            .map(|id| ActivityStepId::Claude(id.into()));
                        let mut observation = call(
                            field(item, "name").unwrap_or(""),
                            &item["input"],
                            step,
                            hash.clone(),
                        );
                        bind_source_context(&mut observation, index.cwd.as_deref());
                        index
                            .event_mut(event)
                            .expect("located event")
                            .activity_observation = Some(observation);
                    }
                }
                Some("tool_result") => complete(
                    index,
                    field(item, "tool_use_id"),
                    &item["content"],
                    item.get("is_error").and_then(Value::as_bool) == Some(true),
                    offset,
                    time,
                    &hash,
                ),
                _ => {}
            }
        }
    } else if harness == "codex" && field(record, "type") == Some("response_item") {
        let payload = &record["payload"];
        match field(payload, "type") {
            Some("function_call" | "custom_tool_call" | "local_shell_call") => {
                if let Some(event) = index
                    .events
                    .iter()
                    .enumerate()
                    .rev()
                    .take_while(|(_, event)| event.o == offset)
                    .find(|(_, event)| event.k == Kind::Tool)
                    .map(|(index, _)| index)
                {
                    let name = field(payload, "name").unwrap_or("shell");
                    // Only existing straight-line code-mode decoding can prove a
                    // command in a custom exec script. Arbitrary JS stays unknown.
                    let decoded_input;
                    let input = if name == "exec" {
                        decoded_input = field(payload, "input")
                            .filter(|text| {
                                text.len() <= MAX_BYTES
                                    && (crate::events::result_source(text)
                                        || crate::events::activity_output_source(text))
                            })
                            .and_then(|script| crate::events::script_command(script).map(|command| {
                                serde_json::json!({"cmd": command, "workdir": crate::events::script_workdir(script)})
                            }))
                            .unwrap_or(Value::Null);
                        &decoded_input
                    } else {
                        payload.get("arguments").unwrap_or(&Value::Null)
                    };
                    let mut observation = call(
                        if name == "exec" { "exec_command" } else { name },
                        input,
                        codex_step(payload),
                        hash.clone(),
                    );
                    bind_source_context(&mut observation, index.cwd.as_deref());
                    observation.formatter = index.events[event].native_direct_call
                        || (name == "exec"
                            && field(payload, "input").is_some_and(crate::events::result_source));
                    // An arbitrary script's printed JSON is opaque. It cannot
                    // forge an exec outcome, nor leave a terminal script's
                    // mutation barrier open just because its data is JSON.
                    observation.envelopes = name != "exec"
                        || field(payload, "input").is_some_and(crate::events::result_source);
                    if name == "exec" && input.is_null() {
                        observation.commands = vec![Command::Unknown];
                    }
                    index
                        .event_mut(event)
                        .expect("located event")
                        .activity_observation = Some(observation);
                }
            }
            Some("function_call_output" | "custom_tool_call_output") => complete(
                index,
                field(payload, "call_id"),
                &payload["output"],
                payload.get("is_error").and_then(Value::as_bool) == Some(true),
                offset,
                time,
                &hash,
            ),
            _ => {}
        }
    }
}

/// An explicit complete identity is evidence. A turn ID, item ID, ordinal or
/// neighboring usage record alone cannot identify the model response.
fn codex_step(payload: &Value) -> Option<ActivityStepId> {
    let ids = ["thread_id", "session_id", "turn_id", "response_id"]
        .map(|key| field(payload, key).filter(|id| !id.is_empty() && id.len() <= 4096));
    let [Some(thread), Some(session), Some(turn), Some(response)] = ids else {
        return None;
    };
    Some(ActivityStepId::Codex([
        thread.into(),
        session.into(),
        turn.into(),
        response.into(),
    ]))
}

fn bind_source_context(observation: &mut Observation, cwd: Option<&str>) {
    for command in &mut observation.commands {
        if let Command::Read(fingerprint, _) | Command::Search(fingerprint, _) = command {
            *fingerprint = digest(
                &serde_json::to_vec(&(&*fingerprint, cwd.filter(|path| path.len() <= 4096)))
                    .expect("source read context serializes"),
            );
        }
    }
}

fn complete(
    index: &mut FileIndex,
    id: Option<&str>,
    output: &Value,
    error: bool,
    offset: u64,
    time: Option<i64>,
    hash: &str,
) {
    let Some(event) = id
        .and_then(|id| index.tool_ids.get(id))
        .filter(|events| events.len() == 1)
        .and_then(|events| events.first())
        .copied()
    else {
        return;
    };
    let event = index.event_mut(event).expect("indexed event");
    if event.r.as_ref().is_none_or(|reply| reply.o != offset) {
        return;
    }
    if let Some(observation) = &mut event.activity_observation {
        finish(observation, output, error, event.t, time, hash.into());
    }
}

/// Read history is source-local: offsets do not imply cross-source chronology.
/// Any unknown or mutating call breaks the proof for subsequent repeats.
pub(crate) fn classify_reads(
    observations: &mut [LocatedObservation],
    compactions: &[(String, u64)],
) {
    observations.sort_by(|a, b| a.position.cmp(&b.position));
    let mut histories: BTreeMap<String, BTreeMap<String, (Option<u64>, u64)>> = BTreeMap::new();
    let mut uncertain_until: BTreeMap<String, u64> = BTreeMap::new();
    let mut compact_offsets: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (source, offset) in compactions {
        compact_offsets.entry(source).or_default().push(*offset);
    }
    for offsets in compact_offsets.values_mut() {
        offsets.sort_unstable();
    }
    for row in observations {
        let reads = histories.entry(row.position.source.clone()).or_default();
        let latest = compact_offsets
            .get(row.position.source.as_str())
            .and_then(|offsets| {
                offsets
                    .partition_point(|offset| *offset < row.position.offset)
                    .checked_sub(1)
                    .map(|index| offsets[index])
            });
        let observation = &mut row.observation;
        if observation.commands.contains(&Command::Setup) {
            // Bash may retain cwd between calls. Only selections explicitly
            // established under this absolute directory prefix can survive;
            // a bare relative selection from an earlier cwd cannot match.
            reads.retain(|fingerprint, _| observation.completed && observation.commands.iter().any(|command| {
                matches!(command, Command::Read(key, _) | Command::Search(key, _) if key == fingerprint)
            }));
        }
        // Agent calls and unknown commands can mutate arbitrary paths. A
        // terminal failed result ends the overlap barrier but never proves that
        // earlier reads survived; unfinished mutators keep the barrier open.
        let mutation = observation.orchestration
            || observation.commands.contains(&Command::Setup) && !observation.completed
            || observation.commands.iter().any(|command| {
                matches!(
                    command,
                    Command::Mutation
                        | Command::Stage
                        | Command::BodyWrite
                        | Command::Ambiguous
                        | Command::Unknown
                        | Command::Commit
                        | Command::Push
                        | Command::PullRequest
                )
            });
        if mutation {
            reads.clear();
            let until = uncertain_until
                .entry(row.position.source.clone())
                .or_default();
            *until = (*until).max(if observation.settled {
                row.result_offset.unwrap_or(u64::MAX)
            } else {
                u64::MAX
            });
        }
        if uncertain_until
            .get(&row.position.source)
            .is_some_and(|until| *until >= row.position.offset)
        {
            continue;
        }
        for command in &observation.commands {
            if let Command::Read(fingerprint, _) | Command::Search(fingerprint, _) = command {
                if let Some((previous_compact, previous_end)) = reads.get(fingerprint)
                    && *previous_end < row.position.offset
                    && observation.completed
                    && observation.waste.is_none()
                {
                    observation.waste = Some(if latest > *previous_compact {
                        WasteKind::Refill
                    } else {
                        WasteKind::RedundantRead
                    });
                }
                if observation.completed
                    && let Some(end) = row.result_offset
                {
                    reads.insert(fingerprint.clone(), (latest, end));
                }
            }
        }
    }
}

pub(crate) fn fact(row: &LocatedObservation, change: Change) -> ChangeFact {
    let identity = digest(
        &serde_json::to_vec(&(&row.position, &row.observation.result_digest, &change))
            .expect("typed fact serializes"),
    );
    ChangeFact {
        identity,
        timestamp: row.timestamp,
        step_id: row.observation.step_id.clone(),
        position: SourcePosition {
            source: row.position.source.clone(),
            offset: row.result_offset.unwrap_or(row.position.offset),
        },
        record_digest: row
            .observation
            .result_digest
            .clone()
            .expect("changes require a result"),
        change,
    }
}

#[cfg(test)]
mod tests;
