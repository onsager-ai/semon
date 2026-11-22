), filtered AS MATERIALIZED (
  SELECT * FROM rows WHERE (?5 IS NULL OR (?5='' AND repo IS NULL) OR repo=?5)
    AND (?6 IS NULL OR model=?6)
), page AS MATERIALIZED (
  SELECT * FROM filtered
  WHERE ?7 IS NULL OR last<?7 OR (last=?7 AND (id>?8 OR (?9 AND id=?8)))
  ORDER BY last DESC,id LIMIT ?10
)
SELECT id,owner,harness,last,title,model,repo,branch,NULL AS total,
       prompt_pos,owner_path,owner_ledger,native_id,agent_path,is_child,start,lineage,guardian,effort FROM page
UNION ALL
SELECT NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,count(*),NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL FROM filtered
ORDER BY last DESC,id
