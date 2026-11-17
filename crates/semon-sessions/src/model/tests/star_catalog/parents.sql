-- Precedence and cycle qualification cross lineage boundaries at read time.
CREATE VIEW parent_selection AS
WITH RECURSIVE native AS MATERIALIZED (
  SELECT d.source AS child,p.source AS parent,NULL AS call_id,'native' AS method,
    0 AS priority,0 AS ordinal
  FROM dim_session d JOIN dim_session p
    ON p.harness=d.harness AND p.native_id=d.parent_id
), evidence AS (
  SELECT e.source AS child,p.source AS parent,e.call_id,e.method,
    CASE e.method WHEN 'marker' THEN 3 WHEN 'environment' THEN 2 ELSE 1 END AS priority,
    e.pos AS ordinal
  FROM fact_parent_observation e JOIN dim_session p
    ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  WHERE e.source<>p.source
), qualified AS MATERIALIZED (
  SELECT * FROM fact_process_observation WHERE qualified=1
), unique_pid AS (
  SELECT epoch,pid,min(source) AS source FROM qualified GROUP BY epoch,pid HAVING count(*)=1
), ancestors AS (
  -- Stop at the first known ancestor pid even when its session is ambiguous.
  SELECT a.*,row_number() OVER(PARTITION BY a.epoch,a.source ORDER BY a.ordinal) AS n
  FROM fact_ancestor_observation a JOIN qualified child
    ON child.epoch=a.epoch AND child.source=a.source
  JOIN (SELECT DISTINCT epoch,pid FROM qualified) known USING(epoch,pid)
), offered AS (
  SELECT * FROM native UNION ALL SELECT * FROM evidence
  UNION ALL
  SELECT e.source,p.source,e.call_id,'environment',2,e.epoch
  FROM fact_environment_observation e JOIN qualified child USING(epoch,source)
  JOIN dim_session p ON p.harness=e.parent_harness AND p.native_id=e.parent_id
  WHERE e.source<>p.source
  UNION ALL
  SELECT a.source,p.source,NULL,'ancestry',1,a.epoch
  FROM ancestors a JOIN unique_pid p USING(epoch,pid)
  LEFT JOIN native n ON n.child=a.source
  WHERE a.n=1 AND n.child IS NULL AND a.source<>p.source
), ranked AS (
  SELECT *,row_number() OVER(PARTITION BY child ORDER BY priority DESC,ordinal DESC) AS n
  FROM offered
), chosen AS MATERIALIZED (SELECT * FROM ranked WHERE n=1),
parent_chain(child,node) AS (
  SELECT child,parent FROM chosen WHERE method<>'native' UNION
  SELECT r.child,c.parent FROM parent_chain r JOIN chosen c ON c.child=r.node
), cyclic AS (SELECT DISTINCT child FROM parent_chain WHERE child=node)
SELECT c.child,c.parent,c.call_id,c.method FROM chosen c
LEFT JOIN cyclic bad USING(child)
-- The current builder revokes observed links on cycles, retaining a native link.
WHERE bad.child IS NULL OR c.method='native'
UNION ALL
SELECT n.child,n.parent,n.call_id,n.method FROM native n
JOIN chosen c USING(child) JOIN cyclic bad USING(child)
WHERE c.method<>'native';
