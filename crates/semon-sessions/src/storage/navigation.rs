//! Backend-neutral, bounded native navigation observations. No Raw authority.
use super::StorageError;
use crate::CatalogReadScope;

#[derive(Default)]
pub(crate) struct NativeResolutionObservation {
    pub version: Option<String>,
    pub generation: Option<String>,
    pub completeness: Option<String>,
    pub keys: Vec<String>,
    pub metadata: Option<String>,
}
impl NativeResolutionObservation {
    pub(crate) fn qualified_head(&self) -> Option<(&str, bool)> {
        if self.version.as_deref()? != crate::model::summary::CATALOG_VERSION.to_string() {
            return None;
        }
        let generation = self.generation.as_deref()?;
        if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let complete = match self.completeness.as_deref() {
            None | Some("complete") => true,
            Some("partial") => false,
            _ => return None,
        };
        Some((generation, complete))
    }
}

pub(crate) trait NativeNavigationStore {
    fn bookmarked_target_keys(&self, target: &str) -> Result<Vec<String>, StorageError>;
    fn native_resolution(
        &self,
        harness: &str,
        native_id: &str,
        scope: CatalogReadScope,
        expected_generation: Option<&str>,
    ) -> Result<NativeResolutionObservation, StorageError>;
}
