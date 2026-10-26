//! Borrowed immutable metadata reads. These are physical observations, not a
//! selected ReaderLease, Raw permission, or an admitted CatalogSnapshot.
use super::*;
use crate::storage::{LogicalSourceNamespace, postgres::{PgCancellation, PgPool, txn::TxnKind}};

pub(super) struct Snapshot<'a, 'tx> {
    tx: RefCell<&'a mut PgTxn<'tx>>,
    generation: String,
}

fn text(bytes: Vec<u8>) -> Result<String, StorageError> {
    String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged)
}

/// All observations in the callback use ONE repeatable-read transaction. The
/// callback cannot return a borrowed adapter or a driver value. An immutable
/// generation is an address, never a replacement for publication admission.
pub(super) fn with_generation<T>(
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
        read(&Snapshot { tx: RefCell::new(tx), generation: generation.to_owned() })
    })
}

impl Snapshot<'_, '_> {
    fn owner(&self, key: &str) -> Result<Option<String>, StorageError> {
        let tx = self.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT session_key=$4,CASE WHEN octet_length(owner_generation)=64 THEN owner_generation END FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3", &[&ns, &self.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        tx.check()?;
        rows.first().map(|row| {
            if !row.get::<_, bool>(0) { return Err(StorageError::ScopeChanged); }
            let owner = text(row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::ScopeChanged)?)?;
            if !owner.bytes().all(|b| b.is_ascii_hexdigit()) { return Err(StorageError::ScopeChanged); }
            Ok(owner)
        }).transpose()
    }

    pub(super) fn catalog(&self, key: &str, history: bool) -> Result<Option<CatalogRow>, StorageError> {
        let owner = if history {
            let Some(owner) = self.owner(key)? else { return Ok(None); };
            owner
        } else { self.generation.clone() };
        let table = if history { "semon_pg_native_catalog_history_headers" } else { "semon_pg_native_catalog_rows" };
        let tx = self.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query(&format!("SELECT session_key=$4,octet_length(metadata)::bigint,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3"), &[&ns, &owner.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        tx.check()?;
        let Some(row) = rows.first() else {
            return if history { Err(StorageError::ScopeChanged) } else { Ok(None) };
        };
        if !row.get::<_, bool>(0) { return Err(StorageError::ScopeChanged); }
        let bytes = row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?;
        crate::selected_read::charge(64, bytes.len()).map_err(|_| StorageError::Busy)?;
        let value: CatalogRow = serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if value.key != key || value.sources.len() > 64 { return Err(StorageError::ScopeChanged); }
        Ok(Some(value))
    }

    /// SQL validates counts and aggregate byte sizes before any recipe text is
    /// returned to the driver. A dangling owner/header is corruption, not empty
    /// retained history. Old immutable generations remain readable across L2.
    pub(super) fn recipe(&self, key: &str, history: bool) -> Result<Option<(catalog::OriginalHeader, Vec<String>)>, StorageError> {
        let owner = if history {
            let Some(owner) = self.owner(key)? else { return Ok(None); };
            owner
        } else { self.generation.clone() };
        let (headers, slots) = if history {
            ("semon_pg_native_catalog_history_headers", "semon_pg_native_catalog_history_slots")
        } else { ("semon_pg_native_catalog_recipe_headers", "semon_pg_native_catalog_recipe_rows") };
        let tx = self.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        let hash = digest(key.as_bytes());
        let args: &[&(dyn ToSql + Sync)] = &[&ns, &owner.as_bytes(), &hash, &key.as_bytes()];
        let rows = tx.driver.query(&format!("SELECT session_key=$4,recipe_generation,version,source_generation,total FROM {headers} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3"), args).map_err(sqlstate::error)?;
        let Some(row) = rows.first() else {
            return if history { Err(StorageError::ScopeChanged) } else { Ok(None) };
        };
        if !row.get::<_, bool>(0) { return Err(StorageError::ScopeChanged); }
        let total: i64 = row.get(4);
        let version = u32::try_from(row.get::<_, i64>(2)).map_err(|_| StorageError::ScopeChanged)?;
        if version != crate::slot_projection::VERSION || !(0..=64).contains(&total) { return Err(StorageError::ScopeChanged); }
        let header = (text(row.get(1))?, version, row.get::<_, Option<Vec<u8>>>(3).map(text).transpose()?, total);
        let measure = tx.driver.query_one(&format!("SELECT count(*)::bigint,coalesce(sum(octet_length(metadata)),0)::bigint,coalesce(bool_and(session_key=$4 AND slot>=0 AND slot<$5),true) FROM (SELECT session_key,slot,metadata FROM {slots} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 ORDER BY slot LIMIT 65) p"), &[&ns, &owner.as_bytes(), &hash, &key.as_bytes(), &total]).map_err(sqlstate::error)?;
        if measure.get::<_, i64>(0) != total || measure.get::<_, i64>(1) > MAX as i64 || !measure.get::<_, bool>(2) { return Err(StorageError::ScopeChanged); }
        crate::selected_read::charge(64, measure.get::<_, i64>(1) as usize).map_err(|_| StorageError::Busy)?;
        tx.check()?;
        let rows = tx.driver.query(&format!("SELECT metadata FROM {slots} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 ORDER BY slot LIMIT 65"), args).map_err(sqlstate::error)?;
        tx.check()?;
        let texts = rows.iter().map(|r| text(r.get(0))).collect::<Result<Vec<_>, _>>()?;
        Ok(Some((header, texts)))
    }
}
