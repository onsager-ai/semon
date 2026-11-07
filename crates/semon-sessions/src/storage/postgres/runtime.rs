//! Internal runtime qualification. Startup remains unexposed while the remaining
//! selected, discovery and custody consumers still depend on SQLite.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure};
use crate::{CatalogReadScope, Options, SessionSourceReader, ViewerReply};

impl PgProjectionStore {
    pub(crate) async fn native_query(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        now: i64,
    ) -> Result<crate::model::QueryData, StorageError> {
        ensure(&self.namespace, namespace)?;
        let inputs = crate::inputs(options).map_err(backend)?;
        let _source_locks = inputs
            .iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .map(|input| {
                crate::source_change::SourceLock::acquire(
                    options,
                    &crate::source_change::lock_key(options, input),
                    false,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        for input in &inputs {
            if !input.path.ends_with(".jsonl") {
                continue;
            }
            self.index_source(namespace, &input.full_path(options), input.root.as_str())
                .await?;
        }
        let tx = self.begin(namespace).await?;
        let mut files = Vec::new();
        for input in inputs.iter().filter(|input| input.path.ends_with(".jsonl")) {
            if tx.raw_write_pending(namespace, input).await? {
                return Err(backend("Raw write is pending"));
            }
            let path = input.full_path(options);
            let name = path
                .to_str()
                .ok_or_else(|| backend("invalid native path"))?;
            let (ledger, index) = tx
                .load_index(namespace, name)
                .await?
                .ok_or_else(|| backend("native query index unavailable"))?;
            files.push((name.to_owned(), ledger, index));
        }
        let mut cache = crate::events::EventCache::from_projection(files, Vec::new());
        let data = crate::model::build_query(
            options,
            &mut cache,
            &mut false,
            &mut crate::model::Texts::default(),
            now,
        )
        .map_err(backend)?;
        tx.commit(namespace).await?;
        Ok(data)
    }

    /// The shared query contract for the implemented transcript window. Other
    /// selected routes explicitly refuse until their PostgreSQL ports exist.
    pub(crate) async fn transcript_query(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        query: &str,
        provider: Option<&dyn SessionSourceReader>,
    ) -> Result<ViewerReply, StorageError> {
        ensure(&self.namespace, namespace)?;
        let invalid = || {
            crate::catalog::error(
                400,
                "invalid_arguments",
                "Supply a selected session and a valid transcript window.",
                false,
            )
        };
        for name in [
            "sid",
            "scope",
            "after",
            "limit",
            "generation",
            "errors_generation",
            "part",
            "field_chunk",
        ] {
            if query
                .split('&')
                .filter(|part| part.split('=').next() == Some(name))
                .count()
                > 1
                || crate::viewer::query_value(query, name)
                    .is_some_and(|value| crate::viewer::decoded(value).is_none())
            {
                return Ok(invalid());
            }
        }
        let argument =
            |name| crate::viewer::query_value(query, name).and_then(crate::viewer::decoded);
        if ["part", "field_chunk", "errors_generation"]
            .into_iter()
            .any(|name| argument(name).is_some())
        {
            return Ok(crate::catalog::error(
                503,
                "capability_unavailable",
                "This selected operation is not yet available on PostgreSQL.",
                false,
            ));
        }
        let Some(key) = argument("sid").filter(|key| !key.is_empty() && key.len() <= 4096) else {
            return Ok(invalid());
        };
        let scope = match argument("scope").as_deref() {
            None | Some("current") => CatalogReadScope::Current,
            Some("retained_history") => CatalogReadScope::RetainedHistory,
            _ => return Ok(invalid()),
        };
        let (Ok(after), Ok(limit)) = (
            argument("after")
                .map(|value| value.parse::<usize>())
                .transpose(),
            argument("limit")
                .map(|value| value.parse::<usize>())
                .transpose(),
        ) else {
            return Ok(invalid());
        };
        let limit = limit.unwrap_or(60);
        if !(1..=100).contains(&limit) {
            return Ok(invalid());
        }
        let expected = argument("generation");
        let tx = self.begin(namespace).await?;
        let request = crate::catalog::Request::parse(&format!(
            "sid={}&scope={}",
            crate::viewer::percent_encode(&key),
            crate::catalog_search::scope_key(scope)
        ))
        .map_err(|_| backend("invalid selection"))?;
        let reply = crate::catalog::postgres_page(&tx, namespace, options, &request)
            .await
            .map_err(backend)?;
        let Some((identity, session)) =
            crate::catalog::selection_from_reply(reply, namespace.as_str(), &key, scope, false)
                .map_err(|_| backend("selected identity unavailable"))?
        else {
            return Ok(crate::catalog::error(
                404,
                "session_not_found",
                "The selected session is absent from this catalog.",
                false,
            ));
        };
        let row = tx
            .catalog_row(namespace, &key, scope)
            .await?
            .ok_or_else(|| backend("selected row unavailable"))?;
        let (generation, total, _) = tx.recipe_page(namespace, &key, scope, None, 0, 1).await?;
        if expected.as_ref().is_some_and(|expected| {
            *expected != crate::transcript_range::entry_generation(&generation)
        }) {
            return Ok(crate::catalog::error(
                409,
                "stale_projection",
                "The selected projection changed. Resynchronize this session.",
                true,
            ));
        }
        let first = after.unwrap_or_else(|| total.saturating_sub(limit));
        if first > total {
            return Ok(crate::catalog::error(
                400,
                "invalid_range",
                "The slot address exceeds this projection.",
                false,
            ));
        }
        let (_, _, slots) = tx
            .recipe_page(namespace, &key, scope, Some(&generation), first, limit)
            .await?;
        let sources = if provider.is_none() {
            let mut proofs = Vec::new();
            for source in &row.sources {
                let name = source
                    .path
                    .to_str()
                    .ok_or_else(|| backend("invalid native path"))?;
                let (ledger, index) = tx
                    .load_index(namespace, name)
                    .await?
                    .ok_or_else(|| backend("selected native proof unavailable"))?;
                // Old selected prefixes need their own retained proof generation.
                // Refuse here until that lazy proof reader is ported.
                if ledger.offset != source.offset
                    || ledger.prefix != source.prefix_sha256
                    || ledger.tail != source.tail_sha256
                {
                    return Err(backend("selected native proof generation changed"));
                }
                proofs.push((source.clone(), index.source_chunks));
            }
            Some(
                crate::query_source::Sources::capture(options, proofs.into_iter())
                    .map_err(backend)?,
            )
        } else {
            None
        };
        let records = sources.as_ref().map(crate::query_source::Sources::reader);
        let reader = provider
            .or_else(|| {
                records
                    .as_ref()
                    .map(|reader| reader as &dyn SessionSourceReader)
            })
            .ok_or_else(|| backend("selected source reader unavailable"))?;
        let reply = crate::transcript_range::render_postgres(
            options,
            identity,
            session,
            &generation,
            total,
            first,
            slots,
            row.sources,
            reader,
        );
        tx.commit(namespace).await?;
        Ok(reply)
    }
}
