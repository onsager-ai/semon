-- Start each candidate probe at a member of the selected parent component.
-- The caller resolves each candidate's winning offer and cycle qualification.
WITH parents AS MATERIALIZED (
  SELECT d.source,d.harness,d.native_id
  FROM lineage_member root
  CROSS JOIN lineage_member m ON m.component=root.component
  CROSS JOIN dim_session d ON d.source=m.source
  WHERE root.source=?1
), calls AS MATERIALIZED (
  SELECT DISTINCT c.call_id FROM parents p
  CROSS JOIN fact_claude_call c ON c.source=p.source WHERE p.harness='claude'
)
SELECT c.source FROM parents p
CROSS JOIN dim_session c ON c.harness=p.harness AND c.parent_id=p.native_id
UNION
SELECT a.source FROM parents p
CROSS JOIN fact_agent_observation a INDEXED BY agent_storage_parent ON a.storage_parent=p.native_id
CROSS JOIN maintenance_cursor current ON current.source=a.source AND current.sidecar_revision=a.revision
WHERE p.harness='claude'
UNION
SELECT a.source FROM calls c
CROSS JOIN fact_agent_observation a INDEXED BY agent_call ON a.call_id=c.call_id
CROSS JOIN maintenance_cursor current ON current.source=a.source AND current.sidecar_revision=a.revision
UNION
SELECT e.source FROM parents p
CROSS JOIN fact_parent_observation e INDEXED BY parent_observed_target
  ON e.parent_harness=p.harness AND e.parent_id=p.native_id
UNION
SELECT e.source FROM parents p
CROSS JOIN fact_eof_observation e INDEXED BY marker_observed_target
  ON e.parent_harness=p.harness AND e.parent_id=p.native_id AND e.kind='marker'
CROSS JOIN maintenance_cursor current ON current.source=e.source AND current.eof_witness=e.witness
UNION
SELECT e.source FROM parents p
CROSS JOIN fact_environment_observation e INDEXED BY process_environment_target
  ON e.parent_harness=p.harness AND e.parent_id=p.native_id
UNION
SELECT a.source FROM parents p
CROSS JOIN fact_process_observation o ON o.source=p.source
CROSS JOIN fact_ancestor_observation a INDEXED BY process_ancestor_pid
  ON a.capture=o.capture AND a.ancestor_pid=o.pid;
