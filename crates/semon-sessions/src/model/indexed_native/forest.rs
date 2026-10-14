//! Qualified source channels over immutable native candidate roots. This
//! adapter projects native keys; it never adds interleaved GroupSummaries.
use crate::{
    events::{
        Outcome,
        store::{
            native_query::{Job, NativeStore},
            native_rank::{
                CandidateBounds, CandidateDirection, CandidatePoint, CandidateReadOutcome,
                CandidateRoot, CandidateWork,
            },
        },
    },
    model::range_stage::{Facts, Order},
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Channel {
    Offset,
    BeforeFirst,
    Observed,
}

/// These references come from the atomically qualified native publication,
/// including the exact source/owner mapping and the origin's copy truth map.
/// They contain no native event, transcript recipe or source body vector.
pub(crate) struct Source<'a> {
    pub(crate) root: &'a CandidateRoot,
    pub(crate) channel: Channel,
    pub(crate) source: usize,
    pub(crate) file_order: usize,
    pub(crate) first_regular_time: Option<i64>,
    pub(crate) included: u64,
}

impl Source<'_> {
    fn prefix(&self, several: bool) -> Option<[u8; 16]> {
        let time = match self.channel {
            Channel::Offset => 0,
            Channel::BeforeFirst => self.first_regular_time.unwrap_or(i64::MIN),
            Channel::Observed => return None,
        };
        let mut prefix = [0; 16];
        prefix[..8].copy_from_slice(&((time as u64) ^ (1 << 63)).to_be_bytes());
        prefix[8..]
            .copy_from_slice(&(if several { self.file_order as u64 } else { 0 }).to_be_bytes());
        Some(prefix)
    }
    fn check(&self, several: bool) -> io::Result<()> {
        if several == matches!(self.channel, Channel::Offset) {
            return Err(io::Error::other(
                "native source channel does not match merge mode",
            ));
        }
        Ok(())
    }
    fn bound(&self, several: bool, global: &[u8; 69], lower: bool) -> Option<Option<[u8; 69]>> {
        if let Some(prefix) = self.prefix(several) {
            match prefix.as_slice().cmp(&global[..16]) {
                std::cmp::Ordering::Less => return if lower { None } else { Some(None) },
                std::cmp::Ordering::Greater => return if lower { Some(None) } else { None },
                std::cmp::Ordering::Equal => {}
            }
            let mut local = *global;
            // Static Offset/BeforeFirst channels always store local time0.
            local[..16].fill(0);
            local[0] = 128;
            return Some(Some(local));
        }
        let file = (self.file_order as u64).to_be_bytes();
        let mut local = *global;
        local[8..16].fill(0);
        match file.as_slice().cmp(&global[8..16]) {
            std::cmp::Ordering::Equal => {}
            std::cmp::Ordering::Less if lower => {
                // All equal-time local candidates precede the global lower
                // bound. A file1 sentinel skips timeT/file0 without scanning.
                local[15] = 1;
                local[16..].fill(0);
            }
            std::cmp::Ordering::Greater if !lower => {
                local[16..].fill(0);
                if !previous(&mut local) {
                    return None;
                }
            }
            _ => local[16..].fill(if lower { 0 } else { 255 }),
        }
        Some(Some(local))
    }
    pub(crate) fn project(
        &self,
        several: bool,
        global: &CandidateBounds,
    ) -> io::Result<Option<CandidateBounds>> {
        self.check(several)?;
        let lower = match global
            .lower
            .as_ref()
            .map(|key| self.bound(several, key, true))
        {
            Some(None) => return Ok(None),
            Some(Some(bound)) => bound,
            None => None,
        };
        let upper = match global
            .upper
            .as_ref()
            .map(|key| self.bound(several, key, false))
        {
            Some(None) => return Ok(None),
            Some(Some(bound)) => bound,
            None => None,
        };
        if lower.zip(upper).is_some_and(|(a, b)| a > b) {
            return Ok(None);
        }
        Ok(Some(CandidateBounds { lower, upper }))
    }
    fn qualify(&self, several: bool, facts: &mut Facts) {
        facts.qualify(
            self.source,
            self.file_order,
            several,
            match self.channel {
                Channel::BeforeFirst => Some(self.first_regular_time.unwrap_or(i64::MIN)),
                _ => None,
            },
        );
    }
    fn point(&self, several: bool, point: CandidatePoint) -> io::Result<Point> {
        let bytes = &point.key;
        if bytes[8..16] != [0; 8]
            || matches!(self.channel, Channel::Offset | Channel::BeforeFirst)
                && bytes[..8] != (1u64 << 63).to_be_bytes()
        {
            return Err(io::Error::other(
                "native source point has a foreign channel key",
            ));
        }
        let time = match self.channel {
            Channel::Offset => 0,
            Channel::BeforeFirst => self.first_regular_time.unwrap_or(i64::MIN),
            Channel::Observed => {
                (u64::from_be_bytes(bytes[..8].try_into().expect("fixed native key")) ^ (1 << 63))
                    as i64
            }
        };
        let order = Order {
            key: (
                time,
                if several { self.file_order } else { 0 },
                u64::from_be_bytes(bytes[16..24].try_into().expect("fixed native key")),
                u32::from_be_bytes(bytes[24..28].try_into().expect("fixed native key")),
                bytes[28],
            ),
            ordinal: u64::from_be_bytes(bytes[29..37].try_into().expect("fixed native key")),
            identity: point.leaf.candidate,
        };
        if bytes[37..] != order.identity {
            return Err(io::Error::other(
                "native point identity disagrees with tree key",
            ));
        }
        Ok(Point {
            source: self.source,
            channel: self.channel,
            order,
            point,
        })
    }
}

fn previous(key: &mut [u8; 69]) -> bool {
    for byte in key.iter_mut().rev() {
        if *byte > 0 {
            *byte -= 1;
            return true;
        }
        *byte = 255;
    }
    false
}

pub(crate) struct Point {
    pub(crate) source: usize,
    pub(crate) channel: Channel,
    pub(crate) order: Order,
    pub(crate) point: CandidatePoint,
}

#[derive(Clone, Copy)]
pub(crate) enum Weight {
    Ordinary,
    Entries,
    NonGap,
    Boundaries,
    Gaps,
}
impl Weight {
    fn count(self, text: &str) -> io::Result<u64> {
        let facts: Facts = serde_json::from_str(text).map_err(io::Error::other)?;
        Ok(match self {
            Self::Ordinary => facts.count,
            Self::Entries => facts.entries,
            Self::Boundaries => facts.boundaries,
            Self::Gaps => facts.gaps,
            Self::NonGap => facts
                .entries
                .checked_sub(facts.gaps)
                .ok_or_else(|| io::Error::other("native gap weight exceeds entries"))?,
        })
    }
}

pub(crate) struct Read<T> {
    pub(crate) outcome: CandidateReadOutcome,
    pub(crate) value: Option<T>,
    pub(crate) work: CandidateWork,
}

fn combine(left: &str, right: &str) -> io::Result<String> {
    let mut left: Facts = serde_json::from_str(left).map_err(io::Error::other)?;
    let right: Facts = serde_json::from_str(right).map_err(io::Error::other)?;
    left.merge(right).map_err(io::Error::other)?;
    serde_json::to_string(&left).map_err(io::Error::other)
}

fn add(total: &mut CandidateWork, part: &CandidateWork) {
    total.sql_reads += part.sql_reads;
    total.sql_writes += part.sql_writes;
    total.metadata_bytes += part.metadata_bytes;
    total.vm_steps += part.vm_steps;
    total.sqlite_vm_steps += part.sqlite_vm_steps;
    total.apply_visits += part.apply_visits;
    total.nodes_written += part.nodes_written;
    total.max_depth = total.max_depth.max(part.max_depth);
    total.max_cursor_bytes = total.max_cursor_bytes.max(part.max_cursor_bytes);
    total.max_apply_bytes = total.max_apply_bytes.max(part.max_apply_bytes);
    total.max_live_node_bytes = total.max_live_node_bytes.max(part.max_live_node_bytes);
}

pub(crate) struct Forest<'a> {
    pub(crate) store: &'a NativeStore,
    pub(crate) job: &'a Job,
    pub(crate) sources: &'a [Source<'a>],
    pub(crate) several: bool,
    pub(crate) closure: &'a dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    pub(crate) cancelled: &'a dyn Fn() -> bool,
}

impl Forest<'_> {
    fn finish<T>(&self, value: T, mut work: CandidateWork) -> io::Result<Read<T>> {
        let (outcome, steps) =
            self.store
                .recheck_closure(self.job, self.closure, self.cancelled)?;
        work.sqlite_vm_steps += steps;
        let outcome = match outcome {
            Outcome::Written => CandidateReadOutcome::Qualified,
            Outcome::Deferred => CandidateReadOutcome::Deferred,
            _ => CandidateReadOutcome::Conflict,
        };
        Ok(Read {
            value: matches!(outcome, CandidateReadOutcome::Qualified).then_some(value),
            outcome,
            work,
        })
    }
    pub(crate) fn range(&self, bounds: &CandidateBounds) -> io::Result<Read<Facts>> {
        let mut facts = Facts::default();
        let mut work = CandidateWork::default();
        for source in self.sources {
            let Some(projected) = source.project(self.several, bounds)? else {
                continue;
            };
            let read = self.store.candidate_tree_range(
                self.job,
                source.root,
                &projected,
                source.included,
                &combine,
                self.closure,
                self.cancelled,
            )?;
            add(&mut work, &read.work);
            if !matches!(read.outcome, CandidateReadOutcome::Qualified) {
                return Ok(Read {
                    outcome: read.outcome,
                    value: None,
                    work,
                });
            }
            let text = read
                .value
                .ok_or_else(|| io::Error::other("qualified native range lacks facts"))?;
            let mut local: Facts = serde_json::from_str(&text).map_err(io::Error::other)?;
            source.qualify(self.several, &mut local);
            facts.merge(local).map_err(io::Error::other)?;
        }
        self.finish(facts, work)
    }
    pub(crate) fn seek(
        &self,
        bound: &[u8; 69],
        direction: CandidateDirection,
        weight: Weight,
    ) -> io::Result<Read<Option<Point>>> {
        let mut selected: Option<Point> = None;
        let mut work = CandidateWork::default();
        let forward = matches!(direction, CandidateDirection::Forward);
        let bounds = CandidateBounds {
            lower: forward.then_some(*bound),
            upper: (!forward).then_some(*bound),
        };
        for source in self.sources {
            let Some(projected) = source.project(self.several, &bounds)? else {
                continue;
            };
            let local = if forward {
                projected.lower.unwrap_or([0; 69])
            } else {
                projected.upper.unwrap_or([255; 69])
            };
            let read = self.store.candidate_tree_seek(
                self.job,
                source.root,
                &local,
                direction,
                source.included,
                &|text| weight.count(text),
                self.closure,
                self.cancelled,
            )?;
            add(&mut work, &read.work);
            if !matches!(read.outcome, CandidateReadOutcome::Qualified) {
                return Ok(Read {
                    outcome: read.outcome,
                    value: None,
                    work,
                });
            }
            if let Some(point) = read.value {
                let candidate = source.point(self.several, point)?;
                if selected.as_ref().is_none_or(|old| {
                    if forward {
                        candidate.order < old.order
                    } else {
                        candidate.order > old.order
                    }
                }) {
                    selected = Some(candidate);
                }
            }
        }
        self.finish(selected, work)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::store::native_rank::{CandidateLeaf, NodeId};

    #[test]
    fn native_forest_projects_inclusive_bounds_without_rewriting_symbolic_keys() {
        // This pure projection oracle intentionally has no admitted authority
        // or SQLite read. Real roots/closures are exercised by storage tests.
        let empty: NodeId =
            serde_json::from_value(serde_json::json!({"level":64,"digest":"0".repeat(64)}))
                .unwrap();
        let root = CandidateRoot::empty(empty.clone());
        for several in [false, true] {
            for channel in [Channel::Offset, Channel::BeforeFirst, Channel::Observed] {
                if several == matches!(channel, Channel::Offset) {
                    continue;
                }
                for file_order in [0, 1, 2, usize::MAX] {
                    for first_regular_time in [None, Some(-1), Some(1)] {
                        let source = Source {
                            root: &root,
                            channel,
                            source: 17,
                            file_order,
                            first_regular_time,
                            included: 0,
                        };
                        let mut points = Vec::new();
                        for time in [i64::MIN, -1, 0, 1, i64::MAX] {
                            if !matches!(channel, Channel::Observed) && time != 0 {
                                continue;
                            }
                            for offset in [0, 1, u64::MAX] {
                                for phase in [0, 1, 2] {
                                    let order = Order {
                                        key: (time, 0, offset, 0, phase),
                                        ordinal: 1,
                                        identity: [7; 32],
                                    };
                                    let point = CandidatePoint {
                                        key: order.encoded(),
                                        leaf: CandidateLeaf {
                                            candidate: order.identity,
                                            aggregate: empty.clone(),
                                        },
                                    };
                                    let qualified = source.point(several, point).unwrap();
                                    points.push((order.encoded(), qualified.order.encoded()));
                                }
                            }
                        }
                        for time in [i64::MIN, -1, 0, 1, i64::MAX] {
                            for file in [0, 1, 2, usize::MAX] {
                                for offset in [0, 1, u64::MAX] {
                                    let bound = Order {
                                        key: (time, file, offset, 0, 1),
                                        ordinal: 1,
                                        identity: [7; 32],
                                    }
                                    .encoded();
                                    for bounds in [
                                        CandidateBounds {
                                            lower: Some(bound),
                                            upper: None,
                                        },
                                        CandidateBounds {
                                            lower: None,
                                            upper: Some(bound),
                                        },
                                        CandidateBounds {
                                            lower: Some(bound),
                                            upper: Some(bound),
                                        },
                                    ] {
                                        let projected = source.project(several, &bounds).unwrap();
                                        for (local, global) in &points {
                                            let expected =
                                                bounds.lower.is_none_or(|lower| *global >= lower)
                                                    && bounds
                                                        .upper
                                                        .is_none_or(|upper| *global <= upper);
                                            let actual = projected.as_ref().is_some_and(|bounds| {
                                                bounds.lower.is_none_or(|lower| *local >= lower)
                                                    && bounds
                                                        .upper
                                                        .is_none_or(|upper| *local <= upper)
                                            });
                                            assert_eq!(
                                                actual, expected,
                                                "channel={channel:?} time={time} file={file} offset={offset} seed={first_regular_time:?} order={file_order}"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
