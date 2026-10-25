//! Private current native physical contract, consumed by schema installation
//! and every transaction admission. This does not expose a native IndexStore.
use super::*;
use sha2::{Digest, Sha256};

struct Table {
    name: &'static str,
    // A trailing '?' means nullable. bytea holds lossless UTF-8 (including NUL)
    // or canonical serde bytes; numeric preserves unsigned native counters.
    columns: &'static str,
    key: &'static str,
}

const TABLES: &[Table] = &[
    Table { name: "files", columns: "file_id bigint,path bytea,dev numeric,ino numeric,size numeric,mtime_ns bytea,ctime_ns bytea?,resume_at numeric,head_sha256 bytea,tail_sha256 bytea,entrypoint bytea?,title bytea?,agent_name bytea?,last_model bytea?,failed boolean,first_ms bigint?,last_ms bigint?,cwd bytea?,branch bytea?,pending bytea,tool_ids bytea,yields bytea,busy bytea,links bytea,codex_tokens bytea,codex_tokens_by_model bytea,rate_limits bytea?,codex_native_usage bytea?,codex_pricing_fingerprint bytea?,copilot bytea?", key: "file_id" },
    Table { name: "events", columns: "file_id bigint,extra boolean,seq bigint,k bytea,o numeric,b numeric,t bigint?,id bytea?,n bytea?,code_mode boolean,code_mode_open boolean,parent bigint?,script boolean,y bytea?,poll bigint?,reply boolean,reply_o numeric?,reply_b numeric?,reply_t bigint?,reply_e bigint?,reply_f bigint?,reply_m bytea?,item bigint?,peer boolean,peer_sock bytea?,peer_pid bigint?,peer_start bytea?,u bytea?", key: "file_id,extra,seq" },
    Table { name: "signals", columns: "file_id bigint,seq bigint,k bytea,o numeric,t bigint?,at numeric,n bytea?,v bigint?", key: "file_id,seq" },
    Table { name: "usage_identity", columns: "file_id bigint,message_id bytea,record_uuid bytea", key: "file_id,message_id,record_uuid" },
    Table { name: "usage", columns: "file_id bigint,message_id bytea,model bytea?,input numeric,cached_input numeric,output numeric,reasoning_output numeric,total numeric,model_input numeric,model_output numeric,model_cache_write numeric,model_cache_read numeric,billing_input numeric,billing_output numeric,billing_cache_read numeric,billing_cache_write_5m numeric,billing_cache_write_1h numeric,billing_web_search_requests numeric,billing_speed bytea?,billing_service_tier bytea?,billing_prompt_size numeric,billing_timestamp bigint?,billing_split_unknown boolean,record_ids bytea", key: "file_id,message_id" },
    Table { name: "codex_usage", columns: "file_id bigint,seq bigint,model bytea,input numeric,output numeric,cache_write numeric,cache_read numeric,t bigint?", key: "file_id,seq" },
    Table { name: "meta", columns: "key bytea,value bytea", key: "key" },
    Table { name: "native_dependencies", columns: "source_path bytea,harness bytea,native_id bytea", key: "source_path,harness,native_id" },
    Table { name: "usage_sources", columns: "source_id bigint,source_path bytea,receipt bytea,checkpoint numeric,parser_version bigint,usage_ready boolean", key: "source_id" },
    Table { name: "usage_current", columns: "source_path bytea,source_id bigint", key: "source_path" },
    Table { name: "usage_facts", columns: "source_id bigint,native_offset numeric,identity bytea,payload bytea,record bytea,outcome bytea,model_outcome bytea", key: "source_id,native_offset" },
    Table { name: "usage_identities", columns: "source_id bigint,scope bytea,identity bytea,native_offset numeric", key: "source_id,scope,identity" },
    Table { name: "usage_dimensions", columns: "source_id bigint,kind bytea,dimension bytea,value bytea", key: "source_id,kind,dimension" },
    Table { name: "tree_generations", columns: "projection_id bigint,source_id bigint,version bigint,receipt bytea,complete boolean,work_pending boolean", key: "projection_id" },
    Table { name: "tree_usage_work", columns: "projection_id bigint,message_id bytea,tokens bytea", key: "projection_id,message_id" },
    Table { name: "tree_codex_work", columns: "projection_id bigint,identity bytea", key: "projection_id,identity" },
    Table { name: "tree_headers", columns: "source_path bytea,file_id bigint,projection_id bigint,ledger bytea", key: "source_path" },
    Table { name: "tree_models", columns: "projection_id bigint,model bytea", key: "projection_id,model" },
    Table { name: "tree_labels", columns: "projection_id bigint,label bytea,value bytea", key: "projection_id,label" },
    Table { name: "tree_tools", columns: "projection_id bigint,tool_id bytea,name bytea?,closed boolean", key: "projection_id,tool_id" },
    Table { name: "tree_usage_models", columns: "projection_id bigint,message_id bytea,model bytea", key: "projection_id,message_id" },
    Table { name: "tree_copied_identities", columns: "file_id bigint,message_id bytea,record_uuid bytea", key: "file_id,message_id,record_uuid" },
    Table { name: "tree_repairs", columns: "source_path bytea,receipt bytea,source_id bigint?,cursor numeric,projection_id bigint?,last_model bytea?,tree_root bytea?,state bytea,last_attempt bigint", key: "source_path" },
    Table { name: "recovery", columns: "cursor bytea,done boolean", key: "cursor" },
];

// Indexes are part of the same authoritative manifest, including key order.
const INDEXES: &[(&str, &str, &str, bool)] = &[
    ("files_path", "files", "path", true),
    ("files_cwd", "files", "cwd,path", false),
    ("usage_peers", "usage_identity", "message_id,record_uuid,file_id", false),
    ("usage_source_path", "usage_sources", "source_path,source_id", false),
    ("usage_current_source", "usage_current", "source_id", true),
    ("usage_observations", "usage_facts", "source_id,identity,native_offset", false),
    ("tree_cleanup", "tree_generations", "complete,work_pending,projection_id", false),
    ("tree_open_tools", "tree_tools", "projection_id,closed,tool_id,name", false),
    ("tree_repair_queue", "tree_repairs", "state,last_attempt,source_path", false),
    ("dependency_native", "native_dependencies", "harness,native_id,source_path", false),
];

const POLICY: &str = "semon_pg_namespace_matches(namespace, namespace_digest, decode(current_setting('semon.namespace'::text, true), 'hex'::text))";

fn contract_hash() -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(b"semon-private-native-v3");
    for table in TABLES {
        for value in [table.name, table.columns, table.key] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
    }
    for index in INDEXES {
        hash.update(format!("{index:?}"));
    }
    hash.update(POLICY);
    hash.finalize().to_vec()
}

pub(super) fn install(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    tx.driver.batch_execute("ALTER TABLE semon_pg_schema ADD COLUMN native_contract bytea NOT NULL DEFAULT ''::bytea").map_err(sqlstate::error)?;
    tx.driver.execute("UPDATE semon_pg_schema SET native_contract=$1", &[&contract_hash()]).map_err(sqlstate::error)?;
    let mut ddl = String::new();
    for table in TABLES {
        let name = format!("semon_pg_native_{}", table.name);
        ddl.push_str(&format!("CREATE TABLE {name}(namespace bytea NOT NULL CHECK(octet_length(namespace) BETWEEN 1 AND 4096),namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED,"));
        for column in table.columns.split(',') {
            ddl.push_str(column.trim_end_matches('?'));
            if !column.ends_with('?') { ddl.push_str(" NOT NULL"); }
            ddl.push(',');
        }
        ddl.push_str(&format!("PRIMARY KEY(namespace_digest,{})); ALTER TABLE {name} ENABLE ROW LEVEL SECURITY; ALTER TABLE {name} FORCE ROW LEVEL SECURITY; CREATE POLICY native_namespace ON {name} USING ({POLICY}) WITH CHECK ({POLICY});", table.key));
    }
    for (name, table, columns, unique) in INDEXES {
        let unique = if *unique { "UNIQUE" } else { "" };
        ddl.push_str(&format!("CREATE {unique} INDEX semon_pg_native_{name} ON semon_pg_native_{table}(namespace_digest,{columns});"));
    }
    tx.driver.batch_execute(&ddl).map_err(sqlstate::error)
}

pub(super) fn verify(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let row = tx.driver.query_one("SELECT native_contract FROM semon_pg_schema", &[]).map_err(sqlstate::error)?;
    if row.try_get::<_, Vec<u8>>(0).map_err(|_| StorageError::ScopeChanged)? != contract_hash() {
        return Err(StorageError::ScopeChanged);
    }
    // One catalog round trip for all native tables. No unsafe connection cache:
    // a warm reused connection is requalified before the effect callback.
    let rows = tx.driver.query("SELECT c.relname, c.relrowsecurity AND c.relforcerowsecurity AND c.relkind='r',
        ARRAY(SELECT a.attname::text || ' ' || format_type(a.atttypid,a.atttypmod) || CASE WHEN a.attnotnull THEN '' ELSE '?' END FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum),
        ARRAY(SELECT a.attname::text FROM pg_constraint k CROSS JOIN LATERAL unnest(k.conkey) WITH ORDINALITY x(num,ord) JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=x.num WHERE k.conrelid=c.oid AND k.contype='p' ORDER BY x.ord),
        ARRAY(SELECT p.polname::text || ':' || p.polcmd::text || ':' || p.polpermissive::text || ':' || p.polroles::text || ':' || pg_get_expr(p.polqual,p.polrelid) || ':' || pg_get_expr(p.polwithcheck,p.polrelid) FROM pg_policy p WHERE p.polrelid=c.oid ORDER BY p.polname),
        (SELECT pg_get_expr(d.adbin,d.adrelid) FROM pg_attribute a JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=c.oid AND a.attname='namespace_digest' AND a.attgenerated='s'),
        ARRAY(SELECT i.relname::text || ':' || x.indisunique::text || ':' || x.indisvalid::text || ':' || x.indisready::text || ':' || array_to_string(ARRAY(SELECT a.attname::text FROM unnest(x.indkey) WITH ORDINALITY z(num,ord) LEFT JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=z.num ORDER BY z.ord),',') || ':' || coalesce(pg_get_expr(x.indpred,x.indrelid),'') FROM pg_index x JOIN pg_class i ON i.oid=x.indexrelid WHERE x.indrelid=c.oid ORDER BY i.relname)
        FROM pg_class c WHERE c.relnamespace=current_schema()::regnamespace AND c.relname LIKE 'semon_pg_native_%' ORDER BY c.relname", &[]).map_err(sqlstate::error)?;
    // The prefix also matches indexes; only table rows are relevant.
    let rows = rows.iter().filter(|r| r.get::<_, String>(0).strip_suffix("_pkey").is_none() && TABLES.iter().any(|t| r.get::<_, String>(0) == format!("semon_pg_native_{}",t.name))).collect::<Vec<_>>();
    if rows.len() != TABLES.len() { return Err(StorageError::ScopeChanged); }
    for table in TABLES {
        let name = format!("semon_pg_native_{}",table.name);
        let row = rows.iter().find(|r| r.get::<_, String>(0)==name).ok_or(StorageError::ScopeChanged)?;
        let mut columns = vec!["namespace bytea".to_owned(), "namespace_digest bytea".to_owned()];
        columns.extend(table.columns.split(',').map(str::to_owned));
        let key = format!("namespace_digest,{}",table.key).split(',').map(str::to_owned).collect::<Vec<_>>();
        let policy = format!("native_namespace:*:true:{{0}}:{POLICY}:{POLICY}");
        let mut indexes = vec![format!("{name}_pkey:true:true:true:namespace_digest,{}:",table.key)];
        for (index, target, keys, unique) in INDEXES {
            if *target==table.name { indexes.push(format!("semon_pg_native_{index}:{unique}:true:true:namespace_digest,{keys}:")); }
        }
        indexes.sort();
        if !row.get::<_, bool>(1) || row.get::<_, Vec<String>>(2)!=columns || row.get::<_, Vec<String>>(3)!=key || row.get::<_, Vec<String>>(4)!=vec![policy] || row.get::<_, Option<String>>(5).as_deref()!=Some("sha256(namespace)") || row.get::<_, Vec<String>>(6)!=indexes {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::postgres::tests::{count, effect, fixture, identity, limits, ns};

    #[test]
    fn pg_native_current_manifest_all_tables_forced_rls_and_warm_namespace_bytes() {
        let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
        drop(PgPool::open(factory.clone(), limits()).unwrap());
        let control = PgCancellation::default();
        for namespace in [ns("a\0中"), ns("a\0é")] {
            pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
                for table in TABLES {
                    let columns = table.columns.split(',').map(|column| column.split_once(' ').unwrap()).collect::<Vec<_>>();
                    let names = columns.iter().map(|(name,_)| *name).collect::<Vec<_>>().join(",");
                    let values = columns.iter().map(|(_,kind)| match kind.trim_end_matches('?') {
                        "bytea" => "decode('7800e4b8ad','hex')",
                        "boolean" => "false",
                        _ => "0",
                    }).collect::<Vec<_>>().join(",");
                    tx.driver.execute(&format!("INSERT INTO semon_pg_native_{}(namespace,{names}) VALUES($1,{values})",table.name), &[&namespace.as_str().as_bytes()]).map_err(sqlstate::error)?;
                    let rows = tx.driver.query(&format!("SELECT namespace FROM semon_pg_native_{}",table.name), &[]).map_err(sqlstate::error)?;
                    assert_eq!(rows.len(),1,"{}",table.name);
                    assert_eq!(rows[0].get::<_,Vec<u8>>(0),namespace.as_str().as_bytes());
                }
                Ok(())
            }).unwrap();
        }
        let mut client = factory.client();
        for table in TABLES {
            assert_eq!(client.query_one(&format!("SELECT count(*) FROM semon_pg_native_{}",table.name),&[]).unwrap().get::<_,i64>(0),0);
            let refused = pool.transaction(&ns("wrong"),TxnKind::Write,&control,|tx| {
                tx.driver.execute(&format!("UPDATE semon_pg_native_{} SET namespace=$1",table.name), &[&b"foreign".as_slice()]).map_err(sqlstate::error)?;
                // No other namespace's rows were visible to mutate.
                let count = tx.driver.query_one(&format!("SELECT count(*) FROM semon_pg_native_{}",table.name), &[]).map_err(sqlstate::error)?;
                assert_eq!(count.get::<_,i64>(0),0);
                Ok(())
            });
            assert_eq!(refused,Ok(()));
            assert!(pool.transaction(&ns("a\0中"),TxnKind::Write,&control,|tx| {
                tx.driver.execute(&format!("UPDATE semon_pg_native_{} SET namespace=$1",table.name), &[&b"foreign".as_slice()]).map_err(sqlstate::error)?;
                Ok(())
            }).is_err());
        }
    }

    #[test]
    fn pg_native_shape_drift_refuses_warm_admission_before_effects() {
        for ddl in [
            "ALTER TABLE semon_pg_native_files DROP COLUMN title",
            "DROP INDEX semon_pg_native_files_cwd",
            "DROP POLICY native_namespace ON semon_pg_native_events",
            "ALTER TABLE semon_pg_native_tree_labels NO FORCE ROW LEVEL SECURITY",
            "ALTER TABLE semon_pg_native_signals ALTER COLUMN v TYPE numeric",
            "ALTER TABLE semon_pg_native_recovery DROP CONSTRAINT semon_pg_native_recovery_pkey",
            "ALTER TABLE semon_pg_native_meta ALTER COLUMN namespace_digest DROP EXPRESSION",
            "UPDATE semon_pg_schema SET native_contract=decode('00','hex')",
        ] {
            let (pool,factory)=fixture(|bounds| bounds.max_connections=1);
            let control=PgCancellation::default();
            pool.transaction(&ns("warm"),TxnKind::ReadSnapshot,&control,|_|Ok(())).unwrap();
            factory.client().batch_execute(ddl).unwrap();
            assert_eq!(pool.apply_operation(&ns("warm"),&identity(),&control,|_|panic!("drift admitted effects: {ddl}")),Err(StorageError::ScopeChanged),"{ddl}");
            assert_eq!(count(&factory),0);
            assert!(matches!(PgPool::open(factory,limits()),Err(StorageError::ScopeChanged)));
        }
    }

    #[test]
    fn pg_native_revoked_privilege_refuses_and_rolls_back_other_effects() {
        let (pool,factory)=fixture(|_|{});
        factory.client().batch_execute("REVOKE ALL ON semon_pg_native_meta FROM PUBLIC; REVOKE ALL ON semon_pg_native_meta FROM CURRENT_USER").unwrap();
        assert!(pool.apply_operation(&ns("revoke"),&identity(),&PgCancellation::default(),|tx| {
            effect(tx)?;
            tx.driver.query("SELECT * FROM semon_pg_native_meta",&[]).map_err(sqlstate::error)?;
            Ok(serde_json::json!(null))
        }).is_err());
        assert_eq!(count(&factory),0);
    }
}
