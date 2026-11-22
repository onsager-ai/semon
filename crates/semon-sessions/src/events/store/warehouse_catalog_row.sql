), members AS (
  SELECT m.component,d.source,d.harness,d.native_id,d.parent_id,d.nickname,d.agent_path,
    coalesce(a.title,a.agent_name) AS title,coalesce(d.model,agent.model) AS model,d.effort,a.cwd,a.branch,a.path,
    row_number() OVER(PARTITION BY m.component ORDER BY d.first_ts DESC,d.continued,d.native_id DESC,d.source) AS rank,
    row_number() OVER(PARTITION BY m.component ORDER BY a.title IS NULL AND a.agent_name IS NULL,
      d.first_ts DESC,d.continued,d.native_id DESC,d.source) AS title_rank,
    row_number() OVER(PARTITION BY m.component ORDER BY coalesce(d.model,agent.model) IS NULL,
      d.first_ts DESC,d.continued,d.native_id DESC,d.source) AS model_rank,
    row_number() OVER(PARTITION BY m.component ORDER BY d.effort IS NULL,
      d.first_ts DESC,d.continued,d.native_id DESC,d.source) AS effort_rank
  FROM admitted a JOIN lineage_member m USING(source) JOIN dim_session d USING(source)
  LEFT JOIN maintenance_cursor current ON d.harness='claude' AND NOT d.lineage AND current.source=d.source
  LEFT JOIN fact_agent_observation agent ON agent.source=d.source AND agent.revision=current.sidecar_revision AND agent.present
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
    c.start,owner.lineage,coalesce(json_extract(source_meta.metadata,'$.guardian_review'),0) AS guardian,effort.effort,
    (t.title IS NOT NULL OR (k.harness='codex' AND (
      nullif(owner.nickname,'') IS NOT NULL OR nullif(owner.agent_path,'') IS NOT NULL
    ))) AS native_title
  FROM eligible_keys k CROSS JOIN lineage_member m ON m.source=k.owner
  JOIN components c ON c.component=m.component AND c.qualified=c.total
  -- Bind the component before looking up each rank. Otherwise SQLite can
  -- multiply rank-one rows from unrelated components before filtering them.
  CROSS JOIN members latest ON latest.component=c.component AND latest.rank=1
  CROSS JOIN members t ON t.component=c.component AND t.title_rank=1
  CROSS JOIN members model ON model.component=c.component AND model.model_rank=1
  CROSS JOIN members effort ON effort.component=c.component AND effort.effort_rank=1
  JOIN dim_session owner ON owner.source=k.owner
  JOIN admitted owner_file ON owner_file.source=owner.source
  LEFT JOIN fact_session_observation source_meta ON source_meta.source=owner.source
    AND source_meta.pos=-1 AND source_meta.block=0 AND source_meta.kind='header' AND source_meta.occurrence=0
  LEFT JOIN json_each(?3) repo ON repo.key=latest.cwd
