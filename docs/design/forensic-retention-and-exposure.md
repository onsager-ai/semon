---
depends_on: [carrier-neutral-trace-storage.md, trace-identity-and-occurrences.md]
identity_impact: none — this governs the forensic region only, which never participates in identity
---

# Forensic retention and exposure policy

**The forensic region is retained in full and indefinitely. It is readable through one explicitly named command that supports bulk selection. The ordinary read path cannot reach it, and that boundary is enforced by a test rather than by convention.**

Decided 2026-09-20. This closes the P4 question from the September roadmap: *what the forensic region retains, what can be read out, by what command, and what is refused.*

## What the region actually holds

*(measured 2026-09-20, one real captured Codex session)*

| | |
|---|---|
| Canonical traces | 46 KB |
| Forensic region | **2.16 MB — 47x the canonical region** |
| Occurrences | 95 rows |

Content, by category rather than by example: 66 verbatim command lines with their working directories, 64 captured `stdout`/`stderr` blocks, 18 file-change records carrying absolute paths and full file contents, and verbatim message text. The README's warning is accurate and not hypothetical — this region can hold prompts, responses, source code, credentials, commercial data and machine paths.

At the time of this decision **nothing read it**: there was no CLI path to it, `ship` explicitly never touches it, and the only caller of `fetch_raw_carrier_records` outside the store was a test.

## Decision 1 — retain in full, indefinitely

No pruning by age or size, no opt-out, no sampling. Capture writes every source line verbatim as it does today.

**Why.** The region is the only thing that makes a bad projection recoverable without the source files, and the projection is young and still moving: it changed in #9, and it will change again in #10 and #16. Each such change means re-deriving traces from something. Source files are the alternative, and they are outside this project's control — Codex decides when to rotate or delete them, and a hedge that depends on another tool's retention policy is not a hedge.

**Costs accepted, recorded here so a later reader knows they were chosen rather than missed:**

- Storage grows at roughly 47x the region anyone reads.
- Exposure is permanent and cumulative. Every credential that has ever passed through a captured session stays in the file until the file is deleted.
- Alternatives were on the table with these costs stated — opt-in with the region off by default, and retain-with-age-pruning — and were not chosen.

**Consequence that follows and is not optional.** Because retention is unbounded, retention is no longer a mitigation. The entire remaining protection is file access and the shape of the read path, which the next two decisions govern. They carry more weight than they would under a pruning policy, and must not be traded away later on the grounds that "it is only local".

## Decision 2 — the store is private to its owner

The database file is created `0600` and its directory `0700`.

This is a defect fix, not a policy choice. The store was being created world-readable at `0644`, while the cursor file — which holds byte offsets and nothing else — was already `0600`. The file holding credentials was less protected than the file holding integers.

Permissions are set at creation and re-asserted on open, so an existing store with looser permissions is tightened rather than trusted.

## Decision 3 — one named command, bulk-capable

`semon forensic` is the only path from the CLI to raw records. It selects by trace, by session, or by day.

**Bulk selection is deliberate.** Reconstructing an incident means reading a whole session, and a tool that can only return one record at a time forces whoever needs that into `sqlite3` — an unaudited path with no guardrails at all. Honest capability beats a bar that is theatre against anyone who knows SQL.

**What the command must do:**

- Be named for what it does. Not a flag on `log`, not a `--raw` modifier on an ordinary read. A reader of a shell history should be able to see that forensic data was accessed.
- Write a one-line warning to stderr, before output, naming what the output may contain.
- Offer `--out FILE`, which writes to a file created `0600` instead of stdout.

**`--out` is an option, not a requirement.** Requiring it for bulk modes would re-impose, through implementation, the narrower command that was explicitly not chosen. The risk it addresses is real — shell history, scrollback and screen shares are where this output should never land — but the answer to that is to make the safer path available and obvious, not to remove the capability.

## Decision 4 — the boundary is enforced, not documented

The ordinary read path must remain structurally incapable of reaching the forensic region.

The enforcing test asserts that `semon log` renders **byte-for-byte identically after `DROP TABLE raw_carrier_records`** — not after emptying it. The difference matters: an emptied table lets a stray join return silently fewer rows, whereas a dropped table turns any surviving read into a hard `no such table` error. The gate is therefore *"the ordinary read cannot express a raw access"*, not *"it happens to produce the same output"*.

This form was adopted after a weaker version of the same check passed vacuously during review — comparing two empty error-path outputs and reporting them identical. A control that cannot fail is worse than no control, because it is believed.

**The gate belongs to the region, not to `log`.** Any read surface added beside it inherits the same requirement.

## Measured consequence: the region cannot currently be deleted

*(measured 2026-09-20, after implementation)*

`TraceStore::open` unconditionally re-runs the additive schema, including `CREATE TABLE IF NOT EXISTS raw_carrier_records`. This is pre-existing and load-bearing — it is how in-place schema upgrades work — but it has a consequence this policy has to state rather than leave to be discovered:

**Dropping the forensic table does not remove it.** Verified: after `DROP TABLE raw_carrier_records`, the next command silently recreates it empty, and `semon forensic` returns an empty success rather than an error. An operator who drops the table to reduce exposure gets it back, with no message saying so.

Two further facts follow, and the second is the uncomfortable one:

- It bounds how the boundary of Decision 4 can be tested. A dropped table only stays dropped within a single never-reopened connection, so the enforcing tests live at the store layer rather than at the CLI. That is a real constraint on the test, not a weakening of the guarantee.
- **There is no supported way to delete forensic data at all.** No command removes raw records, and deleting the store file destroys the canonical traces and occurrences with them. So the property deliberately built in [trace-identity-and-occurrences.md](trace-identity-and-occurrences.md) — that occurrences are persisted independently and the log renders on a store whose raw region is empty, which is enforced by a passing test — currently has **no mechanism that can bring that state about**. The capability is proven and unreachable.

Under Decision 1 this is consistent: retention is unbounded, so nothing is supposed to delete raw. It is recorded because "we chose never to prune" and "there is no way to prune even deliberately" are different statements, and only the first was decided.

## What this does not decide

Whether capture is installed. That is a separate decision: running `scripts/install-user-timer.sh` creates the credential-bearing artifact described above on a schedule, and this document only establishes the policy that was its precondition.
