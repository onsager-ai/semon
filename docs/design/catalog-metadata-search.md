# Bounded catalog metadata search

The source-scoped catalog accepts `q` with Unicode lowercase substring matching
within each of `name`, `key`, `repo`, `branch`, `model`, and `harness`. Lowercasing
is Rust Unicode lowercase conversion, not Unicode case folding. Terms never
match across field boundaries. Native IDs, source paths, transcript bodies,
tool arguments, and live state are excluded. The existing harness and repository
filters still apply. Empty trimmed queries use normal catalog paging. Queries
have a 1,024-byte / 256-input-character bound and reject control characters.

Persistent versioned metadata documents and one-to-three-character postings use
an index ordered by scope, selected gram, descending last timestamp, and ascending
session key. Exact gram membership counts select the rarest available query gram.
The requested page verifies at most 512 candidates and 2 MiB of metadata, with no
body reads, global catalog sort, offset pagination, or request-time row count.
Documents over 8 KiB of normalized fields or 1,024 distinct grams are explicitly
unindexed; `search.index_complete` remains false rather than asserting coverage.

A search response includes `search.semantics`, `fields`, `partial`, `candidates`,
`index_complete`, `candidate_budget`, and `byte_budget`. A page can have zero
results and a continuation cursor when false positives or additional filters
consume its verification budget. Clients must show the partial state and offer
continuation. The cursor resumes after the last checked candidate, including
rejected candidates, so continuation neither skips matches nor scans an unbounded
prefix. A normal full page also has a cursor when more verified results exist.
The generation, source scope, read scope, normalized query, and structured filters
are bound to the cursor; changes require a 409 resynchronization.

Documents, postings, counts, coverage, and readiness generation publish in the
catalog source/revision CAS transaction. Publication changes only affected
metadata documents after initial construction. Initial construction or a search
format migration rebuilds postings during background catalog publication; it
streams one metadata row at a time. Reads never initialize or repair that index.
A missing or incoherent search projection returns an actionable unavailable
response while preserving the normal metadata-only read path.

Search coverage is independent of source discovery. Partial source publications
index only their affected rows; upgrading an existing catalog does not assert
coverage of unchanged rows until metadata backfill completes. An indexed partial
catalog can return useful matches with `index_complete:false`. The collection
`completeness:{state:"partial"}` and updating freshness remain visible separately
from exhaustion of this request's candidate or byte budget. Non-search pages
return `search:null`.

Retained-history metadata extends qualified source/native references without
changing these indexed fields. Its materialized metadata and current metadata
share collection generation, while scope-specific postings keep current-only
membership distinct from retained browsing. This contract does not provide a
multi-source union or full-text transcript search.

Verification and measured complete-path results will be recorded after the
schema-15 producer stack and schema-16 search gates run. Endpoint work alone does
not establish cold-import or whole-browser journey performance.

Focused evidence for this slice:

- Four search tests pass: Unicode field substrings, unchanged requested results
  with unrelated postings growth, 512-candidate continuation through false
  positives, transactional updates/rollback, and explicit oversized coverage.
- Holding two selected rows constant, SQL VM work is 223 steps at both zero and
  20,000 unrelated synthetic postings. Serialized selected metadata is 773 bytes
  in both cases. One in-memory read sample measured 441 then 338 microseconds;
  those samples are not percentile, process-restart, HTTP or browser evidence.
- All 30 catalog tests pass, including actual native-backed query parsing,
  cursor coherence, retained history, range provenance and schema migration.
- Formatting and locked package all-target Clippy pass. The copied exact test
  executable contains 522 tests; full unit and combined integration gates are
  recorded separately after completion.

Logs are `/tmp/semon-q16-{test,catalog,clippy,list,unit}.log` in the execution
workspace. The representative complete browser journey must use a matching UI
consumer advertising metadata search; older clients must not be paired with the
new capability/filter advertisement.
