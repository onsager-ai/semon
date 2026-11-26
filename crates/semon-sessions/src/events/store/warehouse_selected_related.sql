WITH owners(owner) AS (SELECT value FROM json_each(?1)), own AS MATERIALIZED (
  SELECT m.source FROM owners o
  CROSS JOIN lineage_member root ON root.source=o.owner
  CROSS JOIN lineage_member m ON m.component=root.component
), messages AS MATERIALIZED (
  SELECT DISTINCT u.message_id FROM own m
  CROSS JOIN fact_usage_observation u ON u.source=m.source AND u.kind='per_response'
  WHERE ?3 AND u.uuid IS NOT NULL
), sources(source) AS (
  SELECT source FROM own
  UNION
  SELECT other.source FROM messages ids
  CROSS JOIN fact_usage_observation other INDEXED BY usage_message
    ON other.message_id=ids.message_id AND other.kind='per_response' AND other.uuid IS NOT NULL
)
SELECT c.path,s.harness,s.ledger FROM sources a
CROSS JOIN warehouse_current c ON c.source=a.source
CROSS JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset');
