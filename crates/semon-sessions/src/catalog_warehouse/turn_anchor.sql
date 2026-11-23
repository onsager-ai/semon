-- Native prompt links carry an offset, and correlated relay links are scoped
-- to the selected member. Every candidate is filtered against the exact
-- prefix and copy bounds before reading its native body.
WITH requested AS (SELECT ?1 AS source,?2 AS end,?3 AS offset,?4 AS launch,?5 AS ask),
boundaries AS (
  SELECT p.pos,p.kind,NULL AS correlation FROM requested q
  CROSS JOIN fact_prompt p ON p.source=q.source AND p.pos<q.end AND p.pos=q.offset
  WHERE p.kind='user'
  UNION ALL
  SELECT p.pos,p.kind,NULL FROM requested q
  CROSS JOIN fact_prompt p ON p.source=q.source AND p.pos<q.end AND p.kind='ask'
  WHERE q.ask
  UNION ALL
  SELECT (SELECT min(p.pos) FROM fact_prompt p
    WHERE p.source=q.source AND p.pos<q.end AND p.kind='user'),'user',NULL
  FROM requested q WHERE q.launch
), possible AS (
  SELECT * FROM boundaries
  UNION ALL
  SELECT p.pos,'relay',p.correlation FROM requested q
  CROSS JOIN fact_relay_receipt p ON p.source=q.source AND p.pos<q.end
  WHERE ?6
  UNION ALL
  SELECT p.pos,'relay',NULL FROM requested q
  CROSS JOIN fact_subagent_event p ON p.source=q.source AND p.pos<q.end
  WHERE ?6 AND p.kind='agm' AND p.flags & ?7 <> 0
  UNION ALL
  SELECT (SELECT min(p.pos) FROM fact_record p
    WHERE p.source=q.source AND p.pos<q.end AND p.pos>=0 AND NOT p.gap),'launch',NULL
  FROM requested q WHERE q.launch
)
SELECT p.pos,p.kind,p.correlation,r.occurrence FROM possible p
CROSS JOIN fact_record r ON r.source=?1 AND r.pos=p.pos AND r.block=0 AND NOT r.gap;
