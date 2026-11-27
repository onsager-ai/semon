WITH RECURSIVE
page AS MATERIALIZED (SELECT value AS owner,key AS priority FROM json_each(?1)),
member AS MATERIALIZED (
  SELECT p.owner,m.source FROM page p
  CROSS JOIN lineage_member root ON root.source=p.owner
  CROSS JOIN lineage_member m ON m.component=root.component
), messages AS MATERIALIZED (
  SELECT DISTINCT u.message_id FROM member m
  CROSS JOIN fact_usage_observation u ON u.source=m.source
  WHERE u.kind='per_response' AND u.uuid IS NOT NULL
), identities AS MATERIALIZED (
  SELECT DISTINCT u.message_id,u.uuid,b.owner FROM messages ids
  CROSS JOIN fact_usage_observation u INDEXED BY usage_message ON u.message_id=ids.message_id
  CROSS JOIN bridge_session_owner b ON b.source=u.source
  CROSS JOIN warehouse_current c ON c.source=u.source
  CROSS JOIN warehouse_source s ON s.source=c.source
  WHERE u.kind='per_response' AND u.uuid IS NOT NULL
    AND ((?2 IS NOT NULL AND c.path IN (SELECT value FROM json_each(?2)))
      OR (?2 IS NULL AND s.origin=?3))
), edges AS MATERIALIZED (
  SELECT a.message_id,a.owner,b.owner AS other,a.uuid FROM identities a
  JOIN identities b ON b.message_id=a.message_id AND b.uuid=a.uuid AND b.owner<>a.owner
), connected(message_id,owner,member) AS (
  SELECT message_id,owner,other FROM edges
  UNION
  SELECT c.message_id,c.owner,e.other FROM connected c
  JOIN edges e ON e.message_id=c.message_id AND e.owner=c.member
), shared AS (
  SELECT c.message_id,c.owner,min(e.uuid) AS evidence_uuid FROM connected c
  JOIN edges e ON e.message_id=c.message_id AND e.owner=c.member
  GROUP BY c.message_id,c.owner
)
