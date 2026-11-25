-- Correlate the current subagent sidecar with the parent lineage's Agent call.
WITH dispatch AS (
  SELECT min(c.source) AS source,min(c.pos) AS pos,min(c.call_id) AS call_id
  FROM maintenance_cursor cursor
  JOIN fact_agent_observation agent ON agent.source=cursor.source
    AND agent.revision=cursor.sidecar_revision AND agent.present
  JOIN fact_tool_call c ON c.call_id=agent.call_id AND c.tool IN ('Agent','Task')
  JOIN bridge_session_owner parent ON parent.source=c.source AND parent.owner=?2
  WHERE cursor.source=?1 AND NOT EXISTS (
    SELECT 1 FROM fact_agent_observation other
    JOIN maintenance_cursor active ON active.source=other.source AND active.sidecar_revision=other.revision
    JOIN warehouse_current current ON current.source=other.source
    WHERE other.call_id=agent.call_id AND other.present AND other.source<>agent.source
      AND other.storage_parent=agent.storage_parent
  )
  HAVING count(*)=1
), terminal AS (
  SELECT max(pos) AS pos FROM fact_record WHERE source=?1 AND kind='assistant'
), failure AS (
  SELECT EXISTS(
    SELECT 1 FROM dispatch d JOIN bridge_session_owner m ON m.owner=?2
    JOIN fact_subagent_event e ON e.source=m.source AND e.id=d.call_id
    WHERE e.kind='tn' AND e.name='failed' AND (e.source<>d.source OR e.pos>d.pos)
  ) AS failed
)
SELECT d.call_id,o.disposition,o.pr_url,o.head_sha,o.checks_state,f.failed
FROM dispatch d CROSS JOIN terminal t CROSS JOIN failure f
LEFT JOIN fact_worker_outcome o ON o.source=?1 AND o.pos=t.pos AND NOT f.failed
  AND NOT EXISTS (SELECT 1 FROM fact_worker_outcome other WHERE other.source=?1 AND other.pos<>o.pos)
  AND NOT EXISTS (SELECT 1 FROM fact_prompt p WHERE p.source=?1 AND p.pos>o.pos)
  AND NOT EXISTS (
    SELECT 1 FROM fact_tool_result r JOIN bridge_session_owner m ON m.source=r.source AND m.owner=?2
    JOIN fact_tool_call c ON c.source=r.source AND c.call_id=r.call_id AND c.tool IN ('Agent','Task')
    JOIN dim_session child ON child.source=?1 AND child.native_id=r.target
    WHERE r.kind='tool_result' AND (r.source<>d.source OR r.call_id<>d.call_id)
  )
