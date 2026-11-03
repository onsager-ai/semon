//! Authenticated context records for transfer-owned selected receipts.
use super::{sqlstate, txn::PgTxn};
use crate::storage::{StorageError, proof_codec};

// The persisted record is immutable even when application code has UPDATE rights.
const IMMUTABLE: &str =
    "BEGIN RAISE EXCEPTION 'immutable presentation context' USING ERRCODE='55000'; END;";
const RECORD_BOUND: &str = "(octet_length(entry) <= 522105)";

pub(super) fn contract() -> [&'static str; 2] {
    [IMMUTABLE, RECORD_BOUND]
}

pub(super) fn install(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    tx.driver.batch_execute(&format!(
        "ALTER TABLE semon_pg_native_selected_contexts ADD CONSTRAINT selected_context_record_bound CHECK {RECORD_BOUND};
         CREATE FUNCTION semon_pg_native_context_immutable() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS '{}';
         CREATE TRIGGER selected_context_immutable BEFORE UPDATE ON semon_pg_native_selected_contexts FOR EACH ROW EXECUTE FUNCTION semon_pg_native_context_immutable();",
        IMMUTABLE.replace('\'', "''")
    )).map_err(sqlstate::error)
}

pub(super) fn verify(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let constraints = tx.driver.query("SELECT pg_get_expr(k.conbin,k.conrelid),k.convalidated AND NOT k.connoinherit FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid WHERE c.relnamespace=current_schema()::regnamespace AND c.relname='semon_pg_native_selected_contexts' AND k.conname='selected_context_record_bound' AND k.contype='c'", &[]).map_err(sqlstate::error)?;
    if constraints.len() != 1
        || constraints[0].get::<_, String>(0) != RECORD_BOUND
        || !constraints[0].get::<_, bool>(1)
    {
        return Err(StorageError::ScopeChanged);
    }
    let functions = tx.driver.query("SELECT p.prosrc,p.prosecdef,p.provolatile='v' AND p.prokind='f' AND p.prorettype='trigger'::regtype AND p.pronargs=0 AND l.lanname='plpgsql',p.proconfig FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang WHERE p.pronamespace=current_schema()::regnamespace AND p.proname='semon_pg_native_context_immutable'", &[]).map_err(sqlstate::error)?;
    if functions.len() != 1
        || functions[0].get::<_, String>(0) != IMMUTABLE
        || functions[0].get::<_, bool>(1)
        || !functions[0].get::<_, bool>(2)
        || functions[0].get::<_, Option<Vec<String>>>(3)
            != Some(vec!["search_path=pg_catalog".into()])
    {
        return Err(StorageError::ScopeChanged);
    }
    let triggers = tx.driver.query("SELECT t.tgname,p.proname,t.tgtype::int,t.tgenabled='O' AND NOT t.tgisinternal AND NOT t.tgdeferrable AND NOT t.tginitdeferred AND t.tgconstraint=0 AND t.tgqual IS NULL AND t.tgattr=''::int2vector AND t.tgnargs=0 AND p.pronamespace=current_schema()::regnamespace FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_proc p ON p.oid=t.tgfoid WHERE c.relnamespace=current_schema()::regnamespace AND c.relname='semon_pg_native_selected_contexts' AND NOT t.tgisinternal", &[]).map_err(sqlstate::error)?;
    if triggers.len() != 1
        || triggers[0].get::<_, String>(0) != "selected_context_immutable"
        || triggers[0].get::<_, String>(1) != "semon_pg_native_context_immutable"
        || triggers[0].get::<_, i32>(2) != 19
        || !triggers[0].get::<_, bool>(3)
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}

pub(super) fn load(
    tx: &mut PgTxn<'_>,
    publication: &str,
    manifest: &proof_codec::PresentationManifest,
    charge: impl FnOnce(usize) -> Result<(), StorageError>,
) -> Result<proof_codec::PresentationContexts, StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let row = tx.driver.query_one(
        "SELECT count(*),COALESCE(sum(octet_length(digest)+4+octet_length(entry)),0)::bigint,COALESCE(max(octet_length(entry)),0) FROM semon_pg_native_selected_contexts WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND publication=$2",
        &[&ns, &publication.as_bytes()],
    ).map_err(sqlstate::error)?;
    let count: i64 = row.get(0);
    let bytes =
        usize::try_from(row.get::<_, i64>(1)).map_err(|_| StorageError::HistoryRetentionBounds)?;
    if count != manifest.contexts.len() as i64 {
        return Err(StorageError::ScopeChanged);
    }
    let bytes = bytes
        .max(1)
        .checked_add(1)
        .ok_or(StorageError::HistoryRetentionBounds)?;
    if bytes > 8 * proof_codec::METADATA_MAX
        || row.get::<_, i32>(2) > proof_codec::PRESENTATION_CONTEXT_MAX as i32
    {
        return Err(StorageError::HistoryRetentionBounds);
    }
    charge(bytes)?;
    let rows = tx.driver.query(
        "SELECT digest,entry FROM semon_pg_native_selected_contexts WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND publication=$2 ORDER BY digest",
        &[&ns, &publication.as_bytes()],
    ).map_err(sqlstate::error)?;
    let mut contexts = proof_codec::PresentationContexts::new();
    for row in rows {
        let reference = String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?;
        let text = String::from_utf8(row.get(1)).map_err(|_| StorageError::ScopeChanged)?;
        contexts.insert(
            reference.clone(),
            proof_codec::decode_presentation_context(&reference, &text)?,
        );
    }
    proof_codec::verify_presentation_contexts(manifest, &contexts)?;
    Ok(contexts)
}
