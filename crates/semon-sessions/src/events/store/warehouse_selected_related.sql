WITH owners(owner) AS (SELECT value FROM json_each(?1)), own AS MATERIALIZED (
  SELECT m.source FROM owners o
  CROSS JOIN lineage_member root ON root.source=o.owner
  CROSS JOIN lineage_member m ON m.component=root.component
), sources(source) AS (
  SELECT source FROM own
  UNION
  SELECT other.source FROM own m
  CROSS JOIN fact_step_observation u ON u.source=m.source AND u.kind='claude'
  CROSS JOIN fact_step_observation other INDEXED BY usage_message
    ON other.message_id=u.message_id AND other.uuid=u.uuid AND other.kind='claude'
  WHERE u.uuid IS NOT NULL
)
SELECT c.path,s.harness,s.ledger FROM sources a
CROSS JOIN warehouse_current c ON c.source=a.source
CROSS JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset');
