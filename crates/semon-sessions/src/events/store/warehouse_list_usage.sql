WITH page AS MATERIALIZED (SELECT value AS owner FROM json_each(?1)),
member AS MATERIALIZED (
  SELECT p.owner,m.source FROM page p
  CROSS JOIN lineage_member root ON root.source=p.owner
  CROSS JOIN lineage_member m ON m.component=root.component
), ranked AS (
  SELECT m.owner,u.*,row_number() OVER (PARTITION BY m.owner,u.message_id
    ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,u.pos DESC,u.block DESC,u.occurrence DESC) AS n
  FROM member m CROSS JOIN fact_step_observation u USING(source)
  CROSS JOIN dim_session d USING(source)
  WHERE u.kind='claude' OR (u.kind='codex-response' AND u.accepted)
), shared AS MATERIALIZED (
  SELECT DISTINCT m.owner,own.message_id
  FROM member m CROSS JOIN fact_step_observation own USING(source)
  CROSS JOIN fact_step_observation other INDEXED BY usage_message
    ON other.message_id=own.message_id AND other.uuid=own.uuid AND other.kind='claude'
  CROSS JOIN bridge_session_owner b ON b.source=other.source AND b.owner<>m.owner
  CROSS JOIN warehouse_current c ON c.source=other.source
  CROSS JOIN json_each(?2) authorized ON authorized.value=c.path
  WHERE own.kind='claude' AND own.uuid IS NOT NULL
), native_codex AS MATERIALIZED (
  SELECT DISTINCT source FROM ranked WHERE kind='codex-response'
  UNION
  SELECT m.source FROM member m
  CROSS JOIN fact_session_observation h ON h.source=m.source AND h.pos=-1 AND h.kind='header'
  WHERE json_extract(h.metadata,'$.history.forked_from_id') IS NOT NULL
), legacy AS MATERIALIZED (
  SELECT m.owner,u.model,u.ts,u.pos,u.input,u.cached,u.output,
    max(0,u.input-lag(u.input,1,0) OVER ordered) AS input_delta,
    max(0,u.cached-lag(u.cached,1,0) OVER ordered) AS cached_delta,
    max(0,u.output-lag(u.output,1,0) OVER ordered) AS output_delta,
    row_number() OVER(PARTITION BY m.source ORDER BY u.pos DESC,u.block DESC,u.occurrence DESC) AS newest
  FROM member m CROSS JOIN fact_step_observation u ON u.source=m.source AND u.kind='codex-snapshot'
  LEFT JOIN native_codex n ON n.source=m.source WHERE n.source IS NULL
  WINDOW ordered AS (PARTITION BY m.source ORDER BY u.pos,u.block,u.occurrence)
), latest_copilot AS MATERIALIZED (
  SELECT m.source,m.owner,max(s.pos) AS pos FROM member m
  CROSS JOIN fact_copilot_shutdown s USING(source) GROUP BY m.source,m.owner
)
SELECT u.owner,u.kind,u.model,u.input,u.cached,u.output,s.owner IS NOT NULL,u.ts,b.billing,u.message_id
FROM ranked u LEFT JOIN shared s ON s.owner=u.owner AND s.message_id=u.message_id
LEFT JOIN fact_step_billing b
  ON b.source=u.source AND b.pos=u.pos AND b.block=u.block AND b.kind=u.kind AND b.occurrence=u.occurrence
WHERE u.n=1
UNION ALL
SELECT m.owner,'copilot',u.model,u.input,u.cached,u.output,0,NULL,NULL,u.model
FROM latest_copilot m CROSS JOIN fact_copilot_usage u ON u.source=m.source AND u.pos=m.pos
UNION ALL
SELECT owner,'codex-snapshot',model,
  CASE WHEN newest=1 THEN input ELSE 0 END,
  CASE WHEN newest=1 THEN cached ELSE 0 END,
  CASE WHEN newest=1 THEN output ELSE 0 END,
  0,ts,json_object('input',max(0,input_delta-cached_delta),'output',output_delta,
                   'cache_read',cached_delta,'cache_write',0),printf('%020d',pos)
FROM legacy WHERE model IS NOT NULL OR newest=1
ORDER BY 1,2,10
