//! Semantic native Catalog snapshot boundary. No driver or SQL escapes here.
use crate::{catalog::Request, model::summary::{CatalogRow, CatalogSource}};
use serde_json::Value;
use std::io;

pub(crate) trait CatalogSnapshot {
    fn require_current(&self) -> io::Result<()>;
    fn publication_value(&self, key: &str) -> io::Result<Option<String>>;
    fn rows(&self, request: &Request) -> io::Result<Vec<CatalogRow>>;
    fn search(&self, query: crate::catalog_search::Query<'_>) -> io::Result<crate::catalog_search::Selection>;
    fn has_history(&self, key: &str) -> io::Result<bool>;
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>>;
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool>;
    fn selected_details(&self, key: &str, sources: &[CatalogSource], status: &mut dyn FnMut(&CatalogRow) -> Value) -> io::Result<Option<Value>>;
    fn transcript_observation(&self, key: &str, generation: &str, history: bool) -> io::Result<Option<Value>>;
}
