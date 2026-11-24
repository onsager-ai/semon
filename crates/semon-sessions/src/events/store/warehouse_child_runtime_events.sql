WITH parent_member AS MATERIALIZED (
    SELECT m.source FROM lineage_member p
    JOIN lineage_member m ON m.component=p.component WHERE p.source=?2
), invocations AS MATERIALIZED (
    SELECT c.source,c.pos,c.block,c.call_id,t.ts,r.pos AS result_pos,
      CASE WHEN c.tool='SendMessage' THEN row_number() OVER (ORDER BY c.source,c.pos,c.block) ELSE 0 END AS run
    FROM parent_member m JOIN fact_claude_call c USING(source)
    JOIN fact_tool_call t ON t.source=c.source AND t.pos=c.pos AND t.block=c.block
      AND t.occurrence=c.occurrence
    LEFT JOIN fact_tool_result r ON r.source=c.source AND r.call_id=c.call_id
      AND r.call_pos=c.pos AND r.call_block=c.block AND r.flags & ?8<>0 AND r.target=?3
    WHERE (c.tool IN ('Agent','Task') AND c.call_id=?4)
      OR (c.tool='SendMessage' AND r.pos IS NOT NULL)
), unique_invocations AS MATERIALIZED (
    SELECT * FROM invocations i WHERE 1=(SELECT count(*) FROM parent_member m
      JOIN fact_claude_call c USING(source) WHERE c.call_id=i.call_id)
), first_prompt AS (
    SELECT min(pos) AS pos FROM fact_prompt WHERE source=?1 AND kind='user'
), evidence AS (
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,0 AS kind,0 AS failed,0 AS run
    FROM parent_member m JOIN fact_subagent_event e USING(source)
    WHERE e.kind='agm' AND e.flags & ?5<>0 AND (e.id IN (?3,?4) OR e.name IN (?3,?4))
    UNION ALL
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,1,0,0
    FROM fact_subagent_event e WHERE e.source=?1 AND e.kind='tool' AND e.name='SubagentHandback'
    UNION ALL
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,2,e.name NOT IN ('completed','success','done'),i.run
    FROM parent_member m JOIN fact_subagent_event e USING(source)
    JOIN unique_invocations i ON i.call_id=e.id
    WHERE e.kind='tn' AND e.target=?3
      AND ((e.source=i.source AND e.pos>coalesce(i.result_pos,i.pos))
        OR (e.source<>i.source AND i.run=0 AND e.ts>i.ts))
    UNION ALL
    SELECT i.ts,i.source,i.pos,i.block,0,5,0,i.run FROM unique_invocations i WHERE i.run<>0
    UNION ALL
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,3,0,0
    FROM fact_subagent_event e WHERE e.source=?1 AND e.kind='agm' AND e.flags & ?6<>0
    UNION ALL
    SELECT p.ts,p.source,p.pos,p.block,p.occurrence,3,0,0
    FROM fact_prompt p,first_prompt first WHERE p.source=?1 AND p.kind='user' AND p.pos>first.pos
), sync_result AS (
    SELECT r.failed FROM parent_member m
    JOIN fact_claude_call c ON c.source=m.source AND c.call_id=?4
    JOIN fact_tool_result r ON r.source=c.source AND r.call_id=c.call_id AND r.pos>=c.pos
      AND (r.call_pos IS NULL OR (r.call_pos=c.pos AND r.call_block=c.block))
    WHERE c.tool IN ('Agent','Task') AND r.flags & ?7=0 ORDER BY r.ts DESC,r.source,r.pos DESC LIMIT 1
)
SELECT kind,failed,run,ts,source,pos,block,occurrence FROM evidence
UNION ALL SELECT 4,failed,0,NULL,0,0,0,0 FROM sync_result;
