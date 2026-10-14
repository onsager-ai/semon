//! Durable immutable point-key candidate trees. The native producer owns
//! source/time qualification and the graph's Facts algebra; this module owns
//! only fixed-width placement keys, candidate addresses, and graph roots.
use super::{
    Apply, Frame, Graph, NodeId, Pair, Phase, ROW_PREFIX, STEP_BYTES, STEP_ROWS,
    native_query::{self, Job, NativeStore},
};
use crate::events::Outcome;
use crate::model::range_stage::CandidateRef;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) const KEY_BYTES: usize = 69;
const KEY_BITS: usize = KEY_BYTES * 8;
const MAX_DEPTH: usize = KEY_BITS;
const NODE_BYTES: usize = 64 * 1024;
const CURSOR_BYTES: usize = 64 * 1024;
const PATH_FRAME_BYTES: usize = 68;
const EMPTY_TREE: &str = "rank/v1/tree/empty";

#[cfg(test)]
use crate::test_allocations::{
    measure as measure_test_allocations, peak_bytes as measured_peak_bytes,
};

/// Registered by the private native producer schema owner in its versioned
/// migration. Keeping it separate prevents an accidental public Trace change.
pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_rank_candidate_nodes (
 identity TEXT PRIMARY KEY, metadata TEXT NOT NULL, left_id TEXT, right_id TEXT,
 aggregate TEXT NOT NULL, refs INTEGER NOT NULL CHECK(refs>=0),
 CHECK((left_id IS NULL AND right_id IS NULL) OR (left_id IS NOT NULL AND right_id IS NOT NULL))
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_rank_candidate_unused
 ON native_rank_candidate_nodes(identity) WHERE refs=0;
CREATE TABLE IF NOT EXISTS native_rank_candidate_bindings (
 job INTEGER NOT NULL, identity TEXT NOT NULL, PRIMARY KEY(job,identity)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_rank_candidate_roots (
 job INTEGER NOT NULL, identity TEXT NOT NULL, empty TEXT NOT NULL,
 PRIMARY KEY(job,identity,empty)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_rank_candidate_cursors (
 job INTEGER NOT NULL, cursor TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(job,cursor)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_rank_candidate_applies (
 job INTEGER NOT NULL, cursor TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(job,cursor)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_rank_candidate_cursor_job
 ON native_rank_candidate_cursors(job,cursor);
";

/// A candidate-tree root is qualified by its caller-owned empty graph root.
/// Tree nodes never encode native source, time, alias, turn, or copy semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateRoot {
    tree: Option<CandidateNodeId>,
    empty: NodeId,
}

impl CandidateRoot {
    pub(crate) fn empty(empty: NodeId) -> Self {
        Self { tree: None, empty }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.tree.is_none()
    }
    pub(crate) fn empty_graph(&self) -> NodeId {
        self.empty.clone()
    }
}

/// The point payload contains no candidate body or model-derived semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateLeaf {
    pub(crate) candidate: CandidateRef,
    pub(crate) aggregate: NodeId,
    /// Opaque, content-free native point recipe. Immutable tree ownership keeps
    /// it alive across root imports without copying a historical payload Vec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) payload: Option<String>,
}

/// Ordered inclusive native placement bounds. The caller builds these from
/// `range_stage::Order::encoded()` and owns their source/time interpretation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CandidateBounds {
    pub(crate) lower: Option<[u8; KEY_BYTES]>,
    pub(crate) upper: Option<[u8; KEY_BYTES]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateDirection {
    Forward,
    Backward,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidatePoint {
    pub(crate) key: [u8; KEY_BYTES],
    pub(crate) leaf: CandidateLeaf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateReadOutcome {
    Qualified,
    Conflict,
    Deferred,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidateRead<T> {
    pub(crate) outcome: CandidateReadOutcome,
    pub(crate) value: Option<T>,
    pub(crate) work: CandidateWork,
}

#[derive(Debug)]
pub(crate) struct CandidateStep {
    pub(crate) outcome: Outcome,
    pub(crate) complete: Option<CandidateRoot>,
    pub(crate) work: CandidateWork,
}

/// Counts executed point statements and decoded/encoded bytes, plus the
/// existing graph Apply work that maintains native conditional aggregates.
#[derive(Clone, Debug, Default)]
pub(crate) struct CandidateWork {
    /// Explicit tree and graph statements, excluding native job/closure fences.
    pub(crate) sql_reads: usize,
    pub(crate) sql_writes: usize,
    /// Encoded/decoded bytes for the explicit tree and graph rows above.
    pub(crate) metadata_bytes: usize,
    /// StatementStatus totals for explicit tree and graph statements.
    pub(crate) vm_steps: i64,
    /// Actual SQLite progress-hook delta for the complete API operation,
    /// including job/source/runtime witnesses, closure SQL and revision CAS.
    /// External provider/proof I/O is outside this SQLite-only storage layer.
    pub(crate) sqlite_vm_steps: u64,
    pub(crate) apply_visits: usize,
    pub(crate) nodes_written: usize,
    pub(crate) max_depth: usize,
    pub(crate) max_cursor_bytes: usize,
    pub(crate) max_apply_bytes: usize,
    pub(crate) max_live_node_bytes: usize,
}

impl CandidateWork {
    fn rows(&self) -> usize {
        self.sql_reads + self.sql_writes
    }
    fn add_graph(&mut self, work: &super::Work) {
        self.sql_reads += work.sql_reads;
        self.sql_writes += work.sql_writes;
        self.metadata_bytes += work.metadata_bytes;
        self.vm_steps += work.vm_steps;
        self.apply_visits += work.apply_visits;
        self.nodes_written += work.nodes_written;
        self.max_depth = self.max_depth.max(work.max_stack);
        self.max_live_node_bytes = self.max_live_node_bytes.max(work.max_live_node_bytes);
    }
    fn read(&mut self, bytes: usize, vm_steps: i32) {
        self.sql_reads += 1;
        self.metadata_bytes += bytes;
        self.vm_steps += i64::from(vm_steps);
    }
    fn write(&mut self, bytes: usize, vm_steps: i32) {
        self.sql_writes += 1;
        self.metadata_bytes += bytes;
        self.vm_steps += i64::from(vm_steps);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateNodeId {
    digest: String,
}

impl CandidateNodeId {
    fn valid(&self) -> bool {
        self.digest.len() == 64 && self.digest.bytes().all(|b| b.is_ascii_hexdigit())
    }
    fn identity(&self) -> String {
        format!("{ROW_PREFIX}tree/{}", self.digest)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum CandidateNode {
    Leaf {
        key: Vec<u8>,
        candidate: CandidateRef,
        aggregate: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<String>,
    },
    Branch {
        split: u16,
        left: CandidateNodeId,
        right: CandidateNodeId,
        min: Vec<u8>,
        max: Vec<u8>,
        aggregate: NodeId,
    },
}

impl CandidateNode {
    fn min(&self) -> &[u8] {
        match self {
            Self::Leaf { key, .. } => key,
            Self::Branch { min, .. } => min,
        }
    }
    fn max(&self) -> &[u8] {
        match self {
            Self::Leaf { key, .. } => key,
            Self::Branch { max, .. } => max,
        }
    }
    fn aggregate(&self) -> &NodeId {
        match self {
            Self::Leaf { aggregate, .. } | Self::Branch { aggregate, .. } => aggregate,
        }
    }
    fn branch(&self) -> Option<(u16, &CandidateNodeId, &CandidateNodeId)> {
        match self {
            Self::Branch {
                split, left, right, ..
            } => Some((*split, left, right)),
            Self::Leaf { .. } => None,
        }
    }
    fn valid(&self) -> bool {
        match self {
            Self::Leaf {
                key,
                candidate,
                aggregate,
                ..
            } => {
                key.len() == KEY_BYTES
                    && candidate.as_slice() == &key[KEY_BYTES - 32..]
                    && aggregate.valid()
            }
            Self::Branch {
                split,
                left,
                right,
                min,
                max,
                aggregate,
            } => {
                usize::from(*split) < KEY_BITS
                    && left.valid()
                    && right.valid()
                    && left != right
                    && min.len() == KEY_BYTES
                    && max.len() == KEY_BYTES
                    && min < max
                    && key_bit(min, usize::from(*split)) == Some(false)
                    && key_bit(max, usize::from(*split)) == Some(true)
                    && aggregate.valid()
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathFrame {
    node: CandidateNodeId,
    split: u16,
    right: bool,
}

impl PathFrame {
    fn encode(&self, output: &mut String) {
        output.push_str(&self.node.digest);
        output.push_str(&format!("{:03x}", self.split));
        output.push(if self.right { '1' } else { '0' });
    }
    fn decode(path: &str, index: usize) -> io::Result<Self> {
        let start = index * PATH_FRAME_BYTES;
        let frame = path
            .as_bytes()
            .get(start..start + PATH_FRAME_BYTES)
            .ok_or_else(|| invalid("candidate tree cursor path is truncated"))?;
        let split_text = std::str::from_utf8(&frame[64..67])
            .map_err(|_| invalid("candidate tree cursor split is invalid"))?;
        let split = u16::from_str_radix(split_text, 16)
            .map_err(|_| invalid("candidate tree cursor split is invalid"))?;
        let right = match frame[67] {
            b'0' => false,
            b'1' => true,
            _ => return Err(invalid("candidate tree cursor direction is invalid")),
        };
        let digest = std::str::from_utf8(&frame[..64])
            .map_err(|_| invalid("candidate tree cursor address is invalid"))?;
        let node = CandidateNodeId {
            digest: digest.to_owned(),
        };
        if !node.valid() || usize::from(split) >= KEY_BITS {
            return Err(invalid("candidate tree cursor frame is invalid"));
        }
        Ok(Self { node, split, right })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingBranch {
    split: u16,
    left: CandidateNodeId,
    right: CandidateNodeId,
    consumes_parent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateState {
    version: String,
    root: CandidateRoot,
    key: Vec<u8>,
    replacement: Option<CandidateLeaf>,
    path: String,
    current: Option<CandidateNodeId>,
    base: Option<CandidateNodeId>,
    repair: usize,
    pending: Option<PendingBranch>,
    apply_active: bool,
    prepared: bool,
}

impl UpdateState {
    fn valid(&self) -> bool {
        !self.version.is_empty()
            && self.version.len() <= 256
            && self.key.len() == KEY_BYTES
            && self.root.empty.valid()
            && self.root.tree.as_ref().is_none_or(CandidateNodeId::valid)
            && self.path.len() % PATH_FRAME_BYTES == 0
            && self.path.len() / PATH_FRAME_BYTES <= MAX_DEPTH
            && self.current.as_ref().is_none_or(CandidateNodeId::valid)
            && self.base.as_ref().is_none_or(CandidateNodeId::valid)
            && self.repair <= self.path.len() / PATH_FRAME_BYTES
            && self.pending.as_ref().is_none_or(|pending| {
                usize::from(pending.split) < KEY_BITS
                    && pending.left.valid()
                    && pending.right.valid()
                    && pending.left != pending.right
            })
            && self.replacement.as_ref().is_none_or(|leaf| {
                leaf.aggregate.valid() && leaf.candidate.as_slice() == &self.key[37..]
            })
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn encoded<T: Serialize>(value: &T, maximum: usize) -> io::Result<String> {
    let text = serde_json::to_string(value).map_err(io::Error::other)?;
    if text.len() > maximum {
        return Err(invalid("candidate tree scalar metadata budget exceeded"));
    }
    Ok(text)
}
fn candidate_id(text: &str) -> CandidateNodeId {
    CandidateNodeId {
        digest: format!("{:x}", Sha256::digest(text.as_bytes())),
    }
}
fn cursor_name(cursor: &str) -> io::Result<&str> {
    if cursor.is_empty() || cursor.len() > 256 {
        return Err(invalid("invalid candidate tree cursor"));
    }
    Ok(cursor)
}
fn root_identity(root: &CandidateRoot) -> String {
    root.tree
        .as_ref()
        .map(CandidateNodeId::identity)
        .unwrap_or_else(|| EMPTY_TREE.to_owned())
}
fn key_bit(key: &[u8], bit: usize) -> Option<bool> {
    (key.len() == KEY_BYTES && bit < KEY_BITS).then(|| key[bit / 8] & (1 << (7 - bit % 8)) != 0)
}
fn first_difference(left: &[u8], right: &[u8]) -> Option<u16> {
    if left.len() != KEY_BYTES || right.len() != KEY_BYTES {
        return None;
    }
    left.iter()
        .zip(right)
        .enumerate()
        .find_map(|(byte, (a, b))| {
            let changed = a ^ b;
            (changed != 0).then(|| (byte * 8 + changed.leading_zeros() as usize) as u16)
        })
}
fn fixed_key(key: &[u8]) -> io::Result<[u8; KEY_BYTES]> {
    let key: &[u8; KEY_BYTES] = key
        .try_into()
        .map_err(|_| invalid("candidate tree point key has the wrong width"))?;
    Ok(*key)
}
fn root_valid(root: &CandidateRoot) -> bool {
    root.empty.valid() && root.tree.as_ref().is_none_or(CandidateNodeId::valid)
}

struct TreeDb<'a> {
    db: &'a Connection,
    job: i64,
    work: CandidateWork,
}

impl TreeDb<'_> {
    fn read_node(&mut self, id: &CandidateNodeId) -> io::Result<CandidateNode> {
        if !id.valid() {
            return Err(invalid("invalid candidate tree address"));
        }
        // Candidate identities are "rank/v1/tree/" + 64 hex bytes (77).
        // Graph identities add a one/two-digit level (79..=80). These guards
        // mirror the existing address format; no new semantic cap is imposed.
        // Keep nullable child absence separate from a rejected child address.
        let mut q = self
            .db
            .prepare_cached(
                "SELECT CASE WHEN octet_length(n.metadata)<=65536 THEN n.metadata END,
                 CASE WHEN octet_length(n.left_id)=77 THEN n.left_id END,
                 CASE WHEN octet_length(n.right_id)=77 THEN n.right_id END,
                 CASE WHEN octet_length(n.aggregate) BETWEEN 79 AND 80 THEN n.aggregate END,
                 (n.left_id IS NULL OR octet_length(n.left_id)=77)
                   AND (n.right_id IS NULL OR octet_length(n.right_id)=77)
                   AND CASE WHEN n.left_id IS NULL THEN n.right_id IS NULL ELSE n.right_id IS NOT NULL
                   AND EXISTS(SELECT 1 FROM native_rank_candidate_nodes a WHERE a.identity=n.left_id AND a.refs>0)
                   AND EXISTS(SELECT 1 FROM native_rank_candidate_nodes b WHERE b.identity=n.right_id AND b.refs>0)
                 END FROM native_rank_candidate_nodes n WHERE n.identity=?1",
            )
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let row: Option<(
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
        )> = q
            .query_row([id.identity()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let Some((Some(text), left, right, Some(aggregate), children_ok)) = row else {
            return Err(invalid("candidate tree node is absent or over budget"));
        };
        self.work.read(
            text.len()
                + id.identity().len()
                + aggregate.len()
                + left.as_ref().map_or(0, String::len)
                + right.as_ref().map_or(0, String::len),
            vm,
        );
        let node: CandidateNode = serde_json::from_str(&text).map_err(io::Error::other)?;
        if !node.valid()
            || candidate_id(&text) != *id
            || node.aggregate().identity() != aggregate
            || !children_ok
        {
            return Err(invalid(
                "candidate tree node identity or references changed",
            ));
        }
        let (want_left, want_right) = match &node {
            CandidateNode::Leaf { .. } => (None, None),
            CandidateNode::Branch { left, right, .. } => {
                (Some(left.identity()), Some(right.identity()))
            }
        };
        if left != want_left || right != want_right {
            return Err(invalid("candidate tree child columns changed"));
        }
        self.work.max_live_node_bytes = self.work.max_live_node_bytes.max(text.len());
        Ok(node)
    }

    fn read_cursor(&mut self, cursor: &str) -> io::Result<String> {
        let mut q = self
            .db
            .prepare_cached("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_rank_candidate_cursors WHERE job=?1 AND cursor=?2")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let row: Option<Option<String>> = q
            .query_row(params![self.job, cursor], |r| r.get(0))
            .optional()
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let Some(Some(text)) = row else {
            return Err(invalid("candidate tree cursor is absent or over budget"));
        };
        self.work.read(text.len() + cursor.len(), vm);
        self.work.max_cursor_bytes = self.work.max_cursor_bytes.max(text.len());
        Ok(text)
    }

    fn cursor_exists(&mut self, cursor: &str) -> io::Result<bool> {
        let mut q = self
            .db
            .prepare_cached("SELECT EXISTS(SELECT 1 FROM native_rank_candidate_cursors WHERE job=?1 AND cursor=?2)")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let present = q
            .query_row(params![self.job, cursor], |r| r.get(0))
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work.read(0, vm);
        Ok(present)
    }

    fn put_cursor(&mut self, cursor: &str, text: &str) -> io::Result<()> {
        if text.len() > CURSOR_BYTES {
            return Err(invalid("candidate tree cursor exceeds scalar budget"));
        }
        let mut q = self
            .db
            .prepare_cached("INSERT INTO native_rank_candidate_cursors VALUES(?1,?2,?3) ON CONFLICT(job,cursor) DO UPDATE SET metadata=excluded.metadata")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        q.execute(params![self.job, cursor, text])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work.write(text.len() + cursor.len(), vm);
        self.work.max_cursor_bytes = self.work.max_cursor_bytes.max(text.len());
        Ok(())
    }

    fn delete_cursor(&mut self, cursor: &str) -> io::Result<()> {
        let mut q = self
            .db
            .prepare_cached("DELETE FROM native_rank_candidate_cursors WHERE job=?1 AND cursor=?2")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = q
            .execute(params![self.job, cursor])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        if changed != 1 {
            return Err(invalid("candidate tree cursor disappeared"));
        }
        self.work.write(cursor.len(), vm);
        Ok(())
    }

    fn read_apply(&mut self, cursor: &str) -> io::Result<Option<Apply>> {
        let mut q = self
            .db
            .prepare_cached("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_rank_candidate_applies WHERE job=?1 AND cursor=?2")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let row: Option<Option<String>> = q
            .query_row(params![self.job, cursor], |r| r.get(0))
            .optional()
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let text = match row {
            None => {
                self.work.read(cursor.len(), vm);
                return Ok(None);
            }
            Some(None) => {
                return Err(invalid("candidate tree Apply row exceeds scalar budget"));
            }
            Some(Some(text)) => text,
        };
        self.work.read(text.len() + cursor.len(), vm);
        self.work.max_apply_bytes = self.work.max_apply_bytes.max(text.len());
        let apply: Apply = serde_json::from_str(&text).map_err(io::Error::other)?;
        if !apply.valid() {
            return Err(invalid("candidate tree Apply state is invalid"));
        }
        Ok(Some(apply))
    }

    fn put_apply(&mut self, cursor: &str, apply: &Apply) -> io::Result<()> {
        if !apply.valid() {
            return Err(invalid("candidate tree Apply state is invalid"));
        }
        let text = encoded(apply, CURSOR_BYTES)?;
        let mut q = self
            .db
            .prepare_cached("INSERT INTO native_rank_candidate_applies VALUES(?1,?2,?3) ON CONFLICT(job,cursor) DO UPDATE SET metadata=excluded.metadata")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        q.execute(params![self.job, cursor, text])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work.write(text.len() + cursor.len(), vm);
        self.work.max_apply_bytes = self.work.max_apply_bytes.max(text.len());
        Ok(())
    }

    fn delete_apply(&mut self, cursor: &str) -> io::Result<()> {
        let mut q = self
            .db
            .prepare_cached("DELETE FROM native_rank_candidate_applies WHERE job=?1 AND cursor=?2")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = q
            .execute(params![self.job, cursor])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        if changed != 1 {
            return Err(invalid("candidate tree Apply row disappeared"));
        }
        self.work.write(cursor.len(), vm);
        Ok(())
    }

    fn bind_root(&mut self, root: &CandidateRoot, graph: &mut Graph<'_>) -> io::Result<()> {
        if !root_valid(root) {
            return Err(invalid("candidate tree root is invalid"));
        }
        graph.node(&root.empty)?;
        if let Some(tree) = &root.tree {
            self.read_node(tree)?;
        }
        let identity = root_identity(root);
        let empty = root.empty.identity();
        let mut q = self
            .db
            .prepare_cached("INSERT INTO native_rank_candidate_roots VALUES(?1,?2,?3) ON CONFLICT(job,identity,empty) DO NOTHING")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = q
            .execute(params![self.job, identity, empty])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work
            .write(identity.len() + root.empty.identity().len(), vm);
        drop(q);
        if changed != 0 {
            if let Some(tree) = &root.tree {
                self.reference(&tree.identity(), 1)?;
            }
            graph.references(&root.empty.identity(), 1)?;
        }
        Ok(())
    }

    fn root_bound(&mut self, root: &CandidateRoot) -> io::Result<bool> {
        let mut q = self
            .db
            .prepare_cached("SELECT EXISTS(SELECT 1 FROM native_rank_candidate_roots WHERE job=?1 AND identity=?2 AND empty=?3)")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let present = q
            .query_row(
                params![self.job, root_identity(root), root.empty.identity()],
                |r| r.get(0),
            )
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work.read(0, vm);
        Ok(present)
    }

    fn intern(
        &mut self,
        node: CandidateNode,
        graph: &mut Graph<'_>,
    ) -> io::Result<CandidateNodeId> {
        if !node.valid() {
            return Err(invalid("invalid candidate tree node ordering"));
        }
        let aggregate = node.aggregate().clone();
        graph.node(&aggregate)?;
        let (left, right) = match &node {
            CandidateNode::Leaf { .. } => (None, None),
            CandidateNode::Branch {
                split, left, right, ..
            } => {
                let left_node = self.read_node(left)?;
                let right_node = self.read_node(right)?;
                validate_children(*split, &left_node, &right_node)?;
                (Some(left.identity()), Some(right.identity()))
            }
        };
        let text = encoded(&node, NODE_BYTES)?;
        let id = candidate_id(&text);
        let identity = id.identity();
        let aggregate_identity = aggregate.identity();
        let mut q = self
            .db
            .prepare_cached("INSERT INTO native_rank_candidate_nodes VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(identity) DO NOTHING")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let allocated = q
            .execute(params![identity, text, left, right, aggregate_identity])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work.write(
            text.len()
                + identity.len()
                + aggregate_identity.len()
                + left.as_ref().map_or(0, String::len)
                + right.as_ref().map_or(0, String::len),
            vm,
        );
        drop(q);
        if allocated != 0 {
            self.work.nodes_written += 1;
            for child in [left.as_ref(), right.as_ref()].into_iter().flatten() {
                self.reference(child, 1)?;
            }
            graph.references(&aggregate_identity, 1)?;
        }
        self.bind_node(&identity)?;
        self.read_node(&id)?;
        Ok(id)
    }

    fn bind_node(&mut self, identity: &str) -> io::Result<()> {
        let mut q = self
            .db
            .prepare_cached("INSERT INTO native_rank_candidate_bindings VALUES(?1,?2) ON CONFLICT(job,identity) DO NOTHING")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = q
            .execute(params![self.job, identity])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work.write(identity.len(), vm);
        drop(q);
        if changed != 0 {
            self.reference(identity, 1)?;
        }
        Ok(())
    }

    fn reference(&mut self, identity: &str, delta: i64) -> io::Result<()> {
        let mut q = self
            .db
            .prepare_cached("UPDATE native_rank_candidate_nodes SET refs=refs+?2 WHERE identity=?1")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = q
            .execute(params![identity, delta])
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        self.work.write(identity.len(), vm);
        if changed != 1 {
            return Err(invalid("candidate tree dependency is absent"));
        }
        Ok(())
    }
}

/// Candidate references are released before the parent rank cleanup touches
/// graph bindings. Every delete/reference update consumes the inherited row
/// budget; no historical cascade or recursive sweep is used.
pub(super) fn cleanup(
    db: &Connection,
    job: i64,
    budget: usize,
    bytes: &mut usize,
) -> rusqlite::Result<(usize, bool)> {
    cleanup_with_allowance(db, job, budget, STEP_ROWS, bytes)
        .map(|step| (step.removed, step.complete))
}

/// Each stage selects one guarded key at a time. Allowance counts SELECTs,
/// empty/schema probes, dependency validation, deletions and reference writes.
pub(super) fn cleanup_with_allowance(
    db: &Connection,
    job: i64,
    budget: usize,
    allowance: usize,
    bytes: &mut usize,
) -> rusqlite::Result<super::Cleanup> {
    let initial = *bytes;
    let mut removed = 0;
    let mut work = super::Work::default();
    macro_rules! finish {
        ($complete:expr) => {{
            *bytes = initial
                .checked_sub(work.metadata_bytes)
                .ok_or_else(|| cleanup_error("candidate cleanup exceeded byte allowance"))?;
            return Ok(super::Cleanup {
                removed,
                complete: $complete,
                work,
            });
        }};
    }
    macro_rules! reserve {
        ($rows:expr, $writes:expr, $bytes:expr) => {
            if work.rows() + $rows > allowance
                || removed + $writes > budget
                || initial - work.metadata_bytes < $bytes
            {
                finish!(false);
            }
        };
    }
    reserve!(1, 0, 0);
    let installed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='native_rank_candidate_cursors')",
        [], |r| r.get(0),
    )?;
    work.sql_reads += 1;
    if !installed {
        finish!(true);
    }
    reserve!(1, 0, 0);
    let empty: bool = db.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM native_rank_candidate_applies WHERE job=?1) AND NOT EXISTS(SELECT 1 FROM native_rank_candidate_cursors WHERE job=?1) AND NOT EXISTS(SELECT 1 FROM native_rank_candidate_roots WHERE job=?1) AND NOT EXISTS(SELECT 1 FROM native_rank_candidate_bindings WHERE job=?1) AND NOT EXISTS(SELECT 1 FROM native_rank_candidate_nodes WHERE refs=0)",
        [job], |r| r.get(0),
    )?;
    work.sql_reads += 1;
    if empty {
        finish!(true);
    }
    for table in [
        "native_rank_candidate_applies",
        "native_rank_candidate_cursors",
    ] {
        loop {
            reserve!(2, 1, NODE_BYTES + 256);
            let row: Option<(String, i64)> = db.query_row(
                &format!("SELECT CASE WHEN octet_length(cursor) BETWEEN 1 AND 256 AND octet_length(metadata)<=65536 THEN cursor END,octet_length(metadata)+octet_length(cursor) FROM {table} WHERE job=?1 ORDER BY cursor LIMIT 1"),
                [job], |r| Ok((cleanup_text(r, 0)?, r.get(1)?)),
            ).optional()?;
            work.sql_reads += 1;
            let Some((cursor, length)) = row else {
                break;
            };
            work.metadata_bytes += usize::try_from(length)
                .map_err(|_| cleanup_error("invalid candidate cursor length"))?;
            if db.execute(
                &format!("DELETE FROM {table} WHERE job=?1 AND cursor=?2"),
                params![job, cursor],
            )? != 1
            {
                return Err(cleanup_error("candidate cleanup cursor disappeared"));
            }
            work.sql_writes += 1;
            removed += 1;
        }
    }
    loop {
        // Root key + up to two native dependency reads + three writes.
        reserve!(6, 3, 2 * (NODE_BYTES + 4 * 80) + 160);
        let row: Option<(String, String)> = db.query_row(
            "SELECT CASE WHEN identity='rank/v1/tree/empty' OR octet_length(identity)=77 THEN identity END,CASE WHEN octet_length(empty) BETWEEN 79 AND 80 THEN empty END FROM native_rank_candidate_roots WHERE job=?1 ORDER BY identity,empty LIMIT 1",
            [job], |r| Ok((cleanup_text(r, 0)?, cleanup_text(r, 1)?)),
        ).optional()?;
        work.sql_reads += 1;
        let Some((identity, empty)) = row else {
            break;
        };
        work.metadata_bytes += identity.len() + empty.len();
        let mut tree = TreeDb {
            db,
            job,
            work: CandidateWork::default(),
        };
        let mut graph = Graph {
            db,
            job,
            work: super::Work::default(),
        };
        if identity != EMPTY_TREE {
            let id = parse_tree_identity(&identity)
                .ok_or_else(|| cleanup_error("invalid candidate root"))?;
            tree.read_node(&id)
                .map_err(|_| cleanup_error("candidate cleanup root corrupt"))?;
        }
        let empty_id =
            NodeId::parse(&empty).map_err(|_| cleanup_error("invalid candidate empty graph"))?;
        graph
            .node(&empty_id)
            .map_err(|_| cleanup_error("candidate empty graph corrupt"))?;
        if db.execute(
            "DELETE FROM native_rank_candidate_roots WHERE job=?1 AND identity=?2 AND empty=?3",
            params![job, identity, empty],
        )? != 1
        {
            return Err(cleanup_error("candidate cleanup root disappeared"));
        }
        work.sql_writes += 1;
        removed += 1;
        if identity != EMPTY_TREE {
            decrement_tree_ref(db, &identity)?;
            work.sql_writes += 1;
            removed += 1;
        }
        graph
            .references_validated(&empty, -1)
            .map_err(|_| cleanup_error("candidate empty graph absent"))?;
        removed += 1;
        add_cleanup_work(&mut work, tree.work, graph.work);
    }
    loop {
        reserve!(4, 2, NODE_BYTES + 5 * 80);
        let row: Option<String> = db.query_row(
            "SELECT CASE WHEN octet_length(identity)=77 THEN identity END FROM native_rank_candidate_bindings WHERE job=?1 ORDER BY identity LIMIT 1",
            [job], |r| cleanup_text(r, 0),
        ).optional()?;
        work.sql_reads += 1;
        let Some(identity) = row else {
            break;
        };
        work.metadata_bytes += identity.len();
        let mut tree = TreeDb {
            db,
            job,
            work: CandidateWork::default(),
        };
        let id = parse_tree_identity(&identity)
            .ok_or_else(|| cleanup_error("invalid candidate binding"))?;
        tree.read_node(&id)
            .map_err(|_| cleanup_error("candidate binding corrupt"))?;
        if db.execute(
            "DELETE FROM native_rank_candidate_bindings WHERE job=?1 AND identity=?2",
            params![job, identity],
        )? != 1
        {
            return Err(cleanup_error("candidate binding disappeared"));
        }
        decrement_tree_ref(db, &identity)?;
        work.sql_writes += 2;
        removed += 2;
        add_cleanup_work(&mut work, tree.work, super::Work::default());
    }
    loop {
        // Key + parent/two children/aggregate reads + delete/three ref writes.
        reserve!(9, 4, 4 * (NODE_BYTES + 4 * 80) + 80);
        let row: Option<String> = db.query_row(
            "SELECT CASE WHEN octet_length(identity)=77 THEN identity END FROM native_rank_candidate_nodes WHERE refs=0 ORDER BY identity LIMIT 1",
            [], |r| cleanup_text(r, 0),
        ).optional()?;
        work.sql_reads += 1;
        let Some(identity) = row else {
            finish!(true);
        };
        work.metadata_bytes += identity.len();
        let id = parse_tree_identity(&identity)
            .ok_or_else(|| cleanup_error("invalid candidate garbage address"))?;
        let mut tree = TreeDb {
            db,
            job,
            work: CandidateWork::default(),
        };
        let mut graph = Graph {
            db,
            job,
            work: super::Work::default(),
        };
        let node = tree
            .read_node(&id)
            .map_err(|_| cleanup_error("candidate garbage corrupt"))?;
        let children = match node {
            CandidateNode::Leaf { .. } => Vec::new(),
            CandidateNode::Branch {
                ref left,
                ref right,
                ..
            } => vec![left.clone(), right.clone()],
        };
        for child in &children {
            tree.read_node(child)
                .map_err(|_| cleanup_error("candidate garbage child corrupt"))?;
        }
        graph
            .node(node.aggregate())
            .map_err(|_| cleanup_error("candidate garbage aggregate corrupt"))?;
        if db.execute(
            "DELETE FROM native_rank_candidate_nodes WHERE identity=?1 AND refs=0",
            [&identity],
        )? != 1
        {
            return Err(cleanup_error("candidate garbage changed"));
        }
        work.sql_writes += 1;
        removed += 1;
        for child in &children {
            decrement_tree_ref(db, &child.identity())?;
            work.sql_writes += 1;
            removed += 1;
        }
        graph
            .references_validated(&node.aggregate().identity(), -1)
            .map_err(|_| cleanup_error("candidate aggregate absent"))?;
        removed += 1;
        add_cleanup_work(&mut work, tree.work, graph.work);
    }
}

fn add_cleanup_work(work: &mut super::Work, tree: CandidateWork, graph: super::Work) {
    work.add(super::Work {
        sql_reads: tree.sql_reads,
        sql_writes: tree.sql_writes,
        metadata_bytes: tree.metadata_bytes,
        vm_steps: tree.vm_steps,
        max_live_node_bytes: tree.max_live_node_bytes,
        ..super::Work::default()
    });
    work.add(graph);
}

fn parse_tree_identity(identity: &str) -> Option<CandidateNodeId> {
    let digest = identity.strip_prefix("rank/v1/tree/")?;
    let id = CandidateNodeId {
        digest: digest.to_owned(),
    };
    id.valid().then_some(id)
}

fn decrement_tree_ref(db: &Connection, identity: &str) -> rusqlite::Result<()> {
    if db.execute(
        "UPDATE native_rank_candidate_nodes SET refs=refs-1 WHERE identity=?1 AND refs>0",
        [identity],
    )? != 1
    {
        return Err(cleanup_error("candidate tree reference is absent"));
    }
    Ok(())
}

// Every variable-width cleanup column is SQL-guarded before Rust materializes
// it. A NULL guard result is corrupt derived metadata, never child absence.
fn cleanup_text(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<String> {
    row.get::<_, Option<String>>(column)?
        .ok_or_else(|| cleanup_error("candidate tree cleanup address exceeds its width"))
}

fn cleanup_error(message: &'static str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(io::Error::new(
        io::ErrorKind::InvalidData,
        message,
    )))
}

#[cfg(test)]
mod tests {
    use super::super::Node;
    use super::*;
    use crate::{
        events::store::SqliteStore,
        events::{Event, EventCache, FileIndex, Kind},
        model::{
            native_stage::{Entry, EntryKind},
            range_stage::{Facts, Leaf, Order},
            summary::CatalogRow,
            turn_stage::HandoffEnd,
        },
    };
    use rusqlite::Transaction;
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[derive(Clone)]
    struct TestEntry {
        seq: usize,
        key: [u8; KEY_BYTES],
        candidate: CandidateRef,
        facts: Facts,
        guard: Vec<(u8, bool)>,
    }

    struct Fixture {
        root: PathBuf,
        options: crate::Options,
        row: CatalogRow,
        store: NativeStore,
        spec: native_query::JobSpec,
        job: Job,
        empty: NodeId,
        cursor: usize,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-candidate-tree-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let options = crate::Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                cache: root.join("cache"),
                ..crate::Options::default()
            };
            let path = options.claude_home.join("projects/p/chosen.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = b"fixture\n";
            fs::write(&path, bytes).unwrap();
            let ledger = crate::events::Ledger {
                stat: crate::events::Stat::of(&path).unwrap(),
                offset: bytes.len() as u64,
                prefix: Sha256::digest(bytes).into(),
                tail: Sha256::digest(bytes).into(),
            };
            let index = FileIndex {
                events: vec![Event {
                    k: Kind::A,
                    o: 0,
                    t: Some(0),
                    id: Some("native-0".into()),
                    ..Event::default()
                }],
                ..FileIndex::default()
            };
            let source = ledger.source(&path, "chosen");
            let row: CatalogRow = serde_json::from_value(serde_json::json!({
                "key":"chosen", "name":"chosen", "harness":"claude", "kind":"session",
                "native_ids":["chosen"], "parent":null, "parent_source":null, "repo":null,
                "branch":null, "model":"test", "effort":null, "start":0, "last":0,
                "tokens":[0.0,0.0,0.0], "cost":crate::pricing::Cost::default(), "sources":[source]
            }))
            .unwrap();
            let (mut writer, _) =
                SqliteStore::attempt_scoped(&EventCache::path(&options.cache), None, true)
                    .unwrap_or_else(|error| panic!("candidate tree fixture store: {error}"));
            assert_eq!(
                writer
                    .write_one(path.to_str().unwrap(), None, None, &ledger, &index)
                    .unwrap(),
                Outcome::Written
            );
            writer.connection.execute(
                "INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,?2,?3,?4)",
                params![row.key, row.last.unwrap(), row.harness, serde_json::to_string(&row).unwrap()],
            ).unwrap();
            writer
                .connection
                .execute(
                    "INSERT INTO meta(key,value) VALUES('catalog_generation','candidate-tree-one')",
                    [],
                )
                .unwrap();
            let mut store = NativeStore {
                connection: writer.connection,
                options: options.clone(),
            };
            // Exercise the private owner initialization, then register this
            // pending private module through its separately exported schema.
            // No public Trace initializer or schema migration is involved.
            native_query::initialize(&store.connection).unwrap();
            store
                .connection
                .execute_batch(super::super::CANDIDATE_TREE_SCHEMA)
                .unwrap();
            let spec = native_query::JobSpec {
                view_key: "chosen:candidate-tree".into(),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&options).unwrap(),
            };
            let job = store.begin(&spec).unwrap();
            let source = store.source_witness(&row.sources[0]).unwrap();
            let owner = store.owner_witness(&row).unwrap();
            assert_eq!(
                store
                    .admit(&job, &[source], &[owner], "complete-source-closure")
                    .unwrap(),
                Outcome::Written
            );
            let job = store.reload(&spec.view_key).unwrap().unwrap();
            let empty_facts = serde_json::to_string(&Facts::default()).unwrap();
            let (outcome, empty) = store
                .rank_guard(
                    &job,
                    &empty_facts,
                    &empty_facts,
                    &[],
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(outcome, Outcome::Written);
            let job = store.reload(&spec.view_key).unwrap().unwrap();
            Self {
                root,
                options,
                row,
                store,
                spec,
                job,
                empty: empty.unwrap().0,
                cursor: 0,
            }
        }

        fn refresh(&mut self) {
            self.job = self.store.reload(&self.spec.view_key).unwrap().unwrap();
        }

        fn new_job(&mut self, view_key: &str) -> (native_query::JobSpec, Job) {
            let spec = native_query::JobSpec {
                view_key: view_key.into(),
                ..self.spec.clone()
            };
            let job = self.store.begin(&spec).unwrap();
            let source = self.store.source_witness(&self.row.sources[0]).unwrap();
            let owner = self.store.owner_witness(&self.row).unwrap();
            assert_eq!(
                self.store
                    .admit(&job, &[source], &[owner], "complete-source-closure")
                    .unwrap(),
                Outcome::Written
            );
            let job = self.store.reload(view_key).unwrap().unwrap();
            (spec, job)
        }

        fn graph_leaf(&mut self, entry: &TestEntry, variant: usize) -> CandidateLeaf {
            self.graph_leaf_with_guard(entry, variant, &entry.guard)
        }

        fn graph_leaf_with_guard(
            &mut self,
            entry: &TestEntry,
            variant: usize,
            decisions: &[(u8, bool)],
        ) -> CandidateLeaf {
            let facts = facts_for(
                entry.seq,
                entry.facts.first.as_ref().unwrap().order.key.0,
                variant,
            );
            let empty = serde_json::to_string(&Facts::default()).unwrap();
            let facts = serde_json::to_string(&facts).unwrap();
            let (outcome, result) = self
                .store
                .rank_guard(
                    &self.job,
                    &facts,
                    &empty,
                    decisions,
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(outcome, Outcome::Written);
            self.refresh();
            CandidateLeaf {
                candidate: entry.candidate,
                aggregate: result.unwrap().0,
                payload: None,
            }
        }

        fn graph_leaf_with_paged_guard(
            &mut self,
            entry: &TestEntry,
            variant: usize,
            decisions: &[(u8, bool)],
        ) -> CandidateLeaf {
            let facts = facts_for(
                entry.seq,
                entry.facts.first.as_ref().unwrap().order.key.0,
                variant,
            );
            let facts = serde_json::to_string(&facts).unwrap();
            let empty = serde_json::to_string(&Facts::default()).unwrap();
            self.cursor += 1;
            let cursor = format!("wide-guard-{}", self.cursor);
            assert_eq!(
                self.store
                    .rank_begin_guard(
                        &self.job,
                        super::super::GuardInput {
                            cursor: &cursor,
                            facts: &facts,
                            empty: &empty,
                            decisions,
                        },
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap(),
                Outcome::Written
            );
            self.refresh();
            let mut result = None;
            for _ in 0..256 {
                let step = self
                    .store
                    .rank_advance_guard(&self.job, &cursor, &valid_closure, &|| false)
                    .unwrap();
                assert_eq!(step.outcome, Outcome::Written);
                assert!(step.work.sql_reads + step.work.sql_writes <= STEP_ROWS);
                assert!(step.work.metadata_bytes <= STEP_BYTES);
                self.refresh();
                if let Some(root) = step.complete {
                    result = Some(root);
                    break;
                }
            }
            CandidateLeaf {
                candidate: entry.candidate,
                aggregate: result.expect("64-source graph guard exceeded its step bound"),
                payload: None,
            }
        }

        fn update(
            &mut self,
            root: &CandidateRoot,
            key: [u8; KEY_BYTES],
            replacement: Option<CandidateLeaf>,
            version: &str,
        ) -> (CandidateRoot, CandidateWork) {
            self.cursor += 1;
            let cursor = format!("candidate-tree-{}", self.cursor);
            let combine = combine_facts;
            let (start, allocations, allocated_bytes) = measure_test_allocations(|| {
                self.store
                    .candidate_tree_begin_update(
                        &self.job,
                        &cursor,
                        root,
                        key,
                        replacement,
                        version,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap()
            });
            check_allocations(allocations, allocated_bytes);
            assert_eq!(start.outcome, Outcome::Written);
            assert!(start.complete.is_none());
            check_step(&start.work);
            let mut work = start.work;
            self.refresh();
            for _ in 0..2000 {
                let (step, allocations, allocated_bytes) = measure_test_allocations(|| {
                    self.store
                        .candidate_tree_advance_update(
                            &self.job,
                            &cursor,
                            version,
                            &combine,
                            &valid_closure,
                            &|| false,
                        )
                        .unwrap()
                });
                check_allocations(allocations, allocated_bytes);
                assert_eq!(step.outcome, Outcome::Written);
                check_step(&step.work);
                add_work(&mut work, &step.work);
                self.refresh();
                if let Some(root) = step.complete {
                    return (root, work);
                }
            }
            panic!("candidate tree update did not finish in bounded steps")
        }

        fn ready(&mut self) {
            let changed = self
                .store
                .connection
                .execute(
                    "UPDATE native_query_jobs SET phase='ready',revision=revision+1 WHERE job=?1 AND revision=?2",
                    params![self.job.id, self.job.revision],
                )
                .unwrap();
            assert_eq!(changed, 1);
            self.refresh();
        }

        fn reopen(&mut self, spec: &native_query::JobSpec) -> Job {
            self.store = NativeStore::open(&self.options).unwrap();
            self.store.reload(&spec.view_key).unwrap().unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn native_source_forest_cold_seed_omission_ranges_and_seek_match_complete_order() {
        use crate::model::indexed_native::forest::{Channel, Forest, Source, Weight};
        for history in [0usize, 128, 1024, 8192] {
            let mut fixture = Fixture::new();
            let mut roots = vec![CandidateRoot::empty(fixture.empty.clone()); 3];
            let mut native = Vec::new();
            for seq in 0..history + 3 {
                let source = seq % 3;
                let time = if source == 0 {
                    0
                } else {
                    (seq as i64 % 11) - 5
                };
                let identity = Sha256::digest(seq.to_le_bytes()).into();
                let order = Order {
                    key: (time, 0, (history + 3 - seq) as u64, (seq % 2) as u32, 1),
                    ordinal: seq as u64,
                    identity,
                };
                let entry = Entry {
                    kind: match seq % 7 {
                        0 => EntryKind::U,
                        1 => EntryKind::Gap,
                        2 => EntryKind::Operation(crate::model::ToolState::Ok),
                        _ => EntryKind::A,
                    },
                    file: 0,
                    offset: order.key.2,
                    pos: (order.key.2, order.key.3),
                    t: (source != 0).then_some(time),
                    at: Some((0, seq)),
                };
                let event = Event {
                    t: entry.t,
                    ..Event::default()
                };
                let mut facts = Facts::default();
                facts
                    .observe(Leaf {
                        order: order.clone(),
                        entry: &entry,
                        found: Some(&event),
                        outgoing: None,
                        boundary: matches!(entry.kind, EntryKind::U),
                        count: u64::from(!matches!(entry.kind, EntryKind::Operation(_))),
                    })
                    .unwrap();
                let copied = seq % 5 == 0;
                let empty = serde_json::to_string(&Facts::default()).unwrap();
                let facts_text = serde_json::to_string(&facts).unwrap();
                let decisions: &[(u8, bool)] = if copied { &[(1, false)] } else { &[] };
                let (outcome, leaf) = fixture
                    .store
                    .rank_guard(
                        &fixture.job,
                        &facts_text,
                        &empty,
                        decisions,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(outcome, Outcome::Written);
                fixture.refresh();
                roots[source] = fixture
                    .update(
                        &roots[source],
                        order.encoded(),
                        Some(CandidateLeaf {
                            candidate: identity,
                            aggregate: leaf.unwrap().0,
                            payload: None,
                        }),
                        "native-forest-facts-v2",
                    )
                    .0;
                native.push((source, copied, order, facts));
            }
            fixture.ready();
            // Both seeds are cold: only scalars/source order differ. The trees
            // are neither rewritten nor warmed up for either query context.
            for seed in [i64::MIN, -3, 40] {
                for (bits, omitted) in [(0, None), (2, Some(1usize))] {
                    let sources: Vec<_> = roots
                        .iter()
                        .enumerate()
                        .filter(|(source, _)| Some(*source) != omitted)
                        .map(|(source, root)| Source {
                            root,
                            channel: if source == 0 {
                                Channel::BeforeFirst
                            } else {
                                Channel::Observed
                            },
                            source: 17 + source,
                            file_order: 2 - source,
                            first_regular_time: Some(seed),
                            included: bits,
                        })
                        .collect();
                    let forest = Forest {
                        store: &fixture.store,
                        job: &fixture.job,
                        sources: &sources,
                        several: true,
                        closure: &valid_closure,
                        cancelled: &|| false,
                    };
                    let mut oracle: Vec<_> = native
                        .iter()
                        .filter(|(source, copied, _, _)| {
                            Some(*source) != omitted && (!copied || bits & 2 == 0)
                        })
                        .map(|(source, _, order, facts)| {
                            let mut order = order.clone();
                            let mut facts = facts.clone();
                            order.key.0 = if *source == 0 { seed } else { order.key.0 };
                            order.key.1 = 2 - *source;
                            facts.qualify(
                                17 + source,
                                2 - source,
                                true,
                                (*source == 0).then_some(seed),
                            );
                            (order, facts)
                        })
                        .collect();
                    oracle.sort_by(|a, b| a.0.cmp(&b.0));
                    let Some((bound, _)) = oracle.get(oracle.len().saturating_sub(2)) else {
                        continue;
                    };
                    let bounds = CandidateBounds {
                        lower: None,
                        upper: Some(bound.encoded()),
                    };
                    let mut expected = Facts::default();
                    for (order, facts) in &oracle {
                        if order <= bound {
                            expected.merge(facts.clone()).unwrap();
                        }
                    }
                    let reads = crate::events::FILE_READS.with(Cell::get);
                    let (range, calls, allocated) =
                        measure_test_allocations(|| forest.range(&bounds).unwrap());
                    assert!(matches!(range.outcome, CandidateReadOutcome::Qualified));
                    assert_eq!(
                        serde_json::to_value(range.value.unwrap()).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                    assert!(calls > 0 && allocated <= 16 * STEP_BYTES);
                    assert!(
                        range.work.sql_reads < 1000
                            && range.work.metadata_bytes < 16 * STEP_BYTES
                            && range.work.sqlite_vm_steps < 2_000_000,
                        "forest range exceeded fixed context bound: {:?}",
                        range.work
                    );
                    let sought = forest
                        .seek(
                            &bound.encoded(),
                            CandidateDirection::Forward,
                            Weight::Entries,
                        )
                        .unwrap();
                    assert!(matches!(sought.outcome, CandidateReadOutcome::Qualified));
                    let point = sought.value.unwrap().unwrap();
                    assert_eq!(point.order, *bound);
                    assert_eq!(
                        point.source,
                        17 + native
                            .iter()
                            .find(|(_, _, order, _)| order.identity == point.order.identity)
                            .unwrap()
                            .0
                    );
                    assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
                    assert!(
                        sought.work.sql_reads < 1000
                            && sought.work.metadata_bytes < 16 * STEP_BYTES
                            && sought.work.sqlite_vm_steps < 2_000_000
                    );
                    let cancelled = Forest {
                        cancelled: &|| true,
                        ..forest
                    };
                    assert!(matches!(
                        cancelled.range(&bounds).unwrap().outcome,
                        CandidateReadOutcome::Deferred
                    ));
                }
            }
        }
    }

    #[test]
    fn candidate_tree_schema_is_registered_separately_by_the_private_owner() {
        let fixture = Fixture::new();
        assert!(
            native_query::schema_version(&fixture.store.connection)
                .unwrap()
                .is_some()
        );
        assert_eq!(super::super::CANDIDATE_TREE_SCHEMA, SCHEMA);
        for table in [
            "native_rank_candidate_nodes",
            "native_rank_candidate_bindings",
            "native_rank_candidate_roots",
            "native_rank_candidate_cursors",
            "native_rank_candidate_applies",
        ] {
            let main: bool = fixture
                .store
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            let temporary: bool = fixture
                .store
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(main, "private owner did not register {table}");
            assert!(!temporary, "candidate table {table} entered TEMP");
        }
    }

    fn valid_closure(_: &Connection, _: i64) -> rusqlite::Result<bool> {
        Ok(true)
    }

    fn private_cleanup_row_count(db: &Connection) -> i64 {
        db.query_row(
            "SELECT
               (SELECT count(*) FROM native_rank_candidate_applies) +
               (SELECT count(*) FROM native_rank_candidate_cursors) +
               (SELECT count(*) FROM native_rank_candidate_roots) +
               (SELECT count(*) FROM native_rank_candidate_bindings) +
               (SELECT count(*) FROM native_rank_candidate_nodes) +
               (SELECT count(*) FROM native_rank_memos) +
               (SELECT count(*) FROM native_rank_bindings) +
               (SELECT count(*) FROM native_rank_nodes) +
               (SELECT count(*) FROM native_query_rows) +
               (SELECT count(*) FROM native_query_sources) +
               (SELECT count(*) FROM native_query_owners) +
               (SELECT count(*) FROM native_query_jobs)",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn order(seq: usize, time: i64, identity: CandidateRef) -> Order {
        Order {
            key: (time, seq % 3, seq as u64, (seq % 2) as u32, 1),
            ordinal: seq as u64,
            identity,
        }
    }

    fn test_entry(seq: usize, time: i64, variant: usize) -> TestEntry {
        let candidate: CandidateRef = Sha256::digest(seq.to_le_bytes()).into();
        let order = order(seq, time, candidate);
        let facts = facts_for(seq, time, variant);
        TestEntry {
            seq,
            key: order.encoded(),
            candidate,
            facts,
            guard: {
                let required = (seq % 3) as u8;
                let forbidden = ((seq + 1) % 3) as u8;
                let mut decisions = vec![(required, true), (forbidden, false)];
                decisions.sort_by_key(|(source, _)| *source);
                decisions
            },
        }
    }

    fn facts_for(seq: usize, time: i64, variant: usize) -> Facts {
        let candidate: CandidateRef = Sha256::digest(seq.to_le_bytes()).into();
        let order = order(seq, time, candidate);
        let kind = match (seq + variant) % 4 {
            0 => EntryKind::U,
            1 => EntryKind::A,
            2 => EntryKind::Tool(crate::model::ToolState::Pending),
            _ => EntryKind::H(seq),
        };
        let entry = Entry {
            kind,
            file: seq % 3,
            offset: seq as u64,
            pos: (seq as u64, 0),
            t: Some(time),
            at: Some((seq % 3, seq)),
        };
        let event = Event {
            t: Some(time),
            ..Event::default()
        };
        let mut facts = Facts::default();
        facts
            .observe(Leaf {
                order,
                entry: &entry,
                found: Some(&event),
                outgoing: matches!(kind, EntryKind::H(_)).then_some(HandoffEnd {
                    id: "candidate-handoff",
                    kind: "toyou",
                    status: "done",
                }),
                boundary: (seq + variant).is_multiple_of(5),
                count: 1,
            })
            .unwrap();
        facts
    }

    fn combine_facts(left: &str, right: &str) -> io::Result<String> {
        let mut left: Facts = serde_json::from_str(left).map_err(io::Error::other)?;
        left.append(serde_json::from_str(right).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        serde_json::to_string(&left).map_err(io::Error::other)
    }

    fn included(entry: &TestEntry, bits: u64) -> bool {
        entry
            .guard
            .iter()
            .all(|(source, included)| (bits & (1u64 << *source) != 0) == *included)
    }

    fn check_step(work: &CandidateWork) {
        assert!(work.sql_reads + work.sql_writes <= STEP_ROWS);
        assert!(work.metadata_bytes <= STEP_BYTES);
        assert!(work.max_cursor_bytes <= CURSOR_BYTES);
        assert!(work.max_apply_bytes <= CURSOR_BYTES);
        assert!(work.max_depth <= MAX_DEPTH + 1);
        assert!(work.sqlite_vm_steps > 0);
        assert!(work.sqlite_vm_steps <= 500_000);
        assert!(work.max_live_node_bytes <= NODE_BYTES);
    }

    fn check_allocations(calls: usize, bytes: usize) {
        assert!(calls > 0, "the operation recorded no Rust allocations");
        assert!(
            bytes <= 2 * STEP_BYTES,
            "per-operation Rust allocations exceeded 2 MiB"
        );
    }

    fn add_work(total: &mut CandidateWork, next: &CandidateWork) {
        total.sql_reads += next.sql_reads;
        total.sql_writes += next.sql_writes;
        total.metadata_bytes += next.metadata_bytes;
        total.vm_steps += next.vm_steps;
        total.sqlite_vm_steps += next.sqlite_vm_steps;
        total.apply_visits += next.apply_visits;
        total.nodes_written += next.nodes_written;
        total.max_depth = total.max_depth.max(next.max_depth);
        total.max_cursor_bytes = total.max_cursor_bytes.max(next.max_cursor_bytes);
        total.max_apply_bytes = total.max_apply_bytes.max(next.max_apply_bytes);
        total.max_live_node_bytes = total.max_live_node_bytes.max(next.max_live_node_bytes);
    }

    fn facts_count(text: &str) -> io::Result<u64> {
        let facts: Facts = serde_json::from_str(text).map_err(io::Error::other)?;
        Ok(facts.count)
    }

    fn ready_facts(
        fixture: &Fixture,
        root: &CandidateRoot,
        bounds: &CandidateBounds,
        bits: u64,
    ) -> Facts {
        let result = fixture
            .store
            .candidate_tree_range(
                &fixture.job,
                root,
                bounds,
                bits,
                &combine_facts,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(result.outcome, CandidateReadOutcome::Qualified);
        serde_json::from_str(result.value.as_deref().unwrap()).unwrap()
    }

    fn unpublished_stage(fixture: &mut Fixture) -> native_query::source_roots::SourceStage {
        let epoch = native_query::source_roots::Epoch {
            epoch: 91,
            base_generation: Some("candidate-tree-one".into()),
            source_revision: 93,
            runtime: fixture.spec.runtime.clone(),
        };
        fixture.store.connection.execute("INSERT INTO catalog_semantic_stage(singleton,epoch,phase,generation,runtime,revision) VALUES(1,?1,'publication',?2,?3,?4)",params![epoch.epoch,epoch.base_generation,epoch.runtime,epoch.source_revision]).unwrap();
        fixture.store.connection.execute("INSERT INTO meta VALUES('catalog_source_revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[epoch.source_revision.to_string()]).unwrap();
        let witness = fixture
            .store
            .source_witness(&fixture.row.sources[0])
            .unwrap();
        fixture
            .store
            .begin_source_roots(&epoch, &[witness])
            .unwrap()
            .unwrap()
    }
    fn refresh_source_stage(
        fixture: &Fixture,
        stage: &mut native_query::source_roots::SourceStage,
    ) {
        stage.job = fixture
            .store
            .reload(&stage.job.spec.view_key)
            .unwrap()
            .unwrap();
    }

    #[test]
    fn source_neighbor_is_resumable_structural_and_does_not_grant_query_ready() {
        let mut fixture = Fixture::new();
        let mut root = CandidateRoot::empty(fixture.empty.clone());
        let mut keys = Vec::new();
        // A deliberately deep content-address suffix exercises durable seek
        // continuation beyond one24-node attempt. Facts remain opaque here.
        for position in 0..80 {
            let mut entry = test_entry(position, 0, 0);
            entry.guard = vec![(0, true)];
            entry.candidate = [0; 32];
            entry.candidate[position / 8] = 1 << (7 - position % 8);
            entry.key[..37].fill(0);
            entry.key[37..].copy_from_slice(&entry.candidate);
            let leaf = fixture.graph_leaf(&entry, 0);
            root = fixture.update(&root, entry.key, Some(leaf), "facts-v1").0;
            keys.push(entry.key);
        }
        keys.sort();
        fixture.ready();
        let hidden = fixture
            .store
            .candidate_tree_seek(
                &fixture.job,
                &root,
                &[0; KEY_BYTES],
                CandidateDirection::Forward,
                0,
                &facts_count,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(hidden.outcome, CandidateReadOutcome::Qualified);
        assert!(hidden.value.is_none());
        let mut stage = unpublished_stage(&mut fixture);
        let query = fixture
            .store
            .candidate_tree_seek(
                &stage.job,
                &root,
                &[0; KEY_BYTES],
                CandidateDirection::Forward,
                0,
                &|_| Ok(1),
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(query.outcome, CandidateReadOutcome::Conflict);
        let begin = fixture
            .store
            .candidate_tree_source_begin_neighbor(
                &stage,
                "neighbor",
                &root,
                &[0; KEY_BYTES],
                CandidateDirection::Forward,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(begin.outcome, Outcome::Written);
        refresh_source_stage(&fixture, &mut stage);
        let mut attempts = 0;
        let selected = loop {
            let step = fixture
                .store
                .candidate_tree_source_advance_neighbor(&stage, "neighbor", &valid_closure, &|| {
                    false
                })
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            assert!(step.work.sql_reads + step.work.sql_writes <= STEP_ROWS);
            assert!(step.work.metadata_bytes <= STEP_BYTES);
            assert!(step.work.sqlite_vm_steps > 0);
            attempts += 1;
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            refresh_source_stage(&fixture, &mut stage);
            if step.complete {
                break step.point.unwrap();
            }
            assert!(attempts < 32);
        };
        assert!(attempts > 1);
        assert_eq!(selected.key, keys[0]);
        // Begin itself is replay-safe, including a completed result receipt.
        let replay = fixture
            .store
            .candidate_tree_source_begin_neighbor(
                &stage,
                "neighbor",
                &root,
                &[0; KEY_BYTES],
                CandidateDirection::Forward,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(replay.point.unwrap(), selected);
        refresh_source_stage(&fixture, &mut stage);
        assert!(
            fixture
                .store
                .candidate_tree_source_begin_neighbor(
                    &stage,
                    "neighbor",
                    &root,
                    &[255; KEY_BYTES],
                    CandidateDirection::Forward,
                    &valid_closure,
                    &|| false
                )
                .is_err()
        );
        refresh_source_stage(&fixture, &mut stage);
        fixture
            .store
            .candidate_tree_source_begin_neighbor(
                &stage,
                "back",
                &root,
                &keys[40],
                CandidateDirection::Backward,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        refresh_source_stage(&fixture, &mut stage);
        let found = loop {
            let step = fixture
                .store
                .candidate_tree_source_advance_neighbor(&stage, "back", &valid_closure, &|| false)
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            refresh_source_stage(&fixture, &mut stage);
            if step.complete {
                break step.point.unwrap();
            }
        };
        assert_eq!(found.key, keys[40]);
        assert_eq!(stage.job.phase, "capture");
        assert!(
            fixture
                .store
                .ready(&stage.job.spec.view_key, &valid_closure)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn source_update_resume_preserves_cursor_and_completed_replay_is_deterministic() {
        let mut fixture = Fixture::new();
        let old = test_entry(0, 0, 0);
        let old_leaf = fixture.graph_leaf(&old, 0);
        let empty = CandidateRoot::empty(fixture.empty.clone());
        let root = fixture
            .update(&empty, old.key, Some(old_leaf), "facts-v1")
            .0;
        let next = test_entry(1, 1, 0);
        let leaf = fixture.graph_leaf(&next, 0);
        let mut stage = unpublished_stage(&mut fixture);
        let mut completed = Vec::new();
        for _ in 0..2 {
            let step = fixture
                .store
                .candidate_tree_source_begin_update(
                    &stage,
                    "source-update",
                    &root,
                    next.key,
                    Some(leaf.clone()),
                    "facts-v1",
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            refresh_source_stage(&fixture, &mut stage);
            let cursor: String = fixture.store.connection.query_row("SELECT metadata FROM native_rank_candidate_cursors WHERE job=?1 AND cursor='source-update'",[stage.job.id],|r|r.get(0)).unwrap();
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            let step = fixture
                .store
                .candidate_tree_source_begin_update(
                    &stage,
                    "source-update",
                    &root,
                    next.key,
                    Some(leaf.clone()),
                    "facts-v1",
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            refresh_source_stage(&fixture, &mut stage);
            let resumed: String = fixture.store.connection.query_row("SELECT metadata FROM native_rank_candidate_cursors WHERE job=?1 AND cursor='source-update'",[stage.job.id],|r|r.get(0)).unwrap();
            assert_eq!(resumed, cursor);
            let mut different = next.key;
            different[0] ^= 1;
            assert!(
                fixture
                    .store
                    .candidate_tree_source_begin_update(
                        &stage,
                        "source-update",
                        &root,
                        different,
                        Some(leaf.clone()),
                        "facts-v1",
                        &valid_closure,
                        &|| false
                    )
                    .is_err()
            );
            refresh_source_stage(&fixture, &mut stage);
            let result = loop {
                let step = fixture
                    .store
                    .candidate_tree_source_advance_update(
                        &stage,
                        "source-update",
                        "facts-v1",
                        &combine_facts,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(step.outcome, Outcome::Written);
                assert!(step.work.sql_reads + step.work.sql_writes <= STEP_ROWS);
                assert!(step.work.metadata_bytes <= STEP_BYTES);
                refresh_source_stage(&fixture, &mut stage);
                if let Some(root) = step.complete {
                    break root;
                }
            };
            completed.push(result);
        }
        assert_eq!(completed[0], completed[1]);
        assert_eq!(stage.job.phase, "capture");
        let query = fixture
            .store
            .candidate_tree_seek(
                &stage.job,
                &completed[0],
                &[0; KEY_BYTES],
                CandidateDirection::Forward,
                0,
                &|_| Ok(1),
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(query.outcome, CandidateReadOutcome::Conflict);
    }

    #[test]
    fn source_neighbor_latches_cancellation_and_rechecks_epoch_after_snapshot_release() {
        let mut fixture = Fixture::new();
        let entry = test_entry(0, 0, 0);
        let leaf = fixture.graph_leaf(&entry, 0);
        let empty = CandidateRoot::empty(fixture.empty.clone());
        let root = fixture.update(&empty, entry.key, Some(leaf), "facts-v1").0;
        let mut stage = unpublished_stage(&mut fixture);
        fixture
            .store
            .candidate_tree_source_begin_neighbor(
                &stage,
                "cancel",
                &root,
                &[0; KEY_BYTES],
                CandidateDirection::Forward,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        refresh_source_stage(&fixture, &mut stage);
        let revision = stage.job.revision;
        let calls = Cell::new(0);
        let once = || {
            let call = calls.get() + 1;
            calls.set(call);
            call == 2
        };
        let step = fixture
            .store
            .candidate_tree_source_advance_neighbor(&stage, "cancel", &valid_closure, &once)
            .unwrap();
        assert_eq!(step.outcome, Outcome::Deferred);
        assert!(!step.complete && step.point.is_none());
        refresh_source_stage(&fixture, &mut stage);
        assert_eq!(stage.job.revision, revision);
        let external = Cell::new(0);
        let changed = |db: &Connection, _| {
            let call = external.get() + 1;
            external.set(call);
            if call == 3 {
                db.execute("UPDATE catalog_semantic_stage SET revision=revision+1", [])?;
            }
            Ok(true)
        };
        let step = fixture
            .store
            .candidate_tree_source_advance_neighbor(&stage, "cancel", &changed, &|| false)
            .unwrap();
        assert_eq!(external.get(), 3);
        assert_eq!(step.outcome, Outcome::Conflict);
        assert!(!step.complete && step.point.is_none());
        assert!(step.work.sqlite_vm_steps > 0);
    }

    #[test]
    fn candidate_and_orthogonal_cleanup_share_receipt_and_preserve_imported_roots() {
        use super::super::orthogonal;
        let mut fixture = Fixture::new();
        let mut root = CandidateRoot::empty(fixture.empty.clone());
        let mut rectangle_root = orthogonal::Root::empty(9).unwrap();
        let mut expected = Facts::default();
        for seq in 0..4 {
            let entry = test_entry(seq, seq as i64, 0);
            let leaf = fixture.graph_leaf_with_guard(&entry, 0, &[]);
            let point = orthogonal::Point {
                order: entry.key.to_vec(),
                scalar: vec![seq as u8; 9],
                candidate: leaf.candidate,
                facts: leaf.aggregate.clone(),
            };
            root = fixture
                .update(&root, entry.key, Some(leaf), "mixed-cleanup-v1")
                .0;
            expected.append(entry.facts).unwrap();
            let start = fixture
                .store
                .orthogonal_begin(
                    &fixture.job,
                    orthogonal::Mutation {
                        name: "mixed",
                        root: &rectangle_root,
                        order: &point.order,
                        point: Some(&point),
                        version: "mixed-cleanup-v1",
                        identity: &fixture.empty,
                    },
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(start.outcome, Outcome::Written);
            fixture.refresh();
            let mut completed = start.root;
            for _ in 0..2000 {
                if completed.is_some() {
                    break;
                }
                let step = fixture
                    .store
                    .orthogonal_advance(
                        &fixture.job,
                        "mixed",
                        &combine_facts,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(step.outcome, Outcome::Written);
                fixture.refresh();
                completed = step.root;
            }
            rectangle_root = completed.expect("mixed geometry did not progress");
        }
        let original_spec = fixture.spec.clone();
        let old = fixture.job.clone();
        let (next_spec, next) = fixture.new_job("mixed-import");
        assert_eq!(
            fixture
                .store
                .candidate_tree_begin_update(
                    &next,
                    "keep",
                    &root,
                    test_entry(100, 100, 0).key,
                    None,
                    "mixed-cleanup-v1",
                    &valid_closure,
                    &|| false,
                )
                .unwrap()
                .outcome,
            Outcome::Written
        );
        let next = fixture.store.reload(&next_spec.view_key).unwrap().unwrap();
        assert_eq!(
            fixture
                .store
                .orthogonal_import(&next, "mixed", &rectangle_root, &valid_closure, &|| false)
                .unwrap()
                .outcome,
            Outcome::Written
        );
        assert_eq!(fixture.store.restart(&old).unwrap(), Outcome::Written);
        let reads = crate::events::FILE_READS.with(Cell::get);
        let mut complete = false;
        for round in 0..4096 {
            let Some(job) = fixture.store.reload(&original_spec.view_key).unwrap() else {
                complete = true;
                break;
            };
            let (attempt, calls, bytes) =
                measure_test_allocations(|| fixture.store.cleanup_with_work(&job).unwrap());
            assert!(matches!(
                attempt.outcome,
                Outcome::Deferred | Outcome::Written
            ));
            assert!(attempt.work.rows() <= STEP_ROWS);
            assert!(attempt.work.metadata_bytes <= STEP_BYTES);
            assert!(
                attempt.connection_vm_steps >= attempt.work.vm_steps as u64
                    && attempt.connection_vm_steps <= 500_000
            );
            check_allocations(calls, bytes);
            assert!(measured_peak_bytes() <= STEP_BYTES);
            if round == 0 {
                fixture.store = NativeStore::open(&fixture.options).unwrap();
            }
        }
        assert!(complete, "mixed owners failed bounded eventual cleanup");
        fixture.spec = next_spec;
        fixture.refresh();
        fixture.ready();
        assert_eq!(
            serde_json::to_value(ready_facts(
                &fixture,
                &root,
                &CandidateBounds::default(),
                u64::MAX
            ))
            .unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let empty = serde_json::to_string(&Facts::default()).unwrap();
        let rectangle = orthogonal::Rectangle::default();
        let attempt = fixture
            .store
            .orthogonal_rectangle(
                &fixture.job,
                orthogonal::ReadInput {
                    name: "mixed",
                    root: &rectangle_root,
                    rectangle: &rectangle,
                    included: u64::MAX,
                    empty: &empty,
                    cancelled: &|| false,
                },
                &combine_facts,
                &valid_closure,
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&attempt.value.unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(crate::events::FILE_READS.with(Cell::get), reads);
    }

    #[test]
    fn candidate_tree_update_range_select_and_seek_match_native_facts_fold() {
        let mut fixture = Fixture::new();
        let empty = CandidateRoot::empty(fixture.empty.clone());
        let mut root = empty.clone();
        let times = [9, 9, -4, 13, 2, -17, 2, 20, 5, 5, -4, 31, 0, 13, -2, 7];
        let mut entries = Vec::new();
        let mut update_work = CandidateWork::default();
        for (seq, time) in times.into_iter().enumerate() {
            let entry = test_entry(seq, time, 0);
            let leaf = fixture.graph_leaf(&entry, 0);
            let (next, work) = fixture.update(&root, entry.key, Some(leaf), "facts-v1");
            root = next;
            add_work(&mut update_work, &work);
            entries.push(entry);
        }

        let replaced = test_entry(5, times[5], 1);
        let leaf = fixture.graph_leaf(&replaced, 1);
        let (next, work) = fixture.update(&root, replaced.key, Some(leaf), "facts-v1");
        root = next;
        add_work(&mut update_work, &work);
        entries.retain(|entry| entry.seq != replaced.seq);
        entries.push(replaced);

        let removed = entries
            .iter()
            .find(|entry| entry.seq == 10)
            .unwrap()
            .clone();
        let (next, work) = fixture.update(&root, removed.key, None, "facts-v1");
        root = next;
        add_work(&mut update_work, &work);
        entries.retain(|entry| entry.seq != removed.seq);
        entries.sort_by_key(|entry| entry.key);
        assert!(update_work.apply_visits > 0);
        assert!(update_work.nodes_written > 0);
        assert!(update_work.vm_steps > 0);
        assert!(update_work.max_live_node_bytes < NODE_BYTES);

        fixture.ready();
        for bits in 0..8u64 {
            let mut expected = Facts::default();
            let selected: Vec<_> = entries
                .iter()
                .filter(|entry| included(entry, bits))
                .collect();
            for entry in &selected {
                expected.append(entry.facts.clone()).unwrap();
            }
            assert_eq!(
                serde_json::to_value(ready_facts(
                    &fixture,
                    &root,
                    &CandidateBounds::default(),
                    bits,
                ))
                .unwrap(),
                serde_json::to_value(expected).unwrap(),
                "source mask {bits}"
            );
            for (rank, entry) in selected.iter().enumerate() {
                let result = fixture
                    .store
                    .candidate_tree_select(
                        &fixture.job,
                        &root,
                        rank as u64,
                        bits,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(result.outcome, CandidateReadOutcome::Qualified);
                assert_eq!(result.value.as_ref().unwrap().key, entry.key);
                assert!(result.work.sql_reads > 0 && result.work.vm_steps > 0);
            }
            assert!(
                fixture
                    .store
                    .candidate_tree_select(
                        &fixture.job,
                        &root,
                        selected.len() as u64,
                        bits,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap()
                    .value
                    .is_none()
            );
            let lower = selected.get(selected.len() / 3).map(|entry| entry.key);
            let upper = selected
                .get(selected.len().saturating_mul(2) / 3)
                .map(|entry| entry.key);
            let bounds = CandidateBounds { lower, upper };
            let expected_range = selected
                .iter()
                .filter(|entry| {
                    bounds
                        .lower
                        .as_ref()
                        .is_none_or(|lower| entry.key >= *lower)
                        && bounds
                            .upper
                            .as_ref()
                            .is_none_or(|upper| entry.key <= *upper)
                })
                .fold(Facts::default(), |mut total, entry| {
                    total.append(entry.facts.clone()).unwrap();
                    total
                });
            assert_eq!(
                serde_json::to_value(ready_facts(&fixture, &root, &bounds, bits)).unwrap(),
                serde_json::to_value(expected_range).unwrap()
            );
            if let Some(first) = selected.first() {
                let forward = fixture
                    .store
                    .candidate_tree_seek(
                        &fixture.job,
                        &root,
                        &first.key,
                        CandidateDirection::Forward,
                        bits,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(forward.value.unwrap().key, first.key);
                let last = selected.last().unwrap();
                let backward = fixture
                    .store
                    .candidate_tree_seek(
                        &fixture.job,
                        &root,
                        &last.key,
                        CandidateDirection::Backward,
                        bits,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(backward.value.unwrap().key, last.key);
            }
        }
        let empty_result = fixture
            .store
            .candidate_tree_select(
                &fixture.job,
                &empty,
                0,
                u64::MAX,
                &facts_count,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(empty_result.outcome, CandidateReadOutcome::Qualified);
        assert!(empty_result.value.is_none());
        let baseline = fixture
            .store
            .candidate_tree_select(
                &fixture.job,
                &root,
                0,
                u64::MAX,
                &facts_count,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(baseline.outcome, CandidateReadOutcome::Qualified);
        let closure_calls = Cell::new(0);
        let changing_closure = |db: &Connection, job: i64| {
            let source_count: i64 = db.query_row(
                "SELECT count(*) FROM native_query_sources WHERE job=?1",
                [job],
                |row| row.get(0),
            )?;
            assert!(source_count > 0);
            closure_calls.set(closure_calls.get() + 1);
            Ok(closure_calls.get() == 1)
        };
        let changed = fixture
            .store
            .candidate_tree_select(
                &fixture.job,
                &root,
                0,
                u64::MAX,
                &facts_count,
                &changing_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(changed.outcome, CandidateReadOutcome::Conflict);
        assert_eq!(closure_calls.get(), 2);
        assert!(changed.work.sqlite_vm_steps > baseline.work.sqlite_vm_steps);
    }

    #[test]
    fn candidate_tree_payload_preserves_old_encoding_and_existing_cell_caps() {
        let mut fixture = Fixture::new();
        let first = test_entry(0, 10, 0);
        let mut leaf = fixture.graph_leaf(&first, 0);
        let original = CandidateNode::Leaf {
            key: first.key.to_vec(),
            candidate: first.candidate,
            aggregate: leaf.aggregate.clone(),
            payload: None,
        };
        let encoded = serde_json::to_string(&original).unwrap();
        assert!(!encoded.contains("payload"));
        let old: CandidateNode = serde_json::from_str(&encoded).unwrap();
        assert_eq!(serde_json::to_string(&old).unwrap(), encoded);
        let root = CandidateRoot::empty(fixture.empty.clone());
        let before = fixture.job.clone();
        leaf.payload = Some("x".repeat(NODE_BYTES));
        assert!(
            fixture
                .store
                .candidate_tree_begin_update(
                    &fixture.job,
                    "oversized-point-payload",
                    &root,
                    first.key,
                    Some(leaf),
                    "facts-v1",
                    &valid_closure,
                    &|| false,
                )
                .is_err()
        );
        fixture.refresh();
        assert_eq!(fixture.job.revision, before.revision);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_rank_candidate_cursors WHERE job=?1",
                    [fixture.job.id],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn candidate_tree_cursor_reopens_and_imported_root_survives_predecessor_cleanup() {
        let mut fixture = Fixture::new();
        let mut root = CandidateRoot::empty(fixture.empty.clone());
        let first = test_entry(0, 10, 0);
        let mut first_leaf = fixture.graph_leaf(&first, 0);
        let point_payload = r#"{"version":1,"address":{"extra":false,"seq":0},"native_kind":"A"}"#;
        first_leaf.payload = Some(point_payload.into());
        root = fixture
            .update(&root, first.key, Some(first_leaf), "facts-v1")
            .0;

        let second = test_entry(1, 11, 0);
        let full_guard: Vec<_> = (0..8)
            .map(|source| (source, second.seq & (1 << source) != 0))
            .collect();
        let second_leaf = fixture.graph_leaf_with_guard(&second, 0, &full_guard);
        let version = "facts-eight-source-v1";
        let combine = combine_facts;
        let stale_job = fixture.job.clone();
        fixture.cursor += 1;
        let cursor = format!("resume-{}", fixture.cursor);
        let started = fixture
            .store
            .candidate_tree_begin_update(
                &fixture.job,
                &cursor,
                &root,
                second.key,
                Some(second_leaf),
                version,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(started.outcome, Outcome::Written);
        check_step(&started.work);
        fixture.refresh();
        let stale = fixture
            .store
            .candidate_tree_advance_update(
                &stale_job,
                &cursor,
                version,
                &combine,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(stale.outcome, Outcome::Conflict);

        let partial = fixture
            .store
            .candidate_tree_advance_update(
                &fixture.job,
                &cursor,
                version,
                &combine,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(partial.outcome, Outcome::Written);
        assert!(
            partial.complete.is_none(),
            "the eight-source Apply should suspend"
        );
        check_step(&partial.work);
        fixture.refresh();
        let original_spec = fixture.spec.clone();
        fixture.job = fixture.reopen(&original_spec);
        let root = loop {
            let step = fixture
                .store
                .candidate_tree_advance_update(
                    &fixture.job,
                    &cursor,
                    version,
                    &combine,
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            check_step(&step.work);
            fixture.refresh();
            if let Some(root) = step.complete {
                break root;
            }
        };

        let nodes_before: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_rank_candidate_nodes",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let (next_spec, _) = fixture.new_job("chosen:candidate-tree-import");
        let third = test_entry(2, 12, 0);
        let third_leaf = fixture.graph_leaf(&third, 0);
        let mut next_job = fixture.store.reload(&next_spec.view_key).unwrap().unwrap();
        let cursor = "imported-root-update";
        let (start, allocations, allocated_bytes) = measure_test_allocations(|| {
            fixture
                .store
                .candidate_tree_begin_update(
                    &next_job,
                    cursor,
                    &root,
                    third.key,
                    Some(third_leaf),
                    "facts-v1",
                    &valid_closure,
                    &|| false,
                )
                .unwrap()
        });
        check_allocations(allocations, allocated_bytes);
        assert_eq!(start.outcome, Outcome::Written);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_rank_candidate_nodes",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            nodes_before,
            "root import must add one reference without cloning the tree"
        );
        assert_eq!(
            fixture.store.restart(&fixture.job).unwrap(),
            Outcome::Written
        );
        let mut cleaned = false;
        for _ in 0..4096 {
            let old = fixture
                .store
                .reload(&original_spec.view_key)
                .unwrap()
                .unwrap();
            let before_rows = private_cleanup_row_count(&fixture.store.connection);
            match fixture.store.cleanup(&old).unwrap() {
                Outcome::Written => {
                    cleaned = true;
                    break;
                }
                Outcome::Deferred => {
                    let after_rows = private_cleanup_row_count(&fixture.store.connection);
                    assert!(
                        after_rows < before_rows,
                        "old-job cleanup deferred without deleting private rows"
                    );
                }
                outcome => panic!("unexpected cleanup outcome {outcome:?}"),
            }
        }
        assert!(
            cleaned,
            "old-job cleanup exceeded its bounded progress loop"
        );
        let retained: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_rank_candidate_nodes",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(retained > 0);
        next_job = fixture.store.reload(&next_spec.view_key).unwrap().unwrap();
        let mut imported_root = None;
        for _ in 0..2000 {
            let (step, allocations, allocated_bytes) = measure_test_allocations(|| {
                fixture
                    .store
                    .candidate_tree_advance_update(
                        &next_job,
                        cursor,
                        "facts-v1",
                        &combine,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap()
            });
            check_allocations(allocations, allocated_bytes);
            assert_eq!(step.outcome, Outcome::Written);
            check_step(&step.work);
            assert!(step.work.sqlite_vm_steps > 0);
            next_job = fixture.store.reload(&next_spec.view_key).unwrap().unwrap();
            if let Some(root) = step.complete {
                imported_root = Some(root);
                break;
            }
        }
        let imported_root = imported_root.expect("imported-root update exceeded its step bound");
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_query_jobs SET phase='ready',revision=revision+1 WHERE job=?1 AND revision=?2",
                params![next_job.id, next_job.revision],
            )
            .unwrap();
        let ready = fixture.store.reload(&next_spec.view_key).unwrap().unwrap();
        let result = fixture
            .store
            .candidate_tree_select(
                &ready,
                &imported_root,
                0,
                u64::MAX,
                &facts_count,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(result.outcome, CandidateReadOutcome::Qualified);
        let point = result.value.unwrap();
        assert_eq!(point.key, first.key);
        assert_eq!(point.leaf.payload.as_deref(), Some(point_payload));
        let seek = fixture
            .store
            .candidate_tree_seek(
                &ready,
                &imported_root,
                &first.key,
                CandidateDirection::Forward,
                u64::MAX,
                &facts_count,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(seek.outcome, CandidateReadOutcome::Qualified);
        assert_eq!(
            seek.value.unwrap().leaf.payload.as_deref(),
            Some(point_payload)
        );
    }

    #[test]
    fn candidate_tree_cancellation_and_failed_writes_are_atomic_and_corruption_refuses_reads() {
        let mut fixture = Fixture::new();
        let mut root = CandidateRoot::empty(fixture.empty.clone());
        let first = test_entry(20, 20, 0);
        let first_leaf = fixture.graph_leaf(&first, 0);
        root = fixture
            .update(&root, first.key, Some(first_leaf), "facts-v1")
            .0;
        let second = test_entry(21, 21, 0);
        let second_leaf = fixture.graph_leaf(&second, 0);
        fixture.cursor += 1;
        let cursor = format!("atomic-{}", fixture.cursor);
        let before = fixture.job.clone();

        let calls = Cell::new(0);
        let cancelled_after_revision_cas = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        let deferred = fixture
            .store
            .candidate_tree_bind_root(
                &fixture.job,
                &root,
                &valid_closure,
                &cancelled_after_revision_cas,
            )
            .unwrap();
        assert_eq!(deferred.0, Outcome::Deferred);
        assert!(deferred.1.sql_reads + deferred.1.sql_writes > 0);
        assert!(deferred.1.sqlite_vm_steps > 0);
        assert_eq!(
            fixture
                .store
                .reload(&fixture.spec.view_key)
                .unwrap()
                .unwrap()
                .revision,
            before.revision,
            "cancellation after the revision CAS must roll the transaction back"
        );

        let calls = Cell::new(0);
        let cancelled_after_begin_write = || {
            calls.set(calls.get() + 1);
            calls.get() == 2
        };
        let deferred = fixture
            .store
            .candidate_tree_begin_update(
                &fixture.job,
                &cursor,
                &root,
                second.key,
                Some(second_leaf.clone()),
                "facts-v1",
                &valid_closure,
                &cancelled_after_begin_write,
            )
            .unwrap();
        assert_eq!(deferred.outcome, Outcome::Deferred);
        assert!(deferred.work.sql_reads + deferred.work.sql_writes > 0);
        assert!(deferred.work.sqlite_vm_steps > 0);
        assert_eq!(
            fixture
                .store
                .reload(&fixture.spec.view_key)
                .unwrap()
                .unwrap()
                .revision,
            before.revision
        );
        assert!(!fixture.store.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_rank_candidate_cursors WHERE job=?1 AND cursor=?2)",
            params![before.id, cursor],
            |r| r.get::<_, bool>(0),
        ).unwrap());

        let start = fixture
            .store
            .candidate_tree_begin_update(
                &fixture.job,
                &cursor,
                &root,
                second.key,
                Some(second_leaf),
                "facts-v1",
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(start.outcome, Outcome::Written);
        fixture.refresh();
        let at_cursor = fixture.job.clone();
        let calls = Cell::new(0);
        let cancel_during_repair = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        let deferred = fixture
            .store
            .candidate_tree_advance_update(
                &fixture.job,
                &cursor,
                "facts-v1",
                &combine_facts,
                &valid_closure,
                &cancel_during_repair,
            )
            .unwrap();
        assert_eq!(deferred.outcome, Outcome::Deferred);
        assert!(deferred.work.sql_reads + deferred.work.sql_writes > 0);
        assert!(deferred.work.sqlite_vm_steps > 0);
        assert_eq!(
            fixture
                .store
                .reload(&fixture.spec.view_key)
                .unwrap()
                .unwrap()
                .revision,
            at_cursor.revision
        );
        fixture.refresh();

        fixture.store.connection.execute_batch(
            "CREATE TEMP TRIGGER candidate_tree_fail BEFORE INSERT ON native_rank_candidate_nodes BEGIN SELECT RAISE(ABORT,'injected candidate tree write'); END",
        ).unwrap();
        assert!(
            fixture
                .store
                .candidate_tree_advance_update(
                    &fixture.job,
                    &cursor,
                    "facts-v1",
                    &combine_facts,
                    &valid_closure,
                    &|| false,
                )
                .is_err()
        );
        fixture
            .store
            .connection
            .execute_batch("DROP TRIGGER candidate_tree_fail")
            .unwrap();
        assert_eq!(
            fixture
                .store
                .reload(&fixture.spec.view_key)
                .unwrap()
                .unwrap()
                .revision,
            at_cursor.revision
        );
        fixture.refresh();

        let root = loop {
            let step = fixture
                .store
                .candidate_tree_advance_update(
                    &fixture.job,
                    &cursor,
                    "facts-v1",
                    &combine_facts,
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            fixture.refresh();
            if let Some(root) = step.complete {
                break root;
            }
        };
        fixture.ready();
        let identity = root.tree.as_ref().unwrap().identity();
        let original: String = fixture
            .store
            .connection
            .query_row(
                "SELECT metadata FROM native_rank_candidate_nodes WHERE identity=?1",
                [&identity],
                |r| r.get(0),
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_rank_candidate_nodes SET metadata='{}' WHERE identity=?1",
                [&identity],
            )
            .unwrap();
        assert!(
            fixture
                .store
                .candidate_tree_range(
                    &fixture.job,
                    &root,
                    &CandidateBounds::default(),
                    u64::MAX,
                    &combine_facts,
                    &valid_closure,
                    &|| false,
                )
                .is_err()
        );
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_rank_candidate_nodes SET metadata=?2 WHERE identity=?1",
                params![identity, original],
            )
            .unwrap();
        let child: String = fixture
            .store
            .connection
            .query_row(
                "SELECT left_id FROM native_rank_candidate_nodes WHERE identity=?1",
                [&identity],
                |r| r.get(0),
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "DELETE FROM native_rank_candidate_nodes WHERE identity=?1",
                [&child],
            )
            .unwrap();
        assert!(
            fixture
                .store
                .candidate_tree_select(
                    &fixture.job,
                    &root,
                    0,
                    u64::MAX,
                    &facts_count,
                    &valid_closure,
                    &|| false,
                )
                .is_err()
        );
    }

    #[test]
    fn candidate_tree_wal_peer_changes_are_rechecked_after_snapshot_release() {
        for operation in ["select", "range", "seek"] {
            for mutation in ["unchanged", "current", "owner", "job", "related"] {
                let mut fixture = Fixture::new();
                let entry = test_entry(0, 0, 0);
                let leaf = fixture.graph_leaf_with_guard(&entry, 0, &[]);
                let (root, _) = fixture.update(
                    &CandidateRoot::empty(fixture.empty.clone()),
                    entry.key,
                    Some(leaf),
                    "facts-v1",
                );
                fixture.ready();
                fixture
                    .store
                    .connection
                    .execute_batch("PRAGMA journal_mode=WAL")
                    .unwrap();
                fixture
                    .store
                    .connection
                    .execute(
                        "INSERT INTO meta(key,value) VALUES('candidate_related','old')",
                        [],
                    )
                    .unwrap();
                let peer = Connection::open(EventCache::path(&fixture.options.cache)).unwrap();
                let peer_mode: String = peer
                    .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(peer_mode.to_ascii_lowercase(), "wal");
                let calls = Cell::new(0);
                let snapshot_checks = Cell::new(0);
                let callback_vm = Cell::new(0u64);
                let closure = |db: &Connection, id: i64| -> rusqlite::Result<bool> {
                    calls.set(calls.get() + 1);
                    let current = native_query::job_current(db, &fixture.job)?;
                    let mut q =
                        db.prepare("SELECT value='old' FROM meta WHERE key='candidate_related'")?;
                    q.reset_status(rusqlite::StatementStatus::VmStep);
                    let related: bool = q.query_row([], |row| row.get(0))?;
                    callback_vm.set(
                        callback_vm.get() + q.get_status(rusqlite::StatementStatus::VmStep) as u64,
                    );
                    drop(q);
                    if calls.get() <= 2 {
                        assert!(
                            current && related,
                            "the original SQLite snapshot should remain coherent"
                        );
                        snapshot_checks.set(snapshot_checks.get() + 1);
                    }
                    if calls.get() == 1 {
                        match mutation {
                            "current" => {
                                peer.execute(
                                    "DELETE FROM session_catalog WHERE session_key=?1",
                                    [&fixture.row.key],
                                )?;
                            }
                            "owner" => {
                                peer.execute("INSERT INTO native_catalog_owner_versions(session_key,scope,generation) VALUES(?1,'current',1) ON CONFLICT(session_key,scope) DO UPDATE SET generation=generation+1", [&fixture.row.key])?;
                            }
                            "job" => {
                                peer.execute(
                                    "UPDATE native_query_jobs SET revision=revision+1 WHERE job=?1",
                                    [id],
                                )?;
                            }
                            "related" => {
                                peer.execute(
                                    "UPDATE meta SET value='new' WHERE key='candidate_related'",
                                    [],
                                )?;
                            }
                            "unchanged" => {}
                            _ => unreachable!(),
                        }
                    }
                    // Both in-transaction callbacks deliberately return true,
                    // despite the WAL peer's committed generation replacement.
                    Ok(current && related)
                };
                let (outcome, returned, work) = match operation {
                    "select" => {
                        let read = fixture
                            .store
                            .candidate_tree_select(
                                &fixture.job,
                                &root,
                                0,
                                u64::MAX,
                                &facts_count,
                                &closure,
                                &|| false,
                            )
                            .unwrap();
                        (read.outcome, read.value.is_some(), read.work)
                    }
                    "range" => {
                        let read = fixture
                            .store
                            .candidate_tree_range(
                                &fixture.job,
                                &root,
                                &CandidateBounds::default(),
                                u64::MAX,
                                &combine_facts,
                                &closure,
                                &|| false,
                            )
                            .unwrap();
                        (read.outcome, read.value.is_some(), read.work)
                    }
                    "seek" => {
                        let read = fixture
                            .store
                            .candidate_tree_seek(
                                &fixture.job,
                                &root,
                                &entry.key,
                                CandidateDirection::Forward,
                                u64::MAX,
                                &facts_count,
                                &closure,
                                &|| false,
                            )
                            .unwrap();
                        (read.outcome, read.value.is_some(), read.work)
                    }
                    _ => unreachable!(),
                };
                assert_eq!(snapshot_checks.get(), 2);
                assert!(work.sql_reads > 0, "refusal lost attempted read work");
                assert!(
                    work.sqlite_vm_steps >= work.vm_steps as u64 + callback_vm.get(),
                    "the progress hook omitted callback or post-snapshot SQL"
                );
                if mutation == "unchanged" {
                    assert_eq!(outcome, CandidateReadOutcome::Qualified);
                    assert!(returned);
                    assert_eq!(calls.get(), 3, "external closure was not rechecked");
                } else {
                    assert_eq!(
                        outcome,
                        CandidateReadOutcome::Conflict,
                        "{operation}/{mutation}"
                    );
                    assert!(
                        !returned,
                        "{operation}/{mutation} returned old-generation data"
                    );
                    assert!(!fixture.store.recheck(&fixture.job).unwrap() || mutation == "related");
                }
            }
        }
    }

    #[test]
    fn candidate_tree_meter_includes_post_snapshot_closure_sql() {
        let mut fixture = Fixture::new();
        let entry = test_entry(0, 0, 0);
        let leaf = fixture.graph_leaf_with_guard(&entry, 0, &[]);
        let (root, _) = fixture.update(
            &CandidateRoot::empty(fixture.empty.clone()),
            entry.key,
            Some(leaf),
            "facts-v1",
        );
        fixture.ready();
        let mut receipts = Vec::new();
        let external_vm = Cell::new(0u64);
        for extra_sql in [false, true] {
            let calls = Cell::new(0);
            let closure = |db: &Connection, _: i64| -> rusqlite::Result<bool> {
                calls.set(calls.get() + 1);
                if extra_sql && calls.get() == 3 {
                    // Fixed relationship-context stand-in executed only after
                    // release; its actual VM receipt is the comparison oracle.
                    let mut q = db.prepare("WITH RECURSIVE bounded(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM bounded WHERE n<127) SELECT sum(n) FROM bounded")?;
                    q.reset_status(rusqlite::StatementStatus::VmStep);
                    assert_eq!(q.query_row([], |row| row.get::<_, i64>(0))?, 8128);
                    external_vm.set(q.get_status(rusqlite::StatementStatus::VmStep) as u64);
                }
                Ok(true)
            };
            let read = fixture
                .store
                .candidate_tree_select(
                    &fixture.job,
                    &root,
                    0,
                    u64::MAX,
                    &facts_count,
                    &closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(read.outcome, CandidateReadOutcome::Qualified);
            assert_eq!(calls.get(), 3);
            receipts.push(read.work);
        }
        assert_eq!(receipts[0].sql_reads, receipts[1].sql_reads);
        assert_eq!(receipts[0].vm_steps, receipts[1].vm_steps);
        assert!(external_vm.get() > 0);
        assert!(
            receipts[1].sqlite_vm_steps >= receipts[0].sqlite_vm_steps + external_vm.get(),
            "post-snapshot closure SQL was omitted from the complete operation receipt"
        );
    }

    #[test]
    fn candidate_tree_post_snapshot_callback_cancellation_is_latched() {
        let mut fixture = Fixture::new();
        let entry = test_entry(0, 0, 0);
        let leaf = fixture.graph_leaf_with_guard(&entry, 0, &[]);
        let (root, _) = fixture.update(
            &CandidateRoot::empty(fixture.empty.clone()),
            entry.key,
            Some(leaf),
            "facts-v1",
        );
        fixture.ready();
        let calls = Cell::new(0);
        let cancellation = Cell::new(false);
        let closure = |_: &Connection, _: i64| -> rusqlite::Result<bool> {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                cancellation.set(true);
            }
            Ok(true)
        };
        let result = fixture
            .store
            .candidate_tree_select(
                &fixture.job,
                &root,
                0,
                u64::MAX,
                &facts_count,
                &closure,
                &|| cancellation.replace(false),
            )
            .unwrap();
        assert_eq!(calls.get(), 3);
        assert_eq!(result.outcome, CandidateReadOutcome::Deferred);
        assert!(result.value.is_none());
        assert!(result.work.sql_reads > 0 && result.work.sqlite_vm_steps > 0);
    }

    #[test]
    fn candidate_tree_guards_every_materialized_node_column() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let aggregate = NodeId {
            level: 64,
            digest: "0".repeat(64),
        };
        let node = CandidateNode::Leaf {
            key: vec![0; KEY_BYTES],
            candidate: [0; 32],
            aggregate: aggregate.clone(),
            payload: None,
        };
        let metadata = encoded(&node, NODE_BYTES).unwrap();
        let id = candidate_id(&metadata);
        assert_eq!(id.identity().len(), 77);
        assert_eq!(aggregate.identity().len(), 80);
        let huge = "x".repeat(1024 * 1024);
        for column in ["metadata", "left_id", "right_id", "aggregate"] {
            db.execute("DELETE FROM native_rank_candidate_nodes", [])
                .unwrap();
            db.execute(
                "INSERT INTO native_rank_candidate_nodes VALUES(?1,?2,NULL,NULL,?3,0)",
                params![id.identity(), metadata, aggregate.identity()],
            )
            .unwrap();
            let mut tree = TreeDb {
                db: &db,
                job: 1,
                work: CandidateWork::default(),
            };
            assert!(
                tree.read_node(&id).is_ok(),
                "NULL children are legitimate leaf absence"
            );
            if matches!(column, "left_id" | "right_id") {
                db.execute(
                    "UPDATE native_rank_candidate_nodes SET left_id=?1,right_id=?1",
                    [id.identity()],
                )
                .unwrap();
            }
            db.execute(
                &format!("UPDATE native_rank_candidate_nodes SET {column}=?1"),
                [&huge],
            )
            .unwrap();
            let (result, _, bytes) = measure_test_allocations(|| tree.read_node(&id));
            assert!(result.is_err(), "{column} was admitted");
            assert!(bytes < 128 * 1024, "{column} allocated {bytes} Rust bytes");
            assert!(measured_peak_bytes() < 128 * 1024);
        }
    }

    #[test]
    fn candidate_tree_cleanup_guards_all_addresses_before_decoding() {
        let huge = "x".repeat(1024 * 1024);
        let tree_id = CandidateNodeId {
            digest: "0".repeat(64),
        }
        .identity();
        let graph_id = NodeId {
            level: 64,
            digest: "0".repeat(64),
        }
        .identity();
        for (table, column) in [
            ("applies", "cursor"),
            ("cursors", "cursor"),
            ("roots", "identity"),
            ("roots", "empty"),
            ("bindings", "identity"),
            ("nodes", "identity"),
            ("nodes", "metadata"),
            ("nodes", "left_id"),
            ("nodes", "right_id"),
            ("nodes", "aggregate"),
        ] {
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch(SCHEMA).unwrap();
            match table {
                "applies" | "cursors" => {
                    db.execute(
                        &format!(
                            "INSERT INTO native_rank_candidate_{table} VALUES(1,'cursor','{{}}')"
                        ),
                        [],
                    )
                    .unwrap();
                }
                "roots" => {
                    db.execute(
                        "INSERT INTO native_rank_candidate_roots VALUES(1,?1,?2)",
                        params![tree_id, graph_id],
                    )
                    .unwrap();
                }
                "bindings" => {
                    db.execute(
                        "INSERT INTO native_rank_candidate_bindings VALUES(1,?1)",
                        [&tree_id],
                    )
                    .unwrap();
                }
                "nodes" => {
                    db.execute(
                        "INSERT INTO native_rank_candidate_nodes VALUES(?1,'{}',?1,?1,?2,0)",
                        params![tree_id, graph_id],
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            db.execute(
                &format!("UPDATE native_rank_candidate_{table} SET {column}=?1"),
                [&huge],
            )
            .unwrap();
            let mut remaining_bytes = usize::MAX;
            let (result, _, allocated) =
                measure_test_allocations(|| cleanup(&db, 1, 32, &mut remaining_bytes));
            assert!(result.is_err(), "{table}.{column} was admitted");
            assert!(
                allocated < 128 * 1024,
                "{table}.{column} allocated {allocated} Rust bytes"
            );
            assert!(measured_peak_bytes() < 128 * 1024);
        }
    }

    #[derive(Clone)]
    struct BulkItem {
        key: [u8; KEY_BYTES],
        candidate: CandidateRef,
        aggregate: NodeId,
    }

    // Fixture construction is deliberately outside the measured query. It
    // writes the same durable SQLite nodes and graph aggregates as production;
    // it is not a production whole-tree preparation path.
    fn bulk_root(
        tx: &Transaction<'_>,
        job: i64,
        items: &[BulkItem],
        empty: &NodeId,
    ) -> io::Result<CandidateRoot> {
        let mut tree = TreeDb {
            db: tx,
            job,
            work: CandidateWork::default(),
        };
        let mut graph = Graph {
            db: tx,
            job,
            work: super::super::Work::default(),
        };
        let root = if items.is_empty() {
            None
        } else {
            Some(bulk_node(&mut tree, &mut graph, items, empty)?)
        };
        Ok(CandidateRoot {
            tree: root,
            empty: empty.clone(),
        })
    }

    fn bulk_node(
        tree: &mut TreeDb<'_>,
        graph: &mut Graph<'_>,
        items: &[BulkItem],
        empty: &NodeId,
    ) -> io::Result<CandidateNodeId> {
        if items.len() == 1 {
            return tree.intern(
                CandidateNode::Leaf {
                    key: items[0].key.to_vec(),
                    candidate: items[0].candidate,
                    aggregate: items[0].aggregate.clone(),
                    payload: None,
                },
                graph,
            );
        }
        if items.len() < 2 {
            return Err(invalid("bulk candidate fixture has no key"));
        }
        let split = first_difference(&items[0].key, &items[items.len() - 1].key)
            .ok_or_else(|| invalid("bulk candidate fixture repeats a key"))?;
        let pivot =
            items.partition_point(|item| key_bit(&item.key, usize::from(split)) == Some(false));
        if pivot == 0 || pivot == items.len() {
            return Err(invalid("bulk candidate fixture split is reversed"));
        }
        let left = bulk_node(tree, graph, &items[..pivot], empty)?;
        let right = bulk_node(tree, graph, &items[pivot..], empty)?;
        let left_node = tree.read_node(&left)?;
        let right_node = tree.read_node(&right)?;
        let mut apply = Apply {
            version: "candidate-tree-bulk-fixture-v1".into(),
            identity: empty.clone(),
            frames: vec![Frame {
                pair: Pair {
                    left: left_node.aggregate().clone(),
                    right: right_node.aggregate().clone(),
                },
                phase: Phase::Visit,
            }],
            returned: None,
        };
        while !apply.frames.is_empty() {
            graph.work = super::super::Work::default();
            graph.advance(&mut apply, &combine_facts)?;
            if graph.work.rows() == 0 {
                return Err(invalid("bulk candidate fixture Apply made no progress"));
            }
        }
        let aggregate = apply
            .returned
            .ok_or_else(|| invalid("bulk candidate fixture aggregate is absent"))?;
        tree.intern(
            CandidateNode::Branch {
                split,
                left,
                right,
                min: left_node.min().to_vec(),
                max: right_node.max().to_vec(),
                aggregate,
            },
            graph,
        )
    }

    #[test]
    fn candidate_tree_one_point_queries_stay_indexed_at_history_prefixes() {
        one_point_history_prefixes(false);
    }

    #[test]
    fn candidate_tree_full_graph_queries_stay_indexed_at_history_prefixes() {
        one_point_history_prefixes(true);
    }

    fn one_point_history_prefixes(full_graph_guard: bool) {
        let included_sources = if full_graph_guard { 0 } else { u64::MAX };
        let mut fixture = Fixture::new();
        let sizes = [0usize, 128, 1024, 8192];
        let mut items: Vec<_> = (0..8192usize)
            .map(|seq| {
                let candidate: CandidateRef = Sha256::digest(seq.to_le_bytes()).into();
                BulkItem {
                    key: order(seq, seq as i64, candidate).encoded(),
                    candidate,
                    aggregate: fixture.empty.clone(),
                }
            })
            .collect();
        // All leaves carry one native count; branches are built from the
        // model's ordered Facts combine using the real graph Apply store.
        let one_text = serde_json::to_string(&Facts {
            count: 1,
            ..Facts::default()
        })
        .unwrap();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        let mut graph = Graph {
            db: &tx,
            job: fixture.job.id,
            work: super::super::Work::default(),
        };
        let mut one = graph.intern(Node::Terminal { facts: one_text }).unwrap();
        if full_graph_guard {
            for source in (0..64).rev() {
                one = graph
                    .intern(Node::Decision {
                        source,
                        absent: one,
                        present: fixture.empty.clone(),
                    })
                    .unwrap();
            }
        }
        drop(graph);
        for item in &mut items {
            item.aggregate = one.clone();
        }
        let roots = sizes
            .iter()
            .map(|size| bulk_root(&tx, fixture.job.id, &items[..*size], &fixture.empty).unwrap())
            .collect::<Vec<_>>();
        tx.commit().unwrap();
        for root in &roots {
            let ((outcome, work), allocations, allocated_bytes) = measure_test_allocations(|| {
                fixture
                    .store
                    .candidate_tree_bind_root(&fixture.job, root, &valid_closure, &|| false)
                    .unwrap()
            });
            check_allocations(allocations, allocated_bytes);
            assert_eq!(outcome, Outcome::Written);
            assert!(work.sql_writes > 0 && work.sqlite_vm_steps > 0);
            fixture.refresh();
        }

        let mut append_rows = Vec::new();
        let mut mutated_roots = Vec::new();
        for (root, size) in roots.iter().zip(sizes) {
            let seq = 50_000 + size;
            let appended = test_entry(seq, seq as i64, 0);
            let leaf = fixture.graph_leaf_with_guard(&appended, 0, &[]);
            let (appended_root, append_work) =
                fixture.update(root, appended.key, Some(leaf), "facts-v1");
            if size > 0 {
                assert!(append_work.apply_visits > 0);
            }
            assert!(append_work.sqlite_vm_steps > 0);
            append_rows.push(append_work.sql_reads + append_work.sql_writes);

            let replacement = test_entry(seq, seq as i64, 1);
            let leaf = fixture.graph_leaf_with_guard(&replacement, 1, &[]);
            let (replaced_root, replace_work) =
                fixture.update(&appended_root, replacement.key, Some(leaf), "facts-v1");
            if size > 0 {
                assert!(replace_work.apply_visits > 0);
            }

            let (deleted_root, delete_work) =
                fixture.update(&replaced_root, replacement.key, None, "facts-v1");
            assert!(delete_work.sql_reads + delete_work.sql_writes > 0);
            assert!(delete_work.sqlite_vm_steps > 0);
            mutated_roots.push((deleted_root, size));
        }
        assert!(append_rows[3] < append_rows[0] * 8 + 64);
        fixture.ready();

        for (root, size) in &mutated_roots {
            let (result, allocations, allocated_bytes) = measure_test_allocations(|| {
                fixture
                    .store
                    .candidate_tree_select(
                        &fixture.job,
                        root,
                        *size as u64,
                        included_sources,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap()
            });
            check_allocations(allocations, allocated_bytes);
            assert_eq!(result.outcome, CandidateReadOutcome::Qualified);
            assert!(result.value.is_none(), "deleted append remained selectable");
        }

        for (root, size) in roots.iter().zip(sizes) {
            if size == 0 {
                let (selected, allocations, allocated_bytes) = measure_test_allocations(|| {
                    fixture
                        .store
                        .candidate_tree_select(
                            &fixture.job,
                            root,
                            0,
                            included_sources,
                            &facts_count,
                            &valid_closure,
                            &|| false,
                        )
                        .unwrap()
                });
                check_allocations(allocations, allocated_bytes);
                assert_eq!(selected.outcome, CandidateReadOutcome::Qualified);
                assert!(selected.value.is_none());
                assert!(selected.work.sqlite_vm_steps > 0);
                let (range, allocations, allocated_bytes) = measure_test_allocations(|| {
                    fixture
                        .store
                        .candidate_tree_range(
                            &fixture.job,
                            root,
                            &CandidateBounds::default(),
                            included_sources,
                            &combine_facts,
                            &valid_closure,
                            &|| false,
                        )
                        .unwrap()
                });
                check_allocations(allocations, allocated_bytes);
                assert_eq!(range.outcome, CandidateReadOutcome::Qualified);
                assert_eq!(
                    serde_json::from_str::<Facts>(range.value.as_deref().unwrap())
                        .unwrap()
                        .count,
                    0
                );
                let (seek, allocations, allocated_bytes) = measure_test_allocations(|| {
                    fixture
                        .store
                        .candidate_tree_seek(
                            &fixture.job,
                            root,
                            &[0; KEY_BYTES],
                            CandidateDirection::Forward,
                            included_sources,
                            &facts_count,
                            &valid_closure,
                            &|| false,
                        )
                        .unwrap()
                });
                check_allocations(allocations, allocated_bytes);
                assert_eq!(seek.outcome, CandidateReadOutcome::Qualified);
                assert!(seek.value.is_none());
                continue;
            }
            let point = &items[size / 2];
            let (selected, select_allocations, select_allocated_bytes) =
                measure_test_allocations(|| {
                    fixture
                        .store
                        .candidate_tree_select(
                            &fixture.job,
                            root,
                            (size / 2) as u64,
                            included_sources,
                            &facts_count,
                            &valid_closure,
                            &|| false,
                        )
                        .unwrap()
                });
            check_allocations(select_allocations, select_allocated_bytes);
            assert_eq!(selected.outcome, CandidateReadOutcome::Qualified);
            assert_eq!(selected.value.as_ref().unwrap().key, point.key);
            assert!(
                selected.work.sql_reads > 0
                    && selected.work.vm_steps > 0
                    && selected.work.sqlite_vm_steps > 0
            );
            assert!(selected.work.sql_reads + selected.work.sql_writes < 8 * KEY_BITS);
            assert!(selected.work.max_depth <= MAX_DEPTH);
            assert!(selected.work.max_live_node_bytes < NODE_BYTES);

            let bounds = CandidateBounds {
                lower: Some(point.key),
                upper: Some(point.key),
            };
            let (range, range_allocations, range_allocated_bytes) =
                measure_test_allocations(|| {
                    fixture
                        .store
                        .candidate_tree_range(
                            &fixture.job,
                            root,
                            &bounds,
                            included_sources,
                            &combine_facts,
                            &valid_closure,
                            &|| false,
                        )
                        .unwrap()
                });
            check_allocations(range_allocations, range_allocated_bytes);
            let range_facts: Facts = serde_json::from_str(range.value.as_deref().unwrap()).unwrap();
            assert_eq!(range.outcome, CandidateReadOutcome::Qualified);
            assert_eq!(range_facts.count, 1);
            assert!(range.work.sql_reads + range.work.sql_writes < 16 * KEY_BITS);
            assert!(range.work.metadata_bytes < STEP_BYTES);
            assert!(range.work.sqlite_vm_steps > 0);
            assert!(range.work.max_live_node_bytes < NODE_BYTES);

            let mut seek_reads = 0;
            let mut seek_allocations = 0;
            let mut seek_allocated_bytes = 0;
            for direction in [CandidateDirection::Forward, CandidateDirection::Backward] {
                let (seek, allocations, allocated_bytes) = measure_test_allocations(|| {
                    fixture
                        .store
                        .candidate_tree_seek(
                            &fixture.job,
                            root,
                            &point.key,
                            direction,
                            included_sources,
                            &facts_count,
                            &valid_closure,
                            &|| false,
                        )
                        .unwrap()
                });
                check_allocations(allocations, allocated_bytes);
                assert_eq!(seek.outcome, CandidateReadOutcome::Qualified);
                assert_eq!(seek.value.unwrap().key, point.key);
                assert!(seek.work.sql_reads + seek.work.sql_writes < 8 * KEY_BITS);
                assert!(seek.work.vm_steps > 0);
                assert!(seek.work.sqlite_vm_steps > 0);
                seek_allocations = seek_allocations.max(allocations);
                seek_allocated_bytes = seek_allocated_bytes.max(allocated_bytes);
                seek_reads += seek.work.sql_reads;
            }
            println!(
                "candidate_tree full_graph={full_graph_guard} prefix={size} select_reads={} select_vm={} sqlite_vm={} select_bytes={} range_reads={} seek_reads={} depth={} node_bytes={} rust_allocs(select/range/seek)={}/{}/{} rust_bytes={}/{}/{}",
                selected.work.sql_reads,
                selected.work.vm_steps,
                selected.work.sqlite_vm_steps,
                selected.work.metadata_bytes,
                range.work.sql_reads,
                seek_reads,
                range.work.max_depth,
                range.work.max_live_node_bytes,
                select_allocations,
                range_allocations,
                seek_allocations,
                select_allocated_bytes,
                range_allocated_bytes,
                seek_allocated_bytes,
            );
        }
    }

    #[test]
    fn candidate_tree_rejects_reversed_branch_bounds() {
        let fixture = Fixture::new();
        let mut left_key = [0u8; KEY_BYTES];
        left_key[0] = 0x80;
        let right_key = [0u8; KEY_BYTES];
        let left = CandidateNode::Leaf {
            key: left_key.to_vec(),
            candidate: [0; 32],
            aggregate: fixture.empty.clone(),
            payload: None,
        };
        let right = CandidateNode::Leaf {
            key: right_key.to_vec(),
            candidate: [0; 32],
            aggregate: fixture.empty.clone(),
            payload: None,
        };
        assert!(validate_children(0, &left, &right).is_err());
        let malformed = CandidateNode::Branch {
            split: 0,
            left: CandidateNodeId {
                digest: "0".repeat(64),
            },
            right: CandidateNodeId {
                digest: "1".repeat(64),
            },
            min: left_key.to_vec(),
            max: right_key.to_vec(),
            aggregate: fixture.empty.clone(),
        };
        assert!(!malformed.valid());
    }

    #[test]
    fn candidate_tree_deep_path_and_full_graph_guard_checkpoint_independently() {
        let mut fixture = Fixture::new();
        let decisions: Vec<_> = (0..64).map(|source| (source, false)).collect();
        let old_entry = test_entry(0, 0, 0);
        let old_leaf = fixture.graph_leaf_with_paged_guard(&old_entry, 0, &decisions);
        let mut keys = Vec::with_capacity(KEY_BITS + 1);
        keys.push([0; KEY_BYTES]);
        for bit in (0..KEY_BITS).rev() {
            let mut key = [0; KEY_BYTES];
            key[bit / 8] |= 1 << (7 - bit % 8);
            keys.push(key);
        }
        let items: Vec<_> = keys
            .into_iter()
            .map(|key| BulkItem {
                candidate: key[37..].try_into().unwrap(),
                key,
                aggregate: old_leaf.aggregate.clone(),
            })
            .collect();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        let root = bulk_root(&tx, fixture.job.id, &items, &fixture.empty).unwrap();
        tx.commit().unwrap();
        let ((outcome, bind_work), allocations, allocated_bytes) = measure_test_allocations(|| {
            fixture
                .store
                .candidate_tree_bind_root(&fixture.job, &root, &valid_closure, &|| false)
                .unwrap()
        });
        check_allocations(allocations, allocated_bytes);
        assert_eq!(outcome, Outcome::Written);
        assert!(bind_work.sqlite_vm_steps > 0);
        fixture.refresh();

        let replacement_entry = test_entry(0, 0, 1);
        let replacement = fixture.graph_leaf_with_paged_guard(&replacement_entry, 1, &decisions);
        let key = [0; KEY_BYTES];
        let (start, allocations, allocated_bytes) = measure_test_allocations(|| {
            fixture
                .store
                .candidate_tree_begin_update(
                    &fixture.job,
                    "deepest-key-update",
                    &root,
                    key,
                    Some(CandidateLeaf {
                        candidate: [0; 32],
                        aggregate: replacement.aggregate,
                        payload: None,
                    }),
                    "deep-facts-v1",
                    &valid_closure,
                    &|| false,
                )
                .unwrap()
        });
        check_allocations(allocations, allocated_bytes);
        assert_eq!(start.outcome, Outcome::Written);
        check_step(&start.work);
        let mut max_cursor = start.work.max_cursor_bytes;
        let mut max_apply = start.work.max_apply_bytes;
        fixture.refresh();
        let mut checkpoint = None;
        let mut max_path_frames = 0;
        for _ in 0..2000 {
            let (step, allocations, allocated_bytes) = measure_test_allocations(|| {
                fixture
                    .store
                    .candidate_tree_advance_update(
                        &fixture.job,
                        "deepest-key-update",
                        "deep-facts-v1",
                        &combine_facts,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap()
            });
            check_allocations(allocations, allocated_bytes);
            assert_eq!(step.outcome, Outcome::Written);
            check_step(&step.work);
            max_cursor = max_cursor.max(step.work.max_cursor_bytes);
            max_apply = max_apply.max(step.work.max_apply_bytes);
            fixture.refresh();
            let cursor_text: String = fixture
                .store
                .connection
                .query_row(
                    "SELECT metadata FROM native_rank_candidate_cursors WHERE job=?1 AND cursor='deepest-key-update'",
                    [fixture.job.id],
                    |row| row.get(0),
                )
                .unwrap();
            let state: UpdateState = serde_json::from_str(&cursor_text).unwrap();
            max_path_frames = max_path_frames.max(state.path.len() / PATH_FRAME_BYTES);
            if state.path.len() / PATH_FRAME_BYTES == MAX_DEPTH && state.apply_active {
                let apply_text: String = fixture
                    .store
                    .connection
                    .query_row(
                        "SELECT metadata FROM native_rank_candidate_applies WHERE job=?1 AND cursor='deepest-key-update'",
                        [fixture.job.id],
                        |row| row.get(0),
                    )
                    .unwrap();
                let apply: Apply = serde_json::from_str(&apply_text).unwrap();
                assert!(
                    apply.frames.len() > 60,
                    "full64-source Apply was not checkpointed"
                );
                checkpoint = Some((cursor_text, apply_text));
                break;
            }
            assert!(
                step.complete.is_none(),
                "deep path completed before checkpoint"
            );
        }
        let (cursor_before_cancel, apply_before_cancel) =
            checkpoint.expect("deep path and full graph guard did not overlap");

        let spec = fixture.spec.clone();
        fixture.job = fixture.reopen(&spec);
        let revision = fixture.job.revision;
        let calls = Cell::new(0);
        let cancel_after_graph_work = || {
            calls.set(calls.get() + 1);
            calls.get() == 4
        };
        let (deferred, allocations, allocated_bytes) = measure_test_allocations(|| {
            fixture
                .store
                .candidate_tree_advance_update(
                    &fixture.job,
                    "deepest-key-update",
                    "deep-facts-v1",
                    &combine_facts,
                    &valid_closure,
                    &cancel_after_graph_work,
                )
                .unwrap()
        });
        check_allocations(allocations, allocated_bytes);
        assert_eq!(deferred.outcome, Outcome::Deferred);
        assert!(deferred.work.sql_reads + deferred.work.sql_writes > 0);
        assert!(deferred.work.sqlite_vm_steps > 0);
        assert!(calls.get() >= 4);
        fixture.refresh();
        assert_eq!(
            fixture.job.revision, revision,
            "cancelled checkpoint advanced revision"
        );
        let cursor_after_cancel: String = fixture
            .store
            .connection
            .query_row(
                "SELECT metadata FROM native_rank_candidate_cursors WHERE job=?1 AND cursor='deepest-key-update'",
                [fixture.job.id],
                |row| row.get(0),
            )
            .unwrap();
        let apply_after_cancel: String = fixture
            .store
            .connection
            .query_row(
                "SELECT metadata FROM native_rank_candidate_applies WHERE job=?1 AND cursor='deepest-key-update'",
                [fixture.job.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursor_after_cancel, cursor_before_cancel);
        assert_eq!(apply_after_cancel, apply_before_cancel);

        let mut complete = None;
        for _ in 0..2000 {
            let (step, allocations, allocated_bytes) = measure_test_allocations(|| {
                fixture
                    .store
                    .candidate_tree_advance_update(
                        &fixture.job,
                        "deepest-key-update",
                        "deep-facts-v1",
                        &combine_facts,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap()
            });
            check_allocations(allocations, allocated_bytes);
            assert_eq!(step.outcome, Outcome::Written);
            check_step(&step.work);
            max_cursor = max_cursor.max(step.work.max_cursor_bytes);
            max_apply = max_apply.max(step.work.max_apply_bytes);
            fixture.refresh();
            if step.complete.is_some() {
                complete = step.complete;
                break;
            }
        }
        let complete = complete.expect("deep update exceeded its bounded progress loop");
        assert_eq!(max_path_frames, MAX_DEPTH);
        assert!(max_cursor > 32 * 1024);
        assert!(max_cursor <= CURSOR_BYTES);
        assert!(max_apply > 10 * 1024 && max_apply <= CURSOR_BYTES);

        fixture.ready();
        let included = fixture
            .store
            .candidate_tree_select(
                &fixture.job,
                &complete,
                0,
                0,
                &facts_count,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(included.outcome, CandidateReadOutcome::Qualified);
        assert!(included.value.is_some());
        assert_eq!(included.value.as_ref().unwrap().key, [0; KEY_BYTES]);
        // Each selected path has at most 552 branches. Each branch evaluates
        // one left aggregate through at most 64 decisions + one terminal.
        // Include root evaluation, point rows, binding and empty validation.
        let maximum_reads = (MAX_DEPTH + 1) * (usize::from(super::super::VARIABLES) + 3) + 8;
        let check_read = |label: &str,
                          work: &CandidateWork,
                          allocations: usize,
                          allocated: usize| {
            assert!(
                work.sql_reads <= maximum_reads,
                "{label} repeated aggregate evaluation: {work:?}"
            );
            assert_eq!(work.sql_writes, 0);
            assert!(work.vm_steps > 0 && work.sqlite_vm_steps > 0);
            // All tested statements are indexed point lookups. The complete
            // hook also includes the fixed fixture's before/after witness SQL.
            assert!(work.sqlite_vm_steps <= work.vm_steps as u64 + 16_384);
            assert!(work.metadata_bytes <= maximum_reads * 4096);
            assert!(work.max_depth <= MAX_DEPTH);
            assert!(work.max_live_node_bytes <= NODE_BYTES);
            assert!(allocations > 0 && allocated <= maximum_reads * 16 * 1024);
            assert!(measured_peak_bytes() <= 2 * STEP_BYTES);
            println!(
                "candidate_tree deep_full_guard {label} reads={} explicit_vm={} all_sqlite_vm={} decoded_bytes={} rust_allocations={allocations} rust_cumulative_bytes={allocated} rust_heap_high_water={}",
                work.sql_reads,
                work.vm_steps,
                work.sqlite_vm_steps,
                work.metadata_bytes,
                measured_peak_bytes()
            );
        };
        let spec = fixture.spec.clone();
        fixture.job = fixture.reopen(&spec);
        let (selected, allocations, allocated) = measure_test_allocations(|| {
            fixture
                .store
                .candidate_tree_select(
                    &fixture.job,
                    &complete,
                    0,
                    0,
                    &facts_count,
                    &valid_closure,
                    &|| false,
                )
                .unwrap()
        });
        assert_eq!(selected.outcome, CandidateReadOutcome::Qualified);
        assert_eq!(selected.value.unwrap().key, [0; KEY_BYTES]);
        check_read("cold_select", &selected.work, allocations, allocated);
        let (range, allocations, allocated) = measure_test_allocations(|| {
            fixture
                .store
                .candidate_tree_range(
                    &fixture.job,
                    &complete,
                    &CandidateBounds {
                        lower: Some([0; KEY_BYTES]),
                        upper: Some([0; KEY_BYTES]),
                    },
                    0,
                    &combine_facts,
                    &valid_closure,
                    &|| false,
                )
                .unwrap()
        });
        assert_eq!(range.outcome, CandidateReadOutcome::Qualified);
        let expected = serde_json::to_value(&replacement_entry.facts).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(range.value.as_deref().unwrap()).unwrap(),
            expected
        );
        check_read("point_range", &range.work, allocations, allocated);
        for direction in [CandidateDirection::Forward, CandidateDirection::Backward] {
            let (seek, allocations, allocated) = measure_test_allocations(|| {
                fixture
                    .store
                    .candidate_tree_seek(
                        &fixture.job,
                        &complete,
                        &[0; KEY_BYTES],
                        direction,
                        0,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap()
            });
            assert_eq!(seek.outcome, CandidateReadOutcome::Qualified);
            assert_eq!(seek.value.unwrap().key, [0; KEY_BYTES]);
            check_read("point_seek", &seek.work, allocations, allocated);
        }
        let checks = Cell::new(0);
        let (cancelled_read, allocations, allocated) = measure_test_allocations(|| {
            fixture
                .store
                .candidate_tree_select(
                    &fixture.job,
                    &complete,
                    0,
                    0,
                    &facts_count,
                    &valid_closure,
                    &|| {
                        checks.set(checks.get() + 1);
                        checks.get() >= 8
                    },
                )
                .unwrap()
        });
        assert_eq!(cancelled_read.outcome, CandidateReadOutcome::Deferred);
        assert!(cancelled_read.value.is_none());
        check_read(
            "cancelled_select",
            &cancelled_read.work,
            allocations,
            allocated,
        );
        let fence_calls = Cell::new(0);
        let fenced = fixture
            .store
            .candidate_tree_select(
                &fixture.job,
                &complete,
                0,
                0,
                &facts_count,
                &|db, job| {
                    fence_calls.set(fence_calls.get() + 1);
                    Ok(valid_closure(db, job)? && fence_calls.get() == 1)
                },
                &|| false,
            )
            .unwrap();
        assert_eq!(fenced.outcome, CandidateReadOutcome::Conflict);
        assert!(fenced.value.is_none());
        assert!(fenced.work.sql_reads > 0 && fenced.work.sqlite_vm_steps > 0);

        let all_omitted = fixture
            .store
            .candidate_tree_select(
                &fixture.job,
                &complete,
                0,
                u64::MAX,
                &facts_count,
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(all_omitted.outcome, CandidateReadOutcome::Qualified);
        assert!(all_omitted.value.is_none());
    }
}

fn validate_children(split: u16, left: &CandidateNode, right: &CandidateNode) -> io::Result<()> {
    if left.max() >= right.min()
        || first_difference(left.max(), right.min()) != Some(split)
        || key_bit(left.max(), usize::from(split)) != Some(false)
        || key_bit(right.min(), usize::from(split)) != Some(true)
        || left.branch().is_some_and(|(child, _, _)| child <= split)
        || right.branch().is_some_and(|(child, _, _)| child <= split)
    {
        return Err(invalid("candidate tree branch ordering is invalid"));
    }
    Ok(())
}

impl NativeStore {
    /// Import one prior tree root and durably save a bounded update cursor.
    /// Both the tree root and its empty graph aggregate are retained before a
    /// previous producer job can be cleaned up.
    pub(crate) fn candidate_tree_begin_update(
        &mut self,
        job: &Job,
        cursor: &str,
        root: &CandidateRoot,
        key: [u8; KEY_BYTES],
        replacement: Option<CandidateLeaf>,
        combine_version: &str,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CandidateStep> {
        self.candidate_tree_begin_update_internal(
            job,
            cursor,
            root,
            key,
            replacement,
            combine_version,
            closure,
            cancelled,
            false,
        )
    }

    fn candidate_tree_begin_update_internal(
        &mut self,
        job: &Job,
        cursor: &str,
        root: &CandidateRoot,
        key: [u8; KEY_BYTES],
        replacement: Option<CandidateLeaf>,
        combine_version: &str,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        resume: bool,
    ) -> io::Result<CandidateStep> {
        cursor_name(cursor)?;
        if combine_version.is_empty() || combine_version.len() > 256 || !root_valid(root) {
            return Err(invalid("candidate tree update inputs are invalid"));
        }
        if replacement
            .as_ref()
            .is_some_and(|leaf| !leaf.aggregate.valid() || leaf.candidate.as_slice() != &key[37..])
        {
            return Err(invalid("candidate tree leaf does not match its point key"));
        }
        let state = UpdateState {
            version: combine_version.to_owned(),
            root: root.clone(),
            key: key.to_vec(),
            replacement,
            path: String::new(),
            current: root.tree.clone(),
            base: None,
            repair: 0,
            pending: None,
            apply_active: false,
            prepared: false,
        };
        let text = encoded(&state, CURSOR_BYTES)?;
        let captured_work = RefCell::new(CandidateWork::default());
        let (outcome, result, sqlite_vm_steps) =
            self.rank_transaction_measured(job, closure, cancelled, |db, id| {
                let mut tree = TreeDb {
                    db,
                    job: id,
                    work: CandidateWork::default(),
                };
                let mut graph = Graph {
                    db,
                    job: id,
                    work: super::Work::default(),
                };
                tree.bind_root(root, &mut graph)?;
                if let Some(leaf) = &state.replacement {
                    graph.node(&leaf.aggregate)?;
                    graph.bind(&leaf.aggregate)?;
                }
                if tree.cursor_exists(cursor)? {
                    if !resume {
                        return Err(invalid("candidate tree cursor already exists"));
                    }
                    let old: UpdateState = serde_json::from_str(&tree.read_cursor(cursor)?)
                        .map_err(io::Error::other)?;
                    if !old.valid()
                        || old.version != state.version
                        || old.root != state.root
                        || old.key != state.key
                        || old.replacement != state.replacement
                    {
                        return Err(invalid("source candidate update request changed"));
                    }
                } else {
                    tree.put_cursor(cursor, &text)?;
                }
                tree.work.add_graph(&graph.work);
                if tree.work.rows() > STEP_ROWS || tree.work.metadata_bytes > STEP_BYTES {
                    return Err(invalid("candidate tree begin step exceeds work budget"));
                }
                let work = tree.work.clone();
                *captured_work.borrow_mut() = work.clone();
                Ok((None, work))
            })?;
        let (complete, mut work) = result.unwrap_or((None, captured_work.into_inner()));
        work.sqlite_vm_steps = sqlite_vm_steps;
        Ok(CandidateStep {
            outcome,
            complete,
            work,
        })
    }

    /// Resume a fixed-key mutation with a scalar durable path and the graph's
    /// existing resumable Apply algebra.
    pub(crate) fn candidate_tree_advance_update(
        &mut self,
        job: &Job,
        cursor: &str,
        combine_version: &str,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CandidateStep> {
        cursor_name(cursor)?;
        let cancellation_seen = Cell::new(false);
        let latched_cancelled = || {
            if cancelled() {
                cancellation_seen.set(true);
            }
            cancellation_seen.get()
        };
        let captured_work = RefCell::new(CandidateWork::default());
        let (outcome, result, sqlite_vm_steps) =
            self.rank_transaction_measured(job, closure, &latched_cancelled, |db, id| {
                let mut tree = TreeDb {
                    db,
                    job: id,
                    work: CandidateWork::default(),
                };
                let mut graph = Graph {
                    db,
                    job: id,
                    work: super::Work::default(),
                };
                let text = tree.read_cursor(cursor)?;
                let mut state: UpdateState =
                    serde_json::from_str(&text).map_err(io::Error::other)?;
                if !state.valid()
                    || state.version != combine_version
                    || !tree.root_bound(&state.root)?
                {
                    return Err(invalid("candidate tree update cursor is stale or invalid"));
                }
                let had_apply = state.apply_active;
                let mut apply = tree.read_apply(cursor)?;
                if had_apply != apply.is_some() {
                    return Err(invalid("candidate tree Apply cursor ownership changed"));
                }
                let complete = advance_state(
                    &mut tree,
                    &mut graph,
                    &mut state,
                    &mut apply,
                    combine,
                    &latched_cancelled,
                )?;
                state.apply_active = apply.is_some();
                if let Some(apply) = &apply {
                    tree.put_apply(cursor, apply)?;
                } else if had_apply {
                    tree.delete_apply(cursor)?;
                }
                let root = if complete {
                    if state.apply_active {
                        return Err(invalid("candidate tree completed with pending graph Apply"));
                    }
                    let root = CandidateRoot {
                        tree: state.base.clone(),
                        empty: state.root.empty.clone(),
                    };
                    tree.bind_root(&root, &mut graph)?;
                    tree.delete_cursor(cursor)?;
                    Some(root)
                } else {
                    tree.put_cursor(cursor, &encoded(&state, CURSOR_BYTES)?)?;
                    None
                };
                tree.work.add_graph(&graph.work);
                if tree.work.rows() > STEP_ROWS || tree.work.metadata_bytes > STEP_BYTES {
                    return Err(invalid("candidate tree advance exceeds work budget"));
                }
                let work = tree.work.clone();
                *captured_work.borrow_mut() = work.clone();
                Ok((root, work))
            })?;
        let (complete, mut work) = result.unwrap_or((None, captured_work.into_inner()));
        work.sqlite_vm_steps = sqlite_vm_steps;
        Ok(CandidateStep {
            outcome,
            complete,
            work,
        })
    }

    /// Retain a root under this producer job. This imports one root reference;
    /// its immutable descendants are shared without cloning the subtree.
    pub(crate) fn candidate_tree_bind_root(
        &mut self,
        job: &Job,
        root: &CandidateRoot,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, CandidateWork)> {
        let captured_work = RefCell::new(CandidateWork::default());
        let (outcome, result, sqlite_vm_steps) =
            self.rank_transaction_measured(job, closure, cancelled, |db, id| {
                let mut tree = TreeDb {
                    db,
                    job: id,
                    work: CandidateWork::default(),
                };
                let mut graph = Graph {
                    db,
                    job: id,
                    work: super::Work::default(),
                };
                tree.bind_root(root, &mut graph)?;
                tree.work.add_graph(&graph.work);
                if tree.work.rows() > STEP_ROWS || tree.work.metadata_bytes > STEP_BYTES {
                    return Err(invalid("candidate tree root import exceeds work budget"));
                }
                let work = tree.work.clone();
                *captured_work.borrow_mut() = work.clone();
                Ok(work)
            })?;
        let mut work = result.unwrap_or_else(|| captured_work.into_inner());
        work.sqlite_vm_steps = sqlite_vm_steps;
        Ok((outcome, work))
    }

    /// Read indexed metadata under the complete job/source/owner closure. This
    /// returns no native bodies; a consumer rechecks after any provider I/O.
    pub(crate) fn candidate_tree_range(
        &self,
        job: &Job,
        root: &CandidateRoot,
        bounds: &CandidateBounds,
        included_sources: u64,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CandidateRead<String>> {
        self.candidate_tree_read(job, root, closure, cancelled, |tree, graph, cancelled| {
            let mut facts = graph.evaluate(&root.empty, included_sources)?;
            if let Some(tree_root) = &root.tree {
                let mut active = Vec::with_capacity(MAX_DEPTH);
                range_node(
                    tree,
                    graph,
                    tree_root,
                    None,
                    0,
                    &mut active,
                    bounds,
                    included_sources,
                    combine,
                    &mut facts,
                    cancelled,
                )?;
            }
            Ok(Some(facts))
        })
    }

    pub(crate) fn candidate_tree_select(
        &self,
        job: &Job,
        root: &CandidateRoot,
        rank: u64,
        included_sources: u64,
        count: &dyn Fn(&str) -> io::Result<u64>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CandidateRead<CandidatePoint>> {
        self.candidate_tree_read(job, root, closure, cancelled, |tree, graph, cancelled| {
            let Some(mut current) = root.tree.clone() else {
                return Ok(None);
            };
            let mut rank = rank;
            let mut remaining_total = None;
            let mut previous_split = None;
            let mut active = Vec::with_capacity(MAX_DEPTH);
            for depth in 0..=MAX_DEPTH {
                if cancelled() {
                    return Ok(None);
                }
                let identity = current.identity();
                if active.contains(&identity) {
                    return Err(invalid("candidate tree selection encountered a cycle"));
                }
                active.push(identity);
                tree.work.max_depth = tree.work.max_depth.max(depth);
                let node = tree.read_node(&current)?;
                let total = match remaining_total {
                    Some(total) => total,
                    None => count(&graph.evaluate(node.aggregate(), included_sources)?)?,
                };
                if total == 0 || rank >= total {
                    return Ok(None);
                }
                match node {
                    CandidateNode::Leaf {
                        key,
                        candidate,
                        aggregate,
                        payload,
                    } => {
                        if active.len() > MAX_DEPTH + 1 {
                            return Err(invalid("candidate tree selection depth exceeded"));
                        }
                        return Ok(Some(CandidatePoint {
                            key: fixed_key(&key)?,
                            leaf: CandidateLeaf {
                                candidate,
                                aggregate,
                                payload,
                            },
                        }));
                    }
                    CandidateNode::Branch {
                        split, left, right, ..
                    } => {
                        if previous_split.is_some_and(|previous| split <= previous)
                            || depth >= MAX_DEPTH
                        {
                            return Err(invalid("candidate tree selection ordering changed"));
                        }
                        let left_node = tree.read_node(&left)?;
                        let left_count =
                            count(&graph.evaluate(left_node.aggregate(), included_sources)?)?;
                        if left_count > total {
                            return Err(invalid("candidate tree aggregate count is inconsistent"));
                        }
                        if rank < left_count {
                            remaining_total = Some(left_count);
                            current = left;
                        } else {
                            rank -= left_count;
                            remaining_total = Some(total - left_count);
                            current = right;
                        }
                        previous_split = Some(split);
                    }
                }
            }
            Err(invalid("candidate tree selection exceeded key depth"))
        })
    }

    pub(crate) fn candidate_tree_seek(
        &self,
        job: &Job,
        root: &CandidateRoot,
        bound: &[u8; KEY_BYTES],
        direction: CandidateDirection,
        included_sources: u64,
        count: &dyn Fn(&str) -> io::Result<u64>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CandidateRead<CandidatePoint>> {
        self.candidate_tree_read(job, root, closure, cancelled, |tree, graph, cancelled| {
            let Some(tree_root) = &root.tree else {
                return Ok(None);
            };
            let mut active = Vec::with_capacity(MAX_DEPTH);
            seek_node(
                tree,
                graph,
                tree_root,
                None,
                0,
                &mut active,
                bound,
                direction,
                included_sources,
                count,
                cancelled,
            )
        })
    }

    /// Recheck the complete relationship closure after metadata reads. Any
    /// later source/body callback remains the consumer's next recheck boundary.
    fn candidate_tree_read<T>(
        &self,
        job: &Job,
        root: &CandidateRoot,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        operation: impl FnOnce(
            &mut TreeDb<'_>,
            &mut Graph<'_>,
            &dyn Fn() -> bool,
        ) -> io::Result<Option<T>>,
    ) -> io::Result<CandidateRead<T>> {
        let steps = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&steps);
        self.connection.progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        );
        let result = self.candidate_tree_read_body(job, root, closure, cancelled, operation);
        self.connection.progress_handler(0, None::<fn() -> bool>);
        result.map(|mut read| {
            read.work.sqlite_vm_steps = steps.load(Ordering::Relaxed);
            read
        })
    }

    fn candidate_tree_read_body<T>(
        &self,
        job: &Job,
        root: &CandidateRoot,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        operation: impl FnOnce(
            &mut TreeDb<'_>,
            &mut Graph<'_>,
            &dyn Fn() -> bool,
        ) -> io::Result<Option<T>>,
    ) -> io::Result<CandidateRead<T>> {
        let cancellation_seen = Cell::new(false);
        let latched_cancelled = || {
            if cancelled() {
                cancellation_seen.set(true);
            }
            cancellation_seen.get()
        };
        let mut work = CandidateWork::default();
        if latched_cancelled() {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Deferred,
                value: None,
                work,
            });
        }
        if !root_valid(root) || job.phase != "ready" || !self.runtime_current(&job.spec)? {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Conflict,
                value: None,
                work,
            });
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !native_query::job_current(&tx, job).map_err(io::Error::other)?
            || !closure(&tx, job.id).map_err(io::Error::other)?
        {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Conflict,
                value: None,
                work,
            });
        }
        let mut tree = TreeDb {
            db: &tx,
            job: job.id,
            work: CandidateWork::default(),
        };
        let mut graph = Graph {
            db: &tx,
            job: job.id,
            work: super::Work::default(),
        };
        if !tree.root_bound(root)? {
            work = tree.work;
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Conflict,
                value: None,
                work,
            });
        }
        graph.node(&root.empty)?;
        let value = operation(&mut tree, &mut graph, &latched_cancelled)?;
        tree.work.add_graph(&graph.work);
        work = tree.work;
        if latched_cancelled() {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Deferred,
                value: None,
                work,
            });
        }
        if !native_query::job_current(&tx, job).map_err(io::Error::other)?
            || !closure(&tx, job.id).map_err(io::Error::other)?
            || !self.runtime_current(&job.spec)?
        {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Conflict,
                value: None,
                work,
            });
        }
        tx.commit().map_err(io::Error::other)?;
        // WAL writers can commit while both checks above still see the same
        // old read snapshot. Release it before rechecking the owner's current
        // witnesses and the caller's complete relationship closure. Recheck
        // owned witnesses again after the callback, which may itself do I/O.
        if latched_cancelled() {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Deferred,
                value: None,
                work,
            });
        }
        let current = self.recheck(job)?
            && closure(&self.connection, job.id).map_err(io::Error::other)?
            && self.recheck(job)?;
        if latched_cancelled() {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Deferred,
                value: None,
                work,
            });
        }
        if !current {
            return Ok(CandidateRead {
                outcome: CandidateReadOutcome::Conflict,
                value: None,
                work,
            });
        }
        Ok(CandidateRead {
            outcome: CandidateReadOutcome::Qualified,
            value,
            work,
        })
    }
}

fn range_node(
    tree: &mut TreeDb<'_>,
    graph: &mut Graph<'_>,
    id: &CandidateNodeId,
    parent_split: Option<u16>,
    depth: usize,
    active: &mut Vec<String>,
    bounds: &CandidateBounds,
    included_sources: u64,
    combine: &dyn Fn(&str, &str) -> io::Result<String>,
    facts: &mut String,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    if cancelled() {
        return Ok(());
    }
    if depth > MAX_DEPTH {
        return Err(invalid("candidate tree range depth exceeded"));
    }
    let identity = id.identity();
    if active.contains(&identity) {
        return Err(invalid("candidate tree range encountered a cycle"));
    }
    active.push(identity);
    let node = tree.read_node(id)?;
    tree.work.max_depth = tree.work.max_depth.max(depth);
    if let Some(parent) = parent_split
        && node.branch().is_some_and(|(split, _, _)| split <= parent)
    {
        return Err(invalid("candidate tree range split order changed"));
    }
    if disjoint(node.min(), node.max(), bounds) {
        active.pop();
        return Ok(());
    }
    if fully_covered(node.min(), node.max(), bounds) {
        let aggregate = graph.evaluate(node.aggregate(), included_sources)?;
        *facts = combine(facts, &aggregate)?;
        active.pop();
        return Ok(());
    }
    match node {
        CandidateNode::Leaf { aggregate, .. } => {
            let value = graph.evaluate(&aggregate, included_sources)?;
            *facts = combine(facts, &value)?;
        }
        CandidateNode::Branch {
            split, left, right, ..
        } => {
            if depth >= MAX_DEPTH {
                return Err(invalid("candidate tree range key depth exceeded"));
            }
            range_node(
                tree,
                graph,
                &left,
                Some(split),
                depth + 1,
                active,
                bounds,
                included_sources,
                combine,
                facts,
                cancelled,
            )?;
            range_node(
                tree,
                graph,
                &right,
                Some(split),
                depth + 1,
                active,
                bounds,
                included_sources,
                combine,
                facts,
                cancelled,
            )?;
        }
    }
    active.pop();
    Ok(())
}

fn disjoint(minimum: &[u8], maximum: &[u8], bounds: &CandidateBounds) -> bool {
    bounds
        .lower
        .as_ref()
        .is_some_and(|lower| maximum < lower.as_slice())
        || bounds
            .upper
            .as_ref()
            .is_some_and(|upper| minimum > upper.as_slice())
}

fn fully_covered(minimum: &[u8], maximum: &[u8], bounds: &CandidateBounds) -> bool {
    bounds
        .lower
        .as_ref()
        .is_none_or(|lower| lower.as_slice() <= minimum)
        && bounds
            .upper
            .as_ref()
            .is_none_or(|upper| maximum <= upper.as_slice())
}

fn seek_node(
    tree: &mut TreeDb<'_>,
    graph: &mut Graph<'_>,
    id: &CandidateNodeId,
    parent_split: Option<u16>,
    depth: usize,
    active: &mut Vec<String>,
    bound: &[u8; KEY_BYTES],
    direction: CandidateDirection,
    included_sources: u64,
    count: &dyn Fn(&str) -> io::Result<u64>,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<Option<CandidatePoint>> {
    if cancelled() {
        return Ok(None);
    }
    if depth > MAX_DEPTH {
        return Err(invalid("candidate tree seek depth exceeded"));
    }
    let identity = id.identity();
    if active.contains(&identity) {
        return Err(invalid("candidate tree seek encountered a cycle"));
    }
    active.push(identity);
    let node = tree.read_node(id)?;
    tree.work.max_depth = tree.work.max_depth.max(depth);
    if let Some(parent) = parent_split
        && node.branch().is_some_and(|(split, _, _)| split <= parent)
    {
        return Err(invalid("candidate tree seek split order changed"));
    }
    let outside = match direction {
        CandidateDirection::Forward => node.max() < bound.as_slice(),
        CandidateDirection::Backward => node.min() > bound.as_slice(),
    };
    if outside || count(&graph.evaluate(node.aggregate(), included_sources)?)? == 0 {
        active.pop();
        return Ok(None);
    }
    let result = match node {
        CandidateNode::Leaf {
            key,
            candidate,
            aggregate,
            payload,
        } => Some(CandidatePoint {
            key: fixed_key(&key)?,
            leaf: CandidateLeaf {
                candidate,
                aggregate,
                payload,
            },
        }),
        CandidateNode::Branch {
            split, left, right, ..
        } => {
            if depth >= MAX_DEPTH {
                return Err(invalid("candidate tree seek key depth exceeded"));
            }
            let (first, second) = match direction {
                CandidateDirection::Forward => (&left, &right),
                CandidateDirection::Backward => (&right, &left),
            };
            let first_result = seek_node(
                tree,
                graph,
                first,
                Some(split),
                depth + 1,
                active,
                bound,
                direction,
                included_sources,
                count,
                cancelled,
            )?;
            if first_result.is_some() {
                first_result
            } else {
                seek_node(
                    tree,
                    graph,
                    second,
                    Some(split),
                    depth + 1,
                    active,
                    bound,
                    direction,
                    included_sources,
                    count,
                    cancelled,
                )?
            }
        }
    };
    active.pop();
    Ok(result)
}

fn advance_state(
    tree: &mut TreeDb<'_>,
    graph: &mut Graph<'_>,
    state: &mut UpdateState,
    apply_state: &mut Option<Apply>,
    combine: &dyn Fn(&str, &str) -> io::Result<String>,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    if !state.valid() || state.apply_active != apply_state.is_some() {
        return Err(invalid("candidate tree update state is invalid"));
    }
    loop {
        if cancelled() {
            return Ok(false);
        }
        if !state.prepared {
            if let Some(current) = state.current.clone() {
                if tree.work.rows() + graph.work.rows() + 14 > STEP_ROWS {
                    break;
                }
                tree.work.max_depth = tree.work.max_depth.max(state.path.len() / PATH_FRAME_BYTES);
                let node = tree.read_node(&current)?;
                match node {
                    CandidateNode::Branch {
                        split, left, right, ..
                    } => {
                        let previous = if state.path.is_empty() {
                            None
                        } else {
                            Some(
                                PathFrame::decode(
                                    &state.path,
                                    state.path.len() / PATH_FRAME_BYTES - 1,
                                )?
                                .split,
                            )
                        };
                        if previous.is_some_and(|old| split <= old)
                            || state.path.len() / PATH_FRAME_BYTES >= MAX_DEPTH
                            || state
                                .path
                                .as_bytes()
                                .chunks_exact(PATH_FRAME_BYTES)
                                .any(|frame| frame.get(..64) == Some(current.digest.as_bytes()))
                        {
                            return Err(invalid("candidate tree path is cyclic or unordered"));
                        }
                        let rightward = key_bit(&state.key, usize::from(split))
                            .ok_or_else(|| invalid("candidate tree split exceeds key"))?;
                        PathFrame {
                            node: current,
                            split,
                            right: rightward,
                        }
                        .encode(&mut state.path);
                        state.current = Some(if rightward { right } else { left });
                    }
                    leaf @ CandidateNode::Leaf { .. } => {
                        prepare_at_leaf(tree, graph, state, current, leaf)?;
                    }
                }
                continue;
            }
            prepare_empty(tree, graph, state)?;
            continue;
        }

        if let Some(pending) = state.pending.clone() {
            let state_bytes = encoded(state, CURSOR_BYTES)?.len();
            if let Some(apply) = apply_state.as_mut() {
                if apply.frames.is_empty() {
                    let aggregate = apply
                        .returned
                        .clone()
                        .ok_or_else(|| invalid("candidate tree Apply result is absent"))?;
                    let final_branch = (pending.consumes_parent && state.repair == 1)
                        || (!pending.consumes_parent && state.repair == 0);
                    let reserve = if final_branch { 20 } else { 14 };
                    if tree.work.rows() + graph.work.rows() + reserve > STEP_ROWS {
                        break;
                    }
                    let left = tree.read_node(&pending.left)?;
                    let right = tree.read_node(&pending.right)?;
                    let min = left.min().to_vec();
                    let max = right.max().to_vec();
                    let branch = CandidateNode::Branch {
                        split: pending.split,
                        left: pending.left,
                        right: pending.right,
                        min,
                        max,
                        aggregate,
                    };
                    let base = tree.intern(branch, graph)?;
                    state.base = Some(base);
                    if pending.consumes_parent {
                        state.repair = state
                            .repair
                            .checked_sub(1)
                            .ok_or_else(|| invalid("candidate tree repair cursor underflow"))?;
                    }
                    state.pending = None;
                    *apply_state = None;
                    continue;
                }
                let row_limit = STEP_ROWS.saturating_sub(tree.work.rows() + 20);
                let byte_limit =
                    STEP_BYTES.saturating_sub(tree.work.metadata_bytes + state_bytes + 128 * 1024);
                let before = graph.work.rows();
                graph.advance_limited(apply, combine, row_limit, byte_limit)?;
                if graph.work.rows() == before {
                    break;
                }
                continue;
            }
            let left = tree.read_node(&pending.left)?;
            let right = tree.read_node(&pending.right)?;
            validate_children(pending.split, &left, &right)?;
            let apply = Apply {
                version: state.version.clone(),
                identity: state.root.empty.clone(),
                frames: vec![Frame {
                    pair: Pair {
                        left: left.aggregate().clone(),
                        right: right.aggregate().clone(),
                    },
                    phase: Phase::Visit,
                }],
                returned: None,
            };
            if !apply.valid() {
                return Err(invalid("candidate tree Apply inputs are invalid"));
            }
            *apply_state = Some(apply);
            state.apply_active = true;
            continue;
        }

        if state.repair == 0 {
            if state.base.as_ref().is_some_and(|id| !id.valid()) {
                return Err(invalid("candidate tree result root is invalid"));
            }
            return Ok(true);
        }
        if tree.work.rows() + graph.work.rows() + 16 > STEP_ROWS {
            break;
        }
        let frame = PathFrame::decode(&state.path, state.repair - 1)?;
        let parent = tree.read_node(&frame.node)?;
        let CandidateNode::Branch {
            split, left, right, ..
        } = parent
        else {
            return Err(invalid("candidate tree repair parent is not a branch"));
        };
        if split != frame.split {
            return Err(invalid("candidate tree repair split changed"));
        }
        let base = state
            .base
            .clone()
            .ok_or_else(|| invalid("candidate tree repair base is absent"))?;
        state.pending = Some(PendingBranch {
            split,
            left: if frame.right { left } else { base.clone() },
            right: if frame.right { base } else { right },
            consumes_parent: true,
        });
    }
    Ok(false)
}

fn prepare_empty(
    tree: &mut TreeDb<'_>,
    graph: &mut Graph<'_>,
    state: &mut UpdateState,
) -> io::Result<()> {
    if state.root.tree.is_some() || !state.path.is_empty() {
        return Err(invalid(
            "candidate tree unexpectedly reached an empty child",
        ));
    }
    state.base = match &state.replacement {
        Some(leaf) => Some(tree.intern(
            CandidateNode::Leaf {
                key: state.key.clone(),
                candidate: leaf.candidate,
                aggregate: leaf.aggregate.clone(),
                payload: leaf.payload.clone(),
            },
            graph,
        )?),
        None => None,
    };
    state.prepared = true;
    Ok(())
}

fn prepare_at_leaf(
    tree: &mut TreeDb<'_>,
    graph: &mut Graph<'_>,
    state: &mut UpdateState,
    current: CandidateNodeId,
    node: CandidateNode,
) -> io::Result<()> {
    let CandidateNode::Leaf {
        key: existing_key, ..
    } = node
    else {
        return Err(invalid("candidate tree descent did not reach a leaf"));
    };
    if existing_key == state.key {
        state.base = match &state.replacement {
            Some(leaf) => Some(tree.intern(
                CandidateNode::Leaf {
                    key: state.key.clone(),
                    candidate: leaf.candidate,
                    aggregate: leaf.aggregate.clone(),
                    payload: leaf.payload.clone(),
                },
                graph,
            )?),
            None if state.path.is_empty() => None,
            None => {
                let last = PathFrame::decode(&state.path, state.path.len() / PATH_FRAME_BYTES - 1)?;
                let parent = tree.read_node(&last.node)?;
                let CandidateNode::Branch {
                    split, left, right, ..
                } = parent
                else {
                    return Err(invalid("candidate tree deletion parent is not a branch"));
                };
                if split != last.split {
                    return Err(invalid("candidate tree deletion split changed"));
                }
                Some(if last.right { left } else { right })
            }
        };
        let path_len = state.path.len() / PATH_FRAME_BYTES;
        state.repair = if state.replacement.is_some() {
            path_len
        } else {
            path_len.saturating_sub(1)
        };
    } else if let Some(leaf) = &state.replacement {
        let split = first_difference(&state.key, &existing_key)
            .ok_or_else(|| invalid("candidate tree distinct keys have no split"))?;
        let path_len = state.path.len() / PATH_FRAME_BYTES;
        let mut cut = path_len;
        for index in 0..path_len {
            if PathFrame::decode(&state.path, index)?.split > split {
                cut = index;
                break;
            }
        }
        let old = if cut < path_len {
            PathFrame::decode(&state.path, cut)?.node
        } else {
            current
        };
        let new = tree.intern(
            CandidateNode::Leaf {
                key: state.key.clone(),
                candidate: leaf.candidate,
                aggregate: leaf.aggregate.clone(),
                payload: leaf.payload.clone(),
            },
            graph,
        )?;
        let new_on_right = key_bit(&state.key, usize::from(split))
            .ok_or_else(|| invalid("candidate tree insertion split exceeds key"))?;
        state.pending = Some(PendingBranch {
            split,
            left: if new_on_right {
                old.clone()
            } else {
                new.clone()
            },
            right: if new_on_right { new } else { old },
            consumes_parent: false,
        });
        state.repair = cut;
    } else {
        // Deleting a point that is not present leaves the immutable root
        // unchanged. The caller still advances its producer revision/CAS.
        state.base = state.root.tree.clone();
        state.repair = 0;
    }
    state.current = None;
    state.prepared = true;
    Ok(())
}

/// One unpublished, structural neighbor search. No conditional graph is
/// evaluated: even an invisible candidate participates in native tie order.
/// Source qualification and readiness remain separate from public Query reads.
#[derive(Debug)]
pub(crate) struct SourceNeighborStep {
    pub(crate) outcome: Outcome,
    pub(crate) complete: bool,
    pub(crate) point: Option<CandidatePoint>,
    pub(crate) work: CandidateWork,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceNeighbor {
    version: String,
    root: CandidateRoot,
    bound: Vec<u8>,
    forward: bool,
    current: Option<CandidateNodeId>,
    siblings: Vec<CandidateNodeId>,
    complete: bool,
    result: Option<CandidateNodeId>,
}
impl SourceNeighbor {
    fn valid(&self) -> bool {
        self.version == "native-source-neighbor/v1"
            && root_valid(&self.root)
            && self.bound.len() == KEY_BYTES
            && self.siblings.len() <= MAX_DEPTH
            && self.siblings.iter().all(CandidateNodeId::valid)
            && self.current.as_ref().is_none_or(CandidateNodeId::valid)
            && self.result.as_ref().is_none_or(CandidateNodeId::valid)
            && (self.complete || self.result.is_none())
            && self.siblings.iter().enumerate().all(|(index, id)| {
                !self.siblings[..index].contains(id) && self.current.as_ref() != Some(id)
            })
            && (!self.complete || self.current.is_none() && self.siblings.is_empty())
    }
}

fn source_neighbor_point(
    tree: &mut TreeDb<'_>,
    state: &SourceNeighbor,
) -> io::Result<Option<CandidatePoint>> {
    if !state.complete {
        return Ok(None);
    }
    match &state.result {
        Some(id) => match tree.read_node(id)? {
            CandidateNode::Leaf {
                key,
                candidate,
                aggregate,
                payload,
            } => Ok(Some(CandidatePoint {
                key: fixed_key(&key)?,
                leaf: CandidateLeaf {
                    candidate,
                    aggregate,
                    payload,
                },
            })),
            _ => Err(invalid("source neighbor result is not a leaf")),
        },
        None => Ok(None),
    }
}

impl NativeStore {
    /// Save or resume the exact same neighbor request. Replaying begin after a
    /// crash preserves its durable path; another request cannot steal a cursor.
    pub(crate) fn candidate_tree_source_begin_neighbor(
        &mut self,
        stage: &native_query::source_roots::SourceStage,
        cursor: &str,
        root: &CandidateRoot,
        bound: &[u8; KEY_BYTES],
        direction: CandidateDirection,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<SourceNeighborStep> {
        cursor_name(cursor)?;
        if !root_valid(root) {
            return Err(invalid("invalid source neighbor root"));
        }
        let input = SourceNeighbor {
            version: "native-source-neighbor/v1".into(),
            root: root.clone(),
            bound: bound.to_vec(),
            forward: matches!(direction, CandidateDirection::Forward),
            current: root.tree.clone(),
            siblings: Vec::new(),
            complete: root.tree.is_none(),
            result: None,
        };
        self.source_neighbor_transaction(stage, closure, cancelled, |tree, graph, _| {
            tree.bind_root(root, graph)?;
            if tree.cursor_exists(cursor)? {
                let state: SourceNeighbor =
                    serde_json::from_str(&tree.read_cursor(cursor)?).map_err(io::Error::other)?;
                if !state.valid()
                    || state.root != input.root
                    || state.bound != input.bound
                    || state.forward != input.forward
                {
                    return Err(invalid("source neighbor request changed"));
                }
                let point = source_neighbor_point(tree, &state)?;
                Ok((state.complete, point))
            } else {
                tree.put_cursor(cursor, &encoded(&input, CURSOR_BYTES)?)?;
                Ok((input.complete, None))
            }
        })
    }

    /// At most24 structural-node reads per attempt, plus fixed cursor/binding
    /// controls. Both row work and complete-connection SQLite VM are returned.
    /// A completed cursor survives reopen until the producer job is retired.
    pub(crate) fn candidate_tree_source_advance_neighbor(
        &mut self,
        stage: &native_query::source_roots::SourceStage,
        cursor: &str,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<SourceNeighborStep> {
        cursor_name(cursor)?;
        self.source_neighbor_transaction(stage, closure, cancelled, |tree, _, cancelled| {
            let mut state: SourceNeighbor =
                serde_json::from_str(&tree.read_cursor(cursor)?).map_err(io::Error::other)?;
            if !state.valid() || !tree.root_bound(&state.root)? {
                return Err(invalid("invalid source neighbor cursor"));
            }
            for _ in 0..24 {
                if cancelled() {
                    break;
                }
                if state.complete {
                    break;
                }
                let Some(id) = state.current.take() else {
                    state.current = state.siblings.pop();
                    if state.current.is_none() {
                        state.complete = true;
                    }
                    continue;
                };
                if tree.work.metadata_bytes + 2 * NODE_BYTES + CURSOR_BYTES > STEP_BYTES {
                    state.current = Some(id);
                    break;
                }
                match tree.read_node(&id)? {
                    CandidateNode::Leaf { key, .. } => {
                        let eligible = if state.forward {
                            key >= state.bound
                        } else {
                            key <= state.bound
                        };
                        if eligible {
                            state.result = Some(id);
                            state.complete = true;
                            state.siblings.clear();
                        } else {
                            state.current = state.siblings.pop();
                        }
                    }
                    CandidateNode::Branch {
                        left,
                        right,
                        min,
                        max,
                        ..
                    } => {
                        let outside = if state.forward {
                            max < state.bound
                        } else {
                            min > state.bound
                        };
                        if outside {
                            state.current = state.siblings.pop();
                        } else {
                            if state.siblings.len() == MAX_DEPTH {
                                return Err(invalid("source neighbor path exceeds trie depth"));
                            }
                            state.siblings.push(if state.forward {
                                right.clone()
                            } else {
                                left.clone()
                            });
                            state.current = Some(if state.forward { left } else { right });
                        }
                    }
                }
                if state.current.is_none() && state.siblings.is_empty() && state.result.is_none() {
                    state.complete = true;
                }
            }
            let point = source_neighbor_point(tree, &state)?;
            tree.put_cursor(cursor, &encoded(&state, CURSOR_BYTES)?)?;
            Ok((state.complete, point))
        })
    }

    fn source_neighbor_transaction(
        &mut self,
        stage: &native_query::source_roots::SourceStage,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        operation: impl FnOnce(
            &mut TreeDb<'_>,
            &mut Graph<'_>,
            &dyn Fn() -> bool,
        ) -> io::Result<(bool, Option<CandidatePoint>)>,
    ) -> io::Result<SourceNeighborStep> {
        let cancellation_seen = Cell::new(false);
        let latched = || {
            if cancelled() {
                cancellation_seen.set(true);
            }
            cancellation_seen.get()
        };
        let qualified = |db: &Connection, id| {
            Ok(native_query::source_roots::scope_current(db, stage)? && closure(db, id)?)
        };
        let captured = RefCell::new(CandidateWork::default());
        let (mut outcome, result, vm) =
            self.rank_transaction_measured(&stage.job, &qualified, &latched, |db, id| {
                let mut tree = TreeDb {
                    db,
                    job: id,
                    work: CandidateWork::default(),
                };
                let mut graph = Graph {
                    db,
                    job: id,
                    work: super::Work::default(),
                };
                let value = operation(&mut tree, &mut graph, &latched)?;
                tree.work.add_graph(&graph.work);
                if tree.work.rows() > STEP_ROWS || tree.work.metadata_bytes > STEP_BYTES {
                    return Err(invalid("source neighbor step exceeds metadata budget"));
                }
                *captured.borrow_mut() = tree.work.clone();
                Ok((value, tree.work))
            })?;
        let ((complete, point), mut work) =
            result.unwrap_or(((false, None), captured.into_inner()));
        work.sqlite_vm_steps = vm;
        if outcome == Outcome::Written {
            let (checked, steps) = self.source_mutation_final_fence(stage, closure, &latched)?;
            work.sqlite_vm_steps += steps;
            outcome = checked;
        }
        let admitted = outcome == Outcome::Written;
        Ok(SourceNeighborStep {
            outcome,
            complete: admitted && complete,
            point: if admitted { point } else { None },
            work,
        })
    }
}

impl NativeStore {
    pub(crate) fn candidate_tree_source_begin_update(
        &mut self,
        stage: &native_query::source_roots::SourceStage,
        cursor: &str,
        root: &CandidateRoot,
        key: [u8; KEY_BYTES],
        replacement: Option<CandidateLeaf>,
        combine_version: &str,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CandidateStep> {
        let seen = Cell::new(false);
        let latched = || {
            if cancelled() {
                seen.set(true);
            }
            seen.get()
        };
        let qualified = |db: &Connection, id| {
            Ok(native_query::source_roots::scope_current(db, stage)? && closure(db, id)?)
        };
        let mut step = self.candidate_tree_begin_update_internal(
            &stage.job,
            cursor,
            root,
            key,
            replacement,
            combine_version,
            &qualified,
            &latched,
            true,
        )?;
        if step.outcome == Outcome::Written {
            let (outcome, steps) = self.source_mutation_final_fence(stage, closure, &latched)?;
            step.work.sqlite_vm_steps += steps;
            step.outcome = outcome;
            if outcome != Outcome::Written {
                step.complete = None;
            }
        }
        Ok(step)
    }

    pub(crate) fn candidate_tree_source_advance_update(
        &mut self,
        stage: &native_query::source_roots::SourceStage,
        cursor: &str,
        combine_version: &str,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<CandidateStep> {
        let seen = Cell::new(false);
        let latched = || {
            if cancelled() {
                seen.set(true);
            }
            seen.get()
        };
        let qualified = |db: &Connection, id| {
            Ok(native_query::source_roots::scope_current(db, stage)? && closure(db, id)?)
        };
        let mut step = self.candidate_tree_advance_update(
            &stage.job,
            cursor,
            combine_version,
            combine,
            &qualified,
            &latched,
        )?;
        if step.outcome == Outcome::Written {
            let (outcome, steps) = self.source_mutation_final_fence(stage, closure, &latched)?;
            step.work.sqlite_vm_steps += steps;
            step.outcome = outcome;
            if outcome != Outcome::Written {
                step.complete = None;
            }
        }
        Ok(step)
    }

    fn source_mutation_final_fence(
        &self,
        stage: &native_query::source_roots::SourceStage,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, u64)> {
        let mut fresh = native_query::source_roots::SourceStage {
            job: stage.job.clone(),
            epoch: stage.epoch.clone(),
        };
        fresh.job.revision = fresh
            .job
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("source producer job revision overflow"))?;
        self.recheck_source_closure(&fresh, closure, cancelled)
    }
}
