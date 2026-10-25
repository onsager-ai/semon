//! Pure native child-row selection shared by physical storage writers.
//!
//! Values borrow the shared parser's index. No connection, SQL, serialization,
//! source reads or operation identity is created here. The caller must apply
//! every family together with its ledger, proofs and receipt in one transaction.

use std::collections::BTreeSet;

use super::{CodexUsageEvent, DirtyRows, Event, FileIndex, Signal};

pub(crate) struct NativeRowPlan<'a> {
    pub(crate) events: RowChanges<'a, Event>,
    pub(crate) extras: RowChanges<'a, Event>,
    pub(crate) signals: RowChanges<'a, Signal>,
    pub(crate) codex_usage: RowChanges<'a, CodexUsageEvent>,
}

impl<'a> NativeRowPlan<'a> {
    pub(crate) fn new(index: &'a FileIndex, changes: &'a DirtyRows) -> Self {
        Self {
            events: RowChanges::new(changes.before_events, &changes.events, &index.events),
            extras: RowChanges::new(changes.before_extras, &changes.extras, &index.extras),
            signals: RowChanges::new(changes.before_signals, &changes.signals, &index.signals),
            codex_usage: RowChanges::new(
                changes.before_codex_usage,
                &changes.codex_usage,
                &index.codex_usage_events,
            ),
        }
    }
}

pub(crate) struct RowChanges<'a, T> {
    before: usize,
    touched: &'a BTreeSet<usize>,
    rows: &'a [T],
}

impl<'a, T> RowChanges<'a, T> {
    fn new(before: usize, touched: &'a BTreeSet<usize>, rows: &'a [T]) -> Self {
        Self { before, touched, rows }
    }

    /// Original ascending, deduplicated write order: changed prefix, then tail.
    /// An out-of-range dirty ordinal does not manufacture a row.
    pub(crate) fn upserts(&self) -> impl Iterator<Item = (usize, &'a T)> + '_ {
        self.touched
            .range(..self.before)
            .copied()
            .chain(self.before..self.rows.len())
            .filter_map(|seq| self.rows.get(seq).map(|row| (seq, row)))
    }

    /// The append parser never cuts rows; synthetic truncation deltas can.
    pub(crate) fn delete_from(&self) -> Option<usize> {
        (self.rows.len() < self.before).then_some(self.rows.len())
    }

    pub(crate) fn apply<E>(
        &self,
        mut put: impl FnMut(usize, &'a T) -> Result<(), E>,
        cut: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<(), E> {
        for (seq, row) in self.upserts() {
            put(seq, row)?;
        }
        if let Some(from) = self.delete_from() {
            cut(from)?;
        }
        Ok(())
    }
}
