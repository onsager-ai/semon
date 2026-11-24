-- Resolve the successful correlated result of one physical send inside the
-- certified trace prefix. Ambiguous call IDs and deliveries are unavailable.
WITH origin AS MATERIALIZED (
  SELECT CAST(m.key AS INTEGER) AS source,json_extract(m.value,'$[1]') AS end
  FROM json_each(?1) m
  CROSS JOIN fact_record r ON r.source=CAST(m.key AS INTEGER) AND r.pos=?2 AND r.block=?3 AND r.occurrence=?4
  WHERE NOT r.gap AND r.pos<json_extract(m.value,'$[1]')
), send AS MATERIALIZED (
  SELECT s.source,s.call_id,s.pos,s.block,o.end FROM origin o
  CROSS JOIN fact_relay_send s ON s.source=o.source AND s.pos=?2 AND s.block=?5
  WHERE s.call_id IS NOT NULL AND (s.target IN (SELECT value FROM json_each(?8)) OR (s.target='main' AND ?9))
), delivery AS MATERIALIZED (
  SELECT DISTINCT r.correlation FROM send s
  CROSS JOIN fact_relay_result r INDEXED BY relay_result_call
    ON r.source=s.source AND r.call_id=s.call_id AND r.pos>s.pos AND r.pos<s.end
    AND r.call_pos=s.pos AND r.call_block=s.block
  WHERE NOT r.failed AND r.correlation IS NOT NULL AND (SELECT count(*) FROM send)=1
  LIMIT 2
), owners AS MATERIALIZED (
  SELECT DISTINCT r.source,c.pos,c.block FROM delivery d
  CROSS JOIN fact_relay_result r INDEXED BY relay_result_correlation ON r.correlation=d.correlation
  CROSS JOIN json_each(?7) prefix ON CAST(prefix.key AS INTEGER)=r.source AND r.pos<prefix.value
  CROSS JOIN fact_relay_send c ON c.source=r.source AND c.call_id=r.call_id AND c.pos<r.pos
    AND ((r.call_pos=c.pos AND r.call_block=c.block) OR
         (r.call_pos IS NULL AND (SELECT count(*) FROM fact_tool_call t
           WHERE t.source=r.source AND t.call_id=r.call_id AND t.pos<prefix.value)=1))
  WHERE NOT r.failed LIMIT 2
)
SELECT r.source,r.pos,f.occurrence,r.correlation,coalesce(f.ts,-9223372036854775808) FROM delivery d
CROSS JOIN json_each(?6) m
CROSS JOIN fact_relay_receipt r INDEXED BY relay_receipt_correlation
  ON r.correlation=d.correlation AND r.source=CAST(m.key AS INTEGER) AND r.pos<m.value
CROSS JOIN fact_record f ON f.source=r.source AND f.pos=r.pos AND f.block=0 AND NOT f.gap
WHERE (SELECT count(*) FROM delivery)=1 AND (SELECT count(*) FROM owners)=1
  AND EXISTS (SELECT 1 FROM owners o JOIN send s ON (o.source,o.pos,o.block)=(s.source,s.pos,s.block))
LIMIT 2;
