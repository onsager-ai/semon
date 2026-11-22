-- Resolve the successful correlated result of one physical send inside the
-- certified trace prefix. Ambiguous call IDs and deliveries are unavailable.
WITH origin AS MATERIALIZED (
  SELECT m.source,json_extract(m.receipt,'$.offset') AS end
  FROM warehouse_identity_member m
  CROSS JOIN fact_record r ON r.source=m.source AND r.pos=?3 AND r.block=?4 AND r.occurrence=?5
  WHERE m.identity=?1 AND json_extract(m.receipt,'$.path')=?2
    AND NOT r.gap AND r.pos<json_extract(m.receipt,'$.offset')
), send AS MATERIALIZED (
  SELECT s.source,s.call_id,s.pos,s.block,o.end FROM origin o
  CROSS JOIN fact_relay_send s ON s.source=o.source AND s.pos=?3 AND s.block=?6
  WHERE s.call_id IS NOT NULL
), delivery AS MATERIALIZED (
  SELECT DISTINCT r.correlation FROM send s
  CROSS JOIN fact_relay_result r INDEXED BY relay_result_call
    ON r.source=s.source AND r.call_id=s.call_id AND r.pos<s.end
    AND ((r.call_pos IS NULL AND (SELECT count(*) FROM fact_tool_call unique_call
            WHERE unique_call.source=s.source AND unique_call.call_id=s.call_id AND unique_call.pos<s.end)=1)
       OR (r.call_pos=s.pos AND r.call_block=s.block))
  WHERE NOT r.failed AND r.correlation IS NOT NULL AND (SELECT count(*) FROM send)=1
  LIMIT 2
)
SELECT r.source,r.pos,f.occurrence,r.correlation,coalesce(f.ts,-9223372036854775808) FROM delivery d
CROSS JOIN json_each(?7) m
CROSS JOIN fact_relay_receipt r INDEXED BY relay_receipt_correlation
  ON r.correlation=d.correlation AND r.source=CAST(m.key AS INTEGER) AND r.pos<m.value
CROSS JOIN fact_record f ON f.source=r.source AND f.pos=r.pos AND f.block=0 AND NOT f.gap
WHERE (SELECT count(*) FROM delivery)=1
LIMIT 2;
