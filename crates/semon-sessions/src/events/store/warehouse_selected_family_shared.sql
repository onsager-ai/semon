, usage AS (
    SELECT m.owner,u.*,row_number() OVER (PARTITION BY m.owner,u.message_id
        ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,u.pos DESC,u.block DESC,u.occurrence DESC) AS n
    FROM member m CROSS JOIN fact_usage_observation u USING(source)
    CROSS JOIN dim_session d USING(source) WHERE u.kind='per_response'
), winning AS (
    SELECT u.model,u.billing,row_number() OVER (
        PARTITION BY u.message_id,s.evidence_uuid ORDER BY f.priority
    ) AS n
    FROM page f JOIN shared s ON s.owner=f.owner
    JOIN usage u ON u.owner=f.owner AND u.message_id=s.message_id AND u.n=1
)
SELECT model,billing FROM winning WHERE n=1;
