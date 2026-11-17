-- Test-only origin metadata. No message, argument, result or title bodies.
PRAGMA foreign_keys = ON;
CREATE TABLE fact_session_observation (
  source INTEGER PRIMARY KEY, harness TEXT NOT NULL, native_id TEXT NOT NULL,
  parent_id TEXT, nickname TEXT, agent_path TEXT
);
CREATE TABLE fact_record (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, kind TEXT NOT NULL,
  uuid TEXT, message_id TEXT, ts INTEGER, PRIMARY KEY(source,pos)
);
CREATE TABLE fact_relation_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, kind TEXT NOT NULL,
  target TEXT NOT NULL, PRIMARY KEY(source,pos,kind,target)
);
CREATE TABLE fact_step_observation (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, message_id TEXT NOT NULL,
  uuid TEXT, model TEXT, input INTEGER NOT NULL, cached INTEGER NOT NULL,
  output INTEGER NOT NULL, PRIMARY KEY(source,pos)
);
CREATE TABLE fact_tool_call (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, call_id TEXT NOT NULL,
  tool TEXT NOT NULL, task_name TEXT, ts INTEGER,
  PRIMARY KEY(source,pos)
);
CREATE TABLE fact_tool_result (
  source INTEGER NOT NULL, pos INTEGER NOT NULL, call_id TEXT NOT NULL,
  ts INTEGER, PRIMARY KEY(source,pos)
);
CREATE TABLE dim_session (
  source INTEGER PRIMARY KEY, harness TEXT NOT NULL, native_id TEXT NOT NULL,
  parent_id TEXT, nickname TEXT, agent_path TEXT,
  first_ts INTEGER, last_ts INTEGER, title_pos INTEGER, model TEXT,
  prompt_pos INTEGER, continued INTEGER NOT NULL
);
CREATE TABLE index_advance (source INTEGER PRIMARY KEY, complete_bytes INTEGER NOT NULL);

-- Revisable answers are separate from the append-only observations.
CREATE TABLE dim_lineage (
  source INTEGER PRIMARY KEY, component INTEGER NOT NULL
);
CREATE INDEX lineage_component ON dim_lineage(component,source);
CREATE TABLE bridge_session_owner (
  source INTEGER PRIMARY KEY, owner INTEGER NOT NULL, usage_owner INTEGER NOT NULL
);
CREATE INDEX session_owner ON bridge_session_owner(owner,source);
CREATE TABLE bridge_spawn_placement (
  child TEXT PRIMARY KEY, parent_source INTEGER NOT NULL, pos INTEGER NOT NULL, ts INTEGER
);
CREATE INDEX spawn_parent ON bridge_spawn_placement(parent_source);
-- These indexes also bound component discovery during ingestion.
CREATE INDEX relation_endpoint ON fact_relation_observation(target,kind,source);
CREATE INDEX session_native ON dim_session(harness,native_id);
CREATE INDEX session_native_key ON dim_session(native_id,harness,source);
CREATE INDEX session_parent_name ON dim_session(harness,parent_id,nickname);
CREATE INDEX spawn_call_name ON fact_tool_call(source,tool,task_name);
CREATE INDEX record_uuid ON fact_record(source,uuid);
CREATE INDEX usage_message ON fact_step_observation(message_id,uuid,source);
CREATE INDEX tool_result_identity ON fact_tool_result(source,call_id);

-- Connection-local work sets, emptied before every bounded maintenance pass.
CREATE TEMP TABLE affected_lineage (source INTEGER PRIMARY KEY);
CREATE TEMP TABLE affected_spawn_parent (source INTEGER PRIMARY KEY);

CREATE VIEW membership AS SELECT source,owner FROM bridge_session_owner;

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
    ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,u.pos DESC) AS n
  FROM fact_step_observation u JOIN bridge_session_owner m USING(source)
  JOIN dim_session d USING(source)
)
SELECT * FROM ranked WHERE n=1;

-- Sharing uses ALL UUID observations, not just the latest usage block.
CREATE VIEW shared_messages AS
WITH identities AS MATERIALIZED (
  SELECT DISTINCT m.usage_owner AS owner,u.message_id,u.uuid
  FROM fact_step_observation u JOIN bridge_session_owner m USING(source)
  WHERE u.uuid IS NOT NULL
), shared AS (
  SELECT message_id,uuid FROM identities GROUP BY message_id,uuid HAVING count(*)>1
)
SELECT DISTINCT i.owner,i.message_id FROM identities i JOIN shared s USING(message_id,uuid);
CREATE VIEW usage_totals AS
SELECT u.owner,
  sum(CASE WHEN s.owner IS NULL THEN u.input ELSE 0 END) AS input,
  sum(CASE WHEN s.owner IS NULL THEN u.cached ELSE 0 END) AS cached,
  sum(CASE WHEN s.owner IS NULL THEN u.output ELSE 0 END) AS output,
  sum(CASE WHEN s.owner IS NOT NULL THEN u.input ELSE 0 END) AS shared_input,
  sum(CASE WHEN s.owner IS NOT NULL THEN u.cached ELSE 0 END) AS shared_cached,
  sum(CASE WHEN s.owner IS NOT NULL THEN u.output ELSE 0 END) AS shared_output
FROM current_usage u LEFT JOIN shared_messages s
  ON s.owner=u.owner AND s.message_id=u.message_id GROUP BY u.owner;

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
    ELSE 'proj' END AS fallback_title,
  coalesce(model.model,CASE WHEN k.harness='codex' THEN 'codex' ELSE '—' END) AS model,
  CASE WHEN k.harness='codex' THEN 'off' ELSE 'done' END AS status,
  coalesce(u.input,0) AS input,coalesce(u.cached,0) AS cached,
  coalesce(u.output,0) AS output,coalesce(u.shared_input,0) AS shared_input,
  coalesce(u.shared_cached,0) AS shared_cached,coalesce(u.shared_output,0) AS shared_output
FROM keys k JOIN dim_session d ON d.source=k.owner
LEFT JOIN dim_session native_parent ON native_parent.harness=d.harness AND native_parent.native_id=d.parent_id
LEFT JOIN bridge_session_owner pm ON pm.source=native_parent.source
LEFT JOIN keys p ON p.owner=pm.owner
LEFT JOIN titles t ON t.owner=k.owner AND t.n=1
LEFT JOIN models model ON model.owner=k.owner AND model.n=1
LEFT JOIN usage_totals u ON u.owner=k.owner;

CREATE VIEW spawn_placement AS SELECT child,pos,ts FROM bridge_spawn_placement;

CREATE VIEW session_records AS
WITH suppressed AS MATERIALIZED (
  SELECT DISTINCT target.source,old.uuid FROM fact_relation_observation e
  JOIN fact_record old ON old.source=e.source AND old.uuid IS NOT NULL
  JOIN dim_session target ON target.native_id=e.target AND target.harness='claude'
  WHERE e.kind='continued-in'
)
SELECT m.owner,r.* FROM fact_record r JOIN bridge_session_owner m USING(source)
LEFT JOIN suppressed s ON s.source=r.source AND s.uuid=r.uuid WHERE s.source IS NULL;
CREATE VIEW tool_answers AS
WITH unique_calls AS (
  SELECT source,call_id FROM fact_tool_call GROUP BY source,call_id HAVING count(*)=1
)
SELECT c.source,c.pos,c.call_id,r.pos AS result_pos FROM fact_tool_call c
JOIN unique_calls u USING(source,call_id)
LEFT JOIN fact_tool_result r ON r.source=c.source AND r.call_id=c.call_id;
