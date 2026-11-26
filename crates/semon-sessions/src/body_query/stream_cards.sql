SELECT k.owner,k.id,k.harness,k.native_id,b.ts,b.text
FROM body_fts JOIN body b ON b.rowid=body_fts.rowid
JOIN body_source bs ON bs.body_uid=b.body_uid
JOIN warehouse_current c ON c.path=bs.source_path
JOIN warehouse_source s ON s.source=c.source AND s.origin='local' AND s.carrier=c.path
JOIN files f ON f.path=c.path AND f.resume_at=json_extract(s.ledger,'$.offset')
JOIN bridge_session_owner ownership ON ownership.source=c.source
JOIN catalog_keys k ON k.owner=ownership.owner
JOIN warehouse_current owner_current ON owner_current.source=k.owner
JOIN dim_session d ON d.source=k.owner
LEFT JOIN fact_session_observation header ON header.source=d.source
    AND header.pos=-1 AND header.block=0 AND header.kind='header' AND header.occurrence=0
WHERE body_fts MATCH 'stream AND lane AND goal'
    AND b.part='assistant' AND substr(b.text,1,7)='Stream:'
    AND length(CAST(b.text AS BLOB))<=8192
    AND ((k.harness='claude' AND d.lineage) OR
         (k.harness='codex' AND d.parent_id IS NULL
          AND NOT coalesce(json_extract(header.metadata,'$.subagent'),0)
          AND NOT coalesce(json_extract(header.metadata,'$.guardian_review'),0)))
ORDER BY b.ts DESC,b.ord DESC,b.body_uid DESC;
