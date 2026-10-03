# Viewer ownership and migration transactions

Issue [#224](https://github.com/onsager-ai/semon/issues/224) tracks the completed five
legacy-host removal slices and final consumer integration. The application
host and native shell are composed from strict typed controllers. This record
describes source boundaries and does not claim production acceptance.

| Owner | Data and effects | Commit boundary |
| --- | --- | --- |
| Domain modules | Validated session/handoff/turn shapes, normalization, ancestry, outcomes, trace traversal, costs, labels and formatting | Prepare copied normalized data before updating the raw delta baseline or host tables. No document/global state. |
| Model owner | Raw validated delta baseline and normalized relationship indexes | Apply a validated model atomically; keep raw and normalized snapshots separate. |
| Transcript owner | Entries, range identity, growth marks, bounded cache, paging admission and stale briefs | Abort/stale range checks precede page application. Entry keys follow turn placement; slot identity survives extension. |
| Navigation owner | Current/rendered route, outgoing history, abortable destination loads | Save outgoing focus/scroll before native content teardown. Late/aborted native content is destroyed. |
| Scroll owner | Input revision, anchors, focus, opening-end pin and paging restore | Capture, synchronous Preact commit, measurement, restore; deferred restoration yields to reader input and newer routes. |
| Application owner | Chrome, screen roots, listeners, timers, frames, observers, requests and poller | Mount once per document; destroy invalidates pending work before releasing roots and effects. |
| Native shell owner | Rust fallback drawer, copy, confirmation dialogs and readiness polling | Native forms remain native; no second viewer router/model poller. |

The domain slice removed superseded calculations from the handwritten host when switching
callers to `ui/src/domain`. The raw wire snapshot never acquires failed-send
stand-ins or resolved turn references. Derived child lists are cached per domain
instance and invalidated by accepted model/transcript notifications. Cyclic ancestry
and trace traversal terminate. Optional numeric and structural fields are checked
before use, including costs, activity, wait edges and tool/signal counts.

All newly extracted TypeScript is included in strict type checking and the
application security scan. Generated production assets remain checked in; Cargo
requires no Node. Browser reference images, layout, URLs, event contracts, CSP
and performance thresholds remain unchanged.

The model owner is `ViewerModelStore`. It validates a prepared normalized graph
before committing the raw delta baseline, then updates its stable table/index
objects. `TranscriptStore` owns entries, ranges, the existing bounded cache and
paging store. Every request has an owned abort controller; destroying the store
aborts all requests and rejects late page commits. Each directional load checks
the range object's identity and requested boundary before applying its page.
Loaded entries are distributed into the latest model's turns, including when a
model update arrives during loading. An explicit transcript-wire parser validates
entry variants and ranges at the response boundary.

The document entry calls `mountViewerApplication`; it returns a teardown owner
that invalidates callbacks before releasing navigation, model/transcript, poller,
pager, scroll, chrome, screen and dialog owners. `EffectScope` owns document and
media-query listeners, timers, animation frames and outstanding requests. Settled
requests leave the scope so long-lived polling does not retain old controllers.
Application controllers consume explicit typed ports instead of importing a
second router/model/poller. Late boot responses cannot notify the host or recreate
a root after teardown; replacing a mount first destroys the previous owner.

The native shell owner is `mountNativeShell()` in `ui/src/app/native-shell.ts`.
It retains Rust markup and native forms, owns each enhancement listener/timer and
aborts readiness requests before removing effects. Replacement mounts destroy
the previous owner. Full-viewer pages continue to use their existing drawer
owner; sidebar-only and native pages consume this separately served entry.

`mountViewerApplication` replaces the previous owner and constructs a typed service
composition. It carries no model, navigation or rendering algorithms. Focused
factories own transport/adoption, boot, paging, layout preferences, history and
focus, account controls, held ordering, navigation markup, native dialogs/tool
views, screen commits, session lists, document events, live updates and clocks.
Feature-specific mutable values stay behind their owners' typed accessors. Composition
passes service owners directly; consumers select only the capabilities they need. The source scan rejects explicit `any` and
type-check suppression directives throughout the application and consumer code.

## Maintainability boundaries (#277)

Before this change, screens, session chrome and document rendering each selected
many independent fields/callbacks from a universal composition surface. Model
and transcript tables had uppercase aliases, while composition forwarded domain,
transport, paging, navigation and presentation methods. Screen rendering also
owned control slots and performed state preparation around its DOM commit.

The composition now constructs owners, connects lifecycle effects and tears them
down. It no longer forwards those feature methods or duplicates table aliases.
`modelStore` and `transcripts` are the authoritative data owners. Named imported
types and `Pick` capability ports expose exactly the service members required by
each consumer; feature modules never import `ViewerComposition`. Registry
constants are imported directly. Remaining shared services are the document
query helper, presentation clock/formatting, media query, dialog registry, domain
indexes and route/filter preferences; these belong to this one document mount.

| Consumer | Cohesive dependencies |
| --- | --- |
| Screen projections | Model/domain relationships, held ordering, transcript projection, destination commands, tool/paging commands |
| Session chrome | Validated account/model data, layout preferences, navigation/error controller, overlay lifetime and viewport commands |
| Document renderer | Page/native root adapter, route control lifetime, screen owners, bar presentation, render transaction |
| Live updates | Validated model adoption, transcript requests/ranges, dirty-turn calculation, live/refresh owners |
| Composition | Construction and disposal of the above; no replacement proxy/getter table |

Runtime imports between feature factories remain acyclic. Type-only capability
imports describe intentional interaction cycles: destination → render → screen
callbacks → destination; session chrome ↔ viewport; live updates ↔ error
navigation/refresh; analytics ↔ render/filters. Callbacks crossing construction
boundaries resolve the service when invoked, never capture an unconstructed
owner. Stable model tables remain borrowed; replacing a normalized graph updates
those tables in place. Error navigation reads the current growth marks through
its transport owner, rather than retaining the boot-time marks object.

## Accepted state and view transactions

1. Prepare the wire model and all auxiliary fields (growth marks, admin, account,
   native navigation). Normalize/validate the entire graph and machine data
   before changing the raw delta baseline or stable normalized tables.
2. Commit tables, clock, account/navigation fields and transcript placement into
   the latest graph synchronously. Publish one `ViewUpdates` model revision only
   after that state is coherent. Host account callbacks run after publication.
3. Transcript replacement/directional loads retain their abort/range/boundary
   guards. Accepted pages spread entries into the latest turns, then publish one
   session notification. Cache adoption also spreads before publication.
   Live tail/watch metadata publish after their range/identity checks.
4. The domain's one subscription invalidates derived relationships on accepted
   updates. Session-filtered subscribers receive model changes and only their
   own transcript changes. `TranscriptStore.view` borrows entries/range with a
   revision; it does not deep-copy the transcript. Invalid/stale input publishes
   nothing. Teardown disposes subscriptions before aborting owners.
5. Live update generation guards suppress superseded asynchronous refreshes.
   Held ordering, dirty-turn selection and pending overlay refresh remain with
   their existing owners. Notifications invalidate data, not whole DOM trees.
   The single poller still drives refresh; no second store/router is introduced.

`renderTransaction.begin` resets paging input, holds programmatic scroll, closes
account controls, ends the opening pin, captures ordering and advances the clock.
The renderer commits page before bar (counts/metadata depend on page projections).
`complete` applies layout, bar, navigation, retained Recent lanes, drawer account
and jump controls, then releases temporary ordering state. Scroll owners still
capture before commit, synchronously measure afterward and restore only while
the input/navigation revision permits it. Back/Forward and opening-end pins keep
their reader-input guards; no asynchronous hook timing is assumed.

## Preact and native ownership map

| Region | Owner / lifetime |
| --- | --- |
| Viewer frame/navigation/drawer | Existing declarative `createShellChrome`; one shell per mount |
| Viewer `#page` | One `ApplicationView` tree; Home/Machines/Machine have typed view components, Sessions/Analytics/Trace/Transcript use the same synchronous commit boundary |
| Page transitions | `pageRoot` releases previous screen effects/root when the screen kind changes; same-kind updates retain keys and controls |
| Native Machines/native pages | Host `ViewerContent` descendants; `pageRoot.native` releases the viewer root before connection and never renders inside host content |
| Native facet widgets | `routeControls` explicitly retains/destroys route-scoped independent controls; no slot map in document rendering |
| Topbar host slots/Recent | Existing independent owners retained for host geometry and embeddings |
| Trace edges/charts/dialog bodies | Measured or independently embedded subroots; explicit cleanup before their containers are removed |
| Sidebar-only pages | Independent navigation/Recent roots; host retains frame and page |

`clearBox` and the obsolete manual placement path are removed. Preact controls
page descendants, while the small page adapter handles only Rust fallback/native
transitions. Native POST forms, CSP, safe links and the public mount/destroy API
are unchanged. All page commits remain synchronous. See the validation record
in `viewer-maintainability-validation.md` for equivalent size and gate evidence.

## Refactoring contract

This document is the canonical viewer refactoring contract. AGENTS.md and the
viewer-verification workflow require it; they link here rather than duplicating
the rules. Update the contract, affected tests and consumer evidence together
when an intentional architectural change requires different ownership.

| Rule | Reason and evidence |
| --- | --- |
| Composition constructs, connects and disposes owners; feature modules do not import or re-export it, even for types. Only the application mount imports composition. | A universal context recreates hidden coupling. Declare named types and narrow capabilities of actual feature owners. The architecture check rejects composition imports, including aliases and literal dynamic imports. |
| Keep one authoritative owner for each mutable value. Borrow its tables or coherent projections instead of adding aliases, forwarding tables or another store/router/poller. | Replacement references must remain current. Review ownership changes and test interactions after replacement; strict types alone cannot prove that a port is appropriately narrow. |
| Runtime module imports remain acyclic; intentional type-only interaction cycles are permitted. Use literal module paths. | Construction callbacks must resolve owners when invoked rather than capture unconstructed services. The architecture check uses TypeScript resolution and emitted imports, including re-exports and literal dynamic loads. |
| Validate the entire model and auxiliary input before writing the raw baseline or normalized state; publish once after the accepted transaction is coherent. | Invalid input must not expose partial state. Keep rejection/coherence regression tests. Transcript projections borrow entries/ranges; avoid full transcript copies on updates. |
| Check request, range, input and navigation identity before accepting asynchronous work. Dispose subscriptions and invalidate pending work during teardown. | Aborted, superseded or destroyed owners must not commit or notify. Retain late-response, paging and remount coverage. |
| Assign exactly one owner to each DOM region. Viewer pages use the synchronous application commit boundary; native descendants stay host-owned. | Keyed controls, focus and scroll must survive ordinary updates. Measured geometry, native forms and independent embeddings retain documented adapters and explicit disposal. |
| Keep state preparation, synchronous commit, measurement and guarded restoration in their named owners and documented order. | Async hook timing cannot replace focus/scroll guarantees. Retain held ordering, reader-input, opening-end and Back/Forward tests. |
| Separate formatting from behavior, edit canonical sources and regenerate assets explicitly. Keep strict types, security, freshness, visuals and performance gates. | Readability changes should be reviewable independently. Preserve pinned tooling and blame exclusions; compare equivalent bundle costs rather than relaxing budgets. |
| Verify consumers independently when shared contracts change. Final gitlink/revision pins must identify the same merged, verified source. | A passing source build does not establish independent consumer security, runtime uniqueness, native behavior or asset freshness. Record exact source/consumer revisions and applicable gate results. |

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
