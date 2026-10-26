//! Closed physical addressing for native file paths and metadata. Digests are
//! index addresses only; the complete bytea value remains the equality witness.
use super::{sqlstate, txn::PgTxn};
use crate::{selected_read, storage::StorageError};

pub(super) const MATCH_BODY: &str = "BEGIN IF digest <> sha256(expected) THEN RETURN false; END IF; IF stored <> expected THEN RAISE EXCEPTION 'Native key digest collision' USING ERRCODE='55000'; END IF; RETURN stored = expected; END;";

pub(super) fn generated(table: &str) -> &'static [(&'static str, &'static str, bool)] {
    match table {
        "files" => &[
            ("path_digest", "sha256(path)", false),
            ("cwd_digest", "sha256(cwd)", true),
        ],
        "meta" => &[("key_digest", "sha256(key)", false)],
        _ => &[],
    }
}

pub(super) fn put_meta(
    tx: &mut PgTxn<'_>,
    key: &[u8],
    value: &[u8],
) -> Result<(), StorageError> {
    tx.check()?;
    let changed = tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,key_digest) DO UPDATE SET value=excluded.value WHERE semon_pg_native_meta.namespace=excluded.namespace AND semon_pg_native_meta.key=excluded.key", &[&tx.namespace.as_str().as_bytes(),&key,&value]).map_err(sqlstate::error)?;
    if changed != 1 {
        return Err(StorageError::ScopeChanged);
    }
    tx.check()
}

pub(super) fn delete_meta(tx: &mut PgTxn<'_>, key: &[u8]) -> Result<(), StorageError> {
    tx.check()?;
    tx.driver.execute("DELETE FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND semon_pg_native_key_matches(key,key_digest,$2)", &[&tx.namespace.as_str().as_bytes(),&key]).map_err(sqlstate::error)?;
    tx.check()
}

pub(super) fn read_meta(
    tx: &mut PgTxn<'_>,
    key: &[u8],
    limit: usize,
) -> Result<Option<Vec<u8>>, StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes();
    let rows = tx.driver.query("SELECT octet_length(value)::bigint FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND semon_pg_native_key_matches(key,key_digest,$2)", &[&ns,&key]).map_err(sqlstate::error)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let bytes = usize::try_from(row.get::<_, i64>(0)).map_err(|_| StorageError::ScopeChanged)?;
    if bytes > limit {
        return Err(StorageError::ScopeChanged);
    }
    selected_read::charge(64, bytes).map_err(|_| StorageError::Busy)?;
    tx.check()?;
    let row = tx.driver.query_one("SELECT value FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND semon_pg_native_key_matches(key,key_digest,$2)", &[&ns,&key]).map_err(sqlstate::error)?;
    tx.check()?;
    Ok(Some(row.get(0)))
}
