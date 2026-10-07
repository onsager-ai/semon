# Current facts source authority

Focused catalog reads use a versioned, rebuildable projection beside the
original facts record: `facts.json.sources.sqlite`. The original JSON remains
unchanged and authoritative. This projection stores a small versioned head,
optional machine label, explicit known/unknown native selection, and a covering
primary-key table of current Codex relative paths. It contains no transcript
bodies. A selected lookup reads the head and only the requested source keys;
it never falls back to parsing the full facts JSON.

Publication happens during an existing full facts write or observation. Atomic
JSON publication returns its exact file descriptor; the projection binds to
that descriptor's device, inode, length, modification time and change time.
Checking the pathname after rename alone would incorrectly bind an earlier
writer's facts to a later writer's file. Publication verifies the descriptor
identity against the current regular pathname before and inside its SQLite
transaction. Unchanged manifests retain their indexed rows. A SHA-256 digest
of the selection presence and length-delimited sorted paths identifies the
manifest; the JSON version and index schema version remain separate.

Readers validate the original file identity before and after opening the head,
before and after each source-key query, and before returning the catalog page.
Any concurrent replacement makes the request retryable. A publisher cannot
bind different manifest rows to the same original file identity: it must hold
the descriptor belonging to those facts, and a later JSON writer replaces the
file before publishing its own projection. Consequently head and key queries
need no long-lived database read transaction. Publication is atomic; readers
never retain locks across page construction, and first opens require no WAL
recovery scan. Busy waits are capped at 50 ms for readers and 100 ms for writers.

Known empty selection retires native authority; unknown legacy selection is
incomplete. Missing, corrupt, unsupported-version or mismatched projections
report unavailable observation while retaining cached source references.
Source identity changes return a typed retryable `catalog_scope_changed`
response. The original JSON is still readable if optional index I/O fails.
Existing full background observations repair missing or mismatched indexes;
focused requests do not rebuild them. A corrupt SQLite file can be rebuilt by
removing only this derived sidecar while no index publisher is active, then
performing a full observation. Raw source records must not be removed.

New indexes are private mode 0600 and symbolic-link substitution cannot confer
source authority. The stronger file token currently requires Unix metadata;
other platforms retain original facts compatibility and conservatively report
unavailable indexed observation. The index is an optimization, never credential
or runtime authority.

Verification holds the selected path constant with 100 and 20,000 manifest
entries. The primary-key lookup executes 17 SQLite VM steps at both sizes.
Tests cover unknown/empty/current selection, unsupported facts versions, late
writers, in-place changes, replacement/disappearance, symbolic links, optional
cache failures and background repair. Paired timing measures full JSON reads
against opening/querying the index on a shared development host. It excludes
producer rebuild work, browser rendering, complete catalog journeys and memory;
these measurements cannot establish an end-to-end product latency budget.

Recorded debug-profile paired sample (31 iterations, shared host):

| Manifest entries | Original JSON bytes | Full JSON read/parse median | Indexed open + selected lookup median | Key lookup VM steps |
| --- | ---: | ---: | ---: | ---: |
| 100 | 2,719 | 83 µs | 145 µs | 17 |
| 20,000 | 520,119 | 93,162 µs | 823 µs | 17 |

The baseline executes the original file-read/serde decode path, rather than the
new full-observation reader that also checks the optional projection. The small
manifest is faster through the original full read; the index removes growing
manifest allocation/parsing from selected requests. Opening SQLite and shared
host contention still affect timings, so fixed VM work does not imply constant
wall-clock time. RSS and complete cold/warm Viewer journeys remain separate
integration measurements; this sample establishes neither their budgets nor a
production capacity claim.

## Separate observations

Catalog identity now exposes `facts_observation` independently from content
`freshness`, and optional `native_selection`. Codex has a native selection
observation (`cached`, `incomplete`, or `unavailable`); other harnesses return
null because this projection does not contain their native manifest. Neither
field grants runtime, credential or control authority. A currently selected
source may have unavailable local bytes while its machine facts and native
selection remain observed; an authorized archive reader can supply its history.
Body disappearance remains visible in `freshness` and individual source refs.
Explicit native retirement still fails the current-identity scope. Viewing a
retained retired generation requires a separately qualified history-read scope.
