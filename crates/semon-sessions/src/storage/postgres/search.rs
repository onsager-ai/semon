//! Metadata-only substring postings use the shared Unicode normalization and
//! exact candidate verification. Native bodies never enter the search index.
use super::{Namespace, PgProjectionTransaction, StorageError, backend, ensure, number};
use crate::{CatalogReadScope, catalog_search, model::summary::CatalogRow};

pub(crate) struct SearchPage {
    pub(crate) rows: Vec<CatalogRow>,
    pub(crate) next: Option<(i64, String)>,
    pub(crate) partial: bool,
    pub(crate) candidates: usize,
}

impl PgProjectionTransaction<'_> {
    pub(super) async fn publish_search(
        &self,
        namespace: &Namespace,
        row: &CatalogRow,
    ) -> Result<(), StorageError> {
        for scope in [CatalogReadScope::Current, CatalogReadScope::RetainedHistory] {
            let selected = self
                .catalog_row(namespace, &row.key, scope)
                .await?
                .ok_or_else(|| backend("search publication identity unavailable"))?;
            let fields = catalog_search::fields(&selected);
            let encoded = serde_json::to_string(&fields).map_err(backend)?;
            let grams: Vec<_> = catalog_search::grams(&fields).into_iter().collect();
            if encoded.len() > 8192 || grams.len() > 1024 {
                return Err(backend("metadata search document budget exceeded"));
            }
            let scope = catalog_search::scope_key(scope);
            self.tx.execute("DELETE FROM semon_projection.search_postings WHERE namespace=$1 AND session_key=$2 AND read_scope=$3", &[&namespace.binding(), &row.key, &scope]).await.map_err(backend)?;
            self.tx.execute("INSERT INTO semon_projection.search_postings(namespace,session_key,read_scope,gram,last_ms) SELECT $1,$2,$3,g,$4 FROM unnest($5::text[]) g", &[&namespace.binding(), &row.key, &scope, &row.last.unwrap_or(0), &grams]).await.map_err(backend)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn filtered_catalog_page(
        &self,
        namespace: &Namespace,
        scope: CatalogReadScope,
        harness: Option<&str>,
        repo: Option<&str>,
        model: Option<&str>,
        query: Option<&str>,
        after: Option<(i64, &str)>,
        limit: usize,
    ) -> Result<SearchPage, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !(1..=100).contains(&limit) {
            return Err(backend("invalid catalog limit"));
        }
        let query = query.map(|query| catalog_search::normalize_query(query)
            .filter(|query| !query.is_empty()).ok_or_else(|| backend("invalid metadata query"))).transpose()?;
        let anchor: Option<String> = query.as_ref().map(|query| query.chars().take(3).collect());
        let (last, key) = after.unwrap_or((i64::MAX, ""));
        let selected = self.tx.query(
            "SELECT c.session_key,c.last_ms FROM semon_projection.catalog c WHERE c.namespace=$1 AND ($2 OR c.lifecycle='current') AND (c.last_ms<$3 OR (c.last_ms=$3 AND c.session_key>$4)) AND ($5::text IS NULL OR c.harness=$5) AND ($6::text IS NULL OR c.repo=$6) AND ($7::text IS NULL OR c.model=$7) AND ($8::text IS NULL OR EXISTS(SELECT 1 FROM semon_projection.search_postings p WHERE p.namespace=c.namespace AND p.session_key=c.session_key AND p.read_scope=$9 AND p.gram=$8)) ORDER BY c.last_ms DESC,c.session_key LIMIT $10",
            &[&namespace.binding(), &(scope == CatalogReadScope::RetainedHistory), &last, &key, &harness, &repo, &model, &anchor, &catalog_search::scope_key(scope), &number(catalog_search::MAX_CANDIDATES as u64 + 1)?],
        ).await.map_err(backend)?;
        let mut rows = Vec::new();
        let mut checked = None;
        let mut bytes = 0;
        let mut candidates = 0;
        for item in selected.iter().take(catalog_search::MAX_CANDIDATES) {
            let key: String = item.get(0);
            let row = self.catalog_row(namespace, &key, scope).await?
                .ok_or_else(|| backend("catalog candidate unavailable"))?;
            bytes += serde_json::to_vec(&row).map_err(backend)?.len();
            if bytes > catalog_search::MAX_SCAN_BYTES {
                return Ok(SearchPage { rows, next: checked, partial: true, candidates });
            }
            let matches = query.as_ref().is_none_or(|query| catalog_search::fields(&row).iter().any(|field| field.contains(query)));
            if matches && rows.len() == limit {
                return Ok(SearchPage { rows, next: checked, partial: false, candidates });
            }
            candidates += 1;
            checked = Some((item.get(1), key));
            if matches { rows.push(row); }
        }
        let partial = selected.len() > catalog_search::MAX_CANDIDATES;
        Ok(SearchPage { rows, next: if partial { checked } else { None }, partial, candidates })
    }

    pub(crate) async fn search_catalog(
        &self,
        namespace: &Namespace,
        scope: CatalogReadScope,
        query: &str,
        after: Option<(i64, &str)>,
        limit: usize,
    ) -> Result<SearchPage, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !(1..=100).contains(&limit) {
            return Err(backend("invalid search limit"));
        }
        let query = catalog_search::normalize_query(query)
            .filter(|query| !query.is_empty())
            .ok_or_else(|| backend("invalid metadata query"))?;
        let anchor: String = query.chars().take(3).collect();
        let (last, key) = after.unwrap_or((i64::MAX, ""));
        let postings = self.tx.query(
            "SELECT p.session_key,p.last_ms FROM semon_projection.search_postings p JOIN semon_projection.catalog c USING(namespace,session_key) WHERE p.namespace=$1 AND p.gram=$2 AND p.read_scope=$7 AND ($3 OR c.lifecycle='current') AND (p.last_ms<$4 OR (p.last_ms=$4 AND p.session_key>$5)) ORDER BY p.last_ms DESC,p.session_key LIMIT $6",
            &[&namespace.binding(), &anchor, &(scope == CatalogReadScope::RetainedHistory), &last, &key, &number(catalog_search::MAX_CANDIDATES as u64 + 1)?, &catalog_search::scope_key(scope)],
        ).await.map_err(backend)?;
        let mut rows = Vec::new();
        let mut checked = None;
        let mut bytes = 0;
        let mut candidates = 0;
        for posting in postings.iter().take(catalog_search::MAX_CANDIDATES) {
            let key: String = posting.get(0);
            let last: i64 = posting.get(1);
            let row = self
                .catalog_row(namespace, &key, scope)
                .await?
                .ok_or_else(|| backend("search candidate unavailable"))?;
            bytes += serde_json::to_vec(&row).map_err(backend)?.len();
            if bytes > catalog_search::MAX_SCAN_BYTES {
                return Ok(SearchPage {
                    rows,
                    next: checked,
                    partial: true,
                    candidates,
                });
            }
            let matches = catalog_search::fields(&row)
                .iter()
                .any(|field| field.contains(&query));
            if matches && rows.len() == limit {
                return Ok(SearchPage {
                    rows,
                    next: checked,
                    partial: false,
                    candidates,
                });
            }
            candidates += 1;
            checked = Some((last, key));
            if matches {
                rows.push(row);
            }
        }
        let partial = postings.len() > catalog_search::MAX_CANDIDATES;
        Ok(SearchPage {
            rows,
            next: if partial { checked } else { None },
            partial,
            candidates,
        })
    }
}
