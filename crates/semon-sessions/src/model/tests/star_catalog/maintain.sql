-- The caller binds :source and the previously maintained maximum :after.
-- All aggregates probe the (source,pos) primary keys over the new suffix only.
INSERT INTO dim_session
SELECT s.*,r.first_ts,r.last_ts,r.title_pos,m.model,r.prompt_pos,
  coalesce(c.continued,0)
FROM fact_session_observation s
LEFT JOIN (
  SELECT source,min(ts) AS first_ts,max(ts) AS last_ts,
    max(CASE WHEN kind='custom-title' THEN pos END) AS title_pos,
    min(CASE WHEN kind='prompt' THEN pos END) AS prompt_pos
  FROM fact_record WHERE source=:source AND pos>:after GROUP BY source
) r USING(source)
LEFT JOIN (
  SELECT source,model FROM fact_step_observation
  WHERE source=:source AND pos>:after AND model IS NOT NULL ORDER BY pos DESC LIMIT 1
) m USING(source)
LEFT JOIN (
  SELECT source,1 AS continued FROM fact_relation_observation
  WHERE source=:source AND pos>:after AND kind='continued-in' GROUP BY source
) c USING(source)
WHERE s.source=:source
ON CONFLICT(source) DO UPDATE SET
  first_ts=CASE WHEN dim_session.first_ts IS NULL THEN excluded.first_ts
    WHEN excluded.first_ts IS NULL THEN dim_session.first_ts
    ELSE min(dim_session.first_ts,excluded.first_ts) END,
  last_ts=CASE WHEN dim_session.last_ts IS NULL THEN excluded.last_ts
    WHEN excluded.last_ts IS NULL THEN dim_session.last_ts
    ELSE max(dim_session.last_ts,excluded.last_ts) END,
  title_pos=coalesce(excluded.title_pos,dim_session.title_pos),
  prompt_pos=coalesce(dim_session.prompt_pos,excluded.prompt_pos),
  model=coalesce(excluded.model,dim_session.model),
  continued=dim_session.continued OR excluded.continued;

-- Only appended call names and newly admitted child names can alter uniqueness.
DELETE FROM affected_spawn_key;
INSERT OR IGNORE INTO affected_spawn_key
SELECT source,task_name FROM fact_tool_call
WHERE source=:source AND pos>:after AND tool='spawn_agent' AND task_name IS NOT NULL;
INSERT OR IGNORE INTO affected_spawn_key
SELECT p.source,child.nickname FROM dim_session child
JOIN dim_session p ON p.harness='codex' AND p.native_id=child.parent_id
WHERE child.source=:source AND child.harness='codex' AND child.nickname IS NOT NULL
  AND :after=-1;
-- A late parent admits only names that occur in its newly inserted calls above.
DELETE FROM bridge_spawn_placement WHERE child IN (
  SELECT child.native_id FROM affected_spawn_key k
  CROSS JOIN dim_session p ON p.source=k.parent
  CROSS JOIN dim_session child ON child.harness='codex'
    AND child.parent_id=p.native_id AND child.nickname=k.name
);
INSERT OR REPLACE INTO bridge_spawn_placement
WITH calls AS (
  SELECT c.source,c.task_name,min(c.pos) AS pos,min(c.ts) AS ts
  FROM affected_spawn_key k CROSS JOIN fact_tool_call c
    ON c.source=k.parent AND c.tool='spawn_agent' AND c.task_name=k.name
  GROUP BY c.source,c.task_name HAVING count(*)=1
), children AS (
  SELECT k.parent,k.name,min(child.native_id) AS child
  FROM affected_spawn_key k CROSS JOIN dim_session p ON p.source=k.parent
  CROSS JOIN dim_session child ON child.harness='codex'
    AND child.parent_id=p.native_id AND child.nickname=k.name
  WHERE p.harness='codex' GROUP BY k.parent,k.name HAVING count(*)=1
)
SELECT child,c.source,c.pos,c.ts FROM children siblings JOIN calls c
ON c.source=siblings.parent AND c.task_name=siblings.name;
