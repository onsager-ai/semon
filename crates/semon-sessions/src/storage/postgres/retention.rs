//! Metadata-only archive custody through the admitted PostgreSQL owner.
use super::{Namespace, PgProjectionTransaction, StorageError, backend, ensure};
use crate::{CatalogReadScope, Options, SessionSourceProof, SourceProjectionReady};

impl PgProjectionTransaction<'_> {
    pub(crate) async fn source_proof(
        &self,
        namespace: &Namespace,
        options: &Options,
        root: &str,
        path: &str,
    ) -> Result<Option<SessionSourceProof>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let input = input(root, path)?;
        let absolute = input.full_path(options);
        let name = absolute
            .to_str()
            .ok_or_else(|| backend("invalid source path"))?;
        let owners = self.tx.query(
            "SELECT unit_key FROM semon_projection.units u JOIN semon_projection.catalog c ON c.namespace=u.namespace AND c.session_key=u.unit_key WHERE u.namespace=$1 AND u.path=$2 AND c.lifecycle='current' ORDER BY unit_key LIMIT 1",
            &[&namespace.binding(), &name],
        ).await.map_err(backend)?;
        let Some(owner) = owners.first() else {
            return Ok(None);
        };
        let key: &str = owner.get(0);
        let row = self
            .catalog_row(namespace, key, CatalogReadScope::Current)
            .await?
            .ok_or_else(|| backend("source owner unavailable"))?;
        if row.harness != root {
            return Err(backend("source harness changed"));
        }
        let source = row
            .sources
            .iter()
            .find(|source| source.path == absolute)
            .ok_or_else(|| backend("source mapping unavailable"))?;
        let ledger = self
            .tx
            .query_opt(
                "SELECT ledger FROM semon_projection.files WHERE namespace=$1 AND path=$2",
                &[&namespace.binding(), &name],
            )
            .await
            .map_err(backend)?
            .ok_or_else(|| backend("source ledger unavailable"))?;
        let ledger: crate::events::Ledger = serde_json::from_str(ledger.get(0)).map_err(backend)?;
        if ledger != crate::source_carrier::ledger(source) {
            return Err(backend("source ledger changed"));
        }
        let request =
            crate::catalog::Request::parse(&format!("sid={}", crate::viewer::percent_encode(key)))
                .map_err(|_| backend("invalid source owner"))?;
        let reply = crate::catalog::postgres_page(self, namespace, options, &request)
            .await
            .map_err(backend)?;
        let (identity, _) = crate::catalog::selection_from_reply(
            reply,
            namespace.as_str(),
            key,
            CatalogReadScope::Current,
            false,
        )
        .map_err(|_| backend("source identity unavailable"))?
        .ok_or_else(|| backend("source identity absent"))?;
        let observed = identity
            .source_refs
            .iter()
            .find(|observed| observed.source.root == root && observed.source.path == path)
            .ok_or_else(|| backend("source scope changed"))?;
        if !matches!(observed.state.as_str(), "cached" | "incomplete")
            || observed.source.native_id != source.native_id
            || observed.source.offset != source.offset
            || observed.source.prefix_sha256 != source.prefix_sha256
            || observed.source.tail_sha256 != source.tail_sha256
        {
            return Err(backend("source observation changed"));
        }
        Ok(Some(SessionSourceProof {
            source: observed.source.clone(),
            dev: source.dev,
            ino: source.ino,
            size: source.size,
            modified_ns: source.modified_ns,
            changed_ns: source.changed_ns,
            freshness: crate::catalog::CatalogFreshness {
                state: observed.state.clone(),
            },
        }))
    }

    pub(crate) async fn projection_ready(
        &self,
        namespace: &Namespace,
        options: &Options,
        root: &str,
        path: &str,
    ) -> Result<Option<SourceProjectionReady>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let input = input(root, path)?;
        let absolute = input.full_path(options);
        let name = absolute
            .to_str()
            .ok_or_else(|| backend("invalid source path"))?;
        if self.raw_write_pending(namespace, &input).await? {
            return Ok(None);
        }
        // A partial discovery cannot prove that every owning projection is here.
        let ready: bool = self.tx.query_one(
            "SELECT discovery_complete AND NOT EXISTS(SELECT 1 FROM semon_projection.discovery WHERE namespace=$1 AND (pending OR refused)) FROM semon_projection.namespaces WHERE namespace=$1",
            &[&namespace.binding()],
        ).await.map_err(backend)?.get(0);
        if !ready {
            return Ok(None);
        }
        for (lifecycle, scope) in [
            ("current", CatalogReadScope::Current),
            ("retained", CatalogReadScope::RetainedHistory),
        ] {
            let owners = self.tx.query(
                "SELECT session_key FROM semon_projection.catalog WHERE namespace=$1 AND lifecycle=$2 AND EXISTS(SELECT 1 FROM jsonb_array_elements(metadata::jsonb->'sources') s WHERE s->>'path'=$3) ORDER BY session_key LIMIT 9",
                &[&namespace.binding(), &lifecycle, &name],
            ).await.map_err(backend)?;
            if owners.is_empty() {
                continue;
            }
            if owners.len() > 8 {
                return Ok(None);
            }
            let mut result: Option<SourceProjectionReady> = None;
            for owner in owners {
                let key: &str = owner.get(0);
                let row = self
                    .catalog_row(namespace, key, scope)
                    .await?
                    .ok_or_else(|| backend("projection owner unavailable"))?;
                if row.harness != root || row.sources.len() > 64 {
                    return Ok(None);
                }
                for source in &row.sources {
                    let dependency =
                        crate::native_evidence::catalog_input(&source.path, options)
                            .ok_or_else(|| backend("projection source outside configured roots"))?;
                    if self.raw_write_pending(namespace, &dependency).await? {
                        return Ok(None);
                    }
                }
                let Some(source) = row.sources.iter().find(|source| source.path == absolute) else {
                    return Ok(None);
                };
                let (generation, _, _) =
                    self.recipe_page(namespace, key, scope, None, 0, 1).await?;
                let reference = crate::SessionSourceRef {
                    root: root.into(),
                    path: path.into(),
                    native_id: source.native_id.clone(),
                    offset: source.offset,
                    prefix_sha256: source.prefix_sha256,
                    tail_sha256: source.tail_sha256,
                };
                if reference.native_id.is_empty()
                    || result
                        .as_ref()
                        .is_some_and(|ready| ready.source != reference)
                {
                    return Ok(None);
                }
                if result.is_none() {
                    result = Some(SourceProjectionReady {
                        source: reference,
                        projection_version: crate::slot_projection::VERSION,
                        projection_generation: generation,
                        lifecycle: lifecycle.into(),
                    });
                }
            }
            return Ok(result);
        }
        Ok(None)
    }

    pub(crate) async fn retain_index(
        &self,
        namespace: &Namespace,
        options: &Options,
        proof: &SessionSourceProof,
        projection: &SourceProjectionReady,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        self.tx
            .query_one(
                "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
        if proof.source != projection.source
            || !self
                .projection_ready(namespace, options, &proof.source.root, &proof.source.path)
                .await?
                .is_some_and(|ready| crate::retention::same_projection(&ready, projection))
        {
            return Ok(false);
        }
        let path = input(&proof.source.root, &proof.source.path)?.full_path(options);
        let name = path
            .to_str()
            .ok_or_else(|| backend("invalid retention path"))?;
        let Some((ledger, index)) = self.load_index(namespace, name).await? else {
            return Ok(false);
        };
        if ledger != proof_ledger(proof)
            || index.source_chunks.is_none()
            || index.handoff_index.is_none()
            || (matches!(proof.source.root.as_str(), "claude" | "codex")
                && index.marker_index.is_none())
            || (proof.source.root == "codex"
                && index
                    .codex_header
                    .as_ref()
                    .and_then(|header| header.native_id(&path))
                    .as_deref()
                    != Some(proof.source.native_id.as_str()))
            || (proof.source.root == "claude"
                && crate::native_sidecar::agent_path(&path)
                && !index
                    .claude_sidecar
                    .as_ref()
                    .is_some_and(|sidecar| sidecar.verified(&path)))
        {
            return Ok(false);
        }
        let tag = self
            .ledger(namespace, name)
            .await?
            .ok_or_else(|| backend("retention ledger unavailable"))?;
        let digest = index_digest(&ledger, &index)?;
        self.tx.execute(
            "INSERT INTO semon_projection.retained_indexes(namespace,path,ledger,index_digest,projection) VALUES($1,$2,$3,$4,$5) ON CONFLICT(namespace,path) DO UPDATE SET ledger=excluded.ledger,index_digest=excluded.index_digest,projection=excluded.projection",
            &[&namespace.binding(), &name, &serde_json::to_string(&ledger).map_err(backend)?, &&digest[..], &serde_json::to_string(projection).map_err(backend)?],
        ).await.map_err(backend)?;
        // Capture History before the host is allowed to withdraw its Raw.
        self.capture_source_history(namespace, name, tag).await
    }

    pub(crate) async fn index_retained(
        &self,
        namespace: &Namespace,
        options: &Options,
        proof: &SessionSourceProof,
        projection: &SourceProjectionReady,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        if proof.source != projection.source {
            return Ok(false);
        }
        let path = input(&proof.source.root, &proof.source.path)?.full_path(options);
        let name = path
            .to_str()
            .ok_or_else(|| backend("invalid retention path"))?;
        let saved = self.tx.query_opt("SELECT ledger,projection,index_digest FROM semon_projection.retained_indexes WHERE namespace=$1 AND path=$2", &[&namespace.binding(), &name]).await.map_err(backend)?;
        let Some(saved) = saved else {
            return Ok(false);
        };
        let ledger: crate::events::Ledger = serde_json::from_str(saved.get(0)).map_err(backend)?;
        let ready: SourceProjectionReady = serde_json::from_str(saved.get(1)).map_err(backend)?;
        if ledger != proof_ledger(proof) || !crate::retention::same_projection(&ready, projection) {
            return Ok(false);
        }
        let Some((current, index)) = self.load_index(namespace, name).await? else {
            return Ok(false);
        };
        Ok(current == ledger
            && index_digest(&current, &index)?.as_slice() == saved.get::<_, &[u8]>(2)
            && self
                .projection_ready(namespace, options, &proof.source.root, &proof.source.path)
                .await?
                .is_some_and(|ready| crate::retention::same_projection(&ready, projection)))
    }
}

fn index_digest(
    ledger: &crate::events::Ledger,
    index: &crate::events::FileIndex,
) -> Result<[u8; 32], StorageError> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&(ledger, index)).map_err(backend)?);
    for event in index.events.iter().chain(&index.extras) {
        digest.update(serde_json::to_vec(&event.activity_observation).map_err(backend)?);
    }
    let chunks = index
        .source_chunks
        .as_ref()
        .ok_or_else(|| backend("retained proof unavailable"))?;
    digest.update(chunks.fingerprint());
    Ok(digest.finalize().into())
}

fn input(root: &str, path: &str) -> Result<crate::Input, StorageError> {
    if !path.ends_with(".jsonl") || !crate::is_input_path(root, path) {
        return Err(backend("invalid custody source"));
    }
    Ok(crate::Input {
        root: crate::InputRoot::parse(root).ok_or_else(|| backend("invalid custody root"))?,
        path: path.into(),
    })
}

fn proof_ledger(proof: &SessionSourceProof) -> crate::events::Ledger {
    crate::events::Ledger {
        stat: crate::events::Stat {
            dev: proof.dev,
            ino: proof.ino,
            size: proof.size,
            modified_ns: proof.modified_ns,
            changed_ns: proof.changed_ns,
        },
        offset: proof.source.offset,
        prefix: proof.source.prefix_sha256,
        tail: proof.source.tail_sha256,
    }
}
