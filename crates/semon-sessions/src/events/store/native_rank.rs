//! Private conditional range storage. Terminal facts and their ordered algebra
//! belong to the native model. A graph can have exponential source-context
//! complexity; decision depth alone is not an append-work guarantee.
//!
//! Immutable nodes are shared across publication roots. Private root bindings,
//! memo ownership and scalar Apply cursors use the existing producer job. No
//! public/standalone Trace schema, source, archive or custody row changes here.
use super::{native_query, *};
use native_query::{Job, NativeStore};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

mod candidate_tree;
pub(crate) use candidate_tree::{
    CandidateBounds, CandidateDirection, CandidateLeaf, CandidatePoint, CandidateRead,
    CandidateReadOutcome, CandidateRoot, CandidateStep, CandidateWork,
};
/// The candidate-tree schema is exported separately so the private schema
/// owner can register it with its version bump. Public Trace schema is not
/// involved.
pub(super) const CANDIDATE_TREE_SCHEMA: &str = candidate_tree::SCHEMA;

pub(crate) mod orthogonal;
pub(super) const ORTHOGONAL_SCHEMA: &str = orthogonal::SCHEMA;

const NODE_BYTES: usize = 64 * 1024;
const STEP_BYTES: usize = native_query::BYTES;
const STEP_ROWS: usize = native_query::ROWS;
const VARIABLES: u8 = 64;
const ROW_PREFIX: &str = "rank/v1/";

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_rank_nodes (
 identity TEXT PRIMARY KEY, level INTEGER NOT NULL, metadata TEXT NOT NULL,
 absent TEXT, present TEXT, refs INTEGER NOT NULL CHECK(refs>=0)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_rank_unused ON native_rank_nodes(identity) WHERE refs=0;
CREATE TABLE IF NOT EXISTS native_rank_bindings (
 job INTEGER NOT NULL, identity TEXT NOT NULL, PRIMARY KEY(job,identity)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_rank_memos (
 job INTEGER NOT NULL, identity TEXT NOT NULL, metadata TEXT NOT NULL,
 PRIMARY KEY(job,identity)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_rank_memo_identity ON native_rank_memos(identity,job DESC);
";

/// Called by the existing private repair cleanup owner. Memo rows are removed
/// before their job bindings: a shared memo can never name an already-collected
/// result while another transaction is importing that result.
pub(super) fn cleanup(
    db: &Connection,
    job: i64,
    budget: usize,
    bytes: &mut usize,
) -> rusqlite::Result<(usize, bool)> {
    let (mut used, tree_complete) = candidate_tree::cleanup(db, job, budget, bytes)?;
    if !tree_complete || used >= budget {
        return Ok((used, false));
    }
    let (orthogonal_removed, orthogonal_complete) =
        orthogonal::cleanup(db, job, budget - used, bytes)?;
    used += orthogonal_removed;
    if !orthogonal_complete || used >= budget {
        return Ok((used, false));
    }
    let mut q = db.prepare("SELECT identity,octet_length(metadata)+octet_length(identity) FROM native_rank_memos WHERE job=?1 ORDER BY identity LIMIT ?2")?;
    let records = q
        .query_map(params![job, (budget - used) as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (identity, length) in records {
        if length > *bytes || used == budget {
            break;
        }
        db.execute(
            "DELETE FROM native_rank_memos WHERE job=?1 AND identity=?2",
            params![job, identity],
        )?;
        *bytes -= length;
        used += 1;
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_rank_memos WHERE job=?1)",
        [job],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok((used, false));
    }
    let mut q = db.prepare(
        "SELECT identity FROM native_rank_bindings WHERE job=?1 ORDER BY identity LIMIT ?2",
    )?;
    let bindings = q
        .query_map(params![job, (budget - used) / 2], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for identity in bindings {
        if identity.len() > *bytes {
            break;
        }
        db.execute(
            "DELETE FROM native_rank_bindings WHERE job=?1 AND identity=?2",
            params![job, identity],
        )?;
        if db.execute(
            "UPDATE native_rank_nodes SET refs=refs-1 WHERE identity=?1",
            [&identity],
        )? != 1
        {
            return Err(invalid("native rank cleanup dependency is absent"));
        }
        *bytes -= identity.len();
        used += 2;
    }
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_rank_bindings WHERE job=?1)",
        [job],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok((used, false));
    }
    // Each removed node can expose its two children as unused. Seek again,
    // rather than decode a recursive historical graph or run cascading DELETE.
    while budget - used >= 3 {
        let record: Option<(String,Option<String>,Option<String>,usize)> = db.query_row("SELECT identity,absent,present,octet_length(metadata)+octet_length(identity)+COALESCE(octet_length(absent),0)+COALESCE(octet_length(present),0) FROM native_rank_nodes WHERE refs=0 ORDER BY identity LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((identity, absent, present, length)) = record else {
            break;
        };
        if length > *bytes {
            break;
        }
        db.execute(
            "DELETE FROM native_rank_nodes WHERE identity=?1 AND refs=0",
            [&identity],
        )?;
        used += 1;
        for child in [absent, present].into_iter().flatten() {
            if db.execute(
                "UPDATE native_rank_nodes SET refs=refs-1 WHERE identity=?1",
                [&child],
            )? != 1
            {
                return Err(invalid("native rank cleanup child is absent"));
            }
            used += 1;
        }
        *bytes -= length;
    }
    let complete = !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_rank_nodes WHERE refs=0)",
        [],
        |r| r.get::<_, bool>(0),
    )?;
    Ok((used, complete))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        native_stage::{Entry, EntryKind},
        range_stage::{Facts, Leaf, Order},
        turn_stage::HandoffEnd,
    };

    fn combine(left: &str, right: &str) -> io::Result<String> {
        let mut left: Facts = serde_json::from_str(left).map_err(io::Error::other)?;
        left.append(serde_json::from_str(right).map_err(io::Error::other)?)
            .map_err(failure)?;
        serde_json::to_string(&left).map_err(io::Error::other)
    }
    fn leaf(seq: usize) -> Facts {
        let kind = match seq % 4 {
            0 => EntryKind::U,
            1 => EntryKind::A,
            2 => EntryKind::Tool(crate::model::ToolState::Pending),
            _ => EntryKind::H(seq),
        };
        let entry = Entry {
            kind,
            file: seq % 8,
            offset: seq as u64,
            pos: (seq as u64, 0),
            t: Some(seq as i64),
            at: Some((seq % 8, seq)),
        };
        let event = Event {
            t: entry.t,
            ..Event::default()
        };
        let mut result = Facts::default();
        result
            .observe(Leaf {
                order: Order {
                    key: (seq as i64, 0, seq as u64, 0, 1),
                    ordinal: seq as u64,
                    identity: Sha256::digest(seq.to_le_bytes()).into(),
                },
                entry: &entry,
                found: Some(&event),
                outgoing: matches!(kind, EntryKind::H(_)).then_some(HandoffEnd {
                    id: "H-fixed-id",
                    kind: "toyou",
                    status: "done",
                }),
                boundary: matches!(kind, EntryKind::U),
                count: 1,
            })
            .unwrap();
        result
    }
    fn guard(seq: usize, sources: u8) -> (u64, u64) {
        let required = 1u64 << (seq % usize::from(sources));
        let forbidden = (seq / usize::from(sources)) as u64 & ((1u64 << sources) - 1) & !required;
        (required, forbidden)
    }
    fn oracle(earlier: usize, sources: u8, bits: u64) -> Facts {
        let mut facts = Facts::default();
        for seq in 0..earlier {
            let (required, forbidden) = guard(seq, sources);
            if bits & required == required && bits & forbidden == 0 {
                facts.append(leaf(seq)).unwrap();
            }
        }
        facts
    }
    // Independent complete-native fixture construction, deliberately outside
    // the measured request/update. This is not a production warmup path.
    fn fixture_root(
        graph: &mut Graph<'_>,
        earlier: usize,
        sources: u8,
        source: u8,
        bits: u64,
    ) -> NodeId {
        if source == sources {
            return graph
                .intern(Node::Terminal {
                    facts: serde_json::to_string(&oracle(earlier, sources, bits)).unwrap(),
                })
                .unwrap();
        }
        let absent = fixture_root(graph, earlier, sources, source + 1, bits);
        let present = fixture_root(graph, earlier, sources, source + 1, bits | 1u64 << source);
        graph
            .intern(Node::Decision {
                source,
                absent,
                present,
            })
            .unwrap()
    }
    struct Database {
        root: PathBuf,
        db: Connection,
    }
    impl std::ops::Deref for Database {
        type Target = Connection;
        fn deref(&self) -> &Connection {
            &self.db
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn database() -> Database {
        let root = std::env::temp_dir().join(format!(
            "semon-native-rank-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut db = Connection::open(root.join("index.sqlite3")).unwrap();
        assert!(matches!(
            super::super::init(&mut db).unwrap(),
            super::super::Init::Ready
        ));
        Database { root, db }
    }

    #[test]
    fn native_rank_conditional_apply_measures_context_growth_not_only_decision_depth() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        for sources in [3, 5, 8] {
            for earlier in [0, 128, 1024, 8192] {
                let db = database();
                let mut graph = Graph {
                    db: &db,
                    job: 1,
                    work: Work::default(),
                };
                let identity = graph
                    .intern(Node::Terminal {
                        facts: serde_json::to_string(&Facts::default()).unwrap(),
                    })
                    .unwrap();
                let root = fixture_root(&mut graph, earlier, sources, 0, 0);
                let delta = leaf(8193);
                let value = graph
                    .intern(Node::Terminal {
                        facts: serde_json::to_string(&delta).unwrap(),
                    })
                    .unwrap();
                let required = graph
                    .intern(Node::Decision {
                        source: 1,
                        absent: value,
                        present: identity.clone(),
                    })
                    .unwrap();
                let right = graph
                    .intern(Node::Decision {
                        source: 0,
                        absent: identity.clone(),
                        present: required,
                    })
                    .unwrap();
                let mut state = Apply {
                    version: "native-range-test-v1".into(),
                    identity: identity.clone(),
                    frames: vec![Frame {
                        pair: Pair { left: root, right },
                        phase: Phase::Visit,
                    }],
                    returned: None,
                };
                let cursor = cursor_identity("fixed-append").unwrap();
                graph.put(&cursor, &encoded(&state).unwrap()).unwrap();
                let total_vm = Arc::new(AtomicUsize::new(0));
                let observed = total_vm.clone();
                db.progress_handler(
                    1,
                    Some(move || {
                        observed.fetch_add(1, Ordering::Relaxed);
                        false
                    }),
                );
                let mut visits = 0;
                let mut writes = 0;
                let mut bytes = 0;
                let mut steps = 0;
                while !state.frames.is_empty() {
                    let tx = db.unchecked_transaction().unwrap();
                    let mut part = Graph {
                        db: &tx,
                        job: 1,
                        work: Work::default(),
                    };
                    // Scalar cursor is reloaded from durable rows every step.
                    state = serde_json::from_str(&part.get(&cursor).unwrap().unwrap()).unwrap();
                    part.advance(&mut state, &combine).unwrap();
                    part.put(&cursor, &encoded(&state).unwrap()).unwrap();
                    assert!(part.work.rows() <= STEP_ROWS);
                    assert!(part.work.metadata_bytes <= STEP_BYTES);
                    assert!(part.work.max_stack <= usize::from(sources) + 1);
                    assert!(part.work.max_live_node_bytes < 4096);
                    visits += part.work.apply_visits;
                    writes += part.work.nodes_written;
                    bytes += part.work.metadata_bytes;
                    tx.commit().unwrap();
                    steps += 1;
                    assert!(steps < 1000);
                }
                let root = state.returned.unwrap();
                assert!(
                    visits <= 4 * (1usize << sources),
                    "source context bound sources={sources},earlier={earlier},visits={visits}"
                );
                assert!(writes <= 4 * (1usize << sources));
                println!(
                    "native_rank sources={sources} earlier={earlier} visits={visits} allocations={writes} bytes={bytes} sqlite_vm={} steps={steps}",
                    total_vm.load(Ordering::Relaxed)
                );
                for included in 0..1u64 << sources {
                    let mut complete = oracle(earlier, sources, included);
                    if included & 1 != 0 && included & 2 == 0 {
                        complete.append(delta.clone()).unwrap();
                    }
                    let mut reader = Graph {
                        db: &db,
                        job: 1,
                        work: Work::default(),
                    };
                    let facts = reader.evaluate(&root, included).unwrap();
                    assert_eq!(
                        serde_json::from_str::<Value>(&facts).unwrap(),
                        serde_json::to_value(complete).unwrap()
                    );
                    assert!(reader.work.sql_reads <= usize::from(sources) + 1);
                    assert!(reader.work.metadata_bytes < 8192);
                }
                if sources == 8 && earlier == 8192 {
                    assert!(
                        visits > 2 * usize::from(sources),
                        "depth-only bound must not hide context expansion"
                    );
                }
            }
        }
    }

    #[test]
    fn native_rank_immutable_roots_survive_old_job_cleanup_without_prefix_copy() {
        let db = database();
        let mut original = Graph {
            db: &db,
            job: 1,
            work: Work::default(),
        };
        let root = fixture_root(&mut original, 128, 5, 0, 0);
        let nodes_before: i64 = db
            .query_row("SELECT count(*) FROM native_rank_nodes", [], |r| r.get(0))
            .unwrap();
        let mut imported = Graph {
            db: &db,
            job: 2,
            work: Work::default(),
        };
        imported.node(&root).unwrap();
        imported.bind(&root).unwrap();
        assert_eq!(imported.work.sql_reads, 1);
        assert_eq!(imported.work.sql_writes, 2);
        assert_eq!(imported.work.nodes_written, 0);
        let mut rounds = 0;
        loop {
            let mut bytes = STEP_BYTES;
            let (rows, complete) = cleanup(&db, 1, STEP_ROWS, &mut bytes).unwrap();
            assert!(rows <= STEP_ROWS);
            rounds += 1;
            if complete {
                break;
            }
            assert!(rounds < 1000);
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM native_rank_nodes", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            nodes_before
        );
        for bits in 0..32 {
            let mut reader = Graph {
                db: &db,
                job: 2,
                work: Work::default(),
            };
            assert_eq!(
                serde_json::from_str::<Value>(&reader.evaluate(&root, bits).unwrap()).unwrap(),
                serde_json::to_value(oracle(128, 5, bits)).unwrap()
            );
        }
        loop {
            let mut bytes = STEP_BYTES;
            let (_, complete) = cleanup(&db, 2, STEP_ROWS, &mut bytes).unwrap();
            if complete {
                break;
            }
            rounds += 1;
            assert!(rounds < 1000);
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM native_rank_nodes", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeId {
    level: u8,
    digest: String,
}

impl NodeId {
    fn valid(&self) -> bool {
        self.level <= VARIABLES
            && self.digest.len() == 64
            && self.digest.bytes().all(|b| b.is_ascii_hexdigit())
    }
    fn identity(&self) -> String {
        format!("{ROW_PREFIX}node/{}/{}", self.level, self.digest)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Node {
    Terminal {
        facts: String,
    },
    Decision {
        source: u8,
        absent: NodeId,
        present: NodeId,
    },
}

impl Node {
    fn level(&self) -> u8 {
        match self {
            Self::Terminal { .. } => VARIABLES,
            Self::Decision { source, .. } => *source,
        }
    }
    fn valid(&self) -> bool {
        match self {
            Self::Terminal { facts } => facts.len() <= NODE_BYTES,
            Self::Decision {
                source,
                absent,
                present,
            } => {
                *source < VARIABLES
                    && absent.valid()
                    && present.valid()
                    && absent.level > *source
                    && present.level > *source
                    && absent != present
            }
        }
    }
    fn children(&self, source: u8, original: &NodeId) -> (NodeId, NodeId) {
        match self {
            Self::Decision {
                source: own,
                absent,
                present,
            } if *own == source => (absent.clone(), present.clone()),
            _ => (original.clone(), original.clone()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    left: NodeId,
    right: NodeId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Phase {
    Visit,
    Absent { source: u8, present: Pair },
    Present { source: u8, absent: NodeId },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    pair: Pair,
    phase: Phase,
}

/// Scalar DFS state. It holds IDs, never historical nodes or native entries.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    version: String,
    identity: NodeId,
    frames: Vec<Frame>,
    returned: Option<NodeId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Guard {
    decisions: Vec<(u8, bool)>,
    empty: NodeId,
    result: NodeId,
    remaining: usize,
}

fn valid_guard(guard: &[(u8, bool)]) -> bool {
    guard.len() <= usize::from(VARIABLES)
        && guard.iter().all(|(source, _)| *source < VARIABLES)
        && guard.windows(2).all(|w| w[0].0 < w[1].0)
}

impl Apply {
    fn valid(&self) -> bool {
        !self.version.is_empty()
            && self.version.len() <= 256
            && self.frames.len() <= usize::from(VARIABLES) + 1
            && self.identity.valid()
            && self.returned.as_ref().is_none_or(NodeId::valid)
            && self.frames.iter().all(|frame| {
                frame.pair.left.valid()
                    && frame.pair.right.valid()
                    && match &frame.phase {
                        Phase::Visit => true,
                        Phase::Absent { source, present } => {
                            *source < VARIABLES && present.left.valid() && present.right.valid()
                        }
                        Phase::Present { source, absent } => {
                            *source < VARIABLES && absent.valid() && absent.level > *source
                        }
                    }
            })
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Work {
    pub(crate) sql_reads: usize,
    pub(crate) sql_writes: usize,
    pub(crate) metadata_bytes: usize,
    pub(crate) vm_steps: i64,
    pub(crate) apply_visits: usize,
    pub(crate) memo_hits: usize,
    pub(crate) nodes_written: usize,
    pub(crate) max_stack: usize,
    pub(crate) max_live_node_bytes: usize,
}

impl Work {
    fn rows(&self) -> usize {
        self.sql_reads + self.sql_writes
    }
}

pub(crate) struct Advance {
    pub(crate) outcome: Outcome,
    pub(crate) complete: Option<NodeId>,
    pub(crate) work: Work,
}

pub(crate) struct GuardInput<'a> {
    pub(crate) cursor: &'a str,
    pub(crate) facts: &'a str,
    pub(crate) empty: &'a str,
    pub(crate) decisions: &'a [(u8, bool)],
}

pub(crate) struct ApplyInput<'a> {
    pub(crate) cursor: &'a str,
    pub(crate) version: &'a str,
    pub(crate) identity: &'a NodeId,
    pub(crate) left: &'a NodeId,
    pub(crate) right: &'a NodeId,
}

/// Point statements only. Witness/runtime/closure checks belong to the owning
/// transaction. Tests additionally measure all its SQLite VM steps through the
/// progress hook, including those checks, CAS writes and commit.
struct Graph<'a> {
    db: &'a Connection,
    job: i64,
    work: Work,
}

fn failure(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn encoded<T: Serialize>(value: &T) -> io::Result<String> {
    let text = serde_json::to_string(value).map_err(io::Error::other)?;
    if text.len() > NODE_BYTES {
        return Err(failure("native rank scalar budget exceeded"));
    }
    Ok(text)
}
fn node_identity(node: &Node, text: &str) -> NodeId {
    NodeId {
        level: node.level(),
        digest: format!("{:x}", Sha256::digest(text.as_bytes())),
    }
}
fn memo_identity(version: &str, pair: &Pair) -> io::Result<String> {
    Ok(format!(
        "{ROW_PREFIX}memo/{:x}",
        Sha256::digest(encoded(&(version, pair))?.as_bytes())
    ))
}
fn cursor_identity(cursor: &str) -> io::Result<String> {
    if cursor.is_empty() || cursor.len() > 256 {
        return Err(failure("invalid native rank cursor"));
    }
    Ok(format!("{ROW_PREFIX}apply/{cursor}"))
}

impl Graph<'_> {
    fn get(&mut self, identity: &str) -> io::Result<Option<String>> {
        let sql = if identity.starts_with(&format!("{ROW_PREFIX}node/")) {
            "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_rank_nodes WHERE identity=?2 AND ?1 IS NOT NULL"
        } else if identity.starts_with(&format!("{ROW_PREFIX}memo/")) {
            "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_rank_memos WHERE identity=?2 AND ?1 IS NOT NULL ORDER BY job DESC LIMIT 1"
        } else {
            "SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END FROM native_query_rows WHERE job=?1 AND kind='metadata' AND k0=0 AND k1=0 AND k2=0 AND k3=0 AND k4=0 AND ordinal=0 AND identity=?2"
        };
        let mut q = self.db.prepare_cached(sql).map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let row: Option<Option<String>> = q
            .query_row(params![self.job, identity], |r| r.get(0))
            .optional()
            .map_err(io::Error::other)?;
        self.work.sql_reads += 1;
        self.work.vm_steps += i64::from(q.get_status(rusqlite::StatementStatus::VmStep));
        match row {
            Some(Some(text)) => {
                self.work.metadata_bytes += text.len();
                Ok(Some(text))
            }
            Some(None) => Err(failure("native rank row exceeds scalar budget")),
            None => Ok(None),
        }
    }
    fn put(&mut self, identity: &str, text: &str) -> io::Result<()> {
        if text.len() > NODE_BYTES {
            return Err(failure("native rank row exceeds scalar budget"));
        }
        let sql = if identity.starts_with(&format!("{ROW_PREFIX}memo/")) {
            "INSERT INTO native_rank_memos VALUES(?1,?2,?3) ON CONFLICT(job,identity) DO UPDATE SET metadata=excluded.metadata"
        } else {
            "INSERT INTO native_query_rows VALUES(?1,'metadata',0,0,0,0,0,0,?2,?3) ON CONFLICT(job,kind,k0,k1,k2,k3,k4,ordinal,identity) DO UPDATE SET metadata=excluded.metadata"
        };
        let mut q = self.db.prepare_cached(sql).map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        q.execute(params![self.job, identity, text])
            .map_err(io::Error::other)?;
        self.work.sql_writes += 1;
        self.work.metadata_bytes += text.len();
        self.work.vm_steps += i64::from(q.get_status(rusqlite::StatementStatus::VmStep));
        Ok(())
    }
    fn node(&mut self, id: &NodeId) -> io::Result<Node> {
        if !id.valid() {
            return Err(failure("invalid native rank node address"));
        }
        let text = self
            .get(&id.identity())?
            .ok_or_else(|| failure("native rank node is absent"))?;
        let node: Node = serde_json::from_str(&text).map_err(io::Error::other)?;
        if !node.valid() || node_identity(&node, &text) != *id {
            return Err(failure("native rank node identity or ordering changed"));
        }
        self.work.max_live_node_bytes = self.work.max_live_node_bytes.max(text.len());
        Ok(node)
    }
    fn intern(&mut self, node: Node) -> io::Result<NodeId> {
        if let Node::Decision {
            absent, present, ..
        } = &node
            && absent == present
        {
            return Ok(absent.clone());
        }
        if !node.valid() {
            return Err(failure("invalid native rank decision ordering"));
        }
        let text = encoded(&node)?;
        let id = node_identity(&node, &text);
        let (absent, present) = match &node {
            Node::Decision {
                absent, present, ..
            } => (Some(absent.identity()), Some(present.identity())),
            Node::Terminal { .. } => (None, None),
        };
        let mut q = self.db.prepare_cached("INSERT INTO native_rank_nodes VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(identity) DO NOTHING").map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let allocated = q
            .execute(params![id.identity(), id.level, text, absent, present])
            .map_err(io::Error::other)?;
        self.work.sql_writes += 1;
        self.work.metadata_bytes += text.len();
        self.work.vm_steps += i64::from(q.get_status(rusqlite::StatementStatus::VmStep));
        self.work.nodes_written += allocated;
        drop(q);
        if allocated != 0 {
            for child in [absent, present].into_iter().flatten() {
                self.references(&child, 1)?;
            }
        }
        self.bind(&id)?;
        Ok(id)
    }
    fn references(&mut self, identity: &str, delta: i64) -> io::Result<()> {
        let mut q = self
            .db
            .prepare_cached("UPDATE native_rank_nodes SET refs=refs+?2 WHERE identity=?1")
            .map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = q
            .execute(params![identity, delta])
            .map_err(io::Error::other)?;
        self.work.sql_writes += 1;
        self.work.vm_steps += i64::from(q.get_status(rusqlite::StatementStatus::VmStep));
        if changed != 1 {
            return Err(failure("native rank dependency is absent"));
        }
        Ok(())
    }
    fn bind(&mut self, id: &NodeId) -> io::Result<()> {
        let mut q = self.db.prepare_cached("INSERT INTO native_rank_bindings VALUES(?1,?2) ON CONFLICT(job,identity) DO NOTHING").map_err(io::Error::other)?;
        q.reset_status(rusqlite::StatementStatus::VmStep);
        let changed = q
            .execute(params![self.job, id.identity()])
            .map_err(io::Error::other)?;
        self.work.sql_writes += 1;
        self.work.vm_steps += i64::from(q.get_status(rusqlite::StatementStatus::VmStep));
        drop(q);
        if changed != 0 {
            self.references(&id.identity(), 1)?;
        }
        Ok(())
    }
    fn memo(&mut self, version: &str, pair: &Pair, result: &NodeId) -> io::Result<()> {
        self.put(&memo_identity(version, pair)?, &encoded(result)?)
    }
    fn advance(
        &mut self,
        state: &mut Apply,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
    ) -> io::Result<()> {
        self.advance_limited(state, combine, STEP_ROWS, STEP_BYTES)
    }

    fn advance_limited(
        &mut self,
        state: &mut Apply,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        row_limit: usize,
        byte_limit: usize,
    ) -> io::Result<()> {
        if !state.valid() {
            return Err(failure("invalid native rank Apply state"));
        }
        // Reserve node insertion, its two child references, root binding and
        // memo plus the final scalar cursor row. Work counts attempted writes
        // too; existing immutable nodes do not make those statements free.
        while !state.frames.is_empty()
            && self.work.rows() + 12 <= row_limit
            && self.work.metadata_bytes + 12 * NODE_BYTES <= byte_limit
        {
            self.work.max_stack = self.work.max_stack.max(state.frames.len());
            let index = state.frames.len() - 1;
            let frame = state.frames[index].clone();
            if let Some(result) = state.returned.take() {
                match frame.phase {
                    Phase::Absent { source, present } => {
                        state.frames[index].phase = Phase::Present {
                            source,
                            absent: result,
                        };
                        state.frames.push(Frame {
                            pair: present,
                            phase: Phase::Visit,
                        });
                    }
                    Phase::Present { source, absent } => {
                        let result = self.intern(Node::Decision {
                            source,
                            absent,
                            present: result,
                        })?;
                        self.memo(&state.version, &frame.pair, &result)?;
                        state.frames.pop();
                        state.returned = Some(result);
                    }
                    Phase::Visit => {
                        return Err(failure("native rank Apply cursor is inconsistent"));
                    }
                }
                continue;
            }
            if !matches!(frame.phase, Phase::Visit) {
                return Err(failure("native rank Apply child result is absent"));
            }
            self.work.apply_visits += 1;
            // The producer supplies its native ordered-monoid identity. This
            // is algebra, not a guessed zero-count or visibility predicate.
            if frame.pair.left == state.identity || frame.pair.right == state.identity {
                let result = if frame.pair.left == state.identity {
                    frame.pair.right
                } else {
                    frame.pair.left
                };
                self.bind(&result)?;
                state.frames.pop();
                state.returned = Some(result);
                continue;
            }
            if let Some(text) = self.get(&memo_identity(&state.version, &frame.pair)?)? {
                let result: NodeId = serde_json::from_str(&text).map_err(io::Error::other)?;
                if !result.valid() {
                    return Err(failure("invalid native rank memo address"));
                }
                self.bind(&result)?;
                self.work.memo_hits += 1;
                state.frames.pop();
                state.returned = Some(result);
                continue;
            }
            let left = self.node(&frame.pair.left)?;
            let right = self.node(&frame.pair.right)?;
            if let (Node::Terminal { facts: a }, Node::Terminal { facts: b }) = (&left, &right) {
                self.work.max_live_node_bytes =
                    self.work.max_live_node_bytes.max(a.len() + b.len());
                let result = self.intern(Node::Terminal {
                    facts: combine(a, b)?,
                })?;
                self.memo(&state.version, &frame.pair, &result)?;
                state.frames.pop();
                state.returned = Some(result);
            } else {
                let source = left.level().min(right.level());
                let (la, lp) = left.children(source, &frame.pair.left);
                let (ra, rp) = right.children(source, &frame.pair.right);
                state.frames[index].phase = Phase::Absent {
                    source,
                    present: Pair {
                        left: lp,
                        right: rp,
                    },
                };
                state.frames.push(Frame {
                    pair: Pair {
                        left: la,
                        right: ra,
                    },
                    phase: Phase::Visit,
                });
                if state.frames.len() > usize::from(VARIABLES) + 1 {
                    return Err(failure("native rank Apply exceeds source decision depth"));
                }
            }
        }
        Ok(())
    }
    fn evaluate(&mut self, root: &NodeId, included: u64) -> io::Result<String> {
        let mut id = root.clone();
        let mut previous = None;
        for _ in 0..=VARIABLES {
            if previous.is_some_and(|source| id.level <= source) {
                return Err(failure("native rank decision cycle"));
            }
            match self.node(&id)? {
                Node::Terminal { facts } => return Ok(facts),
                Node::Decision {
                    source,
                    absent,
                    present,
                } => {
                    previous = Some(source);
                    id = if included & (1u64 << source) == 0 {
                        absent
                    } else {
                        present
                    };
                }
            }
        }
        Err(failure("native rank decision depth exceeded"))
    }
}

impl NativeStore {
    /// Import an immutable predecessor root under the producer's new, complete
    /// source-prefix/relationship qualification. One root reference retains
    /// its child graph; no whole-history graph cloning is necessary.
    pub(crate) fn rank_bind_root(
        &mut self,
        job: &Job,
        root: &NodeId,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, Option<Work>)> {
        self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut graph = Graph {
                db,
                job: id,
                work: Work::default(),
            };
            graph.node(root)?;
            graph.bind(root)?;
            Ok(graph.work)
        })
    }

    /// The caller owns admitted closure completeness and native guard algebra.
    /// This transaction owns scalar witness checks, cursor/node/memo atomicity,
    /// revision CAS and cancellation. Original custody is never recaptured.
    fn rank_transaction<T>(
        &mut self,
        job: &Job,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        operation: impl FnOnce(&Connection, i64) -> io::Result<T>,
    ) -> io::Result<(Outcome, Option<T>)> {
        self.rank_transaction_body(job, closure, cancelled, operation)
    }

    /// Candidate-tree work reports the whole SQLite instruction delta for its
    /// transaction, including native job/source witnesses, caller closure
    /// queries, revision CAS and COMMIT. `Work::vm_steps` remains the narrower
    /// per-statement counter for the graph/tree statements themselves.
    fn rank_transaction_measured<T>(
        &mut self,
        job: &Job,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        operation: impl FnOnce(&Connection, i64) -> io::Result<T>,
    ) -> io::Result<(Outcome, Option<T>, u64)> {
        let steps = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&steps);
        self.connection.progress_handler(
            1,
            Some(move || {
                observed.fetch_add(1, Ordering::Relaxed);
                false
            }),
        );
        let result = self.rank_transaction_body(job, closure, cancelled, operation);
        self.connection.progress_handler(0, None::<fn() -> bool>);
        result.map(|(outcome, value)| (outcome, value, steps.load(Ordering::Relaxed)))
    }

    fn rank_transaction_body<T>(
        &mut self,
        job: &Job,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        operation: impl FnOnce(&Connection, i64) -> io::Result<T>,
    ) -> io::Result<(Outcome, Option<T>)> {
        if cancelled() {
            return Ok((Outcome::Deferred, None));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok((Outcome::Conflict, None));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if matches!(job.phase.as_str(), "cleanup" | "ready")
            || !native_query::job_current(&tx, job).map_err(io::Error::other)?
            || !closure(&tx, job.id).map_err(io::Error::other)?
        {
            return Ok((Outcome::Conflict, None));
        }
        let result = operation(&tx, job.id)?;
        if cancelled() {
            return Ok((Outcome::Deferred, None));
        }
        if crate::native_evidence::query_runtime_generation(&self.options)? != job.spec.runtime
            || !native_query::job_current(&tx, job).map_err(io::Error::other)?
            || !closure(&tx, job.id).map_err(io::Error::other)?
        {
            return Ok((Outcome::Conflict, None));
        }
        let changed = tx
            .execute(
                "UPDATE native_query_jobs SET revision=revision+1 WHERE job=?1 AND revision=?2",
                params![job.id, job.revision],
            )
            .map_err(io::Error::other)?;
        if changed != 1 {
            return Ok((Outcome::Conflict, None));
        }
        if cancelled() {
            return Ok((Outcome::Deferred, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok((Outcome::Written, Some(result)))
    }

    /// Admit one native terminal and its already model-derived guard. A guard
    /// is source-bit ordered, at most64 decisions. Bits are not resolved from
    /// mutable mtimes until a request evaluates the qualified root.
    pub(crate) fn rank_guard(
        &mut self,
        job: &Job,
        facts: &str,
        empty: &str,
        guard: &[(u8, bool)],
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<(Outcome, Option<(NodeId, Work)>)> {
        if !valid_guard(guard) {
            return Err(failure("native rank guard is not source ordered"));
        }
        // The paged API below admits the full64-source guard. This convenience
        // method is for callers whose native context fits one write group.
        if guard.len() * 5 + 6 > STEP_ROWS {
            return Err(failure("native rank guard requires paged admission"));
        }
        self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut graph = Graph {
                db,
                job: id,
                work: Work::default(),
            };
            let none = graph.intern(Node::Terminal {
                facts: empty.into(),
            })?;
            let mut result = graph.intern(Node::Terminal {
                facts: facts.into(),
            })?;
            for (source, included) in guard.iter().rev() {
                let (absent, present) = if *included {
                    (none.clone(), result)
                } else {
                    (result, none.clone())
                };
                result = graph.intern(Node::Decision {
                    source: *source,
                    absent,
                    present,
                })?;
            }
            if graph.work.metadata_bytes > STEP_BYTES {
                return Err(failure("native rank guard metadata budget exceeded"));
            }
            Ok((result, graph.work))
        })
    }

    pub(crate) fn rank_begin_guard(
        &mut self,
        job: &Job,
        input: GuardInput<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Outcome> {
        if !valid_guard(input.decisions) {
            return Err(failure("native rank guard is not source ordered"));
        }
        let identity = cursor_identity(input.cursor)?;
        let (outcome, _) = self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut graph = Graph {
                db,
                job: id,
                work: Work::default(),
            };
            if graph.get(&identity)?.is_some() {
                return Err(failure("native rank guard cursor already exists"));
            }
            let empty = graph.intern(Node::Terminal {
                facts: input.empty.into(),
            })?;
            let result = graph.intern(Node::Terminal {
                facts: input.facts.into(),
            })?;
            let state = Guard {
                decisions: input.decisions.into(),
                empty,
                result,
                remaining: input.decisions.len(),
            };
            graph.put(&identity, &encoded(&state)?)
        })?;
        Ok(outcome)
    }

    pub(crate) fn rank_advance_guard(
        &mut self,
        job: &Job,
        cursor: &str,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Advance> {
        let identity = cursor_identity(cursor)?;
        let (outcome, result) = self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut graph = Graph {
                db,
                job: id,
                work: Work::default(),
            };
            let text = graph
                .get(&identity)?
                .ok_or_else(|| failure("native rank guard cursor is absent"))?;
            let mut state: Guard = serde_json::from_str(&text).map_err(io::Error::other)?;
            if !valid_guard(&state.decisions)
                || state.remaining > state.decisions.len()
                || !state.empty.valid()
                || !state.result.valid()
            {
                return Err(failure("native rank guard cursor is invalid"));
            }
            while state.remaining > 0
                && graph.work.rows() + 6 <= STEP_ROWS
                && graph.work.metadata_bytes + 2 * NODE_BYTES <= STEP_BYTES
            {
                let (source, included) = state.decisions[state.remaining - 1];
                let (absent, present) = if included {
                    (state.empty.clone(), state.result)
                } else {
                    (state.result, state.empty.clone())
                };
                state.result = graph.intern(Node::Decision {
                    source,
                    absent,
                    present,
                })?;
                state.remaining -= 1;
            }
            graph.put(&identity, &encoded(&state)?)?;
            let complete = (state.remaining == 0).then_some(state.result);
            Ok((complete, graph.work))
        })?;
        let (complete, work) = result.unwrap_or_default();
        Ok(Advance {
            outcome,
            complete,
            work,
        })
    }

    pub(crate) fn rank_begin_apply(
        &mut self,
        job: &Job,
        input: ApplyInput<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Outcome> {
        let state = Apply {
            version: input.version.into(),
            identity: input.identity.clone(),
            frames: vec![Frame {
                pair: Pair {
                    left: input.left.clone(),
                    right: input.right.clone(),
                },
                phase: Phase::Visit,
            }],
            returned: None,
        };
        if !state.valid() {
            return Err(failure("invalid native rank Apply inputs"));
        }
        let identity = cursor_identity(input.cursor)?;
        let (outcome, _) = self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut graph = Graph {
                db,
                job: id,
                work: Work::default(),
            };
            if graph.get(&identity)?.is_some() {
                return Err(failure("native rank Apply cursor already exists"));
            }
            // Import predecessor inputs in the same transaction as the
            // cursor. Their former publication can retire before the next
            // step; persisting IDs alone would not retain their node graph.
            for root in [input.left, input.right, input.identity] {
                graph.node(root)?;
                graph.bind(root)?;
            }
            graph.put(&identity, &encoded(&state)?)
        })?;
        Ok(outcome)
    }

    pub(crate) fn rank_advance_apply(
        &mut self,
        job: &Job,
        cursor: &str,
        combine: &dyn Fn(&str, &str) -> io::Result<String>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Advance> {
        let identity = cursor_identity(cursor)?;
        let (outcome, result) = self.rank_transaction(job, closure, cancelled, |db, id| {
            let mut graph = Graph {
                db,
                job: id,
                work: Work::default(),
            };
            let text = graph
                .get(&identity)?
                .ok_or_else(|| failure("native rank Apply cursor is absent"))?;
            let mut state: Apply = serde_json::from_str(&text).map_err(io::Error::other)?;
            graph.advance(&mut state, combine)?;
            graph.put(&identity, &encoded(&state)?)?;
            let complete = if state.frames.is_empty() {
                state.returned
            } else {
                None
            };
            Ok((complete, graph.work))
        })?;
        let (complete, work) = result.unwrap_or_default();
        Ok(Advance {
            outcome,
            complete,
            work,
        })
    }

    /// A qualified reader evaluates only point-addressed nodes. It rechecks
    /// every source/owner/runtime and the full closure after these metadata
    /// reads; provider/body I/O must receive the caller's additional recheck.
    pub(crate) fn rank_evaluate(
        &self,
        job: &Job,
        root: &NodeId,
        included: u64,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    ) -> io::Result<Option<(String, Work)>> {
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
        if !tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_rank_bindings WHERE job=?1 AND identity=?2)",
                params![job.id, root.identity()],
                |r| r.get::<_, bool>(0),
            )
            .map_err(io::Error::other)?
        {
            return Ok(None);
        }
        let mut graph = Graph {
            db: &tx,
            job: job.id,
            work: Work::default(),
        };
        let facts = graph.evaluate(root, included)?;
        if !native_query::job_current(&tx, job).map_err(io::Error::other)?
            || !closure(&tx, job.id).map_err(io::Error::other)?
            || !self.runtime_current(&job.spec)?
        {
            return Ok(None);
        }
        Ok(Some((facts, graph.work)))
    }
}
