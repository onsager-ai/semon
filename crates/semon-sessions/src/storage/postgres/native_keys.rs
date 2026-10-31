//! Closed physical addressing for native file paths and metadata. Digests are
//! index addresses only; the complete bytea value remains the equality witness.
use super::{sqlstate, txn::PgTxn};
use crate::{selected_read, storage::StorageError};

pub(super) const MATCH_BODY: &str = "BEGIN IF digest <> sha256(expected) THEN RETURN false; END IF; IF stored <> expected THEN RAISE EXCEPTION 'Native key digest collision' USING ERRCODE='55000'; END IF; RETURN stored = expected; END;";

// Fixed-width namespace and domain prefixes make the framing unambiguous.
// This function addresses heap witnesses; it never grants identity authority.
pub(super) const DIGEST_BODY: &str =
    "BEGIN RETURN sha256(sha256(ns) || sha256(convert_to(domain, 'UTF8')) || value); END;";

pub(super) fn generated(table: &str) -> &'static [(&'static str, &'static str, bool)] {
    match table {
        "catalog_rows" => &[("parent_digest", "sha256(parent_key)", true)],
        "files" => &[
            ("path_digest", "sha256(path)", false),
            ("cwd_digest", "sha256(cwd)", true),
        ],
        "meta" => &[("key_digest", "sha256(key)", false)],
        // Encoding conversions are STABLE in PostgreSQL and cannot appear in
        // a generated column. The typed writer supplies the folded byte witness.
        "catalog_sources" => &[(
            "search_native_digest",
            "semon_pg_native_digest(namespace, folded_native_id, 'catalog.search.native'::text)",
            false,
        )],
        "catalog_search_headers" => &[
            (
                "source_path_digest",
                "semon_pg_native_digest(namespace, source_path, 'catalog.search.path'::text)",
                false,
            ),
            (
                "folded_id_digest",
                "semon_pg_native_digest(namespace, folded_id, 'catalog.search.header'::text)",
                false,
            ),
        ],
        "source_invalidations" => &[
            ("search_path_digest", "sha256(source_path)", false),
            (
                "search_filename_digest",
                "sha256(decode(translate(regexp_replace(encode(source_path, 'escape'::text), '^.*/'::text, ''::text), 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text), 'escape'::text))",
                false,
            ),
            (
                "search_copilot_digest",
                "sha256(decode(translate(regexp_replace(regexp_replace(encode(source_path, 'escape'::text), '/events[.]jsonl$'::text, ''::text), '^.*/'::text, ''::text), 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'::text, 'abcdefghijklmnopqrstuvwxyz'::text), 'escape'::text))",
                false,
            ),
        ],
        // Namespace is the leading fixed-width PK component. Keeping it out
        // of the second component gives both engines the same witness order
        // when a current logical source is moved between physical stores.
        "tree_models" => &[(
            "model_digest",
            "sha256(('\\x747265652e6d6f64656c'::bytea || model))",
            false,
        )],
        "tree_tools" => &[(
            "tool_id_digest",
            "sha256(('\\x747265652e746f6f6c'::bytea || tool_id))",
            false,
        )],
        "tree_headers" | "tree_repairs" => &[(
            "source_path_digest",
            "semon_pg_native_digest(namespace, source_path, 'tree.path'::text)",
            false,
        )],
        "tree_usage_work" | "tree_usage_models" => &[(
            "message_id_digest",
            "semon_pg_native_digest(namespace, message_id, 'tree.message'::text)",
            false,
        )],
        "tree_codex_work" => &[(
            "identity_digest",
            "semon_pg_native_digest(namespace, identity, 'tree.identity'::text)",
            false,
        )],
        "tree_copied_identities" => &[
            (
                "message_id_digest",
                "semon_pg_native_digest(namespace, message_id, 'tree.message'::text)",
                false,
            ),
            (
                "record_uuid_digest",
                "semon_pg_native_digest(namespace, record_uuid, 'tree.record'::text)",
                false,
            ),
        ],
        "usage_current" | "usage_sources" => &[(
            "source_path_digest",
            "semon_pg_native_digest(namespace, source_path, 'usage.path'::text)",
            false,
        )],
        "usage_facts" => &[(
            "identity_digest",
            "semon_pg_native_digest(namespace, identity, 'usage.identity'::text)",
            false,
        )],
        "usage_identities" => &[
            (
                "scope_digest",
                "semon_pg_native_digest(namespace, scope, 'usage.scope'::text)",
                false,
            ),
            (
                "identity_digest",
                "semon_pg_native_digest(namespace, identity, 'usage.identity'::text)",
                false,
            ),
        ],
        "usage_dimensions" => &[
            (
                "kind_digest",
                "semon_pg_native_digest(namespace, kind, 'usage.kind'::text)",
                false,
            ),
            (
                "dimension_digest",
                "semon_pg_native_digest(namespace, dimension, 'usage.dimension'::text)",
                false,
            ),
        ],
        "native_dependencies" => &[
            (
                "source_path_digest",
                "semon_pg_native_digest(namespace, source_path, 'dependency.path'::text)",
                false,
            ),
            (
                "harness_digest",
                "semon_pg_native_digest(namespace, harness, 'dependency.harness'::text)",
                false,
            ),
            (
                "native_id_digest",
                "semon_pg_native_digest(namespace, native_id, 'dependency.native'::text)",
                false,
            ),
        ],
        _ => &[],
    }
}

const CODEX_HEADER_PREFIX: &[u8] = b"native_codex_header:";

pub(super) fn ascii_fold(value: &str) -> Vec<u8> {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_uppercase() {
                byte.to_ascii_lowercase()
            } else {
                byte
            }
        })
        .collect()
}

fn project_codex_header(
    tx: &mut PgTxn<'_>,
    key: &[u8],
    value: Option<&[u8]>,
) -> Result<(), StorageError> {
    let Some(path) = key.strip_prefix(CODEX_HEADER_PREFIX) else {
        return Ok(());
    };
    if path.is_empty() || path.len() > 4096 {
        return Err(StorageError::InvalidArguments);
    }
    let folded_id = value
        .map(|value| {
            let record: crate::events::native_aux::CodexHeaderRecord =
                serde_json::from_slice(value).map_err(|_| StorageError::ScopeChanged)?;
            Ok::<_, StorageError>(record.header.id.as_deref().map(ascii_fold))
        })
        .transpose()?
        .flatten();
    let namespace = tx.namespace.as_str().as_bytes();
    if let Some(folded_id) = folded_id {
        let changed = tx.driver.execute(
            "INSERT INTO semon_pg_native_catalog_search_headers(namespace,source_path,folded_id) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path_digest) DO UPDATE SET folded_id=excluded.folded_id WHERE semon_pg_native_catalog_search_headers.namespace=excluded.namespace AND semon_pg_native_catalog_search_headers.source_path=excluded.source_path",
            &[&namespace, &path, &folded_id],
        ).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
    } else {
        tx.driver.execute(
            "DELETE FROM semon_pg_native_catalog_search_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path_digest=semon_pg_native_digest($1,$2,'catalog.search.path'::text) AND source_path=$2",
            &[&namespace, &path],
        ).map_err(sqlstate::error)?;
    }
    Ok(())
}

pub(super) fn put_meta(tx: &mut PgTxn<'_>, key: &[u8], value: &[u8]) -> Result<(), StorageError> {
    tx.check()?;
    let changed = tx.driver.execute("INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,key_digest) DO UPDATE SET value=excluded.value WHERE semon_pg_native_meta.namespace=excluded.namespace AND semon_pg_native_meta.key=excluded.key", &[&tx.namespace.as_str().as_bytes(),&key,&value]).map_err(sqlstate::error)?;
    if changed != 1 {
        return Err(StorageError::ScopeChanged);
    }
    project_codex_header(tx, key, Some(value))?;
    tx.check()
}

pub(super) fn delete_meta(tx: &mut PgTxn<'_>, key: &[u8]) -> Result<(), StorageError> {
    tx.check()?;
    tx.driver.execute("DELETE FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND semon_pg_native_key_matches(key,key_digest,$2)", &[&tx.namespace.as_str().as_bytes(),&key]).map_err(sqlstate::error)?;
    project_codex_header(tx, key, None)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::postgres::{
        PgCancellation,
        tests::{fixture, ns},
        txn::TxnKind,
    };

    #[test]
    fn pg_native_keys125_warm_function_and_generated_expression_refuse_without_effects() {
        for alteration in [
            "CREATE OR REPLACE FUNCTION semon_pg_native_digest(ns bytea,value bytea,domain text) RETURNS bytea LANGUAGE plpgsql IMMUTABLE STRICT SECURITY INVOKER SET search_path=pg_catalog AS 'BEGIN RETURN sha256(value); END;'",
            "CREATE OR REPLACE FUNCTION semon_pg_native_key_matches(stored bytea,digest bytea,expected bytea) RETURNS boolean LANGUAGE plpgsql IMMUTABLE STRICT SECURITY INVOKER SET search_path=pg_catalog AS 'BEGIN RETURN true; END;'",
            "ALTER TABLE semon_pg_native_meta DROP COLUMN key_digest CASCADE; ALTER TABLE semon_pg_native_meta ADD COLUMN key_digest bytea GENERATED ALWAYS AS (sha256(value)) STORED NOT NULL",
        ] {
            let (pool, factory) = fixture(|_| {});
            let namespace = ns("keys125-manifest");
            let control = PgCancellation::default();
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                put_meta(tx, b"preserved-key", b"preserved-value")
            })
            .unwrap();
            // The runtime role cannot author DDL. Exercise the host's actual
            // controlled-change protocol so warm admission observes its epoch.
            pool.controlled_schema_change(&control, |tx| {
                tx.driver.batch_execute(alteration).map_err(sqlstate::error)
            })
            .unwrap();
            let mut entered = false;
            assert!(
                pool.transaction(&namespace, TxnKind::Write, &control, |_| {
                    entered = true;
                    Ok(())
                })
                .is_err()
            );
            assert!(
                !entered,
                "warm admission rejected the changed physical contract"
            );
            let mut client = factory.client();
            client
                .query_one(
                    "SELECT set_config('semon.namespace',$1,false)",
                    &[&super::super::namespace_context(&namespace)],
                )
                .unwrap();
            let row = client
                .query_one("SELECT key,value FROM semon_pg_native_meta", &[])
                .unwrap();
            assert_eq!(row.get::<_, Vec<u8>>(0), b"preserved-key");
            assert_eq!(row.get::<_, Vec<u8>>(1), b"preserved-value");
        }
    }
}
