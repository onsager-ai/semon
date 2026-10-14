//! Point evidence for the single native candidate reducer. Original references
//! remain Original: a missing owned String in scalar() never proves absence.
use super::*;
use crate::events::Kind;
use crate::model::{
    native_producer::{Candidate, EventInput},
    native_stage::{self, EntryKind},
    range_stage::CandidateRef,
};

/// The existing relationship producer decides the member; this adapter never
/// infers placement or incoming membership from rendered output recipes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Member {
    Native { background_end: bool },
    Placed { handoff: usize },
    After { handoff: usize },
    Operation,
}

pub(crate) enum Evidence<'a> {
    Current(&'a SourceWitness),
    Original {
        job: &'a Job,
        witness: &'a SourceWitness,
        repair: &'a native_repair::Repair,
    },
}

pub(crate) struct Request {
    pub(crate) address: EventAddress,
    pub(crate) member: Member,
    pub(crate) identity: CandidateRef,
    /// Supplied by the native stable emission-order owner, never a packed seq.
    pub(crate) ordinal: u64,
    /// Exact, qualified cached predicate produced while folding Original.
    /// Current resolves its owned native name directly. None is pending work.
    pub(crate) subagent_handback: Option<bool>,
}

pub(crate) enum Prepared {
    Candidate {
        candidate: Candidate,
        /// Only scalar event completion observations enter range facts.
        found: Event,
    },
    Omitted,
    /// Queue bounded Original repair for this exact source; never drain it in
    /// the foreground or reject an otherwise valid large native event.
    OriginalRequired {
        metadata_bytes: usize,
    },
    /// The controller must qualify this literal through the existing native
    /// text/literal owner. It must not run a FoldRecord action after Complete.
    NamePredicateRequired(native_repair::OriginalText),
}

pub(crate) struct Read {
    pub(crate) prepared: Prepared,
    /// Primitive statement work. The controller additionally measures complete
    /// connection instructions, source/owner/closure fences and RuntimeFacts.
    pub(crate) work: Work,
}

impl NativeStore {
    pub(crate) fn source_candidate(
        &self,
        evidence: Evidence<'_>,
        request: Request,
    ) -> io::Result<Read> {
        let mut work = Work::default();
        let (found, handback, name) = match &evidence {
            Evidence::Current(witness) => {
                let (found, read) = self.event_point(
                    witness,
                    request.address.extra,
                    request.address.seq,
                    STATE_BYTES,
                )?;
                add_work(&mut work, read)?;
                let found = match found {
                    EventPoint::Event(event) => event,
                    EventPoint::Missing => {
                        return Err(io::Error::other("native candidate event disappeared"));
                    }
                    EventPoint::OriginalRequired { bytes } => {
                        return Ok(Read {
                            prepared: Prepared::OriginalRequired {
                                metadata_bytes: bytes,
                            },
                            work,
                        });
                    }
                };
                let handback = found.n.as_deref() == Some("SubagentHandback");
                let scalar =
                    crate::events::native_event::NativeEvent::<String>::from(found).scalar();
                (scalar, Some(handback), None)
            }
            Evidence::Original {
                job,
                witness,
                repair,
            } => {
                let (found, read) =
                    self.original_event_at(job, witness, repair, request.address)?;
                add_work(&mut work, read)?;
                let found = found
                    .ok_or_else(|| io::Error::other("Original candidate event disappeared"))?;
                let predicate = if found.n.is_none() {
                    Some(false)
                } else {
                    request.subagent_handback
                };
                (found.scalar(), predicate, found.n.clone())
            }
        };
        let time_at = |seq| match &evidence {
            Evidence::Current(witness) => self.source_time_at(witness, seq),
            Evidence::Original {
                job,
                witness,
                repair,
            } => self.original_source_time_at(job, witness, repair, seq),
        };
        let time_before = |offset| match &evidence {
            Evidence::Current(witness) => self.source_time_before_offset(witness, offset),
            Evidence::Original {
                job,
                witness,
                repair,
            } => self.original_source_time_before_offset(job, witness, repair, offset),
        };
        let (inherited, read) = if request.address.extra {
            time_before(found.o)?
        } else {
            time_at(request.address.seq)?
        };
        add_work(&mut work, read)?;
        let candidate = match request.member {
            Member::Operation => {
                if !request.address.extra || found.k != Kind::Operation {
                    return Err(io::Error::other(
                        "native operation member address disagrees",
                    ));
                }
                Candidate::operation(
                    request.address.seq,
                    &found,
                    inherited,
                    request.identity,
                    request.ordinal,
                )
            }
            Member::After { handoff } => {
                if request.address.extra {
                    return Err(io::Error::other(
                        "native after member requires a regular event",
                    ));
                }
                Candidate::after(
                    request.address.seq,
                    &found,
                    handoff,
                    inherited,
                    request.identity,
                    request.ordinal,
                )
            }
            member => {
                if request.address.extra {
                    return Err(io::Error::other("native regular member address disagrees"));
                }
                let (operation, read) = match &evidence {
                    Evidence::Current(witness) => {
                        self.last_operation(witness, request.address.seq)?
                    }
                    Evidence::Original {
                        job,
                        witness,
                        repair,
                    } => self.original_last_operation(job, witness, repair, request.address.seq)?,
                };
                add_work(&mut work, read)?;
                let kind = match member {
                    Member::Native { background_end } => {
                        let eligible = native_stage::event_kind_qualified(
                            &found,
                            background_end,
                            operation.is_some(),
                            false,
                        );
                        // Poll-only calls are omitted before the name predicate
                        // matters. No literal work is required for those calls.
                        if matches!(eligible, Some(EntryKind::Tool(_))) && handback.is_none() {
                            return Ok(Read {
                                prepared: Prepared::NamePredicateRequired(
                                    name.expect("unqualified native name has a descriptor"),
                                ),
                                work,
                            });
                        }
                        native_stage::event_kind_qualified(
                            &found,
                            background_end,
                            operation.is_some(),
                            handback.unwrap_or(false),
                        )
                    }
                    Member::Placed { handoff } => Some(EntryKind::H(handoff)),
                    _ => unreachable!(),
                };
                let Some(kind) = kind else {
                    return Ok(Read {
                        prepared: Prepared::Omitted,
                        work,
                    });
                };
                // Only a failed code-mode call without an explicit reply time
                // needs this native predecessor. Resolve errors before calling
                // the infallible shared model reducer, never fabricate a time.
                let reply_time = if matches!(kind, EntryKind::Tool(crate::model::ToolState::Err))
                    && operation.is_some()
                    && let Some(reply) = found.r.as_ref().filter(|reply| reply.t.is_none())
                {
                    let (time, read) = time_before(reply.o)?;
                    add_work(&mut work, read)?;
                    Some(time)
                } else {
                    None
                };
                Candidate::regular(
                    EventInput {
                        seq: request.address.seq,
                        event: &found,
                        kind,
                        inherited,
                        operation: operation.map(|(_, time, pos)| (time, pos)),
                        identity: request.identity,
                        ordinal: request.ordinal,
                    },
                    |_| reply_time.expect("qualified native reply time"),
                )
            }
        };
        Ok(Read {
            prepared: Prepared::Candidate { candidate, found },
            work,
        })
    }
}
