//! Bounded physical Current ownership lookup; this grants no native authority.
use super::{sqlstate, txn::PgTxn};
use crate::{
    model::{relation_stage::LifecycleStore, summary::CatalogRow},
    storage::StorageError,
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::BTreeMap, io};

struct Owners<'a, 'tx> {
    tx: RefCell<&'a mut PgTxn<'tx>>,
    generation: &'a str,
}

fn text(bytes: Vec<u8>) -> Result<String, StorageError> {
    String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged)
}

impl LifecycleStore for Owners<'_, '_> {
    fn path_owners(&self, path: &str) -> io::Result<Vec<String>> {
        (|| {
            let tx = &mut **self.tx.borrow_mut();
            tx.check()?;
            let rows = tx.driver.query("SELECT session_key FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND path_digest=sha256($3::bytea) AND source_path=$3 ORDER BY session_key LIMIT 65", &[&tx.namespace.as_str().as_bytes(), &self.generation.as_bytes(), &path.as_bytes()]).map_err(sqlstate::error)?;
            rows.iter()
                .map(|row| text(row.get(0)))
                .collect::<Result<Vec<_>, StorageError>>()
        })()
        .map_err(io::Error::other)
    }

    fn owner(&self, key: &str) -> io::Result<CatalogRow> {
        (|| {
            let tx = &mut **self.tx.borrow_mut();
            tx.check()?;
            let ns = tx.namespace.as_str().as_bytes();
            let digest = Sha256::digest(key.as_bytes()).to_vec();
            let stored = tx.driver.query_one("SELECT session_key=$4,harness,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3", &[&ns, &self.generation.as_bytes(), &digest, &key.as_bytes()]).map_err(sqlstate::error)?;
            let row: CatalogRow = serde_json::from_slice(
                &stored
                    .get::<_, Option<Vec<u8>>>(2)
                    .ok_or(StorageError::Busy)?,
            )
            .map_err(|_| StorageError::ScopeChanged)?;
            if !stored.get::<_, bool>(0)
                || row.key != key
                || row.harness.as_bytes() != stored.get::<_, Vec<u8>>(1)
                || row.lifecycle != crate::model::summary::CatalogLifecycle::Current
            {
                return Err(StorageError::ScopeChanged);
            }
            let members = tx.driver.query("SELECT source_path,native_digest,CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata END FROM (SELECT source_path,native_digest,metadata FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 ORDER BY source_path LIMIT 65) s ORDER BY source_path", &[&ns, &self.generation.as_bytes(), &digest, &key.as_bytes()]).map_err(sqlstate::error)?;
            if members.len() > 64 || members.len() != row.sources.len() {
                return Err(StorageError::ScopeChanged);
            }
            let mut expected = row
                .sources
                .iter()
                .map(|source| {
                    (
                        source.path.to_string_lossy().into_owned(),
                        source.native_id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            expected.sort();
            let mut actual = Vec::new();
            for member in members {
                let path = text(member.get(0))?;
                let source: crate::model::summary::CatalogSource = serde_json::from_slice(
                    &member
                        .get::<_, Option<Vec<u8>>>(2)
                        .ok_or(StorageError::Busy)?,
                )
                .map_err(|_| StorageError::ScopeChanged)?;
                if source.path != std::path::Path::new(&path)
                    || Sha256::digest(source.native_id.as_bytes()).to_vec()
                        != member.get::<_, Vec<u8>>(1)
                {
                    return Err(StorageError::ScopeChanged);
                }
                actual.push((path, source.native_id));
            }
            if actual != expected {
                return Err(StorageError::ScopeChanged);
            }
            Ok(row)
        })()
        .map_err(io::Error::other)
    }

    fn neighbors(&self, key: &str) -> io::Result<Vec<String>> {
        (|| {
            let tx = &mut **self.tx.borrow_mut();
            tx.check()?;
            let rows = tx.driver.query("SELECT r.session_key FROM (SELECT peer_digest AS key_digest,peer AS session_key FROM semon_pg_native_catalog_links WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 UNION SELECT key_digest,session_key FROM semon_pg_native_catalog_links WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND peer_digest=sha256($3::bytea) AND peer=$3 UNION SELECT peer.key_digest,peer.session_key FROM semon_pg_native_catalog_sources own JOIN semon_pg_native_native_dependencies d ON d.namespace_digest=own.namespace_digest AND d.namespace=own.namespace AND d.source_path_digest=semon_pg_native_digest($1,own.source_path,'dependency.path') AND d.source_path=own.source_path JOIN semon_pg_native_native_dependencies p ON p.namespace_digest=d.namespace_digest AND p.namespace=d.namespace AND p.harness_digest=d.harness_digest AND p.harness=d.harness AND p.native_id_digest=d.native_id_digest AND p.native_id=d.native_id JOIN semon_pg_native_catalog_sources peer ON peer.namespace_digest=p.namespace_digest AND peer.namespace=p.namespace AND peer.generation=own.generation AND peer.path_digest=sha256(p.source_path) AND peer.source_path=p.source_path WHERE own.namespace_digest=sha256($1::bytea) AND own.namespace=$1 AND own.generation=$2 AND own.key_digest=sha256($3::bytea) AND own.session_key=$3 AND (d.harness='runtime:pid'::bytea OR substring(d.harness from 1 for 8)='lineage:'::bytea OR substring(d.harness from 1 for 7)='bridge:'::bytea) UNION SELECT peer.key_digest,peer.session_key FROM semon_pg_native_catalog_sources own JOIN semon_pg_native_catalog_dependencies d ON d.namespace_digest=own.namespace_digest AND d.namespace=own.namespace AND d.generation=own.generation AND d.path_digest=own.path_digest AND d.source_path=own.source_path JOIN semon_pg_native_catalog_dependencies p ON p.namespace_digest=d.namespace_digest AND p.namespace=d.namespace AND p.generation=d.generation AND p.harness_digest=d.harness_digest AND p.harness=d.harness AND p.native_digest=d.native_digest AND p.native_id=d.native_id JOIN semon_pg_native_catalog_sources peer ON peer.namespace_digest=p.namespace_digest AND peer.namespace=p.namespace AND peer.generation=p.generation AND peer.path_digest=p.path_digest AND peer.source_path=p.source_path WHERE own.namespace_digest=sha256($1::bytea) AND own.namespace=$1 AND own.generation=$2 AND own.key_digest=sha256($3::bytea) AND own.session_key=$3 AND d.harness_digest=sha256('runtime:pid'::bytea) AND d.harness='runtime:pid'::bytea) neighbor JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2 AND r.key_digest=neighbor.key_digest AND r.session_key=neighbor.session_key ORDER BY r.session_key LIMIT 65", &[&tx.namespace.as_str().as_bytes(), &self.generation.as_bytes(), &key.as_bytes()]).map_err(sqlstate::error)?;
            if rows.len() > 64 {
                return Err(StorageError::Busy);
            }
            rows.iter()
                .map(|row| text(row.get(0)))
                .collect::<Result<Vec<_>, StorageError>>()
        })()
        .map_err(io::Error::other)
    }
}

pub(super) fn affected(
    tx: &mut PgTxn<'_>,
    generation: Option<&str>,
    paths: &[String],
) -> Result<BTreeMap<String, CatalogRow>, StorageError> {
    let Some(generation) = generation else {
        return Ok(BTreeMap::new());
    };
    crate::model::relation_stage::affected_owners(
        &Owners {
            tx: RefCell::new(tx),
            generation,
        },
        paths,
    )
    .map_err(|error| {
        error
            .get_ref()
            .and_then(|error| error.downcast_ref::<StorageError>())
            .copied()
            .unwrap_or(StorageError::Busy)
    })
}
