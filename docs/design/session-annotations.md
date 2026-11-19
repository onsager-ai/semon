# Session annotations

Status: format approved by the owner on 2026-11-19 (via Ostrom PM). Semon reads
this format through `semon sessions`, including its Viewer.

An orchestrator that starts harness sessions (first user: Ostrom's `ostromd`)
knows facts about them that the native logs do not carry: which system started
them, the repository and ticket they serve, an issue/PR and a readable title.
Session annotations let that orchestrator publish those facts so Semon can
group and label its sessions. Semon only reads the file.

## Native names come first

Where the harness has its own naming, producers use it and need no annotation
for the label:

* Claude: a `custom-title` record (Agent SDK `renameSession`).
* Codex: `thread_name` in `$CODEX_HOME/session_index.jsonl`. The Codex TS SDK
  (0.167.0) has no naming call, so Codex labels currently come only from the
  annotation `title`.

A native name always wins over an annotation `title`.

## File

* Path: `$XDG_STATE_HOME/ostrom/semon-annotations.jsonl` (`~/.local/state` when
  `XDG_STATE_HOME` is unset or not absolute). This is the default configured
  path. `semon sessions --annotations PATH` overrides it, including with
  `--serve`; Semon never discovers files elsewhere.
* Owner: the producer. It appends lines. Semon never writes, locks, truncates,
  rotates or deletes the file. Compaction, if any, is the producer's job and
  must be an atomic replace (write a new file, then rename).
* Encoding: UTF-8 JSON Lines, one object per line, `\n` terminated. A final
  line without `\n` is treated as incomplete and ignored until it is completed.

## Record

```json
{"v":1,"harness":"codex","native_id":"01a1f225-b86d-7231-a754-0ba53d3264bc","system":"ostrom","repository":"onsager-ai/tolmap","ticket":"9","issue":null,"pr":364,"title":"Fix route ranking","updated_at":"2026-11-19T04:52:01Z"}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `v` | integer, `1` | Format version. Any other value rejects the line. |
| `harness` | `"codex"` or `"claude"` | Native harness of the session. |
| `native_id` | non-empty string | Native session id as it appears in the harness log. |
| `system` | non-empty string | Producing system; `"ostrom"` today. Top grouping level. |
| `repository` | non-empty string | `owner/name`. Second grouping level. |
| `ticket` | non-empty string | Producer's ticket id within the repository. Third grouping level. |
| `issue` | positive integer or `null` | Issue number in `repository`. |
| `pr` | positive integer or `null` | Pull request number in `repository`. |
| `title` | string or `null`, at most 200 characters | Readable label when no native name exists. |
| `updated_at` | RFC 3339 timestamp with offset | When the producer wrote this line. |

All fields are required. Unknown extra fields reject the line, so a producer
cannot silently depend on fields Semon ignores. Lines above 4 KiB are rejected.

## Semon's reading rules

* For each `(harness, native_id)`, the valid line with the latest
  `updated_at` wins. On an equal timestamp the later line in the file wins.
* Invalid lines (bad JSON, unknown `v`, missing or mistyped fields, oversize)
  are skipped and counted. `semon sessions --json` reports the counts of valid
  and rejected lines and the file path in
  `annotations: {files: [{path, valid, rejected}, ...]}`, one entry per distinct
  configured file (one entry for a single machine);
  matched rows expose `annotation: {system, repository, ticket, issue, pr, title,
  source: "annotation"}`. It never echoes rejected content. A missing file has
  zero counts; other read errors fail the read.
* An annotation for a session Semon has not seen is kept and applies when the
  session appears. An annotation never creates a session.
* Annotations are metadata only. They carry no message or tool text, and
  Semon puts no body text into them.

## What annotations change

* Grouping: annotated root sessions are listed under `system → repository →
  ticket`. Subagents stay under their native parent; they inherit the group
  through lineage and need no annotation of their own.
* Label: `title` is used when the session has no native name.
* Links: `issue` and `pr` link to the repository's GitHub pages.
* Every annotated value is shown as coming from the producer ("from Ostrom"),
  not as native evidence.

Annotations never change native identity, lineage, parent selection,
provenance, state, usage or costs. A conflicting annotation (for example two
tickets claiming one session over time) is resolved only by the latest-wins
rule above; Semon does not merge or reconcile them.

## Compatibility

Semon is pre-release. A future format change bumps `v` and updates producer
and reader together; Semon does not keep readers for older `v` values.
