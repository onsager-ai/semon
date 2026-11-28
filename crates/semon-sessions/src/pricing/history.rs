//! Effective intervals for one model's individual rates. An observation is
//! identified by the full field path and, for context tiers, its threshold.
use super::{FastPrice, FastRates, LongContextPrice, Price};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) type History = BTreeMap<String, BTreeMap<String, Vec<Series>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Input,
    Output,
    CacheRead,
    CacheWrite5m,
    CacheWrite1h,
    WebSearch,
    Boundary,
}

impl Kind {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cache_read",
            Self::CacheWrite5m => "cache_write_5m",
            Self::CacheWrite1h => "cache_write_1h",
            Self::WebSearch => "web_search_per_1k",
            Self::Boundary => "threshold_tokens",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub(super) struct Selector {
    pub fast: bool,
    pub threshold: Option<u64>,
    pub kind: Kind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Series {
    pub selector: Selector,
    pub intervals: Vec<Interval>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Interval {
    pub valid_from: i64,
    pub valid_to: Option<i64>,
    pub learned_at: i64,
    pub effective_date_source: Option<String>,
    pub amount: f64,
    pub source: String,
}

pub(super) type Rates = BTreeMap<Selector, (f64, String)>;

pub(super) fn observe(series: &mut Vec<Series>, mut rates: Rates, at: i64) {
    for s in series.iter_mut() {
        let new = rates.remove(&s.selector);
        if let Some(open) = s.intervals.last_mut().filter(|i| i.valid_to.is_none()) {
            if new
                .as_ref()
                .is_some_and(|(amount, source)| *amount == open.amount && source == &open.source)
            {
                continue;
            }
            open.valid_to = Some(at);
        }
        if let Some((amount, source)) = new {
            s.intervals.push(Interval::observed(at, amount, source));
        }
    }
    for (selector, (amount, source)) in rates {
        series.push(Series {
            selector,
            intervals: vec![Interval::observed(at, amount, source)],
        });
    }
}

impl Interval {
    fn observed(at: i64, amount: f64, source: String) -> Self {
        Self {
            valid_from: at,
            valid_to: None,
            learned_at: at,
            effective_date_source: None,
            amount,
            source,
        }
    }
}

pub(super) fn price_at(series: &[Series], at: i64) -> Option<Price> {
    let mut standard = FastRates::default();
    let mut fast = FastRates::default();
    let mut has_fast = false;
    let mut has_price = false;
    let mut web = None;
    for s in series {
        let n = s.intervals.partition_point(|i| i.valid_from <= at);
        let Some(i) = n
            .checked_sub(1)
            .map(|n| &s.intervals[n])
            .filter(|i| i.valid_to.is_none_or(|end| at < end))
        else {
            continue;
        };
        has_price = true;
        let selector = s.selector;
        let rates = if selector.fast {
            has_fast = true;
            &mut fast
        } else {
            &mut standard
        };
        let value = Some(i.amount);
        if let Some(threshold_tokens) = selector.threshold {
            let tier = rates.long_context.get_or_insert(LongContextPrice {
                threshold_tokens,
                ..Default::default()
            });
            match selector.kind {
                Kind::Input => tier.input = value,
                Kind::Output => tier.output = value,
                Kind::CacheRead => tier.cache_read = value,
                Kind::CacheWrite5m => tier.cache_write_5m = value,
                Kind::CacheWrite1h => tier.cache_write_1h = value,
                Kind::Boundary | Kind::WebSearch => {}
            }
        } else {
            match selector.kind {
                Kind::Input => rates.input = value,
                Kind::Output => rates.output = value,
                Kind::CacheRead => rates.cache_read = value,
                Kind::CacheWrite5m => rates.cache_write_5m = value,
                Kind::CacheWrite1h => rates.cache_write_1h = value,
                Kind::WebSearch => web = value,
                Kind::Boundary => {}
            }
        }
    }
    has_price.then_some(Price {
        input: standard.input,
        output: standard.output,
        cache_read: standard.cache_read,
        cache_write_5m: standard.cache_write_5m,
        cache_write_1h: standard.cache_write_1h,
        long_context: standard.long_context,
        fast: has_fast.then_some(FastPrice::Rates { rates: fast }),
        web_search_per_1k: web,
    })
}
