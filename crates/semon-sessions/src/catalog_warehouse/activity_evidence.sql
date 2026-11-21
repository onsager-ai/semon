WITH candidates AS MATERIALIZED (
    SELECT r.* FROM json_each(:events) requested
    CROSS JOIN fact_record r INDEXED BY record_uuid
      ON r.source=:source AND r.uuid=requested.value
    WHERE r.kind='assistant' AND r.pos<:end AND NOT r.gap
    UNION
    SELECT r.* FROM fact_record r
    WHERE r.source=:source AND r.pos=:usage AND r.pos<:end AND r.uuid IS NULL
      AND r.kind='assistant' AND NOT r.gap
    UNION
    SELECT r.* FROM fact_operation o INDEXED BY operation_parent
    JOIN fact_record r ON r.source=o.source AND r.pos=o.pos AND NOT r.gap
    WHERE o.source=:source AND o.call_pos=:call AND o.pos<:end
    UNION
    SELECT r.* FROM fact_record r
    WHERE r.source=:source AND r.pos=:call AND r.pos<:end AND NOT r.gap
      AND EXISTS (SELECT 1 FROM fact_tool_call c WHERE c.source=r.source AND c.pos=r.pos)
      AND NOT EXISTS (SELECT 1 FROM fact_operation o
                      WHERE o.source=r.source AND o.call_pos=r.pos AND o.pos<:end)
)
SELECT coalesce(r.ts,-9223372036854775808) AS time,r.pos,r.block,r.occurrence,r.kind
FROM candidates r WHERE NOT EXISTS (
    SELECT 1 FROM json_each(:predecessors) predecessor
    CROSS JOIN fact_record old ON old.source=CAST(predecessor.key AS INTEGER)
      AND old.uuid=r.uuid AND old.pos<predecessor.value
    WHERE r.uuid IS NOT NULL
)
ORDER BY time,r.pos,r.block,r.occurrence LIMIT 1;
