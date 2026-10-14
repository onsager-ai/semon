//! Physical-window identity uses the native kernels and published star
//! intersections. A profile is evidence, never source or custody authority.
use super::{KeyCursor, UnionCursor, source_order};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ProfileRoot(pub(crate) [u8; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PhysicalRole {
    Top,
    Agent,
    Codex,
    Copilot,
}
impl PhysicalRole {
    pub(crate) fn harness(self) -> &'static str {
        match self {
            Self::Top | Self::Agent => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
        }
    }
    pub(crate) fn recorded(source: &super::super::summary::CatalogSource) -> Option<Self> {
        let role: super::super::Role =
            serde_json::from_value(source.native_role.as_ref()?.clone()).ok()?;
        Some(match role {
            super::super::Role::Top { .. } => Self::Top,
            super::super::Role::Agent(_) => Self::Agent,
            super::super::Role::Codex(_) => Self::Codex,
            super::super::Role::Copilot => Self::Copilot,
        })
    }
}

/// Native scan passes append Claude, Codex and Copilot. Within each pass the
/// scanner sorts PathBuf values; a late path does not renumber older sources.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct NativeScanOrder {
    pub(crate) pass: u8,
    pub(crate) path: PathBuf,
}
impl NativeScanOrder {
    pub(crate) fn new(role: PhysicalRole, path: &Path) -> Self {
        Self {
            pass: match role {
                PhysicalRole::Top | PhysicalRole::Agent => 0,
                PhysicalRole::Codex => 1,
                PhysicalRole::Copilot => 2,
            },
            path: path.to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PhysicalSource {
    pub(crate) profile: ProfileRoot,
    pub(crate) native_id: String,
    pub(crate) role: PhysicalRole,
    pub(crate) scan_order: NativeScanOrder,
    pub(crate) first: Option<i64>,
    pub(crate) continued_empty: bool,
    /// Only session_id observations targeting an actually skipped Top filename.
    pub(crate) turns_truncated: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PhysicalComponent {
    pub(crate) key: String,
    pub(crate) harness: &'static str,
    /// Positions in the supplied source context, in actual native file order.
    pub(crate) sources: Vec<usize>,
    pub(crate) turns_truncated: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct PhysicalSelection {
    pub(crate) components: Vec<PhysicalComponent>,
    /// Matches Builder.by_native's allocation order, including overwritten IDs.
    pub(crate) aliases: BTreeMap<(String, String), usize>,
}

fn root(parents: &mut HashMap<String, String>, key: &str) -> String {
    let mut cursor = UnionCursor::new(key);
    loop {
        if let Some(root) = cursor.step(parents).expect("infallible native union") {
            return root;
        }
    }
}

/// Only admitted Top stars participate. The caller supplies completed pair
/// observations and an exact comparison of opaque per-star raw minima. Raw
/// strings can exceed a page; this API never materializes those strings.
pub(crate) fn select<E>(
    sources: &[PhysicalSource],
    mut adjacent: impl FnMut(ProfileRoot, ProfileRoot) -> Result<bool, E>,
    mut minimum_order: impl FnMut(ProfileRoot, ProfileRoot) -> Result<std::cmp::Ordering, E>,
) -> Result<PhysicalSelection, E> {
    let mut order: Vec<_> = (0..sources.len()).collect();
    order.sort_by(|a, b| sources[*a].scan_order.cmp(&sources[*b].scan_order));
    let top: Vec<_> = order
        .iter()
        .copied()
        .filter(|i| sources[*i].role == PhysicalRole::Top)
        .collect();
    let mut parents = HashMap::new();
    for (position, &a) in top.iter().enumerate() {
        for &b in &top[..position] {
            if adjacent(sources[a].profile, sources[b].profile)? {
                let a = root(&mut parents, &sources[a].native_id);
                let b = root(&mut parents, &sources[b].native_id);
                if a != b {
                    let (low, high) = if a < b { (a, b) } else { (b, a) };
                    parents.insert(high, low);
                }
            }
        }
    }
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for &i in &top {
        groups
            .entry(root(&mut parents, &sources[i].native_id))
            .or_default()
            .push(i);
    }
    let mut lineages: Vec<(ProfileRoot, Vec<usize>)> = Vec::new();
    for mut group in groups.into_values() {
        let mut minimum = sources[group[0]].profile;
        for &i in &group[1..] {
            if minimum_order(sources[i].profile, minimum)?.is_lt() {
                minimum = sources[i].profile;
            }
        }
        group.sort_by(|a, b| {
            let (a, b) = (&sources[*a], &sources[*b]);
            source_order(a.first, a.continued_empty, &a.native_id).cmp(&source_order(
                b.first,
                b.continued_empty,
                &b.native_id,
            ))
        });
        // Fallible insertion sort preserves native minimum-raw component order
        // without collecting raw atoms or concealing comparison errors.
        let mut position = lineages.len();
        for (i, (other, _)) in lineages.iter().enumerate() {
            if minimum_order(minimum, *other)?.is_lt() {
                position = i;
                break;
            }
        }
        lineages.insert(position, (minimum, group));
    }
    let mut groups: Vec<Vec<usize>> = lineages.into_iter().map(|(_, group)| group).collect();
    groups.extend(
        order
            .into_iter()
            .filter(|i| sources[*i].role != PhysicalRole::Top)
            .map(|i| vec![i]),
    );
    let mut native = BTreeMap::<&str, BTreeSet<&str>>::new();
    for source in sources {
        native
            .entry(&source.native_id)
            .or_default()
            .insert(source.role.harness());
    }
    let mut allocated = BTreeSet::<String>::new();
    let mut components = Vec::new();
    let mut aliases = BTreeMap::new();
    for group in groups {
        let first = &sources[group[0]];
        let harness = first.role.harness();
        let collision = native[&*first.native_id]
            .iter()
            .any(|other| *other != harness);
        let mut key = KeyCursor::new(&first.native_id, harness, collision);
        let key = loop {
            if let Some(key) = key.step(|key| native.contains_key(key) || allocated.contains(key)) {
                break key;
            }
        };
        allocated.insert(key.clone());
        let component = components.len();
        for &i in &group {
            aliases.insert(
                (harness.to_owned(), sources[i].native_id.clone()),
                component,
            );
        }
        components.push(PhysicalComponent {
            key,
            harness,
            turns_truncated: group.iter().any(|i| sources[*i].turns_truncated),
            sources: group,
        });
    }
    Ok(PhysicalSelection {
        components,
        aliases,
    })
}

/// Exactly the complete history scanner's skipped-Top filename rule.
pub(crate) fn skipped_top_id(path: &Path, claude_home: &Path) -> Option<String> {
    (path.parent()?.parent()? == claude_home.join("projects"))
        .then(|| path.file_stem()?.to_str().map(str::to_owned))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model as native;
    use std::sync::Arc;

    fn file(
        id: &str,
        role: native::Role,
        first: Option<i64>,
        session: &[&str],
        continued: &[&str],
        bridge: &[&str],
    ) -> native::SourceFile {
        let mut summary = crate::events::FileIndex::default();
        summary.links.session_ids = session.iter().map(|v| (*v).into()).collect();
        summary.links.continued_in = continued.iter().map(|v| (*v).into()).collect();
        summary.links.bridges = bridge.iter().map(|v| (*v).into()).collect();
        native::SourceFile {
            revision: (0, 0, 0, [0; 32], [0; 32], None),
            path: PathBuf::from(format!("/native/{id}.jsonl")),
            stamp: native::Stamp::default(),
            id: id.into(),
            role,
            summary: Arc::new(summary),
            first,
            last: first,
            marker: None,
        }
    }
    fn top() -> native::Role {
        native::Role::Top { slug: "p".into() }
    }
    fn atoms(file: &native::SourceFile) -> BTreeSet<String> {
        // Independent complete native raw-star reference. Deliberately does
        // not call the new profile projection or the shared part emitter.
        std::iter::once(file.id.clone())
            .chain(file.summary.links.session_ids.iter().cloned())
            .chain(file.summary.links.continued_in.iter().cloned())
            .chain(
                file.summary
                    .links
                    .bridges
                    .iter()
                    .map(|v| format!("bridge:{v}")),
            )
            .collect()
    }
    fn check(files: &[native::SourceFile], skipped: &BTreeSet<String>) {
        let stars: Vec<_> = files.iter().map(atoms).collect();
        let sources: Vec<_> = files
            .iter()
            .enumerate()
            .map(|(i, file)| {
                let role = match file.role {
                    native::Role::Top { .. } => PhysicalRole::Top,
                    native::Role::Agent(_) => PhysicalRole::Agent,
                    native::Role::Codex(_) => PhysicalRole::Codex,
                    native::Role::Copilot => PhysicalRole::Copilot,
                };
                let mut profile = [0u8; 32];
                profile[..8].copy_from_slice(&(i as u64).to_be_bytes());
                PhysicalSource {
                    profile: ProfileRoot(profile),
                    native_id: file.id.clone(),
                    role,
                    scan_order: NativeScanOrder::new(role, &file.path),
                    first: file.first,
                    continued_empty: file.summary.links.continued_in.is_empty(),
                    turns_truncated: file
                        .summary
                        .links
                        .session_ids
                        .iter()
                        .any(|id| skipped.contains(id)),
                }
            })
            .collect();
        let index = |root: ProfileRoot| {
            usize::try_from(u64::from_be_bytes(root.0[..8].try_into().unwrap())).unwrap()
        };
        let selected = select::<std::convert::Infallible>(
            &sources,
            |a, b| Ok(!stars[index(a)].is_disjoint(&stars[index(b)])),
            |a, b| Ok(stars[index(a)].first().cmp(&stars[index(b)].first())),
        )
        .unwrap();
        let options = crate::Options::default();
        let source = crate::query_source::Sources::capture(&options, std::iter::empty()).unwrap();
        let records = source.reader();
        let mut texts = native::Texts::default();
        let mut cache = crate::events::EventCache::default();
        let facts = crate::facts::MachineFacts::Local;
        let mut builder = native::Builder::new(
            (files, &records),
            &mut texts,
            0,
            String::new(),
            &facts,
            &[],
            &mut cache,
        );
        builder.sessions(native::lineages(files), &[], &BTreeSet::new());
        assert_eq!(selected.components.len(), builder.sessions.len());
        for (actual, expected) in selected.components.iter().zip(&builder.sessions) {
            assert_eq!(actual.key, expected.key);
            assert_eq!(actual.sources, expected.files);
            assert_eq!(actual.harness, expected.out.harness);
            assert_eq!(
                actual.turns_truncated,
                expected.files.iter().any(|i| files[*i]
                    .summary
                    .links
                    .session_ids
                    .iter()
                    .any(|id| skipped.contains(id)))
            );
        }
        for ((harness, id), owner) in &builder.by_native {
            assert_eq!(selected.aliases[&(harness.to_string(), id.clone())], *owner);
        }
    }

    #[test]
    fn every_omitted_star_subset_matches_complete_lineages_and_builder_keys() {
        // B is a physical connector. Shared phantoms and raw bridge strings
        // remain eligible intersections even when their physical files vanish.
        for mask in 0u8..16 {
            let candidates = [
                file("a", top(), Some(9), &["b", "phantom"], &[], &[]),
                file("b", top(), Some(3), &["c"], &["later"], &[]),
                file(
                    "c",
                    top(),
                    Some(3),
                    &["phantom", "bridge:literal"],
                    &[],
                    &[],
                ),
                file("z", top(), None, &[], &[], &["literal"]),
            ];
            let files: Vec<_> = candidates
                .into_iter()
                .enumerate()
                .filter_map(|(i, f)| ((mask & (1 << i)) != 0).then_some(f))
                .collect();
            check(&files, &BTreeSet::from(["b".into(), "later".into()]));
        }
        // Removing only the connector cannot preserve its c leaf in a's star.
        check(
            &[
                file("a", top(), Some(0), &["b"], &[], &[]),
                file("c", top(), Some(0), &[], &[], &[]),
            ],
            &BTreeSet::new(),
        );
    }

    #[test]
    fn all_role_occupants_and_raw_minimum_allocation_match_native_passes() {
        let mut files = vec![
            file("same", top(), Some(5), &["00-phantom"], &[], &[]),
            file("first", top(), Some(-1), &[], &[], &[]),
            file(
                "claude:same",
                native::Role::Agent(native::AgentMeta::default()),
                None,
                &[],
                &[],
                &[],
            ),
            file(
                "same",
                native::Role::Codex(native::CodexMeta::default()),
                None,
                &[],
                &[],
                &[],
            ),
            file(":claude:same", native::Role::Copilot, None, &[], &[], &[]),
        ];
        files.sort_by(|a, b| {
            NativeScanOrder::new(
                match a.role {
                    native::Role::Top { .. } => PhysicalRole::Top,
                    native::Role::Agent(_) => PhysicalRole::Agent,
                    native::Role::Codex(_) => PhysicalRole::Codex,
                    native::Role::Copilot => PhysicalRole::Copilot,
                },
                &a.path,
            )
            .cmp(&NativeScanOrder::new(
                match b.role {
                    native::Role::Top { .. } => PhysicalRole::Top,
                    native::Role::Agent(_) => PhysicalRole::Agent,
                    native::Role::Codex(_) => PhysicalRole::Codex,
                    native::Role::Copilot => PhysicalRole::Copilot,
                },
                &b.path,
            ))
        });
        check(&files, &BTreeSet::new());
        // Backwards activity and continuation ties change the canonical alias.
        for (first, continued) in [(Some(-10), false), (Some(5), true), (None, false)] {
            check(
                &[
                    file("a", top(), Some(5), &["b"], &[], &[]),
                    file(
                        "b",
                        top(),
                        first,
                        &[],
                        if continued { &["tail"] } else { &[] },
                        &[],
                    ),
                ],
                &BTreeSet::from(["tail".into()]),
            );
        }
    }

    #[test]
    fn skipped_top_shape_matches_native_history_rule() {
        let home = Path::new("/claude");
        assert_eq!(
            skipped_top_id(Path::new("/claude/projects/p/x.jsonl"), home),
            Some("x".into())
        );
        assert_eq!(
            skipped_top_id(Path::new("/claude/projects/p/subagents/x.jsonl"), home),
            None
        );
        assert_eq!(
            skipped_top_id(Path::new("/other/projects/p/x.jsonl"), home),
            None
        );
    }
}
