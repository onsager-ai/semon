-- Start from an indexed native alias and derive keys only for its owners.
-- Follow the same collision and reserved-name rules as the list's Catalog keys.
WITH RECURSIVE aliases(owner) AS MATERIALIZED (
  SELECT DISTINCT b.owner FROM dim_session native
  CROSS JOIN bridge_session_owner b ON b.source=native.source
  WHERE native.harness=?1 AND native.native_id=?2
), owners AS MATERIALIZED (
  SELECT d.source AS owner,d.harness,d.native_id FROM aliases a
  CROSS JOIN dim_session d ON d.source=a.owner
), collisions AS MATERIALIZED (
  SELECT DISTINCT o.native_id FROM owners o
  CROSS JOIN dim_session d ON d.native_id=o.native_id AND d.harness<>o.harness
  CROSS JOIN bridge_session_owner b ON b.source=d.source AND b.owner=d.source
), candidates(owner,harness,native_id,candidate) AS (
  SELECT o.owner,o.harness,o.native_id,
    CASE WHEN c.native_id IS NULL THEN o.native_id ELSE o.harness||':'||o.native_id END
  FROM owners o LEFT JOIN collisions c USING(native_id)
  UNION
  SELECT c.owner,c.harness,c.native_id,':'||c.candidate FROM candidates c
  JOIN dim_session reserved ON reserved.native_id=c.candidate
  WHERE c.candidate<>c.native_id
)
SELECT DISTINCT c.owner,c.harness,c.native_id,c.candidate FROM candidates c
LEFT JOIN dim_session reserved ON reserved.native_id=c.candidate
WHERE c.candidate=c.native_id OR reserved.source IS NULL
ORDER BY c.candidate;
