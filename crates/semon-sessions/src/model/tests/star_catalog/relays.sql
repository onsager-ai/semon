CREATE VIEW relay_endpoints AS
WITH receipts AS MATERIALIZED (
  SELECT r.*,m.owner,row_number() OVER(PARTITION BY r.correlation
    ORDER BY d.native_id,r.pos) AS n
  FROM fact_relay_receipt r JOIN bridge_session_owner m USING(source)
  JOIN dim_session d ON d.source=m.owner
), sends AS MATERIALIZED (
  SELECT s.*,m.owner,r.correlation,r.failed FROM fact_relay_send s
  JOIN bridge_session_owner m USING(source)
  JOIN fact_relay_result r ON r.source=s.source AND r.call_id=s.call_id
), names AS (
  SELECT DISTINCT s.owner,r.name FROM sends s
  JOIN receipts r ON r.correlation=s.correlation AND r.n=1
  WHERE r.name<>''
), unique_names AS (
  SELECT name,min(owner) AS owner FROM names GROUP BY name HAVING count(*)=1
)
SELECT s.source,s.pos,s.call_id,s.owner AS sender,
  coalesce(r.owner,CASE WHEN s.failed=0 AND s.correlation IS NULL THEN n.owner END) AS receiver,
  CASE WHEN r.owner IS NULL AND s.failed=0 AND (s.correlation IS NOT NULL OR n.owner IS NULL)
    THEN 'name:'||s.target END AS stub,
  s.failed,s.correlation,s.ts
FROM sends s LEFT JOIN receipts r ON r.correlation=s.correlation AND r.n=1
LEFT JOIN unique_names n ON n.name=s.target;
