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
viewer state. `src/account-adapter.tsx` is the legacy/Preact library bridge.
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
added. The Rust shell API and shell.js remain available for existing consumers.

All viewer screens, top-bar modes, sheets, Markdown, attachments and transcript
entries now render through typed native Preact components. Keyed turns preserve
unchanged nodes; dirty-turn updates synchronously commit before host scroll
restoration. Components own expansion state, clamps, controls and cleanup.
Typed controllers own validated model/delta snapshots, routing, cache admission,
held ordering, polling/backoff and paging. `viewer.js` remains the browser host
adapter for domain calculations, history, scroll capture/restore and embedding
callbacks; it contains no legacy screen renderer or DOM factory.

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
