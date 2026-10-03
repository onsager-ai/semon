# Shared viewer UI

Native Preact/TSX sources and deterministic production esbuild bundles. Rust embeds
`crates/semon-sessions/src/viewer.generated.js`, which includes the shared runtime,
Select and tooltip prefix. Cargo builds and installations need no JavaScript tooling.

```sh
npm --prefix ui ci --no-audit --no-fund
npm --prefix ui run typecheck
npm --prefix ui run build        # deliberate checked-in asset update
npm --prefix ui test
npm --prefix ui run check:bundle # fails on stale assets; never rewrites them
npm --prefix ui run sizes
```

Use Node 22 or newer. Exact packages and integrity hashes are in package-lock.json.
TypeScript 6.0.3 includes the compiler AST API used by security-check.mjs. The
Viewer UI required aggregate gates type-check/security/freshness before compiling
its Rust fixture binaries and tests the served checked-in bytes.

`src/lib/index.ts` is the shared component/type entry; lib modules import no
viewer state. `src/application.ts` exposes the typed application mount/destroy boundary.
Shared chrome owns widget descendants, trigger/avatar, menu/backdrop, open state,
dismissal listeners and focus. The host owns insertion/removal, history and live
holds. Unmount before removing a container. New typed application code cannot use
HTML sinks or inline styles. The public entry installs a native-VNode prop guard;
parse unknown account JSON before rendering.

`createAccountChrome(host)` returns `mount(AccountMenuProps)`, `close(options)`,
`updateWide(boolean)`, `unmount(root)`, `escape()`, `destroy()` and readonly `open`.
One controller arbitrates phone and desktop roots in one shell. The host supplies:

- `place(widget, trigger)` for measured CSSOM placement.
- `opened(compact)` / `closed({keepEntry?, navigating?})` for history/live transactions.
- `navigate(href)` / `submit(form)` returning whether it consumed phone navigation.
  Return false to retain native links and POST submission.

Forward Escape from the host's overlay arbitration so closing the account sheet
leaves its drawer open. Explicit renders commit before focus and measurements;
wide updates retain keyed trigger, workspace and Display nodes. `destroy()`
unmounts roots and removes listeners; `unmount()` precedes host container removal.
Detached triggers cannot reopen old roots. `src/lib-contract.tsx` has a type-checked
independent consumer; `node --test ui/tests/browser-lifecycle.mjs` tests it with the
browser installed from `tests/ui/package-lock.json` (gated by interaction CI).

`createShellChrome(ShellHost)` now owns the application frame, keyed native
navigation, brand/rail controls, lead button, drawer/scrim gestures and account
roots. `mount(app)` adopts the Rust fallback or creates its same frame;
`update(destinations, rail)` supplies stable keys, safe same-origin hrefs,
labels/icons, counts and active route. Only ordinary primary link clicks reach
`host.navigate(destination)`; false preserves native server navigation. Modified
clicks retain browser behavior. `drawerOpened`, `drawerClosed` and `railChanged`
leave held ordering, persistence and routing in the host. Forward Escape from
host overlay arbitration via `account.escape()` / `closeDrawer()`.

`topbar(ShellBar)` commits host-owned `titleSlot` and `actions` nodes, or `mode`
nodes for Find/error navigation. Supply `lead`, `session`, optional validated
AccountMenuProps, and `accountTarget` for Find's nested account row. Nodes remain
direct children for existing flex geometry. Shared chrome detaches replaced slots
without touching their children. Repeated action nodes stay attached and focused.
`slots.recent` and `slots.content` expose host-owned containers; Preact never
renders into or removes their descendants. The host owns screen data, title
metadata/actions, Recent tree, authorization, URL/history, focus/scroll, model
polling and live transactions. `drawerAccount(props | null)` replaces phone
account roots; `restoreDrawer()` applies a captured host transaction.

`unmount()` releases chrome roots/listeners and detaches topbar slots intact;
Recent/page containers and their children remain for host teardown. It permits
remounting. `destroy()` additionally disposes the account controller and is
idempotent. Call it before removing the application container. Account dismissal
listeners now exist only while there are mounted account roots. The independent
consumer tests repeated mount/update/unmount/destroy in both device sizes/themes,
stale triggers, slot preservation, native link semantics and listener removal.

Sidebar-only embeddings retain their server frame/drawer owner and use
`renderShellNavigation(root, destinations, navigate)` for the same keyed link
root. Do not mutate its descendants; a disposing host may call Preact `render(null,
root)` before removal. Full viewer and sidebar-only navigation remain equivalent.
One Preact instance resolves per production bundle; no router, fetch or poller is
added. The Rust shell API serves generated native-shell enhancements from typed sources.
`src/app/native-shell.ts` owns native-page listeners, confirmation dialogs, copy
timers and abortable readiness polling. `mountNativeShell()` destroys any previous
mount and returns an idempotent `destroy()` controller. Native forms and links
retain server behavior; a destroyed owner cannot submit a confirmation or navigate
after a pending readiness/clipboard response. Cargo embeds `shell.generated.js`
and requires no JavaScript toolchain.

All viewer screens, top-bar modes, sheets, Markdown, attachments and transcript
entries now render through typed native Preact components. Keyed turns preserve
unchanged nodes; dirty-turn updates synchronously commit before host scroll
restoration. Components own expansion state, clamps, controls and cleanup.
Typed controllers own validated model/delta snapshots, routing, cache admission,
held ordering, polling/backoff and paging. `src/app/` composes the application lifecycle, destination transactions, Recent
navigation, session chrome, transcript projections, analytics and live updates.
The handwritten viewer and internal compatibility bridge are deleted.

The shared prefix initializes once when shell and viewer assets coexist. Consumer
bundles already include it: **do not prepend `shell::COMPONENT_JS`** to output from
`ui/build.mjs`. The Rust constant remains available for custom hosts. Application
HTML sinks and inline styles remain banned; measured geometry is finite numeric
CSSOM data, with stylesheet cleanup on disposal. External log links require HTTP(S),
`target="_blank"` and `rel="noopener noreferrer"`.

External consumers: `node ui/build.mjs --entry /absolute/consumer.ts --output /absolute/consumer.generated.js`
(typecheck, source security and one pinned Preact runtime). Repeat with `--check`
for checked-in asset freshness. Recent's typed snapshot renderer is exported
from `src/lib`; ordering, model refresh, focus/scroll and sheet policy remain host-owned.

`createPanelChrome(options, host)` creates a one-shot native dialog. Options are
`title`, optional `label`, `sub` and `className`; strings render as text. It exposes
`dialog`, `body`, `show()` and `destroy()`. Preact owns only `.panel-h` descendants.
Append screen content into `body` (`.panel-b`); teardown preserves its descendants.
`show()` synchronously commits the modal, scroll guards and body focus, then calls
`host.opened()`. Native close (Escape/backdrop/button) unmounts the header, detaches
the dialog and removes guards before `host.closed()` exactly once. The host owns
history, live holds and focus return. Destroy is idempotent; stale controls cannot
revive a disposed panel. Destroying an unused panel emits no callbacks. Destroy
an active panel before disposing its host; a detached active dialog also releases
guards on the next wheel/touch event. Session menu, full tool details and Analytics
work items consume this API. Filters, child runs, image dialogs and search sheets
have typed lifecycle controllers and independently owned body roots.

The legacy-host cleanup is tracked in #224. `src/domain/types.ts` declares the
normalized discriminated shapes; `normalize.ts` copies and validates session,
handoff and turn data before host adoption. `calculations.ts`, `trace.ts` and
`format.ts` contain document-independent relationships, outcomes, costs, run
charts and labels. The host keeps raw delta snapshots separate from these
resolved relationships. Ownership and commit ordering are documented in
[viewer-ownership.md](../docs/design/viewer-ownership.md).

`mountViewerApplication(host?)` returns an idempotent `destroy()` owner. Replacing
a document mount first destroys its predecessor. Hosts may pass the documented
`ViewerHost` port directly; existing `configureViewerHost` callers remain supported.
Teardown invalidates callbacks, aborts requests, stops polling and paging, releases
roots/dialogs, and cancels document listeners, timers and animation frames.
`ui/tests/application-lifecycle.mjs` exercises fixture-backed standalone, sidebar
and native document replacement with deliberately late model responses.

## Formatting and local hooks

Run `npm --prefix ui run format` for authored UI TypeScript, build/test MJS and
package/config JSON, including browser test MJS. `format:check` reads the same
tracked scope without writing; CI enforces it before the required UI aggregate.
Prettier is exact-version pinned. Fixture JSON, Rust, docs, generated agent files,
vendored sources, dependencies and generated bundles are excluded. There are no
authored standalone UI styles in this scope. Rust keeps its rustfmt gate.

Install hooks explicitly from the repository root with
`npm --prefix ui run hooks:install`; remove with `npm --prefix ui run hooks:remove`.
Installation is repository-local and refuses existing hooks/configuration. The
pre-commit check reads staged blobs, supports partial staging, renames and spaces,
and never writes files, stages content, installs dependencies or uses the network.
On failure, format your source and stage only the intended changes. Deletions
need no check. Hooks are optional; CI remains authoritative.

A fast pre-push workflow is format:check, typecheck and UI tests; use
`npm --prefix ui run build` explicitly when sources change generated bytes, then
check:bundle. Browser/Rust gates stay in CI and are not run on each commit.
Use `git config --local blame.ignoreRevsFile .git-blame-ignore-revs` to omit the
initial formatting commit from blame.

Viewer pages share the declarative `ApplicationView` commit boundary. `pageRoot`
is the small fallback/native-content adapter; it never renders into host-owned
native descendants. `routeControls` owns independently embedded facet lifetimes.
`ViewUpdates` publishes accepted coherent model/transcript revisions and owns
subscription disposal. Feature consumers use narrow capabilities of real service
owners rather than composition forwarding methods. The ownership document
records transaction order and the justified measured/native subroots.
