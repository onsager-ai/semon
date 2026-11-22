-- Parent evidence is selected at read time. A marker written outside the
-- committed prefix cannot retroactively convert a prompt into a launch.
WITH parents AS (
    SELECT p.source,0 AS priority,0 AS pos
    FROM dim_session c JOIN dim_session p ON p.harness=c.harness AND p.native_id=c.parent_id
    WHERE c.source=?1 AND c.harness='codex'
    UNION ALL
    SELECT e.parent,0,0 FROM parent_selection e
    JOIN dim_session c ON c.source=e.child AND c.harness='claude' AND NOT c.lineage
    WHERE e.child=?1 AND e.method='native'
    UNION ALL
    SELECT p.source,1,e.pos FROM fact_parent_observation e
    JOIN dim_session c ON c.source=e.source AND c.harness='codex' AND c.parent_id IS NULL
    JOIN dim_session p ON p.harness=e.parent_harness AND p.native_id=e.parent_id
    WHERE e.source=?1 AND e.pos<?2 AND e.method='marker'
)
SELECT c.path,s.ledger,(SELECT min(pos) FROM fact_prompt WHERE source=?1 AND kind='user' AND pos<?2),s.source,s.harness
FROM parents p JOIN warehouse_current c ON c.source=p.source
JOIN warehouse_source s ON s.source=p.source AND s.origin=?3 AND s.carrier=c.path
WHERE p.source<>?1 ORDER BY p.priority,p.pos DESC LIMIT 1;
