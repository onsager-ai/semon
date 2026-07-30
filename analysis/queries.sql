-- Analysis v1 for the normalized devlog.events table.

-- Cost and tokens by repository and harness over the last seven days.
-- Both harnesses use kind=api_request, so this query has no harness branch.
SELECT
    repo,
    harness,
    round(sum(cost_usd), 4) AS cost_usd,
    sum(tokens_in) AS tokens_in,
    sum(tokens_out) AS tokens_out
FROM devlog.events
WHERE ts >= now() - INTERVAL 7 DAY
  AND kind = 'api_request'
GROUP BY repo, harness
ORDER BY cost_usd DESC, tokens_in + tokens_out DESC;

-- Tool failure rates. The normalizer keeps success in extras because it is
-- harness-specific. Missing success values are excluded from the denominator.
WITH lower(toString(extras.success)) AS success
SELECT
    repo,
    harness,
    tool,
    countIf(success IN ('true', 'false')) AS completed,
    countIf(success = 'false') AS failed,
    round(100 * failed / nullIf(completed, 0), 2) AS failure_pct
FROM devlog.events
WHERE ts >= now() - INTERVAL 7 DAY
  AND kind = 'tool_result'
GROUP BY repo, harness, tool
HAVING completed > 0
ORDER BY failure_pct DESC, completed DESC;

-- Permission-denial clusters by one-hour work window and decision source.
SELECT
    toStartOfInterval(ts, INTERVAL 1 HOUR) AS window,
    repo,
    tool,
    toString(extras.source) AS source,
    count() AS denials,
    uniqExact(session_id) AS sessions
FROM devlog.events
WHERE ts >= now() - INTERVAL 30 DAY
  AND kind = 'tool_decision'
  AND decision = 'reject'
GROUP BY window, repo, tool, source
ORDER BY denials DESC, window DESC;

-- ostrom#6 signal: approval pass-rate. A healthy approval policy trends toward
-- 100%, meaning the queue has stopped behaving like a manual gate.
SELECT
    repo,
    harness,
    count() AS decisions,
    countIf(decision = 'accept') AS accepted,
    round(100 * accepted / nullIf(decisions, 0), 2) AS approval_pass_pct
FROM devlog.events
WHERE ts >= now() - INTERVAL 30 DAY
  AND kind = 'tool_decision'
GROUP BY repo, harness
ORDER BY approval_pass_pct ASC, decisions DESC;

-- ostrom#6 signal: human interventions clustered by repository/workstream.
-- Claude's user_* decision sources identify a person answering a prompt.
SELECT
    toStartOfInterval(ts, INTERVAL 1 DAY) AS day,
    repo AS workstream,
    countIf(
        toString(extras.source) IN
            ('user_temporary', 'user_permanent', 'user_abort', 'user_reject')
    ) AS interventions,
    uniqExact(session_id) AS sessions,
    round(interventions / nullIf(sessions, 0), 2) AS interventions_per_session
FROM devlog.events
WHERE ts >= now() - INTERVAL 30 DAY
  AND kind = 'tool_decision'
GROUP BY day, workstream
ORDER BY day DESC, interventions DESC;
