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
),
-- Drive each arm of the union from the selected lineage. Joining the union
-- afterwards lets SQLite materialize the warehouse's fact tables per owner.
member_events AS (
  SELECT e.source,e.pos,e.block,e.occurrence,e.ts,e.kind,e.id,e.name,e.flags,m.native_id,m.harness
  FROM members m CROSS JOIN fact_subagent_event e ON e.source=m.source AND e.pos<m.end
  WHERE e.kind IN ('assistant','agm','xsm','tn') OR (e.kind='tool' AND e.name='SubagentHandback')
  UNION ALL
  SELECT p.source,p.pos,p.block,p.occurrence,p.ts,p.kind,NULL,NULL,0,m.native_id,m.harness
  FROM members m CROSS JOIN fact_prompt p ON p.source=m.source AND p.pos<m.end
  UNION ALL
  SELECT r.source,r.pos,r.block,r.occurrence,r.ts,'gap',NULL,NULL,0,m.native_id,m.harness
  FROM members m CROSS JOIN fact_record r ON r.source=m.source AND r.pos<m.end
  WHERE r.gap AND r.block=0
), visible AS MATERIALIZED (
  SELECT a.source,a.pos,a.block,a.occurrence,coalesce(a.ts,r.ts,-9223372036854775808) AS ts,
    a.kind,a.id,a.name,a.flags,a.native_id,a.harness
  FROM member_events a
  CROSS JOIN fact_record r ON r.source=a.source AND r.pos=a.pos AND r.block=0 AND r.gap=(a.kind='gap')
  WHERE NOT EXISTS (SELECT 1 FROM copies x JOIN fact_record old ON old.source=x.source
    AND old.uuid=r.uuid AND old.pos<x.end WHERE x.target=a.source)
), answer AS MATERIALIZED (
  SELECT * FROM visible WHERE kind='assistant' AND ts>=?2
  ORDER BY ts DESC,source DESC,pos DESC,block DESC,occurrence DESC LIMIT 1
), boundary AS (
  SELECT p.kind,a.harness FROM answer a JOIN visible p
    ON (p.ts,p.source,p.pos,p.block,p.occurrence)<(a.ts,a.source,a.pos,a.block,a.occurrence)
  WHERE p.kind<>'assistant'
    -- A task notification or a final relay from this session's own spawn is
    -- its result, so it does not interrupt the human's turn. Progress relays do.
    AND NOT (p.harness='claude' AND (p.kind='tn' OR (p.kind='agm' AND p.flags & ?3<>0))
      AND (EXISTS (
        SELECT 1 FROM members m JOIN fact_claude_call c ON c.source=m.source AND c.pos<m.end
        JOIN fact_record r ON r.source=c.source AND r.pos=c.pos AND r.block=0 AND NOT r.gap
        WHERE c.tool IN ('Agent','Task') AND (c.call_id=p.id OR (p.kind='agm' AND c.call_id=p.name))
          AND (coalesce(r.ts,-9223372036854775808),c.source,c.pos)<(p.ts,p.source,p.pos)
      ) OR EXISTS (
        SELECT 1 FROM dim_session child JOIN warehouse_current active ON active.source=child.source
        JOIN maintenance_cursor current ON current.source=child.source
        JOIN fact_agent_observation agent ON agent.source=child.source AND agent.revision=current.sidecar_revision
        JOIN members parent ON parent.native_id=agent.storage_parent
        WHERE child.harness='claude' AND agent.present
          AND child.native_id IN (p.id, CASE WHEN p.kind='agm' THEN p.name END)
          AND NOT EXISTS (
            SELECT 1 FROM fact_claude_call elsewhere WHERE elsewhere.call_id=agent.call_id
              AND elsewhere.tool IN ('Agent','Task') AND NOT EXISTS (
                SELECT 1 FROM members m WHERE m.source=elsewhere.source AND elsewhere.pos<m.end
              )
          )
      )))
  ORDER BY p.ts DESC,p.source DESC,p.pos DESC,p.block DESC,p.occurrence DESC LIMIT 1
)
SELECT native_id,pos,block,ts FROM answer
WHERE EXISTS (SELECT 1 FROM boundary WHERE kind='ask' OR (kind='user' AND harness='codex'))
AND (harness='codex' OR NOT EXISTS (
  SELECT 1 FROM visible p WHERE p.kind='ask' AND (p.ts,p.source,p.pos)>(answer.ts,answer.source,answer.pos)
));
