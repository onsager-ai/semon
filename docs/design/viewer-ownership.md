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
instance and explicitly invalidated on model/render transactions. Cyclic ancestry
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
Feature-specific mutable values stay behind their owners' typed accessors; one shared
composition connects those ports without copying state or constructing large
getter tables in the mount function. The source scan rejects explicit `any` and
type-check suppression directives throughout the application and consumer code.
