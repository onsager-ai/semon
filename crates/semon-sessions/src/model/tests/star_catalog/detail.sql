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
), selected AS MATERIALIZED (
  SELECT l.component FROM candidates c JOIN dim_lineage l ON l.source=c.owner
  LEFT JOIN dim_session reserved ON reserved.native_id=c.candidate
  WHERE c.candidate=?1 AND (c.candidate=c.native_id OR reserved.source IS NULL)
), members AS MATERIALIZED (
  SELECT l.source FROM selected s CROSS JOIN dim_lineage l USING(component)
), suppressed AS MATERIALIZED (
  SELECT DISTINCT target.source,old.uuid FROM members member
  CROSS JOIN fact_relation_observation e ON e.source=member.source AND e.kind='continued-in'
  CROSS JOIN fact_record old ON old.source=e.source AND old.uuid IS NOT NULL
  CROSS JOIN dim_session target ON target.harness='claude' AND target.native_id=e.target
)
SELECT r.source,r.pos,r.kind FROM members m CROSS JOIN fact_record r USING(source)
LEFT JOIN suppressed s ON s.source=r.source AND s.uuid=r.uuid
WHERE s.source IS NULL ORDER BY r.ts,r.source,r.pos;
