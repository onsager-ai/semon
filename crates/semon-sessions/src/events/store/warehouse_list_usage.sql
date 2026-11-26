, ranked AS (
  SELECT m.owner,u.*,row_number() OVER (PARTITION BY m.owner,u.kind,u.message_id
    ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,u.pos DESC,u.block DESC,u.occurrence DESC) AS n
  FROM member m CROSS JOIN fact_usage_observation u USING(source)
  CROSS JOIN dim_session d USING(source)
  WHERE u.scope='response' AND u.accepted IS NOT FALSE
), response_cohort AS MATERIALIZED (
  SELECT DISTINCT u.source FROM member m
  CROSS JOIN fact_usage_observation u ON u.source=m.source AND u.kind='accepted_response'
  UNION
  SELECT m.source FROM member m
  CROSS JOIN fact_session_observation h ON h.source=m.source AND h.pos=-1 AND h.kind='header'
  WHERE json_extract(h.metadata,'$.history.forked_from_id') IS NOT NULL
), thread_snapshots AS MATERIALIZED (
  SELECT m.owner,u.model,u.ts,u.pos,u.input,u.cached,u.output,u.provider,
    max(0,u.input-lag(u.input,1,0) OVER ordered) AS input_delta,
    max(0,u.cached-lag(u.cached,1,0) OVER ordered) AS cached_delta,
    max(0,u.output-lag(u.output,1,0) OVER ordered) AS output_delta,
    row_number() OVER(PARTITION BY m.source ORDER BY u.pos DESC,u.block DESC,u.occurrence DESC) AS newest
  FROM member m CROSS JOIN fact_usage_observation u ON u.source=m.source
    AND u.kind='cumulative_snapshot' AND u.scope='thread'
  LEFT JOIN response_cohort n ON n.source=m.source WHERE n.source IS NULL
  WINDOW ordered AS (PARTITION BY m.source ORDER BY u.pos,u.block,u.occurrence)
), latest_session AS MATERIALIZED (
  SELECT m.source,m.owner,max(s.pos) AS pos FROM member m
  CROSS JOIN fact_usage_observation s USING(source)
  WHERE s.kind='cumulative_snapshot' AND s.scope='session' AND s.item=0 GROUP BY m.source,m.owner
)
SELECT u.owner,u.scope,u.model,u.input,u.cached,u.output,
  CASE WHEN s.owner IS NOT NULL THEN 'copied' ELSE 'exclusive' END AS ownership,
  u.ts,u.billing,u.message_id,u.provider,u.cost_provenance,u.coverage,u.reasoning,s.evidence_uuid
FROM ranked u LEFT JOIN shared s ON u.kind='per_response' AND s.owner=u.owner AND s.message_id=u.message_id
WHERE u.n=1
UNION ALL
SELECT m.owner,u.scope,u.model,u.input,u.cached,u.output,'unknown',u.ts,NULL,u.model,NULL,u.cost_provenance,u.coverage,u.reasoning,NULL
FROM latest_session m CROSS JOIN fact_usage_observation u ON u.source=m.source AND u.pos=m.pos
  AND u.kind='cumulative_snapshot' AND u.scope='session'
UNION ALL
SELECT owner,'thread',model,
  CASE WHEN newest=1 THEN input ELSE 0 END,
  CASE WHEN newest=1 THEN cached ELSE 0 END,
  CASE WHEN newest=1 THEN output ELSE 0 END,
  'unknown',ts,json_object('input',max(0,input_delta-cached_delta),'output',output_delta,
                   'cache_read',cached_delta,'cache_write_5m',0,'cache_write_1h',0,
                   'web_search_requests',0,'speed',NULL,'service_tier',NULL,
                   'prompt_size',max(input_delta,cached_delta),'timestamp',ts,'split_unknown',json('false')),
  printf('%020d',pos),provider,'snapshot','snapshot',0,NULL
FROM thread_snapshots WHERE model IS NOT NULL OR newest=1
ORDER BY 1,2,10
