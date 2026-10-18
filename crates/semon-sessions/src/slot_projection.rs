//! Rebuildable, content-free native slot recipes. They are observation metadata,
//! never cached runtime authority or message/tool bodies.
use crate::model::{Background, BgEnd, Shown, SignalData, Slot, SlotFile, SlotKind, Transcript};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub(crate) const VERSION: u32 = 5;

/// Exact transcript observations share the Catalog's source/runtime fence.
/// Recipes retain no live authority; a reader may apply these fields only
/// after qualifying that publication and rechecking it after source reads.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryObservation {
    pub(crate) calls: usize,
    pub(crate) errors: usize,
    live: Vec<usize>,
    pub(crate) backgrounds: Vec<(usize, String)>,
}

impl QueryObservation {
    pub(crate) fn valid(&self, total: usize) -> bool {
        self.live.iter().all(|index| *index < total)
            && self.live.windows(2).all(|pair| pair[0] < pair[1])
            && self
                .backgrounds
                .iter()
                .all(|(index, id)| *index < total && !id.is_empty())
            && self
                .backgrounds
                .windows(2)
                .all(|pair| pair[0].0 < pair[1].0)
    }

    pub(crate) fn capture(transcript: &Transcript) -> Self {
        let mut live = Vec::new();
        let mut backgrounds = Vec::new();
        for (index, slot) in transcript.slots.iter().enumerate() {
            if matches!(
                slot.kind,
                SlotKind::Tool {
                    shown: Shown::Live,
                    ..
                } | SlotKind::Yielded {
                    shown: Shown::Live,
                    ..
                } | SlotKind::Sent {
                    shown: Shown::Live,
                    ..
                }
            ) {
                live.push(index);
            }
            if let SlotKind::Tool {
                bg: Some(background),
                ..
            } = &slot.kind
                && background.live
                && background.end.is_none()
            {
                backgrounds.push((index, background.tid.clone()));
            }
        }
        Self {
            calls: transcript.calls,
            errors: transcript.errors,
            live,
            backgrounds,
        }
    }

    pub(crate) fn apply(&self, saved: &SavedSlot, slot: &mut Slot, index: usize) {
        if matches!(
            saved.recipe,
            Recipe::Tool {
                outcome: Outcome::Pending,
                ..
            } | Recipe::Yielded {
                outcome: Outcome::Pending,
                ..
            } | Recipe::Sent {
                outcome: Outcome::Pending,
                ..
            }
        ) {
            let observed = if self.live.binary_search(&index).is_ok() {
                Shown::Live
            } else {
                Shown::Unfinished
            };
            match &mut slot.kind {
                SlotKind::Tool { shown, .. }
                | SlotKind::Yielded { shown, .. }
                | SlotKind::Sent { shown, .. } => *shown = observed,
                _ => {}
            }
        }
        if let SlotKind::Tool {
            bg: Some(background),
            ..
        } = &mut slot.kind
        {
            background.live = self
                .backgrounds
                .binary_search_by_key(&index, |(slot, _)| *slot)
                .is_ok();
        }
    }
}

type SlotKey = (Option<PathBuf>, u64, u32, String);

fn slot_key(file: Option<&File>, offset: u64, block: u32, recipe: &Recipe) -> Option<SlotKey> {
    Some((
        file.map(|file| file.path.clone()),
        offset,
        block,
        serde_json::to_string(recipe).ok()?,
    ))
}

/// Keep the existing projection digest shared by publication and optional reuse.
pub(crate) fn generation(
    metadata: &[String],
    sources: &[crate::model::summary::CatalogSource],
) -> serde_json::Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}|{}|{}",
                VERSION,
                serde_json::to_string(metadata)?,
                serde_json::to_string(sources)?
            )
            .as_bytes()
        )
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct File {
    pub(crate) path: PathBuf,
    pub(crate) cwd: Option<String>,
}
impl File {
    fn capture(file: &SlotFile) -> Self {
        Self {
            path: file.path.clone(),
            cwd: file.cwd.clone(),
        }
    }
    fn restore(&self) -> Arc<SlotFile> {
        Arc::new(SlotFile {
            path: self.path.clone(),
            cwd: self.cwd.clone(),
            prompts: Arc::new(crate::attachments::PromptCache::default()),
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum Outcome {
    Ok,
    Err,
    Unknown,
    /// The native call has no final result. This grants no live authority.
    Pending,
}
impl Outcome {
    fn capture(shown: Shown) -> Self {
        match shown {
            Shown::Ok => Self::Ok,
            Shown::Err => Self::Err,
            Shown::Unknown => Self::Unknown,
            Shown::Live | Shown::Unfinished => Self::Pending,
        }
    }
    fn restore(self) -> Shown {
        match self {
            Self::Ok => Shown::Ok,
            Self::Err => Shown::Err,
            Self::Unknown | Self::Pending => Shown::Unknown,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct End {
    file: File,
    offset: u64,
    t: Option<i64>,
    status: String,
    failed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackgroundRecipe {
    tid: String,
    end: Option<End>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "recipe", deny_unknown_fields)]
pub(crate) enum Recipe {
    H {
        id: String,
    },
    U,
    A,
    Think,
    Tool {
        outcome: Outcome,
        name: String,
        reply: Option<crate::events::Reply>,
        item: Option<u64>,
        background: Option<BackgroundRecipe>,
    },
    BgEnd {
        call: String,
        status: String,
        source_file: File,
        source_offset: u64,
        source_block: u32,
        start: Option<i64>,
    },
    Operation {
        kind: String,
        ok: Option<bool>,
        script_offset: Option<u64>,
    },
    Yielded {
        outcome: Outcome,
        first: Option<u64>,
        polls: Vec<u64>,
        cut: bool,
        done: Option<crate::events::Reply>,
    },
    Sent {
        outcome: Outcome,
        reply: Option<crate::events::Reply>,
        start: u64,
        done: Option<u64>,
    },
    Gap,
    Harness {
        label: String,
    },
    Signal {
        signal: SignalData,
    },
    Returned {
        to: String,
        at: Option<i64>,
        failed: bool,
    },
    NoActivity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedSlot {
    pub(crate) recipe: Recipe,
    pub(crate) file: Option<File>,
    pub(crate) offset: u64,
    pub(crate) block: u32,
    pub(crate) t: Option<i64>,
    pub(crate) turn: Option<String>,
    pub(crate) first: bool,
    #[serde(default)]
    pub(crate) field: Option<crate::native_field::NativeField>,
    #[serde(default)]
    pub(crate) native_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) images: Vec<serde_json::Value>,
}
impl SavedSlot {
    /// Source paths referenced by this recipe, including background edges.
    pub(crate) fn source_paths(&self) -> Vec<&std::path::PathBuf> {
        let mut paths: Vec<_> = self.file.iter().map(|file| &file.path).collect();
        match &self.recipe {
            Recipe::Tool {
                background: Some(background),
                ..
            } => {
                if let Some(end) = &background.end {
                    paths.push(&end.file.path);
                }
            }
            Recipe::BgEnd { source_file, .. } => paths.push(&source_file.path),
            _ => {}
        }
        paths
    }
    pub(crate) fn identity(&self) -> String {
        let event = self
            .native_event_id
            .clone()
            .unwrap_or_else(|| format!("offset:{}", self.offset));
        let header = match &self.recipe {
            Recipe::H { id } => Some(id),
            _ => None,
        };
        let identity = format!(
            "{:?}|{:?}|{}|{}|{:?}|{:?}",
            self.file.as_ref().map(|file| &file.path),
            header,
            event,
            self.block,
            std::mem::discriminant(&self.recipe),
            self.turn
        );
        // A turn-context record expands into several independent signals at
        // the same file/offset/block. Retention must preserve each kind.
        match &self.recipe {
            Recipe::Signal { signal } => format!("{identity}|{:?}", signal.kind),
            _ => identity,
        }
    }
    fn capture(
        slot: &Slot,
        sources: &BTreeMap<PathBuf, &crate::model::summary::CatalogSource>,
        records: Option<&crate::query_source::Records<'_>>,
        history: Option<&crate::overview_repair::NativeRecords<'_>>,
        previous: &BTreeMap<SlotKey, Self>,
    ) -> Self {
        let recipe = match &slot.kind {
            SlotKind::H(id) => Recipe::H { id: id.clone() },
            SlotKind::U => Recipe::U,
            SlotKind::A => Recipe::A,
            SlotKind::Think => Recipe::Think,
            SlotKind::Tool {
                shown,
                name,
                reply,
                item,
                bg,
            } => Recipe::Tool {
                outcome: Outcome::capture(*shown),
                name: name.clone(),
                reply: reply.clone(),
                item: *item,
                background: bg.as_ref().map(|background| BackgroundRecipe {
                    tid: background.tid.clone(),
                    end: background.end.as_ref().map(|end| End {
                        file: File::capture(&end.file),
                        offset: end.offset,
                        t: end.t,
                        status: end.status.clone(),
                        failed: end.failed,
                    }),
                }),
            },
            SlotKind::BgEnd {
                call,
                status,
                source,
                start,
            } => Recipe::BgEnd {
                call: call.clone(),
                status: status.clone(),
                source_file: File::capture(&source.0),
                source_offset: source.1,
                source_block: source.2,
                start: *start,
            },
            SlotKind::Operation {
                kind,
                ok,
                script_offset,
            } => Recipe::Operation {
                kind: kind.clone(),
                ok: *ok,
                script_offset: *script_offset,
            },
            SlotKind::Yielded {
                shown,
                first,
                polls,
                cut,
                done,
            } => Recipe::Yielded {
                outcome: Outcome::capture(*shown),
                first: *first,
                polls: polls.clone(),
                cut: *cut,
                done: done.clone(),
            },
            SlotKind::Sent {
                shown,
                reply,
                start,
                done,
            } => Recipe::Sent {
                outcome: Outcome::capture(*shown),
                reply: reply.clone(),
                start: *start,
                done: *done,
            },
            SlotKind::Gap => Recipe::Gap,
            SlotKind::Harness(label) => Recipe::Harness {
                label: label.clone(),
            },
            SlotKind::Signal(signal) => Recipe::Signal {
                signal: signal.clone(),
            },
            SlotKind::Returned { to, at, failed } => Recipe::Returned {
                to: to.clone(),
                at: *at,
                failed: *failed,
            },
            SlotKind::NoActivity => Recipe::NoActivity,
        };
        let file = slot.file.as_deref().map(File::capture);
        if let Some(previous) = slot_key(file.as_ref(), slot.offset, slot.block, &recipe)
            .and_then(|key| previous.get(&key))
        {
            return Self {
                images: previous.images.clone(),
                native_event_id: previous.native_event_id.clone(),
                field: previous.field.clone(),
                recipe,
                file,
                offset: slot.offset,
                block: slot.block,
                t: slot.t,
                turn: slot.turn.clone(),
                first: slot.first,
            };
        }
        let mut native_record = None;
        let field = slot.file.as_ref().and_then(|file| {
            if let Some(history) = history
                && history.index(&file.path).is_some()
            {
                return history.field(&file.path, slot.offset, slot.block);
            }
            let (field, record) =
                crate::native_field::capture_with_record(slot.offset, slot.block, &recipe, |at| {
                    if let Some(history) = history {
                        history.record_bytes(&file.path, at)
                    } else {
                        crate::native_field::read_source_record(
                            sources.get(&file.path)?,
                            at,
                            records,
                        )
                    }
                });
            // A tool field can point at its reply. Native entry identity and
            // attachments belong to the original slot record instead.
            native_record = record
                .filter(|(at, _)| *at == slot.offset)
                .map(|(_, record)| record);
            field
        });
        let native_event_id = field
            .as_ref()
            .filter(|field| matches!(field.kind, crate::native_field::NativeFieldKind::Text))
            .and_then(|field| field.native_event_id.clone())
            .or_else(|| {
                let file = slot.file.as_ref()?;
                if native_record.is_none() {
                    let bytes = if let Some(history) = history {
                        history.record_bytes(&file.path, slot.offset)?
                    } else {
                        crate::native_field::read_source_record(
                            sources.get(&file.path)?,
                            slot.offset,
                            records,
                        )?
                    };
                    native_record = crate::tx::parse_native_record(&bytes);
                }
                let record = native_record.as_ref()?;
                record
                    .get("uuid")
                    .or_else(|| record.get("id"))
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty() && value.len() <= 4096)
                    .map(str::to_owned)
            });
        let mut images = if matches!(&slot.kind, SlotKind::U | SlotKind::H(_)) {
            slot.file
                .as_ref()
                .and_then(|file| file.prompts.refs(slot.offset))
                .unwrap_or_else(|| {
                    native_record
                        .as_ref()
                        .map(crate::attachments::refs)
                        .unwrap_or_default()
                })
        } else {
            Vec::new()
        };
        for image in &mut images {
            image["o"] = serde_json::json!(slot.offset);
        }
        Self {
            native_event_id,
            images,
            field,
            recipe,
            file: slot.file.as_deref().map(File::capture),
            offset: slot.offset,
            block: slot.block,
            t: slot.t,
            turn: slot.turn.clone(),
            first: slot.first,
        }
    }
    pub(crate) fn restore(&self) -> Slot {
        let kind = match &self.recipe {
            Recipe::H { id } => SlotKind::H(id.clone()),
            Recipe::U => SlotKind::U,
            Recipe::A => SlotKind::A,
            Recipe::Think => SlotKind::Think,
            Recipe::Tool {
                outcome,
                name,
                reply,
                item,
                background,
            } => SlotKind::Tool {
                shown: outcome.restore(),
                name: name.clone(),
                reply: reply.clone(),
                item: *item,
                bg: background.as_ref().map(|background| Background {
                    tid: background.tid.clone(),
                    live: false,
                    end: background.end.as_ref().map(|end| BgEnd {
                        file: end.file.restore(),
                        offset: end.offset,
                        t: end.t,
                        status: end.status.clone(),
                        failed: end.failed,
                    }),
                }),
            },
            Recipe::BgEnd {
                call,
                status,
                source_file,
                source_offset,
                source_block,
                start,
            } => SlotKind::BgEnd {
                call: call.clone(),
                status: status.clone(),
                source: (source_file.restore(), *source_offset, *source_block),
                start: *start,
            },
            Recipe::Operation {
                kind,
                ok,
                script_offset,
            } => SlotKind::Operation {
                kind: kind.clone(),
                ok: *ok,
                script_offset: *script_offset,
            },
            Recipe::Yielded {
                outcome,
                first,
                polls,
                cut,
                done,
            } => SlotKind::Yielded {
                shown: outcome.restore(),
                first: *first,
                polls: polls.clone(),
                cut: *cut,
                done: done.clone(),
            },
            Recipe::Sent {
                outcome,
                reply,
                start,
                done,
            } => SlotKind::Sent {
                shown: outcome.restore(),
                reply: reply.clone(),
                start: *start,
                done: *done,
            },
            Recipe::Gap => SlotKind::Gap,
            Recipe::Harness { label } => SlotKind::Harness(label.clone()),
            Recipe::Signal { signal } => SlotKind::Signal(signal.clone()),
            Recipe::Returned { to, at, failed } => SlotKind::Returned {
                to: to.clone(),
                at: *at,
                failed: *failed,
            },
            Recipe::NoActivity => SlotKind::NoActivity,
        };
        Slot {
            kind,
            file: self.file.as_ref().map(File::restore),
            offset: self.offset,
            block: self.block,
            t: self.t,
            turn: self.turn.clone(),
            first: self.first,
        }
    }
}

pub(crate) struct Projection {
    pub(crate) key: String,
    pub(crate) slots: Vec<SavedSlot>,
    /// A failed optional reuse is rebuilt from evidence, including when a
    /// damaged stored row left the advertised projection digest unchanged.
    pub(crate) repair: bool,
}
pub(crate) fn capture(
    transcripts: &BTreeMap<String, Arc<Transcript>>,
    rows: &[crate::model::summary::CatalogRow],
    records: Option<&crate::query_source::Records<'_>>,
    history: Option<&crate::overview_repair::NativeRecords<'_>>,
    reuse: Option<(&crate::events::EventCache, Option<&str>)>,
) -> Vec<Projection> {
    let rows_by_key: BTreeMap<_, _> = rows.iter().map(|row| (row.key.as_str(), row)).collect();
    let sources: BTreeMap<_, _> = rows
        .iter()
        .flat_map(|row| row.sources.iter())
        .map(|source| (source.path.clone(), source))
        .collect();
    transcripts
        .iter()
        .map(|(key, transcript)| {
            let previous = reuse.and_then(|(cache, generation)| {
                cache.session_slots(
                    rows_by_key.get(key.as_str()).copied()?,
                    generation,
                    transcript.slots.len(),
                )
            });
            let repair = reuse.is_some() && previous.is_none();
            let previous = previous
                .unwrap_or_default()
                .into_iter()
                .filter_map(|slot| {
                    Some((
                        slot_key(slot.file.as_ref(), slot.offset, slot.block, &slot.recipe)?,
                        slot,
                    ))
                })
                .collect();
            Projection {
                key: key.clone(),
                repair,
                slots: transcript
                    .slots
                    .iter()
                    .map(|slot| SavedSlot::capture(slot, &sources, records, history, &previous))
                    .collect(),
            }
        })
        .collect()
}

/// Complete observation published under the catalog ledger/generation CAS.
pub(crate) struct Publication<'a> {
    pub(crate) rows: &'a [crate::model::summary::CatalogRow],
    pub(crate) transcripts: Option<&'a [Projection]>,
}
