-- The candidate parent must be a certified Trace participant. Only immutable
-- metadata and marker facts inside the child's certified prefix can place a launch.
WITH native AS (
    SELECT parent_id FROM fact_session_observation
    WHERE source=?1 AND kind='header'
      AND EXISTS (SELECT 1 FROM fact_record r WHERE r.source=?1
                  AND r.kind='session_meta' AND r.pos=0 AND r.pos<?2 AND NOT r.gap)
    ORDER BY pos DESC,occurrence DESC LIMIT 1
), marker AS (
    SELECT parent_harness,parent_id FROM fact_parent_observation
    WHERE source=?1 AND pos<?2 AND method='marker'
    ORDER BY pos DESC,block DESC,occurrence DESC LIMIT 1
), parent AS (
    SELECT 'codex' AS harness,parent_id FROM native WHERE parent_id IS NOT NULL
    UNION ALL
    SELECT parent_harness,parent_id FROM marker
    WHERE NOT EXISTS (SELECT 1 FROM native WHERE parent_id IS NOT NULL)
)
SELECT (SELECT min(pos) FROM fact_prompt WHERE source=?1 AND kind='user' AND pos<?2)
FROM parent p
WHERE EXISTS (SELECT 1 FROM json_each(?3) peer
              WHERE json_extract(peer.value,'$[0]')=p.harness
                AND json_extract(peer.value,'$[1]')=p.parent_id);
