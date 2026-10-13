# Fixed-scope Viewer query workload

The checked-in `query-workload` now measures native focused Query reads; it
constructs no ViewerCore and calls no retired full-model or transaction endpoint.
Run `cargo build --release --locked -p semon-sessions --example query-workload`,
then run `target/release/examples/query-workload HISTORY_PER_MACHINE MACHINES`
in a fresh process for each size. It holds one 120-record session and the same
60-entry transcript page constant while increasing unrelated two-record sessions
on independently named machines. All logs are synthetic.

The preparation phase uses existing native Catalog observers, waits for complete
publication on each machine, and reports `cold_catalog_complete_ms` separately.
The observers share two workers. `cold_worker_drain_ms` measures the remaining
publication/cleanup lifetime before allocator snapshots; `active_workers` and
`queued_checks` retain the actual pool state at the end of the reads.
This phase reconciles all seeded machine history; it is not a bounded cold
foreground read claim. Query adopts those same evidence owners. The next 101
`read_transcript` requests use the selected source and Catalog identity, with no
provider or source changes. The first page is reported separately, followed by
100 warm p50/p95 samples. Every response must contain exactly entries 60–119.

Latencies include in-process native rendering and JSON encoding, not HTTP,
compression or browser rendering. GNU libc hosts additionally report live and
reserved allocator bytes before and after focused reads. Those counters help
distinguish live allocations from retained allocator pages; they do not measure
product cache occupancy. Linux VmRSS and `/proc/self/io` counters are
process-wide and include their own small measurement reads. Retained archives,
authorization, concurrency, cancellation and deletion require their separate
repository fixtures; this workload does not qualify them.

## Historical compatibility baseline, 2026-10-07

The measurements below used the retired full-model implementation and the old
workload endpoints. They are preserved as baseline evidence, not a description
of current product architecture or exact final integration qualification.

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

## Current serving boundary

Native Catalog and slot projections own focused selection and byte recipes.
Queries fence the selected source generations and recheck consumed evidence
before publishing. Complete native background reconciliation and selected-source
proof metadata remain measurable costs; the retired ownership-plan cache and
full-model fallback described by the historical baseline are no longer product
paths. Run the current native workload on the exact integration commit before
reporting current timings or memory use.
