-- Origin-only metadata. Byte recipes remain subject to the Index's native
-- source and custody checks. Warehouse rows never authorize body access.
CREATE TABLE warehouse_source (
  source INTEGER PRIMARY KEY,
  origin TEXT NOT NULL, harness TEXT NOT NULL CHECK(harness IN ('claude','codex')),
  logical_source TEXT NOT NULL, incarnation INTEGER NOT NULL,
  carrier TEXT NOT NULL,
  ledger TEXT NOT NULL,
  UNIQUE(origin,harness,logical_source,incarnation)
) STRICT;
CREATE INDEX warehouse_source_logical ON warehouse_source(logical_source,harness,incarnation);
CREATE TABLE warehouse_current (
  path TEXT PRIMARY KEY, source INTEGER NOT NULL UNIQUE REFERENCES warehouse_source(source)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_session_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER,
  native_id TEXT NOT NULL, parent_id TEXT, nickname TEXT, agent_path TEXT, metadata TEXT,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_record (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, kind TEXT, uuid TEXT, message_id TEXT,
  ts INTEGER, gap INTEGER NOT NULL,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_relation_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER, target TEXT NOT NULL,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_step_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER,
  message_id TEXT, uuid TEXT, model TEXT, input INTEGER NOT NULL, cached INTEGER NOT NULL,
  output INTEGER NOT NULL, ts INTEGER, accepted INTEGER NOT NULL,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_step_billing (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER, billing TEXT NOT NULL,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_prompt (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER, ts INTEGER,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_tool_call (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, call_id TEXT, tool TEXT,
  task_name TEXT, ts INTEGER, flags INTEGER NOT NULL, target TEXT,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_tool_result (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER,
  call_id TEXT NOT NULL, call_pos INTEGER, call_block INTEGER, ts INTEGER,
  failed INTEGER NOT NULL, flags INTEGER NOT NULL, correlation TEXT,
  denial TEXT, exit_code INTEGER, target TEXT,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_relay_send (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, call_id TEXT, target TEXT NOT NULL, ts INTEGER,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_relay_result (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER,
  call_id TEXT NOT NULL, call_pos INTEGER, call_block INTEGER, correlation TEXT,
  ts INTEGER, failed INTEGER NOT NULL, flags INTEGER NOT NULL,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_relay_receipt (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, correlation TEXT, name TEXT,
  pid INTEGER, start TEXT, sock TEXT, ts INTEGER,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
-- Results are immutable observations at their own position in fact_tool_result.
CREATE TABLE fact_subagent_event (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER,
  ts INTEGER, id TEXT, name TEXT, flags INTEGER NOT NULL, target TEXT,
  peer_pid INTEGER, peer_start TEXT, peer_sock TEXT,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_claude_call (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, call_id TEXT NOT NULL, tool TEXT NOT NULL,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_agent_observation (
  source INTEGER NOT NULL, revision INTEGER NOT NULL, present INTEGER NOT NULL,
  storage_parent TEXT, call_id TEXT, model TEXT, cwd TEXT, branch TEXT,
  PRIMARY KEY(source,revision)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_parent_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  method TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER,
  parent_harness TEXT NOT NULL, parent_id TEXT NOT NULL, call_id TEXT,
  PRIMARY KEY(source,pos,block,method,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_process_observation (
  source INTEGER NOT NULL, capture INTEGER NOT NULL, pid INTEGER NOT NULL,
  start INTEGER NOT NULL, PRIMARY KEY(source,capture,pid,start)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_environment_observation (
  source INTEGER NOT NULL, capture INTEGER NOT NULL, pid INTEGER NOT NULL, start INTEGER NOT NULL,
  parent_harness TEXT NOT NULL, parent_id TEXT NOT NULL, call_id TEXT,
  PRIMARY KEY(source,capture,pid,start)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_ancestor_observation (
  source INTEGER NOT NULL, capture INTEGER NOT NULL, pid INTEGER NOT NULL, start INTEGER NOT NULL,
  ordinal INTEGER NOT NULL, ancestor_pid INTEGER NOT NULL,
  PRIMARY KEY(source,capture,pid,start,ordinal)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_compaction (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, ts INTEGER, trigger TEXT, pre_tokens INTEGER,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_codex_turn (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER,
  thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, phase TEXT NOT NULL,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE maintenance_cursor (
  source INTEGER PRIMARY KEY, pos INTEGER NOT NULL, sidecar_revision INTEGER NOT NULL
) STRICT;
CREATE TABLE dim_session (
  source INTEGER PRIMARY KEY, harness TEXT NOT NULL, native_id TEXT NOT NULL,
  parent_id TEXT, nickname TEXT, agent_path TEXT,
  first_ts INTEGER, last_ts INTEGER, title_pos INTEGER, model TEXT,
  model_pos INTEGER, prompt_pos INTEGER, continued INTEGER NOT NULL,
  lineage INTEGER NOT NULL
) STRICT;
CREATE TABLE lineage_group (
  id INTEGER PRIMARY KEY, weight INTEGER NOT NULL, minimum INTEGER NOT NULL UNIQUE,
  owner INTEGER NOT NULL
) STRICT;
CREATE TABLE lineage_member (source INTEGER PRIMARY KEY, component INTEGER NOT NULL) STRICT;
CREATE TABLE lineage_node (node TEXT PRIMARY KEY, component INTEGER NOT NULL) STRICT, WITHOUT ROWID;
CREATE TABLE bridge_spawn_placement (
  child TEXT PRIMARY KEY, parent_source INTEGER NOT NULL, pos INTEGER NOT NULL, ts INTEGER
) STRICT, WITHOUT ROWID;
CREATE INDEX lineage_component ON lineage_member(component,source);
CREATE INDEX lineage_node_component ON lineage_node(component,node);
CREATE INDEX spawn_parent ON bridge_spawn_placement(parent_source);
CREATE INDEX relation_endpoint ON fact_relation_observation(target,kind,source);
CREATE INDEX session_native ON dim_session(harness,native_id);
CREATE INDEX session_native_key ON dim_session(native_id,harness,source);
CREATE INDEX session_parent_name ON dim_session(harness,parent_id,nickname);
CREATE INDEX spawn_call_name ON fact_tool_call(source,tool,task_name);
CREATE INDEX record_uuid ON fact_record(source,uuid);
CREATE INDEX usage_message ON fact_step_observation(message_id,uuid,source);
CREATE INDEX tool_result_identity ON fact_tool_result(source,call_id);
CREATE INDEX relay_result_call ON fact_relay_result(source,call_id);
CREATE VIEW dim_lineage AS
SELECT m.source,g.minimum AS component FROM lineage_member m
JOIN lineage_group g ON g.id=m.component;
CREATE VIEW bridge_session_owner AS
SELECT m.source,g.owner,g.owner AS usage_owner FROM lineage_member m
JOIN lineage_group g ON g.id=m.component;
CREATE VIEW membership AS SELECT source,owner FROM bridge_session_owner;
CREATE VIEW spawn_placement AS SELECT child,pos,ts FROM bridge_spawn_placement;
-- Reuse native tree labels, fenced by the tree header's committed receipt.
CREATE VIEW fact_label AS
SELECT c.source,l.label,l.value FROM warehouse_current c
JOIN native_tree_headers h ON h.source_path=c.path
JOIN native_tree_labels l USING(projection_id)
WHERE l.label IN ('first','last','cwd','branch');
