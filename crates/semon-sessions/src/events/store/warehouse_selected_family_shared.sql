-- Use every authorized Claude UUID observation of the family's message IDs.
-- A message can connect owners transitively through several different UUIDs.
WITH RECURSIVE fold AS MATERIALIZED (
    SELECT value AS member,key AS priority FROM json_each(?1)
), members AS MATERIALIZED (
    SELECT family.member AS owner,m.source FROM fold family
    CROSS JOIN lineage_member root ON root.source=family.member
    CROSS JOIN lineage_member m ON m.component=root.component
), messages AS MATERIALIZED (
    SELECT DISTINCT u.message_id FROM members m
    CROSS JOIN fact_step_observation u ON u.source=m.source
    WHERE u.kind='claude' AND u.uuid IS NOT NULL
), identities AS MATERIALIZED (
    SELECT DISTINCT u.message_id,u.uuid,m.owner FROM messages ids
    CROSS JOIN fact_step_observation u INDEXED BY usage_message ON u.message_id=ids.message_id
    CROSS JOIN bridge_session_owner m ON m.source=u.source
    CROSS JOIN warehouse_current c ON c.source=u.source
    CROSS JOIN json_each(?2) authorized ON authorized.value=c.path
    WHERE u.kind='claude' AND u.uuid IS NOT NULL
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
), usage AS (
    SELECT m.owner,u.*,row_number() OVER (PARTITION BY m.owner,u.message_id
        ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,u.pos DESC,u.block DESC,u.occurrence DESC) AS n
    FROM members m CROSS JOIN fact_step_observation u USING(source)
    CROSS JOIN dim_session d USING(source) WHERE u.kind='claude'
), winning AS (
    SELECT u.model,b.billing,row_number() OVER (
        PARTITION BY u.message_id,s.evidence_uuid ORDER BY f.priority
    ) AS n
    FROM fold f JOIN shared s ON s.owner=f.member
    JOIN usage u ON u.owner=f.member AND u.message_id=s.message_id AND u.n=1
    LEFT JOIN fact_step_billing b ON b.source=u.source AND b.pos=u.pos AND b.block=u.block
        AND b.kind=u.kind AND b.occurrence=u.occurrence
)
SELECT model,billing FROM winning WHERE n=1;
