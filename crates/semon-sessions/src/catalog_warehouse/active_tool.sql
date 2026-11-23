-- Use the producer's pending and yielded event keys to seek directly into
-- the native event rows. The caller checks that the winner is in the last turn.
WITH members AS MATERIALIZED (
    SELECT m.source,f.file_id,f.pending,f.yields,f.cwd,d.harness,d.native_id,p.pos AS end
    FROM lineage_member m
    CROSS JOIN dim_session d USING(source)
    CROSS JOIN warehouse_current c USING(source)
    CROSS JOIN warehouse_prefix p ON p.source=m.source AND p.superseded=0
    CROSS JOIN files f ON f.path=c.path AND f.resume_at=p.pos
    WHERE m.component=?1
), running AS (
    SELECT m.source,m.file_id,m.end,call.value AS seq,0 AS yielded
    FROM members m CROSS JOIN json_each(m.pending) call
    UNION ALL
    SELECT m.source,m.file_id,m.end,call.value,1
    FROM members m CROSS JOIN json_each(m.yields) call
)
SELECT coalesce(r.ts,-9223372036854775808),r.source,r.pos,r.block,r.occurrence,
       CASE WHEN call.yielded THEN 'exec_command' ELSE coalesce(e.n,'tool') END,e.t,e.b,m.cwd
FROM running call
CROSS JOIN events e ON e.file_id=call.file_id AND e.extra=0 AND e.seq=call.seq
CROSS JOIN fact_record r ON r.source=call.source AND r.pos=e.o AND r.block=0 AND NOT r.gap
CROSS JOIN members m ON m.source=call.source
WHERE e.k='tool' AND m.harness=?2 AND r.pos<call.end
  AND NOT EXISTS (
      SELECT 1 FROM fact_relation_observation edge
      CROSS JOIN members old ON old.source=edge.source AND edge.pos<old.end
      CROSS JOIN dim_session d ON d.source=old.source AND d.lineage
      CROSS JOIN fact_record original ON original.source=old.source AND original.uuid=r.uuid AND original.pos<old.end
      WHERE r.uuid IS NOT NULL AND m.harness='claude' AND m.source<>old.source
        AND edge.target=m.native_id AND edge.kind='continued-in'
  )
ORDER BY 1 DESC,2 DESC,3 DESC,e.b DESC,5 DESC,call.yielded DESC LIMIT 1;
