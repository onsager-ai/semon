-- Count the same signal marks as the selected annotations page.
WITH members AS MATERIALIZED (
    SELECT m.source,d.native_id,d.harness,CAST(json_extract(s.ledger,'$.offset') AS INTEGER) AS end
    FROM lineage_member root
    CROSS JOIN lineage_member m ON m.component=root.component
    CROSS JOIN dim_session d ON d.source=m.source
    CROSS JOIN warehouse_source s ON s.source=m.source
    WHERE root.source=?1
), copies AS MATERIALIZED (
    SELECT m.source AS target,old.source,old.end
    FROM members m
    CROSS JOIN fact_relation_observation edge ON edge.target=m.native_id AND edge.kind='continued-in'
    CROSS JOIN members old ON old.source=edge.source AND edge.pos<old.end
    CROSS JOIN dim_session d ON d.source=old.source AND d.lineage
    WHERE m.harness='claude' AND m.source<>old.source
), visible AS (
    SELECT DISTINCT r.source,r.pos,r.block,r.occurrence,json_extract(s.payload,'$.k') AS signal
    FROM members m
    CROSS JOIN warehouse_signal s ON s.source=m.source AND s.pos<m.end AND s.retired_revision IS NULL
    CROSS JOIN fact_record r ON r.source=s.source AND r.pos=s.pos AND NOT r.gap
    WHERE NOT EXISTS (
        SELECT 1 FROM copies c
        CROSS JOIN fact_record old ON old.source=c.source AND old.uuid=r.uuid AND old.pos<c.end
        WHERE c.target=m.source AND r.uuid IS NOT NULL
    )
)
SELECT signal,count(*) FROM visible GROUP BY signal;
