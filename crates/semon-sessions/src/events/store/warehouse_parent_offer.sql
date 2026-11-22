-- Resolve the offers for one physical child. Global uniqueness checks are
-- probes of a specific call ID or captured process, not of the whole graph.
WITH child AS MATERIALIZED (
  SELECT d.* FROM dim_session d WHERE d.source=?1
), sidecar AS MATERIALIZED (
  SELECT a.call_id,g.owner AS storage
  FROM maintenance_cursor current
  CROSS JOIN fact_agent_observation a ON a.source=current.source AND a.revision=current.sidecar_revision
  CROSS JOIN warehouse_current active ON active.source=a.source
  LEFT JOIN dim_session p ON p.harness='claude' AND p.native_id=a.storage_parent
  LEFT JOIN lineage_member m ON m.source=p.source
  LEFT JOIN lineage_group g ON g.id=m.component
  WHERE current.source=?1
), agent AS MATERIALIZED (
  SELECT a.call_id,a.storage,
    (SELECT m.owner FROM fact_claude_call c INDEXED BY claude_call_identity
     CROSS JOIN bridge_session_owner m ON m.source=c.source
     CROSS JOIN dim_session d ON d.source=m.owner AND d.harness='claude'
     WHERE c.call_id=a.call_id AND m.owner=a.storage LIMIT 1) AS exact,
    (SELECT CASE WHEN count(*)=1 THEN min(owner) END FROM (
       SELECT DISTINCT m.owner FROM fact_claude_call c INDEXED BY claude_call_identity
       CROSS JOIN bridge_session_owner m ON m.source=c.source
       CROSS JOIN dim_session d ON d.source=m.owner AND d.harness='claude'
       WHERE c.call_id=a.call_id LIMIT 2
     )) AS fallback
  FROM sidecar a
), native(parent,call_id,method,priority,ordinal) AS MATERIALIZED (
  SELECT p.source,NULL,'native',0,0 FROM child d
  CROSS JOIN dim_session p ON p.harness=d.harness AND p.native_id=d.parent_id
  UNION ALL
  SELECT coalesce(exact,fallback,storage),
    CASE WHEN exact IS NOT NULL OR fallback IS NOT NULL THEN call_id END,'native',0,0
  FROM agent WHERE coalesce(exact,fallback,storage) IS NOT NULL
), known_ancestor AS (
  SELECT a.capture,a.ancestor_pid,min(a.ordinal) AS ordinal
  FROM warehouse_current active
  CROSS JOIN fact_process_observation child ON child.source=active.source
  CROSS JOIN fact_ancestor_observation a
    ON a.source=child.source AND a.capture=child.capture AND a.pid=child.pid AND a.start=child.start
  WHERE active.source=?1 AND NOT EXISTS (SELECT 1 FROM native)
    AND EXISTS (
      SELECT 1 FROM fact_process_observation p INDEXED BY process_capture_pid
      CROSS JOIN warehouse_current carrier ON carrier.source=p.source
      WHERE p.capture=a.capture AND p.pid=a.ancestor_pid
    )
  GROUP BY a.capture
), offered AS (
  SELECT * FROM native
  UNION ALL
  SELECT p.source,e.call_id,e.method,
    CASE e.method WHEN 'marker' THEN 3 WHEN 'environment' THEN 2 ELSE 1 END,e.pos
  FROM warehouse_current active
  CROSS JOIN fact_parent_observation e ON e.source=active.source
  CROSS JOIN dim_session p ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  WHERE active.source=?1 AND p.source<>?1
  UNION ALL
  SELECT p.source,e.call_id,'marker',3,e.pos
  FROM warehouse_current active
  CROSS JOIN maintenance_cursor current ON current.source=active.source
  CROSS JOIN fact_eof_observation e ON e.source=active.source AND e.witness=current.eof_witness
  CROSS JOIN dim_session p ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  WHERE active.source=?1 AND e.kind='marker' AND p.source<>?1
  UNION ALL
  SELECT p.source,e.call_id,'environment',2,e.capture
  FROM warehouse_current active
  CROSS JOIN fact_environment_observation e ON e.source=active.source
  CROSS JOIN fact_process_observation child
    ON child.source=e.source AND child.capture=e.capture AND child.pid=e.pid AND child.start=e.start
  CROSS JOIN dim_session p ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  WHERE active.source=?1 AND p.source<>?1
  UNION ALL
  SELECT min(p.source),NULL,'ancestry',1,a.capture
  FROM known_ancestor a
  CROSS JOIN fact_process_observation p INDEXED BY process_capture_pid
    ON p.capture=a.capture AND p.pid=a.ancestor_pid
  CROSS JOIN warehouse_current carrier ON carrier.source=p.source
  GROUP BY a.capture HAVING count(*)=1 AND min(p.source)<>?1
)
SELECT parent,call_id,method FROM (
  SELECT *,0 AS fallback FROM offered
  UNION ALL
  SELECT *,1 FROM native
)
WHERE fallback=?2 ORDER BY priority DESC,ordinal DESC LIMIT 1;
