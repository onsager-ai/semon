//! SQLite physical consumer of the shared immutable Tree capture plan.
use super::*;
use crate::events::{tree_proof as proof, tree_rows::TreeRows};

struct Nodes<'a>(&'a Connection);
impl proof::Store for Nodes<'_> {
    type Error = rusqlite::Error;
    fn invalid() -> Self::Error {
        rusqlite::Error::InvalidQuery
    }
    fn get(&mut self, hash: &proof::Hash) -> rusqlite::Result<Option<Vec<u8>>> {
        crate::selected_read::charge(128, proof::MAX_NODE_BYTES)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        self.0.query_row(tree_state::sql(self.0,
            "SELECT encoded FROM native_tree_capture_nodes WHERE digest=?1 AND length(encoded)<=67",
            "SELECT encoded FROM native_tree_capture_nodes WHERE namespace=semon_namespace() AND digest=?1 AND length(encoded)<=67")?,
            [hash.as_slice()], |r| r.get(0)).optional()
    }
    fn put(&mut self, hash: &proof::Hash, encoded: &[u8]) -> rusqlite::Result<()> {
        crate::selected_read::check().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let n = self.0.execute(tree_state::sql(self.0,
            "INSERT INTO native_tree_capture_nodes(digest,encoded) VALUES(?1,?2) ON CONFLICT(digest) DO UPDATE SET encoded=excluded.encoded WHERE native_tree_capture_nodes.encoded=excluded.encoded",
            "INSERT INTO native_tree_capture_nodes(namespace,digest,encoded) VALUES(semon_namespace(),?1,?2) ON CONFLICT(namespace,digest) DO UPDATE SET encoded=excluded.encoded WHERE native_tree_capture_nodes.encoded=excluded.encoded")?, params![hash.as_slice(),encoded])?;
        if n != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }
}

pub(super) fn begin(c: &Connection, id: i64) -> rusqlite::Result<()> {
    // A missing capture for existing dimensions is corruption, never an empty
    // root or an invitation to scan/rebuild the selected generation.
    c.execute(tree_state::sql(c,
        "INSERT INTO native_tree_capture_roots(projection_id,root) SELECT ?1,?2 WHERE NOT EXISTS(SELECT 1 FROM native_tree_labels WHERE projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_models WHERE projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_tools WHERE projection_id=?1) ON CONFLICT DO NOTHING",
        "INSERT INTO native_tree_capture_roots(namespace,projection_id,root) SELECT semon_namespace(),?1,?2 WHERE NOT EXISTS(SELECT 1 FROM native_tree_labels WHERE namespace=semon_namespace() AND projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_models WHERE namespace=semon_namespace() AND projection_id=?1) AND NOT EXISTS(SELECT 1 FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1) ON CONFLICT DO NOTHING")?, params![id,proof::EMPTY.as_slice()])?;
    root(c, id).map(|_| ())
}

fn root(c: &Connection, id: i64) -> rusqlite::Result<proof::Hash> {
    crate::selected_read::charge(64, 32).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let root: Vec<u8> = c.query_row(tree_state::sql(c,
        "SELECT root FROM native_tree_capture_roots WHERE projection_id=?1 AND length(root)=32",
        "SELECT root FROM native_tree_capture_roots WHERE namespace=semon_namespace() AND projection_id=?1 AND length(root)=32")?, [id], |r| r.get(0))?;
    root.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn row(c: &Connection, id: i64, family: &str, key: &str, encoded: String) -> rusqlite::Result<()> {
    let previous = root(c, id)?;
    let address = proof::key(family, key.as_bytes());
    let value = proof::value(encoded.as_bytes());
    let mut nodes = Nodes(c);
    let next = proof::update(&mut nodes, previous, address, value)?;
    if proof::lookup(&mut nodes, next, address)? != Some(value) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if next != previous {
        let n = c.execute(tree_state::sql(c,
            "UPDATE native_tree_capture_roots SET root=?2 WHERE projection_id=?1 AND root=?3",
            "UPDATE native_tree_capture_roots SET root=?2 WHERE namespace=semon_namespace() AND projection_id=?1 AND root=?3")?, params![id,next.as_slice(),previous.as_slice()])?;
        if n != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

pub(super) fn capture(c: &Connection, id: i64, rows: &TreeRows<'_>) -> rusqlite::Result<()> {
    fn encode<T: serde::Serialize>(v: &T) -> rusqlite::Result<String> {
        serde_json::to_string(v).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    }
    row(
        c,
        id,
        "labels",
        "codex_turn",
        encode(&["codex_turn", rows.codex_turn.as_str()])?,
    )?;
    for (key, value) in rows.labels() {
        row(c, id, "labels", key, encode(&[key, value])?)?;
    }
    for model in rows.models {
        row(c, id, "models", model, encode(&[model.as_str()])?)?;
    }
    // Read back actual heap state. Closing retains the previous name; hashing
    // the input NULL would authenticate a different row from the physical one.
    for tool in rows.tools.keys().chain(rows.closed_tools) {
        let encoded: String = c.query_row(tree_state::sql(c,
            "SELECT json_array(tool_id,name,closed) FROM native_tree_tools WHERE projection_id=?1 AND tool_id=?2",
            "SELECT json_array(tool_id,name,closed) FROM native_tree_tools WHERE namespace=semon_namespace() AND projection_id=?1 AND tool_id=?2")?, params![id,tool], |r| {
                crate::selected_read::row(r,64)?;
                r.get(0)
            })?;
        row(c, id, "tools", tool, encoded)?;
    }
    Ok(())
}
