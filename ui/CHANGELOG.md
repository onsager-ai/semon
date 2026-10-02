# Shared UI changes

## 2026-10-02 — shared shell frame/navigation

Added `createShellChrome`, `ShellChrome`, `ShellHost`, `ShellDestination`,
`ShellSlots`, `ShellBar` and `renderShellNavigation`. The viewer consumes the
shared frame, keyed native links, topbar slot commit, brand/rail controls,
drawer gestures and account root lifecycle. Host title/actions/Find/error nodes,
Recent tree and content remain disjoint from Preact roots. URL/history, polling,
authorization and live/scroll transactions remain host responsibilities.

Topbar nodes use `titleSlot`, `actions` or replacement `mode`; direct children
preserve existing geometry. Unmount detaches these slots intact and releases
chrome listeners; Recent/page content is preserved for host teardown. Destroy
is idempotent. Account dismissal listeners are released when the final account
root unmounts. Sidebar-only embeddings use the same navigation renderer while
retaining their server shell owner. Navigation now has real safe hrefs and
preserves modifier clicks; anchor decoration matches the former buttons.

This is stage 3 item 1. Shared Recent/consumer tooling and Hub adoption/persistent
Sessions–Machines transitions remain pending; Hub pins/deployment are unchanged.

## 2026-10-02 — shared account lifecycle

Added `createAccountChrome`, `AccountChrome`, `AccountChromeHost` and
`AccountCloseOptions`. The controller owns phone/desktop trigger and avatar,
menu/backdrop DOM, open state, focus and dismissal listener cleanup. The host
supplies placement, history/navigation, pending live-data release and Display
state; call `unmount(root)` before removing/replacing a root or `destroy()` when
disposing the shell. The existing AccountMenu content contract remains available.

The viewer consumes this API. Hub remains on its existing pin and separate shell;
adoption requires a consumer/pin PR. Shared shell/navigation and Hub adoption
precede sheets/screens; this account step does not consolidate the Hub experience.

## 2026-10-02 — first Preact pilot

Added native Preact account popover contents and typed Account parsing. This is
an additive source-library pilot; the viewer/shell asset URLs, Rust public API,
embedding prelude/events and existing account data acceptance rules are unchanged.
The complete trigger/history lifecycle is not yet a shared component. Future
public-contract breaks must be documented here with the consumer's required edit.
