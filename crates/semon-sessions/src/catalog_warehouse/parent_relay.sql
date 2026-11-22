WITH sends AS MATERIALIZED (
    SELECT s.* FROM fact_relay_send s
    WHERE s.source=?1 AND s.target='main' AND s.pos<?2
      AND NOT EXISTS (
          SELECT 1 FROM fact_relay_result r WHERE r.source=s.source AND r.call_id=s.call_id
            AND r.pos<?2 AND (r.call_pos IS NULL OR (r.call_pos=s.pos AND r.call_block=s.block))
            AND (r.failed OR r.correlation IS NOT NULL OR r.flags & ?3<>0)
      )
    ORDER BY s.pos,s.block,s.occurrence LIMIT 129
), windows AS (
    SELECT s.*,min(coalesce((SELECT min(p.pos) FROM fact_prompt p
           WHERE p.source=s.source AND p.pos>s.pos AND p.kind='user'),?2),
           coalesce(lead(s.pos) OVER (ORDER BY pos,block,occurrence),?2),?2) AS stop
    FROM sends s
)
SELECT s.pos,s.block,s.call_id,s.ts,(
    SELECT max(time) FROM (
        SELECT r.ts AS time FROM fact_record r
        WHERE r.source=s.source AND r.pos>=s.pos AND r.pos<s.stop AND NOT r.gap
        UNION ALL
        SELECT r.ts FROM fact_tool_result r
        WHERE r.source=s.source AND r.pos<?2 AND (r.call_pos>=s.pos AND r.call_pos<s.stop
           OR r.call_pos IS NULL AND r.call_id=s.call_id)
    )
) FROM windows s ORDER BY s.pos,s.block,s.occurrence;
