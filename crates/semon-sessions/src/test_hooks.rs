//! Deterministic integration hooks. Enable only in test dependencies.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

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
    let hook = hooks().lock().unwrap().get(&options.cache).cloned();
    if let Some(hook) = hook {
        hook(point);
    }
}
