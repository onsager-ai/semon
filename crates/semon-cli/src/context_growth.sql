bounds AS (
    SELECT CAST(? AS BIGINT) AS since_ms, CAST(? AS BIGINT) AS until_ms
)
SELECT ts_ms AS "timestamp", kind, uid, context_tokens, trigger, pre_tokens, post_refill_tokens,
    early, storm
FROM context_points CROSS JOIN bounds
WHERE (since_ms IS NULL OR ts_ms >= since_ms) AND (until_ms IS NULL OR ts_ms <= until_ms)
ORDER BY ts_ms NULLS LAST, kind DESC, uid
