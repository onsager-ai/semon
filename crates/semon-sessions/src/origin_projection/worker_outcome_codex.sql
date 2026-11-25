-- The placement already qualifies uniqueness of the parent spawn and child.
WITH dispatch AS (
  SELECT min(c.call_id) AS call_id
  FROM dim_session child JOIN bridge_spawn_placement b ON b.child=child.native_id
  JOIN fact_tool_call c ON c.source=b.parent_source AND c.pos=b.pos
    AND c.tool IN ('spawn_agent','collaboration.spawn_agent')
  WHERE child.source=?1 AND b.parent_source=?2
  HAVING count(*)=1 AND min(c.call_id) IS NOT NULL
), terminal AS (
  SELECT pos,phase FROM fact_codex_turn WHERE source=?1 ORDER BY pos DESC LIMIT 1
)
SELECT d.call_id,o.disposition,o.pr_url,o.head_sha,o.checks_state,coalesce(t.phase='errored',0)
FROM dispatch d LEFT JOIN terminal t ON true
LEFT JOIN fact_worker_outcome o ON o.source=?1 AND o.pos=t.pos AND t.phase='completed'
  AND (SELECT count(*) FROM fact_codex_turn WHERE source=?1 AND phase='active')=1
