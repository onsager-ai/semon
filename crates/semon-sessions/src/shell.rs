//! Public assets for pages that share the viewer's presentation shell.
//!
//! Serve these assets from the page's own origin at `/viewer.css`, `/shell.css`, `/shell.js`, `/mark.svg` and
//! `/favicon.svg`, and serve each font at `/fonts/<name>`; the viewer's content security policy permits same-origin
//! assets only.

/// The viewer's base styles, including its tokens, fonts, and chrome, and the Select's styles (`select.css`).
pub const VIEWER_CSS: &str = concat!(include_str!("viewer.css"), "\n", include_str!("select.css"));

/// The viewer's page script. Embedders serving the viewer without a `ViewerCore` can serve it at `/viewer.js`. It starts
/// with the tooltip (`tooltip.js`, also the start of [`JS`]), then the Select component (`select.js`), then the viewer itself.
pub const VIEWER_JS: &str = concat!(
    include_str!("tooltip.js"),
    "\n",
    include_str!("select.js"),
    "\n",
    include_str!("viewer.js")
);

/// The viewer page returned by its page routes. Embedders serving the viewer without a `ViewerCore` can serve these
/// bytes for each page route.
pub const PAGE_HTML: &str = include_str!("viewer.html");

/// Additional components for forms and other server-rendered pages.
pub const CSS: &str = include_str!("shell.css");

/// The tooltip for any element with a `data-tip` attribute (its source, `tooltip.js`, is also the start of the viewer's own
/// script), the Select component (`SemonShell.select`, and `<select data-select>` pages), then the drawer, copy, dialog, and
/// readiness-poll behavior for shell pages.
pub const JS: &str = concat!(
    include_str!("tooltip.js"),
    "\n",
    include_str!("select.js"),
    "\n",
    include_str!("shell.js")
);

/// The Semon mark, a monochrome glyph. `.mark` paints it as a mask from `/mark.svg`, so a page that uses `.mark` must
/// serve this at that path (as `image/svg+xml`).
pub const MARK_SVG: &str = include_str!("mark.svg");

/// Embedding pages serve each unmodified icon at its path as `image/svg+xml`.
pub const HARNESS_ICONS: &[(&str, &str)] = crate::HARNESS_ICONS;

/// The mark as a favicon: its fill follows the browser's light or dark scheme. Serve it at `/favicon.svg` and link it
/// with `<link rel="icon" href="/favicon.svg" type="image/svg+xml">`.
pub const FAVICON_SVG: &str = include_str!("favicon.svg");

/// One of the viewer's navigation destinations. [`NAV`] lists them in the order the viewer's sidebar draws them; a page
/// served beside the viewer draws the same rows with [`NavLink::html`], so its drawer lists what the viewer's does, with
/// the same labels and icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavLink {
    /// The viewer's name for the destination: `home`, `sessions`, `analytics` or `machines`.
    pub key: &'static str,
    pub label: &'static str,
    /// The viewer's own path for it. An embedding page that serves a destination at another path links there instead.
    pub path: &'static str,
    /// The icon's SVG path data, drawn in a 24 × 24 box.
    pub icon: &'static str,
}

/// The viewer's navigation, in its sidebar's order. A Rust test checks that the viewer's own script draws exactly these
/// rows with these icons, so a change here or there that leaves the other behind fails the build.
pub const NAV: [NavLink; 4] = [
    NavLink {
        key: "home",
        label: "Home",
        path: "/",
        icon: "M4 11l8-7 8 7M6 9.5V20h12V9.5M10 20v-5h4v5",
    },
    NavLink {
        key: "sessions",
        label: "Sessions",
        path: "/sessions",
        icon: "M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01",
    },
    NavLink {
        key: "analytics",
        label: "Analytics",
        path: "/analytics",
        icon: "M4 19V5M4 19h17M8 15l3-4 3 2 5-7",
    },
    NavLink {
        key: "machines",
        label: "Machines",
        path: "/machines",
        icon: "M3 5h18v11H3zM8 20h8M12 16v4",
    },
];

impl NavLink {
    /// The row as a served page draws it: `<a class="nav-item">` with the viewer's 18 px icon and the label, marked
    /// `aria-current="page"` when `current`. `href` is escaped here.
    pub fn html(&self, href: &str, current: bool) -> String {
        format!(
            "<a class=\"nav-item\" href=\"{href}\"{current}><svg class=\"icon\" viewBox=\"0 0 24 24\" fill=\"none\" \
             stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" stroke-linejoin=\"round\" \
             aria-hidden=\"true\"><path d=\"{icon}\"></path></svg><span>{label}</span></a>",
            href = escape(href),
            current = if current {
                " aria-current=\"page\""
            } else {
                ""
            },
            icon = self.icon,
            label = self.label,
        )
    }
}

/// The sidebar's header row: the mark, `name`, and the drawer's close button (`#drawer-close`), in the `.sidebar-head`
/// the viewer's own sidebar starts with, so the row has the viewer's size, padding and close button at every width.
/// `name` is escaped here. The viewer's header adds its collapse toggle after the brand row; a shell page has none.
pub fn sidebar_head(name: &str) -> String {
    format!("<div class=\"sidebar-head\">{}</div>", brand_row(name))
}

/// The brand row, byte for byte as `viewer.html` has it (a test holds the two together).
fn brand_row(name: &str) -> String {
    format!(
        "<div class=\"brandrow\"><span class=\"mark\" aria-hidden=\"true\"></span><span class=\"brandname\">{name}</span>\
         <button class=\"ibtn close\" id=\"drawer-close\" type=\"button\" aria-label=\"Close menu\">\
         <svg viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.9\" stroke-linecap=\"round\" \
         aria-hidden=\"true\"><path d=\"M6 6l12 12M18 6L6 18\"></path></svg></button></div>",
        name = escape(name),
    )
}

/// The Recent heading and the list the viewer's script draws the sessions into, below the navigation.
const SIDEBAR_RECENT: &str = "<div class=\"side-h\">Recent</div>\n<div class=\"side-list\" id=\"side-list\"><div id=\"lanes\" role=\"tree\" aria-label=\"Recent sessions\"></div></div>";

/// The viewer's own sidebar, for a page that shows the viewer's session list beside its own content (docs/shell.md, "The
/// viewer's sidebar on an embedding page"): the header ([`sidebar_head`]: the rail and its collapse toggle are the viewer's own
/// layout, which an embedding page doesn't take), `nav` inside `<nav id="nav">`, then the Recent heading and its list, each as
/// `viewer.html` has it (a test holds them together).
/// `name` is escaped here; `nav` is markup, the page's own rows (see [`NavLink::html`]), shown until the viewer's script
/// draws its navigation in their place. The page adds anything of its own (an account row) after this, inside the
/// `<aside class="sidebar" id="sidebar">`, marks its `.app` with [`SIDEBAR_ONLY`], and loads `/viewer.js` after
/// `/shell.js`.
pub fn session_sidebar(name: &str, nav: &str) -> String {
    format!(
        "{head}\n\
         <nav id=\"nav\" aria-label=\"Pages\">\n{nav}</nav>\n{SIDEBAR_RECENT}",
        head = sidebar_head(name),
    )
}

/// The attribute on a page's `.app` that has the viewer's script draw only the sidebar ([`session_sidebar`]) and leave
/// the page and its top bar to the page. Add ` data-viewer-nav="<key>"`, a [`NAV`] key, to mark that row current.
pub const SIDEBAR_ONLY: &str = "data-viewer=\"sidebar\"";

/// Text for an HTML attribute value or element content.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Whether `path` matches a viewer page route.
///
/// Embedders serving the viewer without a `ViewerCore` can use this route-shape check. Fixed page paths match exactly;
/// machine, session and trace routes must have the right number of valid, decoded path segments. Dynamic routes are
/// matched without checking whether their ids exist in the model.
pub fn is_page_path(path: &str) -> bool {
    if matches!(
        path,
        "/" | "/timeline" | "/analytics" | "/sessions" | "/machines"
    ) {
        return true;
    }
    if !(path.starts_with("/machines/") || path.starts_with("/s/") || path.starts_with("/trace/")) {
        return false;
    }

    let Some(parts) = path
        .trim_start_matches('/')
        .split('/')
        .map(crate::viewer::decoded)
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let parts = parts.iter().map(String::as_str).collect::<Vec<_>>();
    matches!(
        parts.as_slice(),
        ["machines", _] | ["s", _, _] | ["trace", _, _, _]
    )
}

/// Font file names used by [`VIEWER_CSS`], as served under `/fonts/`.
pub const FONT_FILES: [&str; 4] = [
    "instrument-sans-latin.woff2",
    "instrument-sans-latin-ext.woff2",
    "jetbrains-mono-latin.woff2",
    "jetbrains-mono-latin-ext.woff2",
];

/// Returns the vendored bytes for a served font file name.
pub fn font(name: &str) -> Option<&'static [u8]> {
    let stem = name.strip_suffix(".woff2")?;
    crate::viewer::FONTS
        .iter()
        .find(|(font, _)| *font == stem)
        .map(|(_, bytes)| *bytes)
}

#[cfg(test)]
mod tests {
    fn has_class(css: &str, class: &str) -> bool {
        let needle = format!(".{class}");
        css.match_indices(&needle).any(|(start, _)| {
            css[start + needle.len()..]
                .chars()
                .next()
                .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '-' && next != '_')
        })
    }

    /// Every class name written as `.name` in `css`, in the same "word" sense as [`has_class`]: a dot followed by a
    /// run of ASCII alphanumerics, `-` or `_`, not itself preceded by one of those characters (so `12.5px` and
    /// `step-num` do not yield a spurious `5px` or split `step`/`num`).
    fn defined_classes(css: &str) -> std::collections::BTreeSet<&str> {
        let mut classes = std::collections::BTreeSet::new();
        for (start, _) in css.match_indices('.') {
            let prev = css[..start].chars().next_back();
            if prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                continue; // part of a longer token (a decimal number, or another class's tail)
            }
            let rest = &css[start + 1..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .unwrap_or(rest.len());
            if end == 0 || rest.as_bytes()[0].is_ascii_digit() {
                continue; // not a class name (empty, or a number like ".5")
            }
            classes.insert(&rest[..end]);
        }
        classes
    }

    #[test]
    fn the_viewer_page_has_the_exported_brand_row() {
        let row = super::brand_row("Semon");
        assert!(
            super::PAGE_HTML.contains(&format!("<div class=\"sidebar-head\">{row}")),
            "viewer.html's .sidebar-head does not start with the exported brand row:\n{row}"
        );
        assert!(super::sidebar_head("Semon").contains(&row));
    }

    /// Markup with the whitespace between tags dropped, which a browser draws alike.
    fn squeezed(html: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        while let Some(at) = rest.find('>') {
            out.push_str(&rest[..=at]);
            rest = &rest[at + 1..];
            let text = rest.trim_start();
            if text.starts_with('<') {
                rest = text;
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn the_viewer_page_has_the_exported_session_sidebar() {
        // The viewer's collapse toggle, after the brand row in its header. An embedding page's sidebar has neither the rail
        // nor the toggle, so the exported header is sidebar_head's; everything after it is the viewer's.
        const RAIL_TOGGLE: &str = "<button class=\"ibtn rail-toggle\" id=\"rail-toggle\" type=\"button\" aria-label=\"Collapse sidebar\" aria-expanded=\"true\"></button>";
        let page = squeezed(super::PAGE_HTML);
        let head = super::sidebar_head("Semon");
        let sidebar = squeezed(&super::session_sidebar("Semon", ""));
        let rest = sidebar
            .strip_prefix(&head)
            .expect("the exported sidebar starts with sidebar_head");
        // The viewer's page has the same navigation and Recent list after its header.
        let (before, after) = rest
            .split_once("<nav id=\"nav\" aria-label=\"Pages\"></nav>")
            .expect("the exported sidebar has its navigation");
        let viewer = format!(
            "<div class=\"sidebar-head\">{}{RAIL_TOGGLE}</div>{before}<div id=\"nav\"></div>{after}",
            super::brand_row("Semon")
        );
        assert!(
            page.contains(&viewer),
            "viewer.html's sidebar is not the exported one:\n{viewer}"
        );
        assert!(
            super::session_sidebar("<a & \"b\">", "")
                .contains("<span class=\"brandname\">&lt;a &amp; &quot;b&quot;&gt;</span>")
        );
        let nav = super::NAV[3].html(super::NAV[3].path, true);
        assert!(super::session_sidebar("Semon", &nav).contains(&format!(
            "<nav id=\"nav\" aria-label=\"Pages\">\n{nav}</nav>"
        )));
    }

    /// The embedding page the browser check opens (tests/ui/shell-sidebar.html) is drawn with this API's markup.
    #[test]
    fn the_embedding_page_check_uses_the_exported_session_sidebar() {
        let page = include_str!("../../../tests/ui/shell-sidebar.html");
        let nav: String = super::NAV
            .iter()
            .map(|link| link.html(link.path, link.key == "machines") + "\n")
            .collect();
        assert!(
            page.contains(&super::session_sidebar("Semon", &nav)),
            "tests/ui/shell-sidebar.html does not draw shell::session_sidebar(\"Semon\", NAV rows):\n{}",
            super::session_sidebar("Semon", &nav)
        );
        assert!(page.contains(&format!(
            "<div class=\"app\" {} data-viewer-nav=\"machines\">",
            super::SIDEBAR_ONLY
        )));
        assert!(page.contains(
            "<script src=\"/shell.js\" defer></script>\n<script src=\"/viewer.js\" defer></script>"
        ));
    }

    #[test]
    fn the_brand_name_and_href_are_escaped() {
        assert!(
            super::sidebar_head("<a & \"b\">")
                .contains("<span class=\"brandname\">&lt;a &amp; &quot;b&quot;&gt;</span>")
        );
        let row = super::NAV[0].html("/x?a=1&b=\"2\"'", true);
        assert!(row.contains("href=\"/x?a=1&amp;b=&quot;2&quot;&#39;\" aria-current=\"page\""));
        assert!(!super::NAV[0].html("/", false).contains("aria-current"));
    }

    /// The viewer's script draws its nav itself; it must draw [`super::NAV`]: the same rows in the same order, each
    /// with the icon path `NAV` gives it.
    #[test]
    fn the_viewer_script_draws_the_exported_nav() {
        let js = include_str!("viewer.js");
        let render = js
            .split("function renderNav()")
            .nth(1)
            .expect("viewer.js has renderNav")
            .split("\n  }\n")
            .next()
            .expect("renderNav's body");
        // The viewer's names for these icons in its `I` table.
        let icon_names = ["home", "sessions", "chart", "machine"];
        let mut last = 0;
        for (link, icon_name) in super::NAV.iter().zip(icon_names) {
            let call = format!("item(\"{}\", \"{}\", I.{icon_name}", link.key, link.label);
            let at = render
                .find(&call)
                .unwrap_or_else(|| panic!("renderNav lacks `{call}`"));
            assert!(
                at >= last,
                "renderNav draws {} out of NAV's order",
                link.key
            );
            last = at;
            let entry = format!("{icon_name}: \"{}\"", link.icon);
            assert!(
                js.contains(&entry),
                "viewer.js's I.{icon_name} is not NAV's {} icon",
                link.key
            );
        }
        assert_eq!(
            render.matches("item(\"").count(),
            super::NAV.len(),
            "renderNav draws a row NAV does not list"
        );
    }

    #[test]
    fn scrollbars_are_soft_in_the_base_stylesheet_that_embedding_pages_load() {
        let viewer_css = include_str!("viewer.css");
        for needle in [
            "--scroll-thumb:",
            "--scroll-thumb-hover:",
            "scrollbar-width: thin",
            "scrollbar-color: var(--scroll-thumb) transparent",
            "::-webkit-scrollbar-thumb",
        ] {
            assert!(viewer_css.contains(needle), "viewer.css lacks `{needle}`");
        }
    }

    #[test]
    fn shell_classes_do_not_collide_with_viewer_classes() {
        // Chrome classes the shell's contract deliberately shares with the viewer; everything else shell.css
        // defines must be its own name so cascading shell.css after viewer.css never inherits unrelated rules
        // (as bare .steps/.step once did, leaking the transcript's rail into the shell's steps component).
        const SHARED_CHROME: [&str; 20] = [
            "app",
            "sidebar",
            "sidebar-head",
            "brandrow",
            "brandname",
            "mark",
            "scrim",
            "main",
            "topbar",
            "ttl",
            "ibtn",
            "lead",
            "nav-item",
            "icon",
            "page",
            "ph",
            "sec-h",
            "list",
            "empty",
            "dot",
        ];
        let viewer_classes = defined_classes(include_str!("viewer.css"));
        let shell_classes = defined_classes(concat!(
            include_str!("shell.css"),
            include_str!("select.css")
        ));
        for class in &shell_classes {
            if SHARED_CHROME.contains(class) {
                continue;
            }
            assert!(
                !viewer_classes.contains(class),
                "shell.css class .{class} is also defined by viewer.css; rename the shell.css one (e.g. with an sh- prefix)"
            );
        }
    }

    #[test]
    fn documented_classes_are_defined_by_the_stylesheets() {
        const VIEWER_CLASSES: [&str; 20] = [
            "app",
            "sidebar",
            "sidebar-head",
            "brandrow",
            "brandname",
            "mark",
            "scrim",
            "main",
            "topbar",
            "ttl",
            "ibtn",
            "lead",
            "nav-item",
            "icon",
            "page",
            "ph",
            "sec-h",
            "list",
            "empty",
            "dot",
        ];
        let contract = include_str!("../../../docs/shell.md");
        let classes = contract
            .split("## Classes\n")
            .nth(1)
            .expect("Classes section")
            .split("```")
            .nth(1)
            .expect("class list fence");
        let viewer_css = include_str!("viewer.css");
        let shell_css = concat!(include_str!("shell.css"), include_str!("select.css"));
        for class in classes
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let css = if VIEWER_CLASSES.contains(&class) {
                viewer_css
            } else {
                shell_css
            };
            assert!(
                has_class(css, class),
                "documented class .{class} has no rule in its stylesheet"
            );
        }
    }
}
