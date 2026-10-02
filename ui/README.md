# Shared viewer UI

Native Preact/TSX sources and a deterministic production esbuild tail. Rust embeds
`crates/semon-sessions/src/viewer.generated.js` with the existing tooltip/Select
prefixes; cargo builds and installations need no JavaScript tooling.

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

Account and shell frame/navigation lifecycle are implemented. Shared Recent,
a supported consumer build entry, and Hub adoption follow before sheets/screens. See [the current design](../docs/design/tsx.md)
and [consumer plan](../docs/design/ui-library.md) for remaining stages, runtime sink
review and actual Hub pin/embedding contracts. Legacy sources remain in the
crate during the bounded migration to avoid interfering with parallel work.

External consumers: `node ui/build.mjs --entry /absolute/consumer.ts --output /absolute/consumer.generated.js`
(typecheck, source security and one pinned Preact runtime). Repeat with `--check`
for checked-in asset freshness. Recent's typed snapshot renderer is exported
from `src/lib`; ordering, model refresh, focus/scroll and sheet policy remain host-owned.
