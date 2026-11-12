# ASI enrollment and shared listener

Status: **current enrollment/provisioning design contract**, reconciled 2026-11-12
with the [accepted operator journey](asi-operator-journey.md) and owner decisions.
Implementation, protocol/library suitability and independent technical safety
review remain pending. This document does not lift PM-owned implementation holds
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
keep-alive enrollment binding and revision handles for in-flight reauthorization.
`serve_auth.rs` exposes `RegistryAuthenticator`, `AuthenticatedEnrollment` and
`RevisionHandle`. The bounded registry reader and revision handle inspect registry
custody only; missing pairing-gate custody does not disable existing authentication.
Private owned mode-0600 `DIR/client-trust.pem` supplies PEM client roots; absence
disables client-certificate presentation. Reserved analytics/body routes refuse
closed until their owning integrations land.
[`tokens.rs`](../../crates/semon-push/src/tokens.rs) stores `NAME SHA256(token)`
lines in `DIR/tokens`; `Tokens::authorize` supplies a machine identity. The
[mirror protocol](../mirror-protocol.md) and push client implement supported Push,
not the enrollment, client-certificate identity or remote body read defined here.
#630/#631 and #635 supply libraries, not integrated onboarding or body service.

The receiver registry and kind-aware credential custody are implemented as Unix
libraries in `semon-push::enrollment` and `semon-push::credentials`, consumed by
the listener's registry authenticator. Enrollment creation and pairing remain
unwired from CLI and default provisioning paths. Explicit library
initialization creates `DIR/enrollment/registry.json` (format 1) and
`DIR/enrollment/registry.lock`, relative to the receiver state directory `DIR`.
One atomic registry document contains enrollment bindings, generation-scoped
certificate pins and kind-scoped bearer hashes; raw bearer secrets are never
serialized. The library also exposes legacy `DIR/tokens` refusal without importing
or modifying it. The listener now authenticates only through the current registry;
legacy `DIR/tokens` custody refuses startup and every request with explicit
re-pair/re-issue guidance, preserving the file and all retained history. A connection
is bound to one enrollment and cannot switch machines between requests. The old
flat-token issue/revoke/list commands and reader are retired. Renewal and provisioning route
integration remain unimplemented. The `public_tls`
pairing core is landed unwired in `crates/semon-push/src/pairing.rs` with deterministic
coverage in `pairing_tests.rs`; PR 3.2 will wire provisioning routes, CLI, outputs
and the tokens cutover, including receiver assertion-issuer signing-key custody;
PR 3.1 provisions no signing key. `Core::registry()` exposes a fresh registry with
all fields for the later `origin_read::EnrollmentScope` adapter. Revocation comes
from a separate CLI process editing the registry and is observed through its
revision; pairing holds `pairing.lock`, acquiring `registry.lock` only for short
transactions so that external updates remain possible. Registry format 1 now includes immutable nonsecret pairing
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
remain as terminal revocation history. Per-kind re-enable is not implemented yet;
its fresh-credential transition arrives with the renewal/revocation PR. Existing
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

These are proposed commands to implement, not shipped commands. After installation,
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

The transcript binds receiver identity, exact advertised endpoint/TLS pin,
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
public key. This revises the earlier proposed open-time scope commitment: the
origin first needs those issuer pins to construct its full ActivationInput.
The origin commits the full input in `confirm`'s scope commitment; the code-bound
MACs authenticate the returned issuer metadata through that commitment. The code is delivered once only to the
receiver's trusted local terminal. It has no request or response field.
`confirm` carries version, session ID, 65-byte origin public key, 32-byte origin
nonce, 32-byte scope commitment and 32-byte origin MAC. The receiver reconstructs
the existing pairing.rs transcript with its own exporter/nonce/endpoint. Only a
verified origin MAC permits a `confirmed` response (version and receiver MAC).
`approve` carries version, session ID, the full strict ActivationInput and 64-byte
proof. Recompute input.commitment() and require transcript.scope equality. Apply
receiver policy: named authorized machine, valid binding, offered kinds only,
empty certificate pins and no client-cert requirement for mirror. Only mirror_push activates in this slice; analytics_publish refuses until origin connect / independently validated producer admission lands. Peer-supplied namespace/incarnation bindings cannot authorize activation.
Body-channel enrollment is available after client-certificate issuance lands.
The **initial credential validity ceiling** is 90 days (7,776,000 seconds) from
activation now, exclusive lower bound (`valid_until > now`) and inclusive upper
bound. This is a Manager-chosen parameter for PM review; renewal under valid
enrolled trust arrives in PR4, which may change this value. `cancel` carries
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
revocation is required; recovery cannot revive authority.

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
evidence. Restricted sources remain local unless a separate explicit named-reader
stream-through policy permits them.

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
| Per-kind `revoked_at` | Independent timestamped kind revocation; whole-enrollment revocation overrides all kinds |
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
explicit server root/pins, machine/namespace/incarnations, assigned-or-unassigned
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
| Receiver private registry | Enrollment, kind-scoped credential hashes, activation/consumption, generations and revocation history; no persisted raw bearer secret |
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
is an unwired Unix library. Under `DIR`, mode-0700 directories are `enrollment/`
(existing registry format 1), `issuer-secrets/`, `cache-secrets/`, `body-cache/`, `acl/` and
`receiver-audit/`. Mode-0600 files are `receiver.json` (strict JSON format 1:
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
revocation grant calls, CLI, origin custody and tokens cutover; #594 wires body
service and host-custody decisions. No listener, CLI or serving path uses this
library yet.

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
there is no compatibility reader, migration or dual path for `NAME SHA256(token)`.
Finding old `DIR/tokens` refuses receiver startup with re-issue guidance and leaves
the file untouched. Re-issue kind credentials, explicitly enroll/configure machines
and update supported Push clients/token inputs together. 4a updates the owning CLI,
mirror docs and tests; no compatibility alias conceals the change. Refusal never
authorizes deleting old state or history.

### Rotation and revocation

Transport credentials/certificates auto-renew under still-valid explicitly enrolled
trust, with zero routine operator actions. Require proof of current enrollment/key
authority, keep scope/source consent and private-key custody, confirm replacement
and retire old generations at the reviewed cutover. Review validity, overlap and
cutover mechanics in 4a before implementation merges. No periodic re-pairing or
routine enrollment-expiry prompt. If valid renewal authority cannot be proven,
fail closed and use explicit fresh recovery. Renewal cannot clear a whole/kind
revocation, expand scope or silently transfer publisher authority. A revoked kind
generation remains revoked; re-enabling the kind requires explicit authorization
and fresh credentials. A revoked enrollment is never reactivated.

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
authorization. Revocation immediately removes authority at the next authorization/
release decision and signals all in-flight poll/response/delivery hooks. Recheck
before dispatch and release; #594 integrates cancellation and stops active delivery
within its declared deadlines. No certificate-expiry wait, new bytes under stale
authority or queued work surviving revocation. Already delivered plaintext cannot
be recalled; disconnected origins cannot receive instantaneous remote updates.

| Operation | Contract |
| --- | --- |
| `semon disconnect --receiver <name>` | Origin command plus confirmation disables local enrollment/channel and receiver/issuer trust in active use; cancel active delivery and stop new publication. Preserve local history, analytics, keys and audits. This alone is not receiver-side revocation. |
| `semon receive revoke --machine <name>` | Authenticated receiver admin command plus confirmation revokes enrollment, all kinds/generations, associated grants and in-flight hooks; invalidate affected cache entries under cache rules. Preserve admitted facts/audits and report retired/disconnected coverage. Requires no origin contact. |
| Lost origin | Receiver revokes immediately; replacement pairs with a new enrollment ID. No automatic namespace/claim transfer or erasure of the lost disk. |
| Lost/compromised receiver | Each surviving origin disconnects before reconnecting; explicitly revoke affected authority and repeat fresh pairing for recovery. An offline origin must be handled locally. |
| Audit full/invalid | Proposed `semon receive audit status`, then `semon receive audit roll --preserve-to /private/archive` plus confirmation preserves exact journal custody before a fresh private directory is used. Reads stop; no automatic pruning/repair. Origin requires an equivalent local audit operation specified by its owner. |

Bilateral removal uses both disconnect and revoke, with their confirmations. New
sources require explicit local selection updates; renaming a machine never renames
its namespace. Permanent evidence loss retains frozen facts and reports unavailable
bodies; recovery cannot reconstruct missing source custody.

## Shared listener authentication and dispatch

The implementation status above covers PR2's listener hooks and PR 3.1's unwired
pairing core. PR 3.2 owns provisioning and token cutover; #594 owns body handlers
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

- Successful PAKE pairing (or fallback only after the stated review outcome), and
  refused wrong-code/key/endpoint/pin/version/role/scope or failed confirmation;
  private-root bootstrap with no TOFU, general TLS bypass, redirects or secret leaks.
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
  untouched, stale backups cannot revive authority, no facts/source/audit deletion.
- Concurrent poll/upload/response/mirror exhaustion remains bounded with fair
  response capacity and explicit refusal; required downstream hooks are exercised.

Protocol/library suitability, technical safety review and PM/owner library approval
are required before dependencies/implementation merge. #594 confirms read/lease/
cancellation deadlines; 4a resource tests settle non-poll reservations. The review
records exact pairing/renewal parameters here before implementation merges. Full
publication, body-read and four-action journey acceptance belong to the owning
integration slices; docs-only CI proves none of those runtime guarantees.
