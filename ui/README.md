# Shared viewer UI

Native Preact/TSX sources and deterministic production esbuild bundles. Rust embeds
`crates/semon-sessions/src/viewer.generated.js`, which includes the shared runtime,
Select and tooltip prefix. Cargo builds and installations need no JavaScript tooling.
The Viewer bundle is minified for production. Its emitted module boundaries remain
inline so the Rust sink audit can isolate the exact pinned Preact core and scan
every application module; the runtime exemption and injection assertions are unchanged.

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
`update(root, AccountMenuProps)`, `updateWide(boolean)`, `unmount(root)`,
`escape()`, `destroy()` and readonly `open`.
One controller arbitrates phone and desktop roots in one shell. The host supplies:

- `place(widget, trigger)` for measured CSSOM placement.
- `opened(compact)` / `closed({keepEntry?, navigating?})` for history/live transactions.
- `navigate(href)` / `submit(form)` returning whether it consumed phone navigation.
  Return false to retain native links and POST submission.

Forward Escape from the host's overlay arbitration so closing the account sheet
leaves its drawer open. Explicit renders commit before focus and measurements;
account and wide updates retain keyed trigger, workspace and Display nodes.
Shell publication updates reuse mounted phone and desktop account roots, preserving
focus returned by overlay dismissal. `destroy()`
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

`src/lib/navigation.json` defines the core destination identities, labels, icons
and standalone paths. Rust fallback and every reader project this same registry.
The optional `ViewerHost.navigation` supplies generic path overrides and leading
or trailing destinations; `subscribeNavigation` reports changes without requiring
a content model. Its unsubscribe belongs to the reader's document lifetime.
Hosts validate unknown policy JSON with `parseShellNavigation` before rendering.
Recent rows with their machine. Sessions is the default landing page at `/`;
Analytics shares that reader. Host administration pages share the reader's shell,
routing and metadata lifecycle. `ViewerHost.pages` provides matching/title/
navigation metadata and abortable authorized content, including the initial
server page. The router inserts it in the existing content slot, mounts its
enhancements, and destroys them on departure. Source capability responses and
host responses are fenced against later destinations. Host reads share the
bounded compact Recent publication without selected reads or repair. Account or
workspace changes and redirects require a fresh authorized document; native POST
forms retain server handling. A legacy static `nativePage` embedding port remains
supported, but Hub does not use that mode. Compatibility markers are normalized
away without selecting another application.

`topbar(ShellBar)` commits host-owned `titleSlot` and `actions` nodes, or `mode`
nodes for Find/error navigation. Supply `lead`, `session`, optional validated
AccountMenuProps, and `accountTarget` for Find's nested account row. Nodes remain
direct children for existing flex geometry. Shared chrome detaches replaced slots
without touching their children. Repeated action nodes stay attached and focused.
`slots.recent` and `slots.content` expose host-owned containers; Preact never
renders into or removes their descendants. The host owns screen data, title
metadata/actions, Recent tree, authorization, URL/history, focus/scroll, read
refresh and selected observations. `drawerAccount(props | null)` replaces phone
account roots; `restoreDrawer()` applies a captured host transaction.

`unmount()` releases chrome roots/listeners and detaches topbar slots intact;
Recent/page containers and their children remain for host teardown. It permits
remounting. `destroy()` additionally disposes the account controller and is
idempotent. Call it before removing the application container. Account dismissal
listeners now exist only while there are mounted account roots. The independent
consumer tests repeated mount/update/unmount/destroy in both device sizes/themes,
stale triggers, slot preservation, native link semantics and listener removal.

Hub administration mounts the canonical application with `ViewerHost.pages`.
`mountNativeShell({ chrome: false })` supplies only page-local form enhancements;
its lifetime belongs to the content page, while chrome and routing stay mounted.
Sessions, Analytics and host links share ordinary same-document navigation.
Server fallbacks, identity changes and form submissions retain browser navigation.
Standalone navigation fragments may use pure `renderShellNavigation` under an
explicit host owner, with no reader or poller. The former sidebar-only model
application and its compatibility attributes are removed.
The Rust shell API serves generated native-shell enhancements from typed sources.
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
Typed controllers own validated bounded publications, routing, cache admission,
refresh/backoff and paging. `src/app/` composes the application lifecycle, destination transactions, Recent
navigation, session chrome, transcript projections, analytics and live updates.
The handwritten viewer and internal compatibility bridge are deleted.

The shared prefix initializes once when shell and viewer assets coexist. Consumer
bundles already include it: **do not prepend `shell::COMPONENT_JS`** to output from
`ui/build.mjs`. The Rust constant remains available for custom hosts. Application
HTML sinks and inline styles remain banned; measured geometry is finite numeric
CSSOM data, with stylesheet cleanup on disposal. `setGeometry` also accepts
`minHeight` for a host-owned visible-viewport layout; release that measurement
when the host is destroyed. External log links require HTTP(S),
`target="_blank"` and `rel="noopener noreferrer"`.

External consumers: `node ui/build.mjs --entry /absolute/consumer.ts --output /absolute/consumer.generated.js`
(typecheck, source security and one pinned Preact runtime). Repeat with `--check`
for checked-in asset freshness. Thermally restricted asset preparation may add `--bundle-only` to the `--entry`/`--output` command (or pass `{ bundleOnly: true }` as `buildConsumer`'s fourth argument). This skips TypeScript program creation only; source/sink security and the sole-runtime invariant still run. It writes an unqualified artifact, requires the normal default consumer `--check` and full type/security/freshness gates in CI, and is rejected with `--check` or without a consumer entry/output. It does not qualify a build or change any CI invocation. Recent's typed snapshot renderer is exported
from `src/lib`; ordering, metadata refresh, focus/scroll and sheet policy remain host-owned.

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

The canonical reader owns validated compact overview responses, source-scoped
catalogs and selected native ranges. `domain/relationships.ts` validates bounded
recorded joins; `trace.ts` and `format.ts` supply pure calculations and labels.
No browser owner adopts a complete workspace model or model delta.

`mountViewerApplication(host?)` destroys its predecessor and returns one
idempotent `destroy()` owner. The reader owns navigation, the request lifecycle,
selected identity, cached session roots and the refresh cadence. Teardown aborts
requests, rejects late responses, releases roots/dialogs and disposes document
listeners, timers and animation frames. Standalone and Hub use the same mount.
Native administrative pages retain server content and forms while sharing chrome
and compact Recent metadata. `ui/tests/application-lifecycle.mjs` exercises full,
native and independently configured mounts with deliberately late responses.

`ViewerHost.catalogControlStream` and `catalogRuntimeStream` supply independently
authorized selected observations. Only current identity can grant native controls;
retained content and recorded relationships never grant execution authority.
Content retries use bounded requests and the existing reader cadence. Native
readiness retains its independent deadline and lifetime. Status changes update
existing control leaves without replacing the transcript or losing drafts.

The collection reader temporarily supplies no `ControlView`, launch control or native control stream and removes New Session navigation. Shared Session components render Composer only when an optional consumer explicitly supplies a `ControlView`; their direct fixtures continue qualifying drafts, unknown outcomes and lifecycle without granting collection authority. Passive recorded asks/outcomes and the independent runtime observation remain visible. Metadata search and in-session selected-text navigation remain active; broad full-text find is not advertised. See the [temporary collection scope](../docs/design/session-foundation-retirement.md#temporary-collection-scope).

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

Viewer pages share the declarative `ApplicationView` commit boundary. The
canonical reader owns destination changes and releases the previous screen;
native descendants remain host-owned. Overview facets and contextual Composer
panels own their independent controls and dispose them on route change. Accepted
catalog/range/overview publications are coherent before they notify presentation.
Feature consumers use narrow capabilities of their actual owners. The ownership
document records transactions and measured/native subroots.

Refactoring rules and the required review checklist live in
[the viewer ownership contract](../docs/design/viewer-ownership.md). Read it
before changing ownership, ports, adoption or rendering. Run
`npm --prefix ui run check:architecture` to check canonical reader construction dependencies and
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

A host may provide `controlStream`, a same-origin SSE status resource separate
from its content model. The Viewer sends `selected` (native session ID) and
`machine` (projected presentation identity). Hosts resolve the authoritative
source and recheck custody and access for every delivery. The `control` event
contains `{selected, machine, revision, control}`; `control` uses the existing
validated native snapshot contract or null. Additional fields may be passed to
`modelNavigation`. Revisions identify complete status snapshots independently
of content. Each new subscription starts with a snapshot; unchanged observations
send `heartbeat` at least every 30 seconds. `unavailable` and transport errors
trigger owned capped retries; `ended` clears the old authority. Absent or stale
status disables native capabilities while retaining a current draft.

With this option, content reads use `content=1`; content streams include their
accepted `since` revision. Hosts must preserve conditional content reads while
serving native status independently. The existing model/control transaction
continues unchanged for hosts without this option.

Catalog consumers may instead use `catalogControlStream` with the accepted
`CatalogSelectionStore` port. Requests and snapshots carry exact
`{source_key, catalog_key, native_id, harness}` identities. Canonical catalog
routes and native session IDs remain distinct; the observer never derives
native identity from a route, display hostname, or source-reference order.
Missing or ambiguous native identity opens no status stream and grants no
control authority. Hosts validate the selected catalog source and independently
recheck runtime custody and authorization. Catalog selection does not fall back
to workspace model resolution. The scoped observer retains the same snapshot,
heartbeat, retry and teardown semantics described above.

The catalog read consumers are separate from the complete workspace graph.
`CatalogCapabilities` validates an explicit `catalog-v1` advertisement; unsupported
search, ordering, union, attachment, relationship and large-record capabilities
are not inferred from cached metadata. A recognized contract with selected
transcripts still unavailable must remain a bounded pending view, rather than
starting a concurrent workspace model read.

`CatalogTranscriptStore` accepts only issued requests for the current selected
source and canonical session. Each page has a half-open native slot range,
projection generation, stable entry identities and original source provenance.
Explicit `after=0` includes the first slot; omitting `after` requests the latest
`limit` entries. Pages from different generations cannot mix, and an inconsistent
overlap is rejected before any retained range changes. A stale projection needs
explicit resynchronization of the requested ranges. Slot numbers address ranges;
entry identities preserve renderer ownership across projection versions.
The document mount always constructs the canonical reader. It probes source
capabilities before selected reads; missing or pending capabilities remain a
bounded observation with owned retries. Home and Analytics use compact
`/api/viewer/home` and `/api/viewer/analytics` publications; visible native text
uses the qualified `/api/session-home` preview API. Trace follows one requested
source-qualified causal closure through `/api/session-trace` and reuses the
existing measured rail/chart presentation. No browser content owner requests
`/api/model`, `/api/tx`, `/api/tool` or `/api/image`. Old compatibility markers
are removed while supported URLs and turn anchors remain usable.

The reader supports machine-scoped catalog order, exact harness/repository/model
filters, metadata search, source discovery and selected native field expansion.
Capability advertisements are independently rechecked. Native actions preserve
original text while explaining incomplete relationship context. Source/facts and
historical observations remain separate from runtime authority. Only an accepted
current-scope `/api/session-identity` response can select native controls; a
historical transcript or causal response cannot supply that authority.

Session roots and requested ranges are retained during navigation. Unchanged
range observations update metadata without walking transcript blocks. Stale
projection recovery builds the latest page and previously requested ranges in a
separate store, then commits them together. The shared renderer retains stable
entry keys, expansion state, control drafts and reading anchors. Permanent
failures remain visible; transient observations use owned bounded requests and
capped retries. Native status continues through the existing scoped observer,
independently from content revisions. Native administrative pages
retain their server content and use the same canonical shell lifetime.

The browser contract uses synthetic source payloads and the actual shared
renderers; it does not establish production body-span, archive, host authorization
or connection-to-session integration performance. Those require the matching
backend capability/projection producer and recorded artifact revisions.

When `selected_entry` is advertised and a native entry includes a qualified string
field descriptor, its recorded text can be loaded one chunk at a time through
`/api/session-entry`. Each chunk is checked against the selected source, canonical
session, projection generation, native slot, field name and original provenance.
The first accepted chunk replaces the preview; subsequent actions append only the
next bounded chunk. Loaded text is retained across navigation and projection
changes, with an explicit reload action after a change. Unsupported formats and
failed reads remain visible; this path never requests the compatibility tool,
image or transcript endpoints. Source observation failures retain previously
loaded useful bodies while marking their entry observation incomplete.
Status snapshots may include a typed `runtime` observation on the existing native
control snapshot: `{state, phase, freshness, reconnectable}` and optional
`presence`, `observedAt`, `observationError`, `updating`. Durable session phase,
compute presence and observation freshness are separate. Non-active states
carry disabled native capabilities. Ended and failed states have explicit
presentation; they retain the composer and its draft and do not advertise native
reconnect. A final end invalidates pending command receipts without replay.

Selected catalog read intent also has a separate readonly runtime presentation.
`SessionSnapshot.runtime` carries observation and delivery freshness without native
capabilities, credentials or reconnect authority. `updateSessionRuntime` updates
only the existing session owner's runtime leaf, including before transcript or
current native identity is available. Failed observations retain the last durable
phase with a stale indicator. Native controls keep their separate authority and
draft ownership. The focused browser contract poisons transcript block traversal
while applying final and stale runtime updates and verifies retained input focus.

The current native projection recipe is version 6 (the public API envelope stays
version 1). Range and field decoders reject other projection versions. Native
field descriptors and chunks may declare one or two string-encoding layers;
source byte limits follow that declaration, and decoded text remains bounded.
A metadata-only source chooser handles the initial machine-scope requirement
through `/api/session-sources`, retaining exact source keys even when labels
match. It never loads a complete model to discover sources. The same owner retains source-keyed session roots, recorded ranges, expanded
fields, drafts, filters and page cursors across a source change. Its Sources
destination temporarily clears native and readonly runtime selection while
reading only source metadata; exact capability validation activates the next
source. Browser back and forward restore the corresponding source and session. The sole local CLI source uses
the nonempty catalog alias `local`.

Live range refresh preserves older reading anchors. Following the latest output
scrolls only the selected view, while the shared toolbar Jump action resumes
following deliberately. Sidebar collapse and native drafts survive session
switches. The optional real producer check is `ui/tests/catalog-served.mjs`: set
`SEMON_CATALOG_SERVED_BIN` to a matching test-clock executable and record its
source revision with `SEMON_CATALOG_SERVED_SOURCE`; the test uses the embedded matching UI bundle (set
`SEMON_CATALOG_SERVED_UI_OVERRIDE=1` only for an explicitly recorded consumer overlay) and checks cold first content, bounded selected reads and
retained return without compatibility endpoint requests. This is integration
evidence for its documented synthetic workload, not a general latency budget.

Recipe 6 binds every qualified source-byte read to the original source before
and after the read. Previous recipe headers remain unavailable until rebuilt;
selected ranges still resynchronize through generation checks. Non-success JSON
reads cancel their unread response body while retaining the HTTP status, so a
retired request cannot keep its error transport open.
