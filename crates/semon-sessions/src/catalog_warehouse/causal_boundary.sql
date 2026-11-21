-- Find one member's latest visible boundary at the target's merged position.
-- A Claude continuation must not start a second turn at a copied prompt.
WITH prompt AS (
    SELECT coalesce(p.ts,-9223372036854775808) AS time,p.pos,p.block,p.occurrence,p.kind,
           r.occurrence AS record_occurrence,NULL AS correlation
    FROM fact_prompt p INDEXED BY prompt_position
    JOIN fact_record r ON r.source=p.source AND r.pos=p.pos AND r.block=0 AND NOT r.gap
    WHERE p.source=:source AND p.pos<:end
      AND coalesce(p.ts,-9223372036854775808)<=:time
      AND (coalesce(p.ts,-9223372036854775808),p.pos)<=(:time,:pos)
      AND NOT EXISTS (
          SELECT 1 FROM json_each(:predecessors) predecessor
          CROSS JOIN fact_record old ON old.source=CAST(predecessor.key AS INTEGER)
            AND old.uuid=r.uuid AND old.pos<predecessor.value
          WHERE r.uuid IS NOT NULL
      )
    ORDER BY time DESC,p.pos DESC,p.block DESC,p.occurrence DESC LIMIT 1
), gap AS (
    SELECT coalesce(ts,-9223372036854775808) AS time,pos,block,occurrence,NULL AS kind,
           occurrence AS record_occurrence,NULL AS correlation
    FROM fact_record INDEXED BY record_gap
    WHERE source=:source AND gap AND pos<:end
      AND coalesce(ts,-9223372036854775808)<=:time
      AND (coalesce(ts,-9223372036854775808),pos)<=(:time,:pos)
    ORDER BY time DESC,pos DESC LIMIT 1
), relay AS (
    SELECT coalesce(e.ts,-9223372036854775808) AS time,e.pos,e.block,e.occurrence,'relay' AS kind,
           r.occurrence AS record_occurrence,e.correlation
    FROM fact_relay_receipt e INDEXED BY relay_receipt_position
    JOIN fact_record r ON r.source=e.source AND r.pos=e.pos AND r.block=0 AND NOT r.gap
    WHERE e.source=:source AND e.pos<:end
      AND coalesce(e.ts,-9223372036854775808)<=:time
      AND (coalesce(e.ts,-9223372036854775808),e.pos)<=(:time,:pos)
      AND NOT EXISTS (
          SELECT 1 FROM json_each(:predecessors) predecessor
          CROSS JOIN fact_record old ON old.source=CAST(predecessor.key AS INTEGER)
            AND old.uuid=r.uuid AND old.pos<predecessor.value
          WHERE r.uuid IS NOT NULL
      )
    ORDER BY time DESC,e.pos DESC,e.block DESC,e.occurrence DESC LIMIT 1
)
SELECT * FROM prompt UNION ALL SELECT * FROM gap UNION ALL SELECT * FROM relay
ORDER BY time DESC,pos DESC,block DESC,occurrence DESC LIMIT 1;
