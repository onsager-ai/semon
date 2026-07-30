CREATE DATABASE IF NOT EXISTS devlog;

CREATE TABLE IF NOT EXISTS devlog.events
(
    harness LowCardinality(String),
    session_id String,
    repo LowCardinality(String),
    ts DateTime64(9, 'UTC'),
    kind LowCardinality(String),
    tool LowCardinality(String),
    decision LowCardinality(String),
    tokens_in UInt64,
    tokens_out UInt64,
    cost_usd Float64,
    duration_ms UInt64,
    extras JSON,
    prompt String CODEC(ZSTD(3)),
    response String CODEC(ZSTD(3)),
    tool_input String CODEC(ZSTD(3)),
    tool_output String CODEC(ZSTD(3))
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (repo, harness, toDate(ts), ts, session_id, kind);

CREATE MATERIALIZED VIEW IF NOT EXISTS devlog.events_from_otel_logs
TO devlog.events
AS
WITH
    if(
        EventName != '',
        EventName,
        if(
            LogAttributes['event.name'] != '',
            LogAttributes['event.name'],
            Body
        )
    ) AS raw_kind,
    replaceRegexpOne(raw_kind, '^claude_code\\.', '') AS normalized_kind,
    if(
        LogAttributes['devlog.extras'] != '',
        LogAttributes['devlog.extras'],
        toJSONString(LogAttributes)
    ) AS normalized_extras
SELECT
    if(
        LogAttributes['devlog.harness'] != '',
        LogAttributes['devlog.harness'],
        if(startsWith(raw_kind, 'claude_code.'), 'claude_code', 'unknown')
    ) AS harness,
    coalesce(
        nullIf(LogAttributes['session.id'], ''),
        nullIf(ResourceAttributes['session.id'], ''),
        nullIf(LogAttributes['devlog.session_id'], ''),
        ''
    ) AS session_id,
    coalesce(
        nullIf(LogAttributes['devlog.repo'], ''),
        nullIf(ResourceAttributes['devlog.repo'], ''),
        ''
    ) AS repo,
    Timestamp AS ts,
    normalized_kind AS kind,
    coalesce(
        nullIf(LogAttributes['devlog.tool'], ''),
        nullIf(LogAttributes['tool_name'], ''),
        ''
    ) AS tool,
    coalesce(
        nullIf(LogAttributes['devlog.decision'], ''),
        nullIf(LogAttributes['decision'], ''),
        nullIf(LogAttributes['decision_type'], ''),
        ''
    ) AS decision,
    toUInt64OrZero(coalesce(
        nullIf(LogAttributes['devlog.tokens_in'], ''),
        nullIf(LogAttributes['input_tokens'], ''),
        '0'
    )) AS tokens_in,
    toUInt64OrZero(coalesce(
        nullIf(LogAttributes['devlog.tokens_out'], ''),
        nullIf(LogAttributes['output_tokens'], ''),
        '0'
    )) AS tokens_out,
    toFloat64OrZero(coalesce(
        nullIf(LogAttributes['devlog.cost_usd'], ''),
        nullIf(LogAttributes['cost_usd'], ''),
        '0'
    )) AS cost_usd,
    toUInt64OrZero(coalesce(
        nullIf(LogAttributes['devlog.duration_ms'], ''),
        nullIf(LogAttributes['duration_ms'], ''),
        '0'
    )) AS duration_ms,
    normalized_extras AS extras,
    if(
        normalized_kind = 'user_prompt',
        coalesce(
            nullIf(LogAttributes['devlog.prompt'], ''),
            nullIf(LogAttributes['prompt'], ''),
            Body
        ),
        ''
    ) AS prompt,
    if(
        normalized_kind = 'assistant_response',
        coalesce(
            nullIf(LogAttributes['devlog.response'], ''),
            nullIf(LogAttributes['response'], ''),
            Body
        ),
        ''
    ) AS response,
    coalesce(
        nullIf(LogAttributes['devlog.tool_input'], ''),
        nullIf(LogAttributes['tool_input'], ''),
        ''
    ) AS tool_input,
    coalesce(
        nullIf(LogAttributes['devlog.tool_output'], ''),
        ''
    ) AS tool_output
FROM otel.otel_logs;
