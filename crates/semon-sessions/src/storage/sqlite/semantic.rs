//! Private admitted-Source staging and bounded initial Current publication.
//! Public and complete-domain publication qualifications remain closed.
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
    owner: Option<crate::events::store::configured::NativeOwnership>,
    admission: u64,
    generation: Option<String>,
}

impl AdmittedSource {
    pub(super) fn namespace(&self) -> &str {
        self.config.namespace.as_str()
    }
    pub(super) fn validate_publication(&self, db: &Connection) -> Result<(), StorageError> {
        if self.owner.is_none() {
            return Err(StorageError::NamespaceUpgradePending);
        }
        self.validate(db)
    }
    pub(crate) fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        if let Some(owner) = &self.owner {
            owner.validate_ready(db)?;
        } else {
            NativeReaderBinding::Admitted(self.config.clone())
                .validate(db, &self.config.namespace)?;
            crate::events::store::configured::validate_staging_owner(db, &self.config)?;
            if namespace::require_admitted(db, &self.config.namespace)?.generation != self.admission
            {
                return Err(StorageError::ScopeChanged);
            }
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
    reduction: bool,
    epoch: Option<i64>,
}

/// Internal runtime qualification, deliberately absent from public configuration.
/// It qualifies primitive Codex reduction; composite ownership also admits the
/// bounded initial Current publisher. No public configuration constructs it.
pub(crate) struct SemanticStaging {
    config: StoreConfig,
    reduction: bool,
    owner: Option<crate::events::store::configured::NativeOwnership>,
}

impl SemanticStaging {
    /// Explicit private admission into the authored composite native profile.
    #[allow(dead_code)]
    pub(crate) fn admit(config: &StoreConfig) -> Result<Self, StorageError> {
        let owner = crate::events::store::configured::NativeOwnership::admit(config)?;
        let db = SourceJournal::owned(owner.clone()).connection()?;
        db.execute_batch("BEGIN IMMEDIATE").map_err(failed)?;
        owner.validate_ready(&db)?;
        let composite: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('catalog_semantic_stage','main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [], |r|r.get(0)).map_err(failed)?;
        if !composite {
            return Err(StorageError::NamespaceUpgradePending);
        }
        db.execute_batch("COMMIT")
            .map_err(|_| StorageError::CommitUncertain)?;
        Ok(Self {
            config: config.clone(),
            reduction: true,
            owner: Some(owner),
        })
    }
    pub(super) fn journal(&self) -> SourceJournal {
        self.owner
            .as_ref()
            .map(|owner| SourceJournal::owned(owner.clone()))
            .unwrap_or_else(|| SourceJournal(self.config.clone()))
    }
    pub(super) fn writer(&self) -> crate::events::store::configured::Writer {
        self.owner
            .as_ref()
            .map(|owner| crate::events::store::configured::Writer::owned(owner.clone()))
            .unwrap_or_else(|| crate::events::store::configured::Writer::new(&self.config))
    }
    pub(crate) fn validate(config: &StoreConfig) -> Result<Self, StorageError> {
        crate::events::store::configured::validate_semantic_schema(config)?;
        let mut db = SourceJournal(config.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        NativeReaderBinding::Admitted(config.clone()).validate(&tx, &config.namespace)?;
        crate::events::store::configured::validate_staging_owner(&tx, config)?;
        let composite: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('catalog_semantic_stage','main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [], |r|r.get(0)).map_err(failed)?;
        if !composite {
            return Err(StorageError::NamespaceUpgradePending);
        }
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(Self {
            config: config.clone(),
            reduction: true,
            owner: None,
        })
    }
    pub(super) fn primitive(config: &StoreConfig) -> Result<Self, StorageError> {
        let mut capability = Self::validate(config)?;
        capability.reduction = false;
        Ok(capability)
    }
}

fn refuse(reason: &'static str) -> StoreError {
    StoreError::Data(reason.into())
}

// The admitted transaction has already fenced epoch/generation before these
// namespace-owned physical operations. Claims are observations, never ACKs.
fn reset(c: &Connection, ns: &str, runtime: &str) -> rusqlite::Result<()> {
    if c.execute("UPDATE storage_semantic_profiles SET epoch=epoch+1 WHERE namespace=?1 AND epoch<9223372036854775807", [ns])? != 1 { return Err(rusqlite::Error::InvalidQuery); }
    c.execute("UPDATE main.catalog_semantic_stage SET epoch=(SELECT epoch FROM storage_semantic_profiles WHERE namespace=?1),phase='indexing',runtime=?2,resetting=1,reset_after='',key_after_harness='',key_after_native='',keys_done=0,key_candidate='',key_epoch=key_epoch+1,withdrawal_authority=NULL WHERE namespace=?1", params![ns,runtime])?;
    Ok(())
}

fn reset_page(c: &Connection, ns: &str) -> rusqlite::Result<bool> {
    let mut budget = 64usize;
    for (table, key) in [
        ("catalog_semantic_inputs", "source_path"),
        ("catalog_semantic_keys", "harness,native_id"),
        ("catalog_semantic_relations", "relation_id"),
        ("catalog_semantic_events", "source_path,event_key"),
        ("catalog_semantic_endpoints", "session_key"),
        ("catalog_semantic_native", "harness,native_id,session_key"),
        ("catalog_semantic_owner_sources", "session_key,source_path"),
        ("catalog_semantic_paths", "path,session_key"),
        ("catalog_semantic_names", "name,session_key"),
        ("catalog_semantic_frontier", "kind,value,harness"),
    ] {
        if budget == 0 {
            return Ok(false);
        }
        budget-=c.execute(&format!("DELETE FROM main.{table} WHERE namespace=?1 AND ({key}) IN (SELECT {key} FROM main.{table} WHERE namespace=?1 ORDER BY {key} LIMIT ?2)"), params![ns,budget as i64])?;
    }
    if budget == 0 {
        return Ok(false);
    }
    let after: String = c.query_row(
        "SELECT reset_after FROM main.catalog_semantic_stage WHERE namespace=?1",
        [ns],
        |r| r.get(0),
    )?;
    let paths=c.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path>?2 ORDER BY source_path LIMIT ?3")?.query_map(params![ns,after,budget as i64], |r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for path in &paths {
        c.execute("UPDATE main.catalog_semantic_sources SET claim=(SELECT revision FROM main.session_catalog_invalidations WHERE namespace=?1 AND source_path=?2),ledger=NULL,withdrawal=NULL,index_done=0,event_after=-1,events_done=0,identity_done=0,relation_done=0,publication_done=0,ack_done=0,relation_after='' WHERE namespace=?1 AND source_path=?2", params![ns,path])?;
    }
    if let Some(last) = paths.last() {
        c.execute(
            "UPDATE main.catalog_semantic_stage SET reset_after=?2 WHERE namespace=?1",
            params![ns, last],
        )?;
    }
    if paths.len() == budget {
        return Ok(false);
    }
    c.execute("UPDATE main.catalog_semantic_stage SET resetting=0,reset_after='',revision=(SELECT revision FROM storage_source_journal WHERE namespace=?1) WHERE namespace=?1", [ns])?;
    Ok(true)
}

fn reduction_witnesses(c: &Connection, ns: &str) -> rusqlite::Result<bool> {
    let rows=c.prepare("SELECT source_path,claim,ledger FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=1 ORDER BY source_path LIMIT 65")?.query_map([ns], |r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    // A bounded reducer cannot certify an unbounded component. Native owner
    // closure qualification remains a separate, explicitly limited port.
    if rows.len() > 64 {
        return Ok(false);
    }
    for (path, claim, recorded) in rows {
        let actual = super::source_journal::claims(c, Some(ns), std::slice::from_ref(&path))?[0].1;
        let ledger = crate::events::store::ledger_row(c, &path)?
            .map(|(_, l)| serde_json::to_string(&l))
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        if actual != claim || ledger != recorded {
            return Ok(false);
        }
    }
    Ok(true)
}

fn event_page(c: &Connection, ns: &str) -> Result<bool, StoreError> {
    let unsupported: bool=c.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_inputs WHERE namespace=?1 AND harness<>'codex')",[ns],|r|r.get(0)).map_err(error)?;
    if unsupported {
        return Err(refuse("semantic_nonprimitive_event_harness"));
    }
    // The existing shared event reducer emits relationship-event rows only
    // for Claude. Primitive Codex has no such rows; its native events stay in
    // the original Index, rather than inventing a second event projection.
    c.execute("UPDATE main.catalog_semantic_sources SET events_done=1 WHERE namespace=?1 AND source_path IN (SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=1 AND events_done=0 ORDER BY source_path LIMIT 64)",[ns]).map_err(error)?;
    c.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=1 AND events_done=0)",[ns],|r|r.get(0)).map_err(error)
}

fn native_keys_page(c: &Connection, ns: &str) -> Result<bool, StoreError> {
    let pairs=c.prepare("SELECT harness,native_id FROM main.catalog_semantic_inputs WHERE namespace=?1 GROUP BY harness,native_id ORDER BY harness,native_id LIMIT 65").map_err(error)?.query_map([ns],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
    if pairs.len() > 64 {
        return Err(refuse("semantic_native_key_component_budget"));
    }
    for (harness, native) in pairs {
        let collision: bool=c.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_inputs WHERE namespace=?1 AND native_id=?2 AND harness<>?3 UNION ALL SELECT 1 FROM main.session_catalog_sources WHERE namespace=?1 AND native_id=?2 AND harness<>?3 AND lifecycle='current')",params![ns,native,harness],|r|r.get(0)).map_err(error)?;
        if collision {
            return Err(refuse("semantic_cross_harness_native_key_pending"));
        }
        let key = crate::events::store::configured::primitive_native_key(&native);
        c.execute("INSERT INTO main.catalog_semantic_keys(namespace,harness,native_id,session_key,key_epoch) SELECT ?1,?2,?3,?4,key_epoch FROM main.catalog_semantic_stage WHERE namespace=?1 ON CONFLICT(namespace,harness,native_id) DO UPDATE SET session_key=excluded.session_key,key_epoch=excluded.key_epoch",params![ns,harness,native,key]).map_err(error)?;
    }
    c.execute(
        "UPDATE main.catalog_semantic_stage SET keys_done=1 WHERE namespace=?1",
        [ns],
    )
    .map_err(error)?;
    Ok(true)
}

fn frontier_page(c: &Connection, ns: &str) -> Result<bool, StoreError> {
    let mut budget = 64usize;
    while budget > 0 {
        let task: Option<(String,String,String,String)>=c.query_row("SELECT kind,value,harness,after_key FROM main.catalog_semantic_frontier WHERE namespace=?1 AND done=0 ORDER BY kind,value,harness LIMIT 1",[ns],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(error)?;
        let Some((kind, value, harness, after)) = task else {
            return Ok(true);
        };
        let sql = match kind.as_str() {
            "pathowner" => {
                "SELECT source_path FROM main.session_catalog_sources WHERE namespace=?1 AND source_path=?2 AND lifecycle='current' AND source_path>?3 ORDER BY source_path LIMIT ?4"
            }
            "native" => {
                "SELECT source_path FROM main.session_catalog_sources WHERE namespace=?1 AND native_id=?2 AND source_path>?3 AND lifecycle='current' AND harness=?5 GROUP BY source_path ORDER BY source_path LIMIT ?4"
            }
            "nativeall" => {
                "SELECT source_path FROM main.session_catalog_sources WHERE namespace=?1 AND native_id=?2 AND source_path>?3 AND lifecycle='current' GROUP BY source_path ORDER BY source_path LIMIT ?4"
            }
            "headerid" => {
                if crate::events::store::configured::metadata_sql(c, "", "namespace")
                    .map_err(error)?
                    == "namespace"
                {
                    "SELECT substr(m.key,length('native_codex_header:')+1) FROM main.meta m WHERE m.namespace=?1 AND m.key GLOB 'native_codex_header:*' AND json_valid(m.value) AND json_extract(m.value,'$.header.id')=?2 AND substr(m.key,length('native_codex_header:')+1)>?3 ORDER BY m.key LIMIT ?4"
                } else {
                    "SELECT substr(m.key,length('native_codex_header:')+1) FROM main.meta m JOIN storage_event_tables owner ON owner.namespace=?1 AND owner.table_name='meta' AND owner.qualification=0 WHERE m.key GLOB 'native_codex_header:*' AND json_valid(m.value) AND json_extract(m.value,'$.header.id')=?2 AND substr(m.key,length('native_codex_header:')+1)>?3 ORDER BY m.key LIMIT ?4"
                }
            }
            "headerparent" => {
                if crate::events::store::configured::metadata_sql(c, "", "namespace")
                    .map_err(error)?
                    == "namespace"
                {
                    "SELECT substr(m.key,length('native_codex_header:')+1) FROM main.meta m WHERE m.namespace=?1 AND m.key GLOB 'native_codex_header:*' AND json_valid(m.value) AND json_extract(m.value,'$.header.metadata.parent_thread')=?2 AND substr(m.key,length('native_codex_header:')+1)>?3 ORDER BY m.key LIMIT ?4"
                } else {
                    "SELECT substr(m.key,length('native_codex_header:')+1) FROM main.meta m JOIN storage_event_tables owner ON owner.namespace=?1 AND owner.table_name='meta' AND owner.qualification=0 WHERE m.key GLOB 'native_codex_header:*' AND json_valid(m.value) AND json_extract(m.value,'$.header.metadata.parent_thread')=?2 AND substr(m.key,length('native_codex_header:')+1)>?3 ORDER BY m.key LIMIT ?4"
                }
            }
            _ => return Err(refuse("semantic_nonprimitive_frontier_pending")),
        };
        let mut q = c.prepare(sql).map_err(error)?;
        q.raw_bind_parameter(1, ns).map_err(error)?;
        q.raw_bind_parameter(2, &value).map_err(error)?;
        q.raw_bind_parameter(3, &after).map_err(error)?;
        q.raw_bind_parameter(4, budget as i64).map_err(error)?;
        if q.parameter_count() == 5 {
            q.raw_bind_parameter(5, &harness).map_err(error)?;
        }
        let mut rows = q.raw_query();
        let mut paths = Vec::new();
        while let Some(row) = rows.next().map_err(error)? {
            paths.push(row.get::<_, String>(0).map_err(error)?);
        }
        drop(rows);
        drop(q);
        if kind == "pathowner" && !paths.is_empty() {
            return Err(refuse("semantic_existing_owner_closure_pending"));
        }
        let claims = super::source_journal::claims(c, Some(ns), &paths).map_err(error)?;
        for (path, claim) in claims {
            c.execute("INSERT OR IGNORE INTO main.catalog_semantic_sources(namespace,source_path,claim) VALUES(?1,?2,?3)",params![ns,path,claim]).map_err(error)?;
        }
        let last = paths.last().unwrap_or(&after);
        c.execute("UPDATE main.catalog_semantic_frontier SET after_key=?5,done=?6 WHERE namespace=?1 AND kind=?2 AND value=?3 AND harness=?4",params![ns,kind,value,harness,last,paths.len()<budget]).map_err(error)?;
        budget = budget.saturating_sub(paths.len().max(1));
    }
    c.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.catalog_semantic_frontier WHERE namespace=?1 AND done=0)",[ns],|r|r.get(0)).map_err(error)
}

struct ScopedReduction<'a> {
    db: &'a Connection,
    ns: &'a str,
}
impl crate::events::store::semantic_stage::ReductionStore for ScopedReduction<'_> {
    fn parents(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND parent_resolved=0 ORDER BY parent_native,session_key LIMIT ?2")?.query_map(params![self.ns,limit as i64],|r|r.get(0))?.collect()
    }
    fn parent(&self, key: &str) -> rusqlite::Result<()> {
        let c = self.db;
        let ns = self.ns;
        let ambiguous:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_native own JOIN main.catalog_semantic_native peer ON peer.namespace=own.namespace AND peer.harness=own.harness AND peer.native_id=own.native_id WHERE own.namespace=?1 AND own.session_key=?2 AND peer.session_key<>?2)",params![ns,key],|r|r.get(0))?;
        if ambiguous {
            return Err(rusqlite::Error::InvalidQuery);
        }
        c.execute("UPDATE main.catalog_semantic_endpoints SET parent_key=COALESCE((SELECT session_key FROM main.catalog_semantic_native n WHERE n.namespace=?1 AND n.harness=COALESCE(catalog_semantic_endpoints.parent_native_harness,catalog_semantic_endpoints.harness) AND n.native_id=catalog_semantic_endpoints.parent_native),parent_key),parent_resolved=1,family_cost=cost WHERE namespace=?1 AND session_key=?2",params![ns,key])?;
        c.execute("UPDATE main.catalog_semantic_endpoints SET root_key=CASE WHEN parent_key IS NULL OR NOT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints p WHERE p.namespace=?1 AND p.session_key=catalog_semantic_endpoints.parent_key) THEN session_key ELSE (SELECT root_key FROM main.catalog_semantic_endpoints p WHERE p.namespace=?1 AND p.session_key=catalog_semantic_endpoints.parent_key) END WHERE namespace=?1 AND session_key=?2",params![ns,key])?;
        c.execute("UPDATE main.catalog_semantic_paths SET root_key=(SELECT root_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2) WHERE namespace=?1 AND session_key=?2",params![ns,key])?;
        c.execute("UPDATE main.catalog_semantic_endpoints SET children_pending=children_pending+1 WHERE namespace=?1 AND session_key=(SELECT parent_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2)",params![ns,key])?;
        Ok(())
    }
    fn roots(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT e.session_key FROM main.catalog_semantic_endpoints e WHERE e.namespace=?1 AND e.root_key IS NULL AND EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints p WHERE p.namespace=?1 AND p.session_key=e.parent_key AND p.root_key IS NOT NULL) ORDER BY e.session_key LIMIT ?2")?.query_map(params![self.ns,limit as i64],|r|r.get(0))?.collect()
    }
    fn root(&self, key: &str) -> rusqlite::Result<()> {
        self.db.execute("UPDATE main.catalog_semantic_endpoints SET root_key=(SELECT root_key FROM main.catalog_semantic_endpoints p WHERE p.namespace=?1 AND p.session_key=catalog_semantic_endpoints.parent_key) WHERE namespace=?1 AND session_key=?2",params![self.ns,key])?;
        self.db.execute("UPDATE main.catalog_semantic_paths SET root_key=(SELECT root_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2) WHERE namespace=?1 AND session_key=?2",params![self.ns,key])?;
        Ok(())
    }
    fn missing_roots(&self) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND root_key IS NULL)",[self.ns],|r|r.get(0))
    }
    fn leaves(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND cost_done=0 AND children_pending=0 ORDER BY session_key LIMIT ?2")?.query_map(params![self.ns,limit as i64],|r|r.get(0))?.collect()
    }
    fn unfinished_costs(&self) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND cost_done=0)",[self.ns],|r|r.get(0))
    }
    fn cost(&self, key: &str) -> rusqlite::Result<()> {
        let(parent,cost):(Option<String>,Option<f64>)=self.db.query_row("SELECT parent_key,family_cost FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",params![self.ns,key],|r|Ok((r.get(0)?,r.get(1)?)))?;
        self.db.execute("UPDATE main.catalog_semantic_endpoints SET cost_done=1 WHERE namespace=?1 AND session_key=?2",params![self.ns,key])?;
        if let Some(parent) = parent {
            self.db.execute("UPDATE main.catalog_semantic_endpoints SET children_pending=children_pending-1,family_cost=CASE WHEN family_cost IS NULL OR ?3 IS NULL THEN NULL ELSE family_cost+?3 END WHERE namespace=?1 AND session_key=?2 AND cost_done=0 AND children_pending>0",params![self.ns,parent,cost])?;
        }
        Ok(())
    }
    fn complete(&self) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND (parent_resolved=0 OR root_key IS NULL OR cost_done=0))",[self.ns],|r|r.get(0))
    }
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
    pub(crate) fn publish(
        &mut self,
        batch: &Batch,
        prepared: &PreparedSources,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        use crate::storage::catalog::CatalogPublicationHead;
        use sha2::{Digest, Sha256};
        if batch.phase != Phase::Publication || batch.base_generation != prepared.base_generation {
            return Ok((Outcome::Conflict, None));
        }
        if prepared.rows.len() > 64 || prepared.claims.len() > 64 || prepared.rows.is_empty() {
            return Err(refuse("semantic_publication_component_budget"));
        }
        if !prepared.transcripts.is_empty()
            || !prepared.pids.is_empty()
            || !prepared.withdrawals.is_empty()
            || !prepared.lineage_dependencies.is_empty()
            || !prepared.indexed_native_parents.is_empty()
            || !prepared.relationships.relations.is_empty()
        {
            return Err(refuse("semantic_publication_nonprimitive_owners_pending"));
        }
        let metadata = prepared
            .rows
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Data(e.to_string()))?;
        if metadata.iter().map(String::len).sum::<usize>() > 1048576 {
            return Err(refuse("semantic_publication_metadata_budget"));
        }
        let claims_text =
            serde_json::to_string(&prepared.claims).map_err(|e| StoreError::Data(e.to_string()))?;
        let metadata_text =
            serde_json::to_string(&metadata).map_err(|e| StoreError::Data(e.to_string()))?;
        let proof_digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    batch.epoch,
                    &batch.runtime,
                    &batch.base_generation,
                    &claims_text,
                    &metadata_text
                ))
                .map_err(|e| StoreError::Data(e.to_string()))?
            )
        );
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source
            .validate_publication(&tx)
            .map_err(StoreError::Configured)?;
        let ns = self.source.namespace();
        let publication_head =
            super::publication::admitted_head(&tx, &self.source).map_err(StoreError::Configured)?;
        let previous = publication_head
            .base_generation()
            .map_err(StoreError::Configured)?;
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND phase='publication' AND runtime=?3 AND resetting=0)",params![ns,batch.epoch,batch.runtime],|r|r.get(0)).map_err(error)?;
        if !valid || self.epoch != Some(batch.epoch) {
            return Ok((Outcome::Conflict, None));
        }
        let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM storage_event_operations WHERE namespace=?1 AND state='intent')",[ns],|r|r.get(0)).map_err(error)?;
        if pending {
            return Err(refuse("semantic_publication_recovery_intent_pending"));
        }
        if !reduction_witnesses(&tx, ns).map_err(error)? {
            return Ok((Outcome::Conflict, None));
        }
        for source in prepared.rows.iter().flat_map(|row| &row.sources) {
            let Some((_, ledger)) =
                crate::events::store::ledger_row(&tx, source.path.to_string_lossy().as_ref())
                    .map_err(error)?
            else {
                return Ok((Outcome::Conflict, None));
            };
            if crate::events::Stat::of(&source.path).ok().as_ref() != Some(&ledger.stat)
                || !crate::events::store::admitted_roles_current(
                    &tx,
                    std::slice::from_ref(source),
                    &self.source,
                )
                .map_err(error)?
            {
                return Ok((Outcome::Conflict, None));
            }
        }
        let replay:Option<(String,String)>=tx.query_row("SELECT generation,proof_digest FROM storage_semantic_publications WHERE namespace=?1 AND epoch=?2",params![ns,batch.epoch],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(error)?;
        if let Some((generation, digest)) = replay {
            if previous.as_deref() == Some(generation.as_str())
                && digest == proof_digest
                && super::source_journal::claims(&tx, Some(ns), &batch.paths)
                    .map_err(error)?
                    .iter()
                    .all(|(_, claim)| claim.is_none())
            {
                return Ok((Outcome::Written, Some(generation)));
            }
            return Ok((Outcome::Conflict, None));
        }
        if previous != batch.base_generation || self.source.generation != batch.base_generation {
            return Ok((Outcome::Conflict, None));
        }
        let revision: i64 = tx
            .query_row(
                "SELECT revision FROM storage_source_journal WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(error)?;
        if revision != batch.source_revision {
            return Ok((Outcome::Conflict, None));
        }
        let staged:Vec<String>=tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 ORDER BY source_path LIMIT 65").map_err(error)?.query_map([ns],|r|r.get(0)).map_err(error)?.collect::<rusqlite::Result<_>>().map_err(error)?;
        let mut requested = batch.paths.clone();
        requested.sort();
        if staged != requested
            || prepared
                .claims
                .iter()
                .map(|(p, _)| p.clone())
                .collect::<Vec<_>>()
                != staged
            || super::source_journal::claims(&tx, Some(ns), &staged).map_err(error)?
                != prepared.claims
        {
            return Ok((Outcome::Conflict, None));
        }
        let unfinished:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND (index_done=0 OR events_done=0 OR identity_done=0 OR relation_done=0)) OR EXISTS(SELECT 1 FROM main.catalog_semantic_frontier WHERE namespace=?1 AND done=0) OR EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND (parent_resolved=0 OR root_key IS NULL OR cost_done=0))",[ns],|r|r.get(0)).map_err(error)?;
        if unfinished {
            return Ok((Outcome::Conflict, None));
        }
        let mut covered = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        let mut unchanged = true;
        for (row, encoded) in prepared.rows.iter().zip(&metadata) {
            if !keys.insert(&row.key) || row.sources.is_empty() {
                return Ok((Outcome::Conflict, None));
            }
            let endpoint:Option<String>=tx.query_row("SELECT metadata FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",params![ns,row.key],|r|r.get(0)).optional().map_err(error)?;
            let Some(endpoint) = endpoint else {
                return Ok((Outcome::Conflict, None));
            };
            let endpoint: crate::model::relation_stage::Endpoint =
                serde_json::from_str(&endpoint).map_err(|e| StoreError::Data(e.to_string()))?;
            // This bounded publisher admits the exact staged primitive row.
            // Final transcript/query/slot derivation remains a separate port.
            if serde_json::to_string(&endpoint.row).map_err(|e| StoreError::Data(e.to_string()))?
                != *encoded
            {
                return Ok((Outcome::Conflict, None));
            }
            let old:Option<String>=tx.query_row("SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",params![ns,row.key],|r|r.get(0)).optional().map_err(error)?;
            unchanged &= old.as_ref() == Some(encoded);
            if old.as_ref().is_some_and(|old| old != encoded) {
                return Err(refuse("semantic_publication_retained_history_pending"));
            }
            for source in &row.sources {
                let path = source.path.to_string_lossy().into_owned();
                if !staged.contains(&path) {
                    return Ok((Outcome::Conflict, None));
                }
                let Some((_, ledger)) =
                    crate::events::store::ledger_row(&tx, &path).map_err(error)?
                else {
                    return Ok((Outcome::Conflict, None));
                };
                let mut observed = ledger.source(&source.path, &source.native_id);
                observed.native_role = source.native_role.clone();
                if observed != *source
                    || crate::events::Stat::of(&source.path).ok().as_ref() != Some(&ledger.stat)
                    || !crate::events::store::admitted_roles_current(
                        &tx,
                        std::slice::from_ref(source),
                        &self.source,
                    )
                    .map_err(error)?
                {
                    return Ok((Outcome::Conflict, None));
                }
                covered.insert(path);
            }
        }
        if covered.into_iter().collect::<Vec<_>>() != staged {
            return Ok((Outcome::Conflict, None));
        }
        // All stale/scope/Raw/claim checks precede the first durable write.
        let old_epoch:Option<String>=tx.query_row("SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_publication_epoch'",[ns],|r|r.get(0)).optional().map_err(error)?;
        let repeat = unchanged
            && previous.is_some()
            && prepared.claims.iter().all(|(_, claim)| claim.is_none());
        let old_epoch = old_epoch
            .map(|s| s.parse::<u64>())
            .transpose()
            .map_err(|_| refuse("semantic_publication_epoch"))?
            .unwrap_or(0);
        let epoch = if repeat {
            old_epoch
        } else {
            old_epoch
                .checked_add(1)
                .ok_or_else(|| refuse("semantic_publication_epoch"))?
        }
        .to_string();
        let version = crate::model::summary::CATALOG_VERSION.to_string();
        let generation = if repeat {
            previous
                .clone()
                .ok_or_else(|| refuse("semantic_publication_replay_head"))?
        } else {
            crate::storage::catalog::component_generation(
                &version,
                previous.as_deref(),
                &epoch,
                &metadata,
            )
        };
        for (row, encoded) in prepared.rows.iter().zip(&metadata) {
            tx.execute("INSERT INTO main.session_catalog(namespace,session_key,lifecycle,last_ms,harness,repo,parent_key,metadata) VALUES(?1,?2,'current',?3,?4,?5,?6,?7) ON CONFLICT(namespace,session_key) DO UPDATE SET lifecycle='current',last_ms=excluded.last_ms,harness=excluded.harness,repo=excluded.repo,parent_key=excluded.parent_key,metadata=excluded.metadata WHERE session_catalog.metadata IS NOT excluded.metadata",params![ns,row.key,row.last.unwrap_or(0),row.harness,row.repo,row.parent,encoded]).map_err(error)?;
            for source in &row.sources {
                tx.execute("INSERT INTO main.session_catalog_sources(namespace,session_key,lifecycle,harness,native_id,source_path) VALUES(?1,?2,'current',?3,?4,?5) ON CONFLICT(namespace,session_key,source_path) DO UPDATE SET lifecycle='current',harness=excluded.harness,native_id=excluded.native_id",params![ns,row.key,row.harness,source.native_id,source.path.to_string_lossy()]).map_err(error)?;
            }
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_rows").map_err(error)?;
        tx.execute(
            "INSERT INTO storage_semantic_publications VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                ns,
                generation,
                batch.epoch,
                i64::try_from(self.source.admission)
                    .map_err(|_| refuse("semantic_admission_range"))?,
                batch.base_generation,
                batch.source_revision,
                proof_digest,
                claims_text,
                metadata_text
            ],
        )
        .map_err(error)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_proof").map_err(error)?;
        for (path, claim) in &prepared.claims {
            if let Some(claim) = claim {
                if tx.execute("DELETE FROM main.session_catalog_invalidations WHERE namespace=?1 AND source_path=?2 AND revision=?3",params![ns,path,claim]).map_err(error)?!=1 { return Err(refuse("semantic_publication_claim_changed")); }
            }
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_ack").map_err(error)?;
        publication_head
            .publish_generation(&generation, &version)
            .map_err(StoreError::Configured)?;
        tx.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,'catalog_publication_epoch',?2),(?1,'catalog_observed_at',?3),(?1,'catalog_completeness','partial') ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![ns,epoch,crate::model::now_ms().to_string()]).map_err(error)?;
        tx.execute("UPDATE main.catalog_semantic_sources SET publication_done=1,ack_done=1,claim=NULL WHERE namespace=?1",[ns]).map_err(error)?;
        tx.execute(
            "UPDATE main.catalog_semantic_endpoints SET published=1 WHERE namespace=?1",
            [ns],
        )
        .map_err(error)?;
        tx.execute(
            "UPDATE main.catalog_semantic_stage SET generation=?2 WHERE namespace=?1",
            params![ns, generation],
        )
        .map_err(error)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_head").map_err(error)?;
        tx.commit().map_err(error)?;
        self.source.generation = Some(generation.clone());
        Ok((Outcome::Written, Some(generation)))
    }

    pub(crate) fn finish(&mut self, epoch: i64) -> Result<Outcome, StoreError> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source
            .validate_publication(&tx)
            .map_err(StoreError::Configured)?;
        let ns = self.source.namespace();
        if self.epoch != Some(epoch) || head(&tx).map_err(error)? != self.source.generation {
            return Ok(Outcome::Conflict);
        }
        let ready:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND phase='publication' AND generation IS ?3) AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND (publication_done=0 OR ack_done=0))",params![ns,epoch,self.source.generation],|r|r.get(0)).map_err(error)?;
        if !ready {
            return Ok(Outcome::Conflict);
        }
        for table in crate::events::store::configured::semantic_tables() {
            tx.execute(
                &format!("DELETE FROM main.{table} WHERE namespace=?1"),
                [ns],
            )
            .map_err(error)?;
        }
        tx.commit().map_err(error)?;
        self.epoch = None;
        Ok(Outcome::Written)
    }

    pub(crate) fn select(
        &mut self,
        epoch: i64,
        selector: &crate::model::relation_stage::Selector,
    ) -> Result<crate::model::relation_stage::Selection, StoreError> {
        let tx = self.db.transaction().map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.namespace();
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND resetting=0)",params![ns,epoch],|r|r.get(0)).map_err(error)?;
        if !valid {
            return Err(refuse("stale semantic selector"));
        }
        match selector {
            crate::model::relation_stage::Selector::NativeKey { native, harness } => {
                let key:Option<String>=tx.query_row("SELECT session_key FROM main.catalog_semantic_keys WHERE namespace=?1 AND harness=?2 AND native_id=?3",params![ns,harness,native],|r|r.get(0)).optional().map_err(error)?;
                Ok(crate::model::relation_stage::Selection {
                    count: usize::from(key.is_some()),
                    key,
                    ..Default::default()
                })
            }
            _ => Err(refuse("semantic_nonprimitive_selector_pending")),
        }
    }

    pub(crate) fn replay(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> Result<Context, StoreError> {
        if owners.len() > 64 || !after.is_empty() {
            return Err(refuse("semantic_nonprimitive_replay_pending"));
        }
        let tx = self.db.transaction().map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.namespace();
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND phase='publication' AND resetting=0)",params![ns,epoch],|r|r.get(0)).map_err(error)?;
        if !valid {
            return Err(refuse("stale semantic replay epoch"));
        }
        let relations: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_relations WHERE namespace=?1)",
                [ns],
                |r| r.get(0),
            )
            .map_err(error)?;
        if relations {
            return Err(refuse("semantic_nonprimitive_replay_pending"));
        }
        let mut context = Context {
            epoch: Some(epoch),
            ..Default::default()
        };
        let mut bytes = 0usize;
        for owner in owners {
            let row:Option<(String,String,Option<f64>)>=tx.query_row("SELECT metadata,root_key,family_cost FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",params![ns,owner],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(error)?;
            if let Some((text, root, cost)) = row {
                bytes += text.len();
                if bytes > 1048576 {
                    return Err(refuse("semantic_context_budget"));
                }
                let mut endpoint: crate::model::relation_stage::Endpoint =
                    serde_json::from_str(&text).map_err(|e| StoreError::Data(e.to_string()))?;
                endpoint.root = root;
                context.family_costs.insert(owner.clone(), cost);
                context.endpoints.insert(owner.clone(), endpoint);
            }
        }
        Ok(context)
    }
    pub(crate) fn open(capability: SemanticStaging) -> Result<Self, StorageError> {
        let config = &capability.config;
        let mut db = capability.journal().connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let admission = if let Some(owner) = &capability.owner {
            owner.validate_ready(&tx)?;
            owner.admission()
        } else {
            NativeReaderBinding::Admitted(config.clone()).validate(&tx, &config.namespace)?;
            crate::events::store::configured::validate_staging_owner(&tx, config)?;
            namespace::require_admitted(&tx, &config.namespace)?.generation
        };
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
        if capability.owner.is_some() {
            tx.create_scalar_function(
                "semon_semantic_publisher",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8
                    | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
                |_| Ok(1_i64),
            )
            .map_err(failed)?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS storage_semantic_publications(namespace TEXT NOT NULL,generation TEXT NOT NULL,epoch INTEGER NOT NULL,admission INTEGER NOT NULL,base_generation TEXT,source_revision INTEGER NOT NULL,proof_digest TEXT NOT NULL,claims TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,epoch)) STRICT, WITHOUT ROWID;").map_err(failed)?;
            super::catalog_namespace::guard(&tx, "storage_semantic_publications")?;
            tx.execute_batch("CREATE TRIGGER IF NOT EXISTS storage_semantic_proof_immutable_update BEFORE UPDATE ON storage_semantic_publications BEGIN SELECT RAISE(ABORT,'immutable semantic proof'); END; CREATE TRIGGER IF NOT EXISTS storage_semantic_proof_immutable_delete BEFORE DELETE ON storage_semantic_publications BEGIN SELECT RAISE(ABORT,'immutable semantic proof'); END;").map_err(failed)?;
        }
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
        let epoch = tx
            .query_row(
                "SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=?1",
                [config.namespace.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)?;
        tx.commit().map_err(|_| StorageError::CommitUncertain)?;
        Ok(Self {
            db,
            reduction: capability.reduction,
            epoch,
            source: AdmittedSource {
                config: config.clone(),
                owner: capability.owner.clone(),
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
        // Batch witnesses remain opaque to the shared reducer, but private
        // native ownership also binds them to the logical Source admission.
        let bound_runtime = self
            .source
            .owner
            .as_ref()
            .map(|owner| {
                serde_json::to_string(&(
                    self.source.config.namespace.as_str(),
                    owner.admission(),
                    runtime,
                ))
            })
            .transpose()
            .map_err(|_| refuse("semantic_runtime_binding"))?;
        let runtime = bound_runtime.as_deref().unwrap_or(runtime);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        if head(&tx).map_err(error)? != self.source.generation {
            return Ok(Outcome::Conflict);
        }
        let ns = self.source.config.namespace.as_str();
        let actual_epoch: Option<i64> = tx
            .query_row(
                "SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        if actual_epoch != self.epoch {
            return Ok(Outcome::Conflict);
        }
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
            if !self.reduction {
                return Err(StoreError::Configured(
                    StorageError::NamespaceUpgradePending,
                ));
            }
            reset(&tx, ns, runtime).map_err(error)?;
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
        let epoch = tx
            .query_row(
                "SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(error)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_reset").map_err(error)?;
        tx.commit().map_err(error)?;
        self.epoch = Some(epoch);
        Ok(Outcome::Written)
    }

    pub(crate) fn batch(&mut self, limit: usize) -> Result<Option<Batch>, StoreError> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.config.namespace.as_str();
        let stage: Option<(i64,String,Option<String>,String)> = tx.query_row("SELECT epoch,phase,generation,runtime FROM main.catalog_semantic_stage WHERE namespace=?1", [ns], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(error)?;
        let Some((epoch, phase, base_generation, runtime)) = stage else {
            return Ok(None);
        };
        if self.epoch != Some(epoch) {
            return Err(StoreError::Configured(StorageError::Conflict));
        }
        if head(&tx).map_err(error)? != base_generation || base_generation != self.source.generation
        {
            return Err(StoreError::Configured(StorageError::Conflict));
        }
        if phase != "indexing" && !self.reduction {
            return Err(StoreError::Configured(
                StorageError::NamespaceUpgradePending,
            ));
        }
        let resetting: bool = tx
            .query_row(
                "SELECT resetting FROM main.catalog_semantic_stage WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(error)?;
        let reset_complete = if resetting {
            reset_page(&tx, ns).map_err(error)?
        } else {
            true
        };
        if !resetting && !reduction_witnesses(&tx, ns).map_err(error)? {
            return Err(StoreError::Configured(StorageError::Conflict));
        }
        let mut result_phase = Phase::Indexing;
        let mut paths=tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=0 ORDER BY source_path LIMIT ?2").map_err(error)?.query_map(params![ns,limit.clamp(1,64) as i64], |r|r.get::<_,String>(0)).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
        if !reset_complete {
            paths.clear();
        }
        let mut context = Context::default();
        if paths.is_empty() && !resetting {
            if !self.reduction {
                return Err(StoreError::Configured(
                    StorageError::NamespaceUpgradePending,
                ));
            }
            let events = event_page(&tx, ns)?;
            let frontier = frontier_page(&tx, ns)?;
            paths=tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=0 ORDER BY source_path LIMIT ?2").map_err(error)?.query_map(params![ns,limit.clamp(1,64) as i64], |r|r.get(0)).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
            if paths.is_empty() && events && frontier && native_keys_page(&tx, ns)? {
                tx.execute("UPDATE main.catalog_semantic_stage SET phase='identity' WHERE namespace=?1 AND phase='indexing'",[ns]).map_err(error)?;
                paths=tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND identity_done=0 ORDER BY source_path LIMIT ?2").map_err(error)?.query_map(params![ns,limit.clamp(1,64) as i64],|r|r.get(0)).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
                result_phase = Phase::Identity;
                context.epoch = Some(epoch);
                if paths.is_empty()
                    && crate::events::store::semantic_stage::reduce_with(&ScopedReduction {
                        db: &tx,
                        ns,
                    })
                    .map_err(error)?
                {
                    tx.execute("UPDATE main.catalog_semantic_stage SET phase='relations' WHERE namespace=?1 AND phase='identity'",[ns]).map_err(error)?;
                    result_phase = Phase::Relations;
                    paths=tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND relation_done=0 ORDER BY source_path LIMIT ?2").map_err(error)?.query_map(params![ns,limit.clamp(1,64) as i64],|r|r.get(0)).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
                    let endpoints=tx.prepare("SELECT metadata,root_key,family_cost FROM main.catalog_semantic_endpoints WHERE namespace=?1 ORDER BY session_key LIMIT 65").map_err(error)?.query_map([ns],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<f64>>(2)?))).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
                    if endpoints.len() > 64 {
                        return Err(refuse("semantic_endpoint_component_budget"));
                    }
                    let mut bytes = 0usize;
                    for (text, root, cost) in endpoints {
                        bytes += text.len();
                        if bytes > 1048576 {
                            return Err(refuse("semantic_context_budget"));
                        }
                        let mut endpoint: crate::model::relation_stage::Endpoint =
                            serde_json::from_str(&text)
                                .map_err(|e| StoreError::Data(e.to_string()))?;
                        endpoint.root = root;
                        context.family_costs.insert(endpoint.row.key.clone(), cost);
                        context.endpoints.insert(endpoint.row.key.clone(), endpoint);
                    }
                    if paths.is_empty() {
                        tx.execute("UPDATE main.catalog_semantic_stage SET phase='publication' WHERE namespace=?1",[ns]).map_err(error)?;
                        result_phase = Phase::Publication;
                        paths=tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND publication_done=0 ORDER BY source_path LIMIT ?2").map_err(error)?.query_map(params![ns,limit.clamp(1,64) as i64],|r|r.get(0)).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
                    }
                }
            }
        }
        let source_revision = tx
            .query_row(
                "SELECT revision FROM storage_source_journal WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(error)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_reduction").map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(Some(Batch {
            epoch,
            phase: result_phase,
            paths,
            base_generation,
            source_revision,
            runtime,
            context,
            relation_cursor: None,
        }))
    }

    pub(crate) fn record(
        &mut self,
        batch: &Batch,
        prepared: &PreparedSources,
    ) -> Result<Outcome, StoreError> {
        if batch.paths.len() > 64 {
            return Err(refuse("semantic_source_budget"));
        }
        if !prepared.withdrawals.is_empty() {
            return Err(refuse("semantic_withdrawals_pending"));
        }
        if !prepared.rows.is_empty()
            || !prepared.transcripts.is_empty()
            || !prepared.pids.is_empty()
        {
            return Err(refuse("semantic_nonprimitive_projection_pending"));
        }
        if !prepared.relationships.relations.is_empty() {
            return Err(refuse("semantic_relation_rows_pending"));
        }
        if !prepared.indexed_native_parents.is_empty() || !prepared.lineage_dependencies.is_empty()
        {
            return Err(refuse("semantic_native_parent_lineage_pending"));
        }
        if batch.phase != Phase::Indexing
            && (!self.reduction || !matches!(batch.phase, Phase::Identity | Phase::Relations))
        {
            return Err(refuse("semantic_phase_pending"));
        }
        if batch.phase == Phase::Indexing && !prepared.relationships.endpoints.is_empty() {
            return Err(refuse("semantic_indexing_endpoint_payload"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.config.namespace.as_str();
        let phase = match batch.phase {
            Phase::Indexing => "indexing",
            Phase::Identity => "identity",
            Phase::Relations => "relations",
            Phase::Publication => "publication",
        };
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND phase=?5 AND runtime=?3 AND generation IS ?4 AND resetting=0)", params![ns,batch.epoch,batch.runtime,batch.base_generation,phase], |r| r.get(0)).map_err(error)?;
        if !valid
            || self.epoch != Some(batch.epoch)
            || prepared.base_generation != batch.base_generation
            || head(&tx).map_err(error)? != batch.base_generation
            || self.source.generation != batch.base_generation
        {
            return Ok(Outcome::Conflict);
        }
        let claims = super::source_journal::claims(&tx, Some(ns), &batch.paths).map_err(error)?;
        if claims != prepared.claims {
            return Ok(Outcome::Conflict);
        }
        if batch.phase != Phase::Indexing {
            if !reduction_witnesses(&tx, ns).map_err(error)? {
                return Ok(Outcome::Conflict);
            }
            for path in &batch.paths {
                let sql = if batch.phase == Phase::Identity {
                    "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND index_done=1 AND identity_done=0)"
                } else {
                    "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND identity_done=1 AND relation_done=0)"
                };
                let pending: bool = tx
                    .query_row(sql, params![ns, path], |r| r.get(0))
                    .map_err(error)?;
                if !pending {
                    return Ok(Outcome::Conflict);
                }
            }
            if batch.phase == Phase::Identity {
                if prepared.relationships.endpoints.is_empty() && !batch.paths.is_empty() {
                    return Err(refuse("semantic_identity_endpoints_required"));
                }
                let mut covered = std::collections::BTreeSet::new();
                let mut encoded = Vec::new();
                let mut bytes = 0usize;
                for endpoint in prepared.relationships.endpoints.values() {
                    if endpoint.row.harness != "codex"
                        || endpoint.parent.is_some()
                        || endpoint.row.parent.is_some()
                        || endpoint.row.parent_source.is_some()
                        || !endpoint.native_parents.is_empty()
                        || endpoint.pid.is_some()
                    {
                        return Err(refuse("semantic_nonprimitive_identity_pending"));
                    }
                    if endpoint.row.sources.is_empty() || endpoint.row.sources.len() > 64 {
                        return Err(refuse("semantic_identity_source_budget"));
                    }
                    let ids = endpoint
                        .row
                        .sources
                        .iter()
                        .map(|s| s.native_id.as_str())
                        .collect::<std::collections::BTreeSet<_>>();
                    if ids
                        != endpoint
                            .row
                            .native_ids
                            .iter()
                            .map(String::as_str)
                            .collect::<std::collections::BTreeSet<_>>()
                    {
                        return Ok(Outcome::Conflict);
                    }
                    for source in &endpoint.row.sources {
                        let path = source.path.to_string_lossy().into_owned();
                        if !batch.paths.contains(&path) {
                            return Ok(Outcome::Conflict);
                        }
                        let stored: String=tx.query_row("SELECT metadata FROM main.catalog_semantic_inputs WHERE namespace=?1 AND source_path=?2 AND harness='codex'",params![ns,path],|r|r.get(0)).map_err(error)?;
                        let observed: crate::model::summary::CatalogSource =
                            serde_json::from_str(&stored)
                                .map_err(|e| StoreError::Data(e.to_string()))?;
                        let key: String=tx.query_row("SELECT session_key FROM main.catalog_semantic_keys WHERE namespace=?1 AND harness='codex' AND native_id=?2",params![ns,source.native_id],|r|r.get(0)).map_err(error)?;
                        if observed != *source
                            || key != endpoint.row.key
                            || !endpoint.row.native_ids.contains(&source.native_id)
                        {
                            return Ok(Outcome::Conflict);
                        }
                        covered.insert(path);
                    }
                    let text = serde_json::to_string(endpoint)
                        .map_err(|e| StoreError::Data(e.to_string()))?;
                    bytes += text.len();
                    if bytes > 1048576 {
                        return Err(refuse("semantic_context_budget"));
                    }
                    encoded.push((endpoint, text));
                }
                if batch.paths.iter().any(|p| !covered.contains(p)) {
                    return Ok(Outcome::Conflict);
                }
                for (endpoint, text) in encoded {
                    tx.execute("INSERT INTO main.catalog_semantic_endpoints(namespace,session_key,harness,cost,metadata) VALUES(?1,?2,'codex',?3,?4) ON CONFLICT(namespace,session_key) DO UPDATE SET metadata=excluded.metadata,cost=excluded.cost",params![ns,endpoint.row.key,endpoint.row.cost.usd,text]).map_err(error)?;
                    for source in &endpoint.row.sources {
                        tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_native(namespace,harness,native_id,session_key) VALUES(?1,'codex',?2,?3)",params![ns,source.native_id,endpoint.row.key]).map_err(error)?;
                        tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_owner_sources(namespace,session_key,source_path) VALUES(?1,?2,?3)",params![ns,endpoint.row.key,source.path.to_string_lossy()]).map_err(error)?;
                    }
                }
            }
            for path in &batch.paths {
                let sql = if batch.phase == Phase::Identity {
                    "UPDATE main.catalog_semantic_sources SET identity_done=1 WHERE namespace=?1 AND source_path=?2"
                } else {
                    "UPDATE main.catalog_semantic_sources SET relation_done=1 WHERE namespace=?1 AND source_path=?2"
                };
                tx.execute(sql, params![ns, path]).map_err(error)?;
            }
            #[cfg(test)]
            crate::events::store::configured::effect("semantic_record").map_err(error)?;
            tx.commit().map_err(error)?;
            return Ok(Outcome::Written);
        }
        if prepared.indexed_sources.len() != batch.paths.len() {
            return Ok(Outcome::Conflict);
        }
        let mut seen = std::collections::BTreeSet::new();
        for (_, source) in &prepared.indexed_sources {
            if self.reduction
                && source
                    .native_role
                    .as_ref()
                    .and_then(|r| r.get("Codex"))
                    .and_then(|r| r.get("parent_thread"))
                    .is_some_and(|p| !p.is_null())
            {
                return Err(refuse("semantic_native_parent_lineage_pending"));
            }
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
            if self.reduction && harness != "codex" {
                return Err(refuse("semantic_nonprimitive_event_harness"));
            }
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
            tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,'pathowner',?2,'')",params![ns,path.as_ref()]).map_err(error)?;
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_record").map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(Outcome::Written)
    }
}
