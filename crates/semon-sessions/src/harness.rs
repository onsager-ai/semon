//! Names and official artwork for the harnesses Semon recognizes.
//!
//! These icons identify only the source harness for a session. Their artwork
//! is included unmodified and is never recoloured; theme contrast comes from
//! the surrounding UI.

/// One harness Semon reads logs from, or knows how to name.
pub struct HarnessDefinition {
    /// The id in the model's `harness` field ("claude", "codex", "opencode").
    pub id: &'static str,
    pub name: &'static str,
    pub short: &'static str,
    /// Served path for light themes.
    pub icon_light: &'static str,
    /// Served path for dark themes.
    pub icon_dark: &'static str,
}

/// Harnesses Semon knows how to name and their theme-specific icon paths.
pub const HARNESSES: &[HarnessDefinition] = &[
    HarnessDefinition {
        id: "claude",
        name: "Claude Code",
        short: "Claude",
        icon_light: "/harness/claude-code.svg",
        icon_dark: "/harness/claude-code.svg",
    },
    HarnessDefinition {
        id: "codex",
        name: "Codex",
        short: "Codex",
        icon_light: "/harness/codex-black.svg",
        icon_dark: "/harness/codex.svg",
    },
    HarnessDefinition {
        id: "opencode",
        name: "OpenCode",
        short: "OpenCode",
        icon_light: "/harness/opencode-light.svg",
        icon_dark: "/harness/opencode-dark.svg",
    },
];

/// Served path → the unmodified SVG text.
pub const HARNESS_ICONS: &[(&str, &str)] = &[
    (
        "/harness/claude-code.svg",
        include_str!("../../../assets/harnesses/claude-code.svg"),
    ),
    (
        "/harness/codex.svg",
        include_str!("../../../assets/harnesses/codex.svg"),
    ),
    (
        "/harness/codex-black.svg",
        include_str!("../../../assets/harnesses/codex-black.svg"),
    ),
    (
        "/harness/opencode-light.svg",
        include_str!("../../../assets/harnesses/opencode-light.svg"),
    ),
    (
        "/harness/opencode-dark.svg",
        include_str!("../../../assets/harnesses/opencode-dark.svg"),
    ),
];

/// Look up the definition for a model harness id.
pub fn harness(id: &str) -> Option<&'static HarnessDefinition> {
    HARNESSES.iter().find(|definition| definition.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_definitions_and_icons_are_consistent() {
        for definition in HARNESSES {
            assert_eq!(
                harness(definition.id).map(|found| found.id),
                Some(definition.id)
            );
            for path in [definition.icon_light, definition.icon_dark] {
                assert!(
                    HARNESS_ICONS
                        .iter()
                        .any(|(served_path, _)| *served_path == path),
                    "{} references an unserved icon path {path}",
                    definition.id
                );
            }
        }
        assert!(harness("unknown").is_none());
    }

    #[test]
    fn every_input_root_harness_has_a_definition() {
        for input_root in [
            crate::inputs::InputRoot::Claude,
            crate::inputs::InputRoot::Codex,
        ] {
            let id = match input_root {
                crate::inputs::InputRoot::Claude => "claude",
                crate::inputs::InputRoot::Codex => "codex",
            };
            assert_eq!(crate::inputs::InputRoot::parse(id), Some(input_root));
            assert!(
                harness(id).is_some(),
                "input root {id} has no harness definition"
            );
        }
    }

    #[test]
    fn javascript_registry_mirrors_rust_registry() {
        let js = include_str!("viewer.js");
        let registry_lines: Vec<&str> = js
            .lines()
            .filter(|line| line.starts_with("  const HARNESSES = {"))
            .collect();
        assert_eq!(registry_lines.len(), 1);
        let line = registry_lines[0];
        assert_eq!(line.matches(": { name: \"").count(), HARNESSES.len());

        for (index, definition) in HARNESSES.iter().enumerate() {
            let start_marker = format!("{}: {{", definition.id);
            let start = line
                .find(&start_marker)
                .unwrap_or_else(|| panic!("viewer.js is missing harness {}", definition.id));
            let end = HARNESSES
                .get(index + 1)
                .and_then(|next| line[start..].find(&format!("{}: {{", next.id)))
                .map_or(line.len(), |offset| start + offset);
            let entry = &line[start..end];
            let mut cursor = 0;
            for needle in [
                start_marker,
                format!("name: \"{}\"", definition.name),
                format!("short: \"{}\"", definition.short),
                format!("light: \"{}\"", definition.icon_light),
                format!("dark: \"{}\"", definition.icon_dark),
            ] {
                let offset = entry[cursor..].find(&needle).unwrap_or_else(|| {
                    panic!(
                        "viewer.js harness {} is missing `{needle}` in the expected order",
                        definition.id
                    )
                });
                cursor += offset + needle.len();
            }
        }
    }
}
