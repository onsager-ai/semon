WITH admitted AS MATERIALIZED (
  SELECT c.source,c.path,s.ledger,f.title,f.agent_name,f.cwd,f.branch
  FROM json_each(?1) paths
  JOIN warehouse_current c ON c.path=paths.value
  JOIN warehouse_source s ON s.source=c.source AND s.origin=?2 AND s.carrier=c.path
  JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
), components AS (
  SELECT m.component,max(d.last_ts) AS last,min(d.first_ts) AS start,count(a.source) AS qualified,count(*) AS total
  FROM lineage_member m JOIN lineage_group g ON g.id=m.component
  JOIN dim_session d USING(source) LEFT JOIN admitted a USING(source)
  GROUP BY m.component
), members AS (
  SELECT m.component,d.source,d.harness,d.native_id,d.parent_id,d.nickname,d.agent_path,
    coalesce(a.title,a.agent_name) AS title,d.model,a.cwd,a.branch,a.path,
    row_number() OVER(PARTITION BY m.component ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,d.source) AS rank,
    row_number() OVER(PARTITION BY m.component ORDER BY a.title IS NULL AND a.agent_name IS NULL,
      d.first_ts DESC,d.continued,d.native_id DESC,d.source) AS title_rank,
    row_number() OVER(PARTITION BY m.component ORDER BY d.model IS NULL,
      d.first_ts DESC,d.continued,d.native_id DESC,d.source) AS model_rank
  FROM admitted a JOIN lineage_member m USING(source) JOIN dim_session d USING(source)
), rows AS MATERIALIZED (
  SELECT k.id,k.owner,k.harness,coalesce(c.last,0) AS last,
    coalesce(t.title,CASE k.harness WHEN 'codex' THEN
      CASE WHEN owner.parent_id IS NULL THEN coalesce(nullif(owner.nickname,''),'Codex run')
        ELSE coalesce(nullif(owner.agent_path,''),'Codex subagent') END
      WHEN 'copilot' THEN 'Copilot CLI' WHEN 'claude' THEN semon_claude_project(latest.path) END) AS title,
    coalesce(model.model,CASE k.harness WHEN 'codex' THEN 'codex' ELSE '—' END) AS model,
    repo.value AS repo,
    latest.branch,
    CASE WHEN k.harness='codex' AND (owner.parent_id IS NOT NULL OR coalesce(json_extract(source_meta.metadata,'$.subagent'),0))
      AND NOT coalesce(json_extract(source_meta.metadata,'$.guardian_review'),0)
      AND nullif(owner.agent_path,'') IS NULL AND t.title IS NULL THEN owner.prompt_pos END AS prompt_pos,
    owner_file.path AS owner_path,owner_file.ledger AS owner_ledger,owner.native_id,owner.agent_path,
    ((owner.parent_id IS NOT NULL OR coalesce(json_extract(source_meta.metadata,'$.subagent'),0))
      AND NOT coalesce(json_extract(source_meta.metadata,'$.guardian_review'),0)) AS is_child,
    c.start,owner.lineage,coalesce(json_extract(source_meta.metadata,'$.guardian_review'),0) AS guardian
  FROM catalog_keys k JOIN lineage_member m ON m.source=k.owner
  JOIN components c ON c.component=m.component AND c.qualified=c.total
  JOIN members latest ON latest.component=c.component AND latest.rank=1
  JOIN members t ON t.component=c.component AND t.title_rank=1
  JOIN members model ON model.component=c.component AND model.model_rank=1
  JOIN dim_session owner ON owner.source=k.owner
  JOIN admitted owner_file ON owner_file.source=owner.source
  LEFT JOIN fact_session_observation source_meta ON source_meta.source=owner.source
    AND source_meta.pos=-1 AND source_meta.block=0 AND source_meta.kind='header' AND source_meta.occurrence=0
  LEFT JOIN json_each(?3) repo ON repo.key=latest.cwd
  WHERE (?4 IS NULL OR k.harness=?4)
), filtered AS MATERIALIZED (
  SELECT * FROM rows WHERE (?5 IS NULL OR (?5='' AND repo IS NULL) OR repo=?5)
    AND (?6 IS NULL OR model=?6)
), page AS MATERIALIZED (
  SELECT * FROM filtered
  WHERE ?7 IS NULL OR last<?7 OR (last=?7 AND (id>?8 OR (?9 AND id=?8)))
  ORDER BY last DESC,id LIMIT ?10
)
SELECT id,owner,harness,last,title,model,repo,branch,NULL AS total,
       prompt_pos,owner_path,owner_ledger,native_id,agent_path,is_child,start,lineage,guardian FROM page
UNION ALL
SELECT NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,count(*),NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL FROM filtered
ORDER BY last DESC,id
