# Real-session ASI/Jev evaluation

Evaluation in progress, 2026-10-30. Results from the separate fixture
[feasibility experiment](REPORT.md) are excluded from this evaluation.
No production parser, runtime, storage contract, or mandatory model dependency
is changed. Final results and recommendation are pending the blind reference
freeze and the actual nine-condition replay.

## Cohort and custody

The frozen cohort contains 436 eligible events from 20 complete captured
physical sessions and 20 distinct original tasks: 12 Claude Manager captures
and eight Codex Worker captures. These are this user's real local native CLI
logs, predominantly older standalone orchestrated runs. Harness and role are
confounded; this purposive, small-session, error-enriched cohort does not
represent all interactive work or deployment prevalence. A complete captured
file does not prove that its task finished or that a native terminal event
exists.

Exact local raw copies retain 1,788 native records and 7,297,385 bytes.
Projection accounting records 33 text-field truncations, 20 orchestration
instruction envelopes redacted, 157 internal-reasoning omissions, 924 metadata
omissions, 169 unrecognized-envelope omissions, and 46 exact duplicate native
lines omitted from projection. Original bytes remain intact. Independent
privacy review removed seven whole text fields across six packets, then
rechecked all 436 rows. No whole-session privacy exclusion was needed in the
final cohort.

Raw copies, projections, annotations, predictions, audit material, examples,
and user-review material remain under gitignored `private/`, with directories
0700 and files 0600. Only reviewed sanitized projections may reach the
configured inference providers. No real session material goes to GitHub or CI.

## Frozen method and execution notes

[REAL_PROTOCOL.md](REAL_PROTOCOL.md) freezes the taxonomy, projection,
selection constraints, success criteria, and statistics before predictions.
It includes Codex error reason/message fields and Claude stop reasons. Native
parsing remains authoritative. The bounded semantic projection omits hidden
reasoning, system prompts, credentials, environment values, private paths,
and unsupported payloads.

Two fresh blind labellers use only the controlled annotation interface and
[taxonomy guide](LABEL_GUIDE.md), followed by a separate blind adjudicator.
Every evidence response must parse as complete JSON before labelling, with
40,000-token command and enclosing tool output budgets. The interface verifies
the frozen projection's counts and hashes on each request. Full-session review
opens only after both causal passes finish. No model predictions are visible
to reference labellers or adjudicator. These are AI reference judgements,
not human gold labels; the 50-event user audit is reported separately.

Two preliminary annotation attempts are excluded before classifier outputs:
the first B pass encountered tool-output clipping of unknown extent; the first
A pass lacked positive evidence of complete packet receipt. Original answers
remain archived unchanged locally. Both replacement labellers use fresh
Sol/high contexts. The replacements do not reuse either excluded pass.

One frozen-implementation deviation was discovered during fresh annotation,
before classifier inference: causal windows retain the byte tail of older
evidence and can start mid-record, rather than retaining whole older rows as
the protocol prose says. The current target is reserved. Offline windows use
whole selected context rows. The formatter remains unchanged for labels and
classifiers; interpret context-sensitive results with this limitation.

The nine conditions are rules prefix/offline; Jev per-event prefix,
microbatch prefix, triggered prefix, and offline; and Haiku per-event prefix,
triggered prefix, and offline. Model conditions repeat three times with four
independent session workers. Repetitions measure variability rather than
increasing the independent sample size. The two triggered arms share the
same frozen rules gate. There is no automatic request retry or provider switch.

Both model arms use the remote Magpie gateway. The documented local usage
ledger does not cover this route. Billed costs are unknown; any Jev direct
list-price estimate is separate from a gateway bill. No credential/config
discovery or undocumented remote ledger access is attempted.

CPU processing uses current actual kernel temperature readings before and
after bounded work and stops at 85°C or missing telemetry. Each input read is
one file below 10 MB. Synthetic tests run in CI; no private corpus is uploaded.
No local cargo/node tests or project-wide typechecks are run.

## Results and recommendation

Pending blind-reference freeze and real model experiments. Do not interpret
this preparation document as a completed evaluation or production acceptance.
