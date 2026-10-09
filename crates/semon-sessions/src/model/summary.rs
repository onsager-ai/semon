//! Restart reuse of stable descriptions, not a serialized served model.
//! This allowlist cannot acquire message/tool bodies or liveness when Session
//! gains a field. Current source ledgers and all description dependencies must
//! match before a summary is used. Relationship joins and liveness still rebuild.
use super::*;
use sha2::{Digest, Sha256};

/// Bump when description, identity or pricing derivation rules change.
pub(super) const VERSION: u32 = 2;

pub(super) fn fingerprint(inputs: &DescriptionInputs) -> String {
    format!("{:x}", Sha256::digest(format!("{inputs:?}").as_bytes()))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Summary {
    sources: Vec<(PathBuf, FileRevision)>,
    names: BTreeSet<String>,
    exact_tokens: crate::Tokens,
    first: Option<i64>,
    last_time: Option<i64>,
    name: String,
    model: String,
    effort: Option<String>,
    tokens: [f64; 3],
    tokens_by_model: BTreeMap<String, events::ModelTokens>,
    claude_usage: Option<crate::ClaudeUsageEvidence>,
    copilot: Option<crate::copilot::CopilotEvidence>,
    incomplete_usage_days: BTreeSet<i64>,
    incomplete_usage_time: bool,
    cost: crate::pricing::Cost,
    reported_runs: Vec<ReportedRun>,
    cost_check: Vec<CostCheck>,
    rate_limits: Option<events::RateLimits>,
    codex_history: Option<CodexHistory>,
    repo: Option<String>,
    branch: Option<String>,
    start: i64,
    last: i64,
    busy: Vec<(i64, i64)>,
    role: bool,
}

impl Summary {
    pub(super) fn of(session: &Sess, inputs: &DescriptionInputs) -> Self {
        Self {
            sources: inputs.files.clone(),
            names: session.names.clone(),
            exact_tokens: session.tokens.clone(),
            first: session.first,
            last_time: session.last,
            name: session.out.name.clone(),
            model: session.out.model.clone(),
            effort: session.out.effort.clone(),
            tokens: session.out.tokens,
            tokens_by_model: session.out.tokens_by_model.clone(),
            claude_usage: session.out.claude_usage.clone(),
            copilot: session.out.copilot.clone(),
            incomplete_usage_days: session.out.incomplete_usage_days.clone(),
            incomplete_usage_time: session.out.incomplete_usage_time,
            cost: session.out.cost.clone(),
            reported_runs: session.out.reported_runs.clone(),
            cost_check: session.out.cost_check.clone(),
            rate_limits: session.out.rate_limits.clone(),
            codex_history: session.out.codex_history.clone(),
            repo: session.out.repo.clone(),
            branch: session.out.branch.clone(),
            start: session.out.start,
            last: session.out.last,
            busy: session.out.busy.clone(),
            role: session.out.role,
        }
    }

    pub(super) fn apply(self, session: &mut Sess) {
        session.names = self.names;
        session.tokens = self.exact_tokens;
        session.first = self.first;
        session.last = self.last_time;
        session.out.name = self.name;
        session.out.model = self.model;
        session.out.effort = self.effort;
        session.out.tokens = self.tokens;
        session.out.tokens_by_model = self.tokens_by_model;
        session.out.claude_usage = self.claude_usage;
        session.out.copilot = self.copilot;
        session.out.incomplete_usage_days = self.incomplete_usage_days;
        session.out.incomplete_usage_time = self.incomplete_usage_time;
        session.out.cost = self.cost;
        session.out.reported_runs = self.reported_runs;
        session.out.cost_check = self.cost_check;
        session.out.rate_limits = self.rate_limits;
        session.out.codex_history = self.codex_history;
        session.out.repo = self.repo;
        session.out.branch = self.branch;
        session.out.start = self.start;
        session.out.last = self.last;
        session.out.busy = self.busy;
        session.out.role = self.role;
    }
}

/// Read-model observation state, never runtime or credential authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CatalogLifecycle {
    #[default]
    Current,
    Retained,
}

impl CatalogLifecycle {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Retained => "retained",
        }
    }
}

/// Stable metadata used by focused cold list queries. No clock or process facts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogRow {
    #[serde(default)]
    pub(crate) lifecycle: CatalogLifecycle,
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) harness: String,
    pub(crate) kind: String,
    pub(crate) native_ids: Vec<String>,
    pub(crate) parent: Option<String>,
    pub(crate) parent_source: Option<String>,
    pub(crate) repo: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) model: String,
    pub(crate) effort: Option<String>,
    pub(crate) start: Option<i64>,
    pub(crate) last: Option<i64>,
    pub(crate) tokens: [f64; 3],
    pub(crate) cost: crate::pricing::Cost,
    pub(crate) sources: Vec<CatalogSource>,
}

/// Native source identity and last observed generation. `prefix_sha256` hashes
/// every consumed byte in `[0, offset)`; `tail_sha256` hashes at most the last
/// 4 KiB of that prefix. Neither binds unread trailing bytes or a whole archive
/// object. Serving validates access and current stat before deciding whether
/// this observation is current or stale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogSource {
    pub(crate) path: PathBuf,
    pub(crate) native_id: String,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) size: u64,
    pub(crate) modified_ns: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) changed_ns: Option<i128>,
    pub(crate) offset: u64,
    pub(crate) prefix_sha256: [u8; 32],
    pub(crate) tail_sha256: [u8; 32],
}

pub(crate) const CATALOG_VERSION: u32 = 3;

pub(super) fn catalog(builder: &Builder<'_>, handoffs: &[Handoff]) -> Vec<CatalogRow> {
    let mut rows = Vec::new();
    for session in &builder.sessions {
        if session.kind == SessKind::Stub {
            continue;
        }
        let out = &session.out;
        let mut sources: Vec<_> = session
            .files
            .iter()
            .map(|position| {
                let file = &builder.files[*position];
                let (dev, ino, offset, prefix_sha256, tail_sha256, changed_ns) = file.revision;
                CatalogSource {
                    path: file.path.clone(),
                    native_id: file.id.clone(),
                    dev,
                    ino,
                    size: file.stamp.size,
                    modified_ns: file.stamp.modified_ns,
                    changed_ns,
                    offset,
                    prefix_sha256,
                    tail_sha256,
                }
            })
            .collect();
        sources.sort_by(|a, b| a.path.cmp(&b.path));
        let native_ids = sources
            .iter()
            .map(|source| source.native_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        rows.push(CatalogRow {
            lifecycle: CatalogLifecycle::Current,
            key: session.key.clone(),
            name: out.name.clone(),
            harness: out.harness.into(),
            kind: match session.kind {
                SessKind::Lineage => "session",
                SessKind::Agent => "subagent",
                SessKind::Codex => "codex-run",
                SessKind::Copilot => "copilot-session",
                SessKind::Stub => unreachable!(),
            }
            .into(),
            native_ids,
            parent: session_parent(&session.key, out, handoffs),
            parent_source: out.parent_source.clone(),
            repo: out.repo.clone(),
            branch: out.branch.clone(),
            model: out.model.clone(),
            effort: out.effort.clone(),
            start: session.first,
            last: session.last,
            tokens: out.tokens,
            cost: out.cost.clone(),
            sources,
        });
    }
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    rows
}
