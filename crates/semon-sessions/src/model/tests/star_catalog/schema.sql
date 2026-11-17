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
  prompt_pos INTEGER
);
CREATE TABLE index_advance (source INTEGER PRIMARY KEY, complete_bytes INTEGER NOT NULL);

-- Only natural-key-local facts are used in this Type 1 recompute.
CREATE VIEW session_recompute AS
SELECT s.*,
  (SELECT min(ts) FROM fact_record r WHERE r.source=s.source) AS first_ts,
  (SELECT max(ts) FROM fact_record r WHERE r.source=s.source) AS last_ts,
  (SELECT max(pos) FROM fact_record r WHERE r.source=s.source AND kind='custom-title') AS title_pos,
  (SELECT model FROM fact_step_observation u WHERE u.source=s.source
     AND model IS NOT NULL ORDER BY pos DESC LIMIT 1) AS model,
  (SELECT min(pos) FROM fact_record r WHERE r.source=s.source AND kind='prompt') AS prompt_pos
FROM fact_session_observation s;

CREATE VIEW lineage_edges AS
SELECT a.source AS src,b.source AS dst FROM fact_relation_observation r
JOIN dim_session a ON a.source=r.source AND a.harness='claude'
JOIN dim_session b ON b.native_id=r.target AND b.harness='claude'
WHERE r.kind IN ('continued-in','session_id');
CREATE VIEW membership AS
WITH RECURSIVE edges(src,dst) AS (
  SELECT src,dst FROM lineage_edges UNION SELECT dst,src FROM lineage_edges
), reach(source,node) AS (
  SELECT source,source FROM dim_session UNION
  SELECT r.source,e.dst FROM reach r JOIN edges e ON e.src=r.node
), ranked AS (
  SELECT r.source,r.node AS owner,
    row_number() OVER (PARTITION BY r.source ORDER BY
      d.first_ts IS NULL,d.first_ts,
      NOT EXISTS(SELECT 1 FROM fact_relation_observation x
        WHERE x.source=d.source AND x.kind='continued-in'),d.native_id) AS n
  FROM reach r JOIN dim_session d ON d.source=r.node
)
SELECT source,owner FROM ranked WHERE n=1;

-- Reserve every physical native ID, including names of the form harness:id.
CREATE VIEW catalog_keys AS
WITH RECURSIVE owners AS (
  SELECT DISTINCT m.owner,d.harness,d.native_id FROM membership m
  JOIN dim_session d ON d.source=m.owner
), candidates(owner,harness,native_id,candidate) AS (
  SELECT o.owner,o.harness,o.native_id,
    CASE WHEN EXISTS(SELECT 1 FROM owners b WHERE b.native_id=o.native_id
      AND b.harness<>o.harness) THEN o.harness||':'||o.native_id ELSE o.native_id END
  FROM owners o
  UNION ALL
  SELECT c.owner,c.harness,c.native_id,':'||c.candidate FROM candidates c
  WHERE c.candidate<>c.native_id AND EXISTS
    (SELECT 1 FROM dim_session d WHERE d.native_id=c.candidate)
)
SELECT owner,harness,native_id,candidate AS id FROM candidates c
WHERE c.candidate=c.native_id OR NOT EXISTS
  (SELECT 1 FROM dim_session d WHERE d.native_id=c.candidate);

CREATE VIEW current_usage AS
WITH ranked AS (
  SELECT m.owner,u.*,row_number() OVER (PARTITION BY m.owner,u.message_id
    ORDER BY d.first_ts DESC,
    EXISTS(SELECT 1 FROM fact_relation_observation e WHERE e.source=d.source
      AND e.kind='continued-in'),d.native_id DESC,u.pos DESC) AS n
  FROM fact_step_observation u JOIN membership m USING(source)
  JOIN dim_session d USING(source)
)
SELECT * FROM ranked WHERE n=1;

-- Sharing is based on ALL observed UUIDs, not only the latest usage block.
CREATE VIEW shared_messages AS
SELECT DISTINCT a.owner,u.message_id
FROM fact_step_observation u JOIN membership a ON a.source=u.source
JOIN fact_step_observation v ON v.message_id=u.message_id AND v.uuid=u.uuid
JOIN membership b ON b.source=v.source
WHERE u.uuid IS NOT NULL AND a.owner<>b.owner;
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
SELECT k.id,k.owner,k.harness,
  (SELECT p.id FROM dim_session native_parent
    JOIN membership pm ON pm.source=native_parent.source
    JOIN catalog_keys p ON p.owner=pm.owner
    WHERE native_parent.harness=d.harness AND native_parent.native_id=d.parent_id) AS parent,
  (SELECT x.source FROM membership m JOIN dim_session x USING(source)
    WHERE m.owner=k.owner AND (x.title_pos IS NOT NULL OR
      (k.harness='codex' AND d.parent_id IS NOT NULL AND nullif(d.agent_path,'') IS NULL AND x.prompt_pos IS NOT NULL))
    ORDER BY x.first_ts DESC,
      EXISTS(SELECT 1 FROM fact_relation_observation e WHERE e.source=x.source AND e.kind='continued-in'),
      x.native_id DESC LIMIT 1) AS title_source,
  (SELECT coalesce(x.title_pos,x.prompt_pos) FROM membership m JOIN dim_session x USING(source)
    WHERE m.owner=k.owner AND (x.title_pos IS NOT NULL OR
      (k.harness='codex' AND d.parent_id IS NOT NULL AND nullif(d.agent_path,'') IS NULL AND x.prompt_pos IS NOT NULL))
    ORDER BY x.first_ts DESC,
      EXISTS(SELECT 1 FROM fact_relation_observation e WHERE e.source=x.source AND e.kind='continued-in'),
      x.native_id DESC LIMIT 1) AS title_pos,
  CASE WHEN k.harness='codex' AND d.parent_id IS NOT NULL THEN
    coalesce(nullif(d.agent_path,''),'Codex subagent')
    WHEN k.harness='codex' THEN coalesce(nullif(d.nickname,''),'Codex run')
    ELSE 'proj' END AS fallback_title,
  coalesce((SELECT x.model FROM membership m JOIN dim_session x USING(source)
    WHERE m.owner=k.owner AND x.model IS NOT NULL
    ORDER BY x.first_ts DESC,
      EXISTS(SELECT 1 FROM fact_relation_observation e WHERE e.source=x.source AND e.kind='continued-in'),
      x.native_id DESC LIMIT 1),
    CASE WHEN k.harness='codex' THEN 'codex' ELSE '—' END) AS model,
  CASE WHEN k.harness='codex' THEN 'off' ELSE 'done' END AS status,
  coalesce(u.input,0) AS input,coalesce(u.cached,0) AS cached,
  coalesce(u.output,0) AS output,coalesce(u.shared_input,0) AS shared_input,
  coalesce(u.shared_cached,0) AS shared_cached,coalesce(u.shared_output,0) AS shared_output
FROM catalog_keys k JOIN dim_session d ON d.source=k.owner
LEFT JOIN usage_totals u ON u.owner=k.owner;

CREATE VIEW spawn_placement AS
WITH calls AS (
  SELECT source,task_name,min(pos) AS pos,min(ts) AS ts,count(*) AS n
  FROM fact_tool_call WHERE tool='spawn_agent' GROUP BY source,task_name
), children AS (
  SELECT parent_id,nickname,count(*) AS n FROM dim_session
  WHERE harness='codex' AND parent_id IS NOT NULL GROUP BY parent_id,nickname
)
SELECT child.native_id AS child,c.pos,c.ts FROM dim_session child
JOIN dim_session parent ON parent.native_id=child.parent_id AND parent.harness='codex'
JOIN children siblings ON siblings.parent_id=child.parent_id AND siblings.nickname=child.nickname
JOIN calls c ON c.source=parent.source AND c.task_name=child.nickname
WHERE child.harness='codex' AND siblings.n=1 AND c.n=1;

CREATE VIEW session_records AS
SELECT m.owner,r.* FROM fact_record r JOIN membership m USING(source)
WHERE NOT EXISTS (
 SELECT 1 FROM fact_relation_observation e JOIN fact_record old ON old.source=e.source
 JOIN dim_session target ON target.native_id=e.target AND target.harness='claude'
 WHERE e.kind='continued-in' AND target.source=r.source AND r.uuid IS NOT NULL AND old.uuid=r.uuid
);
CREATE VIEW tool_answers AS
SELECT c.source,c.pos,c.call_id,r.pos AS result_pos FROM fact_tool_call c
LEFT JOIN fact_tool_result r ON r.source=c.source AND r.call_id=c.call_id
WHERE (SELECT count(*) FROM fact_tool_call x
  WHERE x.source=c.source AND x.call_id=c.call_id)=1;
