//! Host collection over the runtime PostgreSQL index, without a SQLite cache.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure};
use crate::{Options, events::EventCache};

impl PgProjectionStore {
    pub(crate) async fn collection_index(
        &mut self,
        namespace: &Namespace,
        options: &Options,
    ) -> Result<EventCache, StorageError> {
        ensure(&self.namespace, namespace)?;
        let inputs = crate::inputs(options).map_err(backend)?;
        for input in inputs.iter().filter(|input| input.path.ends_with(".jsonl")) {
            let tx = self.begin(namespace).await?;
            if tx.raw_write_pending(namespace, input).await? {
                return Err(backend("custody write is pending"));
            }
            tx.commit(namespace).await?;
            self.index_source(namespace, &input.full_path(options), input.root.as_str())
                .await?;
        }
        let tx = self.begin(namespace).await?;
        if tx.facts_write_pending(namespace).await? {
            return Err(backend("Facts write is pending"));
        }
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
                .ok_or_else(|| backend("collection index unavailable"))?;
            files.push((name.to_owned(), ledger, index));
        }
        let mut cache = EventCache::from_projection(files, Vec::new());
        if let Some((_, facts)) = tx.reported_facts(namespace).await? {
            cache.set_reported_runs(facts.reported_runs);
        }
        tx.commit(namespace).await?;
        Ok(cache)
    }

    pub(crate) async fn local_facts(
        &mut self,
        namespace: &Namespace,
        options: &Options,
    ) -> Result<crate::Facts, StorageError> {
        if options.facts.is_some() {
            return Err(backend("local Facts collection requires a local machine"));
        }
        let _locks = collection_locks(options)?;
        let tx = self.begin(namespace).await?;
        let expected = tx
            .reported_facts(namespace)
            .await?
            .map(|(revision, _)| revision);
        tx.commit(namespace).await?;
        let mut cache = self.collection_index(namespace, options).await?;
        let mut dirty = false;
        cache.refresh_reported_runs(&options.claude_json, crate::model::now_ms(), &mut dirty);
        let facts =
            crate::facts::collect_facts(options, &mut cache, &mut dirty).map_err(backend)?;
        let tx = self.begin(namespace).await?;
        let revision = expected
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| backend("Facts revision exhausted"))?;
        if !tx
            .write_facts(namespace, expected, revision, &facts)
            .await?
        {
            return Err(backend("Facts collection revision changed"));
        }
        tx.commit(namespace).await?;
        Ok(facts)
    }

    pub(crate) async fn collect(
        &mut self,
        namespace: &Namespace,
        options: &Options,
    ) -> Result<Vec<crate::Node>, StorageError> {
        let _locks = collection_locks(options)?;
        let mut cache = self.collection_index(namespace, options).await?;
        crate::collect_with_index(options, &mut cache).map_err(backend)
    }
}

fn collection_locks(
    options: &Options,
) -> Result<Vec<crate::source_change::SourceLock>, StorageError> {
    let mut locks =
        vec![crate::source_change::SourceLock::try_shared(options, "facts").map_err(backend)?];
    for input in crate::inputs(options)
        .map_err(backend)?
        .iter()
        .filter(|input| input.path.ends_with(".jsonl"))
    {
        locks.push(
            crate::source_change::SourceLock::try_shared(
                options,
                &crate::source_change::lock_key(options, input),
            )
            .map_err(backend)?,
        );
    }
    Ok(locks)
}
