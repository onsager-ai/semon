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
                "SELECT session_key=$4,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_trace_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND source_generation=$5",
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
        let Some(row) = self.catalog(key, false).map_err(io::Error::other)? else {
            return Ok(None);
        };
        if crate::retention::source_generation(&row.sources).map_err(io::Error::other)? != source {
            return Ok(None);
        }
        let Some(value) = self.trace_value_for_source(key, source)? else {
            return Ok(None);
        };
        let mut details = value["details"].clone();
        details["busy"] = value["busy"].clone();
        details["content_generation"] = value["content_generation"].clone();
        Ok(Some((
            Some(details.to_string()),
            row.lifecycle == crate::model::summary::CatalogLifecycle::Current,
        )))
    }
    fn relative(&self, key: &str) -> io::Result<Option<String>> {
        self.catalog(key, false).map_err(io::Error::other)?.map(|row| {
            let value = serde_json::to_value(&row).map_err(io::Error::other)?;
            Ok(json!({"key":row.key,"name":row.name,"harness":row.harness,"kind":value["presentation_kind"],"parent":value["parent"],"last":row.last,"cost":value["cost"],"_status_catalog":value}).to_string())
        }).transpose()
    }
    fn descendants(&self, key: &str) -> io::Result<Vec<String>> {
        use crate::storage::catalog::CatalogSnapshot;
        let rows = self.rows(
            &crate::catalog::Request::parse("limit=64")
                .map_err(|_| io::Error::other(StorageError::InvalidArguments))?,
        )?;
        if rows.len() > 64 {
            return Err(io::Error::other(StorageError::Busy));
        }
        let mut known = std::collections::BTreeSet::from([key.to_owned()]);
        loop {
            let before = known.len();
            for row in &rows {
                let value = serde_json::to_value(row).map_err(io::Error::other)?;
                if value["parent"]
                    .as_str()
                    .is_some_and(|parent| known.contains(parent))
                {
                    known.insert(row.key.clone());
                }
            }
            if known.len() == before {
                break;
            }
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
