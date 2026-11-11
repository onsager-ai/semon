# ASI enrollment and shared listener

Status: **proposed current contract**, 2026-11-11, for implementation by
[#593](https://github.com/onsager-ai/semon/issues/593) slice 4a. This is the authored
home for enrollment, credential kinds and shared listener hooks. Storage lane
[#594](https://github.com/onsager-ai/semon/issues/594) targets this contract;
its [body-channel proposal in PR #632](https://github.com/onsager-ai/semon/pull/632)
remains a separate proposal. Neither document proves runtime delivery.

[ASI federation](asi-federation.md) owns the accepted custody/security boundary.
Mutual TLS for body routes is accepted. Receiver-as-assertion-issuer is the accepted
default with the explicit trust acknowledgement below. Hub never participates in
this enrollment, the body channel, issuer custody or body authorization.

## Existing implementation and delivery boundary

At base `25fe18de0`, [`serve.rs`](../../crates/semon-push/src/serve.rs) implements
a blocking HTTP/1.1 thread-per-connection receiver with socket deadlines, operator
server TLS, and default total/unauthenticated-per-IP connection limits. Its rustls
configuration uses `with_no_client_auth()`. The dispatcher recognizes mirror
append, facts and status only, authenticating each request before reading its body.

[`tokens.rs`](../../crates/semon-push/src/tokens.rs) currently stores
`NAME SHA256(token)` lines in `DIR/tokens`, with private file/directory modes;
`Tokens::authorize` supplies a machine identity. The
[mirror protocol](../mirror-protocol.md), [`mirror.rs`](../../crates/semon-push/src/mirror.rs)
and [push client](../../crates/semon-push/src/lib.rs) define today's supported Push
surface. They do not implement enrollment, credential kinds, client-certificate
identity, publisher claims or a remote body read.

| Slice / owner | Delivered scope when implemented |
| --- | --- |
| #593 slice 4a | Enrollment registry, kind-aware credential custody, TLS/route/admission hooks and enrollment/token CLI changes defined here |
| #593 slice 4b | Publisher claim activation/epochs, expected-parent commits and namespace heads |
| #593 slice 3 | Immutable publication objects and validation, independent of 4a |
| #593 slice 5 | Analytics routes, publication client, reconnect and transport integration |
| #594 | Body routes/client, assertions, online origin authorization/audit, frames and leases |
| #596 | Explicit replica enrollment and namespace authority transfer |

Slice 4a supplies usable identity and listener hooks, not functioning analytics
publication or remote viewing. Slice 4a introduces no async runtime, HTTP/2,
WebSocket or additional listener. The blocking thread-per-connection listener is
a temporary constraint: an event-driven receiver listener is planned in #636
(after slice 4a and #594's first transport slice; spec before implementation).
Origins initiate HTTPS connections to the existing receiver; they expose no
inbound port. The HTTP/2 example in federation is not a requirement to replace
HTTP/1.1 in this slice.

## Receiver enrollment record

An enrollment is an operator-authorized binding, not a connection or a token.
The proposed versioned registry has one current schema, rejects unknown fields,
unsupported versions, duplicate identities and invalid bindings, and publishes
updates atomically and durably. Invalid/unreadable custody refuses authorization;
it must not appear to be a valid empty registry.

| Field | Contract |
| --- | --- |
| `enrollment_id` | Opaque stable identifier allocated by the receiver; never inferred from a hostname, source filename or socket address |
| `receiver_id` | Explicit receiver identity bound to the exported enrollment bundle |
| `machine` | Registered machine identity, using the existing machine-name grammar |
| `namespace` | Explicit namespace under that machine's authority; use the current namespace grammar, not a path |
| `origin_incarnation`, `index_incarnation` | Origin instance and admitted index incarnation; retain both if their lifecycles differ; no incarnation inferred from contact time |
| `publisher_claim_epoch` | Assigned claim epoch, or explicitly unassigned before 4b binds a claim; never substitute zero as active authority |
| `client_certificate_pins` | Explicit SHA-256 SPKI or leaf-certificate fingerprints, tagged by pin type; a verified certificate must match an enrolled pin |
| `credential_kinds` | Enabled kinds and their per-kind `client_cert_required` setting; a kind is not inferred from the route or another credential |
| `revoked_at` | Whole-enrollment revocation timestamp, absent only while not revoked |
| Per-kind `revoked_at` | Independent timestamped revocation for each kind; whole-enrollment revocation overrides every kind |
| `issuer` | Explicit `receiver` or `external`, with an operator acknowledgement of the selected trust boundary |
| `assertion_issuer_keys` | One current issuer key ID and pinned public key; at most two accepted key-ID/public-key pairs during explicit rotation overlap |

The receiver-side record binds these fields to its configured HTTPS identity.
The origin-side configuration below carries the endpoint and server trust root;
they are not accepted from a poll, publication or body request. A request cannot
change any registry field. Native UID, lineage and provenance semantics remain
those of the admitted native sources.

For analytics/body kinds the full origin/namespace binding is required. A
mirror-only record still binds enrollment ID, receiver and machine; claim epoch,
origin/index incarnation and assertion issuer are explicitly not applicable until
those kinds are enrolled. Mirror machine custody does not fabricate a publisher
claim or grant replica authority.

An enrollment or kind with a revocation timestamp is never silently reactivated.
Whole-enrollment replacement requires explicit new enrollment and a new ID.
Re-enabling a revoked kind requires an explicit operator action and newly issued
credentials, retaining the revoked generation as revoked. Re-importing an older
bundle or reconnecting cannot clear revocation. No revocation operation deletes
native sources, imported facts, mirror archives or audit history.

## Origin configuration and out-of-band enrollment

The proposed origin configuration contains:

- Enrollment ID, registered machine/namespace, origin/index incarnation and the
  assigned-or-unassigned claim epoch.
- Receiver ID, exact HTTPS endpoint and explicit server trust root. HTTPS hostname
  verification remains mandatory; the identity label alone is not TLS proof.
- Enabled credential kinds, per-kind certificate requirements, and protected
  credential references rather than secrets embedded in ordinary configuration.
- Explicit `issuer: receiver|external` acknowledgement, issuer key IDs and pinned
  public keys, with no more than two accepted issuer keys during rotation.
- Origin client certificate/chain and private-key paths, and the enrolled
  certificate fingerprint(s). The origin retains custody of its private key.

Proposed provisioning flow (CLI spelling beyond `semon receive enroll` is owned by
slice 4a and must be documented with its implementation):

1. The operator supplies receiver identity/endpoint/trust root, origin binding,
   enabled kinds, issuer trust choice and the origin's certificate/public identity
   to `semon receive enroll`. This is a local administrative action, not a public
   enrollment route. The origin generates/holds its client private key separately.
2. The receiver records the binding and emits a versioned enrollment bundle with
   those public identities, trust roots/pins, kind settings and issuer keys. Any
   newly issued bearer secret is a separate one-time protected delivery; the
   registry keeps only its hash. The bundle contains no private key.
3. The operator transfers the bundle out of band and checks receiver identity,
   endpoint, trust root, certificate pin, namespace/incarnations and issuer choice.
   A fingerprint learned from the request being authenticated is never trusted.
4. An explicit import on the origin validates the current bundle and installs the
   origin configuration, protected credential reference and local key paths. It
   displays/requires acknowledgement of receiver-as-issuer trust when selected.
5. Only after explicit import may the origin connect. The receiver rechecks the
   current registry; a bundle is provisioning input, not authority to undo a later
   revocation or activate a claim.

Choosing `issuer: receiver` explicitly trusts the receiver operator to impersonate
end users in assertions. This is the accepted default, not end-to-end protection
from that operator. Choose an independently pinned external issuer or local-only
mode when that trust is unacceptable. External issuer enrollment does not relax
origin policy or online audit. No request installs an issuer or adds an issuer key.

## Credential kinds and custody

Slice 4a replaces the single-kind machine token file with **one current versioned
kind-aware credential store**. Each bearer record binds a credential ID, enrollment
ID, exactly one kind, token hash, and timestamped revocation state. Multiple kinds
require separately issued credentials. Raw bearer secrets are shown/delivered once;
they are never persisted by the receiver, logged, or included in ordinary bundles.

| Kind | Permitted receiver routes | `client_cert_required` |
| --- | --- | --- |
| `analytics_publish` | `/v1/analytics/` publication routes, once slice 5 implements them | Explicit per-kind setting; default `false` (owner decision 2026-11-11, revisited when #594 lands); operator may require it |
| `body_channel` | `/v1/body/` origin poll and response routes, once #594 implements them | Always `true`; cannot be disabled while this kind is enabled |
| `mirror_push` | Existing `/v1/mirror/append`, `/facts`, `/status` routes | Explicit per-kind setting; default `false` for supported mirror Push |

These credentials authenticate origin machines, not end users. A body-channel
credential permits transport only; it grants no `body_read`, metadata discovery,
cache admission or source-policy permission. Mirror tokens cannot poll or publish
analytics. Analytics tokens cannot use body routes. Certificate-only possession
also grants no kind: both the kind credential and any required certificate check
must pass against the same enrollment.

The store uses owned 0600 regular files in a dedicated owned 0700 directory,
separate from bodies, publication objects/spools, caches and their backups.
Protect opens/updates against symlinks and accidental cross-store replacement.
Origin bearer files and client private keys have the same private custody modes;
issuer signing private keys stay in receiver-controlled protected custody when
`issuer: receiver`, or with the external issuer otherwise. Origin issuer pins,
receiver trust roots and client-key paths are integrity-protected configuration.
Neither enrollment nor a cache backup includes private keys or bearer secrets.

This deliberately changes the supported `semon receive token` and push-client
surface under the [pre-release policy](../../AGENTS.md#pre-release-compatibility-policy).
There is no compatibility reader, migration or dual store for `NAME SHA256(token)`:
a receiver that finds the old `DIR/tokens` file refuses to start with a re-issue
message and leaves the file untouched.
Operators must re-issue kind-scoped credentials, explicitly enroll/configure the
participating machines, and update Push clients and token files together. Slice
4a must update the owning CLI, mirror documentation and tests in its implementation
PR. Refusing unsupported state does not authorize deleting it or any user history.

### Rotation and revocation

Issuer rotation is explicit: provision/import the next pinned key out of band,
accept the old and new key IDs during a bounded operator-chosen overlap, switch
signing, then explicitly remove the old pin. There are at most two accepted issuer
keys, never an unbounded key list or a request-fetched key. Assertion validity,
nonce replay bounds and signing encoding belong to #594; overlap cannot extend an
assertion's expiry or revive a revoked enrollment.

Client-certificate rotation likewise requires an explicit receiver pin update and
origin configuration update; any overlap is explicitly bounded and removed by the
operator. Tag SPKI versus leaf pins so their renewal behavior is unambiguous.
Bearer replacement issues a fresh kind-scoped secret and explicitly revokes the
old credential; it cannot clear an enrollment/kind revocation timestamp.

The receiver checks current whole-enrollment, kind and credential revocation on
every request, including requests on kept-alive connections. A successful TLS
handshake is not a cached authorization. Active poll/response integrations receive
a revocation signal and must recheck before dispatch/release; #594 owns stopping
in-flight reads within its declared deadlines. Revocation stops new authority
immediately at the next receiver decision, without waiting for certificate expiry.

The origin independently removes local receiver/issuer pins or disables enrollment
to stop trusting that receiver. A receiver-side revocation cannot remove an origin
pin remotely. If the origin is disconnected, propagation waits for contact; no
claim of instantaneous remote revocation is made. Local pin removal refuses new
assertions/connections and cancels use of the affected channel. Already delivered
plaintext cannot be recalled.

## Shared listener authentication and dispatch

Extend the existing rustls listener with **optional client-certificate presentation
and verification**: mirror origins without certificates can still complete TLS.
Presented certificates must validate against configured client trust and match
the enrolled SPKI/leaf pin. The TLS layer preserves verified peer identity;
the route layer enforces `client_cert_required` separately for each kind. No body
route accepts a missing, unverified, foreign or revoked client identity.

Proposed logical interface (not an existing Rust API):

```text
authenticated_enrollment(conn, request_credential, required_kind)
  -> { enrollment_id, machine, namespace, origin_incarnation,
       index_incarnation, publisher_claim_epoch, kinds, verified_client_identity }
```

The connection contributes verified TLS evidence; the request supplies its current
kind credential. The returned scope comes only from the current registry. Every
request repeats this decision; a connection cannot retain a wider kind set after
revocation. A route/body field cannot select a different enrollment or namespace.

Dispatch order is bounded request-head/framing validation, route/kind selection,
current credential/enrollment/certificate verification, resource admission and
size validation, then handler/body consumption. No `100 Continue`, upload staging,
body resolution or queue insertion precedes the authority and capacity checks.
Unknown routes/kinds refuse; handlers cannot opt out. Analytics and body routes
require HTTPS even where the listener also supports loopback mirror HTTP.

`/v1/analytics/` is the receiver namespace for the shared publication mechanism;
#555's Hub `/v1/asi/` paths are not receiver routes. `/v1/body/` carries #594's
polls and bounded responses only. Existing mirror routes retain their own kind
and custody. Body frames never enter analytics objects or the reconnect spool.

### Capacity and admission

All classes charge the same operator-configurable `max_connections`. Polls hold
threads/slots on this blocking listener. Admission accounts for class and enrollment
in addition to the existing unauthenticated per-IP protection; authenticated NAT
peers are not all treated as one machine. Idle keep-alive connections continue to
consume total capacity and remain deadline-bound.

This is the single capacity table. Values marked proposed require confirmation;
existing mirror limits remain owned by `serve.rs` and the mirror protocol.

| Resource / class | Limit and status |
| --- | --- |
| Total connection slots | Existing default 32; slice 4a exposes operator configuration |
| Unauthenticated connections per IP / IPv6 /64 | Existing default 4; retain pre-authentication enforcement |
| Body poll class | Interim limit: at most 1 outstanding poll per enrollment and `floor(max_connections × poll_share)` globally; configurable share, default 50% |
| Publication/upload class | Charged to non-poll slots and total; keep capacity available outside the poll quota; object/body bounds owned by slice 5 |
| Body response-frame class | Charged to non-poll slots and total; must not wait behind long-polls; #594 owns framing and read deadlines |
| Mirror class | Charged to non-poll slots and total; existing 6 MiB request cap and socket deadlines |
| Pending body requests | Interim limit: at most 4 queued requests per enrollment, metadata/control only, expiring under #594 deadlines |
| Poll duration / control envelope | Proposed: 20 s / 16 KiB; #594 confirms these values |
| Complete body page/field envelope | Accepted D3 ceiling 256 KiB including metadata; #594 owns frame accounting, lease/read deadlines and narrower limits |

These caps are the **interim** limits of the thread-per-connection listener, set
by the owner on 2026-11-11 until the event-driven listener of #636 replaces it.
At the default 32 slots and 50% poll share the current listener can hold about 16
concurrent long-polls, with the remaining slots shared by publication, responses
and mirror work. That follows from the threading model; it is not a product
ceiling or a supported-fleet target. Raising the setting costs threads, file
descriptors and memory and needs CI resource measurements. Scale beyond it is the
purpose of #636, not of this slice.

A poll quota is a ceiling, not permission to consume response/publication slots.
Publication/uploads, response frames and mirror operations have separate accounting
classes so admission/fairness can be tested; their precise reservation split is an
open item. Never block all responses behind polls or create an unbounded work queue.

Excess admitted HTTP requests return explicit `503` unavailable with a closed
reason (`connection_capacity`, `poll_capacity` or `pending_queue_full`) and close
the connection. Pre-HTTP/failed-TLS connections may be closed without an HTTP
response; do not allocate an extra worker merely to send 503. Queue expiry and
revocation discard pending work with explicit unavailable outcomes, never empty
successful bodies. #594 owns operation correlation, cancellation and retryability.

## Claims, epochs and heads

Enrollment binds the authenticated machine, namespace and origin/index incarnation
to its publisher claim. It is the authority input, not the claim activation
transaction. Slice 4b consumes that binding and follows the
[shared #555 mechanism](asi-store.md#hub-export-phase-3): pending publisher/epoch,
complete validated replacement, expected-parent commit and atomic visibility
switch. The prior complete head remains visible until activation; a delayed or
revoked publisher cannot overwrite a newer epoch.

Slice 4a represents an unassigned claim explicitly and cannot advance heads.
Once assigned, the epoch returned to either lane is the current registry/claim
binding; client-provided epochs must match it. Body requests bind the same scope
but do not activate claims. An incarnation change requires explicit rebinding and
4b reconciliation, not silent enrollment reuse or an empty-head reset. Replica
custody/authority transfer remains #596; this contract grants neither automatically.

## Interfaces #594 targets

- The versioned enrollment scope and issuer pins above, with explicit receiver
  trust acknowledgement and current revocation state.
- Request authentication by `body_channel` plus verified enrolled client identity,
  and a fresh authority check before dispatch/release.
- One shared HTTPS HTTP/1.1 listener, body route registration and class-aware slot
  admission; a bounded per-enrollment pending queue with expiry/revocation hooks.
- Current issuer key access for the signing component without copying private keys
  into registry records, bundles, bodies or cache; issuer pins imported at origin.

#594 supplies assertion encoding/verification, poll/response payloads, channel
binding, origin audit, per-read policy, cache admission, deadlines and leases.
Enrollment never substitutes for authenticated user grants or D2's online origin
authorization/audit, including cache hits. PR #632 should reference this authored
contract when reconciled; its proposed schema/API names are not shipped interfaces.

## Decisions and open items

Settled by the owner on 2026-11-11:

- **Capacity.** The caps above are the interim limits (1 poll per enrollment, poll
  share at most 50%, 4 pending requests per enrollment, `503` instead of queuing).
  An event-driven receiver listener is planned and tracked in #636; thread-per-
  connection is temporary and ~16 poll-connected origins is not a product ceiling.
- **Token store.** The old `DIR/tokens` file is refused with a re-issue message.
  It is never deleted, migrated or imported (see custody above).
- **`analytics_publish.client_cert_required`** defaults to `false` for now and is
  revisited when #594 lands. Body mTLS is already settled and not open.

Still open for implementation owners:

- #594 confirms poll/control bounds and supplies lease/read/cancellation deadlines;
  slice 4a must expose the required deadline/revocation hooks before integration.
  The non-poll reservation/fairness split is chosen with the 4a resource tests.
- Exact CLI flags/import spelling, on-disk schema names and certificate
  provisioning/rotation tooling are chosen in slice 4a; document and test those
  together. Overlap duration is explicit operator policy, not a hidden default.
  Receiver-as-issuer and Hub exclusion are already settled.

CI acceptance for 4a must exercise cross-kind refusal, wrong/missing certificate,
wrong namespace/incarnation/epoch, revoked credentials on kept-alive connections,
unsupported/insecure store refusal, explicit import/pin rotation, and concurrent
poll/upload/response/mirror exhaustion without unbounded buffering. Transport/body
end-to-end guarantees remain the owning slices' gates, not this docs PR's evidence.
