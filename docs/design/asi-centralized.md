# Centralized ASI for one owner's machines

Status: **current architecture contract**, owner decisions of **2026-11-14**.
This amends [ASI direction](asi-direction.md). It replaces federation as the
default, not a claim that receiver ASI integration has shipped. This change is
docs-only; implementation follows the plan below. No merge, deployment or data
cleanup is authorized by this contract.

## Architecture

Origins send selected, redacted native logs using the existing `semon push` →
`semon receive` [mirror contract](../mirror-protocol.md). The receiver indexes
those logs, materializes ASI, runs analytics/SQL/search and serves transcripts
locally. It is the durable custodian of the selected redacted session text.
Origin availability is required for new uploads, not for reading retained copies.
This is the default for one owner using their own machines. [C1]

The Hub boundary is unchanged: body-free analytics and categorical labels only;
paths are tokenized at export. Neither text nor body/FTS rows enter analytics or
the Hub. Keep recorded costs without silent repricing (federation D4). Keep one
current Semon format, preserve frozen data, and add no compatibility layers (D5).
Incomplete inputs do not justify deleting or resetting retained history. [C1, I]

## Custody and privacy

The origin authorizes **export**, through pairing plus explicit source consent.
The combined approval says that this receiver keeps redacted native text and may
read it locally, including while the origin is offline. Narrowing the source set
is available in that same interaction. Preserve separate persisted source consent;
renewal cannot widen it and future sources need explicit approval. Disconnect or
revocation stops the relevant future access/transport; it cannot retract copies
already exported. Deletion is an explicit operator action. [C1, C4, M, I]

Use the shipped sender redactor and supported input allowlist. Redaction is best
effort, not proof of secret removal. Own-machine reads have **no zero-findings scan
gate**. The old P1 exclusion of raw replicas and online body/cache policy no longer
apply to this default. Export still requires source consent and preserves source
integrity; receiver custody grants no hosted-model or Hub body egress. [C1, C5, M, I]

Recommend receiver **full-disk encryption and encrypted backups** as the at-rest
baseline, on UX/DX grounds. There is no application-level encryption requirement
and no blocking setup check for disk/backup encryption. Keep private files mode
0600 and directories mode 0700. Do not claim disk encryption protects text from a
running receiver or its administrator. [C3, M]

## Identity and provenance

Preserve native session identity, lineage, source generation and provenance.
Received facts describe the origin; receiver hostname, paths and live processes
must not replace origin observations. Redacted received bytes are not original
unredacted source proofs. Keep mirror archive custody and source validation. [M, I]

The received machine needs a stable namespace binding independent of hostname or
directory. Its exact binding and duplicate-session accounting are **open owner
decisions** below; neither may be inferred from today's display/cache keys.
Identity work precedes receiver materialization. [C1, I; open A/B]

## Reads and audit

The receiver authorizes and audits reads locally, using its retained custody and
authenticated loopback Viewer boundary. The origin does not approve or audit each
receiver read. No online origin assertion, pull-body cache, or dual audit is part
of this default. Local read/audit integration remains implementation work; this
amendment chooses no new journal format or authentication protocol. Whether this
release needs users beyond the loopback owner remains open below. [C1, C2, I; open D]

## Four-action journey

Low friction, easy setup and easy daily use are primary design criteria. After
installation and endpoint reachability, the target is exactly four setup actions:

1. Run one receiver command that starts receive, received-machine indexing/ASI
   materialization and the authenticated loopback Viewer. Exact command spelling
   is not selected here.
2. Run origin `semon connect` to that receiver.
3. Enter the receiver's code in the trusted origin prompt.
4. Approve one combined trust, durable custody and source summary, with narrowing
   available inside that interaction. Push starts automatically after approval.

There are two long-running processes: origin push and receiver. Unattended service
installation remains an explicit, separately counted opt-in. Network/TLS setup and
installation are counted separately when needed; browser use and initial indexing
waits are also reported. [C4]

The shipped baseline still needs **seven setup actions**: receiver enrollment
command, origin connect command, code entry, source-selection response, approval,
separate push command, and separate Viewer command. See [README setup](../../README.md#receive-pushes-from-your-other-machines)
and [`connect_terminal.rs`](../../crates/semon-cli/src/connect_terminal.rs).
Do not advertise the four-action target as delivered. [C4, M]

Use shipped public-CA TLS pairing and automatic idempotent renewal under enrolled
trust. The owner deferred private-root/LAN pairing on **2026-11-14**, and #680
removed the rejected 71-character pinned invitation. Do not restore it or invent
a private-root profile. Loss/compromise still requires explicit trust recovery;
normal renewal neither widens consent nor revives revoked authority. [C5, M]

## Implementation plan

Keep each PR at roughly **1,000 changed lines or fewer**; split a step as needed.
These are implementation slices, not authorization to settle the open choices.

| PR | Scope and verified starting point | Trace |
| --- | --- | --- |
| 1 | Received-machine stable identity and durable custody; settle open A/B before encoding accounting or persisted bindings. Preserve mirror source/archive integrity. | C1, M, I |
| 2 | Received-machine indexing/materialization. Reuse [`ReceivedMachines`](../../crates/semon-sessions/src/received.rs) discovery and `SqliteAnalyticsInput` via [`analytics_writer.rs`](../../crates/semon-sessions/src/analytics_writer.rs). [`parse_materialize_args`](../../crates/semon-cli/src/main.rs) currently refuses `--machine`; this is a real integration gap. | C1, I |
| 3 | Receiver SQL/search/evidence and local authorization/audit. [`body_query.rs`](../../crates/semon-sessions/src/body_query.rs) currently refuses `Access::Receiver` in `admit`; its search/view worker already accepts multiple indexes. Viewer machine aggregation exists in [`union.rs`](../../crates/semon-sessions/src/union.rs) and [`overview.rs`](../../crates/semon-sessions/src/overview.rs); it is not proof of receiver ASI SQL delivery. Resolve open D before expanding reader identity. | C1, I |
| 4 | Four-action lifecycle and consent wording: combined trust/custody/source interaction, automatic push, integrated receiver/index/materializer/Viewer lifecycle. Keep enrollment, TLS and renewal. | C4, C5, M |
| 5 | After the switch and a dependency pass, remove federation runtime: #594 body channel, #593 publication objects/heads/egress handles, #595 receiver body cache/dual audit, and `analytics_publish`/`body_channel` credential kinds. Candidates include `origin_read`, `body_*`, `handle_resolver`, `egress`, `objects` and publication paths. Preserve enrollment, mTLS, renewal, mirror, archive custody, ASI projection/store/query and local read audit. No dormant federation mode or compatibility layer. | C2, M, I |

The candidate retirement estimate from the earlier audit was about 16,000 lines;
this is a sizing estimate, not a deletion list. Recheck dependencies and tests in
each slice. Preserve the [federation](asi-federation.md) and
[body-channel](asi-body-channel.md) rationale as superseded history, so a future
multi-user or third-party receiver can revisit it without dormant runtime. [C2, I]

## Open owner decisions

These choices are intentionally unsettled. Recommendations are not decisions.

| Open owner decision | Options | Recommendation |
| --- | --- | --- |
| A. Duplicate-session accounting when self-push or a copied home presents the same session on two machines | Count each machine's occurrence; deduplicate across machines using proven native identity/provenance; require an explicit canonical source choice | Deduplicate only where native identity/provenance proves duplication, retaining both source records. Decide the rule before aggregate accounting changes. |
| B. Stable received-machine namespace/identity binding | Bind to enrollment machine identity/enrollment ID; bind to an origin-generated namespace carried in facts | Prefer origin-generated namespace in facts for continuity, subject to an explicit binding decision. Hostname and directory are not candidates. |
| C. Receiver retention/storage beyond today's 20 GiB per-machine cap | Keep the current configurable refusal cap; add receiver-wide refusal limits; add operator-selected archival tiers | Keep the current cap first. No option automatically deletes history; any deletion remains explicit. |
| D. Reader authentication beyond the loopback owner in this release | Authenticated loopback owner only; add authenticated remote/multiple users | Recommend no additional users in this release: keep the single-owner loopback boundary. |

## Decision trace

All new mechanisms above use these sources; no historical document overrides them.

| Trace | Source and substance |
| --- | --- |
| C1 | Owner, **2026-11-14**: centralized default; existing mirror sends redacted native logs; receiver owns durable custody, indexing/ASI/query/search/transcripts and local read authorization/audit; origin authorizes export. Supersedes federation D1/D2/D3/D6/D8 and P1's no-raw-replica wording; Hub unchanged; D4/D5 retained. |
| C2 | Owner, **2026-11-14**: remove federation runtime after the switch; retain its rationale as superseded history; no dormant code. |
| C3 | Owner, **2026-11-14**: recommend full-disk encryption plus encrypted backups; no app encryption or blocking setup check; keep 0600/0700. |
| C4 | Owner, **2026-11-14**: UX/DX primary; four setup actions, combined narrowing/approval, automatic push, two processes; service install separately counted. |
| C5 | Owner, **2026-11-14**: private-root/LAN pairing deferred, pinned invitation removed (#680), no zero-findings scan gate for own-machine reads. |
| M | Shipped [mirror](../mirror-protocol.md), [enrollment and renewal](asi-enrollment.md): source allowlist/redaction, export consent, transport authentication, credential custody and current trust lifecycle. |
| I | [Repository invariants](../../AGENTS.md): native identity/lineage/provenance, source integrity and custody, body-free analytics/Hub, parser behavior, one current format, preserved frozen/user data and Viewer ownership/security. |
