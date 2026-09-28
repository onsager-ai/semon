//! The API-equivalent token prices shown by the session model.
//!
//! Prices are USD per million tokens. Anthropic cache writes use the 5-minute
//! rate. OpenAI prices use the standard, short-context tier where tiers are
//! offered; long-context, batch, and flex tiers are not modelled. Dated model
//! ids keep their exact keys and also receive one alias with a trailing
//! `-YYYYMMDD` removed.

use std::collections::BTreeMap;

use serde::Serialize;

pub const AS_OF: &str = "2026-09-28";

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

#[derive(Clone, Copy, Serialize)]
struct PricedModel {
    #[serde(flatten)]
    price: Price,
    source: &'static str,
}

#[derive(Serialize)]
pub(crate) struct Pricing {
    as_of: &'static str,
    models: BTreeMap<&'static str, PricedModel>,
}

const ANTHROPIC: &str = "https://platform.claude.com/docs/en/about-claude/pricing";
const OPENAI: &str = "https://developers.openai.com/api/docs/pricing";

/// Exact API model ids and prices read from the official pricing pages.
/// Anthropic dated ids are kept verbatim here; `table` adds only their
/// date-stripped aliases so both dated and aliased log ids can be priced.
const PRICE_ROWS: &[(&str, Price, &str)] = &[
    (
        "claude-fable-5-1",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 0.25,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-5-5",
        Price {
            input: 4.0,
            output: 20.0,
            cache_write: 5.0,
            cache_read: 0.2,
        },
        ANTHROPIC,
    ),
    (
        "claude-sonnet-5",
        Price {
            input: 2.0,
            output: 10.0,
            cache_write: 2.5,
            cache_read: 0.2,
        },
        ANTHROPIC,
    ),
    (
        "claude-haiku-4-5-20251001",
        Price {
            input: 1.0,
            output: 5.0,
            cache_write: 1.25,
            cache_read: 0.1,
        },
        ANTHROPIC,
    ),
    (
        "claude-mythos-5-1",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 0.25,
        },
        ANTHROPIC,
    ),
    (
        "claude-fable-5",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 1.0,
        },
        ANTHROPIC,
    ),
    (
        "claude-mythos-5",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 1.0,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-5",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-4-8",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-4-7",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-4-6",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-4-5-20251101",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-4-1-20250805",
        Price {
            input: 15.0,
            output: 75.0,
            cache_write: 18.75,
            cache_read: 1.5,
        },
        ANTHROPIC,
    ),
    (
        "claude-opus-4-20250514",
        Price {
            input: 15.0,
            output: 75.0,
            cache_write: 18.75,
            cache_read: 1.5,
        },
        ANTHROPIC,
    ),
    (
        "claude-sonnet-4-6",
        Price {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.3,
        },
        ANTHROPIC,
    ),
    (
        "claude-sonnet-4-5-20250929",
        Price {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.3,
        },
        ANTHROPIC,
    ),
    (
        "claude-sonnet-4-20250514",
        Price {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.3,
        },
        ANTHROPIC,
    ),
    (
        "claude-3-5-haiku-20241022",
        Price {
            input: 0.8,
            output: 4.0,
            cache_write: 1.0,
            cache_read: 0.08,
        },
        ANTHROPIC,
    ),
    (
        "gpt-6-astra",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 1.0,
        },
        OPENAI,
    ),
    (
        "gpt-6-sol",
        Price {
            input: 2.0,
            output: 10.0,
            cache_write: 2.5,
            cache_read: 0.2,
        },
        OPENAI,
    ),
    (
        "gpt-6-luna",
        Price {
            input: 0.1,
            output: 0.5,
            cache_write: 0.125,
            cache_read: 0.01,
        },
        OPENAI,
    ),
    (
        "gpt-5.6-sol",
        Price {
            input: 4.0,
            output: 20.0,
            cache_write: 5.0,
            cache_read: 0.4,
        },
        OPENAI,
    ),
    (
        "gpt-5.6-terra",
        Price {
            input: 2.0,
            output: 12.0,
            cache_write: 2.5,
            cache_read: 0.2,
        },
        OPENAI,
    ),
    (
        "gpt-5.6-luna",
        Price {
            input: 0.2,
            output: 1.2,
            cache_write: 0.25,
            cache_read: 0.02,
        },
        OPENAI,
    ),
    (
        "gpt-5.6-cyber",
        Price {
            input: 12.5,
            output: 75.0,
            cache_write: 15.625,
            cache_read: 1.25,
        },
        OPENAI,
    ),
];

pub(crate) fn normalize_model_id(model: &str) -> &str {
    let Some((base, date)) = model.rsplit_once('-') else {
        return model;
    };
    if date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()) {
        base
    } else {
        model
    }
}

pub(crate) fn table() -> Pricing {
    let mut models = BTreeMap::new();
    for &(model, price, source) in PRICE_ROWS {
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

#[cfg(test)]
mod tests {
    use super::{normalize_model_id, table};

    #[test]
    fn dated_model_ids_keep_exact_and_date_stripped_price_keys() {
        let prices = table().models;
        let dated = prices.get("claude-sonnet-4-5-20250929").unwrap();
        let alias = prices.get("claude-sonnet-4-5").unwrap();
        assert_eq!(dated.price, alias.price);
        assert_eq!(
            normalize_model_id("claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_model_id("claude-sonnet-4-5-20250929-extra"),
            "claude-sonnet-4-5-20250929-extra"
        );
        assert_eq!(
            normalize_model_id("claude-sonnet-4-5-latest"),
            "claude-sonnet-4-5-latest"
        );
    }
}
