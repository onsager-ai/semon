//! Native identity kernels with resumable, content-free observations.
//! Storage owns each union parent and canonical-key occupancy observation.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The native file comparator is shared with metadata-only physical identity.
/// Its activity minimum is distinct from the first regular-event time seed.
pub(crate) fn source_order(
    first: Option<i64>,
    continued_empty: bool,
    id: &str,
) -> (i64, bool, &str) {
    (first.unwrap_or(i64::MAX), continued_empty, id)
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
