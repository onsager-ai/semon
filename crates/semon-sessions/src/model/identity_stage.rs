//! Native identity kernels with resumable, content-free observations.
//! Storage owns each union parent and canonical-key occupancy observation.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(crate) mod physical;

const BRIDGE_PREFIX: &str = "bridge:";
fn atom_prefix(kind: &str) -> Result<&'static str, &'static str> {
    match kind {
        "session" | "continued" => Ok(""),
        "bridge" => Ok(BRIDGE_PREFIX),
        _ => Err("unsupported native identity link kind"),
    }
}

/// The same raw-namespace emitter, for native producers that already own a
/// parsed string or an equivalent qualified decoded stream. Parts may split
/// UTF-8 scalars; raw UTF-8 byte order is exactly native String order.
pub(crate) fn link_atom_part(
    kind: &str,
    native: &str,
    offset: usize,
    limit: usize,
) -> Result<(Vec<u8>, bool), &'static str> {
    if limit == 0 || limit > 64 * 1024 {
        return Err("invalid native atom part budget");
    }
    let prefix = atom_prefix(kind)?.as_bytes();
    let length = prefix
        .len()
        .checked_add(native.len())
        .ok_or("native atom length overflow")?;
    if offset > length {
        return Err("native atom offset exceeds length");
    }
    let end = offset.saturating_add(limit).min(length);
    let mut part = Vec::with_capacity(end - offset);
    if offset < prefix.len() {
        part.extend_from_slice(&prefix[offset..end.min(prefix.len())]);
    }
    if end > prefix.len() {
        part.extend_from_slice(
            &native.as_bytes()[offset.saturating_sub(prefix.len())..end - prefix.len()],
        );
    }
    Ok((part, end == length))
}

/// The native file comparator is shared with metadata-only physical identity.
/// Its activity minimum is distinct from the first regular-event time seed.
pub(crate) fn source_order(
    first: Option<i64>,
    continued_empty: bool,
    id: &str,
) -> (i64, bool, &str) {
    (first.unwrap_or(i64::MAX), continued_empty, id)
}

/// Claude lineage atoms occupy one raw string namespace. A literal native
/// `bridge:x` therefore intersects the bridge record `x`; phantom atoms with
/// no physical source still connect eligible stars.
pub(crate) fn link_atom<'a>(
    kind: &str,
    native: &'a str,
) -> Result<std::borrow::Cow<'a, str>, &'static str> {
    match atom_prefix(kind)? {
        "" => Ok(std::borrow::Cow::Borrowed(native)),
        prefix => Ok(std::borrow::Cow::Owned(format!("{prefix}{native}"))),
    }
}

pub(crate) fn star_atoms<'a>(
    id: &'a str,
    session_ids: impl Iterator<Item = &'a String> + 'a,
    continued_in: impl Iterator<Item = &'a String> + 'a,
    bridges: impl Iterator<Item = &'a String> + 'a,
) -> impl Iterator<Item = std::borrow::Cow<'a, str>> + 'a {
    std::iter::once(std::borrow::Cow::Borrowed(id))
        .chain(session_ids.map(|id| std::borrow::Cow::Borrowed(id.as_str())))
        .chain(continued_in.map(|id| std::borrow::Cow::Borrowed(id.as_str())))
        .chain(bridges.map(|id| link_atom("bridge", id).expect("native bridge atom kind")))
}

pub(crate) trait UnionParents {
    type Error;
    fn parent(&mut self, key: &str) -> Result<Option<String>, Self::Error>;
    fn compress(&mut self, key: &str, root: &str) -> Result<(), Self::Error>;
}

impl UnionParents for HashMap<String, String> {
    type Error = std::convert::Infallible;
    fn parent(&mut self, key: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.get(key).cloned())
    }
    fn compress(&mut self, key: &str, root: &str) -> Result<(), Self::Error> {
        self.insert(key.to_owned(), root.to_owned());
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnionCursor {
    key: String,
    current: String,
    root: Option<String>,
}

impl UnionCursor {
    pub(crate) fn new(key: &str) -> Self {
        Self {
            key: key.to_owned(),
            current: key.to_owned(),
            root: None,
        }
    }

    /// One indexed parent read and at most one path-compression write. The
    /// producer checkpoints this cursor with the corresponding parent write.
    pub(crate) fn step<P: UnionParents>(
        &mut self,
        parents: &mut P,
    ) -> Result<Option<String>, P::Error> {
        let next = parents.parent(&self.current)?;
        if let Some(root) = &self.root {
            if let Some(next) = next
                && next != *root
            {
                parents.compress(&self.current, root)?;
                self.current = next;
                return Ok(None);
            }
            return Ok(Some(root.clone()));
        }
        if let Some(next) = next
            && next != self.current
        {
            self.current = next;
        } else {
            self.root = Some(self.current.clone());
            self.current.clone_from(&self.key);
        }
        Ok(None)
    }
}

/// Canonical collision spelling is allocated against all eligible native IDs
/// and already allocated native owners. Only collisions add a harness prefix.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyCursor {
    candidate: String,
    qualified: bool,
}

impl KeyCursor {
    pub(crate) fn new(native: &str, harness: &str, collision: bool) -> Self {
        Self {
            candidate: if collision {
                format!("{harness}:{native}")
            } else {
                native.to_owned()
            },
            qualified: collision,
        }
    }

    /// One indexed occupancy probe. Literal colon-prefixed native IDs keep
    /// their identity; a colliding owner advances only its own candidate.
    pub(crate) fn step(&mut self, occupied: impl FnOnce(&str) -> bool) -> Option<String> {
        self.try_step(|key| Ok::<_, std::convert::Infallible>(occupied(key)))
            .expect("infallible native occupancy")
    }

    pub(crate) fn try_step<E>(
        &mut self,
        occupied: impl FnOnce(&str) -> Result<bool, E>,
    ) -> Result<Option<String>, E> {
        if !self.qualified || !occupied(&self.candidate)? {
            return Ok(Some(self.candidate.clone()));
        }
        self.candidate.insert(0, ':');
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_atom_emission_is_the_complete_native_raw_namespace() {
        let native = format!("{}🦀end", "x".repeat(65531));
        for kind in ["session", "continued", "bridge"] {
            let expected = link_atom(kind, &native).unwrap();
            let mut actual = Vec::new();
            let mut offset = 0;
            loop {
                let (part, last) = link_atom_part(kind, &native, offset, 65536).unwrap();
                assert!(part.len() <= 65536);
                offset += part.len();
                actual.extend(part);
                if last {
                    break;
                }
            }
            assert_eq!(actual, expected.as_bytes());
        }
    }

    #[test]
    fn native_star_atoms_share_literal_and_phantom_namespace() {
        let session = vec!["phantom".to_owned(), "bridge:literal".to_owned()];
        let continued = vec!["phantom".to_owned()];
        let bridges = vec!["literal".to_owned(), "bridge:nested".to_owned()];
        let atoms = star_atoms("owner", session.iter(), continued.iter(), bridges.iter())
            .map(std::borrow::Cow::into_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            atoms,
            [
                "owner",
                "phantom",
                "bridge:literal",
                "phantom",
                "bridge:literal",
                "bridge:bridge:nested"
            ]
        );
        assert_eq!(
            link_atom("session", "bridge:literal").unwrap(),
            link_atom("bridge", "literal").unwrap()
        );
        assert_eq!(link_atom("continued", "phantom").unwrap(), "phantom");
        assert!(link_atom("invented", "x").is_err());
        let other = vec!["phantom".to_owned()];
        assert!(
            star_atoms("other", other.iter(), [].iter(), [].iter())
                .any(|atom| atoms.iter().any(|value| value == atom.as_ref()))
        );
    }

    #[test]
    fn native_union_cursor_resumes_without_retaining_ancestry() {
        let mut parents = HashMap::new();
        for i in 1..1024 {
            parents.insert(format!("{i:04}"), format!("{:04}", i - 1));
        }
        let mut cursor = UnionCursor::new("1023");
        let mut work = 0;
        let root = loop {
            work += 1;
            let found = cursor.step(&mut parents).unwrap();
            if work % 64 == 0 {
                cursor = serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
            }
            assert!(serde_json::to_string(&cursor).unwrap().len() < 128);
            if let Some(root) = found {
                break root;
            }
        };
        assert_eq!(root, "0000");
        assert_eq!(parents["1023"], "0000");
        assert_eq!(parents["0001"], "0000");
    }

    #[test]
    fn native_key_cursor_preserves_literal_qualifier_occupants() {
        let names = ["claude:same", ":claude:same"];
        let mut cursor = KeyCursor::new("same", "claude", true);
        assert_eq!(cursor.step(|key| names.contains(&key)), None);
        let mut cursor: KeyCursor =
            serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        assert_eq!(cursor.step(|key| names.contains(&key)), None);
        assert_eq!(
            cursor.step(|key| names.contains(&key)),
            Some("::claude:same".into())
        );
        let mut literal = KeyCursor::new("claude:same", "claude", false);
        assert_eq!(
            literal.step(|_| panic!("unambiguous literal probed collision namespace")),
            Some("claude:same".into())
        );
    }
}
