# Viewer shell

The shell gives server-rendered pages the viewer's visual language and reusable form components.

## Assets

Serve the viewer's base stylesheet at `/viewer.css`, the component stylesheet at `/shell.css`, and the script at `/shell.js`. Serve the six font files listed by `semon_sessions::shell::FONT_FILES` at `/fonts/<name>`. The page must load `/viewer.css` before `/shell.css`; load `/shell.js` as a same-origin deferred script. Serve those four paths from the page's own origin because the viewer's content security policy allows same-origin assets only.

The brand mark is an image too. `.mark` paints `/mark.svg` as a CSS mask in the current text colour (`var(--ink)`), so a page that uses `.mark` must serve `semon_sessions::shell::MARK_SVG` at `/mark.svg` as `image/svg+xml`, or the mark is invisible. Serve `semon_sessions::shell::FAVICON_SVG` at `/favicon.svg` as `image/svg+xml` too and link it from the page's `<head>` with `<link rel="icon" href="/favicon.svg" type="image/svg+xml">`; its fill switches between light and dark with the browser's colour scheme. Serve both from the page's own origin: the content security policy allows same-origin assets only, so a `data:` URI or another host will not load.

The Rust API exposes `semon_sessions::shell::{VIEWER_CSS, CSS, JS, MARK_SVG, FAVICON_SVG, FONT_FILES, font}`. An embedding server can serve these bytes directly and use `font(name)` for font requests.

## Signed-in page skeleton

Keep the sidebar, scrim, and main content as siblings inside `.app`. The menu button belongs in the top bar. The example page content can be replaced with the server's own content.

The viewer's own nav rows are `<button class="nav-item">` with a leading 18px icon (`<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">`, then a `<span>` label). A served page links between pages instead, so it uses `<a class="nav-item">` with the same icon and label markup; `shell.css` gives `a.nav-item` the viewer's `.nav-item` colours without the browser's underline, and `a.nav-item[aria-current="page"]` the active row's ink colour.

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
    <div class="brandrow">
      <span class="mark" aria-hidden="true"></span><span>Devices</span>
      <button class="ibtn close" id="drawer-close" type="button" aria-label="Close menu">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"></path></svg>
      </button>
    </div>
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

## Script contract

The script uses `#lead-btn`, `#sidebar`, `#drawer-close`, and `#scrim` for the phone drawer. It uses `#topbar` and `#main` to synchronize `.topbar.scrolled` with the active scroll container. Missing elements are allowed, so the script can also be loaded by the sign-in skeleton.

Use `form[data-confirm="dialog-id"]` to open that `dialog.sheet` before submission. A `button[value="confirm"]` submits the original form; another button, Escape, or the backdrop closes the sheet. Use `button[data-open="dialog-id"]` to open a sheet, and `[data-close]` to close its enclosing dialog. A `[data-copy="element-id"]` button copies the target's text. An element with `[data-poll="/path"]` checks that same-origin URL every three seconds for up to 30 minutes and navigates to `data-poll-go` (or `/`) when it receives status 200.

## Classes

These viewer chrome classes and shell components form the supported class contract:

```
app
sidebar
brandrow
mark
scrim
main
topbar
ttl
ibtn
lead
nav-item
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
```

Classes prefixed `sh-` are renamed from a shorter name because `viewer.css` already defines a class of that name for its own, unrelated chrome (for example, the transcript's `.steps`/`.step` draw a numbered rail with a different meaning). A Rust test in `shell.rs` fails the build if a class `shell.css` defines, other than the shared chrome list above, is also defined by `viewer.css`.

Everything else in `viewer.css` is private to the viewer and may change.
