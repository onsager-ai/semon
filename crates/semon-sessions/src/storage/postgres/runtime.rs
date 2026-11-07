//! Internal runtime qualification. Startup remains unexposed while the remaining
//! selected, discovery and custody consumers still depend on SQLite.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure};
use crate::{CatalogReadScope, Options, SessionSourceReader, ViewerReply};

impl PgProjectionStore {
    pub(crate) async fn resolve_native(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        harness: &str,
        native_id: &str,
        scope: CatalogReadScope,
    ) -> Result<crate::CatalogNativeResolution, StorageError> {
        ensure(&self.namespace, namespace)?;
        let tx = self.begin(namespace).await?;
        let mut result = crate::CatalogNativeResolution {
            state: crate::CatalogNativeResolutionState::Unavailable,
            source_key: namespace.to_string(),
            harness: harness.to_owned(),
            native_id: native_id.to_owned(),
            read_scope: scope,
            catalog_key: None,
            generation: tx.catalog_generation(namespace).await?,
        };
        if result.generation.is_none() {
            return Ok(result);
        }
        let mut keys = tx.tx.query(
            "SELECT session_key FROM semon_projection.catalog WHERE namespace=$1 AND lifecycle='current' AND harness=$2 AND EXISTS(SELECT 1 FROM jsonb_array_elements(metadata::jsonb->'sources') s WHERE s->>'native_id'=$3) ORDER BY session_key LIMIT 2",
            &[&namespace.binding(), &harness, &native_id],
        ).await.map_err(backend)?;
        if keys.is_empty() && scope == CatalogReadScope::RetainedHistory {
            keys = tx.tx.query(
                "SELECT session_key FROM (SELECT session_key,metadata,harness FROM semon_projection.catalog WHERE namespace=$1 UNION ALL SELECT session_key,metadata,harness FROM semon_projection.history_catalog WHERE namespace=$1) owners WHERE harness=$2 AND EXISTS(SELECT 1 FROM jsonb_array_elements(metadata::jsonb->'sources') s WHERE s->>'native_id'=$3) GROUP BY session_key ORDER BY session_key LIMIT 2",
                &[&namespace.binding(), &harness, &native_id],
            ).await.map_err(backend)?;
        }
        result.state = match keys.as_slice() {
            [] => crate::CatalogNativeResolutionState::Pending,
            [key] => {
                let key: &str = key.get(0);
                let row = tx
                    .catalog_row(namespace, key, scope)
                    .await?
                    .ok_or_else(|| backend("native alias owner unavailable"))?;
                if row.harness != harness
                    || !row.sources.iter().any(|source| {
                        source.native_id == native_id
                            && crate::catalog::source_relative(options, harness, source).is_some()
                    })
                {
                    return Err(backend("native alias authority mismatch"));
                }
                result.catalog_key = Some(key.to_owned());
                // Explicit cohort publication cannot prove complete machine ownership.
                crate::CatalogNativeResolutionState::Provisional
            }
            _ => crate::CatalogNativeResolutionState::Ambiguous,
        };
        tx.commit(namespace).await?;
        Ok(result)
    }

    pub(crate) async fn catalog_query_part(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        now: i64,
        bytes_left: &mut usize,
    ) -> Result<crate::query::catalog::Part, StorageError> {
        ensure(&self.namespace, namespace)?;
        let tx = self.begin(namespace).await?;
        let runtime = crate::native_evidence::query_runtime_generation(options).map_err(backend)?;
        let retained: bool = tx
            .tx
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM semon_projection.history_catalog WHERE namespace=$1)",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?
            .get(0);
        if retained {
            return Err(backend("Catalog Query composite History is unavailable"));
        }
        let observations = crate::native_evidence::RuntimeObservations::projection(options);
        let mut part = crate::query::catalog::Part {
            generation: tx
                .catalog_generation(namespace)
                .await?
                .ok_or_else(|| backend("Catalog not published"))?,
            runtime,
            rows: Default::default(),
            owners: Default::default(),
            content: Default::default(),
            witnesses: Default::default(),
            edges: Default::default(),
            turns: Default::default(),
        };
        let cutoff = (!options.all).then(|| {
            now.saturating_sub(i64::try_from(options.since.as_millis()).unwrap_or(i64::MAX))
        });
        let mut after = None;
        let mut all = Vec::new();
        loop {
            let page = tx
                .catalog_page(
                    namespace,
                    CatalogReadScope::Current,
                    after
                        .as_ref()
                        .map(|(last, key): &(i64, String)| (*last, key.as_str())),
                    100,
                )
                .await?;
            for row in page.rows {
                let bytes = serde_json::to_vec(&row).map_err(backend)?.len();
                *bytes_left = bytes_left
                    .checked_sub(bytes)
                    .ok_or_else(|| backend("Catalog Query metadata budget"))?;
                let context = tx.tx.query_opt("SELECT metadata,source_generation FROM semon_projection.query_context WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &row.key]).await.map_err(backend)?.ok_or_else(|| backend("Catalog Query context unavailable"))?;
                let text: &str = context.get(0);
                *bytes_left = bytes_left
                    .checked_sub(text.len())
                    .ok_or_else(|| backend("Catalog Query context budget"))?;
                if context.get::<_, String>(1)
                    != crate::retention::source_generation(&row.sources).map_err(backend)?
                {
                    return Err(backend("Catalog Query context changed"));
                }
                let context: serde_json::Value = serde_json::from_str(text).map_err(backend)?;
                all.push((row, context));
            }
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        let mut keep = std::collections::BTreeSet::new();
        let mut needed = std::collections::BTreeSet::new();
        for (row, context) in &all {
            let saved = row
                .query
                .as_ref()
                .ok_or_else(|| backend("Catalog Query facts unavailable"))?;
            if cutoff.is_none_or(|cutoff| {
                saved["last"].as_i64().is_some_and(|last| last >= cutoff)
                    || matches!(saved["state"].as_str(), Some("work" | "wait"))
            }) {
                keep.insert(row.key.clone());
            }
            for edge in context["edges"]
                .as_array()
                .ok_or_else(|| backend("invalid Query edges"))?
            {
                if cutoff.is_none_or(|cutoff| {
                    edge["at"].as_i64().is_some_and(|at| at >= cutoff)
                        || edge["done"].as_i64().is_some_and(|at| at >= cutoff)
                        || matches!(edge["status"].as_str(), Some("work" | "wait" | "new"))
                }) {
                    needed.insert(row.key.clone());
                    if let Some(id) = edge["id"].as_str() {
                        part.edges.insert(id.to_owned());
                    }
                    keep.extend(
                        [edge["from"].as_str(), edge["to"].as_str()]
                            .into_iter()
                            .flatten()
                            .filter(|id| *id != "you")
                            .map(str::to_owned),
                    );
                }
            }
        }
        let initial = keep.clone();
        for (row, context) in &all {
            if !initial.contains(&row.key) {
                continue;
            }
            for turn in context["turns"]
                .as_array()
                .ok_or_else(|| backend("invalid Query turns"))?
            {
                if cutoff.is_none_or(|cutoff| {
                    turn["last"] == true || turn["at"].as_i64().is_some_and(|at| at >= cutoff)
                }) {
                    if let Some(id) = turn["id"].as_str() {
                        part.turns.insert(id.to_owned());
                    }
                    let ids = turn["start"].as_str().into_iter().chain(
                        turn["sent"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(serde_json::Value::as_str),
                    );
                    for id in ids {
                        part.edges.insert(id.to_owned());
                        for (_, other) in &all {
                            for edge in other["edges"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter(|edge| edge["id"] == id)
                            {
                                keep.extend(
                                    [edge["from"].as_str(), edge["to"].as_str()]
                                        .into_iter()
                                        .flatten()
                                        .filter(|id| *id != "you")
                                        .map(str::to_owned),
                                );
                            }
                        }
                    }
                }
            }
        }
        needed.extend(keep.iter().cloned());
        for (row, _) in all {
            if !needed.contains(&row.key) {
                continue;
            }
            if row.sources.iter().any(|source| {
                crate::catalog::source_relative(options, &row.harness, source).is_none()
            }) {
                return Err(backend("Catalog Query source authority changed"));
            }
            let saved = row
                .query
                .as_ref()
                .ok_or_else(|| backend("Catalog Query facts unavailable"))?;
            if !crate::model::summary::query_observation_matches(
                saved,
                &row.sources,
                true,
                observations.expected(&row).as_deref(),
            )
            .map_err(backend)?
            {
                return Err(backend("Catalog Query runtime changed"));
            }
            part.witnesses.push((row.key.clone(), saved.to_string()));
            for (id, saved) in saved["rows"]
                .as_object()
                .ok_or_else(|| backend("invalid Query summaries"))?
                .iter()
                .filter(|(id, _)| keep.contains(*id))
            {
                if part.rows.get(id).is_some_and(|prior| prior != saved) {
                    return Err(backend("Catalog Query context disagrees"));
                }
                part.rows.insert(id.clone(), saved.clone());
                part.owners.insert(id.clone(), row.key.clone());
            }
        }
        if keep.iter().any(|id| !part.rows.contains_key(id)) {
            return Err(backend("Catalog Query relationship unavailable"));
        }
        tx.commit(namespace).await?;
        Ok(part)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn selected_query<T>(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        key: &str,
        provider: Option<&dyn SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
        now: i64,
        read: impl FnOnce(crate::model::QueryData) -> T,
    ) -> Result<T, StorageError> {
        ensure(&self.namespace, namespace)?;
        let _facts_lock =
            crate::source_change::SourceLock::try_shared(options, "facts").map_err(backend)?;
        let runtime = crate::native_evidence::query_runtime_generation(options).map_err(backend)?;
        let tx = self.begin(namespace).await?;
        let generation = tx.catalog_generation(namespace).await?;
        // Follow published ownership and native identity collisions. A missing
        // related owner remains an explicit refusal, never a workspace scan.
        let mut pending = std::collections::BTreeSet::from([key.to_owned()]);
        let mut rows = std::collections::BTreeMap::new();
        while let Some(key) = pending.pop_first() {
            if rows.contains_key(&key) {
                continue;
            }
            let row = tx
                .catalog_row(namespace, &key, CatalogReadScope::RetainedHistory)
                .await?
                .ok_or_else(|| backend("selected Query owner unavailable"))?;
            let mut names = row.native_ids.clone();
            names.push(row.key.clone());
            let context = tx.tx.query_opt("SELECT metadata,source_generation FROM semon_projection.query_context WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &row.key]).await.map_err(backend)?
                .ok_or_else(|| backend("selected Query relationship context unavailable"))?;
            if context.get::<_, String>(1)
                != crate::retention::source_generation(&row.sources).map_err(backend)?
            {
                return Err(backend("selected Query relationship generation changed"));
            }
            let context: serde_json::Value =
                serde_json::from_str(context.get(0)).map_err(backend)?;
            let endpoints = context["edges"]
                .as_array()
                .ok_or_else(|| backend("invalid Query context"))?
                .iter()
                .flat_map(|edge| [edge["from"].as_str(), edge["to"].as_str()])
                .flatten()
                .filter(|key| *key != "you")
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let related = tx.tx.query(
                "SELECT session_key FROM semon_projection.catalog WHERE namespace=$1 AND (session_key=$2 OR session_key=ANY($4::text[]) OR metadata::jsonb->>'parent'=$5 OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(metadata::jsonb->'native_ids') n WHERE n=ANY($3::text[]))) ORDER BY session_key LIMIT 65",
                &[&namespace.binding(), &row.parent.as_deref().unwrap_or(""), &names, &endpoints, &row.key],
            ).await.map_err(backend)?;
            if related.len() > 64 {
                return Err(backend("selected Query relationship bound"));
            }
            pending.extend(related.iter().map(|row| row.get::<_, String>(0)));
            if let Some(parent) = &row.parent {
                pending.insert(parent.clone());
            }
            rows.insert(key, row);
            if rows.len() > 64 {
                return Err(backend("selected Query relationship bound"));
            }
        }
        let rows: Vec<_> = rows.into_values().collect();
        let mut unique = std::collections::BTreeMap::new();
        for row in &rows {
            for source in &row.sources {
                if crate::catalog::source_relative(options, &row.harness, source).is_none() {
                    return Err(backend("selected Query source outside native authority"));
                }
                if let Some(prior) = unique.insert(source.path.clone(), source.clone())
                    && prior != *source
                {
                    return Err(backend("selected Query source generations disagree"));
                }
            }
        }
        if unique.len() > 64 {
            return Err(backend("selected Query source bound"));
        }
        let _locks = unique
            .keys()
            .map(|path| {
                crate::source_change::SourceLock::try_shared(options, &path.to_string_lossy())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        let mut files = Vec::new();
        for (path, source) in unique {
            if !crate::model::catalog_source_in_window(
                &source,
                crate::model::scan_cutoff(options, now),
            ) {
                continue;
            }
            let input = crate::native_evidence::catalog_input(&path, options)
                .ok_or_else(|| backend("invalid native Query source"))?;
            if tx.raw_write_pending(namespace, &input).await? {
                return Err(backend("Raw write is pending"));
            }
            let name = path
                .to_str()
                .ok_or_else(|| backend("invalid native Query path"))?;
            let (ledger, index) = tx
                .load_index(namespace, name)
                .await?
                .ok_or_else(|| backend("selected Query index unavailable"))?;
            if ledger != crate::source_carrier::ledger(&source) {
                return Err(backend("selected Query index generation changed"));
            }
            crate::selected_read::charge(
                index
                    .events
                    .len()
                    .saturating_add(index.extras.len())
                    .saturating_mul(std::mem::size_of::<crate::events::Event>()),
                0,
            )
            .map_err(backend)?;
            files.push((name.to_owned(), ledger, index));
        }
        let mut fields = std::collections::BTreeMap::new();
        for row in &rows {
            let (_, total, _) = tx
                .recipe_page(
                    namespace,
                    &row.key,
                    CatalogReadScope::RetainedHistory,
                    None,
                    0,
                    1,
                )
                .await?;
            if total > 4096 {
                return Err(backend("selected Query recipe bound"));
            }
            for after in (0..total).step_by(100) {
                let (_, _, slots) = tx
                    .recipe_page(
                        namespace,
                        &row.key,
                        CatalogReadScope::RetainedHistory,
                        None,
                        after,
                        100,
                    )
                    .await?;
                for slot in slots {
                    if let (Some(file), Some(field)) = (slot.file, slot.field) {
                        fields.insert((file.path, slot.offset, slot.block), field);
                    }
                }
            }
        }
        let data = crate::overview_repair::projected_query(
            options, &rows, files, fields, provider, mode, now,
        )
        .map_err(backend)?;
        let witness = data.sources.reader_with_mode(provider, mode);
        witness.verify_all().map_err(backend)?;
        drop(witness);
        let result = read(data);
        crate::selected_read::check().map_err(backend)?;
        tx.commit(namespace).await?;
        let check = self.begin(namespace).await?;
        if check.catalog_generation(namespace).await? != generation
            || crate::native_evidence::query_runtime_generation(options).map_err(backend)?
                != runtime
        {
            return Err(backend("selected Query publication changed"));
        }
        check.commit(namespace).await?;
        Ok(result)
    }

    pub(crate) async fn native_query(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        now: i64,
    ) -> Result<crate::model::QueryData, StorageError> {
        ensure(&self.namespace, namespace)?;
        let _facts_lock =
            crate::source_change::SourceLock::try_shared(options, "facts").map_err(backend)?;
        let inputs = crate::inputs(options).map_err(backend)?;
        let _source_locks = inputs
            .iter()
            .filter(|input| input.path.ends_with(".jsonl"))
            .map(|input| {
                crate::source_change::SourceLock::try_shared(
                    options,
                    &crate::source_change::lock_key(options, input),
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
        if let Some((_, facts)) = tx.reported_facts(namespace).await? {
            cache.set_reported_runs(facts.reported_runs);
        }
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
        let reply = if let Some(reader) = provider {
            crate::transcript_range::render_postgres(options, identity, session, &generation, total, first, slots, row.sources, reader)
        } else {
            let mut proofs = std::collections::BTreeMap::new();
            for source in &row.sources {
                let input = crate::native_evidence::catalog_input(&source.path, options).ok_or_else(|| backend("invalid selected source"))?;
                let proof_generation = tx.source_proof_generation(namespace, source).await?;
                proofs.insert((input.root.as_str().into(), input.path), (source.clone(), proof_generation));
            }
            let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
            let reader = super::proof_reader::LocalProofReader::new(options, &row.sources, sender)?;
            let options = options.clone();
            let mut rendering = tokio::task::spawn_blocking(move || {
                let reply = crate::transcript_range::render_postgres(&options, identity, session, &generation, total, first, slots, row.sources, &reader);
                reader.verify().map(|()| reply)
            });
            loop {
                tokio::select! {
                    reply = &mut rendering => break reply.map_err(backend)?.map_err(backend)?,
                    Some(request) = requests.recv() => {
                        super::proof_reader::serve(&tx, namespace, request, &proofs).await;
                    }
                }
            }
        };
        tx.commit(namespace).await?;
        Ok(reply)
    }
}
