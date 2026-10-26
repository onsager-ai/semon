//! One bounded workspace selection over explicitly admitted namespaces.
use super::*;
use crate::{OverviewSource, catalog::Request, model::summary::CatalogRow, storage::{StoreBackend, StoreConfig, StorageError}};
use std::sync::Arc;

pub(crate) struct Page {
    pub(crate) generations: Vec<String>,
    pub(crate) rows: Vec<(usize, CatalogRow)>,
    pub(crate) scanned: usize,
    pub(crate) more: bool,
}

pub(crate) fn read(
    sources: &[&OverviewSource], request: &Request, after: Option<(i64, &str, &str)>,
) -> Result<Option<Page>, StorageError> {
    let configs = sources.iter().map(|source| source.options.source.config(&source.options.cache)).collect::<Result<Vec<_>, _>>()?;
    if !configs.iter().any(|config| matches!(config.backend, StoreBackend::Postgres { .. })) {
        return Ok(None);
    }
    let Some(StoreConfig { backend: StoreBackend::Postgres { pool, control }, .. }) = configs.first() else {
        return Err(StorageError::MultiSourceUnqualified);
    };
    let mut bindings = Vec::new();
    let mut generations = Vec::new();
    for (source, config) in sources.iter().zip(&configs) {
        let StoreBackend::Postgres { pool: other, control: source_control } = &config.backend else { return Err(StorageError::MultiSourceUnqualified); };
        if !Arc::ptr_eq(&pool.inner, &other.inner) || source.key != config.namespace.as_str() {
            return Err(StorageError::MultiSourceUnqualified);
        }
        let bound = other.bind_source(&config.namespace, config.expected_stream_epoch.as_deref(), source_control)?;
        bindings.push(bound.source_binding.ok_or(StorageError::ScopeChanged)?);
        generations.push(super::configured_generation(config, other, source_control)?);
    }
    let names = configs.iter().map(|config| config.namespace.as_str().as_bytes().to_vec()).collect::<Vec<_>>();
    let scope = configs.iter().map(|config| super::namespace_context(&config.namespace)).collect::<Vec<_>>().join(",");
    let rows = pool.transaction(&configs[0].namespace, txn::TxnKind::ReadSnapshot, control, |tx| {
        tx.driver.query_one("SELECT set_config('semon.read_scope',$1,true)", &[&scope]).map_err(sqlstate::error)?;
        // Every namespace is validated in this same repeatable-read snapshot;
        // empty result sets therefore still prove admission, epoch and head.
        for ((config, binding), generation) in configs.iter().zip(&bindings).zip(&generations) {
            let valid: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_semantic_runtime r JOIN semon_pg_source_state s USING(namespace_digest,namespace) WHERE r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.admission=$2 AND r.stream_epoch=$3 AND s.catalog_generation=$4 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_meta m WHERE m.namespace_digest=r.namespace_digest AND m.namespace=r.namespace AND m.key IN ('source-unenrolled','native_transfer_v1:incoming') AND octet_length(m.value)>0) AND EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations o WHERE o.namespace_digest=r.namespace_digest AND o.namespace=r.namespace AND o.generation=convert_to($4,'UTF8')))", &[&config.namespace.as_str().as_bytes(), &(binding.admission as i64), &binding.stream_epoch.as_bytes(), generation]).map_err(sqlstate::error)?.get(0);
            if !valid { return Err(StorageError::ScopeChanged); }
        }
        let epochs = generations.iter().map(|generation| generation.as_bytes().to_vec()).collect::<Vec<_>>();
        let harness = request.harness.as_ref().map(|value| value.as_bytes());
        let repo = request.repo.as_ref().map(|value| value.as_bytes());
        let model = request.model.as_ref().map(|value| value.as_bytes());
        let last = after.map(|value| value.0);
        let key = after.map(|value| value.1.as_bytes());
        let source = after.map(|value| value.2.as_bytes());
        let history = request.scope == crate::CatalogReadScope::RetainedHistory;
        let limit = if request.q.is_some() { crate::catalog_search::MAX_CANDIDATES } else { request.limit + 1 } as i64;
        let rows = tx.driver.query("WITH authorized AS (SELECT * FROM unnest($1::bytea[],$2::bytea[]) AS a(namespace,generation)), visible AS (
            SELECT r.namespace,r.session_key,r.last_ms,r.harness,r.repo,r.model,r.metadata FROM authorized a JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=sha256(a.namespace) AND r.namespace=a.namespace AND r.generation=a.generation WHERE NOT $10 OR NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=r.namespace_digest AND o.namespace=r.namespace AND o.generation=r.generation AND o.key_digest=r.key_digest AND o.session_key=r.session_key)
            UNION ALL SELECT o.namespace,o.session_key,o.last_ms,o.harness,o.repo,o.model,h.metadata FROM authorized a JOIN semon_pg_native_catalog_history_owners o ON o.namespace_digest=sha256(a.namespace) AND o.namespace=a.namespace AND o.generation=a.generation JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key WHERE $10
        ), page AS MATERIALIZED (SELECT * FROM visible WHERE ($3::bytea IS NULL OR harness=$3) AND ($4::bytea IS NULL OR repo=$4) AND ($5::bytea IS NULL OR model=$5) AND ($6::bigint IS NULL OR last_ms<$6 OR (last_ms=$6 AND (session_key>$7 OR (session_key=$7 AND namespace>$8)))) ORDER BY last_ms DESC,session_key,namespace LIMIT $9)
        SELECT namespace,session_key,last_ms,harness,repo,model,CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata END FROM page ORDER BY last_ms DESC,session_key,namespace", &[&names,&epochs,&harness,&repo,&model,&last,&key,&source,&limit,&history]).map_err(sqlstate::error)?;
        rows.into_iter().map(|row| {
            let namespace: Vec<u8> = row.get(0);
            let index = names.iter().position(|name| name == &namespace).ok_or(StorageError::NamespaceMismatch)?;
            let bytes = row.get::<_, Option<Vec<u8>>>(6).ok_or(StorageError::Busy)?;
            let value: CatalogRow = serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
            if value.key.as_bytes() != row.get::<_, Vec<u8>>(1) || value.last.unwrap_or(0) != row.get::<_, i64>(2) || value.harness.as_bytes() != row.get::<_, Vec<u8>>(3) || value.repo.as_deref().unwrap_or("").as_bytes() != row.get::<_, Vec<u8>>(4) || value.model.as_bytes() != row.get::<_, Vec<u8>>(5) { return Err(StorageError::ScopeChanged); }
            Ok((index,value))
        }).collect::<Result<Vec<_>, _>>()
    })?;
    let scanned = rows.len();
    let more = if request.q.is_some() { scanned == crate::catalog_search::MAX_CANDIDATES } else { scanned > request.limit };
    Ok(Some(Page { generations, rows, scanned, more }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{HostSource, LogicalSourceNamespace};

    #[test]
    fn pg_vertical140_workspace_sql_scopes_ties_filters_and_write_refusal() {
        let (pool, _) = super::super::tests::fixture(|_| {});
        let control = PgCancellation::default();
        let generation = "a".repeat(64);
        let mut sources = Vec::new();
        let mut configs = Vec::new();
        for name in ["workspace-a", "workspace-b", "workspace-hidden"] {
            let namespace = LogicalSourceNamespace::new(name.to_owned()).unwrap();
            let source = HostSource::new(namespace.clone(), "e".repeat(64)).unwrap();
            let config = source.store_config(StoreBackend::Postgres { pool: pool.clone(), control: control.clone() }).unwrap();
            config.create_native_store().unwrap();
            let mut options = crate::Options::default();
            options.source = source.with_store(config.clone()).unwrap();
            pool.transaction(&namespace, txn::TxnKind::Write, &control, |tx| {
                let ns = namespace.as_str().as_bytes();
                tx.driver.execute("UPDATE semon_pg_source_state SET catalog_generation=$2 WHERE namespace=$1", &[&ns,&generation]).map_err(sqlstate::error)?;
                tx.driver.execute("INSERT INTO semon_pg_native_catalog_operations(namespace,operation_id,payload,generation,stage_epoch) VALUES($1,'op','payload',$2,1)", &[&ns,&generation.as_bytes()]).map_err(sqlstate::error)?;
                for (key,last,model) in [("tie",100,"m"),("older",90,"other")] {
                    let row = CatalogRow {
                        overview:None,trace:None,query:None,lifecycle:Default::default(),
                        key:key.into(),name:format!("Écho {name}"),harness:"claude".into(),kind:"session".into(),presentation_kind:None,
                        native_ids:vec![],parent:None,parent_source:None,repo:Some("repo".into()),branch:None,model:model.into(),effort:None,
                        start:Some(1),last:Some(last),tokens:[0.0;3],cost:Default::default(),sources:vec![],
                    };
                    let metadata = serde_json::to_vec(&row).unwrap();
                    tx.driver.execute("INSERT INTO semon_pg_native_catalog_rows(namespace,generation,key_digest,session_key,metadata,last_ms,harness,repo,model) VALUES($1,$2,sha256($3::bytea),$3,$4,$5,'claude','repo',$6)", &[&ns,&generation.as_bytes(),&key.as_bytes(),&metadata,&last,&model.as_bytes()]).map_err(sqlstate::error)?;
                }
                tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,'workspace-probe','yes')", &[&ns]).map_err(sqlstate::error)?;
                Ok(())
            }).unwrap();
            sources.push(OverviewSource { key:name.into(),label:name.into(),options,up:true });
            configs.push(config);
        }
        let authorized = vec![&sources[0],&sources[1]];
        let request = Request::parse("limit=1&harness=claude&repo=repo&model=m").unwrap();
        let first = read(&authorized,&request,None).unwrap().unwrap();
        assert_eq!(first.rows.iter().map(|(index,row)|(*index,row.key.as_str())).collect::<Vec<_>>(),vec![(0,"tie"),(1,"tie")]);
        let second = read(&authorized,&request,Some((100,"tie","workspace-a"))).unwrap().unwrap();
        assert_eq!(second.rows.iter().map(|(index,row)|(*index,row.key.as_str())).collect::<Vec<_>>(),vec![(1,"tie")]);
        let scope = authorized.iter().map(|source| super::super::namespace_context(&source.options.source.namespace().unwrap())).collect::<Vec<_>>().join(",");
        pool.transaction(&configs[0].namespace,txn::TxnKind::Write,&control,|tx| {
            tx.driver.query_one("SELECT set_config('semon.read_scope',$1,true)", &[&scope]).map_err(sqlstate::error)?;
            let rows = tx.driver.query("SELECT namespace FROM semon_pg_native_meta WHERE key='workspace-probe' ORDER BY namespace", &[]).map_err(sqlstate::error)?;
            assert_eq!(rows.len(),2);
            let other = configs[1].namespace.as_str().as_bytes();
            assert_eq!(tx.driver.execute("UPDATE semon_pg_native_meta SET value='no' WHERE namespace=$1", &[&other]).map_err(sqlstate::error)?,0);
            assert_eq!(tx.driver.execute("DELETE FROM semon_pg_native_meta WHERE namespace=$1", &[&other]).map_err(sqlstate::error)?,0);
            Ok(())
        }).unwrap();
        assert!(pool.transaction(&configs[0].namespace,txn::TxnKind::Write,&control,|tx| {
            tx.driver.query_one("SELECT set_config('semon.read_scope',$1,true)", &[&scope]).map_err(sqlstate::error)?;
            tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,'forbidden','no')", &[&configs[1].namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
            Ok(())
        }).is_err());
        let admission = configs[1].admit_namespace().unwrap();
        configs[1].unenroll_namespace(&admission).unwrap();
        assert_eq!(read(&authorized,&request,None).err(),Some(StorageError::NamespaceUnadmitted));
    }
}
