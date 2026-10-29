# Jev ASI evaluation corpus

`data/events.jsonl` contains 58 event rows: 30 records copied from sanitized
Claude Code and Codex compatibility fixtures, plus 28 authored adversarial
records. `data/synthetic-adversarial.jsonl` holds the exact raw native-format
lines for those authored records. The challenge split is synthetic and must
not be presented as real-world performance evidence.

## Sources and provenance

The Semon source fixture revision is
`893dd56fdc63fd53b7febd597c1381570c01f64b`. Fixture rows point to the exact
source file, one-based line, manifest, and source-line SHA-256. The line hash
includes its terminating LF; `native.raw_sha256` hashes the exact record text
without that LF. `native.raw` is the source line with no parsing or
re-serialization. Missing native timestamps are represented as `null`.

Observed fixture rows come from these sanitized inputs:

- `tests/fixtures/compatibility/claude-2.1.288/tools/initial-transcript.jsonl`
  and its `tools/manifest.json`.
- `tests/fixtures/compatibility/claude-2.1.288/initial-transcript.jsonl` and
  `manifest.json`.
- `tests/fixtures/compatibility/claude-2.1.288/lifecycle/{resumed-transcript,forked-transcript}.jsonl`
  and `lifecycle/manifest.json`.
- `tests/fixtures/compatibility/codex-0.159.0-alpha.3/initial-rollout.jsonl`
  and its `manifest.json`.

These manifests describe native CLI persistence with deterministic mock
providers. Session identities and roots were rewritten and private harness
payloads removed. The Claude tools fixture contains synthetic commands and
tool results, including one exit code 1; these are harness-format observations,
not real user sessions or an independent measure of failure frequency.

Every authored row is marked `origin: synthetic_adversarial` and
`split: heldout_challenge`; its source line and hash point to
`data/synthetic-adversarial.jsonl`. These examples cover repeated permission
failures and recovery, negated and quoted errors, explicit waits, an interim
assistant completion followed by another user request, unrelated repetition,
environment/model/transport/unknown failures, an unknown record type, and a
truncated native record. The retry burst uses 75 ms event spacing; observed
fixture timestamps are preserved unchanged.

The schema keeps source and labels separate. `context` carries the physical
session ID, native timestamp, `event_order`, and whether the input is a full
authored session, a fixture excerpt, or a partial record. `gold`
contains `streaming` and `offline` views with activity, failure category,
retry/loop, anomaly, and phase labels. Streaming labels use only the current
event and earlier events. Offline labels use all corpus events for that
session. In particular, an assistant turn end does not establish a session
end; later user prompts make earlier completions interim. `ending` is only
assigned in the offline view when the complete corpus session ends there.
`annotation_note` is a top-level field alongside `gold`.

The labels are Worker-authored weak annotations. They have not had independent
human review, and should be treated as an initial benchmark reference rather
than human-reviewed gold. `annotation_note` records ambiguity and the basis
for non-obvious labels.

CI runs the corpus integrity suite with:

```sh
python3 -m unittest discover -s experiments/asi-jev -p 'test_*.py' -v
```

`test_corpus.py` checks original fixture and synthetic source-line hashes,
exact raw-line preservation, schema enums, unique IDs, and strictly increasing
per-session `event_order`. These checks are intended for CI; no local result is
claimed here.

## Native pipeline and PoC seam

At Semon revision `893dd56fdc63fd53b7febd597c1381570c01f64b`,
[`crates/semon-sessions/src/inputs.rs`](../../crates/semon-sessions/src/inputs.rs)
defines Claude project transcripts and Codex session/archived-session rollouts
as native JSONL inputs. The current summary path in
[`crates/semon-sessions/src/native_tree.rs`](../../crates/semon-sessions/src/native_tree.rs)
dispatches Claude transcript records and Codex rollout records by harness.
The full renderer retains each harness's native parsing and event semantics.

Hub's local checkout is at `dac75333f9ce4bf594aec762e284019f78bc66e1`, with
its Semon submodule at `721078a6f68d8cebb034c6b3626fc68abe1ec340`; both are
older than this task's Semon worktree. For current Hub `main` at
`b18d989710ae44f5dac1a9799f734be43b59dd2e`,
[`crates/semon-hub/src/push.rs`](https://github.com/onsager-ai/semon-hub/blob/b18d989710ae44f5dac1a9799f734be43b59dd2e/crates/semon-hub/src/push.rs)
(blob `976835b4711edbceb59a90f36dd24cfdcacb55d1`) was read through GitHub's
contents API. Its `/api/push/v1/mirror/append` route admits machine-scoped
native bytes, writes the mirror, and invalidates the corresponding viewer
source for rebuild; it does not add activity labels.

The standalone PoC seam is after native line acquisition and before any
activity-specific analytics: feed the same `native.raw`, harness, session,
timestamp, and order to rules, Jev, and an optional lightweight model. Compare
per-event, micro-batch, and event-triggered predictions against both gold
views. Preserve event order for streaming runs; group no more than four
arrivals in the 500 ms trigger window. Keep predictions and timings outside
the native raw record and report synthetic challenge results separately from
sanitized fixture results. Do not modify Semon parsers or Hub ingestion for
this PoC.

## Custody and privacy

This corpus contains only checked-in sanitized compatibility fixtures and
Worker-authored synthetic records. It does not export local transcripts, live
Hub mirrors, archives, credentials, or private logs. Fixture provenance must
remain attached to each row; do not relabel mock-provider content as naturally
occurring user behavior. For any later integration, pass only authorized
selected source bytes through Semon's request-scoped native reader as described
in `docs/design/session-source-reader.md`; that reader treats a source reference
as provenance, not permission, and ordinary trace responses do not expose raw
bytes or carrier labels.
