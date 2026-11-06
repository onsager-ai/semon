//! Namespace-bound, transactional state that cannot be rebuilt from local Raw.
//! No connection or credential discovery and no filesystem side effects.
use super::{Namespace, PgProjectionTransaction, StorageError, backend, ensure, number};
use crate::{CatalogChange, CatalogChangePage, CatalogReadScope, Facts};

impl PgProjectionTransaction<'_> {
    /// Store reported machine Facts under a monotonic host-supplied revision.
    /// A stale writer cannot replace a newer report, including after restart.
    pub(crate) async fn write_facts(
        &self,
        namespace: &Namespace,
        expected: Option<u64>,
        revision: u64,
        facts: &Facts,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        if facts.version != crate::FACTS_VERSION || expected.is_some_and(|old| old >= revision) {
            return Err(backend("invalid reported Facts revision or format"));
        }
        let text = serde_json::to_string(facts).map_err(backend)?;
        if text.len() > 1024 * 1024 {
            return Err(backend("reported Facts budget exceeded"));
        }
        let next = number(revision)?;
        let expected = expected.map(number).transpose()?;
        self.tx
            .query_one(
                "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
        let actual = self
            .tx
            .query_opt(
                "SELECT revision FROM semon_projection.reported_facts WHERE namespace=$1",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?
            .map(|row| row.get::<_, i64>(0));
        if actual != expected {
            return Ok(false);
        }
        self.tx.execute(
            "INSERT INTO semon_projection.reported_facts(namespace,revision,metadata) VALUES($1,$2,$3) ON CONFLICT(namespace) DO UPDATE SET revision=excluded.revision,metadata=excluded.metadata",
            &[&namespace.binding(), &next, &text],
        ).await.map_err(backend)?;
        Ok(true)
    }

    pub(crate) async fn reported_facts(
        &self,
        namespace: &Namespace,
    ) -> Result<Option<(u64, Facts)>, StorageError> {
        ensure(&self.namespace, namespace)?;
        self.tx
            .query_opt(
                "SELECT revision,metadata FROM semon_projection.reported_facts WHERE namespace=$1",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?
            .map(|row| {
                let facts: Facts = serde_json::from_str(row.get(1)).map_err(backend)?;
                if facts.version != crate::FACTS_VERSION {
                    return Err(backend("unsupported reported Facts"));
                }
                Ok((row.get::<_, i64>(0) as u64, facts))
            })
            .transpose()
    }

    /// A producer claim and its private staging checkpoint advance together.
    /// Every mutation compares the exact revision, preventing stale-stage ABA.
    pub(crate) async fn write_checkpoint(
        &self,
        namespace: &Namespace,
        key: &str,
        expected: Option<u64>,
        revision: u64,
        checkpoint: &str,
    ) -> Result<bool, StorageError> {
        ensure(&self.namespace, namespace)?;
        if key.is_empty()
            || key.len() > 4096
            || checkpoint.len() > 1024 * 1024
            || expected.is_some_and(|old| old >= revision)
        {
            return Err(backend("invalid custody checkpoint"));
        }
        let next = number(revision)?;
        let expected = expected.map(number).transpose()?;
        self.tx
            .query_one(
                "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
        let actual = self.tx.query_opt("SELECT revision FROM semon_projection.checkpoints WHERE namespace=$1 AND checkpoint_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?.map(|row| row.get::<_, i64>(0));
        if actual != expected {
            return Ok(false);
        }
        self.tx.execute(
            "INSERT INTO semon_projection.checkpoints(namespace,checkpoint_key,revision,metadata) VALUES($1,$2,$3,$4) ON CONFLICT(namespace,checkpoint_key) DO UPDATE SET revision=excluded.revision,metadata=excluded.metadata",
            &[&namespace.binding(), &key, &next, &checkpoint],
        ).await.map_err(backend)?;
        Ok(true)
    }

    pub(crate) async fn checkpoint(
        &self,
        namespace: &Namespace,
        key: &str,
    ) -> Result<Option<(u64, String)>, StorageError> {
        ensure(&self.namespace, namespace)?;
        Ok(self.tx.query_opt("SELECT revision,metadata FROM semon_projection.checkpoints WHERE namespace=$1 AND checkpoint_key=$2", &[&namespace.binding(), &key]).await.map_err(backend)?.map(|row| (row.get::<_, i64>(0) as u64, row.get(1))))
    }

    /// Publication calls this explicitly inside its transaction. There are no
    /// triggers: rollback removes both the mutation and its delivery hints.
    pub(super) async fn catalog_changed(
        &self,
        namespace: &Namespace,
        key: &str,
    ) -> Result<(), StorageError> {
        for scope in ["current", "retained_history"] {
            let revision: i64 = self.tx.query_one(
                "INSERT INTO semon_projection.change_heads(namespace,read_scope,head,floor) VALUES($1,$2,1,0) ON CONFLICT(namespace,read_scope) DO UPDATE SET head=semon_projection.change_heads.head+1 RETURNING head",
                &[&namespace.binding(), &scope],
            ).await.map_err(backend)?.get(0);
            self.tx.execute("INSERT INTO semon_projection.changes(namespace,read_scope,revision,session_key) VALUES($1,$2,$3,$4)", &[&namespace.binding(), &scope, &revision, &key]).await.map_err(backend)?;
        }
        Ok(())
    }

    pub(crate) async fn catalog_changes(
        &self,
        namespace: &Namespace,
        scope: CatalogReadScope,
        expected_epoch: Option<&str>,
        after: u64,
        limit: usize,
    ) -> Result<CatalogChangePage, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !(1..=256).contains(&limit) || (after > 0 && expected_epoch.is_none()) {
            return Err(backend("invalid change cursor"));
        }
        let after = number(after)?;
        let epoch: String = self
            .tx
            .query_one(
                "SELECT custody_epoch FROM semon_projection.namespaces WHERE namespace=$1",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?
            .get(0);
        if expected_epoch.is_some_and(|expected| expected != epoch) {
            return Err(backend("change stream epoch mismatch"));
        }
        let generation = self
            .catalog_generation(namespace)
            .await?
            .ok_or_else(|| backend("catalog not published"))?;
        let scope_key = crate::catalog_search::scope_key(scope);
        let head = self.tx.query_opt("SELECT head,floor FROM semon_projection.change_heads WHERE namespace=$1 AND read_scope=$2", &[&namespace.binding(), &scope_key]).await.map_err(backend)?;
        let (head, floor) = head.map_or((0, 0), |row| (row.get::<_, i64>(0), row.get::<_, i64>(1)));
        if after < floor || after > head {
            return Err(backend("change cursor outside retained stream"));
        }
        let rows = self.tx.query("SELECT revision,session_key FROM semon_projection.changes WHERE namespace=$1 AND read_scope=$2 AND revision>$3 ORDER BY revision LIMIT $4", &[&namespace.binding(), &scope_key, &after, &number(limit as u64)?]).await.map_err(backend)?;
        let changes: Vec<_> = rows
            .iter()
            .map(|row| CatalogChange {
                revision: row.get::<_, i64>(0) as u64,
                catalog_key: row.get(1),
                read_scope: scope,
            })
            .collect();
        let next = changes
            .last()
            .map_or(after as u64, |change| change.revision);
        Ok(CatalogChangePage {
            api: 1,
            source_key: namespace.as_str().into(),
            stream_epoch: epoch,
            source_generation: generation,
            read_scope: scope,
            head: head as u64,
            compacted_through: floor as u64,
            next_revision: next,
            caught_up: next == head as u64,
            changes,
        })
    }
}
