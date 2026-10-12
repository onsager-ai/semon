# Query contract reference

Embedding callers admit a `SessionSources` owner and use
`Query::from_sources(sources)`. The host still owns authorization, admission and
source custody; the native constructor preserves its configured output window.
CLI/stdio callers keep `Query::new` or `Query::with_machines`; these construct
native evidence owners directly, without ViewerCore, machine views, compatibility
state or refresh workers. Construction and argument rejection do not open the
source cache. The same native owner supplies the Viewer producer's complete
Event Index, text preparation and query cohort, so alternating Query, Catalog,
tree and native Query reads do not create a second producer cache. Source snapshots keep
the existing file, process, facts and writer-lock observations; request-owned
readers still verify source bytes and completion independently.

Ordinary single-machine CLI/stdio `get_session` and `read_transcript` reuse
focused Query when an existing Catalog closure is current. Qualification checks native file
generations and the Catalog role against independently current sidecar evidence;
it neither discovers unrelated sources nor initializes a workspace Query cohort.
The two-tool fixture loads four selected indexes with zero or 256 unrelated
sessions in both default-window and `--all` modes (previously one versus 257).
Selected derivation preserves whole-file mtime scan windows and omitted Claude
parent identities. Excluded sources are filtered before index/body adaptation,
including missing-ledger fallback; they cannot trigger archive reads. A wholly
excluded owner returns the ordinary unknown-session error without body I/O.
Local native IDs resolve through the indexed Catalog source owners when scan
exclusion changes a qualified spelling. The shared producer then derives the
requested identity from the bounded collision closure. Resumed sessions retain
omitted-file turns_truncated semantics. This local resolution grants no source
authority and does not relax public hosted selected-Catalog-ID validation.
An existing current owner's changed log or allowlisted Claude sidecar refreshes
through the same bounded Catalog component producer. Old and new relationships,
source revisions and publication generation remain fenced. Two tools after an
isolated append load five indexes with zero or 256 unrelated sessions (formerly
one versus 257); a rewrite loads four in both cases. Unsupported identity/role
changes, missing sources, cold Catalogs and multi-machine automatic selection
still require native reconciliation.

Ordinary `SessionSources::with_machines` constructs no machine view, refresh pool
or worker. Hosts with admitted Catalog observers transfer their existing evidence
owners and refresh coordinators with `SessionSources::from_catalog_observers`,
without closing producers or reopening indexes. Explicit Catalog demand attaches
the existing source producer to the host's bounded pool;
a later `ViewerCore::from_sources` borrows that same producer, index and query
cohort. Retirement stops producers; close drains them off the async reactor.
Hosts hold their existing blocking-worker and deletion guards through actual
completion and close attached Viewer transports before draining sources.
Hosted list/search/stalls use shared native Catalog reads; no hosted tool restores
tenant archives. Healthy observer transfer preserves custody fences and the
admitted source set owns subsequent retirement.
The paired Hub consumer admits native sources directly; the preceding
`from_native_core` constructor and its exclusive Viewer preparation path are
removed. Query owns only native sources. Substituting local scan-window
construction would change the hosted contract. No full-model query fallback remains.

Focused embedding reads use `Query::call_selected` with a host-selected machine
and Catalog key. `session_query_sources` returns the original references for its
parent, children, indexed handoff endpoints and native identity collisions;
references grant no authority. Identity closure includes cross-harness owners
and literal IDs occupying a qualified spelling, so focused derivation preserves
the complete producer's canonical keys. Alias lookup uses the existing source
table's native-ID index and shares the 64-source relationship bound.
Far-end stubs resolve through indexed native handoff owners, and unsent addresses
through existing native bookmarks. Machine-local union aliases remain scoped to
their configured presentation machine; these addresses grant no control rights.
The native producer reuses qualified current or retained Event Index generations.
It does not construct a workspace producer, restore files or publish metadata.
The request lends its reader and shares a 1,024-request budget through derivation,
paging and completion. Source context is limited to 64 sources and native
semantic reads to 64 MiB. An incomplete relationship closure refuses the request.
Provider generation and length stay pinned across separate semantic reads;
transcript bytes additionally verify the existing native block proofs. Completion
rechecks the selected source/relationship footprint, so an unrelated Catalog
publication does not invalidate a focused answer. Union machine labels and
configured windows remain metadata inputs; unrelated native bodies are not read.
Ordinary local listing, search and stalls preserve the native cohort contract.
Embedding hosts instead use `Query::call_catalog` for these three tools: list and
stalls reuse generation-fenced published facts, and search admits eligible source
closures only until the shared matcher reaches its result or byte stop boundary.
The same search engine supplies ordering, snippets and line accounting to both
paths. A selected source still requires full semantic preparation before its first
search slot; long selected-history range bounds remain an implementation gate.

Qualified `read_transcript` requests read a bounded window of the shared native
slot projection and original indexed byte proofs, without loading a complete
Event Index or preparing a semantic cohort. Catalog publishes exact call/error
counts and pending/background observations under its source/runtime fence;
recipes themselves grant no live authority. The shared renderer keeps original
slot addresses and the preceding Thinking timestamp. Turn anchors use the
existing selected-session turn indexes. Completion rechecks the selected row,
slot generation, touched sources and runtime, permitting unrelated publication.
Recipe windows stop at 512 candidates plus one preceding context row; selected
recipe metadata and content-free proof hashes each have a 4 MiB read budget.

Physical scan-window namespaces, aliases/virtual owners, clipped output-window
membership, composite retained history and publications without qualified
observations still use the existing selected native derivation. That remaining
path is bounded by its source/byte limits but prepares the selected history;
it is not a completed range-pager migration. Changed or corrupt admitted
publications and exhausted pager budgets return explicit errors.


The opt-in `selected_transcript_tail_scope_measurement` diagnostic grows one
archived selected session while returning exactly one tail entry. It compares the
complete response with the existing local oracle and verifies no native-file
restoration or workspace producer. At the production code in b73834b, five-sample
debug medians measured:

| Earlier replies | Source bytes | Prepared slots | Range requests / bytes | Read median µs |
| --- | --- | --- | --- | --- |
| 0 | 545 | 2 | 6 / 3270 | 3406 |
| 128 | 113203 | 130 | 5 / 274073 | 11193 |
| 1024 | 902603 | 1026 | 5 / 282977 | 28668 |
| 8192 | 7224779 | 8194 | 5 / 178529 | 106498 |

Every request loaded one complete selected Event Index. Remote bytes are not
proportional to this fixture's prefix, but prepared slots and read latency grow
while the returned scope stays fixed. These observations establish the remaining
selected derivation cost, not a latency threshold or production percentile.
The qualified pager prepares two or three recipes and loads no complete index
in this same fixture. Five-sample debug medians after the pager change measured:

| Earlier replies | Prepared slots | Range requests / bytes | Read median µs |
| --- | --- | --- | --- |
| 0 | 2 | 4 / 2180 | 1749 |
| 128 | 3 | 3 / 143001 | 4501 |
| 1024 | 3 | 3 / 151905 | 4760 |
| 8192 | 3 | 3 / 47457 | 3401 |

Content-free proof hashes still scale with the selected
prefix, within their read budget; this does not establish constant metadata work
for arbitrary selected histories or a production latency percentile.
Reproduce alone:

```sh
cargo test --locked -p semon-sessions --lib selected_transcript_tail_scope_measurement \
  -- --ignored --nocapture --test-threads=1
```

Recorded human-ask briefs remain content-free source offsets in the Query cohort.
Metadata and transcript paging do not materialize every ask's prompt. Selected
session reads resolve their requested briefs through the cohort's verified reader;
search resolves a brief only when its slot is visited, preserving native caps,
ordering, stopping and response fields. Completion rechecks the used source bytes.
Catalog observation and scoped preparation retain only the ask relationships.
The test-only semantic golden Model still obtains its reference brief contents;
production has no compatibility model owner or read fallback.

Native preparation also reuses existing scalar recipes only when their catalog
and complete source generations, version, row count and projection digest agree.
The source path, offset, block and complete recipe must match the new slot; current
turn placement and state still come from fresh native derivation. Reuse carries
no source authority: Transcript/Entry and Query bodies retain independent byte
and completion checks. Missing, stale, corrupt or contended metadata falls back
to source derivation; damaged rows are repaired even when their stored digest was
unchanged. The index schema, projection version and digest encoding are unchanged.
Discovery, relationship derivation and metadata preparation still visit eligible
native sources; this reduces body reads rather than proving bounded total work.

`contract-reference.json` contains 134 synthetic tool responses from the retired
production `Query::from_core` engine at Semon
`7ce1ac4d4f34af9143aa292b631a07fd797890b1`. It preserves the independent oracle
after the compatibility constructor and its model-reading implementation are
removed. It is test data; production does not load it.

The existing fixture families supply the requests and native evidence:

- 56 output-window cases: all five tools, 24-hour/six-hour windows, active sessions,
  retained links, one/two machines, paging, search budgets and exact errors.
- 78 scan cases: all five tools, one/two machines, native source rewrite and process
  disappearance, paging/turn anchors, search stopping and unknown-session errors.

The capture ran the parent's two comparison tests unchanged apart from recording
each legacy result before its existing assertion. Production `query.rs`,
`model.rs`, `viewer.rs` and `union.rs` matched the pinned commit byte for byte;
the instrumented tests passed. The clock is `1790294400000` (epoch milliseconds).
Responses contain synthetic identities/content and no checkout paths or secrets.
No compatibility engine or reference generator is retained in the product.

Current tests compare complete result values and error bodies against these
records, while checking zero compatibility builds and shared native cohorts.
The surrounding semantic fixtures still test identity, relationships, facts,
current/retained windows, closed-source retirement and source-backed
reads. Do not regenerate the reference from the current implementation merely
to make a changed result pass; an intentional public contract change needs its
own reviewed expectations.

Native query transcript paging and search borrow request-owned readers over
the cohort's indexed source proofs. They use the `SessionSourceReader` range
contract and native parsers, with no direct-file fallback in the Query consumer.
Reads stop at complete consumed lines and verify the requested bytes against
64 KiB block digests captured with the existing event ledger. The final partial
block stops at the original consumed offset, so a verified append preserves the
old cohort. Replacement and deletion remain fenced by file identity. A rewrite
of a requested block refuses the read even when size, mtime and change time all
match. Completion rechecks blocks actually used by this request; unchanged
Unix range reads do not hash an entire parent log or unrelated workspace sources.

File metadata still fences native discovery, cached metadata and publication.
Its unchanged-stat shortcut is an observation of freshness, not proof of
arbitrary whole-file byte immutability. Changed metadata retains the original
complete-prefix check. Platforms without a change-time observation also keep
their existing complete-prefix verification. The block proof establishes the identity of bytes the
native reader actually consumes independently of that timestamp shortcut.
Qualifying hosted generation ownership and scoped dependency reconciliation
remain separate migration work.

Block digests contain no source text. They are paged within the current Event
Index metadata region, bounded by the same metadata-value limit, and committed
atomically with the native ledger. Missing, unsupported or corrupt proofs are
repaired from an unchanged verified prefix without replaying event rows. Indexing
also checks the already indexed prefix and newly parsed bytes independently
against the final hash pass before committing. No database, table, schema,
parser version, public transport or body store is added.

The original pager's anchors, counts, entry rendering and line/search budgets
remain shared. Prompt reads retain the 64 MiB native prompt limit, image metadata
and range elision: repeated pages do not parse megabytes of base64 again. Source
proofs contain no bodies or credentials; readers and their verification caches
belong to one call. No query table, new database or public transport is added.

Native query derivation also borrows the generation-bound reader for names,
handoffs, questions, briefs and results. It preserves the producer's separate
64 MiB native-record limit and existing text extractors. Every contributing
source is fenced before derivation and rechecked before catalog/slot publication,
including cached derivations and empty sources that supply native identity.
A failed cohort is not installed as a query snapshot or published to the catalog.
The proof contains the same complete consumed prefix used by paging and search.
Catalog observation, bounded source preparation and native scalar-recipe
preparation use the same reader and publication fence. Full Model preparation
also uses this cohort for body, parent-marker and launch reads, with no direct
body-reader or independent marker/launch fallback. It checks used source bytes
before publishing Catalog/slot projections. Existing independent sidecar fences
for native targets and the Model metadata publication CAS remain unchanged.
Local paths without
a lossless UTF-8 encoding keep private, generation-bound reads; they never enter
the public source-reference lookup under a lossy alias. Public query/source
reference validation remains unchanged.

Native text and scalar preparation reuse at most one immutable, verified 64 KiB block within
the current preparation request. Adjacent short records do not repeatedly read
and hash that block. Cache hits retain file-identity/deletion checks and return
only bytes already verified for the old cohort; completion verifies all used
blocks before publishing. Public ranges and transcript/search continue to
verify their actual reads independently. No cached source body
survives the request.

The producer still discovers, indexes and joins eligible native sources. All
Hub MCP tools admit selected native sources or bounded Catalog facts directly. This local reader does not supply
missing archived index metadata or grant provider authority. Removing those
costs requires scoped dependency selection and independently authorized provider
access for the producer. Verified archive eviction now explicitly retains an
exact existing native index generation in the same database, instead of letting
missing-source reconciliation discard it. This metadata does not make a source
current, contain bodies or select the query's relationship closure by itself.

The native producer now reads Codex identity, ancestry, guardian/subagent role,
cwd and branch from an allowlisted first-record header committed with the event
ledger in the existing index metadata region. Cold queries reuse unchanged
headers. Missing, unsupported or mismatched metadata is repaired from the native
first physical record without replaying unchanged event rows. Header bytes are
rechecked before commit; valid unterminated headers retain the original identity
behavior without advancing the complete-line offset. Unknown fields and message
bodies are excluded. This is a private implementation detail, with no schema or
parser version bump.

New Codex archive eviction requires that generation's persisted header and binds
its exact encoding into the archive marker. Earlier markers still preserve their
event rows; they cannot qualify another Codex eviction without native header
repair. Metadata larger than 1 MiB remains complete in memory, is not persisted
or truncated, and cannot qualify eviction.

Claude subagents now use allowlisted sidecar metadata in the same index region:
call identity, description/agent type, model, worktree cwd and branch. The parent
identity still derives from the supported native path. A separate sidecar
observation records known absence or its physical-generation fingerprint and
full bounded-file digest, bound to the native event ledger in the same commit.
Safe descriptor reads refuse links at every path component. Cold queries reuse
unchanged sidecars; creation, removal, rewrite and malformed JSON preserve the
native fallback behavior without replaying unchanged log events. Generation
checks fence changes during indexing and native preparation. Repeated sidecar
churn keeps the native session and defers a portable proof until a stable scan. The former
`Texts::meta` cache is removed.

New Claude subagent eviction requires a supported sidecar observation and a
version-six marker binding its exact persisted encoding and the source byte
proof. The host rechecks the
sidecar observation and full bounded-file digest before local removal. Earlier markers still retain existing
event rows, but cannot qualify a new subagent eviction. Unreadable, linked or
oversized sidecars keep the native metadata fallback and defer eviction; parsed
values too large to persist remain complete in memory. No raw sidecar JSON,
unknown fields or authority is stored, and sidecar files are never removed by
this operation. Deployments must retain the matching producer before creating
new markers; restore verified archive sources before using an older producer
that cannot recognize their version. No deployment or automatic data migration
is part of this change.

New archive eviction also requires valid paged byte proofs and binds their exact
header encoding in the version-six marker. Prior version-one through
version-five markers still retain their existing event rows; they cannot
qualify another eviction without current proof repair and custody verification.
Restore verified original archive sources before downgrading to a producer
that cannot recognize version six. Native logs, sidecars and existing original
databases are never removed by this proof migration.

Native Catalog, Query and source preparation now resolve Semon parent markers
from content-free locations committed with the event ledger. The event parser
observes the first marker or preceding agent output, preserving the shared
first-output stopping rule. Only offsets, lengths and exact record digests are
persisted; handoff prompt paths and message content remain in the original log.
Missing or unsupported metadata is repaired without replaying unchanged event
rows. Every selected record is independently verified before commit, read and
publication, including timestamp collisions, replacement and deletion.

Valid JSON at EOF without a newline retains its marker behavior through a
separate exact-record digest. It does not advance the complete-line offset or
change transcript paging. Incomplete EOF JSON is reconsidered on append. Marker
reads preserve the legacy parser's uncapped record size; they read only the
selected record, and completion verifies it with bounded scratch memory.
File metadata remains an observation of freshness for cached negative results.

New Claude/Codex eviction also requires supported parent-marker metadata and
binds its exact encoding into the version-six archive marker. Prior markers
continue preserving their existing rows; missing or corrupt metadata defers new
eviction.

Native Catalog, Query and source preparation also resolve launch calls from
locations observed by the same event parser, without using the independent
full-parent launch scan or retaining command inputs in a resident cache. The
existing Bash/Skill, raw candidate prefilter, literal prompt priority, unique
filename and time/worktree ambiguity rules remain shared with legacy consumers.
Each parent retains the newest 4,096 calls within the existing 2 MiB canonical
input/id/name budget; native records larger than 2 MiB remain excluded. Only
locations, record/field digests, timestamps and budget metadata are persisted,
paged in the existing index metadata region and committed under its ledger CAS.
Restart and metadata repair preserve event rows. Valid EOF calls are provisional;
adding a newline indexes them once and leaves public transcript offsets unchanged
until that newline arrives.

Reads decode only the selected id, name and input fields. The entire containing
candidate record is independently verified once per read and rechecked on
completion with bounded scratch memory, binding its timestamp and tool type as
well as its input. Several calls in the same record share that check. Unrelated
parent records are not scanned by this lookup. New eviction requires supported
launch metadata and binds its fingerprint into version-six archive markers;
versions one through five still preserve already retained index rows but cannot
qualify another eviction. Restore verified original archives before downgrading.

The separate JSON summary cache and marker/launch scans still serve normal
`semon sessions` and Viewer tree reads. Their model lists, raw activity timestamps,
usage deduplication, open-tool state and malformed-line diagnostics require an
explicit migration before that cache is removed. Bounded relationship closure
and authorized provider reads remain. Product Viewer model transport and complete
compatibility Model construction are removed; global native reconciliation and
unqualified selected semantic preparation remain distinct scaling costs.
