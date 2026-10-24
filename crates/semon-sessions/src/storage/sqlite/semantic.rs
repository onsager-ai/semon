//! Private admitted-Source staging. No Current publication qualification.
#[cfg(test)]
use super::source_journal::SourceJournal;
use super::{failed, namespace};
use crate::events::{Ledger, Outcome, StoreError};
use crate::model::{
    PreparedSources,
    relation_stage::{Batch, Context, Phase},
};
use crate::storage::{NativeReaderBinding, StorageError, StoreConfig};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// This capability binds the real admission and physical incarnation. Its fields
/// cannot be supplied by a caller or inferred from an Options path.
pub(crate) struct AdmittedSource {
    config: StoreConfig,
    admission: u64,
    generation: Option<String>,
}

impl AdmittedSource {
    pub(crate) fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        NativeReaderBinding::Admitted(self.config.clone()).validate(db, &self.config.namespace)?;
        crate::events::store::configured::validate_staging_owner(db, &self.config)?;
        if namespace::require_admitted(db, &self.config.namespace)?.generation != self.admission {
            return Err(StorageError::ScopeChanged);
        }
        let admission = i64::try_from(self.admission).map_err(|_| StorageError::ScopeChanged)?;
        let profile: bool = db.query_row("SELECT admission=?2 AND recipe=?3 FROM storage_semantic_profiles WHERE namespace=?1", params![self.config.namespace.as_str(),admission,crate::events::store::configured::semantic_recipe()], |r|r.get(0)).map_err(failed)?;
        if !profile {
            return Err(StorageError::NamespaceUpgradePending);
        }
        Ok(())
    }
}

pub(crate) struct Stage {
    db: Connection,
    source: AdmittedSource,
}

fn error(error: rusqlite::Error) -> StoreError {
    StoreError::Configured(failed(error))
}
fn head(db: &Connection) -> rusqlite::Result<Option<String>> {
    db.query_row(
        crate::events::store::configured::metadata_sql(
            db,
            "SELECT value FROM meta WHERE key='catalog_generation'",
            "SELECT value FROM meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
        )?,
        [],
        |r| r.get(0),
    )
    .optional()
}

impl Stage {
    #[cfg(test)]
    pub(crate) fn open(config: &StoreConfig) -> Result<Self, StorageError> {
        crate::events::store::configured::validate_semantic_schema(config)?;
        let mut db = SourceJournal(config.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        NativeReaderBinding::Admitted(config.clone()).validate(&tx, &config.namespace)?;
        crate::events::store::configured::validate_staging_owner(&tx, config)?;
        let admission = namespace::require_admitted(&tx, &config.namespace)?.generation;
        let admission_sql = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
        // The private profile must already have authored composite stage keys.
        // Opening never converts an existing singleton stage or native family.
        let composite: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('catalog_semantic_stage','main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [], |r| r.get(0)).map_err(failed)?;
        if !composite {
            return Err(StorageError::NamespaceUpgradePending);
        }
        tx.create_scalar_function(
            "semon_semantic_writer",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
            |_| Ok(1_i64),
        )
        .map_err(failed)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS storage_semantic_profiles(namespace TEXT PRIMARY KEY,admission INTEGER NOT NULL,recipe TEXT NOT NULL,epoch INTEGER NOT NULL) STRICT;").map_err(failed)?;
        for table in crate::events::store::configured::semantic_tables()
            .iter()
            .copied()
            .chain(["storage_semantic_profiles"])
        {
            for (operation, predicate) in [
                ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
                (
                    "UPDATE",
                    "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
                ),
                ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
            ] {
                tx.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS storage_semantic_guard_{table}_{operation} BEFORE {operation} ON {table} WHEN semon_semantic_writer()<>1 OR {predicate} BEGIN SELECT RAISE(ABORT,'semantic Source refused'); END;")).map_err(failed)?;
            }
        }
        let recipe = crate::events::store::configured::semantic_recipe();
        tx.execute(
            "INSERT OR IGNORE INTO storage_semantic_profiles VALUES(?1,?2,?3,0)",
            params![config.namespace.as_str(), admission_sql, recipe],
        )
        .map_err(failed)?;
        let valid: bool = tx.query_row("SELECT admission=?2 AND recipe=?3 FROM storage_semantic_profiles WHERE namespace=?1", params![config.namespace.as_str(),admission_sql,recipe], |r| r.get(0)).map_err(failed)?;
        if !valid {
            return Err(StorageError::NamespaceUpgradePending);
        }
        let generation = head(&tx).map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(Self {
            db,
            source: AdmittedSource {
                config: config.clone(),
                admission,
                generation,
            },
        })
    }

    pub(crate) fn generation(&self) -> Result<Option<String>, StoreError> {
        self.source
            .validate(&self.db)
            .map_err(StoreError::Configured)?;
        head(&self.db).map_err(error)
    }

    pub(crate) fn begin(&mut self, paths: &[String], runtime: &str) -> Result<Outcome, StoreError> {
        if paths.is_empty()
            || paths.len() > 64
            || runtime.len() > 65536
            || paths.iter().any(|p| p.is_empty() || p.len() > 4096)
            || paths
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != paths.len()
        {
            return Err(StoreError::Configured(StorageError::InvalidArguments));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        if head(&tx).map_err(error)? != self.source.generation {
            return Ok(Outcome::Conflict);
        }
        let ns = self.source.config.namespace.as_str();
        let existing: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT runtime,generation FROM main.catalog_semantic_stage WHERE namespace=?1",
                [ns],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(error)?;
        if let Some((_, generation)) = &existing
            && generation != &self.source.generation
        {
            return Ok(Outcome::Conflict);
        }
        if existing.as_ref().is_some_and(|(old, _)| old != runtime) {
            // A full reset/frontier replay is not yet ported. Refuse BEFORE any
            // writes rather than drop another epoch's durable witnesses.
            return Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ));
        }
        if existing.is_none() {
            if tx.execute("UPDATE storage_semantic_profiles SET epoch=epoch+1 WHERE namespace=?1 AND epoch<9223372036854775807", [ns]).map_err(error)? != 1 { return Ok(Outcome::Conflict); }
            tx.execute("INSERT INTO main.catalog_semantic_stage(namespace,singleton,epoch,phase,generation,runtime,revision) SELECT ?1,1,epoch,'indexing',?2,?3,(SELECT revision FROM storage_source_journal WHERE namespace=?1) FROM storage_semantic_profiles WHERE namespace=?1", params![ns,self.source.generation,runtime]).map_err(error)?;
        }
        let claims = super::source_journal::claims(&tx, Some(ns), paths).map_err(error)?;
        for (path, claim) in claims {
            tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_sources(namespace,source_path,claim) VALUES(?1,?2,?3)", params![ns,path,claim]).map_err(error)?;
            tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,'pathowner',?2,'')", params![ns,path]).map_err(error)?;
        }
        tx.commit().map_err(error)?;
        Ok(Outcome::Written)
    }

    pub(crate) fn batch(&mut self, limit: usize) -> Result<Option<Batch>, StoreError> {
        let tx = self.db.transaction().map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.config.namespace.as_str();
        let stage: Option<(i64,String,Option<String>,String)> = tx.query_row("SELECT epoch,phase,generation,runtime FROM main.catalog_semantic_stage WHERE namespace=?1", [ns], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(error)?;
        let Some((epoch, phase, base_generation, runtime)) = stage else {
            return Ok(None);
        };
        if head(&tx).map_err(error)? != base_generation || base_generation != self.source.generation
        {
            return Err(StoreError::Configured(StorageError::Conflict));
        }
        if phase != "indexing" {
            return Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ));
        }
        let paths = tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=0 ORDER BY source_path LIMIT ?2").map_err(error)?.query_map(params![ns,limit.clamp(1,64) as i64], |r| r.get(0)).map_err(error)?.collect::<rusqlite::Result<Vec<String>>>().map_err(error)?;
        if paths.is_empty() {
            // Namespace-scoped event paging/native-key/frontier reduction is
            // the next port; never certify identity/publication from this page.
            return Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ));
        }
        let source_revision = tx
            .query_row(
                "SELECT revision FROM storage_source_journal WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(error)?;
        Ok(Some(Batch {
            epoch,
            phase: Phase::Indexing,
            paths,
            base_generation,
            source_revision,
            runtime,
            context: Context::default(),
            relation_cursor: None,
        }))
    }

    pub(crate) fn record(
        &mut self,
        batch: &Batch,
        prepared: &PreparedSources,
    ) -> Result<Outcome, StoreError> {
        if batch.phase != Phase::Indexing
            || batch.paths.len() > 64
            || !prepared.withdrawals.is_empty()
            || !prepared.rows.is_empty()
            || !prepared.transcripts.is_empty()
            || !prepared.pids.is_empty()
            || !prepared.relationships.endpoints.is_empty()
            || !prepared.relationships.relations.is_empty()
            || !prepared.indexed_native_parents.is_empty()
            || !prepared.lineage_dependencies.is_empty()
        {
            return Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.config.namespace.as_str();
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND phase='indexing' AND runtime=?3 AND generation IS ?4)", params![ns,batch.epoch,batch.runtime,batch.base_generation], |r| r.get(0)).map_err(error)?;
        if !valid
            || prepared.base_generation != batch.base_generation
            || head(&tx).map_err(error)? != batch.base_generation
            || self.source.generation != batch.base_generation
        {
            return Ok(Outcome::Conflict);
        }
        let claims = super::source_journal::claims(&tx, Some(ns), &batch.paths).map_err(error)?;
        if claims != prepared.claims || prepared.indexed_sources.len() != batch.paths.len() {
            return Ok(Outcome::Conflict);
        }
        let mut seen = std::collections::BTreeSet::new();
        for (_, source) in &prepared.indexed_sources {
            let path = source.path.to_string_lossy().into_owned();
            if !batch.paths.contains(&path) || !seen.insert(path.clone()) {
                return Ok(Outcome::Conflict);
            }
            let Some((_, ledger)) = crate::events::store::ledger_row(&tx, &path).map_err(error)?
            else {
                return Ok(Outcome::Conflict);
            };
            let mut expected = ledger.source(&source.path, &source.native_id);
            expected.native_role = source.native_role.clone();
            if expected != *source
                || crate::events::Stat::of(&source.path).ok().as_ref() != Some(&ledger.stat)
            {
                return Ok(Outcome::Conflict);
            }
            if !crate::events::store::admitted_roles_current(
                &tx,
                std::slice::from_ref(source),
                &self.source,
            )
            .map_err(error)?
            {
                return Ok(Outcome::Conflict);
            }
            let captured: Option<Option<i64>> = tx.query_row("SELECT claim FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND index_done=0", params![ns,path], |r| r.get(0)).optional().map_err(error)?;
            if captured != claims.iter().find(|(p, _)| p == &path).map(|(_, c)| *c) {
                return Ok(Outcome::Conflict);
            }
        }
        for (harness, source) in &prepared.indexed_sources {
            let path = source.path.to_string_lossy();
            let metadata =
                serde_json::to_string(source).map_err(|e| StoreError::Data(e.to_string()))?;
            if metadata.len() > 1048576 {
                return Err(StoreError::Configured(StorageError::InvalidArguments));
            }
            let ledger: Ledger = crate::events::store::ledger_row(&tx, path.as_ref())
                .map_err(error)?
                .ok_or(StoreError::Configured(StorageError::Conflict))?
                .1;
            let ledger =
                serde_json::to_string(&ledger).map_err(|e| StoreError::Data(e.to_string()))?;
            let top = source
                .native_role
                .as_ref()
                .is_some_and(|r| r.get("Top").is_some());
            tx.execute("INSERT INTO main.catalog_semantic_inputs(namespace,source_path,harness,native_id,is_top,metadata) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(namespace,source_path) DO UPDATE SET harness=excluded.harness,native_id=excluded.native_id,is_top=excluded.is_top,metadata=excluded.metadata", params![ns,path.as_ref(),harness,source.native_id,top,metadata]).map_err(error)?;
            tx.execute("UPDATE main.catalog_semantic_sources SET ledger=?3,index_done=1,(dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256)=(SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM main.files WHERE namespace=?1 AND path=?2) WHERE namespace=?1 AND source_path=?2", params![ns,path.as_ref(),ledger]).map_err(error)?;
            for kind in ["native", "nativeall", "headerid", "headerparent"] {
                tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,?2,?3,?4)", params![ns,kind,source.native_id,if kind=="native" {harness.as_str()} else {""}]).map_err(error)?;
            }
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_record").map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(Outcome::Written)
    }
}
