# Shared UI changes

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
