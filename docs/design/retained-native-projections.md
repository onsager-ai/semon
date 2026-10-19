# Retained native projections

A missing local source is an observation failure, rather than proof that a
session never existed. Catalog format 3 records `current` or `retained`
lifecycle. Full native publication still checks the captured collection
revision, committed source membership and every complete consumed-prefix ledger
before changing the projection. Sources absent from that complete publication
retain their metadata and source mappings; their lifecycle changes atomically
with the collection generation. Native event-cache ledgers can retire without
removing the original catalog provenance or versioned transcript recipes.

Lifecycle describes the read projection. It does not establish or revoke
credential custody, runtime ownership, harness authorization or current native
selection. Exact Current identity may use qualified retained metadata when
current native facts and independent durable ownership still bind it. Retired
native manifests remain outside Current scope. Explicit retained-history reads
can inspect past identity without gaining current control authority.

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
by their filter and keyset keys. Retained rows do not enter those trees; schema
17 replaces earlier schemas' lifecycle-prefixed list indexes without rebinding rows.
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

On parser-cache reset, observations become retained while rebuildable event
rows are cleared. Original references and recipes survive. Unsupported recipe
versions remain unavailable until an authorized producer rebuilds them from the
original source; they are never silently interpreted as current versions.
Native source reappearance can reactivate its catalog key through normal source
CAS publication. This retains the latest observation per canonical key, rather
than introducing arbitrary historical revision browsing. Explicit original raw
record retention remains the host/source contract.

The existing complete background builder and collection hash still perform
global producer work. This change protects provenance and bounds indexed reads;
it does not claim that the subsequent dirty-lineage producer is complete.

Verification at the schema-11 checkpoint includes schema-10 coherent/stale
header migration, parser reset with absent source, native source reappearance,
interrupted lifecycle retirement with the dirty observation still pending, and
a provider-rendered native range after a complete rebuild removed the local
source ledger. The exact-path guard rejects missing indexes without creating
them, stale collection markers, missing source digests and unsupported recipe
versions.

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

Schema 27 adds the custody-part table through the storage owner's additive
migration. It leaves the parser version, original rows, headers and archive
markers unchanged; older schema-26 writers refuse the newer store. The migration
regression compares every existing MAIN/TEMP row and schema object before and
after opening the previous schema.
