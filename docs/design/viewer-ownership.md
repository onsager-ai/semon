# Canonical Viewer ownership

The hosted Viewer has one bounded, source-scoped application. A document mounts
`createCatalogViewer` through `mountViewerApplication`; replacement destroys the
previous mount first. Home, Analytics, Sessions, selected transcripts and Trace
share that shell, navigation and effect lifetime. Query parameters do not choose
an application architecture. Old compatibility markers are removed while
preserving supported destinations, source selection and reading anchors.

| Owner | State and effects | Acceptance boundary |
| --- | --- | --- |
| `app/viewer` | Singleton mount and URL normalization | Destroy the previous mount before constructing the canonical reader. |
| `app/catalogViewer` | Source selection, navigation, bounded selected roots, source lists, transport and refresh cadence | Check source, request epoch and route before accepting or painting. |
| `CatalogSelectionStore` | Current-scope selected native identity | Historical identity never grants runtime or control authority. |
| `CatalogTranscriptStore` | Issued requests, source-qualified ranges, stable entry identities and projection generations | Reject holes, inconsistent overlaps, stale selection and mixed generations before replacement. |
| `app/overviewPages` | One accepted Home/Analytics response, facets, seen results and route panels | Adopt coherent metadata first; accept each visible preview only for its requested source generation. Reject changed publication versions. Pending Home/machine reads have one retry timer in the reader's EffectScope, with bounded backoff; navigation and teardown cancel it. |
| `app/tracePresentation` and `domain/trace` | Validated requested causal closure and pure rail/chart calculations | No workspace graph, model stream, native authority or separate router. |
| Shared `lib` components | Shell, Topbar, Recent, screens, panels, measured charts and transcript DOM | One owner per region; synchronous commits retain stable keys and native control roots. |
| Scoped control/runtime observers | Status streams, recovery and selected custody observations | Exact source/native identity, independent authorization and teardown govern every delivery. |
| `app/localControl` | Source-qualified drafts, pending operations and approvals | Preserve uncertain outcomes and edited drafts; do not replay writes. |
| Native host content | Server forms, readiness and application destinations | Shared chrome does not render into native descendants or intercept native submission. |

`EffectScope` owns requests, deadlines, listeners, timers and animation frames.
Settled requests leave that scope. Route changes close contextual panels and
abort superseded overview/causal work. Destroying the application invalidates
callbacks before destroying stores, controls, native content and screen roots.
Navigation's observable route is shared with scoped status observers; it has no
native-page loader or independent destination request lifecycle.

Home and Analytics read compact producer publications across authorized sources.
Machine details borrow that same producer with an exact source key, validated
before source admission, observation scheduling and preview reads. The canonical
reader retains machine bookmarks and shell headers; shared Machine rows use the
accepted sixty-row Recent publication and generation-fenced previews. A scoped
machine response cannot supply global Home counters or the Machines inventory.
The shared Machine screen owns its route-specific geometry class and removes it
when released; section and row styles must not leak into another route.
Home retains the established Needs You, Working Now, Answered and Recent
presentation. Native text is absent from persistent metadata: visible previews
use the qualified selected-session reader with bounded record and byte budgets.
Analytics uses timing, costs, waits and model observations captured before source
window trimming. Causal queries follow recorded starts, spawns and relays through
indexed metadata, with explicit limits and further branch navigation. Missing or
ambiguous observations remain visible rather than becoming inferred joins.

The producer publishes overview and causal metadata in the existing catalog
source-revision transaction. Native generation references remain original
provenance, never authority. Reading these publications neither admits a
workspace ViewerCore nor restores a workspace archive. Selected native reads
reauthorize their exact original generation and remain request-local.

Sessions retain source-scoped catalog pages, metadata search, native resolution,
transcript pagination, qualified field expansion and live revalidation. Lists
have 60-row pages and retain at most 300 metadata rows per Source. Five selected
roots and five Source views are retained. Each selected transcript keeps at most
five recently used ranges and 300 unique entries; revisiting a range updates its
retention order. Discarding a range also releases its presentation and field state.

The selected-session cadence observes a body-free catalog projection generation
and entry count. Unchanged observations do not reread native content. A changed
projection revalidates the accepted ranges through the existing selected reader.
One application ResizeObserver keeps an opening session at its tail while layout
settles; user input ends that initial hold. Listener/frame cleanup belongs to the
application effect lifetime.

Failure and signal navigation reads at most 100 indexed slot addresses per page,
then requests only the selected transcript range. Its close transaction restores
accepted ranges, expansion, focus and scroll. It never loads a complete transcript
or workspace failure model. Plain full-output chunks bind to their published
native result anchor, which can differ from the tool call's provenance. Structured
Codex output expands one qualified entry with a 64 KiB text cap and retains actual
recorded gaps; script and image reads remain independently qualified.

Home sections have 100-row pages. Recent uses a 60-row publication and eight
ancestry roots, borrowing at most 32 ancestors, 99 runs and the current selected
session from its verified family when it is outside that publication. The selected
path remains visible. Approval-review roles use presentation metadata without
changing native harness/kind or cost identity; hidden reviews forward descendants
and current selections retain their ancestors. Ordering holds belong to the same
application owner during pointer, focus, scroll and open-menu interactions.

Old failed-send bookmarks resolve through a private indexed target-to-owner table.
The lookup returns only authorized, source-qualified real session identities;
ordinary causal metadata does not expose native target labels. Ambiguous matches
require a Source choice. Explicit expansion remains separate from background
publication and native authority.

The session composer is owned by the existing session/local-control components.
Its Topbar trigger opens shared contextual Panel chrome on desktop and a bottom
sheet on mobile. Closing or changing route releases panel effects and restores
focus without replacing the control root or losing its draft. Connection status,
read-only behavior, native controls and pending approvals keep their existing
owners. Jump to Bottom belongs to the reading dock and is independent of that
panel. The New Session native creation composer is a separate product flow.

Local CLI and stdio MCP share native Session Query. Hosted focused MCP admits
authorized native selections with bounded worker/custody lifetime; broad hosted
queries still require archive-cohort retirement. ViewerCore retains standalone
native production, the supported CLI tree and compatibility APIs awaiting code
retirement. Hosted browser content does not consume
`/api/model`, model deltas or the retired Hub compatibility/control endpoints.
The former composition, complete-model store, model poller, parallel transcript
owner and compatibility bootstrap are removed. Pure shared presentation,
formatting and causal calculations remain where the canonical reader needs them.

## Refactoring contract

This document is the canonical viewer refactoring contract. AGENTS.md and the
viewer-verification workflow require it; they link here rather than duplicating
the rules. Update the contract, affected tests and consumer evidence together
when an intentional architectural change requires different ownership.

| Rule | Reason and evidence |
| --- | --- |
| The application mount constructs the canonical reader; feature modules do not import that reader, even for types. | A universal context recreates hidden coupling. Declare named types and narrow capabilities of actual feature owners. The architecture check rejects canonical reader imports, including aliases and literal dynamic imports. |
| Keep one authoritative owner for each mutable value. Borrow its tables or coherent projections instead of adding aliases, forwarding tables or another store/router/poller. | Replacement references must remain current. Review ownership changes and test interactions after replacement; strict types alone cannot prove that a port is appropriately narrow. |
| Runtime module imports remain acyclic; intentional type-only interaction cycles are permitted. Use literal module paths. | Construction callbacks must resolve owners when invoked rather than capture unconstructed services. The architecture check uses TypeScript resolution and emitted imports, including re-exports and literal dynamic loads. |
| Validate complete catalog, range, overview and causal responses before replacing accepted state; publish once after the transaction is coherent. | Invalid input must not expose partial state. Keep rejection/coherence regression tests. Transcript projections borrow entries/ranges; avoid full transcript copies on updates. |
| Check request, range, input and navigation identity before accepting asynchronous work. Dispose subscriptions and invalidate pending work during teardown. | Aborted, superseded or destroyed owners must not commit or notify. Retain late-response, paging and remount coverage. |
| Assign exactly one owner to each DOM region. Viewer pages use the synchronous application commit boundary; native descendants stay host-owned. | Keyed controls, focus and scroll must survive ordinary updates. Measured geometry, native forms and independent embeddings retain documented adapters and explicit disposal. |
| Keep state preparation, synchronous commit, measurement and guarded restoration in their named owners and documented order. | Async hook timing cannot replace focus/scroll guarantees. Retain reader-input, opening-end and Back/Forward tests. |
| Separate formatting from behavior, edit canonical sources and regenerate assets explicitly. Keep strict types, security, freshness, visuals and performance gates. | Readability changes should be reviewable independently. Preserve pinned tooling and blame exclusions; compare equivalent bundle costs rather than relaxing budgets. |
| Verify consumers independently when shared contracts change. Final gitlink/revision pins must identify the same reviewed, verified source; coordinated draft PRs pin the exact proposed source commit. | A passing source build does not establish independent consumer security, runtime uniqueness, native behavior or asset freshness. Record exact source/consumer revisions and applicable gate results. |

`npm --prefix ui run check:architecture` checks authored TypeScript and JavaScript modules
under `ui/src`, using the UI TypeScript configuration. Dependencies outside that
source tree are outside its cycle graph; dependency/runtime uniqueness checks
remain with the build. It does not prove atomicity, port cohesion, DOM ownership
or teardown correctness: those require review and behavioral tests.

The architecture check has no cycle allowlist or blanket suppression. Exceptions
to other rules must name the affected owner/region, explain why the existing
pattern cannot serve it, show lifecycle and consumer evidence, and update this
contract through review. Existing measured/native/embedding adapters are listed
in the ownership map above. Do not add arbitrary file-size or complexity quotas.

## Refactoring review checklist

Include this checklist's evidence in the PR; mark an item not applicable with a
reason when that boundary is unchanged. Keep exact commands in their existing
owning references rather than copying toolchain setup here.

- Identify the state, DOM and effect owners before and after the change; record
  changed capability ports and any intentional type-only interaction cycles.
- Confirm one authoritative owner per value/region and no new universal context,
  proxy table, duplicated router/poller or undocumented native-content mutation.
- Show invalid/stale input rejection, coherent notification and disposal evidence
  for changed transactions; verify replacement references stay current.
- Show retained key/focus/scroll and synchronous measurement evidence for changed
  rendering; cover relevant standalone, sidebar, native and independent mounts.
- Run architecture, formatting, types, source security and deterministic/freshness
  checks; select applicable lifecycle/browser/Rust suites and inspect their actual
  aggregates. Record skipped coverage and blocked prerequisites honestly.
- Compare equivalent bundle/performance costs when runtime code changes; review
  visual diffs without loosening budgets or silently changing baselines.
- Record independently verified consumer revisions/pins when shared contracts
  change, and update this ownership map and any justified exception before merge.

## Compact native composer adoption

The opt-in composer remains a consumer-owned native form. Shared chrome owns
compact geometry, responsive Panel/Select picker behavior, tooltip overlay and teardown;
consumers own labels, authorized catalogs, persistence, readiness and submission.
`mountNativeShell` mounts one `mountComposers` owner and destroys its listeners and
open pickers. Picker content is a host-owned slot moved into a shared panel inside
the original form. Named native selects stay the form value owners; shared Select
notifies them with change events. Teardown restores the native controls and details.
Native-shell bundles reuse the guarded shared runtime and Select history owner.
It never reads credential fields or installs a submit controller.
`ViewerHost.nativeNavigation` supplies native destinations through the existing
navigation root; shared safe-path validation and native links apply. It creates
no additional router, model store or poller. The compact gallery covers geometry,
Escape/focus return and remount; Hub provides the real server/form adoption checks.

## Selected observation

Navigation publishes route changes through `subscribeRoute`. Catalog status
borrows the accepted selection owner's narrow identity accessor and subscription.
A missing, superseded, ambiguous or multi-native selection opens no native status
stream and grants no control authority. The host authorizes exact current source
custody independently; historical transcript identity cannot enter that selection
store. Returning to a native thread invalidates previous selection epochs.

One navigation-scoped control observer and one runtime observer update the
existing local control owner and cached Session root. Their snapshots do not
adopt content, reconstruct a model or replace transcript blocks. Transport
freshness, content publication and native runtime facts remain distinct.

## Staging reader ownership

The shared transcript owns Thinking disclosure by stable entry identity and the
reading dock's viewport/resize/scroll listeners, measurements and teardown.
`readingBottom` and `readingClip` are finite geometry values; they change paint
and dock placement, never scroll ownership or transcript identity. Catalog names
refresh through the existing bounded capability cadence (60 metadata rows, the
active exact lookup, and at most eight rotating exact lookups for loaded/cached
history outside the first page). Observed names survive unchanged older content
briefs until a later metadata observation. A name update changes
the cached metadata, header and Recent leaf without rerendering transcript entries.

The local control owner holds drafts by source-qualified session identity. The
composer consumes observed control authority and runtime state; an initial missing
control has a read-only disclosure instead of an absent input surface. Recovery
retains the same draft. Acceptance clears only the submitted draft if the reader
has not edited it meanwhile. Uncertain delivery is never retried automatically.


Codex name observations were checked against installed Codex 0.162.0 and its
[session index](https://github.com/openai/codex/blob/rust-v0.162.0/codex-rs/rollout/src/session_index.rs)
and [thread state](https://github.com/openai/codex/blob/rust-v0.162.0/codex-rs/state/src/runtime/threads.rs).
The latest complete valid `session_index.jsonl` record wins; `state_5.sqlite`
`threads.name` supplies a fallback. `threads.title` is prompt-derived, and
`updated_at` is activity time: updating `name` does not advance it, so it cannot
rank a database name against an index rename. Unsupported schemas fail closed.
Claude's existing `custom-title`/`customTitle` transcript naming remains supported.

Extraction reads at most a 4 MiB index tail, 16,384 records and 4,096 names; each
name is at most 1,024 bytes. Database reads are read-only, limited to 4,096 rows,
25 ms lock wait and a 50 ms VM progress deadline, with a 256 MiB file cap. A WAL
without an existing shared-memory sidecar is skipped to avoid creating native
files. Only validated name observations are carried in machine facts, with native
harness/ID, metadata source, observation time and explicit current/stale freshness.
Received sources use their own facts and never the viewer host's native database.
Metadata changes invalidate the model snapshot but preserve transcript marks.
