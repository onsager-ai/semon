-- Match claude_usage::shared: all UUID observations connect owners for a
-- message, transitively. The minimum shared UUID is its evidence identity.
CREATE VIEW shared_components AS
WITH RECURSIVE identities AS MATERIALIZED (
 SELECT DISTINCT m.owner,u.message_id,u.uuid FROM fact_step_observation u
 JOIN bridge_session_owner m USING(source) WHERE u.uuid IS NOT NULL
), edges AS MATERIALIZED (
 SELECT a.message_id,a.owner,b.owner AS other,a.uuid FROM identities a
 JOIN identities b ON b.message_id=a.message_id AND b.uuid=a.uuid AND b.owner<>a.owner
), connected(message_id,owner,member) AS (
 SELECT message_id,owner,other FROM edges UNION
 SELECT c.message_id,c.owner,e.other FROM connected c
 JOIN edges e ON e.message_id=c.message_id AND e.owner=c.member
)
SELECT c.message_id,c.owner,min(e.uuid) AS evidence_uuid FROM connected c
JOIN edges e ON e.message_id=c.message_id AND e.owner=c.member
GROUP BY c.message_id,c.owner;

CREATE VIEW family_shared_usage AS
WITH evidence AS (
 SELECT f.root,s.message_id,s.evidence_uuid,u.source,u.pos,u.input,u.cached,u.output,
 row_number() OVER(PARTITION BY f.root,s.message_id,s.evidence_uuid ORDER BY f.priority) AS n
 FROM family_fold_order f JOIN shared_components s ON s.owner=f.member
 JOIN current_usage u ON u.owner=s.owner AND u.message_id=s.message_id
)
-- Source/position selects the same native pricing input as the winning step;
-- token categories and cost must never be combined from different observations.
SELECT root,message_id,evidence_uuid,source,pos,input,cached,output FROM evidence WHERE n=1;
