//! Private exact-path current native reads. Physical cells feed shared decoders.
use super::{sqlstate, txn::PgTxn};
use crate::{
    events::{
        Ledger,
        native_decode::{self as decode, Values},
    },
    storage::StorageError,
};
use serde_json::Value;
use tokio_postgres::types::{ToSql, Type};

pub(super) const LEDGER: &str =
    "file_id,dev:u,ino:u,size:u,mtime_ns:x,resume_at:u,head_sha256:x,tail_sha256:x,ctime_ns:x";
const FILE: &str = "file_id,path,dev:u,ino:u,size:u,mtime_ns:x,resume_at:u,head_sha256:x,tail_sha256:x,entrypoint,title,agent_name,last_model,failed,first_ms,last_ms,cwd,branch,pending,tool_ids,yields,busy,links,codex_tokens,codex_tokens_by_model,rate_limits,codex_native_usage,copilot,ctime_ns:x";
const EVENT: &str = "extra,seq,k,o:u,b:u,t,id,n,code_mode,code_mode_open,parent,script,y,poll,reply,reply_o:u,reply_b:u,reply_t,reply_e,reply_f,reply_m,item,peer,peer_sock,peer_pid,peer_start,u";
const SIGNAL: &str = "seq,k,o:u,t,at:u,n,v";
const USAGE: &str = "message_id,model,input:u,cached_input:u,output:u,reasoning_output:u,total:u,model_input:u,model_output:u,model_cache_write:u,model_cache_read:u,billing_input:u,billing_output:u,billing_cache_read:u,billing_cache_write_5m:u,billing_cache_write_1h:u,billing_web_search_requests:u,billing_speed,billing_service_tier,billing_prompt_size:u,billing_timestamp,billing_split_unknown,record_ids";
const CODEX: &str = "seq,model,input:u,output:u,cache_write:u,cache_read:u,t";

fn bad(_: decode::DecodeError) -> StorageError {
    StorageError::ScopeChanged
}

/// The SQL size probe precedes payload decoding, including wide opaque keys.
/// Selected file reads are bounded; unsupported larger work refuses explicitly.
pub(super) fn rows(
    tx: &mut PgTxn<'_>,
    table: &str,
    columns: &str,
    predicate: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<Values>, StorageError> {
    tx.check()?;
    let names = columns.split(',').collect::<Vec<_>>();
    let select = names
        .iter()
        .map(|c| {
            let name = c.split(':').next().unwrap();
            if c.ends_with(":u") {
                format!("{name}::text")
            } else {
                name.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let sizes = names
        .iter()
        .map(|c| {
            format!(
                "coalesce(octet_length({}::text),0)::bigint",
                c.split(':').next().unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("+");
    let probe =
        format!("SELECT {sizes}+128 FROM semon_pg_native_{table} WHERE {predicate} LIMIT 65537");
    let sizes = tx.driver.query(&probe, params).map_err(sqlstate::error)?;
    if sizes.len() > 65536 {
        return Err(StorageError::Busy);
    }
    let mut total = 0usize;
    for row in &sizes {
        total = total
            .checked_add(
                usize::try_from(row.get::<_, i64>(0)).map_err(|_| StorageError::ScopeChanged)?,
            )
            .ok_or(StorageError::Busy)?;
        if total > 1024 * 1024 {
            return Err(StorageError::Busy);
        }
    }
    crate::selected_read::charge(sizes.len() * 128, total).map_err(|_| StorageError::Busy)?;
    tx.check()?;
    let raw = tx
        .driver
        .query(
            &format!("SELECT {select} FROM semon_pg_native_{table} WHERE {predicate} LIMIT 65537"),
            params,
        )
        .map_err(sqlstate::error)?;
    if raw.len() != sizes.len() {
        return Err(StorageError::ScopeChanged);
    }
    raw.into_iter()
        .map(|r| {
            let mut values = Vec::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                let t = r.columns()[i].type_();
                let v = if name.ends_with(":u") {
                    r.try_get::<_, Option<String>>(i)
                        .map_err(|_| StorageError::Unavailable)?
                        .map(|s| {
                            s.parse::<u64>()
                                .map(|v| Value::from(v as i64))
                                .map_err(|_| StorageError::ScopeChanged)
                        })
                        .transpose()?
                        .unwrap_or(Value::Null)
                } else if *t == Type::BYTEA {
                    r.try_get::<_, Option<Vec<u8>>>(i)
                        .map_err(|_| StorageError::Unavailable)?
                        .map(|b| {
                            if name.ends_with(":x") {
                                serde_json::to_value(b).map_err(|_| StorageError::Unavailable)
                            } else {
                                String::from_utf8(b)
                                    .map(Value::String)
                                    .map_err(|_| StorageError::ScopeChanged)
                            }
                        })
                        .transpose()?
                        .unwrap_or(Value::Null)
                } else if *t == Type::BOOL {
                    r.try_get::<_, Option<bool>>(i)
                        .map_err(|_| StorageError::Unavailable)?
                        .map(Value::Bool)
                        .unwrap_or(Value::Null)
                } else if *t == Type::INT2 {
                    r.try_get::<_, Option<i16>>(i)
                        .map_err(|_| StorageError::Unavailable)?
                        .map(Value::from)
                        .unwrap_or(Value::Null)
                } else if *t == Type::INT8 {
                    r.try_get::<_, Option<i64>>(i)
                        .map_err(|_| StorageError::Unavailable)?
                        .map(Value::from)
                        .unwrap_or(Value::Null)
                } else {
                    return Err(StorageError::ScopeChanged);
                };
                values.push(v);
            }
            Ok(Values(values))
        })
        .collect()
}

pub(super) fn ledger(
    tx: &mut PgTxn<'_>,
    path: &str,
) -> Result<Option<(i64, Ledger)>, StorageError> {
    let found = rows(
        tx,
        "files",
        LEDGER,
        "namespace_digest=sha256($1::bytea) AND namespace=$1 AND path=$2",
        &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()],
    )?;
    if found.len() > 1 {
        return Err(StorageError::ScopeChanged);
    }
    found.first().map(decode::ledger).transpose().map_err(bad)
}

pub(super) fn file(
    tx: &mut PgTxn<'_>,
    path: &str,
) -> Result<Option<(Ledger, crate::events::FileIndex)>, StorageError> {
    let Some((id, ledger)) = ledger(tx, path)? else {
        return Ok(None);
    };
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let raw = rows(
        tx,
        "files",
        FILE,
        "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2",
        &[&ns, &id],
    )?;
    let row = raw.first().ok_or(StorageError::ScopeChanged)?;
    let mut events = Vec::new();
    let mut extras = Vec::new();
    // Each ordered cohort has its own dense native ordinal sequence.
    for extra in [false, true] {
        let list = if extra { &mut extras } else { &mut events };
        let children = rows(
            tx,
            "events",
            EVENT,
            "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 AND extra=$3 ORDER BY seq",
            &[&ns, &id, &extra],
        )?;
        for r in children {
            let (stored_extra, seq, event) = decode::event(&r).map_err(bad)?;
            if seq != list.len() || stored_extra != extra {
                return Err(StorageError::ScopeChanged);
            }
            list.push(event);
        }
    }
    let mut signals = Vec::new();
    for r in rows(
        tx,
        "signals",
        SIGNAL,
        "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 ORDER BY seq",
        &[&ns, &id],
    )? {
        let (seq, s) = decode::signal(&r).map_err(bad)?;
        if seq != signals.len() {
            return Err(StorageError::ScopeChanged);
        }
        signals.push(s);
    }
    let mut usage_by_id = std::collections::BTreeMap::new();
    for r in rows(
        tx,
        "usage",
        USAGE,
        "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 ORDER BY message_id",
        &[&ns, &id],
    )? {
        let (key, usage) = decode::usage(&r).map_err(bad)?;
        if usage_by_id.insert(key, usage).is_some() {
            return Err(StorageError::ScopeChanged);
        }
    }
    let mut codex_usage_events = Vec::new();
    for r in rows(
        tx,
        "codex_usage",
        CODEX,
        "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 ORDER BY seq",
        &[&ns, &id],
    )? {
        let (seq, usage) = decode::codex_usage(&r).map_err(bad)?;
        if seq != codex_usage_events.len() {
            return Err(StorageError::ScopeChanged);
        }
        codex_usage_events.push(usage);
    }
    let source = tx.driver.query("SELECT s.source_id,s.usage_ready,s.receipt FROM semon_pg_native_usage_current c JOIN semon_pg_native_usage_sources s USING(namespace,source_id) WHERE c.namespace_digest=sha256($1) AND c.namespace=$1 AND c.source_path=$2 AND s.parser_version=$3", &[&ns,&path.as_bytes(),&i64::from(crate::events::CACHE_VERSION)]).map_err(sqlstate::error)?;
    let s = source.first().ok_or(StorageError::ScopeChanged)?;
    let source_id: i64 = s.get(0);
    let normalized: bool = s.get(1);
    if s.get::<_, Vec<u8>>(2) != crate::events::store::header_ledger(&ledger).as_bytes() {
        return Err(StorageError::ScopeChanged);
    }
    let mut native = row
        .get::<Option<String>>(26)
        .map_err(bad)?
        .map(|s| decode::usage_root(&s).map_err(bad))
        .transpose()?;
    if let Some(native) = &mut native {
        if !normalized {
            return Err(StorageError::ScopeChanged);
        }
        for r in rows(
            tx,
            "usage_dimensions",
            "kind,dimension,value",
            "namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_id=$2 ORDER BY kind,dimension",
            &[&ns, &source_id],
        )? {
            decode::usage_dimension(
                native,
                &r.get::<String>(0).map_err(bad)?,
                r.get(1).map_err(bad)?,
                &r.get::<String>(2).map_err(bad)?,
            )
            .map_err(bad)?;
        }
        decode::finish_usage(native).map_err(bad)?;
    }
    let pinned = super::native_pages::current(tx, path, &ledger)?;
    let header: crate::events::native_pages::chunks::Header =
        serde_json::from_str(&pinned.chunks_header).map_err(|_| StorageError::ScopeChanged)?;
    crate::selected_read::charge(header.count.checked_mul(32).ok_or(StorageError::Busy)?, 0)
        .map_err(|_| StorageError::Busy)?;
    let mut hashes = Vec::with_capacity(header.count);
    for i in 0..header.count {
        hashes.push(pinned.chunk_hash(tx, i)?);
    }
    let chunks = crate::source_chunks::Chunks {
        offset: ledger.offset,
        hashes,
        resume: header.resume,
    };
    if !chunks.valid(ledger.offset) || chunks.fingerprint() != header.fingerprint {
        return Err(StorageError::ScopeChanged);
    }
    let handoff = pinned.launch(tx)?;
    let marker = metadata::<crate::events::native_aux::ParentMarkerRecord>(
        tx,
        &format!("native_parent_marker:{path}"),
    )?
    .map(|r| {
        if r.version != 1
            || r.parser_version != crate::events::CACHE_VERSION
            || r.ledger != crate::events::store::header_ledger(&ledger)
            || !r.index.valid(&ledger)
        {
            return Err(StorageError::ScopeChanged);
        }
        Ok(r.index)
    })
    .transpose()?;
    let codex = metadata::<crate::events::native_aux::CodexHeaderRecord>(
        tx,
        &format!("native_codex_header:{path}"),
    )?
    .map(|r| {
        if r.version != 1
            || r.parser_version != crate::events::CACHE_VERSION
            || r.ledger != crate::events::store::header_ledger(&ledger)
        {
            return Err(StorageError::ScopeChanged);
        }
        Ok(r.header)
    })
    .transpose()?;
    let sidecar = metadata::<crate::events::native_aux::ClaudeSidecarRecord>(
        tx,
        &format!("native_claude_sidecar:{path}"),
    )?
    .map(|r| {
        if r.version != 1
            || r.parser_version != crate::events::CACHE_VERSION
            || r.ledger != crate::events::store::header_ledger(&ledger)
            || !r.sidecar.valid()
        {
            return Err(StorageError::ScopeChanged);
        }
        Ok(r.sidecar)
    })
    .transpose()?;
    let children = decode::Children {
        events,
        extras,
        signals,
        usage_by_id,
        codex_usage_events,
        native_usage_observations: normalized.then(Vec::new),
        source_chunks: Some(std::sync::Arc::new(chunks)),
        marker_index: marker,
        tree_summary: None,
        handoff_index: Some(handoff),
        codex_header: codex,
        claude_sidecar: sidecar,
        codex_native_usage: native,
    };
    let index = decode::file(row, children).map_err(bad)?;
    Ok(Some((ledger, index)))
}

fn metadata<T: serde::de::DeserializeOwned>(
    tx: &mut PgTxn<'_>,
    key: &str,
) -> Result<Option<T>, StorageError> {
    super::tree_writer::metadata(tx, key.as_bytes())?
        .map(|s| serde_json::from_str(&s).map_err(|_| StorageError::ScopeChanged))
        .transpose()
}

pub(super) fn tree_parts(
    tx: &mut PgTxn<'_>,
    path: &str,
    expected: &crate::events::Stat,
) -> Result<
    Option<(
        Ledger,
        crate::native_marker::Index,
        crate::native_handoff::Index,
    )>,
    StorageError,
> {
    let Some((_, ledger)) = ledger(tx, path)? else {
        return Ok(None);
    };
    if ledger.stat != *expected {
        return Ok(None);
    }
    let exists:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_tree_headers WHERE namespace=$1 AND source_path=$2)",&[&tx.namespace.as_str().as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if !exists {
        return Ok(None);
    }
    let marker = metadata::<crate::events::native_aux::ParentMarkerRecord>(
        tx,
        &format!("native_parent_marker:{path}"),
    )?
    .ok_or(StorageError::ScopeChanged)?;
    if marker.version != 1
        || marker.parser_version != crate::events::CACHE_VERSION
        || marker.ledger != crate::events::store::header_ledger(&ledger)
        || !marker.index.valid(&ledger)
    {
        return Err(StorageError::ScopeChanged);
    }
    let pinned = super::native_pages::current(tx, path, &ledger)?;
    let calls = pinned.launch(tx)?;
    Ok(Some((ledger, marker.index, calls)))
}
