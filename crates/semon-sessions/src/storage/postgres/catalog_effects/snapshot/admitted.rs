//! Ordinary Catalog renderer adapter over one acknowledged immutable generation.
use super::*;
use crate::{catalog::Request, storage::catalog::CatalogSnapshot};
use std::io;

fn failed(error: StorageError) -> io::Error {
    io::Error::other(error)
}
fn unavailable<T>() -> io::Result<T> {
    Err(failed(StorageError::NamespaceUpgradePending))
}

impl CatalogSnapshot for Snapshot<'_, '_> {
    fn runtime_binding(&self) -> Option<crate::storage::QueryRuntimeBinding> {
        let mut tx = self.tx.lock().ok()?;
        super::super::super::model_cache::binding(&mut tx).ok()
    }
    fn require_current(&self) -> io::Result<()> {
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        tx.check().map_err(failed)?;
        let admitted: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2)", &[&tx.namespace.as_str().as_bytes(), &self.generation.as_bytes()]).map_err(sqlstate::error).map_err(failed)?.get(0);
        if admitted { Ok(()) } else { unavailable() }
    }
    fn publication_value(&self, key: &str) -> io::Result<Option<String>> {
        self.require_current()?;
        Ok(match key {
            "catalog_generation" => Some(self.generation.clone()),
            "catalog_version" => Some(crate::model::summary::CATALOG_VERSION.to_string()),
            // Completeness is not inferred from successful metadata staging.
            "catalog_completeness" => Some("partial".into()),
            // Search counts/documents/memberships are effects of the same
            // admitted generation operation; carried owners included.
            "search_version" => Some(crate::catalog_search::VERSION.to_string()),
            "search_generation_current" | "search_generation_retained_history" => {
                Some(self.generation.clone())
            }
            _ => None,
        })
    }
    fn rows(&self, request: &Request) -> io::Result<Vec<CatalogRow>> {
        self.require_current()?;
        if request.limit > 64 {
            return unavailable();
        }
        // Retained history is the current rows plus every history owner of
        // this generation without a current row, read from its proof-owned
        // immutable header. Both sides filter and keyset on their own indexed
        // columns (owners carry the header's), and the decode re-checks them.
        let history = request.scope == crate::CatalogReadScope::RetainedHistory;
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        let ns = tx.namespace.as_str().as_bytes();
        // Filter and keyset BEFORE LIMIT. Metadata stays opaque bytes: native
        // keys can contain NUL, which PostgreSQL JSON/text cannot represent.
        let sid = request.sid.as_ref().map(|v| v.as_bytes());
        let harness = request.harness.as_ref().map(|v| v.as_bytes());
        let repo = request.repo.as_ref().map(|v| v.as_bytes());
        let model = request.model.as_ref().map(|v| v.as_bytes());
        let cursor_last = request.cursor.as_ref().map(|v| v.last);
        let cursor_key = request.cursor.as_ref().map(|v| v.key.as_bytes());
        let inclusive = request.cursor.as_ref().is_some_and(|v| v.inclusive);
        let limit = request.limit as i64 + 1;
        let rows = tx
            .driver
            .query(
                "WITH visible AS (
                SELECT session_key,metadata,last_ms,harness,repo,model,false AS retained
                FROM semon_pg_native_catalog_rows
                WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2
                UNION ALL
                SELECT o.session_key,h.metadata,o.last_ms,o.harness,o.repo,o.model,true
                FROM semon_pg_native_catalog_history_owners o
                JOIN semon_pg_native_catalog_history_headers h
                  ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace
                 AND h.generation=o.owner_generation AND h.key_digest=o.key_digest
                 AND h.session_key=o.session_key
                WHERE $11 AND o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2
                  AND NOT EXISTS (SELECT 1 FROM semon_pg_native_catalog_rows r
                    WHERE r.namespace_digest=o.namespace_digest AND r.namespace=o.namespace
                      AND r.generation=$2 AND r.key_digest=o.key_digest)
             ), page AS MATERIALIZED (
                SELECT session_key,metadata,last_ms,harness,repo,model,retained
                FROM visible
                WHERE ($3::bytea IS NULL OR session_key=$3)
                  AND ($4::bytea IS NULL OR harness=$4)
                  AND ($5::bytea IS NULL OR repo=$5)
                  AND ($6::bytea IS NULL OR model=$6)
                  AND ($7::bigint IS NULL OR last_ms<$7
                       OR (last_ms=$7 AND (session_key>$8 OR ($9 AND session_key=$8))))
                ORDER BY last_ms DESC,session_key LIMIT $10
             ) SELECT session_key,
                 CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata END,
                 last_ms,harness,repo,model,retained FROM page ORDER BY last_ms DESC,session_key",
                &[
                    &ns,
                    &self.generation.as_bytes(),
                    &sid,
                    &harness,
                    &repo,
                    &model,
                    &cursor_last,
                    &cursor_key,
                    &inclusive,
                    &limit,
                    &history,
                ],
            )
            .map_err(sqlstate::error)
            .map_err(failed)?;
        let mut output = Vec::new();
        for row in rows {
            let key: Vec<u8> = row.get(0);
            let bytes: Vec<u8> = row
                .get::<_, Option<Vec<u8>>>(1)
                .ok_or_else(|| failed(StorageError::Busy))?;
            crate::selected_read::charge(64, bytes.len())?;
            let mut value: CatalogRow = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if value.key.as_bytes() != key
                || value.sources.len() > 64
                || value.last.unwrap_or(0) != row.get::<_, i64>(2)
                || value.harness.as_bytes() != row.get::<_, Vec<u8>>(3)
                || value.repo.as_deref().unwrap_or("").as_bytes() != row.get::<_, Vec<u8>>(4)
                || value.model.as_bytes() != row.get::<_, Vec<u8>>(5)
            {
                return Err(failed(StorageError::ScopeChanged));
            }
            if row.get::<_, bool>(6) {
                value.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
            }
            output.push(value);
        }
        tx.check().map_err(failed)?;
        Ok(output)
    }
    fn search(
        &self,
        query: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection> {
        use crate::catalog_search::{Candidate, Position};
        self.require_current()?;
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        let ns = tx.namespace.as_str().as_bytes();
        let generation = self.generation.as_bytes();
        let history = query.scope == crate::CatalogReadScope::RetainedHistory;
        let scope = crate::catalog_search::scope_key(query.scope).as_bytes();
        let run = |sql: &str, args: &[&(dyn tokio_postgres::types::ToSql + Sync)]| {
            tx.driver.query(sql, args).map_err(sqlstate::error).map_err(failed)
        };
        // Withdrawal does not yet carry a removed owner's retained search
        // document into the successor; a missing one is pending, never a miss.
        if history {
            let missing = run("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND NOT EXISTS (SELECT 1 FROM semon_pg_native_catalog_search_documents d WHERE d.namespace_digest=o.namespace_digest AND d.namespace=o.namespace AND d.generation=$2 AND d.scope=$3 AND d.key_digest=o.key_digest))", &[&ns, &generation, &scope])?;
            if missing.first().is_none_or(|row| row.get::<_, bool>(0)) {
                return unavailable();
            }
        }
        let selection = crate::catalog_search::select(
            query,
            |text| {
                crate::catalog_search::anchor_with(text, |gram| {
                    let rows = run("SELECT members FROM semon_pg_native_catalog_search_counts WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND scope=$3 AND gram=$4", &[&ns, &generation, &scope, &gram.as_bytes()])?;
                    Ok(rows.first().map(|row| row.get::<_, i64>(0)))
                })
            },
            // Memberships carry no scope; the scope document decides presence.
            |gram, after: Option<&Position>, limit| {
                let rows = run(
                    "SELECT m.last_ms,m.session_key FROM semon_pg_native_catalog_search_memberships m
                     WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.generation=$2 AND m.gram=$4
                       AND EXISTS (SELECT 1 FROM semon_pg_native_catalog_search_documents d
                         WHERE d.namespace_digest=m.namespace_digest AND d.namespace=m.namespace
                           AND d.generation=$2 AND d.scope=$3 AND d.key_digest=m.key_digest)
                       AND ($5::bigint IS NULL OR m.last_ms<$5
                            OR (m.last_ms=$5 AND (m.session_key>$6 OR ($7 AND m.session_key=$6))))
                     ORDER BY m.last_ms DESC,m.session_key LIMIT $8",
                    &[
                        &ns,
                        &generation,
                        &scope,
                        &gram.as_bytes(),
                        &after.map(|v| v.last),
                        &after.map(|v| v.key.as_bytes()),
                        &after.is_some_and(|v| v.inclusive),
                        &(limit as i64),
                    ],
                )?;
                rows.iter()
                    .map(|row| {
                        Ok(Position {
                            inclusive: false,
                            last: row.get(0),
                            key: String::from_utf8(row.get(1)).map_err(io::Error::other)?,
                        })
                    })
                    .collect()
            },
            |key, budget| {
                let rows = run("SELECT octet_length(fields)::bigint,CASE WHEN octet_length(fields)<=$5 THEN fields END FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND scope=$3 AND key_digest=sha256($4::bytea) AND session_key=$4 AND complete", &[&ns, &generation, &scope, &key.as_bytes(), &(budget as i64)])?;
                let row = rows.first().ok_or_else(|| failed(StorageError::ScopeChanged))?;
                let fields = row
                    .get::<_, Option<Vec<u8>>>(1)
                    .map(String::from_utf8)
                    .transpose()
                    .map_err(io::Error::other)?;
                Ok((row.get(0), fields))
            },
            // As the SQLite reader, retained history wins per key over the
            // current row. The lifecycle is the chosen proof-owned source's,
            // which the shared reducer compares with the decoded metadata.
            |key, budget| {
                let rows = run(
                    "WITH live AS (
                        SELECT metadata,last_ms,harness,repo FROM semon_pg_native_catalog_rows
                        WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2
                          AND key_digest=sha256($3::bytea) AND session_key=$3
                     ), retained AS (
                        SELECT h.metadata,o.last_ms,o.harness,o.repo
                        FROM semon_pg_native_catalog_history_owners o
                        JOIN semon_pg_native_catalog_history_headers h
                          ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace
                         AND h.generation=o.owner_generation AND h.key_digest=o.key_digest
                         AND h.session_key=o.session_key
                        WHERE $5 AND o.namespace_digest=sha256($1::bytea) AND o.namespace=$1
                          AND o.generation=$2 AND o.key_digest=sha256($3::bytea) AND o.session_key=$3
                     ), chosen AS (
                        SELECT metadata,last_ms,harness,repo,false AS is_live FROM retained
                        UNION ALL
                        SELECT metadata,last_ms,harness,repo,true FROM live
                        WHERE NOT EXISTS (SELECT 1 FROM retained)
                     ) SELECT octet_length(metadata)::bigint,CASE WHEN octet_length(metadata)<=$4 THEN metadata END,
                         last_ms,harness,repo,is_live FROM chosen",
                    &[&ns, &generation, &key.as_bytes(), &(budget as i64), &history],
                )?;
                if rows.len() > 1 {
                    return Err(failed(StorageError::ScopeChanged));
                }
                let text = |bytes: Vec<u8>| String::from_utf8(bytes).map_err(io::Error::other);
                rows.first()
                    .map(|row| {
                        let repo = text(row.get(4))?;
                        Ok(Candidate {
                            length: row.get(0),
                            encoded: row.get::<_, Option<Vec<u8>>>(1).map(text).transpose()?,
                            last: row.get(2),
                            harness: text(row.get(3))?,
                            repo: (!repo.is_empty()).then_some(repo),
                            lifecycle: if row.get::<_, bool>(5) {
                                crate::model::summary::CatalogLifecycle::Current
                            } else {
                                crate::model::summary::CatalogLifecycle::Retained
                            }
                            .as_str()
                            .to_owned(),
                        })
                    })
                    .transpose()
            },
            || failed(StorageError::ScopeChanged),
        );
        tx.check().map_err(failed)?;
        selection
    }
    fn has_history(&self, key: &str) -> io::Result<bool> {
        self.catalog(key, true)
            .map(|row| row.is_some())
            .map_err(failed)
    }
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>> {
        self.catalog(key, true)
            .map_err(failed)?
            .map(|row| serde_json::to_string(&row).map_err(io::Error::other))
            .transpose()
    }
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool> {
        self.has_history(&row.key)
    }
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow) -> serde_json::Value,
    ) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        crate::storage::selected::selected_details(self, key, sources, &mut |row| status(row))
    }
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        if generation != self.generation {
            return Ok(None);
        }
        let Some((_, header, _)) = self.selected(key, history, 0, 1).map_err(failed)? else {
            return Ok(None);
        };
        if !(0..=9_007_199_254_740_991).contains(&header.3) {
            return Err(failed(StorageError::ScopeChanged));
        }
        Ok(Some(
            serde_json::json!({"generation":crate::transcript_range::entry_generation(&header.0),"total":header.3}),
        ))
    }
}
