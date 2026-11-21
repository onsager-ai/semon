-- Origin-only metadata. Byte recipes remain subject to the Index's native
-- source and custody checks. Warehouse rows never authorize body access.
CREATE TABLE warehouse_source (
  source INTEGER PRIMARY KEY,
  origin TEXT NOT NULL, harness TEXT NOT NULL CHECK(harness IN ('claude','codex','copilot')),
  logical_source TEXT NOT NULL, incarnation INTEGER NOT NULL,
  carrier TEXT NOT NULL,
  ledger TEXT NOT NULL,
  UNIQUE(origin,harness,logical_source,incarnation)
) STRICT;
CREATE INDEX warehouse_source_logical ON warehouse_source(logical_source,harness,incarnation);
CREATE TABLE warehouse_current (
  path TEXT PRIMARY KEY, source INTEGER NOT NULL UNIQUE REFERENCES warehouse_source(source)
) STRICT, WITHOUT ROWID;
-- A certified current recipe can become the original retained binding once.
-- The owner and member IDs refer to immutable source incarnations, not live lineage.
CREATE TABLE warehouse_identity (
  id INTEGER PRIMARY KEY,
  scope TEXT NOT NULL CHECK(scope='retained_history'),
  source_key TEXT NOT NULL, catalog_key TEXT NOT NULL,
  retained INTEGER NOT NULL CHECK(retained IN (0,1)),
  owner INTEGER NOT NULL REFERENCES warehouse_source(source),
  harness TEXT NOT NULL, native_id TEXT NOT NULL,
  member_count INTEGER NOT NULL CHECK(member_count>0),
  recipe_version INTEGER NOT NULL, recipe_generation TEXT NOT NULL,
  recipe_source_generation TEXT NOT NULL, source_generation TEXT NOT NULL,
  complete INTEGER NOT NULL CHECK(complete IN (0,1)),
  UNIQUE(scope,source_key,catalog_key,retained)
) STRICT;
CREATE TABLE warehouse_identity_member (
  identity INTEGER NOT NULL REFERENCES warehouse_identity(id) ON DELETE CASCADE,
  source INTEGER NOT NULL REFERENCES warehouse_source(source),
  ordinal INTEGER NOT NULL, receipt TEXT NOT NULL,
  PRIMARY KEY(identity,ordinal), UNIQUE(identity,source)
) STRICT, WITHOUT ROWID;
CREATE INDEX warehouse_identity_member_source ON warehouse_identity_member(source,identity);
CREATE TABLE fact_session_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER,
  native_id TEXT NOT NULL, parent_id TEXT, nickname TEXT, agent_path TEXT, metadata TEXT,
  PRIMARY KEY(source,pos,block,kind,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_record (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, kind TEXT, uuid TEXT, message_id TEXT,
  parent_id TEXT, ts INTEGER, gap INTEGER NOT NULL,
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
CREATE TABLE fact_copilot_shutdown (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, event_id TEXT NOT NULL,
  ts INTEGER, complete INTEGER NOT NULL,
  PRIMARY KEY(source,pos,block,occurrence)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_copilot_usage (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  occurrence INTEGER NOT NULL, ordinal INTEGER, model TEXT NOT NULL,
  input INTEGER NOT NULL, output INTEGER NOT NULL, cached INTEGER NOT NULL,
  cache_write INTEGER NOT NULL, reasoning INTEGER NOT NULL,
  PRIMARY KEY(source,pos,block,occurrence,model)
) STRICT, WITHOUT ROWID;
CREATE TABLE fact_label_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, ordinal INTEGER, value TEXT,
  present INTEGER NOT NULL,
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
CREATE TABLE fact_eof_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, block INTEGER NOT NULL,
  kind TEXT NOT NULL, occurrence INTEGER NOT NULL, witness TEXT NOT NULL,
  parent_harness TEXT, parent_id TEXT, call_id TEXT, tool TEXT,
  PRIMARY KEY(source,witness,pos,block,kind,occurrence)
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
  source INTEGER PRIMARY KEY, pos INTEGER NOT NULL, sidecar_revision INTEGER NOT NULL, eof_witness TEXT
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

-- Shadow reads for parity. Existing Catalog and Viewer readers remain independent.
-- Reserve every physical native ID, including names of the form harness:id.
-- Materialize the owner/key sets once, rather than expanding lineage per use.
CREATE VIEW catalog_keys AS
WITH RECURSIVE owners AS MATERIALIZED (
  SELECT d.source AS owner,d.harness,d.native_id FROM dim_session d
  JOIN bridge_session_owner m ON m.source=d.source AND m.owner=d.source
), collisions AS (
  SELECT native_id FROM owners GROUP BY native_id HAVING count(DISTINCT harness)>1
), reserved AS MATERIALIZED (
  SELECT DISTINCT native_id FROM dim_session
), candidates(owner,harness,native_id,candidate) AS (
  SELECT o.owner,o.harness,o.native_id,
    CASE WHEN b.native_id IS NOT NULL THEN o.harness||':'||o.native_id ELSE o.native_id END
  FROM owners o LEFT JOIN collisions b USING(native_id)
  UNION ALL
  SELECT c.owner,c.harness,c.native_id,':'||c.candidate FROM candidates c
  JOIN reserved d ON d.native_id=c.candidate WHERE c.candidate<>c.native_id
)
SELECT c.owner,c.harness,c.native_id,c.candidate AS id FROM candidates c
LEFT JOIN reserved d ON d.native_id=c.candidate
WHERE c.candidate=c.native_id OR d.native_id IS NULL;

CREATE VIEW current_usage AS
WITH ranked AS (
  SELECT m.usage_owner AS owner,u.*,row_number() OVER (PARTITION BY m.usage_owner,u.message_id
    ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,u.pos DESC,u.block DESC,u.occurrence DESC) AS n
  FROM fact_step_observation u JOIN bridge_session_owner m USING(source)
  JOIN dim_session d USING(source)
  WHERE u.kind='claude' OR (u.kind='codex-response' AND u.accepted)
)
SELECT * FROM ranked WHERE n=1;

-- Sharing uses ALL UUID observations, not just the latest usage block.
CREATE VIEW shared_messages AS
WITH identities AS MATERIALIZED (
  SELECT DISTINCT m.usage_owner AS owner,u.message_id,u.uuid
  FROM fact_step_observation u JOIN bridge_session_owner m USING(source)
  WHERE u.kind='claude' AND u.uuid IS NOT NULL
), shared AS (
  SELECT message_id,uuid FROM identities GROUP BY message_id,uuid HAVING count(*)>1
)
SELECT DISTINCT i.owner,i.message_id FROM identities i JOIN shared s USING(message_id,uuid);
-- Select the whole latest shutdown before selecting models. An incomplete
-- shutdown, or a model omitted from a later snapshot, clears earlier usage.
CREATE VIEW current_copilot_usage AS
SELECT m.usage_owner AS owner,u.* FROM bridge_session_owner m
JOIN fact_copilot_usage u ON u.source=m.source
 AND u.pos=(SELECT max(s.pos) FROM fact_copilot_shutdown s WHERE s.source=m.source);
CREATE VIEW usage_totals AS
SELECT u.owner,
  sum(CASE WHEN s.owner IS NULL THEN u.input ELSE 0 END) AS input,
  sum(CASE WHEN s.owner IS NULL THEN u.cached ELSE 0 END) AS cached,
  sum(CASE WHEN s.owner IS NULL THEN u.output ELSE 0 END) AS output,
  sum(CASE WHEN s.owner IS NOT NULL THEN u.input ELSE 0 END) AS shared_input,
  sum(CASE WHEN s.owner IS NOT NULL THEN u.cached ELSE 0 END) AS shared_cached,
  sum(CASE WHEN s.owner IS NOT NULL THEN u.output ELSE 0 END) AS shared_output
FROM current_usage u LEFT JOIN shared_messages s
  ON s.owner=u.owner AND s.message_id=u.message_id GROUP BY u.owner
UNION ALL
SELECT owner,sum(input),sum(cached),sum(output),0,0,0
FROM current_copilot_usage GROUP BY owner;

CREATE VIEW session_list AS
WITH keys AS MATERIALIZED (SELECT * FROM catalog_keys),
 titles AS (
  SELECT m.owner,x.source,coalesce(x.title_pos,x.prompt_pos) AS pos,
    row_number() OVER (PARTITION BY m.owner
      ORDER BY x.first_ts DESC,x.continued,x.native_id DESC) AS n
  FROM bridge_session_owner m JOIN dim_session x USING(source)
  JOIN dim_session d ON d.source=m.owner
  WHERE x.title_pos IS NOT NULL OR
    (d.harness='codex' AND d.parent_id IS NOT NULL AND nullif(d.agent_path,'') IS NULL AND x.prompt_pos IS NOT NULL)
), models AS (
  SELECT m.owner,x.model,row_number() OVER (PARTITION BY m.owner
    ORDER BY x.first_ts DESC,x.continued,x.native_id DESC) AS n
  FROM bridge_session_owner m JOIN dim_session x USING(source) WHERE x.model IS NOT NULL
)
SELECT k.id,k.owner,k.harness,p.id AS parent,t.source AS title_source,t.pos AS title_pos,
  CASE WHEN k.harness='codex' AND d.parent_id IS NOT NULL THEN
    coalesce(nullif(d.agent_path,''),'Codex subagent')
    WHEN k.harness='codex' THEN coalesce(nullif(d.nickname,''),'Codex run')
    WHEN k.harness='copilot' THEN 'Copilot CLI'
    ELSE 'proj' END AS fallback_title,
  coalesce(model.model,CASE WHEN k.harness='codex' THEN 'codex' ELSE '—' END) AS model,
  CASE WHEN k.harness='codex' THEN 'off' ELSE 'done' END AS status,
  coalesce(u.input,0) AS input,coalesce(u.cached,0) AS cached,
  coalesce(u.output,0) AS output,coalesce(u.shared_input,0) AS shared_input,
  coalesce(u.shared_cached,0) AS shared_cached,coalesce(u.shared_output,0) AS shared_output
FROM keys k JOIN dim_session d ON d.source=k.owner
LEFT JOIN parent_selection chosen_parent ON chosen_parent.child=d.source
LEFT JOIN bridge_session_owner pm ON pm.source=chosen_parent.parent
LEFT JOIN keys p ON p.owner=pm.owner
LEFT JOIN titles t ON t.owner=k.owner AND t.n=1
LEFT JOIN models model ON model.owner=k.owner AND model.n=1
LEFT JOIN usage_totals u ON u.owner=k.owner;

CREATE VIEW session_records AS
WITH suppressed AS MATERIALIZED (
  SELECT DISTINCT target.source,old.uuid FROM fact_relation_observation e
  JOIN fact_record old ON old.source=e.source AND old.uuid IS NOT NULL
  JOIN dim_session target ON target.native_id=e.target AND target.harness='claude'
  JOIN dim_session active ON active.source=e.source
  WHERE active.lineage AND e.kind='continued-in'
)
SELECT m.owner,r.* FROM fact_record r JOIN bridge_session_owner m USING(source)
LEFT JOIN suppressed s ON s.source=r.source AND s.uuid=r.uuid WHERE s.source IS NULL;
CREATE VIEW tool_answers AS
WITH unique_calls AS (
  SELECT source,call_id FROM fact_tool_call GROUP BY source,call_id HAVING count(*)=1
)
SELECT c.source,c.pos,c.call_id,r.pos AS result_pos FROM fact_tool_call c
JOIN unique_calls u USING(source,call_id)
LEFT JOIN fact_tool_result r ON r.source=c.source AND r.call_id=c.call_id
  AND ((r.call_pos IS NULL AND r.kind<>'copilot-execution')
       OR (r.call_pos=c.pos AND r.call_block=c.block));

-- Precedence and cycle qualification cross lineage boundaries at read time.
CREATE VIEW parent_selection AS
WITH RECURSIVE calls AS MATERIALIZED (
  SELECT m.owner,c.call_id,min(c.pos) AS pos FROM fact_claude_call c
  JOIN bridge_session_owner m USING(source) JOIN dim_session d ON d.source=m.owner
  WHERE d.harness='claude' GROUP BY m.owner,c.call_id
), global_calls AS (
  SELECT call_id,min(owner) AS owner FROM calls GROUP BY call_id HAVING count(*)=1
), agent_parent AS (
  SELECT a.source AS child,coalesce(exact.owner,fallback.owner,storage.owner) AS parent,
    CASE WHEN exact.owner IS NOT NULL OR fallback.owner IS NOT NULL THEN a.call_id END AS call_id
  FROM fact_agent_observation a
  JOIN maintenance_cursor current ON current.source=a.source AND current.sidecar_revision=a.revision
  JOIN warehouse_current active ON active.source=a.source
  LEFT JOIN dim_session p ON p.harness='claude' AND p.native_id=a.storage_parent
  LEFT JOIN bridge_session_owner storage ON storage.source=p.source
  LEFT JOIN calls exact ON exact.owner=storage.owner AND exact.call_id=a.call_id
  LEFT JOIN global_calls fallback ON fallback.call_id=a.call_id
), native AS MATERIALIZED (
  SELECT d.source AS child,p.source AS parent,NULL AS call_id,'native' AS method,
    0 AS priority,0 AS ordinal
  FROM dim_session d JOIN dim_session p
    ON p.harness=d.harness AND p.native_id=d.parent_id
  UNION ALL
  SELECT child,parent,call_id,'native',0,0 FROM agent_parent WHERE parent IS NOT NULL
), evidence AS (
  SELECT e.source AS child,p.source AS parent,e.call_id,e.method,
    CASE e.method WHEN 'marker' THEN 3 WHEN 'environment' THEN 2 ELSE 1 END AS priority,
    e.pos AS ordinal
  FROM fact_parent_observation e JOIN dim_session p
    ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  JOIN warehouse_current current ON current.source=e.source
  WHERE e.source<>p.source
  UNION ALL
  SELECT e.source,p.source,e.call_id,'marker',3,e.pos
  FROM fact_eof_observation e JOIN dim_session p
    ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  JOIN warehouse_current active ON active.source=e.source
  JOIN maintenance_cursor cursor ON cursor.source=e.source AND cursor.eof_witness=e.witness
  WHERE e.kind='marker' AND e.source<>p.source
), qualified AS MATERIALIZED (
  SELECT capture AS epoch,source,pid,start FROM fact_process_observation
  JOIN warehouse_current USING(source)
), unique_pid AS (
  SELECT epoch,pid,min(source) AS source FROM qualified GROUP BY epoch,pid HAVING count(*)=1
), ancestors AS (
  -- Stop at the first known ancestor pid even when its session is ambiguous.
  SELECT a.capture AS epoch,a.source,a.ancestor_pid AS pid,
    row_number() OVER(PARTITION BY a.capture,a.source ORDER BY a.ordinal) AS n
  FROM fact_ancestor_observation a JOIN qualified child
    ON child.epoch=a.capture AND child.source=a.source AND child.pid=a.pid AND child.start=a.start
  JOIN (SELECT DISTINCT epoch,pid FROM qualified) known ON known.epoch=a.capture AND known.pid=a.ancestor_pid
), offered AS (
  SELECT * FROM native UNION ALL SELECT * FROM evidence
  UNION ALL
  SELECT e.source,p.source,e.call_id,'environment',2,e.capture
  FROM fact_environment_observation e JOIN qualified child
    ON child.epoch=e.capture AND child.source=e.source AND child.pid=e.pid AND child.start=e.start
  JOIN dim_session p ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  JOIN warehouse_current current ON current.source=e.source
  WHERE e.source<>p.source
  UNION ALL
  SELECT a.source,p.source,NULL,'ancestry',1,a.epoch
  FROM ancestors a JOIN unique_pid p USING(epoch,pid)
  LEFT JOIN native n ON n.child=a.source
  WHERE a.n=1 AND n.child IS NULL AND a.source<>p.source
), ranked AS (
  SELECT *,row_number() OVER(PARTITION BY child ORDER BY priority DESC,ordinal DESC) AS n
  FROM offered
), chosen AS MATERIALIZED (SELECT * FROM ranked WHERE n=1),
parent_chain(child,node) AS (
  SELECT child,parent FROM chosen WHERE method<>'native' UNION
  SELECT r.child,c.parent FROM parent_chain r JOIN chosen c ON c.child=r.node
), cyclic AS (SELECT DISTINCT child FROM parent_chain WHERE child=node)
SELECT c.child,c.parent,c.call_id,c.method FROM chosen c
LEFT JOIN cyclic bad USING(child)
-- The current builder revokes observed links on cycles, retaining a native link.
WHERE bad.child IS NULL OR c.method='native'
UNION ALL
SELECT n.child,n.parent,n.call_id,n.method FROM native n
JOIN chosen c USING(child) JOIN cyclic bad USING(child)
WHERE c.method<>'native';

CREATE VIEW relay_endpoints AS
WITH receipts AS MATERIALIZED (
  SELECT r.*,m.owner,row_number() OVER(PARTITION BY r.correlation
    ORDER BY d.native_id,r.pos) AS n
  FROM fact_relay_receipt r JOIN bridge_session_owner m USING(source)
  JOIN dim_session d ON d.source=m.owner
), sends AS MATERIALIZED (
  SELECT s.*,m.owner,r.correlation,r.failed FROM fact_relay_send s
  JOIN bridge_session_owner m USING(source)
  JOIN fact_relay_result r ON r.source=s.source AND r.call_id=s.call_id
    AND (r.call_pos=s.pos AND r.call_block=s.block OR r.call_pos IS NULL AND
      (SELECT count(*) FROM fact_relay_send other WHERE other.source=s.source AND other.call_id=s.call_id)=1)
), names AS (
  SELECT DISTINCT s.owner,r.name FROM sends s
  JOIN receipts r ON r.correlation=s.correlation AND r.n=1
  WHERE r.name<>''
), unique_names AS (
  SELECT name,min(owner) AS owner FROM names GROUP BY name HAVING count(*)=1
)
SELECT s.source,s.pos,s.call_id,s.owner AS sender,
  coalesce(r.owner,CASE WHEN s.failed=0 AND s.correlation IS NULL THEN n.owner END) AS receiver,
  CASE WHEN r.owner IS NULL AND s.failed=0 AND (s.correlation IS NOT NULL OR n.owner IS NULL)
    THEN 'name:'||s.target END AS stub,
  s.failed,s.correlation,s.ts
FROM sends s LEFT JOIN receipts r ON r.correlation=s.correlation AND r.n=1
LEFT JOIN unique_names n ON n.name=s.target;

-- Inclusive totals over current selected ownership/parenthood. UNION is a
-- visited set: an owner contributes once even when parent assertions cycle.
CREATE VIEW family_edges AS
SELECT DISTINCT child.owner AS child,parent.owner AS parent
FROM parent_selection p JOIN bridge_session_owner child ON child.source=p.child
JOIN bridge_session_owner parent ON parent.source=p.parent
WHERE child.owner<>parent.owner;

CREATE VIEW family_members AS
WITH RECURSIVE family(root,member) AS (
  SELECT owner,owner FROM bridge_session_owner GROUP BY owner
  UNION
  SELECT f.root,e.child FROM family f JOIN family_edges e ON e.parent=f.member
)
SELECT * FROM family;

CREATE VIEW family_usage AS
SELECT f.root,sum(coalesce(u.input,0)) AS input,
  sum(coalesce(u.cached,0)) AS cached,sum(coalesce(u.output,0)) AS output
FROM family_members f LEFT JOIN usage_totals u ON u.owner=f.member GROUP BY f.root;

-- analytics::families starts with catalog-key-ordered leaves, then enqueues a
-- parent after its last child. Thus queue order is subtree height followed by
-- the greatest key among deepest leaves. A parent retains its own observation.
-- otherwise its first child fold containing the evidence wins. Encode that
-- recursive precedence as a sibling-rank path (the empty path wins at root).
-- Cyclic families have no builder fold, so do not manufacture a shared winner.
CREATE VIEW family_fold_order AS
WITH RECURSIVE descendants(root,member,depth,path,cycle) AS (
 SELECT owner,owner,0,printf('/%d/',owner),0 FROM bridge_session_owner GROUP BY owner
 UNION ALL
 SELECT d.root,e.child,d.depth+1,d.path||e.child||'/',
   instr(d.path,printf('/%d/',e.child))>0
 FROM descendants d JOIN family_edges e ON e.parent=d.member WHERE NOT d.cycle
), heights AS (
 SELECT root,max(depth) AS height FROM descendants GROUP BY root
), priorities AS (
 SELECT d.root,h.height,max(k.id) AS last_leaf FROM descendants d
 JOIN heights h ON h.root=d.root AND h.height=d.depth
 JOIN catalog_keys k ON k.owner=d.member GROUP BY d.root
), siblings AS (
 SELECT e.parent,e.child,row_number() OVER (
   PARTITION BY e.parent ORDER BY p.height,p.last_leaf) AS n
 FROM family_edges e JOIN priorities p ON p.root=e.child
), precedence(root,member,priority) AS (
 SELECT owner,owner,'' FROM bridge_session_owner
 WHERE owner NOT IN (SELECT root FROM descendants WHERE cycle) GROUP BY owner
 UNION ALL
 SELECT p.root,s.child,p.priority||printf('%020d/',s.n)
 FROM precedence p JOIN siblings s ON s.parent=p.member
)
SELECT * FROM precedence;

-- Match claude_usage::shared: all UUID observations connect owners for a
-- message, transitively. The minimum shared UUID is its evidence identity.
CREATE VIEW shared_components AS
WITH RECURSIVE identities AS MATERIALIZED (
 SELECT DISTINCT m.owner,u.message_id,u.uuid FROM fact_step_observation u
 JOIN bridge_session_owner m USING(source) WHERE u.kind='claude' AND u.uuid IS NOT NULL
), edges AS MATERIALIZED (
 SELECT a.message_id,a.owner,b.owner AS other,a.uuid FROM identities a
 JOIN identities b ON b.message_id=a.message_id AND b.uuid=a.uuid AND b.owner<>a.owner
), connected(message_id,owner,member) AS (
 SELECT message_id,owner,other FROM edges UNION
 SELECT c.message_id,c.owner,e.other FROM connected c
 JOIN edges e ON e.message_id=c.message_id AND e.owner=c.member
)
SELECT c.message_id,c.owner,min(e.uuid) AS evidence_uuid FROM connected c
JOIN edges e ON e.message_id=c.message_id AND e.owner=c.member
GROUP BY c.message_id,c.owner;

CREATE VIEW family_shared_usage AS
WITH evidence AS (
 SELECT f.root,s.message_id,s.evidence_uuid,u.source,u.pos,u.block,u.kind,u.occurrence,u.input,u.cached,u.output,
 row_number() OVER(PARTITION BY f.root,s.message_id,s.evidence_uuid ORDER BY f.priority) AS n
 FROM family_fold_order f JOIN shared_components s ON s.owner=f.member
 JOIN current_usage u ON u.owner=s.owner AND u.message_id=s.message_id
)
-- Source/position selects the same native pricing input as the winning step.
-- token categories and cost must never be combined from different observations.
SELECT root,message_id,evidence_uuid,source,pos,block,kind,occurrence,input,cached,output FROM evidence WHERE n=1;
