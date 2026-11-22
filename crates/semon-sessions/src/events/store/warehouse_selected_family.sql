-- Validate every member of the selected graph against current source authority.
SELECT d.source,d.harness,
    NOT EXISTS (
        SELECT 1 FROM lineage_member root
        CROSS JOIN lineage_member part ON part.component=root.component
        LEFT JOIN warehouse_current c ON c.source=part.source
        LEFT JOIN warehouse_source s ON s.source=part.source AND s.origin=?3 AND s.carrier=c.path
        LEFT JOIN json_each(?2) authorized ON authorized.value=s.carrier
        WHERE root.source=d.source AND authorized.value IS NULL
    )
FROM json_each(?1) m CROSS JOIN dim_session d ON d.source=m.value;
