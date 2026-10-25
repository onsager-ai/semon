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
    namespace: Option<&'a str>,
}
impl<'a> SearchWriter<'a> {
    pub(crate) fn unbound(db: &'a Connection) -> Self {
        Self { db, namespace: None }
    }
    pub(super) fn admitted(db: &'a Connection, source: &'a super::semantic::AdmittedSource) -> Result<Self, StorageError> {
        source.validate_publication(db)?;
        Ok(Self { db, namespace: Some(source.namespace()) })
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
    fn begin(&self, rows: &[&CatalogRow]) -> rusqlite::Result<()> {
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS catalog_search_members(session_key TEXT PRIMARY KEY) WITHOUT ROWID; DELETE FROM catalog_search_members;")?;
        for row in rows {
            self.db.prepare_cached("INSERT INTO catalog_search_members VALUES(?1)")?.execute([&row.key])?;
        }
        if let Some(ns) = self.namespace {
            self.db.execute("INSERT OR IGNORE INTO main.session_search_coverage(namespace,read_scope,incomplete) VALUES(?1,'current',0),(?1,'retained_history',0)",[ns])?;
        }
        Ok(())
    }
    fn validate(&self, key: &str) -> rusqlite::Result<()> {
        let Some(ns) = self.namespace else {
            return crate::catalog_search::validate_key(self.db, key);
        };
        let (physical, valid): (i64,i64) = self.db.prepare_cached("SELECT COUNT(*),COUNT(valid) FROM (SELECT CASE WHEN octet_length(gram)<=12 THEN 1 ELSE NULL END AS valid FROM main.session_search_memberships WHERE namespace=?1 AND session_key=?2 LIMIT 2049)")?.query_row(params![ns,key],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if physical>2048 || physical!=valid { return Err(rusqlite::Error::InvalidQuery); }
        for scope in [crate::CatalogReadScope::Current,crate::CatalogReadScope::RetainedHistory] {
            let (column, _) = columns(scope);
            let count:i64=self.db.prepare_cached(&format!("SELECT COUNT(*) FROM (SELECT 1 FROM main.session_search_memberships WHERE namespace=?1 AND session_key=?2 AND {column} IS NOT NULL LIMIT 1025)"))?.query_row(params![ns,key],|r|r.get(0))?;
            if count>1024 { return Err(rusqlite::Error::InvalidQuery); }
        }
        Ok(())
    }
    fn remove(&self, scope: crate::CatalogReadScope) -> rusqlite::Result<()> {
        let scope_key=crate::catalog_search::scope_key(scope);
        let Some(ns)=self.namespace else { return crate::catalog_search::remove_members(self.db, scope_key); };
        let (column,other)=columns(scope);
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
    fn document(&self, scope: crate::CatalogReadScope, row: &CatalogRow, fields: &str, complete: bool) -> rusqlite::Result<()> {
        let scope=crate::catalog_search::scope_key(scope);
        let Some(ns)=self.namespace else {return crate::catalog_search::insert_document(self.db,scope,row,fields,complete);};
        self.db.prepare_cached("INSERT INTO main.session_search_documents(namespace,read_scope,session_key,fields,complete) VALUES(?1,?2,?3,?4,?5)")?.execute(params![ns,scope,row.key,fields,complete])?;
        if !complete {
            self.db.prepare_cached("UPDATE main.session_search_coverage SET incomplete=incomplete+1 WHERE namespace=?1 AND read_scope=?2")?.execute(params![ns,scope])?;
        }
        Ok(())
    }
    fn postings(&self, rows: &[(&String, &CatalogRow)]) -> rusqlite::Result<()> {
        let sql=if self.namespace.is_some() {
            "INSERT INTO main.session_search_memberships(namespace,session_key,gram,current_last_ms,history_last_ms) VALUES(?1,?2,?3,?4,?4)"
        } else {
            "INSERT INTO main.session_search_memberships(session_key,gram,current_last_ms,history_last_ms) VALUES(?2,?3,?4,?4)"
        };
        let mut posting = self.db.prepare_cached(sql)?;
        for (gram, row) in rows {
            posting.execute(params![self.namespace,row.key,gram,row.last.unwrap_or(0)])?;
        }
        Ok(())
    }
    fn counts(&self, scope: crate::CatalogReadScope) -> rusqlite::Result<()> {
        let (column, _)=columns(scope);
        let scope=crate::catalog_search::scope_key(scope);
        let sql=if self.namespace.is_some() {
            format!("INSERT INTO main.session_search_counts(namespace,read_scope,gram,members) SELECT ?1,?2,p.gram,COUNT(*) FROM catalog_search_members m CROSS JOIN main.session_search_memberships p WHERE p.namespace=?1 AND p.session_key=m.session_key AND p.{column} IS NOT NULL GROUP BY p.gram ON CONFLICT(namespace,read_scope,gram) DO UPDATE SET members=members+excluded.members")
        } else {
            format!("INSERT INTO main.session_search_counts(read_scope,gram,members) SELECT ?2,p.gram,COUNT(*) FROM catalog_search_members m CROSS JOIN main.session_search_memberships p WHERE p.session_key=m.session_key AND p.{column} IS NOT NULL GROUP BY p.gram ON CONFLICT(read_scope,gram) DO UPDATE SET members=members+excluded.members")
        };
        self.db.prepare_cached(&sql)?.execute(params![self.namespace,scope])?;
        Ok(())
    }
}
