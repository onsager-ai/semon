//! Durable relationship work in the Event Index. Queues hold content-free
//! metadata; each worker still observes at most the existing 64 native sources.
use super::*;
use crate::model::relation_stage::{
    Batch, Context, Endpoint, Phase, Relation, Selection, Selector, Withdrawal,
};
use std::collections::{BTreeSet, HashMap};

// Receipt keys retain ordinary relay order through bounded staged replay.
// Other stored recipes are unsupported under the current producer contract.
pub(super) const RECIPE: &str = "recorded-origin-turn-native-receipt-order-v5";
pub(super) fn recipe_current(c: &Connection) -> rusqlite::Result<bool> {
    let stored: Option<String> = c
        .query_row(
            native_metadata_value(c)?,
            ["catalog_semantic_recipe"],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored.as_deref() == Some(RECIPE))
}
pub(super) fn record_recipe(c: &Connection) -> rusqlite::Result<()> {
    c.execute(
        native_metadata_put(c)?,
        params!["catalog_semantic_recipe", RECIPE],
    )?;
    Ok(())
}

pub(super) const SCHEMA: &str = "
CREATE TABLE catalog_semantic_stage (
 namespace TEXT NOT NULL, singleton INTEGER NOT NULL CHECK(singleton=1), epoch INTEGER NOT NULL,
 phase TEXT NOT NULL CHECK(phase IN ('indexing','identity','relations','publication')),
 generation TEXT, runtime TEXT NOT NULL, revision INTEGER NOT NULL, resetting INTEGER NOT NULL DEFAULT 0, reset_after TEXT NOT NULL DEFAULT '', key_after_harness TEXT NOT NULL DEFAULT '', key_after_native TEXT NOT NULL DEFAULT '', keys_done INTEGER NOT NULL DEFAULT 0, key_candidate TEXT NOT NULL DEFAULT '', key_epoch INTEGER NOT NULL DEFAULT 0, withdrawal_authority TEXT,
 PRIMARY KEY(namespace,singleton)
) STRICT, WITHOUT ROWID;
CREATE TABLE catalog_semantic_frontier (
 namespace TEXT NOT NULL, kind TEXT NOT NULL,value TEXT NOT NULL,harness TEXT NOT NULL,after_key TEXT NOT NULL DEFAULT '',after_edge TEXT NOT NULL DEFAULT '',done INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace,kind,value,harness)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_semantic_frontier_pending ON catalog_semantic_frontier(namespace,done,kind,value,harness);
CREATE INDEX semantic_catalog_parent_key ON session_catalog(namespace,parent_key,session_key) WHERE lifecycle='current';
CREATE INDEX semantic_catalog_native_paths ON session_catalog_sources(namespace,harness,native_id,source_path,session_key) WHERE lifecycle='current';
CREATE TABLE catalog_semantic_sources (
 namespace TEXT NOT NULL, source_path TEXT NOT NULL, claim INTEGER, ledger TEXT, withdrawal TEXT,
 index_done INTEGER NOT NULL DEFAULT 0, event_after INTEGER NOT NULL DEFAULT -1, events_done INTEGER NOT NULL DEFAULT 0, identity_done INTEGER NOT NULL DEFAULT 0, relation_done INTEGER NOT NULL DEFAULT 0, publication_done INTEGER NOT NULL DEFAULT 0, ack_done INTEGER NOT NULL DEFAULT 0, relation_after TEXT NOT NULL DEFAULT '',
 dev INTEGER, ino INTEGER, size INTEGER, mtime_ns BLOB, ctime_ns BLOB, resume_at INTEGER, head_sha256 BLOB, tail_sha256 BLOB,
 PRIMARY KEY(namespace,source_path)
) STRICT, WITHOUT ROWID;
CREATE TABLE catalog_semantic_inputs(namespace TEXT NOT NULL,source_path TEXT NOT NULL,harness TEXT NOT NULL,native_id TEXT NOT NULL,is_top INTEGER NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,source_path)) STRICT, WITHOUT ROWID;
CREATE TABLE catalog_semantic_keys(namespace TEXT NOT NULL,harness TEXT NOT NULL,native_id TEXT NOT NULL,session_key TEXT NOT NULL,key_epoch INTEGER NOT NULL,PRIMARY KEY(namespace,harness,native_id)) STRICT, WITHOUT ROWID;
CREATE INDEX semantic_input_collision ON catalog_semantic_inputs(namespace,native_id,harness);
CREATE INDEX semantic_copilot_pending_id ON session_catalog_invalidations(namespace,substr(substr(source_path,1,length(source_path)-13),length(rtrim(substr(source_path,1,length(source_path)-13),replace(substr(source_path,1,length(source_path)-13),'/','')))+1),source_path) WHERE source_path GLOB '*/session-state/*/events.jsonl';
CREATE INDEX catalog_semantic_input_native ON catalog_semantic_inputs(namespace,harness,native_id,is_top,source_path);
CREATE INDEX catalog_semantic_index ON catalog_semantic_sources(namespace,index_done,source_path);
CREATE INDEX catalog_semantic_identity ON catalog_semantic_sources(namespace,identity_done,source_path);
CREATE INDEX catalog_semantic_relation ON catalog_semantic_sources(namespace,relation_done,source_path);
CREATE TABLE catalog_semantic_endpoints (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, harness TEXT NOT NULL, parent_native TEXT, parent_native_harness TEXT,
 parent_key TEXT, root_key TEXT, task TEXT, nickname TEXT, cost REAL,
 metadata TEXT NOT NULL, published INTEGER NOT NULL DEFAULT 0, parent_resolved INTEGER NOT NULL DEFAULT 0,
 children_pending INTEGER NOT NULL DEFAULT 0, family_cost REAL, cost_done INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace,session_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_semantic_parent_reduce ON catalog_semantic_endpoints(namespace,parent_resolved,parent_native,session_key);
CREATE INDEX catalog_semantic_root_reduce ON catalog_semantic_endpoints(namespace,root_key,session_key);
CREATE INDEX catalog_semantic_cost_reduce ON catalog_semantic_endpoints(namespace,cost_done,children_pending,session_key);
CREATE INDEX catalog_semantic_parent ON catalog_semantic_endpoints(namespace,harness,parent_native,task,session_key);
CREATE INDEX catalog_semantic_parent_key ON catalog_semantic_endpoints(namespace,parent_key,session_key);
CREATE INDEX catalog_semantic_root_name ON catalog_semantic_endpoints(namespace,root_key,nickname,session_key);
CREATE TABLE catalog_semantic_native (
 namespace TEXT NOT NULL,harness TEXT NOT NULL,native_id TEXT NOT NULL,session_key TEXT NOT NULL,
 PRIMARY KEY(namespace,harness,native_id,session_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_semantic_native_owner ON catalog_semantic_native(namespace,session_key,harness,native_id);
CREATE TABLE catalog_semantic_owner_sources (
 namespace TEXT NOT NULL,session_key TEXT NOT NULL,source_path TEXT NOT NULL,
 PRIMARY KEY(namespace,session_key,source_path)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_semantic_source_owner ON catalog_semantic_owner_sources(namespace,source_path,session_key);
CREATE TABLE catalog_semantic_paths (
 namespace TEXT NOT NULL,root_key TEXT,path TEXT NOT NULL,session_key TEXT NOT NULL,
 PRIMARY KEY(namespace,path,session_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_semantic_root_path ON catalog_semantic_paths(namespace,root_key,path,session_key);
CREATE INDEX semantic_pending_filename ON session_catalog_invalidations(namespace,substr(source_path,length(rtrim(source_path,replace(source_path,'/','')))+1),source_path);
CREATE TABLE catalog_semantic_names(namespace TEXT NOT NULL,name TEXT NOT NULL,session_key TEXT NOT NULL,PRIMARY KEY(namespace,name,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE catalog_semantic_events(
 namespace TEXT NOT NULL,source_path TEXT NOT NULL,event_key TEXT NOT NULL,owner TEXT NOT NULL,kind TEXT NOT NULL,msg_id TEXT,tool_id TEXT,name TEXT,metadata TEXT NOT NULL,native_seq INTEGER NOT NULL,copy_key TEXT,copied INTEGER NOT NULL DEFAULT 0,qualified INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace,source_path,event_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_semantic_event_qualification ON catalog_semantic_events(namespace,qualified,source_path,event_key);
CREATE INDEX catalog_semantic_message ON catalog_semantic_events(namespace,msg_id,kind,copied,qualified,source_path,native_seq);
CREATE INDEX catalog_semantic_call ON catalog_semantic_events(namespace,tool_id,kind,source_path,event_key);
CREATE TABLE catalog_semantic_relations (
 namespace TEXT NOT NULL,relation_id TEXT NOT NULL,from_key TEXT NOT NULL,to_key TEXT,metadata TEXT NOT NULL,
 PRIMARY KEY(namespace,relation_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_semantic_relation_from ON catalog_semantic_relations(namespace,from_key,relation_id);
CREATE INDEX catalog_semantic_relation_to ON catalog_semantic_relations(namespace,to_key,relation_id);
";

pub(super) fn schema() -> &'static str {
    SCHEMA
}

pub(super) fn support_schema() -> &'static str {
    STAGE_SUPPORT_SCHEMA
}

macro_rules! semantic_support_objects {
    () => { "CREATE TABLE catalog_relocation_candidates(namespace TEXT NOT NULL,old_path TEXT NOT NULL,new_path TEXT NOT NULL,native_id TEXT NOT NULL,PRIMARY KEY(namespace,old_path,new_path)) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_relocation_new ON catalog_relocation_candidates(namespace,new_path,old_path);
CREATE TABLE catalog_native_dependencies(namespace TEXT NOT NULL,source_path TEXT NOT NULL,harness TEXT NOT NULL,native_id TEXT NOT NULL,PRIMARY KEY(namespace,source_path,harness,native_id)) STRICT, WITHOUT ROWID;
CREATE INDEX catalog_native_dependency ON catalog_native_dependencies(namespace,harness,native_id,source_path);
CREATE TABLE session_source_mutations(namespace TEXT NOT NULL,source_path TEXT NOT NULL,evidence TEXT NOT NULL,PRIMARY KEY(namespace,source_path)) STRICT, WITHOUT ROWID;
CREATE TABLE session_descriptions(namespace TEXT NOT NULL,session_key TEXT NOT NULL,version INTEGER NOT NULL,inputs TEXT NOT NULL,description TEXT NOT NULL,PRIMARY KEY(namespace,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE reported_runs(namespace TEXT NOT NULL,session_id TEXT NOT NULL,start INTEGER NOT NULL,cost REAL,duration INTEGER,api_duration INTEGER,tool_duration INTEGER,lines_added INTEGER,lines_removed INTEGER,model_usage TEXT NOT NULL,capture_at INTEGER NOT NULL,PRIMARY KEY(namespace,session_id,start)) STRICT, WITHOUT ROWID;
" };
}
const STAGE_SUPPORT_SCHEMA: &str = concat!("
CREATE TABLE session_catalog_invalidations(namespace TEXT NOT NULL,source_path TEXT NOT NULL,revision INTEGER NOT NULL,reason TEXT NOT NULL CHECK(reason IN ('changed','removed')),observed_at INTEGER NOT NULL,PRIMARY KEY(namespace,source_path)) STRICT, WITHOUT ROWID;
CREATE INDEX session_catalog_invalidations_revision ON session_catalog_invalidations(namespace,revision,source_path);
CREATE TABLE catalog_backfill_sources(namespace TEXT NOT NULL,source_path TEXT NOT NULL,PRIMARY KEY(namespace,source_path)) STRICT, WITHOUT ROWID;
", semantic_support_objects!());

pub(super) const STAGE_TABLES: &[&str] = &[
    "catalog_semantic_stage",
    "catalog_semantic_frontier",
    "catalog_semantic_sources",
    "catalog_semantic_inputs",
    "catalog_semantic_keys",
    "catalog_semantic_endpoints",
    "catalog_semantic_native",
    "catalog_semantic_owner_sources",
    "catalog_semantic_paths",
    "catalog_semantic_names",
    "catalog_semantic_events",
    "catalog_semantic_relations",
];

type Columns = Vec<(String, String, i64, Option<String>, i64, i64)>;

fn columns(c: &Connection, table: &str) -> rusqlite::Result<Columns> {
    c.prepare_cached("SELECT name,type,[notnull],dflt_value,pk,hidden FROM pragma_table_xinfo(?1,'main') ORDER BY cid")?
        .query_map([table], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?
        .collect()
}

#[derive(PartialEq)]
struct StageShape {
    columns: Columns,
    definition: String,
    indexes: Vec<(String, String)>,
}

fn stage_shape(c: &Connection, table: &str) -> rusqlite::Result<StageShape> {
    Ok(StageShape {
        columns: columns(c, table)?,
        definition: c.query_row("SELECT sql FROM main.sqlite_schema WHERE type='table' AND name=?1", [table], |r| r.get(0))?,
        indexes: c.prepare_cached("SELECT name,sql FROM main.sqlite_schema WHERE type='index' AND tbl_name=?1 AND sql IS NOT NULL ORDER BY name")?
            .query_map([table], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?,
    })
}

// Compare with the actual authored DDL, including defaults, CHECK constraints,
// STRICT/WITHOUT ROWID and expression/partial indexes. These reference databases
// contain schema only; they never receive source data or act as a store facade.
fn stage_shapes() -> rusqlite::Result<&'static Vec<StageShape>> {
    static CURRENT: OnceLock<Vec<StageShape>> = OnceLock::new();
    let cache = &CURRENT;
    if let Some(shapes) = cache.get() {
        return Ok(shapes);
    }
    let reference = Connection::open_in_memory()?;
    reference.execute_batch("CREATE TABLE session_catalog(namespace TEXT,parent_key TEXT,session_key TEXT,lifecycle TEXT);
CREATE TABLE session_catalog_sources(namespace TEXT,harness TEXT,native_id TEXT,source_path TEXT,session_key TEXT,lifecycle TEXT);
CREATE TABLE session_catalog_invalidations(namespace TEXT,source_path TEXT);")?;
    reference.execute_batch(schema())?;
    let shapes = STAGE_TABLES
        .iter()
        .map(|table| stage_shape(&reference, table))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let _ = cache.set(shapes);
    cache.get().ok_or(rusqlite::Error::InvalidQuery)
}

pub(super) fn stage_layout(c: &Connection) -> rusqlite::Result<()> {
    for (table, expected) in STAGE_TABLES.iter().zip(stage_shapes()?) {
        if stage_shape(c, table)? != *expected {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

fn observation_shapes(c: &Connection) -> rusqlite::Result<()> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(support_schema())?;
    for table in ["session_catalog_invalidations", "catalog_backfill_sources"] {
        let physical: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)",
            [table],
            |r| r.get(0),
        )?;
        if !physical || columns(c, table)? != columns(&reference, table)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

pub(crate) fn validate_support(c: &Connection) -> rusqlite::Result<()> {
    let version: i64 = c.query_row("PRAGMA main.schema_version", [], |r| r.get(0))?;
    let cached: bool = c.prepare_cached("SELECT EXISTS(SELECT 1 FROM pragma_function_list WHERE name='semon_semantic_support_layout' AND narg=1)")?.query_row([], |r| r.get(0))?;
    if cached {
        let layout: Option<bool> = c
            .prepare_cached("SELECT semon_semantic_support_layout(?1)")?
            .query_row([version], |r| r.get(0))?;
        if layout == Some(true) {
            return Ok(());
        }
    }
    observation_shapes(c)?;
    stage_layout(c)?;
    for (table, key) in [
        ("catalog_relocation_candidates", "old_path,new_path"),
        (
            "catalog_native_dependencies",
            "source_path,harness,native_id",
        ),
        ("session_source_mutations", "source_path"),
        ("reported_runs", "session_id,start"),
    ] {
        let pk: String = c.prepare_cached("SELECT coalesce(group_concat(name,','),'') FROM (SELECT name FROM pragma_table_info(?1,'main') WHERE pk>0 ORDER BY pk)")?.query_row([table], |r| r.get(0))?;
        let ns = pk == format!("namespace,{key}");
        let namespace_valid: bool = c.prepare_cached("SELECT count(*)=1 AND min(pk=1 AND [notnull]=1 AND type='TEXT') FROM pragma_table_info(?1,'main') WHERE name='namespace'")?.query_row([table], |r| r.get(0))?;
        let physical: bool = c
            .prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)",
            )?
            .query_row([table], |r| r.get(0))?;
        if !ns || !namespace_valid || !physical {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(support_schema())?;
    for table in [
        "catalog_relocation_candidates",
        "catalog_native_dependencies",
        "session_source_mutations",
        "reported_runs",
    ] {
        if columns(c, table)? != columns(&reference, table)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    for (table, name, columns) in [
        (
            "catalog_relocation_candidates",
            "catalog_relocation_new",
            "new_path,old_path",
        ),
        (
            "catalog_native_dependencies",
            "catalog_native_dependency",
            "harness,native_id,source_path",
        ),
    ] {
        let actual: String = c.prepare_cached("SELECT coalesce(group_concat(name,','),'') FROM (SELECT name FROM pragma_index_info(?1,'main') ORDER BY seqno)")?.query_row([name], |r| r.get(0))?;
        let owned: bool = c.prepare_cached("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='index' AND name=?1 AND tbl_name=?2) AND EXISTS(SELECT 1 FROM pragma_index_list(?2,'main') WHERE name=?1 AND [unique]=0 AND partial=0)")?.query_row(params![name,table], |r| r.get(0))?;
        if actual != format!("namespace,{columns}") || !owned {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    // Cache only committed main-schema decisions. A rolled-back DDL cookie can
    // be reused; never pin its transient layout or any Source binding.
    if c.is_autocommit() {
        c.create_scalar_function(
            "semon_semantic_support_layout",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
            move |context| {
                let observed: i64 = context.get(0)?;
                Ok((observed == version).then_some(true))
            },
        )?;
    }
    Ok(())
}
pub(super) fn metadata_indexes() -> &'static str {
    "CREATE INDEX IF NOT EXISTS semantic_codex_header_native ON meta(namespace,json_extract(value,'$.header.id'),key)
 WHERE key GLOB 'native_codex_header:*' AND json_valid(value);
CREATE INDEX IF NOT EXISTS semantic_codex_header_parent ON meta(namespace,json_extract(value,'$.header.metadata.parent_thread'),key)
 WHERE key GLOB 'native_codex_header:*' AND json_valid(value);"
}
// Keep physical core indexes separate from the semantic stage's own objects.
// Both cold initialization and rebuilds assemble these authored DDL chunks.
pub(super) const SEMANTIC_CORE_INDEXES_NS: &str = "
CREATE INDEX IF NOT EXISTS semantic_copilot_source_id ON files(namespace,substr(substr(path,1,length(path)-13),length(rtrim(substr(path,1,length(path)-13),replace(substr(path,1,length(path)-13),'/','')))+1),path) WHERE path GLOB '*/session-state/*/events.jsonl';
CREATE INDEX IF NOT EXISTS semantic_source_filename ON files(namespace,substr(path,length(rtrim(path,replace(path,'/','')))+1),path);
CREATE INDEX IF NOT EXISTS semantic_index_receipt ON events(namespace,id,file_id,o,b,seq) WHERE k='xsm';
CREATE INDEX IF NOT EXISTS semantic_index_send ON events(namespace,reply_m,file_id,o,b,seq) WHERE k='tool' AND n='SendMessage';
CREATE INDEX IF NOT EXISTS semantic_index_copy ON events(namespace,file_id,COALESCE(u,CASE WHEN k IN ('tool','xsm') THEN id END));
";
const MAX_CONTEXT: usize = 1024 * 1024;

pub(super) fn active(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_stage WHERE namespace=semon_namespace())",
        [],
        |r| r.get(0),
    )
}
/// An incomplete Current header stays visible until its replacement publishes.
/// Admit its repair once, preserving an existing observation or the exact live
/// stage observation even after ACK. Unreachable GC records own no work.
pub(super) fn owns_header_repair(c: &Connection, path: &str) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1)
         OR EXISTS(SELECT 1 FROM main.catalog_semantic_sources s
         JOIN main.catalog_semantic_stage stage ON stage.namespace=s.namespace AND stage.singleton=1 AND stage.resetting=0
         LEFT JOIN main.session_catalog_invalidations i USING(namespace,source_path)
         LEFT JOIN main.files f ON f.namespace=s.namespace AND f.path=s.source_path
         WHERE s.namespace=semon_namespace() AND s.source_path=?1 AND s.index_done=1 AND i.revision IS s.claim
         AND f.dev IS s.dev AND f.ino IS s.ino AND f.size IS s.size
         AND f.mtime_ns IS s.mtime_ns AND f.ctime_ns IS s.ctime_ns
         AND f.resume_at IS s.resume_at AND f.head_sha256 IS s.head_sha256
         AND f.tail_sha256 IS s.tail_sha256)",
        [path],
        |row| row.get(0),
    )
}
fn phase(p: Phase) -> &'static str {
    match p {
        Phase::Indexing => "indexing",
        Phase::Identity => "identity",
        Phase::Relations => "relations",
        Phase::Publication => "publication",
    }
}
fn root_alive(c: &Connection, key: &str) -> rusqlite::Result<bool> {
    c.query_row("SELECT COALESCE(json_extract(metadata,'$.alive'),0) FROM catalog_semantic_endpoints WHERE session_key=?1",[key],|r|r.get(0)).optional().map(|v|v.unwrap_or(false))
}
fn normalized_endpoint(c: &Connection, key: &str, used: &mut usize) -> rusqlite::Result<Endpoint> {
    crate::model::relation_stage::selector_endpoint(
        &OrdinaryLookup {
            db: c,
            replay: Vec::new(),
        },
        key,
        used,
        false,
    )
}
fn native_key(c: &Connection, native: &str, harness: &str) -> rusqlite::Result<String> {
    let ready: bool = c.query_row(
        "SELECT keys_done=1 AND resetting=0 FROM catalog_semantic_stage",
        [],
        |r| r.get(0),
    )?;
    if !ready {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let collision:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_inputs WHERE native_id=?1 AND harness<>?2 UNION ALL SELECT 1 FROM session_catalog_sources n JOIN session_catalog own USING(session_key) WHERE n.native_id=?1 AND n.harness<>?2 AND own.lifecycle='current' AND n.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources excluded WHERE excluded.source_path=n.source_path AND excluded.index_done=1))",params![native,harness],|r|r.get(0))?;
    if !collision {
        return Ok(primitive_native_key(native).to_owned());
    }
    // Prefix occupancy is qualified only by the durable bounded cursor. A
    // selector consumes its result; it never replays an arbitrarily long chain.
    c.query_row("SELECT keys.session_key FROM catalog_semantic_keys keys JOIN catalog_semantic_stage stage ON keys.key_epoch=stage.key_epoch WHERE keys.harness=?1 AND keys.native_id=?2",params![harness,native],|r|r.get(0))
}
pub(super) fn primitive_native_key(native: &str) -> &str {
    native
}
#[cfg(test)]
std::thread_local! {
    static NATIVE_OCCUPANCY_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn native_occupied(c: &Connection, key: &str) -> rusqlite::Result<bool> {
    #[cfg(test)]
    NATIVE_OCCUPANCY_PROBES.with(|count| count.set(count.get() + 1));
    c.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_inputs WHERE native_id=?1 UNION ALL SELECT 1 FROM session_catalog_sources n JOIN session_catalog own USING(session_key) WHERE native_id=?1 AND own.lifecycle='current' AND n.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources observed WHERE observed.source_path=n.source_path AND observed.index_done=1))",[key],|r|r.get(0))
}
fn claim(c: &Connection, path: &str) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT revision FROM session_catalog_invalidations WHERE source_path=?1",
        [path],
        |r| r.get(0),
    )
    .optional()
}
fn ledger(c: &Connection, path: &str) -> rusqlite::Result<Option<String>> {
    ledger_row(c, path)?
        .map(|(_, ledger)| json(&ledger))
        .transpose()
}
pub(super) fn generation(c: &Connection) -> rusqlite::Result<Option<String>> {
    c.query_row(native_metadata_value(c)?, ["catalog_generation"], |r| {
        r.get(0)
    })
    .optional()
}
pub(super) fn revision(c: &Connection) -> rusqlite::Result<i64> {
    c.query_row(
        "SELECT revision FROM storage_source_journal WHERE namespace=semon_namespace()",
        [],
        |r| r.get(0),
    )
}
fn queue(c: &Connection, kind: &str, value: &str, harness: &str) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO catalog_semantic_frontier(kind,value,harness) VALUES(?1,?2,?3) ON CONFLICT(kind,value,harness) DO UPDATE SET done=0,after_key='',after_edge='' WHERE catalog_semantic_frontier.done=1",
        params![kind, value, harness],
    )?;
    Ok(())
}
fn enqueue(c: &Connection, path: &str) -> rusqlite::Result<()> {
    let added = c.execute(
        "INSERT OR IGNORE INTO catalog_semantic_sources(source_path) VALUES(?1)",
        [path],
    )?;
    if added > 0 {
        let missing = ledger_row(c, path)?.is_none();
        let needs_owner:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources own JOIN session_catalog_sources peer USING(session_key) WHERE own.source_path=?1 AND own.lifecycle='current' AND peer.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=peer.source_path))",[path],|r|r.get(0))?;
        if missing || needs_owner {
            queue(c, "pathowner", path, "")?;
        }
        for (kind, sql) in [
            (
                "moveold",
                "SELECT EXISTS(SELECT 1 FROM catalog_relocation_candidates WHERE old_path=?1)",
            ),
            (
                "movenew",
                "SELECT EXISTS(SELECT 1 FROM catalog_relocation_candidates WHERE new_path=?1)",
            ),
        ] {
            if c.query_row(sql, [path], |r| r.get::<_, bool>(0))? {
                queue(c, kind, path, "")?;
            }
        }
    }
    Ok(())
}
/// Physical lookups only; matching order and frontier decisions are shared.
pub(crate) trait NativeFrontierPort {
    fn needed_native(&self, harness: &str, id: &str) -> rusqlite::Result<bool>;
    fn needed_dependency(&self, harness: &str, id: &str) -> rusqlite::Result<bool>;
    fn needed_nativeall(&self, id: &str) -> rusqlite::Result<bool>;
    fn needed_filename(&self, copilot: bool, value: &str) -> rusqlite::Result<bool>;
    fn needed_header(&self, parent: bool, qualified: bool, id: &str) -> rusqlite::Result<bool>;
    fn queue(&self, kind: &str, value: &str, harness: &str) -> rusqlite::Result<()>;
}
fn filename_hint_with(port: &impl NativeFrontierPort, id: &str) -> rusqlite::Result<bool> {
    let basename = format!("{id}.jsonl");
    let unseen = port.needed_filename(false, &basename)?;
    if unseen {
        port.queue("claudeid", &basename, "")?;
    }
    let copilot = port.needed_filename(true, id)?;
    if copilot {
        port.queue("copilotid", id, "")?;
    }
    Ok(unseen || copilot)
}
fn identity_hint_with(port: &impl NativeFrontierPort, id: &str) -> rusqlite::Result<bool> {
    let mut unseen = filename_hint_with(port, id)?;
    if port.needed_nativeall(id)? {
        port.queue("nativeall", id, "")?;
        unseen = true;
    }
    if port.needed_header(false, false, id)? {
        port.queue("headerid", id, "")?;
        unseen = true;
    }
    Ok(unseen)
}
pub(crate) fn native_frontier_with(
    port: &impl NativeFrontierPort,
    harness: &str,
    id: &str,
) -> rusqlite::Result<()> {
    if port.needed_native(harness, id)? {
        port.queue("native", id, harness)?;
    }
    for prefix in [
        "",
        "lineage:",
        "bridge:",
        "semantic-parent:",
        "semantic-native:",
    ] {
        let dependency = format!("{prefix}{harness}");
        if port.needed_dependency(&dependency, id)? {
            port.queue("dependency", id, &dependency)?;
        }
    }
    if port.needed_nativeall(id)? {
        port.queue("nativeall", id, "")?;
    }
    identity_hint_with(port, id)?;
    if harness == "codex" {
        for (kind, parent) in [("headerid", false), ("headerparent", true)] {
            if port.needed_header(parent, true, id)? {
                port.queue(kind, id, "")?;
            }
        }
    }
    Ok(())
}
struct OrdinaryNativeFrontier<'a>(&'a Connection);
impl NativeFrontierPort for OrdinaryNativeFrontier<'_> {
    fn needed_native(&self, harness: &str, id: &str) -> rusqlite::Result<bool> {
        self.0.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources own WHERE own.harness=?1 AND own.native_id=?2 AND own.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=own.source_path))",params![harness,id],|r|r.get(0))
    }
    fn needed_dependency(&self, harness: &str, id: &str) -> rusqlite::Result<bool> {
        self.0.query_row("SELECT EXISTS(SELECT 1 FROM catalog_native_dependencies d WHERE harness=?1 AND native_id=?2 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=d.source_path))",params![harness,id],|r|r.get(0))
    }
    fn needed_nativeall(&self, id: &str) -> rusqlite::Result<bool> {
        self.0.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources own WHERE native_id=?1 AND lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=own.source_path))",[id],|r|r.get(0))
    }
    fn needed_filename(&self, copilot: bool, value: &str) -> rusqlite::Result<bool> {
        self.0.query_row(if copilot { "SELECT EXISTS(SELECT 1 FROM files f WHERE path GLOB '*/session-state/*/events.jsonl' AND substr(substr(path,1,length(path)-13),length(rtrim(substr(path,1,length(path)-13),replace(substr(path,1,length(path)-13),'/','')))+1)=?1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=f.path) UNION ALL SELECT 1 FROM session_catalog_invalidations i WHERE source_path GLOB '*/session-state/*/events.jsonl' AND substr(substr(source_path,1,length(source_path)-13),length(rtrim(substr(source_path,1,length(source_path)-13),replace(substr(source_path,1,length(source_path)-13),'/','')))+1)=?1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=i.source_path))" } else { "SELECT EXISTS(SELECT 1 FROM files f WHERE substr(path,length(rtrim(path,replace(path,'/','')))+1)=?1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=f.path) UNION ALL SELECT 1 FROM session_catalog_invalidations i WHERE substr(source_path,length(rtrim(source_path,replace(source_path,'/','')))+1)=?1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=i.source_path))" },[value],|r|r.get(0))
    }
    fn needed_header(&self, parent: bool, qualified: bool, id: &str) -> rusqlite::Result<bool> {
        let c = self.0;
        let field = if parent {
            "metadata.parent_thread"
        } else {
            "id"
        };
        if !qualified {
            return c.query_row("SELECT EXISTS(SELECT 1 FROM meta m WHERE m.namespace=semon_namespace() AND key GLOB 'native_codex_header:*' AND json_valid(value) AND json_extract(value,'$.header.id')=?1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=substr(m.key,length('native_codex_header:')+1)))",[id],|r|r.get(0));
        }
        let dependency = if !parent { "native" } else { "parent" };
        let scoped = format!(
            "SELECT EXISTS(SELECT 1 FROM meta m WHERE m.namespace=semon_namespace() AND key GLOB 'native_codex_header:*' AND json_valid(value) AND json_extract(value,'$.header.{field}')=?1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=substr(m.key,length('native_codex_header:')+1)) AND NOT EXISTS(SELECT 1 FROM catalog_native_dependencies d WHERE d.source_path=substr(m.key,length('native_codex_header:')+1) AND d.native_id=?1 AND d.harness='semantic-{dependency}:codex'))"
        );
        c.query_row(&scoped, [id], |r| r.get(0))
    }
    fn queue(&self, kind: &str, value: &str, harness: &str) -> rusqlite::Result<()> {
        queue(self.0, kind, value, harness)
    }
}
/// Only cross-harness collisions need prefixed-key reservation discovery.
/// The cursor is reset whenever new primitive native evidence joins the epoch.
/// Every candidate file is parsed/role-qualified before this pass can finish.
fn native_keys_page(c: &Connection) -> rusqlite::Result<bool> {
    let(harness,native,done,mut candidate):(String,String,bool,String)=c.query_row("SELECT key_after_harness,key_after_native,keys_done,key_candidate FROM catalog_semantic_stage",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if done {
        return Ok(true);
    }
    let mut q=c.prepare("SELECT input.harness,input.native_id FROM catalog_semantic_inputs input WHERE (input.harness,input.native_id)>(?1,?2) AND (EXISTS(SELECT 1 FROM catalog_semantic_inputs peer WHERE peer.native_id=input.native_id AND peer.harness<>input.harness) OR EXISTS(SELECT 1 FROM session_catalog_sources existing JOIN session_catalog owner USING(session_key) WHERE existing.native_id=input.native_id AND existing.harness<>input.harness AND owner.lifecycle='current' AND existing.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources observed WHERE observed.source_path=existing.source_path AND observed.index_done=1))) GROUP BY input.harness,input.native_id ORDER BY input.harness,input.native_id LIMIT 64")?;
    let mut rows = q.query(params![harness, native])?;
    let mut after = (harness, native);
    let mut budget = 64usize;
    let mut used = 0usize;
    let mut complete = true;
    'pairs: while let Some(row) = rows.next()? {
        let harness: String = row.get(0)?;
        let native: String = row.get(1)?;
        if candidate.is_empty() {
            candidate = format!("{harness}:{native}");
        }
        loop {
            if candidate.len() > MAX_CONTEXT {
                return Err(rusqlite::Error::InvalidQuery);
            }
            if budget == 0 || used + candidate.len() > MAX_CONTEXT {
                complete = false;
                break 'pairs;
            }
            budget -= 1;
            used += candidate.len();
            // Qualify the occupant of every intermediate key, rather than
            // letting an old Current row skip a deselected/mutated native ID.
            if identity_hint(c, &candidate)? {
                complete = false;
                break 'pairs;
            }
            if !native_occupied(c, &candidate)? {
                c.execute("INSERT INTO main.catalog_semantic_keys(namespace,harness,native_id,session_key,key_epoch) SELECT semon_namespace(),?1,?2,?3,key_epoch FROM main.catalog_semantic_stage WHERE namespace=semon_namespace() ON CONFLICT(namespace,harness,native_id) DO UPDATE SET session_key=excluded.session_key,key_epoch=excluded.key_epoch",params![harness,native,candidate])?;
                after = (harness, native);
                candidate.clear();
                break;
            }
            candidate.insert(0, ':');
        }
    }
    drop(rows);
    drop(q);
    if budget == 0 {
        complete = false;
    }
    let pending: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE done=0)",
        [],
        |r| r.get(0),
    )?;
    complete &= !pending;
    c.execute("UPDATE catalog_semantic_stage SET key_after_harness=?1,key_after_native=?2,keys_done=?3,key_candidate=?4",params![after.0,after.1,complete,candidate])?;
    Ok(complete)
}
fn identity_hint(c: &Connection, id: &str) -> rusqlite::Result<bool> {
    identity_hint_with(&OrdinaryNativeFrontier(c), id)
}
fn native_frontier(c: &Connection, harness: &str, id: &str) -> rusqlite::Result<()> {
    native_frontier_with(&OrdinaryNativeFrontier(c), harness, id)
}
fn endpoint_frontier(c: &Connection, e: &Endpoint) -> rusqlite::Result<()> {
    for source in &e.row.sources {
        enqueue(c, source.path.to_string_lossy().as_ref())?;
    }
    for id in &e.row.native_ids {
        native_frontier(c, &e.row.harness, id)?;
    }
    if e.row.harness == "claude" {
        for source in &e.row.sources {
            if source.path.file_stem().and_then(|stem| stem.to_str())
                == Some(source.native_id.as_str())
            {
                let prefix = format!(
                    "{}/subagents/",
                    source.path.with_extension("").to_string_lossy()
                );
                let needed:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM files f WHERE path>=?1 AND path<?1||char(1114111) AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources s WHERE s.source_path=f.path))",[&prefix],|r|r.get(0))?;
                if needed {
                    queue(c, "claudechildren", &prefix, "")?;
                }
            }
        }
    }

    // Existing graph endpoints only enter a queue when their current source
    // membership contains a still-unseen physical observation.
    let related:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources s WHERE s.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources m WHERE m.source_path=s.source_path) AND s.session_key IN (SELECT ?1 UNION SELECT parent_key FROM session_catalog WHERE session_key=?1 AND lifecycle='current' UNION SELECT session_key FROM session_catalog WHERE parent_key=?1 AND lifecycle='current' UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM session_trace_edges WHERE session_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM session_trace_edges WHERE session_key=?1 UNION SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?1 UNION SELECT session_key FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?1))",[&e.row.key],|r|r.get(0))?;
    if related {
        for kind in ["members", "parent", "children", "edges", "from", "to"] {
            queue(c, kind, &e.row.key, "")?;
        }
    }
    if let Some(parent) = &e.parent {
        let needed:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources s WHERE session_key=?1 AND lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources m WHERE m.source_path=s.source_path))",[parent],|r|r.get(0))?;
        if needed {
            queue(c, "members", parent, "")?;
        }
    }
    Ok(())
}
/// Frontier pages persist keyset cursors; fanout adds at most 64 metadata
/// observations in one call, with no collection-sized INSERT SELECT mutation.
fn frontier_page(c: &Connection) -> rusqlite::Result<bool> {
    let mut budget = 64usize;
    while budget > 0 {
        let task:Option<(String,String,String,String,String)>=c.query_row("SELECT kind,value,harness,after_key,after_edge FROM catalog_semantic_frontier WHERE done=0 ORDER BY kind,value,harness LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let Some((kind, value, harness, after, edge)) = task else {
            return Ok(true);
        };
        let (sql, owner) = match kind.as_str() {
            "pathowner" => (
                "SELECT session_key,'' FROM session_catalog_sources WHERE source_path=?1 AND lifecycle='current' AND session_key>?2 ORDER BY session_key LIMIT ?3",
                true,
            ),
            "members" => (
                "SELECT source_path,'' FROM session_catalog_sources WHERE session_key=?1 AND lifecycle='current' AND source_path>?2 ORDER BY source_path LIMIT ?3",
                false,
            ),
            "moveold" => (
                "SELECT new_path,'' FROM catalog_relocation_candidates WHERE old_path=?1 AND new_path>?2 ORDER BY new_path LIMIT ?3",
                false,
            ),
            "movenew" => (
                "SELECT old_path,'' FROM catalog_relocation_candidates WHERE new_path=?1 AND old_path>?2 ORDER BY old_path LIMIT ?3",
                false,
            ),
            "native" => (
                "SELECT source_path,'' FROM session_catalog_sources WHERE native_id=?1 AND harness=?4 AND lifecycle='current' AND source_path>?2 GROUP BY source_path ORDER BY source_path LIMIT ?3",
                false,
            ),
            "nativeall" => (
                "SELECT source_path,'' FROM session_catalog_sources WHERE native_id=?1 AND lifecycle='current' AND source_path>?2 GROUP BY source_path ORDER BY source_path LIMIT ?3",
                false,
            ),
            "claudeid" => (
                "SELECT source_path,'' FROM (SELECT path source_path FROM files WHERE substr(path,length(rtrim(path,replace(path,'/','')))+1)=?1 UNION SELECT source_path FROM session_catalog_invalidations WHERE substr(source_path,length(rtrim(source_path,replace(source_path,'/','')))+1)=?1) WHERE source_path>?2 ORDER BY source_path LIMIT ?3",
                false,
            ),
            "copilotid" => (
                "SELECT source_path,'' FROM (SELECT path source_path FROM files WHERE path GLOB '*/session-state/*/events.jsonl' AND substr(substr(path,1,length(path)-13),length(rtrim(substr(path,1,length(path)-13),replace(substr(path,1,length(path)-13),'/','')))+1)=?1 UNION SELECT source_path FROM session_catalog_invalidations WHERE source_path GLOB '*/session-state/*/events.jsonl' AND substr(substr(source_path,1,length(source_path)-13),length(rtrim(substr(source_path,1,length(source_path)-13),replace(substr(source_path,1,length(source_path)-13),'/','')))+1)=?1) WHERE source_path>?2 ORDER BY source_path LIMIT ?3",
                false,
            ),
            "dependency" => (
                "SELECT source_path,'' FROM catalog_native_dependencies WHERE native_id=?1 AND harness=?4 AND source_path>?2 ORDER BY source_path LIMIT ?3",
                false,
            ),
            "headerid" => (
                "SELECT key,'' FROM meta WHERE namespace=semon_namespace() AND key GLOB 'native_codex_header:*' AND json_valid(value) AND json_extract(value,'$.header.id')=?1 AND key>?2 ORDER BY key LIMIT ?3",
                false,
            ),
            "headerparent" => (
                "SELECT key,'' FROM meta WHERE namespace=semon_namespace() AND key GLOB 'native_codex_header:*' AND json_valid(value) AND json_extract(value,'$.header.metadata.parent_thread')=?1 AND key>?2 ORDER BY key LIMIT ?3",
                false,
            ),
            "claudechildren" => (
                "SELECT path,'' FROM files WHERE path>=?1 AND path<?1||char(1114111) AND path>?2 ORDER BY path LIMIT ?3",
                false,
            ),
            "receipt" => (
                "SELECT f.path,e.file_id||':'||e.seq FROM events e JOIN files f USING(file_id) WHERE e.k='xsm' AND e.id=?1 AND f.path>?2 GROUP BY f.path ORDER BY f.path LIMIT ?3",
                false,
            ),
            "sender" => (
                "SELECT f.path,e.file_id||':'||e.seq FROM events e JOIN files f USING(file_id) WHERE e.k='tool' AND e.n='SendMessage' AND e.reply_m=?1 AND f.path>?2 GROUP BY f.path ORDER BY f.path LIMIT ?3",
                false,
            ),
            "parent" => (
                "SELECT parent_key,'' FROM session_catalog WHERE session_key=?1 AND lifecycle='current' AND parent_key>?2 LIMIT ?3",
                true,
            ),
            "children" => (
                "SELECT session_key,'' FROM session_catalog WHERE parent_key=?1 AND lifecycle='current' AND session_key>?2 ORDER BY session_key LIMIT ?3",
                true,
            ),
            "edges" => (
                "SELECT edge_id,metadata FROM session_trace_edges WHERE session_key=?1 AND edge_id>?2 ORDER BY edge_id LIMIT ?3",
                true,
            ),
            "from" => (
                "SELECT session_key,edge_id FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?1 AND (session_key>?2 OR (session_key=?2 AND edge_id>?5)) ORDER BY session_key,edge_id LIMIT ?3",
                true,
            ),
            "to" => (
                "SELECT session_key,edge_id FROM session_trace_edges WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?1 AND (session_key>?2 OR (session_key=?2 AND edge_id>?5)) ORDER BY session_key,edge_id LIMIT ?3",
                true,
            ),
            _ => return Err(rusqlite::Error::InvalidQuery),
        };
        let mut q = c.prepare(sql)?;
        q.raw_bind_parameter(1, &value)?;
        q.raw_bind_parameter(2, &after)?;
        q.raw_bind_parameter(3, budget as i64)?;
        if q.parameter_count() >= 4 {
            q.raw_bind_parameter(4, &harness)?;
        }
        if q.parameter_count() >= 5 {
            q.raw_bind_parameter(5, &edge)?;
        }
        let mut rows = q.raw_query();
        let mut count = 0;
        let mut last = after;
        let mut last_edge = edge;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let auxiliary: String = row.get(1)?;
            if kind == "edges" {
                if auxiliary.len() > MAX_CONTEXT {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let metadata: Value =
                    serde_json::from_str(&auxiliary).map_err(|_| rusqlite::Error::InvalidQuery)?;
                for endpoint in ["from", "to"] {
                    if let Some(endpoint) = metadata
                        .get(endpoint)
                        .and_then(Value::as_str)
                        .filter(|key| *key != "you")
                    {
                        queue(c, "members", endpoint, "")?;
                    }
                }
            } else if owner {
                queue(c, "members", &key, "")?;
                if kind == "pathowner" {
                    for next in ["parent", "children", "edges", "from", "to"] {
                        queue(c, next, &key, "")?;
                    }
                }
            } else {
                enqueue(
                    c,
                    if kind.starts_with("header") {
                        key.strip_prefix("native_codex_header:")
                            .ok_or(rusqlite::Error::InvalidQuery)?
                    } else {
                        &key
                    },
                )?;
            }
            last = key;
            if matches!(kind.as_str(), "from" | "to") {
                last_edge = auxiliary;
            }
            count += 1;
        }
        drop(rows);
        drop(q);
        c.execute("UPDATE catalog_semantic_frontier SET after_key=?4,after_edge=?5,done=?6 WHERE kind=?1 AND value=?2 AND harness=?3",params![kind,value,harness,last,last_edge,count<budget])?;
        budget = budget.saturating_sub(count.max(1));
    }
    let pending: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE done=0)",
        [],
        |r| r.get(0),
    )?;
    Ok(!pending)
}
/// A Claude top-level identity is the exact union of its native continuation,
/// session and bridge links. Semantic parent/edge neighbours are excluded.
fn owner_closure(c: &Connection, seed: &str) -> rusqlite::Result<Vec<String>> {
    let Some((harness, top)) = c
        .query_row(
            "SELECT harness,is_top FROM catalog_semantic_inputs WHERE source_path=?1",
            [seed],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()?
    else {
        return Ok(vec![seed.to_owned()]);
    };
    if harness != "claude" || !top {
        return Ok(vec![seed.to_owned()]);
    }
    let mut paths = BTreeSet::new();
    let mut pending = vec![seed.to_owned()];
    let mut ids = BTreeSet::new();
    let mut bridges = BTreeSet::new();
    let mut used = 0usize;
    while let Some(path) = pending.pop() {
        if !paths.insert(path.clone()) {
            continue;
        }
        if paths.len() > 64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let (id,links):(String,Option<String>)=c.query_row("SELECT s.native_id,CASE WHEN octet_length(f.links)<=1048576 THEN f.links END FROM catalog_semantic_inputs s JOIN files f ON f.path=s.source_path WHERE s.source_path=?1",[&path],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let links: super::super::Links = bounded(links, &mut used)?;
        let mut selectors = Vec::new();
        for id in std::iter::once(id)
            .chain(links.session_ids)
            .chain(links.continued_in)
        {
            if ids.insert(id.clone()) {
                selectors.push(("lineage:claude", id));
            }
        }
        for id in links.bridges {
            if bridges.insert(id.clone()) {
                selectors.push(("bridge:claude", id));
            }
        }
        for (kind, id) in selectors {
            let mut q=c.prepare("SELECT source_path FROM catalog_semantic_inputs WHERE harness='claude' AND is_top=1 AND (native_id=?1 AND ?2='lineage:claude' OR source_path IN (SELECT source_path FROM catalog_native_dependencies WHERE harness=?2 AND native_id=?1)) ORDER BY source_path LIMIT 65")?;
            for found in q.query_map(params![id, kind], |r| r.get::<_, String>(0))? {
                let found = found?;
                if !paths.contains(&found) {
                    pending.push(found);
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}
/// Qualification is a separate bounded Identity sub-phase after reduction.
pub(crate) trait QualificationPort {
    fn pending(&self) -> rusqlite::Result<Vec<(String, String, Option<String>)>>;
    fn copied(&self, path: &str, identity: &str) -> rusqlite::Result<bool>;
    fn owner(&self, path: &str) -> rusqlite::Result<String>;
    fn qualify(&self, path: &str, key: &str, owner: &str, copied: bool) -> rusqlite::Result<()>;
    fn done(&self) -> rusqlite::Result<bool>;
}
pub(crate) fn qualify_with(port: &impl QualificationPort) -> rusqlite::Result<bool> {
    for (path, key, identity) in port.pending()? {
        let copied = if let Some(identity) = identity {
            port.copied(&path, &identity)?
        } else {
            false
        };
        let owner = port.owner(&path)?;
        port.qualify(&path, &key, &owner, copied)?;
    }
    port.done()
}
struct OrdinaryQualification<'a>(&'a Connection);
impl QualificationPort for OrdinaryQualification<'_> {
    fn pending(&self) -> rusqlite::Result<Vec<(String, String, Option<String>)>> {
        self.0.prepare("SELECT source_path,event_key,copy_key FROM catalog_semantic_events WHERE qualified=0 ORDER BY source_path,event_key LIMIT 64")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect()
    }
    fn copied(&self, path: &str, identity: &str) -> rusqlite::Result<bool> {
        self.0.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_inputs target JOIN catalog_semantic_owner_sources own ON own.source_path=target.source_path JOIN catalog_semantic_owner_sources predecessor ON predecessor.session_key=own.session_key AND predecessor.source_path<>target.source_path JOIN files source ON source.path=predecessor.source_path JOIN events event ON event.file_id=source.file_id WHERE target.source_path=?1 AND target.is_top=1 AND event.extra=0 AND COALESCE(event.u,CASE WHEN event.k IN ('tool','xsm') THEN event.id END)=?2 AND EXISTS(SELECT 1 FROM json_each(source.links,'$.continued_in') WHERE value=target.native_id))",params![path,identity],|r|r.get(0))
    }
    fn owner(&self, path: &str) -> rusqlite::Result<String> {
        self.0.query_row(
            "SELECT session_key FROM catalog_semantic_owner_sources WHERE source_path=?1",
            [path],
            |r| r.get(0),
        )
    }
    fn qualify(&self, path: &str, key: &str, owner: &str, copied: bool) -> rusqlite::Result<()> {
        self.0.execute("UPDATE catalog_semantic_events SET owner=?3,copied=?4,qualified=1 WHERE source_path=?1 AND event_key=?2",params![path,key,owner,copied])?;
        Ok(())
    }
    fn done(&self) -> rusqlite::Result<bool> {
        self.0.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM catalog_semantic_events WHERE qualified=0)",
            [],
            |r| r.get(0),
        )
    }
}
fn qualify_events(c: &Connection) -> rusqlite::Result<bool> {
    qualify_with(&OrdinaryQualification(c))
}

fn decode_event(
    c: &Connection,
    text: Option<String>,
    used: &mut usize,
) -> rusqlite::Result<crate::model::relation_stage::NativeEvent> {
    crate::model::relation_stage::selector_event(
        &OrdinaryLookup {
            db: c,
            replay: Vec::new(),
        },
        text,
        used,
    )
}
pub(crate) struct ProjectedEvent {
    pub(crate) native: crate::model::relation_stage::NativeEvent,
    pub(crate) key: String,
    pub(crate) kind: &'static str,
    pub(crate) msg: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) encoded: String,
    pub(crate) seq: i64,
}

/// Physical operations only; the shared recipe owns decoding, page accounting,
/// event classification and frontier decisions.
pub(crate) trait EventProjectionPort {
    fn paths(&self) -> rusqlite::Result<Vec<String>>;
    fn input(&self, path: &str) -> rusqlite::Result<Option<(String, Option<String>)>>;
    fn cursor(&self, path: &str) -> rusqlite::Result<(i64, i64)>;
    fn candidates(
        &self,
        file: i64,
        after: i64,
        limit: usize,
        visit: &mut dyn FnMut(usize, Event, usize) -> rusqlite::Result<bool>,
    ) -> rusqlite::Result<()>;
    fn more(&self, file: i64, after: i64) -> rusqlite::Result<bool>;
    fn finish(&self, path: &str, after: Option<i64>, done: bool) -> rusqlite::Result<()>;
    fn put(&self, row: &ProjectedEvent) -> rusqlite::Result<()>;
    fn enqueue(&self, kind: &str, value: &str) -> rusqlite::Result<()>;
    fn native(&self, id: &str) -> rusqlite::Result<()>;
    fn done(&self) -> rusqlite::Result<bool>;
}

pub(crate) fn project_native_event(
    native: crate::model::relation_stage::NativeEvent,
    seq: usize,
) -> rusqlite::Result<ProjectedEvent> {
    let encoded = json(&native)?;
    if encoded.len() > MAX_CONTEXT {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let kind = if native.event.k == super::super::Kind::Xsm {
        "xsm"
    } else if native.event.n.as_deref() == Some("SendMessage") {
        if native
            .event
            .r
            .as_ref()
            .is_some_and(|reply| reply.f & super::super::PIN == 0)
        {
            "send"
        } else {
            "other"
        }
    } else if native.event.k == super::super::Kind::Tool {
        "call"
    } else {
        "other"
    };
    let msg = if kind == "xsm" {
        native.event.id.clone()
    } else if kind == "send" {
        native.event.r.as_ref().and_then(|r| r.m.clone())
    } else {
        None
    };
    let name = (kind == "xsm")
        .then(|| native.event.n.as_deref().map(crate::model::plain_name))
        .flatten();
    Ok(ProjectedEvent {
        key: location_key(&native.at)?,
        native,
        kind,
        msg,
        name,
        encoded,
        seq: i64::try_from(seq).map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}
#[cfg(feature = "postgres")]
pub(crate) fn projects_event(event: &Event) -> bool {
    matches!(
        event.k,
        super::super::Kind::Xsm | super::super::Kind::Agm | super::super::Kind::Tn
    ) || event.k == super::super::Kind::Tool
        && matches!(event.n.as_deref(), Some("SendMessage" | "Task" | "Agent"))
}

pub(crate) fn event_page_with(port: &impl EventProjectionPort) -> rusqlite::Result<bool> {
    let mut budget = 64usize;
    let mut bytes = 0usize;
    for path in port.paths()? {
        if budget == 0 {
            break;
        }
        let input = port.input(&path)?;
        let Some((harness, text)) = input else {
            port.finish(&path, None, true)?;
            budget -= 1;
            continue;
        };
        if harness != "claude" {
            port.finish(&path, None, true)?;
            budget -= 1;
            continue;
        }
        let source: crate::model::summary::CatalogSource = bounded(text, &mut 0)?;
        let (file, after) = port.cursor(&path)?;
        let mut end = after;
        let mut stopped = false;
        port.candidates(file, after, budget, &mut |seq, event, occurrence| {
            let native = crate::model::relation_stage::NativeEvent {
                owner: path.clone(),
                at: crate::model::relation_stage::Location {
                    source: source.clone(),
                    offset: event.o,
                    block: event.b,
                    kind: event.k,
                    occurrence,
                },
                event,
            };
            let row = project_native_event(native, seq)?;
            if bytes + row.encoded.len() > MAX_CONTEXT {
                stopped = true;
                return Ok(false);
            }
            port.put(&row)?;
            if let Some(msg) = &row.msg {
                port.enqueue("receipt", msg)?;
                port.enqueue("sender", msg)?;
            }
            if let Some(peer) = row
                .native
                .event
                .peer
                .as_ref()
                .and_then(|peer| peer.sock.as_deref())
            {
                port.native(peer)?;
            }
            end = row.seq;
            budget -= 1;
            bytes += row.encoded.len();
            Ok(true)
        })?;
        port.finish(&path, Some(end), !port.more(file, end)?)?;
        if stopped {
            break;
        }
    }
    port.done()
}

/// Decode the same physical event columns in both SQLite adapters.
pub(crate) fn projection_candidate(row: &Row<'_>) -> rusqlite::Result<(usize, Event)> {
    let (_, seq, event) = read_event(row)?;
    Ok((seq, event))
}
/// One candidate predicate, expanded by each physical SQL adapter.
pub(crate) fn projection_predicate(alias: &str) -> String {
    format!(
        "({alias}k IN ('xsm','agm','tn') OR {alias}k='tool' AND {alias}n IN ('SendMessage','Task','Agent'))"
    )
}
pub(crate) fn projection_columns() -> &'static str {
    EVENT_COLUMNS
}

struct OrdinaryProjection<'a>(&'a Connection);
impl EventProjectionPort for OrdinaryProjection<'_> {
    fn paths(&self) -> rusqlite::Result<Vec<String>> {
        self.0.prepare("SELECT source_path FROM catalog_semantic_sources WHERE index_done=1 AND events_done=0 ORDER BY source_path LIMIT 64")?.query_map([],|r|r.get(0))?.collect()
    }
    fn input(&self, path: &str) -> rusqlite::Result<Option<(String, Option<String>)>> {
        self.0.query_row("SELECT harness,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_inputs WHERE source_path=?1",[path],|r|Ok((r.get(0)?,r.get(1)?))).optional()
    }
    fn cursor(&self, path: &str) -> rusqlite::Result<(i64, i64)> {
        self.0.query_row("SELECT f.file_id,s.event_after FROM files f JOIN catalog_semantic_sources s ON s.source_path=f.path WHERE f.path=?1",[path],|r|Ok((r.get(0)?,r.get(1)?)))
    }
    fn candidates(
        &self,
        file: i64,
        after: i64,
        limit: usize,
        visit: &mut dyn FnMut(usize, Event, usize) -> rusqlite::Result<bool>,
    ) -> rusqlite::Result<()> {
        let predicate = projection_predicate("");
        let sql = format!(
            "SELECT {EVENT_COLUMNS} FROM events WHERE file_id=?1 AND extra=0 AND seq>?2 AND {predicate} ORDER BY seq LIMIT ?3"
        );
        let mut q = self.0.prepare(&sql)?;
        let mut rows = q.query(params![file, after, limit as i64])?;
        while let Some(row) = rows.next()? {
            let (seq, event) = projection_candidate(row)?;
            let occurrence: i64 = self.0.query_row("SELECT COUNT(*) FROM events WHERE file_id=?1 AND extra=0 AND o=?2 AND b=?3 AND k=?4 AND seq<?5",params![file,int(event.o),event.b,tag(&event.k)?,i64::try_from(seq).map_err(|_|rusqlite::Error::InvalidQuery)?],|r|r.get(0))?;
            if !visit(
                seq,
                event,
                usize::try_from(occurrence).map_err(|_| rusqlite::Error::InvalidQuery)?,
            )? {
                break;
            }
        }
        Ok(())
    }
    fn more(&self, file: i64, after: i64) -> rusqlite::Result<bool> {
        let predicate = projection_predicate("");
        self.0.query_row(&format!("SELECT EXISTS(SELECT 1 FROM events WHERE file_id=?1 AND extra=0 AND seq>?2 AND {predicate})"),params![file,after],|r|r.get(0))
    }
    fn finish(&self, path: &str, after: Option<i64>, done: bool) -> rusqlite::Result<()> {
        if let Some(after) = after {
            self.0.execute("UPDATE catalog_semantic_sources SET event_after=?2,events_done=?3 WHERE source_path=?1",params![path,after,done])?;
        } else {
            self.0.execute(
                "UPDATE catalog_semantic_sources SET events_done=1 WHERE source_path=?1",
                [path],
            )?;
        }
        Ok(())
    }
    fn put(&self, row: &ProjectedEvent) -> rusqlite::Result<()> {
        let native = &row.native;
        self.0.execute("INSERT OR REPLACE INTO catalog_semantic_events(source_path,event_key,owner,kind,msg_id,tool_id,name,metadata,native_seq,copy_key) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![native.owner,row.key,native.owner,row.kind,row.msg,native.event.id,row.name,row.encoded,row.seq,crate::model::native_copy_identity(&native.event)])?;
        Ok(())
    }
    fn enqueue(&self, kind: &str, value: &str) -> rusqlite::Result<()> {
        queue(self.0, kind, value, "")
    }
    fn native(&self, id: &str) -> rusqlite::Result<()> {
        native_frontier(self.0, "claude", id)
    }
    fn done(&self) -> rusqlite::Result<bool> {
        self.0.query_row("SELECT NOT EXISTS(SELECT 1 FROM catalog_semantic_sources WHERE index_done=1 AND events_done=0)",[],|r|r.get(0))
    }
}
fn event_page(c: &Connection) -> rusqlite::Result<bool> {
    event_page_with(&OrdinaryProjection(c))
}

fn reset(c: &Connection, runtime: &str) -> rusqlite::Result<()> {
    c.execute(metadata_increment(c)?, ["semantic_epoch"])?;
    c.execute("UPDATE catalog_semantic_stage SET epoch=(SELECT CAST(value AS INTEGER) FROM meta WHERE namespace=semon_namespace() AND key='semantic_epoch'),phase='indexing',generation=?1,runtime=?2,revision=?3,resetting=1,reset_after='',key_after_harness='',key_after_native='',keys_done=0,key_candidate='',key_epoch=key_epoch+1,withdrawal_authority=NULL",params![generation(c)?,runtime,revision(c)?])?;
    Ok(())
}
/// Unreachable staging records carry no authority without the singleton epoch.
/// Reclaim at most the existing 64 metadata rows, leaving published projections
/// and source ledgers untouched. A new stage waits for this queue to finish.
fn gc_page(c: &Connection) -> rusqlite::Result<Option<usize>> {
    let mut budget = 64;
    for (table, key) in [
        ("catalog_semantic_inputs", "source_path"),
        ("catalog_semantic_keys", "harness,native_id"),
        ("catalog_semantic_relations", "relation_id"),
        ("catalog_semantic_events", "source_path,event_key"),
        ("catalog_semantic_endpoints", "session_key"),
        ("catalog_semantic_native", "harness,native_id,session_key"),
        ("catalog_semantic_owner_sources", "session_key,source_path"),
        ("catalog_semantic_paths", "path,session_key"),
        ("catalog_semantic_names", "name,session_key"),
        ("catalog_semantic_frontier", "kind,value,harness"),
    ] {
        if budget == 0 {
            return Ok(None);
        }
        let count=c.execute(&format!("DELETE FROM {table} WHERE ({key}) IN (SELECT {key} FROM {table} ORDER BY {key} LIMIT ?1)"),[budget as i64])?;
        budget -= count;
    }
    Ok(Some(budget))
}
fn gc_pending(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE namespace=semon_namespace() AND key='catalog_semantic_gc' AND value='1')",
        [],
        |r| r.get(0),
    )
}
fn reset_page(c: &Connection) -> rusqlite::Result<bool> {
    let Some(budget) = gc_page(c)? else {
        return Ok(false);
    };
    if budget == 0 {
        return Ok(false);
    }
    let after: String = c.query_row("SELECT reset_after FROM catalog_semantic_stage", [], |r| {
        r.get(0)
    })?;
    let paths=c.prepare("SELECT source_path FROM catalog_semantic_sources WHERE source_path>?1 ORDER BY source_path LIMIT ?2")?.query_map(params![after,budget as i64],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for path in &paths {
        c.execute("UPDATE catalog_semantic_sources SET claim=NULL,ledger=NULL,withdrawal=NULL,index_done=0,event_after=-1,events_done=0,identity_done=0,relation_done=0,publication_done=0,ack_done=0,relation_after='' WHERE source_path=?1",[path])?;
    }
    if let Some(last) = paths.last() {
        c.execute("UPDATE catalog_semantic_stage SET reset_after=?1", [last])?;
    }
    if paths.len() < budget {
        c.execute(
            "UPDATE catalog_semantic_stage SET resetting=0,reset_after='',revision=?1",
            [revision(c)?],
        )?;
        return Ok(true);
    }
    Ok(false)
}
fn withdrawal(c: &Connection, path: &str) -> rusqlite::Result<Option<Withdrawal>> {
    let text:Option<Option<String>>=c.query_row("SELECT CASE WHEN octet_length(withdrawal)<=1048576 THEN withdrawal END FROM catalog_semantic_sources WHERE source_path=?1 AND withdrawal IS NOT NULL",[path],|r|r.get(0)).optional()?;
    text.map(|text| bounded(text, &mut 0)).transpose()
}
fn withdrawal_authority(c: &Connection) -> rusqlite::Result<Option<Withdrawal>> {
    let text:Option<Option<String>>=c.query_row("SELECT CASE WHEN octet_length(withdrawal_authority)<=1048576 THEN withdrawal_authority END FROM catalog_semantic_stage WHERE withdrawal_authority IS NOT NULL",[],|r|r.get(0)).optional()?;
    text.map(|text| bounded(text, &mut 0)).transpose()
}
fn witness_rejection(c: &Connection) -> rusqlite::Result<Option<&'static str>> {
    // Source inventories are deliberately outside the runtime fingerprint.
    // This constant-size authority fence covers withdrawals acknowledged on
    // earlier pages as well as the selected page, until pointer retirement.
    if withdrawal_authority(c)?.is_some_and(|w| !w.authority_current().unwrap_or(false)) {
        return Ok(Some("withdrawal_authority"));
    }
    // Our own completed steps advance this fence. With no new observation the
    // previous qualification is unchanged, regardless of component width.
    let observed: i64 = c.query_row("SELECT revision FROM catalog_semantic_stage", [], |r| {
        r.get(0)
    })?;
    if observed == revision(c)? {
        return Ok(None);
    }
    let changed: Option<bool> = c.query_row("SELECT i.revision IS NOT s.claim FROM catalog_semantic_sources s LEFT JOIN session_catalog_invalidations i USING(source_path) LEFT JOIN files f ON f.path=s.source_path WHERE s.index_done=1 AND (i.revision IS NOT s.claim OR f.dev IS NOT s.dev OR f.ino IS NOT s.ino OR f.size IS NOT s.size OR f.mtime_ns IS NOT s.mtime_ns OR f.ctime_ns IS NOT s.ctime_ns OR f.resume_at IS NOT s.resume_at OR f.head_sha256 IS NOT s.head_sha256 OR f.tail_sha256 IS NOT s.tail_sha256) LIMIT 1",[],|r|r.get(0)).optional()?;
    Ok(changed.map(|claim_changed| {
        if claim_changed {
            "witness_claim"
        } else {
            "witness_ledger"
        }
    }))
}
fn witnesses(c: &Connection) -> rusqlite::Result<bool> {
    Ok(witness_rejection(c)?.is_none())
}
pub(super) fn batch_rejection(c: &Connection, b: &Batch) -> rusqlite::Result<Option<&'static str>> {
    struct Fence {
        epoch: i64,
        phase: String,
        generation: Option<String>,
        runtime: String,
        revision: i64,
        resetting: bool,
    }
    let actual = c
        .query_row(
            "SELECT epoch,phase,generation,runtime,revision,resetting FROM catalog_semantic_stage",
            [],
            |r| {
                Ok(Fence {
                    epoch: r.get(0)?,
                    phase: r.get(1)?,
                    generation: r.get(2)?,
                    runtime: r.get(3)?,
                    revision: r.get(4)?,
                    resetting: r.get(5)?,
                })
            },
        )
        .optional()?;
    let Some(actual) = actual else {
        return Ok(Some("stage_missing"));
    };
    if actual.resetting {
        return Ok(Some("stage_resetting"));
    }
    if actual.epoch != b.epoch {
        return Ok(Some("stage_epoch"));
    }
    if actual.phase != phase(b.phase) {
        return Ok(Some("stage_phase"));
    }
    if actual.revision != b.source_revision {
        return Ok(Some("stage_revision"));
    }
    if actual.generation != b.base_generation {
        return Ok(Some("stage_generation"));
    }
    if actual.runtime != b.runtime {
        return Ok(Some("stage_runtime"));
    }
    if generation(c)? != b.base_generation {
        return Ok(Some("catalog_generation"));
    }
    witness_rejection(c)
}
fn bounded<T: DeserializeOwned>(text: Option<String>, used: &mut usize) -> rusqlite::Result<T> {
    crate::model::relation_stage::selector_metadata(text, used)
        .map_err(|_| rusqlite::Error::InvalidQuery)
}
/// Advance at most one existing 64-record metadata page. Parent, root and
/// descendant-cost reductions survive replacement of the producer owner.
/// The reduction schedule is shared by unbound and admitted physical adapters.
/// Operations carry semantic values, never caller-supplied SQL.
pub(crate) trait ReductionStore {
    fn parents(&self, limit: usize) -> rusqlite::Result<Vec<String>>;
    fn parent(&self, key: &str) -> rusqlite::Result<()>;
    fn roots(&self, limit: usize) -> rusqlite::Result<Vec<String>>;
    fn root(&self, key: &str) -> rusqlite::Result<()>;
    fn missing_roots(&self) -> rusqlite::Result<bool>;
    fn leaves(&self, limit: usize) -> rusqlite::Result<Vec<String>>;
    fn unfinished_costs(&self) -> rusqlite::Result<bool>;
    fn cost(&self, key: &str) -> rusqlite::Result<()>;
    fn complete(&self) -> rusqlite::Result<bool>;
}

pub(crate) fn reduce_with(store: &impl ReductionStore) -> rusqlite::Result<bool> {
    let mut budget = 64usize;
    while budget > 0 {
        let parents = store.parents(budget)?;
        if !parents.is_empty() {
            budget -= parents.len();
            for key in parents {
                store.parent(&key)?;
            }
            continue;
        }
        let roots = store.roots(budget)?;
        if !roots.is_empty() {
            budget -= roots.len();
            for key in roots {
                store.root(&key)?;
            }
            continue;
        }
        if store.missing_roots()? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let costs = store.leaves(budget)?;
        if costs.is_empty() {
            return if store.unfinished_costs()? {
                Err(rusqlite::Error::InvalidQuery)
            } else {
                Ok(true)
            };
        }
        budget -= costs.len();
        for key in costs {
            store.cost(&key)?;
        }
    }
    store.complete()
}

fn reduce(c: &Connection) -> rusqlite::Result<bool> {
    reduce_with(&UnboundReduction(c))
}
struct UnboundReduction<'a>(&'a Connection);
impl ReductionStore for UnboundReduction<'_> {
    fn parents(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        self.0.prepare("SELECT session_key FROM catalog_semantic_endpoints WHERE parent_resolved=0 ORDER BY parent_native,session_key LIMIT ?1")?.query_map([limit as i64],|r|r.get(0))?.collect()
    }
    fn parent(&self, key: &str) -> rusqlite::Result<()> {
        let c = self.0;
        let ambiguous:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_native own JOIN catalog_semantic_native peer USING(harness,native_id) WHERE own.session_key=?1 AND peer.session_key<>?1)",[key],|r|r.get(0))?;
        if ambiguous {
            return Err(rusqlite::Error::InvalidQuery);
        }
        c.execute("UPDATE catalog_semantic_endpoints SET parent_key=COALESCE((SELECT session_key FROM catalog_semantic_native n WHERE n.harness=COALESCE(catalog_semantic_endpoints.parent_native_harness,catalog_semantic_endpoints.harness) AND n.native_id=catalog_semantic_endpoints.parent_native),parent_key),parent_resolved=1,family_cost=cost WHERE session_key=?1",[key])?;
        c.execute("UPDATE catalog_semantic_endpoints SET root_key=CASE WHEN parent_key IS NULL OR NOT EXISTS(SELECT 1 FROM catalog_semantic_endpoints p WHERE p.session_key=catalog_semantic_endpoints.parent_key) THEN session_key ELSE (SELECT root_key FROM catalog_semantic_endpoints p WHERE p.session_key=catalog_semantic_endpoints.parent_key) END WHERE session_key=?1",[key])?;
        c.execute("UPDATE catalog_semantic_paths SET root_key=(SELECT root_key FROM catalog_semantic_endpoints WHERE session_key=?1) WHERE session_key=?1",[key])?;
        c.execute("UPDATE catalog_semantic_endpoints SET children_pending=children_pending+1 WHERE session_key=(SELECT parent_key FROM catalog_semantic_endpoints WHERE session_key=?1)",[key])?;
        Ok(())
    }
    fn roots(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        self.0.prepare("SELECT e.session_key FROM catalog_semantic_endpoints e WHERE e.root_key IS NULL AND EXISTS(SELECT 1 FROM catalog_semantic_endpoints p WHERE p.session_key=e.parent_key AND p.root_key IS NOT NULL) ORDER BY e.session_key LIMIT ?1")?.query_map([limit as i64],|r|r.get(0))?.collect()
    }
    fn root(&self, key: &str) -> rusqlite::Result<()> {
        self.0.execute("UPDATE catalog_semantic_endpoints SET root_key=(SELECT root_key FROM catalog_semantic_endpoints p WHERE p.session_key=catalog_semantic_endpoints.parent_key) WHERE session_key=?1",[key])?;
        self.0.execute("UPDATE catalog_semantic_paths SET root_key=(SELECT root_key FROM catalog_semantic_endpoints WHERE session_key=?1) WHERE session_key=?1",[key])?;
        Ok(())
    }
    fn missing_roots(&self) -> rusqlite::Result<bool> {
        self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_semantic_endpoints WHERE root_key IS NULL)",
            [],
            |r| r.get(0),
        )
    }
    fn leaves(&self, limit: usize) -> rusqlite::Result<Vec<String>> {
        self.0.prepare("SELECT session_key FROM catalog_semantic_endpoints WHERE cost_done=0 AND children_pending=0 ORDER BY session_key LIMIT ?1")?.query_map([limit as i64],|r|r.get(0))?.collect()
    }
    fn unfinished_costs(&self) -> rusqlite::Result<bool> {
        self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_semantic_endpoints WHERE cost_done=0)",
            [],
            |r| r.get(0),
        )
    }
    fn cost(&self, key: &str) -> rusqlite::Result<()> {
        let (parent, cost): (Option<String>, Option<f64>) = self.0.query_row(
            "SELECT parent_key,family_cost FROM catalog_semantic_endpoints WHERE session_key=?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        self.0.execute(
            "UPDATE catalog_semantic_endpoints SET cost_done=1 WHERE session_key=?1",
            [key],
        )?;
        if let Some(parent) = parent {
            self.0.execute("UPDATE catalog_semantic_endpoints SET children_pending=children_pending-1,family_cost=CASE WHEN family_cost IS NULL OR ?2 IS NULL THEN NULL ELSE family_cost+?2 END WHERE session_key=?1 AND cost_done=0 AND children_pending>0",params![parent,cost])?;
        }
        Ok(())
    }
    fn complete(&self) -> rusqlite::Result<bool> {
        self.0.query_row("SELECT NOT EXISTS(SELECT 1 FROM catalog_semantic_endpoints WHERE parent_resolved=0 OR root_key IS NULL OR cost_done=0)",[],|r|r.get(0))
    }
}
#[derive(Serialize, Deserialize)]
struct RelationCursor {
    path: String,
    after: String,
    end: String,
}
fn location_key(at: &crate::model::relation_stage::Location) -> rusqlite::Result<String> {
    Ok(format!(
        "{:020}:{:010}:{}:{:020}",
        at.offset,
        at.block,
        tag(&at.kind)?,
        at.occurrence
    ))
}
fn event_context(c: &Connection, path: &str, context: &mut Context) -> rusqlite::Result<String> {
    let after: String = c.query_row(
        "SELECT relation_after FROM catalog_semantic_sources WHERE source_path=?1",
        [path],
        |r| r.get(0),
    )?;
    let mut paths: BTreeSet<String> = owner_closure(c, path)?.into_iter().collect();
    paths.extend(context.parent_paths(&[path.to_owned()]));
    // A source without messages still needs the complete exact tool companion.
    for endpoint in context.endpoints.values() {
        for source in &endpoint.row.sources {
            if let Some(id) = source
                .native_role
                .as_ref()
                .and_then(|r| r.get("Agent"))
                .and_then(|r| r.get("tool_use_id"))
                .and_then(Value::as_str)
            {
                let peer:Option<String>=c.query_row("SELECT source_path FROM catalog_semantic_events WHERE kind='call' AND tool_id=?1 AND qualified=1 AND copied=0 ORDER BY source_path,native_seq LIMIT 1",[id],|r|r.get(0)).optional()?;
                if let Some(peer) = peer {
                    paths.extend(owner_closure(c, &peer)?);
                }
            }
        }
    }
    if paths.len() > 64 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut q=c.prepare("SELECT event_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_events WHERE source_path=?1 AND event_key>?2 AND copied=0 AND qualified=1 ORDER BY event_key LIMIT 64")?;
    let mut rows = q.query(params![path, after])?;
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    let mut used = 0usize;
    let mut end = after.clone();
    let mut sender_sources = HashMap::<(String, String), Vec<String>>::new();
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let text: Option<String> = row.get(1)?;
        let mut candidate_used = used;
        let own = match decode_event(c, text, &mut candidate_used) {
            Ok(own) => own,
            Err(rusqlite::Error::InvalidQuery) if !selected.is_empty() => break,
            Err(e) => return Err(e),
        };
        let msg = if own.event.k == super::super::Kind::Xsm {
            own.event.id.as_deref()
        } else if own.event.n.as_deref() == Some("SendMessage") {
            own.event.r.as_ref().and_then(|r| r.m.as_deref())
        } else {
            None
        };
        let mut linked = vec![own.clone()];
        let mut overflow = false;
        if let Some(id) = msg {
            for kind in ["xsm", "send"] {
                let paired: Option<Option<String>> = c
                    .prepare_cached("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_events WHERE msg_id=?1 AND kind=?2 AND copied=0 AND qualified=1 ORDER BY source_path,native_seq LIMIT 1")?
                    .query_row(params![id, kind], |r| r.get(0))
                    .optional()?;
                if let Some(text) = paired {
                    match decode_event(c, text, &mut candidate_used) {
                        Ok(peer) => linked.push(peer),
                        Err(rusqlite::Error::InvalidQuery) if !selected.is_empty() => {
                            overflow = true;
                            break;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        if overflow {
            break;
        }
        let mut candidate = paths.clone();
        // A subagent's send that isn't its hand-back names no message id:
        // the model pairs it with the sender's own `SendMessage`, so the
        // sender's log joins the page.
        if own.event.k == super::super::Kind::Agm
            && own.event.script & super::super::AGM_PROGRESS != 0
            && let Some(sender) = own
                .event
                .peer
                .as_ref()
                .and_then(|peer| peer.sock.as_deref())
                .or(own.event.id.as_deref())
        {
            // The stage's own inputs, not the published catalog: a first
            // build has no catalog yet when relations are matched.
            let root = path
                .find("/subagents/")
                .map(|at| &path[..at])
                .or_else(|| path.strip_suffix(".jsonl"))
                .unwrap_or(path);
            let key = (root.to_owned(), sender.to_owned());
            let sources = if let Some(sources) = sender_sources.get(&key) {
                sources.clone()
            } else {
                let expected = format!("{root}/subagents/agent-{sender}.jsonl");
                let exact = c
                    .prepare_cached("SELECT source_path FROM catalog_semantic_inputs WHERE source_path=?1 AND harness='claude' AND is_top=0")?
                    .query_row([&expected], |r| r.get::<_, String>(0))
                    .optional()?;
                let sources = if let Some(source) = exact {
                    vec![source]
                } else {
                    c.prepare_cached("SELECT source_path FROM catalog_semantic_inputs WHERE harness='claude' AND native_id=?1 AND is_top=0 ORDER BY source_path LIMIT 8")?
                        .query_map([sender], |r| r.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                sender_sources.insert(key, sources.clone());
                sources
            };
            for source in sources {
                candidate.extend(owner_closure(c, &source)?);
            }
        }
        for peer in &linked {
            let mut sources = c.prepare_cached("SELECT source_path FROM catalog_semantic_owner_sources WHERE session_key=?1 ORDER BY source_path LIMIT 65")?;
            for source in sources.query_map([&peer.owner], |r| r.get::<_, String>(0))? {
                candidate.insert(source?);
            }
        }
        if candidate.len() > 64 {
            if selected.is_empty() {
                return Err(rusqlite::Error::InvalidQuery);
            }
            break;
        }
        paths = candidate;
        used = candidate_used;
        for peer in linked {
            let identity = (peer.at.source.path.clone(), location_key(&peer.at)?);
            if seen.insert(identity) {
                selected.push(peer.at);
            }
        }
        end = key;
    }
    drop(rows);
    drop(q);
    // Companion actors also need their normalized true-root/parent descriptors.
    let extra = load_context(
        c,
        &paths.iter().cloned().collect::<Vec<_>>(),
        Phase::Relations,
        context.epoch.ok_or(rusqlite::Error::InvalidQuery)?,
    )?;
    context.endpoints.extend(extra.endpoints);
    context.companions = paths
        .into_iter()
        .filter(|candidate| candidate != path)
        .collect();
    context.matching_locations = Some(selected);
    serde_json::to_string(&RelationCursor {
        path: path.to_owned(),
        after,
        end,
    })
    .map_err(|_| rusqlite::Error::InvalidQuery)
}
fn load_context(
    c: &Connection,
    paths: &[String],
    p: Phase,
    epoch: i64,
) -> rusqlite::Result<Context> {
    let mut owners = BTreeSet::new();
    for path in paths {
        let mut q = c.prepare(
            "SELECT session_key FROM catalog_semantic_owner_sources WHERE source_path=?1",
        )?;
        for owner in q.query_map([path], |r| r.get::<_, String>(0))? {
            owners.insert(owner?);
        }
    }
    let mut selected = owners.clone();
    let mut out = Context {
        epoch: Some(epoch),
        replay_owners: if p == Phase::Publication {
            owners.iter().cloned().collect()
        } else {
            Vec::new()
        },
        ..Context::default()
    };
    let mut used = 0;
    for owner in &owners {
        if p == Phase::Publication {
            // Incident edges can exceed one metadata page for a single owner.
            // Replay them after releasing this writer, through read-only pages.
            let mut q=c.prepare("SELECT session_key FROM catalog_semantic_endpoints WHERE parent_key=?1 ORDER BY session_key")?;
            let mut children = Vec::new();
            for child in q.query_map([owner], |r| r.get::<_, String>(0))? {
                let child = child?;
                used += child.len();
                if used > MAX_CONTEXT {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                children.push(child);
            }
            out.children.insert(owner.clone(), children);
            let total:Option<f64>=c.query_row("SELECT family_cost FROM catalog_semantic_endpoints WHERE session_key=?1 AND cost_done=1",[owner],|r|r.get(0))?;
            out.family_costs.insert(owner.clone(), total);
        }
        let parent: Option<String> = c
            .query_row(
                "SELECT parent_key FROM catalog_semantic_endpoints WHERE session_key=?1",
                [owner],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        selected.extend(parent);
    }
    for key in selected {
        let found:Option<(Option<String>,Option<String>,Option<String>)>=c.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,parent_key,root_key FROM catalog_semantic_endpoints WHERE session_key=?1",[&key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((text, parent, root)) = found {
            let mut endpoint: Endpoint = bounded(text, &mut used)?;
            endpoint.parent = parent.clone();
            endpoint.row.parent = parent;
            endpoint.root = root.unwrap_or_else(|| key.clone());
            endpoint.root_alive = root_alive(c, &endpoint.root)?;
            out.endpoints.insert(key, endpoint);
        }
    }
    Ok(out)
}

/// Only a complete shared matching observation can consume a companion owner.
/// Keep the qualification, incident-edge scope and cursor mutation in one CAS.
struct RelationScope {
    owners: BTreeSet<String>,
    companions: std::collections::BTreeMap<String, String>,
}
fn relation_scope(
    c: &Connection,
    batch: &Batch,
    prepared: &crate::model::PreparedSources,
) -> rusqlite::Result<RelationScope> {
    let mut scope = RelationScope {
        owners: BTreeSet::new(),
        companions: std::collections::BTreeMap::new(),
    };
    for path in &batch.paths {
        let mut owners=c.prepare("SELECT session_key FROM catalog_semantic_owner_sources WHERE source_path=?1 ORDER BY session_key LIMIT 65")?;
        for owner in owners.query_map([path], |row| row.get::<_, String>(0))? {
            scope.owners.insert(owner?);
        }
    }
    let Some(locations) = batch.context.matching_locations.as_ref() else {
        return Ok(scope);
    };
    let mut roster: std::collections::BTreeMap<String, BTreeSet<String>> =
        std::collections::BTreeMap::new();
    let mut candidates = BTreeSet::new();
    for location in locations {
        let path = location.source.path.to_string_lossy().into_owned();
        roster
            .entry(path.clone())
            .or_default()
            .insert(location_key(location)?);
        if !batch.paths.contains(&path) {
            let mut owners=c.prepare("SELECT session_key FROM catalog_semantic_owner_sources WHERE source_path=?1 ORDER BY session_key LIMIT 65")?;
            for owner in owners.query_map([&path], |row| row.get::<_, String>(0))? {
                candidates.insert(owner?);
            }
        }
    }
    if roster.len() > 64 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut budget = 64usize;
    for owner in candidates {
        if budget == 0 {
            break;
        }
        if scope.owners.contains(&owner) {
            continue;
        }
        let Some(endpoint) = prepared.relationships.endpoints.get(&owner) else {
            continue;
        };
        if endpoint.row.harness != "claude" {
            continue;
        }
        let paths=c.prepare("SELECT source_path FROM catalog_semantic_owner_sources WHERE session_key=?1 ORDER BY source_path LIMIT 65")?.query_map([&owner],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        if paths.len() > 64 || paths.len() != endpoint.row.sources.len() {
            continue;
        }
        let mut covered = std::collections::BTreeMap::new();
        let mut complete = true;
        let mut pending = false;
        for path in paths {
            let Some((_, observed)) = prepared
                .claims
                .iter()
                .find(|(selected, _)| selected == &path)
            else {
                complete = false;
                break;
            };
            let Some(source) = endpoint
                .row
                .sources
                .iter()
                .find(|source| source.path == Path::new(&path))
            else {
                complete = false;
                break;
            };
            let qualified:bool=c.query_row("SELECT index_done=1 AND events_done=1 AND identity_done=1 AND claim IS ?2 FROM catalog_semantic_sources WHERE source_path=?1",params![path,observed],|row|row.get(0))?;
            if !qualified
                || !selected_roles_current(c, std::slice::from_ref(source))?
                || locations.iter().any(|location| {
                    location.source.path == source.path && location.source != *source
                })
            {
                complete = false;
                break;
            }
            let done: bool = c.query_row(
                "SELECT relation_done=1 FROM catalog_semantic_sources WHERE source_path=?1",
                [&path],
                |row| row.get(0),
            )?;
            pending |= !done;
            // Require the entire owner's candidate roster, including previously
            // processed sources: a partial later slice cannot refine its state.
            // Metadata inspection is shared across companions under one page.
            if budget == 0 {
                complete = false;
                break;
            }
            let mut events=c.prepare("SELECT event_key FROM catalog_semantic_events WHERE source_path=?1 AND copied=0 AND qualified=1 ORDER BY event_key LIMIT ?2")?;
            let mut rows = events.query(params![path, budget as i64])?;
            let mut end = String::new();
            while let Some(row) = rows.next()? {
                let key: String = row.get(0)?;
                budget -= 1;
                if !roster
                    .get(&path)
                    .is_some_and(|selected| selected.contains(&key))
                {
                    complete = false;
                    break;
                }
                end = key;
            }
            drop(rows);
            drop(events);
            if !complete {
                break;
            }
            let more:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_events WHERE source_path=?1 AND copied=0 AND qualified=1 AND event_key>?2)",params![path,end],|row|row.get(0))?;
            if more {
                complete = false;
                break;
            }
            if !done {
                covered.insert(path, end);
            }
        }
        if complete && pending {
            scope.owners.insert(owner);
            scope.companions.extend(covered);
        }
    }
    Ok(scope)
}

impl SqliteStore {
    #[cfg(test)]
    pub(super) fn semantic_begin(
        &mut self,
        paths: &[String],
        runtime: &str,
    ) -> Result<Outcome, StoreError> {
        self.semantic_begin_with_journal(paths, runtime, false)
    }

    pub(super) fn semantic_begin_with_journal(
        &mut self,
        paths: &[String],
        runtime: &str,
        journaled_runtime: bool,
    ) -> Result<Outcome, StoreError> {
        if paths.len() > 64 {
            return Err(StoreError::Data(
                "semantic source batch budget exceeded".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        if !current(&tx).map_err(failure)? {
            return Ok(Outcome::Stale);
        }
        if !active(&tx).map_err(failure)? && gc_pending(&tx).map_err(failure)? {
            if let Some(budget) = gc_page(&tx).map_err(failure)? {
                let removed=tx.execute("DELETE FROM catalog_semantic_sources WHERE source_path IN (SELECT source_path FROM catalog_semantic_sources ORDER BY source_path LIMIT ?1)",[budget as i64]).map_err(failure)?;
                let remains: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources)",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(failure)?;
                if removed < budget && !remains {
                    tx.execute(
                        native_metadata_delete(&tx).map_err(failure)?,
                        ["catalog_semantic_gc"],
                    )
                    .map_err(failure)?;
                }
            }
            tx.commit().map_err(failure)?;
            return Ok(Outcome::Written);
        }
        if active(&tx).map_err(failure)? {
            let old: String = tx
                .query_row("SELECT runtime FROM catalog_semantic_stage", [], |r| {
                    r.get(0)
                })
                .map_err(failure)?;
            let journaled: bool = journaled_runtime && tx.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE namespace=semon_namespace() AND key='catalog_push_driven' AND value='1')", [], |r| r.get(0)).map_err(failure)?;
            // Journaled Facts/Raw writes invalidate the actual staged native
            // and PID dependencies, including owners not yet published. The
            // next batch still checks every consumed source witness. Unrelated
            // machine activity must not discard this component's progress.
            // Local/unobserved runtime and negative custody keep their original
            // conservative epoch fence.
            if !recipe_current(&tx).map_err(failure)? {
                return Err(StoreError::Failed("unsupported semantic recipe".into()));
            }
            if old != runtime
                && (!journaled || withdrawal_authority(&tx).map_err(failure)?.is_some())
            {
                reset(&tx, runtime).map_err(failure)?;
            } else if old != runtime {
                tx.execute("UPDATE catalog_semantic_stage SET runtime=?1", [runtime])
                    .map_err(failure)?;
            }
        } else {
            if paths.is_empty() {
                return Ok(Outcome::Conflict);
            }
            tx.execute(
                metadata_increment(&tx).map_err(failure)?,
                ["semantic_epoch"],
            )
            .map_err(failure)?;
            tx.execute("INSERT INTO catalog_semantic_stage(singleton,epoch,phase,generation,runtime,revision) SELECT 1,CAST(value AS INTEGER),'indexing',?1,?2,?3 FROM meta WHERE namespace=semon_namespace() AND key='semantic_epoch'",params![generation(&tx).map_err(failure)?,runtime,revision(&tx).map_err(failure)?]).map_err(failure)?;
        }
        record_recipe(&tx).map_err(failure)?;
        for path in paths {
            enqueue(&tx, path).map_err(failure)?;
        }
        let late:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage WHERE phase<>'indexing') AND EXISTS(SELECT 1 FROM catalog_semantic_sources WHERE index_done=0)",[],|r|r.get(0)).map_err(failure)?;
        if late {
            reset(&tx, runtime).map_err(failure)?;
        }

        tx.execute(
            native_metadata_put(&tx).map_err(failure)?,
            params!["catalog_completeness", "partial"],
        )
        .map_err(failure)?;
        tx.commit().map_err(failure)?;
        Ok(Outcome::Written)
    }
    pub(super) fn semantic_replay(
        &mut self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> Result<Context, StoreError> {
        if owners.len() > 64 {
            return Err(StoreError::Data(
                "semantic replay owner budget exceeded".into(),
            ));
        }
        // A read snapshot prevents a concurrent reset from mixing epochs. It
        // never reserves the writer while loading a high-degree owner's edges.
        let tx = self.connection.transaction().map_err(failure)?;
        let current: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_semantic_stage WHERE epoch=?1 AND phase='publication' AND resetting=0)",
            [epoch], |r|r.get(0),
        ).map_err(failure)?;
        if !current {
            return Err(StoreError::Data("stale semantic replay epoch".into()));
        }
        let mut keys = BTreeSet::new();
        for owner in owners {
            for column in ["from_key", "to_key"] {
                let mut q = tx.prepare(&format!("SELECT relation_id FROM catalog_semantic_relations WHERE {column}=?1 AND relation_id>?2 ORDER BY relation_id LIMIT 64")).map_err(failure)?;
                for key in q
                    .query_map(params![owner, after], |r| r.get::<_, String>(0))
                    .map_err(failure)?
                {
                    keys.insert(key.map_err(failure)?);
                    if keys.len() > 64 {
                        keys.pop_last();
                    }
                }
            }
        }
        let mut out = Context {
            epoch: Some(epoch),
            ..Context::default()
        };
        let mut used = 0;
        for key in keys {
            let mut candidate_used = used;
            let candidate = (|| -> rusqlite::Result<_> {
                let text: Option<String> = tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_relations WHERE relation_id=?1",[&key],|r|r.get(0))?;
                let relation: Relation = bounded(text, &mut candidate_used)?;
                let mut endpoints = std::collections::BTreeMap::new();
                for key in std::iter::once(&relation.from).chain(relation.to.iter()) {
                    if out.endpoints.contains_key(key) || endpoints.contains_key(key) {
                        continue;
                    }
                    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_endpoints WHERE session_key=?1)",[key],|r|r.get(0))?;
                    if exists {
                        endpoints.insert(
                            key.clone(),
                            normalized_endpoint(&tx, key, &mut candidate_used)?,
                        );
                    }
                }
                Ok((relation, endpoints))
            })();
            match candidate {
                Ok((relation, endpoints)) => {
                    used = candidate_used;
                    out.endpoints.extend(endpoints);
                    out.relations.insert(key, relation);
                }
                Err(_) if !out.relations.is_empty() => break,
                Err(error) => {
                    return Err(StoreError::Data(format!(
                        "semantic relationship replay item cannot fit a 1 MiB metadata page: {error}"
                    )));
                }
            }
        }
        tx.commit().map_err(failure)?;
        Ok(out)
    }

    pub(super) fn semantic_next(&mut self, limit: usize) -> Result<Option<Batch>, StoreError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        if !current(&tx).map_err(failure)? {
            return Ok(None);
        }
        if !active(&tx).map_err(failure)? {
            if gc_pending(&tx).map_err(failure)? {
                if let Some(budget) = gc_page(&tx).map_err(failure)? {
                    tx.execute("DELETE FROM catalog_semantic_sources WHERE source_path IN (SELECT source_path FROM catalog_semantic_sources ORDER BY source_path LIMIT ?1)",[budget as i64]).map_err(failure)?;
                    let remains: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources)",
                            [],
                            |r| r.get(0),
                        )
                        .map_err(failure)?;
                    if !remains {
                        tx.execute(
                            native_metadata_delete(&tx).map_err(failure)?,
                            ["catalog_semantic_gc"],
                        )
                        .map_err(failure)?;
                    }
                }
                tx.commit().map_err(failure)?;
            }
            return Ok(None);
        }
        let (mut epoch, mut p, mut base, runtime): (i64, String, Option<String>, String) = tx
            .query_row(
                "SELECT epoch,phase,generation,runtime FROM catalog_semantic_stage",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(failure)?;
        if !recipe_current(&tx).map_err(failure)? {
            return Err(StoreError::Failed("unsupported semantic recipe".into()));
        }
        if generation(&tx).map_err(failure)? != base || !witnesses(&tx).map_err(failure)? {
            reset(&tx, &runtime).map_err(failure)?;
            record_recipe(&tx).map_err(failure)?;
            epoch = tx
                .query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r.get(0))
                .map_err(failure)?;
            p = "indexing".into();
            base = generation(&tx).map_err(failure)?;
        }
        let resetting: bool = tx
            .query_row("SELECT resetting FROM catalog_semantic_stage", [], |r| {
                r.get(0)
            })
            .map_err(failure)?;
        if resetting {
            reset_page(&tx).map_err(failure)?;
            let source_revision = revision(&tx).map_err(failure)?;
            tx.commit().map_err(failure)?;
            return Ok(Some(Batch {
                epoch,
                phase: Phase::Identity,
                paths: Vec::new(),
                base_generation: base,
                source_revision,
                runtime,
                context: Context::default(),
                relation_cursor: None,
            }));
        }
        tx.execute(
            "UPDATE catalog_semantic_stage SET revision=?1",
            [revision(&tx).map_err(failure)?],
        )
        .map_err(failure)?;
        let events_complete = event_page(&tx).map_err(failure)?;
        let frontier_complete = frontier_page(&tx).map_err(failure)? && events_complete;
        let limit = limit.clamp(1, 64);
        let mut paths = Vec::new();
        let mut publication_groups: Vec<Vec<String>> = Vec::new();
        let mut result_phase = Phase::Identity;
        for _ in 0..4 {
            let sql = match p.as_str() {
                "indexing" => {
                    "SELECT source_path FROM catalog_semantic_sources WHERE index_done=0 ORDER BY source_path LIMIT ?1"
                }
                "identity" => {
                    "SELECT source_path FROM catalog_semantic_sources WHERE identity_done=0 ORDER BY source_path LIMIT ?1"
                }
                "relations" => {
                    "SELECT source_path FROM catalog_semantic_sources WHERE relation_done=0 ORDER BY source_path LIMIT ?1"
                }
                _ => {
                    "SELECT source_path FROM catalog_semantic_owner_sources WHERE session_key=(SELECT session_key FROM catalog_semantic_endpoints WHERE published=0 ORDER BY session_key LIMIT 1) ORDER BY source_path LIMIT 65"
                }
            };
            if p == "publication" {
                let keys=tx.prepare("SELECT session_key FROM catalog_semantic_endpoints WHERE published=0 ORDER BY session_key LIMIT 64").map_err(failure)?.query_map([],|r|r.get::<_,String>(0)).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)?;
                let mut selected = BTreeSet::new();
                for key in keys {
                    let owner_paths=tx.prepare("SELECT source_path FROM catalog_semantic_owner_sources WHERE session_key=?1 ORDER BY source_path LIMIT 65").map_err(failure)?.query_map([key],|r|r.get::<_,String>(0)).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)?;
                    if owner_paths.len() > 64 {
                        return Err(StoreError::Data(
                            "semantic owner source budget exceeded".into(),
                        ));
                    }
                    let added = owner_paths
                        .iter()
                        .filter(|path| !selected.contains(*path))
                        .count();
                    if selected.len() + added > 64 {
                        break;
                    }
                    publication_groups.push(owner_paths.clone());
                    selected.extend(owner_paths);
                }
                paths = selected.into_iter().collect();
                if !paths.is_empty() {
                    result_phase = Phase::Publication;
                    break;
                }
            }
            let mut q = tx.prepare(sql).map_err(failure)?;
            paths = if p == "publication" {
                q.query_map([], |r| r.get::<_, String>(0))
                    .map_err(failure)?
                    .collect::<Result<_, _>>()
                    .map_err(failure)?
            } else {
                q.query_map(
                    [if p == "relations" {
                        limit.min(32) as i64
                    } else {
                        limit as i64
                    }],
                    |r| r.get::<_, String>(0),
                )
                .map_err(failure)?
                .collect::<Result<_, _>>()
                .map_err(failure)?
            };
            drop(q);
            if p == "identity" && !paths.is_empty() {
                let seeds = std::mem::take(&mut paths);
                let mut whole = BTreeSet::new();
                for seed in seeds {
                    if whole.contains(&seed) {
                        continue;
                    }
                    let group = owner_closure(&tx, &seed).map_err(failure)?;
                    if whole.len() + group.iter().filter(|path| !whole.contains(*path)).count() > 64
                    {
                        break;
                    }
                    whole.extend(group);
                }
                paths = whole.into_iter().collect();
            }
            if !paths.is_empty() {
                result_phase = match p.as_str() {
                    "indexing" => Phase::Indexing,
                    "identity" => Phase::Identity,
                    "relations" => Phase::Relations,
                    _ => Phase::Publication,
                };
                break;
            }
            if p == "publication" {
                paths=tx.prepare("SELECT source_path FROM catalog_semantic_sources WHERE publication_done=0 ORDER BY source_path LIMIT 64").map_err(failure)?.query_map([],|r|r.get::<_,String>(0)).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)?;
                if !paths.is_empty() {
                    result_phase = Phase::Publication;
                    break;
                }
            }
            if p == "indexing" {
                if frontier_complete && native_keys_page(&tx).map_err(failure)? {
                    p = "identity".into();
                } else {
                    let source_revision = revision(&tx).map_err(failure)?;
                    tx.commit().map_err(failure)?;
                    return Ok(Some(Batch {
                        epoch,
                        phase: Phase::Indexing,
                        paths: Vec::new(),
                        base_generation: base,
                        source_revision,
                        runtime,
                        context: Context::default(),
                        relation_cursor: None,
                    }));
                }
            } else if p == "identity" {
                if !frontier_complete {
                    let source_revision = revision(&tx).map_err(failure)?;
                    tx.commit().map_err(failure)?;
                    return Ok(Some(Batch {
                        epoch,
                        phase: Phase::Identity,
                        paths: Vec::new(),
                        base_generation: base,
                        source_revision,
                        runtime,
                        context: Context::default(),
                        relation_cursor: None,
                    }));
                }
                if !reduce(&tx).map_err(failure)? || !qualify_events(&tx).map_err(failure)? {
                    let source_revision = revision(&tx).map_err(failure)?;
                    tx.commit().map_err(failure)?;
                    return Ok(Some(Batch {
                        epoch,
                        phase: Phase::Identity,
                        paths: Vec::new(),
                        base_generation: base,
                        source_revision,
                        runtime,
                        context: Context::default(),
                        relation_cursor: None,
                    }));
                }
                p = "relations".into();
            } else if p == "relations" {
                p = "publication".into();
            } else {
                break;
            }
            tx.execute("UPDATE catalog_semantic_stage SET phase=?1", [&p])
                .map_err(failure)?;
        }
        if paths.len() > 64 {
            return Err(StoreError::Data(
                "semantic owner source budget exceeded".into(),
            ));
        }
        let claude = if result_phase == Phase::Relations && !paths.is_empty() {
            tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_owner_sources s JOIN catalog_semantic_endpoints e USING(session_key) WHERE s.source_path=?1 AND e.harness='claude')",[&paths[0]],|r|r.get::<_,bool>(0)).map_err(failure)?
        } else {
            false
        };
        if claude {
            paths.truncate(1);
        }
        let mut relation_cursor = None;
        let mut context = if matches!(result_phase, Phase::Identity | Phase::Indexing) {
            Context {
                epoch: (result_phase == Phase::Identity).then_some(epoch),
                ..Context::default()
            }
        } else {
            loop {
                match load_context(&tx, &paths, result_phase, epoch) {
                    Ok(context) => break context,
                    Err(_)
                        if result_phase == Phase::Publication && publication_groups.len() > 1 =>
                    {
                        publication_groups.pop();
                        paths = publication_groups
                            .iter()
                            .flatten()
                            .cloned()
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect();
                    }
                    Err(error) => return Err(failure(error)),
                }
            }
        };
        if claude {
            relation_cursor = Some(event_context(&tx, &paths[0], &mut context).map_err(failure)?);
        }
        let revision = revision(&tx).map_err(failure)?;
        tx.commit().map_err(failure)?;
        // Empty Publication is an explicit epoch-completion request.
        Ok(Some(Batch {
            epoch,
            phase: if paths.is_empty() {
                Phase::Publication
            } else {
                result_phase
            },
            paths,
            base_generation: base,
            source_revision: revision,
            runtime,
            context,
            relation_cursor,
        }))
    }
    pub(super) fn semantic_record(
        &mut self,
        b: &Batch,
        prepared: &crate::model::PreparedSources,
    ) -> Result<Outcome, StoreError> {
        self.semantic_rejection = None;
        if b.phase == Phase::Publication
            || b.paths.len() > if b.phase == Phase::Relations { 32 } else { 64 }
            || prepared.claims.len() > 64
        {
            return Err(StoreError::Data(
                "invalid semantic observation batch".into(),
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        if !current(&tx).map_err(failure)? {
            return Ok(Outcome::Stale);
        }
        if let Some(reason) = batch_rejection(&tx, b).map_err(failure)? {
            self.semantic_rejection = Some(reason);
            return Ok(Outcome::Conflict);
        }
        if prepared.base_generation != b.base_generation {
            self.semantic_rejection = Some("prepared_generation");
            return Ok(Outcome::Conflict);
        }
        for (path, expected) in &prepared.claims {
            if claim(&tx, path).map_err(failure)? != *expected {
                self.semantic_rejection = Some("prepared_claim");
                return Ok(Outcome::Conflict);
            }
        }
        for row in &prepared.rows {
            if row.sources.len() > 64 {
                self.semantic_rejection = Some("row_source_budget");
                return Ok(Outcome::Conflict);
            }
            if !selected_roles_current(&tx, &row.sources).map_err(failure)? {
                self.semantic_rejection = Some("row_source_role");
                return Ok(Outcome::Conflict);
            }
        }
        let matched_scope = if b.phase == Phase::Relations {
            Some(relation_scope(&tx, b, prepared).map_err(failure)?)
        } else {
            None
        };
        if b.phase == Phase::Indexing {
            if prepared.withdrawals.len() > 64 {
                return Err(StoreError::Data(
                    "semantic withdrawal source budget exceeded".into(),
                ));
            }
            let mut authority = withdrawal_authority(&tx).map_err(failure)?;
            let mut bytes = 0usize;
            let mut negative_paths = BTreeSet::new();
            for negative in &prepared.withdrawals {
                if !negative_paths.insert(&negative.path) || !b.paths.contains(&negative.path) {
                    self.semantic_rejection = Some("withdrawal_membership");
                    return Ok(Outcome::Conflict);
                }
                if negative.runtime != b.runtime {
                    self.semantic_rejection = Some("withdrawal_runtime");
                    return Ok(Outcome::Conflict);
                }
                if tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_owner_sources WHERE source_path=?1)",[&negative.path],|r|r.get::<_,bool>(0)).map_err(failure)? {
                    self.semantic_rejection = Some("withdrawal_owner");
                    return Ok(Outcome::Conflict);
                }
                if prepared
                    .indexed_sources
                    .iter()
                    .any(|(_, source)| source.path == Path::new(&negative.path))
                {
                    self.semantic_rejection = Some("withdrawal_indexed");
                    return Ok(Outcome::Conflict);
                }
                if !negative.current().unwrap_or(false) {
                    self.semantic_rejection = Some("withdrawal_proof");
                    return Ok(Outcome::Conflict);
                }
                if let Some(previous) = &authority {
                    if previous.facts_path != negative.facts_path
                        || previous.facts_identity != negative.facts_identity
                        || previous.runtime != negative.runtime
                    {
                        self.semantic_rejection = Some("withdrawal_authority");
                        return Ok(Outcome::Conflict);
                    }
                } else {
                    let encoded = json(negative).map_err(failure)?;
                    if encoded.len() > MAX_CONTEXT {
                        return Err(StoreError::Data(
                            "semantic withdrawal metadata budget exceeded".into(),
                        ));
                    }
                    tx.execute(
                        "UPDATE catalog_semantic_stage SET withdrawal_authority=?1",
                        [encoded],
                    )
                    .map_err(failure)?;
                    authority = Some(negative.clone());
                }
                let encoded = json(negative).map_err(failure)?;
                bytes = bytes.checked_add(encoded.len()).ok_or_else(|| {
                    StoreError::Data("semantic withdrawal metadata budget exceeded".into())
                })?;
                if bytes > MAX_CONTEXT {
                    return Err(StoreError::Data(
                        "semantic withdrawal metadata budget exceeded".into(),
                    ));
                }
                tx.execute(
                    "UPDATE catalog_semantic_sources SET withdrawal=?2 WHERE source_path=?1",
                    params![negative.path, encoded],
                )
                .map_err(failure)?;
            }
            tx.execute("UPDATE catalog_semantic_stage SET key_after_harness='',key_after_native='',keys_done=0,key_candidate='',key_epoch=key_epoch+1",[]).map_err(failure)?;
            for (harness, source) in &prepared.indexed_sources {
                if !selected_roles_current(&tx, std::slice::from_ref(source)).map_err(failure)? {
                    self.semantic_rejection = Some("indexed_source_role");
                    return Ok(Outcome::Conflict);
                }
                let encoded = json(source).map_err(failure)?;
                if encoded.len() > MAX_CONTEXT {
                    return Err(StoreError::Data(
                        "semantic source metadata budget exceeded".into(),
                    ));
                }
                let path = source.path.to_string_lossy();
                let top = source
                    .native_role
                    .as_ref()
                    .is_some_and(|role| role.get("Top").is_some());
                tx.execute("INSERT INTO catalog_semantic_inputs VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source_path) DO UPDATE SET harness=excluded.harness,native_id=excluded.native_id,is_top=excluded.is_top,metadata=excluded.metadata",params![path.as_ref(),harness,source.native_id,top,encoded]).map_err(failure)?;
                native_frontier(&tx, harness, &source.native_id).map_err(failure)?;
                if top {
                    let prefix = format!(
                        "{}/subagents/",
                        source.path.with_extension("").to_string_lossy()
                    );
                    queue(&tx, "claudechildren", &prefix, "").map_err(failure)?;
                }
                let old=tx.prepare("SELECT session_key FROM session_catalog_sources WHERE source_path=?1 AND lifecycle='current' ORDER BY session_key LIMIT 2").map_err(failure)?.query_map([path.as_ref()],|r|r.get::<_,String>(0)).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)?;
                for owner in old {
                    let needed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM session_catalog_sources own WHERE own.lifecycle='current' AND NOT EXISTS(SELECT 1 FROM catalog_semantic_sources stage WHERE stage.source_path=own.source_path) AND own.session_key IN (SELECT ?1 UNION SELECT parent_key FROM session_catalog WHERE session_key=?1 AND lifecycle='current' UNION SELECT session_key FROM session_catalog WHERE parent_key=?1 AND lifecycle='current' UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM session_trace_edges WHERE session_key=?1 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM session_trace_edges WHERE session_key=?1))",[&owner],|r|r.get(0)).map_err(failure)?;
                    if !needed {
                        continue;
                    }
                    for kind in ["members", "parent", "children", "edges", "from", "to"] {
                        queue(&tx, kind, &owner, "").map_err(failure)?;
                    }
                }
            }
            for (path, harness, id) in &prepared.indexed_native_parents {
                // Runtime invalidation must not introduce relationship-discovery
                // edges before the corresponding owner is published.
                tx.execute(
                    "INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,?2,?3)",
                    params![path, format!("runtime:native:{harness}"), id],
                )
                .map_err(failure)?;
                native_frontier(&tx, harness, id).map_err(failure)?;
            }
            for (harness, id) in &prepared.lineage_dependencies {
                let (facet, native_harness) = harness
                    .strip_prefix("bridge:")
                    .map(|h| ("bridge:", h))
                    .unwrap_or(("", harness.as_str()));
                if facet.is_empty() {
                    native_frontier(&tx, native_harness, id).map_err(failure)?;
                } else {
                    queue(&tx, "dependency", id, &format!("bridge:{native_harness}"))
                        .map_err(failure)?;
                }
            }
        } else if b.phase == Phase::Identity {
            // Every authoritative owner must have its complete physical lineage
            // in this batch. A newly learned continuation restarts discovery.
            for endpoint in prepared.relationships.endpoints.values() {
                if let Some(source) = endpoint.row.sources.first() {
                    let closed = owner_closure(&tx, source.path.to_string_lossy().as_ref())
                        .map_err(failure)?;
                    if closed.iter().any(|path| !b.paths.contains(path)) {
                        for path in closed {
                            enqueue(&tx, &path).map_err(failure)?;
                        }
                        reset(&tx, &b.runtime).map_err(failure)?;
                        tx.commit().map_err(failure)?;
                        return Ok(Outcome::Written);
                    }
                }
            }
            for endpoint in prepared.relationships.endpoints.values() {
                if endpoint.row.sources.is_empty() {
                    continue;
                }
                let encoded = json(endpoint).map_err(failure)?;
                if encoded.len() > MAX_CONTEXT {
                    return Err(StoreError::Data(
                        "semantic endpoint metadata budget exceeded".into(),
                    ));
                }
                let parents: BTreeSet<_> = endpoint.native_parents.iter().cloned().collect();
                if parents.len() > 1 {
                    return Err(StoreError::Data(
                        "conflicting semantic native parent".into(),
                    ));
                }
                let parent = parents.into_iter().next();
                let task = endpoint
                    .paths
                    .first()
                    .and_then(|path| path.rsplit('/').next())
                    .filter(|task| !task.is_empty())
                    .map(str::to_owned)
                    .or_else(|| endpoint.nickname.clone());
                tx.execute("INSERT INTO catalog_semantic_endpoints(session_key,harness,parent_native,parent_native_harness,parent_key,task,nickname,cost,metadata) VALUES(?1,?2,?3,?9,?4,?5,?6,?7,?8) ON CONFLICT(session_key) DO UPDATE SET metadata=excluded.metadata,parent_native=excluded.parent_native,parent_native_harness=excluded.parent_native_harness,parent_key=excluded.parent_key,task=excluded.task,nickname=excluded.nickname,cost=excluded.cost",params![endpoint.row.key,endpoint.row.harness,parent.as_ref().map(|p|&p.1),endpoint.parent,task,endpoint.nickname,endpoint.row.cost.usd,encoded,parent.as_ref().map(|p|&p.0)]).map_err(failure)?;
                for id in &endpoint.row.native_ids {
                    tx.execute(
                        "INSERT OR IGNORE INTO catalog_semantic_native VALUES(?1,?2,?3)",
                        params![endpoint.row.harness, id, endpoint.row.key],
                    )
                    .map_err(failure)?;
                    native_frontier(&tx, &endpoint.row.harness, id).map_err(failure)?;
                }
                for source in &endpoint.row.sources {
                    if let Some(pid) = endpoint.pid {
                        tx.execute("INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'runtime:pid',?2)", params![source.path.to_string_lossy().as_ref(),pid.to_string()]).map_err(failure)?;
                    }
                    if let Some(observations) = endpoint
                        .row
                        .query
                        .as_ref()
                        .and_then(|query| query["rows"].as_object())
                    {
                        for observation in observations.values() {
                            if let Some(pid) = observation["summary"]["pid"].as_u64() {
                                tx.execute("INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'runtime:pid',?2)", params![source.path.to_string_lossy().as_ref(),pid.to_string()]).map_err(failure)?;
                            }
                        }
                    }
                    tx.execute(
                        "INSERT OR IGNORE INTO catalog_semantic_owner_sources VALUES(?1,?2)",
                        params![endpoint.row.key, source.path.to_string_lossy().as_ref()],
                    )
                    .map_err(failure)?;
                }
                for path in &endpoint.paths {
                    tx.execute("INSERT OR IGNORE INTO catalog_semantic_paths(path,session_key) VALUES(?1,?2)",params![path,endpoint.row.key]).map_err(failure)?;
                }
                crate::model::relation_stage::native_name_hints_with(endpoint, || rusqlite::Error::InvalidQuery, |path, name| {
                    tx.execute("INSERT OR IGNORE INTO catalog_native_dependencies VALUES(?1,'name:claude',?2)", params![path,name])?;
                    Ok::<_, rusqlite::Error>(())
                }).map_err(failure)?;
                endpoint_frontier(&tx, endpoint).map_err(failure)?;
            }
            for endpoint in prepared.relationships.endpoints.values() {
                for name in &endpoint.names {
                    tx.execute(
                        "INSERT OR IGNORE INTO catalog_semantic_names VALUES(?1,?2)",
                        params![name, endpoint.row.key],
                    )
                    .map_err(failure)?;
                }
            }
            for (harness, id) in &prepared.lineage_dependencies {
                let native_harness = harness.strip_prefix("bridge:").unwrap_or(harness);
                native_frontier(&tx, native_harness, id).map_err(failure)?;
            }
            for (harness, id) in prepared.native_parents() {
                native_frontier(&tx, &harness, &id).map_err(failure)?;
            }
        } else {
            for endpoint in prepared.relationships.endpoints.values() {
                if !matched_scope
                    .as_ref()
                    .is_some_and(|scope| scope.owners.contains(&endpoint.row.key))
                {
                    continue;
                }
                let encoded = json(endpoint).map_err(failure)?;
                if encoded.len() > MAX_CONTEXT {
                    return Err(StoreError::Data(
                        "semantic endpoint metadata budget exceeded".into(),
                    ));
                }
                tx.execute(
                    "UPDATE catalog_semantic_endpoints SET metadata=?2 WHERE session_key=?1",
                    params![endpoint.row.key, encoded],
                )
                .map_err(failure)?;
            }
            for relation in prepared.relationships.relations.values() {
                if !matched_scope.as_ref().is_some_and(|scope| {
                    scope.owners.contains(&relation.from)
                        || relation
                            .to
                            .as_ref()
                            .is_some_and(|to| scope.owners.contains(to))
                }) {
                    continue;
                }
                let old: Option<String> = tx
                    .query_row(
                        "SELECT metadata FROM catalog_semantic_relations WHERE relation_id=?1",
                        [&relation.id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(failure)?;
                let mut merged = relation.clone();
                if let Some(old) = old {
                    if old.len() > MAX_CONTEXT {
                        return Err(StoreError::Data(
                            "semantic relation metadata budget exceeded".into(),
                        ));
                    }
                    let prior: Relation = serde_json::from_str(&old)
                        .map_err(|error| StoreError::Data(error.to_string()))?;
                    merged = merged
                        .merge_prior(&prior)
                        .map_err(|error| StoreError::Data(error.to_string()))?;
                }
                let encoded = json(&merged).map_err(failure)?;
                if encoded.len() > MAX_CONTEXT {
                    return Err(StoreError::Data(
                        "semantic relation metadata budget exceeded".into(),
                    ));
                }
                tx.execute(
                    "INSERT INTO catalog_semantic_relations VALUES(?1,?2,?3,?4) ON CONFLICT(relation_id) DO UPDATE SET metadata=excluded.metadata",
                    params![relation.id, relation.from, relation.to, encoded],
                )
                .map_err(failure)?;
            }
        }
        let cursor: Option<RelationCursor> = b
            .relation_cursor
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| StoreError::Data(error.to_string()))?;
        if let Some(cursor) = &cursor {
            let actual: String = tx
                .query_row(
                    "SELECT relation_after FROM catalog_semantic_sources WHERE source_path=?1",
                    [&cursor.path],
                    |r| r.get(0),
                )
                .map_err(failure)?;
            if b.paths.as_slice() != [cursor.path.clone()] || actual != cursor.after {
                self.semantic_rejection = Some("relation_cursor");
                return Ok(Outcome::Conflict);
            }
        }
        for path in &b.paths {
            let observed = prepared
                .claims
                .iter()
                .find(|(p, _)| p == path)
                .ok_or_else(|| StoreError::Data("unclaimed semantic source".into()))?;
            tx.execute(if b.phase==Phase::Indexing {"UPDATE catalog_semantic_sources SET claim=?2,ledger=?3,index_done=1 WHERE source_path=?1"}else if b.phase==Phase::Identity {"UPDATE catalog_semantic_sources SET claim=?2,ledger=?3,identity_done=1 WHERE source_path=?1"}else{"UPDATE catalog_semantic_sources SET claim=?2,ledger=?3,relation_done=1 WHERE source_path=?1"},params![path,observed.1,ledger(&tx,path).map_err(failure)?]).map_err(failure)?;
        }
        if b.phase == Phase::Indexing {
            for (_, source) in &prepared.indexed_sources {
                let path = source.path.to_string_lossy();
                if ledger_row(&tx, path.as_ref())
                    .map_err(failure)?
                    .is_some_and(|(_, ledger)| {
                        ledger.stat.size < source.size
                            || read_append_evidence(&tx, path.as_ref())
                                .ok()
                                .flatten()
                                .is_some_and(|e| e.after.size > ledger.stat.size)
                    })
                {
                    tx.execute(
                        "UPDATE catalog_semantic_sources SET index_done=0 WHERE source_path=?1",
                        [path.as_ref()],
                    )
                    .map_err(failure)?;
                }
            }
        }
        if let Some(cursor) = &cursor {
            let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_events WHERE source_path=?1 AND event_key>?2 AND copied=0 AND qualified=1)",params![cursor.path,cursor.end],|r|r.get(0)).map_err(failure)?;
            tx.execute("UPDATE catalog_semantic_sources SET relation_after=?2,relation_done=?3 WHERE source_path=?1",params![cursor.path,cursor.end,!pending]).map_err(failure)?;
        }
        if let Some(scope) = &matched_scope {
            for (path, end) in &scope.companions {
                let (_, observed) = prepared
                    .claims
                    .iter()
                    .find(|(selected, _)| selected == path)
                    .ok_or_else(|| StoreError::Data("unclaimed semantic companion".into()))?;
                tx.execute("UPDATE catalog_semantic_sources SET relation_after=?2,relation_done=1,claim=?3,ledger=?4 WHERE source_path=?1",params![path,end,observed,ledger(&tx,path).map_err(failure)?]).map_err(failure)?;
            }
        }
        for path in &b.paths {
            tx.execute("UPDATE catalog_semantic_sources SET (dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256)=(SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM files WHERE path=?1) WHERE source_path=?1",[path]).map_err(failure)?;
        }
        tx.execute(
            "UPDATE catalog_semantic_stage SET revision=?1",
            [revision(&tx).map_err(failure)?],
        )
        .map_err(failure)?;
        if let Some(reason) = witness_rejection(&tx).map_err(failure)? {
            self.semantic_rejection = Some(reason);
            return Ok(Outcome::Conflict);
        }
        tx.commit().map_err(failure)?;
        Ok(Outcome::Written)
    }
    pub(super) fn semantic_finish(&mut self, epoch: i64) -> Result<Outcome, StoreError> {
        self.semantic_rejection = None;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failure)?;
        if !current(&tx).map_err(failure)? {
            return Ok(Outcome::Stale);
        }
        let actual:Option<(i64,Option<String>,String)>=tx.query_row("SELECT epoch,generation,runtime FROM catalog_semantic_stage WHERE phase='publication' AND resetting=0 AND keys_done=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(failure)?;
        let Some((actual, expected, runtime)) = actual else {
            self.semantic_rejection = Some("completion_phase");
            return Ok(Outcome::Conflict);
        };
        let unfinished:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources WHERE index_done=0 OR events_done=0 OR identity_done=0 OR relation_done=0 OR publication_done=0) OR EXISTS(SELECT 1 FROM catalog_semantic_endpoints WHERE published=0 OR parent_resolved=0 OR root_key IS NULL OR cost_done=0) OR EXISTS(SELECT 1 FROM catalog_semantic_frontier WHERE done=0)",[],|r|r.get(0)).map_err(failure)?;
        if actual != epoch {
            self.semantic_rejection = Some("stage_epoch");
            return Ok(Outcome::Conflict);
        }
        if unfinished {
            self.semantic_rejection = Some("completion_backlog");
            return Ok(Outcome::Conflict);
        }
        if generation(&tx).map_err(failure)? != expected {
            self.semantic_rejection = Some("catalog_generation");
            return Ok(Outcome::Conflict);
        }
        if let Some(reason) = witness_rejection(&tx).map_err(failure)? {
            self.semantic_rejection = Some(reason);
            return Ok(Outcome::Conflict);
        }
        let sources=tx.prepare("SELECT source_path,claim,ledger FROM catalog_semantic_sources WHERE ack_done=0 ORDER BY source_path LIMIT 64").map_err(failure)?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,Option<String>>(2)?))).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)?;
        for (path, observed, recorded) in sources {
            if claim(&tx, &path).map_err(failure)? != observed {
                self.semantic_rejection = Some("completion_claim");
                return Ok(Outcome::Conflict);
            }
            if ledger(&tx, &path).map_err(failure)? != recorded {
                self.semantic_rejection = Some("completion_ledger");
                return Ok(Outcome::Conflict);
            }
            let captured: Option<Ledger> = recorded
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|error| StoreError::Data(error.to_string()))?;
            let negative = withdrawal(&tx, &path).map_err(failure)?;
            let withdrawn = negative.as_ref().is_some_and(|w| {
                w.path == path && w.runtime == runtime && w.current().unwrap_or(false)
            });
            let physical = match (Stat::of(Path::new(&path)), captured.as_ref()) {
                (Ok(stat), Some(expected)) => stat == expected.stat,
                (Ok(stat), None) => withdrawn && negative.as_ref().is_some_and(|w| w.stat == stat),
                (Err(error), Some(expected)) if error.kind() == io::ErrorKind::NotFound => {
                    archived_index(&tx, &path)
                        .map_err(failure)?
                        .is_some_and(|proof| proof.matches(expected))
                }
                (Err(error), None) if error.kind() == io::ErrorKind::NotFound => true,
                _ => false,
            };
            let typed:Option<Option<String>>=tx.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_inputs WHERE source_path=?1",[&path],|r|r.get(0)).optional().map_err(failure)?;
            let roles = if let Some(text) = typed {
                let source: crate::model::summary::CatalogSource =
                    bounded(text, &mut 0).map_err(failure)?;
                selected_roles_current(&tx, std::slice::from_ref(&source)).map_err(failure)?
            } else {
                withdrawn || captured.is_none() || tx.query_row("SELECT index_done=1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_owner_sources owner WHERE owner.source_path=catalog_semantic_sources.source_path) FROM catalog_semantic_sources WHERE source_path=?1",[&path],|r|r.get::<_,bool>(0)).map_err(failure)?
            };
            if !physical || !roles {
                invalidate_catalog_source(&tx, &path, "changed").map_err(failure)?;
                reset(&tx, &runtime).map_err(failure)?;
                tx.commit().map_err(failure)?;
                return Ok(Outcome::Written);
            }
            if let Some(revision) = observed {
                tx.execute("DELETE FROM session_catalog_invalidations WHERE source_path=?1 AND revision=?2",params![path,revision]).map_err(failure)?;
            }
            relocation::acknowledge(&tx, &path).map_err(failure)?;
            tx.execute(
                "UPDATE catalog_semantic_sources SET claim=NULL,ack_done=1 WHERE source_path=?1",
                [path],
            )
            .map_err(failure)?;
        }
        tx.execute(
            "UPDATE catalog_semantic_stage SET revision=?1",
            [revision(&tx).map_err(failure)?],
        )
        .map_err(failure)?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM catalog_semantic_sources WHERE ack_done=0)",
                [],
                |r| r.get(0),
            )
            .map_err(failure)?;
        if let Some(reason) = witness_rejection(&tx).map_err(failure)? {
            self.semantic_rejection = Some(reason);
            return Ok(Outcome::Conflict);
        }
        if !pending {
            // The pointer is the only authority for staged observations. Its
            // retirement makes old normalized records unreachable; bounded GC
            // reclaims them independently of current projection completeness.
            tx.execute("DELETE FROM catalog_semantic_stage WHERE epoch=?1", [epoch])
                .map_err(failure)?;
            tx.execute(
                native_metadata_put(&tx).map_err(failure)?,
                params!["catalog_semantic_gc", "1"],
            )
            .map_err(failure)?;
            tx.execute(
                "INSERT OR IGNORE INTO meta(namespace,key,value) VALUES(semon_namespace(),'catalog_backfill_state','complete')",
                [],
            )
            .map_err(failure)?;
            tx.execute("UPDATE meta SET value='complete' WHERE namespace=semon_namespace() AND key='catalog_completeness' AND NOT EXISTS(SELECT 1 FROM session_catalog_invalidations) AND NOT EXISTS(SELECT 1 FROM meta WHERE namespace=semon_namespace() AND key='catalog_backfill_state' AND value='discovering')",[]).map_err(failure)?;
        }
        tx.commit().map_err(failure)?;
        Ok(Outcome::Written)
    }
    pub(super) fn semantic_selection(
        &mut self,
        epoch: i64,
        selector: &Selector,
    ) -> Result<Selection, StoreError> {
        let actual: Option<i64> = self
            .connection
            .query_row("SELECT epoch FROM catalog_semantic_stage", [], |r| r.get(0))
            .optional()
            .map_err(failure)?;
        if actual != Some(epoch) {
            return Err(StoreError::Data("stale semantic selector".into()));
        }
        let mut lookup = OrdinaryLookup {
            db: &self.connection,
            replay: Vec::new(),
        };
        let result = crate::model::relation_stage::select_with(&mut lookup, selector);
        let replay = lookup.replay;
        if !replay.is_empty() {
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failure)?;
            let runtime: String = tx
                .query_row(
                    "SELECT runtime FROM catalog_semantic_stage WHERE epoch=?1",
                    [epoch],
                    |r| r.get(0),
                )
                .map_err(failure)?;
            crate::model::relation_stage::replay_members_with(
                &replay,
                || rusqlite::Error::InvalidQuery,
                |key| queue(&tx, "members", key, ""),
            )
            .map_err(failure)?;
            frontier_page(&tx).map_err(failure)?;
            reset(&tx, &runtime).map_err(failure)?;
            tx.commit().map_err(failure)?;
            return Err(StoreError::Data(
                "native name companion requires semantic replay".into(),
            ));
        }
        result.map_err(failure)
    }
}
struct OrdinaryLookup<'a> {
    db: &'a Connection,
    replay: Vec<String>,
}

impl crate::model::relation_stage::SelectorLookup for OrdinaryLookup<'_> {
    type Error = rusqlite::Error;
    fn invalid(&self, message: &'static str) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(message)))
    }
    fn native_key(
        &self,
        native: &str,
        harness: &str,
    ) -> rusqlite::Result<crate::model::relation_stage::Selection> {
        Ok(Selection {
            count: 1,
            key: Some(native_key(self.db, native, harness)?),
            ..Default::default()
        })
    }
    fn receipt_first(&self, id: &str) -> rusqlite::Result<Option<Option<String>>> {
        self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_events WHERE msg_id=?1 AND kind='xsm' AND copied=0 AND qualified=1 ORDER BY source_path,native_seq LIMIT 1", params![id], |r| r.get(0)).optional()
    }
    fn call_owners(&self, id: &str) -> rusqlite::Result<Vec<(String, String)>> {
        self.db.prepare("SELECT source_path,event_key FROM catalog_semantic_events WHERE kind='call' AND tool_id=?1 AND copied=0 AND qualified=1 ORDER BY source_path,event_key LIMIT 2")?.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    }
    fn event_metadata(&self, path: &str, key: &str) -> rusqlite::Result<Option<String>> {
        self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_events WHERE source_path=?1 AND event_key=?2", params![path, key], |r| r.get(0))
    }
    fn event_owner(&self, path: &str) -> rusqlite::Result<String> {
        self.db.query_row(
            "SELECT session_key FROM catalog_semantic_owner_sources WHERE source_path=?1",
            params![path],
            |r| r.get(0),
        )
    }
    fn endpoint_row(
        &self,
        key: &str,
    ) -> rusqlite::Result<crate::model::relation_stage::SelectorEndpointRow> {
        self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,parent_key,root_key FROM catalog_semantic_endpoints WHERE session_key=?1", params![key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
    }
    fn root_alive(&self, key: &str) -> rusqlite::Result<bool> {
        self.db.query_row("SELECT COALESCE(json_extract(metadata,'$.alive'),0) FROM catalog_semantic_endpoints WHERE session_key=?1", params![key], |r| r.get(0)).optional().map(|v| v.unwrap_or(false))
    }
    fn name_candidates(&self, name: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM (SELECT n.session_key FROM catalog_semantic_names n JOIN catalog_semantic_endpoints e USING(session_key) WHERE n.name=?1 AND e.harness='claude' AND json_extract(e.metadata,'$.row.kind')='session' UNION SELECT e.root_key FROM catalog_semantic_events sent JOIN catalog_semantic_events receipt ON receipt.msg_id=sent.msg_id JOIN catalog_semantic_endpoints e ON e.session_key=sent.owner JOIN catalog_semantic_endpoints root ON root.session_key=e.root_key WHERE sent.kind='send' AND receipt.kind='xsm' AND sent.copied=0 AND receipt.copied=0 AND sent.qualified=1 AND receipt.qualified=1 AND receipt.name=?1 AND (receipt.source_path,receipt.native_seq)=(SELECT first.source_path,first.native_seq FROM catalog_semantic_events first WHERE first.kind='xsm' AND first.msg_id=sent.msg_id AND first.copied=0 AND first.qualified=1 ORDER BY first.source_path,first.native_seq LIMIT 1) AND json_extract(root.metadata,'$.row.kind')='session') ORDER BY session_key LIMIT 2")?.query_map(params![name], |r| r.get(0))?.collect()
    }
    fn name_hints(&self, name: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT d.session_key FROM session_descriptions d JOIN session_catalog owner USING(session_key) WHERE owner.lifecycle='current' AND owner.harness='claude' AND json_valid(d.description) AND EXISTS(SELECT 1 FROM json_each(d.description,'$.names') WHERE value=?1) AND NOT EXISTS(SELECT 1 FROM catalog_semantic_endpoints stage WHERE stage.session_key=d.session_key) ORDER BY d.session_key LIMIT 64")?.query_map(params![name], |r| r.get(0))?.collect()
    }
    fn codex_sibling(&self, parent: &str, task: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM catalog_semantic_endpoints WHERE harness='codex' AND parent_native=?1 AND task=?2 LIMIT 2")?.query_map(params![parent, task], |r| r.get(0))?.collect()
    }
    fn codex_relay(&self, root: &str, target: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM catalog_semantic_endpoints WHERE harness='codex' AND root_key=?1 AND (nickname=?2 OR EXISTS(SELECT 1 FROM catalog_semantic_paths p WHERE p.session_key=catalog_semantic_endpoints.session_key AND p.root_key=?1 AND p.path=?2)) LIMIT 2")?.query_map(params![root, target], |r| r.get(0))?.collect()
    }
    fn codex_wait(&self, root: &str, target: &str, exclude: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM catalog_semantic_endpoints WHERE harness='codex' AND root_key=?1 AND session_key<>?3 AND (session_key=?2 OR EXISTS(SELECT 1 FROM catalog_semantic_paths p WHERE p.session_key=catalog_semantic_endpoints.session_key AND p.root_key=?1 AND p.path=?2)) LIMIT 2")?.query_map(params![root, target, exclude], |r| r.get(0))?.collect()
    }
    fn request_members_replay(&mut self, keys: Vec<String>) -> rusqlite::Result<()> {
        self.replay = keys;
        Ok(())
    }
}

/// Each owner's complete incident projection is certified before this source
/// acknowledgement. Later announced changes remain fenced by the retained
/// normalized ledger/claim witness across all previously acknowledged pages.
fn acknowledge_source(c: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let (observed, recorded): (Option<i64>, Option<String>) = c.query_row(
        "SELECT claim,ledger FROM catalog_semantic_sources WHERE source_path=?1",
        [path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if claim(c, path)? != observed || ledger(c, path)? != recorded {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let captured: Option<Ledger> = recorded
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let negative = withdrawal(c, path)?;
    let runtime: String = c.query_row("SELECT runtime FROM catalog_semantic_stage", [], |r| {
        r.get(0)
    })?;
    let withdrawn = negative
        .as_ref()
        .is_some_and(|w| w.path == path && w.runtime == runtime && w.current().unwrap_or(false));
    let physical = match (Stat::of(Path::new(path)), captured.as_ref()) {
        (Ok(stat), Some(ledger)) => stat == ledger.stat,
        (Ok(stat), None) => withdrawn && negative.as_ref().is_some_and(|w| w.stat == stat),
        (Err(e), Some(ledger)) if e.kind() == io::ErrorKind::NotFound => {
            archived_index(c, path)?.is_some_and(|proof| proof.matches(ledger))
        }
        (Err(e), None) if e.kind() == io::ErrorKind::NotFound => true,
        _ => false,
    };
    let typed:Option<Option<String>>=c.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM catalog_semantic_inputs WHERE source_path=?1",[path],|r|r.get(0)).optional()?;
    let roles = if let Some(text) = typed {
        let source: crate::model::summary::CatalogSource = bounded(text, &mut 0)?;
        selected_roles_current(c, std::slice::from_ref(&source))?
    } else {
        withdrawn || captured.is_none() || c.query_row("SELECT index_done=1 AND NOT EXISTS(SELECT 1 FROM catalog_semantic_owner_sources owner WHERE owner.source_path=catalog_semantic_sources.source_path) FROM catalog_semantic_sources WHERE source_path=?1",[path],|r|r.get::<_,bool>(0))?
    };
    if !physical || !roles {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if let Some(revision) = observed {
        c.execute(
            "DELETE FROM session_catalog_invalidations WHERE source_path=?1 AND revision=?2",
            params![path, revision],
        )?;
    }
    relocation::acknowledge(c, path)?;
    c.execute(
        "UPDATE catalog_semantic_sources SET claim=NULL,ack_done=1 WHERE source_path=?1",
        [path],
    )?;
    Ok(())
}
pub(super) fn published(
    tx: &Transaction<'_>,
    b: &Batch,
    rows: &[crate::model::summary::CatalogRow],
    generation: &str,
) -> rusqlite::Result<()> {
    if b.phase != Phase::Publication {
        return Err(rusqlite::Error::InvalidQuery);
    }
    for row in rows {
        tx.execute(
            "UPDATE catalog_semantic_endpoints SET published=1 WHERE session_key=?1",
            [&row.key],
        )?;
    }
    for path in &b.paths {
        tx.execute(
            "UPDATE catalog_semantic_sources SET publication_done=1 WHERE source_path=?1",
            [path],
        )?;
        acknowledge_source(tx, path)?;
    }
    tx.execute(
        "UPDATE catalog_semantic_stage SET generation=?1,revision=?2 WHERE epoch=?3",
        params![generation, revision(tx)?, b.epoch],
    )?;
    if !witnesses(tx)? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        source_observations::SourceObservationStore, sqlite::source_journal::SourceJournal,
    };

    fn header_repair_journal(store: &SqliteStore) -> SourceJournal {
        SourceJournal::owned(store.owner.clone())
    }

    fn header_repair_claim(store: &SqliteStore, path: &str) -> Option<i64> {
        let claims = header_repair_journal(store)
            .claims(&[path.to_owned()])
            .unwrap();
        claims.into_iter().next().unwrap().1
    }

    fn header_repair_invalidate(store: &SqliteStore, path: &str, reason: &str) {
        let journal = header_repair_journal(store);
        let mut connection = journal.connection().unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        journal.validate(&transaction).unwrap();
        crate::storage::sqlite::source_journal::invalidate(
            &transaction,
            journal.namespace(),
            path,
            reason,
        )
        .unwrap();
        transaction.commit().unwrap();
    }

    fn header_repair_catalog(name: &str) -> (PathBuf, SqliteStore, String) {
        let root = std::env::temp_dir().join(format!(
            "semon-header-repair-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.jsonl");
        fs::write(&source, b"").unwrap();
        let path = source.to_string_lossy().into_owned();
        let captured = Ledger {
            stat: Stat::of(&source).unwrap(),
            offset: 0,
            prefix: sha2::Sha256::digest(b"").into(),
            tail: sha2::Sha256::digest(b"").into(),
        };
        let mut store = crate::events::store::current_test_store(
            &root.join("index.sqlite3"),
            &crate::events::store::fixture_namespace(),
        );
        assert_eq!(
            store
                .write_one(&path, None, None, &captured, &FileIndex::default())
                .unwrap(),
            Outcome::Written
        );
        let committed = store.ledger(&path).unwrap().unwrap();
        assert_eq!(committed, captured);
        let source = committed.source(&source, "old");
        let row = crate::model::summary::CatalogRow {
            overview: None,
            trace: None,
            query: None,
            lifecycle: crate::model::summary::CatalogLifecycle::Current,
            key: "old".into(),
            name: "old".into(),
            harness: "codex".into(),
            kind: "session".into(),
            presentation_kind: None,
            native_ids: vec!["old".into()],
            parent: None,
            parent_source: None,
            repo: None,
            branch: None,
            model: String::new(),
            effort: None,
            start: None,
            last: Some(0),
            tokens: [0.0; 3],
            cost: crate::pricing::Cost::default(),
            sources: vec![source],
        };
        let projection = crate::slot_projection::Projection {
            key: "old".into(),
            slots: Vec::new(),
            repair: false,
        };
        assert_eq!(
            store
                .publish_catalog_with_generation(
                    &crate::slot_projection::Publication {
                        rows: std::slice::from_ref(&row),
                        transcripts: Some(std::slice::from_ref(&projection)),
                        observations: None,
                    },
                    None,
                    None,
                    false,
                    None,
                )
                .unwrap()
                .0,
            Outcome::Written
        );

        // Simulate only the stale header under the real admitted Stage owner.
        let config = store.owner.config().clone();
        store.connection = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        assert_eq!(
            store
                .connection
                .execute(
                    "UPDATE main.session_slot_projections SET generation='original',source_generation=NULL WHERE namespace=semon_namespace() AND session_key='old'",
                    [],
                )
                .unwrap(),
            1
        );

        header_repair_invalidate(&store, &path, "removed");
        let pending = header_repair_claim(&store, &path);
        // A pending repair preserves the published Current catalog. Readiness
        // follows its complete generation markers until a stage is active.
        assert!(store.qualified_catalog().unwrap());
        assert_eq!(header_repair_claim(&store, &path), pending);
        assert!(store.connection.query_row("SELECT lifecycle='current' AND EXISTS(SELECT 1 FROM main.session_catalog_sources WHERE namespace=semon_namespace() AND session_key='old' AND source_path=?1 AND lifecycle='current') FROM main.session_catalog WHERE namespace=semon_namespace() AND session_key='old'", [&path], |row| row.get::<_, bool>(0)).unwrap());
        (root, store, path)
    }

    fn header_repair_store(name: &str) -> (PathBuf, SqliteStore, String) {
        let (root, mut store, path) = header_repair_catalog(name);
        assert_eq!(
            store
                .semantic_begin(std::slice::from_ref(&path), "runtime")
                .unwrap(),
            Outcome::Written
        );
        // Install the same normalized indexed observation that semantic_record
        // captures. ACK below uses the actual source/ledger/physical fences.
        store.connection.execute("UPDATE main.catalog_semantic_sources SET index_done=1,claim=?2,ledger=?3,(dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256)=(SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM main.files WHERE namespace=semon_namespace() AND path=?1) WHERE namespace=semon_namespace() AND source_path=?1",params![path,header_repair_claim(&store,&path),ledger(&store.connection,&path).unwrap()]).unwrap();
        (root, store, path)
    }

    fn current_header_repair_store(
        name: &str,
    ) -> (
        PathBuf,
        SqliteStore,
        Box<dyn crate::events::IndexStore>,
        String,
        i64,
    ) {
        let (root, store, path) = header_repair_catalog(name);
        let config = store.owner.config().clone();
        let capability = crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap();
        let mut stage =
            crate::storage::sqlite::configured_index::open_semantic(capability).unwrap();
        assert_eq!(
            stage
                .begin_semantic_stage(std::slice::from_ref(&path), "runtime", false)
                .unwrap(),
            Outcome::Written
        );
        let batch = stage.semantic_batch(64).unwrap().unwrap();
        assert_eq!(batch.phase, Phase::Indexing);
        assert_eq!(batch.paths, vec![path.clone()]);
        let claims = header_repair_journal(&store).claims(&batch.paths).unwrap();
        let mut source = store
            .ledger(&path)
            .unwrap()
            .unwrap()
            .source(Path::new(&path), "old");
        // This diagnostic fixture has a deliberately empty raw file. A valid
        // primitive Codex role is sufficient for the real Stage source checks.
        source.native_role = Some(serde_json::json!({
            "Codex": { "subagent": false, "guardian_review": false }
        }));
        let prepared = crate::model::PreparedSources {
            base_generation: batch.base_generation.clone(),
            claims,
            rows: Vec::new(),
            transcripts: Vec::new(),
            pids: Default::default(),
            lineage_dependencies: Vec::new(),
            message_dependencies: None,
            relationships: Default::default(),
            indexed_sources: vec![("codex".into(), source)],
            removed_sources: Default::default(),
            indexed_native_parents: Vec::new(),
            withdrawals: Vec::new(),
            #[cfg(feature = "postgres")]
            withdrawal_fence: None,
        };
        assert_eq!(
            stage.record_semantic_batch(&batch, &prepared).unwrap(),
            Outcome::Written
        );
        (root, store, stage, path, batch.epoch)
    }

    fn acknowledge_current_header_repair(store: &SqliteStore, path: &str) {
        let claim = header_repair_claim(store, path).unwrap();
        let tx = store.connection.unchecked_transaction().unwrap();
        assert_eq!(
            tx.execute(
                "DELETE FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1 AND revision=?2",
                params![path, claim],
            )
            .unwrap(),
            1
        );
        assert_eq!(
            tx.execute(
                "UPDATE main.catalog_semantic_sources SET claim=NULL,publication_done=1,ack_done=1 WHERE namespace=semon_namespace() AND source_path=?1 AND claim=?2 AND index_done=1",
                params![path, claim],
            )
            .unwrap(),
            1
        );
        assert_eq!(
            tx.execute(
                "UPDATE main.catalog_semantic_stage SET phase='publication' WHERE namespace=semon_namespace() AND singleton=1 AND resetting=0",
                [],
            )
            .unwrap(),
            1
        );
        tx.commit().unwrap();
        assert_eq!(header_repair_claim(store, path), None);
    }

    fn diagnostic_batch(store: &SqliteStore, path: &str) -> Batch {
        store
            .connection
            .query_row(
                "SELECT epoch,revision,generation,runtime FROM main.catalog_semantic_stage WHERE namespace=semon_namespace() AND singleton=1",
                [],
                |r| {
                    Ok(Batch {
                        epoch: r.get(0)?,
                        source_revision: r.get(1)?,
                        base_generation: r.get(2)?,
                        runtime: r.get(3)?,
                        phase: Phase::Indexing,
                        paths: vec![path.to_owned()],
                        context: Context::default(),
                        relation_cursor: None,
                    })
                },
            )
            .unwrap()
    }

    fn empty_preparation(b: &Batch) -> crate::model::PreparedSources {
        crate::model::PreparedSources {
            base_generation: b.base_generation.clone(),
            claims: Vec::new(),
            rows: Vec::new(),
            transcripts: Vec::new(),
            pids: BTreeSet::new(),
            lineage_dependencies: Vec::new(),
            message_dependencies: None,
            relationships: Context::default(),
            indexed_sources: Vec::new(),
            removed_sources: BTreeSet::new(),
            indexed_native_parents: Vec::new(),
            withdrawals: Vec::new(),
            #[cfg(feature = "postgres")]
            withdrawal_fence: None,
        }
    }

    #[test]
    fn catalog_runtime_semantic_replay_pages_preserve_bounds_and_do_not_reserve_writer() {
        use crate::storage::sqlite::semantic::{SemanticStaging, Stage};
        let root = std::env::temp_dir().join(format!(
            "semon-replay-pages-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let options = crate::Options {
            cache: root.join("index.json"),
            claude_home: root.join("claude"),
            claude_json: root.join("claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            ..crate::Options::default()
        };
        fs::create_dir_all(&options.proc_root).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        let input = crate::Input {
            root: crate::InputRoot::Claude,
            path: "projects/project/owner.jsonl".into(),
        };
        let path = input.full_path(&options);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"type\":\"user\",\"uuid\":\"prompt\",\"sessionId\":\"owner\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"replay owner\"}}\n").unwrap();
        let mut cache =
            crate::events::EventCache::open_scoped(&options.cache, &options.source).unwrap();
        let prepared =
            crate::model::prepare_sources(&options, &mut cache, &[input], crate::model::now_ms())
                .unwrap();
        let endpoint = &prepared.relationships.endpoints["owner"];
        let config = options.source.config(&options.cache).unwrap();
        drop(cache);
        let mut stage = Stage::open(SemanticStaging::admit(&config).unwrap()).unwrap();
        assert_eq!(
            stage
                .begin(&[path.to_string_lossy().into_owned()], "runtime")
                .unwrap(),
            Outcome::Written
        );
        let db = Stage::open(SemanticStaging::admit(&config).unwrap())
            .unwrap()
            .into_test_connection();
        db.execute("UPDATE main.catalog_semantic_stage SET phase='publication' WHERE namespace=semon_namespace()", [])
            .unwrap();
        let epoch: i64 = db
            .query_row(
                "SELECT epoch FROM main.catalog_semantic_stage WHERE namespace=semon_namespace()",
                [],
                |r| r.get(0),
            )
            .unwrap();
        db.execute("INSERT INTO main.catalog_semantic_endpoints(namespace,session_key,harness,metadata) VALUES(semon_namespace(),'owner','claude',?1)", [json(endpoint).unwrap()]).unwrap();
        for n in 0..100 {
            let relation = Relation {
                id: format!("edge-{n:03}"),
                kind: "relay".into(),
                from: "owner".into(),
                source_turn: crate::model::relation_stage::OriginTurn::Known("turn".into()),
                to: None,
                target: Some("x".repeat(32 * 1024)),
                at: 0,
                done: None,
                status: "waiting".into(),
                has_result: false,
                unmatched: true,
                ambiguous: false,
                placements: Vec::new(),
                heads: Vec::new(),
                after: Vec::new(),
                previews: Default::default(),
                order: (2, 0, String::new(), n, 0),
                explicit_previews: false,
                stubs: Vec::new(),
                inline: false,
            };
            db.execute("INSERT INTO main.catalog_semantic_relations(namespace,relation_id,from_key,metadata) VALUES(semon_namespace(),?1,'owner',?2)",params![relation.id,json(&relation).unwrap()]).unwrap();
        }
        let writer = Stage::open(SemanticStaging::admit(&config).unwrap())
            .unwrap()
            .into_test_connection();
        writer.busy_timeout(std::time::Duration::ZERO).unwrap();
        writer
            .execute_batch(
                "BEGIN IMMEDIATE; INSERT OR REPLACE INTO main.meta(namespace,key,value) VALUES(semon_meta_scope('catalog_observed_at'),'catalog_observed_at','1')",
            )
            .unwrap();
        let mut after = String::new();
        let mut all = Vec::new();
        loop {
            let page = stage.replay(epoch, &["owner".into()], &after).unwrap();
            if page.relations.is_empty() {
                break;
            }
            assert!(
                page.relations.len() < 64,
                "byte bound must stop before the row bound"
            );
            assert!(
                page.relations
                    .values()
                    .map(|r| json(r).unwrap().len())
                    .sum::<usize>()
                    <= MAX_CONTEXT
            );
            after = page.relations.keys().next_back().unwrap().clone();
            all.extend(page.relations.into_keys());
        }
        assert_eq!(
            all,
            (0..100).map(|n| format!("edge-{n:03}")).collect::<Vec<_>>()
        );
        writer.execute_batch("ROLLBACK").unwrap();
        db.execute("UPDATE main.catalog_semantic_stage SET epoch=epoch+1 WHERE namespace=semon_namespace()", [])
            .unwrap();
        assert!(stage.replay(epoch, &["owner".into()], "").is_err());
        assert!(
            stage
                .replay(epoch + 1, &vec!["owner".into(); 65], "")
                .is_err()
        );
        drop(writer);
        drop(stage);
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn semantic_rejection_distinguishes_stale_batches_without_mutating_the_stage() {
        for reason in [
            "stage_epoch",
            "stage_phase",
            "stage_revision",
            "stage_generation",
            "stage_runtime",
        ] {
            let (root, mut store, path) = header_repair_store(reason);
            let original = diagnostic_batch(&store, &path);
            assert_eq!(batch_rejection(&store.connection, &original).unwrap(), None);
            let mut stale = original.clone();
            match reason {
                "stage_epoch" => stale.epoch += 1,
                "stage_phase" => stale.phase = Phase::Identity,
                "stage_revision" => stale.source_revision += 1,
                "stage_generation" => {
                    stale.base_generation = Some("synthetic-other-generation".into())
                }
                "stage_runtime" => stale.runtime = "synthetic-other-runtime".into(),
                _ => unreachable!(),
            }
            assert_eq!(
                store
                    .semantic_record(&stale, &empty_preparation(&stale))
                    .unwrap(),
                Outcome::Conflict
            );
            assert_eq!(store.semantic_rejection(), Some(reason));
            assert_eq!(batch_rejection(&store.connection, &original).unwrap(), None);
            assert_eq!(
                diagnostic_batch(&store, &path).source_revision,
                original.source_revision
            );
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn semantic_rejection_distinguishes_claims_ledgers_catalog_generation_and_sqlite_busy() {
        for reason in [
            "prepared_claim",
            "witness_claim",
            "witness_ledger",
            "catalog_generation",
        ] {
            let (root, mut store, path) = header_repair_store(reason);
            let b = diagnostic_batch(&store, &path);
            let mut prepared = empty_preparation(&b);
            match reason {
                "prepared_claim" => prepared.claims.push((path.clone(), Some(i64::MAX))),
                "witness_claim" => {
                    header_repair_invalidate(&store, &path, "changed");
                }
                "witness_ledger" => {
                    let observed_revision = revision(&store.connection).unwrap();
                    let staged_claim = header_repair_claim(&store, &path);
                    let original_ledger = store.ledger(&path).unwrap().unwrap();
                    store
                        .connection
                        .execute(
                            "UPDATE main.files SET size=size+1 WHERE namespace=semon_namespace() AND path=?1",
                            [&path],
                        )
                        .unwrap();
                    assert_ne!(store.ledger(&path).unwrap().unwrap(), original_ledger);
                    header_repair_invalidate(&store, &format!("{path}.unrelated"), "changed");
                    assert_eq!(revision(&store.connection).unwrap(), observed_revision + 1);
                    assert_eq!(header_repair_claim(&store, &path), staged_claim);
                }
                "catalog_generation" => {
                    assert_eq!(
                        store
                            .connection
                            .execute(
                                "UPDATE main.meta SET value=?1 WHERE namespace=semon_namespace() AND key='catalog_generation'",
                                ["synthetic-other-generation"],
                            )
                            .unwrap(),
                        1
                    );
                }
                _ => unreachable!(),
            }
            assert_eq!(
                store.semantic_record(&b, &prepared).unwrap(),
                Outcome::Conflict
            );
            assert_eq!(store.semantic_rejection(), Some(reason));
            // A real write-lock conflict remains StoreError::Busy; it must not
            // be reported as the preceding semantic rejection after retry.
            store.connection.busy_timeout(Duration::ZERO).unwrap();
            let blocker = Connection::open(&store.path).unwrap();
            blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
            assert!(matches!(
                store.semantic_record(&b, &prepared),
                Err(StoreError::Busy(_))
            ));
            assert_eq!(store.semantic_rejection(), None);
            blocker.execute_batch("ROLLBACK").unwrap();
            drop(blocker);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn semantic_rejection_keeps_withdrawal_authority_fenced_across_a_facts_rewrite() {
        let (root, mut store, path) = header_repair_store("withdrawal-authority-diagnostic");
        let b = diagnostic_batch(&store, &path);
        let facts_path = root.join("facts.json");
        let mut facts = crate::Facts {
            version: crate::FACTS_VERSION,
            codex_rollouts: Some(BTreeSet::new()),
            ..Default::default()
        };
        crate::write_facts(&facts_path, &facts, &crate::Options::default().source).unwrap();
        let authority = Withdrawal {
            path: path.clone(),
            relative: "sessions/rollout-synthetic.jsonl".into(),
            stat: Stat::of(Path::new(&path)).unwrap(),
            facts_path: facts_path.clone(),
            facts_identity: crate::facts::source_authority::Identity::of(
                &fs::symlink_metadata(&facts_path).unwrap(),
            )
            .unwrap(),
            runtime: b.runtime.clone(),
            authority: None,
        };
        assert!(authority.authority_current().unwrap());
        store
            .connection
            .execute(
                "UPDATE catalog_semantic_stage SET withdrawal_authority=?1",
                [json(&authority).unwrap()],
            )
            .unwrap();
        facts.hostname = "synthetic-new-hostname".into();
        crate::write_facts(&facts_path, &facts, &crate::Options::default().source).unwrap();
        assert_eq!(
            store.semantic_record(&b, &empty_preparation(&b)).unwrap(),
            Outcome::Conflict
        );
        assert_eq!(store.semantic_rejection(), Some("withdrawal_authority"));
        assert!(header_repair_claim(&store, &path).is_some());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsupported_publication_recipe_refuses_without_erasing_history() {
        let root = std::env::temp_dir().join(format!(
            "semon-unsupported-recipe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = crate::Options {
            cache: root.join("index.json"),
            claude_home: root.join("claude"),
            claude_json: root.join("claude.json"),
            codex_home: root.join("codex"),
            copilot_home: root.join("copilot"),
            proc_root: root.join("proc"),
            ..crate::Options::default()
        };
        let source = options.claude_home.join("projects/project/old.jsonl");
        fs::create_dir_all(&options.proc_root).unwrap();
        fs::write(options.proc_root.join("locks"), "").unwrap();
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "{\"type\":\"user\",\"uuid\":\"old-prompt\",\"sessionId\":\"old\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"retained native prompt\"}}\n").unwrap();
        let evidence = crate::native_evidence::NativeEvidence::new(options.clone());
        assert!((0..128).any(|_| {
            evidence
                .refresh_catalog(
                    &mut crate::viewer::lock(&evidence.work),
                    crate::model::now_ms(),
                )
                .unwrap()
        }));
        evidence.retire();
        let config = options.source.config(&options.cache).unwrap();
        use crate::storage::sqlite::semantic::{SemanticStaging, Stage};
        let connection = Stage::open(SemanticStaging::admit(&config).unwrap())
            .unwrap()
            .into_test_connection();
        let next = SemanticStaging::admit(&config).unwrap();
        assert!(connection.query_row("SELECT count(*)>0 FROM main.session_slots WHERE namespace=semon_namespace() AND session_key='old'", [], |row| row.get::<_, bool>(0)).unwrap());
        // The scoped semantic profile is authored by this producer. Global
        // format metadata remains immutable under the ordinary writer guards.
        connection
            .execute(
                "UPDATE main.storage_semantic_profiles SET recipe='unsupported' WHERE namespace=semon_namespace()",
                [],
            )
            .unwrap();
        let before = crate::native_timing::cleanup_tests::snapshot(&connection);
        assert!(Stage::open(next).is_err());
        assert_eq!(
            crate::native_timing::cleanup_tests::snapshot(&connection),
            before
        );
        drop(connection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_header_repair_cannot_borrow_another_namespace_stage() {
        let (root, store, mut stage, path, epoch) = current_header_repair_store("namespace");
        acknowledge_current_header_repair(&store, &path);
        assert!(owns_header_repair(&store.connection, &path).unwrap());
        let foreign = crate::storage::LogicalSourceNamespace::new("foreign-header-repair").unwrap();
        crate::storage::sqlite::catalog_namespace::register(&store.connection, &foreign).unwrap();
        assert!(!owns_header_repair(&store.connection, &path).unwrap());
        crate::storage::sqlite::catalog_namespace::register(
            &store.connection,
            &store.owner.config().namespace,
        )
        .unwrap();
        assert!(owns_header_repair(&store.connection, &path).unwrap());
        assert_eq!(
            stage.finish_semantic_stage(epoch).unwrap(),
            Outcome::Written
        );
        assert!(!owns_header_repair(&store.connection, &path).unwrap());
        drop(stage);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_header_repair_preserves_pending_and_live_ack_observations() {
        let (root, mut store, mut stage, path, old_epoch) = current_header_repair_store("ack");
        let observed = revision(&store.connection).unwrap();
        for _ in 0..3 {
            assert!(!store.qualified_catalog().unwrap());
            assert_eq!(revision(&store.connection).unwrap(), observed);
            assert_eq!(header_repair_claim(&store, &path), Some(observed));
            assert_eq!(
                store
                    .connection
                    .query_row(
                        "SELECT reason FROM main.session_catalog_invalidations WHERE namespace=semon_namespace() AND source_path=?1",
                        [&path],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                "removed"
            );
            assert!(owns_header_repair(&store.connection, &path).unwrap());
        }
        acknowledge_current_header_repair(&store, &path);
        for _ in 0..3 {
            assert!(!store.qualified_catalog().unwrap());
            assert_eq!(revision(&store.connection).unwrap(), observed);
            assert_eq!(header_repair_claim(&store, &path), None);
            assert!(owns_header_repair(&store.connection, &path).unwrap());
        }
        assert_eq!(
            stage.finish_semantic_stage(old_epoch).unwrap(),
            Outcome::Written
        );
        // Ordinary source observations remain independent of projection repair.
        header_repair_invalidate(&store, &path, "changed");
        assert_eq!(header_repair_claim(&store, &path), Some(observed + 1));
        assert_eq!(
            stage
                .begin_semantic_stage(std::slice::from_ref(&path), "runtime", false)
                .unwrap(),
            Outcome::Written
        );
        assert!(!store.qualified_catalog().unwrap());
        assert_eq!(revision(&store.connection).unwrap(), observed + 1);
        let replay = stage.semantic_batch(64).unwrap().unwrap();
        assert!(
            replay.epoch > old_epoch,
            "fresh observation did not start a new stage"
        );
        let header: (String, Option<String>, i64) = store.connection.query_row("SELECT generation,source_generation,version FROM main.session_slot_projections WHERE namespace=semon_namespace() AND session_key='old'",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(
            header,
            (
                "original".into(),
                None,
                i64::from(crate::slot_projection::VERSION)
            )
        );
        drop(stage);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_header_repair_rejects_each_changed_normalized_file_witness() {
        for assignment in [
            "dev=dev+1",
            "ino=ino+1",
            "size=size+1",
            "mtime_ns=zeroblob(16)",
            "ctime_ns=NULL",
            "resume_at=resume_at+1",
            "head_sha256=zeroblob(32)",
            "tail_sha256=zeroblob(32)",
        ] {
            let (root, store, stage, path, _) = current_header_repair_store("witness");
            acknowledge_current_header_repair(&store, &path);
            let observed = revision(&store.connection).unwrap();
            store
                .connection
                .execute(
                    &format!("UPDATE main.files SET {assignment} WHERE namespace=semon_namespace() AND path=?1"),
                    [&path],
                )
                .unwrap();
            assert!(
                !owns_header_repair(&store.connection, &path).unwrap(),
                "{assignment} was ignored"
            );
            assert!(!store.qualified_catalog().unwrap());
            assert_eq!(header_repair_claim(&store, &path), Some(observed + 1));
            assert!(!store.qualified_catalog().unwrap());
            assert_eq!(revision(&store.connection).unwrap(), observed + 1);
            drop(stage);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn current_header_repair_requires_live_indexed_stage_ownership() {
        for (transition, remaining) in [
            (
                "UPDATE main.catalog_semantic_sources SET index_done=0 WHERE namespace=semon_namespace()",
                1,
            ),
            (
                "UPDATE main.catalog_semantic_sources SET claim=123456 WHERE namespace=semon_namespace()",
                1,
            ),
            (
                "UPDATE main.catalog_semantic_stage SET resetting=1 WHERE namespace=semon_namespace() AND singleton=1",
                1,
            ),
            (
                "DELETE FROM main.catalog_semantic_sources WHERE namespace=semon_namespace()",
                0,
            ),
            (
                "DELETE FROM main.catalog_semantic_stage WHERE namespace=semon_namespace()",
                1,
            ),
        ] {
            let (root, store, stage, path, _) = current_header_repair_store("ownership");
            acknowledge_current_header_repair(&store, &path);
            let observed = revision(&store.connection).unwrap();
            store.connection.execute_batch(transition).unwrap();
            assert_eq!(
                store
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM main.catalog_semantic_sources WHERE namespace=semon_namespace() AND source_path=?1",
                        [&path],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                remaining
            );
            assert!(
                !owns_header_repair(&store.connection, &path).unwrap(),
                "{transition} lent repair ownership"
            );
            // Without any live stage, the newly pending repair leaves the
            // previously qualified Current Catalog visible until a stage is
            // begun. Every other transition leaves the active stage in place.
            let remains_qualified = transition
                == "DELETE FROM main.catalog_semantic_stage WHERE namespace=semon_namespace()";
            assert_eq!(store.qualified_catalog().unwrap(), remains_qualified);
            assert_eq!(header_repair_claim(&store, &path), Some(observed + 1));
            assert_eq!(store.qualified_catalog().unwrap(), remains_qualified);
            assert_eq!(revision(&store.connection).unwrap(), observed + 1);
            drop(stage);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn reserved_native_key_cursor_bounds_probes_and_selectors_reuse_qualified_result() {
        // This exercises the production WAL initialization contract. An
        // in-memory connection returns NoWal before any schema is installed.
        let root = std::env::temp_dir().join(format!(
            "semon-semantic-key-cursor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("index.sqlite3");
        let namespace =
            crate::storage::LogicalSourceNamespace::new("semantic-key-current").unwrap();
        // Keep the ordinary on-disk catalog setup, then reopen with the real
        // semantic owner so the fixture writes pass its connection-local guard.
        drop(super::super::current_search_test_connection(
            &path, &namespace,
        ));
        let config = crate::storage::StoreConfig {
            backend: crate::storage::StoreBackend::Sqlite {
                database: path.clone(),
            },
            namespace,
            expected_stream_epoch: Some("a".repeat(64)),
        };
        let connection = crate::storage::sqlite::semantic::Stage::open(
            crate::storage::sqlite::semantic::SemanticStaging::admit(&config).unwrap(),
        )
        .unwrap()
        .into_test_connection();
        connection.execute("INSERT INTO main.catalog_semantic_stage(namespace,singleton,epoch,phase,runtime,revision) VALUES(semon_namespace(),1,7,'indexing','runtime',0)",[]).unwrap();
        for harness in ["claude", "codex"] {
            connection
                .execute(
                    "INSERT INTO main.catalog_semantic_inputs VALUES(semon_namespace(),?1,?2,'same',1,'{}')",
                    params![format!("/{harness}/same.jsonl"), harness],
                )
                .unwrap();
        }
        for n in 0..80 {
            let native = format!("{}codex:same", ":".repeat(n));
            connection
                .execute(
                    "INSERT INTO main.catalog_semantic_inputs VALUES(semon_namespace(),?1,'codex',?2,1,'{}')",
                    params![format!("/codex/reserved-{n}.jsonl"), native],
                )
                .unwrap();
        }
        NATIVE_OCCUPANCY_PROBES.with(|count| count.set(0));
        assert!(!native_keys_page(&connection).unwrap());
        assert_eq!(NATIVE_OCCUPANCY_PROBES.with(|count| count.get()), 64);
        let pending: (bool, String) = connection
            .query_row(
                "SELECT keys_done,key_candidate FROM catalog_semantic_stage",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(!pending.0);
        assert!(!pending.1.is_empty(), "prefix progress was not durable");
        assert!(
            native_key(&connection, "same", "codex").is_err(),
            "unqualified cursor lent identity authority"
        );
        NATIVE_OCCUPANCY_PROBES.with(|count| count.set(0));
        assert!(native_keys_page(&connection).unwrap());
        assert_eq!(NATIVE_OCCUPANCY_PROBES.with(|count| count.get()), 18);
        NATIVE_OCCUPANCY_PROBES.with(|count| count.set(0));
        for _ in 0..100 {
            let mut lookup = OrdinaryLookup {
                db: &connection,
                replay: Vec::new(),
            };
            let result = crate::model::relation_stage::select_with(
                &mut lookup,
                &Selector::NativeKey {
                    native: "same".into(),
                    harness: "codex".into(),
                },
            )
            .unwrap();
            assert_eq!(result.key, Some(format!("{}codex:same", ":".repeat(80))));
            assert_eq!(
                native_key(&connection, "same", "claude").unwrap(),
                "claude:same"
            );
            assert_eq!(
                native_key(&connection, "codex:same", "codex").unwrap(),
                "codex:same"
            );
        }
        assert_eq!(
            NATIVE_OCCUPANCY_PROBES.with(|count| count.get()),
            0,
            "identity selectors replayed qualified prefix occupancy"
        );
        // New primitive evidence invalidates keys logically. Old rows remain
        // unreachable until overwritten or reclaimed by bounded GC.
        connection.execute("UPDATE main.catalog_semantic_stage SET key_epoch=key_epoch+1,key_after_harness='',key_after_native='',key_candidate='',keys_done=0",[]).unwrap();
        assert!(native_key(&connection, "same", "codex").is_err());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM catalog_semantic_keys", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(!native_keys_page(&connection).unwrap());
        assert!(native_keys_page(&connection).unwrap());
        assert_eq!(
            native_key(&connection, "same", "codex").unwrap(),
            format!("{}codex:same", ":".repeat(80))
        );
        drop(connection);
        fs::remove_dir_all(root).unwrap();
    }
}
