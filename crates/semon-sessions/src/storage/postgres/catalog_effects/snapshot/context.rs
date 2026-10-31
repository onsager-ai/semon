//! Immutable selected metadata through the ordinary shared presentation port.
use super::*;
use crate::storage::selected::ContextLookup;
use serde_json::{Value, json};
use std::io;

impl Snapshot<'_, '_> {
    fn trace_text(
        &self,
        table: &str,
        key: &str,
        id: Option<(&str, &str, &str)>,
    ) -> io::Result<Option<String>> {
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        tx.check().map_err(io::Error::other)?;
        let ns = tx.namespace.as_str().as_bytes();
        let hash = digest(key.as_bytes());
        let generation = self.generation.as_bytes();
        let key_bytes = key.as_bytes();
        let mut args: Vec<&(dyn ToSql + Sync)> = vec![&ns, &generation, &hash, &key_bytes];
        let extra = id
            .map(|(column, hash, _)| format!(" AND {hash}=sha256($5::bytea) AND {column}=$5"))
            .unwrap_or_default();
        let id_bytes = id.map(|(_, _, value)| value.as_bytes());
        if let Some(value) = &id_bytes {
            args.push(value);
        }
        let rows = tx.driver.query(&format!("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4{extra} LIMIT 2"), &args).map_err(sqlstate::error).map_err(io::Error::other)?;
        if rows.len() > 1 {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        rows.first()
            .map(|row| text(row.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?))
            .transpose()
            .map_err(io::Error::other)
    }
    fn trace_value(&self, key: &str) -> io::Result<Option<Value>> {
        self.trace_text("semon_pg_native_catalog_trace_headers", key, None)?
            .map(|text| serde_json::from_str(&text).map_err(io::Error::other))
            .transpose()
    }

    fn trace_value_for_source(
        &self,
        key: &str,
        source_generation: &str,
    ) -> io::Result<Option<Value>> {
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        tx.check().map_err(io::Error::other)?;
        let ns = tx.namespace.as_str().as_bytes();
        let generation = self.generation.as_bytes();
        let key_bytes = key.as_bytes();
        let source_bytes = source_generation.as_bytes();
        let rows = tx
            .driver
            .query(
                "SELECT session_key=$4,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND source_generation=$5",
                &[
                    &ns,
                    &generation,
                    &digest(key.as_bytes()),
                    &key_bytes,
                    &source_bytes,
                ],
            )
            .map_err(sqlstate::error)
            .map_err(io::Error::other)?;
        tx.check().map_err(io::Error::other)?;
        if rows.len() > 1 {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        if !row.get::<_, bool>(0) {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        let metadata = row
            .get::<_, Option<Vec<u8>>>(1)
            .ok_or(StorageError::Busy)
            .map_err(io::Error::other)?;
        serde_json::from_slice(&metadata)
            .map(Some)
            .map_err(io::Error::other)
    }
}

impl ContextLookup for Snapshot<'_, '_> {
    fn details(&self, key: &str, source: &str) -> io::Result<Option<(Option<String>, bool)>> {
        let current = self
            .catalog(key, false)
            .map_err(io::Error::other)?
            .is_some_and(|row| row.lifecycle == crate::model::summary::CatalogLifecycle::Current);
        let Some(value) = self.trace_value_for_source(key, source)? else {
            return Ok(None);
        };
        let mut details = value["details"].clone();
        details["busy"] = value["busy"].clone();
        details["content_generation"] = value["content_generation"].clone();
        Ok(Some((Some(details.to_string()), current)))
    }
    fn relative(
        &self,
        key: &str,
    ) -> io::Result<Option<(String, Option<crate::storage::catalog::CatalogObservationOwner>)>> {
        let current = self.catalog(key, false).map_err(io::Error::other)?;
        let current_harness = current.as_ref().map(|row| row.harness.clone());
        let lifecycle = if current_harness.is_some() {
            crate::model::summary::CatalogLifecycle::Current
        } else {
            crate::model::summary::CatalogLifecycle::Retained
        };
        let history = self.catalog(key, true).map_err(io::Error::other)?;
        let owner = if history.is_some() {
            crate::storage::catalog::CatalogObservationOwner::History
        } else {
            crate::storage::catalog::CatalogObservationOwner::Catalog
        };
        let Some(mut row) = history.or(current) else {
            return Ok(None);
        };
        row.lifecycle = lifecycle;
        let harness = current_harness.as_deref().unwrap_or(&row.harness);
        let value = serde_json::to_value(&row).map_err(io::Error::other)?;
        Ok(Some(
            (json!({"key":row.key,"name":row.name,"harness":harness,"kind":value["presentation_kind"],"parent":value["parent"],"last":row.last,"cost":value["cost"],"_status_catalog":value}).to_string(), Some(owner)),
        ))
    }
    fn descendants(&self, key: &str) -> io::Result<Vec<String>> {
        let _scope = crate::selected_read::Scope::ensure();
        // Match SQLite's 101-node recursive bound while reading only indexed
        // children of the selected family. Truncated membership follows the
        // stable key-digest index order; returned keys remain lexically sorted.
        const FAMILY_NODES: usize = 101;
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        tx.check().map_err(io::Error::other)?;
        let namespace = tx.namespace.as_str().as_bytes();
        let generation = self.generation.as_bytes();
        let mut known = std::collections::BTreeSet::from([key.to_owned()]);
        let mut frontier = std::collections::BTreeSet::from([key.to_owned()]);
        while !frontier.is_empty() && known.len() < FAMILY_NODES {
            let mut next = std::collections::BTreeSet::new();
            for parent in &frontier {
                let remaining = FAMILY_NODES - known.len() - next.len();
                if remaining == 0 {
                    break;
                }
                let parent_digest = digest(parent.as_bytes());
                let limit =
                    i64::try_from(remaining).map_err(|_| io::Error::other(StorageError::Busy))?;
                let visited_bytes = known
                    .iter()
                    .chain(next.iter())
                    .fold(0usize, |bytes, known| bytes.saturating_add(known.len()));
                crate::selected_read::charge(0, visited_bytes)
                    .map_err(|_| io::Error::other(StorageError::Busy))?;
                let visited = known
                    .iter()
                    .chain(next.iter())
                    .map(|known| known.as_bytes().to_vec())
                    .collect::<Vec<_>>();
                let byte_limit = crate::selected_read::max_dynamic_bytes(64, remaining)
                    .and_then(|bytes| {
                        i64::try_from(bytes).map_err(|_| io::Error::other(StorageError::Busy))
                    })
                    .map_err(|_| io::Error::other(StorageError::Busy))?;
                let rows = tx
                    .driver
                    .query(
                        r#"
                            WITH candidates AS MATERIALIZED (
                                SELECT parent_key,parent_digest,key_digest,session_key
                                FROM semon_pg_native_catalog_rows
                                WHERE namespace_digest=sha256($1::bytea)
                                  AND namespace=$1 AND generation=$2 AND parent_digest=$3
                                  AND NOT (session_key=ANY($5::bytea[]))
                                UNION ALL
                                SELECT h.parent_key,h.parent_digest,h.key_digest,h.session_key
                                FROM semon_pg_native_catalog_history_owners h
                                WHERE h.namespace_digest=sha256($1::bytea)
                                  AND h.namespace=$1 AND h.generation=$2 AND h.parent_digest=$3
                                  AND NOT (h.session_key=ANY($5::bytea[]))
                                  AND NOT EXISTS (
                                    SELECT 1 FROM semon_pg_native_catalog_rows c
                                    WHERE c.namespace_digest=h.namespace_digest
                                      AND c.namespace=h.namespace AND c.generation=$2
                                      AND c.key_digest=h.key_digest AND c.session_key=h.session_key
                                  )
                                ORDER BY key_digest LIMIT $4
                            ), measured AS (
                                SELECT coalesce(sum(
                                    octet_length(parent_key)::bigint
                                  + octet_length(parent_digest)::bigint
                                  + octet_length(key_digest)::bigint
                                  + octet_length(session_key)::bigint
                                ),0) AS bytes,
                                coalesce(bool_and(
                                    parent_key IS NOT NULL
                                    AND octet_length(parent_key)<=1048576
                                    AND octet_length(parent_digest)<=1048576
                                    AND octet_length(key_digest)<=1048576
                                    AND octet_length(session_key)<=1048576
                                ),true) AS bounded
                                FROM candidates
                            )
                            SELECT true AS refused,NULL::bytea,NULL::bytea,NULL::bytea,NULL::bytea
                            FROM measured WHERE bytes>$6::bigint OR NOT bounded
                            UNION ALL
                            SELECT false AS refused,c.parent_key,c.parent_digest,c.key_digest,c.session_key
                            FROM candidates c CROSS JOIN measured
                            WHERE measured.bytes<=$6::bigint AND measured.bounded
                            ORDER BY 4 NULLS FIRST
                        "#,
                        &[
                            &namespace,
                            &generation,
                            &parent_digest,
                            &limit,
                            &visited,
                            &byte_limit,
                        ],
                    )
                    .map_err(sqlstate::error)
                    .map_err(io::Error::other)?;
                tx.check().map_err(io::Error::other)?;
                for row in rows {
                    if row.get::<_, bool>(0) {
                        return Err(io::Error::other(StorageError::Busy));
                    }
                    let parent_key = row
                        .get::<_, Option<Vec<u8>>>(1)
                        .ok_or(StorageError::Busy)
                        .map_err(io::Error::other)?;
                    let parent_digest = row
                        .get::<_, Option<Vec<u8>>>(2)
                        .ok_or(StorageError::ScopeChanged)
                        .map_err(io::Error::other)?;
                    let key_digest = row.get::<_, Vec<u8>>(3);
                    let session_key = row
                        .get::<_, Option<Vec<u8>>>(4)
                        .ok_or(StorageError::Busy)
                        .map_err(io::Error::other)?;
                    if parent_key.as_slice() != parent.as_bytes()
                        || parent_digest != digest(&parent_key)
                        || key_digest != digest(&session_key)
                    {
                        return Err(io::Error::other(StorageError::ScopeChanged));
                    }
                    crate::selected_read::charge(
                        64,
                        parent_key.len()
                            + parent_digest.len()
                            + key_digest.len()
                            + session_key.len(),
                    )
                    .map_err(|_| io::Error::other(StorageError::Busy))?;
                    next.insert(text(session_key).map_err(io::Error::other)?);
                }
            }
            known.extend(next.iter().cloned());
            frontier = next;
        }
        known.remove(key);
        Ok(known.into_iter().collect())
    }
    fn header(&self, key: &str) -> io::Result<Option<(String, bool)>> {
        let Some(row) = self.catalog(key, false).map_err(io::Error::other)? else {
            return Ok(None);
        };
        let source = crate::retention::source_generation(&row.sources).map_err(io::Error::other)?;
        Ok(self.trace_value(key)?.map(|value| {
            (
                source,
                value["relationships_incomplete"].as_bool().unwrap_or(false),
            )
        }))
    }
    fn turn(&self, key: &str, id: &str) -> io::Result<Option<String>> {
        self.trace_text(
            "semon_pg_native_catalog_trace_turns",
            key,
            Some(("turn_id", "turn_digest", id)),
        )
    }
    fn edge(&self, key: &str, id: &str) -> io::Result<Option<String>> {
        self.trace_text(
            "semon_pg_native_catalog_trace_edges",
            key,
            Some(("edge_id", "edge_digest", id)),
        )
    }
    fn target_turns(&self, key: &str, id: &str) -> io::Result<Vec<String>> {
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        tx.check().map_err(io::Error::other)?;
        let rows = tx.driver.query("SELECT turn_id FROM semon_pg_native_catalog_trace_turns WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND start_id=$4 ORDER BY turn_id LIMIT 2", &[&tx.namespace.as_str().as_bytes(), &self.generation.as_bytes(), &key.as_bytes(), &id.as_bytes()]).map_err(sqlstate::error).map_err(io::Error::other)?;
        rows.iter()
            .map(|row| text(row.get(0)).map_err(io::Error::other))
            .collect()
    }
    fn participant(&self, key: &str) -> io::Result<Option<String>> {
        Ok(self.trace_value(key)?.map(|value| json!({"name":value["name"],"harness":value["harness"],"model":value["model"],"kind":value["kind"],"state":value["state"],"start":value["start"],"last":value["last"],"calls":value["details"]["calls"],"usd":value["family_usd"],"activity":value["details"]["activity"],"live_observation":true}).to_string()))
    }
    fn preview_header(&self, key: &str) -> io::Result<Option<(String, Option<String>)>> {
        let Some(row) = self.catalog(key, false).map_err(io::Error::other)? else {
            return Ok(None);
        };
        let source = crate::retention::source_generation(&row.sources).map_err(io::Error::other)?;
        Ok(self.trace_value(key)?.map(|value| {
            (
                source,
                value["content_generation"].as_str().map(str::to_owned),
            )
        }))
    }
    fn preview_catalog(&self, key: &str) -> io::Result<Option<String>> {
        self.catalog(key, false)
            .map_err(io::Error::other)?
            .map(|row| serde_json::to_string(&row).map_err(io::Error::other))
            .transpose()
    }
}

#[cfg(test)]
mod tests;
