# Cloud receiver hosting options

Status: **Option (a) chosen for the synthetic pilot** ("Fly now, R2 later", owner
decision via Chief PM run 706 on 2026-11-27; baseline `origin/main` `4dd600fc`).

- **Decision summary:** **(a)** native Rust `semon receive` on one Fly Machine + Fly
  Volume is **chosen for the synthetic pilot**; **(b)** hybrid async R2 archival is a
  **separate later decision**; **(c)** full Cloudflare port is **set aside**.
- **Deployment boundary:** The synthetic pilot uses **one native Fly Machine and one Fly
  Volume only**—**no R2** in the pilot and **no real data or backfill**. This option
  selection is **not** a deployment approval or an approved protocol change, and it
  authorizes no accounts, tokens, deploys, real data/backfill, or code/config changes.
  The pilot setup batch below remains pending owner decisions. Independent Fly
  deployment config is owned elsewhere and not duplicated here.

## Current Semon contract

- **Push wire & custody** ([push-protocol.md](../push-protocol.md)): `semon push` sends
  authenticated JSON `POST`s up to 6 MiB (max 4 MiB raw file bytes as base64; 256 KiB
  default chunk, complete `.jsonl` lines, whole small `.json` files) to `/v1/pair`,
  `/v1/unpair`, `/v1/push/append`, `/v1/push/facts`, and optional `/v1/push/status`.
  `semon receive` holds an exclusive OS lock on `DIR`, stores redacted logs under
  `DIR/machines/<machine-id>/` with optional in-place hole-punched `.seal/` zstd
  segments and immutable `archive/` zstd chains, and uses registry format 9.
- **Auth & Viewer** ([README.md](../../README.md)): One process serves plain HTTP
  behind an HTTPS proxy (`--public-url https://HOST`) with split routes: one-time pair
  codes, per-machine write-only push Bearers, and owner read tokens/cookies for the
  Viewer.
- **Analytics scope** ([asi-centralized.md](asi-centralized.md)): The SQLite Session
  Event Index (`sessions-index.sqlite3`, with private FTS5 bodies) and DuckDB-over-Parquet
  analytics (`asi-query`) are derived caches. Today those query commands are
  origin-local; receiver materialization and SQL/search remain planned work.

---

## Option (a): Native Rust receiver on a Fly persistent volume (as-is) — chosen for synthetic pilot

*Status: **Chosen for the synthetic pilot** ("Fly now, R2 later", Chief PM run 706,
2026-11-27). Pilot scope is one native Fly Machine + one Fly Volume only—no R2 and no
real data or backfill; DigitalOcean VPS remains below as a comparable VM cost baseline.*

Run the unchanged `semon receive` binary on one Fly Machine with a local NVMe Fly
Volume (or on a single DigitalOcean Droplet + Block Volume; both are costed below).
Fly's proxy (or a VPS reverse proxy) terminates HTTPS in front of plain HTTP.

- **Ack & retry safety.** Under a per-file writer lock, the receiver verifies
  `offset == current_length` and `head_sha256` (SHA-256 of the first
  `min(4096, offset + len)` bytes), writes and `fsync`s the file (or temp-writes,
  `fsync`s, renames, and `fsync`s the parent dir on `replace`), updates the
  generation/length marker, and only then returns `200 {"length": N}`. Exact offset +
  head checks stop any retry from duplicating or losing bytes.
- **Ack loss.** The receiver committed length `N`, the sender missed `200` and retries
  at `< N`, and the receiver answers `409 {"length": N, "head_sha256": "..."}`. The
  sender verifies the prefix at a line boundary and advances its cursor to `N`.
- **Disk full.** Per-machine `--max-bytes` (default 20 GiB) or `ENOSPC` returns `507`
  (`machine_storage_full` or `receiver_storage_full`), truncates any partial append,
  and acks zero bytes. Watch-mode senders back off (1/2/4/8/10 min) and resume from
  their saved cursor after space is added.
- **Replacement.** Before acking `replace: true` on non-empty data, the receiver copies
  the old logical bytes (via `sealed::LogFile`) into an immutable zstd chain under
  `archive/<archive_source_id>/<chain_id>.zst` and atomically renames `manifest.json`.
  Archive failure refuses the replace and leaves the prior copy intact.
- **Crash & restore.** The exclusive OS lock prevents concurrent receivers. Crashes
  mid-append leave an intact prefix reconciled via `409`; abandoned replace temps are
  cleaned on start. Restoring a crash-consistent daily volume snapshot rolls disk back
  to snapshot time; live origins re-push the missing tail via `409`. Bytes pushed after
  the last snapshot from an origin that also dies are lost.
- **Source of truth & Index rebuild.** `DIR/machines/<machine-id>/` (raw/sealed logs,
  `archive/`, `facts.json`, markers, registry) is authoritative. On Index rebuild, the
  receiver pauses custody writes across all machines (`503 receiver_rebuilding`,
  `Retry-After: 60`), drains admitted writes, replays verified originals/archive/facts
  into a staged DB, and publishes `sessions-index.rebuilt-*.sqlite3` atomically without
  touching the old DB or raw files ([storage contract](session-source-reader.md#current-semon-storage-contract)).

---

## Option (b): Hybrid — Native Rust receiver + async R2 archive — separate later decision

*Status: **Separate later decision** (not part of the synthetic Fly pilot). Revisit only
after the Option (a) pilot if off-host R2 archival is requested.*

Keep Option (a) for live writes, sealing, indexing, and Viewer, and add an asynchronous
background worker that uploads immutable sealed segments (`.seal/` zstd segments),
`archive/` chains, and versioned `facts.json`/registry snapshots to Cloudflare R2.

- **Local commit vs. cloud archive completion (RPO).** Sender `200` ack stays bound to
  local `fsync`—identical to (a), never blocking on R2. Push retries, `409`, `507`, and
  local `replace` archiving are unchanged. Cloud archive completion is a second, async
  boundary recorded in a local upload receipt after R2 confirms each object. If the
  host and volume die permanently, R2-only RPO equals archive lag (minutes to hours);
  surviving origins re-push the unarchived tail via `409` after restore.
- **Safe object keys & immutable identity.** Never overwrite mutable path-only keys.
  Scope immutable keys under the canonical machine ID:
  - Blobs: `v1/machines/<machine-id>/blobs/<archive-source-id>/<chain-or-seg-id>-<plain-sha256>.zst`,
    written with S3 `If-None-Match: *`.
  - Manifests: `v1/machines/<machine-id>/manifests/<archive-source-id>/<seq>-<manifest-sha256>.json`,
    uploaded strictly *after* every referenced blob is confirmed in R2.
- **Checksum & manifest.** Each manifest binds `machine_id`, `archive_source_id`,
  logical `root`/`path`, segment/chain ID, logical byte range or length, **plain
  SHA-256**, compressed size + **compressed SHA-256**, `redactor_version`, and
  generation lineage. Restore decompresses and verifies every blob's plain SHA-256
  before admission.
- **Failure modes (disk full, crash, restore).** R2 outages never block local receive;
  archive lag surfaces as a health alert. Local disk full still returns `507` because
  R2 archival does **not** evict local files. Mid-upload crashes leave at most an
  unreferenced R2 blob; restart resumes from the local receipt cursor. Restore creates
  a fresh volume, downloads verified manifests + blobs from R2, reconstructs
  `machines/<machine-id>/`, and runs a normal custody-aware Index rebuild.
- **Duplicates, orphans, retention & deletion.**
  - *Duplicates:* `If-None-Match: *` on immutable keys makes retries idempotent; equal
    chains under one `redactor_version` reuse the existing verified blob.
  - *Orphans:* Unreferenced blobs from interrupted uploads stay inert until an explicit
    offline operator sweep—never raced with active writers.
  - *Retention & deletion:* **No automatic removal** of local copies, seals, or R2
    objects, and **no change to local custody**. Unpair revokes auth only and deletes
    nothing locally or in R2. Local cold eviction or R2 expiry requires a separate owner
    decision.
- **Source of truth & Index rebuild.** Live host rebuilds from local disk as in (a); R2
  is an immutable off-host custody copy from which a replacement host rebuilds disk and
  Index.

---

## Option (c): Full Cloudflare port (Workers + R2 + D1 or SQLite Durable Objects) — set aside

*Status: **Set aside** by owner decision ("Fly now, R2 later", Chief PM run 706,
2026-11-27). Retained below for comparison context only.*

Re-implement the receiver on Workers storing raw chunks in R2 and metadata/registry/facts
in **SQLite Durable Objects** (one serialized writer per machine) or **D1**. **No
protocol change is approved**; this sketch is only enough to compare tradeoffs.

- **Platform constraints.**
  - *Finite chunk POSTs fit:* 256 KiB–4 MiB raw (6 MiB JSON) is far below Cloudflare's
    100 MB Free/Pro request body limit.
  - *No POSIX append:* R2 objects are immutable—no in-place append, hole-punching, or
    OS `flock`.
  - *No cross-store transactions:* R2 writes and D1/DO SQLite commits cannot share one
    atomic transaction.
  - *Long-connected HTTP != permanent CPU:* Workers allows unlimited wall-clock time
    while an HTTP client stays connected (and Paid allows 30 s default / up to 5 min
    CPU per request), but Workers is not an always-on daemon. Rebuilds, compaction, and
    materialization must be sliced into Alarms/Queues/Cron (15 min wall-clock max) or
    run off-Workers.
- **Two-phase commit, ack timing & failure between phases.** With one single-writer
  SQLite DO per machine (avoiding D1 write contention; D1 optimistic CAS on
  `(offset, generation)` is the alternative):
  1. Verify Bearer token, `is_input_path`, expected `offset`, and `head_sha256` in the
     DO; mismatch returns `409` before touching R2.
  2. Write an immutable R2 chunk at
     `v1/machines/<machine-id>/chunks/<file-id>/<generation>/<offset>-<len>-<chunk-sha256>`
     with `onlyIf: { etagDoesNotMatch: "*" }`.
  3. Commit the chunk manifest row, new `length`, `head_sha256`, and `generation` in
     one atomic DO SQLite transaction.
  4. Return `200 {"length": N}` **only after** step 3 commits.
  - *Failure between phases (crash after step 2, before step 3):* R2 holds a harmless
    orphan chunk; SQLite still has the old offset. Sender retries the same offset; step
    2 is idempotent and step 3 commits once—no duplication or loss.
  - *Ack loss (crash after step 3):* SQLite already holds `N`; retry gets `409` with
    `N` and `head_sha256`, and the sender advances.
  - *Disk full / replacement / restore:* Quota (and D1's hard 10 GB/DB cap) is enforced
    before commit (`507`). `replace: true` freezes the prior immutable R2 chunk list as
    an archive chain row in SQLite, writes chunk 0 under a new generation, and flips
    atomically—no byte copy needed. Restore combines immutable R2 chunks with D1 Time
    Travel (30 days on Paid) or R2 snapshots of DO SQLite; live origins fill any gap
    via `409`.
- **Source of truth, Index rebuild & D1/DO vs. DuckDB/SQLite/FTS.**
  Source of truth is R2 chunks plus committed metadata (exported to R2 so metadata loss
  is recoverable). Rebuilding derived views exposes the core gap: Semon uses local
  SQLite (with FTS5 bodies and proofs) plus **bundled DuckDB over Parquet** in a
  sandboxed 2 GiB OS child process ([asi-query.md](asi-query.md)). Workers caps
  isolates at **128 MB RAM**, cannot run native OS processes or bundled DuckDB, D1
  hard-caps at **10 GB/DB** (single-threaded), and sharded DO SQLite needs
  scatter-gather or external ETL for cross-machine SQL. FTS5/body writes in D1/DO also
  multiply billable row writes. Full ASI parity therefore still needs an external
  Rust/DuckDB compute host.

---

## Cross-cutting comparison

| Dimension | (a) Fly / VPS volume | (b) Hybrid (Fly/VPS + R2) | (c) Workers + R2 + D1/DO |
| --- | --- | --- | --- |
| **Region / residency** | Pinned region (`iad`, `fra`, `sin` on Fly; `nyc3`, `fra1`, `sgp1` on DO). | Pinned host region; R2 **Location Hints** (`enam`/`weur`/`apac`/etc.) are best-effort only. Strict residency requires immutable creation-time R2 **jurisdiction** (`eu`/`fedramp`). | Hints are not residency. Needs creation-time jurisdictions (`eu`/`fedramp` on R2/DO; `eu`/`us`/`fedramp` on D1) + Regional Services for in-region TLS/Worker execution. |
| **Raw logs, backups, E2EE** | Readable redacted logs on disk, proxy, and snapshots. No E2EE; best-effort redaction only. | Same as (a) + readable redacted copies in R2 under Cloudflare account access. No E2EE or redaction guarantee. | Same across Workers, R2, D1/DO, and Time Travel. No E2EE or redaction guarantee. |
| **Access & owner boundaries** | Split Semon tokens (pair/push/read). Cloud org & SSH keys stay owner-only, never on origins. | Same + narrow bucket-scoped R2 write key on receiver only (no delete/admin); never on origins. | Workers bindings hide R2/D1/DO; Cloudflare admin keys stay owner-only. |
| **Amount of change** | **None** in receiver/protocol code (deploy config + ops only). | **Moderate, additive** Rust worker (R2 uploader, manifests/checksums, restore CLI, lag metrics). Zero wire/Viewer/DuckDB rewrite. | **Major rewrite** (Workers receiver, chunk assembly, DO/D1 Catalog/Viewer) + external DuckDB compute. |

**Ostrom over T3 read-only MCP (planned target — not yet shipped).** Neither Ostrom nor
Magpie integration exists today, and Magpie credentials are not shared. Under any
option (a)–(c), Ostrom must read only **authorized projected metadata**—spend, cost
provenance, time/activity, freshness, and explicit `unknown` coverage—through a Semon
read interface (authenticated read-only MCP/HTTP API), **never** via raw session logs,
R2 bucket keys, VPS SSH, or direct SQLite/D1 credentials.

---

## Monthly cost comparison (US USD/month)

Workloads: **1×** = 10 GB retained raw + 0.5 GB index, 5 GB/mo out, 100k API calls/mo;
**10×** = 100 GB retained raw + 5 GB index, 50 GB/mo out, 1M API calls/mo.

| Option | 1× / mo | 10× / mo | Line-item basis |
| --- | --- | --- | --- |
| **(a1) Fly.io volume** | **~$9** | **~$35** | 1×: `shared-cpu-1x` 1 GB ($6.70 `iad`/`ewr`) + 15 GB vol ($2.25 @ $0.15/GB) + 5 GB US/EU out ($0.10 @ $0.02/GB); snapshots in 10 GB free. 10×: `shared-cpu-2x` 2 GB ($13.39) + 130 GB vol ($19.50) + 50 GB out ($1.00) + ~15 GB billable snapshots above 10 GB free ($1.20 @ $0.08/GB). |
| **(a2) DigitalOcean VPS** | **$6** | **~$26** | 1×: Basic 1 GB / 25 GB SSD / 1 TB transfer ($6). 10×: Basic 2 GB / 50 GB SSD / 2 TB transfer ($12) + 140 GB Volume ($14 @ $0.10/GiB). Snapshots extra ($0.06/GiB-mo). |
| **(b1) Hybrid: Fly + R2** | **~$9–$10** | **~$38–$39** | **Additive:** Fly (~$9 / ~$35) + R2 ($0 at 1× within 10 GB incl.; $1.35 at 10× for 90 GB × $0.015) + **Fly outbound for archive writes** (e.g., 10 GB/mo churn = $0.20; 100 GB/mo = $2.00 US/EU, 2× APAC). |
| **(b2) Hybrid: DO + R2** | **$6** | **~$27.35** | **Additive:** DO ($6 / ~$26) + R2 ($0 at 1×; $1.35 at 10×); DO→R2 writes fit inside the 1–2 TB transfer pool unless churn exceeds it. |
| **(c) Workers + R2 + D1/DO** | **$5.00** *(floor)* | **$6.35** *(floor)* | **Low-usage floor:** $5 Workers Paid + R2 ($0 at 1×; $1.35 at 10× above 10 GB incl., $0 Internet egress) + D1 (5 GB incl.; 10× sits at ceiling). Excludes variables below. |

**Cost accounting rules:**
1. **R2 zero egress != zero upload cost from Fly.** Reading *out* of R2 is $0, but Fly
   still charges outbound egress ($0.02/GB US/EU, $0.04/GB APAC) to upload archive
   blobs to R2.
2. **Retained GB != monthly churn.** Storage bills GB-month retained; archive egress
   and R2 Class A ops scale with monthly newly written bytes and object count.
3. **Do not double-count free allowances.** R2's 10 GB / 1M Class A / 10M Class B
   allowances (and Workers/D1 inclusions) apply once per Cloudflare account.
4. **Excluded from the $5 / $6.35 Cloudflare floor:** R2 ops overages ($4.50/M Class A
   writes/lists, $0.36/M Class B reads), Workers CPU above 30M ms ($0.02/M ms), D1/DO
   storage above 5 GB ($0.75/GB D1 with 10 GB/DB hard cap; $0.20/GB DO), SQLite row
   writes ($1.00/M above 50M, amplified by indexes/FTS) and reads ($0.001/M above 25B),
   active DO duration ($12.50/M GB-s above 400k GB-s), backup/replication, and
   **external query compute** for DuckDB/Parquet analytics.

---

## Decision, recommendation, smallest synthetic pilot, and pending setup choices

**Decision & recommendation.** Per the owner's **"Fly now, R2 later"** decision (Chief
PM run 706, 2026-11-27), matching the zero-rewrite recommendation:
- **Option (a) — chosen for the synthetic pilot:** Run **one native `semon receive` Fly
  Machine with one Fly Volume**, with **no R2** and **no real data or backfill**, for
  zero-rewrite operational validation.
- **Option (b) — separate later decision:** Add async R2 archival of immutable sealed
  segments/chains only as a separate follow-up decision once off-host recovery
  expectations justify additive R2 storage and Fly egress.
- **Option (c) — set aside:** The $5/$6.35 floor hides a full server/Viewer rewrite,
  D1's 10 GB cap, and external compute still required for DuckDB/Parquet/FTS parity.

**Smallest isolated synthetic pilot (Option (a) only).** Stand up **one throwaway native
Fly Machine + one Fly Volume** running `semon receive` with **synthetic fixture logs
only** (**no R2**, **no real data or backfill**, zero live secrets) and a tight
`--max-bytes` cap. Run an automated `semon push` pass verifying: (1) pair + append +
simulated ack-loss `409` resume, (2) `replace: true` archive-chain creation, (3) `507`
refusal + recovery, (4) forced Index rebuild (`503 receiver_rebuilding`), and (5) Fly
volume snapshot restore + re-sync, then tear down per the owner's retention rule. (Any
throwaway R2 upload/restore verification belongs only to a separate future Option (b)
decision.)

**Pending owner setup decisions (required before any Fly pilot deployment)** *(option
selection is settled; the setup batch below remains undecided—do not treat pending
proposed values as approvals, and this document triggers no owner asks, accounts, or
deploys)*:
1. **Org & billing:** Fly organization and billing account that own the pilot.
2. **Credentials:** How Fly deploy/admin credentials are supplied and kept isolated from
   pushing machines and Magpie.
3. **Spend:** Hard monthly spend cap and alert threshold for the Fly pilot.
4. **Region:** Exact Fly host region for the pilot Machine and Volume (Cloudflare
   residency/jurisdiction applies only if Option (b) is approved later).
5. **Public sender routes only vs. private Viewer:** Confirm the public endpoint exposes
   **sender routes only, including pairing** (`/v1/pair`, `/v1/unpair`, `/v1/push/*`),
   with **no public Viewer planned**, and decide how the private Viewer is accessed or
   whether it stays unexposed during the pilot.
6. **Retention & teardown:** Maximum lifetime and explicit teardown rule for the
   synthetic pilot Machine, volume, and snapshots (confirming no real data or backfill
   enters the pilot and no automatic deletion of local copies on unpair).
7. **Daily snapshots & recovery:** Daily Fly volume snapshot policy/retention and
   confirmation that daily snapshot recovery (up to 24 h RPO for dead-origin data, with
   live origins resuming via `409`) meets pilot expectations, leaving off-host async R2
   archival to a separate later decision.

---

## References

- **Semon:** [push-protocol.md](../push-protocol.md), [README.md](../../README.md),
  [asi-centralized.md](asi-centralized.md), [asi-query.md](asi-query.md),
  [session-source-reader.md](session-source-reader.md).
- **Fly.io:** [Pricing](https://fly.io/docs/about/pricing/),
  [Volumes](https://fly.io/docs/volumes/),
  [Regions](https://fly.io/docs/reference/regions/).
- **DigitalOcean:** [Droplets pricing](https://docs.digitalocean.com/products/droplets/details/pricing/),
  [Volumes pricing](https://docs.digitalocean.com/products/volumes/details/pricing/),
  [Snapshots pricing](https://docs.digitalocean.com/products/snapshots/details/pricing/),
  [Bandwidth](https://docs.digitalocean.com/products/networking/bandwidth/),
  [Regions](https://docs.digitalocean.com/platform/regional-availability/).
- **Cloudflare:** [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/),
  [Workers limits](https://developers.cloudflare.com/workers/platform/limits/),
  [R2 pricing](https://developers.cloudflare.com/r2/pricing/),
  [R2 data location](https://developers.cloudflare.com/r2/reference/data-location/),
  [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/),
  [R2 S3 API](https://developers.cloudflare.com/r2/api/s3/api/),
  [D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/),
  [D1 limits](https://developers.cloudflare.com/d1/platform/limits/),
  [D1 data location](https://developers.cloudflare.com/d1/configuration/data-location/),
  [Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/),
  [Durable Objects data location](https://developers.cloudflare.com/durable-objects/reference/data-location/),
  [Regional Services](https://developers.cloudflare.com/data-localization/regional-services/).
