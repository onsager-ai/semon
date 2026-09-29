# The viewer in TSX components: bundler and runtime compared

Status: proposal, 2026-09-30. This doc lays the options side by side with measurements and recommends one bundler and one runtime. **Marvin picks** (see [Decisions for Marvin](#decisions-for-marvin)); nothing in the viewer changes until he does. The numbers come from `docs/design/tsx-bench/` and its workflow, `.github/workflows/tsx-bench.yml`.

**Recommendation in one line:** esbuild as the bundler, and a hand-written `h()` JSX factory with no runtime, keeping the viewer's own live-update machinery; the runner-up is esbuild with Preact, vendored. The reasons are in [Recommendation](#recommendation).

## Where the viewer is today

Verified on main at 0f49fb3:

- `crates/semon-sessions/src/viewer.js` is 2,547 lines and 242,840 bytes: one IIFE whose closure holds the state (`SESS`, `H`, `TX`, `TXM`, `TURNS`, `TURN`, `HOLDS`, `LIVE`, `SLOTS`, `ERR`, `AN` and more) and builds every node imperatively through `el()`, `icon()` and `svgEl()` (`createElement`, `setAttribute`, `textContent`). No framework, no build step.
- It is not served alone. `shell::VIEWER_JS` is `concat!(tooltip.js, "\n", select.js, "\n", viewer.js)` (`shell.rs:12-18`), served at `/viewer.js` (`viewer.rs:1571`). `tooltip.js` (214 lines) and `select.js` (261 lines) also open `shell::JS`, the script of server-rendered pages, so they belong to two bundles. The CSS is joined the same way already: `VIEWER_CSS = concat!(viewer.css, "\n", select.css)` (`shell.rs:8`).
- Embedders serve `VIEWER_JS` themselves and may prepend a prelude that sets `window.semonEmbed` and listens for `semon:refresh`, `semon:polled` and `semon:ended` (`docs/shell.md`, "Embedding the viewer page"). The page is served verbatim with one `<script src="/viewer.js" defer>`.
- The content security policy is `default-src 'self'; script-src 'self'; style-src 'self'` (`viewer.rs:308`): no `eval`, no inline scripts, no inline style attributes.
- Rust tests read the script. Two read the source text: `harness.rs:117-146` requires one line starting `  const HARNESSES = {` that mirrors the Rust registry, and `shell.rs:252-283` requires `renderNav()` to draw `NAV` in order with the same icon paths. One reads the served bytes: `viewer.rs:3328-3386` fails if `VIEWER_JS` contains `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, `eval(`, `new Function`, `setAttribute("style"` or `cssText`, or sets a native `title`. That last test is a security property of whatever ships, not of our source.
- Live updates poll `/api/model?since=` every 2 s with backoff, plus a poll soon on the embedding page's `semon:refresh`; the SSE stream is planned (`overhaul.md`, Loading), not on main. On a session page an update computes the dirty turns (`dirtyTurns`), redraws only those (`transcript(sid, { only: dirty })`), swaps each whose DOM signature changed (`morphTurns`, `sig`), and wraps the swap in `capture()` and `restore()`: the anchor block by identity and offset, what was open, focus, and "Show more" measured synchronously before the scroll is placed, then placed again two frames later. Other pages redraw whole inside the same capture and restore. The sidebar is rebuilt with `replaceChildren` on every update, and `tooltip.js` moves an open tip to the node that replaced its target.
- Node and Playwright are CI-only (`tests/ui/package.json`; `ui.yml`: "the Rust build doesn't depend on them"). A Viewer UI run on main takes 18 to 20 minutes (the last ten successful runs). On #81's branch, 22 of its 28 Viewer UI runs failed.
- The overhaul is being ported one screen per PR (`docs/design/overhaul.md`): PR 1 (#72) is merged; PR 2 (#81, the top bar and ⋯ menu; +164 −228 lines in `viewer.js`) and PR 3 (#106, the transcript, draft; +345 −442) are open. PR 4 is the sidebar and rail, PR 5 Home, Sessions and Machines, PR 6 Trace with the Timeline, PR 7 Analytics (from #72's and #81's descriptions); PR 8's scope is not written down in a PR yet, and the overhaul's remaining row is its States (empty, error, not found, loading), so that is the likely scope (inference). Also open on `viewer.js`: #126 (lists keep their order, +184 −27) and #129 (a subagent's brief shows once, +6 −3). #58 touches `viewer.css` only.

## Goals and non-goals

Goals:

- **Component boundaries and types.** Each screen and each repeated block (a sidebar row, a turn, a step, a card) is a function with typed props, in its own module. The data from `/api/model` and `/api/tx` has TypeScript types. A wrong prop, a missing field or a typo in a class-name helper fails `tsc --noEmit` in about a minute of CI instead of a Playwright check twenty minutes in.
- **No behaviour change during the port.** Porting a screen that the overhaul doesn't change leaves the pixel comparison, the checks and the perf budget where they were. The overhaul PRs change behaviour by design; writing them as components is allowed to change exactly what the overhaul changes and nothing else.
- **No Node for `cargo install`.** The bundle is checked in; the Rust build reads it with `include_str!` as today. No `build.rs`, no JS toolchain in cargo.

Non-goals: a dev server or hot reload (the machine rules keep browsers and bundling off the local machine, see [The local loop](#the-local-loop)); server-side rendering; changing the served URL, the embedding contract or the CSP; moving CSS into components; generating the TypeScript types from the Rust structs (possible later, not needed to start).

## Measurements

The bench (`docs/design/tsx-bench/`) writes one sample four times, once per runtime: a typed port of `tooltip.js` (shared by all four, as it has no components), the sidebar's session tree with #126's order rule and its "N updated" chip, and a brief clamped with "Show more". It also bundles today's served script as it is (`legacy`: `tooltip.js`, `select.js` and `viewer.js`, joined as `VIEWER_JS` joins them), to time a viewer-sized input. The workflow installs each bundler, bundles each target as one IIFE, times the first build after install (cold) and ten more after two warm-ups (warm, hyperfine), records sizes, runs the viewer's banned-string test against each bundle, times a single-file transform, and runs `tsc --noEmit` per runtime. The sample's `tsc` passes for all four runtimes, so the code is well-typed, not pseudo-code. It is not run in a browser: the bench measures the build, not rendering speed.

Run [36639736179](https://github.com/onsager-ai/semon/actions/runs/36639736179), at 5786e97, on a 4-vCPU GitHub runner (AMD EPYC 7763), Node 22.23.2. esbuild 0.28.2, Bun 1.4.2, Vite 8.3.1 (Rolldown 1.2.11), vite-plugin-solid 2.11.14, Babel 7.29.7 with babel-preset-solid 1.9.15, TypeScript 7.0.2; Preact 10.29.8, Solid 1.9.15, lit-html 3.3.3. An earlier run on an Intel runner gave the same ordering everywhere, with timings up to a third apart.

**Install footprint**

| What | Needs Node | On disk | Install time |
|---|---|---|---|
| esbuild, the standalone binary from the npm registry (`curl` and `tar`) | no | 10.9 MB | 0.17 s |
| Bun, the standalone binary from its release | no | 75.8 MB | 0.8 s |
| Vite (npm) | yes | 32.1 MB | 5.7 s |
| Vite with vite-plugin-solid (npm) | yes | 48.0 MB | 4.3 s |
| Babel with babel-preset-solid, and esbuild's npm package for its JS API (Solid under esbuild or Bun) | yes, or Bun | 23.9 MB | 2.4 s |
| TypeScript (npm) | yes | 29.0 MB | 1.5 s |
| The runtime packages: preact 1.5 MB, solid-js 1.0 MB, lit-html 1.6 MB | - | 6.6 MB | 1.4 s |

**Bundle time, minified IIFE (ms; warm is the mean ± σ of 10 runs after 2 warm-ups)**

| Target | esbuild cold | esbuild warm | Bun cold | Bun warm | Vite cold | Vite warm |
|---|---|---|---|---|---|---|
| legacy (today's 269 KB script) | 25 | 22 ± 0 | 18 | 15 ± 1 | 282 | 242 ± 3 |
| h | 9 | 6 ± 0 | 10 | 6 ± 0 | 203 | 204 ± 4 |
| preact | 11 | 8 ± 0 | 10 | 7 ± 1 | 207 | 206 ± 3 |
| solid | 437 | 425 ± 4 | 389 | 306 ± 7 | 672 | 565 ± 18 |
| lit | 11 | 7 ± 0 | 10 | 7 ± 1 | 204 | 208 ± 5 |

**Output size: minified / gzip -9 of minified / unminified.** Today's served script is 269.4 KB, 80.0 KB gzipped.

| Target | esbuild | Bun | Vite |
|---|---|---|---|
| legacy | 147.4 / 49.0 / 252.2 KB | 149.5 / 49.2 / 256.8 KB | 146.1 / 46.8 / 236.6 KB |
| h | 10.9 / 4.6 / 20.0 KB | 11.1 / 4.6 / 20.6 KB | 10.8 / 4.5 / 19.7 KB |
| preact | 23.3 / 9.4 / 38.9 KB | 23.5 / 9.5 / 39.5 KB | 23.0 / 9.2 / 36.1 KB |
| solid | 30.6 / 12.0 / 69.0 KB | 32.5 / 12.3 / 72.0 KB | 28.3 / 10.9 / 61.6 KB |
| lit | 20.7 / 8.6 / 34.7 KB | 20.6 / 8.4 / 33.6 KB | 20.1 / 8.2 / 32.3 KB |

**`viewer.rs`'s banned strings in each unminified bundle**, and each bundle's first character (unminified, minified). No bundle has a development build in it (lit-html's `DEV_MODE`, solid-js's `registerGraph`: 0 in all 15).

| Target | esbuild | Bun | Vite |
|---|---|---|---|
| legacy | none (`((`) | none (`((`) | none (`((`) |
| h | none (`""`) | none (`((`) | none (`((`) |
| preact | `innerHTML`×3, `cssText`×2 (`""`) | `innerHTML`×3, `cssText`×2 (`((`) | `innerHTML`×3, `cssText`×2 (`((`) |
| solid | `innerHTML`×1 (`""`) | `innerHTML`×1 (`((`) | `innerHTML`×1 (`((`) |
| lit | `innerHTML`×1 (`""`) | `innerHTML`×1 (`((`) | `innerHTML`×1 (`((`) |

**One file transformed without bundling (ms)**, the local check R-20260929-16 allows: `src/preact/SessionTree.tsx`, `src/solid/SessionTree.tsx` and `src/shared/tooltip.ts` each take 4 ± 0 with esbuild and 2 ± 0 with Bun. Vite has no standalone transform.

**`tsc --noEmit` per sample project (ms, mean of 3):** h 160, preact 165, solid 161, lit 142.

What the tables say:

- **Bundle time is not a factor.** esbuild and Bun bundle the sample in single-digit milliseconds and today's 269 KB script in 22 and 15 ms; Vite takes about 205 ms for the sample and 242 ms for the viewer, mostly Node starting and loading the config. Solid is the exception everywhere: its JSX compiles only through Babel, which adds 300 to 425 ms and Node (or Bun as the JS runtime) to every build.
- **Size.** The `h` row is the sample with no runtime, so each other row's excess over it is the runtime: Preact +12.4 / +4.8 KB, lit-html +9.8 / +4.0 KB, Solid +19.7 / +7.4 KB (esbuild, minified / gzip); unminified, as the bundle would be checked in, +18.9, +14.7 and +49.0 KB. The viewer is served uncompressed (no `Content-Encoding` in `viewer.rs`), so raw bytes are what the perf budget's `transferBytes` counts; today's script is 269 KB as served.
- **Every runtime trips the security test; the factory doesn't.** Preact's bundle has `innerHTML` three times (its `dangerouslySetInnerHTML` support) and `cssText` twice (string `style` props); Solid's has `innerHTML` once (`template()` parses each compiled template's static markup through a `<template>` element); lit-html's has it once (the same technique). None of these puts our data into HTML: the dynamic parts are text nodes and attributes. But the test in `viewer.rs` can no longer prove that by scanning the bundle, which is why it exists.
- **Output format.** esbuild prints `"use strict";` before its IIFE for the samples, whose tsconfig is `strict`; the `legacy` row, bundled without a tsconfig, starts with `(`, so the tsconfig is the likely cause (inference; a build tsconfig without `strict` is not tried here). Where the bundle would sit, after `tooltip.js`, the directive is an inert expression. Bun and Vite start with `(`. The bench gives Vite's library mode a `lib.name` for IIFE output; with nothing exported, no global is assigned.
- **One surprise.** Bun resolves packages' `development` export condition unless `NODE_ENV=production`: the first run bundled lit-html's and solid-js's development builds (22 and 4 development-only markers). The bench now sets it; the table's development-build check is 0 everywhere.

## Bundlers compared

None of the three type-checks. `tsc --noEmit` runs in CI whichever is picked (TypeScript 7: 142 to 165 ms for each sample project, above).

| | esbuild | Bun | Vite |
|---|---|---|---|
| **Install in CI** | One 10.9 MB binary, fetched from the npm registry with `curl` and `tar` in 0.17 s | One 75.8 MB binary, fetched from its release in 0.8 s | 32 MB of npm packages (Vite, Rolldown, Lightning CSS, PostCSS), 5.7 s with npm |
| **Without Node** | Yes, for `h`, Preact and lit-html. Solid needs Babel, so Node or Bun | Yes, and Bun can run Babel itself for Solid | No: Vite is a Node program (Node ≥ 20.19) |
| **Bundle time** | Sample 6 to 8 ms warm; the whole viewer 22 ms (25 ms cold) | Sample 6 to 7 ms; the whole viewer 15 ms (18 ms cold) | Sample about 205 ms; the whole viewer 242 ms |
| **Output size** | Within 3% of the smallest for every target but Solid (8%) | Largest for four of the five targets, up to 15% over Vite (Solid) | Smallest for every target (Rolldown's minifier is slightly tighter) |
| **Config** | Command-line flags; reads `jsx` settings from `tsconfig.json` | Command-line flags; reads `tsconfig.json`; `NODE_ENV=production` needed (above) | A config file (`lib` mode, `oxc.jsx`), plus `vite-plugin-solid` for Solid |
| **TSX** | Native for classic (`jsxFactory: h`) and automatic (`jsxImportSource: preact`) runtimes; Solid through a Babel plugin | Same | Native through Oxc; Solid through its plugin (Babel) |
| **Single-file transform, no bundling** | Yes: `esbuild file.tsx` without `--bundle`, 4 ms | Yes: `bun build --no-bundle file.tsx`, 2 ms | No standalone transform |
| **What we would use** | Bundling, TS and JSX stripping, `--metafile` for size reports; deterministic output for the freshness check | The same, and a JS runtime that could run Babel without Node | Bundling. Its main feature, the dev server with hot reload, is exactly what the machine rules exclude |
| **Lock-in and maintenance** | Low: flags in one script; the output is plain JS we check in. Still 0.x after years, and mostly one maintainer | Low for the build, but a large, fast-moving binary that is also a runtime, package manager and test runner we wouldn't use | Medium: a config file and a plugin ecosystem; four majors (5 to 8) since late 2023 |

## Runtimes compared

The requirements come from the live update, which is where the viewer's hard-won behaviour is: redraw only the changed turns and keep every other node (open steps, measured clamps, focus); anchor the scroll to a block's identity and offset across the redraw, in the same task; keep a list's order through an update and count what is held (#126); derive the masked-thinking line per turn; measure "Show more" after layout and before the scroll is placed; let the tooltip follow a replaced node. All four runtimes can commit DOM synchronously when driven from the top (Preact's `render()`, Solid's signal writes, lit-html's `render()`, the factory by construction), so `capture()` and `restore()` can wrap an update in all four. The differences are below.

| | `h()` factory, no runtime | Preact, vendored | Solid | lit-html |
|---|---|---|---|---|
| **What it is** | JSX compiled to calls that build real nodes (`createElement`, `setAttribute`, text nodes): today's `el()`, typed | A virtual DOM with keyed diffing, hooks and class components | JSX compiled to template cloning plus fine-grained reactive bindings; components run once | Tagged template literals; each render compares each binding with its last value |
| **Runtime size (sample, esbuild, min/gzip)** | 0 | +12.4 / +4.8 KB | +19.7 / +7.4 KB | +9.8 / +4.0 KB |
| **Banned-string test** | Passes unchanged | Fails: `innerHTML`×3, `cssText`×2 | Fails: `innerHTML`×1 | Fails: `innerHTML`×1 |
| **TSX and types** | TSX; tsc checks elements through `h.JSX` and every component's props | TSX; full JSX types | TSX (Solid's JSX types) | No JSX. Templates are strings to `tsc`; checking them needs a separate analyzer |
| **Patch only changed turns** | As today: rebuild the dirty turns, keep the others by key and signature (the bench's `patchList` is `placer` plus `morph`) | Keyed turns with `shouldComponentUpdate` on a turn key skip the rest; inside a changed turn, entries diff by key, so open steps keep their nodes | Natural: a store with `reconcile()` writes only changed fields, and only the bindings reading them run. But it replaces `dirtyTurns` and `morph` with reactivity, which is a rewrite of the update path, not a port | Re-renders the whole template; `repeat()` keeps keyed nodes, and `guard()` per turn can skip unchanged turns' bindings |
| **Scroll anchoring, focus** | Unchanged: `capture()` and `restore()` as they are | Works if updates go through the top-level `render()`; a component's own `setState` renders in a later microtask, after `restore()` has run | Works (updates are synchronous), but reactivity can run a binding outside the capture, so every write has to go through one batched update | Works; `render()` is synchronous |
| **Order holds (#126)** | A snapshot in the list's closure (the bench's `keepOrder`) | The snapshot in a ref, outside render's inputs | Needs `untrack()`: a memo that reads "is the list touched" would re-sort when the pointer leaves the list, which #126 forbids (the bench's `SessionTree.tsx` shows the trap) | A snapshot in the mount's closure |
| **Clamp measurement** | A ResizeObserver per brief, as today | `useLayoutEffect` with a ResizeObserver | `onMount` with a ResizeObserver | A `ref()` callback with a ResizeObserver |
| **Build path** | Any bundler, no Node | Any bundler, no Node (vendored source) | Babel on every build: Node, or Bun as the runtime | Any bundler, no Node |
| **Risk to "no behaviour change"** | Lowest: the same DOM calls in the same order, now typed and split into modules | Medium: diffing replaces `replaceWith`, so a node that used to be rebuilt now survives (usually better, but different, and the pixel and live checks judge it) | Highest: the update pipeline changes model | Medium: whole-template re-render with keyed repeats; templates untyped |

Notes on each:

- **The factory** is what the viewer already does, with JSX in place of nested `el()` calls and types on everything. It gives up reactivity: a component is a function that returns nodes, and an update rebuilds what changed and swaps it in by key, as `morph` and `morphTurns` do. That is the machinery `capture()` and `restore()` were built around, so porting onto it keeps the anchoring, focus and clamp rules intact. It rejects `style` and `title` props at runtime (the CSP has no `unsafe-inline`, and tips are `data-tip`), and it cannot produce HTML from a string, so the banned-string test keeps its meaning. It costs about 100 lines of our own code to own (`src/h/jsx.ts` in the bench).
- **Preact** is the familiar model and the smallest real runtime. Vendored (its ESM source checked in under the web source, pinned), it builds with esbuild alone. It fails the banned-string test only through `dangerouslySetInnerHTML` and string `style` props: the test would scan our modules' output instead of the whole bundle, and a source-level test would ban those two props. That is a change to a security test, which needs its own security review and Marvin's agreement.
- **Solid** fits live patching best on paper: fine-grained updates, no diffing, template cloning that is fast for a long transcript's first paint. In practice it needs Babel (and so Node or Bun) on every build and for the local syntax check to mean anything, its `template()` uses `innerHTML`, and it changes how updates flow, which is where the viewer's subtle behaviour lives. Rendering speed is not measured here (unverified either way).
- **lit-html** is small and needs no compiler, but it has no TSX: templates are strings that `tsc` doesn't check, which gives up much of the "types" goal. It uses `innerHTML` on a `<template>` like Solid.

## How it ships

- **Source** moves to `crates/semon-sessions/web/src/` (TypeScript and TSX, one module per component and one for the API types), with `web/tsconfig.json` and `web/build.sh`. The build script pins the bundler version, fetches the standalone binary from the npm registry into a cache, and bundles `web/src/main.ts` as one IIFE with no exports.
- **The bundle is checked in** at `crates/semon-sessions/src/viewer.gen.js`, unminified (readable diffs, and the banned-string test reads the real code), with a first line saying it is generated from `web/src` by `web/build.sh`. `shell::VIEWER_JS` becomes `concat!(tooltip.js, "\n", select.js, "\n", viewer.gen.js)`: that is the one path `include_str!` reads for the viewer's script. The path changes from `viewer.js` on purpose: if the bundle took the old path, git would see `viewer.js` rewritten instead of moved, and every open PR's edits to it would conflict with the whole file. With `viewer.js` moved unchanged to `web/src/viewer.legacy.js` and the bundle at a new path, git's rename detection carries open PRs' edits to the moved file when they rebase.
- **CI fails when the bundle is stale.** A `web` job, first in `ui.yml` and before the browser job, runs `tsc --noEmit`, bundles, and fails if the result differs from the checked-in file; it uploads the fresh bundle as an artifact. The browser job builds `semon` from the fresh bundle, so the Playwright checks always test the source. esbuild's output is deterministic for a pinned version, which the freshness check relies on. Bundling and type-checking take well under a second at the bench's sizes (esbuild 22 ms for the whole viewer); with the runner's setup, a type error should fail here in about a minute (an estimate) rather than twenty minutes into the UI run.
- **The Rust tests** follow the code. `harness.rs`'s registry mirror and `shell.rs`'s nav test read the source modules that hold `HARNESSES` and the nav (`include_str!` of the `.ts` or `.tsx` file), with their needles rewritten for the JSX form. `viewer.rs`'s banned-string and native-title tests stay on `VIEWER_JS`, the bytes that ship: with the factory they pass unchanged, with any runtime they need the change described above.
- **Unchanged for embedders:** the URL (`/viewer.js`), `VIEWER_JS` still starting with `tooltip.js` (so still with `(`), the prelude contract, `window.semonEmbed`, the three events, and no new globals (an IIFE with no name).
- **`cargo install`** reads checked-in files only, as today.

## The local loop

The machine rules settle what runs locally (R-20260929-16, Principal's reading, 2026-09-30):

- Allowed locally: a syntax check of one file that parses or transforms it without resolving imports or bundling, such as esbuild on a single `.tsx` with no `--bundle`.
- CI only, however fast: a bundle (`esbuild --bundle`, `bun build`), a project-wide `tsc --noEmit`, and any build of the deployable.
- Bundling locally would loosen CLAUDE.md's "no local compiling", which only Marvin's own words can change.

So the implementer's loop is: edit, run the single-file check on each changed file (esbuild: 4 ms per file measured on the runner; it catches syntax and JSX errors, not types or imports), push, and let CI bundle, type-check and run Playwright. When the `web` job says the bundle is stale, the implementer downloads the job's bundle artifact (`gh run download`, a file copy) and commits it; or a CI step commits it to the PR branch (a choice for Marvin, below). This is what makes a bundler's standalone single-file transform matter: esbuild and Bun have one and work without Node; Vite has none, and Solid under any bundler makes the check a Babel run.

**Option for Marvin: allow local bundling.** This is a CLAUDE.md change, in Marvin's words, not something this doc or a ruling can grant. What it buys: the implementer commits a fresh bundle without the artifact round trip, and import and resolution errors show before the push. esbuild bundles the whole viewer in 22 ms (25 ms cold) on the 4-vCPU runner. What it risks: esbuild parses on every core at once, and heat-gate saw 98 °C within 0.5 s on a short multi-core burst on the maintainer's machine. If allowed, it would run through `heat-gate`, niced, with `GOMAXPROCS=1` to keep it on one core (slower, still well under a second at this size: an estimate, not measured). `tsc` and Playwright stay in CI either way.

## Migration plan

1. **Now: #81 and #106 finish in plain JS**, on the current layout, as do #126 and #129. Their reviews and CI rounds are the costly part of the overhaul already; adding a toolchain under them would add a bundle step to every round. Nothing below starts until #81 and #106 are merged.
2. **The toolchain and the move, one PR, no behaviour change.** `viewer.js` moves to `web/src/viewer.legacy.js` byte for byte (a pure rename, so any PR still open rebases through it), `web/build.sh` bundles it as the entry, `viewer.gen.js` is checked in, `shell.rs` points at it, the two source-reading Rust tests point at the legacy file, and the `web` job lands. `tooltip.js` and `select.js` stay where they are, shared with `shell::JS`. Acceptance: the UI suite is green with no pixel-baseline change and the perf budget within noise. The bench's `legacy` row is this bundle already: today's code without its comments (252 KB instead of 269 KB), no banned string, and a leading `(`. This is the one mechanical move of existing code; everything after it moves code screen by screen.
3. **Types and the factory.** `web/src/api.ts` (the `/api/model` and `/api/tx` shapes), `web/src/jsx.ts` (the factory and `patchList`), and the icon table as a typed module. The legacy file imports what it needs from them. No behaviour change.
4. **Overhaul PR 4, the sidebar and rail, as components.** The sidebar is rewritten by the overhaul anyway, so writing it as `Sidebar.tsx` costs little beyond the port itself; the bench's session tree (with #126's order rule, which will be on main by then) is a first cut. Coexistence: the legacy file's `renderLanes()` calls the component's mount and update, and the component owns `#lanes`.
5. **PRs 5 to 8 the same way:** Home, Sessions and Machines (5), Trace with the new Timeline (6, greenfield: no port at all), Analytics with its charts as SVG components (7), and the empty, error and loading states (8). The overhaul's rule that a skeleton is the loaded screen's own components with placeholders (`overhaul.md`, Loading) falls out of components directly: the same component renders a placeholder model.
6. **After the overhaul, the session page**, the live-update core (`transcript`, `patchSession`, `morph`, `capture` and `restore`, errors mode, Find), ported with no visual change, one part per PR, each accepted on an unchanged pixel baseline and green live checks. Last, `tooltip.js` and `select.js` become TypeScript bundled into both `VIEWER_JS` and `shell::JS` (two entry points).

If PRs 4 to 8 start before step 2 lands, they continue in plain JS, and the port of their screens moves to step 6. With a runtime instead of the factory, the order is the same; the runtime is vendored in step 3.

## Risks and costs

- **The CI-only loop.** Every change to the viewer now needs CI to bundle it, and a stale bundle is one more way for a run to go red. Mitigation: the `web` job fails in about a minute, before the 18 to 20 minute browser job, and the browser job tests the fresh bundle regardless. Two PRs that both change the bundle conflict in a generated file; the fix is to rebase and take CI's bundle again.
- **The perf budget.** The factory adds nothing to the transfer. A runtime adds its size to every screen's `transferBytes` (served uncompressed) and its diffing or reactivity to every update's main-thread time. `session-older@390` already blocks for 5.6 s at 4× CPU throttling, and none of the runtimes' rendering speed is measured here. The perf check is report-only today, so a regression would show in the report, not fail the run.
- **The pixel baselines.** The move must change no pixel, and a port must reproduce the DOM exactly: classes, nesting, and text, including the thin spaces `spaced()` puts around middle dots. JSX drops whitespace between elements on separate lines and keeps it on one line; a stray space is a pixel diff.
- **The security test.** It keeps its meaning only with the factory. Any runtime means rewriting a test whose purpose is that no string reaches the DOM as HTML, which needs its own review and Marvin's agreement.
- **Behaviour drift during ports.** The live checks and the pixel comparison are the contract; a port PR that needs to move a baseline is not a port.
- **Maintenance.** esbuild is pinned and fetched as a binary, so an upgrade is a one-line change with a bundle diff to review. A vendored runtime is upgraded by hand. The factory is ours to maintain (about 100 lines).
- **Type drift.** `api.ts` is written by hand from the Rust structs and can drift from them; generating it is possible later.

## Recommendation

**Bundler: esbuild.** It is a single 10.9 MB binary that installs in a quarter of a second with no Node, has the standalone single-file transform the local check needs, compiles TSX for the factory and for Preact natively, and bundles the whole viewer in 22 ms (25 ms cold) with deterministic output for the freshness check. Configuration is a line of flags. **Runner-up: Bun**, equally fast and also standalone, but a 76 MB binary that is mostly a runtime we wouldn't use, and it bundled development builds until told otherwise. Vite brings Node and a config for a dev server we can't use.

**Runtime: the hand-written `h()` factory, no runtime.** It is the only option that keeps the banned-string test as it is, adds no bytes, builds with esbuild alone, and changes nothing about how the viewer updates: the port keeps `dirtyTurns`, `morph`, `capture` and `restore`, which is where the anchoring, focus, holds and clamp rules live and where a behaviour change would be most expensive to find with a 20-minute CI loop. What it gives up is reactivity, which the viewer doesn't use today. **Runner-up: Preact, vendored**, if Marvin wants a component model with state and diffing: esbuild-native, 5 KB gzipped, and the security test can be scoped to our code with a source ban on its two HTML and style props. Solid is the best fit for fine-grained live patching on paper, but Babel on every build, `innerHTML` in its core and a rewrite of the update path make it the costliest; lit-html gives up typed templates.

## Decisions for Marvin

1. The bundler: esbuild (recommended), Bun, or Vite.
2. The runtime: the `h()` factory (recommended), Preact vendored, Solid, or lit-html. Any choice but the factory also asks for a change to the banned-string test in `viewer.rs`.
3. Local bundling: keep the single-file syntax check as the only local step (recommended, as the rules stand), or allow bundling through `heat-gate` as a CLAUDE.md change in his words.
4. A stale bundle: the implementer commits CI's bundle artifact (recommended: nothing writes to branches but the people working on them), or a CI step commits it to the PR branch.
5. The checked-in bundle: unminified (recommended: readable diffs, and the tests read real code) or minified (about 105 KB (42%) smaller on the wire, since the viewer is served uncompressed).
