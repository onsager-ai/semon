//! Exact native request intent and completion. No Raw mutation or Catalog ACK.
use super::{
    OperationIdentity, PgCancellation, PgPool, Reconciliation, native_reader, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::{
    events::{CACHE_VERSION, DirtyRows, FileIndex, Ledger, Stat, store::header_ledger},
    storage::{LogicalSourceNamespace, StorageError, native_events::EventReceipt},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const KIND: &str = "native-event-v1";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub(super) id: String,
    path: String,
    harness: String,
    request: [u8; 32],
    base: String,
    revision: Option<i64>,
}
impl Intent {
    pub(super) fn harness(&self) -> &str {
        &self.harness
    }
}

fn request(
    namespace: &LogicalSourceNamespace,
    path: &str,
    harness: &str,
    id: &str,
) -> Result<[u8; 32], StorageError> {
    if path.is_empty()
        || id.len() != 64
        || !id.bytes().all(|b| b.is_ascii_hexdigit())
        || !matches!(harness, "claude" | "codex" | "copilot")
    {
        return Err(StorageError::InvalidArguments);
    }
    let payload = serde_json::to_vec(&(KIND, namespace.as_str(), path, harness, CACHE_VERSION))
        .map_err(|_| StorageError::InvalidArguments)?;
    Ok(Sha256::digest(payload).into())
}

fn claim(tx: &mut PgTxn<'_>, path: &str) -> Result<Option<i64>, StorageError> {
    let rows=tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2",&[&tx.namespace.as_str().as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?;
    rows.first()
        .map(|r| r.try_get(0).map_err(|_| StorageError::Unavailable))
        .transpose()
}

fn stored(tx: &mut PgTxn<'_>, id: &str) -> Result<Option<Intent>, StorageError> {
    let key = format!("native_event_intent:{id}");
    super::tree_writer::metadata(tx, key.as_bytes())?
        .map(|s| serde_json::from_str(&s).map_err(|_| StorageError::ScopeChanged))
        .transpose()
}

pub(super) fn begin(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    path: &str,
    harness: &str,
    id: &str,
    expected: Option<&Ledger>,
) -> Result<Intent, StorageError> {
    let digest = request(namespace, path, harness, id)?;
    pool.transaction(namespace,TxnKind::Write,control,|tx|{
        super::native_core::qualify(tx,path,expected)?;
        let intent=Intent { id:id.to_owned(),path:path.to_owned(),harness:harness.to_owned(),request:digest,base:expected.map(header_ledger).unwrap_or_default(),revision:claim(tx,path)? };
        if let Some(prior)=stored(tx,id)? {
            if prior!=intent { return Err(StorageError::Conflict); }
        } else {
            let key=format!("native_event_intent:{id}");
            let value=serde_json::to_vec(&intent).map_err(|_|StorageError::Unavailable)?;
            // qualify() holds SourceState through COMMIT, so the exact prior
            // intent check above and this collision-checked write are atomic.
            super::native_keys::put_meta(tx,key.as_bytes(),&value)?;
        }
        Ok(intent)
    })
}

pub(super) fn reconcile(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    id: &str,
    path: &str,
    harness: &str,
) -> Result<Option<EventReceipt>, StorageError> {
    let digest = request(namespace, path, harness, id)?;
    match pool.reconcile(
        namespace,
        &OperationIdentity {
            operation_id: id,
            kind: KIND,
            request_digest: &digest,
        },
        control,
    )? {
        Reconciliation::Absent => {
            pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
                if let Some(intent) = stored(tx, id)?
                    && (intent.request != digest
                        || intent.path != path
                        || intent.harness != harness
                        || intent.id != id)
                {
                    return Err(StorageError::Conflict);
                }
                Ok(None)
            })
        }
        Reconciliation::Committed(value) => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Completion {
                operation_id: String,
                consumed_bytes: u64,
                observed_revision: Option<i64>,
            }
            let c: Completion =
                serde_json::from_value(value).map_err(|_| StorageError::ScopeChanged)?;
            if c.operation_id != id {
                return Err(StorageError::ScopeChanged);
            }
            Ok(Some(EventReceipt {
                operation_id: c.operation_id,
                consumed_bytes: c.consumed_bytes,
                observed_revision: c.observed_revision,
                replayed: true,
            }))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn commit(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    intent: &Intent,
    path: &str,
    expected: Option<&Ledger>,
    changes: Option<&DirtyRows>,
    ledger: &Ledger,
    index: &FileIndex,
) -> Result<(), StorageError> {
    if path != intent.path || intent.base != expected.map(header_ledger).unwrap_or_default() {
        return Err(StorageError::Conflict);
    }
    // The shared scanner qualified the open descriptor, parsed prefix and proof
    // on that same handle. Recheck the named witness before reserving SQL.
    if Stat::of(std::path::Path::new(path)).ok().as_ref() != Some(&ledger.stat) {
        return Err(StorageError::Conflict);
    }
    if !index
        .source_chunks
        .as_ref()
        .is_some_and(|c| c.valid(ledger.offset))
        || index.handoff_index.is_none()
        || (intent.harness != "copilot"
            && (index.tree_summary.is_none() || index.marker_index.is_none()))
        || (intent.harness == "codex" && index.codex_header.is_none())
        || (crate::native_sidecar::agent_path(std::path::Path::new(path))
            && intent.harness == "claude"
            && index.claude_sidecar.is_none())
    {
        return Err(StorageError::NamespaceUpgradePending);
    }
    let result=pool.apply_operation(namespace,&OperationIdentity {operation_id:&intent.id,kind:KIND,request_digest:&intent.request},control,|tx|{
        super::native_core::qualify(tx,path,expected)?;
        if stored(tx,&intent.id)?.as_ref()!=Some(intent) || claim(tx,path)?!=intent.revision { return Err(StorageError::Conflict); }
        // Relocation/retirement are deliberately refused before family effects.
        let relocation:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_relocation_candidates WHERE namespace=$1 AND (old_path=$2 OR new_path=$2))",&[&namespace.as_str().as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
        if relocation { return Err(StorageError::NamespaceUpgradePending); }
        // The bundle is a family effect, never the source of a receipt. Require
        // the actual committed family authority below before recording outcome.
        let _partial=super::native_bundle::stage(tx,crate::events::native_commit::Input {path,expected,changes,ledger,index,semantic_change:expected!=Some(ledger)})?;
        let Some((_,stored_ledger))=native_reader::ledger(tx,path)? else { return Err(StorageError::ScopeChanged); };
        if stored_ledger!=*ledger { return Err(StorageError::ScopeChanged); }
        let pages=super::native_pages::current(tx,path,ledger)?;
        pages.launch(tx)?;
        native_reader::qualify_completion(tx,path,ledger,index)?;
        if index.tree_summary.is_some() {
            let exact:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_tree_headers h JOIN semon_pg_native_tree_generations g USING(namespace,projection_id) WHERE h.namespace=$1 AND h.source_path=$2 AND h.ledger=$3 AND g.receipt=$3 AND g.version=3 AND g.complete AND NOT g.work_pending)",&[&namespace.as_str().as_bytes(),&path.as_bytes(),&header_ledger(ledger).as_bytes()]).map_err(sqlstate::error)?.get(0);
            if !exact { return Err(StorageError::ScopeChanged); }
        }
        let revision=claim(tx,path)?;
        Ok(serde_json::json!({"operation_id":intent.id,"consumed_bytes":ledger.offset,"observed_revision":revision}))
    });
    match result {
        Ok(_) => Ok(()),
        Err(StorageError::CommitUncertain) => {
            reconcile(pool, namespace, control, &intent.id, path, &intent.harness)?
                .ok_or(StorageError::CommitUncertain)
                .map(|_| ())
        }
        Err(e) => Err(e),
    }
}
