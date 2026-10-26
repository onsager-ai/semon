//! Ordinary Catalog renderer adapter over one acknowledged immutable generation.
use super::*;
use crate::{catalog::Request, storage::catalog::CatalogSnapshot};
use std::io;

fn failed(error: StorageError) -> io::Error {
    io::Error::other(error)
}
fn unavailable<T>() -> io::Result<T> {
    Err(failed(StorageError::NamespaceUpgradePending))
}

impl CatalogSnapshot for Snapshot<'_, '_> {
    fn runtime_binding(&self) -> Option<crate::storage::QueryRuntimeBinding> {
        super::super::super::model_cache::binding(&mut self.tx.borrow_mut()).ok()
    }
    fn require_current(&self) -> io::Result<()> {
        let tx = self.tx.borrow_mut();
        tx.check().map_err(failed)?;
        let admitted: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2)", &[&tx.namespace.as_str().as_bytes(), &self.generation.as_bytes()]).map_err(sqlstate::error).map_err(failed)?.get(0);
        if admitted { Ok(()) } else { unavailable() }
    }
    fn publication_value(&self, key: &str) -> io::Result<Option<String>> {
        self.require_current()?;
        Ok(match key {
            "catalog_generation" => Some(self.generation.clone()),
            "catalog_version" => Some(crate::model::summary::CATALOG_VERSION.to_string()),
            // Completeness is not inferred from successful metadata staging.
            "catalog_completeness" => Some("partial".into()),
            _ => None,
        })
    }
    fn rows(&self, request: &Request) -> io::Result<Vec<CatalogRow>> {
        self.require_current()?;
        if request.limit > 64 || request.scope != crate::CatalogReadScope::Current {
            return unavailable();
        }
        let tx = self.tx.borrow_mut();
        let ns = tx.namespace.as_str().as_bytes();
        let bytes: i64 = tx.driver.query_one("SELECT coalesce(sum(octet_length(metadata)),0)::bigint FROM (SELECT metadata FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 65) bounded", &[&ns, &self.generation.as_bytes()]).map_err(sqlstate::error).map_err(failed)?.get(0);
        if bytes > MAX as i64 {
            return Err(failed(StorageError::Busy));
        }
        let rows = tx.driver.query("SELECT session_key,metadata FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 65", &[&ns, &self.generation.as_bytes()]).map_err(sqlstate::error).map_err(failed)?;
        if rows.len() > 64 {
            return Err(failed(StorageError::Busy));
        }
        let mut output = Vec::new();
        for row in rows {
            let key: Vec<u8> = row.get(0);
            let bytes: Vec<u8> = row.get(1);
            crate::selected_read::charge(64, bytes.len())?;
            let value: CatalogRow = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if value.key.as_bytes() != key {
                return Err(failed(StorageError::ScopeChanged));
            }
            if request.sid.as_ref().is_some_and(|key| key != &value.key)
                || request
                    .harness
                    .as_ref()
                    .is_some_and(|h| h != &value.harness)
                || request.model.as_ref().is_some_and(|m| m != &value.model)
                || request
                    .repo
                    .as_ref()
                    .is_some_and(|repo| value.repo.as_deref().unwrap_or("") != repo)
                || request.cursor.as_ref().is_some_and(|cursor| {
                    value.last.unwrap_or(0) > cursor.last
                        || (value.last.unwrap_or(0) == cursor.last
                            && (value.key < cursor.key
                                || (!cursor.inclusive && value.key == cursor.key)))
                })
            {
                continue;
            }
            output.push(value);
        }
        output.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.key.cmp(&b.key)));
        output.truncate(request.limit.saturating_add(1));
        tx.check().map_err(failed)?;
        Ok(output)
    }
    fn search(
        &self,
        _: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection> {
        unavailable()
    }
    fn has_history(&self, key: &str) -> io::Result<bool> {
        self.catalog(key, true)
            .map(|row| row.is_some())
            .map_err(failed)
    }
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>> {
        self.catalog(key, true)
            .map_err(failed)?
            .map(|row| serde_json::to_string(&row).map_err(io::Error::other))
            .transpose()
    }
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool> {
        self.has_history(&row.key)
    }
    fn selected_details(&self, key: &str, sources: &[CatalogSource], status: &mut dyn FnMut(&CatalogRow) -> serde_json::Value) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        crate::storage::selected::selected_details(self, key, sources, &mut |row| status(row))
    }
    fn transcript_observation(&self, key: &str, generation: &str, history: bool) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        if generation != self.generation { return Ok(None); }
        let Some((_, header, _)) = self.selected(key, history, 0, 1).map_err(failed)? else { return Ok(None); };
        if !(0..=9_007_199_254_740_991).contains(&header.3) { return Err(failed(StorageError::ScopeChanged)); }
        Ok(Some(serde_json::json!({"generation":crate::transcript_range::entry_generation(&header.0),"total":header.3})))
    }
}
