WITH filtered AS MATERIALIZED (
  SELECT catalog_key AS key,coalesce(json_extract(metadata,'$.last'),0) AS last,metadata
  FROM warehouse_identity
  WHERE scope='retained_history' AND source_key=?1 AND retained=1
    AND (?2 IS NULL OR harness=?2)
    AND (?3 IS NULL OR (?3='' AND json_extract(metadata,'$.repo') IS NULL)
      OR json_extract(metadata,'$.repo')=?3)
    AND (?4 IS NULL OR json_extract(metadata,'$.model')=?4)
), page AS MATERIALIZED (
  SELECT * FROM filtered
  WHERE ?5 IS NULL OR last<?5 OR (last=?5 AND (key>?6 OR (?7 AND key=?6)))
  ORDER BY last DESC,key LIMIT ?8
)
SELECT key,last,metadata,NULL FROM page
UNION ALL
SELECT NULL,NULL,NULL,count(*) FROM filtered
ORDER BY last DESC,key;
