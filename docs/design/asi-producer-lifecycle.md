# Explicit local producer initialization

Proposal for #652, part of #593. This amends the local producer contract; it does
not implement `connect`, `start`, enrollment activation or their UX.

## Current state

At main `85814509f`, [`run_materialize`](../../crates/semon-cli/src/main.rs)
preflights namespace/store shape, validates an existing index, acquires
`analytics_writer::Owner`, creates/reuses the namespace, opens the store, refreshes
the strict native Catalog and publishes through a captured endpoint.
[`analytics_namespace.rs`](../../crates/semon-sessions/src/analytics_namespace.rs)
installs a private, newline-terminated `origin-` ID atomically; malformed files,
symlinks and a manifest without its namespace refuse. Existing IDs are reused.
[`events/store.rs`](../../crates/semon-sessions/src/events/store.rs) creates the
index schema and random `meta.incarnation` in its cold-initialization transaction;
ordinary native collection can create that index independently of materialization.
Unsupported indexes refuse without rebuilding.

[`Owner::acquire`](../../crates/semon-sessions/src/analytics_writer.rs) validates
existing SQLite read-only before acquiring the nonblocking exclusive
`analytics.lock`. [`Store::open_with_lock`](../../crates/semon-asi/src/store.rs)
creates the analytics directory and validates/recovers the store under that lock.
Its empty manifest is **in memory**: generation 0, no files/live map/checkpoints,
`durable_revision = 0`. The first publication persists a manifest; the durable
revision tracks exported labels, not initialization or index identity.
[`analytics.rs`](../../crates/semon-sessions/src/analytics.rs) is the native activity
aggregation, not a public initialization API. Body reads return `not_materialized`
when the namespace is absent and never create it
([`body_query.rs`](../../crates/semon-sessions/src/body_query.rs)).

The origin `connect` command and its materialize-first precheck are absent at this
revision. The issue's interim C behavior is a requirement for its implementation,
not shipped behavior verified here.

## One entry point

Recommend public `semon_sessions::analytics::initialize(...)`, making that module
public with this focused API. Do not add `semon analytics init`: `materialize`
remains the batch command, while `connect`/`start` absorb setup. A materialize flag
would couple identity initialization to batch publication and invite callers to
invoke the CLI instead of sharing the library contract.

Inputs are the trusted local index/state location using existing path helpers,
explicit local producer intent, and an optional expected namespace/index binding
from existing enrollment. It returns `{ namespace, origin_incarnation,
index_incarnation, disposition: created | existing }` plus retained writer ownership
for the caller to transfer to the writer. `created` means at least one allowed
missing component was created. No caller may separately generate an analytics
namespace or invent an incarnation. The native index producer retains ownership of
schema/index-incarnation creation.

The two incarnations keep one meaning each (agreed with the origin lane, #634):

- `origin_incarnation` identifies the origin's producer instance: an opaque ID
  created once, in the same persisted step as the namespace. It stays constant
  across index rebuilds and changes only when the namespace is created anew or
  transferred (re-initialization, a copied or restored store, a whole-namespace
  transfer).
- `index_incarnation` is the analytics index generation. It changes on rebuild or
  reconcile, which triggers a complete namespace reconciliation, never a reset to
  empty.

The initializer creates `origin_incarnation` with the namespace, persists both IDs
with it, and validates continuity against the store manifest, as
`read_analytics_namespace` does for the namespace today. The receiver only records
both and checks syntax and exact equality; it never infers either from contact
time, hostname or path.

Before mutation, qualify existing namespace, index and store, including supported
format, local ownership, manifest/live-map integrity and namespace membership.
Acquire `analytics.lock`, then revalidate mutable bindings before any creation or
recovery; retain it through writer handoff, never release/reacquire between setup
and producer startup. SQLite retains its own cold-creation/write transactions;
the analytics lock does not replace native index concurrency controls.

On a fresh state directory, create the namespace file, a current empty local index
with its durable incarnation through the existing cold index initializer, and the
empty store directories/lock. A valid pre-existing local index can be adopted;
initialization does not refresh native roots. No persisted empty manifest, live
map, Parquet, checkpoint or fabricated label revision is required. This is
**initialized, publication pending**, not a complete empty analytics snapshot.
An existing valid namespace/index with no published store is also valid, including
the state after explicit reset. Existing publications remain unchanged.

A second successful call returns the same binding without refresh, publication,
namespace regeneration or incarnation change; the ownership lock may be opened.
Check refusals before store recovery, so initialization does not discard ambiguous
bytes. Existing writer recovery of supported uncommitted generations remains the
writer's job after admission, not an initialization fallback.

Closed error kinds are `unsupported_state` (index/store format),
`inconsistent_state` (malformed/unsafe paths, missing namespace beside published
state, namespace without an index, corrupt or partial initialization),
`writer_busy`, `binding_mismatch` (requested namespace/incarnation or local ownership
differs), `io`, `randomness_unavailable`, and `cancelled`. Include a bounded stage
and operator guidance, without source contents or secrets. Failed creation can
leave partial files; preserve and refuse those on retry rather than completing an
unproven binding. No automatic reset, deletion, migration, read-triggered rebuild
or broader source access is authorized. Native roots remain the existing reader's
job; rebuilds still require available authorized sources and custody proof.

## Callers and enrollment

- `analytics materialize` calls initialize, then keeps its existing strict Catalog
  refresh, bounded publication, compaction and optional watch semantics.
- Analytics/body `connect` calls initialize before pairing begins. The replacement
  point is `connect::producer_admission(kinds, analytics_state_dir)` (#651, on
  `main`). That seam needs only a read-only function returning exactly those three
  values, or the refusal “run `semon analytics materialize` first, then connect”;
  nothing else. Replace its interim read-only producer-binding precheck (the seam
  returning “run `semon analytics materialize` first, then connect”) with this call,
  retaining its expected-binding validation. Mirror-only connect skips it. Local
  initialization grants no sharing permission; B4 still gates enrollment/source publication.
- Proposed `start` calls initialize, then runs the local producer and authenticated
  Viewer. Merely opening Viewer, MCP, query, push or receive never initializes.

Enrollment consumes the returned `{ namespace, origin_incarnation,
index_incarnation }`; `Binding::Admitted` already carries both incarnations, and the
body cache and assertions key on namespace + `origin_incarnation`, so they survive
index rebuilds. No rename or collapse. Consequences: a changed `origin_incarnation`
makes the receiver refuse the old enrollment and the operator re-pairs with a new
enrollment ID; a changed `index_incarnation` under the same origin needs a
receiver-side rebind, which the origin lane defines in PR4/4b, not here. Claim epoch
remains unassigned until slice 4b. The caller must revalidate the binding at
activation if ownership was released while waiting for user input. See
[enrollment bindings](asi-enrollment.md#claims-epochs-and-heads).

Reset keeps its existing confirmation/export-or-waiver and lock requirements.
Ordinary reset retains namespace **and Event Index**, so initialization reuses both
and leaves publication pending. `reset --new-namespace` removes the namespace and
egress handles, not the index: the next explicit initialization creates a fresh
namespace and a fresh `origin_incarnation` but reuses a valid index incarnation. Any namespace, origin- or index-incarnation
change requires explicit enrollment rebinding/4b reconciliation; no silent claim
reuse or transfer.

## Operator outcome and implementation gates

Messages (UI/UX reviewed). They state only what has happened and never echo paths,
source contents or secrets.

- Initializer returns `created`: “Local analytics initialized.” `existing`: “Using
  existing local analytics.” Neither claims work the initializer did not do.
- “Preparing history; analytics is not ready yet.” appears only once the producer has
  actually started. If a published snapshot already exists, keep showing it with its
  freshness while updating; do not call all analytics unavailable.
- Every refusal ends progress visibly with a next step:
  - `writer_busy`: “Another process is writing local analytics. Wait for it to
    finish, or stop the Semon analytics writer you started, then retry.”
  - `unsupported_state` / `inconsistent_state`: “Local analytics state is
    unsupported or inconsistent; existing data was preserved. See the analytics
    export/reset recovery guidance before retrying. Do not delete the state
    directory.” Link the [existing guidance](asi-store.md#generations-publication-and-versions).
    Do not prescribe reset where export/reset cannot safely read the state (an
    unsupported or ambiguous store needs the matching older binary for custody
    export first).
  - `binding_mismatch`: say the local analytics do not match the requested
    enrollment and the existing data was preserved; re-run connect against the
    matching setup, or use the same recovery guidance when ownership is unclear.
  - `io` and `randomness_unavailable`: say setup could not complete and nothing was
    deleted; check the state directory's permissions and free space (or system
    randomness), then retry.
  - `cancelled`: say setup was stopped before completion, no sharing was granted and
    existing data was preserved; retry connect when ready.

**Pending state.** Before the first valid publication show pending/unavailable: no
zero totals, no empty-success dashboard and no “no history” claim. An error ends
progress visibly with a next step. A real completed zero-source publication is
distinct from pending and may show empty results. Initialization alone never marks
pairing or evidence ready, nor first publication, source availability or body-read
readiness.

B returns from the interim **five actions** (four target actions plus separate
materialize) to **four**: receiver command, origin connect, protected invitation
transfer/entry, combined trust/source approval. Origin setup uses one connect
command instead of materialize plus connect. A's target remains one startup command
after installation; browser-opening fallback and external setup stay separately
counted. This amendment alone does not deliver integrated startup.

The implementation PR must test fresh and existing-index initialization; repeat
calls preserving all three IDs and published bytes; `origin_incarnation` persisted with the namespace, constant across index rebuild, new for a new namespace, and its continuity checked against the manifest; zero-source initialization without a
fake manifest/revision; concurrent initialization/writer/reset exclusion and
continuous ownership handoff; every refusal kind with existing bytes preserved;
crash/partial-state refusal; expected-binding mismatch before pairing; mirror-only
bypass; connect cancellation without grants; ordinary/new-namespace reset and
explicit rebinding; the pending state (no zero totals, empty-success view or “no history” claim before the first valid publication, distinct from a completed zero-source publication, existing snapshot kept visible while updating); an actionable, path-free message for every refusal kind; unchanged read-only `not_materialized` behavior; and materialize
still draining strict native publication without a recovery/reset fallback. Run
Rust gates in CI; this proposal adds no implementation or local cargo/node checks.
