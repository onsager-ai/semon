//! Opt-in producer measurements. Identifiers are opaque diagnostics, never
//! source authority, and contain no cache paths or native content.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, time::Instant};

pub(crate) fn enabled() -> bool {
    std::env::var("SEMON_BUILD_TIMING").is_ok_and(|value| value == "1")
}

pub(crate) fn id(index: &Path) -> String {
    format!("{:x}", Sha256::digest(index.as_os_str().as_encoded_bytes()))
}

/// Bounded operation aggregates: `(microseconds, workload units)`. Call sites
/// count source observations, rows, postings or scope groups, never physical I/O.
/// Disabled measurements retain no entries and start no clocks.
#[derive(Default)]
pub(crate) struct Measurements {
    enabled: bool,
    values: BTreeMap<&'static str, (u128, usize)>,
}

impl Measurements {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub(crate) fn start(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub(crate) fn record(&mut self, name: &'static str, started: Option<Instant>, units: usize) {
        if let Some(started) = started {
            let entry = self.values.entry(name).or_default();
            entry.0 = entry.0.saturating_add(started.elapsed().as_micros());
            entry.1 = entry.1.saturating_add(units);
        }
    }

    pub(crate) fn count(&mut self, name: &'static str, units: usize) {
        if self.enabled {
            let entry = self.values.entry(name).or_default();
            entry.1 = entry.1.saturating_add(units);
        }
    }

    pub(crate) fn emit(&self, producer: &str, group: &'static str) {
        if self.enabled {
            eprintln!(
                "semon sessions native: producer={producer} breakdown={group} us_units={:?}",
                self.values
            );
        }
    }
}

pub(crate) struct Phase(Option<(String, &'static str, Instant)>);

impl Phase {
    pub(crate) fn new(cache: &Path, name: &'static str) -> Self {
        Self(enabled().then(|| {
            (
                id(&crate::events::EventCache::path(cache)),
                name,
                Instant::now(),
            )
        }))
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some((id, name, started)) = &self.0 {
            eprintln!(
                "semon sessions native: producer={id} phase={name} us={}",
                started.elapsed().as_micros()
            );
        }
    }
}
