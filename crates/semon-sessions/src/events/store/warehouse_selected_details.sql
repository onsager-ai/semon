-- Aggregate only the selected component's committed native facts. Use the
-- transcript's copy suppression and call/result identity rules before counting.
WITH members AS MATERIALIZED (
    SELECT m.source,d.native_id,d.harness,p.pos AS end,p.revision
    FROM lineage_member root
    CROSS JOIN lineage_member m ON m.component=root.component
    CROSS JOIN dim_session d ON d.source=m.source
    CROSS JOIN warehouse_prefix p ON p.source=m.source AND p.superseded=0
    WHERE root.source=?1
), copies AS MATERIALIZED (
    SELECT DISTINCT m.source AS target,old.source,old.end
    FROM members m
    CROSS JOIN fact_relation_observation edge ON edge.target=m.native_id AND edge.kind='continued-in'
    CROSS JOIN members old ON old.source=edge.source AND old.end>edge.pos
    CROSS JOIN dim_session d ON d.source=old.source AND d.lineage
    WHERE m.harness='claude' AND m.source<>old.source
), candidates AS (
    SELECT p.source,p.pos FROM members m CROSS JOIN fact_prompt p ON p.source=m.source AND p.pos<m.end
    UNION
    SELECT c.source,c.pos FROM members m CROSS JOIN fact_tool_call c ON c.source=m.source AND c.pos<m.end
    UNION
    SELECT o.source,o.pos FROM members m CROSS JOIN fact_operation o ON o.source=m.source AND o.pos<m.end
    UNION
    SELECT r.source,r.pos FROM members m CROSS JOIN fact_relay_receipt r ON r.source=m.source AND r.pos<m.end
    UNION
    SELECT s.source,s.pos FROM members m CROSS JOIN warehouse_signal s ON s.source=m.source AND s.pos<m.end
      AND s.revision<=m.revision AND (s.retired_revision IS NULL OR s.retired_revision>m.revision)
), visible AS MATERIALIZED (
    SELECT DISTINCT r.source,r.pos
    FROM candidates a
    CROSS JOIN fact_record r ON r.source=a.source AND r.pos=a.pos AND r.pos>=0 AND NOT r.gap
    WHERE NOT EXISTS (
        SELECT 1 FROM copies c
        CROSS JOIN fact_record old ON old.source=c.source AND old.uuid=r.uuid AND old.pos<c.end
        WHERE c.target=a.source AND r.uuid IS NOT NULL
    )
), calls AS (
    SELECT c.*,m.end FROM members m
    CROSS JOIN fact_tool_call c ON c.source=m.source AND c.pos<m.end
    CROSS JOIN visible v ON v.source=c.source AND v.pos=c.pos
    WHERE c.tool IS NOT NULL AND c.call_id IS NOT NULL
), replies AS (
    SELECT c.source,c.pos,c.block,c.occurrence,c.tool,c.end,
           coalesce((
               SELECT r.failed AND r.flags & ?3=0
               FROM fact_tool_result r INDEXED BY tool_result_identity
               WHERE r.source=c.source AND r.call_id=c.call_id AND r.pos<c.end
                 AND ((r.call_pos IS NULL AND r.kind<>'copilot-execution')
                   OR (r.call_pos=c.pos AND r.call_block=c.block))
                 AND r.flags & ?2=0
               ORDER BY coalesce(r.ts,-9223372036854775808) DESC,r.pos DESC,r.block DESC,r.occurrence DESC
               LIMIT 1
           ), -1) AS failed
    FROM calls c
    WHERE (SELECT count(*) FROM fact_tool_call a WHERE a.source=c.source AND a.pos=c.pos AND a.block=c.block)=1
      AND (SELECT count(*) FROM fact_tool_call a WHERE a.source=c.source AND a.call_id=c.call_id AND a.pos<c.end)=1
), visible_calls AS (
    SELECT c.tool,coalesce(r.failed=1,0) AS failed
    FROM calls c
    LEFT JOIN replies r USING(source,pos,block,occurrence)
    WHERE coalesce(r.failed,-1)<>0 OR NOT EXISTS (
        SELECT 1 FROM fact_operation o INDEXED BY operation_parent
        WHERE o.source=c.source AND o.call_pos=c.pos AND o.call_block=c.block AND o.pos<c.end
    )
    UNION ALL
    SELECT o.tool,coalesce(o.ok=0,0)
    FROM members m
    CROSS JOIN fact_operation o ON o.source=m.source AND o.pos<m.end
    CROSS JOIN visible v ON v.source=o.source AND v.pos=o.pos
), turns AS (
    SELECT DISTINCT p.source,p.pos
    FROM visible v CROSS JOIN fact_prompt p USING(source,pos)
    UNION
    SELECT r.source,r.pos FROM visible v CROSS JOIN fact_relay_receipt r USING(source,pos)
), signal_marks AS (
    SELECT DISTINCT s.source,s.pos,json_extract(s.payload,'$.k') AS signal
    FROM members m
    CROSS JOIN warehouse_signal s ON s.source=m.source AND s.pos<m.end
      AND s.revision<=m.revision AND (s.retired_revision IS NULL OR s.retired_revision>m.revision)
    CROSS JOIN visible v ON v.source=s.source AND v.pos=s.pos
)
SELECT 'turns',NULL,count(*),0 FROM turns
UNION ALL
SELECT 'call',tool,count(*),sum(failed) FROM visible_calls GROUP BY tool
UNION ALL
SELECT 'signal',signal,count(*),0 FROM signal_marks GROUP BY signal;
