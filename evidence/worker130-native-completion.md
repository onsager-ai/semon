# Worker130 native completion — unaccepted WIP / blocked

Task raw-pg-native126-20261026; Worker130 only, no nested agents/harnesses.
Base ee5b125c24f13f8da4de09d6a7837e99486fd0e4. Current schema only.
Origin onsager-ai/semon; branch task/479-pg-native-ingest-20261026.
Normal WIP pushes: 4b99c692, a5a97715, d7b1da6d; remote full SHAs verified.
Final revision/remote SHA is recorded in the external delivery JSON.

## Actual changes

1. Discovery no longer requires an SQLite observation path. That coordinate is
   optional timing only; journal operations retain the logical namespace.
   PG file_ledgers enumerates metadata in keyset pages of 64, in one snapshot.
   The current trait still accumulates the metadata vector for full-scan CAS.
2. Native completion no longer restores an entire FileIndex/Event history.
   It compares exact persisted auxiliary bytes using collision-checked keys,
   refuses writer-size omissions, and qualifies Usage source/path/parser/ledger
   and readiness in the same transaction. Existing ledger, proof/launch and
   Tree-generation qualification and apply_operation completion remain.
3. Selected native rows now use keysets, at most 64 size probes per SQL page,
   reducing the payload page to fit 1 MiB before decode and charging the existing
   selected-read scope. Removed the whole-family 65,536-row/1-MiB refusal.
   This still restores selected-file history; shared SQLite compact/deferred
   load_file extraction and complete resource accounting remain unfinished.
4. Strengthened the existing SQLite-first, process-wide SQLite-denied PG test
   with append, incomplete tail, inode replacement, truncation, ledger enumeration
   and coordinate-free discovery. Existing restart/noop/exact replay after Raw
   changes/disappearance remains. Added >1-MiB family keyset coverage, typed
   Tree error classification and full long-path/namespace request binding.
5. Native intent writes use native_keys::put_meta; path reads use path_digest
   and semon_pg_native_key_matches. Removed the arbitrary 4096-byte request-path
   refusal. Tree errors preserve typed Busy/Unavailable/ScopeChanged.

## CI evidence and limits

- a5a97715bea2f64cecea87320f5dd0de71f48512: run 40256512617,
  job 121359418167, Actual native EventCache PostgreSQL integration SUCCESS.
- d7b1da6dc751c4590849b2d78f626ed7d02e8ee9: run 40256826717,
  Actual native EventCache PostgreSQL integration SUCCESS (includes keysets).
- These are intermediate exact revisions, not final whole-CI acceptance.
  Final-head CI pending, Root collects; fresh independent review is required.
- CI-produced formatting patches applied only to task-owned files. No local
  cargo/build/check/test/clippy/fmt/update/lock generation, Node/typecheck,
  browser/render, corpus/SQL benchmark or live data operation was run.
- Thermal log absent at start and final recheck. Stop CPU work at >=85 C or
  unavailable readings; CI-only checks. Reads remain named-file bounded.

## Remaining work (no completion claim)

1. PG Source native recovery remains explicitly unavailable in PgSourceJournal.
   remove_files/retirement and atomic multi-path cold batches remain fenced.
   No fake SQLite coordinate, empty recovery success or destructive removal.
2. Shared compact/deferred SQLite/PG restoration and complete predecode/resource
   qualification, long Source and 0/256/2048-history measurements remain open.
3. Full field/ledger/history oracles, multiple namespaces with equal paths/IDs,
   retained missing-Raw and tamper qualification remain incomplete.
4. Actual competing writers, every-effect whole-DB faults, stale claims/CAS,
   ACK-loss/restart and cancel/deadline/held-I/O-slot tests remain incomplete.
5. Physical foundation/Usage/Tree/schema work remains owned elsewhere. New files
   retain direct SQL for native families whose full long-key port needs review.

This bounded increment is blocked on remaining implementation and independent
exact-head acceptance; it does not authorize another automatic attempt.
Public qualification stays 0. Full backend, Catalog/Query/leases/GC,
current-contract transfer/catch-up/fenced cutover/qualified rollback, Hub313 and
#380 (#484/#318/#321/#279/#483) before #477 MAIN merge remain open.
Preserve 2s foreground /8s first page /60s background /Source64 /1MiB /VM/security.
No MAIN/release merge, deployment, publication or live migration was performed.
