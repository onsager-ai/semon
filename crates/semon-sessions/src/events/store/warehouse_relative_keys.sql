-- Canonical keys for an explicit owner set, including native-ID reservations.
-- All collision and reservation checks start at the indexed native ID.
WITH RECURSIVE owners AS MATERIALIZED (
  SELECT d.source AS owner,d.harness,d.native_id FROM json_each(?1) selected
  CROSS JOIN dim_session d ON d.source=selected.value
), collisions AS (
  SELECT DISTINCT o.native_id FROM owners o
  CROSS JOIN dim_session d ON d.native_id=o.native_id AND d.harness<>o.harness
  CROSS JOIN bridge_session_owner m ON m.source=d.source AND m.owner=d.source
), candidates(owner,harness,native_id,candidate) AS (
  SELECT o.owner,o.harness,o.native_id,
    CASE WHEN c.native_id IS NULL THEN o.native_id ELSE o.harness||':'||o.native_id END
  FROM owners o LEFT JOIN collisions c USING(native_id)
  UNION
  SELECT c.owner,c.harness,c.native_id,':'||c.candidate FROM candidates c
  CROSS JOIN dim_session d ON d.native_id=c.candidate
  WHERE c.candidate<>c.native_id
)
SELECT c.owner,c.harness,c.native_id,c.candidate FROM candidates c
LEFT JOIN dim_session reserved ON reserved.native_id=c.candidate
WHERE c.candidate=c.native_id OR reserved.source IS NULL;
