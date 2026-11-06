//! Bounded native derivation over PostgreSQL indexes, without a SQLite store.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure};
use crate::{Input, Options, events::EventCache, slot_projection::Publication};
use std::collections::BTreeSet;

impl PgProjectionStore {
    /// Index and publish one bounded native source cohort using the shared
    /// native producer. Hosts retain source custody across this operation and
    /// call it outside their request executor because native parsing blocks.
    /// A changed Catalog or source refuses publication; the caller rescans Raw.
    /// This does not discover unrelated sources or open a SQLite session store.
    pub async fn publish_sources(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        inputs: &[Input],
    ) -> Result<Option<String>, StorageError> {
        ensure(&self.namespace, namespace)?;
        if inputs.is_empty()
            || inputs.len() > 64
            || inputs.iter().any(|input| {
                !crate::is_input_path(input.root.as_str(), &input.path)
                    || !input.path.ends_with(".jsonl")
            })
            || inputs.iter().collect::<BTreeSet<_>>().len() != inputs.len()
        {
            return Err(backend("invalid native producer cohort"));
        }
        let tx = self.begin(namespace).await?;
        for input in inputs {
            if tx.raw_write_pending(namespace, input).await? {
                return Err(backend("Raw write is pending"));
            }
        }
        tx.commit(namespace).await?;
        for input in inputs {
            self.index_source(namespace, &input.full_path(options), input.root.as_str())
                .await?;
        }
        let tx = self.begin(namespace).await?;
        let expected = tx.catalog_generation(namespace).await?;
        let mut files = Vec::new();
        let mut claims = Vec::new();
        for input in inputs {
            let path = input.full_path(options);
            let name = path
                .to_str()
                .ok_or_else(|| backend("invalid native path"))?;
            let (ledger, index) = tx
                .load_index(namespace, name)
                .await?
                .ok_or_else(|| backend("native index unavailable"))?;
            let tag = tx
                .ledger(namespace, name)
                .await?
                .ok_or_else(|| backend("native ledger unavailable"))?;
            claims.push((name.to_owned(), Some(tag.epoch)));
            files.push((name.to_owned(), ledger, index));
        }
        tx.commit(namespace).await?;
        let mut cache = EventCache::from_projection(files, claims);
        let options = Options {
            scan_window: false,
            ..options.clone()
        };
        let prepared =
            crate::model::prepare_sources(&options, &mut cache, inputs, crate::model::now_ms())
                .map_err(backend)?;
        if prepared.rows.is_empty() {
            return Err(backend("native cohort has no qualified Catalog owners"));
        }
        // Shared derivation qualifies byte generations before/after native I/O.
        // Fence a later Raw mutation too, before starting the publication CAS.
        for row in &prepared.rows {
            for source in &row.sources {
                if crate::events::Stat::of(&source.path).map_err(backend)?
                    != crate::source_carrier::ledger(source).stat
                {
                    return Ok(None);
                }
            }
        }
        let tx = self.begin(namespace).await?;
        for input in inputs {
            if tx.raw_write_pending(namespace, input).await? {
                return Ok(None);
            }
        }
        for (path, epoch) in &prepared.claims {
            if tx.ledger(namespace, path).await?.map(|tag| tag.epoch) != *epoch {
                return Ok(None);
            }
        }
        let generation = tx
            .publish_catalog(
                namespace,
                &Publication {
                    rows: &prepared.rows,
                    transcripts: Some(&prepared.transcripts),
                },
                expected.as_deref(),
            )
            .await?;
        tx.commit(namespace).await?;
        Ok(generation)
    }
}
