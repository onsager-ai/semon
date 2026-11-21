-- Start from untyped receipts in the selected parent component. Resolve only
-- children with a qualifying native parent link, including spawn-call aliases.
WITH receipts AS MATERIALIZED (
    SELECT DISTINCT e.id,e.name FROM lineage_member m
    JOIN fact_subagent_event e ON e.source=m.source
    WHERE m.component=?1 AND e.kind='agm' AND e.flags=?2 AND e.pos<(
        SELECT pos FROM warehouse_prefix p WHERE p.source=m.source AND p.revision<=?4
        ORDER BY revision DESC LIMIT 1
    )
), candidates AS MATERIALIZED (
    SELECT d.source FROM receipts r JOIN dim_session d
        ON d.harness='claude' AND NOT d.lineage AND d.native_id IN (r.id,r.name)
    UNION
    SELECT p.child FROM receipts r JOIN parent_selection p ON p.call_id IN (r.id,r.name)
)
SELECT DISTINCT p.child FROM candidates c
JOIN parent_selection p ON p.child=c.source AND p.method='native'
JOIN bridge_session_owner b ON b.source=p.parent AND b.owner=?3
ORDER BY p.child LIMIT 129;
