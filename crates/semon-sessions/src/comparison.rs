//! Comparisons over incrementally indexed runs. Optional PR facts are supplied
//! as metadata; this module never scans transcripts or contacts GitHub.

use crate::{events::ModelTokens, model::Session};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BandThresholds {
    pub medium_lines: u64,
    pub medium_files: u64,
    pub hard_lines: u64,
    pub hard_files: u64,
    pub risky_hard_lines: u64,
    pub small_sample: usize,
}
impl Default for BandThresholds {
    fn default() -> Self {
        Self {
            medium_lines: 400,
            medium_files: 5,
            hard_lines: 1500,
            hard_files: 20,
            risky_hard_lines: 400,
            small_sample: 5,
        }
    }
}

/// Optional facts from a PR join. Missing evidence remains unknown, including
/// the difference between no negative review and a review that was not read.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkItemFacts {
    pub lines: Option<u64>,
    pub files: Option<u64>,
    pub risk: Option<bool>,
    pub first_pass_accepted: Option<bool>,
    pub review_rounds: Option<u64>,
    pub red_ci_heads: Option<u64>,
    pub ci_wait_ms: Option<u64>,
    pub allowance_per_million_input: Option<f64>,
    pub pr_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelComparisonConfig {
    pub bands: BandThresholds,
    /// Session ids are scoped to the machine loading this configuration.
    pub items: BTreeMap<String, WorkItemFacts>,
}
impl ModelComparisonConfig {
    pub(crate) fn load() -> Self {
        let Some(path) = std::env::var_os("SEMON_MODEL_COMPARISON_FILE") else {
            return Self::default();
        };
        crate::read_regular_at_most(Path::new(&path), 1024 * 1024)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|config| {
                config.bands.medium_lines <= config.bands.hard_lines
                    && config.bands.medium_files <= config.bands.hard_files
                    && config.bands.small_sample > 0
            })
            .unwrap_or_default()
    }
    fn band(&self, facts: &WorkItemFacts) -> &'static str {
        let (Some(lines), Some(files), Some(risk)) = (facts.lines, facts.files, facts.risk) else {
            return "unknown";
        };
        if lines >= self.bands.hard_lines
            || files >= self.bands.hard_files
            || risk && lines >= self.bands.risky_hard_lines
        {
            "hard"
        } else if lines >= self.bands.medium_lines || files >= self.bands.medium_files {
            "medium"
        } else {
            "easy"
        }
    }
}

#[derive(Clone, Debug)]
struct Observation {
    model: String,
    tokens: ModelTokens,
    usd: Option<f64>,
}
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) sid: String,
    harness: &'static str,
    band: &'static str,
    observations: Vec<Observation>,
    facts: WorkItemFacts,
    model_ms: Option<u64>,
    small_sample: usize,
}
impl Item {
    pub(crate) fn from_session(
        sid: &str,
        session: &Session,
        config: &ModelComparisonConfig,
        launched: bool,
    ) -> Option<Arc<Self>> {
        // A parent establishes a launched work item exactly. A precomputed
        // explicit item can also name a top-level run; other sessions do not
        // become invented work items merely because they have token counts.
        if !launched && !config.items.contains_key(sid) {
            return None;
        }
        let facts = config.items.get(sid).cloned().unwrap_or_default();
        let observations: Vec<_> = session
            .tokens_by_model
            .iter()
            .map(|(model, tokens)| Observation {
                model: model.clone(),
                tokens: tokens.clone(),
                usd: session.cost.by_model.get(model).and_then(|cost| cost.usd),
            })
            .collect();
        if observations.is_empty() {
            return None;
        }
        let model_ms = if observations.len() == 1 {
            facts.ci_wait_ms.and_then(|wait| {
                u64::try_from(session.last.saturating_sub(session.start))
                    .ok()?
                    .checked_sub(wait)
            })
        } else {
            None
        };
        Some(Arc::new(Self {
            sid: sid.into(),
            harness: session.harness,
            band: config.band(&facts),
            observations,
            facts,
            model_ms,
            small_sample: config.bands.small_sample,
        }))
    }
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.sid.capacity()
            + self.observations.capacity() * std::mem::size_of::<Observation>()
            + self
                .observations
                .iter()
                .map(|o| o.model.capacity())
                .sum::<usize>()
            + self.facts.pr_url.as_ref().map_or(0, String::capacity)
    }
}
fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}
fn valid_pr_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://github.com/") else {
        return false;
    };
    let parts: Vec<_> = rest.split('/').collect();
    parts.len() == 4
        && !parts[0].is_empty()
        && !parts[1].is_empty()
        && parts[2] == "pull"
        && parts[3].parse::<u64>().is_ok()
}

pub(crate) fn summarize<'a>(items: impl Iterator<Item = (&'a str, &'a Item)>) -> Value {
    type GroupItems<'a> = Vec<(&'a str, &'a Item, &'a Observation)>;
    let mut groups: BTreeMap<(&str, &str), GroupItems<'_>> = BTreeMap::new();
    for (machine, item) in items {
        for observation in &item.observations {
            groups
                .entry((item.band, &observation.model))
                .or_default()
                .push((machine, item, observation));
        }
    }
    let groups: Vec<Value> = groups.into_iter().map(|((band, model), items)| {
        let single = |item: &Item| item.observations.len() == 1;
        let acceptance: Vec<_> = items.iter().filter(|(_, item, _)| single(item))
            .filter_map(|(_, item, _)| item.facts.first_pass_accepted).collect();
        let review: Vec<_> = items.iter().filter(|(_, item, _)| single(item))
            .filter_map(|(_, item, _)| item.facts.review_rounds.map(|v| v as f64)).collect();
        let ci: Vec<_> = items.iter().filter(|(_, item, _)| single(item))
            .filter_map(|(_, item, _)| item.facts.red_ci_heads.map(|v| v as f64)).collect();
        let time: Vec<_> = items.iter().filter_map(|(_, item, _)| item.model_ms.map(|v| v as f64)).collect();
        let cost: Vec<_> = items.iter().filter_map(|(_, _, observation)| observation.usd.filter(|v| v.is_finite())).collect();
        let allowance: Vec<_> = items.iter().filter(|(_, item, _)| single(item))
            .filter_map(|(_, item, _)| item.facts.allowance_per_million_input.filter(|v| v.is_finite() && *v >= 0.0)).collect();
        let mut tokens = ModelTokens::default();
        for (_, _, observation) in &items { tokens.add(&observation.tokens); }
        let details: Vec<_> = items.iter().take(100).map(|(machine, item, observation)| json!({
            "sid":item.sid, "machine":machine, "harness":item.harness, "band":item.band,
            "models": item.observations.iter().map(|o| o.model.as_str()).collect::<Vec<_>>(),
            "tokens":observation.tokens, "cost_usd":observation.usd,
            "pr_url":item.facts.pr_url.as_deref().filter(|url| valid_pr_url(url)),
        })).collect();
        json!({"model":model, "band":band, "n":items.len(),
            "small_sample":items.len() < items.iter().map(|(_, item, _)| item.small_sample).max().unwrap_or(5),
            "first_pass_acceptance":if acceptance.is_empty() { None } else { Some(acceptance.iter().filter(|v| **v).count() as f64 / acceptance.len() as f64) },
            "acceptance_n":acceptance.len(), "median_review_rounds":median(review.clone()), "review_rounds_n":review.len(),
            "median_red_ci_heads":median(ci.clone()), "red_ci_n":ci.len(),
            "median_model_ms":median(time.clone()), "model_time_n":time.len(),
            "median_cost_usd":median(cost.clone()), "cost_n":cost.len(),
            "allowance_per_million_input":median(allowance.clone()), "allowance_n":allowance.len(),
            "tokens":tokens, "tokens_n":items.len(), "items":details, "items_more":items.len().saturating_sub(100),
        })
    }).collect();
    json!({"groups":groups, "scope":"Lifetime facts for launched work items active in this range", "unknown_reasons": {
        "band":"Changed lines, files and risk metadata have not all been supplied",
        "acceptance":"Optional PR facts are unavailable, or the work used more than one model",
        "review_rounds":"Optional PR review facts are unavailable, or authorship spans more than one model",
        "ci":"Optional PR check-run facts are unavailable",
        "model_time":"CI wait is unknown, or elapsed time cannot be attributed to one model",
        "allowance":"Per-run allowance delta is unavailable; shared account readings are not attributed to a run",
        "cost":"The model lacks verified prices or its billing split is unknown"
    }})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(models: usize, facts: WorkItemFacts) -> Item {
        Item {
            sid: "s".into(),
            harness: "codex",
            band: "unknown",
            observations: (0..models)
                .map(|n| Observation {
                    model: format!("model-{n}"),
                    tokens: ModelTokens::default(),
                    usd: None,
                })
                .collect(),
            facts,
            model_ms: None,
            small_sample: 5,
        }
    }
    #[test]
    fn missing_metadata_stays_unknown_and_multi_model_work_does_not_assign_acceptance() {
        let single = item(1, WorkItemFacts::default());
        let value = summarize(std::iter::once(("machine", &single)));
        assert_eq!(value["groups"][0]["n"], 1);
        assert_eq!(value["groups"][0]["band"], "unknown");
        assert!(value["groups"][0]["first_pass_acceptance"].is_null());
        assert!(value["groups"][0]["median_model_ms"].is_null());
        let mixed = item(
            2,
            WorkItemFacts {
                first_pass_accepted: Some(true),
                ..Default::default()
            },
        );
        assert!(
            summarize(std::iter::once(("machine", &mixed)))["groups"][0]["first_pass_acceptance"]
                .is_null()
        );
    }
    #[test]
    fn thresholds_are_configurable_and_missing_risk_is_unknown() {
        let mut config = ModelComparisonConfig::default();
        let mut facts = WorkItemFacts {
            lines: Some(20),
            files: Some(1),
            risk: Some(false),
            ..Default::default()
        };
        assert_eq!(config.band(&facts), "easy");
        config.bands.hard_lines = 10;
        assert_eq!(config.band(&facts), "hard");
        facts.risk = None;
        assert_eq!(config.band(&facts), "unknown");
    }
    #[test]
    fn acceptance_uses_its_own_sample_count_and_pr_links_are_exact() {
        let known = item(
            1,
            WorkItemFacts {
                first_pass_accepted: Some(true),
                pr_url: Some("javascript:alert(1)".into()),
                ..Default::default()
            },
        );
        let unknown = item(1, WorkItemFacts::default());
        let value = summarize([("m", &known), ("m", &unknown)].into_iter());
        let row = &value["groups"][0];
        assert_eq!(row["n"], 2);
        assert_eq!(row["acceptance_n"], 1);
        assert_eq!(row["first_pass_acceptance"], 1.0);
        assert!(row["items"][0]["pr_url"].is_null());
        assert!(valid_pr_url("https://github.com/onsager-ai/semon/pull/1"));
    }
}
