-- Inclusive totals over current selected ownership/parenthood. UNION is a
-- visited set: an owner contributes once even when parent assertions cycle.
CREATE VIEW family_edges AS
SELECT DISTINCT child.owner AS child,parent.owner AS parent
FROM parent_selection p JOIN bridge_session_owner child ON child.source=p.child
JOIN bridge_session_owner parent ON parent.source=p.parent
WHERE child.owner<>parent.owner;

CREATE VIEW family_members AS
WITH RECURSIVE family(root,member) AS (
  SELECT owner,owner FROM bridge_session_owner GROUP BY owner
  UNION
  SELECT f.root,e.child FROM family f JOIN family_edges e ON e.parent=f.member
)
SELECT * FROM family;

CREATE VIEW family_usage AS
SELECT f.root,sum(coalesce(u.input,0)) AS input,
  sum(coalesce(u.cached,0)) AS cached,sum(coalesce(u.output,0)) AS output
FROM family_members f LEFT JOIN usage_totals u ON u.owner=f.member GROUP BY f.root;

-- analytics::families starts with catalog-key-ordered leaves, then enqueues a
-- parent after its last child. Thus queue order is subtree height followed by
-- the greatest key among deepest leaves. A parent retains its own observation;
-- otherwise its first child fold containing the evidence wins. Encode that
-- recursive precedence as a sibling-rank path (the empty path wins at root).
-- Cyclic families have no builder fold, so do not manufacture a shared winner.
CREATE VIEW family_fold_order AS
WITH RECURSIVE descendants(root,member,depth,path,cycle) AS (
 SELECT owner,owner,0,printf('/%d/',owner),0 FROM bridge_session_owner GROUP BY owner
 UNION ALL
 SELECT d.root,e.child,d.depth+1,d.path||e.child||'/',
   instr(d.path,printf('/%d/',e.child))>0
 FROM descendants d JOIN family_edges e ON e.parent=d.member WHERE NOT d.cycle
), heights AS (
 SELECT root,max(depth) AS height FROM descendants GROUP BY root
), priorities AS (
 SELECT d.root,h.height,max(k.id) AS last_leaf FROM descendants d
 JOIN heights h ON h.root=d.root AND h.height=d.depth
 JOIN catalog_keys k ON k.owner=d.member GROUP BY d.root
), siblings AS (
 SELECT e.parent,e.child,row_number() OVER (
   PARTITION BY e.parent ORDER BY p.height,p.last_leaf) AS n
 FROM family_edges e JOIN priorities p ON p.root=e.child
), precedence(root,member,priority) AS (
 SELECT owner,owner,'' FROM bridge_session_owner
 WHERE owner NOT IN (SELECT root FROM descendants WHERE cycle) GROUP BY owner
 UNION ALL
 SELECT p.root,s.child,p.priority||printf('%020d/',s.n)
 FROM precedence p JOIN siblings s ON s.parent=p.member
)
SELECT * FROM precedence;
