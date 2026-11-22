-- Find one member's latest visible boundary at the target's merged position.
-- A Claude continuation must not start a second turn at a copied prompt.
WITH prompt AS (
    SELECT coalesce(p.ts,-9223372036854775808) AS time,p.pos,p.block,p.occurrence,p.kind,
           r.occurrence AS record_occurrence,NULL AS correlation,NULL AS legacy_id
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
           occurrence AS record_occurrence,NULL AS correlation,NULL AS legacy_id
    FROM fact_record INDEXED BY record_gap
    WHERE source=:source AND gap AND pos<:end
      AND coalesce(ts,-9223372036854775808)<=:time
      AND (coalesce(ts,-9223372036854775808),pos)<=(:time,:pos)
    ORDER BY time DESC,pos DESC LIMIT 1
), relay AS (
    SELECT coalesce(e.ts,-9223372036854775808) AS time,e.pos,e.block,e.occurrence,'relay' AS kind,
           r.occurrence AS record_occurrence,e.correlation,NULL AS legacy_id
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
), progress AS (
    SELECT coalesce(e.ts,-9223372036854775808) AS time,e.pos,e.block,e.occurrence,'relay' AS kind,
           r.occurrence AS record_occurrence,NULL AS correlation,claimed.value AS legacy_id
    FROM fact_subagent_event e JOIN fact_record r ON r.source=e.source AND r.pos=e.pos AND r.block=0 AND NOT r.gap
    LEFT JOIN json_each(:legacy) claimed ON claimed.key=printf('%d:%d:%d:%d',e.source,e.pos,e.block,e.occurrence)
    WHERE e.source=:source AND e.pos<:end AND e.kind='agm' AND (e.flags & :progress<>0 OR claimed.value IS NOT NULL)
      AND (coalesce(e.ts,-9223372036854775808),e.pos)<=(:time,:pos)
      AND NOT EXISTS (
          SELECT 1 FROM json_each(:predecessors) predecessor
          CROSS JOIN fact_record old ON old.source=CAST(predecessor.key AS INTEGER)
            AND old.uuid=r.uuid AND old.pos<predecessor.value
          WHERE r.uuid IS NOT NULL
      )
    ORDER BY time DESC,e.pos DESC,e.block DESC,e.occurrence DESC LIMIT 1
)
SELECT * FROM prompt UNION ALL SELECT * FROM gap UNION ALL SELECT * FROM relay UNION ALL SELECT * FROM progress
UNION ALL SELECT -9223372036854775808,pos,block,occurrence,'launch',occurrence,NULL,NULL
FROM fact_record WHERE :head AND source=:source AND pos=(
    SELECT min(pos) FROM fact_record WHERE source=:source AND pos>=0 AND pos<:end AND NOT gap
) AND NOT gap
ORDER BY time DESC,pos DESC,block DESC,occurrence DESC LIMIT 1;
