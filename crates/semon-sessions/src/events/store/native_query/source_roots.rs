//! Source candidate work precedes a Catalog owner publication. Its immutable
//! roots carry no Query/custody authority: the final reader imports them only
//! after admitting the newly published complete source and owner witnesses.
use super::*;

const PREFIX: &str = "native-source-root/v1/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Epoch {
    pub(crate) epoch: i64,
    pub(crate) base_generation: Option<String>,
    pub(crate) source_revision: i64,
    pub(crate) runtime: String,
}

impl Epoch {
    pub(super) fn current(&self, db: &Connection) -> rusqlite::Result<bool> {
        db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage WHERE singleton=1 AND resetting=0 AND phase='publication' AND epoch=?1 AND generation IS ?2 AND revision=?3 AND runtime=?4 AND generation IS (SELECT value FROM meta WHERE key='catalog_generation') AND revision=COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key='catalog_source_revision'),0))", params![self.epoch,self.base_generation,self.source_revision,self.runtime], |row|row.get(0))
    }
}

pub(crate) use super::ClosureRead;

#[derive(Default, Debug)]
pub(crate) struct CheckpointWork {
    /// Reserved upper bounds for the fixed source/job/epoch SQL controls.
    /// Actual executed instructions are measured by sqlite_vm_steps below.
    pub(crate) control_rows_reserved: usize,
    pub(crate) control_bytes_reserved: usize,
    pub(crate) closure: Work,
    pub(crate) staged_rows: usize,
    pub(crate) staged_bytes: usize,
    pub(crate) sqlite_vm_steps: u64,
    /// Full RuntimeFacts fence calls are external to this metadata receipt.
    pub(crate) runtime_checks: usize,
}

/// A distinct source producer namespace never grants reader/owner authority.
pub(crate) struct SourceStage {
    pub(crate) job: Job,
    pub(crate) epoch: Epoch,
}

impl NativeStore {
    pub(crate) fn begin_source_roots(
        &mut self,
        epoch: &Epoch,
        sources: &[SourceWitness],
    ) -> io::Result<Option<SourceStage>> {
        if sources.len() != 1 {
            return Err(io::Error::other(
                "one native source root job requires exactly one source",
            ));
        }
        if !epoch.current(&self.connection).map_err(io::Error::other)? {
            return Ok(None);
        }
        // An epoch/source observation is immutable. A subsequent publication
        // gets another job, leaving old roots available for bounded import.
        let mut identity = Sha256::new();
        let mut bytes = 0usize;
        let mut paths = std::collections::BTreeSet::new();
        let mut part = |text: &str| -> io::Result<()> {
            bytes = bytes
                .checked_add(text.len())
                .ok_or_else(|| io::Error::other("native source root metadata budget exceeded"))?;
            if bytes > BYTES {
                return Err(io::Error::other(
                    "native source root metadata budget exceeded",
                ));
            }
            identity.update((text.len() as u64).to_be_bytes());
            identity.update(text.as_bytes());
            Ok(())
        };
        part(&bounded_json(epoch, STATE_BYTES).map_err(io::Error::other)?)?;
        for source in sources {
            if !paths.insert(&source.source.path) {
                return Err(io::Error::other("duplicate native source root witness"));
            }
            part(&bounded_json(&source.source, BYTES).map_err(io::Error::other)?)?;
            part(
                &source
                    .ledger
                    .as_ref()
                    .map(source_fact_generation)
                    .transpose()
                    .map_err(io::Error::other)?
                    .unwrap_or_default(),
            )?;
            part(
                &source
                    .claim
                    .map(|claim| claim.to_string())
                    .unwrap_or_default(),
            )?;
            part(source.archive.as_deref().unwrap_or(""))?;
            part(source.mutation.as_deref().unwrap_or(""))?;
        }
        drop(part);
        let spec = JobSpec {
            view_key: format!("{PREFIX}{:x}", identity.finalize()),
            version: 1,
            catalog_generation: None,
            runtime: epoch.runtime.clone(),
        };
        let mut job = self.begin(&spec)?;
        if job.phase == "cleanup" {
            self.cleanup(&job)?;
            return Ok(None);
        }
        if job.phase == "capture" {
            if self.admit(&job, sources, &[], "source-closure")? != Outcome::Written {
                return Ok(None);
            }
            job = self
                .reload(&spec.view_key)?
                .ok_or_else(|| io::Error::other("source root admission disappeared"))?;
        }
        if !epoch.current(&self.connection).map_err(io::Error::other)? || !self.recheck(&job)? {
            return Ok(None);
        }
        Ok(Some(SourceStage {
            job,
            epoch: epoch.clone(),
        }))
    }

    /// Only this typed source stage permits zero owners. Normal checkpoint()
    /// keeps its source+owner requirement, and reader admission is unchanged.
    /// The returned receipt separates reserved control work, actual closure
    /// reads, and staged output. Whole-connection SQLite instructions include
    /// fences/CAS and survive conflict/cancellation; runtime work is separate.
    pub(crate) fn checkpoint_source_roots(
        &mut self,
        stage: &SourceStage,
        phase: &str,
        cursor: &str,
        state: &Value,
        rows: &[StageRow],
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<ClosureRead>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, CheckpointWork)> {
        if !stage.job.spec.view_key.starts_with(PREFIX)
            || stage.job.spec.catalog_generation.is_some()
            || stage.job.spec.runtime != stage.epoch.runtime
            || rows.len() > 8
            || !matches!(phase, "entries" | "metadata" | "finish")
        {
            return Err(io::Error::other("invalid native source root checkpoint"));
        }
        let state = bounded_json(state, STATE_BYTES).map_err(io::Error::other)?;
        let texts = rows
            .iter()
            .map(|row| bounded_json(&row.facts, BYTES))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(io::Error::other)?;
        let bytes = state.len()
            + cursor.len()
            + rows
                .iter()
                .zip(&texts)
                .map(|(row, text)| 64 + row.key.identity.len() + text.len())
                .sum::<usize>();
        // Reserve before doing SQL: 24 fixed controls, two closure reads of
        // at most16 rows/128KiB each, and at most8 staged output rows. The two
        // bounded job records plus epoch/scalar probes fit the byte reserve.
        let job_bytes = bounded_json(
            &serde_json::json!({"spec":stage.job.spec,"cursor":stage.job.cursor,"state":stage.job.state}),
            STATE_BYTES,
        ).map_err(io::Error::other)?.len();
        let epoch_bytes = bounded_json(&stage.epoch, STATE_BYTES)
            .map_err(io::Error::other)?
            .len();
        let controls = 2 * (job_bytes + epoch_bytes) + 4096;
        if bytes > 256 * 1024 || controls > 256 * 1024 || cursor.len() + state.len() > STATE_BYTES {
            return Err(io::Error::other(
                "native source root metadata budget exceeded",
            ));
        }
        let mut work = CheckpointWork {
            control_rows_reserved: 24,
            control_bytes_reserved: controls,
            staged_rows: rows.len(),
            staged_bytes: bytes,
            ..Default::default()
        };
        let mut read_closure = |db: &Connection| -> io::Result<bool> {
            let read = closure(db, stage.job.id).map_err(io::Error::other)?;
            let bounded = read.work.decoded_rows <= 16 && read.work.metadata_bytes <= 128 * 1024;
            add_work(&mut work.closure, read.work)?;
            Ok(read.current && bounded)
        };
        let measurement = super::vm_meter::begin(&self.connection);
        let result = (|| {
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            work.runtime_checks += 1;
            if !self.runtime_current(&stage.job.spec)? {
                return Ok(Outcome::Conflict);
            }
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(io::Error::other)?;
            if matches!(stage.job.phase.as_str(), "cleanup" | "ready")
                || !job_current(&tx, &stage.job).map_err(io::Error::other)?
                || !stage.epoch.current(&tx).map_err(io::Error::other)?
                || !read_closure(&tx)?
            {
                return Ok(Outcome::Conflict);
            }
            let (sources, owners): (i64, i64) = tx
                .query_row(
                    "SELECT sources,owners FROM native_query_jobs WHERE job=?1",
                    [stage.job.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(io::Error::other)?;
            if sources != 1 || owners != 0 {
                return Err(io::Error::other("native source root witnesses are invalid"));
            }
            for (row, text) in rows.iter().zip(texts) {
                let key = &row.key;
                tx.execute("INSERT INTO native_query_rows VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(job,kind,k0,k1,k2,k3,k4,ordinal,identity) DO UPDATE SET metadata=excluded.metadata", params![stage.job.id,row.kind.name(),key.order[0],key.order[1],key.order[2],key.order[3],key.order[4],key.ordinal,key.identity,text]).map_err(io::Error::other)?;
            }
            // The callback may observe filesystem/related membership. Recheck
            // owned facts after it, before advancing the durable source cursor.
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            let complete = read_closure(&tx)?;
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            if !complete
                || !stage.epoch.current(&tx).map_err(io::Error::other)?
                || !job_current(&tx, &stage.job).map_err(io::Error::other)?
            {
                return Ok(Outcome::Conflict);
            }
            work.runtime_checks += 1;
            if crate::native_evidence::query_runtime_generation(&self.options)?
                != stage.job.spec.runtime
            {
                return Ok(Outcome::Conflict);
            }
            let changed = tx.execute("UPDATE native_query_jobs SET phase=?2,cursor=?3,state=?4,revision=revision+1 WHERE job=?1 AND revision=?5", params![stage.job.id,phase,cursor,state,stage.job.revision]).map_err(io::Error::other)?;
            if changed != 1 {
                return Ok(Outcome::Conflict);
            }
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            tx.commit().map_err(io::Error::other)?;
            Ok(Outcome::Written)
        })();
        work.sqlite_vm_steps = measurement.finish(&self.connection);
        result.map(|outcome| (outcome, work))
    }
}

/// This check grants no reader authority. The generic rank transaction checks
/// the exact job revision/source witnesses; this adds the source-only namespace,
/// admission shape and semantic publication epoch.
pub(in crate::events::store) fn scope_current(
    db: &Connection,
    stage: &SourceStage,
) -> rusqlite::Result<bool> {
    Ok(stage.job.spec.view_key.starts_with(PREFIX)
        && stage.job.spec.catalog_generation.is_none()
        && stage.job.spec.runtime == stage.epoch.runtime
        && stage.epoch.current(db)?
        && db.query_row("SELECT EXISTS(SELECT 1 FROM native_query_jobs WHERE job=?1 AND sources=1 AND owners=0 AND phase IN ('capture','entries','metadata','finish'))",[stage.job.id],|row|row.get::<_,bool>(0))?)
}

impl NativeStore {
    /// Final source producer fence outside the write snapshot. The last
    /// external callback is followed by owned checks and cancellation again.
    /// Unlike recheck_closure(), this typed operation never grants Query Ready.
    pub(in crate::events::store) fn recheck_source_closure(
        &self,
        stage: &SourceStage,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, u64)> {
        let measurement = super::vm_meter::begin(&self.connection);
        let result = (|| {
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            if !self.runtime_current(&stage.job.spec)?
                || !scope_current(&self.connection, stage).map_err(io::Error::other)?
                || !job_current(&self.connection, &stage.job).map_err(io::Error::other)?
                || !closure(&self.connection, stage.job.id).map_err(io::Error::other)?
            {
                return Ok(Outcome::Conflict);
            }
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            if !self.runtime_current(&stage.job.spec)?
                || !scope_current(&self.connection, stage).map_err(io::Error::other)?
                || !job_current(&self.connection, &stage.job).map_err(io::Error::other)?
            {
                return Ok(Outcome::Conflict);
            }
            Ok(if cancelled() {
                Outcome::Deferred
            } else {
                Outcome::Written
            })
        })();
        let steps = measurement.finish(&self.connection);
        result.map(|outcome| (outcome, steps))
    }
}
