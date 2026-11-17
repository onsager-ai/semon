-- Independent, deliberately unbounded test oracle. Never used by append maintenance.
SELECT s.*,r.first_ts,r.last_ts,r.title_pos,m.model,r.prompt_pos,coalesce(c.continued,0)
FROM fact_session_observation s
LEFT JOIN (
 SELECT source,min(ts) first_ts,max(ts) last_ts,
 max(CASE WHEN kind='custom-title' THEN pos END) title_pos,
 min(CASE WHEN kind='prompt' THEN pos END) prompt_pos
 FROM fact_record GROUP BY source
) r USING(source)
LEFT JOIN (
 SELECT source,model,row_number() OVER (PARTITION BY source ORDER BY pos DESC) n
 FROM fact_step_observation WHERE model IS NOT NULL
) m ON m.source=s.source AND m.n=1
LEFT JOIN (
 SELECT source,1 continued FROM fact_relation_observation
 WHERE kind='continued-in' GROUP BY source
) c ON c.source=s.source ORDER BY s.source;

WITH RECURSIVE edges(src,dst) AS (
 SELECT d.native_id,e.target FROM dim_session d
 JOIN fact_relation_observation e USING(source)
 WHERE d.harness='claude' AND e.kind IN ('continued-in','session_id','bridge')
), reach(source,node) AS (
 SELECT source,native_id FROM dim_session WHERE harness='claude'
 UNION
 SELECT r.source,e.dst FROM reach r JOIN edges e ON e.src=r.node
 UNION
 SELECT r.source,e.src FROM reach r JOIN edges e ON e.dst=r.node
), members(source,member) AS (
 SELECT r.source,d.source FROM reach r JOIN dim_session d
 ON d.harness='claude' AND d.native_id=r.node
 UNION SELECT source,source FROM dim_session
)
SELECT source,min(member) FROM members GROUP BY source ORDER BY source;

WITH ranked AS (
 SELECT l.component,d.source,row_number() OVER (
 PARTITION BY l.component ORDER BY d.first_ts IS NULL,d.first_ts,d.continued DESC,d.native_id,d.source
 ) n FROM dim_lineage l JOIN dim_session d USING(source)
)
SELECT l.source,r.source,r.source FROM dim_lineage l
JOIN ranked r ON r.component=l.component AND r.n=1 ORDER BY l.source;

WITH calls AS (
 SELECT source,task_name,min(pos) pos,min(ts) ts FROM fact_tool_call
 WHERE tool='spawn_agent' GROUP BY source,task_name HAVING count(*)=1
), children AS (
 SELECT p.source parent_source,child.nickname,min(child.native_id) child
 FROM dim_session p JOIN dim_session child ON child.harness='codex'
 AND child.parent_id=p.native_id WHERE p.harness='codex'
 GROUP BY p.source,child.nickname HAVING count(*)=1
)
SELECT child,c.source,c.pos,c.ts FROM children siblings JOIN calls c
ON c.source=siblings.parent_source AND c.task_name=siblings.nickname ORDER BY child;
