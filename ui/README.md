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

Hosts may set `ViewerHost.modelStream` to a same-origin SSE endpoint. `model`
events contain full model JSON and enter the existing validated model/transcript
transaction; `unavailable` shows a reconnecting notice, and `ended` closes the
stream and calls `modelFailed(403)`. EventSource reconnects interrupted streams.
The application owns and closes this connection on teardown; stream mode suspends ordinary
browser polling. Initial read failures retry with bounded backoff. A stream
application or observation failure resumes the same live controller for full
snapshot resynchronization; Reconnecting clears only after required model and
transcript reads apply. Requests have a 15-second deadline and teardown cancels
recovery. Native readiness polls have independent 10-second request deadlines
and stop after thirty minutes, removal, success or teardown. Recovery retries
reads only and retains native forms and conversation input. Hosts without this port retain the existing poller.
`modelNavigation` receives an accepted model for host-owned navigation updates.
Rust hosts can call `ViewerCore::session_machine_key` to bind controls to a
caller's mirror key rather than a displayed hostname. Missing or ambiguous
session IDs return no owner; the model JSON format stays unchanged.

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
hook first checks the staged UI scope. Empty, Rust/docs-only and UI deletion-only
commits need neither Node nor UI dependencies. Relevant UI changes require the
pinned local tooling and fail with an actionable setup message when it is missing.
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

Refactoring rules and the required review checklist live in
[the viewer ownership contract](../docs/design/viewer-ownership.md). Read it
before changing ownership, ports, adoption or rendering. Run
`npm --prefix ui run check:architecture` to check composition dependencies and
cycles in emitted runtime imports. Type-only feature cycles remain permitted;
module paths must be literal so the graph can be checked. CI runs this check in
the required UI foundation lane; unit tests verify rejection and acceptance cases.

Opt-in native composers use `.sh-composer`, `.sh-composer-toolbar` and
`.sh-picker` with host-owned textarea/details/form descendants.
`mountComposers(root)` enhances Escape and mutually exclusive pickers and returns
an idempotent teardown owner; `mountNativeShell` installs it once. Native details
and forms remain the fallback. `.sh-compact` separates a 32px visible surface
from 44px phone/coarse targets. The shared tooltip overlay reads `data-tip` from
icon controls and stays outside the composer frame. Consumers supply names,
selected values and all data/actions; no persistence or credential policy lives
in this primitive. `ViewerHost.nativeNavigation` adds validated host-owned native
links to the same shell, without extending the viewer router or poller.
`tests/ui/compact-gallery.mjs` audits the new opt-in geometry and lifecycle;
existing gallery baselines and thresholds are unchanged.

Compact composer pickers adopt the existing `createPanelChrome` and shared
`createSelect` owners. The native-shell lifetime owns their enhancement and
teardown; the consumer retains named form controls and submission. Panels stay
inside their original form, so settings and submitter overrides participate in
native POST and FormData. A nested Select sheet owns its own scroll boundary.
No-JS keeps styled native fields and details. Destroy restores host content and
native fields before removing enhancement roots; remount cannot duplicate them.
