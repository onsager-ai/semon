//! Namespace-owned lookups for the shared native physical-owner plan.
use crate::events::store::semantic_stage::{ContextPort, OwnerClosurePort, PublicationPlanPort};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) struct Plan<'a> {
    pub(super) db: &'a Connection,
    pub(super) ns: &'a str,
}

impl OwnerClosurePort for Plan<'_> {
    fn input(&self, path: &str) -> rusqlite::Result<Option<(String, bool)>> {
        self.db.query_row("SELECT harness,is_top FROM main.catalog_semantic_inputs WHERE namespace=?1 AND source_path=?2", params![self.ns,path], |r| Ok((r.get(0)?,r.get(1)?))).optional()
    }
    fn links(&self, path: &str) -> rusqlite::Result<(String, Option<String>)> {
        self.db.query_row("SELECT s.native_id,CASE WHEN octet_length(f.links)<=1048576 THEN f.links END FROM main.catalog_semantic_inputs s JOIN main.files f ON f.namespace=s.namespace AND f.path=s.source_path WHERE s.namespace=?1 AND s.source_path=?2", params![self.ns,path], |r| Ok((r.get(0)?,r.get(1)?)))
    }
    fn members(&self, kind: &str, id: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT source_path FROM main.catalog_semantic_inputs WHERE namespace=?1 AND harness='claude' AND is_top=1 AND (native_id=?2 AND ?3='lineage:claude' OR source_path IN (SELECT source_path FROM main.catalog_native_dependencies WHERE namespace=?1 AND harness=?3 AND native_id=?2)) ORDER BY source_path LIMIT 65")?.query_map(params![self.ns,id,kind], |r| r.get(0))?.collect()
    }
}

impl PublicationPlanPort for Plan<'_> {
    fn pending(&self) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM (SELECT session_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND published=0 UNION SELECT session_key FROM main.catalog_semantic_prior_owners WHERE namespace=?1 AND published=0) ORDER BY session_key LIMIT 64")?.query_map([self.ns], |r| r.get(0))?.collect()
    }
    fn sources(&self, key: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT source_path FROM (SELECT source_path FROM main.catalog_semantic_owner_sources WHERE namespace=?1 AND session_key=?2 UNION SELECT source_path FROM main.catalog_semantic_prior_sources WHERE namespace=?1 AND session_key=?2) ORDER BY source_path LIMIT 65")?.query_map(params![self.ns,key], |r| r.get(0))?.collect()
    }
    fn owners(&self, path: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM (SELECT session_key FROM main.catalog_semantic_owner_sources WHERE namespace=?1 AND source_path=?2 UNION SELECT session_key FROM main.catalog_semantic_prior_sources WHERE namespace=?1 AND source_path=?2) ORDER BY session_key LIMIT 65")?.query_map(params![self.ns,path], |r| r.get(0))?.collect()
    }
}

impl ContextPort for Plan<'_> {
    fn owners(&self, path: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM main.catalog_semantic_owner_sources WHERE namespace=?1 AND source_path=?2 ORDER BY session_key LIMIT 65")?.query_map(params![self.ns,path], |r| r.get(0))?.collect()
    }
    fn children(&self, key: &str, used: &mut usize) -> rusqlite::Result<Vec<String>> {
        let mut children = Vec::new();
        let mut q = self.db.prepare("SELECT session_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND parent_key=?2 ORDER BY session_key")?;
        for child in q.query_map(params![self.ns,key], |r| r.get::<_,String>(0))? {
            let child = child?;
            *used = used.saturating_add(child.len());
            if *used > 1048576 { return Err(rusqlite::Error::InvalidQuery); }
            children.push(child);
        }
        Ok(children)
    }
    fn cost(&self, key: &str) -> rusqlite::Result<Option<f64>> {
        self.db.query_row("SELECT family_cost FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2 AND cost_done=1", params![self.ns,key], |r| r.get(0))
    }
    fn parent(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.db.query_row("SELECT parent_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2", params![self.ns,key], |r| r.get(0)).optional().map(Option::flatten)
    }
    fn endpoint(&self, key: &str, used: &mut usize) -> rusqlite::Result<Option<crate::model::relation_stage::Endpoint>> {
        let exists: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2)", params![self.ns,key], |r| r.get(0))?;
        if !exists { return Ok(None); }
        super::owners::Owners { db: self.db, namespace: self.ns }.endpoint_with_budget(key, used).map(Some)
    }
}
