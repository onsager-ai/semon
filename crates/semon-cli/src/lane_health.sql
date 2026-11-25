compactions_by_session AS (
    SELECT session_uid, count(*) AS compaction_count,
        list({
            'uid': uid, 'timestamp': ts_ms, 'trigger': trigger, 'pre_tokens': pre_tokens,
            'post_refill_tokens': post_refill_tokens, 'early': early,
            'storm': storm
        } ORDER BY ts_ms NULLS LAST, uid) AS compactions
    FROM context_points WHERE kind = 'compaction' GROUP BY session_uid
), members AS (
    SELECT d.session_uid, d.parent_session_uid, d.role,
        COALESCE(c.compaction_count, 0) AS compaction_count, COALESCE(c.compactions, []) AS compactions
    FROM family f JOIN dim_session d USING (session_uid)
    LEFT JOIN compactions_by_session c USING (session_uid)
), observed_children AS (
    SELECT m.session_uid FROM members m JOIN root_session r ON m.parent_session_uid = r.session_uid
    WHERE m.session_uid <> r.session_uid
), tool_mix AS (
    SELECT t.tool_key, t.command_tag, count(*) AS calls
    FROM fact_tool_call t JOIN root_session r USING (session_uid)
    GROUP BY t.tool_key, t.command_tag
), owners AS (
    SELECT 'exclusive' AS usage_owner UNION ALL SELECT 'copied'
), usage_totals AS (
    SELECT o.usage_owner, count(s.step_uid) AS steps,
        count(*) FILTER (WHERE price_status = 'priced') AS priced_steps,
        count(*) FILTER (WHERE price_status = 'unpriced') AS unpriced_steps,
        sum(usd_est) FILTER (WHERE price_status = 'priced') AS priced_usd_est,
        sum(input_tokens) AS input_tokens, sum(output_tokens) AS output_tokens,
        sum(cache_read_tokens) AS cache_read_tokens,
        sum(cache_write_5m_tokens) AS cache_write_5m_tokens,
        sum(cache_write_1h_tokens) AS cache_write_1h_tokens,
        count(*) FILTER (WHERE split_unknown) AS unknown_cache_split_steps,
        sum(reasoning_tokens) AS known_reasoning_tokens,
        count(reasoning_tokens) AS steps_with_reasoning
    FROM owners o LEFT JOIN steps s USING (usage_owner) GROUP BY o.usage_owner
)
SELECT r.session_uid AS root_session_uid,
    (SELECT list({
        'session_uid': session_uid, 'parent_session_uid': parent_session_uid, 'role': role,
        'compaction_count': compaction_count, 'compactions': compactions
    } ORDER BY session_uid) FROM members) AS members,
    (SELECT COALESCE(list(session_uid ORDER BY session_uid), []) FROM observed_children) AS observed_children,
    (SELECT COALESCE(list({'tool_key': tool_key, 'command_tag': command_tag, 'calls': calls}
        ORDER BY calls DESC, tool_key, command_tag NULLS LAST), []) FROM tool_mix) AS manager_tool_mix,
    (SELECT list({
        'usage_owner': usage_owner, 'steps': steps, 'priced_steps': priced_steps,
        'unpriced_steps': unpriced_steps, 'priced_usd_est': priced_usd_est,
        'input_tokens': input_tokens, 'output_tokens': output_tokens, 'cache_read_tokens': cache_read_tokens,
        'cache_write_5m_tokens': cache_write_5m_tokens, 'cache_write_1h_tokens': cache_write_1h_tokens,
        'unknown_cache_split_steps': unknown_cache_split_steps, 'known_reasoning_tokens': known_reasoning_tokens,
        'steps_with_reasoning': steps_with_reasoning
    } ORDER BY usage_owner) FROM usage_totals) AS usage
FROM root_session r
