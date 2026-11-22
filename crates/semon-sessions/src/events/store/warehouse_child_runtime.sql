-- Only Claude subagents inherit their graph root's process liveness. Marker
-- relationships alone do not turn a separately running session into an agent.
WITH RECURSIVE edges AS MATERIALIZED (SELECT * FROM family_edges),
children AS MATERIALIZED (
    SELECT d.source AS child,d.native_id,min(p.parent) AS parent,min(native.call_id) AS call_id
    FROM json_each(?1) selected
    JOIN dim_session d ON d.source=selected.value AND d.harness='claude' AND NOT d.lineage
    JOIN edges p ON p.child=d.source
    LEFT JOIN parent_selection native ON native.child=d.source
    GROUP BY d.source,d.native_id HAVING count(DISTINCT p.parent)=1
), chain(child,ancestor) AS (
    SELECT child,parent FROM children
    UNION
    SELECT c.child,e.parent FROM chain c JOIN edges e ON e.child=c.ancestor
), roots AS (
    SELECT child,min(ancestor) AS root
    FROM chain c WHERE NOT EXISTS (SELECT 1 FROM edges e WHERE e.child=c.ancestor)
    GROUP BY child HAVING count(*)=1
)
SELECT c.child,c.native_id,c.parent,c.call_id,r.root,
    NOT EXISTS (
        SELECT 1 FROM chain a
        JOIN lineage_member owner ON owner.source=a.ancestor
        JOIN lineage_member member ON member.component=owner.component
        LEFT JOIN warehouse_current current ON current.source=member.source
        LEFT JOIN json_each(?2) authorized ON authorized.value=current.path
        WHERE a.child=c.child AND authorized.value IS NULL
    )
FROM children c JOIN roots r USING(child);
