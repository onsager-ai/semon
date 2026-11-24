SELECT CAST(p.key AS INTEGER),s.block,s.target,s.ts,(
  SELECT min(t.target) FROM fact_tool_result t INDEXED BY tool_result_identity
  WHERE t.source=s.source AND t.call_id=s.call_id AND t.call_pos=s.pos AND t.call_block=s.block
    AND t.pos<json_extract(p.value,'$[2]') AND NOT t.failed AND t.flags & ?2<>0 AND t.correlation IS NULL
  HAVING count(DISTINCT t.target)=1
), EXISTS (SELECT 1 FROM fact_relay_result t INDEXED BY relay_result_call
  WHERE t.source=s.source AND t.call_id=s.call_id AND t.call_pos=s.pos AND t.call_block=s.block
    AND t.pos>s.pos AND t.pos<json_extract(p.value,'$[2]') AND NOT t.failed AND t.correlation IS NOT NULL)
FROM json_each(?1) p
CROSS JOIN fact_relay_send s ON s.source=json_extract(p.value,'$[0]') AND s.pos=json_extract(p.value,'$[1]')
WHERE s.ts IS NOT NULL ORDER BY CAST(p.key AS INTEGER),s.block,s.occurrence;
