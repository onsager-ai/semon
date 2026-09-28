# Viewer shell

The shell gives server-rendered pages the viewer's visual language and reusable form components.

## Assets

Serve the viewer's base stylesheet at `/viewer.css`, the component stylesheet at `/shell.css`, and the script at `/shell.js`. Serve the six font files listed by `semon_sessions::shell::FONT_FILES` at `/fonts/<name>`. The page must load `/viewer.css` before `/shell.css`; load `/shell.js` as a same-origin deferred script. Serve all four paths from the page's own origin because the viewer's content security policy allows same-origin assets only.

The Rust API exposes `semon_sessions::shell::{VIEWER_CSS, CSS, JS, FONT_FILES, font}`. An embedding server can serve these bytes directly and use `font(name)` for font requests.

## Signed-in page skeleton

Keep the sidebar, scrim, and main content as siblings inside `.app`. The menu button belongs in the top bar. The example page content can be replaced with the server's own content.

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Settings</title>
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
      <a class="nav-item" href="/overview">Overview</a>
      <a class="nav-item" href="/devices" aria-current="page">Devices</a>
      <a class="nav-item" href="/activity">Activity</a>
      <a class="nav-item" href="/settings">Settings</a>
    </nav>
    <div class="account">
      <span class="account-login">sample.user@example.invalid</span>
      <button class="btn quiet" type="button">Sign out</button>
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
quiet
copied
btn-row
field
hint
error
rows
row
row-main
nm
meta
actions
status
code
copy
sheet
sheet-h
sheet-body
notice
ok
hero
steps
step
step-num
step-title
step-body
done
current
pending
signin
account
account-login
```

Everything else in `viewer.css` is private to the viewer and may change.
