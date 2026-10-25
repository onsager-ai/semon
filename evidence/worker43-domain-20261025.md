# Worker43 / model5 — domain ownership WIP

This is an implementation checkpoint, not acceptance. CI has not been run and
nothing is qualified. One bounded attempt; no nested agents or controllers.

ACK: original; user-resume-001; user-schema-current-only-005;
domain42-current-001; domain42-pin-correction-002; domain43-reconciled-003.

Base HEAD and actual origin ls-remote both:
`6841d5bd1bd9ab02733d89bbcc162d19484201a5`.
Parents: `4aaed3b58ba257fadda5dcca569b162272e21f6e` and
`10276dab5185055855fee66fc573175a1e564bb1`.
Branch: `task/477-raw-sql-backends-20261024`. Initial status and stash list empty;
workspace evidence directory had no worker42 files. Corrected parent acknowledged.

| family | namespaced now? | current writer | consumer read | non-rebuildable? |
| --- | --- | --- | --- | --- |
| PID dependencies | yes, catalog_native_dependencies | CurrentPublicationStore and native identity stage | runtime dirty-source lookup | observed runtime facts require their original authority |
| lineage and native parent | yes, dependency/input/endpoint/native family | native index links, identity stage and CurrentPublicationStore | frontier and shared parent/root reducer | provenance must be preserved when Raw is unavailable |
| relations | yes, catalog_semantic_relations | existing native relation matcher/stage | final incident replay, trace/slots/Query | recorded placements and custody cannot be invented |
| withdrawal | yes, stage source withdrawal and stage authority | native producer using CurrentSources facts and Raw stat | stage/replay/publication validation | explicit negative admission authority, never inferred from absence |

Planned sites: typed namespace-owned stage effects; exact existing current
physical-profile validation before enrolment; same model identity/matching/replay;
durable owner witness in model/proof; transaction fault and colliding-source tests.
No new parser, Options.cache fallback, public qualification or schema upgrade.

S2 reservations remain untouched, including its proof publish hook and appended
tests. No PG/perf/Hub/#380 worktree writes. #380 incomplete; #477 main merge HOLD.

Remaining: event incident candidate replay, cross-harness native keys, complete
frontier/selectors, recovery intents, complete owner lifecycle and retirement,
S2 integration, S3 remaining owners, S4 qualification, injectable PG producer/full
PG backend, migration/catch-up/cutover/rollback and Hub #313 RLS. This list will
be reconciled against the actual final diff.

Thermal log absent. No local CPU-bound work, cargo build/check/test/clippy/fmt/
update/lockgen, Node checks/formatters, browsers, images or corpus analysis.
CI only, owned by Root. Stop CPU work at >=85 C if readings return. Every read
or search is bounded to one named file under 10 MB. Preserve unrelated out/.
No merge/deploy/publication/live PG/user migration/deletion or auth/remote changes.
