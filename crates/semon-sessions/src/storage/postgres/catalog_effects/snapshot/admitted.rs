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
        let mut tx = self.tx.lock().ok()?;
        super::super::super::model_cache::binding(&mut **tx).ok()
    }
    fn require_current(&self) -> io::Result<()> {
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
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
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        let ns = tx.namespace.as_str().as_bytes();
        // Filter and keyset BEFORE LIMIT. Metadata stays opaque bytes: native
        // keys can contain NUL, which PostgreSQL JSON/text cannot represent.
        let sid = request.sid.as_ref().map(|v| v.as_bytes());
        let harness = request.harness.as_ref().map(|v| v.as_bytes());
        let repo = request.repo.as_ref().map(|v| v.as_bytes());
        let model = request.model.as_ref().map(|v| v.as_bytes());
        let cursor_last = request.cursor.as_ref().map(|v| v.last);
        let cursor_key = request.cursor.as_ref().map(|v| v.key.as_bytes());
        let inclusive = request.cursor.as_ref().is_some_and(|v| v.inclusive);
        let limit = request.limit as i64 + 1;
        let rows = tx.driver.query(
            "WITH page AS MATERIALIZED (
                SELECT session_key,metadata,last_ms,harness,repo,model
                FROM semon_pg_native_catalog_rows
                WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2
                  AND ($3::bytea IS NULL OR session_key=$3)
                  AND ($4::bytea IS NULL OR harness=$4)
                  AND ($5::bytea IS NULL OR repo=$5)
                  AND ($6::bytea IS NULL OR model=$6)
                  AND ($7::bigint IS NULL OR last_ms<$7
                       OR (last_ms=$7 AND (session_key>$8 OR ($9 AND session_key=$8))))
                ORDER BY last_ms DESC,session_key LIMIT $10
             ) SELECT session_key,
                 CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata END,
                 last_ms,harness,repo,model FROM page ORDER BY last_ms DESC,session_key",
            &[&ns, &self.generation.as_bytes(), &sid, &harness, &repo, &model,
              &cursor_last, &cursor_key, &inclusive, &limit],
        ).map_err(sqlstate::error).map_err(failed)?;
        let mut output = Vec::new();
        for row in rows {
            let key: Vec<u8> = row.get(0);
            let bytes: Vec<u8> = row.get::<_, Option<Vec<u8>>>(1)
                .ok_or_else(|| failed(StorageError::Busy))?;
            crate::selected_read::charge(64, bytes.len())?;
            let value: CatalogRow = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if value.key.as_bytes() != key || value.sources.len() > 64
                || value.last.unwrap_or(0) != row.get::<_, i64>(2)
                || value.harness.as_bytes() != row.get::<_, Vec<u8>>(3)
                || value.repo.as_deref().unwrap_or("").as_bytes() != row.get::<_, Vec<u8>>(4)
                || value.model.as_bytes() != row.get::<_, Vec<u8>>(5)
            {
                return Err(failed(StorageError::ScopeChanged));
            }
            output.push(value);
        }
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
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow) -> serde_json::Value,
    ) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        crate::storage::selected::selected_details(self, key, sources, &mut |row| status(row))
    }
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        if generation != self.generation {
            return Ok(None);
        }
        let Some((_, header, _)) = self.selected(key, history, 0, 1).map_err(failed)? else {
            return Ok(None);
        };
        if !(0..=9_007_199_254_740_991).contains(&header.3) {
            return Err(failed(StorageError::ScopeChanged));
        }
        Ok(Some(
            serde_json::json!({"generation":crate::transcript_range::entry_generation(&header.0),"total":header.3}),
        ))
    }
}
