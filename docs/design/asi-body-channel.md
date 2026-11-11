# Online body channel (#594)

Status: coordination note reconciled with the #593 contract in PR #634.
[ASI federation](asi-federation.md) remains binding, including D2/D3 and the
#630 cache/#631 ACL/audit boundaries. [Enrollment](asi-enrollment.md) owns shared
transport infrastructure; this note owns #594 behavior. #568 owns receiver Viewer
integration. Design agreement does not establish runtime delivery.

## Scope and shared transport

**Decision — closed read capability:** `resolve`, `read_page`, `read_field`,
`subscribe_appends`, `cancel`, `acknowledge`; this preserves the read boundary.
No generic RPC, arbitrary native ranges, search, SQL execution, control, runtime,
SSH or Encrypted Relay. Bodies remain origin-owned; Hub never participates.

**Decision — reverse long-poll:** origin POSTs `/v1/body/poll`; receiver returns
at most one pending typed request or an empty response within 20 seconds. Origin
POSTs bounded frames to `/v1/body/requests/<opaque-request-id>/response`. Paths
contain opaque IDs only; selectors/assertions travel in bodies. No inbound origin
port, redirects or alternate rendezvous endpoint. HTTP/1.1 reuses the blocking
receiver; HTTP/2 multiplexing and WebSocket upgrades are unnecessary for this slice.
Thread-per-connection is interim; #636 plans an event-driven listener.

**Agreed boundary — #593 slice 4a, in `crates/semon-push`:** PR #634 is the target
contract for enrollment schema, kind-aware credential custody, rotation/revocation,
TLS and shared-listener dispatch. Consume its logical interface:

```text
authenticated_enrollment(conn, request_credential, required_kind)
  -> { enrollment_id, machine, namespace, origin_incarnation,
       index_incarnation, publisher_claim_epoch, kinds, verified_client_identity }
```

Body-route mTLS is settled, not an amendment. Slice 4a builds client-certificate
support; #594 consumes this authenticated scope and never touches TLS configuration.
See [enrollment](asi-enrollment.md#credential-kinds-and-custody) for kind separation,
including the current analytics client-certificate exception. A transport credential
never supplies per-user body authority. The old `tokens` file is refused with a
re-issue message, never deleted; the owning contract defines replacement custody.

Use the single [capacity table](asi-enrollment.md#capacity-and-admission), including
its separate capped poll class and closed-reason 503 refusals instead of excess
queuing. Those poll/queue caps are agreed interim limits; do not maintain another
table here. #594 confirms 20-second polls and 16 KiB control envelopes, owns read/
lease deadlines, and consumes 4a admission/revocation hooks. Non-poll reservation
split remains open. Body frames never enter analytics objects or reconnect spools.

## Assertions and exact envelopes

**Decision — receiver-issued Ed25519 assertions:** receiver-as-issuer is the
default, explicitly trusting its operator to impersonate users. Independent issuer
or local-only mode remains available when that trust is unacceptable. See
[enrollment provisioning](asi-enrollment.md#origin-configuration-and-out-of-band-enrollment)
for `semon receive enroll`, issuer/key ID/public-key pins and the at-most-two-key
rotation overlap. 4a owns that state/custody; #594 owns encoding/sign/verify,
implemented as `body_assertion` in open PR #635.

Assertions use fixed versioned signing encoding and bind issuer/key ID, issued-at,
expiry (≤60 seconds), audience (origin incarnation + namespace), claim epoch,
nonce/request ID, subject, machine/namespace, ACL grant version, receiver policy
version, request digest, deadline and channel binding. Origin nonce state is bounded
through expiry; exhaustion refuses grants. A fresh origin-loop channel binding
rejects old assertions after restart. Requests cannot install issuer keys.

**Decision — bounded envelopes:** versioned closed enums and `deny_unknown_fields`,
request correlation, ordered frames and final marker prevent ambiguous dispatch.
Complete serialized page/field envelopes, including metadata, stay within D3's
256 KiB; splitting frames never increases this budget. Operation deadline includes
queue/network, decoding and rendering; narrow the existing 2-second focused render
budget by remaining time. Reject oversized, malformed or late frames before release.

Bind responses to request digest/ID, authenticated enrollment/epoch, source handle,
complete cohort generations/consumed boundaries, activity/transcript generations,
export-view generation, selector/anchor, exact mapping, continuation, payload digest,
origin authorization audit ID, both sides' policy/redactor versions and narrower
limits. Verify completeness, bindings and digest before admission; mismatch publishes
nothing. TLS authenticates the origin; a digest alone is not evidence authority.

## Authorization, audits and cache

**Origin:** validate assertion/audience/channel/nonce/deadline → resolve the opaque
handle against admitted native manifest → evaluate complete contributor policy and
source witness → persist/fsync authorization → render/redact → immediately recheck
the full pinned authorization vector → egress → receipts/completion. Pin contributor
policies, scan/redactor versions, enrollment/claim epoch and revocation state, plus
source witness. A changed vector refuses or requires fresh audited authorization;
the same rule covers `authorization_only`. No receiver paths or worker index paths
supply authority.

**Decision — dedicated origin audit:** PR #635 supplies the origin audit store.
Use separately configured owned 0700 `body-audit/`, 0600 `audit.lock` and
`attempts.jsonl`, closed request/principal/source/generation/range/purpose/decision/
destination/version/time metadata and opaque enrollment/export-view bindings.
Fsync events and initial directory creation; exclusive writer, ≤64 MiB ceiling,
completion reservations and at most 16 bounded receipts. No bodies, queries, paths,
credentials or free-text errors. Torn/full/unwritable audits refuse; no automatic
repair, pruning or deletion.

**Receiver:** caller-authenticated principal (login outside this note) → ACL before
discovery, uniform denials → `begin_attempt` → origin round trip → verify response →
`authorize(AttemptId, binding)` append/fsync → non-Clone exact-key `ReadAdmission` →
cache lookup/delivery → `receipt`/`complete`. PR #635 adds the authorization event
within reserved attempt space: matching request ID, exact response identity, verified
origin policy/redactor versions and origin authorization audit ID. Cache hits require
the same binding. Recheck current ACL/policy before release; changes need fresh
admission. Audit failure refuses release; later receipt failure stops emission and
leaves an attempted read, not false success.

**Decision — cache hits contact origin:** `resolve` with `authorization_only` and
candidate envelope key/digest authorizes/audits one cached page without payload.
Candidate metadata does not permit cache body lookup before admission. Analytics
supplies `SourceWitness`, opaque handles and policy vector, **not** an export-view
witness or exact anchor/redaction mapping. #594 slice 3 owns those; until its origin
adapter can validate the exact export witness, authorization-only hits return
`unavailable`. A miss needs fresh page authorization; TTL is never authority.

**Decision — reuse typed `body_query::view` fields:** retain `untrusted_text` and
focused budget. Current origin-local `view` refuses receiver/Hub access and supplies
no remote exact mapping. Slice 3 supplies safe projection/mappings; unsupported or
withheld anchors return unavailable, never approximate matches or masked original-
byte proofs. No whole-session replay/model inference. #594 owns page/field-to-numeric
exported-audit-range mapping; transformed offsets are not native evidence.

## Leases, failures and implementation

**Decision — subscriptions repeat bounded pull pages:** scoped lease ID/expiry,
watcher count, buffer/byte caps, acknowledgements and exact continuation gate each
page, with fresh D2 authorization/audit. Zero watchers, cancel, revocation, disconnect
or expiry stops emission. Reconnect requires fresh grant and continuation/cohort/
export-view checks; rewrites or policy/recipe changes cannot splice generations.

Offline/unreachable/timeout yields explicit unavailable with closed reason and
retryability, never empty success or T3/Hub/mirror fallback. Expire pending work;
discard late responses. No body retry spool; federation's no-store/logging rules apply.

Each PR carries `Part of #594`:

1. **PR #635, already open:** `body_assertion`, origin audit store and receiver
   `authorize` primitive. Later slices target #634 interfaces; no channel wire types
   or routes before #634 merges.
2. Poll/response endpoints, bounded frames/queues and ACL/audit/admission/cache wiring
   through 4a hooks. No release before the origin path exists.
3. Origin loop, resolve/page adapter, export-view witness, exact mappings or explicit
   unavailable, audit-range mapping and end-to-end D2/cache-hit checks.
4. Leases, acknowledge/cancel/backpressure and fresh reconnect grants.
5. Integrated CI acceptance and resource measurements; owning slices include tests.

| Channel acceptance case | Slices |
| --- | --- |
| Traversal/symlink races; stale generations | 3 |
| Replay, foreign namespace, wrong audience | 1, 3 |
| Unsupported verbs/malformed frames; control/runtime/SSH before dispatch | 2, 3 |
| Zero watchers, credential revocation, backpressure within bounds | 2, 4 |
| Cross-user discovery/cache/field refusal | 2, 3 |
| Final policy-to-audit binding, including hits | 1–3 |
| Stream-through without disk writes; key failure fail-closed | 2, 3 |
| Revocation/rewrite invalidation | 2–4 |
| Length-preserving AND length-changing redaction; exact anchors/refusal | 3 |
| CI latency/memory/cancellation; shared-listener exhaustion | 5, with 4a hooks |

## Remaining questions

1. What read, lease and cancellation deadlines/byte limits fit the confirmed poll
   bound and 4a hooks?
2. Which origin adapter/API validates authorization-only export-view witnesses, and
   which initial fields/anchors can it support exactly?
3. Within #594's mapping ownership, what numeric exported-range convention binds
   page/field selectors and export-view identity across both audits?
4. What non-poll reservation/fairness split keeps responses, publication and mirror
   work progressing under shared-listener exhaustion?
