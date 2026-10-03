//! Verified per-token prices and exact session cost calculation.
//!
//! Prices are USD per million tokens. The compatibility `pricing` object
//! keeps its original four price fields; cost calculations use the separate
//! cache-write, context, speed and tool rates below.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::events::{BillingUsage, CodexUsageEvent, MessageUsage};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct LongContextPrice {
    pub threshold_tokens: u64,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct FastPrice {
    pub multiplier: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
struct LegacyPrice {
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
}

#[derive(Clone, Serialize)]
struct PricedModel {
    #[serde(flatten)]
    price: LegacyPrice,
    source: String,
}

#[derive(Serialize)]
pub(crate) struct Pricing {
    as_of: String,
    models: BTreeMap<String, PricedModel>,
}

#[derive(Deserialize)]
struct PriceRow {
    #[serde(flatten)]
    price: Price,
    legacy: LegacyPrice,
    source: String,
    #[serde(default, rename = "sync")]
    _sync: bool,
}

#[derive(Deserialize)]
struct PriceTableData {
    as_of: String,
    models: BTreeMap<String, PriceRow>,
}

const PRICE_JSON: &str = include_str!("pricing.json");
static PRICE_DATA: OnceLock<PriceTableData> = OnceLock::new();

fn price_data() -> &'static PriceTableData {
    PRICE_DATA.get_or_init(|| {
        serde_json::from_str(PRICE_JSON).expect("pricing.json must be valid and complete")
    })
}

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
    price_data()
        .models
        .iter()
        .find(|(id, _)| normalize_model_id(id.as_str()) == model)
        .map(|(_, row)| row.price)
}

pub(crate) fn table() -> Pricing {
    let mut models = BTreeMap::new();
    for (model, row) in &price_data().models {
        let priced = PricedModel {
            price: row.legacy,
            source: row.source.clone(),
        };
        models.insert(model.clone(), priced.clone());
        let alias = normalize_model_id(model);
        if alias != model.as_str() {
            models.insert(alias.to_owned(), priced);
        }
    }
    Pricing {
        as_of: price_data().as_of.clone(),
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
        normalize_model_id, price_data, table,
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
    fn generated_price_json_parses_and_every_model_has_a_source() {
        let parsed = price_data();
        assert!(!parsed.models.is_empty());
        for (model, row) in &parsed.models {
            assert!(
                !row.source.trim().is_empty(),
                "{model} has no pricing source"
            );
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
    fn dotted_gpt_version_does_not_borrow_another_models_price() {
        assert_eq!(normalize_model_id("gpt-6.1-sol"), "gpt-6.1-sol");
        let price = super::price_for("gpt-6.1-sol").unwrap();
        assert_eq!(price.input, Some(2.0));
        assert_eq!(price.output, Some(10.0));
        assert_eq!(price.cache_read, Some(0.1));
        assert_eq!(price.cache_write_5m, Some(2.5));
        assert_eq!(price.fast.unwrap().multiplier, 2.0);
        assert_eq!(super::price_for("gpt-6-sol").unwrap().cache_read, Some(0.2));
        assert!(super::price_for("gpt-6.2-sol").is_none());
        assert_eq!(
            price_data().models["gpt-6.1-sol"].source,
            "https://developers.openai.com/api/docs/pricing"
        );
        let cost = calculate_cost(
            &[usage(
                "gpt-6.1-sol",
                1_000_000,
                1_000_000,
                1_000_000,
                1_000_000,
                0,
            )],
            &[],
        );
        assert_eq!(cost.usd, Some(24.2));
    }

    #[test]
    fn gpt_61_long_context_applies_only_above_the_published_threshold() {
        let row = price_data().models["gpt-6.1-sol"].price;
        let long = row.long_context.unwrap();
        assert_eq!(long.threshold_tokens, 272_000);
        assert_eq!(long.input, Some(4.0));
        assert_eq!(long.output, Some(15.0));
        assert_eq!(long.cache_read, Some(0.2));
        assert_eq!(long.cache_write_5m, Some(5.0));
        let mut message = usage("gpt-6.1-sol", 100_000, 10_000, 100_000, 72_000, 0);
        let short = calculate_cost(&[message.clone()], &[]).usd.unwrap();
        assert!((short - 0.49).abs() < 1e-12);
        message.billing.prompt_size += 1;
        let long = calculate_cost(&[message.clone()], &[]).usd.unwrap();
        assert!((long - 0.93).abs() < 1e-12);
        message.billing.speed = Some("fast".into());
        assert!((calculate_cost(&[message], &[]).usd.unwrap() - 1.86).abs() < 1e-12);
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
        assert!((below_cost.usd.unwrap() - 0.000006).abs() < 1e-12);
        assert!((above_cost.usd.unwrap() - 0.00003).abs() < 1e-12);
        let mut fast = p.clone();
        fast.billing.speed = Some("fast".into());
        let fast_cost = calculate_cost(&[fast], &[]);
        assert!((fast_cost.usd.unwrap() - 0.00006).abs() < 1e-12);
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
