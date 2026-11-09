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
    ChecksPending,
    PathNotFound,
    MalformedCommand,
    ToolInputRejected,
    Timeout,
    ExternalFailure,
    OtherFailure,
    MergeConflict,
    Refill,
    RedundantRead,
    Sleep,
    /// An identical call repeated after a failure, with no intervening state
    /// change, that did not turn a verification failure into a success.
    Retry,
    /// An identical verification or PR-checks call that succeeded on repeat
    /// after failing, with no intervening state change.
    FlakyRerun,
}

/// What a step did, in first-matching precedence order (declaration order).
/// `Recover` is not computed by rules version 8 and is never assigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Activity {
    Coordinate,
    Wait,
    Integrate,
    Verify,
    Recover,
    Implement,
    Explore,
    Setup,
    Report,
    Think,
}

/// Typed evidence that a call addressed another agent or session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum AgentCall {
    /// Spawns, messages or steers another agent or session.
    Coordinate,
    /// Blocks on another agent's result.
    Wait,
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
            Self::OrchestrationWait | Self::ChecksPending => WasteCategory::Orchestration,
            Self::PathNotFound
            | Self::MalformedCommand
            | Self::ToolInputRejected
            | Self::Timeout => WasteCategory::Tooling,
            Self::ExternalFailure | Self::FlakyRerun => WasteCategory::External,
            Self::Refill => WasteCategory::Context,
            Self::OtherFailure
            | Self::MergeConflict
            | Self::RedundantRead
            | Self::Sleep
            | Self::Retry => WasteCategory::Unmapped,
        }
    }

    /// A failed call, as opposed to waiting, context or repeat waste.
    pub(crate) fn is_failure(self) -> bool {
        matches!(
            self,
            Self::PathNotFound
                | Self::MalformedCommand
                | Self::ToolInputRejected
                | Self::Timeout
                | Self::ExternalFailure
                | Self::OtherFailure
                | Self::MergeConflict
        )
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
    /// Blocks on a background process without sleeping. Label evidence only.
    Wait,
    /// Read-only CI status: `gh run view`, `gh run list`.
    CiStatus,
    /// Starts CI outside the repository: `gh run rerun`, `gh workflow run`.
    CiStart,
    /// A message to the user or Manager.
    Report,
    /// A recognized verification command. Like any unknown program, it may
    /// write files, so it still breaks read proofs.
    Verify,
    /// git merge, rebase, cherry-pick or pull.
    Merge,
    Mutation,
    Ambiguous,
    Unknown,
}

/// Allowlisted programs whose `name: ` diagnostic line may identify a failed
/// component of a compound command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Program {
    Cat,
    Rg,
    Grep,
    Ls,
    Sed,
    Head,
    Tail,
    Cd,
    Git,
    Find,
}

impl Program {
    const ALL: [Self; 10] = [
        Self::Cat,
        Self::Rg,
        Self::Grep,
        Self::Ls,
        Self::Sed,
        Self::Head,
        Self::Tail,
        Self::Cd,
        Self::Git,
        Self::Find,
    ];

    fn of(words: &[String]) -> Option<Self> {
        Some(match words.first()?.as_str() {
            "cat" => Self::Cat,
            "rg" => Self::Rg,
            "grep" => Self::Grep,
            "ls" => Self::Ls,
            "sed" => Self::Sed,
            "head" => Self::Head,
            "tail" => Self::Tail,
            "cd" => Self::Cd,
            "git" => Self::Git,
            "find" => Self::Find,
            _ => return None,
        })
    }

    fn diagnostic(self) -> &'static str {
        match self {
            Self::Cat => "cat: ",
            Self::Rg => "rg: ",
            Self::Grep => "grep: ",
            Self::Ls => "ls: ",
            Self::Sed => "sed: ",
            Self::Head => "head: ",
            Self::Tail => "tail: ",
            Self::Cd => "cd: ",
            Self::Git => "git: ",
            Self::Find => "find: ",
        }
    }
}

/// The command whose exit status the result carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Status {
    Verify,
    PrChecks,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Native {
    Read,
    Edit,
}

/// Typed failure evidence, separate from the read/search fingerprints that
/// prove redundant reads. Contains no command text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Attribution {
    /// Command components, counting each pipeline stage. Zero when unattributed.
    components: usize,
    programs: Vec<Program>,
    merges: usize,
    /// Set only when exactly one substantive pipeline owns the final status;
    /// a pipeline's status belongs to its last stage.
    status: Option<Status>,
    native: Option<Native>,
}

struct Shell {
    commands: Vec<Command>,
    attribution: Attribution,
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
    /// Evidence for `activity()` alone: it is never read by state-change,
    /// read, failure or retry proofs, so it cannot make a call look known.
    label_commands: Vec<Command>,
    attribution: Attribution,
    orchestration: bool,
    /// Distinct from `orchestration`, which also marks blocking agent calls as
    /// waste: coordination and waiting are different activities.
    agent: Option<AgentCall>,
    /// SHA-256 of the tool name and exact input (the command and working
    /// directory for shell tools). None when the input is not decoded.
    fingerprint: Option<String>,
    claude_bash: bool,
    formatter: bool,
    envelopes: bool,
    failure_results: bool,
    completed: bool,
    settled: bool,
    exit_code: Option<i32>,
    background: bool,
    pub(crate) waste: Option<WasteKind>,
    pub(crate) wait: Option<WaitInterval>,
    pub(crate) empty_search: bool,
    pub(crate) verify_failure: bool,
    pub(crate) silent_exit: bool,
    pub(crate) changes: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolCallEvidence {
    pub(crate) tool: String,
    /// Raw local file target, held separately from `Observation` so activity
    /// evidence remains body- and path-free.
    pub(crate) target_path: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct LocatedObservation {
    pub(crate) position: SourcePosition,
    pub(crate) timestamp: Option<i64>,
    pub(crate) result_timestamp: Option<i64>,
    pub(crate) result_offset: Option<u64>,
    pub(crate) tool: Option<ToolCallEvidence>,
    pub(crate) observation: Observation,
}

impl Observation {
    pub(crate) fn command_tag(&self) -> CommandTag {
        match self.commands.first().unwrap_or(&Command::Unknown) {
            Command::Read(..) => CommandTag::Read,
            Command::Search(..) => CommandTag::Search,
            Command::Setup => CommandTag::Setup,
            Command::Stage => CommandTag::Stage,
            Command::BodyWrite => CommandTag::BodyWrite,
            Command::Inspect => CommandTag::Inspect,
            Command::Commit => CommandTag::Commit,
            Command::Push => CommandTag::Push,
            Command::PullRequest => CommandTag::PullRequest,
            Command::PrChecks => CommandTag::PrChecks,
            Command::Watch => CommandTag::Watch,
            Command::Sleep => CommandTag::Sleep,
            Command::Wait => CommandTag::Wait,
            Command::CiStatus => CommandTag::CiStatus,
            Command::CiStart => CommandTag::CiStart,
            Command::Report => CommandTag::Report,
            Command::Verify => CommandTag::Verify,
            Command::Merge => CommandTag::Merge,
            Command::Mutation => CommandTag::Mutation,
            Command::Ambiguous => CommandTag::Ambiguous,
            Command::Unknown => CommandTag::Unknown,
        }
    }

    pub(crate) fn ok(&self) -> Option<bool> {
        self.settled.then_some(self.completed)
    }

    pub(crate) fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// The activity of this call: the first match among its commands and agent
    /// evidence. Unknown and ambiguous commands prove nothing.
    pub(crate) fn activity(&self) -> Option<Activity> {
        let pending = self.waste == Some(WasteKind::ChecksPending);
        let agent = self.agent.map(|call| match call {
            AgentCall::Coordinate => Activity::Coordinate,
            AgentCall::Wait => Activity::Wait,
        });
        let commands = self.commands.iter().chain(&self.label_commands);
        let commands = commands.filter_map(|command| match command {
            Command::Sleep | Command::Watch | Command::Wait => Some(Activity::Wait),
            Command::PrChecks if pending => Some(Activity::Wait),
            Command::PrChecks | Command::Verify | Command::CiStatus | Command::CiStart => {
                Some(Activity::Verify)
            }
            Command::Commit
            | Command::Push
            | Command::PullRequest
            | Command::Merge
            | Command::Stage => Some(Activity::Integrate),
            Command::BodyWrite | Command::Mutation => Some(Activity::Implement),
            Command::Read(..) | Command::Search(..) | Command::Inspect => Some(Activity::Explore),
            Command::Setup => Some(Activity::Setup),
            Command::Report => Some(Activity::Report),
            Command::Ambiguous | Command::Unknown => None,
        });
        agent.into_iter().chain(commands).min()
    }

    /// Agent calls and unknown commands can mutate arbitrary paths.
    fn changes_state(&self) -> bool {
        self.orchestration
            || self.commands.contains(&Command::Setup) && !self.completed
            || self.commands.iter().any(|command| {
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
                        | Command::Verify
                        | Command::CiStart
                        | Command::Merge
                )
            })
    }

    fn failed(&self) -> bool {
        self.verify_failure || self.waste.is_some_and(WasteKind::is_failure)
    }

    /// An orchestration call whose result record was not decoded here (for
    /// example a Codex `wait_agent` completed through another native record)
    /// keeps the exact call and reply timestamps the Event Index already
    /// recorded for the same call. No interval is invented without both.
    pub(crate) fn fill_wait_from_reply(&mut self, start: Option<i64>, end: Option<i64>) {
        if self.orchestration
            && self.wait.is_none()
            && let Some((started_at_ms, completed_at_ms)) =
                start.zip(end).filter(|(start, end)| end >= start)
        {
            self.wait = Some(WaitInterval {
                started_at_ms,
                completed_at_ms,
            });
        }
    }
}

pub(crate) fn unknown_boundary(bytes: &[u8]) -> Observation {
    call("unknown", &Value::Null, None, digest(bytes))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct Pipeline {
    stages: Vec<Vec<String>>,
    /// A harmless redirect (`2>&1`, `>/dev/null`, ...) was dropped.
    redirected: bool,
}

#[derive(Default)]
struct Lexer {
    pipelines: Vec<Pipeline>,
    stages: Vec<Vec<String>>,
    words: Vec<String>,
    word: String,
    started: bool,
    /// The current word holds an unquoted `>`.
    redirect: bool,
    /// The previous word was a bare output operator awaiting `/dev/null`.
    pending: bool,
    redirected: bool,
    total: usize,
}

impl Lexer {
    fn end_word(&mut self) -> Option<()> {
        if !self.started {
            return Some(());
        }
        let word = std::mem::take(&mut self.word);
        self.started = false;
        let redirect = std::mem::take(&mut self.redirect);
        if self.pending {
            self.pending = false;
            return (!redirect && word == "/dev/null").then_some(());
        }
        if redirect {
            // Only these discard or merge output. A redirect to any other
            // file writes it, so the whole command stays unattributed.
            match word.as_str() {
                "2>&1" | "1>&2" | ">/dev/null" | "2>/dev/null" | "&>/dev/null" => {}
                ">" | "1>" | "2>" | "&>" => self.pending = true,
                _ => return None,
            }
            self.redirected = true;
        } else {
            self.words.push(word);
        }
        Some(())
    }

    /// `piped` is true at a `|`, where an empty stage is a syntax error.
    fn end_stage(&mut self, piped: bool) -> Option<()> {
        self.end_word()?;
        if self.pending {
            return None;
        }
        if self.words.is_empty() {
            return (!piped && self.stages.is_empty()).then_some(());
        }
        self.total += 1;
        if self.total > MAX_COMPONENTS {
            return None;
        }
        self.stages.push(std::mem::take(&mut self.words));
        Some(())
    }

    fn end_pipeline(&mut self) -> Option<()> {
        self.end_stage(false)?;
        if !self.stages.is_empty() {
            self.pipelines.push(Pipeline {
                stages: std::mem::take(&mut self.stages),
                redirected: std::mem::take(&mut self.redirected),
            });
        }
        Some(())
    }
}

/// A deliberately small shell grammar: sequences of pipelines whose stages are
/// plain words, with only harmless output redirects. Expansions, subshells,
/// braces, input redirects, heredocs and background jobs invalidate command
/// attribution and mutation knowledge.
fn script(text: &str) -> Option<Vec<Pipeline>> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let mut lexer = Lexer::default();
    let mut quote = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        // Single-quoted shell text is literal, including dollars/backticks.
        if quote != Some('\'') && (c == '$' || c == '`') {
            return None;
        }
        if c == '\\' && quote != Some('\'') {
            let next = chars.next()?;
            if next != '\n' {
                lexer.word.push(next);
                lexer.started = true;
            }
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                lexer.word.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                lexer.started = true;
            }
            '<' | '(' | ')' | '{' | '}' => return None,
            '>' => {
                lexer.word.push(c);
                lexer.started = true;
                lexer.redirect = true;
            }
            '&' if chars.next_if_eq(&'&').is_some() => lexer.end_pipeline()?,
            // Only the `&` of `&>` and `>&`; anything else is a background job.
            '&' if (!lexer.started && chars.peek() == Some(&'>'))
                || (lexer.redirect && lexer.word.ends_with('>')) =>
            {
                lexer.word.push(c);
                lexer.started = true;
            }
            '&' => return None,
            '|' if chars.next_if_eq(&'|').is_some() => lexer.end_pipeline()?,
            '|' => lexer.end_stage(true)?,
            ';' | '\n' => lexer.end_pipeline()?,
            c if c.is_whitespace() => lexer.end_word()?,
            _ => {
                lexer.word.push(c);
                lexer.started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    lexer.end_pipeline()?;
    (!lexer.pipelines.is_empty()).then_some(lexer.pipelines)
}

/// Plain command sequences only: pipelines and redirects are not accepted.
fn components(text: &str) -> Option<Vec<Vec<String>>> {
    let mut commands = Vec::new();
    for pipeline in script(text)? {
        let [_] = pipeline.stages.as_slice() else {
            return None;
        };
        if pipeline.redirected {
            return None;
        }
        commands.extend(pipeline.stages);
    }
    Some(commands)
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
            Some("checkout" | "switch" | "reset" | "restore" | "apply") => Command::Mutation,
            Some("merge" | "rebase" | "cherry-pick" | "pull") => Command::Merge,
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
            ["run", "watch", ..] => return Command::Watch,
            ["run", "rerun", ..] | ["workflow", "run", ..] => return Command::CiStart,
            ["run", "view", rest @ ..] if rest.contains(&"--log-failed") => {
                return Command::Verify;
            }
            ["run", "view" | "list", ..] => return Command::CiStatus,
            ["pr", "view" | "diff" | "list" | "status", ..] | ["issue", "view" | "list", ..] => {
                return Command::Inspect;
            }
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
        _ if verify(&args) => Command::Verify,
        ["sleep", ..] => Command::Sleep,
        ["rm" | "mv" | "cp" | "touch" | "mkdir" | "rmdir" | "tee", ..] => Command::Mutation,
        _ => Command::Unknown,
    }
}

/// A tight allowlist of verification commands whose non-zero exit reports the
/// state of the checked work rather than a tooling failure.
fn verify(args: &[&str]) -> bool {
    match args {
        ["cargo", "test" | "check" | "build" | "clippy", ..] => true,
        ["cargo", "fmt", rest @ ..] => rest.contains(&"--check"),
        ["pytest", ..] => true,
        ["go", "test", ..] => true,
        ["npm", rest @ ..] => {
            let rest = match rest {
                ["--prefix", _, rest @ ..] => rest,
                rest => rest,
            };
            matches!(
                rest,
                ["test", ..] | ["run", "test" | "typecheck" | "lint", ..]
            )
        }
        _ => false,
    }
}

/// Unwrap only literal shell invocations. A script remains subject to the same
/// grammar and component/depth bounds; shell expansions never become evidence.
fn shell_commands(text: &str, depth: usize) -> Option<Shell> {
    if depth > MAX_DEPTH {
        return None;
    }
    let (normalized, body_write) = literal_pr_body(text)?;
    if let Some(shell) = read_pipeline(&normalized, depth) {
        return Some(shell);
    }
    let mut commands = Vec::new();
    if body_write {
        commands.push(Command::BodyWrite);
    }
    let mut attribution = Attribution {
        components: usize::from(body_write),
        ..Attribution::default()
    };
    // The status of each pipeline that is not a literal `cd`.
    let mut owners = Vec::new();
    let mut directories = Vec::new();
    for pipeline in script(&normalized)? {
        // A general pipeline or redirect supplies failure evidence only. It
        // never proves a read, so it breaks read history like any unknown.
        if pipeline.stages.len() > 1 || pipeline.redirected {
            for words in &pipeline.stages {
                attribution.note(words, &command(words));
            }
            let last = command(pipeline.stages.last()?);
            owners.push(Status::of(&last));
            commands.push(Command::Unknown);
            continue;
        }
        let words = &pipeline.stages[0];
        let args: Vec<_> = words.iter().map(String::as_str).collect();
        let mut kinds = match args.as_slice() {
            [
                "bash" | "/bin/bash" | "sh" | "/bin/sh",
                "-c" | "-lc",
                script,
            ] => {
                let nested = shell_commands(script, depth + 1)?;
                attribution.components += nested.attribution.components;
                attribution.programs.extend(nested.attribution.programs);
                attribution.merges += nested.attribution.merges;
                owners.push(nested.attribution.status);
                nested.commands
            }
            ["cd", path] if path.starts_with('/') => {
                directories.push((*path).to_owned());
                attribution.note(words, &Command::Setup);
                vec![Command::Setup]
            }
            _ => {
                let kind = command(words);
                attribution.note(words, &kind);
                owners.push(Status::of(&kind));
                vec![kind]
            }
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
    if let [status] = owners.as_slice() {
        attribution.status = *status;
    }
    Some(Shell {
        commands,
        attribution,
    })
}

/// A single read/search piped only through bounded text selectors keeps one
/// complete-pipeline fingerprint. No shell expansion, arbitrary filter or
/// output-producing side command.
fn read_pipeline(normalized: &str, depth: usize) -> Option<Shell> {
    let pipeline = normalized.split(" | ").collect::<Vec<_>>();
    if pipeline.len() < 2 || pipeline.len() > MAX_COMPONENTS {
        return None;
    }
    let mut first = shell_commands(pipeline[0], depth + 1)?;
    if first.commands.len() != 1
        || !matches!(
            first.commands[0],
            Command::Read(_, _) | Command::Search(_, _)
        )
    {
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
        first.attribution.programs.extend(Program::of(words));
    }
    let fingerprint = digest(normalized.as_bytes());
    match &mut first.commands[0] {
        Command::Read(key, _) | Command::Search(key, _) => *key = fingerprint,
        _ => unreachable!(),
    }
    // One read for failure purposes; the selectors never own a diagnostic.
    first.attribution.status = None;
    Some(first)
}

impl Attribution {
    fn note(&mut self, words: &[String], command: &Command) {
        self.components += 1;
        self.programs.extend(Program::of(words));
        self.merges += usize::from(*command == Command::Merge);
    }
}

impl Status {
    fn of(command: &Command) -> Option<Self> {
        match command {
            Command::Verify => Some(Self::Verify),
            Command::PrChecks => Some(Self::PrChecks),
            _ => None,
        }
    }
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

pub(crate) fn tool_evidence(name: &str, input: &Value) -> ToolCallEvidence {
    let tool = name
        .strip_prefix("functions.")
        .or_else(|| name.strip_prefix("tools."))
        .unwrap_or(name)
        .to_owned();
    let input = input_value(input).unwrap_or(Cow::Owned(Value::Null));
    let target_path = match tool.as_str() {
        "Read" | "read_file" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(Value::as_str),
        "Grep" | "Glob" => input.get("path").and_then(Value::as_str),
        "apply_patch" => input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .or_else(|| {
                let patch = input.get("patch").and_then(Value::as_str)?;
                let targets: Vec<_> = patch
                    .lines()
                    .filter_map(|line| {
                        line.strip_prefix("*** Update File: ")
                            .or_else(|| line.strip_prefix("*** Add File: "))
                            .or_else(|| line.strip_prefix("*** Delete File: "))
                    })
                    .collect();
                (targets.len() == 1).then_some(targets[0])
            }),
        _ => None,
    }
    .filter(|path| path.len() <= 4096)
    .map(str::to_owned);
    ToolCallEvidence { tool, target_path }
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
    let mut attribution = Attribution::default();
    let mut commands = match name {
        "Bash" | "bash" | "shell" | "exec_command" => {
            match input
                .get("command")
                .or_else(|| input.get("cmd"))
                .and_then(Value::as_str)
                .and_then(|text| shell_commands(text, 0))
            {
                Some(shell) => {
                    attribution = shell.attribution;
                    shell.commands
                }
                None => vec![Command::Ambiguous],
            }
        }
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
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "apply_patch" => {
            vec![Command::Mutation]
        }
        "Agent" | "Task" | "wait_agent" | "sleep" => Vec::new(),
        "AskUserQuestion" | "SubagentHandback" => vec![Command::Report],
        "WebFetch" | "WebSearch" | "web_search" | "_web_run" | "ToolSearch" | "Skill" => {
            vec![Command::Inspect]
        }
        _ => vec![Command::Unknown],
    };
    // Monitor runs a script, so it stays a state-change barrier. A native sleep
    // changes nothing; neither proves what it waited for, so neither is waste.
    let label_commands = match name {
        "Monitor" | "sleep" => vec![Command::Wait],
        _ => Vec::new(),
    };
    if attribution.components == 0 && commands != [Command::Ambiguous] {
        attribution.components = commands.len();
    }
    attribution.native = match name {
        "Read" | "read_file" => Some(Native::Read),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Some(Native::Edit),
        _ => None,
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
    let agent = match name {
        "Agent" | "Task" | "SendMessage" | "spawn_agent" | "send_message" | "send_input"
        | "followup_task" | "list_agents" | "interrupt_agent" => Some(AgentCall::Coordinate),
        "wait_agent" => Some(AgentCall::Wait),
        _ => None,
    };
    let command = input
        .get("command")
        .or_else(|| input.get("cmd"))
        .filter(|command| command.is_string());
    let workdir = input.get("workdir").or_else(|| input.get("cwd"));
    let identity = if matches!(name, "Bash" | "bash" | "shell" | "exec_command") {
        command.map(|command| serde_json::to_vec(&(name, command, workdir)))
    } else {
        (!input.is_null()).then(|| serde_json::to_vec(&(name, &*input)))
    };
    let fingerprint = identity.map(|bytes| digest(&bytes.expect("call identity serializes")));
    Observation {
        call_digest,
        result_digest: None,
        step_id,
        commands,
        label_commands,
        attribution,
        orchestration,
        agent,
        fingerprint,
        claude_bash: false,
        formatter: false,
        envelopes: false,
        failure_results: false,
        completed: false,
        settled: false,
        exit_code: None,
        background,
        waste: orchestration.then_some(WasteKind::OrchestrationWait),
        wait: None,
        empty_search: false,
        verify_failure: false,
        silent_exit: false,
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

/// Claude Code reports a failed Bash call's exit only as an `Exit code N`
/// first line. Body lines never set it.
fn claude_exit(text: &str) -> Option<i64> {
    let code = text.lines().next()?.strip_prefix("Exit code ")?;
    if code.len() <= 3 && code.bytes().all(|byte| byte.is_ascii_digit()) {
        code.parse().ok()
    } else {
        None
    }
}

/// A non-zero exit from a recognized verification command reports the state of
/// the checked work. It is a signal, not waste, unless the command itself or
/// its working directory could not run.
fn verify_failure(attribution: &Attribution, result: &ResultEvidence) -> bool {
    let Some(exit) = result
        .exit
        .filter(|code| ![0, 124, 126, 127, 143].contains(code))
    else {
        return false;
    };
    let cd_failed = result
        .text
        .lines()
        .any(|line| line.starts_with("cd: ") || line.contains(": cd: "));
    !cd_failed
        && match attribution.status {
            Some(Status::Verify) => true,
            Some(Status::PrChecks) => exit == 1,
            None => false,
        }
}

fn failure(
    commands: &[Command],
    attribution: &Attribution,
    result: &ResultEvidence,
) -> Option<WasteKind> {
    if !result.error && !result.exit.is_some_and(|code| code != 0) {
        return None;
    }
    let text = result.text.to_ascii_lowercase();
    if attribution.native.is_some() && text.contains("<tool_use_error>") {
        return Some(
            if attribution.native == Some(Native::Read) && text.contains("does not exist") {
                WasteKind::PathNotFound
            } else {
                WasteKind::ToolInputRejected
            },
        );
    }
    match result.exit {
        Some(124 | 143) => return Some(WasteKind::Timeout),
        Some(8) if attribution.status == Some(Status::PrChecks) => {
            return Some(WasteKind::ChecksPending);
        }
        _ => {}
    }
    if attribution.merges == 1
        && result
            .text
            .lines()
            .any(|line| line.starts_with("CONFLICT ("))
    {
        return Some(WasteKind::MergeConflict);
    }
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
    if attribution.components <= 1 {
        return Some(subtype);
    }
    // A compound's final status alone does not identify the failed component.
    // Accept only one allowlisted diagnostic prefix whose program appears once.
    let mut named = Program::ALL.into_iter().filter(|program| {
        result
            .text
            .lines()
            .any(|line| line.starts_with(program.diagnostic()))
    });
    let (Some(program), None) = (named.next(), named.next()) else {
        return Some(WasteKind::OtherFailure);
    };
    let matching = attribution
        .programs
        .iter()
        .filter(|candidate| **candidate == program)
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
    // Claude Code writes the header only on failed calls; a successful call's
    // output starting with the same text is program output.
    if observation.claude_bash && error && result.exit.is_none() {
        result.exit = claude_exit(&result.text);
    }
    // Multiple exactly printed exec results may report a failure without
    // proving which individual command owns a read, change or native outcome.
    let mut diagnostic_failure = None;
    let mut silent = false;
    if observation.failure_results {
        let mut budget = MAX_BYTES;
        let parts = output
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(std::slice::from_ref(output));
        for part in parts.iter().take(32) {
            let mut diagnostic = ResultEvidence::default();
            let part = part.get("text").unwrap_or(part);
            if decode(part, 0, &mut budget, &mut diagnostic, true, true).is_none() {
                continue;
            }
            // An exit 1 with no output and no attributed command says nothing
            // about its cause. It is a signal, not waste.
            if observation.attribution.components == 0
                && !diagnostic.error
                && diagnostic.exit == Some(1)
                && diagnostic.text.trim().is_empty()
            {
                silent = true;
                continue;
            }
            if let Some(waste) = failure(&[Command::Unknown], &Attribution::default(), &diagnostic)
            {
                diagnostic_failure = Some(waste);
                result.error = true;
                result.text = diagnostic.text;
                break;
            }
        }
    }
    observation.settled = !observation.background && !result.running;
    observation.exit_code = result.exit.and_then(|code| i32::try_from(code).ok());
    observation.completed =
        observation.settled && !result.error && result.exit.is_none_or(|code| code == 0);
    if result.is_wait {
        observation.orchestration = true;
        observation.agent.get_or_insert(AgentCall::Wait);
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
        observation.silent_exit = silent;
        if !observation.empty_search {
            if diagnostic_failure.is_none() && verify_failure(&observation.attribution, &result) {
                observation.verify_failure = true;
            } else if diagnostic_failure.is_some() || !silent {
                observation.waste = diagnostic_failure
                    .or_else(|| failure(&observation.commands, &observation.attribution, &result));
            }
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
                        let name = field(item, "name").unwrap_or("");
                        let tool = tool_evidence(name, &item["input"]);
                        let mut observation = call(name, &item["input"], step, hash.clone());
                        bind_source_context(&mut observation, index.cwd.as_deref());
                        observation.claude_bash = field(item, "name") == Some("Bash");
                        let event = index.event_mut(event).expect("located event");
                        event.activity_observation = Some(observation);
                        event.activity_tool = Some(tool);
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
                    let tool_name = if name == "exec" { "exec_command" } else { name };
                    let tool = tool_evidence(tool_name, input);
                    let mut observation = call(tool_name, input, codex_step(payload), hash.clone());
                    bind_source_context(&mut observation, index.cwd.as_deref());
                    observation.formatter = index.events[event].native_direct_call
                        || (name == "exec"
                            && field(payload, "input").is_some_and(crate::events::result_source));
                    // An arbitrary script's printed JSON is opaque. It cannot
                    // forge an exec outcome, nor leave a terminal script's
                    // mutation barrier open just because its data is JSON.
                    observation.failure_results = name == "exec"
                        && field(payload, "input")
                            .is_some_and(crate::events::activity_results_source);
                    observation.envelopes = name != "exec"
                        || field(payload, "input").is_some_and(|script| {
                            crate::events::result_source(script)
                                || crate::events::activity_wait_source(script)
                        });
                    if name == "exec" && input.is_null() {
                        observation.commands = vec![Command::Unknown];
                        // The script stays an unattributed barrier; only its
                        // label may come from literal commands it starts.
                        observation.label_commands = field(payload, "input")
                            .filter(|text| text.len() <= MAX_BYTES)
                            .map(crate::events::script_literal_commands)
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|text| shell_commands(text, 0))
                            .flat_map(|shell| shell.commands)
                            .collect();
                    }
                    let event = index.event_mut(event).expect("located event");
                    event.activity_observation = Some(observation);
                    event.activity_tool = Some(tool);
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
    let cwd = cwd.filter(|path| path.len() <= 4096);
    if let Some(fingerprint) = &mut observation.fingerprint {
        *fingerprint = digest(
            &serde_json::to_vec(&(&*fingerprint, cwd)).expect("source call context serializes"),
        );
    }
    for command in &mut observation.commands {
        if let Command::Read(fingerprint, _) | Command::Search(fingerprint, _) = command {
            *fingerprint = digest(
                &serde_json::to_vec(&(&*fingerprint, cwd)).expect("source read context serializes"),
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
        // A terminal failed result ends the overlap barrier but never proves
        // that earlier reads survived; unfinished mutators keep it open.
        let mutation = observation.changes_state();
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

/// An earlier failed call whose identical repeat may be retry waste.
struct Failed {
    result_offset: u64,
    verification: bool,
}

/// Source-local, like read history. A repeat is an identical call (same
/// fingerprint) after a failed call with no state change strictly between the
/// two calls; the repeat itself is not that change. A repeated verification
/// or PR-checks failure that now succeeds is a flaky rerun; every other repeat
/// is a retry. The repeat's own failure kind gives way to the repeat marker,
/// but wait, context and sleep waste are kept.
pub(crate) fn classify_retries(observations: &mut [LocatedObservation]) {
    observations.sort_by(|a, b| a.position.cmp(&b.position));
    let mut histories: BTreeMap<String, BTreeMap<String, Failed>> = BTreeMap::new();
    for row in observations {
        let history = histories.entry(row.position.source.clone()).or_default();
        let observation = &mut row.observation;
        let failed = observation.failed();
        if let Some(fingerprint) = &observation.fingerprint
            && let Some(earlier) = history.get(fingerprint)
            && earlier.result_offset < row.position.offset
            && observation.result_digest.is_some()
            && observation.waste.is_none_or(WasteKind::is_failure)
        {
            observation.waste = Some(if earlier.verification && observation.completed {
                WasteKind::FlakyRerun
            } else {
                WasteKind::Retry
            });
        }
        if observation.changes_state() {
            history.clear();
        }
        if failed
            && let Some(fingerprint) = &observation.fingerprint
            && let Some(result_offset) = row.result_offset
        {
            history.insert(
                fingerprint.clone(),
                Failed {
                    result_offset,
                    verification: observation.verify_failure,
                },
            );
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
        timestamp: row.result_timestamp.or(row.timestamp),
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
