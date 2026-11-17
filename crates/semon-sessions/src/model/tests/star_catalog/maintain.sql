-- The caller binds :source inside the same append/Index transaction.
INSERT OR REPLACE INTO dim_session
SELECT s.*,r.first_ts,r.last_ts,r.title_pos,m.model,r.prompt_pos,
  coalesce(c.continued,0)
FROM fact_session_observation s
LEFT JOIN (
  SELECT source,min(ts) AS first_ts,max(ts) AS last_ts,
    max(CASE WHEN kind='custom-title' THEN pos END) AS title_pos,
    min(CASE WHEN kind='prompt' THEN pos END) AS prompt_pos
  FROM fact_record WHERE source=:source GROUP BY source
) r USING(source)
LEFT JOIN (
  SELECT source,model FROM fact_step_observation
  WHERE source=:source AND model IS NOT NULL ORDER BY pos DESC LIMIT 1
) m USING(source)
LEFT JOIN (
  SELECT source,1 AS continued FROM fact_relation_observation
  WHERE source=:source AND kind='continued-in' GROUP BY source
) c USING(source)
WHERE s.source=:source;

DELETE FROM affected_lineage;
INSERT INTO affected_lineage
WITH RECURSIVE reach(source) AS (
  SELECT :source
  UNION
  SELECT b.source FROM reach r JOIN dim_session a ON a.source=r.source AND a.harness='claude'
  JOIN fact_relation_observation e ON e.source=a.source AND e.kind IN ('continued-in','session_id')
  JOIN dim_session b ON b.harness='claude' AND b.native_id=e.target
  UNION
  SELECT a.source FROM reach r JOIN dim_session b ON b.source=r.source AND b.harness='claude'
  JOIN fact_relation_observation e ON e.target=b.native_id AND e.kind IN ('continued-in','session_id')
  JOIN dim_session a ON a.source=e.source AND a.harness='claude'
)
SELECT source FROM reach;

DELETE FROM dim_lineage WHERE source IN (SELECT source FROM affected_lineage);
INSERT INTO dim_lineage
SELECT source,min(source) OVER () FROM affected_lineage;
DELETE FROM bridge_session_owner WHERE source IN (SELECT source FROM affected_lineage);
INSERT INTO bridge_session_owner
SELECT source,owner,owner FROM (
  SELECT d.source,first_value(d.source) OVER (
    ORDER BY d.first_ts IS NULL,d.first_ts,d.continued DESC,d.native_id
  ) AS owner
  FROM affected_lineage a JOIN dim_session d USING(source)
);

-- Spawn uniqueness is scoped to a parent's immediate children, not its ancestry.
-- A newly arrived child refreshes its parent group even if the parent is late.
DELETE FROM affected_spawn_parent;
INSERT INTO affected_spawn_parent SELECT :source
UNION SELECT p.source FROM dim_session child
JOIN dim_session p ON p.harness='codex' AND p.native_id=child.parent_id
WHERE child.source=:source AND child.harness='codex';
DELETE FROM bridge_spawn_placement
WHERE parent_source IN (SELECT source FROM affected_spawn_parent);
INSERT INTO bridge_spawn_placement
WITH calls AS (
  SELECT c.source,c.task_name,min(c.pos) AS pos,min(c.ts) AS ts
  FROM affected_spawn_parent a JOIN fact_tool_call c USING(source)
  WHERE c.tool='spawn_agent' GROUP BY c.source,c.task_name HAVING count(*)=1
), children AS (
  SELECT p.source AS parent_source,child.nickname,min(child.native_id) AS child
  FROM affected_spawn_parent a JOIN dim_session p USING(source)
  JOIN dim_session child ON child.harness='codex' AND child.parent_id=p.native_id
  WHERE p.harness='codex' GROUP BY p.source,child.nickname HAVING count(*)=1
)
SELECT child,c.source,c.pos,c.ts FROM children siblings JOIN calls c
ON c.source=siblings.parent_source AND c.task_name=siblings.nickname;
