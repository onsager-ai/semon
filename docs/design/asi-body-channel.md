# Online body channel (#594)

Status: reconciled with the current #593 enrollment contract on main (#634/#639).
[ASI federation](asi-federation.md) remains binding, including D2/D3 and the
#630 cache/#631 ACL/audit boundaries. [Enrollment](asi-enrollment.md) owns shared
infrastructure; #568 owns receiver Viewer integration. Agreement is not runtime delivery.

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

**Agreed boundary — #593 slice 4a, in `crates/semon-push`:** the current enrollment
contract owns provisioning, custody, automatic renewal, revocation, TLS and listener
dispatch. The [four-action ceremony](asi-enrollment.md#accepted-four-action-ceremony)
generates enrollment, credentials, issuer keys, certificates, grants and audit
directories; see [custody outputs](asi-enrollment.md#origin-configuration-and-custody).
The [state machine](asi-enrollment.md#pairing-bootstrap-and-provisioning-state)
progresses pending → confirmed → approved → active; [P1 source consent](asi-enrollment.md#source-consent-and-p1)
is persisted separately. #594 adds no second enrollment or operator step and consumes
the single logical interface shared with publication:

```text
authenticated_enrollment(conn, request_credential, required_kind)
  -> { enrollment_id, machine, namespace, origin_incarnation,
       index_incarnation, publisher_claim_epoch, kinds, verified_client_identity }
```

`body_enrollment::RevocationWatcher` is the library-only revision notification
hook. The host creates it before accepting work and calls
`notify_if_revision_changed(source, broker)` when checking registry changes. It
acknowledges a revision only after broker cleanup succeeds; registry read failures
also invoke cleanup and return unavailable. Registry-backed work validates its own
captured authority and fails closed on registry errors. For append leases the host additionally
calls `AppendService::sweep_owner(original_scope, owner_source)` for each active
owner; the watcher does not own append services or schedule background work.

`RegistryEnrollmentSource::authenticate` repeats the registry decision for each
request using verified connection evidence and its bearer credential, always
passing `Kind::BodyChannel`; no request body selects the kind. The existing
`TryFrom<&AuthenticatedEnrollment>` supplies the complete scope. Pending broker
work and append leases retain the authentication's `RevisionHandle` independently
of subsequent requests. Registry read errors fail closed.

The current coarse check (option A) invalidates all captured authority on any
registry revision change. Enrolling another machine, revoking authority, and every
credential renewal drop all pending body work and append leases for every origin
with `Revoked`. Origins must reconnect with a fresh grant. Even unchanged owners
incur this churn. Option B, a credential-bound authority handle exposed by the
origin lane in its planned renewal/revocation PR4, is the planned replacement;
it will allow unaffected owners to retain their work.

Body-route mTLS is settled, not an amendment. Slice 4a builds client-certificate
support; #594 consumes this authenticated scope and never touches TLS configuration.
See [enrollment](asi-enrollment.md#credential-kinds-and-custody) for kind separation,
including the current analytics client-certificate exception. A transport credential
never supplies per-user body authority. The old `tokens` file is refused with a
re-issue message, never deleted; the owning contract defines replacement custody.

Use the single [capacity table](asi-enrollment.md#capacity-and-admission), including
its separate capped poll class and closed-reason 503 refusals instead of excess
queuing. Those caps are agreed interim limits. #594 confirms 20-second polls and
16 KiB control envelopes, owns read/
lease deadlines, and consumes 4a admission/revocation hooks. Non-poll reservation
split remains open. Body frames never enter analytics objects or reconnect spools.

### Receiver route registration (Part of #594)

The existing receiver HTTP/1.1 listener registers only `POST /v1/body/poll`
and `POST /v1/body/requests/<opaque-request-id>/response`. Every exchange uses
`RegistryEnrollmentSource` with the body kind and verified enrolled mTLS identity,
including a fresh registry decision before release. Missing body configuration or
client trust refuses uniformly. There is no redirect or alternate endpoint.
`Expect: 100-continue` is refused for these bounded requests.

`BodyHost` owns the broker, revision watcher and existing ceremony-created cache,
issuer, ACL and audit custody. It opens existing state only when a body kind and
client trust are present. Hosts with active append services retain their original
owner sources and call `BodyHost::sweep_owner` at authorization/release decisions;
transport requests create no append services or leases. Revision checks run before
body authentication too, so a revoked caller still triggers pending-work cleanup.
No background scheduler or second operator ceremony is introduced.

Polls use total listener connections plus the separate broker poll quota: one per
enrollment, globally 50% of configured connections (rounded down). Admission is
reserved immediately after authentication and route/framing selection, before
reading the control body; an RAII guard releases it on every return/disconnect
path and retains it through the response write. Each poll
returns at most one strict versioned control request or a versioned empty result
within the 20-second read/wait budget. Responses use non-poll total slots and a
20-second frame-read deadline; the 256 KiB Content-Length ceiling is checked before
reading, then the u32/header and 16 KiB header bound before raw payload buffering.
The broker enforces the cumulative response envelope, four pending requests per
enrollment, binding/assertion/scope checks and frame ordering. Capacity refusals
use closed reasons and HTTP 503 with connection close. Unknown, foreign and expired
request IDs receive the same authority refusal. No body content enters access logs.
A failed transport write fails the selected request with `unavailable` and wakes its
requester. This listener has no disconnect notification during a condvar wait:
an abandoned poll can retain its slot until work arrives or the 20-second deadline.
That bounded thread-per-connection limitation remains for #636.

Route tests exercise handlers with injected verified identity and deterministic
broker triggers; TLS handshake plus `Server::run` end-to-end coverage is still
pending. These tests do not establish a reachable production body workflow.

This registration remains library/host integration: provisioning still refuses
body-channel activation until the admitted-binding path lands. Production handle
resolution and receiver Viewer delivery (#568) remain separate dependencies; this
change exposes no reachable end-to-end body-reading workflow.

## Assertions and exact envelopes

**Decision — receiver-issued Ed25519 assertions:** receiver-as-issuer is the
default, explicitly trusting its operator to impersonate users. Independent issuer
or local-only mode remains available when that trust is unacceptable. See
[enrollment custody](asi-enrollment.md#origin-configuration-and-custody) and
[renewal/revocation](asi-enrollment.md#rotation-and-revocation) for generated issuer
pins and bounded rotation. 4a owns that state/custody; #594 owns encoding/sign/verify,
implemented as `body_assertion` in merged PR #635 (`db8acabca`).

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
limits and the required closed `delivery_mode` (`cached` or `stream_through`).
Missing, malformed or unknown modes refuse; no wire default exists. The mode also
binds the origin audit export-view digest. Verify completeness, bindings and digest before admission; mismatch publishes
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

**Origin audit — delivered in #635:** consume the ceremony-generated separate
0700 directory outside native roots/cache; no fixed basename. Files are 0600
`audit.lock` and `authorizations.jsonl`. Closed metadata binds request, principal,
source/generation/range, purpose/decision/destination, versions and opaque enrollment/
export-view IDs. Fsync events/creation; exclusive writer, ≤64 MiB, reserved completion
and ≤16 receipts. No bodies, queries, paths, credentials or free-text errors.
Torn/full/unwritable audits refuse; no automatic repair, pruning or deletion.

**Receiver:** caller-authenticated principal (login outside this note) → ACL before
discovery, uniform denials → `begin_attempt` → origin round trip → verify response →
`authorize(AttemptId, binding)` append/fsync → non-Clone exact-key `ReadAdmission` →
cache lookup/delivery → `receipt`/`complete`. PR #635 adds the authorization event
within reserved attempt space: matching request ID, exact response identity, verified
origin policy/redactor versions and origin authorization audit ID. Cache hits require
the same binding. Recheck current ACL/policy before release; changes need fresh
admission. Audit failure refuses release; later receipt failure stops emission and
leaves an attempted read, not false success.

**Decision — origin controls custody (#595):** origin injects the response mode
into `OriginRead`; #592 owns source consent persistence. The receiver's
`ReadInput::stream_through` can only tighten it. Stream-through uses request-owned
bounded memory, bypasses cache lookup/insertion/discard and all cache metadata or
staging writes, and has no retry spool. `PendingDelivery::no_store()` requires the
future HTTP route to send `Cache-Control: no-store`. Buffers and typed text are
zeroized on drop. Release checks captured enrollment authority, ACL and deadline
before exposing any body; a failed release completes the receiver audit with zero
bytes. Origin stream-through renders fresh bytes even for authorization-only
candidate requests. Restricted-source opt-in remains origin-owned and owner-only;
named-user opt-in is deferred with multi-user support. Append pages remain
stream-through and share the same mode binding.

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

### Chain, cohort and snapshot identity

Stable archive `chain_id` is source chain identity. `SourceWitness.prefix_sha256`
is the continuity witness for `[0, consumed_boundary)`, separate from identity.
The cohort ID is a domain-separated SHA-256 of sorted opaque source handles only:
it identifies membership and stays stable across appends. An exact snapshot is
(cohort ID, activity generation, transcript generation), together with every
member's chain ID, prefix digest and consumed boundary. The origin-index evidence
reader must supply chain IDs and both generations against the same pinned cohort
in one qualified read; missing evidence refuses uniformly.

An append lease may survive newer generations only on a strict append extension:
the chain ID is unchanged and the prefix digest at the acknowledged offset is
unchanged. Rewrite, truncation or membership change closes the lease with
`continuation_invalid`, as #653 specifies. The storage lane owns that lease
continuity integration. Cache keys include the prefix digest and consumed boundary
and invalidate conservatively on append.

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

1. **Delivered — PR #635 (`db8acabca`):** `body_assertion`, origin audit store and
   receiver `authorize` primitive. Later slices target the current enrollment
   interfaces; #634 is merged, but working 4a hooks remain an integration dependency.
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
