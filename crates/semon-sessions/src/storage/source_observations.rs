//! Durable source observation revisions, consumed as publication claims.
//! Recording observations grants no Raw access and does not publish a Catalog.
use super::StorageError;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub type SourceClaims = Vec<(String, Option<i64>)>;

/// Stable identity for one typed source-observation mutation. The namespace is
/// bound into the ID; the digest binds the operation kind and payload. Callers
/// retain this value across retries so an uncertain commit can be reconciled.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceOperation {
    pub namespace: String,
    pub id: String,
    pub payload_digest: String,
}

impl SourceOperation {
    /// Capture one operation key. Keep the returned value and pass it again on
    /// every retry; a separate call captures a distinct logical operation.
    /// The payload is encoded by the typed producer, not supplied as SQL.
    pub fn capture(namespace: &str, operation: &str, payload: &[u8]) -> Self {
        let payload_digest = Self::digest(operation, payload);

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let id = format!(
            "source-observation-v1:{}:{:x}:{timestamp:x}:{sequence:x}",
            Self::namespace_tag(namespace),
            std::process::id(),
        );

        Self {
            namespace: namespace.to_owned(),
            id,
            payload_digest,
        }
    }

    pub(crate) fn digest(operation: &str, payload: &[u8]) -> String {
        let mut payload_hash = Sha256::new();
        payload_hash.update(b"semon-source-observation-payload-v1\0");
        payload_hash.update((operation.len() as u64).to_be_bytes());
        payload_hash.update(operation.as_bytes());
        payload_hash.update((payload.len() as u64).to_be_bytes());
        payload_hash.update(payload);
        format!("{:x}", payload_hash.finalize())
    }

    pub(crate) fn namespace_tag(namespace: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(b"semon-source-observation-namespace-v1\0");
        digest.update((namespace.len() as u64).to_be_bytes());
        digest.update(namespace.as_bytes());
        format!("{:x}", digest.finalize())
    }
}

/// Result of committing or reconciling one source operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceMutationReceipt {
    Applied,
    AlreadyApplied,
    Conflict,
}

/// Durable typed producer work. The payload and identity travel together from
/// capture, through an uncertain acknowledgement, to restart reconciliation.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceIntent {
    Recover {
        operation: SourceOperation,
        cursor: Option<String>,
    },
    Discover {
        operation: SourceOperation,
        paths: Vec<String>,
        complete: bool,
    },
    Invalidate {
        operation: SourceOperation,
        expected: SourceClaims,
    },
    FinishBackfill {
        operation: SourceOperation,
        expected_generation: Option<String>,
    },
}

impl SourceIntent {
    pub fn operation(&self) -> &SourceOperation {
        match self {
            Self::Recover { operation, .. }
            | Self::Discover { operation, .. }
            | Self::Invalidate { operation, .. }
            | Self::FinishBackfill { operation, .. } => operation,
        }
    }

    pub fn apply(
        &self,
        store: &mut dyn SourceObservationStore,
    ) -> Result<SourceMutationReceipt, StorageError> {
        match self {
            Self::Recover { operation, cursor } => {
                store.recover(operation, cursor.as_deref()).map(|(receipt, _)| receipt)
            }
            Self::Discover {
                operation,
                paths,
                complete,
            } => store.discover(operation, paths, *complete),
            Self::Invalidate {
                operation,
                expected,
            } => store.invalidate(operation, expected),
            Self::FinishBackfill {
                operation,
                expected_generation,
            } => store.finish_backfill(operation, expected_generation.as_deref()),
        }
    }
}

pub trait SourceObservationStore: Send {
    /// A qualified owner witness proves no recovery remains, without capture.
    /// Observation-only adapters do not own native Recovery and refuse it.
    fn recovery_is_current(&mut self) -> Result<bool, StorageError> {
        Err(StorageError::Unavailable)
    }
    fn recovery_cursor(&self) -> Result<Option<String>, StorageError> {
        Err(StorageError::Unavailable)
    }
    /// Apply at most one recovery page. Done is a durable terminal result and
    /// must be returned unchanged on replay, independently of the current cursor.
    fn recover(
        &mut self,
        _operation: &SourceOperation,
        _cursor: Option<&str>,
    ) -> Result<(SourceMutationReceipt, bool), StorageError> {
        Err(StorageError::Unavailable)
    }

    /// Check an owner-connection committed witness before capturing new work.
    /// True means the same discovery is already current; no intent is needed.
    /// Backends without such a witness always take the durable mutation path.
    fn discovery_is_current(
        &mut self,
        _paths: &[String],
        _complete: bool,
    ) -> Result<bool, StorageError> {
        Ok(false)
    }

    /// Commit an intent before attempting its effect. Never replace another
    /// payload under the same key; enforce the bounded outstanding-work limit.
    fn persist_intent(&mut self, intent: &SourceIntent) -> Result<(), StorageError>;
    /// Load at most 256 intents in durable capture order, refusing overflow.
    fn pending_intents(&self) -> Result<Vec<SourceIntent>, StorageError>;
    /// Remove only an intent with a matching terminal backend receipt.
    fn acknowledge_intent(&mut self, operation: &SourceOperation) -> Result<(), StorageError>;
    /// Record newly discovered paths atomically with their invalidations. An
    /// exact retry returns `AlreadyApplied`; a reused key with another payload
    /// or namespace returns `Conflict` without changing observations.
    fn discover(
        &mut self,
        operation: &SourceOperation,
        paths: &[String],
        complete: bool,
    ) -> Result<SourceMutationReceipt, StorageError>;
    /// Advance distinct paths only if every captured claim is still current.
    /// A conflict changes no row; this never acknowledges or publishes work.
    fn invalidate(
        &mut self,
        operation: &SourceOperation,
        expected: &[(String, Option<i64>)],
    ) -> Result<SourceMutationReceipt, StorageError>;
    /// Revisions are durable CAS witnesses, not worker leases.
    fn claims(&self, paths: &[String]) -> Result<SourceClaims, StorageError>;
    fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError>;
    fn backfill_state(&self) -> Result<Option<String>, StorageError>;
    /// Finish a source backfill only at the captured catalog generation.
    fn finish_backfill(
        &mut self,
        operation: &SourceOperation,
        expected_generation: Option<&str>,
    ) -> Result<SourceMutationReceipt, StorageError>;
}

pub(crate) fn operation_replay(
    operation: &SourceOperation,
    namespace: Option<&str>,
    kind: &str,
    payload: &[u8],
) -> Result<Option<SourceMutationReceipt>, StorageError> {
    if operation.namespace != namespace.unwrap_or("") {
        return Ok(Some(SourceMutationReceipt::Conflict));
    }
    if operation.id.is_empty()
        || operation.id.len() > 256
        || !operation.id.is_ascii()
        || operation.payload_digest.len() != 64
        || !operation
            .payload_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(StorageError::InvalidArguments);
    }
    let prefix = format!(
        "source-observation-v1:{}:",
        SourceOperation::namespace_tag(namespace.unwrap_or(""))
    );
    let Some(suffix) = operation.id.strip_prefix(&prefix) else {
        return Ok(Some(SourceMutationReceipt::Conflict));
    };
    let parts = suffix.split(':').collect::<Vec<_>>();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(StorageError::InvalidArguments);
    }
    if operation.payload_digest != SourceOperation::digest(kind, payload) {
        return Ok(Some(SourceMutationReceipt::Conflict));
    }
    Ok(None)
}

pub(crate) fn validate_intent(
    namespace: Option<&str>,
    intent: &SourceIntent,
) -> Result<(), StorageError> {
    let (kind, payload) = match intent {
        SourceIntent::Recover { cursor, .. } => {
            if cursor.as_ref().is_some_and(|value| value.len() > 4096) {
                return Err(StorageError::InvalidArguments);
            }
            ("recover", serde_json::to_vec(cursor))
        }
        SourceIntent::Discover {
            paths, complete, ..
        } => {
            validate_paths(paths)?;
            ("discover", serde_json::to_vec(&(paths, complete)))
        }
        SourceIntent::Invalidate { expected, .. } => {
            let paths = expected
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            validate_paths(&paths)?;
            if paths
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != paths.len()
            {
                return Err(StorageError::InvalidArguments);
            }
            ("invalidate", serde_json::to_vec(expected))
        }
        SourceIntent::FinishBackfill {
            expected_generation,
            ..
        } => {
            if expected_generation
                .as_ref()
                .is_some_and(|value| value.len() > 4096)
            {
                return Err(StorageError::InvalidArguments);
            }
            ("finish_backfill", serde_json::to_vec(expected_generation))
        }
    };
    let payload = payload.map_err(|_| StorageError::InvalidArguments)?;
    if operation_replay(intent.operation(), namespace, kind, &payload)?.is_some() {
        return Err(StorageError::Conflict);
    }
    Ok(())
}

pub(crate) fn validate_paths(paths: &[String]) -> Result<(), StorageError> {
    if paths.len() > 256 || paths.iter().any(|p| p.is_empty() || p.len() > 4096) {
        return Err(StorageError::InvalidArguments);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod conformance {
    use super::*;
    fn discover_operation(namespace: &str, paths: &[String], complete: bool) -> SourceOperation {
        SourceOperation::capture(
            namespace,
            "discover",
            &serde_json::to_vec(&(paths, complete)).unwrap(),
        )
    }
    fn invalidate_operation(
        namespace: &str,
        expected: &[(String, Option<i64>)],
    ) -> SourceOperation {
        SourceOperation::capture(
            namespace,
            "invalidate",
            &serde_json::to_vec(expected).unwrap(),
        )
    }
    fn finish_operation(namespace: &str, generation: Option<&str>) -> SourceOperation {
        SourceOperation::capture(
            namespace,
            "finish_backfill",
            &serde_json::to_vec(&generation).unwrap(),
        )
    }
    pub(crate) fn shared_source_observation_conformance(
        mut open: impl FnMut() -> Box<dyn SourceObservationStore>,
        namespace: &str,
    ) {
        let paths = vec!["conformance-a".to_owned(), "conformance-b".to_owned()];
        let discovery = discover_operation(namespace, &paths, true);
        let mut store = open();
        assert_eq!(
            store.discover(&discovery, &paths, true).unwrap(),
            SourceMutationReceipt::Applied
        );
        let initial_claims = store.claims(&paths).unwrap();
        drop(store);

        // The exact same id and payload reconcile a committed operation after
        // reopening, including the unknown-ACK retry case.
        let mut store = open();
        assert_eq!(
            store.discover(&discovery, &paths, true).unwrap(),
            SourceMutationReceipt::AlreadyApplied
        );
        assert_eq!(store.claims(&paths).unwrap(), initial_claims);

        let partial = vec!["conformance-partial".to_owned()];
        let partial_operation = discover_operation(namespace, &partial, false);
        assert_eq!(
            store.discover(&partial_operation, &partial, false).unwrap(),
            SourceMutationReceipt::Applied
        );
        let before_cas = store.claims(&partial).unwrap();
        let accepted_cas = invalidate_operation(namespace, &before_cas);
        assert_eq!(
            store.invalidate(&accepted_cas, &before_cas).unwrap(),
            SourceMutationReceipt::Applied
        );
        let after_cas = store.claims(&partial).unwrap();
        let failed_cas = invalidate_operation(namespace, &before_cas);
        assert_eq!(
            store.invalidate(&failed_cas, &before_cas).unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.claims(&partial).unwrap(), after_cas);

        let changed_paths = vec!["conformance-changed-payload".to_owned()];
        let mut changed_operation = discover_operation(namespace, &changed_paths, false);
        changed_operation.id = discovery.id.clone();
        let before_conflict = store.claims(&changed_paths).unwrap();
        assert_eq!(
            store
                .discover(&changed_operation, &changed_paths, false)
                .unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.claims(&changed_paths).unwrap(), before_conflict);

        let foreign_paths = vec!["conformance-foreign".to_owned()];
        let foreign_operation = discover_operation("foreign-namespace", &foreign_paths, true);
        assert_eq!(
            store
                .discover(&foreign_operation, &foreign_paths, true)
                .unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(
            store.claims(&foreign_paths).unwrap(),
            vec![(foreign_paths[0].clone(), None)]
        );

        // Namespace bytes are hashed as length-delimited UTF-8; a NUL never
        // enters the SQL identity or becomes a second operation boundary.
        let encoded = SourceOperation::capture(namespace, "discover", b"nul-safe");
        assert!(encoded.id.is_ascii());
        assert!(!encoded.id.as_bytes().contains(&0));
        assert_eq!(encoded.namespace.as_bytes(), namespace.as_bytes());

        // The same durable intent lifecycle runs in all three adapters.
        let intent_paths = vec!["durable-intent".to_owned()];
        let operation = discover_operation(namespace, &intent_paths, false);
        let intent = SourceIntent::Discover {
            operation: operation.clone(),
            paths: intent_paths.clone(),
            complete: false,
        };
        store.persist_intent(&intent).unwrap();
        store.persist_intent(&intent).unwrap();
        assert_eq!(store.pending_intents().unwrap(), vec![intent.clone()]);
        assert_eq!(
            store.acknowledge_intent(&operation),
            Err(StorageError::Conflict)
        );
        assert_eq!(
            intent.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::Applied
        );
        let durable_claim = store.claims(&intent_paths).unwrap();
        drop(store);
        let mut store = open();
        assert_eq!(store.pending_intents().unwrap(), vec![intent.clone()]);
        for _ in 0..2 {
            assert_eq!(
                intent.apply(store.as_mut()).unwrap(),
                SourceMutationReceipt::AlreadyApplied
            );
        }
        assert_eq!(store.claims(&intent_paths).unwrap(), durable_claim);
        let rebound_paths = vec!["durable-rebound".to_owned()];
        let mut rebound_operation = discover_operation(namespace, &rebound_paths, true);
        rebound_operation.id = operation.id.clone();
        let rebound = SourceIntent::Discover {
            operation: rebound_operation.clone(),
            paths: rebound_paths.clone(),
            complete: true,
        };
        assert_eq!(store.persist_intent(&rebound), Err(StorageError::Conflict));
        assert_eq!(
            rebound.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(
            store.acknowledge_intent(&rebound_operation),
            Err(StorageError::Conflict)
        );
        assert_eq!(store.pending_intents().unwrap(), vec![intent.clone()]);
        assert_eq!(
            store.claims(&rebound_paths).unwrap(),
            vec![(rebound_paths[0].clone(), None)]
        );
        store.acknowledge_intent(&operation).unwrap();
        assert!(store.pending_intents().unwrap().is_empty());
        let failed_expected = vec![(intent_paths[0].clone(), None)];
        let failed = SourceIntent::Invalidate {
            operation: invalidate_operation(namespace, &failed_expected),
            expected: failed_expected,
        };
        store.persist_intent(&failed).unwrap();
        assert_eq!(
            failed.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::Conflict
        );
        drop(store);
        let mut store = open();
        assert_eq!(
            failed.apply(store.as_mut()).unwrap(),
            SourceMutationReceipt::Conflict
        );
        store.acknowledge_intent(failed.operation()).unwrap();
        assert!(store.pending_intents().unwrap().is_empty());
        assert_eq!(store.claims(&intent_paths).unwrap(), durable_claim);

        // This sequence deliberately retains pending invalidations. Neither
        // profile may manufacture a successful consumer acknowledgement here.
        let before = store.backfill_state().unwrap();
        let finish = finish_operation(namespace, None);
        assert_eq!(
            store.finish_backfill(&finish, None).unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.backfill_state().unwrap(), before);
        let wrong_generation = finish_operation(namespace, Some("wrong-generation"));
        assert_eq!(
            store
                .finish_backfill(&wrong_generation, Some("wrong-generation"))
                .unwrap(),
            SourceMutationReceipt::Conflict
        );
        assert_eq!(store.backfill_state().unwrap(), before);
    }
}
