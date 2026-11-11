# Online body channel (#594)

Status: proposed coordination note for #593 and #594; unresolved choices below
are not accepted amendments. [ASI federation](asi-federation.md) remains binding,
including D2/D3 and the #630 cache/#631 ACL/audit boundaries. #568 owns receiver
Viewer integration. This note proposes one shared receiver transport foundation.

## Scope and current boundary

**Decision — closed read capability:** `resolve`, `read_page`, `read_field`,
`subscribe_appends`, `cancel`, `acknowledge`; this preserves the contract's read
boundary. No generic RPC, arbitrary native ranges, search, SQL execution, control,
runtime, SSH or Encrypted Relay. Bodies remain origin-owned; Hub never participates.

At origin/main `25fe18de0`, [serve.rs](../../crates/semon-push/src/serve.rs) is a
blocking, thread-per-connection HTTP/1.1 receiver with operator TLS configuration,
32 default connection slots and socket deadlines. [Mirror](../mirror-protocol.md)
uses bearer machine tokens and per-machine directories; origins initiate pushes.
The merged cache and ACL/audit libraries expose no remote body routes.
`body_query` admits `Access::Origin`, refuses `Receiver`/`Hub`, and caps focused
`view` output at 256 KiB. Governance evaluates admitted contributors; it does not
authenticate users. These are building blocks, not delivered remote guarantees.

## One transport foundation with #593

**Decision — reverse long-poll over the existing listener:** origin POSTs
`/v1/body/poll`; receiver returns at most one pending typed request, or an empty
response after at most 20 seconds. Origin POSTs bounded response frames to
`/v1/body/requests/<opaque-request-id>/response`; no inbound origin port or async
runtime is needed. Paths contain only opaque IDs; sensitive selectors/assertions
travel in bodies. No redirects, alternate providers or rendezvous service.

**Decision — reuse HTTP/1.1 now:** HTTP/2 offers multiplexing but replaces the
current framing/connection machinery; WebSocket adds upgrade and persistent-frame
state. Neither is necessary for bounded pull pages. Long-polls consume threads:
reserve publication/response capacity, cap polls per enrollment and bound pending
queues; do not let polling exhaust the current 32-slot listener. Numeric fleet
capacity must be agreed with #593 before implementation.

**Decision — share infrastructure, separate authority:** #593 and #594 share the
enrollment record, namespace/origin-incarnation identity and claim epoch, TLS/cert
configuration, listener resource accounting and credential custody. Analytics
keeps its content-addressed objects, staged validation and atomic namespace heads;
body frames never enter that store or its reconnect spool. Separate `/v1/body/`
routes require an explicit `body_channel` credential kind for polling/responses;
a push credential grants neither polling nor user body reads. Per-user
`body_read` remains an additional ACL and origin-policy decision.

**Decision proposed — mTLS for body routes:** use operator receiver certificates
plus enrolled origin client certificates, with certificate identity bound to the
same enrollment scope; this satisfies the contract's mutual-TLS requirement.
The current bearer-only listener does not. Extend its rustls client verification
and enforce a verified client identity on body routes, alongside credential-kind
checks; do not create another listener/transport. Reusing only server-TLS plus
bearer would change the accepted contract and requires explicit PM/owner approval.

#593 must agree/provide one enrollment schema: receiver identity and HTTPS
endpoint/trust root, machine, namespace, origin incarnation, publisher claim epoch,
client certificate identity, enabled credential kinds and revocation state, plus
assertion issuer key ID/public key. Store private keys and any bearer secret in
operator-controlled 0600 files/secret storage outside bodies, caches and backups;
receiver bearer records, if used, retain hashes only in a versioned kind-aware
store. These fields/storage APIs are proposed, not claimed to exist in #593.
Main includes publication provenance (#624); the open PR list inspected on
2026-11-11 exposes no #593 transport PR. The brief places slice 2 in progress and
analytics transport/reconnect in slice 5; their concrete interfaces need agreement.

## Identity and envelopes

**Decision — receiver-issued Ed25519 assertions:** use `ring` (already in the
dependency tree through rustls; declare a direct dependency when implemented),
with a fixed versioned signing encoding, avoiding ambiguous JSON signatures.
Signed fields: issuer/key ID, issued-at, expiry (at most 60 seconds), audience
(origin incarnation + namespace), claim epoch, random nonce/request ID, subject
principal, machine/namespace, ACL grant version, receiver policy version, request
digest and deadline. Origin pins the issuer public key through explicit enrollment,
never through a request; issuer private keys stay in separate protected custody.
A bounded origin nonce cache retains accepted IDs until expiry; full cache refuses
new grants. Each origin loop creates a fresh random channel binding, sent in its
poll and included in assertions; restart rejects assertions for the old binding.
Receiver-as-issuer explicitly trusts its operator to impersonate users; choose an
independent trusted issuer or local-only mode if that trust is unacceptable.

**Decision — exact, bounded envelopes:** versioned closed enums and
`deny_unknown_fields` at every object boundary, request-ID correlation, ordered
frame numbers and a final marker prevent ambiguous dispatch/partial publication.
Proposed control-frame cap: 16 KiB; complete serialized page/field envelope cap:
256 KiB including metadata (D3). Each response frame is at most 256 KiB; splitting
never increases a page budget. One operation deadline includes queue/network,
decode and render time, with the existing 2-second focused render budget narrowed
by remaining time. Reject malformed/oversized/late frames before dispatch/release.

Response bindings include the request digest/ID, authenticated enrollment/epoch,
source handle, full cohort generations and consumed boundaries, separate activity
and transcript generations, export-view generation, selector/anchor, exact mapping,
continuation, payload digest, origin audit authorization ID, current origin and
receiver policy/redactor versions and narrower limits. Receiver verifies bindings,
frame completeness and digest before constructing `ReadAdmission`; mismatch
publishes nothing. TLS authenticates the responding enrolled origin; a digest
alone is not evidence authority.

## Read and audit sequence

**Origin:** validate assertion, audience, channel binding, nonce and deadline →
resolve the opaque handle against the admitted native manifest → recheck complete
contributor policy, source identity/generation and consumed boundaries → persist
and fsync origin authorization audit → render a typed page, apply governance and
redaction → recheck the full pinned authorization vector immediately before
egress → release bytes → record receipts/completion. Pin contributor policies, scan/redactor
versions, enrollment/claim epoch and revocation state, and source witness at
authorization time. Any change during rendering refuses egress or requires fresh,
durably audited authorization. Apply the same check to `authorization_only`
cache-hit responses. No receiver paths or worker `indexes` are accepted as authority.

**Decision — dedicated origin audit store:** propose an explicitly configured
owned 0700 `body-audit/` directory outside native roots/cache, containing 0600
`audit.lock` and `attempts.jsonl`. Mirror `receiver_audit`'s closed request/principal/
source/generation/range/purpose/decision/destination/policy/redactor/time fields,
adding enrollment scope and export-view binding as opaque identifiers. Fsync each
event and initial directory creation; use exclusive writer locking, a 64 MiB ceiling
(or smaller), reservations for completion and at most 16 bounded receipts. No body,
query, path, credential or free-text error is recorded. Invalid/torn/full/unwritable
audit refuses new reads; no automatic repair, pruning or deletion. This is a new
store design, not functionality supplied by the receiver audit library.

**Receiver:** caller supplies an authenticated principal (login/session design is
outside this note) → `receiver_acl::decide` before discovery, with uniform denials
→ `receiver_audit::begin_attempt` → origin round trip → verify authorization and
response → append/fsync an authorization-binding audit event → construct the
existing non-Clone, exact-key `ReadAdmission` → cache lookup/rendered-page delivery
→ `receipt`/`complete` with actual bytes or failure. The binding matches request ID,
exact response identity, verified origin policy/redactor versions and origin audit
authorization ID. Cache hits require the same durable binding. Current
`begin_attempt` fixes metadata; `receipt`/`complete` cannot add these bindings.
Recheck ACL/policy versions before release; changed grants require fresh admission.

**Decision — cache hits still contact origin:** `resolve` with `authorization_only`
and a candidate envelope key/digest asks origin to authorize and audit exactly one
cached page/field without returning payload. Candidate identity is metadata only;
no cache body lookup occurs before admission. Origin must validate that exact
export witness, not echo the receiver's claim; inability to validate is unavailable.
A miss requires a fresh bounded page request/authorization. TTL never authorizes a
read. Audit failure on either side refuses release; a later receipt failure stops
further emission and leaves the durable attempted read, never false success.

**Decision — first payload reuses typed `body_query::view` fields:** preserve its
`untrusted_text` wrapper and focused budget. Its current `body_uid`/field output is
not the federation's exact native-anchor/redaction mapping. The origin adapter must
supply that mapping and policy-safe projection before claiming exact evidence;
unsupported/withheld anchors explicitly return unavailable, never approximate text
matches or masked original-byte proofs. No whole-session replay or model inference
is allowed to prepare a page. Receiver owns presentation, not native recipes.

## Leases, failures and delivery slices

**Decision — subscriptions are repeated bounded pull pages:** `subscribe_appends`
returns a scoped lease ID/expiry and exact continuation; watcher count, per-lease
buffer/byte caps and acknowledgements gate each next page. Every page receives a
fresh D2 authorization/audit. Zero watchers, cancel, revocation, disconnect or
expiry stops emission; cancellation uses the next bounded poll/control exchange.
Reconnect obtains a fresh grant and validates continuation/cohort/export-view
again. Rewrite or changed policy/recipe refuses continuation without splicing.

**Decision — fail explicitly:** offline/unreachable/timeout returns unavailable
with closed reason/retryability, never empty success or a T3/Hub/mirror fallback.
Expire pending requests and discard late responses; do not persist a body retry
spool. Use `no-store`, opaque URLs and no body/access logging as federation requires.

Proposed independently reviewable PRs, each carrying `Part of #594`:

1. Shared wire types, assertion signing/verification and origin audit store;
   settle #593 enrollment, credential and TLS interfaces first.
2. Receiver poll/response endpoints, bounded queues and ACL/audit/admission/cache
   wiring; add `receiver_audit::authorize(AttemptId, binding)` to append/fsync the
   binding within the attempt's reserved space before admission/release. No body
   release until the origin client path is present.
3. Origin client loop and resolve/page adapter for typed `view`, including safe
   mapping or explicit unavailable evidence and end-to-end D2/cache-hit checks.
4. Append leases, acknowledgements, cancel/backpressure and fresh reconnect grants.
5. CI acceptance/refusal coverage: unsupported/extra verbs, replay, wrong audience,
   foreign namespace/epoch, stale generations, malformed frames, audit failure,
   redaction mappings and cancellation; control/runtime/SSH fail before dispatch.
   Relevant refusal tests also accompany each earlier slice.

Channel acceptance mapping (federation remains authoritative); slice 5 integrates
these cases, with owning-slice tests accompanying implementation:

| Federation acceptance case | Owning slices |
| --- | --- |
| Traversal/symlink races; stale generations | 3 |
| Replay, foreign namespace, wrong audience | 1, 3 |
| Unsupported verbs, malformed frames; control/runtime/SSH refused before dispatch | 1, 2, 3 |
| Zero watchers, credential revocation, backpressure stop within bounds | 2, 4 |
| Cross-user discovery/cache/field refusal | 2, 3 |
| Final policy-to-audit binding, including cache hits | 1, 2, 3 |
| Stream-through without disk writes; key failure fails closed | 2, 3 |
| Revocation/rewrite invalidates cached admission and continuation | 2, 3, 4 |
| Length-preserving and length-changing redaction fixtures; exact anchors or refusal | 3 |
| CI latency/memory/cancellation measurements; shared-listener exhaustion | 5 (2–4 supply bounds) |

## Open questions for origin lane / PM / owner

1. Can #593 slice 2 own the single enrollment schema and kind-aware credential
   custody above, with slice 5 sharing listener/TLS/reconnect primitives? What exact
   interfaces/ref should #594 target?
2. Will the shared listener support client-certificate authentication for body
   routes? Recommend mTLS; bearer-only requires an explicit contract amendment by
   PM/owner, not an implementation assumption.
3. Does enrollment accept the receiver as assertion issuer, or require an
   independent issuer? Who provisions/pins/rotates issuer and client certificate
   keys, and how is revocation exposed to both lanes?
4. What poll/queue/concurrent-origin/lease caps and total read deadline fit #593's
   listener budget while reserving response/publication capacity? Confirm the
   proposed 20-second poll and 16 KiB control cap.
5. Which existing origin witness/recipe API can validate an authorization-only
   cached export and produce exact mappings? Agree the initial supported fields/
   anchors and explicit unavailable cases; current `view` alone is insufficient.
6. Who owns the adapter from typed page/field selectors to receiver audit's numeric
   exported ranges, and persistence of the origin audit's export-view binding?
   Neither a page token nor transformed offset is a native-byte proof.
