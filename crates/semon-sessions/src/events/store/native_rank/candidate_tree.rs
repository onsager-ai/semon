//! Durable immutable point-key candidate trees. The native producer owns
//! source/time qualification and the graph's Facts algebra; this module owns
//! only fixed-width placement keys, candidate addresses, and graph roots.
use super::{
    Apply, Frame, Graph, Node, NodeId, Pair, Phase, ROW_PREFIX, STEP_BYTES, STEP_ROWS,
    native_query::{self, Job, NativeStore},
};
use crate::events::Outcome;
use crate::model::range_stage::CandidateRef;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{cell::Cell, io};

pub(crate) const KEY_BYTES: usize = 69;
const KEY_BITS: usize = KEY_BYTES * 8;
const MAX_DEPTH: usize = KEY_BITS;
const NODE_BYTES: usize = 64 * 1024;
const CURSOR_BYTES: usize = 64 * 1024;
const PATH_FRAME_BYTES: usize = 68;
const EMPTY_TREE: &str = "rank/v1/tree/empty";

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
}

/// The point payload contains no candidate body or model-derived semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateLeaf {
    pub(crate) candidate: CandidateRef,
    pub(crate) aggregate: NodeId,
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

#[derive(Clone, Debug)]
pub(crate) struct CandidateStep {
    pub(crate) outcome: Outcome,
    pub(crate) complete: Option<CandidateRoot>,
    pub(crate) work: CandidateWork,
}

/// Counts executed point statements and decoded/encoded bytes, plus the
/// existing graph Apply work that maintains native conditional aggregates.
#[derive(Clone, Debug, Default)]
pub(crate) struct CandidateWork {
    pub(crate) sql_reads: usize,
    pub(crate) sql_writes: usize,
    pub(crate) metadata_bytes: usize,
    pub(crate) vm_steps: i64,
    pub(crate) apply_visits: usize,
    pub(crate) nodes_written: usize,
    pub(crate) max_depth: usize,
    pub(crate) max_cursor_bytes: usize,
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
        let right = match frame.as_bytes()[67] {
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
    apply: Option<Apply>,
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
            && self.apply.as_ref().is_none_or(|apply| apply.valid())
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
        let mut q = self
            .db
            .prepare_cached(
                "SELECT CASE WHEN octet_length(n.metadata)<=65536 THEN n.metadata END,n.left_id,n.right_id,n.aggregate,
                 CASE WHEN n.left_id IS NULL THEN n.right_id IS NULL ELSE n.right_id IS NOT NULL
                   AND EXISTS(SELECT 1 FROM native_rank_candidate_nodes a WHERE a.identity=n.left_id AND a.refs>0)
                   AND EXISTS(SELECT 1 FROM native_rank_candidate_nodes b WHERE b.identity=n.right_id AND b.refs>0)
                 END FROM native_rank_candidate_nodes n WHERE n.identity=?1",
            )
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let row: Option<(Option<String>, Option<String>, Option<String>, String, bool)> = q
            .query_row([id.identity()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        let vm = q.get_status(rusqlite::StatementStatus::VmStep);
        drop(q);
        let Some((Some(text), left, right, aggregate, children_ok)) = row else {
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
    let installed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='native_rank_candidate_cursors')",
        [],
        |r| r.get(0),
    )?;
    if !installed {
        return Ok((0, true));
    }
    let mut used = 0;
    let mut q = db.prepare("SELECT cursor,octet_length(metadata)+octet_length(cursor) FROM native_rank_candidate_cursors WHERE job=?1 ORDER BY cursor LIMIT ?2")?;
    let cursors = q
        .query_map(params![job, budget as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (cursor, length) in cursors {
        if used >= budget || length > *bytes {
            break;
        }
        if db.execute(
            "DELETE FROM native_rank_candidate_cursors WHERE job=?1 AND cursor=?2",
            params![job, cursor],
        )? != 1
        {
            return Err(cleanup_error("candidate tree cursor disappeared"));
        }
        *bytes -= length;
        used += 1;
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_rank_candidate_cursors WHERE job=?1)",
        [job],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok((used, false));
    }

    let root_limit = (budget - used) / 3;
    let mut q = db.prepare("SELECT identity,empty,length(identity)+length(empty) FROM native_rank_candidate_roots WHERE job=?1 ORDER BY identity,empty LIMIT ?2")?;
    let roots = q
        .query_map(params![job, root_limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, usize>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (identity, empty, length) in roots {
        let node = identity != EMPTY_TREE;
        let operations = 1 + usize::from(node) + 1;
        if used + operations > budget || length > *bytes {
            break;
        }
        if db.execute(
            "DELETE FROM native_rank_candidate_roots WHERE job=?1 AND identity=?2 AND empty=?3",
            params![job, identity, empty],
        )? != 1
        {
            return Err(cleanup_error("candidate tree root binding disappeared"));
        }
        used += 1;
        if node {
            decrement_tree_ref(db, &identity)?;
            used += 1;
        }
        decrement_graph_ref(db, &empty)?;
        used += 1;
        *bytes -= length;
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_rank_candidate_roots WHERE job=?1)",
        [job],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok((used, false));
    }

    let binding_limit = (budget - used) / 2;
    let mut q = db.prepare("SELECT identity,length(identity) FROM native_rank_candidate_bindings WHERE job=?1 ORDER BY identity LIMIT ?2")?;
    let bindings = q
        .query_map(params![job, binding_limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (identity, length) in bindings {
        if used + 2 > budget || length > *bytes {
            break;
        }
        if db.execute(
            "DELETE FROM native_rank_candidate_bindings WHERE job=?1 AND identity=?2",
            params![job, identity],
        )? != 1
        {
            return Err(cleanup_error("candidate tree node binding disappeared"));
        }
        decrement_tree_ref(db, &identity)?;
        used += 2;
        *bytes -= length;
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_rank_candidate_bindings WHERE job=?1)",
        [job],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok((used, false));
    }

    while budget - used >= 4 {
        let record: Option<(
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            usize,
        )> = db
            .query_row(
                "SELECT identity,CASE WHEN octet_length(metadata)<=65536 THEN metadata END,left_id,right_id,aggregate,
                 octet_length(metadata)+length(identity)+COALESCE(length(left_id),0)+COALESCE(length(right_id),0)+length(aggregate)
                 FROM native_rank_candidate_nodes WHERE refs=0 ORDER BY identity LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((identity, metadata, left, right, aggregate, length)) = record else {
            break;
        };
        if length > *bytes {
            break;
        }
        let metadata = metadata
            .ok_or_else(|| cleanup_error("candidate tree cleanup node exceeds scalar budget"))?;
        let node: CandidateNode = serde_json::from_str(&metadata)
            .map_err(|_| cleanup_error("candidate tree cleanup node is corrupt"))?;
        let id = parse_tree_identity(&identity)
            .ok_or_else(|| cleanup_error("candidate tree cleanup address is invalid"))?;
        if !node.valid() || candidate_id(&metadata) != id {
            return Err(cleanup_error("candidate tree cleanup identity changed"));
        }
        let expected = match &node {
            CandidateNode::Leaf { .. } => (None, None),
            CandidateNode::Branch { left, right, .. } => {
                (Some(left.identity()), Some(right.identity()))
            }
        };
        if (left, right) != expected || node.aggregate().identity() != aggregate {
            return Err(cleanup_error("candidate tree cleanup references changed"));
        }
        if db.execute(
            "DELETE FROM native_rank_candidate_nodes WHERE identity=?1 AND refs=0",
            [&identity],
        )? != 1
        {
            return Err(cleanup_error("candidate tree cleanup node changed"));
        }
        used += 1;
        for child in [expected.0.as_ref(), expected.1.as_ref()]
            .into_iter()
            .flatten()
        {
            decrement_tree_ref(db, &child)?;
            used += 1;
        }
        decrement_graph_ref(db, &node.aggregate().identity())?;
        used += 1;
        *bytes -= length;
    }
    let remains = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_rank_candidate_cursors) OR EXISTS(SELECT 1 FROM native_rank_candidate_roots) OR EXISTS(SELECT 1 FROM native_rank_candidate_bindings) OR EXISTS(SELECT 1 FROM native_rank_candidate_nodes WHERE refs=0)",
        [],
        |r| r.get::<_, bool>(0),
    )?;
    if remains {
        return Ok((used, false));
    }
    Ok((used, true))
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

fn decrement_graph_ref(db: &Connection, identity: &str) -> rusqlite::Result<()> {
    if db.execute(
        "UPDATE native_rank_nodes SET refs=refs-1 WHERE identity=?1 AND refs>0",
        [identity],
    )? != 1
    {
        return Err(cleanup_error("candidate tree graph aggregate is absent"));
    }
    Ok(())
}

fn cleanup_error(message: &'static str) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(io::Error::new(
        io::ErrorKind::InvalidData,
        message,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::store::{EventCache, SqliteStore},
        events::{Event, FileIndex, Kind},
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
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
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
            // This test owns initialization explicitly; production registration
            // remains the private schema owner's versioned migration.
            store
                .connection
                .execute_batch(super::super::SCHEMA)
                .unwrap();
            assert_eq!(native_query::SCHEMA_VERSION, 7);
            store.connection.execute_batch(SCHEMA).unwrap();
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
            let start = self
                .store
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
                .unwrap();
            assert_eq!(start.outcome, Outcome::Written);
            assert!(start.complete.is_none());
            check_step(&start.work);
            let mut work = start.work;
            self.refresh();
            for _ in 0..2000 {
                let step = self
                    .store
                    .candidate_tree_advance_update(
                        &self.job,
                        &cursor,
                        version,
                        &combine,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
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

    fn valid_closure(_: &Connection, _: i64) -> rusqlite::Result<bool> {
        Ok(true)
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
        assert!(work.max_depth <= MAX_DEPTH + 1);
    }

    fn add_work(total: &mut CandidateWork, next: &CandidateWork) {
        total.sql_reads += next.sql_reads;
        total.sql_writes += next.sql_writes;
        total.metadata_bytes += next.metadata_bytes;
        total.vm_steps += next.vm_steps;
        total.apply_visits += next.apply_visits;
        total.nodes_written += next.nodes_written;
        total.max_depth = total.max_depth.max(next.max_depth);
        total.max_cursor_bytes = total.max_cursor_bytes.max(next.max_cursor_bytes);
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
        let closure_calls = Cell::new(0);
        let changing_closure = |_: &Connection, _: i64| {
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
    }

    #[test]
    fn candidate_tree_cursor_reopens_and_imported_root_survives_predecessor_cleanup() {
        let mut fixture = Fixture::new();
        let mut root = CandidateRoot::empty(fixture.empty.clone());
        let first = test_entry(0, 10, 0);
        let first_leaf = fixture.graph_leaf(&first, 0);
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
        let start = fixture
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
            .unwrap();
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
        loop {
            let old = fixture
                .store
                .reload(&original_spec.view_key)
                .unwrap()
                .unwrap();
            match fixture.store.cleanup(&old).unwrap() {
                Outcome::Written => break,
                Outcome::Deferred => continue,
                outcome => panic!("unexpected cleanup outcome {outcome:?}"),
            }
        }
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
        let imported_root = loop {
            let step = fixture
                .store
                .candidate_tree_advance_update(
                    &next_job,
                    cursor,
                    "facts-v1",
                    &combine,
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            next_job = fixture.store.reload(&next_spec.view_key).unwrap().unwrap();
            if let Some(root) = step.complete {
                break root;
            }
        };
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
        assert_eq!(result.value.unwrap().key, first.key);
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
        let one = graph.intern(Node::Terminal { facts: one_text }).unwrap();
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
            let (outcome, work) = fixture
                .store
                .candidate_tree_bind_root(&fixture.job, root, &valid_closure, &|| false)
                .unwrap();
            assert_eq!(outcome, Outcome::Written);
            assert!(work.unwrap().sql_writes > 0);
            fixture.refresh();
        }
        fixture.ready();

        let measured_vm = Arc::new(AtomicUsize::new(0));
        let observed_vm = Arc::clone(&measured_vm);
        fixture.store.connection.progress_handler(
            1,
            Some(move || {
                observed_vm.fetch_add(1, Ordering::Relaxed);
                false
            }),
        );
        for (root, size) in roots.iter().zip(sizes) {
            if size == 0 {
                let selected = fixture
                    .store
                    .candidate_tree_select(
                        &fixture.job,
                        root,
                        0,
                        u64::MAX,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(selected.outcome, CandidateReadOutcome::Qualified);
                assert!(selected.value.is_none());
                continue;
            }
            let point = &items[size / 2];
            let selected = fixture
                .store
                .candidate_tree_select(
                    &fixture.job,
                    root,
                    (size / 2) as u64,
                    u64::MAX,
                    &facts_count,
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(selected.outcome, CandidateReadOutcome::Qualified);
            assert_eq!(selected.value.as_ref().unwrap().key, point.key);
            assert!(selected.work.sql_reads > 0 && selected.work.vm_steps > 0);
            assert!(selected.work.sql_reads + selected.work.sql_writes < 8 * KEY_BITS);
            assert!(selected.work.max_depth <= MAX_DEPTH);
            assert!(selected.work.max_live_node_bytes < NODE_BYTES);

            let bounds = CandidateBounds {
                lower: Some(point.key),
                upper: Some(point.key),
            };
            let range = fixture
                .store
                .candidate_tree_range(
                    &fixture.job,
                    root,
                    &bounds,
                    u64::MAX,
                    &combine_facts,
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            let range_facts: Facts = serde_json::from_str(range.value.as_deref().unwrap()).unwrap();
            assert_eq!(range.outcome, CandidateReadOutcome::Qualified);
            assert_eq!(range_facts.count, 1);
            assert!(range.work.sql_reads + range.work.sql_writes < 16 * KEY_BITS);
            assert!(range.work.metadata_bytes < STEP_BYTES);
            assert!(range.work.max_live_node_bytes < NODE_BYTES);

            let mut seek_reads = 0;
            for direction in [CandidateDirection::Forward, CandidateDirection::Backward] {
                let seek = fixture
                    .store
                    .candidate_tree_seek(
                        &fixture.job,
                        root,
                        &point.key,
                        direction,
                        u64::MAX,
                        &facts_count,
                        &valid_closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(seek.outcome, CandidateReadOutcome::Qualified);
                assert_eq!(seek.value.unwrap().key, point.key);
                assert!(seek.work.sql_reads + seek.work.sql_writes < 8 * KEY_BITS);
                assert!(seek.work.vm_steps > 0);
                seek_reads += seek.work.sql_reads;
            }
            println!(
                "candidate_tree prefix={size} select_reads={} select_vm={} select_bytes={} range_reads={} seek_reads={} depth={} node_bytes={}",
                selected.work.sql_reads,
                selected.work.vm_steps,
                selected.work.metadata_bytes,
                range.work.sql_reads,
                seek_reads,
                range.work.max_depth,
                range.work.max_live_node_bytes,
            );
        }
        fixture
            .store
            .connection
            .progress_handler(0, None::<fn() -> bool>);
        assert!(measured_vm.load(Ordering::Relaxed) > 0);
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
        };
        let right = CandidateNode::Leaf {
            key: right_key.to_vec(),
            candidate: [0; 32],
            aggregate: fixture.empty.clone(),
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
    fn candidate_tree_deepest_key_path_stays_inside_scalar_cursor_budget() {
        let mut fixture = Fixture::new();
        let mut graph = Graph {
            db: &fixture.store.connection,
            job: fixture.job.id,
            work: super::super::Work::default(),
        };
        let one = graph
            .intern(Node::Terminal {
                facts: serde_json::to_string(&Facts {
                    count: 1,
                    ..Facts::default()
                })
                .unwrap(),
            })
            .unwrap();
        drop(graph);
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
                aggregate: one.clone(),
            })
            .collect();
        let tx = fixture.store.connection.unchecked_transaction().unwrap();
        let root = bulk_root(&tx, fixture.job.id, &items, &fixture.empty).unwrap();
        tx.commit().unwrap();
        let (outcome, _) = fixture
            .store
            .candidate_tree_bind_root(&fixture.job, &root, &valid_closure, &|| false)
            .unwrap();
        assert_eq!(outcome, Outcome::Written);
        fixture.refresh();

        let replacement_facts = serde_json::to_string(&Facts {
            count: 2,
            ..Facts::default()
        })
        .unwrap();
        let empty_facts = serde_json::to_string(&Facts::default()).unwrap();
        let (outcome, aggregate) = fixture
            .store
            .rank_guard(
                &fixture.job,
                &replacement_facts,
                &empty_facts,
                &[],
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(outcome, Outcome::Written);
        let aggregate = aggregate.unwrap().0;
        fixture.refresh();
        let key = [0; KEY_BYTES];
        let start = fixture
            .store
            .candidate_tree_begin_update(
                &fixture.job,
                "deepest-key-update",
                &root,
                key,
                Some(CandidateLeaf {
                    candidate: [0; 32],
                    aggregate,
                }),
                "deep-facts-v1",
                &valid_closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(start.outcome, Outcome::Written);
        check_step(&start.work);
        let mut max_cursor = start.work.max_cursor_bytes;
        fixture.refresh();
        let mut complete = None;
        for _ in 0..2000 {
            let step = fixture
                .store
                .candidate_tree_advance_update(
                    &fixture.job,
                    "deepest-key-update",
                    "deep-facts-v1",
                    &combine_facts,
                    &valid_closure,
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Written);
            check_step(&step.work);
            max_cursor = max_cursor.max(step.work.max_cursor_bytes);
            fixture.refresh();
            if step.complete.is_some() {
                complete = step.complete;
                break;
            }
        }
        assert!(complete.is_some());
        assert!(max_cursor > 32 * 1024);
        assert!(max_cursor <= CURSOR_BYTES);
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
            apply: None,
            prepared: false,
        };
        let text = encoded(&state, CURSOR_BYTES)?;
        let (outcome, result) = self.rank_transaction(job, closure, cancelled, |db, id| {
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
                return Err(invalid("candidate tree cursor already exists"));
            }
            tree.put_cursor(cursor, &text)?;
            tree.work.add_graph(&graph.work);
            if tree.work.rows() > STEP_ROWS || tree.work.metadata_bytes > STEP_BYTES {
                return Err(invalid("candidate tree begin step exceeds work budget"));
            }
            Ok((None, tree.work))
        })?;
        let (complete, work) = result.unwrap_or_default();
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
        let (outcome, result) =
            self.rank_transaction(job, closure, &latched_cancelled, |db, id| {
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
                let complete = advance_state(
                    &mut tree,
                    &mut graph,
                    &mut state,
                    combine,
                    &latched_cancelled,
                )?;
                let root = if complete {
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
                Ok((root, tree.work))
            })?;
        let (complete, work) = result.unwrap_or_default();
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
    ) -> io::Result<(Outcome, Option<CandidateWork>)> {
        let (outcome, result) = self.rank_transaction(job, closure, cancelled, |db, id| {
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
            Ok(tree.work)
        })?;
        Ok((outcome, result))
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
            Ok(facts)
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
                let total = count(&graph.evaluate(node.aggregate(), included_sources)?)?;
                if total == 0 || rank >= total {
                    return Ok(None);
                }
                match node {
                    CandidateNode::Leaf {
                        key,
                        candidate,
                        aggregate,
                    } => {
                        if active.len() > MAX_DEPTH + 1 {
                            return Err(invalid("candidate tree selection depth exceeded"));
                        }
                        return Ok(Some(CandidatePoint {
                            key: fixed_key(&key)?,
                            leaf: CandidateLeaf {
                                candidate,
                                aggregate,
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
                            current = left;
                        } else {
                            rank -= left_count;
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
        operation: impl FnOnce(&mut TreeDb<'_>, &mut Graph<'_>, &dyn Fn() -> bool) -> io::Result<T>,
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
        Ok(CandidateRead {
            outcome: CandidateReadOutcome::Qualified,
            value: Some(value),
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
        } => Some(CandidatePoint {
            key: fixed_key(&key)?,
            leaf: CandidateLeaf {
                candidate,
                aggregate,
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
    combine: &dyn Fn(&str, &str) -> io::Result<String>,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    if !state.valid() {
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
            if let Some(apply) = state.apply.as_mut() {
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
                    state.apply = None;
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
            state.apply = Some(apply);
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
