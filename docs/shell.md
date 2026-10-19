# Viewer shell

Semon owns the reusable Viewer chrome, native form components and bounded reader.
Every hosted reading route uses `mountViewerApplication`; query parameters never
choose another application. Native administration pages adopt the same chrome
through its typed host and keep their server forms and content.

## Assets

Serve the viewer's base stylesheet at `/viewer.css`, the component stylesheet at `/shell.css`, and the script at `/shell.js`. Serve the four font files listed by `semon_sessions::shell::FONT_FILES` at `/fonts/<name>`. The page must load `/viewer.css` before `/shell.css`; load `/shell.js` as a same-origin deferred script. Serve those four paths from the page's own origin because the viewer's content security policy allows same-origin assets only.

The brand mark is an image too. `.mark` paints `/mark.svg` as a CSS mask in the current text colour (`var(--ink)`), so a page that uses `.mark` must serve `semon_sessions::shell::MARK_SVG` at `/mark.svg` as `image/svg+xml`, or the mark is invisible. Serve `semon_sessions::shell::FAVICON_SVG` at `/favicon.svg` as `image/svg+xml` too and link it from the page's `<head>` with `<link rel="icon" href="/favicon.svg" type="image/svg+xml">`; its fill switches between light and dark with the browser's colour scheme. Serve both from the page's own origin: the content security policy allows same-origin assets only, so a `data:` URI or another host will not load.

The Rust API exposes `semon_sessions::shell::{VIEWER_CSS, CSS, JS, MARK_SVG, FAVICON_SVG, FONT_FILES, font}`, the sidebar's header and navigation as `sidebar_head` and `NAV` (see [Signed-in page skeleton](#signed-in-page-skeleton)), the viewer's own sidebar with its session list as `session_sidebar` (see [Native pages with Viewer chrome](#native-pages-with-viewer-chrome)), and for the viewer page itself `VIEWER_JS`, `PAGE_HTML` and `is_page_path` (see [Embedding the viewer page](#embedding-the-viewer-page)). An embedding server can serve these bytes directly and use `font(name)` for font requests.

## Embedding the viewer page

Serve `shell::PAGE_HTML` on paths accepted by `shell::is_page_path`,
`VIEWER_JS` at `/viewer.js` and the same-origin assets below. Implement authorized
source inventory/capability, catalog, identity, range, field, anchor, Home,
Analytics and causal APIs documented in `ui/README.md`. Viewer browsing never
uses the standalone `/api/model` or `/api/tx` endpoints.

A customized host compiles one consumer entry with Semon's owning build tool:

```sh
node ui/build.mjs --entry /absolute/host.ts --output /absolute/host.generated.js
node ui/build.mjs --entry /absolute/host.ts --output /absolute/host.generated.js --check
```

Serve that generated bundle directly, including its guarded shared runtime.
Account data enters through `ViewerHost.account` and is validated before rendering.
Host links and machine paths must be safe same-origin paths. The old global
embedding object, model lifecycle events and sidebar compatibility attributes
have been retired; use the typed lifecycle and selected observers instead.

## Native pages with Viewer chrome

Use the complete `.app` frame from the signed-in skeleton, including `#sidebar`,
`#nav`, `#lanes`, `#main`, `#topbar` and `#page`. `shell::session_sidebar` provides
the server fallback header, navigation and Recent container. Add the native page's
content inside `#page`; the canonical reader never renders into its descendants.

Compile a consumer entry equivalent to `tests/ui/sidebar-host.ts`:

```ts
import { mountViewerApplication, mountNativeShell } from './semon/ui/src/application';

const native = mountNativeShell({ chrome: false });
const viewer = mountViewerApplication({
  machinesPath: '/machines',
  catalogSources: true,
  nativePage: { title: 'Devices', nav: 'machines', workspaceChrome: true },
  account: validatedServerAccount,
});
// Before replacing the document-owned frame:
viewer.destroy();
native.destroy();
```

Mount once after the frame exists. The Viewer owns chrome, navigation, bounded
Recent metadata and its refresh lifetime; native enhancements own confirmation,
copy and readiness controls. Links and submissions retain server behavior.
Set `workspaceChrome` only when the authenticated host has a current workspace.
It enables compact published Recent/count metadata, without selecting a source,
hydrating previews or preparing native history. Omit it for workspaceless pages.
Load the single consumer bundle instead of also loading `/viewer.js` and
`/shell.js`. Without JavaScript, fallback links and forms remain usable.
Sidebar fragments without an application frame can use the pure shared
navigation/Recent renderers under an explicit host owner; they do not mount a
second reader. The former sidebar-only model application is removed.

## Signed-in page skeleton

Keep the sidebar, scrim, and main content as siblings inside `.app`. The menu button belongs in the top bar. The example page content can be replaced with the server's own content.

The sidebar starts with the viewer's header: `.sidebar-head` holding the `.brandrow` (the mark, the `.brandname`, and the drawer's `#drawer-close` button). Inside `.sidebar-head` the row has the viewer's own size and padding at every width, and the close button is the viewer's 44 × 44 at the drawer's right edge. `semon_sessions::shell::sidebar_head(name)` returns exactly that markup, the same bytes `viewer.html` uses (a test holds them together), with `name` escaped; the viewer adds its collapse toggle after the brand row, and a shell page has none (the header keeps the same height without it).

The viewer's nav rows are `<a class="nav-item">` with a leading 18px icon (`<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">`, then a `<span>` label). A served page links between pages instead, so it uses `<a class="nav-item">` with the same icon and label markup; `shell.css` gives `a.nav-item` the viewer's `.nav-item` colours without the browser's underline, and `a.nav-item[aria-current="page"]` the active row's ink colour.

A page that links to the viewer's own destinations draws them from `semon_sessions::shell::NAV`: Home, Sessions, Analytics and Machines, in the viewer's order, each with its label, the viewer's path and its icon (a test checks that the viewer's script draws the same rows with the same icons). `NavLink::html(href, current)` returns the row as `<a class="nav-item">`, with `href` escaped and `aria-current="page"` when `current`; pass the viewer's `path`, or the page's own path for a destination it serves elsewhere.

Links inside `.page`, `.signin`, and `.notice` content use `var(--accent)` instead of the browser default, in both the link and visited states.

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Settings</title>
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <link rel="stylesheet" href="/viewer.css">
  <link rel="stylesheet" href="/shell.css">
  <script src="/shell.js" defer></script>
</head>
<body>
<div class="app">
  <aside class="sidebar" id="sidebar" aria-label="Navigation">
    <div class="sidebar-head"><div class="brandrow"><span class="mark" aria-hidden="true"></span><span class="brandname">Devices</span><button class="ibtn close" id="drawer-close" type="button" aria-label="Close menu"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"></path></svg></button></div></div>
    <nav id="nav" aria-label="Pages">
      <a class="nav-item" href="/overview">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5"></path></svg>
        <span>Overview</span>
      </a>
      <a class="nav-item" href="/devices" aria-current="page">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 5h18v11H3zM8 20h8M12 16v4"></path></svg>
        <span>Devices</span>
      </a>
      <a class="nav-item" href="/activity">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 12h4l2.5-6 5 12 2.5-6h4"></path></svg>
        <span>Activity</span>
      </a>
      <a class="nav-item" href="/settings">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c1-4 4-6 8-6s7 2 8 6"></path></svg>
        <span>Settings</span>
      </a>
    </nav>
    <div class="account">
      <span class="account-login">sample.user@example.invalid</span>
      <button class="btn sh-quiet" type="button">Sign out</button>
    </div>
  </aside>
  <div class="scrim" id="scrim"></div>
  <main class="main" id="main">
    <header class="topbar" id="topbar">
      <button class="ibtn lead" id="lead-btn" type="button" aria-label="Open menu" aria-expanded="false">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16"></path></svg>
      </button>
      <div class="ttl"><span class="t">Settings</span></div>
    </header>
    <div class="page" id="page">
      <div class="ph"><h1>Settings</h1></div>
      <section class="hero">
        <span class="mark" aria-hidden="true"></span>
        <h1>Start with a device</h1>
        <p>Add a device to see its current status and recent activity.</p>
        <div class="btn-row"><a class="btn primary" href="/devices/new">Add a device</a></div>
      </section>
    </div>
  </main>
</div>
</body>
</html>
```

## Sign-in page skeleton

Pages without navigation can use `.signin` as their centered single-column layout.

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Sign in</title>
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <link rel="stylesheet" href="/viewer.css">
  <link rel="stylesheet" href="/shell.css">
  <script src="/shell.js" defer></script>
</head>
<body>
<main class="signin">
  <span class="mark" aria-hidden="true"></span>
  <h1>Sign in</h1>
  <p>Enter your details to continue.</p>
  <form action="/signin" method="post">
    <label class="field">
      <span>Email address</span>
      <input type="email" name="email" autocomplete="email">
    </label>
    <div class="btn-row"><button class="btn primary" type="submit">Continue</button></div>
  </form>
</main>
</body>
</html>
```

## Select

The shell includes a Select: a button that opens a list of options, with the look and behaviour of shadcn/ui's Select. Its script is part of `shell.js`; its styles are part of `viewer.css` (not `shell.css`), which every shell page loads first, so a page that loads `viewer.css` and `shell.js` has all of it. A value cut off by an ellipsis shows its full text in a tooltip (`data-tip`). On a desktop it opens a popover under the button (over it when there is no room below) and keeps inside the viewport. At 760 px and narrower it opens a bottom sheet with 44 px rows and safe-area padding; the page behind it does not scroll, and the sheet is a history entry, so Back closes it. Above eight options a search field sits at the top of the list.

The button shows "Label: value" and a chevron. The list is a `listbox` of `option`s; the selected option has a check mark and `aria-selected="true"`, and the first option is usually "All …". The button is a `combobox` with `aria-expanded` and `aria-controls`, and the keyboard follows the WAI-ARIA Authoring Practices "select-only combobox": Enter, Space and Up or Down open it, Up, Down, Home and End move the highlight, letters jump to a matching option, Enter or Space picks, Escape closes and returns focus to the button, Tab picks the highlighted option and moves on, and a click outside closes it.

To use it with a form, mark a native `<select>` and give it a label. Without the script it stays a native select. With it the native select is hidden, keeps its value (so the form submits it) and gets a `change` event when the choice changes:

```html
<select name="room" data-select data-label="Room">
  <option value="">All rooms</option>
  <option value="kitchen" selected>Kitchen</option>
</select>
```

A script builds one with `SemonShell.select`, and gives it new options later without closing it:

```js
const room = SemonShell.select({ label: "Room", options: [{ value: "", label: "All rooms" }, { value: "kitchen", label: "Kitchen" }], value: "", onChange: (value) => {} });
parent.append(room.el);
room.setOptions(next);   // in place: an open list stays open, with its highlight and search text
room.setValue("kitchen"); // without calling onChange
room.value; room.options; room.isOpen; room.open(); room.close(); room.focus();
```

`SemonShell.enhance(select)` is what the `data-select` markup calls. Select lives
in `ui/src/lib/select.tsx`; markup uses safe nodes and text. The guarded shared
Preact/Select/tooltip prefix is `shared.generated.js`. `shell::JS` combines that
prefix with `shell.generated.js`; `shell::VIEWER_JS` is the complete
`viewer.generated.js` bundle. `VIEWER_CSS` combines `viewer.css` and `select.css`.
Generated consumer bundles already contain the prefix and must be served directly.


## Script contract

The script uses `#lead-btn`, `#sidebar`, `#drawer-close`, and `#scrim` for the phone drawer. Esc closes the drawer, except while a popover is open: then Esc closes only the popover, so a menu opened from the drawer (a `popover` element) closes on its own. It uses `#topbar` and `#main` to synchronize `.topbar.scrolled` with the active scroll container. Missing elements are allowed, so the script can also be loaded by the sign-in skeleton.

Use `form[data-confirm="dialog-id"]` to open that `dialog.sheet` before submission. A `button[value="confirm"]` submits the original form; another button, Escape, or the backdrop closes the sheet. Use `button[data-open="dialog-id"]` to open a sheet, and `[data-close]` to close its enclosing dialog. A `[data-copy="element-id"]` button copies the target's text. An element with `[data-poll="/path"]` checks that same-origin URL every three seconds for up to 30 minutes and navigates to `data-poll-go` (or `/`) when it receives status 200.

## Tooltips

Put `data-tip="text"` on any element to give it a tooltip; `/shell.js` and the viewer's own script both include the controller (a page that loaded both would still run one), and `/viewer.css` styles it, so a page needs nothing else. It replaces the native `title` attribute, which the viewer no longer uses anywhere.

- One `<div id="sh-tooltip" role="tooltip">` is made on first use and reused. The text is set as text, never as markup. Set or change `data-tip` at any time (also when the element is re-rendered): it is read when the tooltip opens, and an open tooltip follows a change or closes if its element is removed or hidden.
- It opens after about 500 ms of hover. Once one has shown, another element reached within 300 ms shows at once. Keyboard focus (`:focus-visible` only) shows it with no delay. It closes on pointer leave, blur, Esc, a click, or a scroll that moves its element. Esc keeps it closed until the pointer leaves the element; inside an open `dialog` one Esc closes only the tooltip. If a re-render replaces the element under a shown tooltip, it moves to the replacement under the pointer (or holding focus) and stays open. The tooltip takes no pointer events.
- It sits above the element, below when there is no room, at least 8 px inside the viewport, never over the element. It is 13 px text on the inverse surface (`--ink` behind `--ground`), at most 280 px wide, and a popover, so it shows above an open `dialog.sheet`. It fades in unless the user prefers reduced motion.
- While it shows, the element gets `aria-describedby="sh-tooltip"`, unless its `aria-label` or own text already contains the tip's text. A tip is not a place to keep the only copy of anything: an icon or badge whose text is only in its tip needs `tabindex="0"` (so keyboard focus can show it) and an `aria-label`, or hidden text that its `aria-describedby` names (the tooltip then adds no second description), and a control's own accessible name must carry the information.
- On touch, tapping a static element with a tip toggles it and tapping anywhere else closes it. Tapping a button, link or other control runs the control and shows no tip.
- `data-tip-clipped` on an element with an ellipsis shows the tip only while its text is cut off, for a tip that would otherwise repeat the visible text.

## Classes

These viewer chrome classes and shell components form the supported class contract:

```
app
sidebar
sidebar-head
brandrow
brandname
mark
scrim
main
topbar
ttl
ibtn
lead
nav-item
icon
page
ph
sec-h
list
empty
dot
btn
primary
danger
sh-quiet
copied
btn-row
field
hint
error
rows
row
row-main
sh-nm
sh-meta
actions
status
code
copy
sheet
sheet-h
sheet-body
notice
ok
sh-err
hero
sh-steps
sh-step
step-num
step-title
step-body
sh-done
current
pending
signin
account
account-login
sh-select
sh-select-trigger
sh-select-text
sh-select-label
sh-select-value
sh-select-chevron
sh-select-pop
sh-select-search
sh-select-list
sh-select-option
sh-select-check
sh-select-empty
sh-select-sheet
sh-select-close
```

Classes prefixed `sh-` are renamed from a shorter name because `viewer.css` already defines a class of that name for its own, unrelated chrome (for example, the transcript's `.steps`/`.step` draw a numbered rail with a different meaning). A Rust test in `shell.rs` fails the build if a class `shell.css` defines, other than the shared chrome list above, is also defined by `viewer.css`.

Everything else in `viewer.css` is private to the viewer and may change.

## Typed lifecycle

`ui/src/application.ts` exports `mountViewerApplication` and `mountNativeShell`.
Each returns an idempotent destroy owner and replaces its previous document owner.
The canonical application owns source selection, bounded reading state, one
navigation controller, roots and effects. Native descendants remain host-owned.
See [UI README](../ui/README.md) and [Viewer ownership](design/viewer-ownership.md)
for acceptance, disposal, selected observation and consumer verification contracts.
