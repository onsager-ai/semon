)
SELECT id,owner,harness,last,title,model,repo,branch,NULL AS total,
       prompt_pos,owner_path,owner_ledger,native_id,agent_path,is_child,start,lineage,guardian,effort
FROM rows
