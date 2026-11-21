-- One member's contribution to the selected component page. Prefix receipts
-- resolve copied records against the same committed view as the page's keys.
SELECT coalesce(r.ts,-9223372036854775808) AS time,r.pos,r.block,r.occurrence,r.kind
FROM fact_record r INDEXED BY record_position
WHERE r.source=:source AND r.pos>=0 AND NOT r.gap
  AND r.pos>=:start AND r.pos<:end
  AND (:entry_block IS NULL OR (r.block,r.occurrence)=(:entry_block,:entry_occurrence))
  AND coalesce(r.ts,-9223372036854775808)>=:time
  AND (coalesce(r.ts,-9223372036854775808),r.pos,r.block,r.occurrence)
      > (:time,:pos,:block,:occurrence)
  AND NOT EXISTS (
    SELECT 1 FROM dim_session target
    CROSS JOIN fact_relation_observation e
      ON e.target=target.native_id AND e.kind='continued-in' AND e.source<>r.source
    CROSS JOIN lineage_member original ON original.source=e.source AND original.component=:component
    CROSS JOIN dim_session active ON active.source=e.source AND active.lineage
    CROSS JOIN fact_record old ON old.source=e.source AND old.uuid=r.uuid
    WHERE target.source=r.source AND target.harness='claude'
      AND max(e.pos,old.pos) < (
        SELECT p.pos FROM warehouse_prefix p
        WHERE p.source=e.source AND p.revision<=:view ORDER BY p.revision DESC LIMIT 1
      )
  )
ORDER BY time,r.pos,r.block,r.occurrence LIMIT :limit;
