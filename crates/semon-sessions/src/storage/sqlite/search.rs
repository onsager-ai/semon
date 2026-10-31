//! Authored search effects for the single metadata-search publication algorithm.
use crate::model::summary::CatalogRow;
use crate::storage::{StorageError, catalog::SearchPublicationStore};
use rusqlite::{Connection, params};

pub(crate) const SCHEMA: &str = "
CREATE TABLE session_search_coverage (
 namespace TEXT NOT NULL, read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history')),
 incomplete INTEGER NOT NULL CHECK(incomplete>=0), PRIMARY KEY(namespace,read_scope)
) STRICT, WITHOUT ROWID;
CREATE TABLE session_search_documents (
 namespace TEXT NOT NULL, read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history')),
 session_key TEXT NOT NULL, fields TEXT NOT NULL, complete INTEGER NOT NULL CHECK(complete IN(0,1)),
 PRIMARY KEY(namespace,read_scope,session_key)
) STRICT, WITHOUT ROWID;
CREATE TABLE session_search_memberships (
 namespace TEXT NOT NULL, session_key TEXT NOT NULL, gram TEXT NOT NULL,
 current_last_ms INTEGER, history_last_ms INTEGER, PRIMARY KEY(namespace,session_key,gram),
 CHECK(current_last_ms IS NOT NULL OR history_last_ms IS NOT NULL)
) STRICT, WITHOUT ROWID;
CREATE INDEX session_search_current_order ON session_search_memberships(namespace,gram,current_last_ms DESC,session_key ASC) WHERE current_last_ms IS NOT NULL;
CREATE INDEX session_search_history_order ON session_search_memberships(namespace,gram,history_last_ms DESC,session_key ASC) WHERE history_last_ms IS NOT NULL;
CREATE TABLE session_search_counts (
 namespace TEXT NOT NULL, read_scope TEXT NOT NULL CHECK(read_scope IN ('current','retained_history')),
 gram TEXT NOT NULL, members INTEGER NOT NULL CHECK(members>=0), PRIMARY KEY(namespace,read_scope,gram)
) STRICT, WITHOUT ROWID;
";

pub(crate) struct SearchWriter<'a> {
    db: &'a Connection,
    namespace: &'a str,
}

pub(crate) struct SearchLookupRow {
    pub(crate) row: CatalogRow,
    pub(crate) scopes: crate::catalog_search::SearchScopeEligibility,
}

pub(crate) struct SearchLookup {
    pub(crate) consumed: usize,
    pub(crate) oversized: bool,
    pub(crate) rows: Vec<SearchLookupRow>,
}

impl<'a> SearchWriter<'a> {
    pub(crate) fn admitted(
        db: &'a Connection,
        source: &'a super::semantic::AdmittedSource,
    ) -> Result<Self, StorageError> {
        source.validate_publication(db)?;
        Ok(Self {
            db,
            namespace: source.namespace(),
        })
    }

    pub(crate) fn key_page(&self, after: &str) -> rusqlite::Result<Vec<String>> {
        let mut statement = self.db.prepare_cached(
            "SELECT c.session_key FROM main.session_catalog c WHERE c.namespace=?1 AND c.session_key>?2 AND (c.lifecycle='current' OR EXISTS(SELECT 1 FROM main.session_history_catalog h WHERE h.namespace=c.namespace AND h.session_key=c.session_key)) ORDER BY c.session_key LIMIT ?3",
        )?;
        statement
            .query_map(
                rusqlite::params![
                    self.namespace,
                    after,
                    crate::catalog_search::MAX_PUBLICATION_KEYS as i64
                ],
                |row| row.get(0),
            )?
            .collect()
    }

    /// Resolve a bounded key cohort against Current catalog metadata and its
    /// History eligibility. The returned prefix stays within the metadata
    /// byte budget; later keys remain available to the next lookup call.
    pub(crate) fn lookup(&self, keys: &[String]) -> rusqlite::Result<SearchLookup> {
        if keys.len() > crate::catalog_search::MAX_PUBLICATION_KEYS {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if keys.is_empty() {
            return Ok(SearchLookup {
                consumed: 0,
                oversized: false,
                rows: Vec::new(),
            });
        }
        let requested = serde_json::to_string(keys)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let sql = "WITH requested AS (SELECT CAST(key AS INTEGER) AS ordinal,value AS session_key FROM json_each(?1)), candidates AS (SELECT requested.ordinal,requested.session_key,c.metadata,COALESCE(c.lifecycle='current',0) AS current_eligible,COALESCE(c.session_key IS NOT NULL AND (c.lifecycle='current' OR h.session_key IS NOT NULL),0) AS history_eligible,CASE WHEN c.session_key IS NOT NULL AND (c.lifecycle='current' OR h.session_key IS NOT NULL) THEN octet_length(c.metadata) ELSE 0 END AS metadata_bytes FROM requested LEFT JOIN main.session_catalog c ON c.namespace=?2 AND c.session_key=requested.session_key LEFT JOIN main.session_history_catalog h ON h.namespace=c.namespace AND h.session_key=c.session_key), budgeted AS (SELECT *,SUM(metadata_bytes) OVER(ORDER BY ordinal ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS used_bytes FROM candidates) SELECT session_key,current_eligible,history_eligible,metadata_bytes,CASE WHEN used_bytes<=?3 AND history_eligible THEN metadata ELSE NULL END,used_bytes<=?3 FROM budgeted ORDER BY ordinal";
        let mut statement = self.db.prepare_cached(sql)?;
        let mut values = statement.query(rusqlite::params![
            requested,
            self.namespace,
            crate::catalog_search::MAX_PUBLICATION_BYTES as i64
        ])?;
        let mut consumed = 0;
        let mut oversized = false;
        let mut rows = Vec::new();
        while let Some(value) = values.next()? {
            let key: String = value.get(0)?;
            let fits: bool = value.get(5)?;
            if !fits {
                let bytes: i64 = value.get(3)?;
                oversized = bytes > crate::catalog_search::MAX_PUBLICATION_BYTES as i64;
                break;
            }
            consumed += 1;
            let current: bool = value.get(1)?;
            let retained_history: bool = value.get(2)?;
            if !retained_history {
                continue;
            }
            let encoded: Option<String> = value.get(4)?;
            let encoded = encoded.ok_or(rusqlite::Error::InvalidQuery)?;
            let row = serde_json::from_str::<CatalogRow>(&encoded)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            if row.key != key {
                return Err(rusqlite::Error::InvalidQuery);
            }
            rows.push(SearchLookupRow {
                row,
                scopes: crate::catalog_search::SearchScopeEligibility {
                    current,
                    retained_history,
                },
            });
        }
        Ok(SearchLookup {
            consumed,
            oversized,
            rows,
        })
    }
}
fn columns(scope: crate::CatalogReadScope) -> (&'static str, &'static str) {
    match scope {
        crate::CatalogReadScope::Current => ("current_last_ms", "history_last_ms"),
        crate::CatalogReadScope::RetainedHistory => ("history_last_ms", "current_last_ms"),
    }
}
impl SearchPublicationStore for SearchWriter<'_> {
    type Error = rusqlite::Error;
    fn data(error: serde_json::Error) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }
    fn begin(&self, keys: &[String]) -> rusqlite::Result<()> {
        if keys.len() > crate::catalog_search::MAX_PUBLICATION_KEYS {
            return Err(rusqlite::Error::InvalidQuery);
        }
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_search_members(session_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM catalog_search_members;")?;
        for key in keys {
            self.db
                .prepare_cached("INSERT INTO catalog_search_members VALUES(?1)")?
                .execute([key])?;
        }
        {
            let ns = self.namespace;
            self.db.execute("INSERT OR IGNORE INTO main.session_search_coverage(namespace,read_scope,incomplete) VALUES(?1,'current',0),(?1,'retained_history',0)",[ns])?;
        }
        Ok(())
    }
    fn validate(&self, key: &str) -> rusqlite::Result<()> {
        let ns = self.namespace;
        let (physical, valid): (i64,i64) = self.db.prepare_cached("SELECT COUNT(*),COUNT(valid) FROM (SELECT CASE WHEN octet_length(gram)<=12 THEN 1 ELSE NULL END AS valid FROM main.session_search_memberships WHERE namespace=?1 AND session_key=?2 LIMIT 2049)")?.query_row(params![ns,key],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if physical > 2048 || physical != valid {
            return Err(rusqlite::Error::InvalidQuery);
        }
        for scope in [
            crate::CatalogReadScope::Current,
            crate::CatalogReadScope::RetainedHistory,
        ] {
            let (column, _) = columns(scope);
            let count:i64=self.db.prepare_cached(&format!("SELECT COUNT(*) FROM (SELECT 1 FROM main.session_search_memberships WHERE namespace=?1 AND session_key=?2 AND {column} IS NOT NULL LIMIT 1025)"))?.query_row(params![ns,key],|r|r.get(0))?;
            if count > 1024 {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        Ok(())
    }
    fn remove(&self, scope: crate::CatalogReadScope) -> rusqlite::Result<()> {
        let scope_key = crate::catalog_search::scope_key(scope);
        let ns = self.namespace;
        let (column, other) = columns(scope);
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_search_removals(gram TEXT PRIMARY KEY,members INTEGER NOT NULL) WITHOUT ROWID; DELETE FROM catalog_search_removals;")?;
        self.db.prepare_cached(&format!("INSERT INTO catalog_search_removals SELECT p.gram,COUNT(*) FROM catalog_search_members m CROSS JOIN main.session_search_memberships p WHERE p.namespace=?1 AND p.session_key=m.session_key AND p.{column} IS NOT NULL GROUP BY p.gram ON CONFLICT(gram) DO UPDATE SET members=members+excluded.members"))?.execute([ns])?;
        self.db.prepare_cached("UPDATE main.session_search_counts SET members=members-(SELECT r.members FROM catalog_search_removals r WHERE r.gram=session_search_counts.gram) WHERE namespace=?1 AND read_scope=?2 AND gram IN (SELECT gram FROM catalog_search_removals)")?.execute(params![ns,scope_key])?;
        self.db.prepare_cached("DELETE FROM main.session_search_counts WHERE namespace=?1 AND read_scope=?2 AND members=0 AND gram IN (SELECT gram FROM catalog_search_removals)")?.execute(params![ns,scope_key])?;
        self.db.prepare_cached("UPDATE main.session_search_coverage SET incomplete=incomplete-(SELECT COUNT(*) FROM catalog_search_members m CROSS JOIN main.session_search_documents d WHERE d.namespace=?1 AND d.read_scope=?2 AND d.session_key=m.session_key AND d.complete=0) WHERE namespace=?1 AND read_scope=?2")?.execute(params![ns,scope_key])?;
        self.db.prepare_cached(&format!("DELETE FROM main.session_search_memberships WHERE namespace=?1 AND {other} IS NULL AND session_key IN (SELECT session_key FROM catalog_search_members)"))?.execute([ns])?;
        self.db.prepare_cached(&format!("UPDATE main.session_search_memberships SET {column}=NULL WHERE namespace=?1 AND {column} IS NOT NULL AND session_key IN (SELECT session_key FROM catalog_search_members)"))?.execute([ns])?;
        self.db.prepare_cached("DELETE FROM main.session_search_documents WHERE namespace=?1 AND read_scope=?2 AND session_key IN (SELECT session_key FROM catalog_search_members)")?.execute(params![ns,scope_key])?;
        Ok(())
    }
    fn document(
        &self,
        scope: crate::CatalogReadScope,
        row: &CatalogRow,
        fields: &str,
        complete: bool,
    ) -> rusqlite::Result<()> {
        let scope = crate::catalog_search::scope_key(scope);
        let ns = self.namespace;
        self.db.prepare_cached("INSERT INTO main.session_search_documents(namespace,read_scope,session_key,fields,complete) VALUES(?1,?2,?3,?4,?5)")?.execute(params![ns,scope,row.key,fields,complete])?;
        if !complete {
            self.db.prepare_cached("UPDATE main.session_search_coverage SET incomplete=incomplete+1 WHERE namespace=?1 AND read_scope=?2")?.execute(params![ns,scope])?;
        }
        Ok(())
    }
    fn postings(
        &self,
        rows: &[crate::catalog_search::SearchPosting<'_>],
    ) -> rusqlite::Result<()> {
        let sql = "INSERT INTO main.session_search_memberships(namespace,session_key,gram,current_last_ms,history_last_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,session_key,gram) DO UPDATE SET current_last_ms=excluded.current_last_ms,history_last_ms=excluded.history_last_ms";
        let mut posting = self.db.prepare_cached(sql)?;
        for item in rows {
            let timestamp = item.row.last.unwrap_or(0);
            posting.execute(params![
                self.namespace,
                item.row.key,
                item.gram,
                item.scopes.current.then_some(timestamp),
                item.scopes.retained_history.then_some(timestamp),
            ])?;
        }
        Ok(())
    }
    fn counts(&self, scope: crate::CatalogReadScope) -> rusqlite::Result<()> {
        let (column, _) = columns(scope);
        let scope = crate::catalog_search::scope_key(scope);
        let sql = format!(
            "INSERT INTO main.session_search_counts(namespace,read_scope,gram,members) SELECT ?1,?2,p.gram,COUNT(*) FROM catalog_search_members m CROSS JOIN main.session_search_memberships p WHERE p.namespace=?1 AND p.session_key=m.session_key AND p.{column} IS NOT NULL GROUP BY p.gram ON CONFLICT(namespace,read_scope,gram) DO UPDATE SET members=members+excluded.members"
        );
        self.db
            .prepare_cached(&sql)?
            .execute(params![self.namespace, scope])?;
        Ok(())
    }
}
