//! Native source point recipes, produced before Catalog publication. The
//! address tree is immutable and generation independent; a new writer interval
//! imports its predecessor and applies only captured interests. This is source
//! evidence, never an owner/custody/view readiness token.
use super::*;
use crate::{
    events::store::native_rank::{CandidateDirection, CandidateLeaf, CandidateRoot, CandidateWork},
    model::native_producer::{Candidate, EmissionOrder},
};
use source_roots::{Epoch, SourceStage};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_source_point_heads (
 path TEXT PRIMARY KEY, generation TEXT NOT NULL, change_id INTEGER NOT NULL,
 job INTEGER NOT NULL, metadata TEXT NOT NULL, scope TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS catalog_semantic_point_pending ON catalog_semantic_sources(publication_done,source_path);
CREATE TABLE IF NOT EXISTS native_source_point_manifests (
 path TEXT NOT NULL, generation TEXT NOT NULL, job INTEGER NOT NULL,
 change_id INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(path,generation)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_source_point_stage (
 scope TEXT PRIMARY KEY, after_path TEXT NOT NULL
) STRICT, WITHOUT ROWID;
";
const FORMAT: u32 = 1;
const COMBINE: &str = "native-source-address/v1";

pub(crate) fn epoch_scope(epoch: &Epoch) -> io::Result<String> {
    // Source-only roots survive the Catalog owner publication that advances
    // base_generation. The producer fences that base while writing; its pure
    // result is rebound to the new owner by the native publication transaction.
    Ok(format!(
        "{:x}",
        Sha256::digest(
            bounded_json(
                &serde_json::json!([epoch.epoch, epoch.source_revision, epoch.runtime]),
                STATE_BYTES
            )
            .map_err(io::Error::other)?
            .as_bytes()
        )
    ))
}

fn advance_frontier(db: &Connection, epoch: &Epoch, path: &str) -> io::Result<()> {
    db.execute("INSERT INTO native_source_point_stage VALUES(?1,?2) ON CONFLICT(scope) DO UPDATE SET after_path=MAX(after_path,excluded.after_path)",params![epoch_scope(epoch)?,path]).map_err(io::Error::other)?;
    Ok(())
}

pub(crate) struct PublicationTick {
    pub(crate) epoch: Epoch,
    pub(crate) source: Option<CatalogSource>,
    pub(crate) path: Option<String>,
    pub(crate) complete: bool,
    pub(crate) work: Work,
}

/// A retained point root plus its original source proof. The caller must ask
/// the native Original owner to qualify any byte-prefix/custody reuse; this
/// metadata alone grants neither. No old source is opened or recaptured.
pub(crate) struct Predecessor {
    pub(crate) source: CatalogSource,
    pub(crate) root: CandidateRoot,
    pub(crate) change: i64,
    pub(crate) source_job: i64,
}

impl NativeStore {
    /// Read one publication source after the durable source-point frontier.
    /// Sealing a source and advancing this frontier share one transaction.
    pub(crate) fn source_point_publication_tick(&self) -> io::Result<Option<PublicationTick>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let mut query=tx.prepare("SELECT epoch,generation,revision,runtime FROM catalog_semantic_stage WHERE singleton=1 AND resetting=0 AND phase='publication'").map_err(io::Error::other)?;
        let epoch = query
            .query_row([], |row| {
                Ok(Epoch {
                    epoch: row.get(0)?,
                    base_generation: row.get(1)?,
                    source_revision: row.get(2)?,
                    runtime: row.get(3)?,
                })
            })
            .optional()
            .map_err(io::Error::other)?;
        let mut work = Work {
            decoded_rows: usize::from(epoch.is_some()),
            metadata_bytes: 0,
            vm_steps: query.get_status(rusqlite::StatementStatus::VmStep),
        };
        drop(query);
        let Some(epoch) = epoch else { return Ok(None) };
        let scope = epoch_scope(&epoch)?;
        let after: Option<String> = tx
            .query_row(
                "SELECT after_path FROM native_source_point_stage WHERE scope=?1",
                [&scope],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        work.decoded_rows += usize::from(after.is_some());
        work.metadata_bytes += after.as_ref().map_or(0, String::len);
        let mut query=tx.prepare("SELECT s.source_path,s.withdrawal IS NOT NULL,CASE WHEN octet_length(i.metadata)<=1048576 THEN i.metadata END,COALESCE(octet_length(i.metadata),0) FROM catalog_semantic_sources s LEFT JOIN catalog_semantic_inputs i USING(source_path) WHERE s.publication_done=0 AND s.source_path>?1 ORDER BY s.source_path LIMIT 1").map_err(io::Error::other)?;
        let row = query
            .query_row([after.as_deref().unwrap_or("")], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, usize>(3)?,
                ))
            })
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        let Some((path, withdrawn, text, bytes)) = row else {
            return Ok(Some(PublicationTick {
                epoch,
                source: None,
                path: None,
                complete: true,
                work,
            }));
        };
        if bytes > BYTES {
            return Err(invalid_point(
                "semantic source descriptor exceeds native metadata budget",
            ));
        }
        work.decoded_rows += 1;
        work.metadata_bytes += path.len() + bytes + 65;
        let source = if withdrawn {
            None
        } else {
            Some(
                serde_json::from_str::<CatalogSource>(
                    &text.ok_or_else(|| invalid_point("semantic source descriptor absent"))?,
                )
                .map_err(io::Error::other)?,
            )
        };
        if source
            .as_ref()
            .is_some_and(|source| source.path.to_string_lossy() != path)
        {
            return Err(invalid_point("semantic source descriptor path disagrees"));
        }
        Ok(Some(PublicationTick {
            epoch,
            source,
            path: Some(path),
            complete: false,
            work,
        }))
    }

    /// A withdrawn publication input advances only the source frontier. It
    /// grants no view readiness. Cancellation stays latched for this attempt.
    pub(crate) fn source_point_withdrawal_step(
        &mut self,
        epoch: &Epoch,
        path: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Progress, Receipt)> {
        let seen = std::cell::Cell::new(false);
        let cancel = || {
            if cancelled() {
                seen.set(true);
            }
            seen.get()
        };
        let mut receipt = Receipt::default();
        let measurement = super::vm_meter::begin(&self.connection);
        let result = (|| {
            if cancel() {
                return Ok(Progress::Deferred);
            }
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(io::Error::other)?;
            if !epoch.current(&tx).map_err(io::Error::other)? {
                return Ok(Progress::Conflict);
            }
            let withdrawn:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources WHERE source_path=?1 AND withdrawal IS NOT NULL AND publication_done=0)",[path],|row|row.get(0)).map_err(io::Error::other)?;
            receipt.control.decoded_rows += 1;
            receipt.control.metadata_bytes += 1;
            if !withdrawn {
                return Ok(Progress::Conflict);
            }
            if cancel() {
                return Ok(Progress::Deferred);
            }
            advance_frontier(&tx, epoch, path)?;
            receipt.control.decoded_rows += 1;
            receipt.control.metadata_bytes += epoch_scope(epoch)?.len() + path.len();
            if !epoch.current(&tx).map_err(io::Error::other)? {
                return Ok(Progress::Conflict);
            }
            if cancel() {
                return Ok(Progress::Deferred);
            }
            tx.commit().map_err(io::Error::other)?;
            if !epoch.current(&self.connection).map_err(io::Error::other)? {
                return Ok(Progress::Conflict);
            }
            if cancel() {
                return Ok(Progress::Deferred);
            }
            Ok(Progress::Pending)
        })();
        receipt.sqlite_vm_steps = measurement.finish(&self.connection);
        match result {
            Ok(progress) => Ok((progress, receipt)),
            Err(error) => Err(io::Error::new(error.kind(), Failed { error, receipt })),
        }
    }

    pub(crate) fn source_point_predecessor(
        &self,
        witness: &SourceWitness,
    ) -> io::Result<(Option<Predecessor>, Work)> {
        let mut work = Work::default();
        let Some((_, manifest)) = self.source_point_head(witness, &mut work)? else {
            return Ok((None, work));
        };
        self.source_point_predecessor_proof(witness, manifest, work)
    }

    /// Exact writer before_generation lookup, even when the current point
    /// head has advanced. The caller validates the complete captured change
    /// chain and retained Original receipt; the string is not byte authority.
    pub(crate) fn source_point_predecessor_for_generation(
        &self,
        witness: &SourceWitness,
        generation: &str,
    ) -> io::Result<(Option<Predecessor>, Work)> {
        if generation.len() > STATE_BYTES {
            return Err(invalid_point(
                "source interval generation exceeds native metadata budget",
            ));
        }
        let mut work = Work::default();
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(invalid_point("source predecessor observation changed"));
        }
        let mut query=tx.prepare("SELECT CASE WHEN octet_length(m.metadata)<=65536 THEN m.metadata END,octet_length(m.metadata),m.job FROM native_source_point_manifests m JOIN native_query_jobs j ON j.job=m.job WHERE m.path=?1 AND m.generation=?2 AND j.phase<>'cleanup'").map_err(io::Error::other)?;
        let row: Option<(Option<String>, usize, i64)> = query
            .query_row(
                params![witness.source.path.to_string_lossy(), generation],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        let Some((text, bytes, job)) = row else {
            return Ok((None, work));
        };
        if bytes > STATE_BYTES {
            return Err(invalid_point(
                "source point predecessor exceeds metadata budget",
            ));
        }
        work.decoded_rows += 1;
        work.metadata_bytes += bytes + 8;
        let manifest: PointManifest = serde_json::from_str(
            &text.ok_or_else(|| invalid_point("source point predecessor absent"))?,
        )
        .map_err(io::Error::other)?;
        if manifest.format != FORMAT || manifest.source_job != job {
            return Err(invalid_point("source point predecessor binding changed"));
        }
        drop(query);
        drop(tx);
        self.source_point_predecessor_proof(witness, manifest, work)
    }

    fn source_point_predecessor_proof(
        &self,
        witness: &SourceWitness,
        manifest: PointManifest,
        mut work: Work,
    ) -> io::Result<(Option<Predecessor>, Work)> {
        let mut query=self.connection.prepare("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,octet_length(metadata) FROM native_query_sources WHERE job=?1 AND path=?2").map_err(io::Error::other)?;
        let (text, bytes): (Option<String>, usize) = query
            .query_row(
                params![manifest.source_job, witness.source.path.to_string_lossy()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        if bytes > BYTES {
            return Err(invalid_point(
                "source point predecessor proof exceeds native metadata budget",
            ));
        }
        work.decoded_rows += 1;
        work.metadata_bytes += bytes;
        let source = serde_json::from_str(
            &text.ok_or_else(|| invalid_point("source point predecessor proof absent"))?,
        )
        .map_err(io::Error::other)?;
        drop(query);
        if !self.source_witness_current(witness)? {
            return Err(invalid_point("source predecessor observation changed"));
        }
        // The consumer fences this read with its actual namespace/custody and
        // complete captured writer-change chain before reusing Original bytes.
        Ok((
            Some(Predecessor {
                source,
                root: manifest.root,
                change: manifest.change,
                source_job: manifest.source_job,
            }),
            work,
        ))
    }
}

pub(in crate::events::store) fn publication_current(
    db: &Connection,
    batch: &crate::model::relation_stage::Batch,
) -> rusqlite::Result<bool> {
    let epoch = Epoch {
        epoch: batch.epoch,
        base_generation: batch.base_generation.clone(),
        source_revision: batch.source_revision,
        runtime: batch.runtime.clone(),
    };
    let scope = epoch_scope(&epoch).map_err(|error| invalid(&error.to_string()))?;
    for path in &batch.paths {
        let ready:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=?1 AND (s.withdrawal IS NOT NULL OR EXISTS(SELECT 1 FROM native_source_point_heads h JOIN native_query_jobs j ON j.job=h.job WHERE h.path=s.source_path AND h.scope=?2 AND j.phase<>'cleanup' AND j.sources=1 AND j.owners=0)))",params![path,scope],|row|row.get(0))?;
        if !ready {
            return Ok(false);
        }
    }
    Ok(true)
}

/// No generation, ledger, owner, native source ordinal, or final handoff index
/// is embedded here. A retained root remains usable under a newly admitted
/// exact source proof; final placement/copy/relationship qualification is owned
/// by the candidate forest producer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PointRecipe {
    pub(crate) candidate: Candidate,
    pub(crate) emission: EmissionOrder,
    pub(crate) found: Event,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PointManifest {
    pub(crate) format: u32,
    pub(crate) source_job: i64,
    pub(crate) root: CandidateRoot,
    pub(crate) counts: [usize; 3],
    pub(crate) change: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: u32,
    manifest: PointManifest,
    /// Last fully applied writer observation, which can precede the target.
    generation: String,
    phase: Phase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Phase {
    Changes,
    Interests {
        change: i64,
        after: Option<(u8, usize)>,
        generation: String,
    },
    Range {
        change: i64,
        after: (u8, usize),
        generation: String,
        extra: bool,
        next: usize,
        end: usize,
    },
    Prepared {
        change: i64,
        after: (u8, usize),
        generation: String,
        extra: bool,
        next: usize,
        end: usize,
        leaf: Option<CandidateLeaf>,
        begun: bool,
    },
    Cold {
        extra: bool,
        next: usize,
    },
    ColdPrepared {
        extra: bool,
        next: usize,
        leaf: Option<CandidateLeaf>,
        begun: bool,
    },
    Finish,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Pending,
    /// A source-sized Current text cell must use the existing bounded Original
    /// source producer. No complete Event Index read is attempted here.
    OriginalRequired,
    Complete,
    Conflict,
    Deferred,
}

#[derive(Default, Debug)]
pub(crate) struct Receipt {
    /// All actual closure callbacks, including refusal paths. The checkpoint
    /// receipt's closure field is a subset, never additive to this total.
    pub(crate) closure_all: Work,
    pub(crate) point_evidence: Work,
    pub(crate) control: Work,
    pub(crate) tree: CandidateWork,
    pub(crate) checkpoint: source_roots::CheckpointWork,
    pub(crate) prepared_candidates: usize,
    pub(crate) imported_roots: usize,
    /// Whole API connection delta, including nested primitives, all source/job
    /// controls and closure SQL. Child VM fields are subsets, not additive.
    /// RuntimeFacts filesystem work and provider/proof I/O are separate owners.
    pub(crate) sqlite_vm_steps: u64,
}

#[derive(Debug)]
struct Failed {
    error: io::Error,
    receipt: Receipt,
}
impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}
impl std::error::Error for Failed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
impl Receipt {
    pub(crate) fn from_error(error: &io::Error) -> Option<&Self> {
        error
            .get_ref()?
            .downcast_ref::<Failed>()
            .map(|failure| &failure.receipt)
    }
}

fn invalid_point(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Address order is deliberately independent of transcript placement. Its
/// only use is point replacement/removal, never a native semantic tie order.
pub(crate) fn address_key(address: EventAddress, identity: [u8; 32]) -> [u8; 69] {
    let mut key = [0; 69];
    key[0] = u8::from(address.extra);
    key[1..9].copy_from_slice(&(address.seq as u64).to_be_bytes());
    key[37..].copy_from_slice(&identity);
    key
}

fn identity(path: &Path, address: EventAddress) -> [u8; 32] {
    let path = path.as_os_str().as_encoded_bytes();
    let mut digest = Sha256::new();
    digest.update(b"native-source-point/v1");
    digest.update((path.len() as u64).to_be_bytes());
    digest.update(path);
    digest.update([u8::from(address.extra)]);
    digest.update((address.seq as u64).to_be_bytes());
    digest.finalize().into()
}

fn no_combine(left: &str, right: &str) -> io::Result<String> {
    if left != "{}" || right != "{}" {
        return Err(invalid_point("source address aggregate is not empty"));
    }
    Ok("{}".into())
}

impl NativeStore {
    /// One durable producer operation. This never drains the job in a caller
    /// loop and never loads a selected Index or complete source recipe Vec.
    pub(crate) fn source_points_step(
        &mut self,
        epoch: &Epoch,
        witness: &SourceWitness,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<ClosureRead>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Progress, Receipt)> {
        let reads = std::cell::RefCell::new(Work::default());
        let metered = |db: &Connection, job| {
            let read = closure(db, job)?;
            add_work(&mut reads.borrow_mut(), read.work.clone())
                .map_err(|error| invalid(&error.to_string()))?;
            Ok(read)
        };
        let mut receipt = Receipt::default();
        let measurement = super::vm_meter::begin(&self.connection);
        let result =
            self.source_points_step_inner(epoch, witness, &metered, cancelled, &mut receipt);
        receipt.closure_all = reads.into_inner();
        receipt.sqlite_vm_steps = measurement.finish(&self.connection);
        match result {
            Ok(progress) => Ok((progress, receipt)),
            Err(error) => Err(io::Error::new(error.kind(), Failed { error, receipt })),
        }
    }

    fn source_points_step_inner(
        &mut self,
        epoch: &Epoch,
        witness: &SourceWitness,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<ClosureRead>,
        cancelled: &dyn Fn() -> bool,
        receipt: &mut Receipt,
    ) -> io::Result<Progress> {
        let seen = std::cell::Cell::new(false);
        let cancel = || {
            if cancelled() {
                seen.set(true);
            }
            seen.get()
        };
        if cancel() {
            return Ok(Progress::Deferred);
        }
        if witness.ledger.is_none() {
            return Ok(Progress::OriginalRequired);
        }
        let Some(stage) = self.begin_source_roots(epoch, std::slice::from_ref(witness))? else {
            return Ok(Progress::Conflict);
        };
        let qualify = |db: &Connection, job| {
            let read = closure(db, job)?;
            let bounded = read.work.decoded_rows <= 16 && read.work.metadata_bytes <= 128 * 1024;
            Ok(read.current && bounded)
        };
        let scoped = |db: &Connection, job| {
            Ok(source_roots::scope_current(db, &stage)? && qualify(db, job)?)
        };
        let generation = source_fact_generation(
            witness
                .ledger
                .as_ref()
                .ok_or_else(|| invalid_point("Current source points require a ledger"))?,
        )
        .map_err(io::Error::other)?;
        let mut state: State = if stage.job.state.get("source_points").is_some() {
            serde_json::from_value(stage.job.state["source_points"].clone())
                .map_err(io::Error::other)?
        } else {
            if stage.job.phase != "capture" {
                return Err(invalid_point("source point job state is absent"));
            }
            let Some(counts) = self.source_point_counts(witness, &mut receipt.control)? else {
                return Ok(Progress::OriginalRequired);
            };
            let previous = self.source_point_head(witness, &mut receipt.control)?;
            let (manifest, previous_generation, phase) = if let Some((old_generation, manifest)) =
                previous
            {
                let step = self.candidate_tree_source_begin_neighbor(
                    &stage,
                    "source-point-import",
                    &manifest.root,
                    [0; 69],
                    CandidateDirection::Forward,
                    &qualify,
                    &cancel,
                )?;
                receipt.sqlite_vm_steps += step.work.sqlite_vm_steps;
                receipt.tree = step.work;
                if step.outcome != Outcome::Written {
                    return Ok(progress(step.outcome));
                }
                receipt.imported_roots = 1;
                (manifest, old_generation, Phase::Changes)
            } else {
                let (outcome, value) =
                    self.rank_guard(&stage.job, "{}", "{}", &[], &scoped, &cancel)?;
                if outcome != Outcome::Written {
                    return Ok(progress(outcome));
                }
                let (empty, _) = value.ok_or_else(|| invalid_point("source empty graph absent"))?;
                let change =
                    self.source_point_latest_change(witness, &generation, &mut receipt.control)?;
                // The first preparation has no predecessor to reuse. A later
                // epoch always enters Changes with its immutable old root.
                (
                    PointManifest {
                        format: FORMAT,
                        source_job: stage.job.id,
                        root: CandidateRoot::empty(empty),
                        counts,
                        change,
                    },
                    generation.clone(),
                    Phase::Cold {
                        extra: false,
                        next: 0,
                    },
                )
            };
            let state = State {
                format: FORMAT,
                manifest,
                generation: previous_generation,
                phase,
            };
            let result = self.source_point_checkpoint(&stage, &state, closure, &cancel, receipt)?;
            return Ok(result);
        };
        if state.format != FORMAT || state.manifest.format != FORMAT {
            return Err(invalid_point("unsupported source point state"));
        }
        let action = state.phase.clone();
        let result = match action {
            Phase::Changes => {
                let change = self.source_point_next_change(
                    witness,
                    state.manifest.change,
                    &mut receipt.control,
                )?;
                match change {
                    None => {
                        if state.generation != generation {
                            return Ok(Progress::Conflict);
                        }
                        let Some(counts) =
                            self.source_point_counts(witness, &mut receipt.control)?
                        else {
                            return Ok(Progress::OriginalRequired);
                        };
                        state.manifest.counts = counts;
                        state.manifest.source_job = stage.job.id;
                        state.phase = Phase::Finish;
                    }
                    Some(change) => {
                        if change.before_generation.as_deref() != Some(&state.generation) {
                            return Ok(Progress::Conflict);
                        }
                        let Some(next_generation) = change.generation else {
                            return Ok(Progress::Conflict);
                        };
                        if change.reset {
                            let Some(counts) =
                                self.source_point_counts(witness, &mut receipt.control)?
                            else {
                                return Ok(Progress::OriginalRequired);
                            };
                            let (outcome, value) =
                                self.rank_guard(&stage.job, "{}", "{}", &[], &scoped, &cancel)?;
                            if outcome != Outcome::Written {
                                return Ok(progress(outcome));
                            }
                            let (empty, _) =
                                value.ok_or_else(|| invalid_point("source empty graph absent"))?;
                            // An actual writer reset invalidates the prefix.
                            // Its range is represented once, not enumerated
                            // while capturing the writer transaction.
                            state.manifest = PointManifest {
                                format: FORMAT,
                                source_job: stage.job.id,
                                root: CandidateRoot::empty(empty),
                                counts,
                                change: self.source_point_latest_change(
                                    witness,
                                    &generation,
                                    &mut receipt.control,
                                )?,
                            };
                            state.generation = generation.clone();
                            state.phase = Phase::Cold {
                                extra: false,
                                next: 0,
                            };
                        } else {
                            state.phase = Phase::Interests {
                                change: change.id,
                                after: None,
                                generation: next_generation,
                            };
                        }
                    }
                }
                self.source_point_checkpoint(&stage, &state, closure, &cancel, receipt)?
            }
            Phase::Interests {
                change,
                after,
                generation: next_generation,
            } => {
                let interest =
                    self.source_point_interest(witness, change, after, &mut receipt.control)?;
                if let Some(interest) = interest {
                    let after = (interest.channel, interest.first);
                    if interest.channel <= 1 {
                        let extra = interest.channel == 1;
                        let count = state.manifest.counts[usize::from(extra)];
                        let end = match interest.last {
                            Some(end) if end == interest.first => end
                                .checked_add(1)
                                .ok_or_else(|| invalid_point("source point range overflow"))?,
                            Some(end) => end,
                            None => count,
                        };
                        state.phase = Phase::Range {
                            change,
                            after,
                            generation: next_generation,
                            extra,
                            next: interest.first,
                            end,
                        };
                    } else {
                        // Signal/link owners consume their own writer channel;
                        // they do not manufacture native event point recipes.
                        state.phase = Phase::Interests {
                            change,
                            after: Some(after),
                            generation: next_generation,
                        };
                    }
                } else {
                    state.manifest.change = change;
                    state.generation = next_generation;
                    state.phase = Phase::Changes;
                }
                self.source_point_checkpoint(&stage, &state, closure, &cancel, receipt)?
            }
            Phase::Range {
                change,
                after,
                generation,
                extra,
                next,
                end,
            } => {
                if next >= end {
                    state.phase = Phase::Interests {
                        change,
                        after: Some(after),
                        generation,
                    };
                } else {
                    let Some(leaf) = self.source_point_prepare(
                        witness,
                        EventAddress { extra, seq: next },
                        &state.manifest.root,
                        receipt,
                    )?
                    else {
                        return Ok(Progress::OriginalRequired);
                    };
                    state.phase = Phase::Prepared {
                        change,
                        after,
                        generation,
                        extra,
                        next,
                        end,
                        leaf,
                        begun: false,
                    };
                }
                self.source_point_checkpoint(&stage, &state, closure, &cancel, receipt)?
            }
            Phase::Cold { extra, next } => {
                if next >= state.manifest.counts[usize::from(extra)] {
                    state.phase = if extra {
                        Phase::Finish
                    } else {
                        Phase::Cold {
                            extra: true,
                            next: 0,
                        }
                    };
                } else {
                    let Some(leaf) = self.source_point_prepare(
                        witness,
                        EventAddress { extra, seq: next },
                        &state.manifest.root,
                        receipt,
                    )?
                    else {
                        return Ok(Progress::OriginalRequired);
                    };
                    state.phase = Phase::ColdPrepared {
                        extra,
                        next,
                        leaf,
                        begun: false,
                    };
                }
                self.source_point_checkpoint(&stage, &state, closure, &cancel, receipt)?
            }
            Phase::Prepared {
                change,
                after,
                generation,
                extra,
                next,
                end,
                leaf,
                begun,
            } => {
                let result = self.source_point_update(
                    &stage,
                    &witness.source.path,
                    &state.manifest.root,
                    EventAddress { extra, seq: next },
                    leaf,
                    begun,
                    &scoped,
                    &cancel,
                    receipt,
                )?;
                if let Some(root) = result.1 {
                    state.manifest.root = root;
                    state.phase = Phase::Range {
                        change,
                        after,
                        generation,
                        extra,
                        next: next
                            .checked_add(1)
                            .ok_or_else(|| invalid_point("source point sequence overflow"))?,
                        end,
                    };
                } else if result.0 == Progress::Pending {
                    if let Phase::Prepared { begun, .. } = &mut state.phase {
                        *begun = true;
                    }
                } else {
                    return Ok(result.0);
                }
                self.source_point_checkpoint(&stage, &state, closure, &cancel, receipt)?
            }
            Phase::ColdPrepared {
                extra,
                next,
                leaf,
                begun,
            } => {
                let result = self.source_point_update(
                    &stage,
                    &witness.source.path,
                    &state.manifest.root,
                    EventAddress { extra, seq: next },
                    leaf,
                    begun,
                    &scoped,
                    &cancel,
                    receipt,
                )?;
                if let Some(root) = result.1 {
                    state.manifest.root = root;
                    state.phase = Phase::Cold {
                        extra,
                        next: next
                            .checked_add(1)
                            .ok_or_else(|| invalid_point("source point sequence overflow"))?,
                    };
                } else if result.0 == Progress::Pending {
                    if let Phase::ColdPrepared { begun, .. } = &mut state.phase {
                        *begun = true;
                    }
                } else {
                    return Ok(result.0);
                }
                self.source_point_checkpoint(&stage, &state, closure, &cancel, receipt)?
            }
            Phase::Finish => {
                if state.generation != generation {
                    return Ok(Progress::Conflict);
                }
                self.source_point_seal(
                    &stage,
                    witness,
                    &generation,
                    &state.manifest,
                    &qualify,
                    &cancel,
                    receipt,
                )?
            }
        };
        Ok(result)
    }

    fn source_point_prepare(
        &mut self,
        witness: &SourceWitness,
        address: EventAddress,
        root: &CandidateRoot,
        receipt: &mut Receipt,
    ) -> io::Result<Option<Option<CandidateLeaf>>> {
        let Some(counts) = self.source_point_counts(witness, &mut receipt.control)? else {
            return Ok(None);
        };
        if address.seq >= counts[usize::from(address.extra)] {
            return Ok(Some(None));
        }
        if address.extra {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            let file = current_file(&tx, witness).map_err(io::Error::other)?;
            let mut query = tx
                .prepare("SELECT k='operation' FROM events WHERE file_id=?1 AND extra=1 AND seq=?2")
                .map_err(io::Error::other)?;
            let operation: bool = query
                .query_row(
                    params![file, sequence_sql(address.seq).map_err(io::Error::other)?],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            receipt.control.decoded_rows += 1;
            receipt.control.metadata_bytes += 1;
            receipt.control.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
            if !operation {
                return Ok(Some(None));
            }
        }
        let id = identity(&witness.source.path, address);
        let read = self.source_candidate(
            source_candidate::Evidence::Current(witness),
            &source_candidate::Request {
                address,
                member: if address.extra {
                    source_candidate::Member::Operation
                } else {
                    source_candidate::Member::Native {
                        background_end: false,
                    }
                },
                identity: id,
                ordinal: 0,
                subagent_handback: None,
            },
        )?;
        add_work(&mut receipt.point_evidence, read.work).map_err(io::Error::other)?;
        match read.prepared {
            source_candidate::Prepared::OriginalRequired { .. }
            | source_candidate::Prepared::NamePredicateRequired(_) => Ok(None),
            source_candidate::Prepared::Omitted => Ok(Some(None)),
            source_candidate::Prepared::Candidate { candidate, found } => {
                let emission = if address.extra {
                    EmissionOrder::operation(address.seq)
                } else {
                    EmissionOrder::regular(address.seq, 0)
                };
                let payload = bounded_json(
                    &PointRecipe {
                        candidate,
                        found,
                        emission,
                    },
                    48 * 1024,
                )
                .map_err(io::Error::other)?;
                receipt.prepared_candidates += 1;
                Ok(Some(Some(CandidateLeaf {
                    candidate: id,
                    aggregate: root.empty_graph(),
                    payload: Some(payload),
                })))
            }
        }
    }

    fn source_point_update(
        &mut self,
        stage: &SourceStage,
        path: &Path,
        root: &CandidateRoot,
        address: EventAddress,
        leaf: Option<CandidateLeaf>,
        begun: bool,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        receipt: &mut Receipt,
    ) -> io::Result<(Progress, Option<CandidateRoot>)> {
        let mut digest = Sha256::new();
        digest.update(b"source-point-update/v1");
        digest.update(
            bounded_json(root, STATE_BYTES)
                .map_err(io::Error::other)?
                .as_bytes(),
        );
        let id = identity(path, address);
        digest.update(address_key(address, id));
        let cursor = format!("source-point/{:x}", digest.finalize());
        let exists:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM native_rank_candidate_cursors WHERE job=?1 AND cursor=?2)",params![stage.job.id,cursor],|row|row.get(0)).map_err(io::Error::other)?;
        receipt.control.decoded_rows += 1;
        receipt.control.metadata_bytes += 1;
        let step = if begun && exists {
            self.candidate_tree_source_advance_update(
                stage,
                &cursor,
                COMBINE,
                &no_combine,
                closure,
                cancelled,
            )?
        } else {
            self.candidate_tree_source_begin_update(
                stage,
                &cursor,
                root,
                address_key(address, id),
                leaf,
                COMBINE,
                closure,
                cancelled,
            )?
        };
        receipt.sqlite_vm_steps += step.work.sqlite_vm_steps;
        receipt.tree = step.work;
        Ok((progress(step.outcome), step.complete))
    }

    fn source_point_checkpoint(
        &mut self,
        stage: &SourceStage,
        state: &State,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<ClosureRead>,
        cancelled: &dyn Fn() -> bool,
        receipt: &mut Receipt,
    ) -> io::Result<Progress> {
        // Tree/graph writes increment the same CAS revision. Reload the exact
        // source namespace before recording its resulting root pointer.
        let job = self
            .reload(&stage.job.spec.view_key)?
            .ok_or_else(|| invalid_point("source point job disappeared"))?;
        if job.state != stage.job.state
            || job.cursor != stage.job.cursor
            || job.phase != stage.job.phase
            || job.revision < stage.job.revision
            || job.revision
                > stage
                    .job
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid_point("source point revision overflow"))?
        {
            return Ok(Progress::Conflict);
        }
        let fresh = SourceStage {
            job,
            epoch: stage.epoch.clone(),
        };
        let cursor = fresh.job.revision.to_string();
        let mut next = fresh.job.state.clone();
        next.as_object_mut()
            .ok_or_else(|| invalid_point("source point job state is not an object"))?
            .insert(
                "source_points".into(),
                serde_json::to_value(state).map_err(io::Error::other)?,
            );
        let (outcome, work) = self.checkpoint_source_roots(
            &fresh,
            if matches!(state.phase, Phase::Finish) {
                "finish"
            } else {
                "entries"
            },
            &cursor,
            &next,
            &[],
            closure,
            cancelled,
        )?;
        receipt.sqlite_vm_steps += work.sqlite_vm_steps;
        receipt.checkpoint = work;
        if outcome != Outcome::Written {
            return Ok(progress(outcome));
        }
        let mut after = fresh;
        after.job.phase = if matches!(state.phase, Phase::Finish) {
            "finish"
        } else {
            "entries"
        }
        .into();
        after.job.cursor = cursor;
        after.job.state = next;
        after.job.revision = after
            .job
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid_point("source point revision overflow"))?;
        let checked = |db: &Connection, job| {
            let read = closure(db, job)?;
            Ok(read.current
                && read.work.decoded_rows <= 16
                && read.work.metadata_bytes <= 128 * 1024)
        };
        let (outcome, vm) = self.recheck_source_closure(&after, &checked, cancelled)?;
        receipt.sqlite_vm_steps += vm;
        Ok(progress(outcome))
    }
}

fn progress(outcome: Outcome) -> Progress {
    match outcome {
        Outcome::Written => Progress::Pending,
        Outcome::Deferred => Progress::Deferred,
        _ => Progress::Conflict,
    }
}

impl NativeStore {
    fn source_point_counts(
        &self,
        witness: &SourceWitness,
        work: &mut Work,
    ) -> io::Result<Option<[usize; 3]>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let Some(file) = current_file_if_present(&tx, witness).map_err(io::Error::other)? else {
            return Ok(None);
        };
        let Some(ledger) = &witness.ledger else {
            return Ok(None);
        };
        let generation = source_fact_generation(ledger).map_err(io::Error::other)?;
        let mut query=tx.prepare("SELECT events,extras,signals FROM native_source_facts WHERE file_id=?1 AND generation=?2").map_err(io::Error::other)?;
        let counts = query
            .query_row(params![file, generation], |row| {
                Ok([
                    seq_of(row.get(0)?),
                    seq_of(row.get(1)?),
                    seq_of(row.get(2)?),
                ])
            })
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        if counts.is_some() {
            work.decoded_rows += 1;
            work.metadata_bytes += 24;
        }
        Ok(counts)
    }

    fn source_point_head(
        &self,
        witness: &SourceWitness,
        work: &mut Work,
    ) -> io::Result<Option<(String, PointManifest)>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(invalid_point("source point observation changed"));
        }
        let mut query=tx.prepare("SELECT CASE WHEN octet_length(h.generation)+octet_length(h.metadata)<=65536 THEN h.generation END,CASE WHEN octet_length(h.generation)+octet_length(h.metadata)<=65536 THEN h.metadata END,octet_length(h.generation)+octet_length(h.metadata),h.job FROM native_source_point_heads h JOIN native_query_jobs j USING(job) WHERE h.path=?1 AND j.phase<>'cleanup'").map_err(io::Error::other)?;
        let row: Option<(Option<String>, Option<String>, usize, i64)> = query
            .query_row([witness.source.path.to_string_lossy()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        let Some((generation, text, length, job)) = row else {
            return Ok(None);
        };
        if length > STATE_BYTES {
            return Err(invalid_point(
                "source point predecessor exceeds metadata budget",
            ));
        }
        work.decoded_rows += 1;
        work.metadata_bytes += length;
        let generation = generation
            .ok_or_else(|| invalid_point("source point predecessor generation absent"))?;
        let manifest: PointManifest = serde_json::from_str(
            &text.ok_or_else(|| invalid_point("source point predecessor absent"))?,
        )
        .map_err(io::Error::other)?;
        if manifest.format != FORMAT || manifest.source_job != job {
            return Err(invalid_point("invalid source point predecessor"));
        }
        Ok(Some((generation, manifest)))
    }

    fn source_point_latest_change(
        &self,
        witness: &SourceWitness,
        generation: &str,
        work: &mut Work,
    ) -> io::Result<i64> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(invalid_point("source point observation changed"));
        }
        let mut query=tx.prepare("SELECT change_id,CASE WHEN octet_length(generation)<=65536 THEN generation END,octet_length(generation) FROM native_candidate_changes WHERE path=?1 ORDER BY change_id DESC LIMIT 1").map_err(io::Error::other)?;
        let row: Option<(i64, Option<String>, Option<usize>)> = query
            .query_row([witness.source.path.to_string_lossy()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        let (id, actual, length) =
            row.ok_or_else(|| invalid_point("source point writer observation absent"))?;
        if length.is_some_and(|bytes| bytes > STATE_BYTES) || actual.as_deref() != Some(generation)
        {
            return Err(invalid_point("source point writer observation changed"));
        }
        work.decoded_rows += 1;
        work.metadata_bytes += length.unwrap_or(0) + 8;
        Ok(id)
    }

    fn source_point_next_change(
        &self,
        witness: &SourceWitness,
        after: i64,
        work: &mut Work,
    ) -> io::Result<Option<CandidateChange>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(invalid_point("source point observation changed"));
        }
        let mut query=tx.prepare("SELECT change_id,CASE WHEN COALESCE(octet_length(before_generation),0)+COALESCE(octet_length(generation),0)<=65536 THEN before_generation END,CASE WHEN COALESCE(octet_length(before_generation),0)+COALESCE(octet_length(generation),0)<=65536 THEN generation END,reset,events,extras,signals,COALESCE(octet_length(before_generation),0)+COALESCE(octet_length(generation),0)+64 FROM native_candidate_changes WHERE path=?1 AND change_id>?2 ORDER BY change_id LIMIT 1").map_err(io::Error::other)?;
        let row = query
            .query_row(params![witness.source.path.to_string_lossy(), after], |r| {
                Ok((
                    CandidateChange {
                        id: r.get(0)?,
                        before_generation: r.get(1)?,
                        generation: r.get(2)?,
                        reset: r.get(3)?,
                        counts: (seq_of(r.get(4)?), seq_of(r.get(5)?), seq_of(r.get(6)?)),
                    },
                    r.get::<_, usize>(7)?,
                ))
            })
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        let Some((change, bytes)) = row else {
            return Ok(None);
        };
        if bytes > STATE_BYTES + 64 {
            return Err(invalid_point(
                "source point writer observation exceeds metadata budget",
            ));
        }
        work.decoded_rows += 1;
        work.metadata_bytes += bytes;
        Ok(Some(change))
    }

    fn source_point_interest(
        &self,
        witness: &SourceWitness,
        change: i64,
        after: Option<(u8, usize)>,
        work: &mut Work,
    ) -> io::Result<Option<CandidateInterest>> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !check_source(&tx, witness).map_err(io::Error::other)? {
            return Err(invalid_point("source point observation changed"));
        }
        let start = after.map_or((-1, -1), |(kind, seq)| (i64::from(kind), seq_int(seq)));
        // Raw before/after text belongs to its descriptor/copy/relationship
        // owners. This address job needs only the exact bounded range header.
        let mut query=tx.prepare("SELECT i.channel,i.first,i.last FROM native_candidate_interests i JOIN native_candidate_changes c USING(change_id) WHERE c.path=?1 AND i.change_id=?2 AND (i.channel,i.first)>(?3,?4) ORDER BY i.channel,i.first LIMIT 1").map_err(io::Error::other)?;
        let row = query
            .query_row(
                params![
                    witness.source.path.to_string_lossy(),
                    change,
                    start.0,
                    start.1
                ],
                |r| {
                    Ok(CandidateInterest {
                        channel: r.get(0)?,
                        first: seq_of(r.get(1)?),
                        last: r.get::<_, Option<i64>>(2)?.map(seq_of),
                        before: None,
                        after: None,
                    })
                },
            )
            .optional()
            .map_err(io::Error::other)?;
        work.vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        if row.is_some() {
            work.decoded_rows += 1;
            work.metadata_bytes += 24;
        }
        Ok(row)
    }

    fn source_point_seal(
        &mut self,
        stage: &SourceStage,
        witness: &SourceWitness,
        generation: &str,
        manifest: &PointManifest,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        receipt: &mut Receipt,
    ) -> io::Result<Progress> {
        let text = bounded_json(manifest, STATE_BYTES).map_err(io::Error::other)?;
        if cancelled() {
            return Ok(Progress::Deferred);
        }
        if !self.runtime_current(&stage.job.spec)? {
            return Ok(Progress::Conflict);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if stage.job.phase != "finish"
            || !source_roots::scope_current(&tx, stage).map_err(io::Error::other)?
            || !job_current(&tx, &stage.job).map_err(io::Error::other)?
            || !closure(&tx, stage.job.id).map_err(io::Error::other)?
            || manifest.source_job != stage.job.id
        {
            return Ok(Progress::Conflict);
        }
        tx.execute("INSERT INTO native_source_point_manifests VALUES(?1,?2,?3,?4,?5) ON CONFLICT(path,generation) DO UPDATE SET job=excluded.job,change_id=excluded.change_id,metadata=excluded.metadata",params![witness.source.path.to_string_lossy(),generation,stage.job.id,manifest.change,text]).map_err(io::Error::other)?;
        tx.execute("INSERT INTO native_source_point_heads VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(path) DO UPDATE SET generation=excluded.generation,job=excluded.job,change_id=excluded.change_id,metadata=excluded.metadata,scope=excluded.scope",params![witness.source.path.to_string_lossy(),generation,manifest.change,stage.job.id,text,epoch_scope(&stage.epoch)?]).map_err(io::Error::other)?;
        advance_frontier(&tx, &stage.epoch, &witness.source.path.to_string_lossy())?;
        if cancelled() {
            return Ok(Progress::Deferred);
        }
        if !job_current(&tx, &stage.job).map_err(io::Error::other)?
            || !source_roots::scope_current(&tx, stage).map_err(io::Error::other)?
            || !closure(&tx, stage.job.id).map_err(io::Error::other)?
            || crate::native_evidence::query_runtime_generation(&self.options)?
                != stage.job.spec.runtime
        {
            return Ok(Progress::Conflict);
        }
        tx.commit().map_err(io::Error::other)?;
        receipt.control.decoded_rows += 2;
        receipt.control.metadata_bytes += 2 * text.len();
        let (outcome, vm) = self.recheck_source_closure(stage, closure, cancelled)?;
        receipt.sqlite_vm_steps += vm;
        Ok(if outcome == Outcome::Written {
            Progress::Complete
        } else {
            progress(outcome)
        })
    }
}
