//! Selected native rendering keeps metadata in one PostgreSQL snapshot. The
//! authorized host reader independently qualifies native bytes and their proofs.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure};
use crate::{CatalogReadScope, Options, SessionSourceReader, ViewerReply};

impl PgProjectionStore {
    /// Render one source-backed selected transcript window. The host authorizes
    /// the source reader independently; Catalog provenance grants no access.
    /// No SQLite store, native discovery or complete index decode is involved.
    #[allow(clippy::too_many_arguments)]
    pub async fn transcript_reply(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        key: &str,
        scope: CatalogReadScope,
        expected_generation: Option<&str>,
        after: usize,
        limit: usize,
        reader: &dyn SessionSourceReader,
    ) -> Result<ViewerReply, StorageError> {
        ensure(&self.namespace, namespace)?;
        if key.is_empty() || key.len() > 4096 || !(1..=100).contains(&limit) {
            return Err(backend("invalid selected transcript arguments"));
        }
        let tx = self.begin(namespace).await?;
        let query = format!(
            "sid={}&scope={}",
            crate::viewer::percent_encode(key),
            crate::catalog_search::scope_key(scope)
        );
        let request = crate::catalog::Request::parse(&query)
            .map_err(|_| backend("invalid selected identity"))?;
        let reply = crate::catalog::postgres_page(&tx, namespace, options, &request)
            .await
            .map_err(backend)?;
        let selected =
            crate::catalog::selection_from_reply(reply, namespace.as_str(), key, scope, false)
                .map_err(|_| backend("selected identity unavailable"))?;
        let Some((identity, session)) = selected else {
            return Ok(crate::catalog::error(
                404,
                "session_not_found",
                "The selected session is absent from this catalog.",
                false,
            ));
        };
        let row = tx
            .catalog_row(namespace, key, scope)
            .await?
            .ok_or_else(|| backend("selected identity unavailable"))?;
        let (generation, total, slots) = tx
            .recipe_page(namespace, key, scope, None, after, limit)
            .await?;
        if after > total {
            return Err(backend("selected range exceeds projection"));
        }
        if expected_generation.is_some_and(|expected| {
            expected != crate::transcript_range::entry_generation(&generation)
        }) {
            return Ok(crate::catalog::error(
                409,
                "stale_projection",
                "The selected projection changed. Resynchronize this session.",
                true,
            ));
        }
        let observations = tx.recipe_observations(namespace, key, scope, after, slots.len().max(1)).await?;
        let sources = observations.first().cloned().unwrap_or(row.sources);
        let reply = crate::transcript_range::render_postgres(
            options,
            identity,
            session,
            &generation,
            total,
            after,
            slots,
            sources,
            reader,
        );
        tx.commit(namespace).await?;
        Ok(reply)
    }
}
