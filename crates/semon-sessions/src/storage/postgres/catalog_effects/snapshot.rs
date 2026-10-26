//! Borrowed immutable metadata reads. These are physical observations, not a
//! selected ReaderLease, Raw permission, or an admitted CatalogSnapshot.
use super::*;
use crate::storage::{
    LogicalSourceNamespace,
    postgres::{PgCancellation, PgPool, txn::TxnKind},
};

mod admitted;
mod context;
pub(in crate::storage::postgres) mod query;

/// Lease acquisition checks an acknowledged publication, never just a physical
/// metadata address. Immutable proof refs survive newer heads; no GC is exposed.
pub(in crate::storage::postgres) fn with_admitted_generation<T>(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    generation: &str,
    read: impl FnOnce(&dyn catalog::CatalogSnapshot) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    with_admitted_snapshot(pool, namespace, control, generation, |snapshot| {
        read(snapshot)
    })
}

pub(in crate::storage::postgres) fn with_admitted_snapshot<T>(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    generation: &str,
    read: impl FnOnce(&Snapshot<'_, '_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let lease = pool.transaction(namespace, TxnKind::Write, control, |tx| {
        let ns = namespace.as_str().as_bytes();
        let row = tx.driver.query_one("INSERT INTO semon_pg_native_catalog_leases(namespace,lease_id,generation,expires_at) SELECT $1,convert_to(gen_random_uuid()::text,'UTF8'),$2,(extract(epoch FROM clock_timestamp())*1000)::bigint+60000 WHERE EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2) RETURNING lease_id", &[&ns, &generation.as_bytes()]).map_err(sqlstate::error)?;
        Ok(row.get::<_, Vec<u8>>(0))
    })?;
    let result = with_generation(pool, namespace, control, generation, |snapshot| {
        let now = crate::model::now_ms();
        let tx = snapshot.tx.lock().map_err(|_| StorageError::Unavailable)?;
        let live: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_leases WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND lease_id=$2 AND generation=$3 AND expires_at>$4)", &[&namespace.as_str().as_bytes(), &lease, &generation.as_bytes(), &now]).map_err(sqlstate::error)?.get(0);
        drop(tx);
        if !live {
            return Err(StorageError::ScopeChanged);
        }
        read(snapshot)
    });
    // A cancelled transaction may prevent release; the lease still has a hard
    // expiry. Cleanup never removes a proof generation.
    let _ = pool.transaction(namespace, TxnKind::Write, control, |tx| {
        tx.driver.execute("DELETE FROM semon_pg_native_catalog_leases WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND lease_id=$2", &[&namespace.as_str().as_bytes(), &lease]).map_err(sqlstate::error)?;
        Ok(())
    });
    result
}

pub(in crate::storage::postgres) struct Snapshot<'a, 'tx> {
    pub(in crate::storage::postgres) tx: std::sync::Mutex<&'a mut PgTxn<'tx>>,
    generation: String,
}

fn text(bytes: Vec<u8>) -> Result<String, StorageError> {
    String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged)
}

impl crate::storage::navigation::NativeNavigationStore for Snapshot<'_, '_> {
    fn catalog_generation(&self) -> Result<String, StorageError> {
        use crate::storage::catalog::CatalogSnapshot;
        self.require_current().map_err(|_| StorageError::ScopeChanged)?;
        Ok(self.generation.clone())
    }

    fn bookmarked_target_keys(&self, target: &str) -> Result<Vec<String>, StorageError> {
        use crate::storage::catalog::CatalogSnapshot;
        if target.len() > 4096 {
            return Err(StorageError::InvalidArguments);
        }
        self.require_current().map_err(|_| StorageError::ScopeChanged)?;
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let rows = tx.driver.query(
            "SELECT DISTINCT session_key FROM semon_pg_native_catalog_trace_bookmarks WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND target_digest=sha256($3::bytea) AND target=$3 ORDER BY session_key LIMIT 3",
            &[&tx.namespace.as_str().as_bytes(), &self.generation.as_bytes(), &target.as_bytes()],
        ).map_err(sqlstate::error)?;
        rows.iter().map(|row| text(row.get(0))).collect()
    }

    fn native_resolution(
        &self,
        harness: &str,
        native_id: &str,
        scope: crate::CatalogReadScope,
        expected_generation: Option<&str>,
    ) -> Result<crate::storage::navigation::NativeResolutionObservation, StorageError> {
        use crate::storage::catalog::CatalogSnapshot;
        if !matches!(harness, "claude" | "codex" | "copilot")
            || native_id.is_empty()
            || native_id.len() > 4096
            || native_id.chars().any(char::is_control)
        {
            return Err(StorageError::InvalidArguments);
        }
        self.require_current().map_err(|_| StorageError::ScopeChanged)?;
        if expected_generation.is_some_and(|expected| expected != self.generation) {
            return Err(StorageError::ScopeChanged);
        }
        let mut observation = crate::storage::navigation::NativeResolutionObservation {
            version: Some(crate::model::summary::CATALOG_VERSION.to_string()),
            generation: Some(self.generation.clone()),
            completeness: self.publication_value("catalog_completeness")
                .map_err(|_| StorageError::Unavailable)?,
            ..Default::default()
        };
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let args: &[&(dyn ToSql + Sync)] = &[
            &ns, &self.generation.as_bytes(), &native_id.as_bytes(), &harness.as_bytes(),
        ];
        let current = tx.driver.query(
            "SELECT DISTINCT s.session_key FROM semon_pg_native_catalog_sources s JOIN semon_pg_native_catalog_rows r USING(namespace_digest,namespace,generation,key_digest) WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.native_digest=sha256($3::bytea) AND r.harness=$4 AND convert_from(s.metadata,'UTF8')::jsonb->>'native_id'=convert_from($3,'UTF8') ORDER BY s.session_key LIMIT 2",
            args,
        ).map_err(sqlstate::error)?;
        let history = current.is_empty() && scope == crate::CatalogReadScope::RetainedHistory;
        let matches = if history {
            tx.driver.query(
                "SELECT DISTINCT s.session_key FROM semon_pg_native_catalog_history_sources s JOIN semon_pg_native_catalog_history_owners o ON o.namespace_digest=s.namespace_digest AND o.namespace=s.namespace AND o.owner_generation=s.generation AND o.key_digest=s.key_digest AND o.session_key=s.session_key WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND s.native_digest=sha256($3::bytea) AND o.harness=$4 AND convert_from(s.metadata,'UTF8')::jsonb->>'native_id'=convert_from($3,'UTF8') ORDER BY s.session_key LIMIT 2",
                args,
            ).map_err(sqlstate::error)?
        } else {
            current
        };
        observation.keys = matches.iter().map(|row| text(row.get(0))).collect::<Result<_, _>>()?;
        tx.check()?;
        drop(tx);
        if let [key] = observation.keys.as_slice() {
            observation.metadata = self.catalog(key, history)?
                .map(|row| serde_json::to_string(&row).map_err(|_| StorageError::Unavailable))
                .transpose()?;
        }
        Ok(observation)
    }
}

/// All observations in the callback use ONE repeatable-read transaction. The
/// callback cannot return a borrowed adapter or a driver value. An immutable
/// generation is an address, never a replacement for publication admission.
pub(in crate::storage::postgres) fn with_generation<T>(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    generation: &str,
    read: impl FnOnce(&Snapshot<'_, '_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    if generation.len() != 64 || !generation.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(StorageError::InvalidArguments);
    }
    pool.transaction(namespace, TxnKind::ReadSnapshot, control, |tx| {
        read(&Snapshot {
            tx: std::sync::Mutex::new(tx),
            generation: generation.to_owned(),
        })
    })
}

impl Snapshot<'_, '_> {
    pub(in crate::storage::postgres) fn selected(
        &self,
        key: &str,
        history: bool,
        start: i64,
        limit: i64,
    ) -> Result<Option<(CatalogRow, catalog::OriginalHeader, Vec<String>)>, StorageError> {
        let Some(row) = self.catalog(key, history)? else {
            return Ok(None);
        };
        let (header, slots) = self
            .recipe(key, history, start, limit)?
            .ok_or(StorageError::ScopeChanged)?;
        let source = crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::ScopeChanged)?;
        if header.2.as_deref() != Some(source.as_str()) {
            return Err(StorageError::ScopeChanged);
        }
        Ok(Some((row, header, slots)))
    }

    fn owner(&self, key: &str) -> Result<Option<String>, StorageError> {
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,CASE WHEN octet_length(owner_generation)=64 THEN owner_generation END FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3", &[&ns, &self.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        tx.check()?;
        rows.first()
            .map(|row| {
                if !row.get::<_, bool>(0) {
                    return Err(StorageError::ScopeChanged);
                }
                let owner = text(
                    row.get::<_, Option<Vec<u8>>>(1)
                        .ok_or(StorageError::ScopeChanged)?,
                )?;
                if !owner.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(StorageError::ScopeChanged);
                }
                Ok(owner)
            })
            .transpose()
    }

    pub(in crate::storage::postgres) fn catalog(
        &self,
        key: &str,
        history: bool,
    ) -> Result<Option<CatalogRow>, StorageError> {
        let owner = if history {
            let Some(owner) = self.owner(key)? else {
                return Ok(None);
            };
            owner
        } else {
            self.generation.clone()
        };
        let table = if history {
            "semon_pg_native_catalog_history_headers"
        } else {
            "semon_pg_native_catalog_rows"
        };
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query(&format!("SELECT session_key=$4,octet_length(metadata)::bigint,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3"), &[&ns, &owner.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        tx.check()?;
        let Some(row) = rows.first() else {
            return if history {
                Err(StorageError::ScopeChanged)
            } else {
                Ok(None)
            };
        };
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        let bytes = row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?;
        crate::selected_read::charge(64, bytes.len()).map_err(|_| StorageError::Busy)?;
        let value: CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if value.key != key || value.sources.len() > 64 {
            return Err(StorageError::ScopeChanged);
        }
        Ok(Some(value))
    }

    /// SQL validates counts and aggregate byte sizes before any recipe text is
    /// returned to the driver. A dangling owner/header is corruption, not empty
    /// retained history. Old immutable generations remain readable across L2.
    pub(in crate::storage::postgres) fn recipe(
        &self,
        key: &str,
        history: bool,
        start: i64,
        limit: i64,
    ) -> Result<Option<(catalog::OriginalHeader, Vec<String>)>, StorageError> {
        if start < 0 || !(1..=64).contains(&limit) {
            return Err(StorageError::InvalidArguments);
        }
        let owner = if history {
            let Some(owner) = self.owner(key)? else {
                return Ok(None);
            };
            owner
        } else {
            self.generation.clone()
        };
        let (headers, slots) = if history {
            (
                "semon_pg_native_catalog_history_headers",
                "semon_pg_native_catalog_history_slots",
            )
        } else {
            (
                "semon_pg_native_catalog_recipe_headers",
                "semon_pg_native_catalog_recipe_rows",
            )
        };
        let tx = self.tx.lock().map_err(|_| StorageError::Unavailable)?;
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let hash = digest(key.as_bytes());
        let args: &[&(dyn ToSql + Sync)] = &[&ns, &owner.as_bytes(), &hash, &key.as_bytes()];
        let rows = tx.driver.query(&format!("SELECT session_key=$4,CASE WHEN octet_length(recipe_generation)=64 THEN recipe_generation END,version,CASE WHEN octet_length(source_generation)=64 THEN source_generation END,total FROM {headers} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3"), args).map_err(sqlstate::error)?;
        let Some(row) = rows.first() else {
            return if history {
                Err(StorageError::ScopeChanged)
            } else {
                Ok(None)
            };
        };
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        let total: i64 = row.get(4);
        let version =
            u32::try_from(row.get::<_, i64>(2)).map_err(|_| StorageError::ScopeChanged)?;
        if version != crate::slot_projection::VERSION || total < 0 || start > total {
            return Err(StorageError::ScopeChanged);
        }
        let header = (
            text(
                row.get::<_, Option<Vec<u8>>>(1)
                    .ok_or(StorageError::ScopeChanged)?,
            )?,
            version,
            row.get::<_, Option<Vec<u8>>>(3).map(text).transpose()?,
            total,
        );
        let end = start
            .checked_add(limit)
            .ok_or(StorageError::InvalidArguments)?
            .min(total);
        let measure = tx.driver.query_one(&format!("SELECT count(*)::bigint,coalesce(sum(octet_length(metadata)),0)::bigint,coalesce(bool_and(session_key=$4),true) FROM (SELECT session_key,metadata FROM {slots} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND slot>=$5 AND slot<$6 ORDER BY slot LIMIT 65) p"), &[&ns, &owner.as_bytes(), &hash, &key.as_bytes(), &start, &end]).map_err(sqlstate::error)?;
        if measure.get::<_, i64>(0) != end - start
            || measure.get::<_, i64>(1) > MAX as i64
            || !measure.get::<_, bool>(2)
        {
            return Err(StorageError::ScopeChanged);
        }
        crate::selected_read::charge(64, measure.get::<_, i64>(1) as usize)
            .map_err(|_| StorageError::Busy)?;
        tx.check()?;
        let rows = tx.driver.query(&format!("SELECT metadata FROM {slots} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 AND slot>=$5 AND slot<$6 ORDER BY slot LIMIT 65"), &[&ns, &owner.as_bytes(), &hash, &key.as_bytes(), &start, &end]).map_err(sqlstate::error)?;
        tx.check()?;
        let texts = rows
            .iter()
            .map(|r| text(r.get(0)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some((header, texts)))
    }
}
