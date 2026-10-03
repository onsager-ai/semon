# Viewer ownership and migration transactions

Issue [#224](https://github.com/onsager-ai/semon/issues/224) tracks five remaining
legacy-host removal slices. Component rendering is integrated; the application
host and native shell are still being migrated. This record describes source
boundaries and does not claim production acceptance.

| Owner | Data and effects | Commit boundary |
| --- | --- | --- |
| Domain modules | Validated session/handoff/turn shapes, normalization, ancestry, outcomes, trace traversal, costs, labels and formatting | Prepare copied normalized data before updating the raw delta baseline or host tables. No document/global state. |
| Model owner | Raw validated delta baseline and normalized relationship indexes | Apply a validated model atomically; keep raw and normalized snapshots separate. |
| Transcript owner | Entries, range identity, growth marks, bounded cache, paging admission and stale briefs | Abort/stale range checks precede page application. Entry keys follow turn placement; slot identity survives extension. |
| Navigation owner | Current/rendered route, outgoing history, abortable destination loads | Save outgoing focus/scroll before native content teardown. Late/aborted native content is destroyed. |
| Scroll owner | Input revision, anchors, focus, opening-end pin and paging restore | Capture, synchronous Preact commit, measurement, restore; deferred restoration yields to reader input and newer routes. |
| Application owner | Chrome, screen roots, listeners, timers, frames, observers, requests and poller | Mount once per document; destroy invalidates pending work before releasing roots and effects. |
| Native shell owner | Rust fallback drawer, copy, confirmation dialogs and readiness polling | Native forms remain native; no second viewer router/model poller. |

The first slice removes superseded calculations from `viewer.js` when switching
callers to `ui/src/domain`. The raw wire snapshot never acquires failed-send
stand-ins or resolved turn references. Derived child lists are cached per domain
instance and explicitly invalidated on model/render transactions. Cyclic ancestry
and trace traversal terminate. Optional numeric and structural fields are checked
before use, including costs, activity, wait edges and tool/signal counts.

All newly extracted TypeScript is included in strict type checking and the
application security scan. Generated production assets remain checked in; Cargo
requires no Node. Browser reference images, layout, URLs, event contracts, CSP
and performance thresholds remain unchanged.
