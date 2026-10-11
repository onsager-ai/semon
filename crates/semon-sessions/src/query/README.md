# Query contract reference

Embedding callers replace `Query::from_core(core)` with
`Query::from_native_core(core)`. The host still owns authorization, admission and
source custody; the native constructor preserves its configured output window.
CLI/stdio callers keep `Query::new` or `Query::with_machines`. Existing hosted
production pins keep their original API until the paired consumer adopts the
new Semon revision. No constructor alias or full-model fallback remains.

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
Removing the duplicate launch scan and qualifying parent-marker/EOF evidence
and hosted generation ownership remain separate migration work.

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
preparation use the same reader and publication fence. The legacy Model target
retains its existing body reader
while its remaining Viewer consumers are migrated.

Native text and scalar preparation reuse at most one immutable, verified 64 KiB block within
the current preparation request. Adjacent short records do not repeatedly read
and hash that block. Cache hits retain file-identity/deletion checks and return
only bytes already verified for the old cohort; completion verifies all used
blocks before publishing. Public ranges and transcript/search continue to
verify their actual reads independently. No cached source body
survives the request.

The producer still discovers, indexes and joins eligible native sources. Hub MCP
still restores archives before admission; this local reader does not supply
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
version-four marker binding its exact persisted encoding and the source byte
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
header encoding in the version-four marker. Prior version-one, version-two and
version-three markers still retain their existing event rows; they cannot
qualify another eviction without current proof repair and custody verification.
Restore verified original archive sources before downgrading to a producer
that cannot recognize version four. Native logs, sidecars and existing original
databases are never removed by this proof migration.

Semon parent markers, bounded relationship closure and authorized provider reads
remain. No full-workspace discovery or archive cost is claimed retired yet.
