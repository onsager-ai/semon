# ASI enrollment and shared listener

Status: **current enrollment/provisioning design contract**, reconciled 2026-11-12
with the [accepted operator journey](asi-operator-journey.md) and owner decisions.
Pairing, mirror-only origin connect and client-certificate issuance are implemented
at base `f167df6dbeaeb32e88cace13f48f0b843b4598bf`. PR4 renewal, revoke/enable
and disconnect below are normative requirements, not shipped behavior. Their
implementation and independent technical safety review remain pending. PM accepted
the guarded replacement ceremony, authenticated renewal-due header and last-kind
revoke refusal, and the live-state compaction model below. Operator wording remains
subject to the separate UI/UX Manager review. This document does not lift PM-owned implementation holds
or establish runtime delivery. It is the authored home for #593 slice 4a;
[#594](https://github.com/onsager-ai/semon/issues/594) consumes the same enrollment.
Its [body-channel proposal in PR #632](https://github.com/onsager-ai/semon/pull/632)
owns the separate read transport contract.

[ASI federation](asi-federation.md) owns D1–D8 and custody/security boundaries.
Body-route mTLS and receiver-as-assertion-issuer are accepted, with explicit origin
trust approval. Hub never participates in enrollment, body transport, issuer
custody or body authorization. Machine credentials are never user identity or
source authority.

## Existing implementation and delivery boundary

The blocking HTTP/1.1 receiver in [`serve.rs`](../../crates/semon-push/src/serve.rs)
retains socket deadlines, operator server TLS and total/unauthenticated-per-IP
limits. Slice 4a PR2 (#646) adds route classification before body reads, optional
verified TLS client certificates, current registry authorization per request,
keep-alive enrollment binding and credential handles for in-flight reauthorization.
`serve_auth.rs` exposes `RegistryAuthenticator`, `AuthenticatedEnrollment` and
`CredentialHandle`. The bounded registry reader and credential handle inspect registry
custody only; missing pairing-gate custody does not disable existing authentication.
Private owned mode-0600 `DIR/client-trust.pem` supplies PEM client roots; absence
disables client-certificate presentation. Analytics routes remain reserved. #661
registers the closed body poll/response routes when existing body custody and
client trust are configured; body enrollment and a reachable end-to-end workflow
remain blocked on admitted-binding and owning integrations. Its fresh registry
release checks use the shipped credential-bound `CredentialHandle` from PR4.1.
PR4.2 renewal routes remain separate from body dispatch
and pairing-ceremony state. See [receiver route registration](asi-body-channel.md#receiver-route-registration-part-of-594).
The listener uses kind-scoped enrollment-registry authentication; the former
flat-file token reader and commands are removed. Legacy `DIR/tokens` custody
refuses startup/routes without import or deletion. The [mirror protocol](../mirror-protocol.md)
and Push client retain supported mirror transport with enrolled mirror_push bearers.
#630/#631 and #635 supply libraries, not integrated body service.

The Unix enrollment/credential libraries now feed the receiver provisioning
controller and `receive enroll` terminal. Receiver-side public_tls provisioning
creates mirror-only enrollments; origin connect/client and optional client certificates
are integrated. Independently validated analytics/body admission, renewal and
revocation CLI remain follow-up integrations. Shipped custody creates
`DIR/enrollment/registry.json` (strict current format 3)
and `DIR/enrollment/registry.lock`, relative to receiver state directory `DIR`.
One atomic registry document contains enrollment bindings, generation-scoped
certificate pins and kind-scoped bearer hashes; raw bearer secrets are never
serialized. The library also exposes legacy `DIR/tokens` refusal without importing
or modifying it. The listener now authenticates only through the current registry;
legacy `DIR/tokens` custody refuses startup and every request with explicit
re-pair/re-issue guidance, preserving the file and all retained history. A connection
is bound to one enrollment and cannot switch machines between requests. The old
flat-token issue/revoke/list commands and reader are retired. The public_tls core
in `pairing.rs` is wired through strict provisioning routes and local owner CLI;
receiver identity/assertion-issuer custody comes from ReceiverCustody.
`Core::registry()` exposes a fresh registry with
all fields for the later `origin_read::EnrollmentScope` adapter. Revocation comes
from a separate CLI process editing the registry and is observed through its
revision; pairing holds `pairing.lock`, acquiring `registry.lock` only for short
transactions so that external updates remain possible. Registry format 2 includes live nonsecret pairing
activation receipts in the same commit as enrollment and credential hashes. The
activation input's commitment binds the exact machine, namespace/incarnation or
mirror-only binding, kind settings, certificate pins and validity to B4. Secrets
are independently domain-derived per kind from the confirmed application key;
the registry stores hashes only. A receiver crash before commit leaves no enrollment;
a crash after commit before delivery uses the confirmed origin key/session and
application key to recover the same outcome. Each return durably reserves a delivery
before exposing secrets: one initial delivery plus one interrupted-delivery recovery,
then explicit revocation is required if delivery remains uncertain. Recovery checks
current revocation and cannot issue a second enrollment or revive old authority.
Cross-machine persistence remains non-atomic; origin activation waits for matching
durable local trust/consent and receiver commit. Updates compare a
persisted monotonic revision under the registry lock; replaced certificate pins
remain as terminal revocation history. PR4.2 implements the per-generation
revocation and explicit owner-authorized re-enable contract below. Existing
binding/issuer fields remain frozen; narrower issuer-rotation and mirror-to-
analytics enablement transition APIs arrive later. Validating admitted namespace/
incarnation continuity remains the provisioning caller's responsibility.

| Slice / owner | Delivery boundary |
| --- | --- |
| #593 slice 4a, this contract | Reviewed pairing/bootstrap, enrollment registry, generated identity and kind credentials, persisted source-consent input, automatic renewal, revocation, TLS/route/admission hooks, provisioning outputs and owning CLI/token changes |
| Receiver integration | Consume cache key/directory, authenticated owner principal and exact ACL/self-grant inputs, separate receiver audit directory; integrate owner Viewer authentication and custody checks |
| Storage lane #595 (#630/#631) | Own `body_cache`, `receiver_acl` and `receiver_audit`; 4a produces their inputs, not replacement modules |
| Storage lane #635 | Own `origin_audit`, receiver audit authorization binding and `body_assertion`; 4a produces custody/issuer/scope inputs, not assertion encoding or audit implementations |
| Governance/origin follow-up to #633 | Generate P1 policy and complete contributor registry from admitted sources and real scan/version evidence; scanning is not delivered by #633 or 4a |
| Store lifecycle amendment | Authorize/integrate producer startup by `connect`; currently `analytics materialize` is the only enablement surface |
| #593 slice 4b | Publisher claim activation/epochs, expected-parent commits and namespace heads |
| #593 slice 3 | Immutable publication objects and validation, independent of 4a |
| #593 slice 5 | Analytics routes, publication client, reconnect and transport integration |
| #594 | Body routes/client, assertion encoding/verification, origin rendering and exact evidence mapping, online authorization, dual-audit/cache admission, frames, cancellation and leases |
| #596 | Explicit replica enrollment and namespace authority transfer |

Slice 4a supplies provisioning and identity/listener hooks; working analytics
publication and remote viewing require the downstream integrations. It introduces
no async runtime, HTTP/2, WebSocket or additional general-purpose listener.
Thread-per-connection is interim: #636 plans an event-driven receiver after 4a and
#594's first transport slice, with specification before implementation. Origins
initiate connections and expose no inbound port. Federation's HTTP/2 example
does not require replacing HTTP/1.1 in this slice.

## Accepted four-action ceremony

The mirror-only command path is shipped; the full analytics/body journey and owner
Viewer integration remain pending. After installation,
the local-owner target is exactly four actions B1–B4. Reachability, host custody,
service installation and remote-owner login are separate prerequisites/actions as
counted by the journey; a conditional four-action target is not a clean-host claim.

| Action | Operation | Provisioning effect |
| --- | --- | --- |
| B1 | Receiver admin: `semon receive enroll --machine <name> --listen <addr> --advertise <https url>` | The authenticated local OS owner authorizes one enrollment for that machine. First run generates receiver identity/TLS custody, assertion issuer key, separate cache master key, stable owner principal, exact self-grant inputs and receiver audit directory. It starts foreground receiver transport and a loopback-only authenticated owner Viewer through receiver integration. Show a code only on the authenticated local pairing surface. Later enrollments reuse the owned service through its local administrative interface. |
| B2 | Origin: `semon connect --to <https url>` | The nonsecret address selects the destination, not trust. Begin bounded provisioning and show a non-echoing Semon code prompt. |
| B3 | Enter the receiver code directly into that trusted origin prompt | Complete the reviewed pairing/bootstrap and mutual key confirmation. No transfer through chat, files, URLs, command arguments, logs or other untrusted channels. |
| B4 | Approve one combined trust/source summary in `connect` | Confirm receiver-as-issuer trust, authenticated receiver owner, selected current sources, typed name/path disclosure and P1 restrictions. Bind the origin-generated public key, issue separate kind credentials and certificate, and atomically activate the enrollment. Persist source consent separately; downstream policy/producer/publication integrations consume the outputs. |

D4 selects `public_tls` short-code confirmation for receivers whose advertised
HTTPS endpoint already has normal trusted certificate-chain and hostname validation.
Private-root/LAN receivers keep the long `pinned_invitation` (71 characters).
PAKE (password-authenticated key exchange) is a separate later track, only after a
maintained implementation clears independent review and owner approval. No
bootstrap TLS profile skips chain trust. Tailscale `ts.net` names appear in public
certificate-transparency logs, disclosing the hostname.

The fallback preserves the same trusted-device protected entry, combined approval,
scope binding and single-use activation requirements. Neither mechanism adds a
fifth human receiver identity check. PAKE confirms shared session-key possession,
not human/account identity; the local receiver owner authorizes one enrollment,
not a remote owner login. Another OS user or pairing participant never obtains
the receiver owner's session or arbitrary administration authority.

## Pairing bootstrap and provisioning state

Normal TLS chain and endpoint validation precedes pairing for both profiles.
A private-root/LAN receiver uses `pinned_invitation`: its protected transfer adds
the receiver pin, while explicit trusted-root provisioning remains necessary.
A matching pin never substitutes for chain trust. No system trust-store mutation,
general verification bypass, TOFU or peer-installed unverified root is permitted.
Before confirmation the channel carries only bounded protocol material, never
application bodies, issued credentials, source policy or grants.

The transcript binds receiver identity and exact advertised endpoint; the
pinned_invitation profile additionally binds its TLS pin. public_tls validates
public CA roots and hostname and records no receiver certificate fingerprint.
Both profiles bind the
origin-generated public key, both roles, fresh session identity, protocol version
and intended enrollment/kind/scope. Require mutual key confirmation and proof of
origin key possession before approval can activate authority. The origin client
private key is generated and retained on the origin; it never leaves it. Bootstrap
provisioning is separate from body routes. Normal post-provisioning connections
require HTTPS hostname and configured root/pin validation; body routes require
mTLS. Redirects are refused during provisioning and subsequent transport. Requests
cannot install issuer keys, change endpoints or relax trust.

| State | Transition and authority |
| --- | --- |
| `pending` | After local owner authorization, the peer connection contacts the receiver and `create_session(connection_binding)` allocates a fresh connection-bound session and in-memory code. The local admin surface receives the code once through a redacting secret wrapper. No active credential, user grant, claim or body authority. |
| `confirmed` | Both peers confirm the bound cryptographic transcript and origin key possession. Pending custody remains inactive; confirmation alone is not source approval. |
| `approved` | Origin B4 approves the authenticated trust/source summary. Persist the exact approval and separate source-consent record before activation; no partial usable credentials or grants. |
| `active` | One durable atomic activation consumes the single-use session, commits the scoped enrollment/credential generation and releases exact self-grant inputs. No claim/head is activated by this transaction. |
| `rejected`, `cancelled`, `expired` | Terminal pre-activation outcomes. Discard pending secret material, retain bounded nonsecret outcome/replay state, and activate no credentials/grants. A new attempt requires an explicit admin action under aggregate bounds. |
| `revoked` | Terminal enrollment outcome, including revocation after activation. All kinds and active hooks lose authority; recovery requires fresh pairing and a new enrollment ID. |

Activation is single-use and serialized against competing confirmation, approval,
expiry, cancellation and revocation. At most one origin key wins a session; losing
races, replay and consumed-session retries cannot issue another credential or
clear revocation. Crash/lost-ack recovery reports the same committed outcome to
the authenticated peer, never creates a second enrollment or exposes partial
activation. Cross-machine persistence is not a distributed atomic transaction:
until durable local trust/consent and matching receiver activation are verified,
the origin stays inactive. Failed or cancelled pending state cannot become active
on restart; post-activation cancellation uses disconnect/revocation.

### Approved pairing parameters (2026-11-12)

`public_tls` uses Crockford base32: nine uniformly sampled five-bit data characters
(45 secret bits) plus one weighted mod-32 check character. Odd data weights are
1, 3, 5, 7, 9, 11, 13, 15, 17. Display `XXXXX-XXXXX`; accept case-insensitive
input, optional middle hyphen and surrounding whitespace, O as 0 and I/L as 1;
reject U. At most ten significant characters are typed. The pure origin parser
checks locally before sending or spending an attempt. Every single-character
substitution is detected; transpositions are not all detected (adjacent unequal
data pairs are detected in 30/31 cases; pairs differing by 16 can collide).

Both profiles have ten minutes from creation to confirmation, ten minutes from
confirmation to B4 approval, and an absolute twenty-minute session lifetime.
Exact deadline instants are expired. The injected Unix-seconds wall clock is
clamped to the latest persisted/observed value: backward jumps never reopen gates
or extend sessions. Codes exist only in memory; restart destroys pending sessions,
which cannot activate. Three guesses are atomically reserved before verification,
never refunded for disconnect/timeout. The fourth request burns/rejects the session
and destroys the code. Attempts charge only the connection-bound session; unknown
or foreign session identifiers cannot charge another session. Bound malformed
requests spend an attempt; the normal origin flow rejects malformed codes locally.

`public_tls` permits four pending sessions and at most six creations per rolling
hour across all connections/addresses. Three burns in a rolling hour impose a
persisted thirty-minute pause, measured from the latest burn. The pause gates only
NEW creations, never attempts in existing sessions. Reissue/restart cannot reset
it; there is no override API. The creation limiter is the binding guessing bound;
the pause is an additional abuse response. One session gives 3/2^45 = 8.5e-14.
A pairing event lasts at most twenty minutes and fits within one rolling hour:
at most 6 x 3 = 18 guesses, or 18/2^45 = 5.1e-13, within the <=1e-12 target.
The yearly worst case is 18/hour x 8,760 = 157,680 guesses / 2^45 = 4.5e-9,
within <=1e-8. These public_tls targets replace the earlier 2^-80 target.

The gate is `DIR/enrollment/pairing-gate.json`, strict format 1, bounded to 4 KiB.
It persists creation/burn timestamps, latest observed time and pause-until using
registry custody helpers and an explicitly unlocked `pairing.lock` owner lock,
atomic durable writes, 0700 directories,
0600 single-link files and no-symlink opens. History drops entries at least one
hour old. Creation history is capped at six; burn history at twelve to cover
sessions created before the rolling window as well as its six new creations.
Missing, corrupt, unreadable or unsupported gate state produces the typed
`GateUnavailable` refusal. `Store::initialize` creates the gate before publishing
the registry, under the same first-initialization lock; there is no standalone
gate initializer on existing custody. Interrupted initialization remains refused,
never repaired by reopening. A missing gate beside an existing registry is never
recreated. Recovery requires an explicit operator-visible procedure outside this PR.

Any gate persistence failure poisons the in-process Core before another operation
can reload older disk state: creation and further gate operations fail closed,
retaining unsaved creation/burn records and pause-until in memory. Restart loads
only durable state and destroys all pending sessions. A crash between rejecting a
session and persisting its burn can lose that one burn and therefore its pause
trigger; its guesses were already spent and cannot be recovered by that session.
This in-process poison is not a claim that an unpersisted burn survives a crash.

Confirmation uses `ring` PBKDF2-HMAC-SHA256 at the named
`PBKDF2_ITERATIONS = 600_000`. Salt is the canonical length-prefixed transcript
prefix (version, session ID, both fresh nonces, roles). HKDF-SHA256 uses distinct
origin-confirmation, receiver-confirmation and application labels. Directional
HMAC-SHA256 covers the full length-prefixed transcript: version, roles, session ID,
exact normalized advertised receiver HTTPS scheme/host/explicit port, caller-supplied
TLS exporter, origin public key, intended scope commitment, nonces and profile.
The origin MAC comes FIRST. Only after constant-time `ring::hmac::verify` succeeds
may the receiver emit its MAC; an incorrect guess produces no receiver code-keyed
MAC. The origin verifies that MAC before considering confirmation complete.

PBKDF2 increases the cost of offline search of a captured 45-bit-code MAC:
2^45 guesses x 600,000 iterations is about 2.1e19 HMAC-SHA256 evaluations.
At ~1e10 evaluations/s per GPU, a 1,000-GPU attacker covers ~0.02% of the
space during ten minutes. This is a review cost model, not measured attacker
hardware. CI logs one real per-side derivation as `PAIRING_KDF_COST_MS` with no
speed assertion. A mistyped hostname hands that host the code; endpoint/exporter
binding and PBKDF2 cost bound what that host can do. PAKE is the real fix and
remains a separate later reviewed track.

`pinned_invitation` parameters, **implemented in PR 3b**: 128-bit secret, 64-bit
session ID, 128-bit truncated receiver pin, 71-character protected invitation,
three attempts, the same ten/ten/twenty-minute windows, pending cap two,
six issuances/hour and three burns/hour causing the same persisted thirty-minute
pause. Its admission limits are four concurrent provisioning connections,
8 KiB maximum request, ten-second read deadline and thirty unknown-ID requests
per minute. PR 3.1 does not implement this profile.

Codes, invitations, keys and bearer secrets never enter Debug/Display output,
logs, URLs, assertions or errors. Pairing does not persist P1 source consent;
the caller supplies verified private-key signature evidence binding B4 to the
transcript and exact summary digest, and must durably persist consent separately.

### public_tls provisioning wire contract

Contract additions for review (receiver PR3.2b-1): provisioning uses the existing
listener, with dedicated HTTPS POST routes `/v1/provision/open`, `confirm`,
`approve` and `cancel`. The dispatcher precedes analytics/body classification.
TLS 1.3 is mandatory before allocating pairing state. Obtain exactly 32 bytes
from the live rustls connection's `export_keying_material` with label
`EXPORTER-semon-asi-pairing-v1` and empty context. Bind the session to that
connection's exporter; never accept exporter bytes from a request. Resumption is
permitted, but a resumed connection has its own exporter and cannot inherit a
pending session's authority.

Messages are strict JSON objects (`Content-Type: application/json`), with explicit
`version: 1`, no unknown or duplicate fields, and canonical base64url without
padding for binary fields. Session IDs retain pairing.rs's 64 lowercase hex
characters. Every request and response has an exact 8,192-byte encoded ceiling;
check Content-Length before reading and bound the reader. Provisioning has a
10-second request read deadline and at most eight admitted connections across the
receiver, separate from body/analytics dispatch. Core retains the four-pending,
six-creations/hour and ten/ten/twenty-minute session limits. Excess admission
allocates no session. All external protocol failures are the same generic
refusal, including unknown session, wrong code, expiry and exhausted budgets.
No redirects are emitted or followed; cookies and Authorization headers confer
no provisioning authority.

`open` carries only version; its response carries version, session ID, receiver
nonce, canonical advertised endpoint, receiver ID, assertion issuer key ID and
public key. The origin copies receiver ID into ActivationInput and commits the
full input in `confirm`. Receiver ID is required for all kinds and must equal the
receiver's own identity before approval or activation. Mirror-only enrollment
uses Binding::Inapplicable and installs no issuer pins. Issuer pins additionally
belong in Binding::Admitted for analytics/body kinds, which are not enabled yet.
The code is delivered once only to the
receiver's trusted local terminal. It has no request or response field.
`confirm` carries version, session ID, 65-byte origin public key, 32-byte origin
nonce, 32-byte scope commitment and 32-byte origin MAC. The receiver reconstructs
the existing pairing.rs transcript with its own exporter/nonce/endpoint. Only a
verified origin MAC permits a `confirmed` response (version and receiver MAC).
`approve` carries version, session ID, the full strict ActivationInput and 64-byte
proof. Recompute input.commitment() and require transcript.scope equality. Apply
receiver policy: named authorized machine, valid binding, offered kinds only,
empty certificate pins and no client-cert requirement for mirror. Only mirror_push activates in this slice; analytics_publish refuses until origin connect / independently validated producer admission lands. Peer-supplied namespace/incarnation bindings cannot authorize activation.
Unsupported-kind diagnostics are released only after connection/session binding, current confirmed state, exact commitment and origin possession proof verify; earlier failures remain generic. Body-channel enrollment is available after client-certificate issuance lands.
The **initial credential validity ceiling** is 90 days (7,776,000 seconds) from
activation now, exclusive lower bound (`valid_until > now`) and inclusive upper
bound. The accepted PR4 contract retains this 90-day validity and renews in the
last 30 days under valid enrolled trust. `cancel` carries
version and session ID and remains bound to the same TLS connection.

The origin key is an uncompressed P-256 point. Proof of possession uses
`ring::signature::ECDSA_P256_SHA256_FIXED` over
`b"semon-asi-pairing-pop-v1\0" || transcript.digest() || session_id.as_bytes()`.
The transcript digest is pairing.rs's existing SHA-256 canonical transcript
commitment, including the exact B4 scope digest. Verify this signature before
constructing `VerifiedApproval` or activating authority. The wire module does not
choose enrollment scope, install grants, or certify durable origin consent.
`recover` carries version, original canonical transcript fields, application key
(32 bytes) and proof (64 bytes) on a new normally validated TLS connection. Sign
`b"semon-asi-recovery-pop-v1\0" || transcript.digest() || session_id.as_bytes()
|| new_connection_exporter`. Rebuild the origin-held ConfirmedKey only after proof
verification; deliver independently matches origin-key, transcript and application
hashes in the durable receipt, expiry, revocation and the two-delivery cap.
Application key custody is equivalent to its derived bearer authority; it travels
only to the enrolled TLS endpoint. The new exporter binds recovery to its current
connection; generic failures and provisioning admission bounds apply. No raw
application key or bearer is persisted. After two interrupted deliveries explicit
revocation is required; recovery cannot revive authority. Receiver recovery refuses receipts with any kind not currently offered (mirror_push only) before reserving another delivery.

The receiver controller is enabled only by local `receive enroll` authorization.
Codes go directly to `/dev/tty`, never stdout/stderr or a wire field. Its local
terminal accepts `pending` and `cancel SESSION_ID`. Activation commits enrollment,
kind hashes and receipt together, followed by exact owner grant installation;
recovery revalidates committed authority before repairing interrupted grants. Grant installation holds owners.lock and a fresh registry lock through the authority check and ACL write; busy/revoked state refuses. The mutex-held KDF and /dev/tty write are known limits: other provisioning and local pending/cancel actions wait without a mutex deadline; existing authenticated routes use separate locks.

## Source consent and P1

B4 confirms the journey's accepted first-owner policy profile P1. Show the selected
currently discovered local sources, including private/work material, and allow
narrowing before approval. Default selection is all current sources; apply an
`internal` floor only to currently unclassified selected sources. Preserve existing
classifications, stricter floors, baselines and scans. Unknown classification or
missing, failed, unsupported, incomplete or stale scans never count as successful
evidence. Restricted sources remain local unless the source owner explicitly opts
into stream-through for the owner principal; named-user opt-in is deferred with
multi-user support. The origin binds the required delivery mode to each response;
the receiver may tighten it to stream-through but never relax it to cached.
Stream-through bypasses cache operations and retry spools and requires no-store
responses, as specified by [the body-channel contract](asi-body-channel.md#authorization-audits-and-cache).
Renewal preserves this consent and cannot change the delivery mode policy.

P1 is scoped to the named receiver and its authenticated owner principal: typed
receiver analytics including plain names/paths and permitted coverage, plus
approved bounded redacted evidence with encrypted cache under D3 (at most
15 minutes, 64 MiB/source, 256 MiB global, narrowed by every contributor). No Hub,
other user, raw replica or hosted-model evidence egress is included. Explicitly
state that receiver-as-issuer trusts that operator to impersonate users; choose
independently pinned external issuer administration or local-only use when that
trust is unacceptable. Declining B4 keeps sources local and cancels activation.

Persist selected-source consent and its receiver/owner/scope binding independently
from enrollment and transport credentials. Renewal cannot widen it, future
roots/sources require a new explicit local selection update, and machine
credentials cannot substitute for it. The policy generator must resolve complete
admitted contributor identities and actual detector/redactor results under
[the governance API](asi-governance-api.md#pinned-receiver-projection-593-slice-2).
It cannot omit a denied contributor, invent a successful scan or fall back on an
absent registry. Scan-backed P1 registry generation is a downstream dependency,
not implementation assigned to 4a. Body availability remains refused until actual
policy, custody, user authentication and both audits pass, including cache hits.

## Receiver enrollment record

An enrollment is an operator-authorized binding, not a connection or token. One
current versioned registry rejects unknown fields, unsupported versions, duplicate
identities and invalid bindings; updates are atomic and durable. Invalid or
unreadable custody refuses authorization rather than appearing empty.

| Field | Contract |
| --- | --- |
| `enrollment_id` | Receiver-allocated opaque stable identity; fresh ID on lost/compromised trust recovery, never inferred from a hostname/path/address |
| `receiver_id` | Receiver identity bound by confirmed provisioning to its HTTPS endpoint/TLS identity |
| `machine` | Registered identity using the existing machine-name grammar; display rename cannot change namespace/native identity |
| `namespace` | Validated existing local namespace binding, not a path or a copy of another origin's identity |
| `origin_incarnation`, `index_incarnation` | Validated origin instance and admitted index identity; retain both when lifecycles differ; never infer from contact time |
| `publisher_claim_epoch` | Explicitly unassigned before 4b binds a claim; never use zero as fabricated active authority |
| `client_certificate_pins` | Tagged SHA-256 SPKI or leaf-certificate fingerprints bound to the origin-generated key and current credential generations |
| `credential_kinds` | Explicit enabled kinds and per-kind `client_cert_required`; not inferred from another credential |
| `revoked_at` | Whole-enrollment timestamp; absent only while not revoked |
| Per-generation `revoked_at`, kind availability and owner enable authorization | PR4.2 records terminal generation revocation separately from current availability; only a consumed owner authorization permits fresh kind credentials; whole-enrollment revocation overrides all kinds |
| `issuer` | Explicit `receiver` or `external`, with origin trust acknowledgement |
| `assertion_issuer_keys` | Current key ID/pinned public key; at most two accepted pairs during explicit bounded issuer rotation |

For analytics/body kinds require the full origin/namespace binding. Generate
identity material and validate existing namespace/incarnation continuity locally;
ordinary pairing does not ask the user to construct these internal fields. If
there is no admitted namespace/index binding yet, activation of these kinds waits
for the owning producer lifecycle; 4a cannot invent a namespace/claim to bypass it.
Mirror-only enrollment binds receiver, enrollment and machine; incarnation, claim
and issuer fields are explicitly inapplicable until relevant kinds are enabled.
Mirror custody grants no publisher claim or replica authority. Native UID,
lineage and provenance rules remain unchanged.

<a id="origin-configuration-and-out-of-band-enrollment"></a>

## Origin configuration and custody

Origin configuration records enrollment/receiver identity, exact endpoint and
explicit TLS trust profile (public CA roots for public_tls), machine/namespace/incarnations, assigned-or-unassigned
claim epoch, enabled kinds/certificate requirements, issuer acknowledgement and
pinned public keys, client certificate/pins and protected secret references.
Requests cannot overwrite configuration. Provisioning installs this state through
the confirmed exchange; bundles are internal protocol objects, never user-handled
files. No manual public-bundle transfer, separate bearer delivery or import step
is part of normal enrollment.

| Location / custody class | Generated or persisted state; integration output |
| --- | --- |
| Origin private secret custody | Origin-generated client private key and separately protected kind bearer credentials; never exported to receiver or ordinary backups |
| Origin integrity-protected configuration | Endpoint/root/pins, enrollment/scope, certificate and issuer public pins, secret references and revocation/generation state; separate persisted source consent for policy generation |
| Origin source/policy custody | Existing namespace, handles and admitted source identity; downstream generated policy/registry from consent plus real scan evidence, separate from credentials |
| Receiver private registry | Live enrollment/credential hashes, activation/consumption, generations and compact terminal tombstones; ended-generation evidence moves audit-first to receiver-audit; no persisted raw bearer secret |
| Receiver separate secret custody | Receiver TLS private key/root custody, assertion signing key and independent random 32-byte cache master key; none stored in cache or ordinary history backups |
| Receiver authenticated-owner/ACL custody | Stable principal from authenticated local OS owner, with stable opaque audit mapping; generate exact machine/namespace `analytics` and permitted `body_read` self-grant inputs for `receiver_acl`, no wildcard or pairing-participant principal |
| Receiver temporary cache custody | Dedicated owned cache directory and separate master-key reference for `body_cache`; D3 limits/exclusions validated by receiver integration, no bodies or keys in enrollment records |
| Receiver sensitive audit custody | Dedicated receiver directory for `receiver_audit`, separate from cache/ACL and origin audit; `audit.lock` and `attempts.jsonl` |
| Origin sensitive audit custody | Separate origin directory outside native roots/cache for `origin_audit`; `audit.lock` and `authorizations.jsonl` |

4a generates custody and explicit cache key/directory, ACL owner/self-grant and
separate receiver/origin audit-directory outputs. Their owning libraries validate
and consume them; receiver integration establishes the owner browser session and
wires safe body service. Do not duplicate storage-lane modules or equate generated
files with functioning access. First-run precise grant inputs are pending until
an enrollment has an approved scope and atomic activation; no early active grant.
Later enrollments reuse receiver identity, owner, keys and directories.

Implementation status (slice 4a PR3.2a, part of #593): `receiver_custody`
is a Unix library consumed by receiver-side provisioning. Under `DIR`, mode-0700 directories are `enrollment/`
(current registry format 3), `issuer-secrets/`, `cache-secrets/`, `body-cache/`, `acl/` and
`receiver-audit/`. Mode-0600 files are `receiver.json` (strict JSON format 2:
random receiver ID and current issuer key ID), `custody.lock`,
`issuer-secrets/issuer-<key-id>.seed` (raw 32-byte Ed25519 seed),
`cache-secrets/cache-master.key` (independent raw 32-byte key), `acl/grants.json`
(existing ACL JSON format 1), `acl/owners.json` (strict JSON format 1 enrollment
ownership and durable pending ACL transaction), and `acl/owners.lock`.
Issuer IDs are the first 16 bytes of SHA-256 over
`semon/receiver-assertion-issuer/v1\0` followed by the public key, lowercase hex.
Issuer seed, cache master key and cache occupy three distinct custody directories;
backing up an individual custody directory does not capture the other secrets.
Whole-state backups must exclude secrets and cache; exclusions still require
operator attestation. Receiver identity is published last; partial initial
custody refuses without replacing secrets. Owner identity and opaque audit mapping
are domain-separated SHA-256 of the authenticated effective UID; a replacement
account assigned the same UID receives the same principal and audit mapping.
Audit journal/lock
creation remains owned by `receiver_audit`. PR3.2b wires provisioning, activation/
grant calls, CLI, origin custody and tokens cutover; the listener and provisioning
controller consume this library. PR4.2 adds operator revocation/enable commands;
#594 wires body service and host-custody decisions.

Owned 0700 directories and 0600 regular files, checked ownership/links and
no-symlink opens protect secret/registry/ACL/audit custody according to their
libraries. Keep secrets, policy/configuration, body cache and audit stores separate;
protect atomic updates against cross-store replacement. Ordinary enrollment/cache
backups contain no private keys or bearer secrets. Explicit secret backups remain
a separate protected custody procedure. A stale registry backup cannot establish
current revocation state or authorize resurrection of an enrollment. The checked
library API rejects stale in-process snapshots, but replacing the registry file
on disk with an older valid copy is not detectable: no revision high-water mark
is persisted outside that file. Restoring such a copy does not itself establish
current revocation state; stale-backup resurrection protection beyond the revision
check needs a separate high-water-mark custody decision and is deferred. Preserve
namespace/handle continuity only where proven, and preserve facts, sources and
audits on refusal.

Generating private files does not establish host safety. Receiver integration
checks observable core/swap settings and requires reviewable external backup
exclusions under the journey's custody prerequisites. Unknown/failed custody leaves
body service disabled and reports analytics-only partial setup. Stream-through is
an explicit policy choice, not a silent fallback. Enrollment installs no OS service
or firewall change; foreground lifetime and service setup follow the journey.

### Receiver client-certificate issuance (slice 4a)

Receiver initialization generates an independent ECDSA P-256 client-issuing CA.
Its 0700 `client-ca-secrets/` contains only `client-ca.pk8` (0600); it is distinct
from assertion `issuer-secrets/` and `cache-secrets/`. The public `client-ca.der`
and its creation time/hash in `receiver.json` bind custody. The CA lasts ten years,
has CA basic constraints with path length zero and only keyCertSign usage.
`client-trust.pem` is generated atomically as the exact CA PEM, mode 0600. Reopening
reuses this state and refuses a changed trust file; operators never edit it.
Existing receiver custody without the CA refuses with
“receiver custody predates client certificates; initialize a fresh receiver directory”.
Nothing is deleted, migrated or silently repaired.

Activation issues one clientAuth-only P-256 leaf for the existing origin pairing
key whenever any enabled kind requires a certificate. The scope MAC commits
exactly that key's tagged SHA-256 SPKI pin; foreign pins refuse before registry
commit. The leaf has CA:false, digitalSignature only, an opaque enrollment subject,
a random 128-bit serial, notBefore five minutes before activation and notAfter
exactly the approved credential expiry. Certificate issuance and enrollment are
one registry commit. The public DER is retained in the nonsecret delivery receipt;
approval and the single recovery delivery return identical base64url DER, without
padding, within the existing 8 KiB envelope.

Origin custody verifies parsing, its own SPKI, clientAuth EKU and approved validity
before atomic installation of `client-certificate.der` (0600), whose hash is covered
by signed `enrollment.json`. No client-CA trust anchor is assumed at the origin. Origin validation checks
SPKI, clientAuth EKU and validity against its clock; it does not enforce
BasicConstraints or KeyUsage. Expired and not-yet-valid certificates refuse with
an explicit diagnostic, without falling back to certificate-less transport.
`connect --require-client-certificate` enables this for mirror_push; `push` presents
the leaf with the existing `origin-key.pk8`. The default stays off. Body and
analytics activation remain refused pending producer/storage admission. Renewal,
rotation and private-root provisioning remain outside this issuance slice.
`DIR/client-trust.pem` and `DIR/client-ca.der` are in the receiver directory root;
only `client-ca.pk8` is in the mode-0700 `DIR/client-ca-secrets/` directory.

## Credential kinds and custody

Slice 4a replaces the single-kind token file with one current versioned kind-aware
store. Each bearer record binds credential ID, enrollment ID, exactly one kind,
hash, generation/validity and revocation state. Issue separate secrets per kind
inside protected provisioning/renewal; the receiver never persists raw secrets or
prints them for separate delivery.

| Kind | Permitted routes | `client_cert_required` |
| --- | --- | --- |
| `analytics_publish` | `/v1/analytics/` once slice 5 implements it | Explicit per-kind setting, default `false` for now; revisited when #594 lands, operator may require it |
| `body_channel` | `/v1/body/` origin poll/response once #594 implements it | Always `true`, cannot be disabled while enabled |
| `mirror_push` | Existing `/v1/mirror/append`, `/facts`, `/status` | Explicit per-kind setting, default `false` for supported mirror Push |

A body credential permits machine transport only, with no `body_read`, metadata
discovery, cache admission or source-policy permission. Mirror credentials cannot
publish analytics or poll bodies; analytics credentials cannot use body routes.
Certificate possession alone grants no kind. Credential and any required verified
certificate must match the same current enrollment.

Under the [pre-release policy](../../AGENTS.md#pre-release-compatibility-policy),
there is no compatibility reader, migration or dual path for `M SHA256(token)`.
Finding old `DIR/tokens` refuses receiver startup with re-issue guidance and leaves
the file untouched. Re-issue kind credentials, explicitly enroll/configure machines
and update supported Push clients/token inputs together. 4a updates the owning CLI,
mirror docs and tests; no compatibility alias conceals the change. Refusal never
authorizes deleting old state or history.

### Rotation and revocation

The following PR4 contract records the accepted renewal/revocation parameters
before implementation merges, including PM's accepted replacement, renewal hint,
last-kind refusal and compaction decisions. Routine renewal requires no operator action. It
preserves enrollment identity, binding, source consent, kinds, certificate
requirements and origin key/pin. It never rotates the assertion issuer, server
identity or cache key, adds a kind, clears a revocation or transfers a claim.

### Validity, installation and cutover

Initial and renewed credentials last 90 days (7,776,000 seconds). On each `push`,
and continuously during `push --watch` even without new source events, check the
remaining validity. Renew when strictly less than 30 days (2,592,000 seconds)
remain. Emit the first due notice at the 30-day boundary. Validity is inclusive at
`valid_from` and exclusive at `valid_until`; no grace applies after expiry.

For each unrevoked kind, issue generation N+1 with fresh credential IDs/secrets.
Use the existing `Generations { current, retiring }` and
`Retiring { generation, cutover_at }` shape: N+1 becomes current and N retires at
`min(commit_time + 86_400, N.valid_until)`. Never extend N's own expiry. The first
successfully authenticated application request using N+1 durably clears that
kind's retiring slot before dispatch, immediately ending N's authority. Fresh
authorization and slot removal happen in ONE transaction under the exclusive,
nonblocking registry lock, using today's durable whole-document replacement. Derive
the PR4.1 authority handle from the COMMITTED state. Lock contention or uncertain
commit refuses this request; the origin retries, and N remains valid until its
cutover/expiry unless a committed cutover already ended it. Concurrent N+1 requests
re-authenticate against committed state after losing the lock race. Once the slot
is clear, later N+1 requests need no write. This path is independent of the
provisioning controller mutex and terminal. A renewal response alone is not first
use. Each kind cuts over independently; a retiring credential cannot start another
renewal. An enabled revoked kind gets a fresh current generation with NO retiring
slot; its revoked generation never regains authority.
An ordinary new begin outside the renewal window refuses; an outstanding owner
enable authorization permits an early begin under otherwise-valid current authority.

When a certificate is required, reissue a clientAuth leaf through `client_ca.rs`
for the SAME origin P-256 key and SAME tagged SPKI pin, with the renewed validity.
Record the same pin under the new generation; never replace it with a new key or
leaf fingerprint. CA validity still bounds issuance; renewal cannot repair expired
CA custody or change its trust. The private origin key never leaves the origin.

PR4.3 stages the complete renewed credential set, certificate, signed configuration
and unchanged consent binding using connect's owned 0700 directories, 0600 files,
no-follow opens, fsync and serialized atomic publication pattern. Keep the old
credential until receiver delivery is verified and the new local generation is
durable; keep it available for bounded recovery until first-use confirmation or
its cutover/expiry. A single durable generation selection prevents mixed files.
Interrupted staging remains retained and inactive. On restart, reconcile the
receipt and staged generation before sending application traffic. Never fall back
to an old credential after confirmed cutover, revocation or expiry.

### Offline machines and missed renewal

Expired credentials fail closed; failed renewal does not stop access while existing credentials remain valid. Recovery uses a NEW enrollment ID and the same
four actions: B1 authorize `receive enroll` on the receiver; B2 run `connect --to`
on the origin; B3 enter the code in its trusted prompt; B4 approve the combined
trust/source summary. Retained keys, consent and history do not authorize revival.

Fresh pairing must complete against retained custody. On the receiver, an enrollment
with no currently authorizable kind (all kinds revoked, or all credentials expired)
no longer reserves its machine name. Pairing `receive enroll --machine M` creates a
NEW enrollment ID and terminally revokes the prior enrollment in the SAME activation
registry commit, keeping its evidence in audit custody. If M still has a usable
kind, the receiver MUST ask for replacement confirmation in the same authenticated
owner terminal BEFORE showing any code. Only literal `yes` authorizes replacement;
other input/EOF cancels with no state change and exit 0. If M has no usable kind,
the name is simply freed with no replacement prompt.

Confirmation authorizes only a pending replacement ceremony. During pending,
confirmed and approved phases, the old enrollment stays usable and unchanged; no
old authority is revoked merely by generating a code. After origin pairing, key
confirmation and B4 approval complete, the single durable activation registry
commit creates the new enrollment's authority and terminally revokes the exact
prior enrollment. Carry the predecessor ID observed before the terminal prompt,
or explicit absence when no usable enrollment existed, into the pairing session.
Serialize and recheck that observation under the commit lock; a changed predecessor
refuses activation rather than revoking another enrollment.
Cancellation, expiry or failure before that commit leaves old authority intact.
This changes no namespace/claim transfer rules.

On the origin, `connect --to URL` permits replacement of existing endpoint custody,
including usable custody. The combined B4 approval summary explicitly states
replacement; that single approval covers it, with no separate disconnect. The old
origin enrollment stays selected until the complete new enrollment is durably
staged. Under the install lock, one atomic swap selects the new enrollment for the
endpoint and retains the previous files as non-active `replaced-<enrollment-id>/`.
Never delete those files; push selection ignores retained replacements. The atomic
selection and durable recovery metadata prevent mixed files or two active selections.

Before receiver activation commit, a crash leaves both old enrollments unchanged.
After receiver commit but before delivery or origin swap, the receiver has revoked
the old enrollment, while the origin may still select its old files; those files
cannot authorize receiver access. Resume the existing pairing delivery-recovery
path to obtain the SAME committed enrollment, then finish the staged atomic swap.
An uncertain origin swap is resolved under the install lock from durable selection
and staging metadata, never by pairing another enrollment or deleting old files.
After swap but before its acknowledgement, reopening finds the new active selection
and retained old files. Recovery exhausted or expired requires a fresh ceremony;
no old authority is resurrected. An application refusal qualifies custody for
replacement but does not prove revocation; generic renewal refusal proves neither.

| Operation / starting state | Human actions |
| --- | --- |
| Expired, receiver has no authorizable kind | 4: B1 enroll, B2 connect, B3 enter code, B4 approve replacement |
| Active replacement, including other receiver kinds still usable | 5: the four actions plus receiver replacement confirmation before code display |
| Re-enable with a running watch | 2: receive enable command and its confirmation; renewal is automatic |
| Re-enable without a running watch | 3: enable command, confirmation, then `semon push --receiver URL` on the origin |

These counts use the public_tls code profile and existing listener/TLS prerequisites.
The pinned_invitation profile uses an invitation, never a public_tls code. Standalone
disconnect retains its command plus confirmation; replacement never requires it.

For unsupported receiver format or actual capacity exhaustion, keep the old receiver
directory unchanged and initialize a new empty one, then pair each origin again
using this section's ceremony. Old history, keys and audits stay preserved in the
old directory but are NOT automatically available in the new receiver. There is no
standalone receiver recovery guide yet; this section is the recovery reference.

### Renewal wire and authority

PR4.2 adds only `POST /v1/renew/begin` and `POST /v1/renew/finish` to the existing
provisioning listener. These are not body/mirror routes and do not require a new
listener. Serve them during ORDINARY `semon receive`, independently of an active
pairing ceremony, selected machine, controlling terminal or pairing authorization.
Renewal has its own registry-backed handler; it must not acquire the provisioning
controller's blocking mutex or wait for its terminal. Share provisioning connection
admission accounting even when `receive enroll` is inactive. Normal endpoint/root
validation and TLS 1.3 are mandatory. Use exactly
32 live connection-exporter bytes with label `EXPORTER-semon-asi-pairing-v1` and
empty context, as provisioning does; never accept exporter bytes from JSON.

Use `Content-Type: application/json`, strict version 1 structs with
`deny_unknown_fields`, duplicate-field refusal, canonical unpadded base64url and
an 8,192-byte encoded ceiling for every request/response. IDs are 64 lowercase hex
characters, nonces/application keys 32 bytes, P-256 points 65 bytes and fixed-width
ECDSA P-256/SHA-256 proofs 64 bytes. Times are Unix seconds; generations are
positive u64 values with checked increment. All nested field lists below are exact.

| Message | Exact fields |
| --- | --- |
| Begin request | `version`, `enrollment_id`, `credentials` (array of `{ kind, generation, bearer }`) |
| Begin response | `version`, `enrollment_id`, `renewal_id`, `receiver_nonce`, `expires_at`, `generations` (array of `{ kind, generation }` for the complete target generations), `enable_authorization_ids` (array of bound owner authorization IDs) |
| Finish request | `version`, `enrollment_id`, `renewal_id`, `action` (`commit`, `recover` or `cancel`), `credentials` (same shape as begin), `application_key`, `proof` |
| Finish commit/recover response | `version`, `enrollment_id`, `renewal_id`, `credentials` (array of `{ kind, credential_id, generation, bearer, valid_from, valid_until }`), `client_certificate` (base64url DER or null) |
| Finish cancel response | `version`, `enrollment_id`, `renewal_id`, `cancelled` (true) |

The receiver selects scope from the registry, never from a requested target kind
list. Begin must present the current valid bearer for EVERY unrevoked kind, with
no duplicates, omissions or extras. If any returned kind requires a certificate,
require a verified enrolled client certificate on both calls. Any certificate
presented must match the enrollment and authenticated credential generations even
when optional. Neither a stolen bearer nor the origin key alone can renew.
Begin authenticates bearers before allocating its fresh receiver nonce; finish
also verifies origin-key proof before committing or exposing credentials.

Bind two separate sets at begin. The AUTHENTICATION set is exactly the previously
authorized unrevoked kinds and old generations presented at begin. The OUTPUT set
is those renewed kinds plus revoked kinds covered by owner authorizations already
pending at begin. Bind the complete target generation set AND those authorization
IDs in the nonce state and receipt. An enable recorded after begin does not
invalidate the attempt; it remains pending for the next attempt. Under the commit
lock, recheck that every bound authorization is still pending; revocation or
cancellation of any bound authorization makes finish refuse.

The signed message is `b"semon-asi-renewal-pop-v1\0"` followed by the canonical
length-prefixed fields: version (decimal ASCII `1`), receiver ID, canonical endpoint,
enrollment ID, renewal ID, receiver nonce, current connection exporter, action,
the SHA-256 of the exact begin credentials' nonsecret identity list and target
generation list, the SHA-256 of the sorted bound enable-authorization ID list
(compact JSON array), and SHA-256 of the application key. Use pairing's eight-byte
big-endian length prefix for each field; sort kind lists by their wire names and
encode each list as compact JSON with the field order shown above, excluding
bearer values from the identity list (`{ kind, generation }`). The scope digest
hashes the authentication-list JSON followed by the target-list JSON, each with
its own eight-byte big-endian length prefix. Sign with
`ECDSA_P256_SHA256_FIXED` and verify against the enrolled origin key. The receiver
must retain that verified key from pairing in integrity-protected registry custody;
a pin alone is not a proof-verification key. The nonce, action and live exporter
bind cancellation/recovery as well as commit. No old-connection proof is reusable.

The origin generates and durably stages the application key before finish. Derive
separate fresh kind bearers with HMAC-SHA256 over pairing-style length-prefixed
fields: `semon-renewal-bearer-v1`, enrollment ID, renewal ID, kind wire name and
target generation in decimal ASCII. Persist only its hash and bearer hashes at the
receiver. The protected application key permits regeneration of the same delivery;
it never enters logs, URLs, errors or receiver secret storage.

### Races, recovery, cancellation and bounds

One pending target generation set per enrollment is allowed, with a ten-minute
lifetime from begin (the exact deadline is expired). Repeated begin for the same
current generations is idempotent: return the same renewal ID, nonce, expiry and
targets, without extending time or allocating another generation. Each finish
proof binds its own live exporter, so a reconnect can finish the same pending
attempt. Begin with the old generation set after commit returns the committed
attempt's same identifiers while its recovery authority remains valid, without
reserving delivery; use finish `recover`, never another `commit`, for lost delivery. Competing finishes serialize under the registry lock; only the first
application-key commitment wins. Foreign keys, different application keys, altered
scope and replayed proofs refuse. An already committed attempt cannot issue N+2.

Commit the new generation set, retiring slots, certificate and nonsecret delivery
receipt in ONE registry transaction, rechecking current authority under the lock.
The receipt records renewal/enrollment IDs, origin-key hash, nonce and scope digest,
authentication and output sets, bound owner authorization IDs, application-key hash,
creation/expiry/cutover times, certificate DER, delivery count and outcome. After
commit, recovery expiry is the earliest old credential expiry or retiring cutover
over the AUTHENTICATION set ONLY, rather than the pre-commit ten-minute deadline.
A newly enabled kind contributes no old credential or retiring slot to that bound.
Durably reserve each delivery before exposing secrets: one initial delivery and one
`recover`, never two recoveries. Recover authenticates with EXACTLY the receipt's
authentication set of still-valid retiring bearers, plus key proof, current exporter
and the output set's certificate requirements. Return identical committed
credentials/certificate, never a second generation. Recovery ends if any old
authentication authority expires or retires, or if the enrollment or ANY kind in
the receipt's authentication/output sets is later revoked. After two uncertain
deliveries, no further secret delivery is allowed. Client handling follows the
conservative refusal rule below; do not infer revocation or mint another generation.

Cancel is a signed finish action with the same credential checks and a staged
application-key commitment. Before commit it terminally cancels the pending ID and
issues nothing; after commit it refuses without changing committed authority.
Stopping committed local use requires disconnect; receiver revocation is a separate
owner action, never advice inferred from that generic refusal. Restart
destroys uncommitted pending nonce state, so its finish cannot activate; a new begin
may allocate a new attempt for the same next generation. Committed receipts and
delivery reservations survive restart. Prune nonsecret renewal receipts once their
recovery authority has ended by expiry, retirement or revocation. Move ended
generation evidence to append-only audit custody before compacting registry records,
as specified below. Missing/pruned receipt IDs always refuse; persisted generation
high-water marks prevent an old begin from recreating the committed attempt. Keep cancelled/expired in-memory
attempt IDs only within the ten-minute window; unknown IDs always refuse.

Renewal shares provisioning's eight admitted connections and the listener total
and unauthenticated per-IP limits. Each request has a ten-second total read deadline.
Allow at most four pending renewals receiver-wide, one per enrollment, and at most
six NEW authenticated begins per enrollment per rolling hour; idempotent begins
allocate nothing. Persist creation timestamps/latest observed time with registry
custody so restart cannot reset that rate. Expire old pending entries before
admission; never evict a live attempt to admit another. No unbounded queue or
additional worker is created for refusal.

The registry ceiling stays 4 MiB (4,194,304 bytes). Reserve a FIXED 256 KiB
(262,144 bytes) exclusively for revoke, cutover, cancel and audit-neutral terminal
updates. New enrollments, begins and renewal commits must leave the encoded document
at or below 3,932,160 bytes, including reserved pending-outcome space. Reserve the
worst-case terminal delta before admission so revocation still succeeds at that
boundary. Terminal operations must not depend on allocating new receipt/audit
payload in this reserve; required audit evidence uses separate custody.

### Live-state budget and audit-before-compaction

An ended generation (expired, retired at cutover/overlap end, or revoked) cannot
authenticate. Do not retain its full credential or per-generation pin record in the
registry. PR4.2 adds the append-only `credential_generation_ended` audit record in
`DIR/receiver-audit/`, separate from cache and registry. Its exact fields are
`version` (1), `record_type` (`credential_generation_ended`), `event_id`, `observed_at`,
`ended_reason` (`expired`, `retired` or `revoked`), `credential_id`, `enrollment_id`,
`kind`, `generation`, `valid_from`, `valid_until`, `revoked_at` (timestamp or null)
and `pin` (tagged enrolled pin or null). The deterministic event ID is SHA-256 over
pairing-style length-prefixed `semon-generation-ended-v1`, enrollment ID, kind wire
name, decimal generation and credential ID. Never include bearer hashes/secrets.
Preserve this evidence forever; never automatically delete audit records.

Append the ended-generation record FIRST under audit custody's append lock and
fsync it, then commit the registry compaction under the exclusive registry lock
using durable whole-document replacement. Recheck the generation is still ended
and preserve its high-water mark. Never remove a registry record before durable
audit evidence exists. A crash between append and compaction leaves evidence in
both stores; restart repeats compaction safely. Repeated appends with the same
event ID denote the same evidence, not new events or authority. Crash before audit
fsync leaves the registry record intact; crash/uncertain registry commit is resolved
by rereading custody. Terminal updates may mark generations ended first and defer
compaction; no distributed transaction or audit-derived authorization is implied.

Keep only compact registry tombstones: revoked enrollment IDs and their tagged
pins, and the highest generation EVER issued per kind per enrollment, including
revoked enrollments. Keep one pin tombstone per enrollment for repeated same-SPKI
renewals; per-generation copies move to audit. Tombstones never grant authority
and are never pruned or lowered. New generations must exceed the persisted high-water
mark; revoked enrollment IDs and pins cannot be assigned to another enrollment.
A dropped credential has no hash match and therefore refuses. Authentication never
loads audit as a fallback or reconstructs credentials from tombstones. Once compacted,
a wall-clock rollback cannot restore a missing record; clamp the persisted/observed
clock so rollback cannot extend surviving validity or pending gates either. These
properties do not solve the separately deferred on-disk stale-backup replacement
boundary: restoring an old registry is not authorized recovery.

Budget on LIVE current/retiring credentials, pending attempts, recoverable receipts,
rolling admission state and compact tombstones, never accumulated renewal rounds.
Each bound below includes its JSON wrapper/separators and must be checked against
actual encoded size in CI; reject overlong fields before admission.

| Registry allocation | Encoded maximum |
| --- | --- |
| Live enrollment identity/binding, key, consent references, kind scopes, enable records and high-water marks | 8 KiB per enrollment; no source text |
| Live generation record, including credential and generation pin | 1 KiB; at most current + retiring for each of three kinds (6 KiB per enrollment) |
| Pending renewal nonce/targets/authorization bindings and reserved outcome | 4 KiB; one per enrollment, four receiver-wide |
| Live delivery receipt (pairing or renewal) | 8 KiB; at most one per enrollment; new attempts wait for recovery authority to end |
| Live rolling admission records | 1 KiB per enrollment; prune timestamps outside the rolling window using the clamped clock |
| Terminal enrollment tombstone: enrollment ID, validated machine name (at most 63 bytes), insertion sequence, one tagged pin and three kind high-water marks | 512 bytes; same pin appears once; machine/sequence support state-idempotent lookup only |
| Registry envelope, receiver-wide gates and allocation accounting | 64 KiB receiver-wide |

A fully occupied live enrollment budgets 27 KiB (8 + 6 + 4 + 8 + 1), conservatively
charging a pending attempt even though only four exist globally. The supported
capacity is 128 live three-kind enrollments PLUS 512 terminal enrollment tombstones:
128 × 27 KiB + 512 × 512 bytes + 64 KiB = 3,866,624 bytes, below the 3,932,160-byte
admission ceiling, leaving 65,536 bytes of additional slack and the untouched
256 KiB terminal reserve. Repeated renewals do not consume lifetime history space
in this registry; tombstones grow with distinct retired enrollments, not generations.
Actual encoded size and the per-record limits control admission, not the estimate.

If audit storage is full, invalid or unavailable, compaction cannot proceed. Keep
the ended records in the bounded registry; refuse renewals/new enrollments as local
`audit unavailable` (the wire refusal stays generic). New authority already requires
working audit custody. Revoke and cutover continue from the terminal reserve without
waiting for compaction/audit; preserve ended evidence for a later audit-first pass.
Admission reserves their worst-case deltas, including tombstone transitions, so
revoke still succeeds at the boundary. Never grow beyond 4 MiB or delete evidence
to recover space. At actual live/tombstone capacity, the owner starts a fresh receiver
directory under the existing refusal path, preserving the old directory and audit
custody. There is no renewal-count/lifetime cliff, migration or silent reset.

Local error classes are malformed/version, invalid bearer/key/certificate,
revoked/expired/retired authority, scope mismatch, replay/terminal attempt,
capacity/rate limit, custody/unavailable and commit-uncertain. All externally use
the same provisioning refusal status/body; no class, existence or remaining-budget
oracle. Pre-HTTP/TLS failures may close without HTTP. Audit begin, commit, delivery
reservation/recovery, cancel, first-use cutover, expiry and refusals with time,
nonsecret IDs, kind/generation, outcome and local reason class. Owner operations
also record authenticated owner and target. Never audit bearer/application keys,
private keys, proofs, exporter bytes or source text. Audit storage stays separate
from registry/cache; audit failure refuses new authority or release. Revocation
still removes authority under its reserved registry budget; audit failure must not
turn revocation into permission to continue access.

Because wire refusals are indistinguishable, the client treats EVERY renewal error
or refusal as temporary while its old credentials remain valid. Keep using valid
application traffic, retry within the admission/one-minute bounds and show days
remaining. One-shot push reports the notice and may complete ordinary traffic
successfully; renewal failure alone does not set exit 1. Watch continues. Never
claim a generic refusal proves revocation or instruct the owner to revoke. Show
fresh-pairing guidance only after real application traffic refuses the existing
credential (for example mirror HTTP 401) or local credential/certificate expiry.
Those conditions stop the channel and exit 1.

### Revoke, enable and current registry format

`semon receive revoke --dir DIR --machine M [--kind K]` requires the authenticated
local receiver owner and confirmation. Without kind it terminally revokes the
whole enrollment; with kind it revokes all current/retiring credentials of that
kind. Refuse a per-kind revoke if K is the enrollment's only unrevoked kind;
direct the owner to whole-enrollment revoke instead, before changing anything.
One registry commit records revocation, invalidates pending renewal/enable
authorizations for the target and signals in-flight hooks. A concurrent renew
either commits first and is included in revoke, or sees revoke and refuses.
Grants/cache invalidation follow their owning custody rules; access stops at the
registry commit even if cleanup is interrupted. Preserve facts, keys and audits.

The shipped PR4.1 `CredentialHandle` is current only while that
EXACT credential, its kind scope, enrollment and any presented pin remain
authorized and in validity. Re-read current custody before dispatch/release and
signal revoked in-flight work on kept-alive connections. Unrelated registry
revisions NEVER invalidate it. Missing/unreadable custody fails closed. It replaced
the former `RevisionHandle` that invalidated on any revision.

`semon receive enable --dir DIR --machine M --kind K` requires that same owner and
confirmation. BEFORE recording anything, require another unrevoked kind with a
currently valid, authorizable credential; otherwise refuse and direct fresh pairing.
It records a one-use authorization for an EXISTING revoked kind,
bound to enrollment ID, kind, owner, authorization ID, creation time and consumed
renewal ID (initially null). It grants no immediate transport authority. A second
enable with an unconsumed authorization is an idempotent no-op. Revoke cancels it.
The next authenticated renewal begun after that authorization consumes it atomically
and issues a fresh generation
for that kind as well as renewing the existing unrevoked kinds. This is the sole
owner-authorized extension to the normal unrevoked-kind result set. No client
request can choose it, add a never-enrolled kind, alter consent/binding/pin or
resurrect a revoked generation. A revoked enrollment is never reactivated.

The PR4.2 registry is one strict current format, replacing format 1. Keep live
credential records and compact tombstones; move ended-generation evidence to
append-only audit custody using the audit-before-compaction rule above. Replace
the terminal whole-kind `KindScope.revoked_at` shape with current kind availability
and explicit one-use owner authorization records. A fresh generation may make the
kind available only by consuming its matching authorization; previous revoked
records stay terminal. Retain the enrollment's proof-verification public key,
renewal receipts and bounded persisted admission records in the same atomic
registry. No format-1 reader, migration, dual write or compatibility path remains.
Old receiver directories refuse with the exact fresh-directory message below;
leave their files, keys, sources, history and audits untouched.

The no-valid-bearer authority boundary is closed by the last-kind revoke and
enable preconditions above. Never accept a revoked/expired bearer or key-only
renewal. If authority expires before consumption, the authorization grants no
exception; use fresh pairing and the replacement ceremony.

When any owner enable authorization is pending, or the renewal window is reached,
add the fixed response header `Semon-Renewal: due` to ordinary AUTHENTICATED mirror
and body responses. It means only renew now for that credential, carries no other
information or secret and is never sent to an unauthenticated
caller. The origin starts renewal on seeing it in addition to its local 30-day
check, including during watch without source changes. A signal received during an
attempt schedules the next eligible attempt; it does not change that attempt's
bound authorization set. Apply the same retry limits and certificate/authority
checks; the header grants no authority and cannot widen scope.

Revoke, enable and disconnect are state-idempotent. Reread durable custody before
prompting; that read reconciles an uncertain whole-document commit. Do not persist
operation IDs, outcome receipts, acknowledgements or expiry state. Revoke resolves
M to its most recent non-tombstoned enrollment, or its most recent revoked enrollment
if all are revoked. Already-revoked targets report the existing success without a
prompt or write. Enable with an already-pending authorization reports its existing
success without a prompt or new record; an already-enabled kind reports the table's
error. Otherwise use the normal confirmation. A replacement enrollment can only be
changed after the owner types literal `yes` at a new prompt; declining changes
nothing. Compact tombstones retain the validated machine name and insertion
sequence solely for this lookup, never as authority. Origin-side disconnect in
PR4.3 follows the same state-idempotent rule. If custody remains unreadable or the
outcome remains uncertain, refuse with the table's same-command guidance; never
direct operators to custody files.

### Origin disconnect and delivery split

PR4.3 adds `semon disconnect --receiver URL` with confirmation. Under origin
custody's install lock, durably disable the selected enrollment/channel and active
receiver/issuer trust, stop renewal/publication and cancel active delivery. Keep
history, keys, consent and audits. Repeated disconnect is harmless. It is local
only: it sends no receiver revoke request and requires no receiver contact. A
concurrent renewal cannot publish active custody after disconnect wins the lock.

| PR | Required delivery |
| --- | --- |
| 4.0, this docs-only PR | Accepted renewal/revoke/enable contract, exact operator strings and CI obligations; no runtime implementation |
| 4.1, separately in flight | Credential-bound authority handle and in-flight checks independent of unrelated registry revisions |
| 4.2a (#665), shipped | Current registry, revoke/enable, audit-before-compaction, credential_generation_ended records, tombstones and first-use cutover |
| 4.2b, implementation draft | Receiver renewal begin/finish during ordinary receive, enrolled-key proof, generation receipts and reserved recovery deliveries; strict registry format 3 adds renewal receipts and persisted admission. Receiver CI and independent review remain required. |
| 4.3 | Origin push/watch renewal, staged installation/recovery, local disconnect and URL-based --receiver on connect/push/disconnect using one endpoint normalization; replace #651's hash selector in docs/tests together |

PR3b private-root follows PR4; a future `--trust-root FILE` is outside this contract
and its behavior is not decided.

### Exact new operator-visible strings

This is the single wording inventory for PR4 and the UI/UX Manager's wording/step
review. `M`, `K` and `D` mean validated machine, kind and whole days remaining
(rounded up). `DIR` is the receiver directory. `URL` is the full validated HTTPS
endpoint given to connect, normalized with `pairing::normalize_endpoint`; `HOST` is
its sanitized host[:port], with IPv6 brackets preserved. Notices/status/confirmation
show HOST; commands use URL. Never display userinfo or query parts. Connect, push
and disconnect use ONE endpoint normalization, including default ports and IPv6,
without DNS alias guessing. `--receiver URL` accepts that HTTPS URL; the 64-hex
custody key stays internal. No hash alias or operator-chosen alias is supported.
The registry is authoritative for one current normalized advertised endpoint,
initialized by `receive enroll --advertise URL`. Immutable `receiver.json` identity
custody contains no endpoint. Every live enrollment retains its own normalized
enrolled endpoint within its 8 KiB budget. Plain `receive --dir` keeps its invocation
and loads the registry endpoint. Enable's push command uses the target enrollment's
own endpoint; the common fresh-pairing recovery block uses the receiver's CURRENT
endpoint, including after compaction. Neither guesses from a listen address. The
origin custody name is SHA-256 of its enrolled endpoint.

An address change is exceptional and is not a four-action enrollment claim. On an
existing directory, `receive enroll --advertise NEW` requires the authenticated
owner's literal `yes` at the address-change prompt BEFORE any code is displayed.
Decline/EOF changes nothing. Confirmation replaces only the current endpoint in ONE
registry commit, with requested/committed owner audit metadata; unavailable audit
must not preserve old authority, following revoke's rule. Prior enrollments remain
unchanged and are not revoked or deleted. The shared credential authority predicate
refuses an enrollment whose enrolled endpoint differs from the current endpoint;
this applies to every route, first-use cutover and in-flight `CredentialHandle`
checks. Such an enrollment has no usable kind, freeing its machine name without a
replacement prompt; enable refuses with current-endpoint fresh-pairing guidance.
Old endpoint enrollments remain budgeted live state until credentials end and
audit-first compaction applies. PR4.3 changes main's existing #651 push selector contract,
with docs and tests updated together. `TARGET`
is `machine M` or `kind K for machine M`. Do not substitute secrets. Prompts and
confirmation feedback use the controlling terminal; no terminal means refuse.
Only literal `yes` confirms; any other input/EOF cancels without changing state.
Success uses stdout and notices/errors use stderr. Errors with a `semon: ` prefix
below use main's existing error path; the capitalized multiline recovery/status
messages are emitted exactly as shown, without an extra prefix. Operational errors
exit 1; cancellation exits 0. No additional renewal prompts.

| Surface | Trigger | Exact text | Stream / exit behavior |
| --- | --- | --- | --- |
| receive enroll | M has a usable enrollment, before any code display | `Replace the active enrollment for machine M? Its access will stop when pairing completes. Type yes to confirm: ` | owner terminal; only literal yes proceeds; no registry change yet |
| receive enroll | Replacement declined or EOF | `Enrollment replacement cancelled; nothing changed.` | terminal; exit 0 |
| receive revoke / enable / disconnect | Confirmation input accepted | `Confirmation received.` | terminal; no success claim until durable commit |
| receive revoke | Confirmation prompt | `Revoke TARGET? This stops access. Type yes to confirm: ` | terminal; wait for input |
| receive revoke | Cancelled input | `Revocation cancelled; nothing changed.` | terminal; exit 0 |
| receive revoke | Durable success, including already revoked | `Revoked TARGET. History and audits kept.` | stdout; exit 0 |
| receive revoke | K is the only unrevoked kind | `semon: kind K is the last enabled kind; revoke the enrollment with semon receive revoke --dir DIR --machine M instead.` | stderr; exit 1; change nothing |
| receive revoke | Unknown machine | `semon: machine M was not found; check --machine, then retry revocation.` | stderr; exit 1 |
| receive revoke | Unknown kind | `semon: kind K was not found for machine M; check --kind, then retry revocation.` | stderr; exit 1 |
| receive revoke | Wrong owner or missing terminal | `semon: revocation requires the receiver owner's terminal; run this command there.` | stderr; exit 1 |
| receive revoke | Custody failure or uncertain commit | `semon: revocation could not be confirmed; rerun the same command to check its earlier outcome before retrying.` | stderr; exit 1 |
| receive enable | Confirmation prompt | `Allow fresh access for kind K on machine M? Type yes to confirm: ` | terminal; wait for input |
| receive enable | Cancelled input | `Enable cancelled; nothing changed.` | terminal; exit 0 |
| receive enable | Durable authorization, including already pending | `Re-enable approved for kind K on machine M; waiting for origin renewal. A running watch will renew automatically; otherwise run semon push --receiver URL on the origin.` | stdout; exit 0; access is not yet restored |
| receive enable | Unknown machine or kind | `semon: enable refused; machine M or kind K was not found. Check --machine and --kind, then retry.` | stderr; exit 1 |
| receive enable | Wrong owner or missing terminal | `semon: enable requires the receiver owner's terminal; run this command there.` | stderr; exit 1 |
| receive enable | Custody failure or uncertain commit | `semon: enable could not be confirmed; rerun the same command to check its earlier outcome before retrying.` | stderr; exit 1 |
| receive enable | Kind is not revoked | `semon: kind K is already enabled for machine M; check the kind before retrying.` | stderr; exit 1 |
| receive enable | Revoked enrollment or no valid remaining kind | `semon: enable refused; no valid access remains.` followed by newline and the common recovery block below | stderr; exit 1; record nothing; origin steps run on origin machine |
| disconnect | Confirmation prompt | `Disconnect receiver HOST on this machine? Type yes to confirm: ` | terminal; wait for input |
| disconnect | Cancelled input | `Disconnect cancelled; nothing changed.` | terminal; exit 0 |
| disconnect | Durable success, including already disconnected | `Disconnected receiver HOST on this machine. History, keys and audits kept. Receiver access was not revoked.` | stdout; exit 0 |
| disconnect | Unknown receiver | `semon: receiver HOST was not found; use the HTTPS URL given to connect as --receiver URL, then retry.` | stderr; exit 1 |
| disconnect | Wrong owner or missing terminal | `semon: disconnect requires the local owner's terminal; run this command there.` | stderr; exit 1 |
| disconnect | Custody failure or uncertain commit | `semon: disconnect could not be confirmed; rerun the same command to check its earlier outcome before retrying.` | stderr; exit 1 |
| push / --watch | 30-day boundary, no attempt under way | `Receiver HOST renewal is due soon; D days remain.` | stderr; no exit change |
| push / --watch | Attempt actually under way, no failure in this cycle | `Receiver HOST credentials are renewing automatically; D days remain.` | stderr; no exit change |
| --watch | 14-day or 7-day crossing, or daily reminder at that severity | `Automatic renewal has not completed; D days remain. Check receiver HOST is reachable; renewal will retry.` | stderr; no exit change |
| --watch | First renewal error/refusal, changed failure outcome, or daily failure reminder | `Could not renew receiver HOST credentials; D days remain. Check the receiver is reachable; renewal will retry.` | stderr; valid traffic continues; no exit change |
| one-shot push | Renewal error/refusal or 14/7-day escalation | `Could not renew receiver HOST credentials; D days remain. Check the receiver is reachable, then run semon push --receiver URL again.` | stderr; valid traffic continues; renewal alone changes no exit status |
| push / --watch | Existing credential refused by application traffic | `Receiver HOST refused access.` followed by newline and the common recovery block below | stderr; stop channel; exit 1; refusal does not prove revocation |
| push / --watch | Credential expired | `Receiver HOST credentials expired.` followed by newline and the common recovery block below | stderr; stop channel; exit 1; NEW enrollment ID |
| push / --watch | Client certificate expired | `Receiver HOST client certificate expired.` followed by newline and the common recovery block below | stderr; stop channel; exit 1; NEW enrollment ID |
| Recovery block | Appended to enable-no-authority and the three push recovery first lines | `To restore access, pair again: on the receiver, run semon receive enroll --machine M with its listener/TLS options and confirm replacement if prompted; on the origin, run semon connect --to URL, enter the code, and approve replacement.` | stderr; public_tls uses code; pinned_invitation substitutes invitation |
| push / --watch | Durable installation and first-use confirmation | `Receiver HOST credentials renewed.` | stderr; no exit change; generation/kind details diagnostic only |
| connect | Replacement line in combined approval summary | `This replaces the previous enrollment for receiver HOST. Its files and history will be kept.` | terminal; shown before B4 approval; no extra approval step |
| receiver startup / owner command | Unsupported pre-renewal registry | `Receiver state uses an unsupported format. Keep DIR unchanged; use a new empty receiver directory and pair each origin again. See docs/design/asi-enrollment.md#offline-machines-and-missed-renewal.` | stderr; exit 1; [recovery section](#offline-machines-and-missed-renewal); never used for capacity |
| Receiver admission | Actual live/tombstone capacity exhausted | `Receiver state is full. Keep DIR unchanged; use a new empty receiver directory and pair each origin again. See docs/design/asi-enrollment.md#offline-machines-and-missed-renewal.` | local stderr; exit 1; wire remains generic |
| receive enroll | Address change confirmation | `Change this receiver's address from OLD to NEW? Existing origins must pair again. Type yes to confirm: ` | owner terminal; literal yes; before any code |
| receive enroll | Address change declined or EOF | `Address change cancelled; nothing changed.` | terminal; exit 0; preserve custody |
| receive enroll | Endpoint mismatch without confirmed change | `Receiver directory records endpoint URL. To change address, run semon receive enroll --advertise NEW with its listener/TLS options and confirm the address change.` | stderr; exit 1; preserve custody |
| Receiver new authority | Audit unavailable | `Receiver audit is unavailable. Keep DIR unchanged; restore access to its audit storage before enrolling or renewing.` | local stderr; exit 1; wire remains generic; revoke/cutover still work |

For watch, evaluate deadlines independently of file events, at least once per
minute, using an injected clock/scheduler. Keep automatic retry attempts at most
once per minute and within receiver admission limits. Notices follow a separate
schedule: report the FIRST failure immediately, a changed outcome/severity, each
30/14/7-day crossing, and at most one reminder per 24 hours while unchanged. If
starting late, use only the highest reached severity. Suppress redundant due,
attempt-start and failure lines within one cycle; failure/severity wins. A running
watch never tells the operator to run push. At 30 days say due soon unless an
attempt is actually under way. Attempt-start never resets suppression and never
counts as a changed outcome: alternating starts and the same failure cannot flood
the log. Reset notices only after success. One-shot push emits
one combined outcome per cycle. Tests advance injected time; no real-time sleeps
(#623).

Normal renewal never rotates the assertion issuer, receiver server/root identity
or cache master key. Exceptional `semon receive rotate --identity` requires a
reviewed authenticated administrative trust update on each origin, not an ordinary
read request or routine renewal. Specify and count that operation's commands,
transfers and approvals before its implementation; no four-action maintenance
claim. Issuer rotation accepts at most two pinned key IDs in a bounded explicit
overlap, switches signing and removes the old key. Offline origins fail closed
after cutover. Compromised trust cannot authenticate its own repair: revoke it and
use a fresh independently trusted pairing and new enrollment ID.

Exceptional `semon receive rotate --cache-key` plus confirmation generates a
protected replacement and explicitly invalidates disposable cache entries; it
changes no machine identity and deletes no facts/audits. #594 owns assertion
validity, nonce bounds and encoding; issuer overlap cannot extend assertion expiry.

The receiver checks current enrollment, kind and credential revocation on every
request, including keep-alive connections. TLS handshake success is not cached
authorization. In-flight work retains a secret-free credential authority handle,
bound to its credential ID, enrollment, kind, generation and authenticated client
pin. Each release re-reads current authority and validity, including any retiring
generation cutover scheduled after admission. Unrelated registry writes and
renewals preserve that work while its own credential remains authorized. Revocation immediately removes authority at the next authorization/
release decision and signals all in-flight poll/response/delivery hooks. Recheck
before dispatch and release; #594 integrates cancellation and stops active delivery
within its declared deadlines. No certificate-expiry wait, new bytes under stale
authority or queued work surviving revocation. Already delivered plaintext cannot
be recalled; disconnected origins cannot receive instantaneous remote updates.

| Operation | Contract |
| --- | --- |
| `semon disconnect --receiver URL` | Origin command plus confirmation disables local enrollment/channel and receiver/issuer trust in active use; cancel active delivery and stop new publication. Preserve local history, analytics, keys and audits. This alone is not receiver-side revocation. |
| `semon receive revoke --machine <name>` | Authenticated receiver admin command plus confirmation revokes enrollment, all kinds/generations, associated grants and in-flight hooks; invalidate affected cache entries under cache rules. Preserve admitted facts/audits and report retired/disconnected coverage. Requires no origin contact. |
| Lost origin | Receiver revokes immediately; replacement pairs with a new enrollment ID. No automatic namespace/claim transfer or erasure of the lost disk. |
| Lost/compromised receiver | Recover through fresh independently trusted pairing with a new enrollment ID. Connect replaces existing origin custody under the single B4 approval and retains its old files; no separate disconnect is required. Receiver replacement confirmation applies if a usable predecessor remains there. Handle offline origins locally; disconnect remains a standalone way to stop local use. |
| Audit full/invalid | Proposed `semon receive audit status`, then `semon receive audit roll --preserve-to /private/archive` plus confirmation preserves exact journal custody before a fresh private directory is used. Reads stop; no automatic pruning/repair. Origin requires an equivalent local audit operation specified by its owner. |

Bilateral removal uses both disconnect and revoke, with their confirmations. New
sources require explicit local selection updates; renaming a machine never renames
its namespace. Permanent evidence loss retains frozen facts and reports unavailable
bodies; recovery cannot reconstruct missing source custody.

## Shared listener authentication and dispatch

The implementation status above covers listener hooks and receiver-side public_tls
provisioning/token cutover and mirror-only origin connect. PR4.1–4.3 implement the
renewal contract above; #594 owns body handlers
and in-flight cancellation; slice 5 owns analytics handlers.

Extend rustls with optional client-certificate presentation/verification: analytics
and mirror origins without certificates may complete TLS where their kind permits.
Presented certificates must validate against configured client trust and match the
enrolled tagged SPKI/leaf pin. Preserve verified peer evidence; route-level
`client_cert_required` is independent. Body routes refuse missing, unverified,
foreign or revoked client identity.

One logical interface is shared with #594 (not an existing Rust API):

```text
authenticated_enrollment(conn, request_credential, required_kind)
  -> { enrollment_id, machine, namespace, origin_incarnation,
       index_incarnation, publisher_claim_epoch, kinds, verified_client_identity }
```

TLS contributes verified evidence, the request its kind credential; scope comes
only from the current registry. Every request repeats the decision. Route/body
fields cannot choose another namespace/enrollment or retain authority after
revocation. Provisioning has a separate bounded dispatcher and cannot bypass this
interface for application routes or install keys through a body request.

Dispatch order: bounded request-head/framing checks, route/kind selection, current
credential/enrollment/certificate verification, resource admission/size checks,
then handler/body consumption. No `100 Continue`, staging, body resolution or queue
insertion precedes authority and capacity checks. Unknown routes/kinds refuse;
handlers cannot opt out. Analytics and body require HTTPS even if loopback mirror
HTTP is supported. `/v1/analytics/` is distinct from Hub `/v1/asi/`; `/v1/body/`
carries only #594 polls/responses. Body frames never enter analytics objects/spools.

### Capacity and admission

All classes charge operator-configurable `max_connections`. Polls hold threads/
slots on this blocking listener. Admission accounts for class and enrollment as
well as unauthenticated per-IP protection; NAT peers are not one machine. Idle
keep-alive connections still consume total capacity and remain deadline-bound.

This is the single capacity table. Existing mirror limits remain owned by
`serve.rs` and the mirror protocol; pairing has the additional reviewed aggregate
bounds above and cannot consume unbounded pending state or listener capacity.

| Resource / class | Limit and status |
| --- | --- |
| Total connection slots | Existing default 32; 4a exposes operator configuration |
| Unauthenticated connections per IP / IPv6 /64 | Existing default 4; retain pre-authentication enforcement |
| Body poll class | Interim: at most 1 outstanding poll per enrollment and `floor(max_connections × poll_share)` globally; configurable share at most 50%, default 50% |
| Publication/upload class | Non-poll and total slots; preserve capacity outside poll quota; slice 5 owns object/body bounds |
| Body response-frame class | Non-poll and total slots; cannot wait behind long-polls; #594 owns framing/read deadlines |
| Mirror class | Non-poll and total slots; existing 6 MiB request cap and socket deadlines |
| Pending body requests | Interim: at most 4 queued per enrollment, expiring metadata/control only under #594 deadlines |
| Poll duration / control envelope | #594 proposal confirms 20 s / 16 KiB; #594 owns these bounds |
| Complete body page/field envelope | D3 ceiling 256 KiB including metadata; #594 owns frame accounting and narrower lease/read limits |

These are interim thread-per-connection limits, set by the owner on 2026-11-11
until #636 replaces the listener. Default 32 slots and 50% poll share yield about
16 concurrent long-polls; remaining slots serve publication, responses and mirror.
This is not a product ceiling or fleet target. Raising configuration costs threads,
file descriptors and memory and needs CI resource evidence. Scale beyond this
listener belongs to #636, not 4a.

Poll quota is a ceiling, never permission to consume response/publication slots.
Non-poll classes have separate accounting; choose their precise reservation/fairness
split with 4a resource tests and #594 integration. Never queue unbounded work or
block all responses behind polls. Excess admitted HTTP requests return `503` with
a closed reason (`connection_capacity`, `poll_capacity`, `pending_queue_full`) and
close. Pre-HTTP/failed-TLS connections may close without HTTP; do not allocate a
worker just to send 503. Expiry/revocation discards pending work with explicit
unavailable outcomes, never empty success. #594 owns correlation and retryability.

## Claims, epochs and heads

Enrollment binds authenticated machine, namespace and origin/index incarnation as
an authority input, not a claim activation transaction. Slice 4b follows the
[shared #555 mechanism](asi-store.md#hub-export-phase-3): pending publisher/epoch,
validated complete replacement, expected-parent commit and atomic head visibility.
Prior complete heads remain until activation; delayed/revoked publishers cannot
overwrite newer epochs. 4a represents unassigned claims and cannot advance heads.
Assigned epochs must match current claim binding, not client assertions. Body
scope does not activate claims. Incarnation change needs explicit rebinding/4b
reconciliation, never silent enrollment reuse, publisher transfer or empty reset.
Replica custody/authority remains #596.

## Interfaces #594 targets

- One `authenticated_enrollment` and provisioned versioned scope, current generation/
  revocation state and issuer pins installed by confirmed provisioning.
- `body_channel` authentication plus verified enrolled client identity, with fresh
  authority checks before dispatch/release and in-flight revocation/cancel hooks.
- Shared HTTPS HTTP/1.1 registration/admission and bounded expiring per-enrollment
  queues; no second body enrollment or manual credential import.
- Protected current issuer-key access for the signer, separate from registry/cache;
  origin receives public pins only through authenticated provisioning/maintenance.
- Explicit generated cache key/directory, authenticated owner/exact ACL inputs and
  separate receiver/origin audit-directory outputs for the storage-owned libraries.

#594 integrates origin authorization/audit and body policy, assertions, payloads,
channel binding, exact rendering/mappings, cache admission, leases and deadlines.
#635 supplies its assertion/audit libraries. Enrollment grants neither end-user
permission nor D2 online authority, including cache hits. Scan-backed P1 generation,
producer lifecycle, 4b claims/heads and slice 5 publication remain the dependencies
listed above. PR #632's proposed names are not shipped interfaces.

## Verification obligations for slice 4a

CI must verify the implementation, not infer safety from this docs PR:

- Successful accepted public_tls code pairing and refused wrong-code/key/endpoint/
  pin/version/role/scope or failed confirmation; no TOFU, general TLS bypass,
  redirects or secret leaks. Private-root belongs to PR3b; PAKE remains a separate
  reviewed track, not a gate claiming it ships in PR4.
- Tight per-session and receiver-wide guessing, issuance/pending-state and recovery
  bounds across addresses/connections/retries; reissue cannot reset aggregate bounds.
- Concurrent race, replay, expiry, cancellation, consumed-session retry, restart and
  lost acknowledgement produce at most one atomic activation and no partial grants.
- Cross-kind, namespace/incarnation/epoch and owner isolation; machine credentials
  never authenticate users; source consent remains separate and cannot widen through
  renewal. Body missing/wrong certificates refuse; analytics optionality is preserved.
- Automatic renewal under valid trust, explicit cutover/old-generation retirement,
  terminal enrollment/kind revocation on kept-alive connections and in-flight hooks,
  plus fresh-ID recovery without resurrection, scope expansion or claim transfer.
- Custody preservation: private origin key never leaves origin, separate secret/
  cache/ACL/audit state, unsupported/insecure stores refuse, old `tokens` stays
  untouched, no facts/source/audit deletion. Stale snapshots refuse; on-disk stale
  backup detection remains the explicitly deferred custody boundary above.
- Concurrent poll/upload/response/mirror exhaustion remains bounded with fair
  response capacity and explicit refusal; required downstream hooks are exercised.

### PR4 CI obligations

- Refuse wrong key, wrong/missing bearer, missing/foreign/unverified required
  certificate, revoked/expired/retired authority and every scope-widening request.
  A stolen key or bearer alone never renews; generic errors reveal no local class.
- Concurrent begin/finish, replay, cancel, restart, commit-uncertain writes and lost
  acknowledgements leave one committed target generation set and at most two
  reserved deliveries. Recovery returns identical secrets/certificate; no N+2 retry.
- Separate receipt authentication/output sets when enabling a revoked kind; recover
  with exactly the old authentication set and its expiry/cutover minimum. Enabled
  kinds have no retiring slot; later revoke of any receipt kind ends all recovery.
- Check 90/30-day boundaries, 24-hour overlap capped at old expiry, exact expiry,
  per-kind first-use cutover and unchanged SPKI pin with reissued leaf validity.
  Exercise atomic authorize-and-cutover, committed-state PR4.1 handles, lock
  contention/uncertain commits, concurrent N+1 requests and no subsequent writes.
- Revoke current/retiring credentials during polls, responses and delivery on
  kept-alive connections. The PR4.1 handle stops affected work and remains current
  across unrelated enrollment/kind/owner revisions. Recheck before release.
- Exercise cross-kind, cross-enrollment and owner isolation, explicit one-use enable,
  renewal-versus-revoke races, no revoked-generation resurrection and terminal
  whole-enrollment revocation. Refuse last-kind per-kind revoke and enable without
  valid remaining authority before writing. Enable after begin stays pending;
  cancellation/revoke of a bound authorization makes finish refuse.
- Check normalized endpoint mismatch refusal on every route, in-flight handles and
  first-use cutover. Address change uses one registry commit; crashes before/after
  commit preserve a coherent endpoint and leave old enrollments unchanged. Decline
  changes nothing; old-endpoint machine names are freed. Enable's push line uses
  the enrolled endpoint; fresh-pairing recovery uses the current endpoint after
  change/compaction. Test default-port/IPv6 normalization and origin custody names
  equaling SHA-256 of the enrolled endpoint. Plain receive loads the registry
  endpoint without a new --advertise argument.
- Rerun each revoke/enable/disconnect command after durable commit and report the
  existing end state without a prompt or repeated authorization. After replacement,
  rerun prompts for the replacement and declining changes nothing. Exercise crashes
  and uncertain commits at every step, including the fresh custody read before
  prompting. Check tombstone lookup at the maximum validated machine-name length.
- Preserve old valid origin custody across failed staging, crash and disconnect;
  admit no partial/mixed generation or late renewal after disconnect. Preserve keys,
  consent, facts, history and audits; old registry format refuses without migration.
- Bound renew-route framing, TLS version, deadline, shared connection admission,
  pending count, per-enrollment rate and receipt/storage capacity, including restart
  and hostile/unknown IDs. Admission refusal allocates no unbounded work. Verify
  per-record live-state budgets, ended-receipt pruning, 256 KiB headroom and
  successful revoke/cutover at the admission boundary even with unavailable audit.
  Exercise audit append/fsync before compaction, every crash point, repeated event
  IDs and idempotent restart. Tombstones preserve no-resurrection, pin isolation and
  generation monotonicity; rollback never revives compacted authority. Audit-full
  state blocks compaction/new authority while preserving evidence and bounded state.
  Ordinary receive serves renewal without pairing authorization, machine selection,
  controller mutex or terminal, sharing the same admission budget.
- Complete expired/revoked and active replacement with the same machine/endpoint.
  Usable predecessors require receiver confirmation before code display; rejection,
  EOF, expiry and precommit crashes leave old authority intact. Activation atomically
  revokes the exact predecessor. No prompt for unusable predecessor. Verify 4/5-action
  replacement and 2/3-action enable counts, single B4 approval, receiver-commit versus
  delivery/origin-swap crash recovery and preserved `replaced-<enrollment-id>/` files.
- Authenticate the `Semon-Renewal: due` header trigger and withhold it from refused
  callers. Header and local-window triggers obey one retry budget. Every generic
  renewal error preserves valid application traffic; only application refusal or
  expiry gives re-pair guidance and exits. Exercise this with one-shot and watch.
- Exercise exact operator strings, confirmation/cancellation, 30/14/7-day escalation,
  watch without source events, first-failure immediacy, changed outcomes/severity,
  threshold crossings, daily reminder ceiling and suppressed duplicate due/failure
  lines. Attempt-start never resets suppression or counts as a changed outcome;
  alternating start/same-failure cycles stay quiet. A running watch never advises
  another push. Test --receiver URL resolution through the SAME pairing endpoint
  normalization on connect/push/disconnect, including default ports and IPv6,
  without hash aliases or DNS alias guessing. Notices never show userinfo/query;
  receive enable prints the correct semon push --receiver URL for the enrolled
  advertised endpoint. Distinguish client-certificate expiry and unsupported-format
  versus capacity guidance; all recovery strings use code/invitation consistently.
  Inject wall/monotonic clocks and scheduler advancement, clamp backward wall time,
  and use no real-time sleeps (#623), including deadline and race tests.

Protocol/library suitability, technical safety review and PM/owner library approval
are required before dependencies/implementation merge. #594 confirms read/lease/
cancellation deadlines; 4a resource tests settle non-poll reservations. The review
records exact pairing/renewal parameters here before implementation merges. Full
publication, body-read and four-action journey acceptance belong to the owning
integration slices; docs-only CI proves none of those runtime guarantees.

Origin PR3.2c status (part of #593): `connect --to URL [--machine M]` uses
public-CA TLS 1.3 and a non-echoing controlling terminal, followed by explicit
mirror-only trust/source approval. The shipped push selector uses the internal
endpoint hash; PR4.3 replaces that operator interface with
`push [--receiver URL] [--watch]` under the contract above. Push consumes
stored enrollment; the superseded `--to`/`--token-file` push CLI is removed.
Custody is `$XDG_CONFIG_HOME/semon/origin/<sha256-canonical-endpoint>/` (default
`~/.config/semon/origin/`): `enrollment.json` signed by `origin-key.pk8`,
`mirror-push.secret`, and separately bound `source-consent.json`. Directories are
0700 and regular files 0600, owned by the local user; fd-relative no-follow opens
refuse links. A private `stage-<random>/` is synced before its atomic publication;
only the final directory is admitted. Interrupted staging is retained, never
silently deleted or loaded. No credential is written before receiver confirmation
and approval delivery. `install.lock` serializes publication. Receiver selection
uses the full endpoint hash; an omitted name requires one enrolled receiver.
`connect::producer_admission` calls the explicit library initializer for requested
analytics_publish/body_channel kinds before pairing or network activity. It passes
no expected binding until enrollment supplies one. Initialization creates or reuses
the current local producer binding without publication, sharing, rebuild or reset.
Successful initialization still refuses with `analytics/body enrollment available
after producer admission and activation integration lands`, because the receiver
still refuses those kinds. Mirror-only connect skips initialization and works
without a namespace. See the [producer lifecycle](asi-producer-lifecycle.md) for
identity persistence, pending publication and refusal behavior.

Public-TLS origin custody records the canonical endpoint and public CA trust
profile, not a receiver certificate fingerprint. The code-bound ActivationInput
commitment authenticates receiver_id; issuer pins apply only to future admitted
analytics/body bindings. Custody files have a shared 4 MiB encoded ceiling. Every
file is size-validated before publication; larger source lists require narrowing.
Relative XDG_CONFIG_HOME is ignored; an absolute HOME fallback is required.
