//! Public reference data only: no provider credentials or native source access.
use super::{FastPrice, FastRates, LongContextPrice, Price, normalize_model_id, price_data};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_BODY: u64 = 16 * 1024 * 1024;
const INTERVAL: u64 = 6 * 60 * 60;
const RETRY: u64 = 30 * 60;
const CACHE_VERSION: u32 = 1;
const PROVIDERS: [&str; 2] = ["anthropic", "openai"];

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Row {
    price: Price,
    provenance: BTreeMap<String, String>,
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
        // and fetch time. Include provenance as well as all effective rates.
        self.fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&self.providers)?));
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
                validate_price(&row.price)?;
            }
        }
        Ok(())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
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
    *store().write().unwrap_or_else(|e| e.into_inner()) = Arc::new(table);
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
            let value = value.as_f64().ok_or_else(|| invalid("nonnumeric rate"))?;
            validate_rate(value)?;
            Ok(Some(value))
        }
    }
}

fn rates(cost: &Value, provider: &str) -> io::Result<FastRates> {
    if !cost.is_object() {
        return Err(invalid("invalid cost object"));
    }
    // Validate even fields whose billing semantics we deliberately do not map.
    for (key, value) in cost.as_object().expect("object checked") {
        if key != "tiers" && key != "context_over_200k" && !value.is_null() {
            let value = value.as_f64().ok_or_else(|| invalid("nonnumeric rate"))?;
            validate_rate(value)?;
        }
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
            let tiers = value.as_array().ok_or_else(|| invalid("invalid tiers"))?;
            // The native consumer supports one context boundary. Reject a new
            // shape atomically rather than flatten multiple tiers incorrectly.
            if tiers.len() > 1 {
                return Err(invalid("multiple context tiers unsupported"));
            }
            tiers
                .first()
                .map(|tier| {
                    if tier["tier"]["type"] != "context" {
                        return Err(invalid("unsupported tier type"));
                    }
                    let size = tier["tier"]["size"]
                        .as_u64()
                        .filter(|v| *v > 0)
                        .ok_or_else(|| invalid("invalid context boundary"))?;
                    Ok((size, tier))
                })
                .transpose()?
        }
    };
    if let Some(compatibility) = cost.get("context_over_200k").filter(|v| !v.is_null()) {
        rates(compatibility, provider)?;
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
    let r = rates(&value, provider)?;
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
            .ok_or_else(|| invalid("missing provider"))?;
        if entry["id"] != provider {
            return Err(invalid("provider identity mismatch"));
        }
        let models = entry["models"]
            .as_object()
            .ok_or_else(|| invalid("missing models"))?;
        let mut priced = 0;
        for (id, model) in models {
            if model["id"] != *id {
                return Err(invalid("model identity mismatch"));
            }
            let Some(cost) = model.get("cost").filter(|v| !v.is_null()) else {
                continue;
            };
            let raw = rates(cost, provider)?;
            if raw.input.is_some() && raw.output.is_some() {
                priced += 1;
            }
            // Exact lookup first; the existing native dated alias is allowed
            // within the same provider only for bundled fallback.
            let base = bootstrap.price_for(provider, id).unwrap_or_default();
            let r = overlay_rates(raw, base_rates(base));
            let explicit_tier = long_context(cost, provider)?;
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
                let mut r = rates(fast, provider)?;
                r.long_context = long_context(fast, provider)?;
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
                .insert(id.clone(), Row { price, provenance });
        }
        if priced < 2 {
            return Err(invalid("insufficient upstream priced models"));
        }
    }
    table.validate()?;
    table.seal()?;
    Ok(table)
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
}
struct HttpFetcher;
impl Fetcher for HttpFetcher {
    fn fetch(&self) -> io::Result<Vec<u8>> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| invalid("pricing HTTP client unavailable"))?;
        let response = client
            .get("https://models.dev/api.json")
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|_| invalid("pricing request failed"))?;
        if response.content_length().is_some_and(|n| n > MAX_BODY) {
            return Err(invalid("pricing response too large"));
        }
        let mut body = Vec::new();
        response.take(MAX_BODY + 1).read_to_end(&mut body)?;
        if body.len() as u64 > MAX_BODY {
            return Err(invalid("pricing response too large"));
        }
        Ok(body)
    }
}

fn refresh(path: &Path, fetcher: &impl Fetcher) -> io::Result<EffectiveTable> {
    let body = fetcher.fetch()?;
    let table = adapt(
        &body,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )?;
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
static INDEXES: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
pub(crate) fn register_index(path: PathBuf) {
    INDEXES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path);
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
                    Err(_)=>{eprintln!("semon: pricing refresh failed; retaining last-known-good prices");RETRY},
                };
                std::thread::sleep(Duration::from_secs(delay));
            }).is_err() { eprintln!("semon: pricing refresh worker unavailable"); }
        }
        if std::thread::Builder::new().name("semon-reprice".into()).spawn(|| loop {
            let paths: Vec<_>=INDEXES.get_or_init(Mutex::default).lock().unwrap_or_else(|e|e.into_inner()).iter().cloned().collect();
            let mut progress=false;
            for path in paths {
                match crate::events::store::reprice::step(&path,snapshot()) {
                    Ok(changed)=>progress |= changed,
                    Err(_error)=>{
                    // Failure is retried; it must never discard native facts.
                    eprintln!("semon: derived cost refresh deferred");
                    }
                }
            }
            std::thread::sleep(if progress { Duration::from_millis(100) } else { Duration::from_secs(2) });
        }).is_err() { eprintln!("semon: pricing projection worker unavailable"); }
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
