//! Bounded native derivation over PostgreSQL indexes, without a SQLite store.
use super::{Namespace, PgProjectionStore, StorageError, backend, ensure};
use crate::{Input, Options, events::EventCache, slot_projection::Publication};
use std::collections::BTreeSet;

impl PgProjectionStore {
    pub(crate) async fn discover_sources(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        inputs: &[Input],
        start: bool,
        complete: bool,
    ) -> Result<(), StorageError> {
        ensure(&self.namespace, namespace)?;
        let tx = self.begin(namespace).await?;
        if start {
            tx.tx.execute("UPDATE semon_projection.namespaces SET discovery_complete=false WHERE namespace=$1", &[&namespace.binding()]).await.map_err(backend)?;
            tx.tx
                .execute(
                    "UPDATE semon_projection.discovery SET seen=false WHERE namespace=$1",
                    &[&namespace.binding()],
                )
                .await
                .map_err(backend)?;
        }
        for input in inputs {
            if !crate::is_input_path(input.root.as_str(), &input.path)
                || !input.path.ends_with(".jsonl")
            {
                return Err(backend("invalid discovery input"));
            }
            tx.tx.execute("INSERT INTO semon_projection.discovery(namespace,root,path) VALUES($1,$2,$3) ON CONFLICT(namespace,root,path) DO UPDATE SET seen=true,pending=true,refused=false", &[&namespace.binding(), &input.root.as_str(), &input.path]).await.map_err(backend)?;
        }
        if complete {
            tx.tx.execute("UPDATE semon_projection.namespaces SET discovery_complete=true WHERE namespace=$1", &[&namespace.binding()]).await.map_err(backend)?;
            let absent = tx.tx.query("SELECT root,path FROM semon_projection.discovery WHERE namespace=$1 AND NOT seen", &[&namespace.binding()]).await.map_err(backend)?;
            for row in absent {
                let root: &str = row.get(0);
                let input = Input {
                    root: crate::InputRoot::parse(root)
                        .ok_or_else(|| backend("invalid discovery root"))?,
                    path: row.get(1),
                };
                let path = input.full_path(options);
                let name = path
                    .to_str()
                    .ok_or_else(|| backend("invalid discovery path"))?;
                if let Some(tag) = tx.ledger(namespace, name).await? {
                    let owned: bool = tx.tx.query_one("SELECT EXISTS(SELECT 1 FROM semon_projection.units u JOIN semon_projection.catalog c ON c.namespace=u.namespace AND c.session_key=u.unit_key WHERE u.namespace=$1 AND u.path=$2 AND c.lifecycle='current')", &[&namespace.binding(), &name]).await.map_err(backend)?.get(0);
                    if owned && !tx.withdraw_source(namespace, name, tag).await? {
                        return Err(backend("removed source custody changed"));
                    }
                }
            }
            tx.tx
                .execute(
                    "DELETE FROM semon_projection.discovery WHERE namespace=$1 AND NOT seen",
                    &[&namespace.binding()],
                )
                .await
                .map_err(backend)?;
            // An observed empty machine is a real empty publication.
            tx.tx.execute("INSERT INTO semon_projection.catalog_heads(namespace,generation) SELECT $1,repeat('0',64) WHERE NOT EXISTS(SELECT 1 FROM semon_projection.discovery WHERE namespace=$1 AND seen) ON CONFLICT DO NOTHING", &[&namespace.binding()]).await.map_err(backend)?;
        }
        tx.commit(namespace).await
    }

    pub(crate) async fn pending_sources(
        &mut self,
        namespace: &Namespace,
    ) -> Result<Vec<Input>, StorageError> {
        let tx = self.begin(namespace).await?;
        let rows = tx.tx.query("SELECT root,path FROM semon_projection.discovery WHERE namespace=$1 AND seen AND pending ORDER BY root,path LIMIT 64", &[&namespace.binding()]).await.map_err(backend)?;
        let inputs = rows
            .into_iter()
            .map(|row| {
                let root: &str = row.get(0);
                Ok(Input {
                    root: crate::InputRoot::parse(root)
                        .ok_or_else(|| backend("invalid discovery root"))?,
                    path: row.get(1),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        tx.commit(namespace).await?;
        Ok(inputs)
    }

    pub(crate) async fn publish_discovered(
        &mut self,
        namespace: &Namespace,
        options: &Options,
        inputs: &[Input],
    ) -> Result<(), StorageError> {
        // Revisit already-published owners that share a native identity or an
        // explicit parent with this cohort. The same 64-source bound applies.
        let mut component: BTreeSet<Input> = inputs.iter().cloned().collect();
        let tx = self.begin(namespace).await?;
        let published = tx.tx.query("SELECT metadata FROM semon_projection.catalog WHERE namespace=$1 AND lifecycle='current' ORDER BY session_key", &[&namespace.binding()]).await.map_err(backend)?;
        let mut owners = Vec::new();
        let mut bytes = 0usize;
        for item in published {
            let text: &str = item.get(0);
            bytes += text.len();
            if bytes > 8 * 1024 * 1024 {
                return Err(backend("discovery relationship metadata budget"));
            }
            owners.push(
                serde_json::from_str::<crate::model::summary::CatalogRow>(text).map_err(backend)?,
            );
        }
        tx.commit(namespace).await?;
        let mut identities = BTreeSet::new();
        for input in inputs {
            if input.root == crate::InputRoot::Claude {
                if let Some(id) = input
                    .full_path(options)
                    .file_stem()
                    .and_then(|name| name.to_str())
                {
                    identities.insert(id.to_owned());
                }
                if let Some(parent) = input
                    .path
                    .split("/subagents/")
                    .next()
                    .filter(|_| input.path.contains("/subagents/"))
                    .and_then(|prefix| prefix.rsplit('/').next())
                {
                    identities.insert(parent.to_owned());
                }
            } else if input.root == crate::InputRoot::Codex {
                let file =
                    crate::sealed::LogFile::open(&input.full_path(options)).map_err(backend)?;
                let mut line = String::new();
                use std::io::{BufRead, Read};
                std::io::BufReader::new(file.take(32 * 1024 * 1024))
                    .read_line(&mut line)
                    .map_err(backend)?;
                let header = crate::tx::parse_native_record(line.as_bytes())
                    .map(|value| crate::native_header::CodexHeader::from_record(&value));
                if let Some(header) = header {
                    identities.extend(header.native_id(&input.full_path(options)));
                    identities.extend(
                        serde_json::to_value(&header.metadata).map_err(backend)?["parent_thread"]
                            .as_str()
                            .map(str::to_owned),
                    );
                    let tx = self.begin(namespace).await?;
                    let discovered = tx.tx.query("SELECT path FROM semon_projection.discovery WHERE namespace=$1 AND root='codex' AND seen ORDER BY path LIMIT 4097", &[&namespace.binding()]).await.map_err(backend)?;
                    tx.commit(namespace).await?;
                    if discovered.len() > 4096 {
                        return Err(backend("Codex lineage candidate bound"));
                    }
                    for candidate in discovered {
                        let candidate = Input {
                            root: crate::InputRoot::Codex,
                            path: candidate.get(0),
                        };
                        if candidate == *input {
                            continue;
                        }
                        let mut line = String::new();
                        let file = crate::sealed::LogFile::open(&candidate.full_path(options))
                            .map_err(backend)?;
                        std::io::BufReader::new(file.take(32 * 1024 * 1024))
                            .read_line(&mut line)
                            .map_err(backend)?;
                        if let Some(value) = crate::tx::parse_native_record(line.as_bytes()) {
                            let other = crate::native_header::CodexHeader::from_record(&value);
                            let id = other.native_id(&candidate.full_path(options));
                            let parent = serde_json::to_value(&other.metadata).map_err(backend)?["parent_thread"].as_str().map(str::to_owned);
                            if id.as_ref().is_some_and(|id| identities.contains(id))
                                || parent.as_ref().is_some_and(|id| identities.contains(id))
                            {
                                identities.extend(id);
                                identities.extend(parent);
                                component.insert(candidate);
                            }
                        }
                    }
                }
            }
        }
        loop {
            let before = component.len();
            for row in &owners {
                if row.native_ids.iter().any(|id| identities.contains(id))
                    || row
                        .parent
                        .as_ref()
                        .is_some_and(|id| identities.contains(id))
                    || row.sources.iter().any(|source| {
                        crate::native_evidence::catalog_input(&source.path, options)
                            .is_some_and(|input| component.contains(&input))
                    })
                {
                    identities.extend(row.native_ids.iter().cloned());
                    identities.extend(row.parent.iter().cloned());
                    component.extend(row.sources.iter().filter_map(|source| {
                        crate::native_evidence::catalog_input(&source.path, options)
                    }));
                }
            }
            if component.len() > 64 {
                return Err(backend("discovery relationship source bound"));
            }
            if component.len() == before {
                break;
            }
        }
        let component: Vec<_> = component.into_iter().collect();
        match self.publish_sources(namespace, options, &component).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(backend("native Catalog generation changed")),
            Err(error) => {
                // A refused cohort is durable unavailable work, not a hot queue
                // item. A later source audit re-admits it after discovery.
                let tx = self.begin(namespace).await?;
                for input in inputs {
                    tx.tx.execute("UPDATE semon_projection.discovery SET pending=false,refused=true WHERE namespace=$1 AND root=$2 AND path=$3", &[&namespace.binding(), &input.root.as_str(), &input.path]).await.map_err(backend)?;
                }
                tx.commit(namespace).await?;
                Err(error)
            }
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
        let _facts_lock =
            crate::source_change::SourceLock::try_shared(options, "facts").map_err(backend)?;
        let _source_locks = inputs
            .iter()
            .map(|input| {
                crate::source_change::SourceLock::try_shared(
                    options,
                    &crate::source_change::lock_key(options, input),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        let tx = self.begin(namespace).await?;
        if tx.facts_write_pending(namespace).await? {
            return Err(backend("Facts write is pending"));
        }
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
        let reported_runs = tx
            .reported_facts(namespace)
            .await?
            .map(|(_, facts)| facts.reported_runs)
            .unwrap_or_default();
        tx.commit(namespace).await?;
        let nonempty = inputs
            .iter()
            .filter(|input| {
                files.iter().any(|(path, ledger, _)| {
                    *path == input.full_path(options).to_string_lossy() && ledger.stat.size > 0
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut cache = EventCache::from_projection(files, claims.clone());
        cache.set_reported_runs(reported_runs);
        let options = Options {
            scan_window: false,
            ..options.clone()
        };
        let mut prepared =
            crate::model::prepare_sources(&options, &mut cache, &nonempty, crate::model::now_ms())
                .map_err(backend)?;
        prepared.claims = claims;
        if prepared.rows.is_empty() {
            let tx = self.begin(namespace).await?;
            tx.tx
                .query_one(
                    "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                    &[&namespace.binding()],
                )
                .await
                .map_err(backend)?;
            if tx.catalog_generation(namespace).await? != expected {
                return Ok(None);
            }
            for (path, epoch) in &prepared.claims {
                if tx.ledger(namespace, path).await?.map(|tag| tag.epoch) != *epoch {
                    return Ok(None);
                }
            }
            for input in inputs {
                if tx.raw_write_pending(namespace, input).await? {
                    return Ok(None);
                }
                let path = input.full_path(&options);
                let name = path
                    .to_str()
                    .ok_or_else(|| backend("invalid empty source path"))?;
                // An empty source can retire only its previously qualified
                // observation; retained metadata and recipes remain in custody.
                let owners = tx.tx.query("SELECT DISTINCT unit_key FROM semon_projection.units WHERE namespace=$1 AND path=$2", &[&namespace.binding(), &name]).await.map_err(backend)?;
                for owner in owners {
                    let key: &str = owner.get(0);
                    tx.preserve_history(namespace, key).await?;
                    tx.tx.execute("UPDATE semon_projection.catalog SET lifecycle='retained' WHERE namespace=$1 AND session_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?;
                    tx.catalog_changed(namespace, key).await?;
                }
            }
            use sha2::{Digest, Sha256};
            let generation = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&(expected, &prepared.claims)).map_err(backend)?)
            );
            tx.tx.execute("INSERT INTO semon_projection.catalog_heads(namespace,generation) VALUES($1,$2) ON CONFLICT(namespace) DO UPDATE SET generation=excluded.generation", &[&namespace.binding(), &generation]).await.map_err(backend)?;
            for input in inputs {
                tx.tx.execute("UPDATE semon_projection.discovery SET pending=false,refused=false WHERE namespace=$1 AND root=$2 AND path=$3", &[&namespace.binding(), &input.root.as_str(), &input.path]).await.map_err(backend)?;
            }
            tx.commit(namespace).await?;
            return Ok(Some(generation));
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
        if prepared
            .lineage_dependencies
            .iter()
            .any(|(harness, native)| {
                prepared
                    .rows
                    .iter()
                    .all(|row| row.harness != *harness || !row.native_ids.contains(native))
            })
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
        if generation.is_some() {
            // Checkpoint only the source generations qualified by this exact
            // publication transaction. A later Raw/Facts completion requeues
            // its work instead of losing it in a separate dequeue transaction.
            for input in inputs {
                tx.tx.execute("UPDATE semon_projection.discovery SET pending=false,refused=false WHERE namespace=$1 AND root=$2 AND path=$3", &[&namespace.binding(), &input.root.as_str(), &input.path]).await.map_err(backend)?;
            }
        }
        if generation.is_some() {
            for row in &prepared.rows {
                tx.derive_history_query(namespace, &options, &row.key)
                    .await?;
            }
        }
        tx.commit(namespace).await?;
        Ok(generation)
    }
}
