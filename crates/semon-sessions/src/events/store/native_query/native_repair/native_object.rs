//! Qualified object metadata: lexical lookup plus native insertion-order iteration.
//! Native serde_json preserves first insertion position and last duplicate value.
//! The tree retains descriptors, never decoded keys or native values. Its only
//! comparator is the existing qualified native text ordering owner.
use super::*;

pub(in super::super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_original_object_epochs (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, revision INTEGER NOT NULL,
 PRIMARY KEY(original,record_offset)
) WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_original_object_field_update AFTER UPDATE ON native_original_fields
 WHEN (EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=OLD.original AND record_offset=OLD.record_offset) OR EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=NEW.original AND record_offset=NEW.record_offset))
 BEGIN INSERT INTO native_original_object_epochs VALUES(OLD.original,OLD.record_offset,1)
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 INSERT INTO native_original_object_epochs SELECT NEW.original,NEW.record_offset,1
 WHERE NEW.original IS NOT OLD.original OR NEW.record_offset IS NOT OLD.record_offset
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 END;
CREATE TRIGGER IF NOT EXISTS native_original_object_field_delete AFTER DELETE ON native_original_fields
 WHEN EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=OLD.original AND record_offset=OLD.record_offset)
 BEGIN INSERT INTO native_original_object_epochs VALUES(OLD.original,OLD.record_offset,1)
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 END;
CREATE TRIGGER IF NOT EXISTS native_original_object_field_insert AFTER INSERT ON native_original_fields
 WHEN EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=NEW.original AND record_offset=NEW.record_offset)
 BEGIN INSERT INTO native_original_object_epochs VALUES(NEW.original,NEW.record_offset,1)
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 END;
CREATE TRIGGER IF NOT EXISTS native_original_object_node_update AFTER UPDATE ON native_original_nodes
 WHEN (EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=OLD.original AND record_offset=OLD.record_offset) OR EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=NEW.original AND record_offset=NEW.record_offset))
 BEGIN INSERT INTO native_original_object_epochs VALUES(OLD.original,OLD.record_offset,1)
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 INSERT INTO native_original_object_epochs SELECT NEW.original,NEW.record_offset,1
 WHERE NEW.original IS NOT OLD.original OR NEW.record_offset IS NOT OLD.record_offset
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 END;
CREATE TRIGGER IF NOT EXISTS native_original_object_node_delete AFTER DELETE ON native_original_nodes
 WHEN EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=OLD.original AND record_offset=OLD.record_offset)
 BEGIN INSERT INTO native_original_object_epochs VALUES(OLD.original,OLD.record_offset,1)
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 END;
CREATE TRIGGER IF NOT EXISTS native_original_object_node_insert AFTER INSERT ON native_original_nodes
 WHEN EXISTS(SELECT 1 FROM native_original_record_key_maps WHERE original=NEW.original AND record_offset=NEW.record_offset)
 BEGIN INSERT INTO native_original_object_epochs VALUES(NEW.original,NEW.record_offset,1)
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1;
 END;
CREATE TABLE IF NOT EXISTS native_original_record_key_maps (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, revision INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(original,record_offset)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_record_keys (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, parent_offset INTEGER NOT NULL,
 ordinal INTEGER NOT NULL, key_start INTEGER NOT NULL, value_address INTEGER NOT NULL,
 PRIMARY KEY(original,record_offset,parent_offset,ordinal),
 UNIQUE(original,record_offset,key_start)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_objects (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, object_address INTEGER NOT NULL,
 revision INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,object_address)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_object_nodes (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, object_address INTEGER NOT NULL,
 class INTEGER NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(original,record_offset,object_address,class)
) WITHOUT ROWID;
";
// Minimum AVL nodes satisfy N(h)=1+N(h-1)+N(h-2). N(96) >
// 2^64-1 (tested below), hence no supported u64 tree can have height 96.
const HEIGHT: usize = 96;
const MAP_PAGE: usize = 8;
// Each tree step loads a bounded neighborhood; each page step visits one
// edge/node. This also supports theoretical heights above the SQL row budget.
const DESCRIPTOR: usize = 4096;

fn bad(message: &'static str) -> io::Error {
    io::Error::other(message)
}
fn same<T: Serialize>(left: &T, right: &T) -> io::Result<bool> {
    Ok(serde_json::to_vec(left).map_err(io::Error::other)?
        == serde_json::to_vec(right).map_err(io::Error::other)?)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordBinding {
    original: [u8; 32],
    record: u64,
    end: u64,
    epoch: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    record: RecordBinding,
    object: OriginalNode,
    end: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyMap {
    binding: RecordBinding,
    after: Option<u64>,
    complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    field: OriginalField,
    value: OriginalNode,
    text: Option<OriginalText>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Edge {
    class: u64,
    right: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Stage {
    Input,
    Identity,
    Search(Option<u64>),
    Balance(u64),
    Seal,
    Complete,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Build {
    binding: Binding,
    root: Option<u64>,
    count: u64,
    after: Option<u64>,
    pending: Option<Pending>,
    path: Vec<Edge>,
    stage: Stage,
    seal: Vec<Branch>,
    sealed: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    class: u64,
    first_ordinal: u64,
    lexical_start: Option<u64>,
    key: OriginalField,
    value: OriginalNode,
    left: Option<u64>,
    right: Option<u64>,
    height: u8,
    count: u64,
}
impl Node {
    fn basic(&self, b: &Binding) -> bool {
        self.class > 0
            && self.height > 0
            && usize::from(self.height) < HEIGHT
            && self.count > 0
            && self.first_ordinal <= self.key.ordinal
            && self.key.key
            && self.key.location.parent == Some(b.object.address)
            && self.value.location.parent == Some(b.object.address)
            && self.value.location.key == Some(self.key.span)
            && self.key.span.start <= self.key.span.end
            && self.key.span.end < b.end
            && self.value.address > self.key.span.end
            && self.value.address < b.end
            && self.left != Some(self.class)
            && self.right != Some(self.class)
            && (self.left.is_none() || self.left != self.right)
    }
}
/// Only a completed, epoch-bound tree can create this handle. It grants no
/// source/owner/custody authority; every read requalifies the caller's witnesses.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualifiedObject {
    binding: Binding,
    revision: i64,
    root: Option<u64>,
    count: u64,
}
impl QualifiedObject {
    pub(crate) fn len(&self) -> u64 {
        self.count
    }
}
#[derive(Debug)]
pub(crate) struct ObjectStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) object: Option<QualifiedObject>,
    pub(crate) sql_vm_steps: i32,
}
#[derive(Clone, Debug)]
pub(crate) struct ObjectEntry {
    pub(crate) key: OriginalText,
    pub(crate) physical_key: OriginalField,
    pub(crate) value: OriginalNode,
}
/// One subtree's immutable lexical interval. Frontier intervals let resumed
/// cursors reject omitted ancestors before cloning or performing any I/O.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Branch {
    class: u64,
    start: u64,
    count: u64,
}
impl Branch {
    fn end(&self) -> io::Result<u64> {
        if self.class == 0 || self.count == 0 {
            return Err(bad("native object empty/invalid page branch"));
        }
        self.start
            .checked_add(self.count)
            .ok_or_else(|| bad("native object page interval overflow"))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    branch: Branch,
    rank: u64,
    right: Option<Branch>,
}
fn page_path<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Frame>, D::Error> {
    struct PathVisitor;
    impl<'de> serde::de::Visitor<'de> for PathVisitor {
        type Value = Vec<Frame>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded native object ancestor path")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            // Never reserve the untrusted size_hint or allocate an oversized path.
            let mut path = Vec::new();
            while let Some(frame) = seq.next_element::<Frame>()? {
                if path.len() + 1 >= HEIGHT {
                    return Err(serde::de::Error::custom(
                        "native object ancestor path exceeds height",
                    ));
                }
                path.push(frame);
            }
            Ok(path)
        }
    }
    deserializer.deserialize_seq(PathVisitor)
}
/// Lexical metadata traversal, distinct from native serde_json insertion order.
/// Construct with seek(0) for first; retain returned continuation for next.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectPageCursor {
    object: QualifiedObject,
    next_rank: u64,
    at: Option<Branch>,
    #[serde(deserialize_with = "page_path")]
    path: Vec<Frame>,
    seeking: bool,
    complete: bool,
}
impl ObjectPageCursor {
    pub(crate) fn seek(object: &QualifiedObject, rank: u64) -> Self {
        Self {
            object: object.clone(),
            next_rank: rank,
            at: object.root.map(|class| Branch {
                class,
                start: 0,
                count: object.count,
            }),
            path: Vec::new(),
            seeking: true,
            complete: rank >= object.count,
        }
    }
    fn valid(&self) -> io::Result<()> {
        if self.path.len() >= HEIGHT || (self.object.count == 0) != self.object.root.is_none() {
            return Err(bad("native object page root/path disagrees"));
        }
        if self.complete && self.seeking {
            let root = self.object.root.map(|class| Branch {
                class,
                start: 0,
                count: self.object.count,
            });
            if self.next_rank < self.object.count || !self.path.is_empty() || self.at != root {
                return Err(bad("native object initial completed seek is not canonical"));
            }
            return Ok(());
        }
        if self.next_rank > self.object.count
            || self.complete != (self.next_rank == self.object.count)
        {
            return Err(bad("native object page completion rank disagrees"));
        }
        if self.complete && (self.at.is_some() || !self.path.is_empty()) {
            return Err(bad("native object complete page retains branches"));
        }
        let mut end = if let Some(at) = &self.at {
            let end = at.end()?;
            if at.start > self.next_rank
                || self.next_rank >= end
                || !self.seeking && at.start != self.next_rank
            {
                return Err(bad("native object page active interval disagrees"));
            }
            end
        } else {
            if self.seeking {
                return Err(bad("native object seek lost its active branch"));
            }
            self.next_rank
        };
        for (i, frame) in self.path.iter().enumerate().rev() {
            if frame.branch.start > frame.rank
                || frame.rank >= frame.branch.end()?
                || frame.rank != end
                || self
                    .at
                    .as_ref()
                    .is_some_and(|at| at.class == frame.branch.class)
                || self.path[..i]
                    .iter()
                    .any(|other| other.branch.class == frame.branch.class)
            {
                return Err(bad("native object page ancestor/interval disagrees"));
            }
            let after = frame
                .rank
                .checked_add(1)
                .ok_or_else(|| bad("native object page rank overflow"))?;
            end = if let Some(right) = &frame.right {
                if right.start != after {
                    return Err(bad("native object page right interval disagrees"));
                }
                right.end()?
            } else {
                after
            };
            if end != frame.branch.end()? {
                return Err(bad("native object ancestor does not cover its subtree"));
            }
        }
        if end != self.object.count {
            return Err(bad("native object page frontier omits native keys"));
        }
        Ok(())
    }
}
#[derive(Debug)]
pub(crate) struct ObjectPageStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) cursor: ObjectPageCursor,
    pub(crate) entry: Option<ObjectEntry>,
    pub(crate) complete: bool,
    pub(crate) sql_vm_steps: i32,
}
#[derive(Default)]
struct Meter {
    work: RepairWork,
    vm: i32,
}
impl Meter {
    fn row(&mut self, bytes: usize) -> io::Result<()> {
        self.work.metadata_rows += 1;
        self.work.metadata_bytes += bytes;
        if self.work.metadata_rows > ROWS || self.work.metadata_bytes > BYTES {
            return Err(bad("native object exceeds metadata allowance"));
        }
        Ok(())
    }
    fn statement(&mut self, s: &rusqlite::Statement<'_>) {
        self.vm += s.get_status(rusqlite::StatementStatus::VmStep);
    }
    fn merge(&mut self, w: RepairWork, vm: i32) -> io::Result<()> {
        self.work.metadata_rows += w.metadata_rows;
        self.work.metadata_bytes += w.metadata_bytes;
        self.work.range_requests += w.range_requests;
        self.work.native_bytes += w.native_bytes;
        self.vm += vm;
        if self.work.metadata_rows > ROWS
            || self.work.metadata_bytes > BYTES
            || self.work.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX
        {
            return Err(bad("native object nested allowance exceeded"));
        }
        Ok(())
    }
    fn result(self, outcome: Outcome, object: Option<QualifiedObject>) -> ObjectStep {
        ObjectStep {
            outcome,
            work: self.work,
            object,
            sql_vm_steps: self.vm,
        }
    }
}
fn load_json<T: serde::de::DeserializeOwned, P: rusqlite::Params>(
    db: &Connection,
    sql: &str,
    params: P,
    m: &mut Meter,
) -> io::Result<Option<(i64, T)>> {
    let mut q = db.prepare(sql).map_err(io::Error::other)?;
    let row: Option<(i64, Option<String>, i64)> = q
        .query_row(params, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(io::Error::other)?;
    m.statement(&q);
    m.row(16)?;
    row.map(|(revision, text, size)| {
        let size = usize::try_from(size).map_err(io::Error::other)?;
        if revision < 0 || size > STATE_BYTES {
            return Err(bad("native object stored state exceeds residency"));
        }
        m.work.metadata_bytes += size;
        Ok((
            revision,
            serde_json::from_str(
                text.as_deref()
                    .ok_or_else(|| bad("native object oversized state"))?,
            )
            .map_err(io::Error::other)?,
        ))
    })
    .transpose()
}
fn record_binding(db: &Connection, repair: &Repair, m: &mut Meter) -> io::Result<RecordBinding> {
    let mut q=db.prepare("SELECT end_offset,coalesce((SELECT revision FROM native_original_object_epochs WHERE original=?1 AND record_offset=?2),0) FROM native_original_records WHERE original=?1 AND record_offset=?2 AND status='lexed'").map_err(io::Error::other)?;
    let (end, epoch): (i64, i64) = q
        .query_row(
            params![
                repair.original.as_slice(),
                seq_u64(repair.cursor.record_at)?
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(io::Error::other)?;
    m.statement(&q);
    m.row(16)?;
    let b = RecordBinding {
        original: repair.original,
        record: repair.cursor.record_at,
        end: u64::try_from(end).map_err(io::Error::other)?,
        epoch: u64::try_from(epoch).map_err(io::Error::other)?,
    };
    if b.end != repair.cursor.source_at || b.end > repair.source.offset || b.record >= b.end {
        return Err(bad("native object record fence disagrees"));
    }
    Ok(b)
}
fn native_node(
    db: &Connection,
    r: &RecordBinding,
    address: u64,
    m: &mut Meter,
) -> io::Result<(OriginalNode, u64)> {
    let mut q=db.prepare("SELECT CASE WHEN octet_length(metadata)<=4096 THEN metadata END,octet_length(metadata),end_offset,parent_offset,key_start,element_index FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND address=?3").map_err(io::Error::other)?;
    let (text, size, end, parent, key, index): (
        Option<String>,
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = q
        .query_row(
            params![r.original.as_slice(), seq_u64(r.record)?, seq_u64(address)?],
            |v| {
                Ok((
                    v.get(0)?,
                    v.get(1)?,
                    v.get(2)?,
                    v.get(3)?,
                    v.get(4)?,
                    v.get(5)?,
                ))
            },
        )
        .map_err(io::Error::other)?;
    m.statement(&q);
    m.row(usize::try_from(size).map_err(io::Error::other)? + 40)?;
    let n: OriginalNode = serde_json::from_str(
        text.as_deref()
            .ok_or_else(|| bad("native object node descriptor oversized"))?,
    )
    .map_err(io::Error::other)?;
    let end = u64::try_from(end.ok_or_else(|| bad("native object node incomplete"))?)
        .map_err(io::Error::other)?;
    if n.address != address
        || end < address
        || end > r.end - r.record
        || n.location.parent.map(seq_u64).transpose()? != parent
        || n.location.key.map(|s| seq_u64(s.start)).transpose()? != key
        || n.location.index.map(seq_u64).transpose()? != index
    {
        return Err(bad("native object node descriptor disagrees"));
    }
    Ok((n, end))
}
fn binding(
    db: &Connection,
    repair: &Repair,
    object: &OriginalNode,
    m: &mut Meter,
) -> io::Result<Binding> {
    let r = record_binding(db, repair, m)?;
    let (node, end) = native_node(db, &r, object.address, m)?;
    if !matches!(node.value, OriginalValue::Object) || !same(&node, object)? {
        return Err(bad("native object requires exact qualified object node"));
    }
    Ok(Binding {
        record: r,
        object: node,
        end,
    })
}
fn current(
    db: &Connection,
    job: &Job,
    witness: &SourceWitness,
    repair: &Repair,
    b: &Binding,
    closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    m: &mut Meter,
) -> io::Result<bool> {
    check_record_view(db, job, witness, repair).map_err(io::Error::other)?;
    if !closure(db, job.id).map_err(io::Error::other)? {
        return Ok(false);
    }
    Ok(same(&binding(db, repair, &b.object, m)?, b)?)
}
fn load_build(db: &Connection, b: &Binding, m: &mut Meter) -> io::Result<Option<(i64, Build)>> {
    let v: Option<(i64, Build)> = load_json(
        db,
        "SELECT revision,CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_objects WHERE original=?1 AND record_offset=?2 AND object_address=?3",
        params![
            b.record.original.as_slice(),
            seq_u64(b.record.record)?,
            seq_u64(b.object.address)?
        ],
        m,
    )?;
    if let Some((_, v)) = &v {
        if !same(&v.binding, b)?
            || v.path.len() >= HEIGHT
            || v.seal.len() >= HEIGHT
            || v.sealed > v.count
            || matches!(v.stage, Stage::Complete) && (v.sealed != v.count || !v.seal.is_empty())
            || !matches!(v.stage, Stage::Seal | Stage::Complete)
                && (v.sealed != 0 || !v.seal.is_empty())
            || v.path
                .iter()
                .enumerate()
                .any(|(i, e)| v.path[..i].iter().any(|p| p.class == e.class))
            || (matches!(v.stage, Stage::Input | Stage::Complete)
                && (v.pending.is_some() || !v.path.is_empty()))
        {
            return Err(bad("native object build binding/path changed"));
        }
    }
    Ok(v)
}
fn node(db: &Connection, b: &Binding, id: u64, m: &mut Meter) -> io::Result<Node> {
    let v: Option<(i64, Node)> = load_json(
        db,
        "SELECT 0,CASE WHEN octet_length(metadata)<=4096 THEN metadata END,octet_length(metadata) FROM native_original_object_nodes WHERE original=?1 AND record_offset=?2 AND object_address=?3 AND class=?4",
        params![
            b.record.original.as_slice(),
            seq_u64(b.record.record)?,
            seq_u64(b.object.address)?,
            seq_u64(id)?
        ],
        m,
    )?;
    let n = v.ok_or_else(|| bad("native object tree edge missing"))?.1;
    if n.class != id || !n.basic(b) {
        return Err(bad("native object invalid tree node"));
    }
    Ok(n)
}
fn children(
    db: &Connection,
    b: &Binding,
    n: &Node,
    m: &mut Meter,
) -> io::Result<(Option<Node>, Option<Node>)> {
    Ok((
        n.left.map(|id| node(db, b, id, m)).transpose()?,
        n.right.map(|id| node(db, b, id, m)).transpose()?,
    ))
}
fn height(n: &Option<Node>) -> u8 {
    n.as_ref().map_or(0, |v| v.height)
}
fn count(n: &Option<Node>) -> u64 {
    n.as_ref().map_or(0, |v| v.count)
}
fn refresh(n: &mut Node, l: &Option<Node>, r: &Option<Node>) -> io::Result<()> {
    n.height = height(l)
        .max(height(r))
        .checked_add(1)
        .ok_or_else(|| bad("native object height overflow"))?;
    n.count = count(l)
        .checked_add(count(r))
        .and_then(|x| x.checked_add(1))
        .ok_or_else(|| bad("native object count overflow"))?;
    if usize::from(n.height) >= HEIGHT {
        return Err(bad("native object impossible AVL height"));
    }
    Ok(())
}
fn checked(
    db: &Connection,
    b: &Binding,
    n: &Node,
    m: &mut Meter,
) -> io::Result<(Option<Node>, Option<Node>)> {
    let (l, r) = children(db, b, n, m)?;
    let mut expected = n.clone();
    refresh(&mut expected, &l, &r)?;
    if expected.height != n.height
        || expected.count != n.count
        || (i16::from(height(&l)) - i16::from(height(&r))).abs() > 1
    {
        return Err(bad("native object structural metadata disagrees"));
    }
    Ok((l, r))
}
fn save_node(db: &Connection, b: &Binding, n: &Node, m: &mut Meter) -> io::Result<()> {
    if !n.basic(b) {
        return Err(bad("native object invalid node write"));
    }
    let text = bounded_json(n, DESCRIPTOR).map_err(io::Error::other)?;
    let mut q=db.prepare("INSERT INTO native_original_object_nodes VALUES(?1,?2,?3,?4,?5) ON CONFLICT(original,record_offset,object_address,class) DO UPDATE SET metadata=excluded.metadata").map_err(io::Error::other)?;
    q.execute(params![
        b.record.original.as_slice(),
        seq_u64(b.record.record)?,
        seq_u64(b.object.address)?,
        seq_u64(n.class)?,
        text
    ])
    .map_err(io::Error::other)?;
    m.statement(&q);
    m.row(text.len() + 64)
}
// One AVL ancestor/rotation per durable transition. The child subtree is already
// balanced; the parent is deliberately unavailable until this repair completes.
fn balance(db: &Connection, b: &Binding, mut n: Node, m: &mut Meter) -> io::Result<Node> {
    let (l, r) = children(db, b, &n, m)?;
    let delta = i16::from(height(&l)) - i16::from(height(&r));
    if delta.abs() > 2 {
        return Err(bad("native object invalid AVL repair delta"));
    }
    if delta == 2 {
        let mut left = l.ok_or_else(|| bad("native object missing left rotation child"))?;
        let (ll, lr) = children(db, b, &left, m)?;
        if height(&ll) < height(&lr) {
            let mut mid = lr.ok_or_else(|| bad("native object missing double rotation child"))?;
            let (ml, mr) = children(db, b, &mid, m)?;
            left.right = mid.left;
            refresh(&mut left, &ll, &ml)?;
            save_node(db, b, &left, m)?;
            n.left = mid.right;
            refresh(&mut n, &mr, &r)?;
            save_node(db, b, &n, m)?;
            mid.left = Some(left.class);
            mid.right = Some(n.class);
            refresh(&mut mid, &Some(left), &Some(n))?;
            save_node(db, b, &mid, m)?;
            return Ok(mid);
        }
        n.left = left.right;
        refresh(&mut n, &lr, &r)?;
        save_node(db, b, &n, m)?;
        left.right = Some(n.class);
        refresh(&mut left, &ll, &Some(n))?;
        save_node(db, b, &left, m)?;
        return Ok(left);
    }
    if delta == -2 {
        let mut right = r.ok_or_else(|| bad("native object missing right rotation child"))?;
        let (rl, rr) = children(db, b, &right, m)?;
        if height(&rr) < height(&rl) {
            let mut mid = rl.ok_or_else(|| bad("native object missing double rotation child"))?;
            let (ml, mr) = children(db, b, &mid, m)?;
            n.right = mid.left;
            refresh(&mut n, &l, &ml)?;
            save_node(db, b, &n, m)?;
            right.left = mid.right;
            refresh(&mut right, &mr, &rr)?;
            save_node(db, b, &right, m)?;
            mid.left = Some(n.class);
            mid.right = Some(right.class);
            refresh(&mut mid, &Some(n), &Some(right))?;
            save_node(db, b, &mid, m)?;
            return Ok(mid);
        }
        n.right = right.left;
        refresh(&mut n, &l, &rl)?;
        save_node(db, b, &n, m)?;
        right.left = Some(n.class);
        refresh(&mut right, &Some(n), &rr)?;
        save_node(db, b, &right, m)?;
        return Ok(right);
    }
    refresh(&mut n, &l, &r)?;
    save_node(db, b, &n, m)?;
    Ok(n)
}
fn map_state(
    db: &Connection,
    b: &RecordBinding,
    m: &mut Meter,
) -> io::Result<Option<(i64, KeyMap)>> {
    let value: Option<(i64, KeyMap)> = load_json(
        db,
        "SELECT revision,CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_record_key_maps WHERE original=?1 AND record_offset=?2",
        params![b.original.as_slice(), seq_u64(b.record)?],
        m,
    )?;
    if let Some((_, v)) = &value {
        if !same(&v.binding, b)? {
            return Err(bad("native object record key-map epoch changed"));
        }
    }
    Ok(value)
}
fn map_advance(
    db: &Connection,
    b: &RecordBinding,
    revision: Option<i64>,
    mut state: KeyMap,
    m: &mut Meter,
) -> io::Result<()> {
    // PK seek across all physical fields ONCE per record, including old retained
    // qualified records. No initializer indexes old history, and no object
    // request rescans fields after this record map is complete.
    let mut q=db.prepare("SELECT ordinal,CASE WHEN octet_length(metadata)<=4096 THEN metadata END,octet_length(metadata),is_key,parent_offset,start_offset,end_offset FROM native_original_fields WHERE original=?1 AND record_offset=?2 AND ordinal>?3 ORDER BY ordinal LIMIT 8").map_err(io::Error::other)?;
    let mut rows = q
        .query(params![
            b.original.as_slice(),
            seq_u64(b.record)?,
            state.after.map(seq_u64).transpose()?.unwrap_or(-1)
        ])
        .map_err(io::Error::other)?;
    let mut fields = Vec::new();
    while let Some(row) = rows.next().map_err(io::Error::other)? {
        let ordinal = u64::try_from(row.get::<_, i64>(0).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        let text: Option<String> = row.get(1).map_err(io::Error::other)?;
        let size: i64 = row.get(2).map_err(io::Error::other)?;
        m.row(usize::try_from(size).map_err(io::Error::other)? + 48)?;
        let f: OriginalField = serde_json::from_str(
            text.as_deref()
                .ok_or_else(|| bad("native object key-map descriptor oversized"))?,
        )
        .map_err(io::Error::other)?;
        let key: bool = row.get(3).map_err(io::Error::other)?;
        let parent: Option<i64> = row.get(4).map_err(io::Error::other)?;
        let start: i64 = row.get(5).map_err(io::Error::other)?;
        let end: i64 = row.get(6).map_err(io::Error::other)?;
        if f.ordinal != ordinal
            || f.key != key
            || f.location.parent.map(seq_u64).transpose()? != parent
            || seq_u64(f.span.start)? != start
            || seq_u64(f.span.end)? != end
            || f.span.start > f.span.end
            || f.span.end >= b.end - b.record
        {
            return Err(bad("native object key-map field disagrees"));
        }
        fields.push(f);
    }
    drop(rows);
    m.statement(&q);
    drop(q);
    for f in &fields {
        if f.key {
            let parent = f
                .location
                .parent
                .ok_or_else(|| bad("native object key lacks parent"))?;
            let mut q=db.prepare("SELECT address FROM native_original_nodes WHERE original=?1 AND record_offset=?2 AND parent_offset=?3 AND key_start=?4").map_err(io::Error::other)?;
            let address: i64 = q
                .query_row(
                    params![
                        b.original.as_slice(),
                        seq_u64(b.record)?,
                        seq_u64(parent)?,
                        seq_u64(f.span.start)?
                    ],
                    |r| r.get(0),
                )
                .map_err(io::Error::other)?;
            m.statement(&q);
            m.row(8)?;
            drop(q);
            let (n, _) = native_node(db, b, u64::try_from(address).map_err(io::Error::other)?, m)?;
            if n.location.parent != Some(parent) || n.location.key != Some(f.span) {
                return Err(bad("native object key/value linkage disagrees"));
            }
            let mut q = db
                .prepare("INSERT INTO native_original_record_keys VALUES(?1,?2,?3,?4,?5,?6)")
                .map_err(io::Error::other)?;
            q.execute(params![
                b.original.as_slice(),
                seq_u64(b.record)?,
                seq_u64(parent)?,
                seq_u64(f.ordinal)?,
                seq_u64(f.span.start)?,
                address
            ])
            .map_err(io::Error::other)?;
            m.statement(&q);
            m.row(64)?;
        }
        state.after = Some(f.ordinal);
    }
    state.complete = fields.len() < MAP_PAGE;
    let text = bounded_json(&state, STATE_BYTES).map_err(io::Error::other)?;
    let mut q=db.prepare("INSERT INTO native_original_record_key_maps VALUES(?1,?2,0,?3) ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1,metadata=excluded.metadata WHERE revision=?4").map_err(io::Error::other)?;
    let changed = q
        .execute(params![
            b.original.as_slice(),
            seq_u64(b.record)?,
            text,
            revision
        ])
        .map_err(io::Error::other)?;
    m.statement(&q);
    m.row(text.len() + 56)?;
    if changed != 1 {
        return Err(bad("native object key-map revision changed"));
    }
    Ok(())
}
fn next_key(
    db: &Connection,
    b: &Binding,
    after: Option<u64>,
    repair: &Repair,
    m: &mut Meter,
) -> io::Result<Option<Pending>> {
    let mut q=db.prepare("SELECT ordinal,key_start,value_address FROM native_original_record_keys WHERE original=?1 AND record_offset=?2 AND parent_offset=?3 AND ordinal>?4 ORDER BY ordinal LIMIT 1").map_err(io::Error::other)?;
    let row: Option<(i64, i64, i64)> = q
        .query_row(
            params![
                b.record.original.as_slice(),
                seq_u64(b.record.record)?,
                seq_u64(b.object.address)?,
                after.map(seq_u64).transpose()?.unwrap_or(-1)
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(io::Error::other)?;
    m.statement(&q);
    m.row(24)?;
    let Some((ordinal, start, address)) = row else {
        return Ok(None);
    };
    let (field, _, vm) = native_classes::qualified_field_at(
        db,
        repair,
        b.record.record,
        u64::try_from(ordinal).map_err(io::Error::other)?,
        &mut m.work,
    )?;
    m.vm += vm;
    let (value, end) = native_node(
        db,
        &b.record,
        u64::try_from(address).map_err(io::Error::other)?,
        m,
    )?;
    if !field.key
        || field.location.parent != Some(b.object.address)
        || seq_u64(field.span.start)? != start
        || value.location.parent != field.location.parent
        || value.location.key != Some(field.span)
        || end > b.end
    {
        return Err(bad("native object mapped key descriptor changed"));
    }
    Ok(Some(Pending {
        field,
        value,
        text: None,
    }))
}
fn validate_pending(
    db: &Connection,
    repair: &Repair,
    b: &Binding,
    p: &Pending,
    m: &mut Meter,
) -> io::Result<()> {
    let (field, _, vm) = native_classes::qualified_field_at(
        db,
        repair,
        b.record.record,
        p.field.ordinal,
        &mut m.work,
    )?;
    m.vm += vm;
    let (value, end) = native_node(db, &b.record, p.value.address, m)?;
    let (text, vm) = native_classes::qualified_identity_at(
        db,
        repair,
        b.record.record,
        p.field.ordinal,
        &mut m.work,
    )?;
    m.vm += vm;
    if !same(&field, &p.field)? || !same(&value, &p.value)? || !same(&text, &p.text)? || end > b.end
    {
        return Err(bad("native object pending key/value/class changed"));
    }
    Ok(())
}

fn qualified(build: &Build, revision: i64) -> Option<QualifiedObject> {
    matches!(build.stage, Stage::Complete).then(|| QualifiedObject {
        binding: build.binding.clone(),
        revision,
        root: build.root,
        count: build.count,
    })
}
impl NativeStore {
    /// One bounded map/input/identity/comparison/AVL transition. Repeated calls
    /// resume committed progress, including after process replacement. Only
    /// Some(object) is a completed lexical map; Deferred is explicit Pending.
    pub(crate) fn original_object_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        object: &OriginalNode,
        records: &crate::query_source::Records<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<ObjectStep> {
        let mut m = Meter::default();
        let latch = std::cell::Cell::new(false);
        let cancelled = || {
            if cancelled() {
                latch.set(true);
            }
            latch.get()
        };
        if cancelled() {
            return Ok(m.result(Outcome::Deferred, None));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(m.result(Outcome::Conflict, None));
        }
        let (b, map_revision, map, revision, mut build) = {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            check_record_view(&tx, job, witness, repair).map_err(io::Error::other)?;
            if !closure(&tx, job.id).map_err(io::Error::other)? {
                return Ok(m.result(Outcome::Conflict, None));
            }
            let b = binding(&tx, repair, object, &mut m)?;
            let map = map_state(&tx, &b.record, &mut m)?;
            let (map_revision, map) = match map {
                Some((r, v)) => (Some(r), v),
                None => (
                    None,
                    KeyMap {
                        binding: b.record.clone(),
                        after: None,
                        complete: false,
                    },
                ),
            };
            let old = load_build(&tx, &b, &mut m)?;
            let (revision, build) = match old {
                Some((r, v)) => (Some(r), v),
                None => (
                    None,
                    Build {
                        binding: b.clone(),
                        root: None,
                        count: 0,
                        after: None,
                        pending: None,
                        path: Vec::new(),
                        stage: Stage::Input,
                        seal: Vec::new(),
                        sealed: 0,
                    },
                ),
            };
            (b, map_revision, map, revision, build)
        };
        let map_complete = map.complete;
        let already_complete = matches!(build.stage, Stage::Complete);
        let mut order = None;
        if map_complete {
            match build.stage.clone() {
                Stage::Identity => {
                    let pending = build
                        .pending
                        .as_mut()
                        .ok_or_else(|| bad("native object identity lacks input"))?;
                    // Reserve outer cursor/fence reads and one anchor BEFORE I/O.
                    let step = self.original_identity_step_with_budget(
                        job,
                        witness,
                        repair,
                        pending.field.ordinal,
                        records,
                        &cancelled,
                        crate::SESSION_SOURCE_CHUNK_MAX - 1,
                        ROWS - m.work.metadata_rows - 17,
                    )?;
                    let outcome = step.outcome;
                    let text = step.text;
                    m.merge(step.work, step.class_sql_vm_steps)?;
                    if outcome == Outcome::Conflict {
                        return Ok(m.result(Outcome::Conflict, None));
                    }
                    if cancelled() {
                        return Ok(m.result(Outcome::Deferred, None));
                    }
                    if let Some(text) = text {
                        pending.text = Some(text);
                        build.stage = Stage::Search(build.root);
                    }
                }
                Stage::Search(Some(id)) => {
                    let n = node(&self.connection, &b, id, &mut m)?;
                    checked(&self.connection, &b, &n, &mut m)?;
                    let pending = build
                        .pending
                        .as_ref()
                        .ok_or_else(|| bad("native object search lacks input"))?;
                    let text = pending
                        .text
                        .as_ref()
                        .ok_or_else(|| bad("native object search lacks identity"))?;
                    let other = OriginalText::qualified(repair.original, n.class);
                    let left = ComparisonSource {
                        witness,
                        repair,
                        text,
                        records,
                    };
                    let right = ComparisonSource {
                        witness,
                        repair,
                        text: &other,
                        records,
                    };
                    let step = self.original_text_order_step_with_budget(
                        job,
                        left,
                        right,
                        closure,
                        &cancelled,
                        crate::SESSION_SOURCE_CHUNK_MAX - 1,
                        ROWS - m.work.metadata_rows - 17,
                    )?;
                    let outcome = step.outcome;
                    order = step.order;
                    m.merge(step.work, step.comparison_sql_vm_steps)?;
                    if outcome == Outcome::Conflict {
                        return Ok(m.result(Outcome::Conflict, None));
                    }
                    if cancelled() {
                        return Ok(m.result(Outcome::Deferred, None));
                    }
                }
                _ => {}
            }
        }
        verify_member_anchor(records, repair, &mut m.work)?;
        records.finish()?;
        if cancelled() {
            return Ok(m.result(Outcome::Deferred, None));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(m.result(Outcome::Conflict, None));
        }
        let options = self.options.clone();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !current(&tx, job, witness, repair, &b, closure, &mut m)? {
            return Ok(m.result(Outcome::Conflict, None));
        }
        let actual = load_build(&tx, &b, &mut m)?;
        if actual.as_ref().map(|(r, _)| *r) != revision {
            return Ok(m.result(Outcome::Conflict, None));
        }
        let actual_map = map_state(&tx, &b.record, &mut m)?;
        if actual_map.as_ref().map(|(r, _)| *r) != map_revision {
            return Ok(m.result(Outcome::Conflict, None));
        }
        if !map_complete {
            map_advance(&tx, &b.record, map_revision, map, &mut m)?;
        } else {
            match build.stage.clone() {
                Stage::Input => {
                    if let Some(input) = next_key(&tx, &b, build.after, repair, &mut m)? {
                        build.pending = Some(input);
                        build.stage = Stage::Identity;
                    } else {
                        if let Some(root) = build.root {
                            let n = node(&tx, &b, root, &mut m)?;
                            checked(&tx, &b, &n, &mut m)?;
                            if n.count != build.count {
                                return Err(bad("native object completion count changed"));
                            }
                        } else if build.count != 0 {
                            return Err(bad("native object missing completed root"));
                        }
                        build.seal = build
                            .root
                            .map(|class| Branch {
                                class,
                                start: 0,
                                count: build.count,
                            })
                            .into_iter()
                            .collect();
                        build.stage = Stage::Seal;
                    }
                }
                Stage::Search(Some(id)) => {
                    if let Some(order) = order {
                        let mut n = node(&tx, &b, id, &mut m)?;
                        let p = build
                            .pending
                            .as_ref()
                            .ok_or_else(|| bad("native object search lacks pending input"))?;
                        if order == NativeTextOrder::Equal {
                            if p.text.as_ref().is_none_or(|t| t.identity() != n.class) {
                                return Err(bad(
                                    "native object equal keys have different qualified classes",
                                ));
                            }
                            if p.field.ordinal <= n.key.ordinal {
                                return Err(bad(
                                    "native object duplicate key physical order changed",
                                ));
                            }
                            validate_pending(&tx, repair, &b, p, &mut m)?;
                            n.key = p.field.clone();
                            n.value = p.value.clone();
                            save_node(&tx, &b, &n, &mut m)?;
                            build.after = Some(p.field.ordinal);
                            build.pending = None;
                            build.path.clear();
                            build.stage = Stage::Input;
                        } else {
                            if build.path.len() + 1 >= HEIGHT
                                || build.path.iter().any(|e| e.class == id)
                            {
                                return Err(bad("native object path is cyclic or too high"));
                            }
                            let right = order == NativeTextOrder::Greater;
                            build.path.push(Edge { class: id, right });
                            build.stage = Stage::Search(if right { n.right } else { n.left });
                        }
                    }
                }
                Stage::Search(None) => {
                    let p = build
                        .pending
                        .as_ref()
                        .ok_or_else(|| bad("native object insertion lacks input"))?;
                    validate_pending(&tx, repair, &b, p, &mut m)?;
                    let id = p
                        .text
                        .as_ref()
                        .ok_or_else(|| bad("native object insertion lacks qualified text"))?
                        .identity();
                    let n = Node {
                        class: id,
                        first_ordinal: p.field.ordinal,
                        lexical_start: None,
                        key: p.field.clone(),
                        value: p.value.clone(),
                        left: None,
                        right: None,
                        height: 1,
                        count: 1,
                    };
                    // INSERT, rather than UPSERT: a repeated class on a different
                    // path signals a corrupted tree, never another logical key.
                    let text = bounded_json(&n, DESCRIPTOR).map_err(io::Error::other)?;
                    if !n.basic(&b) {
                        return Err(bad("native object insertion descriptor changed"));
                    }
                    let mut q = tx
                        .prepare("INSERT INTO native_original_object_nodes VALUES(?1,?2,?3,?4,?5)")
                        .map_err(io::Error::other)?;
                    q.execute(params![
                        b.record.original.as_slice(),
                        seq_u64(b.record.record)?,
                        seq_u64(b.object.address)?,
                        seq_u64(id)?,
                        text
                    ])
                    .map_err(io::Error::other)?;
                    m.statement(&q);
                    m.row(text.len() + 64)?;
                    build.count = build
                        .count
                        .checked_add(1)
                        .ok_or_else(|| bad("native object count overflow"))?;
                    build.stage = Stage::Balance(id);
                }
                Stage::Balance(child) => {
                    if let Some(edge) = build.path.pop() {
                        let mut n = node(&tx, &b, edge.class, &mut m)?;
                        if edge.right {
                            n.right = Some(child);
                        } else {
                            n.left = Some(child);
                        }
                        let n = balance(&tx, &b, n, &mut m)?;
                        build.stage = Stage::Balance(n.class);
                    } else {
                        let n = node(&tx, &b, child, &mut m)?;
                        checked(&tx, &b, &n, &mut m)?;
                        if n.count != build.count {
                            return Err(bad("native object balanced root count disagrees"));
                        }
                        build.root = Some(child);
                        build.after = Some(
                            build
                                .pending
                                .as_ref()
                                .ok_or_else(|| bad("native object finished insertion lacks input"))?
                                .field
                                .ordinal,
                        );
                        build.pending = None;
                        build.stage = Stage::Input;
                    }
                }
                Stage::Seal => {
                    if let Some(branch) = build.seal.pop() {
                        let mut n = node(&tx, &b, branch.class, &mut m)?;
                        let (left, right) = checked(&tx, &b, &n, &mut m)?;
                        if n.count != branch.count
                            || n.lexical_start.is_some()
                            || branch.end()? > build.count
                        {
                            return Err(bad("native object seal cursor/subtree changed"));
                        }
                        let rank = branch
                            .start
                            .checked_add(count(&left))
                            .ok_or_else(|| bad("native object seal rank overflow"))?;
                        build.sealed = build
                            .sealed
                            .checked_add(1)
                            .ok_or_else(|| bad("native object seal count overflow"))?;
                        if build.sealed > build.count {
                            return Err(bad("native object seal exceeds qualified node count"));
                        }
                        n.lexical_start = Some(branch.start);
                        save_node(&tx, &b, &n, &mut m)?;
                        if let Some(right) = right {
                            build.seal.push(Branch {
                                class: right.class,
                                start: rank + 1,
                                count: right.count,
                            });
                        }
                        if let Some(left) = left {
                            build.seal.push(Branch {
                                class: left.class,
                                start: branch.start,
                                count: left.count,
                            });
                        }
                        if build.seal.len() >= HEIGHT {
                            return Err(bad("native object seal exceeds AVL height"));
                        }
                    } else {
                        if build.sealed != build.count {
                            return Err(bad("native object seal ended before all nodes"));
                        }
                        build.stage = Stage::Complete;
                    }
                }
                Stage::Identity | Stage::Complete => {}
            }
            if !already_complete {
                let text = bounded_json(&build, STATE_BYTES).map_err(io::Error::other)?;
                let mut q=tx.prepare("INSERT INTO native_original_objects VALUES(?1,?2,?3,0,?4) ON CONFLICT(original,record_offset,object_address) DO UPDATE SET revision=revision+1,metadata=excluded.metadata WHERE revision=?5").map_err(io::Error::other)?;
                let changed = q
                    .execute(params![
                        b.record.original.as_slice(),
                        seq_u64(b.record.record)?,
                        seq_u64(b.object.address)?,
                        text,
                        revision
                    ])
                    .map_err(io::Error::other)?;
                m.statement(&q);
                m.row(text.len() + 64)?;
                if changed != 1 {
                    return Ok(m.result(Outcome::Conflict, None));
                }
            }
        }
        // The external membership fence may itself observe/reenter metadata.
        // Recheck every local witness after its final invocation as well.
        if !closure(&tx, job.id).map_err(io::Error::other)?
            || !job_current(&tx, job).map_err(io::Error::other)?
            || !check_source(&tx, witness).map_err(io::Error::other)?
            || !repair_current(&tx, repair).map_err(io::Error::other)?
            || !admitted_original(&tx, job, witness).map_err(io::Error::other)?
            || !same(&binding(&tx, repair, object, &mut m)?, &b)?
        {
            return Ok(m.result(Outcome::Conflict, None));
        }
        if crate::native_evidence::query_runtime_generation(&options)? != job.spec.runtime {
            return Ok(m.result(Outcome::Conflict, None));
        }
        if cancelled() {
            return Ok(m.result(Outcome::Deferred, None));
        }
        let result = if map_complete {
            qualified(
                &build,
                revision.map_or(0, |r| r + i64::from(!already_complete)),
            )
        } else {
            None
        };
        tx.commit().map_err(io::Error::other)?;
        Ok(m.result(
            if result.is_some() {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            result,
        ))
    }
}
fn handle_current(db: &Connection, handle: &QualifiedObject, m: &mut Meter) -> io::Result<bool> {
    let Some((revision, b)) = load_build(db, &handle.binding, m)? else {
        return Ok(false);
    };
    Ok(revision == handle.revision
        && qualified(&b, revision)
            .is_some_and(|v| v.root == handle.root && v.count == handle.count))
}
fn entry_current(
    db: &Connection,
    repair: &Repair,
    b: &Binding,
    n: &Node,
    m: &mut Meter,
) -> io::Result<()> {
    let (field, _, vm) = native_classes::qualified_field_at(
        db,
        repair,
        b.record.record,
        n.key.ordinal,
        &mut m.work,
    )?;
    m.vm += vm;
    let (value, end) = native_node(db, &b.record, n.value.address, m)?;
    if !same(&field, &n.key)? || !same(&value, &n.value)? || end > b.end {
        return Err(bad("native object selected physical descriptor changed"));
    }
    let (text, vm) = native_classes::qualified_identity_at(
        db,
        repair,
        b.record.record,
        n.key.ordinal,
        &mut m.work,
    )?;
    m.vm += vm;
    if text
        .as_ref()
        .is_none_or(|t| t.identity() != n.class || t.original() != b.record.original)
    {
        return Err(bad("native object key/class membership changed"));
    }
    Ok(())
}
impl NativeStore {
    /// Advance one indexed traversal transition, returning at most one member.
    /// Seek takes O(height) transitions; subsequent entries take O(height+output)
    /// total traversal work. No key decoding, hashing, Index load or source-value
    /// preparation occurs here. Each successful entry is a real native node.
    pub(crate) fn original_object_page_step(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        cursor: &ObjectPageCursor,
        records: &crate::query_source::Records<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<ObjectPageStep> {
        cursor.valid()?;
        let mut m = Meter::default();
        let mut next = cursor.clone();
        let latch = std::cell::Cell::new(false);
        let cancelled = || {
            if cancelled() {
                latch.set(true);
            }
            latch.get()
        };
        let result = |m: Meter,
                      outcome: Outcome,
                      cursor: ObjectPageCursor,
                      entry: Option<ObjectEntry>| ObjectPageStep {
            outcome,
            work: m.work,
            complete: cursor.complete,
            cursor,
            entry,
            sql_vm_steps: m.vm,
        };
        if cancelled() {
            return Ok(result(m, Outcome::Deferred, cursor.clone(), None));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
        }
        let b = &cursor.object.binding;
        let selected = {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            if !current(&tx, job, witness, repair, b, closure, &mut m)?
                || !handle_current(&tx, &cursor.object, &mut m)?
            {
                return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
            }
            let mut selected = None;
            if !next.complete {
                if let Some(branch) = next.at.clone() {
                    let n = node(&tx, b, branch.class, &mut m)?;
                    let (left, right) = checked(&tx, b, &n, &mut m)?;
                    if n.count != branch.count || n.lexical_start != Some(branch.start) {
                        return Err(bad("native object active subtree count changed"));
                    }
                    let rank = branch
                        .start
                        .checked_add(count(&left))
                        .ok_or_else(|| bad("native object rank overflow"))?;
                    let right = right.as_ref().map(|n| Branch {
                        class: n.class,
                        start: rank + 1,
                        count: n.count,
                    });
                    let left = left.as_ref().map(|n| Branch {
                        class: n.class,
                        start: branch.start,
                        count: n.count,
                    });
                    if next.next_rank < rank {
                        next.path.push(Frame {
                            branch,
                            rank,
                            right,
                        });
                        next.at = left;
                    } else if next.next_rank == rank {
                        next.next_rank += 1;
                        next.seeking = false;
                        next.at = right;
                        selected = Some(n);
                    } else if next.seeking {
                        next.at = right;
                    } else {
                        return Err(bad("native object page skipped an unconsumed key"));
                    }
                } else if let Some(frame) = next.path.pop() {
                    if next.seeking || frame.rank != next.next_rank {
                        return Err(bad("native object page ancestor rank changed"));
                    }
                    let n = node(&tx, b, frame.branch.class, &mut m)?;
                    let (left, right) = checked(&tx, b, &n, &mut m)?;
                    let rank = frame
                        .branch
                        .start
                        .checked_add(count(&left))
                        .ok_or_else(|| bad("native object ancestor rank overflow"))?;
                    let right = right.as_ref().map(|n| Branch {
                        class: n.class,
                        start: rank + 1,
                        count: n.count,
                    });
                    if n.lexical_start != Some(frame.branch.start)
                        || n.count != frame.branch.count
                        || rank != frame.rank
                        || right != frame.right
                    {
                        return Err(bad("native object page ancestor edge changed"));
                    }
                    next.next_rank += 1;
                    next.at = right;
                    selected = Some(n);
                } else {
                    return Err(bad("native object page ended before its qualified count"));
                }
                next.complete = next.next_rank == next.object.count;
                next.valid()?;
            }
            if let Some(n) = &selected {
                entry_current(&tx, repair, b, n, &mut m)?;
            }
            selected
        };
        // No provider I/O under a SQLite read/write transaction. Cached pages
        // still prove the Original and reauthorize archive/provider callbacks.
        verify_member_anchor(records, repair, &mut m.work)?;
        records.finish()?;
        if cancelled() {
            return Ok(result(m, Outcome::Deferred, cursor.clone(), None));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
        }
        {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            if !current(&tx, job, witness, repair, b, closure, &mut m)?
                || !handle_current(&tx, &cursor.object, &mut m)?
            {
                return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
            }
            if let Some(n) = &selected {
                let now = node(&tx, b, n.class, &mut m)?;
                if !same(&now, n)? {
                    return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
                }
                entry_current(&tx, repair, b, n, &mut m)?;
            }
        }
        // Release the read snapshot BEFORE the last external fence. A callback
        // can invalidate related owners without mutating this object's source.
        if !closure(&self.connection, job.id).map_err(io::Error::other)?
            || !self.runtime_current(&job.spec)?
            || !job_current(&self.connection, job).map_err(io::Error::other)?
            || !check_source(&self.connection, witness).map_err(io::Error::other)?
            || !repair_current(&self.connection, repair).map_err(io::Error::other)?
            || !admitted_original(&self.connection, job, witness).map_err(io::Error::other)?
            || !same(&binding(&self.connection, repair, &b.object, &mut m)?, b)?
            || !handle_current(&self.connection, &cursor.object, &mut m)?
        {
            return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
        }
        if cancelled() {
            return Ok(result(m, Outcome::Deferred, cursor.clone(), None));
        }
        bounded_json(&next, STATE_BYTES).map_err(io::Error::other)?;
        let entry = selected.map(|n| ObjectEntry {
            key: OriginalText::qualified(b.record.original, n.class),
            physical_key: n.key,
            value: n.value,
        });
        Ok(result(m, Outcome::Written, next, entry))
    }
}

/// Background native-map continuation. Native serde_json preserve_order emits a
/// key at its FIRST physical insertion, selecting its LAST physical value.
/// Duplicates advance this cursor without an entry; callers checkpoint every
/// step and must never drain an unfinished object inside a foreground request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectNativeCursor {
    object: QualifiedObject,
    after: Option<u64>,
    emitted: u64,
    complete: bool,
}
impl ObjectNativeCursor {
    pub(crate) fn begin(object: &QualifiedObject) -> Self {
        Self {
            object: object.clone(),
            after: None,
            emitted: 0,
            complete: false,
        }
    }
    fn valid(&self) -> io::Result<()> {
        if (self.object.count == 0) != self.object.root.is_none()
            || self.emitted > self.object.count
            || self.after.is_none() && self.emitted != 0
            || self.after.is_some() && self.emitted == 0
            || self.complete && self.emitted != self.object.count
        {
            return Err(bad("native insertion-order cursor state disagrees"));
        }
        if let Some(after) = self.after {
            seq_u64(after)?;
        }
        Ok(())
    }
}
#[derive(Debug)]
pub(crate) struct ObjectNativeStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) cursor: ObjectNativeCursor,
    pub(crate) entry: Option<ObjectEntry>,
    pub(crate) complete: bool,
    pub(crate) sql_vm_steps: i32,
}
fn native_first(
    db: &Connection,
    repair: &Repair,
    b: &Binding,
    n: &Node,
    m: &mut Meter,
) -> io::Result<()> {
    let (first, _, vm) = native_classes::qualified_field_at(
        db,
        repair,
        b.record.record,
        n.first_ordinal,
        &mut m.work,
    )?;
    m.vm += vm;
    let (text, vm) = native_classes::qualified_identity_at(
        db,
        repair,
        b.record.record,
        n.first_ordinal,
        &mut m.work,
    )?;
    m.vm += vm;
    if !first.key
        || first.location.parent != Some(b.object.address)
        || text
            .as_ref()
            .is_none_or(|t| t.original() != b.record.original || t.identity() != n.class)
    {
        return Err(bad("native object first insertion/class changed"));
    }
    Ok(())
}
impl NativeStore {
    pub(crate) fn original_object_native_step(
        &self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        cursor: &ObjectNativeCursor,
        records: &crate::query_source::Records<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<ObjectNativeStep> {
        cursor.valid()?;
        let mut next = cursor.clone();
        let mut m = Meter::default();
        let latch = std::cell::Cell::new(false);
        let cancelled = || {
            if cancelled() {
                latch.set(true);
            }
            latch.get()
        };
        let result = |m: Meter,
                      outcome: Outcome,
                      cursor: ObjectNativeCursor,
                      entry: Option<ObjectEntry>| ObjectNativeStep {
            outcome,
            work: m.work,
            complete: cursor.complete,
            cursor,
            entry,
            sql_vm_steps: m.vm,
        };
        if cancelled() {
            return Ok(result(m, Outcome::Deferred, cursor.clone(), None));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
        }
        let b = &cursor.object.binding;
        let mut observed_input = None;
        let mut observed_node = None;
        let selected = {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            if !current(&tx, job, witness, repair, b, closure, &mut m)?
                || !handle_current(&tx, &cursor.object, &mut m)?
            {
                return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
            }
            // Exactly one indexed physical input key, not a loop that drains
            // duplicate keys or decodes a complete object/key String.
            let input = next_key(&tx, b, cursor.after, repair, &mut m)?;
            match input {
                None => {
                    if next.emitted != next.object.count {
                        return Err(bad("native insertion cursor ended before all keys"));
                    }
                    next.complete = true;
                    None
                }
                Some(input) => {
                    if next.complete {
                        return Err(bad(
                            "native insertion cursor completed before physical input end",
                        ));
                    }
                    let (text, vm) = native_classes::qualified_identity_at(
                        &tx,
                        repair,
                        b.record.record,
                        input.field.ordinal,
                        &mut m.work,
                    )?;
                    m.vm += vm;
                    let text = text
                        .ok_or_else(|| bad("native insertion key identity remains unqualified"))?;
                    let n = node(&tx, b, text.identity(), &mut m)?;
                    if n.lexical_start
                        .and_then(|start| start.checked_add(n.count))
                        .is_none_or(|end| end > cursor.object.count)
                    {
                        return Err(bad("native insertion tree is not sealed"));
                    }
                    checked(&tx, b, &n, &mut m)?;
                    native_first(&tx, repair, b, &n, &mut m)?;
                    entry_current(&tx, repair, b, &n, &mut m)?;
                    if input.field.ordinal < n.first_ordinal {
                        return Err(bad("native insertion key precedes its first ordinal"));
                    }
                    observed_input = Some((input.field.clone(), text));
                    observed_node = Some(n.clone());
                    next.after = Some(input.field.ordinal);
                    if input.field.ordinal == n.first_ordinal {
                        next.emitted = next
                            .emitted
                            .checked_add(1)
                            .ok_or_else(|| bad("native insertion output count overflow"))?;
                        if next.emitted > next.object.count {
                            return Err(bad("native insertion output exceeds qualified count"));
                        }
                        Some(n)
                    } else {
                        None
                    }
                }
            }
        };
        next.valid()?;
        verify_member_anchor(records, repair, &mut m.work)?;
        records.finish()?;
        if cancelled() {
            return Ok(result(m, Outcome::Deferred, cursor.clone(), None));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
        }
        {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            if !current(&tx, job, witness, repair, b, closure, &mut m)?
                || !handle_current(&tx, &cursor.object, &mut m)?
            {
                return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
            }
            if let Some(n) = &observed_node {
                let now = node(&tx, b, n.class, &mut m)?;
                if !same(&now, n)? {
                    return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
                }
                native_first(&tx, repair, b, n, &mut m)?;
                entry_current(&tx, repair, b, n, &mut m)?;
            }
        }
        if let Some((field, text)) = &observed_input {
            let (now, _, vm) = native_classes::qualified_field_at(
                &self.connection,
                repair,
                b.record.record,
                field.ordinal,
                &mut m.work,
            )?;
            m.vm += vm;
            let (identity, vm) = native_classes::qualified_identity_at(
                &self.connection,
                repair,
                b.record.record,
                field.ordinal,
                &mut m.work,
            )?;
            m.vm += vm;
            if !same(field, &now)? || identity.as_ref() != Some(text) {
                return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
            }
        }
        if !closure(&self.connection, job.id).map_err(io::Error::other)?
            || !self.runtime_current(&job.spec)?
            || !job_current(&self.connection, job).map_err(io::Error::other)?
            || !check_source(&self.connection, witness).map_err(io::Error::other)?
            || !repair_current(&self.connection, repair).map_err(io::Error::other)?
            || !admitted_original(&self.connection, job, witness).map_err(io::Error::other)?
            || !same(&binding(&self.connection, repair, &b.object, &mut m)?, b)?
            || !handle_current(&self.connection, &cursor.object, &mut m)?
        {
            return Ok(result(m, Outcome::Conflict, cursor.clone(), None));
        }
        if cancelled() {
            return Ok(result(m, Outcome::Deferred, cursor.clone(), None));
        }
        bounded_json(&next, STATE_BYTES).map_err(io::Error::other)?;
        let entry = selected.map(|n| ObjectEntry {
            key: OriginalText::qualified(b.record.original, n.class),
            physical_key: n.key,
            value: n.value,
        });
        Ok(result(
            m,
            if entry.is_some() || next.complete {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            next,
            entry,
        ))
    }
}
