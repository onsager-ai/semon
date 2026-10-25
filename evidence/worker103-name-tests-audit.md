# Source103 Name/member closure (implementation, not acceptance)

Base: bb016075b4c31131e9870aad47cd4de5706084d5 on
`task/477-raw-sql-backends-20261024`. Source103 is the same approved bounded
attempt labelled Source102 in the historical plan; no counter or retry reset.
Binding scope remains the full task intake and named #477/#478/#479/Hub313
requirements, including the #380 gate before any #477 merge to main.

The producer's unsupported-format inspector now derives the current semantic
recipe family/version from `semantic_stage::RECIPE`, without an old hardcoded
prefix or an `expect`. A different family or version refuses without mutation.
The former old-recipe rebuild fixture now runs in the stronger all-entrypoint
unsupported-state suite, with opaque retained custody and exact original bytes
preserved. An older version of the current recipe family is also refused.
The other obsolete-format checks and both current physical profiles remain.

Two new real sanitized Claude native-producer tests are registered by the
existing domain94 CI filter and in writer-inventory.json:

- `configured_semantic_domain94_name_members_source_overflow_rolls_back_before_raw`
  publishes 64 hinted native owners in bounded groups, stages an initiator,
  makes the peer Raw files unavailable, and requires the 65-source member
  discovery to hit the actual source budget. Repeated calls and reopening
  must preserve the whole database and populated foreign namespace exactly.
- `configured_semantic_domain94_late_native_member_fences_publication_and_restarts`
  admits a new native source after the caller has completed its negative name
  lookup and prepared its publication model. Old selection, model record and
  publication must refuse without partial writes. Reopening produces a new
  epoch and a verified unique match equal to ordinary native preparation.

Actual CI at 8a1aecd8 exposed a stale model-record defect: recording the old
publication model returned Written after a new native source advanced the
journal. Existing claim checks cover staged paths only. The publication-phase
model record now checks the current namespace journal revision against the
batch, as final publication already does, inside its existing fenced owner
transaction and before any writes. The original Conflict assertion remains.

The existing member-hint replay test additionally inspects the actual durable
member keysets across reopening and retains its unchanged native-event counts,
fault rollback, stale publication and ordinary preparation oracle assertions.
This proves completed member cursor persistence; it does not establish an
unfinished member page resuming across a restart. That required stronger case
remains explicitly unqualified. In the new overflow case the first 64 member
observations plus the initiator exceed the source bound in `staged_paths`, so
the transaction rolls back its attempted cursor changes before any Raw replay.
No limit, growth assertion, cold gate, existing fault or oracle was weakened.

CI at d19a290eeeefe0a1945d1242e268931dedd12597 proved the original recipe panic
fixed: all 15 unsupported-state cases passed. It exposed the obsolete fixture's
old-recipe upgrade expectation, which the current-contract test update removes.
Formatting changes are applied from the CI-generated patch. Final exact-head
CI outcomes and revision/remote evidence are recorded in the external
`evidence/worker103-name-tests-delivery.json`; these interim results do not
qualify the final revision.

No local cargo/build/check/test/clippy/fmt/lockgen, node/typecheck, browser,
screenshot/image, corpus or SQL bench ran. Thermal telemetry is absent; checks
are CI-only, and CPU work must stop at 85 C or unavailable readings. Reads and
searches remain bounded to one named file under 10 MB and 150 lines/4000 tokens.
No nested agent, main merge, deployment, publication or live migration ran.
Public qualification stays zero. Full PG/no-hidden-SQLite, all native families,
migration/catch-up/fenced cutover/qualified rollback and Hub313 remain mandatory.
