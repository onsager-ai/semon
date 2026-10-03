# The UI library: semon's `ui/` as components an embedding app imports

## Current library decision — 2026-10-02

### Stage 4 panel pilot — 2026-10-03

Stage 3 is merged/adopted: Semon `26da6ef`, Hub `eb45207`, both Hub pins
`26da6ef`; all exact-main workflows pass. The next bounded source slice exports
`createPanelChrome`, `PanelOptions`, `PanelHost` and `PanelChrome`. Session menu,
full tool details and Analytics work items consume its Preact header/native dialog
lifecycle. `.panel-b` is host-owned; screen bodies, history, pending navigation,
focus return and live refresh stay outside the library. Synchronous show commits
focus before the opened callback; close unmounts/removes listeners before the
one-shot closed callback. Destroy is idempotent and stale controls are inert.
The independent consumer covers phone/desktop × light/dark, hostile strings,
native dismissal, scroll guards and cleanup. No shared screen/model global,
router or poller is introduced. Other sheets/screens remain future bounded work;
transcript/paging/live orchestration stays last. Hub pin and rollout are separate.

### Account and shell contract record

[#224](https://github.com/onsager-ai/semon/issues/224) selects native **Preact + TypeScript/TSX + esbuild**, superseding the custom `h()` factory in the historical plan below. See [tsx.md](tsx.md#current-decision--2026-10-02) for the current baseline, build, scheduling, security and measured bundle costs. Source stays in Semon's top-level `ui/`; Hub consumes a pinned submodule rather than maintaining a second framework implementation.

The source library exports `AccountMenu`, `AccountMenuProps`, `Account`,
`parseAccount`, `safePath`, `createAccountChrome`, `AccountChrome`,
`AccountChromeHost` and `AccountCloseOptions` from `ui/src/lib/index.ts`.
`AccountMenu` remains the popover-content component. `createAccountChrome` owns
phone/desktop triggers, avatar, menu/backdrop, open state, focus and dismissal
listener cleanup. Validate unknown API/prelude data with `parseAccount` first.
Account links/workspaces retain the existing same-origin POST/path contract.

The host supplies `place(widget, trigger)`, `opened(compact)`,
`closed({keepEntry?, navigating?})`, `navigate(href)` and `submit(form)`; the last
two return whether they consumed phone navigation. Display state is explicitly
supplied through AccountMenuProps; no viewer globals enter the library. One
controller owns both device roots, with synchronous `mount`, `updateWide`,
`close`, `escape`, `unmount`, `destroy` and readonly `open`. Forward Escape from
host overlay arbitration; unmount before removing a root, destroy on shell
teardown. The viewer retains history/polling transactions and legacy measured
CSSOM placement behind those callbacks; Preact owns root descendants completely.

`ui/src/account-adapter.tsx` is the viewer's library entry bridge. No shared lib
module imports viewer state. `ui/src/lib-contract.tsx` type-checks the exports and
an independent consumer's mount/destroy implementation; its real-browser test
checks root/listener cleanup. `ui/tsconfig.lib.json` provides strict native Preact
JSX settings; install the exact versions from `ui/package-lock.json`. One native
Preact instance serves each bundle; the runtime license stays in `ui/PREACT-LICENSE`.

### Stage 3 item 1 — shared shell frame/navigation

The viewer now consumes `createShellChrome(ShellHost)` through the library bridge.
The implemented exports and consumer contract are documented in [ui/README.md](../../ui/README.md).
Chrome owns the frame, brand/rail controls, keyed native navigation, lead button,
drawer/scrim gestures and account root replacement/cleanup. Native links carry
validated same-origin destinations; host navigation handles ordinary primary
clicks only. No router, poller or endpoint is introduced. Sidebar-only consumers
retain their Rust/server shell owner and share `renderShellNavigation`.

Host-owned title metadata/actions and Find/error modes enter as direct DOM slots;
the shell commits or detaches these nodes without rendering/removing their
children. Recent and page containers are disjoint from all Preact roots. Host
screen state, held ordering, authorization, history/focus/scroll and live model
transactions remain outside the library. Explicit renders preserve synchronous
commit semantics and keyed navigation; persistent action nodes remain attached.
Unmount releases roots/listeners while preserving host slot descendants; destroy
also disposes the reusable account controller. Account dismissal listeners are
registered only while roots exist. The pinned-runtime sink boundary and all
application bans remain unchanged; Rust also explicitly proves shell.tsx is scanned.

The independent consumer verifies repeated mount/update/unmount/destroy, native
modifier clicks, hostile labels/paths, stable focus and real listener removal on
phone/desktop in light/dark. This bounded source step is not Hub consolidation.
Next: shared Recent/sidebar snapshot rendering and consumer build contract, then
Hub adoption with equal pins and end-to-end persistent Sessions–Machines navigation.
Sheets/screens follow; transcript/paging/live orchestration remains last.

### Hub integration against the actual consumer

Hub main `149e382` pins Semon `acf79df` in `semon.rev` and the `semon` gitlink. Its viewer is served through `ViewerCore` (which embeds `shell::VIEWER_JS`); it supplies account JSON through `account::with_account`. Hub's own signed-in pages currently render separate account chrome in `crates/semon-hub/src/pages.rs` and load unchanged `shell::JS`. There is no Hub Preact build today. Its browser checks cover page/viewer account and drawer geometry plus one viewer load, while Semon owns the broader viewer suite.

After the account source stage is reviewed and green, a separate Hub bump must keep both pins equal and rerun Rust, PostgreSQL, browser and image/smoke workflows. That bump adopts the generated viewer automatically and needs no Node in its production image. To replace the shell page's own account menu, import the public library entry, extend `ui/tsconfig.lib.json`, use the pinned tooling and check in the Hub entry's generated asset with freshness CI. Preserve Hub's same-origin POST/CSRF checks, account avatar proxy, no-JS fallbacks, prelude/events and authenticated model handling. The consumer must share Semon's Preact instance within each bundle. No Hub pins, runtime deployment, pricing/filter/MCP work or shell page behavior change in the source pilot.

### Remaining API and migration plan

`TopBar`, `Sidebar`, `RecentSessions`, `pollRecentSessions`, `Sheet`, `enhanceSheet`, `sidebarOf`, `/api/sidebar`, extra `MenuSection`s, static-frame comparison fixtures and a Breaking changelog gate remain planned work. The original export table below is a target API, not an implemented contract. CSS stays in existing crate files for this pilot; splitting it or changing shell geometry would enlarge the refactor unnecessarily.

Proceed from account trigger/lifecycle ownership to shared shell/navigation and Hub adoption, then sheets/screens, and transcript/paging/live orchestration last. Every stage preserves current URLs, embedding contract, CSP, focus, scroll, ordering, expanded state and unchanged visual gates. Server/component parity should use actual Preact in a browser or an established DOM implementation; the earlier tiny fake DOM sufficient for `h()` is not a Preact verification strategy. Measure equivalent production builds and consumer request budgets at each adoption.

### Unified shell follow-up contract (proposed, not yet exported)

The next bounded source PR should establish one typed shell host with authenticated
account/workspace inputs, safe navigation destinations and active route, Recent
session data from the existing host poller, and a content slot for viewer or Hub
administrative content. Shared components own topbar/sidebar/drawer chrome; the
host owns content rendering, authorization, URL/history and saved focus/scroll.
The content slot must be outside chrome-owned roots, with explicit unmount and
commit transactions when its host changes. Semon source remains shared by both
applications rather than introducing a Hub framework copy.

Hub supplies Machines/account/workspace destinations. Evaluate persistent shell
navigation across viewer/Hub routes while preserving native links and server
fallbacks for reload, absent JS and authorization errors. Test Sessions → Machines
→ Sessions on phone/desktop and light/dark, correct active navigation, Recent,
account/drawer spacing, Back/Forward, workspace changes and no duplicate poller.
Account-only adoption is not the consolidated-experience milestone. Necessary
visual unification requires explicit reviewed comparison evidence; refactor
references are not silently refreshed. This is the next contract to implement,
not an assertion that the shell or persistent routing already exists.

## Historical library plan — 2026-09-30

The approved original plan is preserved below as a comparison and decision record. Its helper/installer references, obsolete overhaul dependencies, estimates and unimplemented API descriptions are superseded by the current stages above. The current pilot intentionally extracts content first, keeping existing history/focus behavior in its legacy controller until the next bounded stage.


Status: decided 2026-09-30; see [Decided](#decided). Code follows the [migration order](#migration-order). Marvin asked for the plan in the Semon session, 2026-09-30T02:04:21Z: "Library plan + cheap fixes now (Recommended)", and approved it there at 2026-09-30T02:27:47Z. This amends [tsx.md](tsx.md): the same `ui/` directory, the same pinned esbuild, the same `h()` factory, no runtime library.

## Why

An audit on 2026-09-30 compared an embedding app's server-rendered pages with the viewer they sit beside. They drift:

- The top bar is 60 px tall on the embedding pages and 52 px in the viewer.
- The sidebar had no session list. #138 fixed that by loading the viewer's whole script in a sidebar-only mode.
- The account menu exists twice: once in the server's templates, once as the viewer's `accountPopover()`.
- There are two sheets: `shell.css`'s `dialog.sheet` and the viewer's `dialog.viewer`. Their padding, headers and phone behaviour differ; the viewer's kids, runs and analytics sheets push a history entry so Back closes them, the shell's sheet doesn't.
- Serif headings remain in `shell.css` (`.hero`, `.signin`) while the viewer drops its serif (#148).

The cause is the same each time. The shell shares CSS classes and a few Rust markup helpers, not components. Anything with behaviour (a menu, a sheet, a list that updates) the embedding app writes again, and the copy drifts. The library shares the components and their behaviour.

This adds to the embedding contract. The contract for embedding the viewer page itself (the prelude, `window.semonEmbed`, the three events; `docs/shell.md`) stays as it is.

## What the library exports

- Library modules live in `ui/src/lib/`, with one entry, `ui/src/lib/index.ts`. The viewer imports from it like any other consumer. A module in `lib/` never imports from outside `lib/`; the `ui` job checks this.
- A component is an `h()` factory function (tsx.md): typed props in, DOM nodes out. A component that updates returns `{ el, update(props), destroy() }`.
- CSS stays in the crate as files and is served as today (`/viewer.css`, `/shell.css`); tsx.md keeps CSS out of components. Step (a) splits the files by owner.
- The first cut exports the page chrome only. The primitives (buttons, form fields, list rows, empty states) are exported as documented CSS classes, not components.

### Components

| Export | Props | Replaces |
|---|---|---|
| `TopBar` | `{ title, lead: "menu" \| "back", actions?: Node[], account?: Account }` | The viewer's top bar code; an embedding page's own top bar. 52 px tall (`--topbar-h`) and a 32 px avatar at every width. |
| `Sidebar` | `{ brand, nav: NavItem[], current?, recent?: Node, foot?: Node }` | `renderNav`, `sidebar_head`'s markup, #138's sidebar-only mode. No search field: the sidebar has none (see [Decided](#decided)). |
| `RecentSessions` | `{ open(id): void, history: boolean }`; returns `{ el, update(sidebar: SidebarData), resort(), stopped(), destroy() }` | `renderLanes` and its helpers. Its tree, #126's order rule, "N updated" and the "All N" sheet stay inside it. |
| `pollRecentSessions` | `(list, { endpoint: string })`, where `endpoint` is the same-origin `/api/sidebar` path ([step d](#migration-order)) | #138's polling of the full `/api/model` in sidebar-only mode: the viewer's backoff, `semon:ended`, and `list.stopped()` on a 403 |
| `AccountMenu` | `{ account: Account, compact: boolean, sections?: MenuSection[] }` | `accountWidget`, `accountPopover`, `toggleAccountMenu`; an embedding page's own menu |
| `Sheet` | `{ title, size: "sm" \| "md" \| "lg", body: Node, actions?: Node[], phone: "bottom" \| "full", onClose? }`, with `open()` and `close()`; and `enhanceSheet(dialog)` for a server-rendered `dialog.sheet` | The viewer's `dialog.viewer` code; `shell.css`'s `dialog.sheet` and `shell.js`'s sheet code |
| `sidebarOf` | `(model) => SidebarData` | The same projection `/api/sidebar` makes on the server, for the viewer's pages |
| `h`, `Fragment`, chrome icons | | The factory and the icons the chrome draws |

How the sidebar is fed:

- **On the viewer's pages** the viewer already polls `/api/model`. It calls `list.update(sidebarOf(model))` from its own update, so there is no second poller and no second fetch. `open` is the viewer's router, and `history: true` gives the "All N" sheet its history entry.
- **On an embedding page** the page calls `pollRecentSessions(list, { endpoint: "/api/sidebar" })`, which fetches only what the list needs, not the full model. `open` is `(id) => location.assign(path)`, a page load, not a history entry, and `history: false` leaves the history alone, as #138 does. On a refused poll, `pollRecentSessions` fires `semon:ended` and, unless it is cancelled, calls `list.stopped()`, which puts "Sessions stopped updating: reload the page" under the list. On `semon:drawer-open` the page calls `list.resort()`, as the viewer's drawer does.

Button, Field, ListRow and Empty stay viewer-internal. An embedding page draws them with the classes `docs/shell.md` documents (`.btn`, `.field`, `.list` with `.nrow`, `.rows .row`, `.empty`, `.hero`). The Select and tooltips are shared already, through `select.js` and `tooltip.js` in both `VIEWER_JS` and `shell::JS`. They join `lib/` when tsx.md's step 6 makes them TypeScript.

### Data contracts

Types live in `ui/src/lib/types.ts`.

- **`Account`** is exactly the `account` shape in `session-viewer.md`: `{ name, login, initials, avatar_href, workspaces: [{ name, role, current, switch_href }], links: [{ label, href, method, danger }] }`, with the same limits. `parseAccount(value): Account | null` is today's `accountOf`, exported. Every component that takes an account runs it at its boundary, so an embedding page's data meets the same rules as a server's. The Rust side validates the same shape in `AccountMenu::new`. Shared fixtures in `ui/tests/fixtures/account/` (valid, and each limit broken once) are checked by both: a Rust test runs them through `AccountMenu::new`, and the `ui` job runs them through `parseAccount`, and both must accept and reject the same files.
- **`NavItem`** is `{ key, label, href, icon, count?, hot? }`. `NAV_ITEMS` is Rust's `shell::NAV` in TypeScript, with the viewer's paths; the existing Rust test holds the two together. A page that serves a destination elsewhere passes its own `href`, so today's `nav: { machines }` override becomes plain data.
- **`SidebarData`** is `{ now, sessions: [{ id, title, harness, state, last, parent }], nav: { waiting, offline } }`: each session the Recent list can draw (children included, for the tree and the "All N" sheet) with its title, harness, state, last activity and parent id, and the two counts the nav's badges show (Home's sessions waiting on you, Machines offline). It is what `/api/sidebar` answers and what `sidebarOf(model)` returns.
- **`MenuSection`** is `{ heading?, rows: MenuRow[], phone?: boolean }`. A `MenuRow` is a link `{ kind: "link", label, href, danger? }`, a form `{ kind: "post", label, href, danger? }`, or a switch `{ kind: "check", label, checked, onChange }`. The Display section with the "Wide reading mode" switch (#163, open: desktop only, the phone drawer omits it) is one such section with `phone: false`. The library has no special case for it.
- Text from props is set as text, never as HTML. Every `href` passes the same same-origin check (`safePath`) the viewer uses today.

### Tokens and CSS

The tokens are the custom properties in `viewer.css`'s `:root` block, in light and dark: colour, type sizes, space, radius, `--tap`, `--sans`, `--mono`, plus a new `--topbar-h: 52px`. The fonts come with them (`shell::FONT_FILES`). `docs/shell.md` lists the tokens next to its class list, and a Rust test fails if a listed token or class is not defined.

### Not exported

These are viewer-internal and may change in any PR without a changelog entry: the primitives' components (Button, Field, ListRow, Empty; their classes are public); the transcript, turns, steps and briefs; Trace and the Timeline; Analytics, its charts and the facet filters; the bodies of Home, Sessions and Machines; the live-update core (`dirtyTurns`, `morph`, `capture`, `restore`); routing and history; the `/api/model` and `/api/tx` types beyond the contracts above; and all CSS outside the tokens and chrome files.

## How an embedding app consumes it

- semon is a git submodule of the app, pinned at a commit. That pin is the version. There is no npm package and no release.
- The app's page script imports from the submodule's entry only: `import { TopBar, Sidebar, AccountMenu } from "../semon/ui/src/lib/index.ts"`. Deep imports into `lib/` modules are not API.
- Its `tsconfig.json` extends `semon/ui/tsconfig.lib.json`, which holds the JSX factory settings and `strict`.
- It bundles with semon's pinned esbuild. It runs `semon/ui/esbuild.sh` for the binary and `semon/ui/build.sh --entry <its entry> --out <its bundle>`, so the flags are the same too. `ui/build.sh` gains those two arguments in step (a); semon's own build stays the default. There is one esbuild pin for both repositories: an upgrade in semon reaches the app at its next bump, as a diff of its bundle.
- The app checks its bundle in, with a freshness check in its CI, as semon does. Its production image build does not bundle, so it fetches nothing from `curl` or a package registry.
- Page data: the server writes the page's props as JSON in `<script type="application/json" id="page-data">` and the entry reads it with `JSON.parse(textContent)`. A data block is never executed, so `script-src 'self'` stands. The server writes every `<` in the JSON as `\u003c`, so a string holding `</script>` cannot end the block early; the app's Rust side has a test that a value holding `</script>` round-trips through the block unchanged. Or the entry fetches its data from the app's own API.

### API stability

The public API is: the exports of `ui/src/lib/index.ts` and their types, the data contracts, the classes and tokens `docs/shell.md` lists, and the `pub` items of `semon_sessions::shell`.

A change is breaking when it:

- removes or renames any of those;
- adds a required prop or field, or makes an optional one required;
- tightens a rule on data (a lower limit, a check that rejects what passed before);
- changes what an existing prop means;
- changes a component's markup where server-rendered markup must match it (the comparison checks below fail).

Not breaking: new exports, new optional props, new tokens or classes, a change to the library's look (colour, spacing, motion: embedding pages are meant to follow it), and any change to viewer-internal code.

How a break is caught:

- **The app, on every bump.** It pins a semon commit and its CI already keeps its pins equal. On every bump it type-checks its entry against the new submodule, bundles, and runs its UI checks, so a bump that breaks the app fails in the bump PR, before anything deploys. The type-check uses the TypeScript version semon pins in `ui/package.json` and its lockfile (the bench ran 7.0.2); the app installs from that lockfile, so both repositories check with the same compiler.
- **semon, on a contract change.** `ui/tests/lib-contract.ts` uses every export the way the docs show it and is part of the `ui` job's `tsc --noEmit`. A PR that changes `lib-contract.ts`, `docs/shell.md`'s class or token lists, or a `pub` item of `shell.rs` must add a **Breaking** entry to `ui/CHANGELOG.md` saying what a consumer changes; the `ui` job checks this with `git diff --name-only`, plus a diff of `shell.rs`'s `pub` lines. Other changes to `lib/` need no entry. On a bump, the app reads `git diff <old>..<new> -- ui/CHANGELOG.md` in the submodule.

## Rendering

- **Pages that must work without JavaScript** (sign-in, terms and other documents, error pages) stay server-rendered and use only the library's CSS classes: `.signin`, `.field`, `.btn`, `.btn-row`, `.notice`, `.hero`, `.rows`, `.list`, `.empty` and the page column. They load no bundle. If such a page needs an account affordance, it uses the static `.account` row (the login and a Sign out form), not the menu.
- **Signed-in pages mount components.** They already need `/shell.js` for the drawer, so JavaScript is required there today. An account menu that works without script (a native `popover`, as an embedding page may draw today) is given up; that is accepted, for the same reason. The server still writes the chrome's skeleton: the brand row, the nav rows, and the top bar's frame with its avatar trigger. The first paint has the chrome, and the components take it over on mount without moving it.

Two ways to keep server markup and components in sync:

1. **Rust helpers and comparison checks (chosen).** Four pieces exist in both forms: the brand row (`shell::sidebar_head`, existing), the nav row (`NavLink::html`, existing), the top bar's frame and the sheet's frame. The last two get new public helpers, `shell::topbar` and `shell::sheet`. An embedding app switches its hand-written markup to these helpers; otherwise the check covers code nobody calls. Each piece has a fixture of props in `ui/tests/fixtures/`. A Rust test renders the helper with the fixture into a checked-in `<name>.server.html` and fails if the file is stale, as #138 holds `shell-sidebar.html`. Two checks compare the component with that file after normalising attribute order and whitespace between tags:
   - **Fast, in the `ui` job.** A fake DOM of about 50 lines (`createElement`, `createElementNS`, `setAttribute`, text nodes, `append`, and a serializer) is enough for `h()`, which uses nothing else. esbuild bundles `ui/tests/server-markup.test.ts` with it and Node runs it. The `ui` job has Node already, for `tsc` from npm, so a drift fails in about a minute.
   - **Backstop, in the Viewer UI job.** The same comparison in a real browser, with the real CSS, about 20 minutes into CI.
2. **Components emit static HTML for the server.** One implementation instead of two. But the server is Rust: it would need a JavaScript engine at request time, or a build step that renders components into Rust templates. tsx.md keeps Node out of the Rust build and rules out server-side rendering, and a string renderer would be the path from strings to HTML that `viewer.rs`'s banned-string test exists to forbid.

The trade-off: option 1 writes four small static frames twice, held together by the fast check. Option 2 removes that duplication at the cost of a toolchain in the Rust build. Option 1 is the plan.

Behaviour is never duplicated. The sheet's behaviour (the history entry, Back, focus return) lives in `lib/sheet.ts`. `Sheet` uses it, and so does `shell.js`: from step (c), `shell::JS` is built by esbuild from a second entry, `ui/src/shell.ts`, whose `data-open` and `data-confirm` call `enhanceSheet` on the server's `dialog.sheet`. This brings forward tsx.md's step 6 for `shell.js` alone. The phone drawer's code, which is in both `viewer.js` and `shell.js` today, moves into `lib/` the same way in step (e).

## Migration order

Nothing here starts before tsx.md's step 3 (the factory and the API types) lands, and tsx.md's steps 1 to 3 wait for #81 and #106 as written there. One PR per step. The embedding app adopts each piece as soon as its step lands, not all at once at the end.

a. **Tokens and chrome CSS.** `viewer.css` splits by owner into `tokens.css` (fonts, `:root` tokens, dark theme), `chrome.css` (app, sidebar, top bar, nav, menus, account menu, sheet, buttons, rows, empty states, fields; `shell.css`'s components move in) and `viewer.css` (the rest, private). `VIEWER_CSS` and `CSS` concatenate them, so the served URLs stay. The top bar uses `--topbar-h`. `.hero` and `.signin` headings drop the serif if #148 has landed. `ui/src/lib/` starts with `index.ts` and the factory; `ui/build.sh` takes `--entry` and `--out`; `lib-contract.ts`, the changelog and the **Breaking** check land. Acceptance: the viewer's pixel baseline is unchanged; the shell gallery's screenshots are unchanged apart from the heading font; `shell.rs`'s collision test covers the three files; the new token test passes.

b. **Account menu.** `AccountMenu`, `parseAccount` and `MenuSection` go into `lib/`, with the shared account fixtures. The viewer's `accountWidget`, `accountPopover`, `toggleAccountMenu` and `closeAccountMenu` are replaced by the component, and the viewer passes its Display section (#163) as a section. Acceptance:
   - `embed.mjs`, the account menu's checks and #163's Display checks pass unchanged; the pixel baseline is unchanged at 390 and 1280 px, light and dark.
   - Accessibility: the trigger's `aria-expanded` follows the menu; the menu is `role="menu"` with `menuitem` rows and a `menuitemcheckbox` switch; Esc closes it and returns focus to the trigger; on a phone it opens above its row inside the drawer, and Back closes it.
   - First paint: on a page whose skeleton has the avatar trigger, mounting the menu moves no element of the top bar or the drawer's foot (bounding boxes compared before and after), and focus on the skeleton's trigger stays on the mounted one.
   - `lib-contract.ts` mounts the menu with a fixture account and one extra section; both validators agree on the account fixtures.

   The embedding app then replaces its own menu with `AccountMenu`.

c. **Sheet and dialog.** One `Sheet` component, `lib/sheet.ts` for its behaviour, and one `dialog.sheet` rule set with the viewer's look: its header and close button, three widths on desktop, a bottom sheet or full screen on phones, Back closes it on phones, and focus returns to the control that opened it. The viewer's seven dialogs move to it (the kids, runs, filters and analytics-slice sheets, session details, the image viewer and the tool viewer). `shell.js` becomes the bundle of `ui/src/shell.ts` and opens server-rendered sheets through `enhanceSheet`. `shell::sheet` lands. If the diff is too large for one review, the viewer's dialogs move in a second PR. Acceptance: the viewer's pixel baseline is unchanged; the shell gallery's sheet screenshots change to the viewer's look and are reviewed at 390 and 1280 px, light and dark; both comparison checks cover the sheet frame; at 390 px, Back closes a server-rendered sheet and focus returns to its opener. The embedding app then switches its sheets to `shell::sheet`.

d. **A sidebar endpoint.** Before the sidebar is exported, `ViewerCore` gains `/api/sidebar`, answering `SidebarData` and nothing else, and `sidebarOf` lands in `lib/`. It sits behind the same access checks as `/api/model`: an embedding server routes it through the same gate, so it shows a reader exactly what the viewer's sidebar would. It answers 304 when the model's version is unchanged, as `/api/model` does, and caps each title at 120 characters (the sidebar truncates it with an ellipsis anyway). Acceptance:
   - A shared fixture holds both projections together: for the primary UI fixture's model, the endpoint's body (a Rust test) and `sidebarOf(model)` (the `ui` job) both equal the checked-in `ui/tests/fixtures/sidebar.json`.
   - Budget: a Rust test builds a 400-session model and fails if the endpoint's body passes 64 KB uncompressed. The full model was reported at about 1.3 MB before compression in the Semon session on 2026-09-30.
   - The endpoint returns 304 on an unchanged version, and the same status as `/api/model` for a reader it refuses.
   - Nothing uses it yet on the viewer's own pages; `RecentSessions` on an embedding page switches to it in (e).

e. **Top bar and sidebar.** Built on #138 and on tsx.md's step 4 (overhaul PR 4, the sidebar as components): `TopBar`, `Sidebar`, `RecentSessions` and `pollRecentSessions` are exported, `shell::topbar` lands, and the drawer's code moves into `lib/`. The viewer feeds `RecentSessions` through `update(sidebarOf(model))`; an embedding page polls `/api/sidebar` from (d). #138's sidebar-only mode stays until embedding pages have switched. Acceptance:
   - Everything `embedsidebar.mjs` checks today passes against a page built from the components instead of the sidebar-only mode: rows and badges equal the viewer's, a live poll redraws without moving a row, a row opens the viewer's page by a page load with the address and history otherwise untouched, the "All N" sheet adds no history entry, a refused poll shows "Sessions stopped updating: reload the page", and opening the drawer re-sorts what #126 held.
   - The viewer's page has one `/api/model` poller, not two, and fetches nothing from `/api/sidebar`; an embedding page fetches `/api/sidebar` and never `/api/model`.
   - Both comparison checks cover the brand row, the nav rows and the top bar's frame; the top bar is 52 px at 390 and 1280 px on the viewer and on the shell gallery; the viewer's pixel baseline is unchanged.
   - First paint: mounting `TopBar` and `Sidebar` over the skeleton moves no element (bounding boxes compared), and focus on a skeleton control stays on its mounted counterpart.
   - The bundle size of a page built from the components, and the size of its `/api/sidebar` answer, are recorded (see [Risks](#risks)).

f. **The embedding app finishes switching, page by page.** These are the app's own PRs, planned in its own repository. Per page, the page mounts the library's chrome, the app's own copies (top bar CSS, sidebar markup) are deleted, and the app's checks compare the page with the viewer. semon's part ends by removing the sidebar-only mode and any shell class nothing uses, each with a **Breaking** changelog entry.

## Public and private boundary

- semon is public. The library names nothing specific to an embedding app: no product names, paths, account kinds, plans or roles beyond the generic ones the account contract already has. Fixtures and docs use neutral names (`Devices`, `example.invalid`), as `docs/shell.md` does.
- Features only an embedding app has go through extension points: the account's `links`, extra `MenuSection`s, `TopBar`'s `actions`, `Sidebar`'s `foot`, and nav items with their own `href`. A feature that fits none of these gets a new, generic extension point in a semon PR that describes it without the app's feature, not a special case.
- The library fetches nothing but the same-origin sidebar path `pollRecentSessions` is given. All other data comes in as props.
- An embedding app's CSS does not restyle library classes. If a page needs a different look, the library gains a variant, or the app styles its own content with its own classes.

## Risks

- **Data on every embedding page.** The sidebar is on every signed-in page, and fetching the full model there (about 1.3 MB before compression) would be the real cost on a phone, more than the script. Step (d)'s `/api/sidebar` answers that, under a 64 KB budget. Its cost is a second API shape to keep in step with the model; the shared fixture holds the server's projection and `sidebarOf` together.
- **Bundle size on embedding pages.** A signed-in page gains a script: the factory, the chrome components and what they use. The bench's sample (a sidebar tree, the tooltip and a clamped brief) bundles to 20.0 KB unminified and 10.9 KB minified (tsx.md, Measurements); the chrome is likely the same order of size, an estimate, not measured. A page with #138's sidebar loads the whole 269 KB viewer script today, so the library should make those pages lighter.
- **CSP.** The bundle is a same-origin file under `script-src 'self'`: no inline script, no `eval`, no `style` attributes (the factory rejects `style` props). Page data comes from a JSON data block, escaped as above, or an API. The banned-string test proves the absence of HTML sinks only in bundles it scans, so the app should run the same test on its own bundle.
- **No-JS pages.** A no-JS page that copies markup by hand drifts unseen unless a comparison check covers it. So no-JS pages use only the documented classes, and the four frames come from the Rust helpers.
- **One-way submodule coupling.** semon never builds or tests the embedding app. A change that passes semon's CI can break the app at its next bump; the app's type-check, bundle and UI checks on every bump catch it there, and the **Breaking** entries say what to change. The app cannot patch the library in place; a library fix is a semon PR and a bump, so a library bug holds the app's fix for one merge cycle. Small, frequent bumps keep that short.
- **Waiting.** Nothing starts before tsx.md's step 3, which waits for #81 and #106. Until then an embedding app fixes drift in its own CSS.
- **A second consumer.** tsx.md names "a second consumer of the viewer" as a reason to revisit splitting `ui/` into its own repository. The embedding app becomes one. Marvin chose to keep `ui/` in semon, and the submodule pin serves as the version, so no split is proposed.

## Decided

Marvin's answers in the Semon session, 2026-09-30T02:27:47Z: "Approve all three (Recommended)" for 1 to 3, and "Lighter endpoint first (Recommended)" for 4.

1. **The first cut exports the chrome only:** `TopBar`, `Sidebar`, `RecentSessions` with `pollRecentSessions`, `AccountMenu`, `Sheet` with `enhanceSheet`, `sidebarOf`, the factory and icons. The primitives (buttons, fields, list rows, empty states) stay public as CSS classes, so four frames are written twice (Rust and TSX). Not chosen: primitives as components too, which would double the public API and write about nine pieces twice.
2. **API breaks are caught by pin and type-check:** the app type-checks, bundles and runs its UI checks on every bump, and semon requires a **Breaking** changelog entry only when `lib-contract.ts`, `docs/shell.md`'s lists or `shell.rs`'s `pub` items change. Not chosen: a changelog gate on every `lib/` change.
3. **The app adopts piece by piece:** `AccountMenu` right after (b), sheets right after (c), the top bar and sidebar after (e). Not chosen: one switch at the end, which would keep the menu and sheet drift longer.
4. **A lighter endpoint first:** `/api/sidebar` (step d) lands before the sidebar is exported, and embedding pages never fetch the full model for it. Not chosen: the full `/api/model` on every embedding page.

5. **No search in the sidebar.** Marvin, in the Semon session, 2026-09-30: "search not needed in sidebar, remove". The viewer's sidebar and `session_sidebar` lose the search field in their own PR, so `Sidebar` takes no search and (e)'s acceptance has none. The Sessions page keeps its own search.

Also part of the plan, with no alternative proposed: Rust helpers plus comparison checks for the four frames ([Rendering](#rendering), option 1), and a checked-in bundle in the app, not a bundle made during its production build.

The implemented Recent boundary is `createRecentRenderer(root, host)`, with
`RecentSnapshot` and recursive `RecentItem` presentation data. The host supplies
ordered rows, selected/ancestor status, open groups, metadata and View all state;
callbacks report open, toggle, all and fewer actions. The renderer owns its keyed
DOM descendants and metadata fitting. `update` commits synchronously; `destroy`
unmounts the root, cancels measurements and removes its resize listener. Hosts
retain polling, held-order logic, focus restoration, scroll and sheet decisions.
It does not fetch data or introduce another sidebar endpoint.

External typed consumers build with Semon's pinned installation:
`node semon/ui/build.mjs --entry ui/shell.ts --output ui/shell.generated.js`.
Add `--check` in CI to reject stale assets. The command typechecks the consumer,
checks source boundaries, resolves one pinned Preact runtime and emits a
standalone browser bundle. The library imports no viewer, router or poller.
