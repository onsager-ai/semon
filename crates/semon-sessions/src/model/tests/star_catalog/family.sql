-- Inclusive totals over current selected ownership/parenthood. UNION is a
-- visited set: an owner contributes once even when parent assertions cycle.
CREATE VIEW family_usage AS
WITH RECURSIVE edges AS MATERIALIZED (
  SELECT DISTINCT child.owner AS child,parent.owner AS parent
  FROM parent_selection p JOIN bridge_session_owner child ON child.source=p.child
  JOIN bridge_session_owner parent ON parent.source=p.parent
  WHERE child.owner<>parent.owner
), family(root,member) AS (
  SELECT owner,owner FROM bridge_session_owner GROUP BY owner
  UNION
  SELECT f.root,e.child FROM family f JOIN edges e ON e.parent=f.member
)
SELECT f.root,sum(coalesce(u.input,0)) AS input,
  sum(coalesce(u.cached,0)) AS cached,sum(coalesce(u.output,0)) AS output
FROM family f LEFT JOIN usage_totals u ON u.owner=f.member GROUP BY f.root;
