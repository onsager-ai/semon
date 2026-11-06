# Worker55 domain52 repair

## Scope and classification

This is a fixture defect (classification **a**), not a production lifecycle
defect. The domain52 test created and durably ingested 65 real Codex sources,
but staged only `budget-00` (`configured.rs:6868-6901` before this repair).
The staged source inventory is the production authority for subsequent batches:
`semantic_next` selects unfinished `catalog_semantic_sources`
(`events/store/semantic_stage.rs:2070-2079`). Consequently, the old fixture
could trip the stage witness fence before it had supplied the whole component to
the real production discovery/publishing path.

The corrected fixture derives `paths` from every one of its 65 real
`crate::Input` values and passes that set to
`begin_semantic_stage_with_journal` (`configured.rs:6868-6901`). It does not
construct `PreparedSources`, catalog rows, endpoints, or an alternative
algorithm.

## Budget reachability

With every component member staged, the unchanged production path reaches the
publication owner lookup. That path reads owner paths with `LIMIT 65` and
refuses an owner with more than 64 sources with `semantic owner source budget
exceeded` (`events/store/semantic_stage.rs:2081-2100`). The parent-linked 65
real Codex inputs are therefore a valid real-producer witness for that bound.

The earlier refusal is consistent with the witness fence:
`witness_rejection` compares stage revision and each indexed source's retained
claim/ledger to current observations (`events/store/semantic_stage.rs:1209-1234`).
The fix supplies the complete staged inventory instead of weakening that fence.

## Oracle and verification

The oracle remains unchanged: the error must contain `budget`, shared state
must equal its pre-call snapshot, and `session_catalog` has zero rows for
`source-a` (`configured.rs:6902-6943`). The change has no production code and
does not alter foreign-row handling.

No local Cargo build/check/test/clippy/fmt, Node checks, typechecks, formatter,
or CI polling was run, as required. `git diff --check` completed successfully.
Fresh CI on the committed revision must demonstrate all 70 configured tests,
32 SQLite tests, 3 journal tests, the format gate, and that the 90-group is
reached; Root owns that CI observation.

## Delivery

Base and remote before this repair: `178bc8b276b2ce2d0cd0965e97082e52bfd68a5e`
on `origin/task/477-raw-sql-backends-20261024`.

Implementation commit: `fdc8fc9b4072330430c36dfa9c7b0559f6cb48f8`

Parent: `178bc8b276b2ce2d0cd0965e97082e52bfd68a5e`

Remote: `origin/task/477-raw-sql-backends-20261024` — pending push verification

## Remaining work

This bounded repair is not full #477 acceptance. Remaining scope includes D3/D4
independent review; incidents/selectors/recovery; S3/S4; full PostgreSQL;
migration; Hub #313; cold timing; full UI binding; and the #380 predecessor
completion gate before any #477 main merge.
