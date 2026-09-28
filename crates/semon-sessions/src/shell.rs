//! Public assets for pages that share the viewer's presentation shell.
//!
//! Serve these assets from the page's own origin at `/viewer.css`, `/shell.css`, and `/shell.js`, and serve each font at
//! `/fonts/<name>`; the viewer's content security policy permits same-origin assets only.

/// The viewer's base styles, including its tokens, fonts, and chrome.
pub const VIEWER_CSS: &str = include_str!("viewer.css");

/// Additional components for forms and other server-rendered pages.
pub const CSS: &str = include_str!("shell.css");

/// The drawer, copy, dialog, and readiness-poll behavior for shell pages.
pub const JS: &str = include_str!("shell.js");

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
    fn shell_classes_do_not_collide_with_viewer_classes() {
        // Chrome classes the shell's contract deliberately shares with the viewer; everything else shell.css
        // defines must be its own name so cascading shell.css after viewer.css never inherits unrelated rules
        // (as bare .steps/.step once did, leaking the transcript's rail into the shell's steps component).
        const SHARED_CHROME: [&str; 17] = [
            "app", "sidebar", "brandrow", "mark", "scrim", "main", "topbar", "ttl", "ibtn", "lead",
            "nav-item", "page", "ph", "sec-h", "list", "empty", "dot",
        ];
        let viewer_classes = defined_classes(include_str!("viewer.css"));
        let shell_classes = defined_classes(include_str!("shell.css"));
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
        let shell_css = include_str!("shell.css");
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
