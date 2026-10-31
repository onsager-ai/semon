//! Typed owner effects for the existing namespace-leading semantic family.
use crate::model::relation_stage::{Endpoint, OwnerStore, Relation, Withdrawal};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) struct Owners<'a> {
    pub(super) db: &'a Connection,
    pub(super) namespace: &'a str,
}

fn invalid(error: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error.to_string())))
}

pub(super) fn validate(db: &Connection) -> Result<(), crate::storage::StorageError> {
    use crate::storage::StorageError;
    let shape: Option<String> = db.query_row("SELECT group_concat(name||':'||type||':'||[notnull]||':'||pk,',') FROM (SELECT * FROM pragma_table_info('catalog_native_dependencies','main') ORDER BY cid)",[],|r|r.get(0)).map_err(super::failed)?;
    let strict: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema='main' AND name='catalog_native_dependencies' AND strict=1 AND wr=1)",[],|r|r.get(0)).map_err(super::failed)?;
    let index: Option<String> = db.query_row("SELECT group_concat(name,',') FROM (SELECT * FROM pragma_index_xinfo('catalog_native_dependency','main') WHERE key=1 ORDER BY seqno)",[],|r|r.get(0)).map_err(super::failed)?;
    let valid_index: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_index_list('catalog_native_dependencies','main') WHERE name='catalog_native_dependency' AND origin='c' AND [unique]=0 AND partial=0)",[],|r|r.get(0)).map_err(super::failed)?;
    if shape.as_deref()
        != Some("namespace:TEXT:1:1,source_path:TEXT:1:2,harness:TEXT:1:3,native_id:TEXT:1:4")
        || !strict
        || index.as_deref() != Some("namespace,harness,native_id,source_path")
        || !valid_index
    {
        return Err(StorageError::NamespaceUpgradePending);
    }
    // Endpoints, sources/withdrawal and relations are validated byte-for-byte
    // against authored current DDL by support_sql/stage_layout before admission.
    Ok(())
}

impl OwnerStore for Owners<'_> {
    type Error = rusqlite::Error;
    fn retire(&self, key: &str, withdrawal: bool) -> rusqlite::Result<()> {
        self.retire_current(key, withdrawal)
    }
    fn endpoint(&self, endpoint: &Endpoint) -> rusqlite::Result<()> {
        let parents = endpoint
            .native_parents
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        if parents.len() > 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let parent = parents.into_iter().next();
        let task = endpoint
            .paths
            .first()
            .and_then(|p| p.rsplit('/').next())
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .or_else(|| endpoint.nickname.clone());
        let prior: Option<(String, String)> = self.db.query_row(
            "SELECT harness,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",
            params![self.namespace, endpoint.row.key], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        if let Some((harness, text)) = prior {
            let prior: Endpoint = serde_json::from_str(&text).map_err(invalid)?;
            if harness != endpoint.row.harness
                || prior.row.harness != harness
                || prior.row.key != endpoint.row.key
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        let metadata = serde_json::to_string(endpoint).map_err(invalid)?;
        self.db.execute("INSERT INTO main.catalog_semantic_endpoints(namespace,session_key,harness,parent_native,parent_native_harness,parent_key,task,nickname,cost,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(namespace,session_key) DO UPDATE SET metadata=excluded.metadata,parent_native=excluded.parent_native,parent_native_harness=excluded.parent_native_harness,parent_key=excluded.parent_key,task=excluded.task,nickname=excluded.nickname,cost=excluded.cost",params![self.namespace,endpoint.row.key,endpoint.row.harness,parent.map(|p|&p.1),parent.map(|p|&p.0),endpoint.parent,task,endpoint.nickname,endpoint.row.cost.usd,metadata])?;
        for path in &endpoint.paths {
            self.db.execute("INSERT OR IGNORE INTO main.catalog_semantic_paths(namespace,path,session_key) VALUES(?1,?2,?3)",params![self.namespace,path,endpoint.row.key])?;
        }
        for name in &endpoint.names {
            self.db.execute("INSERT OR IGNORE INTO main.catalog_semantic_names(namespace,name,session_key) VALUES(?1,?2,?3)",params![self.namespace,name,endpoint.row.key])?;
        }
        crate::model::relation_stage::native_name_hints_with(
            endpoint,
            || rusqlite::Error::InvalidQuery,
            |path, name| self.dependency(path, "name:claude", name),
        )?;
        if let Some(pid) = endpoint.pid {
            for source in &endpoint.row.sources {
                self.dependency(
                    source.path.to_string_lossy().as_ref(),
                    "runtime:pid",
                    &pid.to_string(),
                )?;
            }
        }
        Ok(())
    }
    fn relation(&self, relation: &Relation) -> rusqlite::Result<()> {
        let old: Option<Option<String>> = self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.catalog_semantic_relations WHERE namespace=?1 AND relation_id=?2",params![self.namespace,relation.id],|r|r.get(0)).optional()?;
        let merged = if let Some(old) = old {
            let old = old.ok_or(rusqlite::Error::InvalidQuery)?;
            relation
                .merge_prior(&serde_json::from_str(&old).map_err(invalid)?)
                .map_err(invalid)?
        } else {
            relation.clone()
        };
        let metadata = serde_json::to_string(&merged).map_err(invalid)?;
        if metadata.len() > 1048576 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        self.db.execute("INSERT INTO main.catalog_semantic_relations(namespace,relation_id,from_key,to_key,metadata) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,relation_id) DO UPDATE SET metadata=excluded.metadata",params![self.namespace,merged.id,merged.from,merged.to,metadata])?;
        Ok(())
    }
    fn dependency(&self, path: &str, harness: &str, native: &str) -> rusqlite::Result<()> {
        self.db.execute("INSERT OR IGNORE INTO main.catalog_native_dependencies(namespace,source_path,harness,native_id) VALUES(?1,?2,?3,?4)",params![self.namespace,path,harness,native])?;
        Ok(())
    }
    fn withdrawal(&self, withdrawal: &Withdrawal) -> rusqlite::Result<()> {
        let metadata = serde_json::to_string(withdrawal).map_err(invalid)?;
        if metadata.len() > 1048576 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let previous: Option<String> = self.db.query_row(
            "SELECT withdrawal_authority FROM main.catalog_semantic_stage WHERE namespace=?1",
            [self.namespace],
            |r| r.get(0),
        )?;
        if let Some(previous) = previous {
            let previous: Withdrawal = serde_json::from_str(&previous).map_err(invalid)?;
            if previous.facts_path != withdrawal.facts_path
                || previous.facts_identity != withdrawal.facts_identity
                || previous.runtime != withdrawal.runtime
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
        } else {
            self.db.execute(
                "UPDATE main.catalog_semantic_stage SET withdrawal_authority=?2 WHERE namespace=?1",
                params![self.namespace, metadata],
            )?;
        }
        let ledger = crate::events::store::ledger_row(self.db, &withdrawal.path)?
            .map(|(_, l)| serde_json::to_string(&l))
            .transpose()
            .map_err(invalid)?;
        if self.db.execute("UPDATE main.catalog_semantic_sources SET withdrawal=?3,ledger=?4,index_done=1,events_done=1,identity_done=1,relation_done=1,(dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256)=(SELECT dev,ino,size,mtime_ns,ctime_ns,resume_at,head_sha256,tail_sha256 FROM main.files WHERE namespace=?1 AND path=?2) WHERE namespace=?1 AND source_path=?2 AND index_done=0",params![self.namespace,withdrawal.path,metadata,ledger])? != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }
}

enum ReplayEndpoint {
    Staged(Box<Endpoint>),
    Unpublished,
    Virtual,
    Missing,
}

impl crate::model::relation_stage::LifecycleStore for Owners<'_> {
    fn path_owners(&self, path: &str) -> std::io::Result<Vec<String>> {
        let result = (|| -> rusqlite::Result<Vec<String>> {
            self.db.prepare("SELECT session_key FROM main.session_catalog_sources WHERE namespace=?1 AND source_path=?2 AND lifecycle='current' ORDER BY session_key LIMIT 65")?
                .query_map(params![self.namespace,path], |r|r.get(0))?.collect()
        })();
        result.map_err(std::io::Error::other)
    }
    fn owner(&self, key: &str) -> std::io::Result<crate::model::summary::CatalogRow> {
        let text: String = self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.session_catalog WHERE namespace=?1 AND session_key=?2 AND lifecycle='current'",params![self.namespace,key],|r|r.get(0)).map_err(std::io::Error::other)?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        let members = (|| -> rusqlite::Result<Vec<(String,String,String)>> {
            self.db.prepare("SELECT source_path,harness,native_id FROM main.session_catalog_sources WHERE namespace=?1 AND session_key=?2 AND lifecycle='current' ORDER BY source_path LIMIT 65")?
                .query_map(params![self.namespace,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect()
        })().map_err(std::io::Error::other)?;
        let mut expected = row
            .sources
            .iter()
            .map(|s| {
                (
                    s.path.to_string_lossy().into_owned(),
                    row.harness.clone(),
                    s.native_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        expected.sort();
        if row.lifecycle != crate::model::summary::CatalogLifecycle::Current
            || members.len() > 64
            || members != expected
        {
            return Err(std::io::Error::other("semantic_affected_owner_membership"));
        }
        Ok(row)
    }
    fn neighbors(&self, key: &str) -> std::io::Result<Vec<String>> {
        // These are physical incident indexes, not a relationship reducer.
        // Trace stubs without Current owners carry no Raw membership. Every
        // real Current neighbor is enrolled and parsed by the shared producer.
        let result = (|| -> rusqlite::Result<Vec<String>> {
            let mut q = self.db.prepare("SELECT session_key FROM main.session_catalog WHERE namespace=?1 AND lifecycle='current' AND session_key IN (SELECT parent_key FROM main.session_catalog WHERE namespace=?1 AND session_key=?2 UNION SELECT session_key FROM main.session_catalog WHERE namespace=?1 AND parent_key=?2 AND lifecycle='current' UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM main.session_trace_edges INDEXED BY session_trace_from WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?2 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM main.session_trace_edges INDEXED BY session_trace_from WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?2 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM main.session_trace_edges INDEXED BY session_trace_to WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?2 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM main.session_trace_edges INDEXED BY session_trace_to WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?2 UNION SELECT peer.session_key FROM main.session_catalog_sources own CROSS JOIN main.catalog_native_dependencies d ON d.namespace=own.namespace AND d.source_path=own.source_path CROSS JOIN main.catalog_native_dependencies p INDEXED BY catalog_native_dependency ON p.namespace=d.namespace AND p.harness=d.harness AND p.native_id=d.native_id CROSS JOIN main.session_catalog_sources peer INDEXED BY session_catalog_current_source_path ON peer.namespace=p.namespace AND peer.source_path=p.source_path AND peer.lifecycle='current' WHERE own.namespace=?1 AND own.session_key=?2 AND own.lifecycle='current' AND (d.harness='runtime:pid' OR d.harness LIKE 'lineage:%' OR d.harness LIKE 'bridge:%')) ORDER BY session_key LIMIT 65")?;
            let rows = q
                .query_map(params![self.namespace, key], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            if rows.len() > 64 {
                return Err(invalid("semantic_affected_owner_budget"));
            }
            Ok(rows)
        })();
        result.map_err(std::io::Error::other)
    }
}

impl Owners<'_> {
    pub(super) fn neighbor_page(
        &self,
        key: &str,
        after: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<String>> {
        self.db.prepare("SELECT session_key FROM main.session_catalog WHERE namespace=?1 AND lifecycle='current' AND session_key IN (SELECT parent_key FROM main.session_catalog WHERE namespace=?1 AND session_key=?2 UNION SELECT session_key FROM main.session_catalog WHERE namespace=?1 AND parent_key=?2 AND lifecycle='current' UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM main.session_trace_edges INDEXED BY session_trace_from WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?2 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM main.session_trace_edges INDEXED BY session_trace_from WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?2 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END FROM main.session_trace_edges INDEXED BY session_trace_to WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?2 UNION SELECT CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END FROM main.session_trace_edges INDEXED BY session_trace_to WHERE namespace=?1 AND CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?2 UNION SELECT peer.session_key FROM main.session_catalog_sources own CROSS JOIN main.catalog_native_dependencies d ON d.namespace=own.namespace AND d.source_path=own.source_path CROSS JOIN main.catalog_native_dependencies p INDEXED BY catalog_native_dependency ON p.namespace=d.namespace AND p.harness=d.harness AND p.native_id=d.native_id CROSS JOIN main.session_catalog_sources peer INDEXED BY session_catalog_current_source_path ON peer.namespace=p.namespace AND peer.source_path=p.source_path AND peer.lifecycle='current' WHERE own.namespace=?1 AND own.session_key=?2 AND own.lifecycle='current' AND (d.harness='runtime:pid' OR d.harness LIKE 'lineage:%' OR d.harness LIKE 'bridge:%')) AND session_key>?3 ORDER BY session_key LIMIT ?4")?.query_map(params![self.namespace,key,after,limit as i64], |r| r.get(0))?.collect()
    }

    /// Capture the original Current observation before the discovery barrier.
    /// Later publications read this immutable row rather than the moving head.
    pub(super) fn snapshot(&self, key: &str) -> rusqlite::Result<usize> {
        let exists: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_prior_owners WHERE namespace=?1 AND session_key=?2)", params![self.namespace,key], |r| r.get(0))?;
        if exists {
            return Ok(0);
        }
        let row =
            crate::model::relation_stage::LifecycleStore::owner(self, key).map_err(invalid)?;
        let text = serde_json::to_string(&row).map_err(invalid)?;
        if text.len() > 1048576 || row.sources.is_empty() || row.sources.len() > 64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        self.db.execute("INSERT INTO main.catalog_semantic_prior_owners(namespace,session_key,metadata) VALUES(?1,?2,?3)", params![self.namespace,key,text])?;
        for source in &row.sources {
            self.db.execute("INSERT INTO main.catalog_semantic_prior_sources(namespace,session_key,source_path) VALUES(?1,?2,?3)", params![self.namespace,key,source.path.to_string_lossy()])?;
        }
        for kind in ["members", "oldneighbors"] {
            self.db.execute("INSERT OR IGNORE INTO main.catalog_semantic_frontier(namespace,kind,value,harness) VALUES(?1,?2,?3,'')", params![self.namespace,kind,key])?;
        }
        Ok(text.len())
    }

    pub(super) fn prior(
        &self,
        paths: &[String],
    ) -> rusqlite::Result<std::collections::BTreeMap<String, crate::model::summary::CatalogRow>>
    {
        let selected = serde_json::to_string(paths).map_err(invalid)?;
        let mut q = self.db.prepare("SELECT o.session_key,CASE WHEN octet_length(o.metadata)<=1048576 THEN o.metadata END FROM main.catalog_semantic_prior_owners o WHERE o.namespace=?1 AND o.session_key IN (SELECT session_key FROM main.catalog_semantic_prior_sources WHERE namespace=?1 AND source_path IN (SELECT value FROM json_each(?2))) ORDER BY o.session_key LIMIT 65")?;
        let mut out = std::collections::BTreeMap::new();
        let mut used = 0usize;
        for row in q.query_map(params![self.namespace, selected], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })? {
            let (key, text) = row?;
            let text = text.ok_or(rusqlite::Error::InvalidQuery)?;
            used = used.saturating_add(text.len());
            if out.len() == 64 || used > 1048576 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            out.insert(key, serde_json::from_str(&text).map_err(invalid)?);
        }
        Ok(out)
    }

    fn retire_current(&self, key: &str, withdrawal: bool) -> rusqlite::Result<()> {
        self.db.execute("UPDATE main.session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE namespace=?1 AND session_key=?2 AND lifecycle='current'",params![self.namespace,key])?;
        self.db.execute("UPDATE main.session_catalog_sources SET lifecycle='retained' WHERE namespace=?1 AND session_key=?2",params![self.namespace,key])?;
        // Preserve the existing explicit-withdrawal contract. Non-withdrawal
        // retirement never rewrites original History or any retained recipe.
        if withdrawal {
            self.db.execute("UPDATE main.session_history_catalog SET metadata=json_set(metadata,'$.lifecycle','retained') WHERE namespace=?1 AND session_key=?2",params![self.namespace,key])?;
        }
        Ok(())
    }

    fn replay_endpoint(&self, key: &str, virtual_owner: bool) -> rusqlite::Result<ReplayEndpoint> {
        let exists: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2)",params![self.namespace,key],|r|r.get(0))?;
        if exists {
            return self
                .endpoint(key)
                .map(|endpoint| ReplayEndpoint::Staged(Box::new(endpoint)));
        }
        if virtual_owner {
            return Ok(ReplayEndpoint::Virtual);
        }
        // Only an allocated identity whose real indexed source is still
        // awaiting identity reduction is positively known to be unpublished.
        let pending: bool = self.db.query_row("SELECT NOT EXISTS(SELECT 1 FROM main.session_catalog WHERE namespace=?1 AND session_key=?2) AND EXISTS(SELECT 1 FROM main.catalog_semantic_keys k JOIN main.catalog_semantic_inputs i ON i.namespace=k.namespace AND i.harness=k.harness AND i.native_id=k.native_id JOIN main.catalog_semantic_sources s ON s.namespace=i.namespace AND s.source_path=i.source_path WHERE k.namespace=?1 AND k.session_key=?2 AND s.index_done=1 AND s.identity_done=0)",params![self.namespace,key],|r|r.get(0))?;
        if pending {
            Ok(ReplayEndpoint::Unpublished)
        } else {
            Ok(ReplayEndpoint::Missing)
        }
    }
    pub(super) fn replay(
        &self,
        epoch: i64,
        owners: &[String],
        after: &str,
    ) -> rusqlite::Result<crate::model::relation_stage::Context> {
        let mut keys = std::collections::BTreeSet::new();
        for owner in owners {
            for column in ["from_key", "to_key"] {
                let mut q = self.db.prepare(&format!("SELECT relation_id FROM main.catalog_semantic_relations WHERE namespace=?1 AND {column}=?2 AND relation_id>?3 ORDER BY relation_id LIMIT 64"))?;
                for key in q.query_map(params![self.namespace, owner, after], |r| {
                    r.get::<_, String>(0)
                })? {
                    keys.insert(key?);
                    if keys.len() > 64 {
                        keys.pop_last();
                    }
                }
            }
        }
        let mut out = crate::model::relation_stage::Context {
            epoch: Some(epoch),
            ..Default::default()
        };
        let mut bytes = 0usize;
        if after.is_empty() {
            for owner in owners {
                let endpoint = match self.replay_endpoint(owner, false)? {
                    ReplayEndpoint::Staged(endpoint) => *endpoint,
                    ReplayEndpoint::Unpublished => {
                        return Err(invalid("semantic replay owner identity is unpublished"));
                    }
                    ReplayEndpoint::Missing => {
                        return Err(invalid("semantic replay owner is missing"));
                    }
                    ReplayEndpoint::Virtual => return Err(rusqlite::Error::InvalidQuery),
                };
                bytes = bytes.saturating_add(serde_json::to_vec(&endpoint).map_err(invalid)?.len());
                if bytes > 1048576 {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let cost: Option<f64> = self.db.query_row("SELECT family_cost FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",params![self.namespace,owner],|r|r.get(0))?;
                out.family_costs.insert(owner.clone(), cost);
                out.endpoints.insert(owner.clone(), endpoint);
            }
        }
        for key in keys {
            let text: String = self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM main.catalog_semantic_relations WHERE namespace=?1 AND relation_id=?2",params![self.namespace,key],|r|r.get(0))?;
            let relation: Relation = serde_json::from_str(&text).map_err(invalid)?;
            let mut next = bytes.saturating_add(text.len());
            let mut endpoints = std::collections::BTreeMap::new();
            for key in std::iter::once(&relation.from).chain(relation.to.iter()) {
                if out.endpoints.contains_key(key) || endpoints.contains_key(key) {
                    continue;
                }
                match self
                    .replay_endpoint(key, relation.stubs.iter().any(|stub| &stub.key == key))?
                {
                    ReplayEndpoint::Staged(endpoint) => {
                        next = next
                            .saturating_add(serde_json::to_vec(&endpoint).map_err(invalid)?.len());
                        endpoints.insert(key.clone(), *endpoint);
                    }
                    ReplayEndpoint::Virtual => {}
                    ReplayEndpoint::Unpublished => {
                        return Err(invalid("semantic replay endpoint identity is unpublished"));
                    }
                    ReplayEndpoint::Missing => {
                        return Err(invalid("semantic replay endpoint is missing"));
                    }
                }
            }
            if next > 1048576 {
                if out.relations.is_empty() {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                break;
            }
            bytes = next;
            out.endpoints.extend(endpoints);
            out.relations.insert(key, relation);
        }
        Ok(out)
    }

    pub(super) fn endpoint(&self, key: &str) -> rusqlite::Result<Endpoint> {
        self.endpoint_with_budget(key, &mut 0)
    }
    pub(super) fn endpoint_with_budget(
        &self,
        key: &str,
        used: &mut usize,
    ) -> rusqlite::Result<Endpoint> {
        let (text,parent,root):(String,Option<String>,Option<String>) = self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END,parent_key,root_key FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",params![self.namespace,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        *used = used.saturating_add(text.len());
        if *used > 1048576 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let mut endpoint: Endpoint = serde_json::from_str(&text).map_err(invalid)?;
        endpoint.parent = parent.clone();
        endpoint.row.parent = parent;
        endpoint.root = root.unwrap_or_else(|| key.to_owned());
        endpoint.root_alive = self.db.query_row("SELECT coalesce(json_extract(metadata,'$.alive'),0) FROM main.catalog_semantic_endpoints WHERE namespace=?1 AND session_key=?2",params![self.namespace,endpoint.root],|r|r.get(0)).optional()?.ok_or(rusqlite::Error::InvalidQuery)?;
        Ok(endpoint)
    }
}
