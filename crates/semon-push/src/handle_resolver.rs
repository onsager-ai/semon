//! Origin-owned admission for current source handles. No body UID is authority.
use crate::{
    body_cache::SourceWitness,
    origin_read::{EnrollmentScope, HandleResolver, OpaqueId, Refused, ResolvedSource},
    receiver_audit::Identifier,
};
use semon_asi::{
    egress::{AdmittedCohort, Handles, Registry, Snapshot},
    governance::{Destination, Principal, Scope},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

type Result<T> = std::result::Result<T, Refused>;

/// Must read both origin-local projection generations against these exact pinned
/// cohorts, refusing stale/missing/ambiguous evidence. Registry generations are
/// not activity or transcript generations.
pub trait Generations {
    fn read(&self, cohorts: &[AdmittedCohort]) -> Result<(OpaqueId, OpaqueId)>;
}

/// No public cohort-bound generation reader exists yet. Production integrations
/// must retain this refusal until the index supplies that reader.
pub struct UnavailableGenerations;
impl Generations for UnavailableGenerations {
    fn read(&self, _: &[AdmittedCohort]) -> Result<(OpaqueId, OpaqueId)> {
        Err(Refused::Unavailable)
    }
}

/// Trusted local configuration only. The principal is the publication principal,
/// never a receiver-supplied policy or a replacement for body governance.
pub struct OriginHandleResolver<'a> {
    index: &'a Path,
    registry: &'a Registry,
    principal: &'a Principal,
    generations: &'a dyn Generations,
}
impl<'a> OriginHandleResolver<'a> {
    pub fn new(
        index: &'a Path,
        registry: &'a Registry,
        principal: &'a Principal,
        generations: &'a dyn Generations,
    ) -> Self {
        Self {
            index,
            registry,
            principal,
            generations,
        }
    }
}
impl HandleResolver for OriginHandleResolver<'_> {
    fn resolve(
        &self,
        scope: &EnrollmentScope,
        handle: &OpaqueId,
    ) -> Result<Option<ResolvedSource>> {
        // Source selectors accept only the current src handle. Handles::resolve
        // applies the exact tag/lowercase-hex check before any reverse lookup.
        let binding = semon_sessions::analytics::read_binding(self.index)
            .map_err(|_| Refused::Unavailable)?
            .ok_or(Refused::Unavailable)?;
        if scope.namespace.as_str() != binding.namespace
            || scope.origin_incarnation.as_str() != binding.origin_incarnation
            || scope.index_incarnation.as_str() != binding.index_incarnation
        {
            return Ok(None);
        }
        let state = self.index.parent().ok_or(Refused::Unavailable)?;
        let handles = Handles::open(state, &binding.namespace).map_err(|_| Refused::Unavailable)?;
        let Some(native) = handles
            .resolve("src", handle.as_str())
            .map_err(|_| Refused::Unavailable)?
        else {
            return Ok(None);
        };
        let snapshot = Snapshot::pin(
            &state.join("analytics-store"),
            &state.join("analytics-namespace"),
            &binding.namespace,
        )
        .map_err(|_| Refused::Unavailable)?;
        let audience = Scope {
            destination: Destination::Receiver,
            namespace: binding.namespace,
            machine: scope.machine.as_str().to_owned(),
        };
        let cohorts: Vec<_> = snapshot
            .admitted_cohorts(Some(self.registry), self.principal, &audience)
            .map_err(|_| Refused::Unavailable)?
            .into_iter()
            .filter(|cohort| cohort.source_ids.contains(&native))
            .collect();
        if cohorts.is_empty() {
            return Ok(None);
        }
        // A source can contribute to several sessions. Pin their full union;
        // never select an arbitrary first session and lose relation witnesses.
        let cohort = map_cohorts(&handles, &cohorts)?;
        let cohort_id = Identifier::from_bytes(
            Sha256::digest(serde_json::to_vec(&cohort).map_err(|_| Refused::Unavailable)?).into(),
        );
        let (activity_generation, transcript_generation) = self.generations.read(&cohorts)?;
        Ok(Some(ResolvedSource {
            admitted_source: handle.clone(),
            cohort_id,
            cohort,
            activity_generation,
            transcript_generation,
        }))
    }
}

fn map_cohorts(handles: &Handles, cohorts: &[AdmittedCohort]) -> Result<Vec<SourceWitness>> {
    let mut members = BTreeMap::new();
    for cohort in cohorts {
        let mut seen = std::collections::BTreeSet::new();
        for witness in cohort.witnesses.values() {
            for source in &witness.sources {
                seen.insert(source.native_id.clone());
                let source_id = handles
                    .existing("src", &source.native_id)
                    .map_err(|_| Refused::Unavailable)?
                    .ok_or(Refused::Unavailable)?;
                // Prefix digest conservatively changes the chain on any prefix
                // change, including append. No archive-chain continuity is claimed.
                let member = SourceWitness {
                    source_id: source_id.clone(),
                    chain: source.prefix_sha256.clone(),
                    generation: witness.generation.clone(),
                    consumed_boundary: source.consumed_offset,
                };
                if let Some(previous) = members.insert(source_id, member.clone())
                    && previous != member
                {
                    return Err(Refused::Unavailable);
                }
            }
        }
        if seen != cohort.source_ids {
            return Err(Refused::Unavailable);
        }
    }
    if members.is_empty() {
        return Err(Refused::Unavailable);
    }
    Ok(members.into_values().collect())
}

#[cfg(test)]
mod tests;
