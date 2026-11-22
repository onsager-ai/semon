-- Select the latest visible assistant text across the committed component.
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
), visible AS (
  SELECT a.source,a.pos,a.block,coalesce(a.ts,r.ts,-9223372036854775808) AS ts,a.kind,m.native_id,m.harness
  FROM members m JOIN (
    SELECT source,pos,block,ts,kind FROM fact_subagent_event
      WHERE kind IN ('assistant','agm','xsm','tn') OR (kind='tool' AND name='SubagentHandback')
    UNION ALL
    SELECT source,pos,block,ts,kind FROM fact_prompt
  ) a ON a.source=m.source AND a.pos<m.end
  JOIN fact_record r ON r.source=a.source AND r.pos=a.pos AND r.block=0 AND NOT r.gap
  WHERE NOT EXISTS (SELECT 1 FROM copies x JOIN fact_record old ON old.source=x.source
    AND old.uuid=r.uuid AND old.pos<x.end WHERE x.target=a.source)
), answer AS MATERIALIZED (
  SELECT * FROM visible WHERE kind='assistant' AND ts>=?2
  ORDER BY ts DESC,source DESC,pos DESC,block DESC LIMIT 1
), boundary AS (
  SELECT p.kind,a.harness FROM answer a JOIN visible p
    ON (p.ts,p.source,p.pos,p.block)<(a.ts,a.source,a.pos,a.block)
  WHERE p.kind<>'assistant'
  ORDER BY p.ts DESC,p.source DESC,p.pos DESC,p.block DESC LIMIT 1
)
SELECT native_id,pos,block,ts FROM answer
WHERE EXISTS (SELECT 1 FROM boundary WHERE kind='ask' OR (kind='user' AND harness='codex'))
AND (harness='codex' OR NOT EXISTS (
  SELECT 1 FROM visible p WHERE p.kind='ask' AND (p.ts,p.source,p.pos)>(answer.ts,answer.source,answer.pos)
));
