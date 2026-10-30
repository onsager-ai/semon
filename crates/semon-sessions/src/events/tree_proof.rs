//! Persistent authenticated Tree capture. Nodes are immutable and addressed by
//! their complete encoded content. Updating a leaf rewrites only its crit-bit
//! path; no sorted dimension collection or generation-wide copy is required.
//!
//! This capture is not yet the authority of the Tree scalar codec. Consumers
//! must retain the current witness check until deferred payload verification is
//! connected to the captured scalar on both engines.
use sha2::{Digest, Sha256};

pub(crate) type Hash = [u8; 32];
pub(crate) const EMPTY: Hash = [0; 32];
pub(crate) const MAX_NODE_BYTES: usize = 67;

pub(crate) trait Store {
    type Error;
    fn invalid() -> Self::Error;
    fn get(&mut self, hash: &Hash) -> Result<Option<Vec<u8>>, Self::Error>;
    fn put(&mut self, hash: &Hash, encoded: &[u8]) -> Result<(), Self::Error>;
}

enum Node {
    Leaf(Hash, Hash),
    Branch(u16, Hash, Hash),
}

fn hash(encoded: &[u8]) -> Hash {
    let mut digest = Sha256::new();
    digest.update(b"semon.tree.capture.1.node\0");
    digest.update(encoded);
    digest.finalize().into()
}

pub(crate) fn key(family: &str, key: &[u8]) -> Hash {
    let mut digest = Sha256::new();
    digest.update(b"semon.tree.capture.1.key\0");
    digest.update((family.len() as u64).to_be_bytes());
    digest.update(family.as_bytes());
    digest.update((key.len() as u64).to_be_bytes());
    digest.update(key);
    digest.finalize().into()
}

/// Full canonical row bytes (including identity, NULL and closed state) are
/// framed separately from the address. No truncation or lossy normalization.
pub(crate) fn value(encoded: &[u8]) -> Hash {
    let mut digest = Sha256::new();
    digest.update(b"semon.tree.capture.1.value\0");
    digest.update((encoded.len() as u64).to_be_bytes());
    digest.update(encoded);
    digest.finalize().into()
}

fn bit(key: &Hash, at: u16) -> bool {
    key[usize::from(at / 8)] & (128 >> (at % 8)) != 0
}

fn load<S: Store>(store: &mut S, expected: &Hash) -> Result<Node, S::Error> {
    let encoded = store.get(expected)?.ok_or_else(S::invalid)?;
    if encoded.len() > MAX_NODE_BYTES || hash(&encoded) != *expected {
        return Err(S::invalid());
    }
    match encoded.first() {
        Some(0) if encoded.len() == 65 => Ok(Node::Leaf(
            encoded[1..33].try_into().map_err(|_| S::invalid())?,
            encoded[33..65].try_into().map_err(|_| S::invalid())?,
        )),
        Some(1) if encoded.len() == 67 => {
            let at = u16::from_be_bytes([encoded[1], encoded[2]]);
            let left = encoded[3..35].try_into().map_err(|_| S::invalid())?;
            let right = encoded[35..67].try_into().map_err(|_| S::invalid())?;
            if at >= 256 || left == EMPTY || right == EMPTY || left == right {
                return Err(S::invalid());
            }
            Ok(Node::Branch(at, left, right))
        }
        _ => Err(S::invalid()),
    }
}

fn save<S: Store>(store: &mut S, node: Node) -> Result<Hash, S::Error> {
    let mut bytes = Vec::with_capacity(MAX_NODE_BYTES);
    match node {
        Node::Leaf(key, value) => {
            bytes.push(0);
            bytes.extend_from_slice(&key);
            bytes.extend_from_slice(&value);
        }
        Node::Branch(at, left, right) => {
            bytes.push(1);
            bytes.extend_from_slice(&at.to_be_bytes());
            bytes.extend_from_slice(&left);
            bytes.extend_from_slice(&right);
        }
    }
    let digest = hash(&bytes);
    store.put(&digest, &bytes)?;
    Ok(digest)
}

/// Authenticate inclusion or absence. Every visited node is checked before
/// interpretation; strictly increasing bit positions bound traversal to 257
/// fixed-size nodes even for adversarial keys or corrupt cyclic storage.
pub(crate) fn lookup<S: Store>(
    store: &mut S,
    root: Hash,
    key: Hash,
) -> Result<Option<Hash>, S::Error> {
    if root == EMPTY {
        return Ok(None);
    }
    let mut cursor = root;
    let mut previous = None;
    loop {
        match load(store, &cursor)? {
            Node::Leaf(actual, value) => return Ok((actual == key).then_some(value)),
            Node::Branch(at, left, right) => {
                if previous.is_some_and(|p| at <= p) {
                    return Err(S::invalid());
                }
                previous = Some(at);
                cursor = if bit(&key, at) { right } else { left };
            }
        }
    }
}

/// Return a new immutable root. Unchanged values cause no node writes.
/// The caller publishes this root atomically with its actual physical rows.
pub(crate) fn update<S: Store>(
    store: &mut S,
    root: Hash,
    key: Hash,
    value: Hash,
) -> Result<Hash, S::Error> {
    if root == EMPTY {
        return save(store, Node::Leaf(key, value));
    }
    let mut path = Vec::new();
    let mut cursor = root;
    let (old_key, old_value) = loop {
        match load(store, &cursor)? {
            Node::Leaf(key, value) => break (key, value),
            Node::Branch(at, left, right) => {
                if path.last().is_some_and(|&(p, _, _)| at <= p) {
                    return Err(S::invalid());
                }
                path.push((at, left, right));
                cursor = if bit(&key, at) { right } else { left };
            }
        }
    };
    if old_key == key && old_value == value {
        return Ok(root);
    }
    let differing = (0..256).find(|&at| bit(&key, at) != bit(&old_key, at));
    let mut replacement = save(store, Node::Leaf(key, value))?;
    if let Some(at) = differing {
        // Skip the suffix below the new split; it remains immutable and is
        // referenced as one subtree rather than copied or rehashed.
        cursor = root;
        let keep = path.iter().take_while(|&&(p, _, _)| p < at).count();
        for &(p, left, right) in &path[..keep] {
            cursor = if bit(&key, p) { right } else { left };
        }
        path.truncate(keep);
        replacement = if bit(&key, at) {
            save(store, Node::Branch(at, cursor, replacement))?
        } else {
            save(store, Node::Branch(at, replacement, cursor))?
        };
    }
    for (at, left, right) in path.into_iter().rev() {
        replacement = if bit(&key, at) {
            save(store, Node::Branch(at, left, replacement))?
        } else {
            save(store, Node::Branch(at, replacement, right))?
        };
    }
    Ok(replacement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Memory {
        nodes: BTreeMap<Hash, Vec<u8>>,
        writes: usize,
    }
    impl Store for Memory {
        type Error = ();
        fn invalid() {}
        fn get(&mut self, key: &Hash) -> Result<Option<Vec<u8>>, ()> {
            Ok(self.nodes.get(key).cloned())
        }
        fn put(&mut self, key: &Hash, bytes: &[u8]) -> Result<(), ()> {
            self.writes += 1;
            self.nodes.insert(*key, bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn tree_capture137_immutable_roots_dirty_paths_and_corruption() {
        let mut store = Memory::default();
        let mut root = EMPTY;
        let mut roots = Vec::new();
        for n in 0u32..2048 {
            let key = key("tool", &n.to_be_bytes());
            root = update(&mut store, root, key, value(&n.to_be_bytes())).unwrap();
            roots.push(root);
        }
        let selected = key("tool", &0u32.to_be_bytes());
        assert_eq!(lookup(&mut store, roots[0], selected), Ok(Some(value(&0u32.to_be_bytes()))));
        assert_eq!(lookup(&mut store, root, key("tool", b"absent")), Ok(None));
        let before = store.writes;
        assert_eq!(update(&mut store, root, selected, value(&0u32.to_be_bytes())), Ok(root));
        assert_eq!(store.writes, before);
        let changed = update(&mut store, root, selected, value("changed\0😀".as_bytes())).unwrap();
        assert!(store.writes - before <= 257);
        assert_eq!(lookup(&mut store, changed, selected), Ok(Some(value("changed\0😀".as_bytes()))));
        assert_eq!(lookup(&mut store, root, selected), Ok(Some(value(&0u32.to_be_bytes()))));
        store.nodes.get_mut(&changed).unwrap()[1] ^= 1;
        assert!(lookup(&mut store, changed, selected).is_err());
    }
}
