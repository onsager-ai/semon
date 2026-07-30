#!/bin/sh
set -eu

client() {
  clickhouse-client \
    --host "${CLICKHOUSE_HOST}" \
    --user "${CLICKHOUSE_USER}" \
    --password "${CLICKHOUSE_PASSWORD}" \
    "$@"
}

until client --query \
  "EXISTS TABLE otel.otel_logs" 2>/dev/null | grep -q '^1$'
do
  sleep 1
done

client --multiquery < /schema/schema.sql
