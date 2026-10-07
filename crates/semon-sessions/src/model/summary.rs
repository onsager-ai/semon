//! Restart reuse of stable descriptions, not a serialized served model.
//! This allowlist cannot acquire message/tool bodies or liveness when Session
//! gains a field. Current source ledgers and all description dependencies must
//! match before a summary is used. Relationship joins and liveness still rebuild.
use super::*;
use sha2::{Digest, Sha256};

/// Bump when description, identity or pricing derivation rules change.
pub(super) const VERSION: u32 = 1;

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
