//! Physical search effects only. Normalization and posting order are shared.
use super::*;
use crate::storage::catalog::SearchPublicationStore;

impl SearchPublicationStore for Effects<'_, '_> {
    type Error = StorageError;
    fn data(_: serde_json::Error) -> StorageError {
        StorageError::Unavailable
    }
    fn begin(&self, rows: &[&CatalogRow]) -> Result<(), StorageError> {
        if rows.len() > 64 {
            return Err(StorageError::Busy);
        }
        self.tx.borrow().check()
    }
    fn validate(&self, key: &str) -> Result<(), StorageError> {
        let tx = self.tx.borrow_mut();
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT count(*),count(*) FILTER (WHERE octet_length(gram)<=12) FROM (SELECT gram FROM semon_pg_native_catalog_search_memberships WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 LIMIT 1025) p", &[&ns, &self.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        let count: i64 = rows[0].get(0);
        if count > 1024 || count != rows[0].get::<_, i64>(1) {
            return Err(StorageError::ScopeChanged);
        }
        tx.check()
    }
    fn remove(&self, _: crate::CatalogReadScope) -> Result<(), StorageError> {
        // Each generation is a new immutable cohort. Replays must compare exact
        // bytes rather than deleting a previous generation's postings.
        self.tx.borrow().check()
    }
    fn document(
        &self,
        scope: crate::CatalogReadScope,
        row: &CatalogRow,
        fields: &str,
        complete: bool,
    ) -> Result<(), StorageError> {
        if fields.len() > MAX {
            return Err(StorageError::Busy);
        }
        self.put(
            "semon_pg_native_catalog_search_documents",
            &["scope", "key_digest", "session_key", "fields", "complete"],
            "scope,key_digest",
            &[
                &crate::catalog_search::scope_key(scope).as_bytes(),
                &digest(row.key.as_bytes()),
                &row.key.as_bytes(),
                &fields.as_bytes(),
                &complete,
            ],
        )
    }
    fn postings(&self, rows: &[(&String, &CatalogRow)]) -> Result<(), StorageError> {
        // Bounded arrays avoid one round trip per gram. The full identity is
        // compared on every conflict, including a theoretical digest collision.
        for rows in rows.chunks(64) {
            let keys = rows
                .iter()
                .map(|(_, row)| row.key.as_bytes().to_vec())
                .collect::<Vec<_>>();
            let grams = rows
                .iter()
                .map(|(gram, _)| gram.as_bytes().to_vec())
                .collect::<Vec<_>>();
            let times = rows
                .iter()
                .map(|(_, row)| row.last.unwrap_or(0))
                .collect::<Vec<_>>();
            if grams.iter().any(|gram| gram.len() > 12) {
                return Err(StorageError::Busy);
            }
            let tx = self.tx.borrow_mut();
            tx.check()?;
            let ns = tx.namespace.as_str().as_bytes();
            let changed = tx.driver.execute("INSERT INTO semon_pg_native_catalog_search_memberships(namespace,generation,key_digest,session_key,gram,last_ms) SELECT $1,$2,sha256(k),k,g,t FROM unnest($3::bytea[],$4::bytea[],$5::bigint[]) AS p(k,g,t) ON CONFLICT(namespace_digest,generation,key_digest,gram) DO UPDATE SET generation=excluded.generation WHERE semon_pg_native_catalog_search_memberships.namespace=excluded.namespace AND semon_pg_native_catalog_search_memberships.session_key=excluded.session_key AND semon_pg_native_catalog_search_memberships.last_ms=excluded.last_ms", &[&ns, &self.generation.as_bytes(), &keys, &grams, &times]).map_err(sqlstate::error)?;
            if changed != rows.len() as u64 {
                return Err(StorageError::ScopeChanged);
            }
            boundary(&tx)?;
        }
        Ok(())
    }
    fn counts(&self, scope: crate::CatalogReadScope) -> Result<(), StorageError> {
        let tx = self.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        // The generation contains at most 64 owners, each with <=1024 grams.
        // No unrelated retained generation participates in this aggregate.
        let row = tx.driver.query_one("WITH counts AS MATERIALIZED (SELECT gram,count(*) AS members FROM semon_pg_native_catalog_search_memberships WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 GROUP BY gram), written AS (INSERT INTO semon_pg_native_catalog_search_counts(namespace,generation,scope,gram,members) SELECT $1,$2,$3,gram,members FROM counts ON CONFLICT(namespace_digest,generation,scope,gram) DO UPDATE SET generation=excluded.generation WHERE semon_pg_native_catalog_search_counts.namespace=excluded.namespace AND semon_pg_native_catalog_search_counts.members=excluded.members RETURNING 1) SELECT (SELECT count(*) FROM counts)=(SELECT count(*) FROM written)", &[&ns, &self.generation.as_bytes(), &crate::catalog_search::scope_key(scope).as_bytes()]).map_err(sqlstate::error)?;
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        boundary(&tx)?;
        Ok(())
    }
}
