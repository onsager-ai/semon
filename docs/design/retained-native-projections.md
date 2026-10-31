# Retained native projections

See the [native storage contract and partial port inventory](session-storage-contract.md)
for the #478 backend boundary and explicit remaining work.

A missing local source is an observation failure, rather than proof that a
session never existed. Catalog format 3 records `current` or `retained`
lifecycle. Full native publication still checks the captured collection
revision, committed source membership and every complete consumed-prefix ledger
before changing the projection. Sources absent from that complete publication
retain their metadata and source mappings; their lifecycle changes atomically
with the collection generation. Native event-cache ledgers can retire without
removing the original catalog provenance or versioned transcript recipes.

Current-format transfer enumerates reachable immutable prefixes even after their
live file rows retire. Export selects an exact live ledger header or its exact
retained header, as the normal Native reader does. Retiring an owner moves its
selected proof reference from Current to History in the publication transaction.
`TransferInbox::admit_catalog_from` requires the actual completed frozen source
capture and the exact applied target checkpoint. Its destination operation keeps
the original publication receipts and retained rows, and commits a new import
receipt with the Catalog head CAS. It neither replays completed producer staging
nor creates reader leases or acknowledges pending claims. This Catalog-scoped
effect reports partial completeness; full inventory, catchup, fenced ownership
cutover and qualified rollback remain separate unqualified obligations.

Ordinary imported Catalog/Query reads preserve the producer's runtime fingerprint
and validate its per-row selected proof, original receipt and presentation root
under the destination's current admission grant. They compare that original
binding with the current authoritative runtime instead of changing stored
observations. Re-enrollment cannot revive the old import grant. SQLite records
eligibility for the applied Catalog/query closure separately from full-source
completeness, which remains partial. A real native component publication may
inherit an exact qualified imported listing base after affected query/projection
work and claim acknowledgements leave no invalidations. Local publishers mint
separate exact-row observation authority only for their actual outputs; transport
and carry never mint it. Later local heads therefore cannot replace the original
import grants for carried observations, even when admission counters coincide.
Cost-only SQLite repricing authenticates the exact stored preimage against its
own Current or History local grant, or its qualified immutable local Stage proof,
before replacing metadata and the matching owner grant in one transaction.
Same-source History qualifies independently; divergent cohorts retain their
original metadata. Missing or mismatched authority leaves an observation unchanged.
Imported receipts alone cannot authorize repricing or mint local grants, even
when producer and destination admission counters coincide. Original Stage proofs
remain immutable; a SQLite grant-write failure rolls back metadata and pricing
progress together.
Forward and reverse reads with authorized
originals and absent local Raw remain subject to exact-revision CI qualification.

Runtime freshness uses per-session Facts/PID dependencies inside the existing
namespace/admission/stream observation envelope. Native identities, names,
reported runs, process evidence and source-CWD repository facts share one derived
recipe on both engines. CWD metadata comes from an exact ledger in the qualified
storage snapshot; shared derivation never opens an Options-derived SQLite file.
Captured runtime observations distinguish the owner, exact source generation and
canonical typed dependency set, including source-CWD repository dependencies.
Final Query rows and queryless family endpoints can share an owner and sources
while retaining distinct observations. Replay derives the same identity from
the row and a captured source-cohort CWD-set fingerprint, independently of saved
Query runtime strings. Sorted CWDs are hashed incrementally with length prefixes;
only the fixed-size fingerprint enters the capture, even for a long native CWD.
Conflicting observations for one identity or CWDs for one source cohort
refuse capture; the existing 260-observation bound still applies.
Ordinary PostgreSQL full, partial and closed publications carry this typed capture
with a separate raw global runtime fingerprint and its current storage witness.
The producer reuses its original runtime snapshot and in-memory source CWDs,
recording each scope before assigning the row's Query runtime; its existing
final global check brackets that capture. The physical publisher qualifies every
row against its captured scope and uses only the witnessed global for owner
state. Semantic publication additionally binds the raw global to its stage.
Exact request replay precedes live qualification. Initial empty generic
publication refuses; an empty successor retains the prior owner's runtime, while
an empty semantic publication retains its explicit stage global. PostgreSQL's
existing 64-row request bound remains; larger SQLite full/Sources publications
keep their existing behavior outside this PostgreSQL qualification.
The PostgreSQL producer prepares identity-and-activity-addressed dependency
digests with its existing current Facts head, so ordinary reads use point keys
without parsing whole Raw Facts. Immutable presentation manifests capture each
row's derived runtime observation. An unrelated runtime change preserves a
row's observation, while an unproved owner or dependency remains unavailable.

Lifecycle describes the read projection. It does not establish or revoke
credential custody, runtime ownership, harness authorization or current native
selection. Exact Current identity may use qualified retained metadata when
current native facts and independent durable ownership still bind it. Retired
native manifests remain outside Current scope. Explicit retained-history reads
can inspect past identity without gaining current control authority.

SQLite semantic catch-up accepts a removed path only with its durable ordinary
producer removal invalidation and no remaining live ledger. An exact index
retained by the ordinary removal writer remains historical custody; staging
captures and rechecks its ledger without making it a live source. Index preparation
observes physical absence under its existing Source locks before recording it;
the write transaction consumes that observation with the exact claim/ledger CAS.
Each later reduction and publication rechecks the persisted evidence and exact
claim/ledger, without filesystem I/O under the SQL write lock.
This completes the existing staged path and retains outgoing owners through the
ordinary custody publication; it does not fabricate a Facts admission withdrawal.
The ordinary SQLite metadata audit records physically absent, ledgered sources
through the existing removal writer after its physical observations, retaining
its Source locks and read snapshot through the ledger comparison and write.
PostgreSQL staging likewise consumes prepared physical absence with durable
removal evidence; an unledgered missing path alone cannot authorize retirement.
PostgreSQL Facts-admission withdrawals retain exact process-local Facts/path
fence provenance with the prepared model. Before each record or publication,
the adapter acquires the existing shared Facts and sorted negative Source locks
and revalidates the complete physical witness before opening its SQL writer.
The live guard remains outside the transaction callback through commit or
rollback, then releases even if the prepared model is retained. The SQL stage
rechecks persisted Facts selection and its existing exact claims, runtime,
epoch, model and Catalog witnesses without physical withdrawal I/O. Completed
publication replay still qualifies its original request and acknowledgement.
This exclusion covers cooperating SourceChange writers sharing the lock
domain. Advisory locks do not exclude arbitrary external writers; non-Unix
shared locks retain their existing no-op behavior and exclusive SourceChange
writes remain unsupported. SQLite's existing physical validation is unchanged.
Completed PostgreSQL native relocation keeps the old path's exact removal claim
until qualified successor publication acknowledges it. Re-observing that removed
path during semantic staging is a metadata-only no-op only when both exact native
ledgers, identities and immutable proof headers still qualify the recorded move.
The relocation transaction retains its verbatim original request under the old
path together with its exact hashed acknowledgement; an appended successor may
advance the semantic relocation witness without rewriting that receipt. A retired
marker alone cannot bypass the active-stage withdrawal guard or grant archive
custody. This qualifies the ordinary PostgreSQL producer only. Transfer of the
complete relocation authority, including its original receipt, acknowledgement,
retired ledger and mutable semantic witness, remains unqualified.
Resumed primitive indexing also sends physically absent, ledgered inputs through
the ordinary removal writer before recording. If that advances a claim, the
producer yields its old preparation. The next qualified negative primitive pins
the exact removal claim and retained ledger for later reduction checks.

Pending slot outcomes record the absence of a final native result, separately
from a recorded result with unknown exit status. Recipes never store live flags
or elapsed runtime. A current read may display an active call only when qualified
activity identifies that exact tool and start time; after that observation ends,
the same pending recipe reports a result not recorded. Retained history cannot
gain runtime authority from this display metadata.

Exact selected details retain Claude copied-usage evidence and Copilot shutdown
usage with its native observation. These recorded facts remain bound to the
header's source generation and do not enter list pages. Unknown original usage
ownership, approvals and lineage remain unknown; a usage snapshot grants no
runtime or control authority.

Current list indexes are partial B-trees (`WHERE lifecycle='current'`) ordered
by their filter and keyset keys. Retained rows do not enter those trees.
Separate unprefixed indexes support history across both observation
states. Current exact-key lookup remains a primary-key operation. Source maps
carry the same lifecycle and have indexed current native/path lookups. Default
current lists therefore do not scan unrelated retained history.

A retained recipe keeps its immutable slot generation and supported recipe
version. Each header also records SHA-256 of the typed source-reference vector;
this is source/projection agreement, not a complete archive-object hash. Source
references preserve the original physical observation and SHA-256 of the full
consumed prefix `[0,offset)` plus its final up-to-4-KiB window. Neither marker
binds bytes after the consumed offset. A host must independently authorize and
verify an archived generation before supplying a requested source range.

Selected transcript windows, turn anchors and annotation pages resolve their
Catalog identity and load their recipe headers/rows in one read-only SQLite
transaction. The global Catalog/slot publication marker must still agree inside
that snapshot; a missing or mismatched marker is never repaired by a reader.
Concurrent atomic publication therefore cannot pair an old selected identity
with new recipes or spuriously reject a fresh first page merely because an
unrelated owner changed. Current and retained-history still choose their own
source cohorts and recipe tables. Explicit slot/error generations still reject
stale continuation requests. Web reads now retain this metadata snapshot through
lazy byte-proof rendering, including continuation and native field expansion;
native/provider generation checks keep their before/after I/O fences. Focused
MCP transfers its selection snapshot to the request-owned proof readers and
retains its later metadata publication checks. The last proof reader drop
rolls back and closes the connection, releasing its WAL pin on every exit path.

Retained selected Query fields use the embedded recipes of their qualified
history Catalog and source-generation header. A newer Current publication may
replace the mutable Query header for the same native owner; that header cannot
invalidate the independently retained recipe. Selected fields and search recheck
the complete relationship context after rendering. PostgreSQL selected reads
also check the namespace/epoch-bound runtime observation before rendering and
recheck runtime inputs afterward. Search eligibility traverses long recipes in
64-slot pages under the existing transaction and metadata budgets.

Index mutation preserves the outgoing path proof only while Current or
retained-history source vectors reference its exact ledger. The reference
decision, copy and replacement are one transaction. The next path mutation
collects unreferenced proof metadata; existing request snapshots can still read
collected pages. The version manifest is bounded by published ledger references,
rather than the number of appends. This stores no native content, grants no
archive authority and does not change the schema. Rebuilt live proofs take
precedence over obsolete retained headers from parser reset. A previously
orphaned publication cannot be repaired by inventing proofs in a reader.

Selected Home/Trace repair acknowledges an unchanged ready observation or a
committed publication only. Losing its Catalog/source compare-and-swap returns
a retryable observation error, rather than reporting `ready` after discarding
the prepared metadata. This does not turn selected repair into a global Catalog
rebuild or grant authority to read different source bytes.

`source_projection_ready(options, root, path)` provides a bounded read-only
pre-eviction guard. It never creates an index, reads bodies, opens the global
EventCache or builds a model. It checks exact allowlisted path mappings, the
supported recipe version, per-session source digest and complete publication
marker. It prefers current observations, then qualified retained observations;
pending source observations, ambiguous references, missing headers and unsupported/stale versions return
not-ready. At most eight owner metadata records, each at most 1 MiB with 64 source
references, are considered. SQLite checks stored byte length before returning
metadata; over-budget context defers eviction. Their source
vectors are necessary relationship context for the selected source.

The result is historical source provenance, not permission to remove bytes or
control a native session. Before removing local bytes, an authorized host must
verify the remote original and the actual bytes against the reference, recheck
readiness under its lifecycle gates, and defer eviction if no qualified snapshot
exists. A consumed prefix may be shorter than the archived object; that remains
an explicit incomplete observation, rather than a claim that the whole object
was indexed. Archive locations, credentials and tenant policy do not enter the
public read model.

Exactly owned Codex turn outcomes remain recorded observations in current and
retained metadata. They never inherit writer liveness or archive read authority.
Parser 29 qualifies those lifecycle facts; current-source tree repair restores
them from verified source slices without importing retired Semon storage.

Semon reads only its current schema and parser contract. Producers and focused
readers refuse unsupported existing state without reset under the
[current storage contract](session-source-reader.md#current-semon-storage-contract).
Retained references, recipes, proofs, reported facts and archive custody remain
untouched, including when original native bytes are absent. Fresh empty stores
initialize; current stores reopen without repair DDL.
Selected handoff recipes capture both endpoints' observed field descriptors
before publication. Retaining one endpoint therefore does not depend on the
other endpoint continuing to publish that relationship. Each descriptor binds
its exact consumed source witness; a later Current source cannot silently
replace that witness during rendering. Existing selected context, host, proof
and byte budgets still apply.
This work remains under qualification: complete PostgreSQL business flow,
current-format transfer/reverse and the full paired host regression are required
before backend readiness can be claimed.
Physical-corruption recovery is unchanged. Unsupported individual recipe versions
in a current store remain explicitly unavailable; they are never interpreted as
current versions.
Native source reappearance can reactivate its catalog key through normal source
CAS publication. This retains the latest observation per canonical key, rather
than introducing arbitrary historical revision browsing. Explicit original raw
record retention remains the host/source contract.

The existing complete background builder and collection hash still perform
global producer work. This change protects provenance and bounds indexed reads;
it does not claim that the subsequent dirty-lineage producer is complete.

Current verification covers cold initialization, incremental writes and restart,
unsupported-state refusal, current-store concurrent reopen without changing original
bytes or custody, native source
reappearance, interrupted lifecycle retirement with the dirty observation still
pending, and provider-rendered ranges when local source ledgers are absent.
The exact-path guard rejects missing indexes without creating them, stale
collection markers, missing source digests and unsupported recipe versions.

An isolated indexed SQL check holds one selected current row and a three-row
request constant while unrelated retained rows grow from 1 to 20,001. The
returned metadata is identical and the partial harness/repo order
has the same SQLite VM-step count in both cases. This qualifies SQLite
execution work for the current index shape, not complete journey latency, browser work, memory or the background
producer. The reader scope/filter integration is a separate dependent change.

Exact five-tool summaries are published in the existing native Catalog metadata,
using the shared Query facts and state derivation. They retain integer token counts,
lineage, process observations and question identity, rather than inferring facts from
rounded presentation totals. A runtime observation digest fences process fields;
retained rows cannot inherit live process or pending-question authority. Question
text stays source-backed and request-local through the existing preview recipes.

Schema 23 adds indexes for the Query output window and recorded relationship/turn
closure. Published listing and stall reads do not decode transcript event rows.
Composite current/history recipes initially have no exact Query aggregate: the
existing authorized selected producer publishes those facts after qualifying their
whole source closure. Readers check source-vector agreement and recheck the selected
metadata witnesses after preview I/O. Incomplete, changed or over-budget observations
return unavailable, rather than reporting a current fragment as complete history.

Metadata Query reads reject current and retained source vectors outside the
configured, allowlisted native roots. A matching runtime digest or retained
publication does not grant authority to another source tree.

Catalog-selected Query search shares the five-tool matching, snippets, ordering,
line cache and stop rules. It visits eligible native session closures in order,
with a request-owned source reader only for reached sessions. Limit/byte stops
qualify remaining slot metadata before admitting another archive. It never restores
unrelated archives or constructs a workspace Query cohort. The reached session's
semantic derivation still reads its Event Index and bounded relationship context;
this is not a claim of bounded long-session range preparation.

A seven-sample debug workload holds one matching session fixed while unrelated
old sessions grow from 0 to 128 to 512. The former cold Query cohort loads 1, 129,
and 513 source indexes (median 5,618, 278,865 and 1,147,614 microseconds); Catalog
search loads one each time (5,726, 5,716 and 5,731 microseconds) with identical
JSON. Metadata/edge/turn counts remain 1/1/1; indexed selection VM steps are
292, 295 and 295. These measurements qualify this stopped search workload,
not broad search, long selected history, total memory or production latency.
Reproduce with `cargo test --locked -p semon-sessions
catalog_search_selected_work_stays_fixed -- --nocapture`.

A host may transfer existing Catalog observers into `SessionSources` as one
admitted machine set. The same native evidence, index and refresh coordinator
survive that transfer; admission does not close a healthy writer or reopen its
index. The host must transfer lifecycle ownership and retain custody/admission
fences. Closing the admitted set closes those original observers too.

For hosted sources whose Raw and Facts changes are journaled, an unrelated
runtime observation can advance an active semantic stage's runtime fence without
resetting its native identity/relationship progress. Facts invalidation includes
indexed staged owners before their first Catalog publication and their native
parent/PID dependencies. Each batch still checks the exact source claims,
consumed ledgers, native roles, Catalog generation and prepared runtime; changed
dependencies reset through the existing witness checks. Unjournaled/local
observations and staged negative source-custody proofs retain the conservative
epoch reset. This lets a large historical component finish while unrelated
sessions start and stop, so the pending new-source journal remains reachable.

Selected relationship pages derive sender-turn witnesses from the physical
sender's complete native receipt boundaries. Indexed canonical receipt lookups
preserve the existing first-receipt and copied-prefix rules without admitting
additional physical sources or publishing unselected relationships. The
temporary boundaries are discarded before staged relationship capture; the
existing parent-turn conflict checks remain. Earlier staged recipes rebuild
once so partial-page turn witnesses cannot survive into publication.

Final owner publication replays incident relationships through indexed read-only
pages of at most 64 relations and 1 MiB of metadata. A single owner's complete
incident set may span pages; it is not required to fit one page. Each page
qualifies the stage epoch, and the final publication retains all source, runtime
and generation fences. Replay preserves producer relationship/placement order
and compact sender-turn witnesses while discarding each metadata page. Only the
selected owners' native derived model is assembled, outside the SQLite writer
transaction; unrelated component sources are not admitted.

The native Event Index connection reuses at most 32 compiled SQL statements for
per-source ledger checks and row writes. Bound values clear when a statement
returns to the connection cache; generation checks still execute within each
existing transaction. A separate-connection regression changes schema/parser
versions after the statements are warm and verifies the old owner refuses both
write and removal without changing the original ledger.

Three isolated cold-Catalog diagnostic runs before/after this change measured
2108-source indexing times of 1905/1975/1533 ms and 1200/1795/1099 ms respectively;
complete-test times were 7.99/7.81/6.15 s and 5.87/7.20/5.89 s. Runs without
compilation reported 174616–174984 KiB peak process RSS across both variants.
The first page still indexed only 64 sources with unrelated history present.
These local samples qualify reduced compilation overhead; they neither prove the
CI contention gate passes nor establish a production latency or memory bound.


## Paged original custody on cross-harness key replacement

A replacement harness advances original-custody transfer in the existing Event
Index publication owner. One attempt copies at most 64 recipes and 1 MiB of
recipe bytes. Private checkpoints retain the original Catalog/header witness,
slot cursor, partial-byte cursor, copied count and qualification state. Current
and its durable source claims remain pending while a transfer is incomplete.
`Deferred` is progress, distinct from a generation conflict; the worker releases
prepared Index payloads and resumes through its ordinary publication queue.

Partial recipes have no History Catalog row or qualified projection header.
Cancellation before commit rolls back the step; process replacement resumes its
committed cursor. Each attempt first checks the ordinary parser, publication,
source membership, ledger and source-revision fences. A changed original
Catalog/header witness causes bounded cleanup of private staged rows before a
new copy starts. Unrelated publication can proceed between steps. Final History
metadata and the replacement Current publish under the existing transaction.
A batch with several replacement owners checkpoints at most one owner per step.

Supported recipes retain their exact bytes and slot order. Unsupported original
headers retain their exact nullable header separately and remain incomplete.
An unsupported individual recipe larger than 1 MiB is retained in ordered byte
parts in `session_history_slot_parts`, without whole-value concatenation; its
history remains unavailable to ordinary recipe readers. Source references,
generations and existing archive markers are retained unchanged. This is
custody preservation, never a grant of control or new eviction authority.

The current schema creates the custody-part table together with the rest of the
storage contract. Recognized obsolete stores are discarded by writable producers,
without rebinding old rows, headers, archives or facts. Custody preservation of an
unsupported individual recipe in a current store is distinct from whole-index
invalidation; neither permits deletion of original native or archive bytes.
