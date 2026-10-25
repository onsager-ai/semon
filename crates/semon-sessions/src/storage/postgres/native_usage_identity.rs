//! Atomic maintenance of the existing copied-usage relation. This is a native
//! row effect, not an ingest completion; no ledger or receipt is manufactured.
use super::{sqlstate, txn::PgTxn};
use crate::storage::StorageError;

// These are the same peer relation and delete rules as the SQLite Tree family.
// Native message/record identity continues to come from the shared parser.
const LOCK: &str = r#"
DECLARE ns bytea := decode(current_setting('semon.namespace', true), 'hex');
BEGIN
    INSERT INTO @SCHEMA@.semon_pg_source_state(namespace) VALUES(ns)
        ON CONFLICT(namespace_digest) DO NOTHING;
    PERFORM 1 FROM @SCHEMA@.semon_pg_source_state
        WHERE namespace_digest=sha256(ns) AND namespace=ns FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Native namespace unavailable' USING ERRCODE='55000'; END IF;
    RETURN NULL;
END;
"#;

const COPIES: &str = r#"
BEGIN
    IF TG_OP <> 'INSERT' THEN
        DELETE FROM @SCHEMA@.semon_pg_native_tree_copied_identities
            WHERE namespace=OLD.namespace AND file_id=OLD.file_id
                AND message_id=OLD.message_id AND record_uuid=OLD.record_uuid;
        DELETE FROM @SCHEMA@.semon_pg_native_tree_copied_identities c
            WHERE c.namespace=OLD.namespace AND c.message_id=OLD.message_id AND c.record_uuid=OLD.record_uuid
                AND NOT EXISTS(SELECT 1 FROM @SCHEMA@.semon_pg_native_usage_identity p
                    WHERE p.namespace=c.namespace AND p.message_id=c.message_id
                        AND p.record_uuid=c.record_uuid AND p.file_id<>c.file_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        INSERT INTO @SCHEMA@.semon_pg_native_tree_copied_identities(namespace,file_id,message_id,record_uuid)
            SELECT i.namespace,i.file_id,i.message_id,i.record_uuid
            FROM @SCHEMA@.semon_pg_native_usage_identity i
            WHERE i.namespace=NEW.namespace AND i.message_id=NEW.message_id AND i.record_uuid=NEW.record_uuid
                AND EXISTS(SELECT 1 FROM @SCHEMA@.semon_pg_native_usage_identity p
                    WHERE p.namespace=i.namespace AND p.message_id=i.message_id
                        AND p.record_uuid=i.record_uuid AND p.file_id<>i.file_id)
            ON CONFLICT DO NOTHING;
    END IF;
    RETURN NULL;
END;
"#;

pub(super) fn contract() -> [&'static str; 2] {
    [LOCK, COPIES]
}

fn schema(tx: &mut PgTxn<'_>) -> Result<String, StorageError> {
    let name: String = tx
        .driver
        .query_one("SELECT current_schema()", &[])
        .map_err(sqlstate::error)?
        .try_get(0)
        .map_err(|_| StorageError::ScopeChanged)?;
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

pub(super) fn install(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let schema = schema(tx)?;
    for (name, body) in [
        ("semon_pg_native_identity_lock", LOCK),
        ("semon_pg_native_identity_copies", COPIES),
    ] {
        let body = body.replace("@SCHEMA@", &schema);
        let body = body.replace('\'', "''");
        tx.driver.batch_execute(&format!("CREATE FUNCTION {schema}.{name}() RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS '{body}';")).map_err(sqlstate::error)?;
    }
    tx.driver
        .batch_execute(&format!(
            "
        CREATE TRIGGER semon_pg_native_identity_lock BEFORE INSERT OR UPDATE OR DELETE
            ON {schema}.semon_pg_native_usage_identity FOR EACH STATEMENT
            EXECUTE FUNCTION {schema}.semon_pg_native_identity_lock();
        CREATE TRIGGER semon_pg_native_identity_copies AFTER INSERT OR UPDATE OR DELETE
            ON {schema}.semon_pg_native_usage_identity FOR EACH ROW
            EXECUTE FUNCTION {schema}.semon_pg_native_identity_copies();
    "
        ))
        .map_err(sqlstate::error)
}

pub(super) fn verify(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let schema = schema(tx)?;
    let functions = tx.driver.query("SELECT p.proname,p.prosrc,p.prosecdef,p.provolatile='v' AND p.prokind='f' AND p.prorettype='trigger'::regtype AND p.pronargs=0 AND l.lanname='plpgsql',p.proconfig FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang WHERE p.pronamespace=current_schema()::regnamespace AND p.proname IN ('semon_pg_native_identity_lock','semon_pg_native_identity_copies') ORDER BY p.proname", &[]).map_err(sqlstate::error)?;
    if functions.len() != 2 {
        return Err(StorageError::ScopeChanged);
    }
    for (name, body) in [
        ("semon_pg_native_identity_lock", LOCK),
        ("semon_pg_native_identity_copies", COPIES),
    ] {
        let row = functions
            .iter()
            .find(|r| r.get::<_, String>(0) == name)
            .ok_or(StorageError::ScopeChanged)?;
        if row.get::<_, String>(1) != body.replace("@SCHEMA@", &schema)
            || row.get::<_, bool>(2)
            || !row.get::<_, bool>(3)
            || row.get::<_, Option<Vec<String>>>(4) != Some(vec!["search_path=pg_catalog".into()])
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    let triggers = tx.driver.query("SELECT t.tgname,p.proname,t.tgtype::int,c.relname,t.tgenabled='O' AND NOT t.tgisinternal AND NOT t.tgdeferrable AND NOT t.tginitdeferred AND t.tgconstraint=0 AND t.tgqual IS NULL AND t.tgattr=''::int2vector AND t.tgnargs=0 AND p.pronamespace=current_schema()::regnamespace FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_proc p ON p.oid=t.tgfoid WHERE c.relnamespace=current_schema()::regnamespace AND c.relname='semon_pg_native_usage_identity' AND NOT t.tgisinternal ORDER BY t.tgname", &[]).map_err(sqlstate::error)?;
    if triggers.len() != 2 {
        return Err(StorageError::ScopeChanged);
    }
    for (name, kind) in [
        ("semon_pg_native_identity_lock", 30),
        ("semon_pg_native_identity_copies", 29),
    ] {
        let row = triggers
            .iter()
            .find(|r| r.get::<_, String>(0) == name)
            .ok_or(StorageError::ScopeChanged)?;
        if row.get::<_, String>(1) != name
            || row.get::<_, i32>(2) != kind
            || row.get::<_, String>(3) != "semon_pg_native_usage_identity"
            || !row.get::<_, bool>(4)
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
