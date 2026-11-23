-- Walk the same member position index backwards for the latest window and
-- its earlier pages. Bound suppression to the selected candidates.
WITH candidates AS MATERIALIZED (
  SELECT coalesce(r.ts,-9223372036854775808) AS time,r.pos,r.block,r.occurrence,CASE WHEN r.gap THEN char(0) ELSE nullif(r.kind,char(0)) END AS kind,r.uuid
  FROM fact_record r INDEXED BY record_position
  WHERE r.source=:source AND r.pos>=0
    AND r.pos>=:start AND r.pos<:end
    AND (:entry_block IS NULL OR (r.block,r.occurrence)=(:entry_block,:entry_occurrence))
    AND coalesce(r.ts,-9223372036854775808)<=:time
    AND (coalesce(r.ts,-9223372036854775808),r.pos,r.block,r.occurrence)
        < (:time,:pos,:block,:occurrence)
  ORDER BY time DESC,r.pos DESC,r.block DESC,r.occurrence DESC LIMIT :limit
), suppressed AS MATERIALIZED (
  SELECT DISTINCT c.uuid FROM candidates c
  CROSS JOIN json_each(:predecessors) predecessor
  CROSS JOIN fact_record old ON old.source=CAST(predecessor.key AS INTEGER) AND old.uuid=c.uuid
    AND old.pos<predecessor.value
  WHERE c.uuid IS NOT NULL
)
SELECT c.time,c.pos,c.block,c.occurrence,c.kind,s.uuid IS NOT NULL AS copied
FROM candidates c LEFT JOIN suppressed s ON s.uuid=c.uuid
ORDER BY c.time DESC,c.pos DESC,c.block DESC,c.occurrence DESC;
