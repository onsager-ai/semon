-- Bind a selected family owner and register semon_step_usd with the reader's
-- effective price snapshot. The persistent schema has no pricing dependency.
WITH members AS MATERIALIZED (
 SELECT member FROM family_members WHERE root=?1
), steps AS (
 SELECT u.owner,u.message_id,s.owner AS shared,
   semon_step_usd(u.model,b.billing) AS usd
 FROM members m JOIN current_usage u ON u.owner=m.member
 LEFT JOIN shared_messages s ON s.owner=u.owner AND s.message_id=u.message_id
 LEFT JOIN fact_step_billing b
   ON b.source=u.source AND b.pos=u.pos AND b.block=u.block
   AND b.kind=u.kind AND b.occurrence=u.occurrence
), session_cost AS (
 SELECT k.owner,CASE WHEN k.harness='claude'
   AND count(s.message_id)=count(s.usd) AND count(s.shared)=0
   THEN coalesce(sum(s.usd),0.0) END AS usd
 FROM members m JOIN catalog_keys k ON k.owner=m.member
 LEFT JOIN steps s ON s.owner=k.owner GROUP BY k.owner
), family_cost AS (
 SELECT CASE WHEN EXISTS (SELECT 1 FROM family_fold_order WHERE root=?1)
   AND count(*)=count(c.usd) THEN sum(c.usd) END AS usd
 FROM members m LEFT JOIN session_cost c ON c.owner=m.member
), priced_shared AS (
 SELECT semon_step_usd(u.model,b.billing) AS usd
 FROM family_shared_usage f JOIN fact_step_observation u USING(source,pos,block,kind,occurrence)
 LEFT JOIN fact_step_billing b USING(source,pos,block,kind,occurrence)
 WHERE f.root=?1
), shared_cost AS (
 SELECT coalesce(sum(usd),0.0) AS known,count(*)-count(usd) AS unknown_steps FROM priced_shared
)
SELECT family_cost.usd,shared_cost.known,shared_cost.unknown_steps FROM family_cost,shared_cost;
