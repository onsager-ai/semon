# Owner management dashboard

Chief PM approved the v1 dashboard on 2026-11-24. The receiver hosts a
read-only `/manage` page in the existing Viewer. The Chief PM publishes a full
snapshot of decisions, budgets, lanes and hosts. Semon reads stalled sessions
from its live Catalog through `stalls`.

## Snapshot ingest

`POST /v1/manage/snapshot` accepts a UTF-8 `application/json` object with version
`1`. The Chief PM sends a complete replacement for the current view each time.
An empty array explicitly reports that a section has no entries.

### JSON Schema

This is JSON Schema Draft 2020-12. The receiver also enforces the validation rules
below, including timestamp and URL checks.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["version", "generated_at", "chief_pm_checked_at", "decisions_waiting", "decisions_today", "budgets", "lanes", "hosts"],
  "properties": {
    "version": { "type": "integer", "const": 1 },
    "generated_at": { "$ref": "#/$defs/time" },
    "chief_pm_checked_at": { "$ref": "#/$defs/time" },
    "decisions_waiting": { "type": "array", "items": { "$ref": "#/$defs/waiting" } },
    "decisions_today": { "type": "array", "items": { "$ref": "#/$defs/decision" } },
    "budgets": { "type": "array", "items": { "$ref": "#/$defs/budget" } },
    "lanes": { "type": "array", "items": { "$ref": "#/$defs/lane" } },
    "hosts": { "type": "array", "items": { "$ref": "#/$defs/host" } }
  },
  "$defs": {
    "text": { "type": "string", "minLength": 1 },
    "time": { "type": "string", "format": "date-time", "pattern": "Z$" },
    "money_or_credits": { "type": "number", "minimum": 0 },
    "waiting": {
      "type": "object", "additionalProperties": false,
      "required": ["id", "lane", "question", "recommendation", "waiting_since"],
      "properties": {
        "id": { "$ref": "#/$defs/text" },
        "lane": { "$ref": "#/$defs/text" },
        "question": { "$ref": "#/$defs/text" },
        "recommendation": { "$ref": "#/$defs/text" },
        "waiting_since": { "$ref": "#/$defs/time" }
      }
    },
    "decision": {
      "type": "object", "additionalProperties": false,
      "required": ["id", "lane", "summary", "decided_at", "link"],
      "properties": {
        "id": { "$ref": "#/$defs/text" },
        "lane": { "$ref": "#/$defs/text" },
        "summary": { "$ref": "#/$defs/text" },
        "decided_at": { "$ref": "#/$defs/time" },
        "link": { "type": "string", "format": "uri", "pattern": "^https://" }
      }
    },
    "budget": {
      "type": "object", "additionalProperties": false,
      "required": ["id", "label", "spent", "cap", "unit", "reset_at", "run_out_at"],
      "properties": {
        "id": { "$ref": "#/$defs/text" },
        "label": { "$ref": "#/$defs/text" },
        "spent": { "$ref": "#/$defs/money_or_credits" },
        "cap": { "$ref": "#/$defs/money_or_credits" },
        "unit": { "enum": ["usd", "kiro-credits"] },
        "reset_at": { "anyOf": [{ "$ref": "#/$defs/time" }, { "type": "null" }] },
        "run_out_at": { "anyOf": [{ "$ref": "#/$defs/time" }, { "type": "null" }] },
        "breakdown": { "type": "array", "items": { "$ref": "#/$defs/budget_detail" } }
      }
    },
    "budget_detail": {
      "type": "object", "additionalProperties": false,
      "required": ["spent"],
      "anyOf": [{ "required": ["lane"] }, { "required": ["agent"] }],
      "properties": {
        "lane": { "$ref": "#/$defs/text" },
        "agent": { "$ref": "#/$defs/text" },
        "model": { "$ref": "#/$defs/text" },
        "spent": { "$ref": "#/$defs/money_or_credits" }
      }
    },
    "activity": { "enum": ["running", "idle", "finished", "unknown"] },
    "lane": {
      "type": "object", "additionalProperties": false,
      "required": ["status", "alerts", "health", "activity", "last_update_at", "last_seen_at", "agents"],
      "properties": {
        "status": { "$ref": "#/$defs/lane_status" },
        "alerts": { "type": "array", "items": { "$ref": "#/$defs/text" } },
        "health": { "enum": ["green", "amber", "red"] },
        "activity": { "$ref": "#/$defs/activity" },
        "last_update_at": { "$ref": "#/$defs/time" },
        "last_seen_at": { "$ref": "#/$defs/time" },
        "agents": { "type": "array", "items": { "$ref": "#/$defs/agent" } }
      }
    },
    "agent": {
      "type": "object", "additionalProperties": false,
      "required": ["id", "activity", "last_seen_at"],
      "properties": {
        "id": { "$ref": "#/$defs/text" },
        "activity": { "$ref": "#/$defs/activity" },
        "last_seen_at": { "anyOf": [{ "$ref": "#/$defs/time" }, { "type": "null" }] }
      }
    },
    "lane_status": {
      "type": "object", "additionalProperties": false,
      "required": ["Lane", "State", "Goal", "Next", "Blockers", "Decisions", "Spend", "Links", "State-of-work"],
      "properties": {
        "Lane": { "$ref": "#/$defs/text" },
        "State": { "enum": ["active", "parked", "done"] },
        "Goal": { "$ref": "#/$defs/text" },
        "Next": { "type": "string", "pattern": "^.+ · due [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}Z$" },
        "Blockers": { "$ref": "#/$defs/text" },
        "Decisions": { "$ref": "#/$defs/text" },
        "Spend": { "$ref": "#/$defs/text" },
        "Links": { "$ref": "#/$defs/text" },
        "State-of-work": { "$ref": "#/$defs/text" }
      }
    },
    "host": {
      "type": "object", "additionalProperties": false,
      "required": ["host", "temperature_c", "gate", "observed_at"],
      "properties": {
        "host": { "$ref": "#/$defs/text" },
        "temperature_c": { "type": "number" },
        "gate": { "enum": ["ok", "gated"] },
        "observed_at": { "$ref": "#/$defs/time" }
      }
    }
  }
}
```

A small example with synthetic values follows.

```json
{
  "version": 1,
  "generated_at": "2026-11-24T14:00:00Z",
  "chief_pm_checked_at": "2026-11-24T13:55:00Z",
  "decisions_waiting": [
    { "id": "eval-D1", "lane": "Eval · Fixtures", "question": "Extend the fixture set?", "recommendation": "Finish the current set first.", "waiting_since": "2026-11-24T12:00:00Z" }
  ],
  "decisions_today": [
    { "id": "eval-D2", "lane": "Eval · Fixtures", "summary": "Deferred the optional fixture batch.", "decided_at": "2026-11-24T13:00:00Z", "link": "https://example.com/decisions/eval-D2" }
  ],
  "budgets": [
    { "id": "eval", "label": "Eval", "spent": 120, "cap": 473.50, "unit": "usd", "reset_at": null, "run_out_at": null,
      "breakdown": [{ "lane": "Eval · Fixtures", "agent": "fixture-review", "model": "example-model", "spent": 45 }] }
  ],
  "lanes": [
    {
      "status": {
        "Lane": "Eval · Fixtures",
        "State": "active",
        "Goal": "Verify the fixture set.",
        "Next": "Finish fixture review · due 2026-11-25T15:00Z",
        "Blockers": "none",
        "Decisions": "eval-D1 Extend the fixture set? · to owner · asked 2026-11-24T12:00Z",
        "Spend": "spent=120 cap=473.50 unit=usd",
        "Links": "https://example.com/issues/8",
        "State-of-work": "open PRs none; holds none; next action Review fixtures; owner sources none"
      },
      "alerts": [],
      "health": "green",
      "activity": "finished",
      "last_update_at": "2026-11-24T13:45:00Z",
      "last_seen_at": "2026-11-24T13:52:00Z",
      "agents": [
        { "id": "fixture-review", "activity": "finished", "last_seen_at": "2026-11-24T13:50:00Z" }
      ]
    }
  ],
  "hosts": [
    { "host": "build-1", "temperature_c": 64, "gate": "ok", "observed_at": "2026-11-24T13:50:00Z" }
  ]
}
```

### Lane fields

The following is the full `kind=status` field list from the
[Agent Workbench v0.2.13 manager-sync reference](https://github.com/onsager-ai/agent-workbench/blob/v0.2.13/skills/orchestrate-harnesses/references/manager-sync.md#lane-status-and-alerts).

```text
Lane: <project> · <scope>
State: active|parked|done
Goal: <one sentence>
Next: <milestone> · due <YYYY-MM-DDTHH:MMZ>
Blockers: none | <item>; <item>
Decisions: none | <lane>-D<n> <question> · to owner|chief-pm · asked <YYYY-MM-DDTHH:MMZ>; ...
Spend: none | spent=<number> cap=<number> unit=<unit>; ...
Links: <PR, issue or thread links>
State-of-work: open PRs <#n@sha>, ...; holds none | <item>; next action <one line>; owner sources <link to the owner message for each standing authorization>
```

The Chief PM splits these lines into exact key and value pairs under `status`.
Keep the text and embedded time format, including the complete `State-of-work`
handover record. `State` and `Next` describe lane work and its milestone.
`last_update_at` timestamps the last full status. A heartbeat advances
`last_seen_at` and keeps the last full status. For each lane and listed agent,
`last_seen_at` is the last heartbeat or direct observation at that scope. An
agent timestamp can be null if there is no observation. A fresh snapshot alone
does not advance any of these timestamps.

The reference says `kind=alert` messages “Name the lane, what happened
and what is needed.” Carry the current alert texts for that lane in `alerts`.
The Chief PM supplies `health`. V0.2.13 defines no run state. The separate
`activity` fields report observed lane and agent execution, or `unknown` when
there is no evidence. A `finished` run leaves `State` and `Next` as reported.
Use stable Chief PM agent IDs within the snapshot. These IDs identify the spend
and availability rows. Native session IDs are deferred to v2. For hosts, use
the infra lane's `Thermal` reading and its observation time.

### Receiver validation

- Enforce a **256 KiB** limit on the entire request body before JSON
  parsing. Enforce the limit while reading as well as for declared lengths.
  Require Content-Length and accept uncompressed UTF-8 JSON with all required fields. Reject malformed UTF-8 or
  JSON, duplicate object keys, unknown fields or versions and invalid enums.
  Version `1` is the sole current contract under the pre-release policy.
- Parse structured timestamps as UTC RFC 3339 with an uppercase `Z` and
  seconds. Require a real UTC date and time in the minute-precision `Next`
  suffix and preserve the manager-sync text. Give `generated_at` the time the
  Chief PM assembled the snapshot and `chief_pm_checked_at` the time it last
  reviewed the reported state. Reject `generated_at` over five minutes ahead
  of receiver time. The check, decision, lane update and host observation
  times and non-null `last_seen_at` values must be at or before generation.
  A lane's `last_seen_at` must be at or after its `last_update_at`. Milestones,
  resets and forecasts may be in the future.
- Require unique decision IDs across both decision lists and unique budget IDs,
  `status.Lane` values, agent IDs and host names within their respective lists.
  A breakdown row must name a lane, an agent, or both. These names must resolve
  to the snapshot's entries and, when both are present, identify the same lane.
  Require a host in decision links and disallow URL credentials. Treat all
  supplied text as plain text. Snapshots must contain no secrets. Screen every
  text value for credential assignments, bearer markers, common token prefixes,
  private keys, and high-entropy strings. A 40-character hex Git SHA in the
  required `#n@sha` PR reference form is allowed. Credential assignments and
  known credential forms are still rejected. Reject invalid payloads as a whole.
- Return `204` after durable append, `400` for invalid JSON or schema, `401`
  for absent, invalid, revoked or wrong-scope credentials, `413` for oversized
  bodies, `415` for unsupported content type or encoding, and `411` when the
  request has no Content-Length. Responses carry
  no snapshot data. A failed write leaves the last committed snapshot readable.

## Authority and storage

Follow the receiver's push bearer pattern. Extend `semon token --dir DIR` with
`--snapshot` as the explicit minting scope for `POST /v1/manage/snapshot` and issue that token
only to the Chief PM. Recheck the route scope and revocation on every request.
The token grants only this POST. Owner read credentials and push credentials
cannot submit snapshots. The receiver operator lists IDs with
`semon token --dir DIR --list` and revokes by ID with
`semon token --dir DIR --revoke ID`. Revocation blocks subsequent writes and
preserves committed records. Audit issuance, revocation and ingest outcome by
credential ID where known, with no bearer, payload or decision links in logs.
Use HTTPS for non-loopback transport. [Push protocol](../push-protocol.md)

The receiver appends one immutable private record per accepted POST, with
receiver receipt time, credential ID and the validated snapshot. Show the
committed record with the newest `generated_at`, breaking ties by append
order. A delayed older snapshot stays in history without replacing newer
content. Retention is indefinite. Only explicit operator action deletes records. There is no automatic expiry or page history browser.
Snapshot data stays receiver-local and outside analytics, the Catalog and the
Hub. Native body text and T3 thread reads are outside the ingest path.

## Viewer page

Reuse the Viewer shell and sidebar, with `/manage` as an authorized receiver
host page under the canonical router. Require the existing owner read token
or its session cookie on every page or data request. Keep the route out of
public surfaces, and prevent shared caches from storing its responses. Fit
one column at 360 px with wrapping rows, readable text and status labels in
addition to color. [Viewer ownership](viewer-ownership.md) · [Viewer security](../../ui/README.md#viewer-security-boundary)

The page header shows generation time, snapshot age and the last Chief PM
check. At 90 minutes from `generated_at` the snapshot is amber, and at 3 hours
it is red. Use receiver time and clamp negative age to zero for allowed clock
skew. Show the last known data with its age even when stale. Before the first
snapshot, show “No snapshot received.”

1. **Waiting on the owner.** Show decision ID, lane, question, recommendation
   and `waiting_since`.
2. **Spend.** Show each budget's `spent`, `cap`, `unit`, reset and run-out
   forecast. Give the two Kiro pools, OpenRouter and each lane cap its own
   budget ID. A null time means the Chief PM supplied no reset or forecast. Show “Forecast
   unavailable” when forecast data is incomplete. Do not extrapolate.
   Show optional breakdown rows by lane or agent and include the model where
   known. Rows use the parent budget's unit and reporting period. Breakdown
   rows can cover part of a budget. Use the budget's reported total.
3. **Lane health.** Show one row per `Lane` with reported color and text, work
   `State`, milestone and due time from `Next`, and `last_update_at`. Label
   lane and agent `activity` as run activity and show their `last_seen_at`
   times separately. Show unavailable timestamps as unknown. Flag an active
   lane as overdue when its due time has passed and stale when its last contact
   is 6 hours 30 minutes old, allowing 30 minutes beyond the six-hour heartbeat.
   Parked and done lanes have no heartbeat requirement. The separate Catalog
   stalls list uses a fixed `idle_minutes = 60`.
   Surface current alerts with their lane.
4. **Decisions made without the owner today.** Show the Chief PM decision log's
   entries for the current Asia/Shanghai day, with decision ID, lane, summary, time and
   source link. The owner follows the link to review or veto in the decision
   log. `/manage` has no write controls.
5. **Hosts.** Show temperature in °C, `ok` or `gated`, and observation time.
   Below the host readings, show live stalled sessions from the existing
   owner-authorized Catalog `stalls` query. Show Catalog unavailability
   independently of the snapshot's status. The query uses Catalog metadata and
   leaves native question bodies unread.

## V1 data boundaries

Structured snapshot timestamps are stored in UTC. The Viewer converts them to
Asia/Shanghai for display and for the “today” boundary. Budgets, forecasts,
lane health and run activity come from the supplied snapshot. Native session
activity does not cross-check the snapshot in v1. The stalled-session list is
an independent Catalog read.

Prior art: multica (custom license, no code reuse).

V2 may cross-check a lane's reported state against native session activity.
That requires lane reports to name their native session ID and is outside v1.
