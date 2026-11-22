WITH members AS MATERIALIZED (
  SELECT json_extract(value,'$[0]') AS source,json_extract(value,'$[1]') AS end,
         json_extract(value,'$[2]') AS revision,json_extract(value,'$[3]') AS predecessors
  FROM json_each(?1)
), calls AS (
  SELECT c.*,m.end FROM members m
  CROSS JOIN fact_tool_call c ON c.source=m.source AND c.pos<m.end
  WHERE ?2='errors' AND c.call_id IS NOT NULL AND c.tool IS NOT NULL
), unique_calls AS MATERIALIZED (
  SELECT source,min(pos) AS pos,min(block) AS block,min(call_id) AS call_id,max(end) AS end
  FROM calls GROUP BY source,call_id HAVING count(*)=1
), ranked AS (
  SELECT c.source,c.pos,c.block,r.failed,r.flags,row_number() OVER (
    PARTITION BY c.source,c.pos,c.block ORDER BY coalesce(r.ts,-9223372036854775808) DESC,
    r.pos DESC,r.block DESC,r.occurrence DESC
  ) AS rank
  FROM unique_calls c CROSS JOIN fact_tool_result r INDEXED BY tool_result_identity
    ON r.source=c.source AND r.call_id=c.call_id AND r.pos<c.end
    AND ((r.call_pos IS NULL AND r.kind<>'copilot-execution')
      OR (r.call_pos=c.pos AND r.call_block=c.block))
  WHERE r.flags & ?3=0
), marks(source,pos,display_block,signal) AS (
  SELECT source,pos,block,NULL FROM ranked WHERE rank=1 AND failed AND flags & ?4=0
  UNION ALL
  SELECT c.source,c.pos,c.block,NULL FROM calls c
  WHERE ?7 AND NOT EXISTS (
    SELECT 1 FROM ranked r WHERE r.source=c.source AND r.pos=c.pos AND r.block=c.block
  )
  UNION ALL
  SELECT o.source,o.pos,o.block,NULL FROM members m
  CROSS JOIN fact_operation o ON o.source=m.source AND o.pos<m.end
  WHERE ?2='errors' AND o.ok=0
  UNION ALL
  SELECT s.source,s.pos,0,json_extract(s.payload,'$.k') FROM members m
  CROSS JOIN warehouse_signal s ON s.source=m.source AND s.pos<m.end
    AND s.revision<=m.revision AND (s.retired_revision IS NULL OR s.retired_revision>m.revision)
  WHERE ?2='signals'
), visible AS MATERIALIZED (
  SELECT DISTINCT r.source,r.pos,r.block,r.occurrence,r.uuid,r.ts,a.display_block,a.signal
  FROM marks a JOIN fact_record r ON r.source=a.source AND r.pos=a.pos AND NOT r.gap
  JOIN members m ON m.source=a.source
  WHERE NOT EXISTS (
    SELECT 1 FROM json_each(m.predecessors) p
    CROSS JOIN fact_record old ON old.source=CAST(p.key AS INTEGER) AND old.uuid=r.uuid AND old.pos<p.value
    WHERE r.uuid IS NOT NULL
  )
), page AS (
  SELECT * FROM visible ORDER BY ts,source,pos,block,occurrence,display_block,signal LIMIT ?5 OFFSET ?6
)
SELECT total.n,page.source,page.pos,page.block,page.occurrence,page.uuid,page.display_block,page.signal
FROM (SELECT count(*) AS n FROM visible) total LEFT JOIN page ON true
ORDER BY page.ts,page.source,page.pos,page.block,page.occurrence,page.display_block,page.signal;
