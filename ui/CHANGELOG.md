# Shared UI changes

## 2026-10-03 — complete screen and controller migration

Typed native Preact owns every viewer screen, transcript entry, sheet body and
Find/error mode. Shared typed model/delta, routes, cache, ordering, polling and
paging controllers replace corresponding global orchestration. Browser history,
scroll transactions and domain calculations remain in the JavaScript host adapter.
Typed Select and tooltip preserve native shell APIs and dismissal/cleanup behavior.
Measured geometry uses finite numeric stylesheet rules; application HTML/style
sink restrictions remain in force.

**Consumer build contract:** generated viewer and external consumer bundles now
include the shared production prefix. Remove any extra `shell::COMPONENT_JS`
prepend when serving them. The constant remains for custom script hosts. Viewer
and shell assets loaded together share one runtime/control instance. Rust builds
still require no Node; asset URLs, embedding events and native form APIs persist.

## 2026-10-03 — shared top-bar panel pilot

Added `createPanelChrome`, `PanelChrome`, `PanelHost` and `PanelOptions`.
The session menu, full tool-details panel and Analytics work-item panel share
a native dialog controller and Preact header. The body is a separate host-owned
slot; screen data/actions, history and live-update release remain in the viewer.
The controller releases wheel/touch listeners and its header root before its
one-shot `closed()` callback. `destroy()` is idempotent and stale controls cannot
affect later panels. No CSS, endpoint, router, poller or Rust installation change.

This starts stage 4; filter/Recent/transcript sheets and individual screens remain
legacy. Hub still pins the completed stage-3 source; adoption requires a separate
tested pin. Transcript/paging/live migration stays last.

## 2026-10-02 — shared Recent and consumer builds

Added `createRecentRenderer`, `RecentSnapshot`, `RecentItem`, `RecentHost` and
`RecentRenderer`. Keyed tree descendants, metadata fitting and cleanup are shared;
model polling, held ordering, expansion policy, focus, scroll and sheets stay in
the host. External typed entry/output builds typecheck and enforce security,
asset freshness and one pinned Preact runtime.

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

Stage 3 Hub integration adds a narrow `configureViewerHost` content port for
Machines and native administrative chrome. It retains the viewer's router and
poller; screens/transcript orchestration remain legacy. Host content loads are
abortable, commit only while current, and carry explicit teardown. Failed or
unsupported navigation stays native. `COMPONENT_JS` supplies legacy component
prefixes without duplicate drawer/account handlers.
