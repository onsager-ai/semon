//! One state machine for native usage. Backends supply only typed row effects;
//! the caller owns the transaction, ledger CAS and durable operation receipt.
use super::{Observation, header_ledger};
use crate::{events::{FileIndex, Ledger}, storage::StorageError};
use std::collections::BTreeSet;

pub(crate) struct Fact<'a> {
    pub source: i64,
    pub offset: u64,
    pub identity: &'a str,
    pub payload: &'a str,
    pub record: &'a str,
    pub tree: &'a str,
    pub model: &'a str,
}

pub(crate) trait Effects {
    fn current(&mut self, path: &str) -> Result<Option<(i64, String)>, StorageError>;
    fn create(&mut self, path: &str, receipt: &str) -> Result<i64, StorageError>;
    fn receipt(&mut self, source: i64, receipt: &str) -> Result<(), StorageError>;
    fn unblock(&mut self, path: &str, receipt: &str) -> Result<(), StorageError>;
    fn fact(&mut self, source: i64, offset: u64) -> Result<Option<(String, String)>, StorageError>;
    fn first(&mut self, source: i64, scope: &str, identity: &str) -> Result<Option<String>, StorageError>;
    fn identity(&mut self, source: i64, scope: &str, identity: &str, offset: u64) -> Result<(), StorageError>;
    fn insert(&mut self, fact: Fact<'_>) -> Result<(), StorageError>;
    fn dimension(&mut self, source: i64, kind: &str, key: &str, value: &str) -> Result<(), StorageError>;
    fn ready(&mut self, source: i64, checkpoint: u64) -> Result<(), StorageError>;
}

pub(crate) fn begin(
    effects: &mut impl Effects,
    path: &str,
    expected: Option<&Ledger>,
    ledger: &Ledger,
    append: bool,
) -> Result<i64, StorageError> {
    let receipt = header_ledger(ledger);
    let id = match effects.current(path)? {
        Some((id, old_receipt))
            if (append || expected == Some(ledger))
                && expected.is_some_and(|old| old_receipt == header_ledger(old)) => id,
        _ => effects.create(path, &receipt)?,
    };
    effects.receipt(id, &receipt)?;
    effects.unblock(path, &receipt)?;
    Ok(id)
}

fn encode(value: &impl serde::Serialize) -> Result<String, StorageError> {
    serde_json::to_string(value).map_err(|_| StorageError::InvalidArguments)
}

fn outcome(
    effects: &mut impl Effects,
    source: i64,
    scope: &str,
    observation: &Observation,
    payload: &str,
) -> Result<&'static str, StorageError> {
    Ok(match effects.first(source, scope, &observation.identity)?.as_deref() {
        None => {
            effects.identity(source, scope, &observation.identity, observation.record.offset)?;
            "accepted"
        }
        Some(first) if first == payload => "duplicate",
        Some(_) => "conflict",
    })
}

pub(crate) fn observations(effects: &mut impl Effects, source: i64, index: &FileIndex) -> Result<(), StorageError> {
    let mut models = BTreeSet::new();
    let mut days = BTreeSet::new();
    for observation in index.native_usage_observations.as_deref().unwrap_or_default() {
        let payload = encode(&observation.payload)?;
        let record = encode(&observation.record)?;
        if let Some(replay) = effects.fact(source, observation.record.offset)? {
            if replay != (payload, record) {
                return Err(StorageError::Conflict);
            }
            continue;
        }
        let tree = outcome(effects, source, "tree", observation, &payload)?;
        let model = if observation.payload.model.is_some() {
            outcome(effects, source, "model", observation, &payload)?
        } else {
            "unavailable"
        };
        effects.insert(Fact { source, offset: observation.record.offset, identity: &observation.identity,
            payload: &payload, record: &record, tree, model })?;
        if model == "accepted" && let Some(model) = &observation.payload.model {
            models.insert(model.clone());
            if let Some(time) = observation.payload.timestamp {
                days.insert(crate::pricing::utc_day(time));
            }
        }
    }
    if let Some(native) = &index.codex_native_usage {
        let cost = index.codex_cost();
        for model in models {
            if let Some(tokens) = native.by_model.get(&model) {
                effects.dimension(source, "model", &model, &encode(tokens)?)?;
            }
            if let Some(value) = cost.models.get(&model) {
                effects.dimension(source, "cost_model", &model, &encode(value)?)?;
            }
        }
        for day in days {
            if let Some(value) = cost.by_day.get(&day) {
                effects.dimension(source, "cost_day", &day, &encode(value)?)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn write(effects: &mut impl Effects, path: &str, ledger: &Ledger, index: &FileIndex) -> Result<(), StorageError> {
    if index.native_usage_observations.is_none() {
        return Ok(());
    }
    let source = effects.current(path)?.ok_or(StorageError::Conflict)?.0;
    observations(effects, source, index)?;
    effects.ready(source, ledger.offset)
}
