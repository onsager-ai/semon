//! Verified per-token prices and exact session cost calculation.
//!
//! Prices are USD per million tokens. The compatibility `pricing` object
//! keeps its original four price fields; cost calculations use the separate
//! cache-write, context, speed and tool rates below.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::events::{BillingUsage, CodexUsageEvent, MessageUsage};

pub const AS_OF: &str = "2026-09-28";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Price {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
    pub long_context: Option<LongContextPrice>,
    pub fast: Option<FastPrice>,
    pub web_search_per_1k: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LongContextPrice {
    pub threshold_tokens: u64,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FastPrice {
    pub multiplier: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
struct LegacyPrice {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

#[derive(Clone, Copy, Serialize)]
struct PricedModel {
    #[serde(flatten)]
    price: LegacyPrice,
    source: &'static str,
}

#[derive(Serialize)]
pub(crate) struct Pricing {
    as_of: &'static str,
    models: BTreeMap<&'static str, PricedModel>,
}

const ANTHROPIC: &str = "https://platform.claude.com/docs/en/about-claude/pricing";
const OPENAI: &str = "https://developers.openai.com/api/docs/pricing";

const fn legacy(input: f64, output: f64, cache_write: f64, cache_read: f64) -> LegacyPrice {
    LegacyPrice {
        input,
        output,
        cache_write,
        cache_read,
    }
}

const fn anthropic(input: f64, output: f64, cache_read: f64, fast: bool) -> Price {
    anthropic_with_fast(
        input,
        output,
        cache_read,
        if fast {
            Some(FastPrice { multiplier: 2.0 })
        } else {
            None
        },
    )
}

const fn anthropic_with_fast(
    input: f64,
    output: f64,
    cache_read: f64,
    fast: Option<FastPrice>,
) -> Price {
    Price {
        input: Some(input),
        output: Some(output),
        cache_read: Some(cache_read),
        cache_write_5m: Some(input * 1.25),
        cache_write_1h: Some(input * 2.0),
        long_context: None,
        fast,
        web_search_per_1k: Some(10.0),
    }
}

const fn openai(input: f64, output: f64, cache_write: f64, cache_read: f64, fast: bool) -> Price {
    Price {
        input: Some(input),
        output: Some(output),
        cache_read: Some(cache_read),
        cache_write_5m: Some(cache_write),
        // The page publishes one cache-write price, without a time split.
        cache_write_1h: None,
        // The page lists another context tier but not its token threshold.
        long_context: None,
        fast: if fast {
            Some(FastPrice { multiplier: 2.0 })
        } else {
            None
        },
        web_search_per_1k: Some(10.0),
    }
}

const fn unverified() -> Price {
    Price {
        input: None,
        output: None,
        cache_read: None,
        cache_write_5m: None,
        cache_write_1h: None,
        long_context: None,
        fast: None,
        web_search_per_1k: None,
    }
}

/// Exact model ids and prices read from the official pricing pages. Rows
/// marked `unverified()` remain in the legacy object for compatibility, but
/// their rates are deliberately unavailable to the new cost calculation.
const PRICE_ROWS: &[(&str, Price, LegacyPrice, &str)] = &[
    (
        "claude-fable-5-1",
        anthropic(10.0, 50.0, 0.25, false),
        legacy(10.0, 50.0, 12.5, 0.25),
        ANTHROPIC,
    ),
    (
        "claude-opus-5-5",
        anthropic(4.0, 20.0, 0.2, true),
        legacy(4.0, 20.0, 5.0, 0.2),
        ANTHROPIC,
    ),
    (
        "claude-sonnet-5",
        anthropic(2.0, 10.0, 0.2, false),
        legacy(2.0, 10.0, 2.5, 0.2),
        ANTHROPIC,
    ),
    (
        "claude-haiku-4-5-20251001",
        anthropic(1.0, 5.0, 0.1, false),
        legacy(1.0, 5.0, 1.25, 0.1),
        ANTHROPIC,
    ),
    (
        "claude-mythos-5-1",
        anthropic(10.0, 50.0, 0.25, false),
        legacy(10.0, 50.0, 12.5, 0.25),
        ANTHROPIC,
    ),
    (
        "claude-fable-5",
        anthropic(10.0, 50.0, 1.0, false),
        legacy(10.0, 50.0, 12.5, 1.0),
        ANTHROPIC,
    ),
    (
        "claude-mythos-5",
        anthropic(10.0, 50.0, 1.0, false),
        legacy(10.0, 50.0, 12.5, 1.0),
        ANTHROPIC,
    ),
    (
        "claude-opus-5",
        anthropic(5.0, 25.0, 0.5, true),
        legacy(5.0, 25.0, 6.25, 0.5),
        ANTHROPIC,
    ),
    (
        "claude-opus-4-8",
        anthropic(5.0, 25.0, 0.5, true),
        legacy(5.0, 25.0, 6.25, 0.5),
        ANTHROPIC,
    ),
    (
        "claude-opus-4-7",
        anthropic(5.0, 25.0, 0.5, false),
        legacy(5.0, 25.0, 6.25, 0.5),
        ANTHROPIC,
    ),
    (
        "claude-opus-4-6",
        anthropic_with_fast(5.0, 25.0, 0.5, Some(FastPrice { multiplier: 1.0 })),
        legacy(5.0, 25.0, 6.25, 0.5),
        ANTHROPIC,
    ),
    (
        "claude-opus-4-5-20251101",
        anthropic(5.0, 25.0, 0.5, false),
        legacy(5.0, 25.0, 6.25, 0.5),
        ANTHROPIC,
    ),
    (
        "claude-opus-4-1-20250805",
        anthropic(15.0, 75.0, 1.5, false),
        legacy(15.0, 75.0, 18.75, 1.5),
        ANTHROPIC,
    ),
    (
        "claude-opus-4-20250514",
        anthropic(15.0, 75.0, 1.5, false),
        legacy(15.0, 75.0, 18.75, 1.5),
        ANTHROPIC,
    ),
    (
        "claude-sonnet-4-6",
        anthropic(3.0, 15.0, 0.3, false),
        legacy(3.0, 15.0, 3.75, 0.3),
        ANTHROPIC,
    ),
    (
        "claude-sonnet-4-5-20250929",
        anthropic(3.0, 15.0, 0.3, false),
        legacy(3.0, 15.0, 3.75, 0.3),
        ANTHROPIC,
    ),
    (
        "claude-sonnet-4-20250514",
        anthropic(3.0, 15.0, 0.3, false),
        legacy(3.0, 15.0, 3.75, 0.3),
        ANTHROPIC,
    ),
    (
        "claude-3-5-haiku-20241022",
        anthropic(0.8, 4.0, 0.08, false),
        legacy(0.8, 4.0, 1.0, 0.08),
        ANTHROPIC,
    ),
    (
        "gpt-6-astra",
        openai(10.0, 50.0, 12.5, 1.0, true),
        legacy(10.0, 50.0, 12.5, 1.0),
        OPENAI,
    ),
    (
        "gpt-6-sol",
        openai(2.0, 10.0, 2.5, 0.2, true),
        legacy(2.0, 10.0, 2.5, 0.2),
        OPENAI,
    ),
    (
        "gpt-6-luna",
        openai(0.1, 0.5, 0.125, 0.01, true),
        legacy(0.1, 0.5, 0.125, 0.01),
        OPENAI,
    ),
    (
        "gpt-5.6-sol",
        openai(4.0, 20.0, 5.0, 0.4, false),
        legacy(4.0, 20.0, 5.0, 0.4),
        OPENAI,
    ),
    (
        "gpt-5.6-terra",
        unverified(),
        legacy(2.0, 12.0, 2.5, 0.2),
        OPENAI,
    ),
    (
        "gpt-5.6-luna",
        unverified(),
        legacy(0.2, 1.2, 0.25, 0.02),
        OPENAI,
    ),
    (
        "gpt-5.6-cyber",
        openai(12.5, 75.0, 15.625, 1.25, false),
        legacy(12.5, 75.0, 15.625, 1.25),
        OPENAI,
    ),
];

pub(crate) fn normalize_model_id(model: &str) -> &str {
    let model = model
        .rfind('[')
        .filter(|_| model.ends_with(']'))
        .map_or(model, |start| &model[..start]);
    let Some((base, date)) = model.rsplit_once('-') else {
        return model;
    };
    if date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()) {
        base
    } else {
        model
    }
}

fn price_for(model: &str) -> Option<Price> {
    let model = normalize_model_id(model);
    PRICE_ROWS
        .iter()
        .find(|(id, _, _, _)| normalize_model_id(id) == model)
        .map(|(_, price, _, _)| *price)
}

pub(crate) fn table() -> Pricing {
    let mut models = BTreeMap::new();
    for &(model, _, price, source) in PRICE_ROWS {
        let priced = PricedModel { price, source };
        models.insert(model, priced);
        let alias = normalize_model_id(model);
        if alias != model {
            models.insert(alias, priced);
        }
    }
    Pricing {
        as_of: AS_OF,
        models,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct CostTokens {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write_5m: u64,
    pub(crate) cache_write_1h: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ModelCost {
    pub(crate) usd: Option<f64>,
    pub(crate) tokens: CostTokens,
    pub(crate) usd_by_kind: BTreeMap<&'static str, f64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Cost {
    pub(crate) usd: Option<f64>,
    pub(crate) unpriced_models: Vec<String>,
    pub(crate) split_unknown_messages: u64,
    pub(crate) by_model: BTreeMap<String, ModelCost>,
    pub(crate) by_day: BTreeMap<String, f64>,
}

impl Default for Cost {
    fn default() -> Self {
        Self {
            usd: Some(0.0),
            unpriced_models: Vec::new(),
            split_unknown_messages: 0,
            by_model: BTreeMap::new(),
            by_day: BTreeMap::new(),
        }
    }
}

const KINDS: [&str; 6] = [
    "input",
    "output",
    "cache_read",
    "cache_write_5m",
    "cache_write_1h",
    "web_search",
];

#[derive(Default)]
struct ModelCostBuild {
    tokens: CostTokens,
    usd_by_kind: BTreeMap<&'static str, f64>,
    usd: f64,
    unpriced: bool,
}

#[derive(Default)]
struct CostBuild {
    usd: f64,
    has_unpriced: bool,
    split_unknown_messages: u64,
    models: BTreeMap<String, ModelCostBuild>,
    unpriced_models: BTreeSet<String>,
    by_day: BTreeMap<String, f64>,
}

impl CostBuild {
    fn add(
        &mut self,
        model: &str,
        usage: &BillingUsage,
        timestamp: Option<i64>,
        split_unknown: bool,
        price: Option<Price>,
    ) {
        let prompt_long = price
            .and_then(|price| price.long_context)
            .is_some_and(|tier| usage.prompt_size > tier.threshold_tokens);
        let fast_multiplier = match price {
            Some(price) if usage.speed.as_deref() == Some("fast") => {
                price.fast.map(|fast| fast.multiplier)
            }
            Some(_) => Some(1.0),
            None => None,
        };
        let categories = [
            (
                "input",
                usage.input,
                price.and_then(|p| {
                    select_rate(p.input, p.long_context.and_then(|l| l.input), prompt_long)
                }),
            ),
            (
                "output",
                usage.output,
                price.and_then(|p| {
                    select_rate(p.output, p.long_context.and_then(|l| l.output), prompt_long)
                }),
            ),
            (
                "cache_read",
                usage.cache_read,
                price.and_then(|p| {
                    select_rate(
                        p.cache_read,
                        p.long_context.and_then(|l| l.cache_read),
                        prompt_long,
                    )
                }),
            ),
            (
                "cache_write_5m",
                usage.cache_write_5m,
                price.and_then(|p| {
                    select_rate(
                        p.cache_write_5m,
                        p.long_context.and_then(|l| l.cache_write_5m),
                        prompt_long,
                    )
                }),
            ),
            (
                "cache_write_1h",
                usage.cache_write_1h,
                price.and_then(|p| {
                    select_rate(
                        p.cache_write_1h,
                        p.long_context.and_then(|l| l.cache_write_1h),
                        prompt_long,
                    )
                }),
            ),
        ];
        let mut entry_usd = 0.0;
        let mut has_unpriced = false;
        let model_cost = self.models.entry(model.to_owned()).or_default();
        for (kind, tokens, rate) in categories {
            add_tokens(&mut model_cost.tokens, kind, tokens);
            if tokens == 0 {
                continue;
            }
            let amount = rate
                .zip(fast_multiplier)
                .map(|(rate, multiplier)| tokens as f64 * rate * multiplier / 1_000_000.0);
            if let Some(amount) = amount {
                *model_cost.usd_by_kind.entry(kind).or_default() += amount;
                model_cost.usd += amount;
                entry_usd += amount;
            } else {
                has_unpriced = true;
            }
        }
        if usage.web_search_requests > 0 {
            let amount = price
                .and_then(|price| price.web_search_per_1k)
                .map(|rate| usage.web_search_requests as f64 * rate / 1_000.0);
            if let Some(amount) = amount {
                *model_cost.usd_by_kind.entry("web_search").or_default() += amount;
                model_cost.usd += amount;
                entry_usd += amount;
            } else {
                has_unpriced = true;
            }
        }
        if split_unknown {
            self.split_unknown_messages += 1;
        }
        if has_unpriced {
            model_cost.unpriced = true;
            self.has_unpriced = true;
            self.unpriced_models.insert(model.to_owned());
        }
        self.usd += entry_usd;
        if let Some(timestamp) = timestamp {
            *self.by_day.entry(utc_day(timestamp)).or_default() += entry_usd;
        }
    }

    fn finish(self) -> Cost {
        let by_model = self
            .models
            .into_iter()
            .map(|(model, mut cost)| {
                for kind in KINDS {
                    cost.usd_by_kind.entry(kind).or_default();
                }
                (
                    model,
                    ModelCost {
                        usd: (!cost.unpriced).then_some(cost.usd),
                        tokens: cost.tokens,
                        usd_by_kind: cost.usd_by_kind,
                    },
                )
            })
            .collect();
        Cost {
            usd: (!self.has_unpriced).then_some(self.usd),
            unpriced_models: self.unpriced_models.into_iter().collect(),
            split_unknown_messages: self.split_unknown_messages,
            by_model,
            by_day: self.by_day,
        }
    }
}

fn select_rate(base: Option<f64>, long: Option<f64>, use_long: bool) -> Option<f64> {
    if use_long { long } else { base }
}

fn add_tokens(tokens: &mut CostTokens, kind: &str, amount: u64) {
    match kind {
        "input" => tokens.input += amount,
        "output" => tokens.output += amount,
        "cache_read" => tokens.cache_read += amount,
        "cache_write_5m" => tokens.cache_write_5m += amount,
        "cache_write_1h" => tokens.cache_write_1h += amount,
        _ => unreachable!("known token category"),
    }
}

fn utc_day(timestamp_ms: i64) -> String {
    let days = timestamp_ms.div_euclid(86_400_000);
    // Howard Hinnant's civil_from_days, with Unix epoch day zero.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}")
}

pub(crate) fn calculate_cost(messages: &[MessageUsage], codex_events: &[CodexUsageEvent]) -> Cost {
    calculate_cost_with(messages, codex_events, price_for)
}

fn calculate_cost_with(
    messages: &[MessageUsage],
    codex_events: &[CodexUsageEvent],
    mut lookup: impl FnMut(&str) -> Option<Price>,
) -> Cost {
    let mut cost = CostBuild::default();
    for message in messages {
        let model = message.model.as_deref().unwrap_or("unknown");
        cost.add(
            model,
            &message.billing,
            message.billing.timestamp,
            message.billing.split_unknown,
            lookup(model),
        );
    }
    for event in codex_events {
        let billing = BillingUsage {
            input: event.tokens.input,
            output: event.tokens.output,
            cache_read: event.tokens.cache_read,
            cache_write_5m: 0,
            cache_write_1h: 0,
            web_search_requests: 0,
            speed: None,
            service_tier: None,
            prompt_size: event.tokens.input + event.tokens.cache_read,
            timestamp: event.timestamp,
            split_unknown: false,
        };
        cost.add(
            &event.model,
            &billing,
            event.timestamp,
            false,
            lookup(&event.model),
        );
    }
    cost.finish()
}

pub(crate) fn cost_check_ok(computed: Option<f64>, reported: Option<f64>) -> Option<bool> {
    let (computed, reported) = (computed?, reported?);
    Some((computed - reported).abs() <= reported.abs() * 0.01 + f64::EPSILON)
}

#[cfg(test)]
mod tests {
    use super::{
        FastPrice, LongContextPrice, Price, calculate_cost, calculate_cost_with, cost_check_ok,
        normalize_model_id, table,
    };
    use crate::events::{BillingUsage, CodexUsageEvent, MessageUsage, ModelTokens};

    fn usage(
        model: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        write_5m: u64,
        write_1h: u64,
    ) -> MessageUsage {
        MessageUsage {
            model: Some(model.to_owned()),
            tokens: crate::Tokens::default(),
            model_tokens: ModelTokens::default(),
            billing: BillingUsage {
                input,
                output,
                cache_read,
                cache_write_5m: write_5m,
                cache_write_1h: write_1h,
                web_search_requests: 0,
                speed: Some("standard".into()),
                service_tier: Some("standard".into()),
                prompt_size: input + cache_read + write_5m + write_1h,
                timestamp: Some(0),
                split_unknown: false,
            },
        }
    }

    #[test]
    fn opus_five_prices_match_the_official_cache_rates_and_real_run() {
        let p = super::price_for("claude-opus-5[1m]").unwrap();
        assert_eq!(p.input, Some(5.0));
        assert_eq!(p.output, Some(25.0));
        assert_eq!(p.cache_write_5m, Some(6.25));
        assert_eq!(p.cache_write_1h, Some(10.0));
        assert_eq!(p.cache_read, Some(0.5));
        let mut real = usage("claude-opus-5", 1_034, 2_080, 518_410, 0, 151_872);
        real.billing.prompt_size = 671_316;
        let cost = calculate_cost(&[real], &[]);
        assert!((cost.usd.unwrap() - 1.835095).abs() < 1e-12);
    }

    #[test]
    fn five_minute_and_one_hour_writes_use_their_own_rates() {
        let one_hour = calculate_cost(&[usage("claude-opus-5", 0, 0, 0, 0, 1_000_000)], &[]);
        let five_minutes = calculate_cost(&[usage("claude-opus-5", 0, 0, 0, 1_000_000, 0)], &[]);
        assert_eq!(one_hour.usd, Some(10.0));
        assert_eq!(five_minutes.usd, Some(6.25));
    }

    #[test]
    fn suffix_maps_to_the_base_price_and_compatibility_shape_stays_stable() {
        assert_eq!(normalize_model_id("claude-opus-5[1m]"), "claude-opus-5");
        let json = serde_json::to_value(table()).unwrap();
        assert_eq!(json["models"]["claude-opus-5"]["cache_write"], 6.25);
        assert!(
            json["models"]["claude-opus-5"]
                .get("cache_write_1h")
                .is_none()
        );
    }

    #[test]
    fn long_context_and_fast_rates_use_only_the_selected_message_facts() {
        let mut p = usage("claude-opus-5", 1, 1, 0, 0, 0);
        p.billing.prompt_size = 101;
        let mut below = p.clone();
        below.billing.prompt_size = 100;
        let tier_price = Price {
            input: Some(3.0),
            output: Some(3.0),
            cache_read: Some(0.1),
            cache_write_5m: Some(1.25),
            cache_write_1h: Some(2.0),
            long_context: Some(LongContextPrice {
                threshold_tokens: 100,
                input: Some(10.0),
                output: Some(20.0),
                cache_read: Some(0.3),
                cache_write_5m: Some(3.75),
                cache_write_1h: Some(6.0),
            }),
            fast: Some(FastPrice { multiplier: 2.0 }),
            web_search_per_1k: None,
        };
        let below_cost = calculate_cost_with(&[below], &[], |_| Some(tier_price));
        let above_cost = calculate_cost_with(&[p.clone()], &[], |_| Some(tier_price));
        assert_eq!(below_cost.usd, Some(0.000006));
        assert_eq!(above_cost.usd, Some(0.00003));
        let mut fast = p.clone();
        fast.billing.speed = Some("fast".into());
        let fast_cost = calculate_cost(&[fast], &[]);
        assert_eq!(fast_cost.usd, Some(0.00006));
        // The official pages do not publish a usable OpenAI long-context
        // threshold, so no such tier is installed in its rows.
        assert!(
            super::price_for("gpt-6-luna")
                .unwrap()
                .long_context
                .is_none()
        );
    }

    #[test]
    fn thinking_is_already_in_output_and_days_sum_to_total() {
        let mut first = usage("claude-opus-5", 1_000_000, 2_000_000, 0, 0, 0);
        first.billing.timestamp = Some(0);
        let mut second = usage("claude-opus-5", 1_000_000, 0, 0, 0, 0);
        second.billing.timestamp = Some(86_400_000);
        let cost = calculate_cost(&[first, second], &[]);
        assert_eq!(cost.usd, Some(60.0));
        assert_eq!(cost.by_day.values().sum::<f64>(), cost.usd.unwrap());
        assert_eq!(
            cost.by_day.keys().cloned().collect::<Vec<_>>(),
            vec!["1970-01-01".to_owned(), "1970-01-02".to_owned()]
        );
    }

    #[test]
    fn cost_check_allows_one_percent_and_flags_larger_gaps() {
        assert_eq!(cost_check_ok(Some(1.009), Some(1.0)), Some(true));
        assert_eq!(cost_check_ok(Some(1.02), Some(1.0)), Some(false));
    }

    #[test]
    fn missing_cache_split_uses_five_minute_rate_and_counts_one_unknown_split() {
        let mut message = usage("claude-opus-5", 0, 0, 0, 1_000_000, 0);
        message.billing.split_unknown = true;
        let cost = calculate_cost(&[message], &[]);
        assert_eq!(cost.usd, Some(6.25));
        assert_eq!(cost.split_unknown_messages, 1);
    }

    #[test]
    fn verified_web_search_charge_is_added_per_thousand_requests() {
        let mut message = usage("claude-opus-5", 0, 0, 0, 0, 0);
        message.billing.web_search_requests = 1;
        let cost = calculate_cost(&[message], &[]);
        assert_eq!(cost.usd, Some(0.01));
    }

    #[test]
    fn codex_token_count_deltas_are_priced_on_their_event_days() {
        let events = [
            CodexUsageEvent {
                model: "gpt-6-luna".into(),
                tokens: ModelTokens {
                    input: 80_000,
                    output: 10_000,
                    cache_write: 0,
                    cache_read: 20_000,
                },
                timestamp: Some(0),
            },
            CodexUsageEvent {
                model: "gpt-6-sol".into(),
                tokens: ModelTokens {
                    input: 70_000,
                    output: 20_000,
                    cache_write: 0,
                    cache_read: 30_000,
                },
                timestamp: Some(86_400_000),
            },
        ];
        let cost = calculate_cost(&[], &events);
        assert!((cost.usd.unwrap() - 0.3592).abs() < 1e-12);
        assert!((cost.by_day["1970-01-01"] - 0.0132).abs() < 1e-12);
        assert!((cost.by_day["1970-01-02"] - 0.346).abs() < 1e-12);
        assert_eq!(cost.by_day.values().sum::<f64>(), cost.usd.unwrap());
    }
}
