//! Deterministic integration hooks. Enable only in test dependencies.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

fn paused_catalogs() -> &'static Mutex<BTreeSet<PathBuf>> {
    static PAUSED: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    PAUSED.get_or_init(Mutex::default)
}

/// A cache-scoped publication barrier, present only in test-hook builds.
pub struct CatalogPause(PathBuf);
impl Drop for CatalogPause {
    fn drop(&mut self) {
        paused_catalogs().lock().unwrap().remove(&self.0);
    }
}

/// Let synthetic fixtures advance Index independently of published Catalog.
pub fn pause_catalog(cache: &Path) -> CatalogPause {
    let path = crate::events::EventCache::path(cache);
    assert!(paused_catalogs().lock().unwrap().insert(path.clone()));
    CatalogPause(path)
}

pub(crate) fn catalog_paused(path: &Path) -> bool {
    paused_catalogs().lock().unwrap().contains(path)
}

type Hook = Arc<dyn Fn(&'static str) + Send + Sync>;
fn hooks() -> &'static Mutex<BTreeMap<PathBuf, Hook>> {
    static HOOKS: OnceLock<Mutex<BTreeMap<PathBuf, Hook>>> = OnceLock::new();
    HOOKS.get_or_init(Mutex::default)
}

pub struct Registration(PathBuf);
impl Drop for Registration {
    fn drop(&mut self) {
        hooks().lock().unwrap().remove(&self.0);
    }
}

/// Register one cache-scoped callback; unrelated workers are unaffected.
pub fn register(cache: &Path, hook: impl Fn(&'static str) + Send + Sync + 'static) -> Registration {
    assert!(
        hooks()
            .lock()
            .unwrap()
            .insert(cache.to_owned(), Arc::new(hook))
            .is_none()
    );
    Registration(cache.to_owned())
}

pub(crate) fn run(options: &crate::Options, point: &'static str) {
    run_cache(&options.cache, point);
}

pub(crate) fn run_cache(cache: &Path, point: &'static str) {
    let hook = hooks().lock().unwrap().get(cache).cloned();
    if let Some(hook) = hook {
        hook(point);
    }
}

/// Commit only the native Index/proofs, deliberately withholding Catalog
/// publication. Synthetic integration fixtures use this causal checkpoint.
pub fn advance_index_only(
    options: &crate::Options,
    path: &Path,
    harness: &str,
) -> std::io::Result<()> {
    let mut cache =
        crate::events::EventCache::open_scoped(&options.cache).map_err(std::io::Error::other)?;
    crate::events::scan_file(path, harness, &mut cache, &mut false)?;
    Ok(())
}
