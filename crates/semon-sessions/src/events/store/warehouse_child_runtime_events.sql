WITH parent_member AS MATERIALIZED (
    SELECT m.source FROM lineage_member p
    JOIN lineage_member m ON m.component=p.component WHERE p.source=?2
), first_prompt AS (
    SELECT min(pos) AS pos FROM fact_prompt WHERE source=?1 AND kind='user'
), evidence AS (
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,0 AS kind,0 AS failed
    FROM parent_member m JOIN fact_subagent_event e USING(source)
    WHERE e.kind='agm' AND e.flags & ?5<>0 AND (e.id IN (?3,?4) OR e.name IN (?3,?4))
    UNION ALL
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,1,0
    FROM fact_subagent_event e WHERE e.source=?1 AND e.kind='tool' AND e.name='SubagentHandback'
    UNION ALL
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,2,coalesce(e.name NOT IN ('completed','success','done'),1)
    FROM parent_member m JOIN fact_subagent_event e USING(source)
    WHERE e.kind='tn' AND e.id=?4
    UNION ALL
    SELECT e.ts,e.source,e.pos,e.block,e.occurrence,3,0
    FROM fact_subagent_event e WHERE e.source=?1 AND e.kind='agm' AND e.flags & ?6<>0
    UNION ALL
    SELECT p.ts,p.source,p.pos,p.block,p.occurrence,3,0
    FROM fact_prompt p,first_prompt first WHERE p.source=?1 AND p.kind='user' AND p.pos>first.pos
), sync_result AS (
    SELECT r.failed FROM parent_member m
    JOIN fact_claude_call c ON c.source=m.source AND c.call_id=?4
    JOIN fact_tool_result r ON r.source=c.source AND r.call_id=c.call_id AND r.pos>=c.pos
      AND (r.call_pos IS NULL OR (r.call_pos=c.pos AND r.call_block=c.block))
    WHERE r.flags & ?7=0 ORDER BY r.ts DESC,r.source,r.pos DESC LIMIT 1
)
SELECT kind,failed FROM (SELECT kind,failed,0 AS fallback,ts,source,pos,block,occurrence FROM evidence
    UNION ALL SELECT 4,failed,1,NULL,0,0,0,0 FROM sync_result)
ORDER BY fallback,ts IS NULL,ts,kind,source,pos,block,occurrence;
