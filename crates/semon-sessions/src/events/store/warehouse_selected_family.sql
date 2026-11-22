-- Limit the recursive walk to the requested family. Preserve a cycle marker
-- separately because a visited set alone would turn a cycle into a subtotal.
WITH RECURSIVE edges AS MATERIALIZED (SELECT * FROM family_edges),
members(owner) AS (
    SELECT ?1 UNION SELECT e.child FROM members m JOIN edges e ON e.parent=m.owner
)
SELECT m.owner,d.harness,
    EXISTS (SELECT 1 FROM edges e JOIN members p ON p.owner=e.parent WHERE e.child=?1),
    NOT EXISTS (
        SELECT 1 FROM lineage_member root
        CROSS JOIN lineage_member part ON part.component=root.component
        LEFT JOIN warehouse_current c ON c.source=part.source
        LEFT JOIN warehouse_source s ON s.source=part.source AND s.origin=?3 AND s.carrier=c.path
        LEFT JOIN json_each(?2) authorized ON authorized.value=s.carrier
        WHERE root.source=m.owner AND authorized.value IS NULL
    )
FROM members m JOIN dim_session d ON d.source=m.owner ORDER BY m.owner;
