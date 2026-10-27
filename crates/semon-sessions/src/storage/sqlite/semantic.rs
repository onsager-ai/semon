//! Private admitted-Source staging and bounded initial Current publication.
//! Public and complete-domain publication qualifications remain closed.
#[path = "semantic_owners.rs"]
mod owners;
use super::source_journal::SourceJournal;
use super::{failed, namespace};
use crate::events::{Ledger, Outcome, StoreError};
use crate::model::relation_stage::OwnerStore;
use crate::model::{
    PreparedSources,
    relation_stage::{Batch, Context, Phase},
};
use crate::storage::{NativeReaderBinding, StorageError, StoreConfig};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

pub(crate) fn validate_owners(db: &Connection) -> Result<(), StorageError> {
    owners::validate(db)
}

#[cfg(test)]
thread_local! {
    static PUBLICATION_PHASE: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
struct PublicationDiagnostic(Option<&'static str>);
#[cfg(test)]
impl Drop for PublicationDiagnostic {
    fn drop(&mut self) {
        PUBLICATION_PHASE.with(|phase| phase.set(self.0));
    }
}
fn publication_phase(label: &'static str) {
    #[cfg(test)]
    PUBLICATION_PHASE.with(|phase| phase.set(Some(label)));
    #[cfg(not(test))]
    let _ = label;
}
#[cfg(test)]
pub(super) fn diagnose(error: &rusqlite::Error) {
    PUBLICATION_PHASE.with(|phase| {
        if let Some(phase) = phase.get() {
            // Never print SQLite messages: they can contain SQL or native data.
            eprintln!(
                "semantic_publication_failure phase={phase} sqlite_extended_code={:?}",
                error.sqlite_error().map(|e| e.extended_code)
            );
        }
    });
}

/// This capability binds the real admission and physical incarnation. Its fields
/// cannot be supplied by a caller or inferred from an Options path.
pub(crate) struct AdmittedSource {
    config: StoreConfig,
    owner: Option<crate::events::store::configured::NativeOwnership>,
    admission: u64,
    generation: Option<String>,
}

impl AdmittedSource {
    /// Restore the existing native capability for the ordinary publisher. The
    /// publication transaction still validates this capability before effects.
    pub(crate) fn native(owner: crate::events::store::configured::NativeOwnership) -> Self {
        Self {
            config: owner.config().clone(),
            admission: owner.admission(),
            owner: Some(owner),
            generation: None,
        }
    }
    pub(super) fn namespace(&self) -> &str {
        self.config.namespace.as_str()
    }
    pub(super) fn validate_publication(&self, db: &Connection) -> Result<(), StorageError> {
        if self.owner.is_none() {
            return Err(StorageError::NamespaceUpgradePending);
        }
        // Private profiles are authored as one physical family. Never upgrade
        // a profile that predates the scoped custody/recipe/search effects.
        for table in [
            "session_trace_headers",
            "session_trace_turns",
            "session_trace_edges",
            "session_trace_bookmarks",
            "session_slot_projections",
            "session_slots",
            "session_history_catalog",
            "session_history_projections",
            "session_history_slots",
            "session_history_slot_parts",
            "session_search_coverage",
            "session_search_documents",
            "session_search_memberships",
            "session_search_counts",
        ] {
            let scoped: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1,'main') WHERE name='namespace' AND pk=1 AND [notnull]=1)", [table], |r|r.get(0)).map_err(failed)?;
            if !scoped {
                return Err(StorageError::NamespaceUpgradePending);
            }
        }
        self.validate(db)
    }
    pub(crate) fn validate(&self, db: &Connection) -> Result<(), StorageError> {
        if let Some(owner) = &self.owner {
            return owner.validate_ready(db);
        }
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
    reduction: bool,
    epoch: Option<i64>,
    begun: bool,
}

/// Cost recomputation writes the same admitted Catalog and retained metadata
/// as publication. Use that existing producer connection, including its owner
/// checks and namespace guards, rather than an unbound event connection.
pub(crate) fn pricing_connection(
    owner: &crate::events::store::configured::NativeOwnership,
) -> Result<Connection, StorageError> {
    Ok(Stage::open(SemanticStaging {
        config: owner.config().clone(),
        reduction: true,
        owner: Some(owner.clone()),
    })?
    .db)
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
    /// Same typed custody used by the producer; no public qualification grant.
    #[allow(dead_code)] // Private producer/reader qualification; no public factory.
    pub(crate) fn reader_binding(&self) -> Result<NativeReaderBinding, StorageError> {
        self.owner
            .clone()
            .map(NativeReaderBinding::Semantic)
            .ok_or(StorageError::NamespaceUpgradePending)
    }
    /// Explicit private admission into the authored composite native profile.
    #[allow(dead_code)]
    pub(crate) fn admit(config: &StoreConfig) -> Result<Self, StorageError> {
        let owner = crate::events::store::configured::NativeOwnership::admit(config)?;
        let db = SourceJournal::owned(owner.clone()).connection()?;
        db.execute_batch("BEGIN DEFERRED").map_err(failed)?;
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
    // EventCache owns the typed source-observation producer.  Keep this
    // crate-visible until the active semantic work can consume the same port.
    pub(crate) fn journal(&self) -> SourceJournal {
        self.owner
            .as_ref()
            .map(|owner| SourceJournal::owned(owner.clone()))
            .unwrap_or_else(|| SourceJournal(self.config.clone()))
    }
    pub(super) fn writer(&self) -> Result<crate::events::store::configured::Writer, StorageError> {
        self.owner
            .as_ref()
            .map(|owner| crate::events::store::configured::Writer::owned(owner.clone()))
            .unwrap_or_else(|| crate::events::store::configured::Writer::new(&self.config))
    }
    /// Primitive staging has no reported-facts port. Its reported reads/writes
    /// still refuse; only a composite owner restores the persisted stamp.
    pub(super) fn reported_stamp(
        &self,
    ) -> Result<Option<crate::events::ReportedFileStamp>, StorageError> {
        let Some(owner) = &self.owner else {
            return Ok(None);
        };
        crate::events::store::configured::Writer::owned(owner.clone())?.reported_stamp()
    }
    pub(crate) fn validate(config: &StoreConfig) -> Result<Self, StorageError> {
        crate::events::store::configured::validate_semantic_schema(config)?;
        let mut db = SourceJournal(config.clone()).connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Deferred)
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

const SEMANTIC_SUPPORT_SCHEMA: &[(&str, &str)] = &[
    (
        "storage_semantic_publications",
        "CREATE TABLE IF NOT EXISTS storage_semantic_publications(namespace TEXT NOT NULL,generation TEXT NOT NULL,epoch INTEGER NOT NULL,admission INTEGER NOT NULL,base_generation TEXT,source_revision INTEGER NOT NULL,proof_digest TEXT NOT NULL,claims TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,epoch)) STRICT, WITHOUT ROWID;",
    ),
    (
        "storage_semantic_profiles",
        "CREATE TABLE IF NOT EXISTS storage_semantic_profiles(namespace TEXT PRIMARY KEY,admission INTEGER NOT NULL,recipe TEXT NOT NULL,epoch INTEGER NOT NULL) STRICT;",
    ),
    (
        "storage_semantic_model_batches",
        "CREATE TABLE IF NOT EXISTS storage_semantic_model_batches(namespace TEXT PRIMARY KEY,epoch INTEGER NOT NULL,metadata TEXT NOT NULL,projections TEXT NOT NULL,claims TEXT NOT NULL) STRICT, WITHOUT ROWID;",
    ),
    (
        "storage_semantic_proof_immutable_update",
        "CREATE TRIGGER IF NOT EXISTS storage_semantic_proof_immutable_update BEFORE UPDATE ON storage_semantic_publications BEGIN SELECT RAISE(ABORT,'immutable semantic proof'); END;",
    ),
    (
        "storage_semantic_proof_immutable_delete",
        "CREATE TRIGGER IF NOT EXISTS storage_semantic_proof_immutable_delete BEFORE DELETE ON storage_semantic_publications BEGIN SELECT RAISE(ABORT,'immutable semantic proof'); END;",
    ),
];

/// Provision the current semantic profile only while enrolling a namespace.
pub(crate) fn enroll(
    db: &Connection,
    config: &StoreConfig,
    admission: u64,
) -> Result<(), StorageError> {
    let admission_sql = i64::try_from(admission).map_err(|_| StorageError::ScopeChanged)?;
    for (_, sql) in SEMANTIC_SUPPORT_SCHEMA {
        db.execute_batch(sql).map_err(failed)?;
    }
    db.create_scalar_function(
        "semon_semantic_writer",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(failed)?;
    db.create_scalar_function(
        "semon_semantic_publisher",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |_| Ok(1_i64),
    )
    .map_err(failed)?;
    super::catalog_namespace::guard(db, "storage_semantic_publications")?;
    super::proof_generations::initialize(db)?;
    for table in crate::events::store::configured::semantic_tables()
        .iter()
        .copied()
        .chain([
            "storage_semantic_profiles",
            "storage_semantic_model_batches",
        ])
    {
        for (operation, predicate) in [
            ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
            (
                "UPDATE",
                "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
            ),
            ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
        ] {
            db.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS storage_semantic_guard_{table}_{operation} BEFORE {operation} ON {table} WHEN semon_semantic_writer()<>1 OR {predicate} BEGIN SELECT RAISE(ABORT,'semantic Source refused'); END;")).map_err(failed)?;
        }
    }
    let recipe = crate::events::store::configured::semantic_recipe();
    db.execute(
        "INSERT INTO storage_semantic_profiles VALUES(?1,?2,?3,0)",
        params![config.namespace.as_str(), admission_sql, recipe],
    )
    .map_err(failed)?;
    Ok(())
}

fn refuse(reason: &'static str) -> StoreError {
    StoreError::Data(reason.into())
}

// The admitted transaction has already fenced epoch/generation before these
// namespace-owned physical operations. Claims are observations, never ACKs.
fn reset(c: &Connection, ns: &str, runtime: &str) -> rusqlite::Result<()> {
    if c.execute("UPDATE storage_semantic_profiles SET epoch=max(epoch,(SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=?1))+1 WHERE namespace=?1 AND max(epoch,(SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=?1))<9223372036854775807", [ns])? != 1 { return Err(rusqlite::Error::InvalidQuery); }
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
        // Member replay is a durable discovery cursor, including across the
        // epoch reset that invalidates the old model and its callers.
        let filter = if table == "catalog_semantic_frontier" {
            " AND kind<>'members'"
        } else {
            ""
        };
        budget-=c.execute(&format!("DELETE FROM main.{table} WHERE namespace=?1{filter} AND ({key}) IN (SELECT {key} FROM main.{table} WHERE namespace=?1{filter} ORDER BY {key} LIMIT ?2)"), params![ns,budget as i64])?;
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

enum ReductionWitness {
    Current,
    Changed,
    SourceBudget,
    WithdrawalBudget,
}

impl ReductionWitness {
    fn current(self) -> Result<bool, StoreError> {
        match self {
            Self::Current => Ok(true),
            Self::Changed => Ok(false),
            Self::SourceBudget => Err(refuse("semantic_component_source_budget")),
            Self::WithdrawalBudget => Err(refuse("semantic_component_withdrawal_budget")),
        }
    }
}

fn reduction_witnesses(c: &Connection, ns: &str) -> rusqlite::Result<ReductionWitness> {
    let rows=c.prepare("SELECT s.source_path,s.claim,s.ledger,s.withdrawal IS NOT NULL,EXISTS(SELECT 1 FROM main.catalog_semantic_inputs i WHERE i.namespace=s.namespace AND i.source_path=s.source_path) FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.index_done=1 ORDER BY s.source_path LIMIT 65")?.query_map([ns], |r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,bool>(3)?,r.get::<_,bool>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    // A bounded reducer cannot certify an unbounded component. Native owner
    // closure qualification remains a separate, explicitly limited port.
    if rows.len() > 64 {
        return Ok(ReductionWitness::SourceBudget);
    }
    let negatives = c.prepare("SELECT withdrawal FROM main.catalog_semantic_sources WHERE namespace=?1 AND withdrawal IS NOT NULL ORDER BY source_path LIMIT 65")?.query_map([ns],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if negatives.len() > 64 {
        return Ok(ReductionWitness::WithdrawalBudget);
    }
    for text in negatives {
        let withdrawal: crate::model::relation_stage::Withdrawal =
            serde_json::from_str(&text).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if !withdrawal.current().unwrap_or(false) {
            return Ok(ReductionWitness::Changed);
        }
    }
    for (path, claim, recorded, withdrawn, indexed) in rows {
        let actual = super::source_journal::claims(c, ns, std::slice::from_ref(&path))?[0].1;
        let ledger = crate::events::store::ledger_row(c, &path)?
            .map(|(_, l)| serde_json::to_string(&l))
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        if actual != claim
            || ledger != recorded
            || (!indexed && !withdrawn && !removed_source(c, ns, &path)?)
        {
            #[cfg(test)]
            eprintln!(
                "semantic_reduction_witness claim_changed={} ledger_changed={} negative_missing={}",
                actual != claim,
                ledger != recorded,
                !indexed && !withdrawn && !removed_source(c, ns, &path)?
            );
            return Ok(ReductionWitness::Changed);
        }
    }
    Ok(ReductionWitness::Current)
}

// A missing scan result is not a withdrawal. Only an ordinary producer's
// durable removal, with no live ledger, can complete its staged path. Physical
// absence is observed by the producer under Source locks before this transaction;
// reduction/publication consume only persisted evidence and exact claims/ledgers.
// Retained recipes and proofs remain owned by the Catalog.
fn removed_source(c: &Connection, ns: &str, path: &str) -> rusqlite::Result<bool> {
    let removed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=?1 AND source_path=?2 AND reason='removed')", params![ns,path], |r| r.get(0))?;
    if !removed {
        return Ok(false);
    }
    match crate::events::store::ledger_row(c, path)? {
        None => Ok(true),
        Some((_, ledger)) => crate::events::store::archive_ledger_retained(c, path, &ledger),
    }
}

fn removed_paths(c: &Connection, ns: &str) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let paths = c.prepare("SELECT s.source_path FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.index_done=1 AND s.withdrawal IS NULL AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_inputs i WHERE i.namespace=s.namespace AND i.source_path=s.source_path) ORDER BY s.source_path LIMIT 65")?.query_map([ns], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if paths.len() > 64 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    for path in &paths {
        if !removed_source(c, ns, path)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(paths.into_iter().collect())
}

fn withdrawal_runtime_matches(
    source: &AdmittedSource,
    batch: &Batch,
    withdrawal: &crate::model::relation_stage::Withdrawal,
) -> Result<bool, StoreError> {
    let runtime = if source.owner.is_some() {
        serde_json::to_string(&(source.namespace(), source.admission, &withdrawal.runtime))
            .map_err(|e| StoreError::Data(e.to_string()))?
    } else {
        withdrawal.runtime.clone()
    };
    Ok(runtime == batch.runtime)
}

fn validate_withdrawals(
    db: &Connection,
    source: &AdmittedSource,
    batch: &Batch,
    prepared: &PreparedSources,
) -> Result<std::collections::BTreeSet<String>, StoreError> {
    let ns = source.namespace();
    let mut paths = std::collections::BTreeSet::new();
    for withdrawal in &prepared.withdrawals {
        if !paths.insert(withdrawal.path.clone())
            || !batch.paths.contains(&withdrawal.path)
            || prepared
                .rows
                .iter()
                .flat_map(|row| &row.sources)
                .any(|source| source.path == std::path::Path::new(&withdrawal.path))
            || !withdrawal_runtime_matches(source, batch, withdrawal)?
            || !withdrawal.current().unwrap_or(false)
        {
            return Err(refuse("semantic_withdrawal_changed"));
        }
        let encoded =
            serde_json::to_string(withdrawal).map_err(|e| StoreError::Data(e.to_string()))?;
        let exact: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND withdrawal=?3 AND index_done=1)",params![ns,withdrawal.path,encoded],|r|r.get(0)).map_err(error)?;
        if !exact {
            return Err(refuse("semantic_withdrawal_witness"));
        }
    }
    let count: i64 = db.query_row("SELECT count(*) FROM main.catalog_semantic_sources WHERE namespace=?1 AND withdrawal IS NOT NULL",[ns],|r|r.get(0)).map_err(error)?;
    if count != paths.len() as i64 {
        return Err(refuse("semantic_withdrawal_membership"));
    }
    Ok(paths)
}

fn staged_paths(db: &Connection, ns: &str) -> Result<Vec<String>, StoreError> {
    let paths = db.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 ORDER BY source_path LIMIT 65").map_err(error)?
        .query_map([ns], |r| r.get(0)).map_err(error)?
        .collect::<rusqlite::Result<Vec<String>>>().map_err(error)?;
    if paths.len() > 64 {
        return Err(refuse("semantic_affected_owner_source_budget"));
    }
    Ok(paths)
}

fn retirement_plan(
    db: &Connection,
    source: &AdmittedSource,
    prepared: &PreparedSources,
) -> Result<Vec<(String, bool)>, StoreError> {
    let ns = source.namespace();
    let paths = staged_paths(db, ns)?;
    let effects = owners::Owners { db, namespace: ns };
    let prior = effects
        .affected(&paths)
        .map_err(|e| StoreError::Data(e.to_string()))?;
    if !prior.is_empty()
        && prepared
            .rows
            .iter()
            .any(|row| row.query.is_some() || row.trace.is_some())
        && prepared.relationships.lifecycle_authority.is_none()
    {
        return Err(refuse("semantic_affected_owner_authority_missing"));
    }
    let mut outgoing = Vec::new();
    let mut plan = Vec::new();
    for old in prior.values() {
        // Even surviving incident owners must be rederived, never silently
        // omitted because the transition changed their native identity.
        if old
            .sources
            .iter()
            .any(|s| !paths.contains(&s.path.to_string_lossy().into_owned()))
        {
            return Err(refuse("semantic_affected_owner_scope"));
        }
        if prepared.rows.iter().any(|row| row.key == old.key) {
            continue;
        }
        let withdrawn = old.sources.iter().all(|s| {
            prepared
                .withdrawals
                .iter()
                .any(|w| s.path == std::path::Path::new(&w.path))
        });
        if !withdrawn {
            for s in &old.sources {
                if !prepared
                    .rows
                    .iter()
                    .flat_map(|r| &r.sources)
                    .any(|new| new.path == s.path)
                    && !removed_source(db, ns, s.path.to_string_lossy().as_ref()).map_err(error)?
                {
                    return Err(refuse("semantic_affected_owner_transition_pending"));
                }
            }
        }
        outgoing.push(old.clone());
        plan.push((old.key.clone(), withdrawn));
    }
    if serde_json::to_string(&outgoing).map_err(|e| StoreError::Data(e.to_string()))?
        != serde_json::to_string(&prepared.relationships.retired_owners)
            .map_err(|e| StoreError::Data(e.to_string()))?
    {
        return Err(refuse("semantic_affected_owner_witness"));
    }
    Ok(plan)
}

fn lifecycle_current(prepared: &PreparedSources) -> bool {
    let Some(authority) = &prepared.relationships.lifecycle_authority else {
        return prepared.relationships.retired_owners.is_empty();
    };
    authority.current().unwrap_or(false)
}

struct ScopedProjection<'a> {
    db: &'a Connection,
    ns: &'a str,
    layout: &'a FrontierLayout<'a>,
}
impl crate::events::store::semantic_stage::EventProjectionPort for ScopedProjection<'_> {
    fn paths(&self) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=1 AND events_done=0 ORDER BY source_path LIMIT 64")?.query_map([self.ns],|r|r.get(0))?.collect()
    }
    fn input(&self, path: &str) -> rusqlite::Result<Option<(String, Option<String>)>> {
        self.db.query_row("SELECT harness,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.catalog_semantic_inputs WHERE namespace=?1 AND source_path=?2",params![self.ns,path],|r|Ok((r.get(0)?,r.get(1)?))).optional()
    }
    fn cursor(&self, path: &str) -> rusqlite::Result<(i64, i64)> {
        self.db.query_row("SELECT f.file_id,s.event_after FROM main.files f JOIN main.catalog_semantic_sources s ON s.namespace=f.namespace AND s.source_path=f.path WHERE f.namespace=?1 AND s.namespace=?1 AND f.path=?2",params![self.ns,path],|r|Ok((r.get(0)?,r.get(1)?)))
    }
    fn candidates(
        &self,
        file: i64,
        after: i64,
        limit: usize,
        visit: &mut dyn FnMut(usize, crate::events::Event, usize) -> rusqlite::Result<bool>,
    ) -> rusqlite::Result<()> {
        let columns = crate::events::store::semantic_stage::projection_columns();
        let predicate = crate::events::store::semantic_stage::projection_predicate("");
        let sql = format!(
            "SELECT {columns} FROM main.events WHERE namespace=?1 AND file_id=?2 AND extra=0 AND seq>?3 AND {predicate} ORDER BY seq LIMIT ?4"
        );
        let mut q = self.db.prepare(&sql)?;
        let mut rows = q.query(params![self.ns, file, after, limit as i64])?;
        while let Some(row) = rows.next()? {
            let (seq, event) = crate::events::store::semantic_stage::projection_candidate(row)?;
            let kind = serde_json::to_value(event.k).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let kind = kind.as_str().ok_or(rusqlite::Error::InvalidQuery)?;
            let occurrence: i64 = self.db.query_row("SELECT COUNT(*) FROM main.events WHERE namespace=?1 AND file_id=?2 AND extra=0 AND o=?3 AND b=?4 AND k=?5 AND seq<?6",params![self.ns,file,i64::try_from(event.o).map_err(|_|rusqlite::Error::InvalidQuery)?,event.b,kind,i64::try_from(seq).map_err(|_|rusqlite::Error::InvalidQuery)?],|r|r.get(0))?;
            if !visit(
                seq,
                event,
                usize::try_from(occurrence).map_err(|_| rusqlite::Error::InvalidQuery)?,
            )? {
                break;
            }
        }
        Ok(())
    }
    fn more(&self, file: i64, after: i64) -> rusqlite::Result<bool> {
        let predicate = crate::events::store::semantic_stage::projection_predicate("");
        self.db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM main.events WHERE namespace=?1 AND file_id=?2 AND extra=0 AND seq>?3 AND {predicate})"),params![self.ns,file,after],|r|r.get(0))
    }
    fn finish(&self, path: &str, after: Option<i64>, done: bool) -> rusqlite::Result<()> {
        if let Some(after) = after {
            self.db.execute("UPDATE main.catalog_semantic_sources SET event_after=?3,events_done=?4 WHERE namespace=?1 AND source_path=?2",params![self.ns,path,after,done])?;
        } else {
            self.db.execute("UPDATE main.catalog_semantic_sources SET events_done=1 WHERE namespace=?1 AND source_path=?2",params![self.ns,path])?;
        }
        Ok(())
    }
    fn put(
        &self,
        row: &crate::events::store::semantic_stage::ProjectedEvent,
    ) -> rusqlite::Result<()> {
        let native = &row.native;
        self.db.execute("INSERT OR REPLACE INTO main.catalog_semantic_events(namespace,source_path,event_key,owner,kind,msg_id,tool_id,name,metadata,native_seq,copy_key) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![self.ns,native.owner,row.key,native.owner,row.kind,row.msg,native.event.id,row.name,row.encoded,row.seq,crate::model::native_copy_identity(&native.event)])?;
        Ok(())
    }
    fn enqueue(&self, kind: &str, value: &str) -> rusqlite::Result<()> {
        self.db.execute("INSERT INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,?2,?3,'') ON CONFLICT(namespace,kind,value,harness) DO UPDATE SET done=0,after_key='',after_edge='' WHERE catalog_semantic_frontier.done=1",params![self.ns,kind,value])?;
        Ok(())
    }
    fn native(&self, id: &str) -> rusqlite::Result<()> {
        crate::events::store::semantic_stage::native_frontier_with(
            &self.layout.frontier(self.ns),
            "claude",
            id,
        )
    }
    fn done(&self) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=1 AND events_done=0)",[self.ns],|r|r.get(0))
    }
}

/// A page-local choice of authored physical statements. Never retained by a
/// Stage, connection or profile: bind after custody validation in its owner TX,
/// use only the fixed DML below, and drop before that TX ends. No page runs DDL.
struct FrontierLayout<'tx> {
    db: &'tx Connection,
}
impl<'tx> FrontierLayout<'tx> {
    fn bind(tx: &'tx rusqlite::Transaction<'_>) -> rusqlite::Result<Self> {
        let db: &Connection = tx;
        Ok(Self { db })
    }
    fn frontier<'a>(&'a self, ns: &'a str) -> ScopedNativeFrontier<'a> {
        ScopedNativeFrontier { db: self.db, ns }
    }
}

struct ScopedNativeFrontier<'a> {
    db: &'a Connection,
    ns: &'a str,
}
impl crate::events::store::semantic_stage::NativeFrontierPort for ScopedNativeFrontier<'_> {
    fn needed_native(&self, harness: &str, id: &str) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_catalog_sources own WHERE own.namespace=?1 AND own.harness=?2 AND own.native_id=?3 AND own.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.source_path=own.source_path))",params![self.ns,harness,id],|r|r.get(0))
    }
    fn needed_dependency(&self, harness: &str, id: &str) -> rusqlite::Result<bool> {
        let sql = "SELECT EXISTS(SELECT 1 FROM main.catalog_native_dependencies d WHERE d.namespace=?1 AND d.harness=?2 AND d.native_id=?3 AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.source_path=d.source_path))";
        self.db
            .query_row(sql, params![self.ns, harness, id], |r| r.get(0))
    }
    fn needed_nativeall(&self, id: &str) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.session_catalog_sources own WHERE own.namespace=?1 AND own.native_id=?2 AND own.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.source_path=own.source_path))",params![self.ns,id],|r|r.get(0))
    }
    fn needed_filename(&self, copilot: bool, value: &str) -> rusqlite::Result<bool> {
        let predicate = if copilot {
            "source_path GLOB '*/session-state/*/events.jsonl' AND substr(substr(source_path,1,length(source_path)-13),length(rtrim(substr(source_path,1,length(source_path)-13),replace(substr(source_path,1,length(source_path)-13),'/','')))+1)=?2"
        } else {
            "substr(source_path,length(rtrim(source_path,replace(source_path,'/','')))+1)=?2"
        };
        let files = predicate.replace("source_path", "f.path");
        let invalidations = predicate.replace("source_path", "i.source_path");
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM main.files f WHERE f.namespace=?1 AND {files} AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.source_path=f.path) UNION ALL SELECT 1 FROM main.session_catalog_invalidations i WHERE i.namespace=?1 AND {invalidations} AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.source_path=i.source_path))"
        );
        self.db
            .query_row(&sql, params![self.ns, value], |r| r.get(0))
    }
    fn needed_header(&self, parent: bool, qualified: bool, id: &str) -> rusqlite::Result<bool> {
        let field = if parent {
            "metadata.parent_thread"
        } else {
            "id"
        };
        let dependency = if parent { "parent" } else { "native" };
        // These authored partial indexes match the JSON expression and native
        // key predicate below. Pin the access path: the namespace/key primary
        // key is also eligible, but scans unrelated native headers.
        let index = if parent {
            "semantic_codex_header_parent"
        } else {
            "semantic_codex_header_native"
        };
        let qualification = if qualified {
            format!(
                "AND NOT EXISTS(SELECT 1 FROM main.catalog_native_dependencies d WHERE d.namespace=?1 AND d.source_path=substr(m.key,length('native_codex_header:')+1) AND d.native_id=?2 AND d.harness='semantic-{dependency}:codex')"
            )
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM main.meta m INDEXED BY {index} WHERE m.namespace=?1 AND m.key GLOB 'native_codex_header:*' AND json_valid(m.value) AND json_extract(m.value,'$.header.{field}')=?2 AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=?1 AND s.source_path=substr(m.key,length('native_codex_header:')+1)) {qualification})"
        );
        self.db.query_row(&sql, params![self.ns, id], |r| r.get(0))
    }
    fn queue(&self, kind: &str, value: &str, harness: &str) -> rusqlite::Result<()> {
        self.db.execute("INSERT INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,kind,value,harness) DO UPDATE SET done=0,after_key='',after_edge='' WHERE catalog_semantic_frontier.done=1",params![self.ns,kind,value,harness])?;
        Ok(())
    }
}

// Measure the actual physical adapter, not a parallel EXPLAIN-only query.
#[cfg(test)]
pub(crate) fn native_frontier_vm_probe(c: &Connection, ns: &str, id: &str) -> Vec<(String, usize)> {
    use crate::events::store::semantic_stage::NativeFrontierPort;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let tx = c.unchecked_transaction().unwrap();
    let mut result = Vec::new();
    c.execute_batch("SAVEPOINT native_layout_probe").unwrap();
    for family in ["metadata", "support"] {
        let started = std::time::Instant::now();
        let steps = Arc::new(AtomicUsize::new(0));
        let observed = steps.clone();
        c.progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
        if family == "metadata" {
            // Shape work now belongs to entry, while metadata_sql selects
            // only the authored current statement. Charge the real entry check.
            crate::events::store::configured::validate_statement_layout(c).unwrap();
        } else {
            crate::events::store::semantic_stage::validate_support(c).unwrap();
        }
        c.progress_handler(0, None::<fn() -> bool>).unwrap();
        let elapsed = started.elapsed();
        let steps = steps.load(Ordering::Relaxed);
        eprintln!(
            "native_frontier_layout family={family} vm_steps={steps} elapsed_us={}",
            elapsed.as_micros()
        );
        // Wall time also includes the validator's reference-connection work,
        // which this real-connection progress handler cannot observe.
        assert!(elapsed < std::time::Duration::from_secs(1));
        // Setup is measured independently, never hidden as uncharged work.
        // This fixed qualification budget is additional to the unchanged
        // 4096 operation budget, and must not grow with native history.
        assert!(
            steps > 0 && steps < 32768,
            "{family} layout qualification: {steps}"
        );
        result.push((format!("setup:{family}"), steps));
    }
    c.execute_batch("ROLLBACK TO native_layout_probe; RELEASE native_layout_probe")
        .unwrap();
    // Binding the single current layout performs no SQL. Schema qualification
    // above remains measured separately at the actual admission boundary.
    let setup = Arc::new(AtomicUsize::new(0));
    let observed = setup.clone();
    let started = std::time::Instant::now();
    c.progress_handler(
        1,
        Some(move || {
            observed.fetch_add(1, Ordering::Relaxed);
            false
        }),
    )
    .unwrap();
    let layout = FrontierLayout::bind(&tx).unwrap();
    c.progress_handler(0, None::<fn() -> bool>).unwrap();
    let elapsed = started.elapsed();
    let setup = setup.load(Ordering::Relaxed);
    eprintln!(
        "native_frontier_layout family=page vm_steps={setup} elapsed_us={}",
        elapsed.as_micros()
    );
    assert!(elapsed < std::time::Duration::from_secs(1));
    assert_eq!(setup, 0, "current layout binding must perform no SQL");
    result.push(("setup:page".to_owned(), setup));
    // Each read/page starts from exactly the same source/stage state.
    for kind in [
        "filename",
        "copilot",
        "header",
        "parent",
        "claudeid",
        "copilotid",
        "headerid",
        "headerparent",
    ] {
        c.execute_batch("SAVEPOINT native_vm_probe").unwrap();
        let port = layout.frontier(ns);
        if !matches!(kind, "filename" | "copilot" | "header" | "parent") {
            let value = if kind == "claudeid" {
                format!("{id}.jsonl")
            } else {
                id.to_owned()
            };
            port.queue(kind, &value, "").unwrap();
        }
        let steps = Arc::new(AtomicUsize::new(0));
        let observed = steps.clone();
        let started = std::time::Instant::now();
        c.progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
        match kind {
            "filename" => {
                port.needed_filename(false, &format!("{id}.jsonl")).unwrap();
            }
            "copilot" => {
                port.needed_filename(true, id).unwrap();
            }
            "header" => {
                port.needed_header(false, false, id).unwrap();
            }
            "parent" => {
                port.needed_header(true, true, id).unwrap();
            }
            _ => {
                frontier_page(&layout, ns).unwrap();
            }
        }
        c.progress_handler(0, None::<fn() -> bool>).unwrap();
        let elapsed = started.elapsed();
        let steps = steps.load(Ordering::Relaxed);
        eprintln!(
            "native_frontier_query kind={kind} vm_steps={steps} elapsed_us={}",
            elapsed.as_micros()
        );
        assert!(elapsed < std::time::Duration::from_secs(1));
        result.push((kind.to_owned(), steps));
        c.execute_batch("ROLLBACK TO native_vm_probe; RELEASE native_vm_probe")
            .unwrap();
    }
    result
}

struct ScopedQualification<'a> {
    db: &'a Connection,
    ns: &'a str,
}
impl crate::events::store::semantic_stage::QualificationPort for ScopedQualification<'_> {
    fn pending(&self) -> rusqlite::Result<Vec<(String, String, Option<String>)>> {
        self.db.prepare("SELECT source_path,event_key,copy_key FROM main.catalog_semantic_events WHERE namespace=?1 AND qualified=0 ORDER BY source_path,event_key LIMIT 64")?.query_map([self.ns],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect()
    }
    fn copied(&self, path: &str, identity: &str) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_inputs target JOIN main.catalog_semantic_owner_sources own ON own.namespace=target.namespace AND own.source_path=target.source_path JOIN main.catalog_semantic_owner_sources predecessor ON predecessor.namespace=own.namespace AND predecessor.session_key=own.session_key AND predecessor.source_path<>target.source_path JOIN main.files source ON source.namespace=predecessor.namespace AND source.path=predecessor.source_path JOIN main.events event ON event.namespace=source.namespace AND event.file_id=source.file_id WHERE target.namespace=?1 AND own.namespace=?1 AND predecessor.namespace=?1 AND source.namespace=?1 AND event.namespace=?1 AND target.source_path=?2 AND target.is_top=1 AND event.extra=0 AND COALESCE(event.u,CASE WHEN event.k IN ('tool','xsm') THEN event.id END)=?3 AND EXISTS(SELECT 1 FROM json_each(source.links,'$.continued_in') WHERE value=target.native_id))",params![self.ns,path,identity],|r|r.get(0))
    }
    fn owner(&self, path: &str) -> rusqlite::Result<String> {
        self.db.query_row("SELECT session_key FROM main.catalog_semantic_owner_sources WHERE namespace=?1 AND source_path=?2",params![self.ns,path],|r|r.get(0))
    }
    fn qualify(&self, path: &str, key: &str, owner: &str, copied: bool) -> rusqlite::Result<()> {
        self.db.execute("UPDATE main.catalog_semantic_events SET owner=?4,copied=?5,qualified=1 WHERE namespace=?1 AND source_path=?2 AND event_key=?3",params![self.ns,path,key,owner,copied])?;
        Ok(())
    }
    fn done(&self) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.catalog_semantic_events WHERE namespace=?1 AND qualified=0)",[self.ns],|r|r.get(0))
    }
}

fn event_page(layout: &FrontierLayout<'_>, ns: &str) -> Result<bool, StoreError> {
    let c = layout.db;
    let unsupported: bool=c.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_inputs WHERE namespace=?1 AND (harness NOT IN ('codex','claude') OR (harness='claude' AND is_top=0)))",[ns],|r|r.get(0)).map_err(error)?;
    if unsupported {
        return Err(refuse("semantic_nonprimitive_event_harness"));
    }
    crate::events::store::semantic_stage::event_page_with(&ScopedProjection { db: c, ns, layout })
        .map_err(error)
}

fn native_keys_page(c: &Connection, ns: &str) -> Result<bool, StoreError> {
    let pairs=c.prepare("SELECT harness,native_id FROM main.catalog_semantic_inputs WHERE namespace=?1 GROUP BY harness,native_id ORDER BY harness,native_id LIMIT 65").map_err(error)?.query_map([ns],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
    if pairs.len() > 64 {
        return Err(refuse("semantic_native_key_component_budget"));
    }
    for (harness, native) in pairs {
        // An outgoing source already observed in this epoch is replaced by
        // the staged input cohort. Its old Current row is custody, not a live
        // collision. The publication CAS still owns its history transfer.
        let collision: bool=c.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_inputs WHERE namespace=?1 AND native_id=?2 AND harness<>?3 UNION ALL SELECT 1 FROM main.session_catalog_sources existing WHERE existing.namespace=?1 AND existing.native_id=?2 AND existing.harness<>?3 AND existing.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources observed WHERE observed.namespace=existing.namespace AND observed.source_path=existing.source_path AND observed.index_done=1))",params![ns,native,harness],|r|r.get(0)).map_err(error)?;
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

fn frontier_page(layout: &FrontierLayout<'_>, ns: &str) -> Result<bool, StoreError> {
    let c = layout.db;
    let mut budget = 64usize;
    while budget > 0 {
        let task: Option<(String,String,String,String)>=c.query_row("SELECT kind,value,harness,after_key FROM main.catalog_semantic_frontier WHERE namespace=?1 AND done=0 ORDER BY kind,value,harness LIMIT 1",[ns],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(error)?;
        let Some((kind, value, harness, after)) = task else {
            return Ok(true);
        };
        // Member discovery yields after 32 observations, independently of the
        // 64-source component bound. This leaves a durable continuation even
        // for valid components and keeps name-triggered replay ticks bounded.
        if kind == "members" {
            budget = budget.min(32);
        }
        let sql = match kind.as_str() {
            "members" => {
                "SELECT source_path FROM main.session_catalog_sources WHERE namespace=?1 AND session_key=?2 AND lifecycle='current' AND source_path>?3 ORDER BY source_path LIMIT ?4"
            }
            "pathowner" => {
                "SELECT peer.source_path FROM main.session_catalog_sources own JOIN main.session_catalog_sources peer ON peer.namespace=own.namespace AND peer.session_key=own.session_key WHERE own.namespace=?1 AND own.source_path=?2 AND own.lifecycle='current' AND peer.lifecycle='current' AND peer.source_path>?3 GROUP BY peer.source_path ORDER BY peer.source_path LIMIT ?4"
            }
            "native" => {
                // Both are authored current profiles. The singleton family is
                // read only through its admitted physical owner, never unscoped.
                "SELECT source_path FROM (SELECT source_path FROM main.session_catalog_sources WHERE namespace=?1 AND native_id=?2 AND lifecycle='current' AND harness=?5 UNION SELECT source_path FROM main.catalog_native_dependencies WHERE namespace=?1 AND native_id=?2 AND harness='lineage:'||?5) WHERE source_path>?3 GROUP BY source_path ORDER BY source_path LIMIT ?4"
            }
            "receipt" => {
                "SELECT f.path FROM main.events e JOIN main.files f ON f.namespace=e.namespace AND f.file_id=e.file_id WHERE e.namespace=?1 AND f.namespace=?1 AND e.k='xsm' AND e.id=?2 AND f.path>?3 GROUP BY f.path ORDER BY f.path LIMIT ?4"
            }
            "sender" => {
                "SELECT f.path FROM main.events e JOIN main.files f ON f.namespace=e.namespace AND f.file_id=e.file_id WHERE e.namespace=?1 AND f.namespace=?1 AND e.k='tool' AND e.n='SendMessage' AND e.reply_m=?2 AND f.path>?3 GROUP BY f.path ORDER BY f.path LIMIT ?4"
            }
            "dependency" => {
                "SELECT source_path FROM main.catalog_native_dependencies WHERE namespace=?1 AND native_id=?2 AND harness=?5 AND source_path>?3 GROUP BY source_path ORDER BY source_path LIMIT ?4"
            }
            "claudeid" => {
                "SELECT source_path FROM (SELECT path source_path FROM main.files WHERE namespace=?1 AND substr(path,length(rtrim(path,replace(path,'/','')))+1)=?2 UNION SELECT source_path FROM main.session_catalog_invalidations WHERE namespace=?1 AND substr(source_path,length(rtrim(source_path,replace(source_path,'/','')))+1)=?2) WHERE source_path>?3 ORDER BY source_path LIMIT ?4"
            }
            "copilotid" => {
                "SELECT source_path FROM (SELECT path source_path FROM main.files WHERE namespace=?1 AND path GLOB '*/session-state/*/events.jsonl' AND substr(substr(path,1,length(path)-13),length(rtrim(substr(path,1,length(path)-13),replace(substr(path,1,length(path)-13),'/','')))+1)=?2 UNION SELECT source_path FROM main.session_catalog_invalidations WHERE namespace=?1 AND source_path GLOB '*/session-state/*/events.jsonl' AND substr(substr(source_path,1,length(source_path)-13),length(rtrim(substr(source_path,1,length(source_path)-13),replace(substr(source_path,1,length(source_path)-13),'/','')))+1)=?2) WHERE source_path>?3 ORDER BY source_path LIMIT ?4"
            }
            "nativeall" => {
                "SELECT source_path FROM main.session_catalog_sources WHERE namespace=?1 AND native_id=?2 AND source_path>?3 AND lifecycle='current' GROUP BY source_path ORDER BY source_path LIMIT ?4"
            }
            "headerid" => {
                "SELECT substr(m.key,length('native_codex_header:')+1) FROM main.meta m INDEXED BY semantic_codex_header_native WHERE m.namespace=?1 AND m.key GLOB 'native_codex_header:*' AND json_valid(m.value) AND json_extract(m.value,'$.header.id')=?2 AND substr(m.key,length('native_codex_header:')+1)>?3 ORDER BY m.key LIMIT ?4"
            }
            "headerparent" => {
                "SELECT substr(m.key,length('native_codex_header:')+1) FROM main.meta m INDEXED BY semantic_codex_header_parent WHERE m.namespace=?1 AND m.key GLOB 'native_codex_header:*' AND json_valid(m.value) AND json_extract(m.value,'$.header.metadata.parent_thread')=?2 AND substr(m.key,length('native_codex_header:')+1)>?3 ORDER BY m.key LIMIT ?4"
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
        let claims = super::source_journal::claims(c, ns, &paths).map_err(error)?;
        for (path, claim) in claims {
            c.execute("INSERT OR IGNORE INTO main.catalog_semantic_sources(namespace,source_path,claim) VALUES(?1,?2,?3)",params![ns,path,claim]).map_err(error)?;
        }
        let last = paths.last().unwrap_or(&after);
        c.execute("UPDATE main.catalog_semantic_frontier SET after_key=?5,done=?6 WHERE namespace=?1 AND kind=?2 AND value=?3 AND harness=?4",params![ns,kind,value,harness,last,paths.len()<budget]).map_err(error)?;
        budget = budget.saturating_sub(paths.len().max(1));
    }
    c.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.catalog_semantic_frontier WHERE namespace=?1 AND done=0)",[ns],|r|r.get(0)).map_err(error)
}

struct AdmittedLookup<'a> {
    db: &'a Connection,
    ns: &'a str,
    replay: Vec<String>,
}

fn member_capacity(c: &Connection, ns: &str, replay: &[String]) -> Result<i64, StoreError> {
    let keys = serde_json::to_string(replay).map_err(|e| StoreError::Data(e.to_string()))?;
    c.query_row("SELECT count(*) FROM (SELECT DISTINCT source_path FROM (SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 UNION ALL SELECT member.source_path FROM json_each(?2) requested JOIN main.session_catalog_sources member ON member.namespace=?1 AND member.session_key=requested.value AND member.lifecycle='current') LIMIT 65)", params![ns,keys], |r| r.get(0)).map_err(error)
}

#[cfg(test)]
pub(crate) fn name_members_vm_probe(
    c: &Connection,
    ns: &str,
    name: &str,
) -> Vec<(String, usize, usize)> {
    use crate::model::relation_stage::SelectorLookup;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let tx = c.unchecked_transaction().unwrap();
    let steps = Arc::new(AtomicUsize::new(0));
    let observed = steps.clone();
    c.progress_handler(
        1,
        Some(move || {
            observed.fetch_add(1, Ordering::Relaxed);
            false
        }),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let layout = FrontierLayout::bind(&tx).unwrap();
    c.progress_handler(0, None::<fn() -> bool>).unwrap();
    let setup = steps.load(Ordering::Relaxed);
    assert_eq!(setup, 0, "current layout binding must perform no SQL");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    eprintln!("name_members_setup vm_steps={setup}");
    let mut result = vec![("setup".into(), setup, 0)];
    let mut targets = Vec::new();
    for kind in ["hints", "candidates", "capacity", "members"] {
        tx.execute_batch("SAVEPOINT name_members_probe").unwrap();
        let steps = Arc::new(AtomicUsize::new(0));
        let observed = steps.clone();
        c.progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let lookup = AdmittedLookup {
            db: &tx,
            ns,
            replay: Vec::new(),
        };
        let rows = match kind {
            "hints" => {
                targets = lookup.name_hints(name).unwrap();
                targets.len()
            }
            "candidates" => lookup.name_candidates(name).unwrap().len(),
            "capacity" => member_capacity(&tx, ns, &targets).unwrap() as usize,
            _ => {
                for target in &targets {
                    tx.execute("INSERT INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,'members',?2,'') ON CONFLICT(namespace,kind,value,harness) DO UPDATE SET done=0,after_key=''", params![ns,target]).unwrap();
                }
                frontier_page(&layout, ns).unwrap();
                staged_paths(&tx, ns).unwrap().len()
            }
        };
        c.progress_handler(0, None::<fn() -> bool>).unwrap();
        let steps = steps.load(Ordering::Relaxed);
        assert!(steps > 0 && steps <= 4096, "{kind}: {steps}");
        assert!(rows <= 64);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        eprintln!("name_members_query kind={kind} vm_steps={steps} rows={rows}");
        result.push((kind.into(), steps, rows));
        tx.execute_batch("ROLLBACK TO name_members_probe; RELEASE name_members_probe")
            .unwrap();
    }
    result
}

impl crate::model::relation_stage::SelectorLookup for AdmittedLookup<'_> {
    type Error = rusqlite::Error;
    fn invalid(&self, message: &'static str) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(message)))
    }
    fn native_key(
        &self,
        native: &str,
        harness: &str,
    ) -> rusqlite::Result<crate::model::relation_stage::Selection> {
        let key: Option<String> = self.db.query_row("SELECT session_key FROM main.catalog_semantic_keys WHERE namespace=?1 AND harness=?2 AND native_id=?3", params![self.ns,harness,native], |r| r.get(0)).optional()?;
        Ok(crate::model::relation_stage::Selection {
            count: usize::from(key.is_some()),
            key,
            ..Default::default()
        })
    }
    fn receipt_first(&self, id: &str) -> rusqlite::Result<Option<Option<String>>> {
        self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.catalog_semantic_events WHERE namespace=?1 AND msg_id=?2 AND kind='xsm' AND copied=0 AND qualified=1 ORDER BY source_path,native_seq LIMIT 1", params![self.ns, id], |r| r.get(0)).optional()
    }
    fn call_owners(&self, id: &str) -> rusqlite::Result<Vec<(String, String)>> {
        self.db.prepare("SELECT source_path,event_key FROM main.catalog_semantic_events WHERE namespace=?1 AND kind='call' AND tool_id=?2 AND copied=0 AND qualified=1 ORDER BY source_path,event_key LIMIT 2")?.query_map(params![self.ns, id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    }
    fn event_metadata(&self, path: &str, key: &str) -> rusqlite::Result<Option<String>> {
        self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.catalog_semantic_events WHERE namespace=?1 AND source_path=?2 AND event_key=?3", params![self.ns, path, key], |r| r.get(0))
    }
    fn event_owner(&self, path: &str) -> rusqlite::Result<String> {
        self.db.query_row("SELECT session_key FROM main.catalog_semantic_owner_sources WHERE namespace=?1 AND source_path=?2", params![self.ns, path], |r| r.get(0))
    }
    fn endpoint_row(
        &self,
        key: &str,
    ) -> rusqlite::Result<crate::model::relation_stage::SelectorEndpointRow> {
        self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,parent_key,root_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2", params![self.ns, key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
    }
    fn root_alive(&self, key: &str) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT COALESCE(json_extract(metadata,'$.alive'),0) FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2", params![self.ns, key], |r| r.get(0)).optional().map(|v| v.unwrap_or(false))
    }
    fn name_candidates(&self, name: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM (SELECT n.session_key FROM main.catalog_semantic_names n JOIN main.catalog_semantic_endpoints e ON e.namespace=n.namespace AND e.session_key=n.session_key WHERE n.namespace=?1 AND e.namespace=?1 AND n.name=?2 AND e.harness='claude' AND json_extract(e.metadata,'$.row.kind')='session' UNION SELECT e.root_key FROM main.catalog_semantic_events sent JOIN main.catalog_semantic_events receipt ON receipt.namespace=sent.namespace AND receipt.msg_id=sent.msg_id JOIN main.catalog_semantic_endpoints e ON e.namespace=sent.namespace AND e.session_key=sent.owner JOIN main.catalog_semantic_endpoints root ON root.namespace=e.namespace AND root.session_key=e.root_key WHERE sent.namespace=?1 AND receipt.namespace=?1 AND e.namespace=?1 AND root.namespace=?1 AND sent.kind='send' AND receipt.kind='xsm' AND sent.copied=0 AND receipt.copied=0 AND sent.qualified=1 AND receipt.qualified=1 AND receipt.name=?2 AND (receipt.source_path,receipt.native_seq)=(SELECT first.source_path,first.native_seq FROM main.catalog_semantic_events first WHERE first.namespace=?1 AND first.kind='xsm' AND first.msg_id=sent.msg_id AND first.copied=0 AND first.qualified=1 ORDER BY first.source_path,first.native_seq LIMIT 1) AND json_extract(root.metadata,'$.row.kind')='session') ORDER BY session_key LIMIT 2")?.query_map(params![self.ns, name], |r| r.get(0))?.collect()
    }
    fn name_hints(&self, name: &str) -> rusqlite::Result<Vec<String>> {
        let paths = self.db.prepare("SELECT CASE WHEN octet_length(source_path)<=1048576 THEN source_path END FROM main.catalog_native_dependencies INDEXED BY catalog_native_dependency WHERE namespace=?1 AND harness='name:claude' AND native_id=?2 ORDER BY source_path LIMIT 65")?.query_map(params![self.ns,name], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if paths.len() > 64 || paths.iter().map(String::len).sum::<usize>() > 1048576 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let mut keys = std::collections::BTreeSet::new();
        let mut observations = 0usize;
        for path in paths {
            let members = self.db.prepare("SELECT member.session_key FROM main.session_catalog_sources member JOIN main.session_catalog owner ON owner.namespace=member.namespace AND owner.session_key=member.session_key WHERE member.namespace=?1 AND member.source_path=?2 AND member.lifecycle='current' AND owner.namespace=?1 AND owner.lifecycle='current' AND owner.harness='claude' AND (NOT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints stage WHERE stage.namespace=?1 AND stage.session_key=member.session_key) OR EXISTS(SELECT 1 FROM main.session_catalog_sources peer WHERE peer.namespace=?1 AND peer.session_key=member.session_key AND peer.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources staged WHERE staged.namespace=?1 AND staged.source_path=peer.source_path AND staged.index_done=1))) LIMIT 65")?.query_map(params![self.ns,path], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            observations += members.len();
            if observations > 64 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            keys.extend(members);
        }
        Ok(keys.into_iter().collect())
    }
    fn codex_sibling(&self, parent: &str, task: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND harness='codex' AND parent_native=?2 AND task=?3 LIMIT 2")?.query_map(params![self.ns, parent, task], |r| r.get(0))?.collect()
    }
    fn codex_relay(&self, root: &str, target: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND harness='codex' AND root_key=?2 AND (nickname=?3 OR EXISTS(SELECT 1 FROM main.catalog_semantic_paths p WHERE p.namespace=?1 AND p.session_key=catalog_semantic_endpoints.session_key AND p.root_key=?2 AND p.path=?3)) LIMIT 2")?.query_map(params![self.ns, root, target], |r| r.get(0))?.collect()
    }
    fn codex_wait(&self, root: &str, target: &str, exclude: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND harness='codex' AND root_key=?2 AND session_key<>?4 AND (session_key=?3 OR EXISTS(SELECT 1 FROM main.catalog_semantic_paths p WHERE p.namespace=?1 AND p.session_key=catalog_semantic_endpoints.session_key AND p.root_key=?2 AND p.path=?3)) LIMIT 2")?.query_map(params![self.ns, root, target, exclude], |r| r.get(0))?.collect()
    }
    fn request_members_replay(&mut self, keys: Vec<String>) -> rusqlite::Result<()> {
        self.replay = keys;
        Ok(())
    }
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
        "SELECT value FROM meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
        [],
        |r| r.get(0),
    )
    .optional()
}

impl Stage {
    /// Fixture access to the real admitted producer, with its ordinary roles
    /// and namespace guards. Opening the Stage performs the production checks.
    #[cfg(test)]
    pub(crate) fn into_test_connection(self) -> Connection {
        self.db
    }

    pub(crate) fn publish(
        &mut self,
        batch: &Batch,
        prepared: &PreparedSources,
    ) -> Result<(Outcome, Option<String>), StoreError> {
        use crate::storage::catalog::CatalogPublicationHead;
        use sha2::{Digest, Sha256};
        #[cfg(test)]
        let _diagnostic =
            PublicationDiagnostic(PUBLICATION_PHASE.with(|phase| phase.replace(Some("prepare"))));
        if batch.phase != Phase::Publication || batch.base_generation != prepared.base_generation {
            return Ok((Outcome::Conflict, None));
        }
        if prepared.rows.len() > 64
            || prepared.claims.len() > 64
            || (prepared.rows.is_empty()
                && prepared.withdrawals.is_empty()
                && prepared.relationships.retired_owners.is_empty())
        {
            return Err(refuse("semantic_publication_component_budget"));
        }
        if prepared
            .rows
            .iter()
            .any(|row| row.query.is_some() && row.trace.is_none())
        {
            return Err(refuse("semantic_publication_trace_missing"));
        }
        let projection_rows: std::collections::BTreeMap<_, _> =
            prepared.rows.iter().map(|r| (r.key.as_str(), r)).collect();
        let mut projection_keys = std::collections::BTreeSet::new();
        let mut real_projection_keys = std::collections::BTreeSet::new();
        let mut slot_bytes = 0usize;
        for projection in &prepared.transcripts {
            // 64 bounds source/owner admission, not a native transcript's
            // length. Slot work is bounded by the aggregate byte budget below,
            // as the shared writer accepts complete multi-page transcripts.
            if !projection_keys.insert(&projection.key) {
                return Err(refuse("semantic_publication_projection_budget"));
            }
            let row = projection_rows.get(projection.key.as_str());
            if row.is_some() {
                real_projection_keys.insert(&projection.key);
            } else {
                // Ordinary slot publication skips virtual trace endpoints. Keep
                // their exact model witness, but never give them Raw membership
                // or a Catalog/slot row. The real anchor is independently checked
                // against this namespace's staged identity below, and the entire
                // relation/projection payload must match the recorded witness.
                let anchored_stub = prepared.relationships.relations.values().any(|relation| {
                    relation.stubs.iter().any(|stub| stub.key == projection.key)
                        && (relation.from == projection.key
                            || relation.to.as_ref() == Some(&projection.key))
                        && std::iter::once(&relation.from)
                            .chain(relation.to.iter())
                            .any(|key| {
                                projection_rows.contains_key(key.as_str())
                                    && prepared.relationships.endpoints.contains_key(key)
                            })
                });
                if !anchored_stub {
                    return Err(refuse("semantic_publication_projection_owner"));
                }
            }
            for slot in &projection.slots {
                if slot
                    .source_paths()
                    .iter()
                    .any(|path| row.is_none_or(|row| !row.sources.iter().any(|s| &s.path == *path)))
                {
                    return Err(refuse("semantic_publication_projection_source"));
                }
                slot_bytes = slot_bytes.saturating_add(
                    serde_json::to_vec(slot)
                        .map_err(|e| StoreError::Data(e.to_string()))?
                        .len(),
                );
                if slot_bytes > 1048576 {
                    return Err(refuse("semantic_publication_projection_budget"));
                }
            }
        }
        if !prepared.transcripts.is_empty() && real_projection_keys.len() != projection_rows.len() {
            return Err(refuse("semantic_publication_projection_owner"));
        }
        let projection_witness = prepared
            .transcripts
            .iter()
            .map(|p| (&p.key, &p.slots, p.repair))
            .collect::<Vec<_>>();
        let producer_witness = prepared
            .staged_model_witness()
            .map_err(|e| StoreError::Data(e.to_string()))?;
        if producer_witness.len() > 2 * 1048576 {
            return Err(refuse("semantic_context_budget"));
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
                    &metadata_text,
                    &projection_witness,
                    &producer_witness
                ))
                .map_err(|e| StoreError::Data(e.to_string()))?
            )
        );
        publication_phase("admission_and_witnesses");
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
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=stage_epoch");
            return Ok((Outcome::Conflict, None));
        }
        if !lifecycle_current(prepared) {
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=lifecycle_authority");
            return Ok((Outcome::Conflict, None));
        }
        let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM storage_event_operations WHERE namespace=?1 AND state='intent')",[ns],|r|r.get(0)).map_err(error)?;
        if pending {
            return Err(refuse("semantic_publication_recovery_intent_pending"));
        }
        if !reduction_witnesses(&tx, ns).map_err(error)?.current()? {
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=reduction_witness");
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
                && super::proof_generations::replayed_presentation(
                    &tx,
                    ns,
                    &generation,
                    &proof_digest,
                    &digest,
                )
                .map_err(StoreError::Configured)?
                && super::source_journal::claims(&tx, ns, &batch.paths)
                    .map_err(error)?
                    .iter()
                    .all(|(_, claim)| claim.is_none())
            {
                return Ok((Outcome::Written, Some(generation)));
            }
            return Ok((Outcome::Conflict, None));
        }
        if previous != batch.base_generation || self.source.generation != batch.base_generation {
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=base_generation");
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
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=source_revision");
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
            || super::source_journal::claims(&tx, ns, &staged).map_err(error)? != prepared.claims
        {
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=source_membership_claims");
            return Ok((Outcome::Conflict, None));
        }
        let unfinished:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND (index_done=0 OR events_done=0 OR identity_done=0 OR relation_done=0)) OR EXISTS(SELECT 1 FROM main.catalog_semantic_frontier WHERE namespace=?1 AND done=0) OR EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND (parent_resolved=0 OR root_key IS NULL OR cost_done=0))",[ns],|r|r.get(0)).map_err(error)?;
        if unfinished {
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=unfinished");
            return Ok((Outcome::Conflict, None));
        }
        let mut covered = validate_withdrawals(&tx, &self.source, batch, prepared)?;
        covered.extend(removed_paths(&tx, ns).map_err(error)?);
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
            let _ = endpoint;
            let endpoint = owners::Owners {
                db: &tx,
                namespace: ns,
            }
            .endpoint(&row.key)
            .map_err(error)?;
            // Identity is independently staged before final Query/slot capture.
            // Nonempty model projections additionally require the exact durable
            // producer witness recorded under this epoch, generation and claims.
            let mut identity = row.clone();
            identity.query = None;
            if serde_json::to_string(&endpoint.row).map_err(|e| StoreError::Data(e.to_string()))?
                != serde_json::to_string(&identity).map_err(|e| StoreError::Data(e.to_string()))?
            {
                #[cfg(test)]
                eprintln!("semantic_publication_conflict reason=staged_identity");
                return Ok((Outcome::Conflict, None));
            }
            let old:Option<String>=tx.query_row("SELECT metadata FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",params![ns,row.key],|r|r.get(0)).optional().map_err(error)?;
            unchanged &= old.as_ref() == Some(encoded);
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
            #[cfg(test)]
            eprintln!("semantic_publication_conflict reason=source_coverage");
            return Ok((Outcome::Conflict, None));
        }
        let retired = retirement_plan(&tx, &self.source, prepared)?;
        if prepared
            .rows
            .iter()
            .any(|row| row.query.is_some() || row.trace.is_some())
            || !prepared.transcripts.is_empty()
            || !prepared.withdrawals.is_empty()
            || !prepared.relationships.relations.is_empty()
            || !prepared.pids.is_empty()
            || !prepared.lineage_dependencies.is_empty()
            || !prepared.relationships.retired_owners.is_empty()
        {
            let witness = prepared
                .staged_model_witness()
                .map_err(|e| StoreError::Data(e.to_string()))?;
            let exact: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM storage_semantic_model_batches WHERE namespace=?1 AND epoch=?2 AND metadata=?3 AND projections=?4 AND claims=?5)",params![ns,batch.epoch,metadata_text,witness,claims_text],|r|r.get(0)).map_err(error)?;
            if !exact {
                #[cfg(test)]
                eprintln!("semantic_publication_conflict reason=staged_model_witness");
                return Ok((Outcome::Conflict, None));
            }
        }
        // All stale/scope/Raw/claim checks precede the first durable write.
        let old_epoch:Option<String>=tx.query_row("SELECT value FROM main.meta WHERE namespace=?1 AND key='catalog_publication_epoch'",[ns],|r|r.get(0)).optional().map_err(error)?;
        let mut projections_unchanged = true;
        let trace_effects = super::trace::TraceWriter::admitted(&tx, &self.source)
            .map_err(StoreError::Configured)?;
        for row in &prepared.rows {
            if let Some(trace) = &row.trace {
                projections_unchanged &=
                    crate::causal_trace::is_published(&trace_effects, row, trace).map_err(error)?;
            }
        }
        for projection in &prepared.transcripts {
            let Some(row) = projection_rows.get(projection.key.as_str()) else {
                // Virtual stubs are witness-only, just as in publish_slot_rows.
                continue;
            };
            let slots = projection
                .slots
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| StoreError::Data(e.to_string()))?;
            let expected = crate::slot_projection::generation(&slots, &row.sources)
                .map_err(|e| StoreError::Data(e.to_string()))?;
            let actual:Option<String> = tx.query_row("SELECT generation FROM main.session_slot_projections WHERE namespace=?1 AND session_key=?2 AND version=?3",params![ns,projection.key,crate::slot_projection::VERSION],|r|r.get(0)).optional().map_err(error)?;
            projections_unchanged &= actual.as_ref() == Some(&expected);
        }
        let repeat = unchanged
            && projections_unchanged
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
            let mut generation_metadata = metadata.clone();
            if !projection_witness.is_empty() {
                generation_metadata.push(
                    serde_json::to_string(&projection_witness)
                        .map_err(|e| StoreError::Data(e.to_string()))?,
                );
            }
            if prepared.rows.iter().any(|row| row.trace.is_some())
                || !prepared.withdrawals.is_empty()
                || !prepared.relationships.relations.is_empty()
                || !prepared.pids.is_empty()
                || !prepared.lineage_dependencies.is_empty()
                || !prepared.relationships.retired_owners.is_empty()
            {
                generation_metadata.push(producer_witness.clone());
            }
            crate::storage::catalog::component_generation(
                &version,
                previous.as_deref(),
                &epoch,
                &generation_metadata,
            )
        };
        publication_phase("original_history");
        let history = super::history::OriginalHistory::admitted(&tx, &self.source)
            .map_err(StoreError::Configured)?;
        if !crate::storage::catalog::preserve_rows(
            &history,
            &prepared.rows,
            &prepared.relationships.retired_owners,
        )
        .map_err(error)?
        {
            // Only private custody checkpoints commit; Current and its claims
            // remain owned by the original generation until custody completes.
            tx.commit().map_err(error)?;
            return Ok((Outcome::Deferred, None));
        }
        publication_phase("composite_history");
        crate::storage::catalog::publish_history_rows(
            &history,
            &prepared.rows,
            Some(&prepared.transcripts),
        )
        .map_err(error)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_history").map_err(error)?;
        publication_phase("slot_effects");
        if !prepared.transcripts.is_empty() {
            crate::storage::catalog::publish_slot_rows(
                &super::publication::SlotWriter::admitted(&tx, &self.source)
                    .map_err(StoreError::Configured)?,
                &prepared.rows,
                &prepared.transcripts,
            )?;
            tx.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,'slot_projection_catalog_generation',?2) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![ns,generation]).map_err(error)?;
        } else if !repeat {
            tx.execute("DELETE FROM main.meta WHERE namespace=?1 AND key='slot_projection_catalog_generation'",[ns]).map_err(error)?;
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_slots").map_err(error)?;
        publication_phase("current_effects");
        let current_effects = super::publication::CurrentWriter::admitted(&tx, &self.source)
            .map_err(StoreError::Configured)?;
        for (row, encoded) in prepared.rows.iter().zip(&metadata) {
            use crate::storage::catalog::CurrentPublicationStore;
            if current_effects.current(row, encoded).map_err(error)? {
                crate::storage::catalog::publish_current_sources(&current_effects, row)
                    .map_err(error)?;
            }
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_rows").map_err(error)?;
        publication_phase("owner_effects");
        let owner_effects = owners::Owners {
            db: &tx,
            namespace: ns,
        };
        for endpoint in prepared.relationships.endpoints.values() {
            // Current membership replacement clears runtime dependencies. Add
            // hints only from this newly qualified native projection, inside
            // the same publication fence, rather than retaining stale names.
            crate::model::relation_stage::native_name_hints_with(
                endpoint,
                || rusqlite::Error::InvalidQuery,
                |path, name| owner_effects.dependency(path, "name:claude", name),
            )
            .map_err(error)?;
            for source in &endpoint.row.sources {
                if let Some(pid) = endpoint.pid {
                    owner_effects
                        .dependency(
                            source.path.to_string_lossy().as_ref(),
                            "runtime:pid",
                            &pid.to_string(),
                        )
                        .map_err(error)?;
                }
            }
        }
        for relation in prepared.relationships.relations.values() {
            owner_effects.relation(relation).map_err(error)?;
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_owners").map_err(error)?;
        for (key, withdrawal) in &retired {
            OwnerStore::retire(&owner_effects, key, *withdrawal).map_err(error)?;
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_retirement").map_err(error)?;
        publication_phase("trace_effects");
        let trace = super::trace::TraceWriter::admitted(&tx, &self.source)
            .map_err(StoreError::Configured)?;
        for row in &prepared.rows {
            if let Some(projection) = &row.trace {
                crate::causal_trace::publish_with_store(&trace, row, projection).map_err(error)?;
            }
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_trace").map_err(error)?;
        publication_phase("search_effects");
        crate::catalog_search::publish_current_rows_with_store(
            &tx,
            &prepared.rows.iter().collect::<Vec<_>>(),
            &super::search::SearchWriter::admitted(&tx, &self.source)
                .map_err(StoreError::Configured)?,
        )
        .map_err(error)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_search").map_err(error)?;
        if !retired.is_empty() {
            use crate::storage::catalog::SearchPublicationStore;
            let search = super::search::SearchWriter::admitted(&tx, &self.source)
                .map_err(StoreError::Configured)?;
            let mut rows = Vec::new();
            for (key, _) in &retired {
                let text: String = tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_catalog WHERE namespace=?1 AND session_key=?2",params![ns,key],|r|r.get(0)).map_err(error)?;
                rows.push(
                    serde_json::from_str::<crate::model::summary::CatalogRow>(&text)
                        .map_err(|e| StoreError::Data(e.to_string()))?,
                );
            }
            search
                .begin(&rows.iter().collect::<Vec<_>>())
                .map_err(error)?;
            for row in &rows {
                search.validate(&row.key).map_err(error)?;
            }
            search
                .remove(crate::CatalogReadScope::Current)
                .map_err(error)?;
        }
        publication_phase("publication_proof");
        let selected_observed_at = crate::model::now_ms();
        let presentations = super::proof_generations::capture_presentations(
            &tx,
            ns,
            &generation,
            prepared,
            selected_observed_at,
        )
        .map_err(StoreError::Configured)?;
        let proof_digest = super::proof_generations::bind_presentations(
            &tx,
            ns,
            &generation,
            &proof_digest,
            &presentations,
            prepared,
        )
        .map_err(StoreError::Configured)?;
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
        super::proof_generations::publish(
            &tx,
            ns,
            self.source.admission,
            self.source
                .config
                .expected_stream_epoch
                .as_deref()
                .ok_or_else(|| refuse("semantic_stream_epoch"))?,
            &generation,
            &proof_digest,
            super::proof_generations::PreparedPresentations {
                prepared,
                rows: &presentations,
            },
        )
        .map_err(StoreError::Configured)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_generation_refs")
            .map_err(error)?;
        super::proof_generations::collect(&tx, ns, 16).map_err(error)?;
        publication_phase("claim_ack");
        for (path, claim) in &prepared.claims {
            if let Some(claim) = claim
                && tx.execute("DELETE FROM main.session_catalog_invalidations WHERE namespace=?1 AND source_path=?2 AND revision=?3",params![ns,path,claim]).map_err(error)? != 1
            {
                return Err(refuse("semantic_publication_claim_changed"));
            }
        }
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_publish_ack").map_err(error)?;
        publication_phase("head_and_hints");
        publication_head
            .publish_generation(&generation, &version)
            .map_err(StoreError::Configured)?;
        tx.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,'catalog_publication_epoch',?2),(?1,'catalog_observed_at',?3),(?1,'catalog_completeness','partial') ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",params![ns,epoch,selected_observed_at.to_string()]).map_err(error)?;
        // Readiness is solely for the private listing cohort. No global
        // completeness claim can qualify selected/proof/search readers.
        tx.execute(
            "DELETE FROM main.meta WHERE namespace=?1 AND key='semantic_listing_generation'",
            [ns],
        )
        .map_err(error)?;
        if prepared.rows.iter().all(|row| row.query.is_some())
            && projection_keys.len() == prepared.rows.len()
        {
            let complete: bool = tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.files f WHERE f.namespace=?1 AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources s WHERE s.namespace=f.namespace AND s.source_path=f.path)) AND NOT EXISTS(SELECT 1 FROM main.session_catalog c WHERE c.namespace=?1 AND c.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints e WHERE e.namespace=c.namespace AND e.session_key=c.session_key))",[ns],|r|r.get(0)).map_err(error)?;
            if complete {
                tx.execute("INSERT INTO main.meta(namespace,key,value) VALUES(?1,'semantic_listing_generation',?2)",params![ns,generation]).map_err(error)?;
            }
        }
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
        publication_phase("commit");
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
        tx.execute(
            "DELETE FROM storage_semantic_model_batches WHERE namespace=?1",
            [ns],
        )
        .map_err(error)?;
        // A repair stage temporarily publishes a partial head. Once its exact
        // claims are acknowledged, preserve an already completed discovery;
        // otherwise the observer starts recovery and rediscovers unchanged
        // sources solely because this representation was repaired.
        tx.execute(
            "UPDATE main.meta SET value='complete' WHERE namespace=?1 AND key='catalog_completeness' AND NOT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=?1) AND EXISTS(SELECT 1 FROM main.storage_source_journal WHERE namespace=?1 AND backfill_state='complete')",
            [ns],
        ).map_err(error)?;
        tx.commit().map_err(error)?;
        self.epoch = None;
        Ok(Outcome::Written)
    }

    pub(crate) fn select(
        &mut self,
        epoch: i64,
        selector: &crate::model::relation_stage::Selector,
    ) -> Result<crate::model::relation_stage::Selection, StoreError> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.namespace();
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND resetting=0)",params![ns,epoch],|r|r.get(0)).map_err(error)?;
        if !valid {
            return Err(refuse("stale semantic selector"));
        }
        if self.epoch != Some(epoch)
            || head(&tx).map_err(error)? != self.source.generation
            || !reduction_witnesses(&tx, ns).map_err(error)?.current()?
        {
            return Err(refuse("stale semantic selector"));
        }
        if matches!(
            selector,
            crate::model::relation_stage::Selector::ClaudeReceipt { .. }
                | crate::model::relation_stage::Selector::ClaudeCall { .. }
                | crate::model::relation_stage::Selector::ClaudeName { .. }
        ) {
            // A negative is authoritative only after discovery, reduction and
            // the separate copied-event qualification sub-phase all complete.
            let complete: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND phase IN ('relations','publication')) AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND (index_done=0 OR events_done=0 OR identity_done=0)) AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_events WHERE namespace=?1 AND qualified=0) AND NOT EXISTS(SELECT 1 FROM main.catalog_semantic_frontier WHERE namespace=?1 AND done=0)",[ns],|r|r.get(0)).map_err(error)?;
            if !complete {
                return Err(refuse("semantic_claude_event_projection_pending"));
            }
        }
        if matches!(
            selector,
            crate::model::relation_stage::Selector::ClaudeName { .. }
        ) {
            let current: bool = tx.query_row("SELECT revision=(SELECT revision FROM storage_source_journal WHERE namespace=?1) FROM main.catalog_semantic_stage WHERE namespace=?1", [ns], |r| r.get(0)).map_err(error)?;
            if !current {
                return Err(refuse("stale semantic selector"));
            }
            for path in staged_paths(&tx, ns)? {
                let Some((_, ledger)) =
                    crate::events::store::ledger_row(&tx, &path).map_err(error)?
                else {
                    return Err(refuse("stale semantic selector"));
                };
                if crate::events::Stat::of(std::path::Path::new(&path))
                    .ok()
                    .as_ref()
                    != Some(&ledger.stat)
                {
                    return Err(refuse("stale semantic selector"));
                }
            }
        }
        let mut lookup = AdmittedLookup {
            db: &tx,
            ns,
            replay: Vec::new(),
        };
        let result = crate::model::relation_stage::select_with(&mut lookup, selector);
        let replay = lookup.replay;
        if !replay.is_empty() {
            // Capacity is a property of the complete requested component,
            // not of this tick's discovery page. Check it before committing
            // an unfinished cursor or admitting any Raw preparation.
            if member_capacity(&tx, ns, &replay)? > 64 {
                return Err(refuse("semantic_affected_owner_source_budget"));
            }
            crate::model::relation_stage::replay_members_with(
                &replay,
                || rusqlite::Error::InvalidQuery,
                |key| {
                    tx.execute("INSERT INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,'members',?2,'') ON CONFLICT(namespace,kind,value,harness) DO UPDATE SET done=0,after_key='',after_edge=''", params![ns,key])?;
                    Ok(())
                },
            ).map_err(error)?;
            let layout = FrontierLayout::bind(&tx).map_err(error)?;
            frontier_page(&layout, ns)?;
            staged_paths(&tx, ns)?;
            let runtime: String = tx
                .query_row(
                    "SELECT runtime FROM main.catalog_semantic_stage WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(error)?;
            reset(&tx, ns, &runtime).map_err(error)?;
            tx.execute(
                "DELETE FROM storage_semantic_model_batches WHERE namespace=?1",
                [ns],
            )
            .map_err(error)?;
            let next_epoch = tx
                .query_row(
                    "SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(error)?;
            #[cfg(test)]
            crate::events::store::configured::effect("semantic_name_members_reset")
                .map_err(error)?;
            tx.commit().map_err(error)?;
            self.epoch = Some(next_epoch);
            return Err(refuse("semantic_claude_name_members_pending"));
        }
        result.map_err(error)
    }

    pub(crate) fn replay(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> Result<Context, StoreError> {
        if owners.len() > 64 {
            return Err(refuse("semantic_replay_owner_budget"));
        }
        let tx = self.db.transaction().map_err(error)?;
        self.source.validate(&tx).map_err(StoreError::Configured)?;
        let ns = self.source.namespace();
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=?1 AND epoch=?2 AND phase='publication' AND resetting=0)",params![ns,epoch],|r|r.get(0)).map_err(error)?;
        if !valid {
            return Err(refuse("stale semantic replay epoch"));
        }
        owners::Owners {
            db: &tx,
            namespace: ns,
        }
        .replay(epoch, owners, after)
        .map_err(error)
    }
    pub(crate) fn open(capability: SemanticStaging) -> Result<Self, StorageError> {
        let config = &capability.config;
        let mut db = capability.journal().connection()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(failed)?;
        #[cfg(test)]
        let reserved = std::time::Instant::now();
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
        }
        for (name, sql) in SEMANTIC_SUPPORT_SCHEMA {
            let expected = sql.replace(" IF NOT EXISTS", "");
            let expected = expected.trim_end_matches(';');
            let actual: Option<String> = tx
                .query_row(
                    "SELECT sql FROM main.sqlite_schema WHERE name=?1",
                    [name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(failed)?;
            if actual.as_deref() != Some(expected) {
                return Err(StorageError::NamespaceUpgradePending);
            }
        }
        super::proof_generations::validate(&tx)?;
        super::catalog_namespace::validate_guard(&tx, "storage_semantic_publications")?;
        for table in crate::events::store::configured::semantic_tables()
            .iter()
            .copied()
            .chain([
                "storage_semantic_profiles",
                "storage_semantic_model_batches",
            ])
        {
            for (operation, predicate) in [
                ("INSERT", "NEW.namespace IS NOT semon_namespace()"),
                (
                    "UPDATE",
                    "OLD.namespace IS NOT semon_namespace() OR NEW.namespace IS NOT semon_namespace()",
                ),
                ("DELETE", "OLD.namespace IS NOT semon_namespace()"),
            ] {
                let name = format!("storage_semantic_guard_{table}_{operation}");
                let expected = format!(
                    "CREATE TRIGGER {name} BEFORE {operation} ON {table} WHEN semon_semantic_writer()<>1 OR {predicate} BEGIN SELECT RAISE(ABORT,'semantic Source refused'); END"
                );
                let actual: Option<String> = tx.query_row("SELECT sql FROM main.sqlite_schema WHERE type='trigger' AND name=?1 AND tbl_name=?2", params![name,table], |r|r.get(0)).optional().map_err(failed)?;
                if actual.as_deref() != Some(expected.as_str()) {
                    return Err(StorageError::NamespaceUpgradePending);
                }
            }
        }
        let recipe = crate::events::store::configured::semantic_recipe();
        let valid: bool = tx.query_row("SELECT admission=?2 AND recipe=?3 FROM storage_semantic_profiles WHERE namespace=?1", params![config.namespace.as_str(),admission_sql,recipe], |r| r.get(0)).map_err(failed)?;
        if !valid {
            return Err(StorageError::NamespaceUpgradePending);
        }
        #[cfg(test)]
        if reserved.elapsed() >= std::time::Duration::from_millis(50) {
            eprintln!(
                "configured_reservation stage=semantic_open total_us={}",
                reserved.elapsed().as_micros()
            );
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
            begun: false,
            source: AdmittedSource {
                config: config.clone(),
                owner: capability.owner.clone(),
                admission,
                generation,
            },
        })
    }

    pub(crate) fn runtime_binding(
        &self,
    ) -> Result<crate::storage::QueryRuntimeBinding, StoreError> {
        self.source
            .validate(&self.db)
            .map_err(StoreError::Configured)?;
        Ok(crate::storage::QueryRuntimeBinding {
            namespace: self.source.config.namespace.clone(),
            admission: self.source.admission,
            stream_epoch: self
                .source
                .config
                .expected_stream_epoch
                .clone()
                .ok_or(StoreError::Configured(StorageError::ScopeChanged))?,
        })
    }

    pub(crate) fn begin(&mut self, paths: &[String], runtime: &str) -> Result<Outcome, StoreError> {
        self.begin_with_journal(paths, runtime, false)
    }

    pub(crate) fn begin_with_journal(
        &mut self,
        paths: &[String],
        runtime: &str,
        journaled_runtime: bool,
    ) -> Result<Outcome, StoreError> {
        if paths.len() > 64
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
                crate::storage::semantic_owner::bound_runtime(
                    self.source.config.namespace.as_str(),
                    &owner.admission(),
                    runtime,
                )
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
        // An observed old epoch cannot seize a successor. A quiescent handle
        // may take over, but must fence every old holder atomically below.
        if actual_epoch != self.epoch && self.epoch.is_some() {
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
        // The ordinary producer probes for resumable work with no new seeds.
        // Retain every runtime/epoch/generation fence when resuming, and do
        // not create an empty stage in place of an ordinary publication.
        if paths.is_empty() && existing.is_none() {
            return Ok(Outcome::Conflict);
        }
        if let Some((_, generation)) = &existing
            && generation != &self.source.generation
        {
            return Ok(Outcome::Conflict);
        }
        let authority: Option<String> = if existing.is_some() {
            tx.query_row(
                "SELECT withdrawal_authority FROM main.catalog_semantic_stage WHERE namespace=?1",
                [ns],
                |r| r.get(0),
            )
            .map_err(error)?
        } else {
            None
        };
        let invalid_authority = authority
            .as_deref()
            .map(|text| {
                serde_json::from_str::<crate::model::relation_stage::Withdrawal>(text)
                    .map(|w| !w.authority_current().unwrap_or(false))
                    .map_err(|e| StoreError::Data(e.to_string()))
            })
            .transpose()?
            .unwrap_or(false);
        // The ordinary Facts writer journals actual native/PID dependencies,
        // including unpublished identities. Unrelated runtime activity may
        // update the witness without discarding this component's progress.
        // Local/unobserved changes and negative custody retain full resets.
        let journaled = journaled_runtime
            && self.source.owner.is_some()
            && authority.is_none()
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key='catalog_push_driven' AND value='1')",
                [ns], |r| r.get::<_, bool>(0),
            ).map_err(error)?
            && reduction_witnesses(&tx, ns).map_err(error)?.current()?;
        use crate::storage::semantic_owner::{self, Begin, Observed};
        let plan = match semantic_owner::begin(
            Observed {
                epoch: actual_epoch,
                runtime: existing.as_ref().map(
                    |(old, _)| {
                        if journaled { runtime } else { old.as_str() }
                    },
                ),
                generation: existing
                    .as_ref()
                    .and_then(|(_, generation)| generation.as_deref()),
                invalid_withdrawal: invalid_authority,
            },
            self.epoch,
            self.begun,
            self.source.generation.as_deref(),
            runtime,
            self.reduction,
        ) {
            Ok(plan) => plan,
            Err(StorageError::Conflict) => return Ok(Outcome::Conflict),
            Err(error) => return Err(StoreError::Configured(error)),
        };
        if plan == Begin::Reset {
            reset(&tx, ns, runtime).map_err(error)?;
        } else if plan == Begin::Resume {
            // Same-runtime crash resume retains valid reduction progress and
            // its exact model witness; only ownership changes. All updates
            // commit together, including overflow/fault rollback.
            if tx.execute("UPDATE storage_semantic_profiles SET epoch=max(epoch,?2)+1 WHERE namespace=?1 AND max(epoch,?2)<9223372036854775807", params![ns,actual_epoch]).map_err(error)? != 1 { return Ok(Outcome::Conflict); }
            tx.execute("UPDATE main.catalog_semantic_stage SET epoch=(SELECT epoch FROM storage_semantic_profiles WHERE namespace=?1) WHERE namespace=?1",[ns]).map_err(error)?;
            tx.execute("UPDATE storage_semantic_model_batches SET epoch=(SELECT epoch FROM storage_semantic_profiles WHERE namespace=?1) WHERE namespace=?1",[ns]).map_err(error)?;
        }
        if plan == Begin::Create {
            if tx.execute("UPDATE storage_semantic_profiles SET epoch=epoch+1 WHERE namespace=?1 AND epoch<9223372036854775807", [ns]).map_err(error)? != 1 { return Ok(Outcome::Conflict); }
            tx.execute("INSERT INTO main.catalog_semantic_stage(namespace,singleton,epoch,phase,generation,runtime,revision) SELECT ?1,1,epoch,'indexing',?2,?3,(SELECT revision FROM storage_source_journal WHERE namespace=?1) FROM storage_semantic_profiles WHERE namespace=?1", params![ns,self.source.generation,runtime]).map_err(error)?;
        }
        if journaled {
            tx.execute(
                "UPDATE main.catalog_semantic_stage SET runtime=?2 WHERE namespace=?1 AND runtime<>?2",
                params![ns, runtime],
            ).map_err(error)?;
        }
        let claims = super::source_journal::claims(&tx, ns, paths).map_err(error)?;
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
        self.begun = true;
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
            #[cfg(test)]
            eprintln!("semantic_batch_conflict reason=epoch");
            return Err(StoreError::Configured(StorageError::Conflict));
        }
        if head(&tx).map_err(error)? != base_generation || base_generation != self.source.generation
        {
            #[cfg(test)]
            eprintln!("semantic_batch_conflict reason=generation");
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
        // Publication already consumed its exact claims atomically. Return
        // only the final cleanup batch, rather than trying to reduce against
        // removal invalidations that publication just ACKed. Select/replay
        // still use the full live witness checks. New dirty claims remain in
        // the journal; finish cannot mark a dirty Catalog complete.
        let acknowledged: bool = phase == "publication" && !resetting
            && tx.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND (publication_done=0 OR ack_done=0))",
                [ns], |r| r.get(0),
            ).map_err(error)?;
        if acknowledged {
            let source_revision = tx
                .query_row(
                    "SELECT revision FROM main.storage_source_journal WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(error)?;
            tx.commit().map_err(error)?;
            return Ok(Some(Batch {
                epoch,
                phase: Phase::Publication,
                paths: Vec::new(),
                base_generation,
                source_revision,
                runtime,
                context: Context::default(),
                relation_cursor: None,
            }));
        }
        let reset_complete = if resetting {
            reset_page(&tx, ns).map_err(error)?
        } else {
            true
        };
        if !resetting && !reduction_witnesses(&tx, ns).map_err(error)?.current()? {
            #[cfg(test)]
            eprintln!("semantic_batch_conflict reason=reduction");
            return Err(StoreError::Configured(StorageError::Conflict));
        }
        let mut result_phase = Phase::Indexing;
        // Enroll the complete recorded old-owner incident closure before any
        // native reduction. Missing Raw requires the producer's exact durable
        // removal; it never fabricates a Facts withdrawal or omits an old owner.
        if self.source.owner.is_some() && !resetting {
            let seeds = staged_paths(&tx, ns)?;
            let prior = owners::Owners {
                db: &tx,
                namespace: ns,
            }
            .affected(&seeds)
            .map_err(|e| StoreError::Data(e.to_string()))?;
            let mut paths = std::collections::BTreeSet::new();
            paths.extend(seeds.iter().cloned());
            for row in prior.values() {
                paths.extend(
                    row.sources
                        .iter()
                        .map(|s| s.path.to_string_lossy().into_owned()),
                );
            }
            if paths.len() > 64 {
                return Err(refuse("semantic_affected_owner_source_budget"));
            }
            let added = paths.iter().any(|path| !seeds.contains(path));
            if phase != "indexing"
                && paths
                    .iter()
                    .filter(|path| !seeds.contains(path))
                    .any(|path| !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
            {
                return Err(refuse("semantic_affected_owner_late_incident_pending"));
            }
            // A new native child has no Current owner yet. Revisit the real
            // indexed header/dependency frontiers after a journal advance;
            // the prior-owner closure alone cannot discover that child.
            let revision: i64 = tx
                .query_row(
                    "SELECT revision FROM storage_source_journal WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(error)?;
            let changed: bool = tx.query_row(
                "SELECT revision<>?2 OR EXISTS(SELECT 1 FROM main.catalog_semantic_sources WHERE namespace=?1 AND index_done=0) FROM main.catalog_semantic_stage WHERE namespace=?1",
                params![ns,revision],
                |r| r.get(0),
            ).map_err(error)?;
            let paths = paths.into_iter().collect::<Vec<_>>();
            for (path, claim) in super::source_journal::claims(&tx, ns, &paths).map_err(error)? {
                tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_sources(namespace,source_path,claim) VALUES(?1,?2,?3)",params![ns,path,claim]).map_err(error)?;
            }
            if phase != "indexing" && (added || changed) {
                // Use the existing bounded reset, including a fresh epoch.
                // Old in-memory batches and the durable model witness cannot
                // survive this transition. Raw and published custody stay put.
                reset(&tx, ns, &runtime).map_err(error)?;
                tx.execute(
                    "DELETE FROM storage_semantic_model_batches WHERE namespace=?1",
                    [ns],
                )
                .map_err(error)?;
                let next_epoch = tx
                    .query_row(
                        "SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=?1",
                        [ns],
                        |r| r.get(0),
                    )
                    .map_err(error)?;
                #[cfg(test)]
                crate::events::store::configured::effect("semantic_late_incident_reset")
                    .map_err(error)?;
                tx.commit().map_err(error)?;
                self.epoch = Some(next_epoch);
                return Ok(Some(Batch {
                    epoch: next_epoch,
                    phase: Phase::Indexing,
                    paths: Vec::new(),
                    base_generation,
                    source_revision: revision,
                    runtime,
                    context: Context::default(),
                    relation_cursor: None,
                }));
            }
            #[cfg(test)]
            crate::events::store::configured::effect("semantic_lifecycle_enroll").map_err(error)?;
        }
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
            // The source/epoch/schema/ownership fences above remain mandatory
            // on every batch. Only statement selection is reused within this
            // transaction's fixed pages; it conveys no Source authority.
            let layout = FrontierLayout::bind(&tx).map_err(error)?;
            let events = event_page(&layout, ns)?;
            let frontier = frontier_page(&layout, ns)?;
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
                    && crate::events::store::semantic_stage::qualify_with(&ScopedQualification {
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
                        endpoint = owners::Owners {
                            db: &tx,
                            namespace: ns,
                        }
                        .endpoint(&endpoint.row.key)
                        .map_err(error)?;
                        endpoint.root = root;
                        context.family_costs.insert(endpoint.row.key.clone(), cost);
                        context.endpoints.insert(endpoint.row.key.clone(), endpoint);
                    }
                    if paths.is_empty() {
                        tx.execute("UPDATE main.catalog_semantic_stage SET phase='publication' WHERE namespace=?1",[ns]).map_err(error)?;
                        result_phase = Phase::Publication;
                        context.replay_owners = context.endpoints.keys().cloned().collect();
                        for endpoint in context.endpoints.values() {
                            if let Some(parent) = &endpoint.parent {
                                context
                                    .children
                                    .entry(parent.clone())
                                    .or_default()
                                    .push(endpoint.row.key.clone());
                            }
                        }
                        paths=tx.prepare("SELECT source_path FROM main.catalog_semantic_sources WHERE namespace=?1 AND publication_done=0 ORDER BY source_path LIMIT ?2").map_err(error)?.query_map(params![ns,limit.clamp(1,64) as i64],|r|r.get(0)).map_err(error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(error)?;
                    }
                }
            }
        }
        if result_phase == Phase::Publication && self.source.owner.is_some() {
            context.prior_owners = owners::Owners {
                db: &tx,
                namespace: ns,
            }
            .affected(&staged_paths(&tx, ns)?)
            .map_err(|e| StoreError::Data(e.to_string()))?;
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
        if batch.phase == Phase::Indexing
            && (!prepared.rows.is_empty() || !prepared.transcripts.is_empty())
        {
            return Err(refuse("semantic_indexing_projection_payload"));
        }
        if batch.phase != Phase::Indexing && !self.reduction {
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
            #[cfg(test)]
            eprintln!("semantic_record_conflict: stage_binding");
            return Ok(Outcome::Conflict);
        }
        let claims = super::source_journal::claims(&tx, ns, &batch.paths).map_err(error)?;
        if claims != prepared.claims {
            #[cfg(test)]
            eprintln!("semantic_record_conflict: source_claims");
            return Ok(Outcome::Conflict);
        }
        if batch.phase == Phase::Publication {
            // A new native member can change a previously complete name
            // lookup without changing any already staged source claim.
            let revision: i64 = tx
                .query_row(
                    "SELECT revision FROM storage_source_journal WHERE namespace=?1",
                    [ns],
                    |r| r.get(0),
                )
                .map_err(error)?;
            if revision != batch.source_revision {
                return Ok(Outcome::Conflict);
            }
            if !lifecycle_current(prepared) {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: publication_lifecycle");
                return Ok(Outcome::Conflict);
            }
            if !reduction_witnesses(&tx, ns).map_err(error)?.current()? {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: publication_witness");
                return Ok(Outcome::Conflict);
            }
            if (prepared.rows.is_empty()
                && prepared.withdrawals.is_empty()
                && prepared.relationships.retired_owners.is_empty())
                || prepared.rows.len() > 64
                || prepared.transcripts.len() > 64
            {
                return Err(refuse("semantic_publication_component_budget"));
            }
            let mut metadata = Vec::new();
            let mut covered = validate_withdrawals(&tx, &self.source, batch, prepared)?;
            covered.extend(removed_paths(&tx, ns).map_err(error)?);
            let mut keys = std::collections::BTreeSet::new();
            for row in &prepared.rows {
                if !keys.insert(&row.key) || row.sources.is_empty() {
                    #[cfg(test)]
                    eprintln!("semantic_record_conflict: publication_owner_shape");
                    return Ok(Outcome::Conflict);
                }
                let staged: Option<String> = tx.query_row("SELECT metadata FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",params![ns,row.key],|r|r.get(0)).optional().map_err(error)?;
                let Some(staged) = staged else {
                    #[cfg(test)]
                    eprintln!("semantic_record_conflict: publication_staged_owner");
                    return Ok(Outcome::Conflict);
                };
                let _ = staged;
                let endpoint = owners::Owners {
                    db: &tx,
                    namespace: ns,
                }
                .endpoint(&row.key)
                .map_err(error)?;
                let mut identity = row.clone();
                identity.query = None;
                if serde_json::to_string(&identity).map_err(|e| StoreError::Data(e.to_string()))?
                    != serde_json::to_string(&endpoint.row)
                        .map_err(|e| StoreError::Data(e.to_string()))?
                {
                    #[cfg(test)]
                    eprintln!("semantic_record_conflict: publication_identity");
                    return Ok(Outcome::Conflict);
                }
                covered.extend(
                    row.sources
                        .iter()
                        .map(|s| s.path.to_string_lossy().into_owned()),
                );
                metadata
                    .push(serde_json::to_string(row).map_err(|e| StoreError::Data(e.to_string()))?);
            }
            if covered != batch.paths.iter().cloned().collect() {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: publication_source_coverage");
                return Ok(Outcome::Conflict);
            }
            if self.source.owner.is_some() {
                retirement_plan(&tx, &self.source, prepared)?;
            }
            let metadata =
                serde_json::to_string(&metadata).map_err(|e| StoreError::Data(e.to_string()))?;
            let projections = prepared
                .staged_model_witness()
                .map_err(|e| StoreError::Data(e.to_string()))?;
            let claims = serde_json::to_string(&prepared.claims)
                .map_err(|e| StoreError::Data(e.to_string()))?;
            if metadata.len().saturating_add(projections.len()) > 2 * 1048576 {
                return Err(refuse("semantic_context_budget"));
            }
            tx.execute("INSERT INTO storage_semantic_model_batches VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace) DO UPDATE SET epoch=excluded.epoch,metadata=excluded.metadata,projections=excluded.projections,claims=excluded.claims",params![ns,batch.epoch,metadata,projections,claims]).map_err(error)?;
            #[cfg(test)]
            crate::events::store::configured::effect("semantic_model_record").map_err(error)?;
            tx.commit().map_err(error)?;
            return Ok(Outcome::Written);
        }
        if batch.phase != Phase::Indexing {
            if !reduction_witnesses(&tx, ns).map_err(error)?.current()? {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: reduction_witness");
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
                    #[cfg(test)]
                    eprintln!("semantic_record_conflict: phase_pending_path");
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
                    if !matches!(endpoint.row.harness.as_str(), "codex" | "claude") {
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
                        #[cfg(test)]
                        eprintln!("semantic_record_conflict: identity_native_membership");
                        return Ok(Outcome::Conflict);
                    }
                    for source in &endpoint.row.sources {
                        let path = source.path.to_string_lossy().into_owned();
                        if !batch.paths.contains(&path) {
                            #[cfg(test)]
                            eprintln!("semantic_record_conflict: identity_batch_membership");
                            return Ok(Outcome::Conflict);
                        }
                        let stored: String=tx.query_row("SELECT metadata FROM main.catalog_semantic_inputs WHERE namespace=?1 AND source_path=?2 AND harness=?3",params![ns,path,endpoint.row.harness],|r|r.get(0)).map_err(error)?;
                        let observed: crate::model::summary::CatalogSource =
                            serde_json::from_str(&stored)
                                .map_err(|e| StoreError::Data(e.to_string()))?;
                        let key: String=tx.query_row("SELECT session_key FROM main.catalog_semantic_keys WHERE namespace=?1 AND harness=?3 AND native_id=?2",params![ns,source.native_id,endpoint.row.harness],|r|r.get(0)).map_err(error)?;
                        if observed != *source
                            || (endpoint.row.harness != "claude" && key != endpoint.row.key)
                            || !endpoint.row.native_ids.contains(&source.native_id)
                        {
                            #[cfg(test)]
                            eprintln!("semantic_record_conflict: identity_source_binding");
                            return Ok(Outcome::Conflict);
                        }
                        covered.insert(path);
                    }
                    let canonical: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_keys WHERE namespace=?1 AND harness=?2 AND session_key=?3 AND native_id IN (SELECT json_each.value FROM json_each(?4)))",params![ns,endpoint.row.harness,endpoint.row.key,serde_json::to_string(&endpoint.row.native_ids).map_err(|e|StoreError::Data(e.to_string()))?],|r|r.get(0)).map_err(error)?;
                    if !canonical {
                        #[cfg(test)]
                        eprintln!("semantic_record_conflict: identity_canonical_key");
                        return Ok(Outcome::Conflict);
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
                    #[cfg(test)]
                    eprintln!("semantic_record_conflict: identity_source_coverage");
                    return Ok(Outcome::Conflict);
                }
                for (endpoint, text) in encoded {
                    let _ = text;
                    OwnerStore::endpoint(
                        &owners::Owners {
                            db: &tx,
                            namespace: ns,
                        },
                        endpoint,
                    )
                    .map_err(error)?;
                    for source in &endpoint.row.sources {
                        tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_native(namespace,harness,native_id,session_key) VALUES(?1,?4,?2,?3)",params![ns,source.native_id,endpoint.row.key,endpoint.row.harness]).map_err(error)?;
                        tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_owner_sources(namespace,session_key,source_path) VALUES(?1,?2,?3)",params![ns,endpoint.row.key,source.path.to_string_lossy()]).map_err(error)?;
                    }
                }
            }
            if batch.phase == Phase::Relations {
                let effects = owners::Owners {
                    db: &tx,
                    namespace: ns,
                };
                let mut bytes = 0usize;
                for endpoint in prepared.relationships.endpoints.values() {
                    let own: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2)",params![ns,endpoint.row.key],|r|r.get(0)).map_err(error)?;
                    if !own {
                        #[cfg(test)]
                        eprintln!("semantic_record_conflict: relation_staged_owner");
                        return Ok(Outcome::Conflict);
                    }
                    let text = serde_json::to_string(endpoint)
                        .map_err(|e| StoreError::Data(e.to_string()))?;
                    bytes = bytes.saturating_add(text.len());
                    if bytes > 1048576 {
                        return Err(refuse("semantic_context_budget"));
                    }
                    tx.execute("UPDATE main.catalog_semantic_endpoints SET metadata=?3 WHERE namespace=?1 AND session_key=?2",params![ns,endpoint.row.key,text]).map_err(error)?;
                }
                if prepared.relationships.relations.len() > 64 {
                    return Err(refuse("semantic_relation_budget"));
                }
                for relation in prepared.relationships.relations.values() {
                    // The shared native model represents unmatched calls and
                    // receipts with explicit trace stubs. They carry no Raw
                    // membership, just as in Owners::replay. Every relation
                    // still needs a real owner and evidence for both ends.
                    let real = |key: &String| prepared.relationships.endpoints.contains_key(key);
                    let known = |key: &String| {
                        real(key) || relation.stubs.iter().any(|stub| &stub.key == key)
                    };
                    if !(real(&relation.from) || relation.to.as_ref().is_some_and(real))
                        || !known(&relation.from)
                        || relation.to.as_ref().is_some_and(|to| !known(to))
                    {
                        #[cfg(test)]
                        eprintln!("semantic_record_conflict: relation_endpoint_evidence");
                        #[cfg(test)]
                        eprintln!("semantic_record_conflict: relation_endpoint_evidence");
                        return Ok(Outcome::Conflict);
                    }
                    effects.relation(relation).map_err(error)?;
                }
                #[cfg(test)]
                crate::events::store::configured::effect("semantic_record_relations")
                    .map_err(error)?;
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
        if prepared.indexed_sources.len() + prepared.withdrawals.len() > batch.paths.len() {
            #[cfg(test)]
            eprintln!("semantic_record_conflict: indexing_source_coverage");
            return Ok(Outcome::Conflict);
        }
        let mut seen = std::collections::BTreeSet::new();
        for (_, source) in &prepared.indexed_sources {
            let path = source.path.to_string_lossy().into_owned();
            if !batch.paths.contains(&path) || !seen.insert(path.clone()) {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: indexing_batch_membership");
                return Ok(Outcome::Conflict);
            }
            let Some((_, ledger)) = crate::events::store::ledger_row(&tx, &path).map_err(error)?
            else {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: indexing_ledger_absent");
                return Ok(Outcome::Conflict);
            };
            let mut expected = ledger.source(&source.path, &source.native_id);
            expected.native_role = source.native_role.clone();
            if expected != *source
                || crate::events::Stat::of(&source.path).ok().as_ref() != Some(&ledger.stat)
            {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: indexing_ledger_changed");
                return Ok(Outcome::Conflict);
            }
            if !crate::events::store::admitted_roles_current(
                &tx,
                std::slice::from_ref(source),
                &self.source,
            )
            .map_err(error)?
            {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: indexing_role_changed");
                return Ok(Outcome::Conflict);
            }
            let captured: Option<Option<i64>> = tx.query_row("SELECT claim FROM main.catalog_semantic_sources WHERE namespace=?1 AND source_path=?2 AND index_done=0", params![ns,path], |r| r.get(0)).optional().map_err(error)?;
            if captured != claims.iter().find(|(p, _)| p == &path).map(|(_, c)| *c) {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: indexing_claim_changed");
                return Ok(Outcome::Conflict);
            }
        }
        let mut negative_paths = std::collections::BTreeSet::new();
        for withdrawal in &prepared.withdrawals {
            if !negative_paths.insert(&withdrawal.path)
                || !batch.paths.contains(&withdrawal.path)
                || seen.contains(&withdrawal.path)
                || !withdrawal_runtime_matches(&self.source, batch, withdrawal)?
                || !withdrawal.current().unwrap_or(false)
            {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: withdrawal_evidence");
                return Ok(Outcome::Conflict);
            }
            owners::Owners {
                db: &tx,
                namespace: ns,
            }
            .withdrawal(withdrawal)
            .map_err(error)?;
        }
        for path in &batch.paths {
            if seen.contains(path) || prepared.withdrawals.iter().any(|w| &w.path == path) {
                continue;
            }
            if !prepared.removed_sources.contains(path)
                || !removed_source(&tx, ns, path).map_err(error)?
            {
                #[cfg(test)]
                eprintln!(
                    "semantic_record_conflict: removal_evidence physical_absent={} durable_removed={}",
                    prepared.removed_sources.contains(path),
                    removed_source(&tx, ns, path).map_err(error)?
                );
                #[cfg(test)]
                {
                    let reason: Option<String> = tx.query_row(
                        "SELECT reason FROM main.session_catalog_invalidations WHERE namespace=?1 AND source_path=?2",
                        params![ns, path], |r| r.get(0),
                    ).optional().map_err(error)?;
                    let reason = match reason.as_deref() {
                        None => "absent",
                        Some("removed") => "removed",
                        Some("changed") => "changed",
                        Some(_) => "other",
                    };
                    let ledger = crate::events::store::ledger_row(&tx, path).map_err(error)?;
                    let retained = ledger
                        .as_ref()
                        .map(|(_, ledger)| {
                            crate::events::store::archive_ledger_retained(&tx, path, ledger)
                        })
                        .transpose()
                        .map_err(error)?;
                    let marker: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM main.meta WHERE namespace=?1 AND key=?2)",
                            params![ns, format!("archive_event_index:{path}")],
                            |r| r.get(0),
                        )
                        .map_err(error)?;
                    let current =
                        super::source_journal::claims(&tx, ns, std::slice::from_ref(path))
                            .map_err(error)?;
                    eprintln!(
                        "semantic_removal_state reason={reason} ledger={} marker={marker} retained={retained:?} claim={:?} prepared_claim={:?}",
                        ledger.is_some(),
                        current[0].1,
                        prepared
                            .claims
                            .iter()
                            .find(|(p, _)| p == path)
                            .map(|(_, claim)| claim)
                    );
                }
                return Ok(Outcome::Conflict);
            }
            let retained = crate::events::store::ledger_row(&tx, path)
                .map_err(error)?
                .map(|(_, ledger)| serde_json::to_string(&ledger))
                .transpose()
                .map_err(|e| StoreError::Data(e.to_string()))?;
            // The ordinary removal can create a newer dirty claim during
            // preparation. The current claims were compared with the prepared
            // vector above; bind this exact durable negative observation before
            // marking the primitive complete. Later reductions recheck it.
            tx.execute("UPDATE main.catalog_semantic_sources SET claim=(SELECT revision FROM main.session_catalog_invalidations WHERE namespace=?1 AND source_path=?2),ledger=?3,index_done=1,events_done=1,identity_done=1,relation_done=1 WHERE namespace=?1 AND source_path=?2 AND index_done=0", params![ns,path,retained]).map_err(error)?;
        }
        for (path, harness, native) in &prepared.indexed_native_parents {
            if !seen.contains(path) {
                #[cfg(test)]
                eprintln!("semantic_record_conflict: parent_source_membership");
                return Ok(Outcome::Conflict);
            }
            owners::Owners {
                db: &tx,
                namespace: ns,
            }
            .dependency(path, &format!("runtime:native:{harness}"), native)
            .map_err(error)?;
            for kind in ["native", "nativeall", "headerid", "headerparent"] {
                tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,?2,?3,?4)",params![ns,kind,native,if kind=="native" {harness.as_str()} else {""}]).map_err(error)?;
            }
        }
        for (harness, native) in &prepared.lineage_dependencies {
            let kind = if harness.starts_with("bridge:") {
                "dependency"
            } else {
                "native"
            };
            tx.execute("INSERT OR IGNORE INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,?2,?3,?4)",params![ns,kind,native,harness]).map_err(error)?;
        }
        for (harness, source) in &prepared.indexed_sources {
            if self.reduction && !matches!(harness.as_str(), "codex" | "claude") {
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
        crate::events::store::configured::effect("semantic_record_owners").map_err(error)?;
        #[cfg(test)]
        crate::events::store::configured::effect("semantic_record").map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(Outcome::Written)
    }
}
