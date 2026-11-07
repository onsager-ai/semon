//! Bounded native derivation over PostgreSQL indexes, without a SQLite store.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure};
use crate::{Input, Options, events::EventCache, slot_projection::Publication};
use std::collections::BTreeSet;

impl PgProjectionStore {
    pub(crate) async fn discover_sources(
        &mut self, namespace: &Namespace, options: &Options, inputs: &[Input], start: bool, complete: bool,
    ) -> Result<(), StorageError> {
        ensure(&self.namespace, namespace)?;
        let tx = self.begin(namespace).await?;
        if start { tx.tx.execute("UPDATE semon_projection.discovery SET seen=false WHERE namespace=$1", &[&namespace.binding()]).await.map_err(backend)?; }
        for input in inputs {
            if !crate::is_input_path(input.root.as_str(), &input.path) || !input.path.ends_with(".jsonl") { return Err(backend("invalid discovery input")); }
            tx.tx.execute("INSERT INTO semon_projection.discovery(namespace,root,path) VALUES($1,$2,$3) ON CONFLICT(namespace,root,path) DO UPDATE SET seen=true,pending=true", &[&namespace.binding(), &input.root.as_str(), &input.path]).await.map_err(backend)?;
        }
        if complete {
            let absent = tx.tx.query("SELECT root,path FROM semon_projection.discovery WHERE namespace=$1 AND NOT seen", &[&namespace.binding()]).await.map_err(backend)?;
            for row in absent {
                let root: &str = row.get(0);
                let input = Input { root: crate::InputRoot::parse(root).ok_or_else(|| backend("invalid discovery root"))?, path: row.get(1) };
                let path = input.full_path(options);
                let name = path.to_str().ok_or_else(|| backend("invalid discovery path"))?;
                if let Some(tag) = tx.ledger(namespace,name).await? {
                    let owned: bool = tx.tx.query_one("SELECT EXISTS(SELECT 1 FROM semon_projection.units u JOIN semon_projection.catalog c ON c.namespace=u.namespace AND c.session_key=u.unit_key WHERE u.namespace=$1 AND u.path=$2 AND c.lifecycle='current')", &[&namespace.binding(), &name]).await.map_err(backend)?.get(0);
                    if owned && !tx.withdraw_source(namespace,name,tag).await? { return Err(backend("removed source custody changed")); }
                }
            }
            tx.tx.execute("DELETE FROM semon_projection.discovery WHERE namespace=$1 AND NOT seen", &[&namespace.binding()]).await.map_err(backend)?;
            // An observed empty machine is a real empty publication.
            tx.tx.execute("INSERT INTO semon_projection.catalog_heads(namespace,generation) SELECT $1,repeat('0',64) WHERE NOT EXISTS(SELECT 1 FROM semon_projection.discovery WHERE namespace=$1 AND seen) ON CONFLICT DO NOTHING", &[&namespace.binding()]).await.map_err(backend)?;
        }
        tx.commit(namespace).await
    }

    pub(crate) async fn pending_sources(&mut self, namespace: &Namespace) -> Result<Vec<Input>, StorageError> {
        let tx = self.begin(namespace).await?;
        let rows = tx.tx.query("SELECT root,path FROM semon_projection.discovery WHERE namespace=$1 AND seen AND pending ORDER BY root,path LIMIT 64", &[&namespace.binding()]).await.map_err(backend)?;
        let inputs = rows.into_iter().map(|row| {
            let root: &str = row.get(0);
            Ok(Input { root: crate::InputRoot::parse(root).ok_or_else(|| backend("invalid discovery root"))?, path: row.get(1) })
        }).collect::<Result<Vec<_>,StorageError>>()?;
        tx.commit(namespace).await?;
        Ok(inputs)
    }

    pub(crate) async fn publish_discovered(&mut self, namespace: &Namespace, options: &Options, inputs: &[Input]) -> Result<(),StorageError> {
        match self.publish_sources(namespace,options,inputs).await {
            Ok(Some(_)) => {
                let tx = self.begin(namespace).await?;
                for input in inputs { tx.tx.execute("UPDATE semon_projection.discovery SET pending=false WHERE namespace=$1 AND root=$2 AND path=$3", &[&namespace.binding(), &input.root.as_str(), &input.path]).await.map_err(backend)?; }
                tx.commit(namespace).await
            }
            Ok(None) => Err(backend("native Catalog generation changed")),
            Err(error) => Err(error),
        }
    }

    /// Index and publish one bounded native source cohort using the shared
    /// native producer. Hosts retain source custody across this operation and
    /// call it outside their request executor because native parsing blocks.
    /// A changed Catalog or source refuses publication; the caller rescans Raw.
    /// The host must supply the complete relationship component. Cohorts with
    /// unresolved cross-component lineage are not qualified by this API.
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
        let _source_locks = inputs
            .iter()
            .map(|input| {
                crate::source_change::SourceLock::acquire(
                    options,
                    &crate::source_change::lock_key(options, input),
                    false,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
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
        // Raw intents update this row. Locking it serializes the publication
        // with a new intent; repeatable-read refuses a changed snapshot.
        tx.tx
            .query_one(
                "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
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
        let keys = prepared
            .rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<BTreeSet<_>>();
        if !prepared.lineage_dependencies.is_empty()
            || prepared.rows.iter().any(|row| {
                row.parent
                    .as_deref()
                    .is_some_and(|parent| !keys.contains(parent))
            })
            || prepared
                .rows
                .iter()
                .filter_map(|row| row.trace.as_ref())
                .any(|trace| !trace.closed_over(&keys))
        {
            return Err(backend(
                "native cohort requires complete relationship closure",
            ));
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
