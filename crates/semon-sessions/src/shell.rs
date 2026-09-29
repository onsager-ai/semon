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
pub const FONT_FILES: [&str; 6] = [
    "instrument-sans-latin.woff2",
    "instrument-sans-latin-ext.woff2",
    "jetbrains-mono-latin.woff2",
    "jetbrains-mono-latin-ext.woff2",
    "source-serif-4-latin.woff2",
    "source-serif-4-latin-ext.woff2",
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
        const SHARED_CHROME: [&str; 17] = [
            "app", "sidebar", "brandrow", "mark", "scrim", "main", "topbar", "ttl", "ibtn", "lead",
            "nav-item", "page", "ph", "sec-h", "list", "empty", "dot",
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
        const VIEWER_CLASSES: [&str; 17] = [
            "app", "sidebar", "brandrow", "mark", "scrim", "main", "topbar", "ttl", "ibtn", "lead",
            "nav-item", "page", "ph", "sec-h", "list", "empty", "dot",
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
