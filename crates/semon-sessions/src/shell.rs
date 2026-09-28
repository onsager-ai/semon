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
