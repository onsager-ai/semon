compaction_windows AS (
    SELECT *, COALESCE(
        lead(ts_ms, 2) OVER chronological - ts_ms <= 900000
        OR lead(ts_ms) OVER chronological - lag(ts_ms) OVER chronological <= 900000
        OR ts_ms - lag(ts_ms, 2) OVER chronological <= 900000,
        false
    ) AS storm
    FROM compactions
    WINDOW chronological AS (PARTITION BY session_uid ORDER BY ts_ms NULLS LAST, compaction_uid)
), timeline AS (
    SELECT session_uid, step_uid AS uid, epoch_ms(ts) AS ts_ms, 'step' AS kind, context_tokens,
        NULL AS trigger, NULL AS pre_tokens, NULL AS storm
    FROM steps
    UNION ALL
    SELECT session_uid, compaction_uid, ts_ms, 'compaction', NULL, trigger, pre_tokens, storm
    FROM compaction_windows
), measured AS (
    SELECT *,
        max(context_tokens) OVER (PARTITION BY session_uid) AS peak_context_tokens,
        first_value(CASE WHEN ts_ms IS NOT NULL THEN context_tokens END IGNORE NULLS) OVER (
            PARTITION BY session_uid ORDER BY ts_ms NULLS LAST, kind DESC, uid
            ROWS BETWEEN 1 FOLLOWING AND UNBOUNDED FOLLOWING
        ) AS next_context_tokens
    FROM timeline
), context_points AS (
    SELECT session_uid, uid, ts_ms, kind, context_tokens, trigger, pre_tokens,
        CASE WHEN kind = 'compaction' AND ts_ms IS NOT NULL THEN next_context_tokens END AS post_refill_tokens,
        CASE WHEN kind = 'compaction' THEN
            COALESCE(trigger = 'auto' AND CAST(pre_tokens AS HUGEINT) * 2 < peak_context_tokens, false)
        END AS early,
        storm
    FROM measured
),
