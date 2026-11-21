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
), latest_copilot AS MATERIALIZED (
  SELECT m.source,m.owner,max(s.pos) AS pos FROM member m
  CROSS JOIN fact_copilot_shutdown s USING(source) GROUP BY m.source,m.owner
)
SELECT u.owner,
  sum(CASE WHEN s.owner IS NULL THEN u.input ELSE 0 END),
  sum(CASE WHEN s.owner IS NULL THEN u.cached ELSE 0 END),
  sum(CASE WHEN s.owner IS NULL THEN u.output ELSE 0 END),
  sum(CASE WHEN s.owner IS NOT NULL THEN u.input ELSE 0 END),
  sum(CASE WHEN s.owner IS NOT NULL THEN u.cached ELSE 0 END),
  sum(CASE WHEN s.owner IS NOT NULL THEN u.output ELSE 0 END)
FROM ranked u LEFT JOIN shared s ON s.owner=u.owner AND s.message_id=u.message_id
WHERE u.n=1 GROUP BY u.owner
UNION ALL
SELECT m.owner,sum(u.input),sum(u.cached),sum(u.output),0,0,0
FROM latest_copilot m CROSS JOIN fact_copilot_usage u ON u.source=m.source AND u.pos=m.pos
GROUP BY m.owner
