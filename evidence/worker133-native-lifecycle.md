# Worker133 native lifecycle — provisional WIP

Base 1c5da64035de4e6ad5e33d77a8a52b505fc61931.
Recovery, retirement/removal and atomic cold batches are the priority.
Current schema only; preserve Raw, retained facts and immutable generations.
Public qualification is 0. Exact-head CI and fresh review remain required.
Thermal telemetry is absent: no local build/test/fmt or benchmarks; CI only.
No MAIN merge, deployment, publication or live migration.

Implementation WIP: shared SQLite/PG cold-batch source plan; request-owned
EventCache batch using ordinary scan_file; all PG intent/base/Source fences
before native family effects; borrowed operation protocol keeps individual
receipts and all effects in one transaction. Existing single-file completion
qualification is reused unchanged. SQLite-first/PG-only multi-harness batch,
every measured bundle-effect whole-schema rollback and missing-Raw restart
replay are added to the existing CI integration test. Not yet verified.

Recovery, retirement/remove_files, paged Source producer contract and shared
compact/deferred reader restoration remain open. Concurrency/ACK-loss/cancel
qualification beyond existing tests remains open. No full lifecycle claim.
