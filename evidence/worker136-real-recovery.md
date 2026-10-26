# Worker136 recovery revision inventory

Base: 686ba91f08a3e2a2fa74f24cfc0887c9cb57d904, local/remote verified clean.
Provisional scope: shared current recovery reducer, bounded SQL adapters and
durable exact page receipts. Current schema only. No public qualification.
Thermal telemetry unavailable: no local builds/tests/fmt; CI only.
Exact-head checks and independent review pending; Root collects CI.

Substantive revision fe74e95dabd918f70a4242e330773e6e491d256e was normal-pushed
and full remote SHA verified. CI run40260962848 started; not a passing result.
New source_observations/recovery.rs owns the current page reducer. SQLite store
uses it and predecode path/custody probes; archive marker decoding is shared.
New postgres/native_recovery.rs connects durable typed intents to borrowed
apply_operation, Source locking, exact applied/done receipts and atomic ACK.
The final candidate seeks the Catalog primary key in pages64 and binds the head
in its continuation, avoiding whole-Source path sorting. Full-row owner/path,
lifecycle and archive validation precede repairs; Raw and retained facts untouched.
Authored new real-parser SQLite-first recovery test with isolated PG SQLite-deny,
restart/lost ACK, original outcome replay, stale cursor and whole-DB fault checks.
No new test has actually passed yet. Catalog head is a physical fixture only.
Remaining: admitted Catalog/runtime recovery prerequisites, full measured-effect,
admission/profile/cancel/concurrency/noise qualification; full-scan Vec enumeration,
retirement/removal and compact/deferred read. Public qualification remains zero.

37ed77ea0237aeb215bc40cd53347fa472f05802 normal-pushed and remote verified.
Exact run40261433441 native integration FAILED compilation: missing StorageError
import in the new recovery test. The final revision fixes that import, applies
CI-generated formatting to owned files, strengthens full key-digest equality and
consumes re-captured exact committed intents on replay. No passing new oracle is
claimed. Earlier fe74 run40260962848 existing native integration succeeded, but
its native-key step failed and it predates the new recovery oracle.

Final status: BLOCKED / unaccepted substantive WIP. A has an actual connected
transactional implementation but lacks final CI/admission/resource qualification;
D has a bounded recovery producer, while the full-scan ledger Vec API is open.
B/E open, C inherited unqualified. No public backend readiness, mainline merge,
deployment, publication or live data action. Final exact-head CI: Root collects.
