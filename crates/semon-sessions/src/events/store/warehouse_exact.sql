-- Resolve only possible native IDs encoded by the requested catalog key.
-- Qualification adds harness: and zero or more leading colons. Every possible
-- native ID is therefore a suffix after a colon (or the original key).
WITH RECURSIVE probes(native_id) AS (
  SELECT ?1
  UNION
  SELECT substr(native_id,instr(native_id,':')+1) FROM probes
  WHERE instr(native_id,':')>0
), owners AS MATERIALIZED (
  SELECT d.source AS owner,d.harness,d.native_id FROM probes p
  JOIN dim_session d ON d.native_id=p.native_id
  JOIN bridge_session_owner m ON m.source=d.source AND m.owner=d.source
), collisions AS (
  SELECT DISTINCT o.native_id FROM owners o
  CROSS JOIN dim_session d ON d.native_id=o.native_id AND d.harness<>o.harness
  CROSS JOIN bridge_session_owner m ON m.source=d.source AND m.owner=d.source
), candidates(owner,native_id,candidate) AS (
  SELECT o.owner,o.native_id,
    CASE WHEN c.native_id IS NULL THEN o.native_id ELSE o.harness||':'||o.native_id END
  FROM owners o LEFT JOIN collisions c USING(native_id)
  UNION
  SELECT c.owner,c.native_id,':'||c.candidate FROM candidates c
  JOIN dim_session d ON d.native_id=c.candidate
  WHERE c.candidate<>c.native_id
), exact AS MATERIALIZED (
  SELECT c.owner FROM candidates c
  LEFT JOIN dim_session reserved ON reserved.native_id=c.candidate
  WHERE c.candidate=?1 AND (c.candidate=c.native_id OR reserved.source IS NULL)
), members AS (
  SELECT e.owner,count(*) AS total,count(f.file_id) AS qualified
  FROM exact e CROSS JOIN lineage_member original ON original.source=e.owner
  CROSS JOIN lineage_member m ON m.component=original.component
  LEFT JOIN warehouse_current current ON current.source=m.source
  LEFT JOIN json_each(?2) paths ON paths.value=current.path
  LEFT JOIN warehouse_source s ON s.source=m.source AND s.origin=?3 AND s.carrier=paths.value
  LEFT JOIN files f ON f.path=paths.value AND f.resume_at=json_extract(s.ledger,'$.offset')
  GROUP BY e.owner
)
SELECT m.owner,d.harness,d.native_id FROM members m
CROSS JOIN dim_session d ON d.source=m.owner WHERE m.total=m.qualified
