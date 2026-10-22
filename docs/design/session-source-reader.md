# Native session source readers

`SessionSourceReader` is a provider-neutral per-request callback for native
source ranges. It supplements the logical `SessionSourceRef` identities emitted
by the focused catalog. It provides no authorization grant or raw HTTP route.
The host selects the authorized machine and source generation; Semon retains
native record and transcript rendering rules.

```rust,ignore
fn read_range(
    &self,
    source: &SessionSourceRef,
    expected_generation: Option<&str>,
    offset: u64,
    max_bytes: usize,
) -> std::io::Result<SessionSourceRange>;
```

One callback request is limited to128KiB. A range returns its independently
bound generation, logical length, actual offset, immutable bytes and cached
observation flag. The range deliberately has no Serialize implementation, and
Debug prints byte length rather than source content. Native bytes must feed
parsed views under the applicable source/forensic permissions; ordinary trace
responses must not expose the original record bytes or their carrier labels.

A source reference is provenance, not bearer authority. The host reauthorizes
every callback, including byte-cache hits, against the request, current machine
registry, logical root/path, native identity and source generation. First reads
may resolve the verified generation; later chunks supply it explicitly and
require the same generation before and after I/O. The callback must reject
missing, ambiguous, stale or independently unverified source bindings. The
consumed-prefix SHA covers `[0,source.offset)`. A verified whole archived-object
SHA binds that prefix directly only when the consumed offset equals the object
length. An incomplete prefix needs its own independently retained proof;
reading a small range must not trigger a complete-prefix rehash or silently
claim a different object as the requested snapshot.

The reader belongs to one authorized request. Pass it by reference into the
parsed renderer; never retain it or its credentials in a shared ViewerCore,
workspace model, source cache or persistent projection. Async hosts run native
rendering outside their request runtime and bridge their existing bounded I/O
service. The reader captures the request's whole-operation deadline and
cancellation signal: a per-chunk timeout alone can multiply across a large
native line, and blocking rendering does not automatically stop when the client
disconnects. Keep diagnostics sanitized; provider URLs, object keys and
credential errors must not leak through parser failures.

For selected paged transcript reads, the native renderer loads Catalog identity
and slot recipes from one SQLite snapshot and retains its Catalog/slot marker
check. A host that binds a provider before that snapshot must compare the full
selected identity/source witness with the rendered identity, including machine,
read scope, canonical key, harness, owner qualification, native lineage and every
exact source reference (path, consumed boundary, prefix and tail proofs). An
unrelated Catalog generation advance alone does not change that witness. This
comparison also covers qualified local reads that bypass the provider callback;
it neither authorizes a changed source cohort nor replaces host access checks
before and after rendering. Byte generations and explicit page/field/error
generations retain their independent fences.

`LocalOrPinnedProvider` keeps exact stat equality as the local fast path.
On a changed physical witness (append, copy/remount, touch or a missing legacy
change token), a local carrier may instead supply only ranges whose covering
64 KiB blocks verify against persisted native proofs for the exact original
ledger, consumed boundary and prefix generation. One request-local verified
block is retained, and physical block reads charge the existing page/field or
attachment byte budget. The named file and descriptor
must agree before/after I/O. The request pins the first physical observation;
later physical changes fail closed. A small read never rehashes the full prefix
or reparses history. Missing or incompatible proofs remain unavailable; readers
do not manufacture proof for legacy/provisional rows.

If local qualification fails before that source's first accepted bytes, a host
with independently verified originals may use the exact selected callback.
Each source's local/provider carrier is pinned for the request; a later failure
never switches carriers. The hosted paged transcript/field and focused MCP
Catalog/selected reads explicitly opt into this qualification, including when
the host has no provider. Complete Query, selected Home and `LocalThenProvider`
retain their existing semantics. Metadata-only observe acknowledges or repairs
the selected metadata generation; it neither refreshes physical witnesses nor
proves that a transcript body can be read.

Native transcript 409 replies include a closed `fence_reason`: Catalog
publication, projection/errors generation, local source revision, source
generation or source range. These codes contain no path, body or provider error.
Local revision failures add
a closed `local_revision_reason`: identity, change_time, size_grew, size_shrank,
modified or missing_change_token.
Focused MCP `read_transcript` and `get_session` failures preserve the same
closed local-revision diagnostic in their tool error (`stale_projection`,
`status: 409`, no entries or session). The MCP transport retains its normal
JSON-RPC tool-result envelope. A host may retry an unanchored first page for a
Catalog publication conflict, but must return a byte-read fence without
rebinding it to a newly stable local carrier.

Complete selected native preparation also accepts `SelectedReadControl` through `Query::call_selected_with_control` and `repair_session_overview_with_control`. Hosts pass the existing whole-request deadline and cancellation flag, retaining their async guard until blocking work exits. Existing entrypoints use a 20-second default. The request checks cancellation during SQLite work, row decoding and native derivation; cancellation or exhausted preparation limits never trigger source-body replay or partial publication. Nested preparation shares its original accounting budget.

The complete selected fallback admits at most 131,072 decoded rows and 64 MiB of preparation accounting, charging fixed row representation plus eight times dynamic cell bytes before decoding. Individual SQLite rows are limited to 32 MiB. These are explicit work/availability limits, not measured whole-process or allocator peak bounds; the native body fallback retains its separate existing 64 MiB/1,024-request bounds and shares preparation accounting. An over-limit selection returns unavailable with a narrower-selection recommendation rather than truncated success. Successful admitted requests retain complete native semantics and may load complete selected Indexes; this path is not history-independent. The replay path charges raw body bytes eightfold before buffering and parsed line bytes eightfold again, plus fixed event/catalog costs, against that same 64 MiB budget. Its effective raw-body replay ceiling is therefore about 4 MiB before those other costs, often less; the separate 64 MiB raw I/O ceiling is not an availability promise for replay. A long selected owner can return unavailable during complete preparation even when its requested transcript page is small. Narrowing turns or page size, or a time window that still includes the same complete source, does not reduce complete Index preparation; excluding source files can. The normal generation-bound native paged transcript/tool path remains available for long sessions and does not become complete-history replay.

The native-line oracle requests bounded chunks, pins generation and length,
rejects incorrect offsets, overlarge/empty responses and cross-generation reads,
and reads only complete consumed lines. It reuses the local transcript's JSON
object parser and line budget. Tests cover a three-chunk record, generation
replacement, trailing partial bytes and a reference at the consumed boundary.

This checkpoint establishes the callback contract, not archived Viewer
activation. The production per-request renderer registration, versioned
persistent transcript slot projection, indexed range loading, source/relationship
resolution and full native page/tool oracle comparisons remain pending. A full
selected FileIndex decode must not become the normal per-page fallback.

## R5 publication and proof audit (Phase A)

This audit describes the pre-repair implementation. A confirmed diagnostic defect is distinct from a reachable publication interleaving, and neither establishes the live HP Code 1 failure without stage evidence. The sanitized 2026-10-21 05:21 UTC sample selected 27,751,814 consumed bytes and returned `size_grew`; it did not sample Raw length, Index ledger or proof header in a common snapshot.

Raw owns physical bytes. Receiver `SessionSourceWrite` withdraws append trust before mutation and completes after fsync under its publication fence. The durable mutation/invalidation journals retain generation-bound evidence and monotonically increasing claims across restart. These mechanisms protect against acknowledged-but-unindexed writes; they do not make Raw, Index and Catalog one transaction. Unjournaled local writes retain conservative verification.

The incremental parser owns the complete-record consumed boundary and prefix/tail digests in its ledger. Partial EOF records are outside that boundary; a cold checkpoint may consume approximately 4 MiB plus one complete long record. Persisted chunk continuation checks the old prefix before resuming. Index ledger CAS and physical rechecks protect parsing/commit races. Raw may lead Index without making its unconsumed suffix readable.

`write_one_dirty` commits Index ledger, native rows and `chunk_proofs::write` together in one SQLite `BEGIN IMMEDIATE` transaction. The same database file also contains Catalog, recipes and proof metadata: `EventCache::path(options.cache)`. `invalidate_catalog_source` only advances the durable revision journal. Catalog publication is a later `BEGIN IMMEDIATE` transaction in `publish_catalog_with_writer`, with collection-generation, source-ledger, claim and closure checks. Catalog identity, recipes and source context advance atomically; failed claims remain pending on restart. Index can therefore lead the published Catalog. This is a reachable interleaving, not a missing Index/Catalog CAS.

The confirmed proof-lifetime gap is that the proof namespace is only a hash of the physical path. Index L2 overwrites its one header/pages/tree, while `Focused::with_db` requires exact equality with selected Catalog L1's full ledger, parser and block count. There is no retained L1 proof reference or reader pin. Its exact-ledger guard protects integrity but can withdraw availability before Catalog catches up; a causal test must establish the observed rejection stage with Raw held stable. Restart preserves the same latest-only mismatch rather than repairing it automatically. Current and retained-history recipes can both outlive Index rows, so any later retention design must account for both cohorts rather than assume two versions always suffice.

Web selection loads identity and recipes in one SQLite read transaction and commits it before local bytes are read. `Focused::open` then opens a separate connection, and each hash lookup starts its own transaction. Thus the metadata snapshot does not pin proof pages for the request. A captured proof also rejects publication changes between block reads. The stat fast path, exact source witness, block hash, named-file/descriptor recheck, byte budgets and no-partial-response fence already protect their respective boundaries. The confirmed diagnostic defect was mapping any proof-open/read error to the initial physical stat reason, concealing header, page, hash and database failures.

Focused MCP captures proofs before creating its Records reader. `capture_focused` opens the exact selected ledger on another connection; previously `query/transcript.rs` discarded that error as generic `io`, which Hub's `query_result` reduced to `unavailable`. A typed error need not have existed before this point. Records separately retains physical-revision failures through tolerant native parsing. Web may retry a Catalog publication conflict on an unanchored page; a source fence must remain explicit without silently rebinding the carrier or returning partial content.

Phase A preserves these read decisions while adding closed failure stages and a deterministic Catalog L1 / Index-proof L2 checkpoint. Phase B requires its CI evidence first. The repair invariant is that selected recipes, consumed boundary and covering proofs remain compatible and obtainable for the request lifetime while background Index advances. Append availability must be asserted separately from safety: a verified unchanged committed prefix should remain readable during append, while changed old bytes, truncation and replacement still reject before response publication. Request work must remain proportional to covering blocks. Retention, GC, concurrent old readers, retained-history references and restart are design obligations, not established properties of the current implementation.

Diagnostics are bounded observations, not a common database/Raw snapshot. Header failures use a per-source transition limiter with at most 256 retained sources. An MCP observation without Raw may be upgraded once by a Web observation with `raw_before`/`raw_after`; later MCP/Web repeats of that transition are suppressed. Web stat, mode, observation and recheck failures use the same limiter. Eviction can allow a transition to be logged again. `proof_offset=None` is expected for proof headers written before this build, which did not persist the optional consumed-offset diagnostic.

Publication diagnostics hash each distinct candidate path once per attempt and log at most 64 sources, prioritizing the bounded recent failure-interest set. Before a source's first failure registers interest, a target outside that fallback sample can be absent; its next eligible publication attempt is covered after interest registration while it remains in that set. Interest expiry, eviction and sources absent from the candidate batch remain coverage limits. `invalidations_total=unavailable` means its independent count query failed, not that there are zero pending invalidations. Staged focused-query failures retain the `io` code and its JSON shape with only an additive `stage`; native Web fences retain their existing status and fence kind. Diagnostic availability does not establish HP Code 1 readability.

## Published proof retention and request snapshots

Before replacing or clearing a path's live proof, the Index writer preserves
the outgoing proof namespace when a Current or retained-history Catalog source
still references its exact ledger. The reference decision, namespace copy and
Index mutation use the same SQLite transaction. The per-path manifest contains
only those referenced ledgers; the next source mutation collects superseded
metadata, including incompatible parser versions. This preserves content-free
hash pages and their tree/legacy witnesses, never native source bytes. No schema
version changes. A rebuilt exact live header takes precedence over obsolete
retained headers left by parser reset.

Web transcript, continuation and field reads retain the selection transaction
through lazy proof I/O. Focused MCP transfers its selected recipe connection to
the request-owned proof reader too; it no longer opens its initial proofs in a
different snapshot. Later MCP metadata reads retain their publication rechecks.
All covering proof pages come from the pinned snapshot even if a producer
replaces or collects their namespaces. The final reader drop rolls back and
closes the owned read connection on success, failure or cancellation, releasing
its WAL pin. A new request sees the new committed proofs and still rejects a
changed header, false page root, tampered prefix, truncation or replacement.

Legacy checksum preparation still commits bounded metadata steps independently.
A fresh read transaction qualifies its exact header, complete marker and selected
page witness together. If replacement or GC removed that preparation, the old
reader qualifies the original whole-vector fingerprint by streaming one hash
page at a time from its pinned snapshot. It never reconstructs proofs from Raw.
Modern rooted proofs continue reading only covering pages and tree witnesses.

Retention protects publications present at a future Index mutation. An upgrade
which starts with Catalog L1 already orphaned by Index L2 cannot recreate L1
proofs: reads remain unavailable until normal authorized producer publication
supplies a compatible projection. With unchanged selected publications, bounded
browser recovery retries after 1, 2 and 4 seconds, then displays a terminal failure
and manual retry while retaining previously loaded text. Changed selected source,
Catalog or identity publications can resume recovery inside a fixed 45-second
budget measured from the first failure; repeated metadata polls cannot extend it.
Identity notes cannot replace the terminal recovery explanation.
Neither recovery nor these regression cases
establishes original HP Code 1 acceptance under persistent push.

## Exact-path consumed-prefix proof

`session_source_proof(options, root, relative_path)` resolves one configured
native source through the indexed published source-path mapping and its current
file ledger. It decodes at most one selected relationship metadata row (1 MiB,
64 source references), compares the full device/inode/size/modification and
consumed-offset/prefix/tail revision, then reuses scoped catalog validation for
current native selection. Publication races, unavailable facts, changed roots,
missing source metadata and stale ledger/native mappings fail closed. The API
returns no native body and does not reopen or rebuild the event cache.

The result is an observation, not custody or permission to read an archive. A
host archiving source bytes can use it as a candidate, verify that file identity
still matches before and after copying, and hash the recorded consumed boundary
while streaming the actual immutable generation. Private host provenance and
access policy remain outside Semon. Missing proof for an older archived partial
source remains explicit until its original bytes can be qualified.

The proof helper uses the versioned `CurrentSources` facts index for header and
current native-selection validation. It does not deserialize the full recorded
manifest in its read path. Its before/after facts identity check and catalog
publication-generation check both fail closed on replacement.

A fixed one-source proof workload on 2026-10-07 increased unrelated native
manifest entries from 100 to 20,000 (recorded facts 2,750 to 520,150 bytes). Across
31 warm samples, complete helper median latency was 1,428 to 1,453 microseconds;
selected catalog query VM steps remained 20. The source-path query is separately
asserted to use `session_catalog_source_path`; indexed facts lookup scaling is
qualified by `facts::source_authority` tests. These measurements concern a
metadata proof helper, not first parsed transcript content or a browser journey.
Reproduce with `cargo test --locked -p semon-sessions
exact_source_proof_work_stays_fixed -- --nocapture`.

## Hosted workspace Catalog lists

`session_catalog_union` merges the existing persisted Catalog pages supplied by
an authorized host. It opens one source database at a time and retains at most
two page buffers. It does not construct a Viewer core, load selected details,
restore archived Raw, enumerate native trees or create a second index. Hosts
still authorize source membership and workspace access before and after reads.

A host may explicitly advertise `global_union: true`, `source_key: ""` and
`order: "last_desc_key_asc_machine_asc"`. Union rows add
`machine: { key, label }`; session keys remain source-local. Selected identity,
transcript and control reads always require a concrete source. Existing focused
Catalog contracts, filters and cursors remain unchanged.

Workspace cursors bind the workspace, normalized metadata filters, source set
and every Catalog generation. Ordering is descending recorded activity, then
ascending session key and machine key. Source changes return `stale_cursor`,
following focused Catalog resynchronization semantics under continuing writes.
A bounded metadata search publishes only through the earliest scanned source
frontier; later candidates are re-read from that global keyset on the next page.
This preserves reachability even when a source exhausts its search scan budget
before returning a full page or any matches.

The merge admits at most 64 sources and 100 output rows per request. Each source
retains its existing 512-candidate / 2 MiB search budget; the union search report
states the aggregate maximum explicitly. A source read failure fails the union;
it never becomes an empty source or a complete partial success. Capacity and
source-unavailable responses remain distinct. The hosted adapter adds bounded
request admission, a deadline and the existing Catalog publication coordinator.
