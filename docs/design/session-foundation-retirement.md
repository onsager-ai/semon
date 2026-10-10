# Session foundation and staged subsystem retirement

Decision: center Semon on harness-native session evidence, focused reads and future
Session Intelligence. The implementation tracker is
[#380](https://github.com/onsager-ai/semon/issues/380). This assessment describes
the verified source tree and staged changes; it does not claim complete retirement,
production adoption or inspection of users' deployed services.

## Verified revisions

Audited on October 10, 2026:

| Repository | Main commit | Adopted Semon source |
| --- | --- | --- |
| Semon | `02a9ac17d525eb134b5952f7e55d30b47b16b8ec` | Not applicable |
| Hub | `9623986977ec69869276fa4224a939e0c9fa4312` | `8d38fbbf254d52a9b9bac123d47df8b9c593800d` |

The refreshed main revisions are Semon
`263a02da278716ab2309b8e34a463419e5c781e5` and Hub
`45a9a5d4d705775c6f65daf59058f4984c8b5cff`. Their source-pin contract and
Hub navigation/native-picker changes are retained in the staged PRs; the
retirement components audited below remain present on main until those PRs land.

Hub's gitlink is its sole source pin. Latest Semon and the adopted Hub/Semon pair
are distinct validation targets. Adoption PRs must run
`scripts/verify-source-pair.sh` on their clean committed pair and record both SHAs.

## Actual architecture and consumers

Semon Sessions discovers native Claude, Codex and Copilot inputs. Its per-file
Session Event Index (`crates/semon-sessions/src/events.rs` and `events/store.rs`)
persists metadata, complete-line ledgers, source generations and offsets. The
Catalog and saved native slot projections support current and retained-history
queries. Transcript and Entry readers reopen source evidence through verified
ranges. Facts carry allowlisted process, native selection and relationship
observations; cached facts do not grant execution authority.

Focused endpoints already include `/api/session-sources`, `/api/sessions`,
`/api/session-transcript`, `/api/session-entry`, `/api/session-identity`,
`/api/session-resolve` and `/api/session-capabilities`. Hub's selected query and
archive readers authorize source custody and validate it again after asynchronous
reads. Those foundations remain supported.

The remaining global costs are real code paths:

- `MachineView::refresh_model_locked` constructs `model::Built`, including native
  joins, text preparation and analytics. `note_catalog_read` schedules this same
  work. Cold demand can publish a batch of 64 sources before complete observation,
  but subsequently still builds the full selected machine. Catalog production is
  not yet independent of compatibility model construction.
- `ViewerCore` serves the workspace union, `/api/model`, `/api/tx`, older
  tree/transcript/entry routes and model deltas. Native derivation inside
  `model.rs` also feeds the focused projections; deleting that file would destroy
  shared identity, lineage, turns and source-backed semantics.
- `Query::from_core` and the local Query engine consume full model snapshots. The
  five CLI/stdio/hosted MCP tools are active consumers of this contract.
- Hub's `viewer.rs` restores tenant history for legacy requests. `warm_at_start`
  can restore archive content and `warm_now` calls `warm_machine`; pushes also
  prepare models. Hosted MCP calls `restore_tenant` before obtaining a core.
- The focused `/sessions` shell and machine-qualified selected-session shell
  avoid that legacy request path. Shared compatibility bootstrap, home/timeline,
  analytics/machine consumers, model SSE and native navigation still require an
  explicit migration. Existing no-JavaScript forms and authorization survive it.

Encrypted Relay lives in `crates/semon-relay`: crypto/auth/identity, protocol,
receiver/sender/discovery/state, snapshots, leases, takeover/restore and durable
deletion. Sessions' `remote.rs` and `semon sessions --remote` use it to decrypt a
remote metadata tree. `semon forget` also sends explicit queued remote deletion.
The repository ships timer/resident units and installation scripts. Hub and
`semon-runtime` have no direct protocol consumer, but Hub currently inherits Relay
through Sessions and Push. Hub's credential-vault cryptography is independent and
must remain.

The Canonical Trace Store lives in `crates/semon-store`, backed by
`traces.sqlite3`. `canonical_traces`, `occurrences`, `raw_carrier_records` and
`raw_record_traces` hold canonical content and capture provenance/forensic bytes.
Codex, Claude **and Copilot** capture adapters write it. Its readers are `semon
log`, explicitly separated `semon forensic`, canonical-only `semon ship`, repair
and deliberate forget commands. No current Viewer, mirror or runtime consumer of
these trace tables was found. The main `semon` binary and its integration tests
are nonetheless owned by this crate. The library itself does not need Sessions,
Control, Push or Relay; those dependencies belong to the binary.

Historical forensic records may be the only surviving copy after native logs
rotate. Native logs cannot be assumed to reproduce every existing forensic row.
That is an export obligation, not a reason to complete carrier-neutral memory,
capability negotiation, cue retrieval or cross-harness reactivation.

## Target architecture

```text
Harness-native logs and allowlisted native metadata
  -> source discovery and generic synchronization
  -> local or hosted source custody and provenance
  -> incremental Session Event Index and Session Catalog
  -> focused source-backed query APIs
  -> Viewer / CLI / MCP
  -> Session Intelligence derived from verifiable evidence
```

Hub PostgreSQL retains tenancy, authentication/authorization, credentials,
runtime coordination and operational metadata. Optional verified archive storage
and complete recovery artifacts meet explicit product requirements. No new
database, vector store, messaging system or generalized pipeline is needed.

## Component decisions

| Component | Classification | Action and removal boundary |
| --- | --- | --- |
| Native discovery/parsing, harness metadata, facts | Core and retained | Preserve allowlists and native semantics |
| Session identity, lineage, forks, parent/spawn/handoff/peer events | Core and retained | Extract shared derivation when separating model transport; preserve ambiguity |
| Session Event Index, source ledgers and generation validation | Core and retained | Keep existing SQLite index; distinct from Canonical Trace Store |
| Catalog, slot/source projections, Transcript/Entry readers | Core and retained | Preserve current/retained selection, completeness and source fencing |
| Push/mirror, received sources and SSH onboarding | Core and retained | Preserve generic transport, credentials, provenance and source evidence |
| CLI, Viewer, MCP, Control and Runtime | Core and retained | Extract CLI owner and migrate full-model consumers |
| Hub tenancy/RLS, vault, claims and uncertain-result reconciliation | Core and retained | Keep independent security/lifecycle machinery |
| Native semantic builder in model/tx | Shared dependency requiring extraction | Retain needed parsing/joins/slot semantics; remove only obsolete construction/transport |
| Main binary in semon-store | Shared dependency requiring extraction | Move to semon-cli with unchanged commands/tests first |
| compat=1, model/tx/delta/SSE, global hydration and warmers | Legacy compatibility to remove | Migrate every active UI/CLI/MCP/native consumer and its equivalence tests first |
| Encrypted frames/receivers/senders/resident, enrollment/keys, remote trees | Experimental subsystem to retire | Isolate dependencies, qualify legacy export/decryption, then remove code/new setup |
| Relay snapshot/lease/epoch/takeover/restore | Experimental subsystem to retire | Preserve historical artifacts; replace future runtime roadmap dependency |
| Canonical traces/occurrences/forensic store and capture/ship timers | Experimental subsystem to retire | Preserve inspection/export, extract CLI, then remove capture/replication/store |
| Ciphertext, enrolled keys, snapshots and deletion outboxes | Historical data | Inventory and verified export; never delete implicitly |
| traces.sqlite3, WAL/SHM, raw records and adapter cursors | Historical data | Consistent private backup and explicit export; never prune or delete on upgrade |
| Verified archives and recovery artifact contracts | Optional and retained | Use only with justified requirements, complete verification and authority |

Agent handoff/peer events called `relay` remain session semantics. They are not
encrypted transport. Redacted mirrors are readable by their receiver, use best
effort sender redaction and TLS, and **do not provide end-to-end encryption**.
They are not complete workspace/harness recovery artifacts.

## Dependency-ordered delivery

1. [#382](https://github.com/onsager-ai/semon/issues/382): make encrypted remote
   viewing an explicit transitional library feature; core/Hub do not enable it.
   Retain the small Node lease wire type and existing CLI access. Feature removal
   is owned by #386, rather than becoming a permanent compatibility API.
2. [#381](https://github.com/onsager-ai/semon/issues/381): extract the binary/tests
   into `semon-cli`, remove binary dependencies from the trace library, and update
   all build owners. The CLI temporarily retains `semon-store` for existing
   inspection/deletion commands; extraction alone is not Trace Store retirement.
   [Hub #227](https://github.com/onsager-ai/semon-hub/issues/227) adopts the exact
   source and updates production/managed/SSH/CI package selections.
3. [#383](https://github.com/onsager-ai/semon/issues/383): extract native catalog
   production from full-machine model/text/analytics preparation. Preserve
   dependency closure, disappearance reconciliation and non-rebuildable observed
   runs. Partial batches cannot claim complete lineage or authorize control.
4. [#384](https://github.com/onsager-ai/semon/issues/384) and
   [Hub #226](https://github.com/onsager-ai/semon-hub/issues/226): migrate local and
   hosted Query/MCP, preserving the five tool contracts and selected archive
   reads. Reuse Hub #122/#123/#124, rather than duplicating their owned work.
5. [#385](https://github.com/onsager-ai/semon/issues/385): migrate remaining UI,
   native destinations, analytics and machine/timeline consumers. Remove obsolete
   transport, routes, eager preparation and fixtures after replacement coverage.
6. [#386](https://github.com/onsager-ai/semon/issues/386) and
   [#387](https://github.com/onsager-ai/semon/issues/387): inventory external
   deployment/data access and qualify historical export before full subsystem
   removal. Remove obsolete installation/code/docs only when obligations are met.

Reuse completed #319/#320. Preserve unique scope/evidence in open Semon residual
PRs #341/#342/#345/#349/#355/#360 and native backfill #365. Coordinate Hub's
ongoing sync #212 and source/navigation adoption #224 before updating its pin.

## Historical-data and deployment migration

Checked-in units do not prove any deployed receiver/timer is absent. Required
operator inventory includes installed capture/Relay binaries and units, identity
and recipient/key custody, ciphertext generations, snapshot heads and forks,
deletion outboxes, state/cursor directories and configured replication endpoints.
Record private locations and version/checksum inventories without publishing keys
or payloads. No live service is stopped or uninstalled by these PRs.

Retain a pinned legacy binary/source revision while migration is qualified.
The [forensic Store exporter](../trace-store-export.md) provides complete,
versioned all-table snapshots for schemas 0 through 7. Its standalone
`semon-forensic-export` package builds without Store, capture, Sessions, Relay or
Runtime; the existing `semon forensic --export-store` alias delegates to the
same implementation during migration. It preserves original
DB/WAL/SHM/journal bytes and permissions. It requires quiescent source files,
verifies private input copies against their generations and full digests, and
uses SQLite only on those copies. Publication refuses source changes and newer
schemas. No capture service is stopped and no original is deleted. New periodic capture
installation, source timer templates and the three collector packages are
retired. Installed units and manual collectors still require explicit inventory
before export or uninstall. The [capture custody reference](../trace-capture-retirement.md)
pins the legacy source, locked build and repair/backfill contracts. Shared native
parsers/fixtures and the main CLI's historical log/forensic/export/forget commands
remain. The canonical-only `semon ship` sender is retired; no Store HTTP client or
replication setup remains. Store reader removal is a later dependency slice.

For legacy trace inspection, first make a consistent owner-private SQLite
backup using its backup API (including committed WAL state), and inspect that
copy. `TraceStore::open` reasserts permissions and may migrate schema or recreate
tables, so existing CLI access is **not read-only at the database level**. Do not
run it against the sole historical original when preserving exact database bytes.

`semon log --store COPY` reads canonical occurrences. `semon forensic --store
COPY --session SESSION --out PRIVATE_FILE` exports linked and unprojected raw
records for the selected session; trace selection alone misses unprojected rows.
This selected access is not a complete all-table/versioned export; use the
dedicated exporter for that migration obligation. `ship` is not a forensic backup.

Retain Relay signing/age identities, recipient enrollment, ciphertext and exact
legacy protocol version until decryption/export verification succeeds. Historical
recipients and divergent snapshot heads require explicit handling; adding a
recipient does not automatically rewrap every historical blob. Preserve gaps and
integrity failures as failures. Never substitute redacted mirror data for an
encrypted original or silently overwrite restore targets. No automatic data/key
deletion or retention change is part of retirement.

## Recovery boundary and roadmap reconciliation

`semon-runtime` already has `RecoveryStorage`, checkpoint references, durable
logical sessions/operations and writer-exclusion contracts without a Relay
dependency. [#255](https://github.com/onsager-ai/semon/issues/255) now specifies
explicit verified/versioned artifacts rather than an encrypted-frame adapter.
Complete independent reconstruction is still unqualified.

Artifacts must bind workspace/harness/runtime identity and versions, original
source generation, committed/uncommitted/untracked workspace files, required
harness state, consistent SQLite backups, thread/path metadata and dispatch
journals. Publication/read-back verification precedes checkpoint commit. Restore
checks scope, completeness, compatibility and hashes before publishing paths or
launching a harness. Current credential authority is revalidated and protected
credentials are reinjected through qualified custody. Artifacts cannot reauthorize
revoked credentials or replay unknown external writes. A stream lease, epoch or
backup is not proof that an old workspace writer is excluded.

Semon #3/#6/#8/#9 are superseded with historical obligations carried into the
retirement children. #7/#10 remain open for native Claude/Codex version/path/state
qualification through recovery artifacts. #248/#255 and Hub #67/#122/#123/#124
are reconciled. Historical decisions and useful evidence remain accessible; this
does not mark unmerged implementation or unrun recovery drills complete.

## Evidence and remaining gates

The staged catalog producer separates shared native derivation from legacy
analytics, window transport and model serialization. Bounded source preparation
and complete catalog observation return native rows/slot recipes before those
compatibility phases. Catalog-only background demand refreshes its own source
snapshot without publishing a `Built` model. Explicit legacy reads and model
warmers remain supported until their consumers migrate. Complete observation
still discovers and joins all native sources; this slice does not claim bounded
incremental reconciliation or removal of Hub legacy hydration and MCP dependencies.
[Hub #232](https://github.com/onsager-ai/semon-hub/pull/232) separately removes
startup tenant enumeration, full-model warming and archive hydration. Restart
tests retain Catalog and selected archived Transcript reads without a ViewerCore;
explicit legacy reads and push warmers remain until their consumers migrate.

Extraction must preserve authored CLI/test bytes and all existing command
behavior. Feature isolation must test both core and explicit encrypted-reader
builds; whole-workspace feature unification alone cannot prove core independence.
Each PR records actual Rust results and the exact tested tree. Adoption additionally
records restricted-role PostgreSQL/API, browser/native-control, typed-consumer
freshness and committed gitlink checks, with unrun checks named explicitly.

Full compatibility removal requires fixed-scope measurements while unrelated
history grows, plus identity/lineage/order/windows, paging/tool expansion,
reader-position/focus, runtime/control and no-JavaScript equivalence. Revocation,
deletion, source replacement, partial/corrupt archive data and cancellation must
fail safely. Existing fixture thresholds and approved pixel baselines remain.

The next local Query slice returns typed native evidence before compatibility
analytics, growth marks, pricing transport, serialization and version hashing.
All five local CLI/stdio MCP tools read that evidence; a call's window, identity,
facts and transcript offsets belong to the same cached source snapshot. The
shared ownership planner still qualifies machine-local stubs and refuses native
identity conflicts. Transcript paging and search read the original source lines.
Summary reads serialize summary fields and open questions; selected-session
reads serialize that session's turns and handoffs. No database or generalized
event pipeline is added.

This slice still discovers and joins every native source eligible for the local
file-selection window. It does not establish constant-cost selected-session
dependency closure or replace all query metadata with persisted Catalog rows.
`Query::from_core` retains the embedding server's cached compatibility contract;
Hub MCP still needs authorized selected archive/provider access and source-generation
fencing before that path can be removed. These obligations remain in #383/#384
and Hub #226; preserving the old hosted path during migration does not retire it.

The remaining blockers are consumer migration, shared semantic extraction,
remaining encrypted-data export qualification and external deployment inventories.
The versioned forensic Store export is qualified in
[#392](https://github.com/onsager-ai/semon/pull/392) and the independent exporter in
[#396](https://github.com/onsager-ai/semon/pull/396). Collector source retirement
depends on that qualified export; existing installations and subsequent Store
removal still require operator migration, not completion of the old memory roadmap. No merge,
deployment, live infrastructure change, paid harness/provider drill or user-data
deletion is authorized by this assessment.
