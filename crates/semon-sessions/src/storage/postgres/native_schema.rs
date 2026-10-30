//! Private current native physical contract, consumed by schema installation
//! and every transaction admission. This does not expose a native IndexStore.
use super::super::native_keys;
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
    Table {
        name: "semantic_publications",
        columns: "generation bytea,epoch bigint,admission bigint,base_generation bytea?,source_revision bigint,proof_digest bytea,claims bytea,metadata bytea",
        key: "epoch",
    },
    Table {
        name: "selected_roots",
        columns: "publication bytea,base bytea,manifest bytea,digest bytea",
        key: "digest",
    },
    Table {
        name: "selected_generations",
        columns: "id bytea,key_digest bytea,session_key bytea,publication bytea,proof bytea,admission bigint,stream bytea,metadata bytea,slots bytea,prefix bytea",
        key: "id,key_digest",
    },
    Table {
        name: "selected_presentations",
        columns: "id bytea,key_digest bytea,session_key bytea,metadata bytea",
        key: "id,key_digest",
    },
    Table {
        name: "selected_sources",
        columns: "id bytea,key_digest bytea,session_key bytea,path_digest bytea,path bytea,ledger bytea",
        key: "id,key_digest,path_digest",
    },
    Table {
        name: "selected_refs",
        columns: "key_digest bytea,session_key bytea,kind bytea,id bytea",
        key: "key_digest,kind,id",
    },
    Table {
        name: "change_feed",
        columns: "read_scope bytea,revision bigint,session_key bytea",
        key: "read_scope,revision",
    },
    Table {
        name: "change_feed_state",
        columns: "read_scope bytea,head bigint,floor bigint",
        key: "read_scope",
    },
    Table {
        name: "catalog_stage_items",
        columns: "stage_epoch bigint,seq bigint,payload bytea,payload_hash bytea,active boolean",
        key: "stage_epoch,seq",
    },
    Table {
        name: "catalog_leases",
        columns: "lease_id bytea,generation bytea,expires_at bigint",
        key: "lease_id",
    },
    Table {
        name: "catalog_operations",
        columns: "operation_id bytea,payload bytea,generation bytea,stage_epoch bigint",
        key: "operation_id",
    },
    Table {
        name: "catalog_proofs",
        columns: "generation bytea,path_digest bytea,source_path bytea,page_generation bytea,usage_generation bigint,usage_ready boolean",
        key: "generation,path_digest,page_generation",
    },
    Table {
        name: "catalog_changes",
        columns: "generation bytea,previous bytea?,stage_epoch bigint",
        key: "generation",
    },
    Table {
        name: "catalog_stage",
        columns: "singleton boolean,stage_epoch bigint,runtime bytea,generation bytea?",
        key: "singleton",
    },
    Table {
        name: "semantic_runtime",
        columns: "singleton boolean,admission bigint,stream_epoch bytea",
        key: "singleton",
    },
    Table {
        name: "descriptions",
        columns: "key_digest bytea,session_key bytea,version bigint,inputs bytea,metadata bytea",
        key: "key_digest",
    },
    Table {
        name: "catalog_history_owners",
        columns: "generation bytea,key_digest bytea,session_key bytea,owner_generation bytea,original boolean,last_ms bigint,harness bytea,repo bytea,model bytea",
        key: "generation,key_digest",
    },
    Table {
        name: "catalog_history_headers",
        columns: "generation bytea,key_digest bytea,session_key bytea,recipe_generation bytea,source_generation bytea?,version bigint,total bigint,incomplete boolean,metadata bytea,original_header bytea?,last_ms bigint,harness bytea,repo bytea,model bytea",
        key: "generation,key_digest",
    },
    Table {
        name: "catalog_history_slots",
        columns: "generation bytea,key_digest bytea,session_key bytea,slot bigint,metadata bytea",
        key: "generation,key_digest,slot",
    },
    Table {
        name: "catalog_history_sources",
        columns: "generation bytea,key_digest bytea,session_key bytea,path_digest bytea,source_path bytea,metadata bytea,native_digest bytea",
        key: "generation,key_digest,path_digest",
    },
    Table {
        name: "catalog_history_progress",
        columns: "generation bytea,key_digest bytea,session_key bytea,metadata bytea",
        key: "generation,key_digest",
    },
    Table {
        name: "catalog_search_documents",
        columns: "generation bytea,scope bytea,key_digest bytea,session_key bytea,fields bytea,complete boolean",
        key: "generation,scope,key_digest",
    },
    Table {
        name: "catalog_search_memberships",
        columns: "generation bytea,key_digest bytea,session_key bytea,gram bytea,last_ms bigint",
        key: "generation,key_digest,gram",
    },
    Table {
        name: "catalog_search_counts",
        columns: "generation bytea,scope bytea,gram bytea,members bigint",
        key: "generation,scope,gram",
    },
    Table {
        name: "catalog_trace_headers",
        columns: "generation bytea,key_digest bytea,session_key bytea,trace_generation bytea,source_generation bytea,metadata bytea",
        key: "generation,key_digest",
    },
    Table {
        name: "catalog_trace_turns",
        columns: "generation bytea,key_digest bytea,session_key bytea,turn_digest bytea,turn_id bytea,start_id bytea?,metadata bytea",
        key: "generation,key_digest,turn_digest",
    },
    Table {
        name: "catalog_trace_edges",
        columns: "generation bytea,key_digest bytea,session_key bytea,edge_digest bytea,edge_id bytea,metadata bytea",
        key: "generation,key_digest,edge_digest",
    },
    Table {
        name: "catalog_trace_bookmarks",
        columns: "generation bytea,key_digest bytea,session_key bytea,target_digest bytea,target bytea,edge_digest bytea,edge_id bytea",
        key: "generation,key_digest,target_digest,edge_digest",
    },
    Table {
        name: "catalog_rows",
        columns: "generation bytea,key_digest bytea,session_key bytea,metadata bytea,last_ms bigint,harness bytea,repo bytea,model bytea",
        key: "generation,key_digest",
    },
    Table {
        name: "catalog_sources",
        columns: "generation bytea,key_digest bytea,session_key bytea,path_digest bytea,source_path bytea,metadata bytea,native_digest bytea",
        key: "generation,key_digest,path_digest",
    },
    Table {
        name: "catalog_links",
        columns: "generation bytea,key_digest bytea,session_key bytea,peer_digest bytea,peer bytea",
        key: "generation,key_digest,peer_digest",
    },
    Table {
        name: "catalog_dependencies",
        columns: "generation bytea,path_digest bytea,source_path bytea,harness_digest bytea,harness bytea,native_digest bytea,native_id bytea",
        key: "generation,path_digest,harness_digest,native_digest",
    },
    Table {
        name: "catalog_recipe_headers",
        columns: "generation bytea,key_digest bytea,session_key bytea,recipe_generation bytea,source_generation bytea,version bigint,total bigint",
        key: "generation,key_digest",
    },
    Table {
        name: "catalog_recipe_rows",
        columns: "generation bytea,key_digest bytea,session_key bytea,slot bigint,metadata bytea",
        key: "generation,key_digest,slot",
    },
    Table {
        name: "page_blobs",
        columns: "digest bytea,payload bytea",
        key: "digest",
    },
    Table {
        name: "page_generations",
        columns: "generation bytea,source_id bigint,source_path bytea,ledger bytea,chunks_header bytea,launch_header bytea?",
        key: "generation",
    },
    Table {
        name: "page_refs",
        columns: "generation bytea,kind smallint,page_key bytea,digest bytea",
        key: "generation,kind,page_key",
    },
    Table {
        name: "page_current",
        columns: "path_digest bytea,source_path bytea,generation bytea",
        key: "path_digest",
    },
    Table {
        name: "reported_runs",
        columns: "session_digest bytea,session_id bytea,start bigint,record bytea",
        key: "session_digest,start",
    },
    Table {
        name: "reported_stamp",
        columns: "singleton boolean,stamp bytea",
        key: "singleton",
    },
    Table {
        name: "source_mutations",
        columns: "source_path bytea,evidence bytea",
        key: "source_path",
    },
    Table {
        name: "relocation_candidates",
        columns: "old_path bytea,new_path bytea",
        key: "old_path",
    },
    Table {
        name: "files",
        columns: "file_id bigint,path bytea,dev numeric,ino numeric,size numeric,mtime_ns bytea,ctime_ns bytea?,resume_at numeric,head_sha256 bytea,tail_sha256 bytea,entrypoint bytea?,title bytea?,agent_name bytea?,last_model bytea?,failed boolean,first_ms bigint?,last_ms bigint?,cwd bytea?,branch bytea?,pending bytea,tool_ids bytea,yields bytea,busy bytea,links bytea,codex_tokens bytea,codex_tokens_by_model bytea,rate_limits bytea?,codex_native_usage bytea?,codex_pricing_fingerprint bytea?,copilot bytea?",
        key: "file_id",
    },
    Table {
        name: "events",
        columns: "file_id bigint,extra boolean,seq bigint,k bytea,o numeric,b numeric,t bigint?,id bytea?,n bytea?,code_mode boolean,code_mode_open boolean,parent bigint?,script smallint,y bytea?,poll bigint?,reply boolean,reply_o numeric?,reply_b numeric?,reply_t bigint?,reply_e boolean?,reply_f smallint?,reply_m bytea?,item bigint?,peer boolean,peer_sock bytea?,peer_pid bigint?,peer_start bytea?,u bytea?",
        key: "file_id,extra,seq",
    },
    Table {
        name: "signals",
        columns: "file_id bigint,seq bigint,k bytea,o numeric,t bigint?,at numeric,n bytea?,v bigint?",
        key: "file_id,seq",
    },
    Table {
        name: "usage_identity",
        columns: "file_id bigint,message_id bytea,record_uuid bytea",
        key: "file_id,message_id,record_uuid",
    },
    Table {
        name: "usage",
        columns: "file_id bigint,message_id bytea,model bytea?,input numeric,cached_input numeric,output numeric,reasoning_output numeric,total numeric,model_input numeric,model_output numeric,model_cache_write numeric,model_cache_read numeric,billing_input numeric,billing_output numeric,billing_cache_read numeric,billing_cache_write_5m numeric,billing_cache_write_1h numeric,billing_web_search_requests numeric,billing_speed bytea?,billing_service_tier bytea?,billing_prompt_size numeric,billing_timestamp bigint?,billing_split_unknown boolean,record_ids bytea",
        key: "file_id,message_id",
    },
    Table {
        name: "codex_usage",
        columns: "file_id bigint,seq bigint,model bytea,input numeric,output numeric,cache_write numeric,cache_read numeric,t bigint?",
        key: "file_id,seq",
    },
    Table {
        name: "meta",
        columns: "key bytea,value bytea",
        key: "key_digest",
    },
    Table {
        name: "native_dependencies",
        columns: "source_path bytea,harness bytea,native_id bytea",
        key: "source_path_digest,harness_digest,native_id_digest",
    },
    Table {
        name: "usage_sources",
        columns: "source_id bigint,source_path bytea,receipt bytea,checkpoint numeric,parser_version bigint,usage_ready boolean",
        key: "source_id",
    },
    Table {
        name: "usage_current",
        columns: "source_path bytea,source_id bigint",
        key: "source_path_digest",
    },
    Table {
        name: "usage_facts",
        columns: "source_id bigint,native_offset numeric,identity bytea,payload bytea,record bytea,outcome bytea,model_outcome bytea",
        key: "source_id,native_offset",
    },
    Table {
        name: "usage_identities",
        columns: "source_id bigint,scope bytea,identity bytea,native_offset numeric",
        key: "source_id,scope_digest,identity_digest",
    },
    Table {
        name: "usage_dimensions",
        columns: "source_id bigint,kind bytea,dimension bytea,value bytea",
        key: "source_id,kind_digest,dimension_digest",
    },
    Table {
        name: "tree_generations",
        columns: "projection_id bigint,source_id bigint,version bigint,receipt bytea,complete boolean,work_pending boolean",
        key: "projection_id",
    },
    Table {
        name: "tree_usage_work",
        columns: "projection_id bigint,message_id bytea,tokens bytea",
        key: "projection_id,message_id_digest",
    },
    Table {
        name: "tree_codex_work",
        columns: "projection_id bigint,identity bytea",
        key: "projection_id,identity_digest",
    },
    Table {
        name: "tree_headers",
        columns: "source_path bytea,file_id bigint,projection_id bigint,ledger bytea",
        key: "source_path_digest",
    },
    Table {
        name: "tree_models",
        columns: "projection_id bigint,model bytea",
        key: "projection_id,model_digest",
    },
    Table {
        name: "tree_labels",
        columns: "projection_id bigint,label bytea,value bytea",
        key: "projection_id,label",
    },
    Table {
        name: "tree_tools",
        columns: "projection_id bigint,tool_id bytea,name bytea?,closed boolean",
        key: "projection_id,tool_id_digest",
    },
    Table {
        name: "tree_usage_models",
        columns: "projection_id bigint,message_id bytea,model bytea",
        key: "projection_id,message_id_digest",
    },
    Table {
        name: "tree_copied_identities",
        columns: "file_id bigint,message_id bytea,record_uuid bytea",
        key: "file_id,message_id_digest,record_uuid_digest",
    },
    Table {
        name: "tree_repairs",
        columns: "source_path bytea,receipt bytea,source_id bigint?,cursor numeric,projection_id bigint?,last_model bytea?,tree_root bytea?,state bytea,last_attempt bigint",
        key: "source_path_digest",
    },
    Table {
        name: "recovery",
        columns: "cursor bytea,done boolean",
        key: "cursor",
    },
];

const FOUNDATION: &[Table] = &[
    Table {
        name: "operations",
        columns: "operation_id character(64),kind text,request_digest bytea,outcome jsonb,committed_at timestamp with time zone",
        key: "operation_id",
    },
    Table {
        name: "source_state",
        columns: "revision bigint,backfill_state text?,catalog_generation text?,semantic_pending boolean,catalog_completeness text?,search_backfill_complete boolean,recovery_reason text?",
        key: "",
    },
    Table {
        name: "source_members",
        columns: "source_path bytea",
        key: "source_path",
    },
    Table {
        name: "source_invalidations",
        columns: "source_path bytea,revision bigint,reason text,observed_at bigint",
        key: "source_path",
    },
    Table {
        name: "source_receipts",
        columns: "operation_id text,payload_digest text,outcome text",
        key: "operation_id",
    },
    Table {
        name: "source_intents",
        columns: "operation_id text,payload_digest text,payload text,sequence bigint",
        key: "operation_id",
    },
];

// Indexes are part of the same authoritative manifest, including key order.
const INDEXES: &[(&str, &str, &str, bool)] = &[
    (
        "catalog_operation_generation",
        "catalog_operations",
        "generation",
        false,
    ),
    (
        "semantic_publication_generation",
        "semantic_publications",
        "generation,proof_digest,admission",
        false,
    ),
    (
        "catalog_order",
        "catalog_rows",
        "generation,last_ms,key_digest",
        false,
    ),
    (
        "catalog_history_order",
        "catalog_history_headers",
        "generation,last_ms,key_digest",
        false,
    ),
    (
        "catalog_history_owner_order",
        "catalog_history_owners",
        "generation,last_ms,session_key",
        false,
    ),
    (
        "catalog_search_order",
        "catalog_search_memberships",
        "generation,gram,last_ms,session_key",
        false,
    ),
    (
        "catalog_source_path",
        "catalog_sources",
        "generation,path_digest",
        false,
    ),
    (
        "catalog_source_native",
        "catalog_sources",
        "generation,native_digest",
        false,
    ),
    (
        "catalog_history_source_native",
        "catalog_history_sources",
        "native_digest,generation,key_digest",
        false,
    ),
    (
        "catalog_bookmark_target",
        "catalog_trace_bookmarks",
        "generation,target_digest,session_key",
        false,
    ),
    (
        "catalog_link_peer",
        "catalog_links",
        "generation,peer_digest",
        false,
    ),
    (
        "catalog_dependency_native",
        "catalog_dependencies",
        "generation,harness_digest,native_digest,path_digest",
        false,
    ),
    (
        "relocation_new",
        "relocation_candidates",
        "new_path,old_path",
        false,
    ),
    ("files_path", "files", "path_digest", true),
    ("files_cwd", "files", "cwd_digest,path_digest", false),
    (
        "usage_peers",
        "usage_identity",
        "message_id,record_uuid,file_id",
        false,
    ),
    (
        "usage_source_path",
        "usage_sources",
        "source_path_digest,source_id",
        false,
    ),
    ("usage_current_source", "usage_current", "source_id", true),
    (
        "usage_observations",
        "usage_facts",
        "source_id,identity_digest,native_offset",
        false,
    ),
    (
        "tree_cleanup",
        "tree_generations",
        "complete,work_pending,projection_id",
        false,
    ),
    (
        "tree_copied_peers",
        "tree_copied_identities",
        "message_id_digest,record_uuid_digest,file_id",
        false,
    ),
    (
        "tree_pending_source",
        "tree_generations",
        "source_id,complete,projection_id",
        false,
    ),
    (
        "tree_open_tools",
        "tree_tools",
        "projection_id,closed,tool_id_digest",
        false,
    ),
    (
        "tree_repair_queue",
        "tree_repairs",
        "state,last_attempt,source_path_digest",
        false,
    ),
    (
        "dependency_native",
        "native_dependencies",
        "harness_digest,native_id_digest,source_path_digest",
        false,
    ),
];

const POLICY: &str = "semon_pg_namespace_matches(namespace, namespace_digest, decode(current_setting('semon.namespace'::text, true), 'hex'::text))";

// SELECT-only scope for one host-authorized workspace metadata request. The
// ordinary policy remains the sole authority for every mutation and Raw table.
const WORKSPACE_POLICY: &str = "(encode(namespace, 'hex'::text) = ANY (string_to_array(current_setting('semon.read_scope'::text, true), ','::text)))";
const WORKSPACE_TABLES: &[&str] = &[
    "semon_pg_native_catalog_rows",
    "semon_pg_native_catalog_history_owners",
    "semon_pg_native_catalog_history_headers",
    "semon_pg_native_semantic_runtime",
    "semon_pg_native_meta",
    "semon_pg_native_catalog_operations",
    "semon_pg_source_state",
];

fn contract_hash() -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(b"semon-private-native-v8");
    for table in TABLES.iter().chain(FOUNDATION) {
        for value in [table.name, table.columns, table.key] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        hash.update(format!("{:?}", native_keys::generated(table.name)));
    }
    for index in INDEXES {
        hash.update(format!("{index:?}"));
    }
    hash.update(POLICY);
    hash.update(WORKSPACE_POLICY);
    for table in WORKSPACE_TABLES {
        hash.update(table);
    }
    hash.update(native_keys::MATCH_BODY);
    hash.update(native_keys::DIGEST_BODY);
    for body in super::super::native_usage_identity::contract() {
        hash.update((body.len() as u64).to_be_bytes());
        hash.update(body.as_bytes());
    }
    hash.finalize().to_vec()
}

pub(super) fn install(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    tx.driver.batch_execute(&format!("CREATE FUNCTION semon_pg_native_digest(ns bytea,value bytea,domain text) RETURNS bytea LANGUAGE plpgsql IMMUTABLE STRICT SECURITY INVOKER SET search_path=pg_catalog AS '{}';", native_keys::DIGEST_BODY.replace('\'', "''"))).map_err(sqlstate::error)?;
    tx.driver.batch_execute(&format!("CREATE FUNCTION semon_pg_native_key_matches(stored bytea,digest bytea,expected bytea) RETURNS boolean LANGUAGE plpgsql IMMUTABLE STRICT SECURITY INVOKER SET search_path=pg_catalog AS '{}';", native_keys::MATCH_BODY.replace('\'', "''"))).map_err(sqlstate::error)?;
    tx.driver
        .batch_execute("ALTER TABLE semon_pg_schema ADD COLUMN native_contract bytea")
        .map_err(sqlstate::error)?;
    tx.driver
        .execute(
            "UPDATE semon_pg_schema SET native_contract=$1",
            &[&contract_hash()],
        )
        .map_err(sqlstate::error)?;
    tx.driver
        .batch_execute("ALTER TABLE semon_pg_schema ALTER COLUMN native_contract SET NOT NULL")
        .map_err(sqlstate::error)?;
    let mut ddl = String::new();
    for table in TABLES {
        let name = format!("semon_pg_native_{}", table.name);
        ddl.push_str(&format!("CREATE TABLE {name}(namespace bytea NOT NULL CHECK(octet_length(namespace) BETWEEN 1 AND 4096),namespace_digest bytea GENERATED ALWAYS AS (sha256(namespace)) STORED,"));
        for column in table.columns.split(',') {
            ddl.push_str(column.trim_end_matches('?'));
            if !column.ends_with('?') {
                ddl.push_str(" NOT NULL");
            }
            ddl.push(',');
        }
        for (column, expression, nullable) in native_keys::generated(table.name) {
            let null = if *nullable { "" } else { " NOT NULL" };
            ddl.push_str(&format!(
                "{column} bytea GENERATED ALWAYS AS ({expression}) STORED{null},"
            ));
        }
        ddl.push_str(&format!("PRIMARY KEY(namespace_digest,{})); ALTER TABLE {name} ENABLE ROW LEVEL SECURITY; ALTER TABLE {name} FORCE ROW LEVEL SECURITY; CREATE POLICY native_namespace ON {name} USING ({POLICY}) WITH CHECK ({POLICY});", table.key));
    }
    for table in WORKSPACE_TABLES {
        ddl.push_str(&format!(
            "CREATE POLICY workspace_catalog ON {table} FOR SELECT USING ({WORKSPACE_POLICY});"
        ));
    }
    for (name, table, columns, unique) in INDEXES {
        let unique = if *unique { "UNIQUE" } else { "" };
        ddl.push_str(&format!("CREATE {unique} INDEX semon_pg_native_{name} ON semon_pg_native_{table}(namespace_digest,{columns});"));
    }
    tx.driver.batch_execute(&ddl).map_err(sqlstate::error)?;
    super::super::native_usage_identity::install(tx)
}

pub(super) fn verify(tx: &mut PgTxn<'_>) -> Result<(), StorageError> {
    let digest = tx.driver.query("SELECT p.prosrc,p.prosecdef,p.proisstrict AND p.provolatile='i' AND p.prokind='f' AND p.prorettype='bytea'::regtype AND p.proargtypes='17 17 25'::oidvector AND l.lanname='plpgsql',p.proconfig FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang WHERE p.pronamespace=current_schema()::regnamespace AND p.proname='semon_pg_native_digest'", &[]).map_err(sqlstate::error)?;
    if digest.len() != 1
        || digest[0].get::<_, String>(0) != native_keys::DIGEST_BODY
        || digest[0].get::<_, bool>(1)
        || !digest[0].get::<_, bool>(2)
        || digest[0].get::<_, Option<Vec<String>>>(3) != Some(vec!["search_path=pg_catalog".into()])
    {
        return Err(StorageError::ScopeChanged);
    }
    let function = tx.driver.query("SELECT p.prosrc,p.prosecdef,p.proisstrict AND p.provolatile='i' AND p.prokind='f' AND p.prorettype='boolean'::regtype AND p.proargtypes='17 17 17'::oidvector AND l.lanname='plpgsql',p.proconfig FROM pg_proc p JOIN pg_language l ON l.oid=p.prolang WHERE p.pronamespace=current_schema()::regnamespace AND p.proname='semon_pg_native_key_matches'", &[]).map_err(sqlstate::error)?;
    if function.len() != 1
        || function[0].get::<_, String>(0) != native_keys::MATCH_BODY
        || function[0].get::<_, bool>(1)
        || !function[0].get::<_, bool>(2)
        || function[0].get::<_, Option<Vec<String>>>(3)
            != Some(vec!["search_path=pg_catalog".into()])
    {
        return Err(StorageError::ScopeChanged);
    }
    let row = tx
        .driver
        .query_one("SELECT native_contract FROM semon_pg_schema", &[])
        .map_err(sqlstate::error)?;
    if row
        .try_get::<_, Vec<u8>>(0)
        .map_err(|_| StorageError::ScopeChanged)?
        != contract_hash()
    {
        return Err(StorageError::ScopeChanged);
    }
    // One catalog round trip for all native tables. No unsafe connection cache:
    // a warm reused connection is requalified before the effect callback.
    let rows = tx.driver.query("SELECT c.relname, c.relrowsecurity AND c.relforcerowsecurity AND c.relkind='r' AND has_table_privilege(c.oid,'SELECT') AND has_table_privilege(c.oid,'INSERT') AND has_table_privilege(c.oid,'UPDATE') AND has_table_privilege(c.oid,'DELETE'),
        ARRAY(SELECT a.attname::text || ' ' || format_type(a.atttypid,a.atttypmod) || CASE WHEN a.attnotnull THEN '' ELSE '?' END FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum),
        ARRAY(SELECT a.attname::text FROM pg_constraint k CROSS JOIN LATERAL unnest(k.conkey) WITH ORDINALITY x(num,ord) JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=x.num WHERE k.conrelid=c.oid AND k.contype='p' ORDER BY x.ord),
        ARRAY(SELECT p.polname::text || ':' || p.polcmd::text || ':' || p.polpermissive::text || ':' || p.polroles::text || ':' || pg_get_expr(p.polqual,p.polrelid) || ':' || coalesce(pg_get_expr(p.polwithcheck,p.polrelid),'') FROM pg_policy p WHERE p.polrelid=c.oid ORDER BY p.polname),
        (SELECT pg_get_expr(d.adbin,d.adrelid) FROM pg_attribute a JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=c.oid AND a.attname='namespace_digest' AND a.attgenerated='s'),
        ARRAY(SELECT i.relname::text || ':' || x.indisunique::text || ':' || x.indisvalid::text || ':' || x.indisready::text || ':' || array_to_string(ARRAY(SELECT a.attname::text FROM unnest(x.indkey) WITH ORDINALITY z(num,ord) LEFT JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=z.num ORDER BY z.ord),',') || ':' || coalesce(pg_get_expr(x.indpred,x.indrelid),'') FROM pg_index x JOIN pg_class i ON i.oid=x.indexrelid WHERE x.indrelid=c.oid ORDER BY i.relname),
        ARRAY(SELECT a.attname::text || ':' || pg_get_expr(d.adbin,d.adrelid) FROM pg_attribute a JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=c.oid AND a.attgenerated='s' ORDER BY a.attname)
        FROM pg_class c WHERE c.relnamespace=current_schema()::regnamespace AND c.relname LIKE 'semon_pg_%' AND c.relname<>'semon_pg_schema' AND c.relname NOT IN ('semon_pg_store_owner','semon_pg_store_state','semon_pg_store_operations') AND c.relkind NOT IN ('i','I','S') ORDER BY c.relname", &[]).map_err(sqlstate::error)?;
    if rows.len() != TABLES.len() + FOUNDATION.len() {
        return Err(StorageError::ScopeChanged);
    }
    for (table, native) in TABLES
        .iter()
        .map(|table| (table, true))
        .chain(FOUNDATION.iter().map(|table| (table, false)))
    {
        let prefix = if native {
            "semon_pg_native_"
        } else {
            "semon_pg_"
        };
        let name = format!("{prefix}{}", table.name);
        let row = rows
            .iter()
            .find(|r| r.get::<_, String>(0) == name)
            .ok_or(StorageError::ScopeChanged)?;
        let mut columns = vec![
            "namespace bytea".to_owned(),
            "namespace_digest bytea".to_owned(),
        ];
        columns.extend(table.columns.split(',').map(str::to_owned));
        columns.extend(
            native_keys::generated(table.name)
                .iter()
                .map(|(name, _, nullable)| {
                    format!("{name} bytea{}", if *nullable { "?" } else { "" })
                }),
        );
        let mut generated = vec!["namespace_digest:sha256(namespace)".to_owned()];
        generated.extend(
            native_keys::generated(table.name)
                .iter()
                .map(|(name, expression, _)| format!("{name}:{expression}")),
        );
        generated.sort();
        let keys = if table.key.is_empty() {
            "namespace_digest".to_owned()
        } else {
            format!("namespace_digest,{}", table.key)
        };
        let key = keys.split(',').map(str::to_owned).collect::<Vec<_>>();
        let policy_name = if native {
            "native_namespace".to_owned()
        } else if table.name == "operations" {
            "semon_pg_operation_namespace".to_owned()
        } else {
            format!("{name}_namespace")
        };
        let check = if !native && table.name == "operations" {
            "(namespace = decode(current_setting('semon.namespace'::text, true), 'hex'::text))"
        } else {
            POLICY
        };
        let policy = format!("{policy_name}:*:true:{{0}}:{POLICY}:{check}");
        let mut policies = vec![policy];
        if WORKSPACE_TABLES.contains(&name.as_str()) {
            policies.push(format!(
                "workspace_catalog:r:true:{{0}}:{WORKSPACE_POLICY}:"
            ));
        }
        let mut indexes = vec![format!("{name}_pkey:true:true:true:{keys}:")];
        for (index, target, keys, unique) in INDEXES {
            if native && *target == table.name {
                indexes.push(format!(
                    "semon_pg_native_{index}:{unique}:true:true:namespace_digest,{keys}:"
                ));
            }
        }
        if !native && table.name == "source_invalidations" {
            indexes.push(
                "semon_pg_source_pending:false:true:true:namespace_digest,revision,source_path:"
                    .to_owned(),
            );
        }
        if !native && table.name == "source_intents" {
            indexes.push(
                "semon_pg_source_intent_order:false:true:true:namespace_digest,sequence:"
                    .to_owned(),
            );
        }
        indexes.sort();
        if !row.get::<_, bool>(1)
            || row.get::<_, Vec<String>>(2) != columns
            || row.get::<_, Vec<String>>(3) != key
            || row.get::<_, Vec<String>>(4) != policies
            || row.get::<_, Option<String>>(5).as_deref() != Some("sha256(namespace)")
            || row.get::<_, Vec<String>>(6) != indexes
            || row.get::<_, Vec<String>>(7) != generated
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    super::super::native_usage_identity::verify(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::postgres::tests::{count, fixture, identity, limits, ns};

    #[test]
    fn pg_native_shared_parser_tree_values_roundtrip_without_sqlite() {
        use crate::events::{EventCache, scan_file, tree_rows::TreeRows};
        let (pool, _) = fixture(|_| {});
        let path = std::env::temp_dir().join(format!(
            "pg88-parser-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let id = "tool\0中é";
        let line = serde_json::json!({"type":"assistant","uuid":"u-1","timestamp":"2026-09-29T00:00:01Z","message":{"id":"m-1","role":"assistant","model":"claude-test","content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":"fixture body stays in Raw"}}]}});
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let cache_path = path.with_extension("index.json");
        let source = crate::storage::HostSource::new(ns("parser-fixture"), "a".repeat(64)).unwrap();
        let mut cache = EventCache::open(&cache_path, &source).unwrap();
        let index = scan_file(&path, "claude", &mut cache, &mut false).unwrap();
        let tree = TreeRows::new(index.tree_summary.as_ref().unwrap()).unwrap();
        assert_eq!(tree.tools.get(id).map(String::as_str), Some("Bash"));
        pool.transaction(&ns("parser\0中"),TxnKind::Write,&PgCancellation::default(),|tx| {
            for (id,name) in tree.tools {
                tx.driver.execute("INSERT INTO semon_pg_native_tree_tools(namespace,projection_id,tool_id,name,closed) VALUES($1,1,$2,$3,false)",&[&tx.namespace.as_str().as_bytes(),&id.as_bytes(),&name.as_bytes()]).map_err(sqlstate::error)?;
            }
            let row=tx.driver.query_one("SELECT tool_id,name FROM semon_pg_native_tree_tools",&[]).map_err(sqlstate::error)?;
            assert_eq!(row.get::<_,Vec<u8>>(0),id.as_bytes());
            assert_eq!(row.get::<_,Vec<u8>>(1),b"Bash");
            Ok(())
        }).unwrap();
        drop(cache);
        std::fs::remove_file(EventCache::path(&cache_path)).unwrap();
        std::fs::remove_file(path).unwrap();
    }

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
            assert_eq!(
                client
                    .query_one(
                        &format!("SELECT count(*) FROM semon_pg_native_{}", table.name),
                        &[]
                    )
                    .unwrap()
                    .get::<_, i64>(0),
                0
            );
            let refused = pool.transaction(&ns("wrong"), TxnKind::Write, &control, |tx| {
                tx.driver
                    .execute(
                        &format!("UPDATE semon_pg_native_{} SET namespace=$1", table.name),
                        &[&b"foreign".as_slice()],
                    )
                    .map_err(sqlstate::error)?;
                // No other namespace's rows were visible to mutate.
                let count = tx
                    .driver
                    .query_one(
                        &format!("SELECT count(*) FROM semon_pg_native_{}", table.name),
                        &[],
                    )
                    .map_err(sqlstate::error)?;
                assert_eq!(count.get::<_, i64>(0), 0);
                Ok(())
            });
            assert_eq!(refused, Ok(()));
            assert!(
                pool.transaction(&ns("a\0中"), TxnKind::Write, &control, |tx| {
                    tx.driver
                        .execute(
                            &format!("UPDATE semon_pg_native_{} SET namespace=$1", table.name),
                            &[&b"foreign".as_slice()],
                        )
                        .map_err(sqlstate::error)?;
                    Ok(())
                })
                .is_err()
            );
        }
    }

    #[test]
    fn pg_native_shape_drift_refuses_warm_admission_before_effects() {
        for (change, controlled) in [
            ("ALTER TABLE semon_pg_native_files DROP COLUMN title", true),
            ("DROP INDEX semon_pg_native_files_cwd", true),
            (
                "DROP POLICY native_namespace ON semon_pg_native_events",
                true,
            ),
            (
                "ALTER TABLE semon_pg_native_tree_labels NO FORCE ROW LEVEL SECURITY",
                true,
            ),
            (
                "ALTER TABLE semon_pg_native_signals ALTER COLUMN v TYPE numeric",
                true,
            ),
            (
                "ALTER TABLE semon_pg_native_recovery DROP CONSTRAINT semon_pg_native_recovery_pkey",
                true,
            ),
            (
                "ALTER TABLE semon_pg_native_meta ALTER COLUMN namespace_digest DROP EXPRESSION",
                true,
            ),
            (
                "UPDATE semon_pg_schema SET native_contract=decode('00','hex')",
                false,
            ),
        ] {
            let (pool, factory) = fixture(|bounds| bounds.max_connections = 1);
            let control = PgCancellation::default();
            pool.transaction(&ns("warm"), TxnKind::ReadSnapshot, &control, |_| Ok(()))
                .unwrap();
            if controlled {
                pool.controlled_schema_change(&control, |tx| {
                    tx.driver.batch_execute(change).map_err(sqlstate::error)
                })
                .unwrap();
            } else {
                factory.client().batch_execute(change).unwrap();
            }
            assert_eq!(
                pool.apply_operation(&ns("warm"), &identity(), &control, |_| panic!(
                    "drift admitted effects: {change}"
                )),
                Err(StorageError::ScopeChanged),
                "{change}"
            );
            assert_eq!(count(&factory), 0);
            assert!(matches!(
                PgPool::open(factory, limits()),
                Err(StorageError::ScopeChanged)
            ));
        }
    }

    #[test]
    fn pg_native_revoked_privilege_refuses_before_other_effects() {
        let (pool, factory) = fixture(|_| {});
        factory.client().batch_execute("REVOKE ALL ON semon_pg_native_meta FROM PUBLIC; REVOKE ALL ON semon_pg_native_meta FROM CURRENT_USER").unwrap();
        assert_eq!(
            pool.apply_operation(
                &ns("revoke"),
                &identity(),
                &PgCancellation::default(),
                |_| panic!("revocation admitted effects")
            ),
            Err(StorageError::ScopeChanged)
        );
        assert_eq!(count(&factory), 0);
    }
}
