//! Opt-in producer measurements. Identifiers are opaque diagnostics, never
//! source authority, and contain no cache paths or native content.
use sha2::{Digest, Sha256};
use std::{path::Path, time::Instant};

pub(crate) fn enabled() -> bool {
    std::env::var("SEMON_BUILD_TIMING").is_ok_and(|value| value == "1")
}

pub(crate) fn id(index: &Path) -> String {
    format!("{:x}", Sha256::digest(index.as_os_str().as_encoded_bytes()))
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
