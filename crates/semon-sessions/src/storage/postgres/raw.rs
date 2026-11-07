//! Durable host Raw intents and generation markers. These are custody state,
//! never reconstructed from the projection cache or inferred from native bytes.
use super::{Namespace, PgProjectionTransaction, StorageError, backend, ensure, number};
use crate::Input;
use serde::{Deserialize, Serialize};

/// Host-qualified immutable Raw generation. This records the host's custody
/// proof; it grants no authorization to access, replace or withdraw bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PgRawGeneration {
    pub generation: String,
    pub length: u64,
    pub sha256: [u8; 32],
}

/// A durable revision token for exactly one namespace and logical source.
/// Dropping it never completes the intent or grants append trust.
pub struct PgRawIntent {
    namespace: Namespace,
    root: String,
    path: String,
    revision: i64,
    intent_id: String,
}

impl PgProjectionTransaction<'_> {
    /// Commit the returned intent before mutating Raw. The host retains its
    /// exclusive source-custody fence through mutation, fsync and completion.
    /// An interrupted intent remains pending across restart; a later intent
    /// supersedes its revision and stale completion cannot acknowledge it.
    pub async fn begin_raw_write(
        &self,
        namespace: &Namespace,
        input: &Input,
    ) -> Result<PgRawIntent, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !crate::is_input_path(input.root.as_str(), &input.path) {
            return Err(backend("Raw intent outside native allowlist"));
        }
        self.tx
            .query_one(
                "UPDATE semon_projection.namespaces SET epoch=epoch+1 WHERE namespace=$1 RETURNING epoch",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
        let root = input.root.as_str();
        let issued = self.tx.query_one(
            "INSERT INTO semon_projection.raw_sources(namespace,root,path,revision,pending) VALUES($1,$2,$3,1,true) ON CONFLICT(namespace,root,path) DO UPDATE SET revision=semon_projection.raw_sources.revision+1,pending=true,generation=NULL,intent_id=gen_random_uuid()::text RETURNING revision,intent_id",
            &[&namespace.binding(), &root, &input.path],
        ).await.map_err(backend)?;
        let revision = issued.get(0);
        let intent_id = issued.get(1);
        Ok(PgRawIntent {
            namespace: namespace.clone(),
            root: root.into(),
            path: input.path.clone(),
            revision,
            intent_id,
        })
    }

    /// Only after the host has fsynced and independently qualified these exact
    /// bytes. This atomic compare-and-swap records completion and its generation;
    /// a stale token has no partial effect. It does not mutate Raw itself.
    pub async fn complete_raw_write(
        &self,
        namespace: &Namespace,
        intent: PgRawIntent,
        generation: &PgRawGeneration,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        ensure(&intent.namespace, namespace)?;
        if generation.generation.len() != 32
            || !generation
                .generation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(backend("invalid Raw generation"));
        }
        number(generation.length)?;
        let metadata = serde_json::to_string(generation).map_err(backend)?;
        let completed = self.tx.execute(
            "UPDATE semon_projection.raw_sources SET pending=false,generation=$5 WHERE namespace=$1 AND root=$2 AND path=$3 AND revision=$4 AND pending AND intent_id=$6",
            &[&namespace.binding(), &intent.root, &intent.path, &intent.revision, &metadata, &intent.intent_id],
        ).await.map_err(backend)? == 1;
        if completed {
            if intent.path.ends_with(".jsonl") {
                self.tx.execute("INSERT INTO semon_projection.discovery(namespace,root,path) VALUES($1,$2,$3) ON CONFLICT(namespace,root,path) DO UPDATE SET seen=true,pending=true,refused=false", &[&namespace.binding(), &intent.root, &intent.path]).await.map_err(backend)?;
            } else {
                // A sidecar can change native ownership; a PID record changes
                // runtime evidence. Revisit every known owner conservatively.
                self.tx.execute("UPDATE semon_projection.discovery SET pending=true WHERE namespace=$1 AND seen", &[&namespace.binding()]).await.map_err(backend)?;
            }
        }
        Ok(completed)
    }

    /// A pending write has no readable completion marker, even when an older
    /// generation was complete. Recovery must qualify Raw under the host fence.
    pub async fn raw_generation(
        &self,
        namespace: &Namespace,
        input: &Input,
    ) -> Result<Option<PgRawGeneration>, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !crate::is_input_path(input.root.as_str(), &input.path) {
            return Err(backend("Raw marker outside native allowlist"));
        }
        let row = self.tx.query_opt(
            "SELECT generation FROM semon_projection.raw_sources WHERE namespace=$1 AND root=$2 AND path=$3 AND NOT pending",
            &[&namespace.binding(), &input.root.as_str(), &input.path],
        ).await.map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get(0)).map_err(backend))
            .transpose()
    }

    pub async fn raw_write_pending(
        &self,
        namespace: &Namespace,
        input: &Input,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !crate::is_input_path(input.root.as_str(), &input.path) {
            return Err(backend("Raw intent outside native allowlist"));
        }
        Ok(self.facts_write_pending(namespace).await?
            || self.runtime_write_pending(namespace).await?
            || self.tx.query_opt(
                "SELECT pending FROM semon_projection.raw_sources WHERE namespace=$1 AND root=$2 AND path=$3",
                &[&namespace.binding(), &input.root.as_str(), &input.path],
            ).await.map_err(backend)?.is_some_and(|row| row.get(0)))
    }

    pub(crate) async fn runtime_write_pending(
        &self,
        namespace: &Namespace,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        Ok(self.tx.query_one(
            "SELECT EXISTS(SELECT 1 FROM semon_projection.raw_sources WHERE namespace=$1 AND pending AND path NOT LIKE '%.jsonl')",
            &[&namespace.binding()],
        ).await.map_err(backend)?.get(0))
    }
}
