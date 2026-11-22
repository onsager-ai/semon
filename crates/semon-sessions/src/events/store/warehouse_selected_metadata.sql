WITH eligible_keys AS MATERIALIZED (
  SELECT json_extract(value,'$[0]') AS owner,json_extract(value,'$[1]') AS id,
    json_extract(value,'$[2]') AS harness FROM json_each(?4)
), selected_members AS MATERIALIZED (
  SELECT m.component,m.source,d.first_ts,d.last_ts FROM eligible_keys k
  CROSS JOIN lineage_member root ON root.source=k.owner
  CROSS JOIN lineage_member m ON m.component=root.component
  CROSS JOIN dim_session d ON d.source=m.source
), admitted AS MATERIALIZED (
  SELECT c.source,c.path,s.ledger,f.title,f.agent_name,f.cwd,f.branch
  FROM selected_members m
  CROSS JOIN warehouse_current c ON c.source=m.source
  CROSS JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
  CROSS JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
  JOIN json_each(?1) paths ON paths.value=c.path
), components AS (
  SELECT m.component,max(m.last_ts) AS last,min(m.first_ts) AS start,count(a.source) AS qualified,count(*) AS total
  FROM selected_members m LEFT JOIN admitted a USING(source) GROUP BY m.component
