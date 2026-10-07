# Fixed-scope Viewer query workload

This reproducible synthetic workload supports the remaining measurements in
#29, #53 and the first independently reviewable serving slice in #319. It is
not production, browser, provider, archive or connection-to-session evidence.
Those acceptance criteria remain open.

Run `cargo build --release --locked -p semon-sessions --example query-workload`,
then run `target/release/examples/query-workload HISTORY_PER_MACHINE MACHINES`
in a fresh process for each size. The example creates made-up Claude JSONL
sources and removes them after closing its ViewerCore. It holds one 120-record
session and the same 60-entry transcript page constant while increasing other
two-record sessions. There are four fixed machines; they deliberately share a
hostname, with independent caller keys and unique native session ids. No
personal source records or private hosted metadata are used.

The first `/api/model` includes cold index/model creation and full serialization.
The subsequent 101 `/api/tx?sid=selected&before=120` requests use OnInvalidate
refresh, without source appends or a background stat worker. The first request
is reported separately; the following 100 give p50/p95 latency. Latencies are
in-process server response construction; response bytes exclude HTTP headers
and compression. Linux VmRSS and `/proc/self/io` counters are process-wide;
read counters include the tiny measurement reads themselves. Browser layout,
paint, frames, filtering, live updates and tool expansion are not measured here.

## Observed 2026-10-07

Baseline library: main `83e23259ba5b71d14d6262f951c58cc01186bf64`.
Baseline workload binary SHA256:
`1a1f24f21dfdac5c6af78de8139ed05011b0d25a7fcee579921c9587e6f5c8c3`.
Changed library: the ownership projection implementation in the accompanying
change, with the same workload. Changed workload binary SHA256:
`6a1f12daec7918efd8ddb308dd6436091e6e884de18897d63f45f8a759905b43`.
UI bundle unchanged:
`f8e53117df754dde006726c1b87f09864480e875fc13c1cab07bf5fbdcbbf059`.
No guest artifact participates in this in-process read workload.

Rust stable 1.99.0, release optimization; Linux x86_64, five visible AMD EPYC
9V74 CPUs. Other builds/tests were using the same machine. One fresh process
per cell, 100 warm latency samples, no discarded timing outliers. Because
machine load differs between cells, do not interpret incidental cold latency
or RSS differences as an improvement or use these samples to set hard budgets.

| Other sessions per machine | Cold full model ms, before / after | Cold bytes, both | Warm page p50 ms, before / after | Warm page p95 ms, before / after | Warm RSS KiB, before / after |
| --- | --- | --- | --- | --- | --- |
| 100 | 488.35 / 383.26 | 372,175 | 15.11 / 12.22 | 39.64 / 39.25 | 21,316 / 21,424 |
| 1,000 | 6,497.15 / 4,191.33 | 3,513,215 | 40.60 / 8.41 | 93.76 / 30.25 | 144,776 / 144,996 |
| 5,000 | 18,154.48 / 12,414.89 | 17,633,215 | 232.70 / 10.55 | 346.87 / 31.81 | 668,088 / 667,796 |

Every transcript response was 8,672 bytes. The 101 transcript reads consumed
165,419,120–165,419,131 rchar bytes and 24,244–24,247 syscr calls, independent of
unrelated history. These process counters count source-validation and buffered
reads, not just the returned text. A separate syscall audit at history=100
using `strace -f -e trace=read,pread64,openat,newfstatat,getdents64,write`, bounded
by the example's WARM_BEGIN/WARM_END stderr markers, found the same 387,844
openat, 24,240 pread64 and four read calls before and after. Traced latency was
excluded from the table. Repeated descriptor-safe source opens/validation are
a significant remaining selected-page cost; this change does not bypass them.

Before, every warm transcript page reconstructed the union's ownership plan
over all 404 / 4,004 / 20,004 session identities, including temporary sets,
string copies and ambiguity detection. After, the cold model builds that plan
once. Unchanged warm snapshots reuse it, examining four snapshot identities
and looking up the selected id in the map. Tests check reuse by Arc identity
and compare every plan field with the uncached ownership oracle.

## Serving and limits

The cache holds at most one ownership plan per ViewerCore. Admission allows
at most 4,096 machine keys, 100,000 combined owner/conflict/dropped entries and
8 MiB of retained string contents. These bound structural counts and string
contents rather than claiming an exact allocator heap limit. Oversized plans
use the existing uncached path. Snapshot keys are Weak references, so the
cache retains no old model, source generation, transcript index or body.

Any changed model object, caller key, machine order/count or received-copy
policy invalidates the cache. An identical transport version is insufficient
to reuse it. Concurrent misses may compute twice, but a caller always receives
the plan for its own snapshots. Session id conflicts, machine-local stub
namespaces, received-copy precedence, source access and model API semantics
remain those of the full ownership oracle.

Cold full-model initialization, global lineage/joins, full-model serialization,
and per-machine snapshot collection remain. At 20,004 sessions the compatibility
model still returns 17.6 MB and this workload retains about 652 MiB RSS. A warm
routing cache cannot satisfy bounded first content or filtered list acceptance.
The next #319/#320 slice needs a persisted metadata catalog with bounded page
reads and generation-bound cursors, while preserving `/api/model` compatibility.
Received-directory follow/stat/facts refresh, unknown transcript lookup and
real multi-machine workload acceptance in #53 also still need direct evidence.
