//! Namespace-owned lookups for the shared native physical-owner plan.
use crate::events::store::semantic_stage::{ContextPort, OwnerClosurePort, PublicationPlanPort};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) struct Plan<'a> {
    pub(super) db: &'a Connection,
    pub(super) ns: &'a str,
}

impl OwnerClosurePort for Plan<'_> {
    fn input(&self, path: &str) -> rusqlite::Result<Option<(String, bool, String)>> {
        self.db.query_row("SELECT harness,is_top,native_id FROM main.catalog_semantic_inputs WHERE namespace=?1 AND source_path=?2", params![self.ns,path], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()
    }
    fn links(&self, path: &str) -> rusqlite::Result<(String, Option<String>)> {
        self.db.query_row("SELECT s.native_id,CASE WHEN octet_length(f.links)<=1048576 THEN f.links END FROM main.catalog_semantic_inputs s JOIN main.files f ON f.namespace=s.namespace AND f.path=s.source_path WHERE s.namespace=?1 AND s.source_path=?2", params![self.ns,path], |r| Ok((r.get(0)?,r.get(1)?)))
    }
    fn members(&self, kind: &str, id: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT source_path FROM main.catalog_semantic_inputs WHERE namespace=?1 AND harness='claude' AND is_top=1 AND (native_id=?2 AND ?3='lineage:claude' OR source_path IN (SELECT source_path FROM main.catalog_native_dependencies WHERE namespace=?1 AND harness=?3 AND native_id=?2)) ORDER BY source_path LIMIT 65")?.query_map(params![self.ns,id,kind], |r| r.get(0))?.collect()
    }
    fn native_members(&self, harness: &str, id: &str) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT source_path FROM main.catalog_semantic_inputs WHERE namespace=?1 AND harness COLLATE BINARY=?2 COLLATE BINARY AND native_id COLLATE BINARY=?3 COLLATE BINARY ORDER BY is_top,source_path COLLATE BINARY LIMIT 65")?.query_map(params![self.ns,harness,id], |r| r.get(0))?.collect()
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
        for child in q.query_map(params![self.ns, key], |r| r.get::<_, String>(0))? {
            let child = child?;
            *used = used.saturating_add(child.len());
            if *used > 1048576 {
                return Err(rusqlite::Error::InvalidQuery);
            }
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
    fn endpoint(
        &self,
        key: &str,
        used: &mut usize,
    ) -> rusqlite::Result<Option<crate::model::relation_stage::Endpoint>> {
        let exists: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2)", params![self.ns,key], |r| r.get(0))?;
        if !exists {
            return Ok(None);
        }
        super::owners::Owners {
            db: self.db,
            namespace: self.ns,
        }
        .endpoint_with_budget(key, used)
        .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::store::semantic_stage::owner_closure_with;

    fn owner_db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE main.catalog_semantic_inputs(namespace TEXT NOT NULL,source_path TEXT NOT NULL,harness TEXT COLLATE NOCASE NOT NULL,native_id TEXT COLLATE NOCASE NOT NULL,is_top INTEGER NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,source_path));",
        )
        .unwrap();
        db
    }

    fn add_owner_input(
        db: &Connection,
        namespace: &str,
        path: &str,
        harness: &str,
        native_id: &str,
        is_top: bool,
    ) {
        db.execute(
            "INSERT INTO main.catalog_semantic_inputs VALUES(?1,?2,?3,?4,?5,'{}')",
            params![namespace, path, harness, native_id, is_top],
        )
        .unwrap();
    }

    #[test]
    fn codex_native_members_are_exact_and_namespace_scoped() {
        let db = owner_db();
        for (namespace, path, harness, id, is_top) in [
            ("source-a", "z-peer", "codex", "Case-ID", true),
            ("source-a", "a-seed", "codex", "Case-ID", false),
            ("source-a", "lower-case", "codex", "case-id", false),
            ("source-a", "different-id", "codex", "other-id", true),
            ("source-a", "different-harness", "claude", "Case-ID", false),
            ("source-b", "different-namespace", "codex", "Case-ID", false),
        ] {
            add_owner_input(&db, namespace, path, harness, id, is_top);
        }

        let source_a = Plan {
            db: &db,
            ns: "source-a",
        };
        for count in [1, 2, 64] {
            let id = format!("plan-{count}");
            for i in (0..count).rev() {
                add_owner_input(
                    &db,
                    "source-a",
                    &format!("group-{count}/source-{i:02}"),
                    "codex",
                    &id,
                    i % 2 == 1,
                );
            }
            let expected = (0..count)
                .map(|i| format!("group-{count}/source-{i:02}"))
                .collect::<Vec<_>>();
            assert_eq!(
                owner_closure_with(&source_a, &expected[0]).unwrap(),
                expected
            );
        }
        assert_eq!(
            owner_closure_with(&source_a, "a-seed").unwrap(),
            vec!["a-seed".to_owned(), "z-peer".to_owned()]
        );
        for i in 0..65 {
            add_owner_input(
                &db,
                "source-a",
                &format!("overflow/source-{i:02}"),
                "codex",
                "plan-overflow",
                i % 2 == 1,
            );
        }
        match owner_closure_with(&source_a, "overflow/source-00").unwrap_err() {
            rusqlite::Error::ToSqlConversionFailure(source) => {
                assert_eq!(source.to_string(), "semantic_physical_owner_source_budget");
            }
            error => panic!("unexpected source budget error: {error}"),
        }
        let source_b = Plan {
            db: &db,
            ns: "source-b",
        };
        assert_eq!(
            owner_closure_with(&source_b, "different-namespace").unwrap(),
            vec!["different-namespace".to_owned()]
        );
    }
}
