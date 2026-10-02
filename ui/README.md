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
viewer state. `src/account-adapter.tsx` is the sole legacy/Preact boundary.
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

Account lifecycle is implemented; shared application shell/navigation and Hub
adoption come next, before sheets/screens. See [the current design](../docs/design/tsx.md)
and [consumer plan](../docs/design/ui-library.md) for remaining stages, runtime sink
review and actual Hub pin/embedding contracts. Legacy sources remain in the
crate during the bounded migration to avoid interfering with parallel work.
