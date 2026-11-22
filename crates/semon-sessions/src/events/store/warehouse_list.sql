WITH admitted AS MATERIALIZED (
  SELECT c.source,c.path,s.ledger,f.title,f.agent_name,f.cwd,f.branch
  FROM json_each(?1) paths
  JOIN warehouse_current c ON c.path=paths.value
  JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
  JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
), components AS (
  SELECT m.component,max(d.last_ts) AS last,min(d.first_ts) AS start,count(a.source) AS qualified,count(*) AS total
  FROM lineage_member m JOIN lineage_group g ON g.id=m.component
  JOIN dim_session d USING(source) LEFT JOIN admitted a USING(source)
  GROUP BY m.component
), eligible_keys AS (
  SELECT id,owner,harness FROM catalog_keys WHERE (?4 IS NULL OR harness=?4)
