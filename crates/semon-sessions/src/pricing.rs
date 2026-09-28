//! The API-equivalent token prices shown by the session model.
//!
//! Prices are USD per million tokens. Claude cache writes use the 5-minute
//! rate; GPT-6 Luna uses the short-context rates.

use std::{collections::BTreeMap, sync::LazyLock};

use serde::Serialize;

pub const AS_OF: &str = "2026-09-28";

#[derive(Clone, Copy, Debug, Serialize)]
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

/// Exact model ids and the matching official pricing source.
static PRICES: LazyLock<BTreeMap<&'static str, PricedModel>> = LazyLock::new(|| {
    BTreeMap::from([
        (
            "claude-opus-5-5",
            PricedModel {
                price: Price {
                    input: 4.0,
                    output: 20.0,
                    cache_write: 5.0,
                    cache_read: 0.2,
                },
                source: ANTHROPIC,
            },
        ),
        (
            "claude-sonnet-5",
            PricedModel {
                price: Price {
                    input: 2.0,
                    output: 10.0,
                    cache_write: 2.5,
                    cache_read: 0.2,
                },
                source: ANTHROPIC,
            },
        ),
        (
            "claude-haiku-4-5",
            PricedModel {
                price: Price {
                    input: 1.0,
                    output: 5.0,
                    cache_write: 1.25,
                    cache_read: 0.1,
                },
                source: ANTHROPIC,
            },
        ),
        (
            "gpt-6-luna",
            PricedModel {
                price: Price {
                    input: 0.1,
                    output: 0.5,
                    cache_write: 0.125,
                    cache_read: 0.01,
                },
                source: OPENAI,
            },
        ),
    ])
});

pub(crate) fn table() -> Pricing {
    Pricing {
        as_of: AS_OF,
        models: PRICES
            .iter()
            .map(|(model, price)| (*model, *price))
            .collect(),
    }
}
