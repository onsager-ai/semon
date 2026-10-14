//! Source candidate work precedes a Catalog owner publication. Its immutable
//! roots carry no Query/custody authority: the final reader imports them only
//! after admitting the newly published complete source and owner witnesses.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

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
    fn current(&self, db: &Connection) -> rusqlite::Result<bool> {
        db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage WHERE singleton=1 AND resetting=0 AND phase='publication' AND epoch=?1 AND generation IS ?2 AND revision=?3 AND runtime=?4)", params![self.epoch,self.base_generation,self.source_revision,self.runtime], |row|row.get(0))
    }
}

/// A distinct producer namespace prevents confusing a prepublication source
/// tree with a qualified reader view. Construction admits only the exact
/// source witness; it never invents a synthetic Catalog owner.
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
        if sources.is_empty() || sources.len() > ROWS {
            return Err(io::Error::other("source root closure budget exceeded"));
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
    /// Every attempted SQL instruction, including all witness and epoch checks,
    /// is returned even for conflict/cancellation; no source body is read here.
    pub(crate) fn checkpoint_source_roots(
        &mut self,
        stage: &SourceStage,
        phase: &str,
        cursor: &str,
        state: &Value,
        rows: &[StageRow],
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, u64)> {
        if !stage.job.spec.view_key.starts_with(PREFIX)
            || stage.job.spec.catalog_generation.is_some()
            || stage.job.spec.runtime != stage.epoch.runtime
            || rows.len() > ROWS
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
        if bytes > BYTES || cursor.len() > STATE_BYTES {
            return Err(io::Error::other(
                "native source root metadata budget exceeded",
            ));
        }
        let steps = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&steps);
        self.connection.progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        );
        let result = (|| {
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
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
                || !closure(&tx, stage.job.id).map_err(io::Error::other)?
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
            if sources == 0 || owners != 0 {
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
            let complete = closure(&tx, stage.job.id).map_err(io::Error::other)?;
            if cancelled() {
                return Ok(Outcome::Deferred);
            }
            if !complete
                || !stage.epoch.current(&tx).map_err(io::Error::other)?
                || !job_current(&tx, &stage.job).map_err(io::Error::other)?
                || crate::native_evidence::query_runtime_generation(&self.options)?
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
        self.connection.progress_handler(0, None::<fn() -> bool>);
        result.map(|outcome| (outcome, steps.load(Ordering::Relaxed)))
    }
}
