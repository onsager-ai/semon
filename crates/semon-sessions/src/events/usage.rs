//! Source-local usage records shared by native parsing, storage and pricing.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Tokens, field};

/// The four token categories the model serves for an exact model id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTokens {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl ModelTokens {
    pub(crate) fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }
}

/// A Claude assistant message's token report. `tokens` preserves the legacy
/// triple's input/cache-read accounting; `model_tokens` keeps cache writes
/// separate for the model JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MessageUsage {
    pub(crate) offset: u64,
    pub(crate) record_ordinal: Option<u64>,
    #[serde(default)]
    pub(crate) record_ids: BTreeSet<String>,
    pub(crate) model: Option<String>,
    pub(crate) tokens: Tokens,
    pub(crate) model_tokens: ModelTokens,
    pub(crate) billing: BillingUsage,
}

/// Per-message facts used only for price calculation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BillingUsage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write_5m: u64,
    pub(crate) cache_write_1h: u64,
    pub(crate) web_search_requests: u64,
    pub(crate) speed: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) prompt_size: u64,
    pub(crate) timestamp: Option<i64>,
    pub(crate) split_unknown: bool,
}

/// One Codex cumulative `token_count` event converted to the delta since the
/// preceding event in its file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CodexUsageEvent {
    pub(crate) model: String,
    pub(crate) tokens: ModelTokens,
    pub(crate) timestamp: Option<i64>,
}

/// Explicit per-response usage, kept separate from legacy cumulative counters.
/// Native thread counters can include a fork's inherited context.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct CodexNativeUsage {
    #[serde(default)]
    pub(super) seen: BTreeSet<String>,
    pub(super) tokens: Tokens,
    #[serde(default)]
    pub(super) by_model: BTreeMap<String, ModelTokens>,
    #[serde(default)]
    pub(super) events: Vec<CodexUsageEvent>,
    #[serde(skip)]
    pub(super) cost: Option<crate::pricing::CostBuild>,
}

impl CodexNativeUsage {
    pub(crate) fn tokens(&self) -> Tokens {
        self.tokens.clone()
    }

    pub(crate) fn compact(&self) -> Self {
        Self {
            tokens: self.tokens.clone(),
            ..Self::default()
        }
    }

    pub(crate) fn observe(
        slot: &mut Option<Self>,
        payload: &Value,
        model: String,
        time: Option<i64>,
    ) -> bool {
        let Some((key, tokens, counts)) = Self::decode(payload) else {
            return false;
        };
        let native = slot.get_or_insert_with(Default::default);
        if !native.seen.insert(key) {
            return false;
        }
        native.tokens.input += tokens.input;
        native.tokens.cached_input += tokens.cached_input;
        native.tokens.output += tokens.output;
        native.tokens.total += tokens.total;
        native.tokens.reasoning_output += tokens.reasoning_output;
        native
            .by_model
            .entry(model.clone())
            .or_default()
            .add(&counts);
        let event = CodexUsageEvent {
            model,
            tokens: counts,
            timestamp: time,
        };
        if let Some(cost) = &mut native.cost {
            cost.codex(&event);
        } else {
            native.events.push(event);
        }
        true
    }

    /// Shared validation for indexing and receipt-checked representation migration.
    pub(crate) fn decode(payload: &Value) -> Option<(String, Tokens, ModelTokens)> {
        let usage = payload.get("usage")?;
        let identity =
            ["thread_id", "session_id", "turn_id", "response_id"].map(|key| field(payload, key));
        if identity.iter().any(|id| id.is_none_or(str::is_empty)) {
            return None;
        }
        let [
            Some(input),
            Some(output),
            Some(total),
            Some(cache_read),
            Some(cache_write),
            Some(reasoning),
        ] = [
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "reasoning_output_tokens",
        ]
        .map(|key| usage.get(key).and_then(Value::as_u64))
        else {
            return None;
        };
        // Serialize the exact tuple, avoiding delimiter collisions or timing joins.
        let key = serde_json::to_string(&identity).expect("string tuple serializes");
        let counts = ModelTokens {
            input: input.saturating_sub(cache_read),
            output,
            cache_read,
            cache_write,
        };
        Some((
            key,
            Tokens {
                input,
                cached_input: cache_read,
                output,
                total,
                reasoning_output: reasoning,
            },
            counts,
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RateLimitWindow {
    pub(crate) minutes: u64,
    pub(crate) used_percent: f64,
    pub(crate) resets_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RateLimits {
    pub(crate) recorded_at: i64,
    pub(crate) windows: Vec<RateLimitWindow>,
}
