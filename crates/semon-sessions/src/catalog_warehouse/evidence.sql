-- Native event and call identities are source-scoped. Resolve only unique
-- surviving candidates inside the selected component and its committed view.
WITH matches AS (
    SELECT r.* FROM fact_record r INDEXED BY record_uuid
    WHERE r.source=:source AND r.uuid=:event AND r.pos<:end AND NOT r.gap
    UNION ALL
    SELECT r.* FROM fact_tool_call c INDEXED BY tool_call_identity
    JOIN fact_record r ON r.source=c.source AND r.pos=c.pos AND r.block=0 AND NOT r.gap
    WHERE c.source=:source AND c.call_id=:call AND c.pos<:end
)
SELECT coalesce(r.ts,-9223372036854775808),r.pos,r.block,r.occurrence,r.kind
FROM matches r WHERE NOT EXISTS (
    SELECT 1 FROM json_each(:predecessors) predecessor
    CROSS JOIN fact_record old ON old.source=CAST(predecessor.key AS INTEGER)
      AND old.uuid=r.uuid AND old.pos<predecessor.value
    WHERE r.uuid IS NOT NULL
) LIMIT 2;
