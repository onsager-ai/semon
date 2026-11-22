-- Start from untyped receipts in the selected parent component. Resolve only
-- children with a qualifying native parent link, including spawn-call aliases.
WITH receipts AS MATERIALIZED (
    SELECT DISTINCT e.id,e.name FROM lineage_member m
    JOIN fact_subagent_event e ON e.source=m.source
    WHERE m.component=?1 AND e.kind='agm' AND e.flags=?2 AND e.pos<(
        SELECT pos FROM warehouse_prefix p WHERE p.source=m.source AND p.revision<=?3
        ORDER BY revision DESC LIMIT 1
    )
), candidates AS MATERIALIZED (
    SELECT d.source FROM receipts r JOIN dim_session d
        ON d.harness='claude' AND NOT d.lineage AND d.native_id IN (r.id,r.name)
    UNION
    SELECT a.source FROM receipts r
    CROSS JOIN fact_agent_observation a INDEXED BY agent_call ON a.call_id IN (r.id,r.name)
    CROSS JOIN maintenance_cursor current ON current.source=a.source AND current.sidecar_revision=a.revision
)
SELECT source FROM candidates ORDER BY source;
