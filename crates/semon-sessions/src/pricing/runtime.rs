//! Public reference data only: no provider credentials or native source access.
use super::{FastPrice, FastRates, LongContextPrice, Price, normalize_model_id, price_data};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_BODY: u64 = 16 * 1024 * 1024;
const INTERVAL: u64 = 6 * 60 * 60;
const RETRY: u64 = 30 * 60;
const CACHE_VERSION: u32 = 1;
const PROVIDERS: [&str; 2] = ["anthropic", "openai"];
const MAX_INDEXES: usize = 64;
const INDEX_RETIRE_AFTER: Duration = Duration::from_secs(30 * 60);
const WORKER_FALLBACK_WAKE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Row {
    price: Price,
    provenance: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v2_fetched_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct EffectiveTable {
    version: u32,
    pub(crate) fetched_at: u64,
    pub(crate) fingerprint: String,
    providers: BTreeMap<String, BTreeMap<String, Row>>,
}

impl EffectiveTable {
    pub(crate) fn price_for(&self, provider: &str, id: &str) -> Option<Price> {
        let rows = self.providers.get(provider)?;
        rows.get(id)
            .or_else(|| {
                let alias = normalize_model_id(id);
                rows.get(alias).or_else(|| {
                    rows.iter()
                        .find(|(key, _)| normalize_model_id(key) == alias)
                        .map(|(_, row)| row)
                })
            })
            .map(|row| row.price)
    }

    fn seal(&mut self) -> io::Result<()> {
        // BTreeMap ordering makes the digest independent of upstream object order
        // and fetch time. Include provenance as well as all effective rates,
        // but not the last successful supplement fetch timestamp.
        let mut fingerprint_rows = self.providers.clone();
        for rows in fingerprint_rows.values_mut() {
            for row in rows.values_mut() {
                row.v2_fetched_at = None;
            }
        }
        self.fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&fingerprint_rows)?)
        );
        Ok(())
    }

    fn validate(&self) -> io::Result<()> {
        if self.version != CACHE_VERSION || self.providers.len() != PROVIDERS.len() {
            return Err(invalid("unsupported pricing cache"));
        }
        for provider in PROVIDERS {
            let rows = self
                .providers
                .get(provider)
                .ok_or_else(|| invalid("missing provider"))?;
            // A complete API currently contains 17/51 rows. Two priced models
            // per provider allows small deterministic offline fixtures while
            // rejecting empty or severely malformed responses.
            if rows
                .values()
                .filter(|r| r.price.input.is_some() && r.price.output.is_some())
                .count()
                < 2
            {
                return Err(invalid("insufficient priced models"));
            }
            for (id, row) in rows {
                if id.is_empty() || id.len() > 512 {
                    return Err(invalid("invalid model identity"));
                }
                if row
                    .provenance
                    .values()
                    .any(|source| source == "models_dev_v2")
                    && row.v2_fetched_at.is_none()
                {
                    return Err(invalid(format!(
                        "provider {provider} model {id} field v2_fetched_at: missing"
                    )));
                }
                validate_price(&row.price)?;
            }
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn model_error(provider: &str, model: &str, error: io::Error) -> io::Error {
    invalid(format!("provider {provider} model {model}: {error}"))
}

fn bundled() -> EffectiveTable {
    let mut providers: BTreeMap<String, BTreeMap<String, Row>> = PROVIDERS
        .into_iter()
        .map(|p| (p.into(), BTreeMap::new()))
        .collect();
    for (id, row) in &price_data().models {
        // The authored bootstrap catalog contains native lab IDs only. Runtime
        // identity selection is explicit, never based on this bootstrap rule.
        let provider = if id.starts_with("claude-") {
            "anthropic"
        } else if id.starts_with("gpt-")
            || id.starts_with("o1")
            || id.starts_with("o3")
            || id.starts_with("o4")
        {
            "openai"
        } else {
            continue;
        };
        let value = serde_json::to_value(row.price).expect("bundled price serializes");
        let mut provenance = BTreeMap::new();
        provenance_fields(&value, "", "bundled", &mut provenance);
        providers
            .get_mut(provider)
            .expect("provider exists")
            .insert(
                id.clone(),
                Row {
                    price: row.price,
                    provenance,
                    v2_fetched_at: None,
                },
            );
    }
    let mut table = EffectiveTable {
        version: CACHE_VERSION,
        fetched_at: 0,
        fingerprint: String::new(),
        providers,
    };
    table.seal().expect("bundled table serializes");
    table
}

fn provenance_fields(
    value: &Value,
    prefix: &str,
    source: &str,
    into: &mut BTreeMap<String, String>,
) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            let key = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            provenance_fields(value, &key, source, into);
        }
    } else if !value.is_null() {
        into.insert(prefix.into(), source.into());
    }
}

static TABLE: OnceLock<RwLock<Arc<EffectiveTable>>> = OnceLock::new();
fn store() -> &'static RwLock<Arc<EffectiveTable>> {
    TABLE.get_or_init(|| RwLock::new(Arc::new(bundled())))
}
pub(crate) fn snapshot() -> Arc<EffectiveTable> {
    Arc::clone(&store().read().unwrap_or_else(|e| e.into_inner()))
}

fn install(table: EffectiveTable) {
    let changed = {
        let mut current = store().write().unwrap_or_else(|e| e.into_inner());
        let changed = current.fingerprint != table.fingerprint;
        *current = Arc::new(table);
        changed
    };
    if changed {
        notify_reprice();
    }
}

fn validate_rate(value: f64) -> io::Result<()> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(invalid("invalid rate"))
    }
}
fn validate_price(price: &Price) -> io::Result<()> {
    // serde_json cannot encode nonfinite numbers; validate before fingerprinting.
    for value in [
        price.input,
        price.output,
        price.cache_read,
        price.cache_write_5m,
        price.cache_write_1h,
        price.web_search_per_1k,
    ]
    .into_iter()
    .flatten()
    {
        validate_rate(value)?;
    }
    if let Some(tier) = price.long_context {
        validate_tier(tier)?;
    }
    if let Some(fast) = price.fast {
        match fast {
            FastPrice::Multiplier { multiplier } => validate_rate(multiplier)?,
            FastPrice::Rates { rates } => {
                for value in [
                    rates.input,
                    rates.output,
                    rates.cache_read,
                    rates.cache_write_5m,
                    rates.cache_write_1h,
                ]
                .into_iter()
                .flatten()
                {
                    validate_rate(value)?;
                }
                if let Some(tier) = rates.long_context {
                    validate_tier(tier)?;
                }
            }
        }
    }
    Ok(())
}
fn validate_tier(tier: LongContextPrice) -> io::Result<()> {
    if tier.threshold_tokens == 0 {
        return Err(invalid("zero context threshold"));
    }
    for value in [
        tier.input,
        tier.output,
        tier.cache_read,
        tier.cache_write_5m,
        tier.cache_write_1h,
    ]
    .into_iter()
    .flatten()
    {
        validate_rate(value)?;
    }
    Ok(())
}

fn rate(cost: &Value, key: &str) -> io::Result<Option<f64>> {
    match cost.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let value = value
                .as_f64()
                .ok_or_else(|| invalid(format!("field {key}: nonnumeric rate")))?;
            validate_rate(value).map_err(|_| invalid(format!("field {key}: invalid rate")))?;
            Ok(Some(value))
        }
    }
}

fn rates(cost: &Value, provider: &str) -> io::Result<FastRates> {
    if !cost.is_object() {
        return Err(invalid("field cost: invalid object"));
    }
    let write_5m = if provider == "anthropic" {
        // Generic cache_write has no TTL in the API contract. Explicit aliases
        // are accepted; generic writes are never guessed, even for Anthropic.
        rate(cost, "cache_write_5m")?.or(rate(cost, "cache_write5m")?)
    } else {
        None
    };
    Ok(FastRates {
        input: rate(cost, "input")?,
        output: rate(cost, "output")?,
        cache_read: rate(cost, "cache_read")?,
        cache_write_5m: write_5m,
        cache_write_1h: if provider == "anthropic" {
            rate(cost, "cache_write_1h")?
        } else {
            None
        },
        long_context: None,
    })
}

fn long_context(cost: &Value, provider: &str) -> io::Result<Option<LongContextPrice>> {
    let explicit = match cost.get("tiers") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let tiers = value
                .as_array()
                .ok_or_else(|| invalid("field cost.tiers: invalid array"))?;
            // Unknown non-context tiers do not affect the native context
            // calculator. More than one context boundary cannot be represented.
            let mut context = None;
            for tier in tiers {
                if tier.pointer("/tier/type").and_then(Value::as_str) != Some("context") {
                    continue;
                }
                if context.is_some() {
                    return Err(invalid(
                        "field cost.tiers: multiple context tiers unsupported",
                    ));
                }
                let size = tier
                    .pointer("/tier/size")
                    .and_then(Value::as_u64)
                    .filter(|v| *v > 0)
                    .ok_or_else(|| invalid("field cost.tiers[].tier.size: invalid boundary"))?;
                context = Some((size, tier));
            }
            context
        }
    };
    if let Some(compatibility) = cost.get("context_over_200k").filter(|v| !v.is_null()) {
        rates(compatibility, provider)
            .map_err(|error| invalid(format!("field cost.context_over_200k: {error}")))?;
    }
    let Some((threshold_tokens, value)) = explicit.or_else(|| {
        cost.get("context_over_200k")
            .filter(|v| !v.is_null())
            .map(|v| (200_000, v))
    }) else {
        return Ok(None);
    };
    // `tier` is metadata, not a rate.
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("tier");
    }
    let r = rates(&value, provider)
        .map_err(|error| invalid(format!("field cost.tiers[].cost: {error}")))?;
    Ok(Some(LongContextPrice {
        threshold_tokens,
        input: r.input,
        output: r.output,
        cache_read: r.cache_read,
        cache_write_5m: r.cache_write_5m,
        cache_write_1h: r.cache_write_1h,
    }))
}

fn overlay_rates(mut upstream: FastRates, base: FastRates) -> FastRates {
    upstream.input = upstream.input.or(base.input);
    upstream.output = upstream.output.or(base.output);
    upstream.cache_read = upstream.cache_read.or(base.cache_read);
    upstream.cache_write_5m = upstream.cache_write_5m.or(base.cache_write_5m);
    upstream.cache_write_1h = upstream.cache_write_1h.or(base.cache_write_1h);
    upstream
}
fn base_rates(price: Price) -> FastRates {
    FastRates {
        input: price.input,
        output: price.output,
        cache_read: price.cache_read,
        cache_write_5m: price.cache_write_5m,
        cache_write_1h: price.cache_write_1h,
        long_context: price.long_context,
    }
}
fn overlay_tier(
    upstream: Option<LongContextPrice>,
    base: Option<LongContextPrice>,
) -> Option<LongContextPrice> {
    match (upstream, base) {
        (Some(mut u), Some(b)) if u.threshold_tokens == b.threshold_tokens => {
            u.input = u.input.or(b.input);
            u.output = u.output.or(b.output);
            u.cache_read = u.cache_read.or(b.cache_read);
            u.cache_write_5m = u.cache_write_5m.or(b.cache_write_5m);
            u.cache_write_1h = u.cache_write_1h.or(b.cache_write_1h);
            Some(u)
        }
        (Some(u), _) => Some(u),
        (None, b) => b,
    }
}

pub(crate) fn adapt(body: &[u8], fetched_at: u64) -> io::Result<EffectiveTable> {
    if body.len() as u64 > MAX_BODY {
        return Err(invalid("pricing response too large"));
    }
    let api: Value = serde_json::from_slice(body)?;
    let bootstrap = bundled();
    // Absent upstream models keep their authored, same-provider bootstrap rows.
    let mut table = bootstrap.clone();
    table.fetched_at = fetched_at;
    for provider in PROVIDERS {
        let entry = api
            .get(provider)
            .ok_or_else(|| invalid(format!("provider {provider} field provider: missing")))?;
        if entry["id"] != provider {
            return Err(invalid(format!(
                "provider {provider} field id: identity mismatch"
            )));
        }
        let models = entry["models"]
            .as_object()
            .ok_or_else(|| invalid(format!("provider {provider} field models: missing")))?;
        let mut priced = 0;
        for (id, model) in models {
            if model["id"] != *id {
                return Err(invalid(format!(
                    "provider {provider} model {id} field id: identity mismatch"
                )));
            }
            let Some(cost) = model.get("cost").filter(|v| !v.is_null()) else {
                continue;
            };
            let raw = rates(cost, provider).map_err(|error| model_error(provider, id, error))?;
            if raw.input.is_some() && raw.output.is_some() {
                priced += 1;
            }
            // Exact lookup first; the existing native dated alias is allowed
            // within the same provider only for bundled fallback.
            let base = bootstrap.price_for(provider, id).unwrap_or_default();
            let r = overlay_rates(raw, base_rates(base));
            let explicit_tier =
                long_context(cost, provider).map_err(|error| model_error(provider, id, error))?;
            let tier = match (explicit_tier, base.long_context) {
                // A compatibility 200k field may not erase an authored exact
                // hotfix tier. Explicit tiers are authoritative.
                (Some(_), Some(b))
                    if cost
                        .get("tiers")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty) =>
                {
                    Some(b)
                }
                (u, b) => overlay_tier(u, b),
            };
            let mut price = Price {
                input: r.input,
                output: r.output,
                cache_read: r.cache_read,
                cache_write_5m: r.cache_write_5m,
                cache_write_1h: r.cache_write_1h,
                long_context: tier,
                fast: base.fast,
                web_search_per_1k: base.web_search_per_1k,
            };
            let mut provenance = BTreeMap::new();
            provenance_fields(&serde_json::to_value(base)?, "", "bundled", &mut provenance);
            let explicit = Price {
                input: raw.input,
                output: raw.output,
                cache_read: raw.cache_read,
                cache_write_5m: raw.cache_write_5m,
                cache_write_1h: raw.cache_write_1h,
                long_context: explicit_tier.filter(|_| {
                    base.long_context.is_none()
                        || cost
                            .get("tiers")
                            .and_then(Value::as_array)
                            .is_some_and(|tiers| !tiers.is_empty())
                }),
                fast: None,
                web_search_per_1k: None,
            };
            provenance_fields(
                &serde_json::to_value(explicit)?,
                "",
                "models_dev",
                &mut provenance,
            );
            if let Some(fast) = model.pointer("/experimental/modes/fast/cost") {
                let mut r = rates(fast, provider).map_err(|error| {
                    invalid(format!(
                        "provider {provider} model {id} field experimental.fast: {error}"
                    ))
                })?;
                r.long_context = long_context(fast, provider).map_err(|error| {
                    invalid(format!(
                        "provider {provider} model {id} field experimental.fast: {error}"
                    ))
                })?;
                // Fill only categories with an actual same-model fast fallback,
                // never multiply newly updated ordinary rates implicitly.
                let fallback = match base.fast {
                    Some(FastPrice::Rates { rates }) => rates,
                    Some(FastPrice::Multiplier { multiplier }) => {
                        let mut r = base_rates(base);
                        r.input = r.input.map(|v| v * multiplier);
                        r.output = r.output.map(|v| v * multiplier);
                        r.cache_read = r.cache_read.map(|v| v * multiplier);
                        r.cache_write_5m = r.cache_write_5m.map(|v| v * multiplier);
                        r.cache_write_1h = r.cache_write_1h.map(|v| v * multiplier);
                        if let Some(mut t) = r.long_context {
                            t.input = t.input.map(|v| v * multiplier);
                            t.output = t.output.map(|v| v * multiplier);
                            t.cache_read = t.cache_read.map(|v| v * multiplier);
                            t.cache_write_5m = t.cache_write_5m.map(|v| v * multiplier);
                            t.cache_write_1h = t.cache_write_1h.map(|v| v * multiplier);
                            r.long_context = Some(t);
                        }
                        r
                    }
                    None => FastRates::default(),
                };
                provenance_fields(
                    &serde_json::to_value(r)?,
                    "fast.rates",
                    "models_dev",
                    &mut provenance,
                );
                r = overlay_rates(r, fallback);
                let fallback_tier = fallback.long_context.filter(|t| {
                    price
                        .long_context
                        .is_some_and(|p| p.threshold_tokens == t.threshold_tokens)
                });
                r.long_context = overlay_tier(r.long_context, fallback_tier);
                price.fast = Some(FastPrice::Rates { rates: r });
            }
            table
                .providers
                .get_mut(provider)
                .expect("provider exists")
                .insert(
                    id.clone(),
                    Row {
                        price,
                        provenance,
                        v2_fetched_at: None,
                    },
                );
        }
        if priced < 2 {
            return Err(invalid(format!(
                "provider {provider} field cost: insufficient upstream priced models"
            )));
        }
    }
    table.validate()?;
    table.seal()?;
    Ok(table)
}

#[derive(Clone, Copy, Default)]
struct TtlRates {
    five_minutes: Option<f64>,
    one_hour: Option<f64>,
}

fn v2_ttl_rates(value: &Value, field: &str) -> io::Result<TtlRates> {
    if !value.is_object() {
        return Err(invalid(format!("field {field}: invalid cost object")));
    }
    let Some(write) = value.pointer("/text/input/cache/write") else {
        return Ok(TtlRates::default());
    };
    let object = write.as_object().ok_or_else(|| {
        invalid(format!(
            "field {field}.text.input.cache.write: invalid object"
        ))
    })?;
    let mut rates = TtlRates::default();
    for (key, target) in [("5m", &mut rates.five_minutes), ("1h", &mut rates.one_hour)] {
        if let Some(value) = object.get(key).filter(|value| !value.is_null()) {
            let rate = value.as_f64().ok_or_else(|| {
                invalid(format!(
                    "field {field}.text.input.cache.write.{key}: nonnumeric rate"
                ))
            })?;
            validate_rate(rate).map_err(|_| {
                invalid(format!(
                    "field {field}.text.input.cache.write.{key}: invalid rate"
                ))
            })?;
            *target = Some(rate);
        }
    }
    Ok(rates)
}

fn cache_write_path(one_hour: bool) -> &'static str {
    if one_hour {
        "cache_write_1h"
    } else {
        "cache_write_5m"
    }
}

fn copy_v2_category(target: &mut Row, previous: &Row, path: &str) {
    if previous.provenance.get(path).map(String::as_str) != Some("models_dev_v2")
        || target.provenance.get(path).map(String::as_str) == Some("models_dev")
    {
        return;
    }
    match path {
        "cache_write_5m" => {
            let Some(value) = previous.price.cache_write_5m else {
                return;
            };
            target.price.cache_write_5m = Some(value);
        }
        "cache_write_1h" => {
            let Some(value) = previous.price.cache_write_1h else {
                return;
            };
            target.price.cache_write_1h = Some(value);
        }
        "long_context.cache_write_5m" => {
            let (Some(new), Some(old)) =
                (&mut target.price.long_context, previous.price.long_context)
            else {
                return;
            };
            if new.threshold_tokens != old.threshold_tokens {
                return;
            }
            let Some(value) = old.cache_write_5m else {
                return;
            };
            new.cache_write_5m = Some(value);
        }
        "long_context.cache_write_1h" => {
            let (Some(new), Some(old)) =
                (&mut target.price.long_context, previous.price.long_context)
            else {
                return;
            };
            if new.threshold_tokens != old.threshold_tokens {
                return;
            }
            let Some(value) = old.cache_write_1h else {
                return;
            };
            new.cache_write_1h = Some(value);
        }
        _ => return,
    }
    target
        .provenance
        .insert(path.into(), "models_dev_v2".into());
    target.v2_fetched_at = previous.v2_fetched_at;
}

fn preserve_v2_supplement(table: &mut EffectiveTable, previous: Option<&EffectiveTable>) {
    let Some(previous) = previous else { return };
    let (Some(current_rows), Some(previous_rows)) = (
        table.providers.get_mut("anthropic"),
        previous.providers.get("anthropic"),
    ) else {
        return;
    };
    for (id, prior) in previous_rows {
        let Some(current) = current_rows.get_mut(id) else {
            continue;
        };
        for path in [
            "cache_write_5m",
            "cache_write_1h",
            "long_context.cache_write_5m",
            "long_context.cache_write_1h",
        ] {
            copy_v2_category(current, prior, path);
        }
    }
}

/// Apply only explicit Anthropic cache TTLs from the optional schema-2
/// supplement. The required v1 catalog remains independently usable.
fn supplement_v2(
    table: &mut EffectiveTable,
    v1_body: &[u8],
    v2_body: &[u8],
    fetched_at: u64,
    previous: Option<&EffectiveTable>,
) -> io::Result<()> {
    if v2_body.len() as u64 > MAX_BODY {
        return Err(invalid("models.dev v2 field response: too large"));
    }
    let api_v2: Value = serde_json::from_slice(v2_body)
        .map_err(|error| invalid(format!("models.dev v2 field JSON: {error}")))?;
    let provider = api_v2
        .get("anthropic")
        .ok_or_else(|| invalid("provider anthropic field provider: missing in v2"))?;
    if provider.get("id").and_then(Value::as_str) != Some("anthropic") {
        return Err(invalid(
            "provider anthropic field id: identity mismatch in v2",
        ));
    }
    let models = provider
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("provider anthropic field models: missing in v2"))?;
    let api_v1: Value = serde_json::from_slice(v1_body)
        .map_err(|error| invalid(format!("models.dev v1 field JSON: {error}")))?;
    let v1_models = api_v1
        .pointer("/anthropic/models")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("provider anthropic field models: missing in v1"))?;
    let rows = table
        .providers
        .get_mut("anthropic")
        .ok_or_else(|| invalid("provider anthropic field cache: missing"))?;

    for (id, model) in models {
        // Key/model mismatches and v2 aliases are ignored. Only exact IDs in
        // the merged v1/bootstrap table can receive this supplement.
        if model.get("id").and_then(Value::as_str) != Some(id.as_str()) {
            eprintln!(
                "semon: ignored models.dev v2 provider anthropic model {id} field id: mismatch"
            );
            if let (Some(current), Some(prior)) = (
                rows.get_mut(id),
                previous
                    .and_then(|table| table.providers.get("anthropic"))
                    .and_then(|rows| rows.get(id)),
            ) {
                for path in [
                    "cache_write_5m",
                    "cache_write_1h",
                    "long_context.cache_write_5m",
                    "long_context.cache_write_1h",
                ] {
                    copy_v2_category(current, prior, path);
                }
            }
            continue;
        }
        let Some(current) = rows.get_mut(id) else {
            continue;
        };
        let Some(cost) = model.get("cost") else {
            eprintln!(
                "semon: ignored models.dev v2 provider anthropic model {id} field cost: missing"
            );
            if let Some(prior) = previous
                .and_then(|table| table.providers.get("anthropic"))
                .and_then(|rows| rows.get(id))
            {
                for path in [
                    "cache_write_5m",
                    "cache_write_1h",
                    "long_context.cache_write_5m",
                    "long_context.cache_write_1h",
                ] {
                    copy_v2_category(current, prior, path);
                }
            }
            continue;
        };
        let base = match v2_ttl_rates(cost, "cost") {
            Ok(rates) => rates,
            Err(error) => {
                eprintln!("semon: ignored models.dev v2 provider anthropic model {id} {error}");
                if let Some(prior) = previous
                    .and_then(|table| table.providers.get("anthropic"))
                    .and_then(|rows| rows.get(id))
                {
                    for path in [
                        "cache_write_5m",
                        "cache_write_1h",
                        "long_context.cache_write_5m",
                        "long_context.cache_write_1h",
                    ] {
                        copy_v2_category(current, prior, path);
                    }
                }
                continue;
            }
        };

        for (one_hour, value) in [(false, base.five_minutes), (true, base.one_hour)] {
            let path = cache_write_path(one_hour);
            if let Some(value) = value
                && current.provenance.get(path).map(String::as_str) != Some("models_dev")
            {
                if one_hour {
                    current.price.cache_write_1h = Some(value);
                } else {
                    current.price.cache_write_5m = Some(value);
                }
                current
                    .provenance
                    .insert(path.into(), "models_dev_v2".into());
                current.v2_fetched_at = Some(fetched_at);
            }
        }

        // Context cache-write tiers are useful only at the exact v1 boundary.
        let v1_cost = v1_models.get(id).and_then(|model| model.get("cost"));
        let v1_threshold = v1_cost
            .and_then(|cost| long_context(cost, "anthropic").ok().flatten())
            .map(|tier| tier.threshold_tokens);
        let Some(threshold) = v1_threshold else {
            continue;
        };
        let tier_entries = match cost.get("tiers") {
            None | Some(Value::Null) => continue,
            Some(Value::Array(entries)) => entries,
            Some(_) => {
                eprintln!(
                    "semon: ignored models.dev v2 provider anthropic model {id} field cost.tiers: invalid array"
                );
                if let Some(prior) = previous
                    .and_then(|table| table.providers.get("anthropic"))
                    .and_then(|rows| rows.get(id))
                {
                    for path in ["long_context.cache_write_5m", "long_context.cache_write_1h"] {
                        copy_v2_category(current, prior, path);
                    }
                }
                continue;
            }
        };
        let mut matching = tier_entries.iter().filter(|tier| {
            tier.pointer("/tier/type").and_then(Value::as_str) == Some("context")
                && tier.pointer("/tier/size").and_then(Value::as_u64) == Some(threshold)
        });
        let Some(entry) = matching.next() else {
            continue;
        };
        if matching.next().is_some() {
            eprintln!(
                "semon: ignored models.dev v2 provider anthropic model {id} field cost.tiers: duplicate context boundary"
            );
            if let Some(prior) = previous
                .and_then(|table| table.providers.get("anthropic"))
                .and_then(|rows| rows.get(id))
            {
                for path in ["long_context.cache_write_5m", "long_context.cache_write_1h"] {
                    copy_v2_category(current, prior, path);
                }
            }
            continue;
        }
        let tier_cost = entry.get("cost").unwrap_or(entry);
        let tier = match v2_ttl_rates(tier_cost, "cost.tiers[].cost") {
            Ok(rates) => rates,
            Err(error) => {
                eprintln!("semon: ignored models.dev v2 provider anthropic model {id} {error}");
                if let Some(prior) = previous
                    .and_then(|table| table.providers.get("anthropic"))
                    .and_then(|rows| rows.get(id))
                {
                    for path in ["long_context.cache_write_5m", "long_context.cache_write_1h"] {
                        copy_v2_category(current, prior, path);
                    }
                }
                continue;
            }
        };
        let Some(price_tier) = current
            .price
            .long_context
            .as_mut()
            .filter(|tier| tier.threshold_tokens == threshold)
        else {
            continue;
        };
        for (one_hour, value) in [(false, tier.five_minutes), (true, tier.one_hour)] {
            let path = if one_hour {
                "long_context.cache_write_1h"
            } else {
                "long_context.cache_write_5m"
            };
            if let Some(value) = value
                && current.provenance.get(path).map(String::as_str) != Some("models_dev")
            {
                if one_hour {
                    price_tier.cache_write_1h = Some(value);
                } else {
                    price_tier.cache_write_5m = Some(value);
                }
                current
                    .provenance
                    .insert(path.into(), "models_dev_v2".into());
                current.v2_fetched_at = Some(fetched_at);
            }
        }
    }
    table.validate()?;
    table.seal()?;
    Ok(())
}

fn read_cache(path: &Path) -> io::Result<EffectiveTable> {
    let file = fs::File::open(path)?;
    let mut body = Vec::new();
    file.take(MAX_BODY + 1).read_to_end(&mut body)?;
    if body.len() as u64 > MAX_BODY {
        return Err(invalid("pricing cache too large"));
    }
    let table: EffectiveTable = serde_json::from_slice(&body)?;
    table.validate()?;
    let mut verified = table.clone();
    verified.seal()?;
    if verified.fingerprint != table.fingerprint {
        return Err(invalid("pricing fingerprint mismatch"));
    }
    Ok(table)
}

fn persist(path: &Path, table: &EffectiveTable) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing pricing cache directory"))?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        let bytes = serde_json::to_vec(table)?;
        if bytes.len() as u64 > MAX_BODY {
            return Err(invalid("normalized pricing cache too large"));
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

trait Fetcher {
    fn fetch(&self) -> io::Result<Vec<u8>>;
    fn fetch_v2(&self) -> io::Result<Vec<u8>> {
        Err(invalid("optional models.dev v2 endpoint unavailable"))
    }
}
struct HttpFetcher;
fn fetch_url(url: &str) -> io::Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| invalid("pricing HTTP client unavailable"))?;
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|_| invalid(format!("pricing request failed for {url}")))?;
    if response.content_length().is_some_and(|n| n > MAX_BODY) {
        return Err(invalid(format!("pricing response too large for {url}")));
    }
    let mut body = Vec::new();
    response.take(MAX_BODY + 1).read_to_end(&mut body)?;
    if body.len() as u64 > MAX_BODY {
        return Err(invalid(format!("pricing response too large for {url}")));
    }
    Ok(body)
}
impl Fetcher for HttpFetcher {
    fn fetch(&self) -> io::Result<Vec<u8>> {
        fetch_url("https://models.dev/api.json")
    }
    fn fetch_v2(&self) -> io::Result<Vec<u8>> {
        fetch_url("https://models.dev/experimental/v2.0/api.json")
    }
}

fn refresh(path: &Path, fetcher: &impl Fetcher) -> io::Result<EffectiveTable> {
    let body = fetcher.fetch()?;
    let previous = read_cache(path).ok();
    let fetched_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut table = adapt(&body, fetched_at)?;
    match fetcher.fetch_v2() {
        Ok(v2) => {
            let mut candidate = table.clone();
            match supplement_v2(&mut candidate, &body, &v2, fetched_at, previous.as_ref()) {
                Ok(()) => table = candidate,
                Err(error) => {
                    eprintln!(
                        "semon: optional models.dev v2 refresh failed ({error}); retaining prior TTL supplement"
                    );
                    preserve_v2_supplement(&mut table, previous.as_ref());
                }
            }
        }
        Err(error) => {
            eprintln!(
                "semon: optional models.dev v2 refresh failed ({error}); retaining prior TTL supplement"
            );
            preserve_v2_supplement(&mut table, previous.as_ref());
        }
    }
    table.validate()?;
    table.seal()?;
    // A disk failure is an unsuccessful update. The old table remains active.
    persist(path, &table)?;
    Ok(table)
}

static INITIALIZED: OnceLock<PathBuf> = OnceLock::new();
/// Load validated local reference data once, with no network I/O. Call before
/// admitting consumers. One-shot commands may use this without starting threads.
pub fn initialize(data_dir: &Path) {
    INITIALIZED.get_or_init(|| {
        let path = data_dir.join("models-dev-pricing.json");
        match read_cache(&path) {
            Ok(table) => install(table),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => eprintln!("semon: invalid pricing cache; using bundled prices"),
        }
        path
    });
}

/// Load the cache from Semon's existing XDG state directory.
pub fn initialize_default() {
    initialize(&crate::state_dir());
}
/// Long-running standalone Semon entrypoint.
pub fn start_default_refresh() {
    start_refresh(&crate::state_dir());
}

static STARTED: OnceLock<()> = OnceLock::new();
#[derive(Default)]
struct IndexRegistry {
    entries: BTreeMap<PathBuf, Instant>,
    wake_epoch: u64,
}
impl IndexRegistry {
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|path, touched| {
            let age = now.saturating_duration_since(*touched);
            age < INDEX_RETIRE_AFTER && path.is_file()
        });
    }

    fn register(&mut self, path: PathBuf, now: Instant) -> bool {
        self.prune(now);
        let newly_registered = !self.entries.contains_key(&path);
        self.entries.insert(path, now);
        if self.entries.len() > MAX_INDEXES
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, touched)| **touched)
                .map(|(path, _)| path.clone())
        {
            self.entries.remove(&oldest);
        }
        if newly_registered {
            self.wake_epoch = self.wake_epoch.wrapping_add(1);
        }
        newly_registered
    }
}

static INDEXES: OnceLock<(Mutex<IndexRegistry>, Condvar)> = OnceLock::new();
fn registry() -> &'static (Mutex<IndexRegistry>, Condvar) {
    INDEXES.get_or_init(|| (Mutex::new(IndexRegistry::default()), Condvar::new()))
}

/// A new index wakes the worker; re-registering one only refreshes its LRU age.
/// The process-local set is capped; persistent SQLite cursors remain authoritative.
pub(crate) fn register_index(path: PathBuf) {
    let (state, wake) = registry();
    let newly_registered = state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .register(path, Instant::now());
    if newly_registered {
        wake.notify_one();
    }
}

/// Wake a parked projection worker after a writer may have enqueued an owner.
pub(crate) fn notify_reprice() {
    let (state, wake) = registry();
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    state.wake_epoch = state.wake_epoch.wrapping_add(1);
    wake.notify_one();
}

fn reprice_worker() {
    loop {
        let (state_lock, wake) = registry();
        let mut state = state_lock.lock().unwrap_or_else(|e| e.into_inner());
        state.prune(Instant::now());
        let mut observed_epoch = state.wake_epoch;
        if state.entries.is_empty() {
            let epoch = state.wake_epoch;
            let (next, _) = wake
                .wait_timeout_while(state, WORKER_FALLBACK_WAKE, |state| {
                    state.entries.is_empty() && state.wake_epoch == epoch
                })
                .unwrap_or_else(|error| error.into_inner());
            state = next;
            state.prune(Instant::now());
            observed_epoch = state.wake_epoch;
        }
        let paths: Vec<_> = state.entries.keys().cloned().collect();
        drop(state);

        let table = snapshot();
        let mut progress = false;
        for path in paths {
            match crate::events::store::reprice::step(&path, Arc::clone(&table)) {
                Ok(changed) => progress |= changed,
                Err(error) => {
                    // Failure is retried; it must never discard native facts.
                    eprintln!("semon: derived cost refresh deferred: {error}");
                }
            }
        }
        if !progress {
            let (state_lock, wake) = registry();
            let state = state_lock.lock().unwrap_or_else(|e| e.into_inner());
            if state.wake_epoch != observed_epoch {
                continue;
            }
            let epoch = observed_epoch;
            let _ = wake
                .wait_timeout_while(state, WORKER_FALLBACK_WAKE, |state| {
                    state.wake_epoch == epoch
                })
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

/// Start exactly one process-wide background refresh and repricing worker.
/// Foreground reads only clone an immutable Arc; they never wait for HTTP.
pub fn start_refresh(data_dir: &Path) {
    initialize(data_dir);
    STARTED.get_or_init(|| {
        let path=INITIALIZED.get().expect("pricing initialized").clone();
        let off=std::env::var("SEMON_PRICING_REFRESH").ok().as_deref()==Some("off");
        if !off {
            let interval=std::env::var("SEMON_PRICING_REFRESH_SECS").ok().and_then(|v|v.parse::<u64>().ok()).filter(|s|*s>0).unwrap_or(INTERVAL);
            if std::thread::Builder::new().name("semon-prices".into()).spawn(move || loop {
                let delay=match refresh(&path,&HttpFetcher) {
                    Ok(table)=>{install(table);interval},
                    Err(error)=>{eprintln!("semon: pricing refresh failed ({error}); retaining last-known-good prices");RETRY},
                };
                std::thread::sleep(Duration::from_secs(delay));
            }).is_err() { eprintln!("semon: pricing refresh worker unavailable"); }
        }
        if std::thread::Builder::new()
            .name("semon-reprice".into())
            .spawn(reprice_worker)
            .is_err()
        {
            eprintln!("semon: pricing projection worker unavailable");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{BillingUsage, MessageUsage},
        pricing::CostBuild,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    const FIXTURE: &[u8] = include_bytes!("fixtures/models-dev.json");
    const V2_FIXTURE: &[u8] = include_bytes!("fixtures/models-dev-v2.json");
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn path() -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "semon-pricing-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
            .join("prices.json")
    }
    fn fixture() -> Value {
        serde_json::from_slice(FIXTURE).unwrap()
    }
    fn table(value: Value) -> EffectiveTable {
        adapt(&serde_json::to_vec(&value).unwrap(), 10).unwrap()
    }

    #[test]
    fn v2_ttl_supplement_prices_new_exact_anthropic_ids_and_matching_context_tiers() {
        let mut v1 = fixture();
        v1["anthropic"]["models"]["claude-new"] = serde_json::json!({
            "id":"claude-new",
            "cost":{
                "input":7,"output":11,"cache_write":9,
                "tiers":[
                    {"tier":{"type":"image","size":1},"input":"ignored"},
                    {"tier":{"type":"context","size":272000},"input":10,"output":15}
                ],
                "future_metric":{"not_a_rate":"ignored"}
            }
        });
        v1["anthropic"]["models"]["claude-opus-5-5"]["cost"]["cache_write_5m"] = Value::from(2.0);
        let v1_body = serde_json::to_vec(&v1).unwrap();
        let mut v2: Value = serde_json::from_slice(V2_FIXTURE).unwrap();
        v2["anthropic"]["models"]["claude-opus-5-5"] = serde_json::json!({
            "id":"claude-opus-5-5",
            "cost":{"text":{"input":{"cache":{"write":{"5m":9.0,"1h":10.0}}}}}
        });
        let mut effective = adapt(&v1_body, 100).unwrap();
        supplement_v2(
            &mut effective,
            &v1_body,
            &serde_json::to_vec(&v2).unwrap(),
            200,
            None,
        )
        .unwrap();

        let new = effective.price_for("anthropic", "claude-new").unwrap();
        assert_eq!(new.cache_write_5m, Some(3.75));
        assert_eq!(new.cache_write_1h, Some(6.0));
        let long = new.long_context.unwrap();
        assert_eq!(long.threshold_tokens, 272_000);
        assert_eq!(long.cache_write_5m, Some(5.5));
        assert_eq!(long.cache_write_1h, Some(8.0));
        let row = &effective.providers["anthropic"]["claude-new"];
        assert_eq!(row.provenance["cache_write_5m"], "models_dev_v2");
        assert_eq!(row.v2_fetched_at, Some(200));

        // Explicit v1 category data outranks the optional v2 field.
        let opus = &effective.providers["anthropic"]["claude-opus-5-5"];
        assert_eq!(opus.price.cache_write_5m, Some(2.0));
        assert_eq!(opus.provenance["cache_write_5m"], "models_dev");
        // The same model ID in OpenAI, and an Anthropic alias absent from v1,
        // are not mapped from the supplement.
        assert_eq!(
            effective
                .price_for("openai", "claude-new")
                .unwrap()
                .cache_write_5m,
            None
        );
        assert!(
            effective
                .providers
                .get("anthropic")
                .is_none_or(|rows| !rows.contains_key("claude-new-alias"))
        );

        let used = MessageUsage {
            model: Some("claude-new".into()),
            billing: BillingUsage {
                cache_write_5m: 1_000_000,
                prompt_size: 1_000_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cost = CostBuild::with_table(Arc::new(effective));
        cost.message(&used);
        assert_eq!(cost.finish().usd, Some(3.75));
    }

    #[test]
    fn optional_v2_failure_and_invalid_entries_keep_prior_ttl_while_v1_advances() {
        let mut v1 = fixture();
        v1["anthropic"]["models"]["claude-new"] = serde_json::json!({
            "id":"claude-new","cost":{"input":7,"output":11,"cache_write":9}
        });
        let v1_body = serde_json::to_vec(&v1).unwrap();
        let mut prior = adapt(&v1_body, 100).unwrap();
        supplement_v2(&mut prior, &v1_body, V2_FIXTURE, 200, None).unwrap();

        let mut changed_v1 = v1.clone();
        changed_v1["anthropic"]["models"]["claude-new"]["cost"]["input"] = Value::from(9.0);
        let changed_body = serde_json::to_vec(&changed_v1).unwrap();
        let mut updated = adapt(&changed_body, 300).unwrap();
        preserve_v2_supplement(&mut updated, Some(&prior));
        assert_eq!(
            updated.price_for("anthropic", "claude-new").unwrap().input,
            Some(9.0)
        );
        let cached = &updated.providers["anthropic"]["claude-new"];
        assert_eq!(cached.price.cache_write_5m, Some(3.75));
        assert_eq!(cached.v2_fetched_at, Some(200));

        let mut invalid_v2: Value = serde_json::from_slice(V2_FIXTURE).unwrap();
        invalid_v2["anthropic"]["models"]["claude-new"]["cost"]["text"]["input"]["cache"]["write"]
            ["5m"] = Value::from("bad");
        let mut candidate = adapt(&changed_body, 400).unwrap();
        supplement_v2(
            &mut candidate,
            &changed_body,
            &serde_json::to_vec(&invalid_v2).unwrap(),
            500,
            Some(&prior),
        )
        .unwrap();
        let row = &candidate.providers["anthropic"]["claude-new"];
        assert_eq!(row.price.input, Some(9.0));
        assert_eq!(row.price.cache_write_5m, Some(3.75));
        assert_eq!(row.v2_fetched_at, Some(200));
    }

    struct Responses {
        primary: Vec<u8>,
        supplement: Option<Vec<u8>>,
    }
    impl Fetcher for Responses {
        fn fetch(&self) -> io::Result<Vec<u8>> {
            Ok(self.primary.clone())
        }
        fn fetch_v2(&self) -> io::Result<Vec<u8>> {
            self.supplement
                .clone()
                .ok_or_else(|| invalid("optional v2 fixture failure"))
        }
    }

    #[test]
    fn refresh_commits_valid_v1_when_optional_v2_fetch_fails() {
        let path = path();
        let mut v1 = fixture();
        v1["anthropic"]["models"]["claude-new"] = serde_json::json!({
            "id":"claude-new","cost":{"input":7,"output":11,"cache_write":9}
        });
        let first_body = serde_json::to_vec(&v1).unwrap();
        let first = refresh(
            &path,
            &Responses {
                primary: first_body,
                supplement: Some(V2_FIXTURE.to_vec()),
            },
        )
        .unwrap();
        let mut changed_v1 = v1;
        changed_v1["anthropic"]["models"]["claude-new"]["cost"]["input"] = Value::from(9.0);
        let second = refresh(
            &path,
            &Responses {
                primary: serde_json::to_vec(&changed_v1).unwrap(),
                supplement: None,
            },
        )
        .unwrap();
        let row = &second.providers["anthropic"]["claude-new"];
        assert_eq!(row.price.input, Some(9.0));
        assert_eq!(row.price.cache_write_5m, Some(3.75));
        assert_eq!(
            row.v2_fetched_at,
            first.providers["anthropic"]["claude-new"].v2_fetched_at
        );
        assert_eq!(read_cache(&path).unwrap().fingerprint, second.fingerprint);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn index_registry_caps_entries_and_evicts_the_least_recently_used() {
        let root = path().parent().unwrap().join("registry");
        fs::create_dir_all(&root).unwrap();
        let start = Instant::now();
        let mut registry = IndexRegistry::default();
        for index in 0..MAX_INDEXES {
            let path = root.join(format!("index-{index}"));
            fs::write(&path, b"").unwrap();
            registry.register(path, start + Duration::from_secs(index as u64));
        }
        let recently_used = root.join("index-0");
        registry.register(recently_used.clone(), start + Duration::from_secs(70));
        let over_cap = root.join("index-over-cap");
        fs::write(&over_cap, b"").unwrap();
        registry.register(over_cap, start + Duration::from_secs(71));
        assert_eq!(registry.entries.len(), MAX_INDEXES);
        assert!(registry.entries.contains_key(&recently_used));
        assert!(!registry.entries.contains_key(&root.join("index-1")));
        fs::remove_file(&recently_used).unwrap();
        registry.prune(start + Duration::from_secs(72));
        assert!(!registry.entries.contains_key(&recently_used));
        registry.prune(start + INDEX_RETIRE_AFTER + Duration::from_secs(72));
        assert!(registry.entries.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_registry_repeated_registration_wakes_only_for_new_or_retired_paths() {
        let root = path().parent().unwrap().join("registry-wakes");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("first");
        let second = root.join("second");
        fs::write(&first, b"").unwrap();
        fs::write(&second, b"").unwrap();
        let start = Instant::now();
        let mut registry = IndexRegistry::default();
        let initial_epoch = registry.wake_epoch;

        assert!(registry.register(first.clone(), start));
        assert_eq!(registry.wake_epoch, initial_epoch + 1);
        for touch in 1..8 {
            assert!(!registry.register(first.clone(), start + Duration::from_secs(touch)));
        }
        assert_eq!(registry.wake_epoch, initial_epoch + 1);

        assert!(registry.register(second.clone(), start + Duration::from_secs(8)));
        assert_eq!(registry.wake_epoch, initial_epoch + 2);

        let retired_at = start + INDEX_RETIRE_AFTER + Duration::from_secs(9);
        registry.prune(retired_at);
        assert!(!registry.entries.contains_key(&first));
        let before_reregister = registry.wake_epoch;
        assert!(registry.register(first.clone(), retired_at));
        assert_eq!(registry.wake_epoch, before_reregister + 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_identity_tiers_fast_and_cache_ttl_semantics() {
        let mut value = fixture();
        let cost = &mut value["anthropic"]["models"]["claude-opus-5-5"]["cost"];
        cost["cache_write_5m"] = Value::from(6.75);
        let fast =
            &mut value["openai"]["models"]["gpt-6.1-sol"]["experimental"]["modes"]["fast"]["cost"];
        fast["output"] = Value::from(17.0); // deliberately not a uniform multiplier
        let table = table(value);
        let openai = table.price_for("openai", "gpt-6.1-sol").unwrap();
        assert_eq!(openai.long_context.unwrap().threshold_tokens, 272_000);
        assert_eq!(openai.cache_write_5m, None);
        assert_eq!(openai.cache_write_1h, None);
        let anthropic = table.price_for("anthropic", "claude-opus-5-5").unwrap();
        assert_eq!(anthropic.cache_write_5m, Some(6.75));
        assert_eq!(
            anthropic.cache_write_1h,
            bundled()
                .price_for("anthropic", "claude-opus-5-5")
                .unwrap()
                .cache_write_1h
        );
        assert!(table.price_for("anthropic", "gpt-6.1-sol").is_none());
        assert!(table.price_for("openai", "claude-opus-5-5").is_none());
        let FastPrice::Rates { rates } = openai.fast.unwrap() else {
            panic!("explicit fast rates expected")
        };
        assert_eq!(rates.input, Some(4.0));
        assert_eq!(rates.output, Some(17.0));
        assert_eq!(
            table.providers["anthropic"]["claude-opus-5-5"].provenance["cache_write_1h"],
            "bundled"
        );
    }

    #[test]
    fn newly_discovered_models_price_actual_billing_and_missing_categories_stay_unknown() {
        let mut value = fixture();
        value["anthropic"]["models"]["claude-new"] =
            serde_json::json!({"id":"claude-new","cost":{"input":7,"output":11,"cache_write":9}});
        let table = Arc::new(table(value));
        assert!(bundled().price_for("anthropic", "claude-new").is_none());
        let mut used = MessageUsage {
            model: Some("claude-new".into()),
            billing: BillingUsage {
                input: 1_000_000,
                prompt_size: 1_000_000,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut cost = CostBuild::with_table(Arc::clone(&table));
        cost.message(&used);
        assert_eq!(cost.finish().usd, Some(7.0));
        used.billing.cache_write_5m = 1;
        let mut cost = CostBuild::with_table(table);
        cost.message(&used);
        assert_eq!(cost.finish().usd, None);
    }

    #[test]
    fn long_threshold_and_explicit_fast_are_used_by_the_cost_consumer() {
        let mut value = fixture();
        value["openai"]["models"]["gpt-6.1-sol"]["experimental"]["modes"]["fast"]["cost"]["output"] =
            Value::from(17.0);
        let mut new = value["openai"]["models"]["gpt-6.1-sol"].clone();
        new["id"] = Value::from("gpt-new");
        value["openai"]["models"]["gpt-new"] = new;
        let table = Arc::new(table(value));
        let event = crate::events::CodexUsageEvent {
            model: "gpt-6.1-sol".into(),
            tokens: crate::events::ModelTokens {
                input: 272_000,
                output: 0,
                cache_write: 0,
                cache_read: 0,
            },
            timestamp: None,
        };
        let mut at = CostBuild::with_table(Arc::clone(&table));
        at.codex(&event);
        assert_eq!(at.finish().usd, Some(0.544));
        let mut above = event.clone();
        above.tokens.input += 1;
        let mut cost = CostBuild::with_table(Arc::clone(&table));
        cost.codex(&above);
        assert_eq!(cost.finish().usd, Some(1.088004));
        // The shared calculator accepts explicit billing dimensions, including
        // nonuniform fast rates, independently of native adapter selection.
        let p = table.price_for("openai", "gpt-new").unwrap();
        let mut message = MessageUsage {
            model: Some("synthetic".into()),
            billing: BillingUsage {
                input: 1_000_000,
                output: 1_000_000,
                speed: Some("fast".into()),
                prompt_size: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            super::super::calculate_cost_with(&[message.clone()], &[], |_| Some(p)).usd,
            Some(21.0)
        );
        message.billing.prompt_size = 300_000;
        // No upstream fast long-context tier or same-boundary bundled fast
        // fallback exists for this model: unknown is the honest result.
        assert_eq!(
            super::super::calculate_cost_with(&[message], &[], |_| Some(p)).usd,
            None
        );
    }

    struct Offline(io::Result<Vec<u8>>);
    impl Fetcher for Offline {
        fn fetch(&self) -> io::Result<Vec<u8>> {
            self.0.as_ref().cloned().map_err(|_| invalid("offline"))
        }
    }

    #[test]
    fn invalid_updates_retain_the_last_known_good_cache() {
        let path = path();
        let good = adapt(FIXTURE, 1).unwrap();
        persist(&path, &good).unwrap();
        let before = fs::read(&path).unwrap();
        let mut cases = Vec::new();
        for rate in [Value::from(-1), Value::from("2.0")] {
            let mut value = fixture();
            value["openai"]["models"]["gpt-6.1-sol"]["cost"]["input"] = rate;
            cases.push(serde_json::to_vec(&value).unwrap());
        }
        let mut missing = fixture();
        missing.as_object_mut().unwrap().remove("anthropic");
        cases.push(serde_json::to_vec(&missing).unwrap());
        let mut empty = fixture();
        empty["anthropic"]["models"] = serde_json::json!({});
        cases.push(serde_json::to_vec(&empty).unwrap());
        let mut zero = fixture();
        zero["openai"]["models"]["gpt-6.1-sol"]["cost"]["tiers"][0]["tier"]["size"] =
            Value::from(0);
        cases.push(serde_json::to_vec(&zero).unwrap());
        cases.extend([
            b"".to_vec(),
            b"{\"openai\":NaN}".to_vec(),
            vec![b' '; MAX_BODY as usize + 1],
        ]);
        for body in cases {
            assert!(refresh(&path, &Offline(Ok(body))).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        assert!(refresh(&path, &Offline(Err(invalid("offline")))).is_err());
        assert_eq!(read_cache(&path).unwrap().fingerprint, good.fingerprint);
        let mut nonfinite = good.clone();
        nonfinite
            .providers
            .get_mut("openai")
            .unwrap()
            .get_mut("gpt-6.1-sol")
            .unwrap()
            .price
            .input = Some(f64::NAN);
        assert!(nonfinite.validate().is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn atomic_cache_reload_corruption_and_disk_failure() {
        let path = path();
        let first = refresh(&path, &Offline(Ok(FIXTURE.to_vec()))).unwrap();
        assert_eq!(read_cache(&path).unwrap().fingerprint, first.fingerprint);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let mut reordered = fixture();
        reordered["openai"]["models"]["gpt-6.1-sol"]["name"] = Value::from("irrelevant metadata");
        assert_eq!(table(reordered).fingerprint, first.fingerprint);
        fs::write(&path, b"corrupt").unwrap();
        assert!(read_cache(&path).is_err());
        // The startup selection has the same pure, offline fallback rule.
        let selected = read_cache(&path).unwrap_or_else(|_| bundled());
        assert_eq!(selected.fingerprint, bundled().fingerprint);
        let obstacle = path.parent().unwrap().join("file");
        fs::write(&obstacle, b"file").unwrap();
        assert!(refresh(&obstacle.join("prices"), &Offline(Ok(FIXTURE.to_vec()))).is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
