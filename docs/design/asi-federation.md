# Federated ASI custody and on-demand viewing

Status: **accepted architecture**, 2026-11-09, following the user's D1–D8
answers through the PM's structured decision process on #587. This supersedes
option B as the default and is reconciled with [ASI direction](asi-direction.md),
[store](asi-store.md) and [query](asi-query.md). Acceptance is a design decision,
not evidence that these safeguards have shipped. This docs change grants no merge,
deployment or data-deletion authority. The PM applies the lane plan after the
user approves the merge; this document does not itself redirect active lanes.

## Accepted direction and scope

Lead with the user's design: keep native raw history at its origin, compute
analytics there, and push governed, body-free facts to the operator's self-hosted
receiver. Fetch bodies only for an authorized read, through a bounded receiver
cache or a stream-through path. Hub receives analytics only; it never terminates
the body channel, holds cache keys, caches bodies or serves transcripts.

Analytics remains Semon's core. Standalone, read-only raw-log viewing is its
foundation, and analytics evidence links into it. Semon works without T3 installed
or reachable. Federation changes the availability of that foundation: an offline
or lost origin can leave useful analytics with unavailable evidence. It does not
promise that all retained analytics remain inspectable or re-derivable forever.

The user's motivating requirements are:

> “raw logs clearly expose data. but how do we safely teleport it for both
> viewer and analytics”
>
> “something similar to data mesh or federated data governance”
>
> “raw session logs only pull from original machines whenever needed … for
> analytics, we should pre-compute to allow cross-machine analytics … local
> (pre-)classification before reaching to receiver”

Adopt origin ownership, a shared typed data contract and origin-evaluated policy.
Do not build a general distributed query engine or a remote execution platform.
The new transport carries enumerated read operations and analytics publication;
it cannot start a harness, run a command, change origin policy or manipulate files.

First delivery: origin analytics publication, origin-local archive and viewer,
and online-authorized receiver pulls with encrypted cache. Local search and remote
view ship first; receiver search stays unavailable until federated coverage and
audit exist. Offline body grants are excluded. An explicit opt-in replica mode
retains supported mirror custody under the security requirements below.

## Compared with option B

Option B was the 2026-11-09 amendment to [#549][roadmap], specified by
[#553][spec-pr]. It replaced the origin-derived metadata approach in
[#550][old-decision] with central derivation over receiver-held redacted Raw.
The accepted federation decision now restores origin derivation with demand-driven
body access, bounded caching and computational governance. The comparison below
records the superseded default, not a second current contract. The old wire
format and retired Encrypted Relay are not restored.

| Concern | Accepted federated default | Superseded option B default |
| --- | --- | --- |
| Raw custody | Origin native logs and local archive; receiver cache is expendable | Origin plus receiver immutable redacted mirror/archive |
| Analytics producer | Origin SQLite projection, then local analytics writer | Receiver SQLite projection and analytics writer; local pipeline when standalone |
| Remote bodies | Explicit bounded read; policy may prohibit disk cache | Already resident on receiver |
| Offline origin | Latest admitted analytics remain queryable; default body read unavailable | Receiver can view held raw and re-derive from it |
| Rules or raw-dependent transform change | Origin re-derives when it returns and has complete raw | Receiver can re-derive its complete retained inputs centrally |
| Lost origin | Facts remain at their last versions; evidence may be permanently lost | Receiver's redacted copy can preserve evidence and re-derivation, subject to redaction and retention |
| Privacy exposure | Smaller resident body set on receiver; metadata still sensitive | Larger receiver exposure and indefinite raw retention by default |
| Cost | Origin compute, channel and policy distribution, version/coverage management | Receiver compute and raw storage; simpler centralized refresh |
| Pricing correction | Origin can reprice stored billing inputs without raw; optional central valuation is a separate decision | Receiver can reprice its stored billing inputs without origin availability |
| Hub | Body-free analytics and categories only | Same boundary |

The benefit is reduced off-machine body custody, not end-to-end secrecy from a
receiver that renders plaintext. A receiver operator with process/root access can
copy every body they are authorized to receive. TTL, audit and encryption at rest
cannot force that operator to forget a copy. Use local-only sources when that
trust is unacceptable.

## Data ownership and the two publication paths

| Location | Durable data | Derived/temporary data | Authority |
| --- | --- | --- | --- |
| Origin | Native raw; policy-controlled local archive; origin-created labels/adjudications | SQLite index/activity/StepFacts, local Parquet analytics, local FTS | Admits source identity, classifies, redacts and authorizes egress |
| Self-hosted receiver | Accepted body-free origin heads; receiver-created human state; body-read audit | Cross-origin query snapshot; encrypted bounded body cache | Authenticates users, enforces namespace grants and origin policy, validates imports |
| Hub tenant | Accepted body-free publisher heads and permitted categories | Sandboxed analytics queries | Tenant/workspace authorization and import validation; no body authority |

The receiver's imported facts are durable published evidence, **not disposable
projection caches**: it may be unable to reconstruct them. Eviction of body cache
entries must not delete catalog entries, labels, facts or source/version coverage.

Native namespace/harness/session identity, lineage and exclusive/copied usage
ownership remain intact. A transport connection, a matching filename or a similar
session title does not establish identity. Bind a registered machine identity and
origin incarnation to a namespace under an explicit operator enrollment. Use the
existing native UID contract internally. External source/evidence handles are
opaque mappings, never filesystem paths or the record-anchor filename stem.
See the [proposed enrollment and shared listener contract](asi-enrollment.md) for #593 slice 4a.

### Origin derivation

1. The existing native parser admits complete source ledgers into SQLite. Local
   archive custody preserves consumed prefixes across native pruning and rewrite.
   Archives are owned by the origin, not implied to exist merely because an index
   row exists. Native originals are never deleted by this design.
2. Activity/L0 rules, StepFacts and pricing are derived once in the SQLite activity
   producer. The [#557 one-model agreement][one-model] remains authoritative.
   `semon-asi` owns the input trait; `semon-sessions` implements it using the
   admitted generation in one SQLite read transaction. DuckDB projects those facts;
   it does not independently classify steps or overwrite their costs.
3. The origin analytics writer consumes `silver_change` and publishes complete
   changed-session generations. A partial source loss preserves the last complete
   admitted generation and publishes missing-source coverage. It must not replace
   a multi-source session with a surviving fragment: [#582][i582] tracks that gap.
   The local store retains that generation's complete admitted source witness,
   activity generation, producer/parser and recorded versions, index incarnation
   and known derivation timestamps in a manifest-bound provenance artifact (#593
   slice 1). It survives Pending, source loss, compaction and restart with the
   facts. Local paths remain local-only and are excluded from SQL; this custody
   record is not a transport object or authenticated publisher identity.
4. A distinct export projection applies the policies of **every contributing
   source**, as specified below. Never serialize internal StepFacts or the native
   mirror `Facts` object onto this path. StepFacts contains raw target paths;
   mirror facts contain home paths, process observations and repository mappings.

For a multi-source step, session aggregate, lineage relation or L1 assertion,
compute the intersection of all contributing source grants for the destination,
principal and exported field. Use the highest sensitivity floor, shortest cache
TTL and strongest body/storage restriction. Record the contributing policy-version
vector; a single permissive source cannot authorize another source's contribution.
Unknown contribution provenance refuses export. L1 provider egress must satisfy
the same intersection before inference, not just before exporting its label.

If any contributor prohibits a required field, destination or disclosure of the
row's existence, withhold the entire derived row/assertion. Optional fields may
be omitted only where the typed contract represents `withheld` distinctly from
unknown or zero. Never drop restricted input sources and publish the remainder
as a complete session aggregate. Retain the full local admitted generation;
export privacy-filtered coverage that says the visible cohort is incomplete only
where even that disclosure is permitted. Do not expose withheld counts, source
handles, versions or denominators through coverage when their policies forbid it.
A later policy restriction also withdraws affected exported rows in a versioned
head update; it cannot recall copies already read by a recipient.

A standalone origin uses the same pipeline locally, with no receiver or Hub.
Re-derivation is background producer work scheduled locally; a receiver request
cannot install rules, invoke a classifier or execute a re-derivation job remotely.

### Body-free analytics transport

Reuse #555's **publication mechanism** for origin-to-receiver and receiver-to-Hub:
immutable content-addressed objects, staged validation, expected-parent commits,
namespace publisher claims/epochs and atomic head replacement. Do not copy its
current field policy unchanged: federation adds an earlier egress boundary.
This is one current publication contract, with explicit destination profiles,
not two competing row and partition protocols.

A publication binds:

- authenticated producer identity, allowed namespace and claim epoch;
- schema/producer format version, origin/index incarnation, monotonically ordered
  publication revision and expected parent;
- native session/step identity and an opaque manifest of the complete source
  cohort, source generations and consumed boundaries (no raw paths);
- parser, transform, L0 rules, price fingerprint, policy, detector and redactor
  versions; taxonomy/labeller/model/prompt/input-generation versions for L1;
- per-session computation status, source availability, version coverage,
  observation time, computation time and publication time;
- exact object digests and lengths, live-map changes and explicit withdrawals.

The receiver validates schema, types, enum values, allowed fields, metadata and
namespace membership before atomically advancing a head. Validate decompressed
sizes, row counts and resource budgets in an isolated parser. Uploads and commits
are idempotent; a lost ACK cannot duplicate facts or billing. A changed index
incarnation causes complete namespace reconciliation, not a reset to empty. A
new publisher activates only after a complete valid head; the previous head
remains visible meanwhile. Delayed publishers cannot overwrite a newer epoch.
Missing contact or a partial inventory is never a deletion instruction.

Queries pin a coherent map of per-origin heads. Heads need not have the same
computation time or rules version; the result carries those distinctions. No
atomic global clock or simultaneous cross-origin snapshot is promised.

**D7: plain names and paths are accepted between origin and the operator's
receiver.** Export the typed schema's name/path fields, including repository,
branch, machine, tool/model names and target paths, under the contributor-policy
intersection. Do not substitute an origin-tokenized default. Credential stripping,
field/size validation and source restrictions still apply. This choice knowingly
reveals project/person names, local filesystem structure and cross-source joins
to the receiver; body-free is not privacy-free. It does not authorize commands,
output, snippets, error messages or free-text titles/goals/explanations as metadata.
An explicit allowlisted `name` field is not a place to smuggle arbitrary body text.
Native source handles and evidence links remain opaque; plain analytics paths do
not become remote filesystem authority or URL content. Internal unkeyed path
fingerprints remain non-exported implementation details.

**PM interpretation of the destination boundary, open to user correction:** the
user's plain-name/path choice applies to origin→operator receiver. The already
settled Hub rule remains: all path-class fields, including local repo paths, are
HMAC-tokenized at the Hub export boundary; Hub never gets plain paths or the key.
This records the PM's interpretation rather than claiming the user explicitly
restated the boundary. Origin tokenization is not required for receiver analytics.
Use the existing #555 workspace-key/canonical-path contract at the exporting store,
record token-key and canonicalization versions, and never equate different token
versions. Hub validates tokens; it does not re-HMAC them. Names remain typed and
policy-approved under the Hub export profile. HMAC is pseudonymity: equality and
frequency still leak, and key holders can test guesses.

Receiver-to-Hub export preserves origin provenance, status and versions while
stripping disallowed names and metadata. Hub retains per-tenant publisher epochs
and namespace ownership; a receiver cannot remove another publisher's objects.
Use tenant-scoped object authorization, including digest existence/deduplication
responses; a content hash is not permission. Hub cannot use an evidence handle to
fetch a body. A user may follow an explicitly configured operator destination and
authenticate there; Hub neither proxies the request nor exchanges credentials.

### L0, L1 and governance classification

These are separate products even when they inspect the same local records:

- **Governance:** explicit source classification (`public`, `internal`,
  `restricted`), plus secret/PII detections and an `unknown` state. Source-owner
  tags set a floor; detectors can make the result more restrictive, not override
  an explicit restriction. Unscanned, unsupported or failed scans fail closed for
  remote bodies. Detection is a fallible signal, not proof of safety.
- **L0:** deterministic activity/waste/trigger categories from the sole activity
  producer. Preserve unknown/unclassified, incomplete observations and rules
  versions; an absent label is not a negative observation.
- **L1:** origin-side inference over explicitly selected evidence. Only categorical
  assertions, confidence, taxonomy/labeller versions and opaque exact evidence
  references export. Free-text goals, mechanisms, explanations and adjudication
  reasons are bodies. No prompt or evidence excerpt travels with a categorical
  assertion. A missing evidence link does not erase the label or fabricate proof.

Local execution with a hosted model still sends raw evidence off-machine.
Per-source opt-in names the provider/model, permitted evidence classes and egress
policy; receiver enrollment, analytics export and body-view permission grant none
of it. Prefer local inference for restricted sources. Policy evaluation itself is
deterministic and local; an LLM cannot authorize a read or relax a tag.

## Policy-as-code and audit

A source owner approves a versioned declarative policy bundle at the origin.
An organization may propose a baseline; the origin applies the stricter of that
baseline and local restrictions. The receiver cannot push executable code or
silently relax policy through its read channel. Policy versions are content-bound
identifiers recorded in facts, cache admissions and audits, like
`redactor_version`; an unevaluated version is not accepted as authorization.

The policy contains source selection, sensitivity floors, permitted principals
and groups, namespace/machine scope, analytics fields, body mode, redaction and
detector requirements, byte/rate/concurrency limits, cache TTL/size limits, local
archive retention, optional replica retention and model-provider consent.
Unknown subjects, invalid policy, unresolved source membership and failed identity
checks refuse before body I/O. Receiver policy can narrow an origin grant only.

Every body read is audited at both origin and receiver, including cache hits,
field expansions, streamed ranges and search snippets. Record correlated request
ID, authenticated principal, source handle, generation, permitted range, purpose
class, policy/redactor versions, decision, destination, bytes and time; never the
body, query text, command, native path, credential or snippet. Persist an origin
authorization audit and receiver read-attempt audit before release; append actual
completion/bytes/cancellation afterward. A crash leaves an explicit attempted read,
not a false success. Stream delivery records bounded range receipts, not one
unbounded session audit entry. Bound and protect audit storage separately from
cache retention; audit failure refuses new reads. Audits themselves are sensitive.

**Accepted initial guarantee (D2):** origin must authorize and audit every new body
read, even a cache hit. This gives immediate origin policy checks while connected
and means no newly requested cached body is shown when the origin is offline.
An existing tab can show bytes already delivered, clearly marked disconnected;
Semon cannot revoke information a viewer has already seen.

Offline grants are not part of the accepted design. Eventual audit reconciliation
cannot satisfy the origin's authorization and audit requirement before each read;
a lost machine might never receive the audit. Neither a valid TTL nor replica
custody bypasses the online check. Adding offline grants or delegated receiver-only
body authorization later requires a new explicit user decision.

### Receiver ACL and read-attempt library boundary (#595)

Unix receiver custody lives in `semon-push::receiver_acl` and
`semon-push::receiver_audit`. The caller supplies an authenticated end-user
principal; authentication and metadata discovery integration are outside these
libraries. A push machine token never supplies user authority. Before resolving
metadata, integrations must check the appropriate grant and use a uniform refusal
for all cross-user lookups. Analytics and body reads are distinct permissions.

The operator edits `grants.json` (0600, owned, regular, one link) in a dedicated
owned 0700 directory. There is no ACL management CLI in this slice. The current
JSON format is `{"format":1,"grants":[{"principal":"alice","machine":"laptop",
"namespace":"work","permissions":["analytics","body_read"]}]}`. Principals
are opaque caller-authenticated strings (1–256 bytes, no control characters),
machines use the existing machine-name grammar, and namespaces use 1–128 ASCII
letters/digits/underscores/hyphens. Unknown fields, duplicate scope entries,
duplicate permissions and unsupported formats invalidate the entire document.
Read-at-decision reload makes revocation effective on the next decision. Missing,
invalid, insecure and unreadable files return the same `Denied` value as unknown
principals/scopes and absent permission. A granted decision contains the SHA-256
of canonical JSON `[1, grant]`, with struct field order and sorted permissions;
changing that grant changes its version. It grants no origin or cache authority.

The receiver audit directory is separately configured and must be distinct from
ACL and cache directories; its only members are `audit.lock` and
`attempts.jsonl`. The same descriptor-relative, no-symlink, owner/mode/link checks
as the cache protect its 0600 files in an owned 0700 directory. One writer holds
an exclusive file lock; concurrent callers serialize through a mutex. JSONL
`attempted` records have format 1, an opaque attempt ID, the closed metadata
record, zero bytes and injected epoch time. Before admission or release,
`authorize` fsyncs exactly one binding of matching request ID, exact opaque response
identity, verified origin policy/redactor IDs and origin audit authorization ID.
Receipts and successful or nonzero-byte completion refuse without that binding;
an unbound attempt is not authorization. `receipt` and `completion` events
append actual bytes/ranges/outcomes and time. Each successful append fsyncs before
return; creating the journal also fsyncs its directory. A crash leaves an attempt
without a completion. A torn/invalid journal or any write failure refuses reads;
there is no automatic history repair or deletion.

Audit IDs (including the identity adapter's stable principal ID and policy
versions) are exactly 32 opaque bytes, serialized as 64 lowercase hex characters.
Purpose, decision, destination and outcome are closed enums; ranges are numeric,
half-open exported ranges. No free-form body/query/command/path/credential/snippet
or error field exists. The adapter must map authenticated identities to stable
opaque IDs, never credentials. Origin policy/redactor IDs may be absent while the
channel is absent; an audit record alone cannot authorize body release.

The audit ceiling is 64 MiB (operators may configure a smaller limit), independent
of cache retention. Begin reserves space for one authorization binding, one
completion and at most 16 range
receipts (8 KiB per event), so capacity exhaustion refuses new attempts without
dropping existing records or consuming active attempts' reserved space. Receipts
must be ordered, non-overlapping and within the permitted range. After 16 receipts,
a stream needs a fresh authorization and attempt. Completion records total actual
bytes, including cancellation/failure, and cannot undercount receipts. No journal
rotation or pruning runs automatically. At capacity the operator must stop reads,
preserve the journal under explicit sensitive-history custody, and explicitly
configure a new dedicated audit directory; cache eviction never touches audits.

`semon-push::origin_audit` supplies a separate origin authorization journal in an
explicitly configured owned 0700 directory outside native roots/cache, with 0600
`audit.lock` and `authorizations.jsonl`. Its closed metadata adds opaque enrollment
scope and export-view binding IDs. Begin fsyncs the decision before render/egress
and returns the opaque authorization ID for the receiver binding; completion and
up to 16 receipts use reserved space. Initial directory creation and every event
are fsynced. Invalid/torn/full/unwritable history refuses reads, without repair,
pruning or deletion. Reopening never resumes interrupted authorizations.

#594 must wire origin audit-before-release,
validate issuer/audience/nonce/deadline identity assertions, bind both sides'
current policy versions to the exact response/admission, and wire user
authentication, metadata discovery checks, delivery and cancellation. These
libraries neither define that origin assertion contract nor change
`ReadAdmission`; no remote body route or Hub body access is introduced.

## Outbound-only body channel and read boundary

See the proposed [body-channel coordination note](asi-body-channel.md) for #593/#594 transport alignment.

The origin opens a mutually authenticated TLS connection to its enrolled
self-hosted receiver over HTTPS (for example a bounded HTTP/2 stream). It opens no
inbound listening port. NAT traversal therefore requires no inbound origin route.
Hub is not a rendezvous server or fallback endpoint. Bind enrollment to the
receiver identity and machine namespace; do not follow arbitrary redirect URLs or
accept receiver-supplied storage/provider addresses.

The body channel has a closed request enum: resolve an opaque source/evidence
handle, read a bounded transcript page or field, subscribe to append pages of that
selected source, cancel, and acknowledge delivery. Native byte-range reads remain
internal to the origin; the initial channel does not export arbitrary raw ranges. A later federated `search` is a separately
versioned read capability, not arbitrary SQL or an RPC escape hatch. Unknown
operations and extra payload fields refuse. Analytics publication uses a distinct
permission from body reads; a push credential does not authorize viewing.

For each request:

1. Receiver authenticates the end user, verifies machine/namespace/user grants
   and resolves only metadata that user may discover. An authorized analytics
   user is not automatically a body reader. Denials must not reveal existence
   through different cross-user lookup errors.
2. Receiver forwards a short-lived audience-bound signed user identity assertion
   plus nonce/request ID, source handle, generation, range and deadline. Origin
   validates a configured identity issuer and local policy. If the receiver is
   the identity issuer, the origin explicitly trusts that operator to impersonate
   users; where this is unacceptable, use an independent trusted issuer or
   local-only mode. No anonymous machine-wide read token.
3. Origin resolves the handle against its admitted native source manifest and
   rechecks current policy, source identity, generation and consumed boundary.
   Requests cannot name arbitrary paths, URLs, executables, environment variables
   or shell syntax. Source descriptors stay inside the allowlisted native roots;
   safe descriptor-relative opens and before/after identity checks prevent
   symlink/path races. Source references are provenance, never bearer authority.
4. Origin resolves and renders a typed transcript page from its pinned local
   Catalog/slot recipes and exact native-byte proofs, then applies field-level
   governance/redaction **before egress** and journals authorization. It returns
   untrusted structured transcript data, never executable HTML, with the qualified
   source cohort, activity/transcript generations, export-view generation, digest
   and policy/redactor versions. Omitted content has a permitted redaction/coverage
   marker; it is never presented as absent original evidence.
5. Receiver verifies the response against the request/admission, journals the
   read, and presents the typed fields using Semon's HTML-sink protections. A
   mismatch publishes no mixed-generation page. The receiver does not reconstruct
   native slot recipes from analytics or feed redacted bytes to an original-byte
   `SessionSourceReader` proof.

**Projection ownership:** choose origin-rendered typed pages for the initial
remote viewer. The origin owns Catalog/slot recipes, byte proofs, exact native
anchor resolution and the mapping from original evidence to redacted fields.
The receiver owns presentation and the encrypted page cache, not a second native
transcript index. Local viewing continues through the existing native reader.
Length-preserving redaction changes digests too: no redacted response can satisfy
an original-byte proof, even if offsets happen to remain equal.

An opaque export-view generation binds the complete original source witness,
transcript recipe version, redaction/policy-version vector and export schema.
Each authenticated page envelope binds that generation, requested opaque anchor,
page/field selector, native-evidence mapping, continuation and payload digest.
The origin retains original offsets internally; the exported continuation addresses
typed pages/fields, not offsets into transformed bytes. Length-changing masks
therefore do not shift native anchors. If a target is entirely withheld or its
identity cannot safely export, return an explicit unavailable anchor. A receiver
verifies the mapping assertion and envelope against the requested analytics witness;
it cannot independently prove the original bytes and must not claim it has.

Live append pages use the same projection and mapping. Changed source cohorts,
rewrites, policy/redactor changes or recipe changes invalidate the export-view
continuation. A verified prefix may remain readable during append only where the
native source fences establish that fact. The bounded origin render must not
trigger whole-session replay or new model inference; over-budget preparation is
unavailable. Native-range transport and a separate redacted-native index would
require a future explicit contract, not an implicit cache fallback.

No verb writes origin files, starts/resumes/stops a harness, controls a process,
opens a terminal, forwards a socket, installs a policy/model, changes a credential
or invokes arbitrary computation. The service has read-only native-root access;
only its own cache/index/audit state is writable under separate custody. Its
dispatcher has no dependency on retired control/runtime/SSH crates or generic
command handlers. L1 and producer scheduling are local operations outside this
channel. CI must test the absence/refusal of such operations, not merely hide
buttons in the viewer. TLS and an outbound socket alone do not enforce this boundary.

Live subscriptions are short leases tied to an authorized watcher, not origin
agent liveness. Apply bounded buffering/backpressure, rate and byte limits,
whole-operation deadlines and cancellation. No watchers, revoked authorization,
lease expiry or disconnect stops emission. A reconnect performs a fresh grant and
exact continuation check; it does not resume an old capability indefinitely.
Emit complete admitted records only. Appends may extend a verified prefix; a
rewrite creates a new source generation and invalidates continuations. Never
splice old and new bytes or use a first-4-KiB hash as proof of an entire prefix.

The library lease window is 60 seconds, explicitly renewed by acknowledgement;
there are at most two leases per enrollment, one unacknowledged page per lease,
256 KiB per complete framed page and 512 KiB buffered per enrollment. Each page
gets a fresh assertion, receiver attempt/authorization, origin authorization and
one receiver range receipt followed by completion. This attempt-per-page choice
stays below the existing 16-receipt reservation. Append pages stream through
memory without cache admission. Acknowledgement names the previous request and
exact opaque continuation; source/cohort generation, export view and pinned policy
must remain qualified. Cancellation travels in the next bounded origin poll.
The host drives the nonblocking receiver maintenance hook at each exchange and
revocation notification. Native complete-record tail rendering is supplied through
`AppendPageSource`; the library does not define a production source-handle format.
The closing operation returns its specific reason. Later calls use the explicit
uniform `lease_closed` reason for unknown, foreign and already closed IDs; no
historical enrollment/capability lookup is retained. A persistent audit write
failure cannot promise a durable failure completion: emission stops and the
existing attempted/authorized record remains incomplete. A transient followup
failure permits a failed completion with the actual delivered byte count.
Delivery reports validate and preserve actual bytes before lifecycle closure;
expiry or revocation during delivery yields a non-success completion without
renewing release authority. Maintenance is scoped to the authenticated enrollment;
revocation sweeps additionally name the known owner scope, never foreign leases.

### Origin read library integration

The Unix `semon-push::origin_read` core consumes one authenticated enrollment
through traits for live enrollment scope, admitted handles, the full policy vector,
rendering and export-view validation. It defines no transport or enrollment storage.
A renderer first prepares metadata without reading/rendering bodies: exact canonical
payload length, export-view generation, mapping and continuation. This lets the
origin fsync an exact authorization range before rendering. Missing preparation or
mapping evidence is unavailable. `body_adapter::BodyQueryRenderer` reuses
`body_query::view` through its public narrowed-timeout entry. That API cannot
prepare an exact canonical length without reading bodies, so this adapter performs
one bounded render during preparation and immediately discards it. It returns no
export-view generation or exact mapping; the core therefore refuses remote egress.
Truncated views and unknown field kinds are unavailable. Cancellation is checked
before and after SQLite; its progress handler enforces the narrowed deadline.

The analytics registry produces flat `<tag>:<64 lowercase hex>` handles;
`Handles::resolve` and pinned `Snapshot::admitted_cohorts` supply the reverse lookup
and native cohort fence. `semon-push::handle_resolver::OriginHandleResolver` accepts
source (`src`) handles, compares the complete existing producer binding with the
authenticated enrollment, and maps the union of admitted contributing sessions and
relation witnesses. Missing, foreign and withheld handles all return no admission.
Contributor IDs use existing published source handles. Stable archive `chain_id`
identifies each chain; the pinned prefix SHA-256 and consumed boundary are separate
continuity witnesses. The domain-separated cohort ID hashes sorted source handles
only, identifying membership across appends; the cohort ID plus activity/transcript
generations and each member's chain/prefix/boundary identify the exact snapshot.
Cache keys include the prefix witness and invalidate conservatively on append.
Local body UIDs never supply remote authority.

The resolver's production `IndexGenerations` implements the `Generations` seam
through `origin_projection::read_cohort_generations`: activity and transcript
generations and every native member's stable archive chain ID come from the same
qualified index transaction against the exact pinned cohort. The host constructs
it with the index path and an expected `analytics::Binding` independently held in
trusted local configuration. The adapter does not derive that expected binding
from the index. It maps every accessor refusal and invalid opaque generation to
`Unavailable`; missing chain evidence refuses. It never reads archives itself or
substitutes analytics generations. `UnavailableGenerations` remains an explicit
refusal implementation for callers without configured index evidence.
`LocalView` exact target lookup and the complete governance/scan-backed `PolicyVector`
remain integration dependencies. Exact export-view witness production and validation
remain required before remote reads or authorization-only cache hits can succeed.

Audit ranges are half-open `[0, canonical_payload_bytes)` for each exported typed
page/field. Continuations identify another typed payload; they never accumulate byte
offsets. These are exported offsets, never native offsets or original-byte proofs.
Empty/withheld targets are unavailable. Canonical library bytes retain explicit
`untrusted_text` tags; they define digest/range accounting, not a wire envelope.
The core accounts for its complete internal binding plus payload under 256 KiB;
transport integration must also account for its eventual serialization overhead by
narrowing the supplied limit. A non-Clone authorized result precedes actual egress;
the caller completes it with the actual payload byte count after delivery, failure
or cancellation. Dropping/crashing leaves an attempted authorization. Page/field
completion uses the existing audit completion event; streaming receipts remain in
the lease/append integration.

The `origin_client` library runs serial poll/response steps over an injected
transport, clock and entropy source. Reconnect rotates the channel binding and
never resumes body bytes. Backoff is a bounded next-poll timestamp; the host owns
scheduling. Posting succeeds for a complete accepted frame or fails the response;
a transport cannot report partial-frame progress, so completion counts accepted
payload frames only. No socket, HTTP client or enrollment custody is wired here.
Frame-zero bindings now carry optional positive origin cache TTL, per-source and
total-byte limits from an `OriginLimits` provider. They bind the audited response
identity. Receiver admission and insertion take the minimum of these values,
receiver limits and the default hard ceilings; absent values do not narrow, larger
values clamp, and malformed/zero values refuse during strict parsing. Origin byte
budgets count and evict only entries in the request's namespace and enrolled origin
incarnation; the cache key has no separate enrollment identifier. Receiver ceilings
retain global LRU enforcement. Lookup applies origin limits as admission checks
without origin-driven eviction, including a miss for a narrowed TTL.

### Body wire library framing (#594 slice 2)

Version 1 control envelopes use strict JSON, capped at 16 KiB. A response frame
is a four-byte big-endian JSON-header length, the strict JSON header, then exactly
`payload_bytes` raw canonical typed-body bytes. Frame zero carries the complete
`origin_read::ResponseBinding`; following data frames carry canonical typed payload
bytes. A terminal refusal carries a closed reason and retryability. Ordered frame
numbers start at zero, request IDs correlate every frame, and a final marker ends
exactly one response. The total encoded frame bytes, including metadata, stay within
256 KiB and the request's narrower limit; splitting cannot increase that budget.
Assertions are the existing canonical signing encoding followed by its 64-byte
signature, transported as lowercase hex. Destination is part of the signed request
digest. Page/field reads account actual bytes in completion; range receipts remain
stream-only under the existing audit contract. The receiver broker additionally
limits retained work to four requests per enrollment, including dispatched results,
so an abandoned caller cannot grow response memory without bound.

Receiver reads return a non-Clone pending delivery. Each `release()` checks current
enrollment, ACL and deadline before exposing typed fields; `delivered(bytes)` then
records actual completion. Drop/abandon records a closed failure and removes that
delivery's temporary cache entry, even following a successful release. This cannot
recall plaintext already handed to the caller. A refused release removes its entry
and exposes no new body. An authorization-only candidate refused as unavailable is
discarded by exact key before a fresh fetch, including when the candidate's old
authorization scope differs from the new response.

## Receiver cache: bounded temporary custody

Only the operator's self-hosted receiver holds this cache. It is separate from
Raw archives, import objects, SQLite durable state and the public web asset cache.
No automatic whole-machine prefetch, backup, archive promotion or central
re-derivation from it is allowed. A requested session is fetched in bounded pages
or ranges; opening it does not force retrieval of every source byte.

Accepted limits (D3): 256 MiB global encrypted cache,
64 MiB per source, 15-minute absolute TTL from fetch, and a 256 KiB remote page/field
response cap (the focused `view` budget). Origin-native reads retain the existing
128 KiB per-range bound and whole-operation budgets. Origin policy and receiver
limits take the minimum. LRU chooses eviction under the hard size cap; reads do
not refresh TTL. An over-budget request returns a narrower-read/resource-limit
response; it does not disable encryption or silently switch modes.

Cache the redacted typed page/field envelopes, not raw native bytes. Key entries
by namespace, enrolled origin incarnation, opaque source IDs/chains, source
cohort generation and consumed boundaries, export-view generation, requested
page/field selector, verified payload digest, redactor/policy-version vector and
authorization scope. A complete session may depend on several sources: retain the
exact cohort vector. Activity/transcript generations remain separate and must
agree through the origin's existing anchor resolver and exported mapping. A cache
key never grants access; authorize every hit.

Use an audited authenticated-encryption implementation, with per-entry data keys
wrapped by a receiver-operator master key kept outside the cache and its backups
(OS secret service or explicit operator secret store). Protect key files and
service identities separately from other receiver users; fail closed when keys
are unavailable. Bind namespace/generation/range/policy metadata as authenticated
data. Revoke wrapped entry keys and remove entries on expiry, eviction, policy
change, detected rewrite or source access withdrawal. Key rotation invalidates or
rewraps entries under an explicit operation; it cannot make old policy current.

There are no plaintext body files, temp spills, logs, crash dumps, service-worker
caches or cache backups. Bound in-memory buffers; disable core dumps and avoid
unencrypted swap for the body-serving process. Deletion destroys keys and removes
ciphertext; it cannot promise physical erasure of storage snapshots or plaintext
already copied by an operator/client. Configuration must make excluded backups and
key custody reviewable. A restart sweeps expired/invalid entries before serving;
an untrustworthy time/TTL state invalidates the cache, not the facts.

### Initial cache library boundary

The receiver cache library lives in `semon-push`, without listener or Viewer
wiring. Its initial custody backend is Unix-only: an explicit operator file holds
exactly 32 random key bytes, must be a regular 0600 file opened with `O_NOFOLLOW`,
and must live outside the dedicated 0700 cache directory and its excluded backups.
ChaCha20-Poly1305 wraps independent per-entry keys and authenticates envelope
metadata; explicit rotation invalidates entries. The 256 KiB response ceiling
includes the serialized envelope metadata. Cache accounting includes encrypted
records, staging and reserved clock-state replacement space; each cohort member
is charged the full entry size. Clock regression tolerance is two seconds, using
a persisted authenticated high-water mark; reads never extend expiry. LRU is
maintained in process; restart orders surviving entries by fetch time.

A lookup consumes an exact-key `ReadAdmission` assertion from the future trusted
channel/ACL layer, including current narrower origin limits. Its constructor does
not authenticate anyone: that layer must first complete online origin
authorization/audit and receiver authorization/read-attempt audit. This library
alone does not establish D2 or expose a remote body read.

`stream-through` prohibits disk persistence and uses bounded request-owned memory
only, with `no-store` responses and no retry spool. It does not mean zero copies:
origin, receiver TLS endpoint and browser see plaintext. Restricted sources default
to local-only; a source owner's opt-in covers only the owner principal. Named-user
opt-in remains deferred with multi-user support. Public/internal classification
does not itself authorize publication.

**Decision — origin-bound stream-through (#595):** every response carries a required
closed `delivery_mode` (`cached` or `stream_through`) in its authenticated response
identity and origin authorization's export-view binding. The origin injects this
source-policy decision into `OriginRead`; consent persistence belongs to #592.
Receivers may tighten either response to stream-through, never relax it to cached.
No-store delivery bypasses every cache operation, including lookup, clock metadata,
staging and abandonment cleanup; existing entries are never served or modified by
that response. An authorization-only request under origin stream-through policy
renders fresh bounded bytes instead. Complete encoded responses remain capped at
256 KiB; buffers and typed text are zeroized on drop. There is no retry spool.
`PendingDelivery::no_store()` is the typed marker the future route must translate
to `Cache-Control: no-store`. Release repeats live authority, ACL and deadline
checks; refusal records zero bytes, completion records actual delivered bytes.

## Viewer and search

### Local and receiver viewing

The local viewer binds loopback and uses its current authentication/Origin
protections; loopback alone is not an authorization scheme. Browser requests use
session-bound anti-CSRF controls, Host/Origin validation and no permissive CORS.
Remote exposure requires explicit receiver configuration with TLS and authenticated
per-user sessions. Receiver catalog, analytics and body routes independently enforce
machine and namespace grants; source policy further narrows body access. Do not
assume existing receiver controls satisfy this contract: verification is a gate.

Bodies, search strings, snippets, paths and credentials never enter URLs, browser
history, referrers, access logs or shared proxy/CDN caches. Send body operations and
search in request bodies; opaque link identifiers only. Set `Cache-Control: no-store`,
a restrictive referrer policy, and exclude these responses from browser
service-worker storage. The encrypted application cache is the sole receiver body
cache. Render commands, outputs and markdown as untrusted content with Semon's
existing sanitizer/CSP rules. No auto-execution or external image fetching from a
transcript. Keep existing sidebar/navigation; this is not a transcript redesign.

Represent origin reachability, body availability and coverage separately; expose
a single clear presentation state without inferring that availability is liveness:

| State | What the user sees |
| --- | --- |
| `live-origin` / `streaming` | Connected source; admitted append pages under a qualified generation; watcher lease status |
| `pulling` | Cancellable progress for requested ranges, not a fabricated percentage of all history |
| `cached-valid` | Generation, fetch time, expiry, policy/redactor versions and current origin status |
| `cached-stale` / `rewrite-detected` | Continuation refused; explicit “open latest” action, no mixed pages |
| `expired` / `evicted` | Metadata/analytics retained; a new authorized pull is needed |
| `offline-uncached` | Origin offline, evidence unavailable; no fallback import or recovery |
| `offline-cached` | Cache exists but new read is unavailable under D2's online authorization/audit rule; no offline grants |
| `refused` / `source-missing` | Authorized user receives a bounded reason; permission denial differs internally from permanent source loss and transient disconnect |

Partial cache coverage is “N of M requested ranges held”, with unknown totals
explicit. It is not a complete transcript. Local archive loss/native pruning,
TTL eviction and offline status are different conditions. `body_unavailable`
includes a closed reason and retryability: disconnected may recover, confirmed
source loss is permanent without an explicit new custody source. Do not label
unavailable evidence `not_found` or return an empty successful transcript.

Exact analytics jumps retain [activity anchor semantics][activity]: source,
activity and transcript generations plus exact native record identity. A cached
copy must match that evidence witness. Ordinals, time, title, similar text and
copied usage cannot manufacture a match. Existing Codex text-only steps may have
no exact anchor. No silent switch to a newer source or T3 copy; an explicit
“open latest” is ordinary browsing and is not presented as the original evidence.

### Search

Local FTS remains at the origin. The receiver must never present search over only
its cached bodies as a complete machine search. Initial remote `search` may return
`capability_unavailable`; on-demand `view` is sufficient for the first receiver
slice. A later opt-in search fans out bounded queries to authorized origins over a
separate enumerated read capability. Query text is itself sensitive and sent only
to the selected origins. Each origin evaluates policy, audits any returned snippets
and applies query/range/deadline limits. Merge per-namespace results with the
specified reciprocal rank fusion, retaining origin attribution and coverage.

Responses name searched, refused, timed-out, offline and incomplete namespaces;
partial results never look exhaustive. Deadline includes network time, decoding
and rendering. Existing snippet/response caps and `untrusted_text` wrapping stay.
Do not promise a 500 ms fleet search over offline/slow machines. SQL remains over
local body-free imported facts and never triggers a body pull. Hub `search` and
`view` refuse before I/O.

## Re-derivation, coverage and schema changes

The receiver keeps the last complete admitted facts while an origin is offline.
On reconnect, exchange acknowledged head/version manifests, replay idempotently,
and let the locally scheduled producer queue stale sessions in bounded batches.
Publish replacement rows only after complete admitted derivation and policy export.
Never replace a session with partial rows to make freshness appear better.

Separate these axes in query metadata and dashboards:

- contact: online/offline and last contact;
- source: complete, incomplete, missing, or not recently verified;
- computation: current, pending refresh, failed, or frozen at recorded versions;
- evidence: locally available, pullable, policy denied, expired or unavailable;
- freshness: observed/derived/published times and admitted generation;
- version coverage: session/step counts and exclusive-usage denominators by rules,
  transform, price and taxonomy version, including unclassified/unknown counts.

Offline is not proof that raw is gone. “Frozen” is an explicit retired/unrecoverable
condition or an operator decision, not a timer on a disconnected laptop. A retired
machine that never returns retains its last valid rows permanently at those rules
unless the operator removes them. An origin with a partial multi-source history
cannot claim full refresh. Mixed-version analytics is useful but must be labelled;
version-sensitive comparisons can filter a version/cohort and expose the excluded
denominator. Broad SQL results include a snapshot coverage envelope even when the
query does not select version columns. Privacy-filtered coverage need not reveal
the existence of inaccessible namespaces.

Price changes are different from rules changes. The current activity store can
reprice saved complete billing inputs even when raw is unavailable; an offline
origin still cannot publish that correction. D4 accepts retaining the recorded
origin valuation first. A separately named central valuation may be specified later;
it is not part of the current pricing path.
Such a view would label its selected price version and missing-input coverage,
respect exclusive/copied usage and avoid multiplying cost through tool-call joins.
It must not replace origin values or pretend to reclassify L0. Price-dependent
waste/component/summary views would need consistent recalculation under the same
valuation. This is an amendment to the one-model agreement, not existing behavior.

Pre-release policy remains **one current Semon format**. Rules/price skew is data
provenance within that format, not permission to accept arbitrary old schemas.
Reject mismatched origin/receiver/Hub schema or protocol versions before activation;
keep the previous valid head and report `schema_mismatch` and withheld coverage.
Compatible binaries may publish different rules versions. Roll out a format change
by updating producers/consumers together. An origin that cannot return cannot be
magically migrated: retain/export its old-format custody separately, refuse that
format in the new query engine, and require an explicit operator decision before
reset/removal. Do not implement parallel old readers, converters or dual writers.

This means a breaking schema change can make frozen history unavailable to current
queries. Do not run `asi reset` automatically or treat remote imported evidence as
rebuildable. D5 rejects a multi-version exception and keeps the current
pre-release rule; frozen data must be preserved and exportable without conversion.

## Hybrid opt-in: receiver raw replica

Allow a source to select a distinct `receiver-replica` custody mode by explicit
origin policy and operator configuration, when it needs evidence and re-derivation
after origin retirement or loss. Suitable cases include centrally owned workstations
with an approved receiver trust domain. It is not a cache promotion, automatic
fallback from offline origin, or a permission granted by a classification tag.

Replica enrollment names receiver identity, permitted source cohort, redaction,
encryption/key owner, readers, retention and deletion authority. Redacted raw may
then flow continuously into immutable receiver custody, including replaced bytes;
#572's chain/seal guarantees remain necessary. Redacted copies may lack evidence
removed by policy and are not full harness recovery backups. Encryption at rest,
access enforcement and audit apply to replica bodies and their derivatives too.

Raw replication is selectable per source, but **analytics publisher authority
transfers only for a whole machine namespace**, matching #555. For a mixed machine
with replica source A and pull-only source B, the origin remains the sole publisher
for both; A's receiver copy improves custody but does not let the receiver publish
replacement analytics independently. There is no per-source publisher composition,
new namespace identity or automatic failover hidden in replica enrollment.

Option B's central re-derivation mode is available only when every contributing
source in that namespace's admitted session cohorts permits receiver derivation
and has verified replica custody. A session spanning A and B cannot be centrally
re-derived from A alone. If one source is absent or disallows replication, retain
origin authority and frozen coverage as needed. Deliberately excluding that source
is a separate policy withdrawal; it cannot manufacture complete replacement facts.

A namespace authority switch stages a complete replacement head, retaining all
unaffected admitted facts and categorical label assertions with their original
versions/provenance, plus permitted new derivations and explicit withdrawals.
Verify a full session/cohort inventory and durable-label checkpoint against the
previous head before the atomic epoch switch. Receiver human adjudications remain
in their separate owned durable store; do not erase them or relabel origin L1
assertions as receiver-generated. Missing admitted rows or labels block activation
unless explicitly withdrawn by their authorized policy owner. Origin and receiver
heads must never be summed as two copies. Local origin analytics may remain for
offline use but is not independently exported into that namespace. L0 derives
under the same single-model code. L1 remains origin-side by default; receiver
L1/model egress requires a separate future source consent, not replica enrollment.

Per-read origin authorization and audit remain mandatory even for a replica.
D2 excludes offline grants and receiver-only body-policy delegation. A replica
preserves raw and may centrally re-derive under its enrolled producer policy, but
new offline viewer reads remain unavailable. Replica custody improves recoverability;
it does not establish offline viewing authority. If the origin is permanently lost,
body access stays blocked until a separate explicit recovery/authority decision;
there is no automatic reassignment.

Origin and replica archive retention are separate from cache TTL. D3 preserves
the existing origin retention policy until it is deliberately changed. Replica
enrollment records its retention explicitly; no new finite-retention default or
automatic purge is introduced by this decision. Native
log deletion remains an operator action. Approving this proposal never removes
existing mirrors, archive chains, keys or user data. Switching back to pull-only
stops new replication after explicit activation; old receiver custody remains
until separately authorized deletion. Never relabel an archive as disposable cache.

## Threat model and present gaps

Assets include raw prompts, tool output/code, secrets/PII, local archives, identity
and lineage, billing facts and labels, path-token keys, cache keys, authorization
policy, audit receipts and the integrity/availability of analytics history.

| Actor or failure | Threat | Proposed controls and residual limits |
| --- | --- | --- |
| Receiver operator/admin | Reads/copies bodies, forges subjects when acting as identity issuer, tampers with retention/audit | Explicit trust enrollment, independent identity issuer where needed, minimal body transfer, origin policy/audit, encryption at rest. Root can observe authorized plaintext; no cryptographic deletion guarantee against them. |
| Another receiver user | Cross-user/namespace body reads, cache poisoning, query-based data leakage | Per-request machine/namespace/user checks, origin grants, authorization-scoped cache admission, tenant-scoped SQL inputs, OS service isolation; analytics ACLs separate from body ACLs. |
| Hub or another Hub tenant | Body access, cross-tenant facts or object-existence leakage, metadata correlation | No body protocol/storage/key, fail-closed export/import allowlists, per-tenant heads/workers/objects, path tokens, approved names only. Tokens/categories/timing remain correlatable. |
| Network observer/attacker | Eavesdropping, replay, response substitution, receiver impersonation | TLS identity validation, short grants, request nonces/deadlines, namespace/epoch binding, generation/range digests. Traffic volume and timing remain observable. |
| Compromised origin | Fabricates facts, steals shared token key, sends malicious records/Parquet, floods receiver | Bind claims to its namespaces; quotas, bounded parser/query workers, validation and untrusted rendering; revoke enrollment. Authentication proves sender, not truth. A shared token key broadens privacy impact; no remote attestation is promised. |
| Lost laptop/disk | Raw/evidence loss and irreproducible labels | Origin archive and user-owned backups; explicit optional replica. Facts remain useful but cannot recreate raw. |
| Policy/detector failure | Unknown secrets or PII escape, tag false negatives, stale grants | Restrictive source floors, local fail-closed scan status, explicit grants, version provenance and adversarial corpus checks. Detection is best-effort, not data-loss-prevention certification. |
| Browser/client compromise | Bodies copied, unsafe HTML executed, secrets in logs/referrers | Sanitization/CSP, no external transcript loads, no-store, opaque URLs and bounded responses. Authorized clients can retain what they receive. |

**Verified current baseline, not delivered safeguards:**

- [`redact.rs`](../../crates/semon-push/src/redact.rs) documents and implements
  best-effort, length-preserving masking of known secret shapes. It is not a PII,
  proprietary-code or general secret detector; unrecognized shapes can pass.
- The [mirror protocol](../mirror-protocol.md) and
  [Copilot redacted viewing copies](../copilot-cli.md#redacted-viewing-copies)
  explicitly say the mirror is receiver-readable and not end-to-end encrypted.
  Zstd “sealed” copies are compression/storage representation, not encryption.
- The accepted [Raw spec](asi-store.md#raw) keeps raw indefinitely by default and
  originally specified no encryption at rest. The reconciled store and this
  contract require key custody and explicit retention before remote cache/replica
  activation; implementation evidence remains required.
- Receiver-viewer per-user/per-machine authorization and body-read audit have
  **not been verified by this design review**. Existing authenticated routes and
  source fences do not establish those guarantees. Do not expose remote bodies on
  the strength of this document alone.

## In-flight work: keep, change, drop

This table is a proposed plan for the PM after approval, not a command to a lane.
Issue/PR descriptions were read on 2026-11-09; CI and review states are volatile.
Final base `50c5255` includes merged #571, #572 (closes #558), #583 (closes #556),
#577, #584, #586 and #589 (docs-only CI). Their rows describe preserved delivered
scope, not pending implementation or merge requests. Costs are qualitative design
estimates, not delivery promises.

| Slice | Disposition | Effect and proposed approval boundary |
| --- | --- | --- |
| #544 / #545 | Change specs | Direction/store/query are reconciled in this PR; PM updates issue criteria after merge approval. |
| #559 / #571 | Keep | Transactional query-ready change log, incarnation and repo identity work at origin unchanged. Export field policy is separate. Near-zero rework. |
| #558 / #572 local archive (merged) | Keep delivered foundation | Essential origin custody across prune/rewrite. No permission to delete native data. |
| #558 / #572 receiver custody (merged) | Keep delivered supported mirror/opt-in replica custody | D6 preserves both halves of the landed change. No removal or rescoping of #572; new pull-cache work is separate. |
| #575 | Keep, change policy scope | Archive-backed reads become critical at origin. Separate origin/replica retention from cache TTL; freeze coverage after authorized source loss. No automatic historical deletion. Low rework. |
| #554 / #577 schema | Keep foundation; extend export contract | Typed body-free model and billing inputs survive. Add version/coverage and field-policy definitions in a focused subsequent schema slice; internal `path` is not export permission. |
| #554 / #578 publication | Keep | Atomic generation/live-map writer remains local to each publisher. Add remote staging/namespace heads at the boundary, not inside local atomicity. |
| #554 / #579 publication tests | Keep, extend later | Crash consistency and source-missing preservation still required; add federation replay/epoch/partial-cohort cases with transport. |
| #554 / #580 compaction/retirement | Keep | Operates on admitted analytics files; never collect the only imported frozen facts as “rebuildable.” Cache eviction is independent. |
| #554 / #581 DuckDB and build cache | Keep | Same query/storage engine and CI cache gate. No federation dependency. |
| #556 / #583 StepFacts (merged) | Keep delivered foundation | Sole activity model, complete billing inputs and usage ownership remain origin-local. Do not export raw StepFacts. Federation/L1 are follow-up work, not scope changes to landed #583. |
| #557 transforms | Change execution/publication | Execute at origin, project the one admitted SQLite generation, add mixed-version/frozen coverage and exported-head ownership. D4 keeps origin recorded costs; central valuation is later work. Low relocation cost; material semantics. |
| #570 performance | Change measured path | Preserve local budgets; add connected origin→receiver freshness, cache/range/cancel budgets and bounded origin resources. Separate unavailable coverage from latency success. Fleet FTS cannot inherit a 500 ms local promise. |
| #576 / #584 CI flake | Keep | Independent CI correctness work; no threshold relaxation. |
| #585 Git includes | Keep | Origin-side repo identity resolution unchanged. |
| #582 partial-source loss | Keep; federation prerequisite | Preserve last complete admitted activity and source witness, not a fragment. Coordinate existing owner through PM. |
| Closed #561 | Change/reintroduce scope after approval | Metadata receiver validation, staging, idempotency and binding return, implemented with the shared #555 publication mechanism. Do not revive the old row wire format or reopen mechanically. |
| Closed #562 | Change/reintroduce scope after approval | Metadata-default sender returns; body replica is explicit policy, on-demand channel separate. No default mirror facts/status leak. |
| Closed #563 | Drop duplicate scope | Keep body indexing at origin under #567; no second central body-index project and no indexing cache as a complete corpus. |
| #560 durable state | Keep, clarify ownership | Origin categorical assertions export append-only; free text stays at its authoring store. Receiver human adjudications have their own provenance, never overwrite origin assertions. No general multi-master durable-state sync. |
| #564 SQL sandbox | Keep | Body-free snapshots, namespace-scoped inputs, AST/process limits unchanged. SQL never fetches bodies. |
| #565 schema reference | Keep, extend | Generate version/coverage, export classes and refused-format semantics from current definitions. |
| #566 MCP/CLI | Keep, extend envelope | `sql`/`schema` and `find` retirement survive; expose imported freshness/coverage. |
| #567 search/view | Change substantially | Origin FTS; on-demand authenticated view; later optional fan-out with explicit partial coverage. No receiver cache-only global FTS. |
| #568 Viewer routes | Change | Per-user/machine/namespace ACLs, origin policy/audit, generation-qualified cache and offline states. Keep read-only Semon viewer. |
| #555 Hub export | Keep mechanism, change profiles/provenance | Reuse atomic objects/heads/epochs for receiver import with plain typed paths/names; HMAC at Hub boundary under D7's recorded interpretation; preserve origin versions. Hub stays body-free; no deploy implied. |
| #546 L1 | Change location | Origin inference, consent, category/evidence provenance and accuracy/coverage work; not delivered by StepFacts. |
| #547 / #586 Viewer scope | Keep accepted framing | Standalone read-only raw viewer and analytics; reconcile availability copy, without presentation redesign or T3 dependency. |

**Execution boundary:** D1–D8 are accepted. The PM will apply the concrete lane
plan after the user approves this docs merge. Existing phase 1 continues under its
current authority until that coordination. Keep supported mirror/#572 custody,
origin archive reads and the analytics foundation; do not enlarge current PRs with
new channel/cache/authorization work. Offline grants, central valuation and
multi-version support are excluded from the current scope, not unresolved choices.
Receiver search remains unavailable until its separate coverage/audit work exists.
No issue is closed/reopened and no lane is redirected by this document itself.

The direction/store/query specs change together in this PR. Subsequent producer,
consumer, fixture and issue changes implement one current contract without
compatibility adapters. The following actions are for the PM to apply after merge
approval; new issue titles are concrete proposals, not already-created issues.

### Lane-change plan for PM application

| PM action | Issue/PR and next work |
| --- | --- |
| Retitle/update #544 | “ASI store: origin derivation, governed publication and optional replica custody”; replace default receiver derivation acceptance with this architecture and link new transport/governance slices below. |
| Retitle/update #545 | “ASI query: body-free SQL, local search and online remote view”; add availability/version coverage and explicit receiver search refusal. |
| Keep #559/#571 and #585 | #571 landed during final integration; preserve its typed identity/change-log contract. Storage lane proceeds with #585; no federation protocol is added to this slice. Plain typed repo/path fields remain allowed to operator receiver. |
| Keep landed #558/#572 | #572 merged as `50c5255`; #558 is complete. Preserve origin archive and supported receiver mirror custody. No retitle, reopen, removal or follow-up merge is needed; new cache/enrollment work gets separate issues. |
| Retitle #575 | “Origin archive-backed reads and explicit archive retention”; implement prune/rewrite reads first, preserve default origin retention, distinguish explicit replica retention from TTL. |
| Keep #554 stack #578–#581 | Finish publication/compaction/DuckDB layers following merged #577; no re-splitting or transport scope expansion. Follow-up provenance/export fields belong with transport integration. |
| Keep landed #556/#583; assign #582 | #583 merged as `5f379b7`; #556 is complete. PM assigns the remaining complete-generation preservation in #582 before claiming complete federated updates. Do not reopen StepFacts or wake the settled activity lane implicitly. |
| Retitle/update #557 | “Origin analytics projection, version coverage and reconnect refresh”; use the single SQLite model, origin recorded pricing and admitted cohorts. Wire local consumers first, then the publication slice; do not recompute costs in DuckDB. |
| Keep #564/#565/#566 | Query lane implements sandbox/schema/MCP/CLI with coverage envelopes and exact-format refusal; SQL never fetches bodies. No current PR scope expansion beyond those contracts. |
| Split/update #567 | Retitle “Origin-local FTS and focused view”; implement local search and view contracts. Move receiver fan-out into a new deferred issue; do not reopen closed #563. |
| Retitle/update #568 | “Authenticated online receiver view and explicit availability states”; depend on new read-channel/cache gates. Origin-rendered typed pages and exact anchors; no offline grants. |
| Retitle/update #555 | “Hub analytics export: tokenized paths and tenant-bound publisher heads”; reuse shared publication mechanism below, apply HMAC only at Hub boundary under the recorded D7 interpretation, preserve origin provenance. Hub deploy pause unchanged. |
| Update #560/#546 | Durable author ownership/category export for #560; origin L1 inference, consent and exact evidence mapping for #546. Free text stays bodies. |
| Update #570 | Keep local performance gates; add controlled connected origin→receiver freshness, bounded online view/cancel/cache resource cases. Defer fleet-search latency acceptance to its issue. |
| Leave closed; link successors | #561/#562 remain historical closed issues; their metadata-default intent returns in the new publication issue. #563 remains closed as duplicate of #567. No issue closure is newly required. |
| Leave completed work alone | Merged #571/#572/#583/#577/#584/#586/#589 and their closed issues retain their scope. #589's docs-only CI selection is preserved. #547 references online/offline evidence states; no presentation redesign is authorized here. |
| Open “ASI governance: origin policy, classification and export profiles” | Versioned policy, multi-source intersection, secret/PII scan status, provider consent, plain receiver name/path profile and tokenized Hub profile. Own test fixtures for body leakage and withheld coverage. |
| Open “ASI publication: origin-to-receiver objects, heads and reconnect” | Shared #555 mechanism, namespace enrollment/epochs, typed allowlist validation, complete-source provenance, idempotent staging, mixed-version coverage and frozen-data export. Dependencies: #557, governance, complete-generation preservation. |
| Open “ASI online body channel: origin-rendered pages and exact evidence” | Outbound-only read capability, identity assertions, origin recipes/proofs, redacted field mappings, bounded append subscriptions, cancellation and per-read origin audit. No generic RPC or remote execution. |
| Open “ASI receiver body access: ACLs, dual audit and encrypted TTL cache” | Per-user/machine/namespace authorization, origin online checks even on hits, 256/64 MiB caps and 15-min TTL, key custody, stream-through and no-store boundaries. Depends on governance/channel; gates #568 exposure. |
| Open “ASI replica enrollment and namespace authority transfer” | Source-approved raw custody; central producer activation only for complete eligible namespaces with unaffected facts/labels preserved. Supported mirror continues; offline body reads prohibited. |
| Open deferred “ASI federated search with coverage and audit” | Later authorized-origin query fan-out, sensitive-query egress, per-namespace partial coverage, snippets/audit and measured budgets. Receiver `search` refuses until delivered. |

Suggested order: finish existing storage/activity foundations; implement governance
and shared publication; complete SQL/schema coverage; build the online read channel
and receiver security/cache together before #568; add replica authority enrollment;
implement Hub export after publication/profile validation, with no deployment
implied. Local FTS can proceed independently after its native/durable prerequisites.
The PM assigns new work to active or explicitly relaunched lanes; this plan does
not wake the settled activity/Hub Managers or authorize unsolicited lane messages.

## Consultation and accepted decisions

The design owner requested targeted advice from peer Managers. Advice is evidence,
not approval. Following the PM's correction, there were no follow-ups to the settled
Hub or activity lanes. Hub requirements below were checked against #555 and Hub's
[ASI MVP ownership/custody document][hub-mvp]; current Hub ingest implementation and
its enforcement have not been verified.

- **ASI storage Manager** (`mcp:f3620d89-268d-47dd-a17d-79b4d29e187c`, local
  environment): recommends keeping most phase 1 slices, including intact #572
  while mirror is supported; reusing #555's mechanism; treating #567 as the major
  redesign. Objects to permanently frozen rules, origin-only loss of custody,
  origin-frozen prices and exact-format mismatch. Recommends central price views
  and a scoped multi-version exception. D4/D5 resolved those disagreements:
  recorded origin costs first and no multi-version exception. Reuse and coverage
  are retained.
- **Viewer research Manager** (`mcp:0f1ba7d5-f083-4d84-a519-d0d6a60f42d3`, local
  environment): recommends explicitly labelled TTL-valid cached copies while
  offline, exact generation-qualified anchors and no T3 fallback. Flags that origin
  loss removes the raw-viewing foundation. Source: [#586 research at 3d59253e][viewer-research].
  States/anchors are retained; D2 selected online authorization and audit for every
  read, rejecting the offline-cache alternative for the initial delivery.
- **Session activity Manager** (thread
  `e167507d-cfbd-47b7-8ccc-ae5659c30304`, environment
  `40683a5c-9e29-48b4-930c-68d74f9a2bba`) supplied advice before the settled-lane
  correction: origin execution fits the one-model contract; use explicit exports,
  complete source/version manifests and exact opaque evidence. Repricing from
  billing facts differs from raw-dependent reclassification. These claims were
  reconciled with [#557][one-model] and [activity docs][activity], not treated as a
  new implementation authorization. Historical aggregate checks (39/39 sampled
  lists; one 11,820-step selected read over budget; incomplete waste labels) do not
  establish current federation, L1 accuracy or anchor acceptance.

The user accepted the following on 2026-11-09 through the PM's structured
question process. D7 chose the plain-name/path option rather than the author's
origin-tokenization recommendation. All eight decisions are settled; the PM's
interpretation of D7's destination boundary is explicitly marked below and remains
open to user correction, not a new approval request.

| Decision | Accepted choice | Consequence |
| --- | --- | --- |
| D1: Default architecture | Federation replaces option B as the default. | Mixed rules versions and possible permanent evidence loss are accepted. |
| D2: Online reads | Origin authorizes and audits every read, including cache hits; no offline grants. | Cached bytes and replica custody do not authorize offline viewing. |
| D3: Cache and origin retention | 256 MiB global, 64 MiB per source, 15-minute absolute TTL; stream-through opt-in. Origin retention unchanged until deliberately changed. | Reads do not extend TTL; no archive purge or cache-to-archive conversion. |
| D4: Costs | Keep origin recorded costs first. Separately named central valuation can come later. | Never silently mix valuations or add central repricing to current transforms. |
| D5: Format compatibility | Refuse old-format origins; no multi-version exception. Preserve and export frozen data. | No adapters/dual readers; unavailable history is not silently reset or deleted. |
| D6: Replicas | Opt-in replica; namespace-wide authority transfer only. Keep supported mirror and #572 custody. | Mixed-source replica/pull-only namespaces retain origin analytics authority; cache never becomes archive. |
| D7: Metadata privacy | Plain names and paths for richer analytics. **PM interpretation:** this applies between origin and operator receiver; settled Hub rules remain, with paths tokenized at Hub boundary and no plain paths in Hub. Open to user correction of that interpretation. | Origin tokenization is not the default. Body exclusions, credential stripping and source policy still apply; receiver learns sensitive names and paths. |
| D8: Search sequence | Local search plus remote view first. | Receiver search stays unavailable until federated coverage and audit exist. |

## Acceptance before implementation can claim these guarantees

All runtime validation belongs in CI or separately authorized bounded local
aggregate checks. This docs proposal ran no product builds, tests or raw-data scans.
Required acceptance slices include:

- End-to-end outbound capture over synthetic fixtures: receiver analytics contains
  only allowlisted typed facts, with plain name/path fields under D7, and no bodies
  or free-text labels. Hub exports contain no plain paths; Parquet metadata,
  errors and logs cannot bypass the destination profile. Known/unknown secret and
  PII cases measure detector misses rather than claiming perfect redaction. L1
  opt-out produces zero provider evidence egress.
- Transport refuses path traversal/symlink races, replayed grants, foreign
  namespaces, wrong audiences, unsupported verbs, stale generations and malformed
  frames; all control/runtime/SSH requests fail before dispatch. Cancellation,
  zero watchers, token revocation and backpressure stop delivery within bounds.
- Every returned body/cache hit has both audit records under D2's selected model.
  Failures before audit cannot leak bytes. Cross-user catalog, SQL, cache and field
  reads refuse. Origin and receiver policy versions bind every admission.
- Cache contains only encrypted bodies; keys/logs/temp/spill/backups respect their
  contract; TTL does not slide; cap includes staged writes and metadata; restart,
  expiry, revocation and rewrite prevent stale reuse. Stream-through writes no
  body to disk. Key unavailability fails closed and leaves facts intact.
- Native identity/lineage and exact anchors survive demand fetch/redaction where
  supported, with honest refusal elsewhere; append/rewrite/cancel cannot combine
  source generations. Missing sources and #582 never replace complete facts with
  fragments. An unavailable origin cannot cause a T3, Hub or mirror fallback.
- Multi-source analytics and L1 fixtures combine permissive and restricted inputs:
  every exported field, label, provider request and coverage disclosure obeys all
  contributor policies; removing a denied source cannot produce a complete total.
- Original-byte proofs are never accepted for masked bytes. Length-preserving and
  length-changing redaction preserve only explicit native-to-exported mappings;
  withheld targets refuse exact navigation and stale export-view cursors fail.
- Publication lost-ACK replay, parent conflict, receiver restart, source
  incarnation change, namespace transfer and interrupted full refresh preserve one
  complete head. Mixed replica/pull-only sources keep origin authority; whole-
  namespace transfers retain unaffected facts/labels and refuse partial cohorts.
  Untrusted files cannot escape parser/tenant resource limits.
- Mixed rules/taxonomy/price versions, offline reconnect and retired origins show
  correct coverage. Copied usage is not double-counted. Current-format mismatch
  refuses without deleting frozen facts or resetting custody.
- Hub rejects bodies and unsupported export fields before activation, validates
  already-tokenized paths, scopes object access per tenant and never resolves an
  evidence handle. No Hub application deployment is part of this proposal.
- CI measures origin compute and publication lag, connected freshness, bounded
  range latency/memory, cancellation and offline/partial search separately. Existing
  Rust/UI/security gates and native fixtures remain; no thresholds are loosened.

## Sources and review record

Initial source/review baseline: `6e8a6b6007f8cfb1c0185f911d0b1cc9dd725e5c`.
Publication base refreshed on 2026-11-09 to
`ff6194172dbf3a58a4cfd031a9dc4b5beafd698b`, including #577, #584 and #586;
the changed Viewer direction was reconciled with this contract. Final integration
also incorporates `45fbcc17baeded7aca0311e3b14d23ea2d91dec1` (#571), preserving
its typed repository-identity amendment in the store spec, then base `50c5255`
with landed #572 custody, #583 StepFacts and #589 docs-only CI. These final status
updates do not change the independently reviewed architecture.
Relevant native contracts: [source readers](session-source-reader.md) and
[retained projections](retained-native-projections.md). Their generation, custody
and authorization invariants are reused; historical hosted-body activation text
is not a mandate to restore bodies to Hub.

Independent review completed 2026-11-09 by Codex `gpt-6.1-sol` at high effort,
given the document and original sources without an acceptance conclusion. It
found two P1 contract gaps and one P2 projection gap. The author addressed them:

- P1, source replica versus namespace publisher ownership: authority transfer is
  namespace-wide; mixed-source replica mode retains the origin publisher; a
  complete replacement preserves unaffected facts and categorical label history.
- P1, multi-source policy composition: every derivative satisfies the intersection
  of contributor policies; prohibited rows/fields and coverage are withheld
  without presenting a filtered fragment as a complete session.
- P2, redacted-byte proofs: the origin produces bounded typed transcript pages and
  owns recipes/proofs and original-to-redacted evidence mappings; the receiver
  never tests redacted bytes against original-byte proofs.

D1–D8 were then accepted by the user as recorded above. A fresh independent
Codex `gpt-6.1-sol` review at high effort completed on 2026-11-09 with **no
actionable findings** across the reconciled federation, direction, store and query
contract. It checked the original sources and accepted decisions, including D7's
qualified boundary interpretation, replica authority, policy intersection, native
proofs and the lane plan. `git diff --check` passed. Runtime enforcement, detector
effectiveness, performance, Hub ingest and exact-tree CI were not certified by
that review and remain separate evidence requirements. Merge approval is separate.

[roadmap]: https://github.com/onsager-ai/semon/issues/549
[spec-pr]: https://github.com/onsager-ai/semon/pull/553
[old-decision]: https://github.com/onsager-ai/semon/pull/550
[one-model]: https://github.com/onsager-ai/semon/issues/557#issuecomment-6733930340
[i582]: https://github.com/onsager-ai/semon/issues/582
[activity]: session-activity-analytics.md
[hub-mvp]: https://github.com/onsager-ai/semon-hub/blob/main/docs/asi-mvp.md
[viewer-research]: https://github.com/onsager-ai/semon/blob/3d59253e850517a34d362cf2a21f2ee78d3ff110/docs/design/t3-viewer-reuse-research.md
