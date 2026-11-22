-- Home event metadata uses the component's committed, non-copied positions.
WITH members AS MATERIALIZED (
  SELECT m.source,d.native_id,d.harness,p.pos AS end
  FROM lineage_member root JOIN lineage_member m USING(component)
  JOIN dim_session d ON d.source=m.source JOIN warehouse_prefix p ON p.source=m.source
  WHERE root.source=?1 AND NOT p.superseded
), copies AS MATERIALIZED (
  SELECT DISTINCT m.source AS target, old.source, old.end
  FROM members m
  JOIN fact_relation_observation e ON e.target=m.native_id AND e.kind='continued-in'
  JOIN members old ON old.source=e.source AND old.end>e.pos
  JOIN dim_session d ON d.source=old.source AND d.lineage
  WHERE m.harness='claude' AND m.source<>old.source
), questions AS (
  SELECT c.*,m.native_id,m.harness,m.end,
    (SELECT t.pos FROM fact_codex_turn t WHERE t.source=c.source AND t.pos<m.end
      AND t.thread_id=m.native_id ORDER BY t.pos DESC,t.block DESC,t.occurrence DESC LIMIT 1) AS turn_pos
  FROM members m JOIN fact_tool_call c ON c.source=m.source AND c.pos<m.end
  JOIN fact_record r ON r.source=c.source AND r.pos=c.pos AND r.block=0 AND NOT r.gap
  WHERE c.tool IN ('AskUserQuestion','request_user_input','request_user_input_async',
                  'functions.request_user_input','functions.request_user_input_async')
  AND NOT EXISTS (SELECT 1 FROM copies x JOIN fact_record old ON old.source=x.source
    AND old.uuid=r.uuid AND old.pos<x.end WHERE x.target=c.source)
), answers AS (
  SELECT c.native_id,c.pos,c.call_id,c.ts,c.source,c.block,t.failed,t.flags,
    coalesce(t.ts,next.ts) AS done,
    CASE c.harness WHEN 'codex' THEN ?4='work' AND c.pos>=c.turn_pos
      AND t.pos IS NULL AND next.pos IS NULL
    ELSE ?4 IN ('work','wait','idle') AND t.pos IS NULL END AS waiting
  FROM questions c
  LEFT JOIN fact_tool_result t ON (t.source,t.pos,t.block,t.kind,t.occurrence)=(
    SELECT r.source,r.pos,r.block,r.kind,r.occurrence FROM fact_tool_result r
    WHERE r.source=c.source AND r.call_id=c.call_id AND r.pos>c.pos AND r.pos<c.end
      AND ((r.call_pos=c.pos AND r.call_block=c.block) OR (r.call_pos IS NULL AND 1=(
        SELECT count(*) FROM fact_tool_call a WHERE a.source=c.source AND a.call_id=c.call_id AND a.pos<c.end)))
      AND (c.harness<>'codex' OR (r.flags & ?3)=0 OR
           c.tool IN ('request_user_input_async','functions.request_user_input_async'))
    ORDER BY r.pos DESC,r.block DESC,r.occurrence DESC LIMIT 1)
  LEFT JOIN fact_prompt next ON c.harness='codex' AND (next.source,next.pos,next.block,next.kind,next.occurrence)=(
    SELECT p.source,p.pos,p.block,p.kind,p.occurrence FROM fact_prompt p
    WHERE p.source=c.source AND p.pos>c.pos AND p.pos<c.end AND p.kind='user'
    ORDER BY p.pos,p.block,p.occurrence LIMIT 1)
)
SELECT native_id,pos,call_id,ts,done,failed,flags,coalesce(waiting,0)
FROM answers WHERE ts>=?2 OR done>=?2 OR waiting
ORDER BY ts DESC,source DESC,pos DESC,block DESC;
