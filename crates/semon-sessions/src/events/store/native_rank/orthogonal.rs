//! Persistent geometry for native Order × model scalar rectangles.
//! Geometry never interprets native identities, expiration or annotation rules.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const ORDER_BYTES: usize = 69;
const PREFIX: &str = "orthogonal/v1/";

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_orthogonal_nodes (
 identity TEXT PRIMARY KEY, metadata TEXT NOT NULL,
 zero TEXT, one TEXT, secondary TEXT, aggregate TEXT,
 refs INTEGER NOT NULL CHECK(refs>=0)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_orthogonal_roots (
 job INTEGER NOT NULL, name TEXT NOT NULL, identity TEXT,
 PRIMARY KEY(job,name)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_orthogonal_work (
 job INTEGER NOT NULL, name TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(job,name)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_orthogonal_frames (
 job INTEGER NOT NULL, name TEXT NOT NULL, family INTEGER NOT NULL,
 depth INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(job,name,family,depth)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_orthogonal_gc (
 job INTEGER NOT NULL, identity TEXT NOT NULL,
 PRIMARY KEY(job,identity)
) STRICT, WITHOUT ROWID;
";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Root {
    scalar_bytes: usize,
    node: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    axis: u8,
    bit: usize,
    digest: String,
}

impl Id {
    fn identity(&self) -> String {
        format!("{PREFIX}node/{}/{}/{}", self.axis, self.bit, self.digest)
    }
    fn valid(&self, width: usize, axis: u8) -> bool {
        self.axis == axis
            && self.bit <= width * 8
            && self.digest.len() == 64
            && self.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
    fn parse(identity: &str) -> io::Result<Self> {
        let mut parts = identity
            .strip_prefix(&format!("{PREFIX}node/"))
            .ok_or_else(|| failure("orthogonal root identity is invalid"))?
            .split('/');
        let axis = parts
            .next()
            .ok_or_else(|| failure("orthogonal root axis is absent"))?
            .parse()
            .map_err(io::Error::other)?;
        let bit = parts
            .next()
            .ok_or_else(|| failure("orthogonal root bit is absent"))?
            .parse()
            .map_err(io::Error::other)?;
        let digest = parts
            .next()
            .ok_or_else(|| failure("orthogonal root digest is absent"))?
            .to_owned();
        if parts.next().is_some() {
            return Err(failure("orthogonal root identity has trailing parts"));
        }
        Ok(Self { axis, bit, digest })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Point {
    pub(crate) order: Vec<u8>,
    pub(crate) scalar: Vec<u8>,
    pub(crate) candidate: [u8; 32],
    pub(crate) facts: NodeId,
}

impl Point {
    fn key(&self, axis: u8) -> Vec<u8> {
        if axis == 0 {
            self.order.clone()
        } else {
            let mut key = self.scalar.clone();
            key.extend_from_slice(&self.order);
            key
        }
    }
    fn valid(&self, scalar_bytes: usize) -> bool {
        matches!(scalar_bytes, 9 | 12)
            && self.order.len() == ORDER_BYTES
            && self.scalar.len() == scalar_bytes
            && self.facts.valid()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum TreeNode {
    Leaf {
        point: Point,
        secondary: Option<Id>,
    },
    Branch {
        bit: usize,
        low: Vec<u8>,
        high: Vec<u8>,
        zero: Id,
        one: Id,
        secondary: Option<Id>,
        aggregate: Option<NodeId>,
    },
}

impl TreeNode {
    fn range(&self, axis: u8) -> (Vec<u8>, Vec<u8>) {
        match self {
            Self::Leaf { point, .. } => {
                let key = point.key(axis);
                (key.clone(), key)
            }
            Self::Branch { low, high, .. } => (low.clone(), high.clone()),
        }
    }
    fn bit(&self, axis: u8) -> usize {
        match self {
            Self::Leaf { point, .. } => point.key(axis).len() * 8,
            Self::Branch { bit, .. } => *bit,
        }
    }
    fn secondary(&self) -> Option<&Id> {
        match self {
            Self::Leaf { secondary, .. } | Self::Branch { secondary, .. } => secondary.as_ref(),
        }
    }
    fn aggregate(&self) -> Option<&NodeId> {
        match self {
            Self::Leaf { point, secondary } if secondary.is_none() => Some(&point.facts),
            Self::Branch { aggregate, .. } => aggregate.as_ref(),
            _ => None,
        }
    }
    fn valid(&self, axis: u8, scalar_bytes: usize) -> bool {
        let key_bytes = width(axis, scalar_bytes);
        match self {
            Self::Leaf { point, secondary } => {
                point.valid(scalar_bytes)
                    && if axis == 0 {
                        secondary
                            .as_ref()
                            .is_some_and(|id| id.valid(width(1, scalar_bytes), 1))
                    } else {
                        secondary.is_none()
                    }
            }
            Self::Branch {
                bit,
                low,
                high,
                zero,
                one,
                secondary,
                aggregate,
            } => {
                *bit < key_bytes * 8
                    && low.len() == key_bytes
                    && high.len() == key_bytes
                    && low < high
                    && difference(low, high) == Some(*bit)
                    && zero.valid(key_bytes, axis)
                    && one.valid(key_bytes, axis)
                    && zero.bit > *bit
                    && one.bit > *bit
                    && if axis == 0 {
                        aggregate.is_none()
                            && secondary
                                .as_ref()
                                .is_some_and(|id| id.valid(width(1, scalar_bytes), 1))
                    } else {
                        secondary.is_none() && aggregate.as_ref().is_some_and(NodeId::valid)
                    }
            }
        }
    }
}

fn width(axis: u8, scalar: usize) -> usize {
    ORDER_BYTES + if axis == 0 { 0 } else { scalar }
}
fn bit(key: &[u8], position: usize) -> bool {
    key[position / 8] & (0x80 >> (position % 8)) != 0
}
fn difference(left: &[u8], right: &[u8]) -> Option<usize> {
    left.iter()
        .zip(right)
        .enumerate()
        .find_map(|(index, (left, right))| {
            let different = left ^ right;
            (different != 0).then(|| index * 8 + different.leading_zeros() as usize)
        })
}
fn address(axis: u8, node: &TreeNode, text: &str) -> Id {
    Id {
        axis,
        bit: node.bit(axis),
        digest: format!("{:x}", Sha256::digest(text.as_bytes())),
    }
}

/// Counts decoded geometry and graph rows. All SQL VM work, including caller
/// fences and CAS/COMMIT, is measured independently by the connection hook.
#[derive(Clone, Debug, Default)]
pub(crate) struct Measure {
    pub(crate) geometry: Work,
    pub(crate) connection_vm_steps: u64,
    pub(crate) cursor_bytes: usize,
    /// Conservative bound for live geometry payloads, not process RSS or owner I/O.
    pub(crate) resident_bytes: usize,
}

struct Geometry<'a> {
    db: &'a Connection,
    job: i64,
    scalar: usize,
    work: Work,
}

impl Geometry<'_> {
    fn execute(&mut self, sql: &str, parameters: impl rusqlite::Params) -> io::Result<usize> {
        let mut statement = self.db.prepare_cached(sql).map_err(io::Error::other)?;
        statement.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = statement.execute(parameters).map_err(io::Error::other)?;
        self.work.sql_writes += 1;
        self.work.vm_steps += i64::from(statement.get_status(rusqlite::StatementStatus::VmStep));
        Ok(changed)
    }
    fn read(&mut self, id: &Id) -> io::Result<TreeNode> {
        if !id.valid(width(id.axis, self.scalar), id.axis) || id.axis > 1 {
            return Err(failure("invalid orthogonal node address"));
        }
        let mut statement = self.db.prepare_cached("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_orthogonal_nodes WHERE identity=?1").map_err(io::Error::other)?;
        statement.reset_status(rusqlite::StatementStatus::VmStep);
        let text: Option<Option<String>> = statement
            .query_row([id.identity()], |row| row.get(0))
            .optional()
            .map_err(io::Error::other)?;
        self.work.sql_reads += 1;
        self.work.vm_steps += i64::from(statement.get_status(rusqlite::StatementStatus::VmStep));
        let text = text
            .flatten()
            .ok_or_else(|| failure("orthogonal node absent or over budget"))?;
        self.work.metadata_bytes += text.len();
        self.work.max_live_node_bytes = self.work.max_live_node_bytes.max(text.len());
        let node: TreeNode = serde_json::from_str(&text).map_err(io::Error::other)?;
        if !node.valid(id.axis, self.scalar) || address(id.axis, &node, &text) != *id {
            return Err(failure("orthogonal node identity or order changed"));
        }
        Ok(node)
    }
    fn reference(&mut self, id: &Id, delta: i64) -> io::Result<()> {
        if self.execute(
            "UPDATE native_orthogonal_nodes SET refs=refs+?2 WHERE identity=?1",
            params![id.identity(), delta],
        )? != 1
        {
            return Err(failure("orthogonal dependency absent"));
        }
        if delta < 0 {
            self.execute("INSERT INTO native_orthogonal_gc SELECT ?1,identity FROM native_orthogonal_nodes WHERE identity=?2 AND refs=0 ON CONFLICT(job,identity) DO NOTHING", params![self.job, id.identity()])?;
        }
        Ok(())
    }
    fn intern(&mut self, axis: u8, node: TreeNode) -> io::Result<Id> {
        if !node.valid(axis, self.scalar) {
            return Err(failure("invalid orthogonal node"));
        }
        let text = encoded(&node)?;
        let id = address(axis, &node, &text);
        let (zero, one) = match &node {
            TreeNode::Branch { zero, one, .. } => (Some(zero), Some(one)),
            _ => (None, None),
        };
        let secondary = node.secondary();
        let aggregate = node.aggregate();
        let created = self.execute("INSERT INTO native_orthogonal_nodes VALUES(?1,?2,?3,?4,?5,?6,0) ON CONFLICT(identity) DO NOTHING", params![id.identity(), text, zero.map(Id::identity), one.map(Id::identity), secondary.map(Id::identity), aggregate.map(NodeId::identity)])?;
        self.work.metadata_bytes += text.len();
        self.work.nodes_written += created;
        if created != 0 {
            for child in [zero, one, secondary].into_iter().flatten() {
                self.reference(child, 1)?;
            }
            if let Some(aggregate) = aggregate {
                if self.execute(
                    "UPDATE native_rank_nodes SET refs=refs+1 WHERE identity=?1",
                    [aggregate.identity()],
                )? != 1
                {
                    return Err(failure("orthogonal graph dependency absent"));
                }
            }
        }
        Ok(id)
    }
}

impl Root {
    fn valid(&self) -> bool {
        matches!(self.scalar_bytes, 9 | 12)
            && self
                .node
                .as_ref()
                .is_none_or(|node| node.valid(ORDER_BYTES, 0))
    }
    pub(crate) fn empty(scalar_bytes: usize) -> io::Result<Self> {
        if !matches!(scalar_bytes, 9 | 12) {
            return Err(failure("unsupported orthogonal scalar width"));
        }
        Ok(Self {
            scalar_bytes,
            node: None,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spine {
    node: Id,
    one: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Walk {
    Find,
    Descend { split: Option<usize> },
    Unwind,
    Build,
    Done,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchBuild {
    bit: usize,
    zero: Id,
    one: Id,
    low: Vec<u8>,
    high: Vec<u8>,
    base: Option<Id>,
    remove: bool,
    inserted: bool,
    aggregate: Option<NodeId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeWalk {
    axis: u8,
    root: Option<Id>,
    node: Option<Id>,
    target: Vec<u8>,
    point: Option<Point>,
    old: Option<Point>,
    depth: usize,
    phase: Walk,
    result: Option<Id>,
    build: Option<BranchBuild>,
}

impl TreeWalk {
    fn new(axis: u8, root: Option<Id>, target: Vec<u8>, point: Option<Point>) -> Self {
        Self {
            axis,
            node: root.clone(),
            root,
            target,
            point,
            old: None,
            depth: 0,
            phase: Walk::Find,
            result: None,
            build: None,
        }
    }
    fn refs(&self, refs: &mut BTreeMap<String, Id>) {
        for id in [&self.root, &self.node, &self.result].into_iter().flatten() {
            refs.insert(id.identity(), id.clone());
        }
        if let Some(build) = &self.build {
            for id in [Some(&build.zero), Some(&build.one), build.base.as_ref()]
                .into_iter()
                .flatten()
            {
                refs.insert(id.identity(), id.clone());
            }
        }
    }
    fn valid(&self, scalar: usize) -> bool {
        let key_bytes = width(self.axis, scalar);
        self.axis <= 1
            && self.target.len() == key_bytes
            && self.depth <= key_bytes * 8
            && [&self.root, &self.node, &self.result]
                .into_iter()
                .flatten()
                .all(|id| id.valid(key_bytes, self.axis))
            && self
                .point
                .as_ref()
                .is_none_or(|point| point.valid(scalar) && point.key(self.axis) == self.target)
            && self.old.as_ref().is_none_or(|point| point.valid(scalar))
            && match &self.phase {
                Walk::Descend { split: Some(split) } => *split < key_bytes * 8,
                _ => true,
            }
            && self.build.as_ref().is_none_or(|build| {
                build.bit < key_bytes * 8
                    && build.low.len() == key_bytes
                    && build.high.len() == key_bytes
                    && build.low < build.high
                    && difference(&build.low, &build.high) == Some(build.bit)
                    && build.zero.valid(key_bytes, self.axis)
                    && build.one.valid(key_bytes, self.axis)
                    && build.zero.bit > build.bit
                    && build.one.bit > build.bit
                    && build
                        .base
                        .as_ref()
                        .is_none_or(|id| id.valid(width(1, scalar), 1))
                    && build.aggregate.as_ref().is_none_or(NodeId::valid)
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphWalk {
    depth: usize,
    result: Option<NodeId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    root: Root,
    target: Vec<u8>,
    point: Option<Point>,
    version: String,
    identity: NodeId,
    outer: TreeWalk,
    inner: Option<TreeWalk>,
    graph: Option<GraphWalk>,
    complete: bool,
}

impl Update {
    fn refs(&self) -> BTreeMap<String, Id> {
        let mut refs = BTreeMap::new();
        if let Some(id) = &self.root.node {
            refs.insert(id.identity(), id.clone());
        }
        self.outer.refs(&mut refs);
        if let Some(inner) = &self.inner {
            inner.refs(&mut refs);
        }
        refs
    }
    fn valid(&self) -> bool {
        matches!(self.root.scalar_bytes, 9 | 12)
            && self
                .root
                .node
                .as_ref()
                .is_none_or(|id| id.valid(ORDER_BYTES, 0))
            && self.target.len() == ORDER_BYTES
            && self.point.as_ref().is_none_or(|point| {
                point.valid(self.root.scalar_bytes) && point.order == self.target
            })
            && !self.version.is_empty()
            && self.version.len() <= 256
            && self.identity.valid()
            && self.outer.axis == 0
            && self.outer.valid(self.root.scalar_bytes)
            && self
                .inner
                .as_ref()
                .is_none_or(|inner| inner.axis == 1 && inner.valid(self.root.scalar_bytes))
            && self.graph.as_ref().is_none_or(|graph| {
                graph.depth <= usize::from(VARIABLES) + 1
                    && graph.result.as_ref().is_none_or(NodeId::valid)
            })
    }
}

impl Geometry<'_> {
    fn frame<T: for<'de> Deserialize<'de>>(
        &mut self,
        name: &str,
        family: u8,
        depth: usize,
    ) -> io::Result<T> {
        let mut statement = self.db.prepare_cached("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_orthogonal_frames WHERE job=?1 AND name=?2 AND family=?3 AND depth=?4").map_err(io::Error::other)?;
        statement.reset_status(rusqlite::StatementStatus::VmStep);
        let text: Option<String> = statement
            .query_row(params![self.job, name, family, depth as i64], |row| {
                row.get(0)
            })
            .map_err(io::Error::other)?;
        self.work.sql_reads += 1;
        self.work.vm_steps += i64::from(statement.get_status(rusqlite::StatementStatus::VmStep));
        let text = text.ok_or_else(|| failure("orthogonal cursor frame exceeds budget"))?;
        self.work.metadata_bytes += text.len();
        self.work.max_live_node_bytes = self.work.max_live_node_bytes.max(text.len());
        serde_json::from_str(&text).map_err(io::Error::other)
    }
    fn put_frame<T: Serialize>(
        &mut self,
        name: &str,
        family: u8,
        depth: usize,
        frame: &T,
    ) -> io::Result<()> {
        let text = encoded(frame)?;
        self.work.metadata_bytes += text.len();
        self.execute("INSERT INTO native_orthogonal_frames VALUES(?1,?2,?3,?4,?5) ON CONFLICT(job,name,family,depth) DO UPDATE SET metadata=excluded.metadata", params![self.job, name, family, depth as i64, text])?;
        Ok(())
    }
    fn pop_frame(&mut self, name: &str, family: u8, depth: usize) -> io::Result<()> {
        if self.execute("DELETE FROM native_orthogonal_frames WHERE job=?1 AND name=?2 AND family=?3 AND depth=?4", params![self.job, name, family, depth as i64])? != 1 {
            return Err(failure("orthogonal cursor frame is absent"));
        }
        Ok(())
    }
    fn children(
        &mut self,
        axis: u8,
        zero: &Id,
        one: &Id,
        at: usize,
    ) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let zero = self.read(zero)?;
        let one = self.read(one)?;
        let (low, zero_high) = zero.range(axis);
        let (one_low, high) = one.range(axis);
        if zero_high >= one_low
            || bit(&zero_high, at)
            || !bit(&one_low, at)
            || difference(&low, &high) != Some(at)
        {
            return Err(failure("orthogonal child branches are swapped or overlap"));
        }
        Ok((low, high))
    }
    fn leaf(&mut self, axis: u8, point: Point) -> io::Result<Id> {
        let secondary = if axis == 0 {
            Some(self.intern(
                1,
                TreeNode::Leaf {
                    point: point.clone(),
                    secondary: None,
                },
            )?)
        } else {
            None
        };
        self.intern(axis, TreeNode::Leaf { point, secondary })
    }
    fn walk(&mut self, name: &str, state: &mut TreeWalk) -> io::Result<()> {
        self.work.max_stack = self.work.max_stack.max(state.depth);
        match state.phase.clone() {
            Walk::Find => {
                let Some(id) = &state.node else {
                    state.result = state
                        .point
                        .clone()
                        .map(|point| self.leaf(state.axis, point))
                        .transpose()?;
                    state.phase = Walk::Done;
                    return Ok(());
                };
                match self.read(id)? {
                    TreeNode::Branch {
                        bit: at, zero, one, ..
                    } => {
                        state.node = Some(if bit(&state.target, at) { one } else { zero });
                    }
                    TreeNode::Leaf { point, .. } => {
                        let key = point.key(state.axis);
                        let split = difference(&state.target, &key);
                        if split.is_some() && state.point.is_none() {
                            state.result = state.root.clone();
                            state.phase = Walk::Done;
                            return Ok(());
                        }
                        state.old = split.is_none().then_some(point);
                        state.node = state.root.clone();
                        state.phase = Walk::Descend { split };
                    }
                }
            }
            Walk::Descend { split } => {
                let id = state
                    .node
                    .clone()
                    .ok_or_else(|| failure("orthogonal descent has no node"))?;
                let node = self.read(&id)?;
                if let Some(at) = split
                    && id.bit >= at
                {
                    let leaf = self.leaf(
                        state.axis,
                        state
                            .point
                            .clone()
                            .ok_or_else(|| failure("orthogonal insertion has no point"))?,
                    )?;
                    let (zero, one) = if bit(&state.target, at) {
                        (id.clone(), leaf)
                    } else {
                        (leaf, id.clone())
                    };
                    let (low, high) = self.children(state.axis, &zero, &one, at)?;
                    state.build = Some(BranchBuild {
                        bit: at,
                        zero,
                        one,
                        low,
                        high,
                        base: node.secondary().cloned(),
                        remove: false,
                        inserted: false,
                        aggregate: None,
                    });
                    state.phase = Walk::Build;
                } else if let TreeNode::Branch {
                    bit: at, zero, one, ..
                } = node
                {
                    let one_side = bit(&state.target, at);
                    self.put_frame(
                        name,
                        state.axis,
                        state.depth,
                        &Spine {
                            node: id,
                            one: one_side,
                        },
                    )?;
                    state.depth += 1;
                    if state.depth > state.target.len() * 8 {
                        return Err(failure("orthogonal descent exceeds fixed key depth"));
                    }
                    state.node = Some(if one_side { one } else { zero });
                } else {
                    state.result = state
                        .point
                        .clone()
                        .map(|point| self.leaf(state.axis, point))
                        .transpose()?;
                    state.phase = Walk::Unwind;
                }
            }
            Walk::Unwind => {
                if state.depth == 0 {
                    state.phase = Walk::Done;
                    return Ok(());
                }
                let frame: Spine = self.frame(name, state.axis, state.depth - 1)?;
                self.pop_frame(name, state.axis, state.depth - 1)?;
                state.depth -= 1;
                let TreeNode::Branch {
                    bit: at,
                    zero,
                    one,
                    secondary,
                    ..
                } = self.read(&frame.node)?
                else {
                    return Err(failure("orthogonal spine refers to a leaf"));
                };
                let Some(replacement) = state.result.clone() else {
                    state.result = Some(if frame.one { zero } else { one });
                    return Ok(());
                };
                let (zero, one) = if frame.one {
                    (zero, replacement)
                } else {
                    (replacement, one)
                };
                let (low, high) = self.children(state.axis, &zero, &one, at)?;
                state.build = Some(BranchBuild {
                    bit: at,
                    zero,
                    one,
                    low,
                    high,
                    base: secondary,
                    remove: false,
                    inserted: false,
                    aggregate: None,
                });
                state.phase = Walk::Build;
            }
            Walk::Build | Walk::Done => {}
        }
        Ok(())
    }

    fn add_graph_work(&mut self, graph: Work) {
        self.work.sql_reads += graph.sql_reads;
        self.work.sql_writes += graph.sql_writes;
        self.work.metadata_bytes += graph.metadata_bytes;
        self.work.vm_steps += graph.vm_steps;
        self.work.apply_visits += graph.apply_visits;
        self.work.memo_hits += graph.memo_hits;
        self.work.nodes_written += graph.nodes_written;
        self.work.max_stack = self.work.max_stack.max(graph.max_stack);
        self.work.max_live_node_bytes =
            self.work.max_live_node_bytes.max(graph.max_live_node_bytes);
    }
    fn graph_step(
        &mut self,
        name: &str,
        state: &mut GraphWalk,
        identity: &NodeId,
        version: &str,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
    ) -> io::Result<()> {
        if state.depth == 0 {
            return Ok(());
        }
        if state.depth > usize::from(VARIABLES) + 1 {
            return Err(failure(
                "orthogonal Graph cursor exceeds native source depth",
            ));
        }
        let index = state.depth - 1;
        let mut frame: Frame = self.frame(name, 2, index)?;
        let mut graph = Graph {
            db: self.db,
            job: self.job,
            work: Work::default(),
        };
        graph.work.max_stack = state.depth;
        if let Some(result) = state.result.take() {
            match frame.phase.clone() {
                Phase::Absent { source, present } => {
                    frame.phase = Phase::Present {
                        source,
                        absent: result,
                    };
                    self.put_frame(name, 2, index, &frame)?;
                    self.put_frame(
                        name,
                        2,
                        state.depth,
                        &Frame {
                            pair: present,
                            phase: Phase::Visit,
                        },
                    )?;
                    state.depth += 1;
                }
                Phase::Present { source, absent } => {
                    let result = graph.intern(Node::Decision {
                        source,
                        absent,
                        present: result,
                    })?;
                    graph.memo(version, &frame.pair, &result)?;
                    self.pop_frame(name, 2, index)?;
                    state.depth -= 1;
                    state.result = Some(result);
                }
                Phase::Visit => {
                    return Err(failure("orthogonal Graph parent cursor is inconsistent"));
                }
            }
        } else {
            if !matches!(frame.phase, Phase::Visit) {
                return Err(failure("orthogonal Graph child result is absent"));
            }
            graph.work.apply_visits += 1;
            let result = if frame.pair.left == *identity || frame.pair.right == *identity {
                Some(if frame.pair.left == *identity {
                    frame.pair.right.clone()
                } else {
                    frame.pair.left.clone()
                })
            } else if let Some(text) = graph.get(&memo_identity(version, &frame.pair)?)? {
                let result: NodeId = serde_json::from_str(&text).map_err(io::Error::other)?;
                if !result.valid() {
                    return Err(failure("orthogonal Graph memo address is invalid"));
                }
                graph.work.memo_hits += 1;
                Some(result)
            } else {
                let left = graph.node(&frame.pair.left)?;
                let right = graph.node(&frame.pair.right)?;
                match (&left, &right) {
                    (Node::Terminal { facts: left }, Node::Terminal { facts: right }) => {
                        graph.work.max_live_node_bytes =
                            graph.work.max_live_node_bytes.max(left.len() + right.len());
                        let result = graph.intern(Node::Terminal {
                            facts: combine(left, right)?,
                        })?;
                        graph.memo(version, &frame.pair, &result)?;
                        Some(result)
                    }
                    _ => {
                        let source = left.level().min(right.level());
                        let (la, lp) = left.children(source, &frame.pair.left);
                        let (ra, rp) = right.children(source, &frame.pair.right);
                        frame.phase = Phase::Absent {
                            source,
                            present: Pair {
                                left: lp,
                                right: rp,
                            },
                        };
                        self.put_frame(name, 2, index, &frame)?;
                        self.put_frame(
                            name,
                            2,
                            state.depth,
                            &Frame {
                                pair: Pair {
                                    left: la,
                                    right: ra,
                                },
                                phase: Phase::Visit,
                            },
                        )?;
                        state.depth += 1;
                        None
                    }
                }
            };
            if let Some(result) = result {
                graph.bind(&result)?;
                self.pop_frame(name, 2, index)?;
                state.depth -= 1;
                state.result = Some(result);
            }
        }
        self.add_graph_work(graph.work);
        Ok(())
    }

    fn build_inner(&mut self, name: &str, update: &mut Update) -> io::Result<()> {
        let inner = update
            .inner
            .as_mut()
            .ok_or_else(|| failure("orthogonal secondary cursor absent"))?;
        if !matches!(inner.phase, Walk::Build) {
            self.walk(name, inner)?;
            return Ok(());
        }
        let build = inner
            .build
            .as_mut()
            .ok_or_else(|| failure("orthogonal secondary branch absent"))?;
        if let Some(graph) = update.graph.as_mut() {
            if graph.depth != 0 {
                return Err(failure("orthogonal Graph stage has not finished"));
            }
            build.aggregate = graph.result.clone();
            update.graph = None;
        }
        if let Some(aggregate) = build.aggregate.clone() {
            inner.result = Some(self.intern(
                1,
                TreeNode::Branch {
                    bit: build.bit,
                    low: build.low.clone(),
                    high: build.high.clone(),
                    zero: build.zero.clone(),
                    one: build.one.clone(),
                    secondary: None,
                    aggregate: Some(aggregate),
                },
            )?);
            inner.build = None;
            inner.phase = Walk::Unwind;
        } else {
            let zero = self.read(&build.zero)?;
            let one = self.read(&build.one)?;
            let left = zero
                .aggregate()
                .ok_or_else(|| failure("orthogonal secondary aggregate absent"))?
                .clone();
            let right = one
                .aggregate()
                .ok_or_else(|| failure("orthogonal secondary aggregate absent"))?
                .clone();
            self.put_frame(
                name,
                2,
                0,
                &Frame {
                    pair: Pair { left, right },
                    phase: Phase::Visit,
                },
            )?;
            update.graph = Some(GraphWalk {
                depth: 1,
                result: None,
            });
        }
        Ok(())
    }

    fn update_step(
        &mut self,
        name: &str,
        update: &mut Update,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
    ) -> io::Result<()> {
        if let Some(graph) = &mut update.graph
            && graph.depth != 0
        {
            self.graph_step(name, graph, &update.identity, &update.version, combine)?;
            return Ok(());
        }
        if let Some(inner) = &update.inner
            && !matches!(inner.phase, Walk::Done)
        {
            self.build_inner(name, update)?;
            return Ok(());
        }
        if let Some(inner) = update.inner.take() {
            let build = update
                .outer
                .build
                .as_mut()
                .ok_or_else(|| failure("orthogonal outer branch absent"))?;
            build.base = inner.result;
        }
        if matches!(update.outer.phase, Walk::Done) {
            update.complete = true;
            return Ok(());
        }
        if !matches!(update.outer.phase, Walk::Build) {
            return self.walk(name, &mut update.outer);
        }
        let build = update
            .outer
            .build
            .as_mut()
            .ok_or_else(|| failure("orthogonal outer build absent"))?;
        if !build.remove {
            build.remove = true;
            if let Some(old) = &update.outer.old
                && update
                    .point
                    .as_ref()
                    .is_none_or(|new| new.scalar != old.scalar)
            {
                update.inner = Some(TreeWalk::new(1, build.base.clone(), old.key(1), None));
                return Ok(());
            }
        }
        if !build.inserted {
            build.inserted = true;
            if let Some(point) = &update.point {
                update.inner = Some(TreeWalk::new(
                    1,
                    build.base.clone(),
                    point.key(1),
                    Some(point.clone()),
                ));
                return Ok(());
            }
        }
        let secondary = build
            .base
            .clone()
            .ok_or_else(|| failure("orthogonal branch secondary is empty"))?;
        update.outer.result = Some(self.intern(
            0,
            TreeNode::Branch {
                bit: build.bit,
                low: build.low.clone(),
                high: build.high.clone(),
                zero: build.zero.clone(),
                one: build.one.clone(),
                secondary: Some(secondary),
                aggregate: None,
            },
        )?);
        update.outer.build = None;
        update.outer.phase = Walk::Unwind;
        Ok(())
    }
}

impl Geometry<'_> {
    fn state(&mut self, name: &str) -> io::Result<Option<Update>> {
        let mut statement = self.db.prepare_cached("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_orthogonal_work WHERE job=?1 AND name=?2").map_err(io::Error::other)?;
        statement.reset_status(rusqlite::StatementStatus::VmStep);
        let text: Option<Option<String>> = statement
            .query_row(params![self.job, name], |row| row.get(0))
            .optional()
            .map_err(io::Error::other)?;
        self.work.sql_reads += 1;
        self.work.vm_steps += i64::from(statement.get_status(rusqlite::StatementStatus::VmStep));
        match text {
            None => Ok(None),
            Some(None) => Err(failure("orthogonal cursor exceeds scalar budget")),
            Some(Some(text)) => {
                self.work.metadata_bytes += text.len();
                self.work.max_live_node_bytes = self.work.max_live_node_bytes.max(text.len());
                let state: Update = serde_json::from_str(&text).map_err(io::Error::other)?;
                if !state.valid() {
                    return Err(failure("orthogonal cursor is invalid"));
                }
                Ok(Some(state))
            }
        }
    }
    fn save(&mut self, name: &str, previous: Option<&Update>, state: &Update) -> io::Result<usize> {
        if !state.valid() {
            return Err(failure("orthogonal cursor is invalid"));
        }
        let old = previous.map(Update::refs).unwrap_or_default();
        let new = state.refs();
        for (identity, id) in &new {
            if !old.contains_key(identity) {
                self.reference(id, 1)?;
            }
        }
        for (identity, id) in &old {
            if !new.contains_key(identity) {
                self.reference(id, -1)?;
            }
        }
        let text = encoded(state)?;
        self.work.metadata_bytes += text.len();
        let bytes = text.len();
        self.execute("INSERT INTO native_orthogonal_work VALUES(?1,?2,?3) ON CONFLICT(job,name) DO UPDATE SET metadata=excluded.metadata", params![self.job, name, text])?;
        Ok(bytes)
    }
    fn bind(&mut self, name: &str, root: &Root) -> io::Result<()> {
        let mut statement = self
            .db
            .prepare_cached("SELECT identity FROM native_orthogonal_roots WHERE job=?1 AND name=?2")
            .map_err(io::Error::other)?;
        statement.reset_status(rusqlite::StatementStatus::VmStep);
        let old: Option<Option<String>> = statement
            .query_row(params![self.job, name], |row| row.get(0))
            .optional()
            .map_err(io::Error::other)?;
        self.work.sql_reads += 1;
        self.work.vm_steps += i64::from(statement.get_status(rusqlite::StatementStatus::VmStep));
        drop(statement);
        let next = root.node.as_ref().map(Id::identity);
        if old.as_ref().and_then(Option::as_ref) == next.as_ref() && old.is_some() {
            return Ok(());
        }
        if let Some(id) = &root.node {
            self.reference(id, 1)?;
        }
        self.execute("INSERT INTO native_orthogonal_roots VALUES(?1,?2,?3) ON CONFLICT(job,name) DO UPDATE SET identity=excluded.identity", params![self.job, name, next])?;
        if let Some(Some(old)) = old {
            self.reference(&Id::parse(&old)?, -1)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct UpdateResult {
    pub(crate) outcome: Outcome,
    pub(crate) root: Option<Root>,
    pub(crate) work: Measure,
}

pub(crate) struct Mutation<'a> {
    pub(crate) name: &'a str,
    pub(crate) root: &'a Root,
    pub(crate) order: &'a [u8],
    pub(crate) point: Option<&'a Point>,
    pub(crate) version: &'a str,
    pub(crate) identity: &'a NodeId,
}

// The hook measures actual connection instructions, including SELECTs in a
// caller's closure and all native owner fences, writes, CAS and COMMIT.
fn vm_meter(connection: &Connection) -> Arc<AtomicU64> {
    let counter = Arc::new(AtomicU64::new(0));
    let hook = counter.clone();
    connection.progress_handler(
        1,
        Some(move || {
            hook.fetch_add(1, Ordering::Relaxed);
            false
        }),
    );
    counter
}
fn finish_meter(connection: &Connection, meter: &AtomicU64, measure: &mut Measure) {
    connection.progress_handler(0, None::<fn() -> bool>);
    measure.connection_vm_steps = meter.load(Ordering::Relaxed);
}

impl NativeStore {
    pub(crate) fn orthogonal_begin(
        &mut self,
        job: &Job,
        input: Mutation<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<UpdateResult> {
        if input.name.is_empty() || input.name.len() > 256 {
            return Err(failure("invalid orthogonal operation name"));
        }
        let state = Update {
            root: input.root.clone(),
            target: input.order.to_vec(),
            point: input.point.cloned(),
            version: input.version.to_owned(),
            identity: input.identity.clone(),
            outer: TreeWalk::new(
                0,
                input.root.node.clone(),
                input.order.to_vec(),
                input.point.cloned(),
            ),
            inner: None,
            graph: None,
            complete: false,
        };
        if !state.valid() {
            return Err(failure("invalid orthogonal mutation"));
        }
        let meter = vm_meter(&self.connection);
        let mut attempted = Measure::default();
        let result = self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut geometry = Geometry {
                db,
                job: id,
                scalar: input.root.scalar_bytes,
                work: Work::default(),
            };
            let previous = geometry.state(input.name)?;
            if let Some(previous) = &previous {
                if previous.root == state.root
                    && previous.target == state.target
                    && previous.point == state.point
                    && previous.version == state.version
                    && previous.identity == state.identity
                {
                    attempted = Measure {
                        geometry: geometry.work.clone(),
                        ..Measure::default()
                    };
                    return Ok((
                        previous.complete.then(|| Root {
                            scalar_bytes: previous.root.scalar_bytes,
                            node: previous.outer.result.clone(),
                        }),
                        Measure {
                            geometry: geometry.work,
                            ..Measure::default()
                        },
                    ));
                }
                if !previous.complete {
                    return Err(failure("orthogonal mutation is already pending"));
                }
            }
            if let Some(root) = &input.root.node {
                geometry.read(root)?;
            }
            let mut graph = Graph {
                db,
                job: id,
                work: Work::default(),
            };
            graph.node(input.identity)?;
            graph.bind(input.identity)?;
            if let Some(point) = input.point {
                graph.node(&point.facts)?;
                graph.bind(&point.facts)?;
            }
            geometry.add_graph_work(graph.work);
            geometry.bind(input.name, input.root)?;
            let cursor_bytes = geometry.save(input.name, previous.as_ref(), &state)?;
            if geometry.work.rows() > STEP_ROWS || geometry.work.metadata_bytes > STEP_BYTES {
                return Err(failure("orthogonal admission exceeds work budget"));
            }
            let resident_bytes = cursor_bytes + geometry.work.max_live_node_bytes + 3 * NODE_BYTES;
            attempted = Measure {
                geometry: geometry.work,
                cursor_bytes,
                resident_bytes,
                ..Measure::default()
            };
            Ok((None, attempted.clone()))
        });
        let mut measure = Measure::default();
        finish_meter(&self.connection, &meter, &mut measure);
        let (outcome, output) = result?;
        let (root, mut work) = output.unwrap_or((None, attempted));
        work.connection_vm_steps = measure.connection_vm_steps;
        Ok(UpdateResult {
            outcome,
            root,
            work,
        })
    }

    pub(crate) fn orthogonal_advance(
        &mut self,
        job: &Job,
        name: &str,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<UpdateResult> {
        if name.is_empty() || name.len() > 256 {
            return Err(failure("invalid orthogonal operation name"));
        }
        let meter = vm_meter(&self.connection);
        let mut attempted = Measure::default();
        let result = self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut geometry = Geometry {
                db,
                job: id,
                scalar: 9,
                work: Work::default(),
            };
            let previous = geometry
                .state(name)?
                .ok_or_else(|| failure("orthogonal mutation is absent"))?;
            geometry.scalar = previous.root.scalar_bytes;
            let mut state = previous.clone();
            // Each unit is one indexed geometry or native Graph transition.
            // Cursor references and final root binding have their own reserve.
            if !state.complete {
                geometry.update_step(name, &mut state, combine)?;
            }
            let root = state.complete.then(|| Root {
                scalar_bytes: state.root.scalar_bytes,
                node: state.outer.result.clone(),
            });
            if let Some(root) = &root {
                geometry.bind(name, root)?;
            }
            let cursor_bytes = geometry.save(name, Some(&previous), &state)?;
            if geometry.work.rows() > STEP_ROWS || geometry.work.metadata_bytes > STEP_BYTES {
                return Err(failure("orthogonal durable step exceeds work budget"));
            }
            let resident_bytes =
                2 * cursor_bytes + geometry.work.max_live_node_bytes + 3 * NODE_BYTES;
            attempted = Measure {
                geometry: geometry.work,
                cursor_bytes,
                resident_bytes,
                ..Measure::default()
            };
            Ok((root, attempted.clone()))
        });
        let mut measure = Measure::default();
        finish_meter(&self.connection, &meter, &mut measure);
        let (outcome, output) = result?;
        let (root, mut work) = output.unwrap_or((None, attempted));
        work.connection_vm_steps = measure.connection_vm_steps;
        Ok(UpdateResult {
            outcome,
            root,
            work,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Edge {
    pub(crate) key: Vec<u8>,
    pub(crate) inclusive: bool,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Bounds {
    pub(crate) lower: Option<Edge>,
    pub(crate) upper: Option<Edge>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Rectangle {
    pub(crate) order: Bounds,
    pub(crate) scalar: Bounds,
}
enum Cover {
    Outside,
    Inside,
    Partial,
}
impl Bounds {
    fn intersect(&self, other: &Self) -> Self {
        let choose = |left: &Option<Edge>, right: &Option<Edge>, lower: bool| match (left, right) {
            (None, edge) | (edge, None) => edge.clone(),
            (Some(left), Some(right)) if left.key == right.key => Some(Edge {
                key: left.key.clone(),
                inclusive: left.inclusive && right.inclusive,
            }),
            (Some(left), Some(right)) => Some(if (left.key > right.key) == lower {
                left.clone()
            } else {
                right.clone()
            }),
        };
        Self {
            lower: choose(&self.lower, &other.lower, true),
            upper: choose(&self.upper, &other.upper, false),
        }
    }
    fn valid(&self, width: usize) -> bool {
        [&self.lower, &self.upper]
            .into_iter()
            .flatten()
            .all(|edge| edge.key.len() == width)
    }
    fn covers(&self, low: &[u8], high: &[u8]) -> Cover {
        let above_lower = |key: &[u8]| {
            self.lower.as_ref().is_none_or(|edge| {
                key > edge.key.as_slice() || (edge.inclusive && key == edge.key.as_slice())
            })
        };
        let below_upper = |key: &[u8]| {
            self.upper.as_ref().is_none_or(|edge| {
                key < edge.key.as_slice() || (edge.inclusive && key == edge.key.as_slice())
            })
        };
        if !above_lower(high) || !below_upper(low) {
            Cover::Outside
        } else if above_lower(low) && below_upper(high) {
            Cover::Inside
        } else {
            Cover::Partial
        }
    }
}

impl Geometry<'_> {
    fn aggregate(&mut self, root: &NodeId, included: u64) -> io::Result<String> {
        let mut graph = Graph {
            db: self.db,
            job: self.job,
            work: Work::default(),
        };
        let facts = graph.evaluate(root, included)?;
        self.add_graph_work(graph.work);
        Ok(facts)
    }
    fn inner_range(
        &mut self,
        root: &Id,
        bounds: &Bounds,
        included: u64,
        empty: &str,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        outer_stack: usize,
    ) -> io::Result<String> {
        let mut output = empty.to_owned();
        let mut stack = vec![root.clone()];
        while let Some(id) = stack.pop() {
            let node = self.read(&id)?;
            let (low, high) = node.range(1);
            match bounds.covers(&low[..self.scalar], &high[..self.scalar]) {
                Cover::Outside => {}
                Cover::Inside => {
                    let aggregate = node
                        .aggregate()
                        .ok_or_else(|| failure("orthogonal scalar aggregate absent"))?;
                    let facts = self.aggregate(aggregate, included)?;
                    self.work.max_live_node_bytes = self
                        .work
                        .max_live_node_bytes
                        .max(output.len() + facts.len());
                    output = combine(&output, &facts)?;
                    if output.len() > NODE_BYTES {
                        return Err(failure("orthogonal native range facts exceed budget"));
                    }
                }
                Cover::Partial => match node {
                    TreeNode::Branch {
                        bit: at, zero, one, ..
                    } => {
                        self.children(1, &zero, &one, at)?;
                        stack.push(one);
                        stack.push(zero);
                    }
                    TreeNode::Leaf { .. } => {
                        return Err(failure("orthogonal scalar leaf is partially covered"));
                    }
                },
            }
            self.work.max_stack = self.work.max_stack.max(outer_stack + stack.len());
            if stack.len() > width(1, self.scalar) * 8 + 1 {
                return Err(failure("orthogonal scalar range exceeds fixed key depth"));
            }
        }
        Ok(output)
    }
    fn range(
        &mut self,
        root: Option<&Id>,
        rectangle: &Rectangle,
        included: u64,
        empty: &str,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
    ) -> io::Result<String> {
        let mut output = empty.to_owned();
        let mut stack: Vec<Id> = root.into_iter().cloned().collect();
        while let Some(id) = stack.pop() {
            let node = self.read(&id)?;
            let (low, high) = node.range(0);
            match rectangle.order.covers(&low, &high) {
                Cover::Outside => {}
                Cover::Inside => {
                    let secondary = node
                        .secondary()
                        .ok_or_else(|| failure("orthogonal outer secondary absent"))?;
                    let facts = self.inner_range(
                        secondary,
                        &rectangle.scalar,
                        included,
                        empty,
                        combine,
                        stack.len(),
                    )?;
                    self.work.max_live_node_bytes = self
                        .work
                        .max_live_node_bytes
                        .max(output.len() + facts.len());
                    output = combine(&output, &facts)?;
                    if output.len() > NODE_BYTES {
                        return Err(failure("orthogonal native rectangle facts exceed budget"));
                    }
                }
                Cover::Partial => match node {
                    TreeNode::Branch {
                        bit: at, zero, one, ..
                    } => {
                        self.children(0, &zero, &one, at)?;
                        stack.push(one);
                        stack.push(zero);
                    }
                    TreeNode::Leaf { .. } => {
                        return Err(failure("orthogonal outer leaf is partially covered"));
                    }
                },
            }
            self.work.max_stack = self.work.max_stack.max(stack.len());
            if stack.len() > ORDER_BYTES * 8 + 1 {
                return Err(failure("orthogonal range exceeds fixed key depth"));
            }
        }
        Ok(output)
    }
    fn select(
        &mut self,
        root: Option<&Id>,
        rectangle: &Rectangle,
        mut rank: u64,
        included: u64,
        empty: &str,
        algebra: Algebra<'_>,
    ) -> io::Result<Option<Selected>> {
        let combine = algebra.combine;
        let count = algebra.count;
        let total = count(&self.range(root, rectangle, included, empty, combine)?)?;
        if rank >= total {
            return Ok(None);
        }
        let mut id = root
            .cloned()
            .ok_or_else(|| failure("orthogonal nonempty count has no root"))?;
        for _ in 0..=ORDER_BYTES * 8 {
            match self.read(&id)? {
                TreeNode::Leaf { point, .. } => {
                    return Ok(Some(Selected {
                        point,
                        within: rank,
                    }));
                }
                TreeNode::Branch {
                    bit: at, zero, one, ..
                } => {
                    self.children(0, &zero, &one, at)?;
                    let left =
                        count(&self.range(Some(&zero), rectangle, included, empty, combine)?)?;
                    if rank < left {
                        id = zero;
                    } else {
                        rank -= left;
                        id = one;
                    }
                }
            }
        }
        Err(failure("orthogonal select exceeds fixed key depth"))
    }
    fn bound(&mut self, name: &str, root: &Root) -> io::Result<bool> {
        let mut statement = self.db.prepare_cached("SELECT EXISTS(SELECT 1 FROM native_orthogonal_roots WHERE job=?1 AND name=?2 AND identity IS ?3)").map_err(io::Error::other)?;
        statement.reset_status(rusqlite::StatementStatus::VmStep);
        let bound = statement
            .query_row(
                params![self.job, name, root.node.as_ref().map(Id::identity)],
                |row| row.get(0),
            )
            .map_err(io::Error::other)?;
        self.work.sql_reads += 1;
        self.work.vm_steps += i64::from(statement.get_status(rusqlite::StatementStatus::VmStep));
        Ok(bound)
    }
}

/// Native keyed-extrema/count algebra; secondary order is not transcript order.
#[derive(Clone, Copy)]
pub(crate) struct Algebra<'a> {
    pub(crate) combine: &'a dyn Fn(&str, &str) -> io::Result<String>,
    pub(crate) count: &'a dyn Fn(&str) -> io::Result<u64>,
}

#[derive(Clone, Copy)]
pub(crate) struct ReadInput<'a> {
    pub(crate) name: &'a str,
    pub(crate) root: &'a Root,
    pub(crate) rectangle: &'a Rectangle,
    pub(crate) included: u64,
    pub(crate) empty: &'a str,
}

#[derive(Clone, Debug)]
pub(crate) struct Selected {
    pub(crate) point: Point,
    /// A native model weight may represent more than one item at a point.
    /// Keep the residual rank instead of silently choosing its first item.
    pub(crate) within: u64,
}

/// Private cleanup follows only this job's references and its resulting
/// garbage queue. Another live job's roots never hold this cleanup incomplete.
pub(super) fn cleanup(
    db: &Connection,
    job: i64,
    budget: usize,
    bytes: &mut usize,
) -> rusqlite::Result<(usize, bool)> {
    if budget == 0 {
        return Ok((0, false));
    }
    let mut used = 1;
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='native_orthogonal_gc')",[],|row|row.get::<_,bool>(0))? {
        return Ok((used,true));
    }
    loop {
        if used + 2 > budget {
            return Ok((used, false));
        }
        let frame: Option<(String, i64, i64, i64)> = db.query_row("SELECT name,family,depth,octet_length(metadata)+octet_length(name)+32 FROM native_orthogonal_frames WHERE job=?1 ORDER BY name,family,depth LIMIT 1", [job], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
        used += 1;
        let Some((name, family, depth, length)) = frame else {
            break;
        };
        let length =
            usize::try_from(length).map_err(|_| invalid("invalid orthogonal cleanup length"))?;
        if length > *bytes {
            return Ok((used, false));
        }
        db.execute("DELETE FROM native_orthogonal_frames WHERE job=?1 AND name=?2 AND family=?3 AND depth=?4",params![job,name,family,depth])?;
        used += 1;
        *bytes -= length;
    }
    loop {
        if used + 2 > budget {
            return Ok((used, false));
        }
        let record: Option<(String,Option<String>)> = db.query_row("SELECT name,CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_orthogonal_work WHERE job=?1 ORDER BY name LIMIT 1",[job],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        used += 1;
        let Some((name, text)) = record else {
            break;
        };
        let text = text.ok_or_else(|| invalid("orthogonal cleanup cursor exceeds budget"))?;
        let state: Update = serde_json::from_str(&text)
            .map_err(|_| invalid("orthogonal cleanup cursor is invalid"))?;
        if !state.valid() {
            return Err(invalid("orthogonal cleanup cursor is invalid"));
        }
        let refs = state.refs();
        let length = text.len() + name.len();
        if used + 1 + 2 * refs.len() > budget || length > *bytes {
            return Ok((used, false));
        }
        for id in refs.values() {
            if db.execute(
                "UPDATE native_orthogonal_nodes SET refs=refs-1 WHERE identity=?1",
                [id.identity()],
            )? != 1
            {
                return Err(invalid("orthogonal cleanup dependency absent"));
            }
            db.execute("INSERT INTO native_orthogonal_gc SELECT ?1,identity FROM native_orthogonal_nodes WHERE identity=?2 AND refs=0 ON CONFLICT(job,identity) DO NOTHING",params![job,id.identity()])?;
            used += 2;
        }
        db.execute(
            "DELETE FROM native_orthogonal_work WHERE job=?1 AND name=?2",
            params![job, name],
        )?;
        used += 1;
        *bytes -= length;
    }
    loop {
        if used + 4 > budget {
            return Ok((used, false));
        }
        let record: Option<(String,Option<String>)> = db.query_row("SELECT name,identity FROM native_orthogonal_roots WHERE job=?1 ORDER BY name LIMIT 1",[job],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        used += 1;
        let Some((name, identity)) = record else {
            break;
        };
        let length = name.len() + identity.as_ref().map_or(0, String::len);
        if length > *bytes {
            return Ok((used, false));
        }
        if let Some(identity) = identity {
            if db.execute(
                "UPDATE native_orthogonal_nodes SET refs=refs-1 WHERE identity=?1",
                [&identity],
            )? != 1
            {
                return Err(invalid("orthogonal cleanup root absent"));
            }
            db.execute("INSERT INTO native_orthogonal_gc SELECT ?1,identity FROM native_orthogonal_nodes WHERE identity=?2 AND refs=0 ON CONFLICT(job,identity) DO NOTHING",params![job,identity])?;
            used += 2;
        }
        db.execute(
            "DELETE FROM native_orthogonal_roots WHERE job=?1 AND name=?2",
            params![job, name],
        )?;
        used += 1;
        *bytes -= length;
    }
    loop {
        // A reclaimed node has at most three geometry edges and one Graph edge.
        if used + 10 > budget {
            return Ok((used, false));
        }
        let record: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>)> = db.query_row("SELECT g.identity,n.zero,n.one,n.secondary,n.aggregate,n.refs FROM native_orthogonal_gc g LEFT JOIN native_orthogonal_nodes n ON n.identity=g.identity WHERE g.job=?1 ORDER BY g.identity LIMIT 1",[job],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
        used += 1;
        let Some((identity, zero, one, secondary, aggregate, refs)) = record else {
            return Ok((used, true));
        };
        let length = identity.len()
            + 8
            + [&zero, &one, &secondary, &aggregate]
                .into_iter()
                .flatten()
                .map(String::len)
                .sum::<usize>();
        if length > *bytes {
            return Ok((used, false));
        }
        db.execute(
            "DELETE FROM native_orthogonal_gc WHERE job=?1 AND identity=?2",
            params![job, identity],
        )?;
        used += 1;
        if refs == Some(0) {
            db.execute(
                "DELETE FROM native_orthogonal_nodes WHERE identity=?1 AND refs=0",
                [&identity],
            )?;
            used += 1;
            for child in [zero, one, secondary].into_iter().flatten() {
                if db.execute(
                    "UPDATE native_orthogonal_nodes SET refs=refs-1 WHERE identity=?1",
                    [&child],
                )? != 1
                {
                    return Err(invalid("orthogonal cleanup child absent"));
                }
                db.execute("INSERT INTO native_orthogonal_gc SELECT ?1,identity FROM native_orthogonal_nodes WHERE identity=?2 AND refs=0 ON CONFLICT(job,identity) DO NOTHING",params![job,child])?;
                used += 2;
            }
            if let Some(aggregate) = aggregate {
                if db.execute(
                    "UPDATE native_rank_nodes SET refs=refs-1 WHERE identity=?1",
                    [aggregate],
                )? != 1
                {
                    return Err(invalid("orthogonal cleanup Graph child absent"));
                }
                used += 1;
            }
        }
        *bytes -= length;
    }
}

/// A refused qualification still reports the work already attempted. For a
/// selection, value=Some(None) is qualified absence; value=None is refusal.
pub(crate) struct ReadAttempt<T> {
    pub(crate) value: Option<T>,
    pub(crate) work: Measure,
}
#[cfg(test)]
impl<T> ReadAttempt<T> {
    fn qualified(self) -> Option<(T, Measure)> {
        self.value.map(|value| (value, self.work))
    }
}

impl NativeStore {
    /// Provider/body callers must recheck the same job and closure after I/O.
    fn orthogonal_read<T>(
        &self,
        job: &Job,
        input: ReadInput<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        operation: impl FnOnce(&mut Geometry<'_>) -> io::Result<T>,
    ) -> io::Result<ReadAttempt<T>> {
        if input.name.is_empty()
            || input.name.len() > 256
            || !input.root.valid()
            || !input.rectangle.order.valid(ORDER_BYTES)
            || !input.rectangle.scalar.valid(input.root.scalar_bytes)
            || input.empty.len() > NODE_BYTES
        {
            return Err(failure("invalid orthogonal read bounds"));
        }
        let meter = vm_meter(&self.connection);
        let mut attempted = Measure::default();
        let result = (|| {
            if job.phase != "ready" || !self.runtime_current(&job.spec)? {
                return Ok(None);
            }
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            if !native_query::job_current(&tx, job).map_err(io::Error::other)?
                || !closure(&tx, job.id).map_err(io::Error::other)?
            {
                return Ok(None);
            }
            let mut geometry = Geometry {
                db: &tx,
                job: job.id,
                scalar: input.root.scalar_bytes,
                work: Work::default(),
            };
            if !geometry.bound(input.name, input.root)? {
                attempted.geometry = geometry.work;
                return Ok(None);
            }
            let output = operation(&mut geometry)?;
            attempted = Measure {
                resident_bytes: geometry.work.max_stack * (std::mem::size_of::<Id>() + 64)
                    + geometry.work.max_live_node_bytes
                    + 2 * NODE_BYTES,
                geometry: geometry.work,
                ..Measure::default()
            };
            if !native_query::job_current(&tx, job).map_err(io::Error::other)?
                || !closure(&tx, job.id).map_err(io::Error::other)?
                || !self.runtime_current(&job.spec)?
            {
                return Ok(None);
            }
            tx.commit().map_err(io::Error::other)?;
            // Releasing the read snapshot is necessary: a peer commit during
            // metadata I/O must not pass both checks in an old WAL snapshot.
            if !self.recheck(job)?
                || !closure(&self.connection, job.id).map_err(io::Error::other)?
            {
                return Ok(None);
            }
            Ok(Some(output))
        })();
        finish_meter(&self.connection, &meter, &mut attempted);
        result.map(|value| ReadAttempt {
            value,
            work: attempted,
        })
    }
    pub(crate) fn orthogonal_rectangle(
        &self,
        job: &Job,
        input: ReadInput<'_>,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    ) -> io::Result<ReadAttempt<String>> {
        self.orthogonal_read(job, ReadInput { ..input }, closure, |geometry| {
            geometry.range(
                input.root.node.as_ref(),
                input.rectangle,
                input.included,
                input.empty,
                combine,
            )
        })
    }
    pub(crate) fn orthogonal_select(
        &self,
        job: &Job,
        input: ReadInput<'_>,
        rank: u64,
        algebra: Algebra<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    ) -> io::Result<ReadAttempt<Option<Selected>>> {
        self.orthogonal_read(job, ReadInput { ..input }, closure, |geometry| {
            geometry.select(
                input.root.node.as_ref(),
                input.rectangle,
                rank,
                input.included,
                input.empty,
                algebra,
            )
        })
    }
    pub(crate) fn orthogonal_rank(
        &self,
        job: &Job,
        input: ReadInput<'_>,
        edge: &Edge,
        algebra: Algebra<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    ) -> io::Result<ReadAttempt<u64>> {
        let count = algebra.count;
        let combine = algebra.combine;
        let rectangle = Rectangle {
            order: input.rectangle.order.intersect(&Bounds {
                lower: None,
                upper: Some(edge.clone()),
            }),
            scalar: input.rectangle.scalar.clone(),
        };
        let ranged = ReadInput {
            rectangle: &rectangle,
            ..input
        };
        self.orthogonal_read(job, ranged, closure, |geometry| {
            count(&geometry.range(
                input.root.node.as_ref(),
                &rectangle,
                input.included,
                input.empty,
                combine,
            )?)
        })
    }
    pub(crate) fn orthogonal_seek(
        &self,
        job: &Job,
        input: ReadInput<'_>,
        edge: &Edge,
        forward: bool,
        algebra: Algebra<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    ) -> io::Result<ReadAttempt<Option<Selected>>> {
        let count = algebra.count;
        let combine = algebra.combine;
        let order = Bounds {
            lower: forward.then(|| edge.clone()),
            upper: (!forward).then(|| edge.clone()),
        };
        let rectangle = Rectangle {
            order: input.rectangle.order.intersect(&order),
            scalar: input.rectangle.scalar.clone(),
        };
        self.orthogonal_read(
            job,
            ReadInput {
                rectangle: &rectangle,
                ..input
            },
            closure,
            |geometry| {
                let total = count(&geometry.range(
                    input.root.node.as_ref(),
                    &rectangle,
                    input.included,
                    input.empty,
                    combine,
                )?)?;
                if total == 0 {
                    return Ok(None);
                }
                geometry.select(
                    input.root.node.as_ref(),
                    &rectangle,
                    if forward { 0 } else { total - 1 },
                    input.included,
                    input.empty,
                    algebra,
                )
            },
        )
    }
    pub(crate) fn orthogonal_import(
        &mut self,
        job: &Job,
        name: &str,
        root: &Root,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<UpdateResult> {
        if name.is_empty() || name.len() > 256 || !root.valid() {
            return Err(failure("invalid orthogonal root import"));
        }
        let meter = vm_meter(&self.connection);
        let mut attempted = Measure::default();
        let result = self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut geometry = Geometry {
                db,
                job: id,
                scalar: root.scalar_bytes,
                work: Work::default(),
            };
            if let Some(root) = &root.node {
                geometry.read(root)?;
            }
            geometry.bind(name, root)?;
            if geometry.work.rows() > STEP_ROWS || geometry.work.metadata_bytes > STEP_BYTES {
                return Err(failure("orthogonal import exceeds work budget"));
            }
            attempted = Measure {
                geometry: geometry.work,
                ..Measure::default()
            };
            Ok(attempted.clone())
        });
        let mut measure = Measure::default();
        finish_meter(&self.connection, &meter, &mut measure);
        let (outcome, output) = result?;
        let mut work = output.unwrap_or(attempted);
        work.connection_vm_steps = measure.connection_vm_steps;
        Ok(UpdateResult {
            outcome,
            root: (outcome == Outcome::Written).then(|| root.clone()),
            work,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{EventCache, Kind},
        model::{
            native_stage::{Entry, EntryKind},
            range_stage::{Facts, Leaf, Order},
            summary::CatalogRow,
        },
    };
    use std::cell::Cell;

    fn combine(left: &str, right: &str) -> io::Result<String> {
        let mut left: Facts = serde_json::from_str(left).map_err(io::Error::other)?;
        left.merge(serde_json::from_str(right).map_err(io::Error::other)?)
            .map_err(failure)?;
        serde_json::to_string(&left).map_err(io::Error::other)
    }
    fn count(text: &str) -> io::Result<u64> {
        Ok(serde_json::from_str::<Facts>(text)
            .map_err(io::Error::other)?
            .count)
    }
    fn expiry(value: i64) -> Vec<u8> {
        let mut result = vec![1];
        result.extend_from_slice(&((value as u64) ^ (1 << 63)).to_be_bytes());
        result
    }
    fn native(seq: usize) -> Facts {
        let entry = Entry {
            kind: EntryKind::A,
            file: seq % 3,
            offset: seq as u64,
            pos: (seq as u64, 0),
            t: Some(seq as i64),
            at: Some((seq % 3, seq)),
        };
        let event = Event {
            k: Kind::A,
            t: entry.t,
            ..Default::default()
        };
        let mut facts = Facts::default();
        facts
            .observe(Leaf {
                order: order(seq),
                entry: &entry,
                found: Some(&event),
                outgoing: None,
                boundary: false,
                count: 1,
            })
            .unwrap();
        facts
    }
    fn order(seq: usize) -> Order {
        Order {
            key: (seq as i64, seq % 3, seq as u64, 0, 1),
            ordinal: seq as u64,
            identity: Sha256::digest(seq.to_be_bytes()).into(),
        }
    }

    struct Fixture {
        home: PathBuf,
        store: NativeStore,
        job: Job,
        row: CatalogRow,
        identity: NodeId,
    }
    impl Fixture {
        fn new() -> Self {
            let home = std::env::temp_dir().join(format!(
                "semon-orthogonal-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let options = crate::Options {
                claude_home: home.join("claude"),
                claude_json: home.join("claude.json"),
                codex_home: home.join("codex"),
                copilot_home: home.join("copilot"),
                cache: home.join("cache"),
                proc_root: home.join("proc"),
                ..Default::default()
            };
            let path = options.claude_home.join("projects/p/chosen.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"fixture\n").unwrap();
            let ledger = Ledger {
                stat: Stat::of(&path).unwrap(),
                offset: 8,
                prefix: Sha256::digest(b"fixture\n").into(),
                tail: Sha256::digest(b"fixture\n").into(),
            };
            let (mut writer, _) =
                SqliteStore::attempt_scoped(&EventCache::path(&options.cache), None, true)
                    .unwrap_or_else(|error| panic!("fixture store must open: {error}"));
            assert_eq!(
                writer
                    .write_one(
                        path.to_str().unwrap(),
                        None,
                        None,
                        &ledger,
                        &FileIndex::default()
                    )
                    .unwrap(),
                Outcome::Written
            );
            writer.connection.execute_batch(SCHEMA).unwrap();
            let row: CatalogRow = serde_json::from_value(serde_json::json!({
                "key":"chosen","name":"chosen","harness":"claude","kind":"session",
                "native_ids":["chosen"],"parent":null,"parent_source":null,
                "repo":null,"branch":null,"model":"test","effort":null,"start":0,"last":0,
                "tokens":[0.0,0.0,0.0],"cost":crate::pricing::Cost::default(),
                "sources":[ledger.source(&path,"chosen")]
            }))
            .unwrap();
            writer.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,0,'claude',?2)",params![row.key,serde_json::to_string(&row).unwrap()]).unwrap();
            let mut store = NativeStore {
                connection: writer.connection,
                options,
            };
            let spec = native_query::JobSpec {
                view_key: "geometry-one".into(),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&store.options).unwrap(),
            };
            let job = store.begin(&spec).unwrap();
            let source = store.source_witness(&row.sources[0]).unwrap();
            let owner = store.owner_witness(&row).unwrap();
            assert_eq!(
                store.admit(&job, &[source], &[owner], "complete").unwrap(),
                Outcome::Written
            );
            let job = store.reload(&spec.view_key).unwrap().unwrap();
            let identity = Graph {
                db: &store.connection,
                job: job.id,
                work: Work::default(),
            }
            .intern(Node::Terminal {
                facts: serde_json::to_string(&Facts::default()).unwrap(),
            })
            .unwrap();
            Self {
                home,
                store,
                job,
                row,
                identity,
            }
        }
        fn reload(&mut self) {
            self.job = self.store.reload(&self.job.spec.view_key).unwrap().unwrap();
        }
        fn point(&self, seq: usize) -> Point {
            let mut graph = Graph {
                db: &self.store.connection,
                job: self.job.id,
                work: Work::default(),
            };
            let terminal = graph
                .intern(Node::Terminal {
                    facts: serde_json::to_string(&native(seq)).unwrap(),
                })
                .unwrap();
            let facts = match seq % 3 {
                0 => graph
                    .intern(Node::Decision {
                        source: 0,
                        absent: self.identity.clone(),
                        present: terminal,
                    })
                    .unwrap(),
                1 => graph
                    .intern(Node::Decision {
                        source: 1,
                        absent: terminal,
                        present: self.identity.clone(),
                    })
                    .unwrap(),
                _ => graph
                    .intern(Node::Decision {
                        source: 63,
                        absent: self.identity.clone(),
                        present: terminal,
                    })
                    .unwrap(),
            };
            Point {
                order: order(seq).encoded().to_vec(),
                scalar: expiry((seq % 19) as i64 - 9),
                candidate: order(seq).identity,
                facts,
            }
        }
        // Fixture construction is outside read/append counters. Real durable
        // mutation attempts below use the public fenced one-step APIs.
        fn populate(&mut self, earlier: usize) -> Root {
            // This complete fixture constructor is deliberately independent of
            // the point-update cursor being measured. Each canonical range is
            // built directly from complete native inputs, with no warmup path
            // available to production reads.
            fn graph(owner: &mut Graph<'_>, facts: &[Facts; 8], at: usize, mask: usize) -> NodeId {
                if at == 3 {
                    return owner
                        .intern(Node::Terminal {
                            facts: serde_json::to_string(&facts[mask]).unwrap(),
                        })
                        .unwrap();
                }
                let absent = graph(owner, facts, at + 1, mask);
                let present = graph(owner, facts, at + 1, mask | (1 << at));
                owner
                    .intern(Node::Decision {
                        source: [0, 1, 63][at],
                        absent,
                        present,
                    })
                    .unwrap()
            }
            fn tree(
                geometry: &mut Geometry<'_>,
                native_graph: &mut Graph<'_>,
                points: &[&(usize, Point)],
                axis: u8,
            ) -> (Id, [Facts; 8]) {
                if points.len() == 1 {
                    let (seq, point) = points[0];
                    let facts = std::array::from_fn(|mask| {
                        let included = (mask as u64 & 3) | (((mask as u64 >> 2) & 1) << 63);
                        if allowed(*seq, included) {
                            native(*seq)
                        } else {
                            Facts::default()
                        }
                    });
                    return (geometry.leaf(axis, point.clone()).unwrap(), facts);
                }
                let low = points.first().unwrap().1.key(axis);
                let high = points.last().unwrap().1.key(axis);
                let at = difference(&low, &high).unwrap();
                let split = points.partition_point(|(_, point)| !bit(&point.key(axis), at));
                let (zero, left) = tree(geometry, native_graph, &points[..split], axis);
                let (one, right) = tree(geometry, native_graph, &points[split..], axis);
                let facts = std::array::from_fn(|index| {
                    let mut facts = left[index].clone();
                    facts.merge(right[index].clone()).unwrap();
                    facts
                });
                let (secondary, aggregate) = if axis == 0 {
                    let mut inner = points.to_vec();
                    inner.sort_by_key(|(_, point)| point.key(1));
                    (Some(tree(geometry, native_graph, &inner, 1).0), None)
                } else {
                    (None, Some(graph(native_graph, &facts, 0, 0)))
                };
                (
                    geometry
                        .intern(
                            axis,
                            TreeNode::Branch {
                                bit: at,
                                low,
                                high,
                                zero,
                                one,
                                secondary,
                                aggregate,
                            },
                        )
                        .unwrap(),
                    facts,
                )
            }
            let points: Vec<_> = (0..=earlier).map(|seq| (seq, self.point(seq))).collect();
            let tx = self.store.connection.unchecked_transaction().unwrap();
            let mut geometry = Geometry {
                db: &tx,
                job: self.job.id,
                scalar: 9,
                work: Work::default(),
            };
            let mut native_graph = Graph {
                db: &tx,
                job: self.job.id,
                work: Work::default(),
            };
            let refs: Vec<_> = points.iter().collect();
            let root = Root {
                scalar_bytes: 9,
                node: Some(tree(&mut geometry, &mut native_graph, &refs, 0).0),
            };
            geometry.bind("native", &root).unwrap();
            tx.commit().unwrap();
            root
        }
        fn deep_geometry(&mut self) -> (Root, Point) {
            let tx = self.store.connection.unchecked_transaction().unwrap();
            let mut geometry = Geometry {
                db: &tx,
                job: self.job.id,
                scalar: 12,
                work: Work::default(),
            };
            let mut graph = Graph {
                db: &tx,
                job: self.job.id,
                work: Work::default(),
            };
            let point =
                |seq: usize, order: Vec<u8>, scalar: Vec<u8>, graph: &mut Graph<'_>| Point {
                    candidate: Sha256::digest(&order).into(),
                    order,
                    scalar,
                    facts: graph
                        .intern(Node::Terminal {
                            facts: serde_json::to_string(&native(seq)).unwrap(),
                        })
                        .unwrap(),
                };
            let base = point(0, vec![0; ORDER_BYTES], vec![0; 12], &mut graph);
            let mut facts = native(0);
            let mut root = geometry.leaf(0, base.clone()).unwrap();
            let mut secondary = geometry.read(&root).unwrap().secondary().unwrap().clone();
            // Canonical outer combs and their secondary combs share the same
            // complete member sets. No inconsistent placeholder aggregate is
            // used to manufacture the maximum outer depth.
            for at in (0..ORDER_BYTES * 8).rev() {
                let mut order = vec![0; ORDER_BYTES];
                order[at / 8] = 0x80 >> (at % 8);
                let next = point(ORDER_BYTES * 8 - at, order, vec![0; 12], &mut graph);
                facts.merge(native(ORDER_BYTES * 8 - at)).unwrap();
                let leaf = geometry.leaf(0, next.clone()).unwrap();
                let inner_leaf = geometry.read(&leaf).unwrap().secondary().unwrap().clone();
                secondary = geometry
                    .intern(
                        1,
                        TreeNode::Branch {
                            bit: 12 * 8 + at,
                            low: base.key(1),
                            high: next.key(1),
                            zero: secondary,
                            one: inner_leaf,
                            secondary: None,
                            aggregate: Some(
                                graph
                                    .intern(Node::Terminal {
                                        facts: serde_json::to_string(&facts).unwrap(),
                                    })
                                    .unwrap(),
                            ),
                        },
                    )
                    .unwrap();
                root = geometry
                    .intern(
                        0,
                        TreeNode::Branch {
                            bit: at,
                            low: base.order.clone(),
                            high: next.order.clone(),
                            zero: root,
                            one: leaf,
                            secondary: Some(secondary.clone()),
                            aggregate: None,
                        },
                    )
                    .unwrap();
            }
            let mut root = Root {
                scalar_bytes: 12,
                node: Some(root),
            };
            // Distinct outer addresses for every scalar prefix split preserve
            // the unique native point address while attaining all 648 inner bits.
            for at in (0..12 * 8).rev() {
                let mut order = vec![0; ORDER_BYTES];
                order[0] = 255;
                order[1] = at as u8 + 1;
                let mut scalar = vec![0; 12];
                scalar[at / 8] = 0x80 >> (at % 8);
                let next = point(ORDER_BYTES * 8 + 1 + at, order, scalar, &mut graph);
                let mut update = Update {
                    root: root.clone(),
                    target: next.order.clone(),
                    point: Some(next.clone()),
                    version: "native-facts-merge-v1".into(),
                    identity: self.identity.clone(),
                    outer: TreeWalk::new(0, root.node.clone(), next.order.clone(), Some(next)),
                    inner: None,
                    graph: None,
                    complete: false,
                };
                let previous = geometry.state("deep-fixture").unwrap();
                geometry
                    .save("deep-fixture", previous.as_ref(), &update)
                    .unwrap();
                for _ in 0..100000 {
                    let previous = update.clone();
                    geometry
                        .update_step("deep-fixture", &mut update, &combine)
                        .unwrap();
                    geometry
                        .save("deep-fixture", Some(&previous), &update)
                        .unwrap();
                    if update.complete {
                        break;
                    }
                }
                assert!(update.complete);
                root.node = update.outer.result;
            }
            geometry.bind("native", &root).unwrap();
            tx.commit().unwrap();
            (root, base)
        }
        fn mutate(&mut self, root: &Root, key: &[u8], point: Option<&Point>) -> (Root, Work) {
            let result = self
                .store
                .orthogonal_begin(
                    &self.job,
                    Mutation {
                        name: "native",
                        root,
                        order: key,
                        point,
                        version: "native-facts-merge-v1",
                        identity: &self.identity,
                    },
                    &|_, _| Ok(true),
                    &|| false,
                )
                .unwrap();
            assert_eq!(result.outcome, Outcome::Written);
            self.reload();
            let mut total = Work::default();
            for _ in 0..2_000_000 {
                let result = self
                    .store
                    .orthogonal_advance(&self.job, "native", &combine, &|_, _| Ok(true), &|| false)
                    .unwrap();
                assert_eq!(result.outcome, Outcome::Written);
                assert!(result.work.geometry.rows() <= STEP_ROWS);
                assert!(result.work.geometry.metadata_bytes <= STEP_BYTES);
                assert!(result.work.cursor_bytes <= NODE_BYTES);
                assert!(result.work.resident_bytes <= STEP_BYTES);
                assert!(result.work.connection_vm_steps >= result.work.geometry.vm_steps as u64);
                total.sql_reads += result.work.geometry.sql_reads;
                total.sql_writes += result.work.geometry.sql_writes;
                total.apply_visits += result.work.geometry.apply_visits;
                total.nodes_written += result.work.geometry.nodes_written;
                self.reload();
                if let Some(root) = result.root {
                    return (root, total);
                }
            }
            panic!("durable geometry failed eventual progress");
        }
        fn qualify(&mut self) {
            assert_eq!(
                self.store
                    .checkpoint(
                        &self.job,
                        "finish",
                        "",
                        &serde_json::json!({}),
                        &[],
                        &|| false
                    )
                    .unwrap(),
                Outcome::Written
            );
            self.reload();
            assert_eq!(
                self.store
                    .qualify(
                        &self.job,
                        &serde_json::json!({"geometry":1}),
                        &|_, _| Ok(true),
                        &|| false
                    )
                    .unwrap(),
                Outcome::Written
            );
            self.reload();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    fn allowed(seq: usize, mask: u64) -> bool {
        match seq % 3 {
            0 => mask & 1 != 0,
            1 => mask & 2 == 0,
            _ => mask & (1u64 << 63) != 0,
        }
    }
    fn reference(earlier: usize, mask: u64, rectangle: &Rectangle) -> Facts {
        let mut expected = Facts::default();
        for seq in 0..=earlier {
            let order = order(seq).encoded();
            let scalar = expiry((seq % 19) as i64 - 9);
            let inside = |bounds: &Bounds, key: &[u8]| {
                bounds.lower.as_ref().is_none_or(|edge| {
                    if edge.inclusive {
                        key >= edge.key.as_slice()
                    } else {
                        key > edge.key.as_slice()
                    }
                }) && bounds.upper.as_ref().is_none_or(|edge| {
                    if edge.inclusive {
                        key <= edge.key.as_slice()
                    } else {
                        key < edge.key.as_slice()
                    }
                })
            };
            if allowed(seq, mask)
                && inside(&rectangle.order, &order)
                && inside(&rectangle.scalar, &scalar)
            {
                expected.merge(native(seq)).unwrap();
            }
        }
        expected
    }

    #[test]
    fn orthogonal_native_rectangles_and_fixed_page_match_independent_reference_as_history_grows() {
        let mut rows = Vec::new();
        let mut appends = Vec::new();
        for earlier in [0, 128, 1024, 8192] {
            let mut fixture = Fixture::new();
            let root = fixture.populate(earlier);
            let point = fixture.point(8193);
            let (root, work) = fixture.mutate(&root, &point.order, Some(&point));
            appends.push(work.rows());
            fixture.qualify();
            let full_reads = crate::events::FILE_READS.with(Cell::get);
            let rectangle = Rectangle {
                order: Bounds::default(),
                scalar: Bounds {
                    lower: Some(Edge {
                        key: expiry(-3),
                        inclusive: true,
                    }),
                    upper: Some(Edge {
                        key: expiry(4),
                        inclusive: false,
                    }),
                },
            };
            for mask in [u64::MAX, u64::MAX ^ 2, 0, 1] {
                let mut expected = reference(earlier, mask, &rectangle);
                if allowed(8193, mask) && (-3..4).contains(&((8193 % 19) as i64 - 9)) {
                    expected.merge(native(8193)).unwrap();
                }
                let input = ReadInput {
                    name: "native",
                    root: &root,
                    rectangle: &rectangle,
                    included: mask,
                    empty: &serde_json::to_string(&Facts::default()).unwrap(),
                };
                let (actual, work) = fixture
                    .store
                    .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
                    .unwrap()
                    .qualified()
                    .unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&actual).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
                assert!(work.connection_vm_steps >= work.geometry.vm_steps as u64);
                assert!(work.geometry.sql_reads <= 100_000);
                assert!(work.resident_bytes <= STEP_BYTES);
            }
            let tail = Rectangle {
                order: Bounds {
                    lower: Some(Edge {
                        key: point.order.clone(),
                        inclusive: true,
                    }),
                    upper: None,
                },
                scalar: Bounds::default(),
            };
            let input = ReadInput {
                name: "native",
                root: &root,
                rectangle: &tail,
                included: u64::MAX,
                empty: &serde_json::to_string(&Facts::default()).unwrap(),
            };
            let (selected, work) = fixture
                .store
                .orthogonal_select(
                    &fixture.job,
                    input,
                    0,
                    Algebra {
                        combine: &combine,
                        count: &count,
                    },
                    &|_, _| Ok(true),
                )
                .unwrap()
                .qualified()
                .unwrap();
            let selected = selected.unwrap();
            assert_eq!(selected.point.candidate, point.candidate);
            assert_eq!(selected.within, 0);
            assert!(
                fixture
                    .store
                    .orthogonal_select(
                        &fixture.job,
                        input,
                        1,
                        Algebra {
                            combine: &combine,
                            count: &count
                        },
                        &|_, _| Ok(true)
                    )
                    .unwrap()
                    .qualified()
                    .unwrap()
                    .0
                    .is_none()
            );
            rows.push(work.geometry.sql_reads);
            assert_eq!(crate::events::FILE_READS.with(Cell::get), full_reads);
        }
        // Fixed-width paths bound measured work; these checks also distinguish
        // a prefix scan at8192 from tree traversal instead of merely asserting
        // that some counters were nonzero.
        assert!(
            rows.iter().all(|rows| *rows <= 2048),
            "fixed page rows: {rows:?}"
        );
        assert!(
            rows[3] <= 4 * rows[2] + 32,
            "fixed page became a prefix scan: {rows:?}"
        );
        assert!(
            appends.iter().all(|rows| *rows <= 500_000),
            "point path work: {appends:?}"
        );
        assert!(
            appends[3] <= 4 * appends[2] + 128,
            "append became a prefix rewrite: {appends:?}"
        );
    }

    #[test]
    fn orthogonal_bounds_rank_seek_replacement_delete_and_literal_extremes_are_exact() {
        let mut fixture = Fixture::new();
        let root = fixture.populate(8);
        let mut changed = fixture.point(4);
        changed.scalar = [vec![2], vec![0; 8]].concat();
        let (root, _) = fixture.mutate(&root, &changed.order, Some(&changed));
        let (root, _) = fixture.mutate(&root, &order(3).encoded(), None);
        let mut never = fixture.point(9);
        never.scalar = vec![0; 9];
        let (root, _) = fixture.mutate(&root, &never.order, Some(&never));
        let mut always = fixture.point(10);
        always.scalar = [vec![2], vec![0; 8]].concat();
        let (root, _) = fixture.mutate(&root, &always.order, Some(&always));
        fixture.qualify();
        let empty = serde_json::to_string(&Facts::default()).unwrap();
        let rectangle = Rectangle {
            order: Bounds::default(),
            scalar: Bounds {
                lower: Some(Edge {
                    key: [vec![2], vec![0; 8]].concat(),
                    inclusive: true,
                }),
                upper: None,
            },
        };
        let input = ReadInput {
            name: "native",
            root: &root,
            rectangle: &rectangle,
            included: u64::MAX ^ 2,
            empty: &empty,
        };
        let (facts, _) = fixture
            .store
            .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
            .unwrap()
            .qualified()
            .unwrap();
        assert_eq!(count(&facts).unwrap(), 2);
        for (rank, seq) in [(0, 4), (1, 10)] {
            let (selected, _) = fixture
                .store
                .orthogonal_select(
                    &fixture.job,
                    input,
                    rank,
                    Algebra {
                        combine: &combine,
                        count: &count,
                    },
                    &|_, _| Ok(true),
                )
                .unwrap()
                .qualified()
                .unwrap();
            assert_eq!(selected.unwrap().point.order, order(seq).encoded());
        }
        for inclusive in [false, true] {
            let edge = Edge {
                key: changed.order.clone(),
                inclusive,
            };
            assert_eq!(
                fixture
                    .store
                    .orthogonal_rank(
                        &fixture.job,
                        input,
                        &edge,
                        Algebra {
                            combine: &combine,
                            count: &count
                        },
                        &|_, _| Ok(true)
                    )
                    .unwrap()
                    .qualified()
                    .unwrap()
                    .0,
                u64::from(inclusive)
            );
            let (next, _) = fixture
                .store
                .orthogonal_seek(
                    &fixture.job,
                    input,
                    &edge,
                    true,
                    Algebra {
                        combine: &combine,
                        count: &count,
                    },
                    &|_, _| Ok(true),
                )
                .unwrap()
                .qualified()
                .unwrap();
            assert_eq!(
                next.unwrap().point.order,
                order(if inclusive { 4 } else { 10 }).encoded()
            );
            let (previous, _) = fixture
                .store
                .orthogonal_seek(
                    &fixture.job,
                    input,
                    &edge,
                    false,
                    Algebra {
                        combine: &combine,
                        count: &count,
                    },
                    &|_, _| Ok(true),
                )
                .unwrap()
                .qualified()
                .unwrap();
            assert_eq!(
                previous.map(|point| point.point.order),
                inclusive.then(|| changed.order.clone())
            );
        }
        let rectangle = Rectangle {
            order: Bounds::default(),
            scalar: Bounds {
                lower: None,
                upper: Some(Edge {
                    key: vec![0; 9],
                    inclusive: true,
                }),
            },
        };
        let input = ReadInput {
            rectangle: &rectangle,
            ..input
        };
        let (point, _) = fixture
            .store
            .orthogonal_select(
                &fixture.job,
                input,
                0,
                Algebra {
                    combine: &combine,
                    count: &count,
                },
                &|_, _| Ok(true),
            )
            .unwrap()
            .qualified()
            .unwrap();
        assert_eq!(point.unwrap().point.candidate, never.candidate);
    }

    #[test]
    fn orthogonal_cancel_reopen_generation_fences_and_shared_root_cleanup_preserve_custody() {
        let mut fixture = Fixture::new();
        let root = fixture.populate(16);
        let point = fixture.point(50);
        let before: Vec<(String, i64)> = fixture
            .store
            .connection
            .prepare("SELECT identity,refs FROM native_orthogonal_nodes ORDER BY identity")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let calls = Cell::new(0);
        let cancelled = || {
            let value = calls.get() + 1;
            calls.set(value);
            value >= 2
        };
        let result = fixture
            .store
            .orthogonal_begin(
                &fixture.job,
                Mutation {
                    name: "cancel",
                    root: &root,
                    order: &point.order,
                    point: Some(&point),
                    version: "native-facts-merge-v1",
                    identity: &fixture.identity,
                },
                &|_, _| Ok(true),
                &cancelled,
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Deferred);
        assert!(
            result.work.geometry.rows() > 0,
            "rolled-back attempted work was lost"
        );
        let after: Vec<(String, i64)> = fixture
            .store
            .connection
            .prepare("SELECT identity,refs FROM native_orthogonal_nodes ORDER BY identity")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(before, after);
        assert!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM native_orthogonal_work WHERE name='cancel')",
                    [],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
        let result = fixture
            .store
            .orthogonal_begin(
                &fixture.job,
                Mutation {
                    name: "native",
                    root: &root,
                    order: &point.order,
                    point: Some(&point),
                    version: "native-facts-merge-v1",
                    identity: &fixture.identity,
                },
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Written);
        fixture.reload();
        let before_cursor: String = fixture
            .store
            .connection
            .query_row(
                "SELECT metadata FROM native_orthogonal_work WHERE job=?1 AND name='native'",
                [fixture.job.id],
                |row| row.get(0),
            )
            .unwrap();
        calls.set(0);
        let refused = fixture
            .store
            .orthogonal_advance(
                &fixture.job,
                "native",
                &combine,
                &|_, _| Ok(true),
                &cancelled,
            )
            .unwrap();
        assert_eq!(refused.outcome, Outcome::Deferred);
        assert!(refused.work.geometry.rows() > 0);
        let after_cursor: String = fixture
            .store
            .connection
            .query_row(
                "SELECT metadata FROM native_orthogonal_work WHERE job=?1 AND name='native'",
                [fixture.job.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            before_cursor, after_cursor,
            "cancelled advance committed its cursor"
        );
        let options = fixture.store.options.clone();
        let reopened = NativeStore::open(&options).unwrap();
        fixture.store = reopened;
        fixture.reload();
        let mut completed = None;
        for _ in 0..10000 {
            let result = fixture
                .store
                .orthogonal_advance(&fixture.job, "native", &combine, &|_, _| Ok(true), &|| {
                    false
                })
                .unwrap();
            assert_eq!(result.outcome, Outcome::Written);
            fixture.reload();
            if result.root.is_some() {
                completed = result.root;
                break;
            }
        }
        let root = completed.unwrap();
        let old_job = fixture.job.clone();
        let spec = native_query::JobSpec {
            view_key: "geometry-two".into(),
            ..fixture.job.spec.clone()
        };
        let new_job = fixture.store.begin(&spec).unwrap();
        let source = fixture
            .store
            .source_witness(&fixture.row.sources[0])
            .unwrap();
        let owner = fixture.store.owner_witness(&fixture.row).unwrap();
        assert_eq!(
            fixture
                .store
                .admit(&new_job, &[source], &[owner], "complete")
                .unwrap(),
            Outcome::Written
        );
        let new_job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        let nodes: i64 = fixture
            .store
            .connection
            .query_row("SELECT count(*) FROM native_orthogonal_nodes", [], |row| {
                row.get(0)
            })
            .unwrap();
        let imported = fixture
            .store
            .orthogonal_import(&new_job, "native", &root, &|_, _| Ok(true), &|| false)
            .unwrap();
        assert_eq!(imported.outcome, Outcome::Written);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_orthogonal_nodes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            nodes
        );
        assert!(imported.work.geometry.rows() <= 8);
        assert_eq!(fixture.store.restart(&old_job).unwrap(), Outcome::Written);
        for _ in 0..100000 {
            let Some(old) = fixture.store.reload(&old_job.spec.view_key).unwrap() else {
                break;
            };
            fixture.store.cleanup(&old).unwrap();
        }
        assert!(
            fixture
                .store
                .reload(&old_job.spec.view_key)
                .unwrap()
                .is_none()
        );
        fixture.job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        fixture.qualify();
        let empty = serde_json::to_string(&Facts::default()).unwrap();
        let rectangle = Rectangle::default();
        let input = ReadInput {
            name: "native",
            root: &root,
            rectangle: &rectangle,
            included: u64::MAX,
            empty: &empty,
        };
        assert!(
            fixture
                .store
                .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
                .unwrap()
                .value
                .is_some()
        );
        let ledger_before: Vec<u8> = fixture
            .store
            .connection
            .query_row(
                "SELECT head_sha256 FROM files WHERE path=?1",
                [fixture.row.sources[0].path.to_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE session_catalog SET metadata=metadata WHERE session_key='chosen'",
                [],
            )
            .unwrap();
        assert!(
            fixture
                .store
                .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
                .unwrap()
                .value
                .is_none()
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT head_sha256 FROM files WHERE path=?1",
                    [fixture.row.sources[0].path.to_str().unwrap()],
                    |row| row.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
            ledger_before
        );
    }

    #[test]
    fn orthogonal_annotation_bounds_and_corrupt_branches_are_refused_without_a_prefix_scan() {
        let mut fixture = Fixture::new();
        let mut root = Root::empty(12).unwrap();
        for seq in 0..8 {
            let mut point = fixture.point(seq);
            point.scalar = (if seq % 2 == 0 { u64::MAX } else { 3 })
                .to_be_bytes()
                .to_vec();
            point
                .scalar
                .extend_from_slice(&(seq as u32 % 2).to_be_bytes());
            root = fixture.mutate(&root, &point.order, Some(&point)).0;
        }
        fixture.qualify();
        let empty = serde_json::to_string(&Facts::default()).unwrap();
        let mut reached = u64::MAX.to_be_bytes().to_vec();
        reached.extend_from_slice(&0u32.to_be_bytes());
        let rectangle = Rectangle {
            order: Bounds::default(),
            scalar: Bounds {
                lower: Some(Edge {
                    key: reached,
                    inclusive: true,
                }),
                upper: None,
            },
        };
        let input = ReadInput {
            name: "native",
            root: &root,
            rectangle: &rectangle,
            included: u64::MAX ^ 2,
            empty: &empty,
        };
        let (facts, _) = fixture
            .store
            .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
            .unwrap()
            .qualified()
            .unwrap();
        assert_eq!(count(&facts).unwrap(), 4);
        let id = root.node.as_ref().unwrap().identity();
        let original: String = fixture
            .store
            .connection
            .query_row(
                "SELECT metadata FROM native_orthogonal_nodes WHERE identity=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        for corruption in ["swapped", "cycle", "missing-child"] {
            let mut node: TreeNode = serde_json::from_str(&original).unwrap();
            if let TreeNode::Branch { zero, one, .. } = &mut node {
                match corruption {
                    "swapped" => std::mem::swap(zero, one),
                    "cycle" => *zero = root.node.as_ref().unwrap().clone(),
                    _ => zero.digest = "f".repeat(64),
                }
            } else {
                panic!("fixture root is not a branch");
            }
            fixture
                .store
                .connection
                .execute(
                    "UPDATE native_orthogonal_nodes SET metadata=?2 WHERE identity=?1",
                    params![id, serde_json::to_string(&node).unwrap()],
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
                    .is_err(),
                "accepted {corruption}"
            );
        }
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_orthogonal_nodes SET metadata=?2 WHERE identity=?1",
                params![id, original],
            )
            .unwrap();
        fixture.store.connection.execute("UPDATE native_orthogonal_nodes SET identity=identity||'/missing' WHERE identity=?1",[&id]).unwrap();
        assert!(
            fixture
                .store
                .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
                .is_err()
        );
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_orthogonal_nodes SET identity=?1 WHERE identity=?1||'/missing'",
                [&id],
            )
            .unwrap();
        assert!(
            fixture
                .store
                .orthogonal_rectangle(&fixture.job, input, &combine, &|_, _| Ok(true))
                .unwrap()
                .value
                .is_some()
        );
    }
    #[test]
    fn orthogonal_full_source_depth_uses_individual_frames_and_reopens_with_maximum_version() {
        let mut fixture = Fixture::new();
        let mut points = Vec::new();
        for seq in 0..2 {
            let mut point = fixture.point(seq);
            let mut graph = Graph {
                db: &fixture.store.connection,
                job: fixture.job.id,
                work: Work::default(),
            };
            let mut facts = graph
                .intern(Node::Terminal {
                    facts: serde_json::to_string(&native(seq)).unwrap(),
                })
                .unwrap();
            for source in (0..VARIABLES).rev() {
                facts = graph
                    .intern(Node::Decision {
                        source,
                        absent: fixture.identity.clone(),
                        present: facts,
                    })
                    .unwrap();
            }
            point.facts = facts;
            point.scalar = expiry(0);
            points.push(point);
        }
        let (root, _) =
            fixture.mutate(&Root::empty(9).unwrap(), &points[0].order, Some(&points[0]));
        let version = "v".repeat(256);
        let begin = fixture
            .store
            .orthogonal_begin(
                &fixture.job,
                Mutation {
                    name: "native",
                    root: &root,
                    order: &points[1].order,
                    point: Some(&points[1]),
                    version: &version,
                    identity: &fixture.identity,
                },
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        assert_eq!(begin.outcome, Outcome::Written);
        fixture.reload();
        let mut deepest = 0;
        let mut reopened = false;
        let mut completed = None;
        for _ in 0..10000 {
            let result = fixture
                .store
                .orthogonal_advance(&fixture.job, "native", &combine, &|_, _| Ok(true), &|| {
                    false
                })
                .unwrap();
            assert_eq!(result.outcome, Outcome::Written);
            assert!(result.work.geometry.rows() <= STEP_ROWS);
            assert!(result.work.cursor_bytes <= NODE_BYTES);
            fixture.reload();
            let (depth, largest): (usize, usize) = fixture.store.connection.query_row(
                "SELECT count(*),COALESCE(max(octet_length(metadata)),0) FROM native_orthogonal_frames WHERE job=?1 AND name='native' AND family=2",
                [fixture.job.id], |row| Ok((usize::try_from(row.get::<_,i64>(0)?).map_err(|_|invalid("negative frame count"))?,usize::try_from(row.get::<_,i64>(1)?).map_err(|_|invalid("negative frame bytes"))?))).unwrap();
            deepest = deepest.max(depth);
            assert!(largest <= NODE_BYTES);
            if depth == usize::from(VARIABLES) + 1 && !reopened {
                let options = fixture.store.options.clone();
                fixture.store = NativeStore::open(&options).unwrap();
                fixture.reload();
                reopened = true;
            }
            if let Some(root) = result.root {
                completed = Some(root);
                break;
            }
        }
        assert!(
            reopened,
            "full supported source depth never reached the restart oracle"
        );
        assert_eq!(deepest, usize::from(VARIABLES) + 1);
        let root = completed.expect("full-depth durable operation did not finish");
        fixture.qualify();
        let empty = serde_json::to_string(&Facts::default()).unwrap();
        let rectangle = Rectangle {
            order: Bounds::default(),
            scalar: Bounds::default(),
        };
        for included in [u64::MAX, u64::MAX ^ 1, u64::MAX ^ (1 << 63)] {
            let (facts, _) = fixture
                .store
                .orthogonal_rectangle(
                    &fixture.job,
                    ReadInput {
                        name: "native",
                        root: &root,
                        rectangle: &rectangle,
                        included,
                        empty: &empty,
                    },
                    &combine,
                    &|_, _| Ok(true),
                )
                .unwrap()
                .qualified()
                .unwrap();
            assert_eq!(
                count(&facts).unwrap(),
                if included == u64::MAX { 2 } else { 0 }
            );
        }
    }

    #[test]
    fn orthogonal_geometry_uses_every_native_order_and_scalar_bit_without_predecessors() {
        for scalar in [9, 12] {
            for axis in [0, 1] {
                let bytes = width(axis, scalar);
                let zero = vec![0; bytes];
                for bit in 0..bytes * 8 {
                    let mut key = zero.clone();
                    key[bit / 8] = 1 << (7 - bit % 8);
                    assert_eq!(difference(&zero, &key), Some(bit));
                    assert!(!super::bit(&zero, bit));
                    assert!(super::bit(&key, bit));
                    let exact = Bounds {
                        lower: Some(Edge {
                            key: key.clone(),
                            inclusive: true,
                        }),
                        upper: Some(Edge {
                            key: key.clone(),
                            inclusive: true,
                        }),
                    };
                    assert!(matches!(exact.covers(&key, &key), Cover::Inside));
                    let open = Bounds {
                        lower: Some(Edge {
                            key: key.clone(),
                            inclusive: false,
                        }),
                        ..exact
                    };
                    assert!(matches!(open.covers(&key, &key), Cover::Outside));
                }
            }
        }
    }
    #[test]
    fn orthogonal_peer_generation_change_after_metadata_io_refuses_and_reports_attempted_work() {
        for scope in ["owner", "source", "publication", "relationship"] {
            let mut fixture = Fixture::new();
            let root = fixture.populate(4);
            fixture.qualify();
            let empty = serde_json::to_string(&Facts::default()).unwrap();
            let rectangle = Rectangle::default();
            let calls = Cell::new(0);
            let relationship = Cell::new(true);
            let path = EventCache::path(&fixture.store.options.cache);
            let fence = |_: &Connection, _: i64| {
                let n = calls.get() + 1;
                calls.set(n);
                if n == 2 {
                    let peer = Connection::open(&path)?;
                    match scope {
                        "owner" => {
                            peer.execute("UPDATE session_catalog SET metadata=metadata WHERE session_key='chosen'",[])?;
                        }
                        "source" => {
                            peer.execute(
                                "UPDATE files SET resume_at=resume_at+1 WHERE path=?1",
                                [fixture.row.sources[0].path.to_str().unwrap()],
                            )?;
                        }
                        "publication" => {
                            peer.execute(
                                "UPDATE native_query_jobs SET revision=revision+1 WHERE job=?1",
                                [fixture.job.id],
                            )?;
                        }
                        _ => relationship.set(false),
                    }
                }
                // Model relationship closure is rechecked after the native
                // source/publication closure, including after the read snapshot.
                Ok(relationship.get())
            };
            let attempt = fixture
                .store
                .orthogonal_rectangle(
                    &fixture.job,
                    ReadInput {
                        name: "native",
                        root: &root,
                        rectangle: &rectangle,
                        included: u64::MAX,
                        empty: &empty,
                    },
                    &combine,
                    &fence,
                )
                .unwrap();
            assert!(
                attempt.value.is_none(),
                "mixed-generation output in {scope}"
            );
            assert!(
                attempt.work.geometry.sql_reads > 0,
                "refused read lost attempted work"
            );
            assert!(attempt.work.connection_vm_steps >= attempt.work.geometry.vm_steps as u64);
        }
    }
    #[test]
    fn orthogonal_deep_outer_and_inner_spines_checkpoint_separately_and_resume_bounded_units() {
        let mut fixture = Fixture::new();
        let (root, base) = fixture.deep_geometry();
        let mut changed = base.clone();
        changed.facts = Graph {
            db: &fixture.store.connection,
            job: fixture.job.id,
            work: Work::default(),
        }
        .intern(Node::Terminal {
            facts: serde_json::to_string(&native(1000)).unwrap(),
        })
        .unwrap();
        let begin = fixture
            .store
            .orthogonal_begin(
                &fixture.job,
                Mutation {
                    name: "native",
                    root: &root,
                    order: &changed.order,
                    point: Some(&changed),
                    version: "native-facts-merge-v1",
                    identity: &fixture.identity,
                },
                &|_, _| Ok(true),
                &|| false,
            )
            .unwrap();
        assert_eq!(begin.outcome, Outcome::Written);
        fixture.reload();
        let mut reached_outer = false;
        for _ in 0..2000 {
            let result = fixture
                .store
                .orthogonal_advance(&fixture.job, "native", &combine, &|_, _| Ok(true), &|| {
                    false
                })
                .unwrap();
            assert_eq!(result.outcome, Outcome::Written);
            assert!(result.work.geometry.rows() <= STEP_ROWS);
            fixture.reload();
            let geometry = Geometry {
                db: &fixture.store.connection,
                job: fixture.job.id,
                scalar: 12,
                work: Work::default(),
            };
            let mut geometry = geometry;
            if geometry.state("native").unwrap().unwrap().outer.depth == ORDER_BYTES * 8 {
                reached_outer = true;
                break;
            }
        }
        assert!(reached_outer);
        let secondary = {
            let mut geometry = Geometry {
                db: &fixture.store.connection,
                job: fixture.job.id,
                scalar: 12,
                work: Work::default(),
            };
            geometry
                .read(root.node.as_ref().unwrap())
                .unwrap()
                .secondary()
                .unwrap()
                .clone()
        };
        // Exercise the actual secondary kernel as a separate unqualified
        // producer component. The complete outer mutation remains Pending;
        // this never promotes a partial geometry into a qualified Query.
        let mut kernel = Update {
            root: root.clone(),
            target: changed.order.clone(),
            point: Some(changed.clone()),
            version: "native-facts-merge-v1".into(),
            identity: fixture.identity.clone(),
            outer: TreeWalk::new(
                0,
                root.node.clone(),
                changed.order.clone(),
                Some(changed.clone()),
            ),
            inner: Some(TreeWalk::new(
                1,
                Some(secondary),
                changed.key(1),
                Some(changed),
            )),
            graph: None,
            complete: false,
        };
        {
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            let mut geometry = Geometry {
                db: &tx,
                job: fixture.job.id,
                scalar: 12,
                work: Work::default(),
            };
            geometry.save("inner-kernel", None, &kernel).unwrap();
            tx.commit().unwrap();
        }
        let mut reopened = false;
        let mut finished = false;
        for _ in 0..20000 {
            let tx = fixture.store.connection.unchecked_transaction().unwrap();
            let mut geometry = Geometry {
                db: &tx,
                job: fixture.job.id,
                scalar: 12,
                work: Work::default(),
            };
            let previous = geometry.state("inner-kernel").unwrap().unwrap();
            kernel = previous.clone();
            geometry
                .update_step("inner-kernel", &mut kernel, &combine)
                .unwrap();
            let cursor = geometry
                .save("inner-kernel", Some(&previous), &kernel)
                .unwrap();
            assert!(geometry.work.rows() <= STEP_ROWS);
            assert!(geometry.work.metadata_bytes <= STEP_BYTES);
            assert!(cursor <= NODE_BYTES);
            let inner = kernel.inner.as_ref().unwrap();
            let deep = inner.depth == width(1, 12) * 8;
            let done = matches!(inner.phase, Walk::Done);
            tx.commit().unwrap();
            if deep && !reopened {
                let (outer,inner,largest):(usize,usize,usize)=fixture.store.connection.query_row(
                    "SELECT sum(CASE WHEN name='native' AND family=0 THEN 1 ELSE 0 END),sum(CASE WHEN name='inner-kernel' AND family=1 THEN 1 ELSE 0 END),max(octet_length(metadata)) FROM native_orthogonal_frames WHERE job=?1",[fixture.job.id],|row|Ok((usize::try_from(row.get::<_,i64>(0)?).map_err(|_|invalid("negative outer depth"))?,usize::try_from(row.get::<_,i64>(1)?).map_err(|_|invalid("negative inner depth"))?,usize::try_from(row.get::<_,i64>(2)?).map_err(|_|invalid("negative frame bytes"))?))).unwrap();
                assert_eq!((outer, inner), (552, 648));
                assert!(largest <= NODE_BYTES);
                let options = fixture.store.options.clone();
                fixture.store = NativeStore::open(&options).unwrap();
                fixture.reload();
                reopened = true;
            }
            if done {
                finished = true;
                break;
            }
        }
        assert!(
            reopened && finished,
            "deep secondary kernel did not resume to completion"
        );
        let result = kernel.inner.as_ref().unwrap().result.as_ref().unwrap();
        let mut geometry = Geometry {
            db: &fixture.store.connection,
            job: fixture.job.id,
            scalar: 12,
            work: Work::default(),
        };
        let aggregate = geometry.read(result).unwrap().aggregate().unwrap().clone();
        let actual = geometry.aggregate(&aggregate, u64::MAX).unwrap();
        let mut expected = Facts::default();
        for seq in 1..=648 {
            expected.merge(native(seq)).unwrap();
        }
        expected.merge(native(1000)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let before = geometry.state("native").unwrap().unwrap();
        assert_eq!(before.outer.depth, 552);
        assert!(!before.complete);
        // Restart/cancellation relinquishes the unfinished outer attempt; all
        // cursor rows and newly derived nodes must eventually clean up.
        assert_eq!(
            fixture.store.restart(&fixture.job).unwrap(),
            Outcome::Written
        );
        for _ in 0..100000 {
            let Some(job) = fixture.store.reload(&fixture.job.spec.view_key).unwrap() else {
                break;
            };
            fixture.store.cleanup(&job).unwrap();
        }
        assert!(
            fixture
                .store
                .reload(&fixture.job.spec.view_key)
                .unwrap()
                .is_none()
        );
    }
}
