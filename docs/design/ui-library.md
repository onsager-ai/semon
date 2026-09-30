# The UI library: semon's `ui/` as components an embedding app imports

Status: proposed 2026-09-30, waiting for Marvin's approval. No code until he approves it. Marvin's answer in the Semon session, 2026-09-30T02:04:21Z: "Library plan + cheap fixes now (Recommended)". This amends [tsx.md](tsx.md): the same `ui/` directory, the same pinned esbuild, the same `h()` factory, no runtime library.

## Why

An audit on 2026-09-30 compared an embedding app's server-rendered pages with the viewer they sit beside. They drift:

- The top bar is 60 px tall on the embedding pages and 52 px in the viewer.
- The sidebar had no session list. #138 fixed that by loading the viewer's whole script in a sidebar-only mode.
- The account menu exists twice: once in the server's templates, once as the viewer's `accountPopover()`.
- There are two sheets: `shell.css`'s `dialog.sheet` and the viewer's `dialog.viewer`. Their padding, headers and phone behaviour differ; the viewer's kids, runs and analytics sheets push a history entry so Back closes them, the shell's sheet doesn't.
- Serif headings remain in `shell.css` (`.hero`, `.signin`) while the viewer drops its serif (#148).

The cause is the same each time. The shell shares CSS classes and a few Rust markup helpers, not components. Anything with behaviour (a menu, a sheet, a list that updates) the embedding app writes again, and the copy drifts. The library shares the components themselves.

This adds to the embedding contract. The contract for embedding the viewer page itself (the prelude, `window.semonEmbed`, the three events; `docs/shell.md`) stays as it is.

## What the library exports

- Library modules live in `ui/src/lib/`, with one entry, `ui/src/lib/index.ts`. The viewer imports from it like any other consumer. A module in `lib/` never imports from outside `lib/`; the `ui` job checks this.
- A component is an `h()` factory function (tsx.md): typed props in, DOM nodes out. A component that updates returns `{ el, update(props), destroy() }`.
- CSS stays in the crate as files and is served as today (`/viewer.css`, `/shell.css`); tsx.md keeps CSS out of components. Step (a) splits the files by owner.
- The Rust markup helpers for server-rendered pages stay (`sidebar_head`, `NavLink::html`, `session_sidebar`), held to the components by the check in [Rendering](#rendering).

### Components

| Export | Props | Replaces |
|---|---|---|
| `TopBar` | `{ title, lead: "menu" \| "back", actions?: Node[], account?: Account }` | The viewer's top bar code; an embedding page's own top bar. 52 px tall (`--topbar-h`) and a 32 px avatar at every width. |
| `Sidebar` | `{ brand, nav: NavItem[], current?, recent?: Node, foot?: Node }` | `renderNav`, `sidebar_head`'s markup, #138's sidebar-only mode |
| `RecentSessions` | `{ model: string, open(id): void }`, where `model` is a same-origin `/api/model` path | `renderLanes` and its helpers. It polls on its own; its tree, #126's order rule, "N updated" and the "All N" sheet stay inside it. |
| `AccountMenu` | `{ account: Account, compact: boolean, sections?: MenuSection[] }` | `accountWidget`, `accountPopover`, `toggleAccountMenu`; an embedding page's own menu |
| `Sheet` | `{ title, size: "sm" \| "md" \| "lg", body: Node, actions?: Node[], phone: "bottom" \| "full", onClose? }`, with `open()` and `close()` | The viewer's `dialog.viewer` code; `shell.css`'s `dialog.sheet` |
| `ListRow` | `{ dot?, name, meta?, trailing?, second?, href? \| onClick?, actions?: Node[] }` | The viewer's `.nrow` builders; the shell's `.rows .row` |
| `Button` | `{ label, variant: "default" \| "primary" \| "danger" \| "quiet", icon?, type?, onClick? }` | `.btn` markup |
| `Empty` | `{ title?, text, action?: Node }` | `.empty` and `.hero` markup |
| `Field` | `{ label, control: Node, hint?, error? }` | `.field` markup |
| `h`, `Fragment`, chrome icons | | The factory and the icons the chrome draws |

The Select and tooltips are shared already, through `select.js` and `tooltip.js` in both `VIEWER_JS` and `shell::JS`. They join `lib/` when tsx.md's step 6 makes them TypeScript; until then an embedding page gets them from `/shell.js`.

### Data contracts

Types live in `ui/src/lib/types.ts`.

- **`Account`** is exactly the `account` shape in `session-viewer.md`: `{ name, login, initials, avatar_href, workspaces: [{ name, role, current, switch_href }], links: [{ label, href, method, danger }] }`, with the same limits. `parseAccount(value): Account | null` is today's `accountOf`, exported. Every component that takes an account runs it at its boundary, so an embedding page's data meets the same rules as a server's.
- **`NavItem`** is `{ key, label, href, icon, count?, hot? }`. `NAV_ITEMS` is Rust's `shell::NAV` in TypeScript, with the viewer's paths; the existing Rust test holds the two together. A page that serves a destination elsewhere passes its own `href`, so today's `nav: { machines }` override becomes plain data.
- **`MenuSection`** is `{ heading?, rows: MenuRow[], phone?: boolean }`. A `MenuRow` is a link `{ kind: "link", label, href, danger? }`, a form `{ kind: "post", label, href, danger? }`, or a switch `{ kind: "check", label, checked, onChange }`. The Display section the account menu is gaining (Wide page, desktop only) is one such section with `phone: false`. The library has no special case for it.
- Text from props is set as text, never as HTML. Every `href` passes the same same-origin check (`safePath`) the viewer uses today.

### Tokens and CSS

The tokens are the custom properties in `viewer.css`'s `:root` block, in light and dark: colour, type sizes, space, radius, `--tap`, `--sans`, `--mono`, plus a new `--topbar-h: 52px`. The fonts come with them (`shell::FONT_FILES`). `docs/shell.md` lists the tokens next to its class list, and a Rust test fails if a listed token or class is not defined.

### Not exported

These are viewer-internal and may change in any PR without a changelog entry: the transcript, turns, steps and briefs; Trace and the Timeline; Analytics, its charts and the facet filters; the bodies of Home, Sessions and Machines; the live-update core (`dirtyTurns`, `morph`, `capture`, `restore`); routing and history; the `/api/model` and `/api/tx` types beyond the contracts above; and all CSS outside the tokens and chrome files.

## How an embedding app consumes it

- semon is a git submodule of the app. The app's submodule pin is the version. There is no npm package and no release.
- The app's page script imports from the submodule's entry only: `import { TopBar, Sidebar, AccountMenu } from "../semon/ui/src/lib/index.ts"`. Deep imports into `lib/` modules are not API.
- Its `tsconfig.json` extends `semon/ui/tsconfig.lib.json`, which holds the JSX factory settings and `strict`.
- It bundles with semon's pinned esbuild. It runs `semon/ui/esbuild.sh` for the binary and `semon/ui/build.sh --entry <its entry> --out <its bundle>`, so the flags are the same too. `ui/build.sh` gains those two arguments in step (a); semon's own build stays the default. There is one pin for both repositories: an esbuild upgrade in semon reaches the app at its next submodule bump, as a diff of its bundle.
- The app checks its bundle in with a freshness check, as semon does, or bundles in its deploy. esbuild needs no Node, so either works.
- Page data: the server writes the page's props as JSON in `<script type="application/json" id="page-data">`, escaping `<` as `<`, and the entry reads it with `JSON.parse(textContent)`. A data block is never executed, so `script-src 'self'` stands. Or the entry fetches its data from the app's own API.

### API stability

The public API is: the exports of `ui/src/lib/index.ts` and their types, the data contracts, the classes and tokens `docs/shell.md` lists, and the `semon_sessions::shell` constants and functions.

A change is breaking when it:

- removes or renames any of those;
- adds a required prop or field, or makes an optional one required;
- tightens a rule on data (a lower limit, a check that rejects what passed before);
- changes what an existing prop means;
- changes a component's markup where server-rendered markup must match it (the comparison check below fails).

Not breaking: new exports, new optional props, new tokens or classes, a change to the library's look (colour, spacing, motion: embedding pages are meant to follow it), and any change to viewer-internal code.

How the embedding app finds out:

- **`ui/CHANGELOG.md`.** Newest first, one entry per PR that touches the public API, headed by the PR number. A breaking entry starts with **Breaking** and says in a line or two what a consumer changes. The `ui` job fails a PR that changes `ui/src/lib/`, the tokens file or `docs/shell.md`'s class and token lists without changing the changelog (a `git diff --name-only` check). On a bump, the app reads `git diff <old>..<new> -- ui/CHANGELOG.md` in the submodule.
- **A type-checked consumer in semon.** `ui/tests/lib-contract.ts` uses every export the way the docs show it and is part of the `ui` job's `tsc --noEmit`. A PR that has to change it changed the API, and its changelog entry says **Breaking**; review holds the two together.
- **The app's own CI.** It type-checks its entry against the pinned submodule and bundles it. A bump that breaks the app fails in the bump PR, before anything deploys.

## Rendering

- **Pages that must work without JavaScript** (sign-in, terms and other documents, error pages) stay server-rendered and use only the library's CSS classes: `.signin`, `.field`, `.btn`, `.btn-row`, `.notice`, `.hero`, `.rows`, `.list`, `.empty` and the page column. They load no bundle. If such a page needs an account affordance, it uses the static `.account` row (the login and a Sign out form), not the menu.
- **Signed-in pages mount components.** They already need `/shell.js` for the drawer, so JavaScript is required there today. The server still writes the chrome's skeleton (the brand row, the nav rows, the top bar's frame) with the Rust helpers, so the first paint has the chrome, and the components replace it on mount, as the viewer's script replaces #138's nav rows today.

Two ways to keep server markup and components in sync:

1. **A class contract and a comparison check (proposed).** Each piece that exists in both forms has a Rust helper in `semon_sessions::shell` and a component, both documented in `docs/shell.md`. Each has a fixture of props in `ui/tests/fixtures/`. A Rust test renders the helper with the fixture into a checked-in `<name>.server.html` and fails if the file is stale, as #138 holds `shell-sidebar.html`. A UI check mounts the component with the same fixture and compares its DOM with that file, after normalising attribute order and whitespace between tags. The static primitives are checked the same way: the markup `docs/shell.md` documents against `Button`, `Field`, `ListRow` and `Empty`.
2. **Components emit static HTML for the server.** One implementation instead of two. But the server is Rust: it would need a JavaScript engine at request time, or a build step that renders components into Rust templates. tsx.md keeps Node out of the Rust build and rules out server-side rendering, and a string renderer would be the path from strings to HTML that `viewer.rs`'s banned-string test exists to forbid.

The trade-off: option 1 writes about ten small, mostly static pieces twice (brand row, nav row, top bar frame, sheet frame, list row, button, field, empty state, notice), held together by the check. Anything with behaviour exists only as a component. The check needs a browser, so it runs in the Viewer UI job and a drift fails there, about 20 minutes into CI, not in the one-minute `ui` job. Option 2 removes the duplication at the cost of a toolchain in the Rust build. Option 1 is the proposal.

## Migration order

Nothing here starts before tsx.md's step 3 (the factory and the API types) lands, and tsx.md's steps 1 to 3 wait for #81 and #106 as written there. One PR per step.

a. **Tokens and chrome CSS.** `viewer.css` splits by owner into `tokens.css` (fonts, `:root` tokens, dark theme), `chrome.css` (app, sidebar, top bar, nav, menus, account menu, sheet, buttons, rows, empty states, fields; `shell.css`'s components move in) and `viewer.css` (the rest, private). `VIEWER_CSS` and `CSS` concatenate them, so the served URLs stay. The top bar uses `--topbar-h`. `.hero` and `.signin` headings drop the serif if #148 has landed. `ui/src/lib/` starts with `index.ts` and the factory; `ui/build.sh` takes `--entry` and `--out`; the changelog, `lib-contract.ts` and the changelog check land. Acceptance: the viewer's pixel baseline is unchanged; the shell gallery's screenshots are unchanged apart from the heading font; `shell.rs`'s collision test covers the three files; the new token test passes.

b. **Account menu.** `AccountMenu`, `parseAccount` and `MenuSection` go into `lib/`. The viewer's `accountWidget`, `accountPopover`, `toggleAccountMenu` and `closeAccountMenu` are replaced by the component, and the viewer passes its Display section as a section. Acceptance: `embed.mjs` and the account menu's and Display section's checks pass unchanged; the pixel baseline is unchanged at 390 and 1280 px, light and dark; `lib-contract.ts` mounts the menu with a fixture account and one extra section.

c. **Sheet and dialog.** One `Sheet` component and one `dialog.sheet` rule set, with the viewer's look: its header and close button, three widths on desktop, a bottom sheet or full screen on phones, Back closes it on phones, and focus returns to the control that opened it. The viewer's seven dialogs move to it (the kids, runs, filters and analytics-slice sheets, session details, the image viewer and the tool viewer). `shell.js`'s `data-open` and `data-confirm` open the same markup, with the same history entry. `Button`, `Field`, `ListRow` and `Empty` land here, since the sheet's body and actions use them. If the diff is too large for one review, the viewer's dialogs move in a second PR. Acceptance: the viewer's pixel baseline is unchanged; the shell gallery's sheet screenshots change to the viewer's look and are reviewed at 390 and 1280 px, light and dark; the comparison check covers the sheet frame, button, field, list row and empty state; at 390 px, Back closes a server-rendered sheet.

d. **Top bar and sidebar.** Built on #138 and on tsx.md's step 4 (overhaul PR 4, the sidebar as components): `TopBar`, `Sidebar` and `RecentSessions` are exported. #138's sidebar-only mode stays until embedding pages have switched. Acceptance: `embedsidebar.mjs` passes against a page built from the components; the comparison check covers the brand row, the nav rows and the top bar's frame; the top bar is 52 px at 390 and 1280 px on the viewer and on the shell gallery; the viewer's pixel baseline is unchanged; the bundle size of a page built from the components is recorded (see [Risks](#risks)).

e. **The embedding app switches, page by page.** These are the app's own PRs, planned in its own repository. Per page, the page mounts the library's chrome, the app's own copies (menu, sheet, top bar CSS) are deleted, and the app's checks compare the page with the viewer. semon's part ends by removing the sidebar-only mode and any shell class nothing uses, each with a **Breaking** changelog entry.

## Public and private boundary

- semon is public. The library names nothing specific to an embedding app: no product names, paths, account kinds, plans or roles beyond the generic ones the account contract already has. Fixtures and docs use neutral names (`Devices`, `example.invalid`), as `docs/shell.md` does.
- Features only an embedding app has go through extension points: the account's `links`, extra `MenuSection`s, `TopBar`'s `actions`, `Sidebar`'s `foot`, and nav items with their own `href`. A feature that fits none of these gets a new, generic extension point in a semon PR that describes it without the app's feature, not a special case.
- The library fetches nothing but the same-origin model path `RecentSessions` is given. All other data comes in as props.
- An embedding app's CSS does not restyle library classes. If a page needs a different look, the library gains a variant, or the app styles its own content with its own classes.

## Risks

- **Bundle size on embedding pages.** A signed-in page gains a script: the factory, the chrome components and what they use. The bench's sample (a sidebar tree, the tooltip and a clamped brief) bundles to 20.0 KB unminified and 10.9 KB minified (tsx.md, Measurements); the chrome is likely the same order of size, an estimate, not measured. A page with #138's sidebar loads the whole 269 KB viewer script today, so the library should make those pages lighter. Step (d) measures it, and the app may minify its own bundle.
- **CSP.** The bundle is a same-origin file under `script-src 'self'`: no inline script, no `eval`, no `style` attributes (the factory rejects `style` props). Page data comes from a JSON data block or an API. The banned-string test proves the absence of HTML sinks only in bundles it scans, so the app should run the same test on its own bundle.
- **No-JS pages.** A no-JS page that copies a component's markup by hand drifts unseen unless the comparison check covers it. So no-JS pages use only the documented static primitives, and each primitive has a fixture.
- **One-way submodule coupling.** semon never builds or tests the embedding app. A change that passes semon's CI can break the app at its next bump: the changelog, `lib-contract.ts` and the app's type-check catch API breaks, and the app's own UI checks catch visual ones. The app cannot patch the library in place; a library fix is a semon PR and a bump, so a library bug holds the app's fix for one merge cycle. Small, frequent bumps keep that short.
- **Two forms of the static pieces.** The cost of option 1 in [Rendering](#rendering): about ten pieces written twice, and a drift fails only in the browser job.
- **Waiting.** Nothing starts before tsx.md's step 3, which waits for #81 and #106. Until then an embedding app fixes drift in its own CSS.
- **A second consumer.** tsx.md names "a second consumer of the viewer" as a reason to revisit splitting `ui/` into its own repository. The embedding app becomes one. Marvin chose to keep `ui/` in semon, and the submodule pin serves as the version, so no split is proposed.

## What needs Marvin's approval

1. The export list and the not-exported list above.
2. Consumption through the submodule with semon's own `ui/build.sh` and esbuild pin, and the changelog plus type-checked contract as the stability mechanism.
3. Option 1 in [Rendering](#rendering): Rust helpers for the static pieces, held to the components by a comparison check in the browser job.
4. The order (a) to (e), each step after tsx.md's step 3.
