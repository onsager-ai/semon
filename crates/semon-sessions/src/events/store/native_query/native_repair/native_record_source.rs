//! Request/worker-local descriptor adapter for the shared native record phases.
//! Every method borrows the native store only for one bounded metadata action;
//! provider credentials and Records never become persistent/shared state.

use super::*;
use crate::events::native_harness::record::RecordSource;
use crate::events::native_producer::ToolStore;
use std::cell::RefCell;

#[derive(Debug)]
pub(super) enum Error {
    Deferred,
    Conflict,
    Io(io::Error),
}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        if matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) {
            Self::Deferred
        } else {
            Self::Io(error)
        }
    }
}

pub(super) struct DescriptorRecord<'a, 'r, 's> {
    pub(super) store: &'a RefCell<&'s mut NativeStore>,
    pub(super) job: &'a Job,
    pub(super) witness: &'a SourceWitness,
    pub(super) repair: &'a Repair,
    pub(super) records: &'a crate::query_source::Records<'r>,
    pub(super) cancelled: &'a dyn Fn() -> bool,
    pub(super) root: OriginalNode,
    pub(super) work: &'a RefCell<RepairWork>,
    pub(super) validated: RefCell<std::collections::BTreeMap<u64, String>>,
}
impl DescriptorRecord<'_, '_, '_> {
    fn remaining(&self, rows: usize, bytes: usize) -> Result<(usize, usize), Error> {
        let work = self.work.borrow();
        let left = (
            crate::SESSION_SOURCE_CHUNK_MAX.saturating_sub(work.native_bytes),
            ROWS.saturating_sub(12 + work.metadata_rows),
        );
        if (self.cancelled)()
            || left.1 < rows
            || work.metadata_bytes + bytes + 4 * STATE_BYTES > BYTES
        {
            return Err(Error::Deferred);
        }
        Ok(left)
    }
    fn account(&self, work: RepairWork) -> Result<(), Error> {
        let mut total = self.work.borrow_mut();
        total.range_requests += work.range_requests;
        total.native_bytes += work.native_bytes;
        total.metadata_rows += work.metadata_rows;
        total.metadata_bytes += work.metadata_bytes;
        total.cleanup_vm_steps += work.cleanup_vm_steps;
        if total.metadata_rows > ROWS
            || total.metadata_bytes > BYTES
            || total.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(Error::Io(io::Error::other(
                "native record work exceeded its reservation",
            )));
        }
        Ok(())
    }
    fn outcome(&self, outcome: Outcome) -> Result<(), Error> {
        match outcome {
            Outcome::Written => Ok(()),
            Outcome::Deferred => Err(Error::Deferred),
            Outcome::Conflict => Err(Error::Conflict),
        }
    }
    fn node_current(&self, node: &OriginalNode) -> Result<(), Error> {
        let expected = json(node).map_err(io::Error::other)?;
        if let Some(actual) = self.validated.borrow().get(&node.address) {
            return if actual == &expected {
                Ok(())
            } else {
                Err(Error::Io(io::Error::other(
                    "native record node changed source",
                )))
            };
        }
        self.remaining(1, STATE_BYTES)?;
        let store = self.store.borrow();
        let actual = store
            .original_node(self.job, self.witness, self.repair, node.address)?
            .ok_or_else(|| io::Error::other("native record node disappeared"))?;
        self.work.borrow_mut().metadata_rows += 1;
        let actual = json(&actual).map_err(io::Error::other)?;
        self.work.borrow_mut().metadata_bytes += actual.len();
        if actual != expected {
            return Err(Error::Io(io::Error::other(
                "native record node changed source",
            )));
        }
        self.validated.borrow_mut().insert(node.address, actual);
        Ok(())
    }
}
impl RecordSource for DescriptorRecord<'_, '_, '_> {
    type Error = Error;
    type Node = OriginalNode;
    type Text = OriginalText;
    fn root(&self) -> OriginalNode {
        self.root.clone()
    }
    fn member(
        &mut self,
        node: &OriginalNode,
        name: &'static str,
    ) -> Result<Option<OriginalNode>, Error> {
        self.node_current(node)?;
        if !matches!(node.value, OriginalValue::Object) {
            return Ok(None);
        }
        let (native, rows) = self.remaining(8, 4 * STATE_BYTES)?;
        if native == 0 {
            return Err(Error::Deferred);
        }
        let step = self.store.borrow_mut().original_member_step_with_budget(
            self.job,
            self.witness,
            self.repair,
            node.address,
            name,
            self.records,
            self.cancelled,
            native,
            rows,
        )?;
        self.account(step.work)?;
        self.outcome(step.outcome)?;
        if !step.complete {
            return Err(Error::Deferred);
        }
        Ok(step.node)
    }
    fn text(&mut self, node: &OriginalNode) -> Result<Option<OriginalText>, Error> {
        self.node_current(node)?;
        if !matches!(node.value, OriginalValue::String(_)) {
            return Ok(None);
        }
        let (native, rows) = self.remaining(12, 4 * STATE_BYTES)?;
        let step = self.store.borrow_mut().original_identity_step_with_budget(
            self.job,
            self.witness,
            self.repair,
            node.ordinal,
            self.records,
            self.cancelled,
            native,
            rows,
        )?;
        self.account(step.work)?;
        self.outcome(step.outcome)?;
        step.text.map(Some).ok_or(Error::Deferred)
    }
    fn string_present(&mut self, node: &OriginalNode) -> Result<bool, Error> {
        self.node_current(node)?;
        Ok(matches!(node.value, OriginalValue::String(_)))
    }
    fn string_nonempty(&mut self, node: &OriginalNode) -> Result<bool, Error> {
        self.node_current(node)?;
        if !matches!(node.value, OriginalValue::String(_)) {
            return Ok(false);
        }
        let store = self.store.borrow();
        self.remaining(1, 8)?;
        let bytes:i64=store.connection.query_row("SELECT decoded_bytes FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND ordinal=?3 AND digest IS NOT NULL",params![self.repair.original.as_slice(),seq_u64(self.repair.cursor.record_at)?,seq_u64(node.ordinal)?],|row|row.get(0)).map_err(io::Error::other)?;
        self.work.borrow_mut().metadata_rows += 1;
        self.work.borrow_mut().metadata_bytes += 8;
        if bytes < 0 {
            return Err(Error::Io(io::Error::other("native text length changed")));
        }
        Ok(bytes != 0)
    }
    fn text_nonempty(&mut self, text: &OriginalText) -> Result<bool, Error> {
        self.remaining(2, STATE_BYTES + 64)?;
        let representative = self.store.borrow().original_text_representative(
            self.job,
            self.witness,
            self.repair,
            text,
        )?;
        self.work.borrow_mut().metadata_rows += 2;
        self.work.borrow_mut().metadata_bytes +=
            json(&representative.field).map_err(io::Error::other)?.len() + 64;
        Ok(representative.receipt.decoded_bytes != 0)
    }
    fn text_literal(&mut self, text: &OriginalText, literal: &'static str) -> Result<bool, Error> {
        let (native, rows) = self.remaining(7, 4 * STATE_BYTES)?;
        let step = self
            .store
            .borrow_mut()
            .original_text_literal_step_with_budget(
                self.job,
                self.witness,
                self.repair,
                text,
                literal,
                self.records,
                self.cancelled,
                native,
                rows,
            )?;
        self.account(step.work)?;
        self.outcome(step.outcome)?;
        step.matches.ok_or(Error::Deferred)
    }
    fn literal(&mut self, node: &OriginalNode, literal: &'static str) -> Result<bool, Error> {
        self.node_current(node)?;
        if !matches!(node.value, OriginalValue::String(_)) {
            return Ok(false);
        }
        let (native, rows) = self.remaining(5, 4 * STATE_BYTES)?;
        let step = self.store.borrow_mut().original_literal_step_with_budget(
            self.job,
            self.witness,
            self.repair,
            node.ordinal,
            literal,
            self.records,
            self.cancelled,
            native,
            rows,
        )?;
        self.account(step.work)?;
        self.outcome(step.outcome)?;
        step.matches.ok_or(Error::Deferred)
    }
    fn boolean(&mut self, node: &OriginalNode) -> Result<Option<bool>, Error> {
        self.node_current(node)?;
        Ok(match node.value {
            OriginalValue::Literal {
                kind: Literal::True,
                ..
            } => Some(true),
            OriginalValue::Literal {
                kind: Literal::False,
                ..
            } => Some(false),
            _ => None,
        })
    }
    fn integer(&mut self, node: &OriginalNode) -> Result<Option<i64>, Error> {
        self.node_current(node)?;
        if !matches!(node.value, OriginalValue::Number(_)) {
            return Ok(None);
        }
        self.remaining(1, STATE_BYTES)?;
        let (number, size) = self.store.borrow().original_number_with_size(
            self.job,
            self.witness,
            self.repair,
            node.address,
        )?;
        self.work.borrow_mut().metadata_rows += 1;
        self.work.borrow_mut().metadata_bytes += size;
        Ok(number.and_then(|number| number.number().as_i64()))
    }
    fn array_next(
        &mut self,
        node: &OriginalNode,
        after: Option<u64>,
    ) -> Result<Option<(u64, OriginalNode)>, Error> {
        self.node_current(node)?;
        if !matches!(node.value, OriginalValue::Array) {
            return Ok(None);
        }
        let store = self.store.borrow();
        self.remaining(2, STATE_BYTES + 16)?;
        check_record_view(&store.connection, self.job, self.witness, self.repair)
            .map_err(io::Error::other)?;
        let found:Option<(i64,i64)>=store.connection.query_row("SELECT element_index,address FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND parent_offset=?3 AND element_index>?4 ORDER BY element_index LIMIT 1",params![self.repair.original.as_slice(),seq_u64(self.repair.cursor.record_at)?,seq_u64(node.address)?,after.map(seq_u64).transpose()?.unwrap_or(-1)],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(io::Error::other)?;
        self.work.borrow_mut().metadata_rows += 1;
        self.work.borrow_mut().metadata_bytes += 16;
        found
            .map(|(index, address)| {
                let node = store
                    .original_node(
                        self.job,
                        self.witness,
                        self.repair,
                        u64::try_from(address).map_err(io::Error::other)?,
                    )?
                    .ok_or_else(|| io::Error::other("native array node disappeared"))?;
                self.work.borrow_mut().metadata_rows += 1;
                self.work.borrow_mut().metadata_bytes +=
                    json(&node).map_err(io::Error::other)?.len();
                Ok((u64::try_from(index).map_err(io::Error::other)?, node))
            })
            .transpose()
    }
}

/// Observation adapter: it borrows the connection only for one indexed native
/// read, so descriptor qualification never holds a SQL transaction over I/O.
pub(super) struct Observation<'a, 's> {
    pub(super) store: &'a RefCell<&'s mut NativeStore>,
    pub(super) original: [u8; 32],
    pub(super) work: &'a RefCell<RepairWork>,
}
impl Observation<'_, '_> {
    fn read<T>(
        &self,
        action: impl FnOnce(&native_events::Events<'_>) -> io::Result<T>,
    ) -> io::Result<T> {
        let before = self.work.borrow();
        if before.metadata_rows + 3 + 12 > ROWS
            || before.metadata_bytes + STATE_BYTES + 4 * STATE_BYTES > BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "native observation awaits next record step",
            ));
        }
        drop(before);
        let store = self.store.borrow();
        let events = native_events::Events::read_only(&store.connection, self.original);
        let result = action(&events);
        let work = events.work();
        let mut total = self.work.borrow_mut();
        total.metadata_rows += work.metadata_rows;
        total.metadata_bytes += work.metadata_bytes;
        result
    }
}
impl crate::events::native_producer::ToolStore for Observation<'_, '_> {
    type Error = io::Error;
    type Identity = u64;
    type Text = OriginalText;
    fn event_count(&self) -> io::Result<usize> {
        self.read(|s| s.event_count())
    }
    fn event(
        &self,
        seq: usize,
    ) -> io::Result<Option<crate::events::native_event::NativeEvent<OriginalText>>> {
        self.read(|s| s.event(seq))
    }
    fn pending(&self, id: &u64) -> io::Result<Option<usize>> {
        self.read(|s| s.pending(id))
    }
    fn latest_tool(&self, id: &u64, plain: bool, floor: usize) -> io::Result<Option<usize>> {
        self.read(|s| s.latest_tool(id, plain, floor))
    }
    fn recent_tool(&self, id: &u64, floor: usize) -> io::Result<Option<usize>> {
        self.read(|s| s.recent_tool(id, floor))
    }
    fn unique_open_code_mode(&self) -> io::Result<Option<usize>> {
        self.read(|s| s.unique_open_code_mode())
    }
    fn replace_event(
        &mut self,
        _: usize,
        _: crate::events::native_event::NativeEvent<OriginalText>,
    ) -> io::Result<()> {
        Err(io::Error::other("native observation cannot write"))
    }
    fn append_event(
        &mut self,
        _: crate::events::native_event::NativeEvent<OriginalText>,
    ) -> io::Result<usize> {
        Err(io::Error::other("native observation cannot write"))
    }
    fn take_pending(&mut self, _: &u64) -> io::Result<Option<usize>> {
        Err(io::Error::other("native observation cannot write"))
    }
    fn set_pending(&mut self, _: &u64, _: usize) -> io::Result<()> {
        Err(io::Error::other("native observation cannot write"))
    }
    fn add_tool_id(&mut self, _: &u64, _: usize) -> io::Result<()> {
        Err(io::Error::other("native observation cannot write"))
    }
}
impl crate::events::native_harness::CopilotStore for Observation<'_, '_> {
    fn tool_occurrence_after(&self, id: &u64, after: Option<usize>) -> io::Result<Option<usize>> {
        use crate::events::native_harness::CopilotStore;
        self.read(|s| s.tool_occurrence_after(id, after))
    }
}

impl crate::events::native_harness::CopilotIdentityStore for Observation<'_, '_> {
    fn copilot_started(&self) -> io::Result<bool> {
        use crate::events::native_harness::CopilotIdentityStore;
        self.read(|s| s.copilot_started())
    }
    fn copilot_record_seen(&mut self, _: &OriginalText) -> io::Result<bool> {
        Err(io::Error::other("native observation cannot write"))
    }
}
