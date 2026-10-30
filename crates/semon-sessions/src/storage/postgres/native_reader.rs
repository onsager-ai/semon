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
pub(super) const FILE: &str = "file_id,path,dev:u,ino:u,size:u,mtime_ns:x,resume_at:u,head_sha256:x,tail_sha256:x,entrypoint,title,agent_name,last_model,failed,first_ms,last_ms,cwd,branch,pending,tool_ids,yields,busy,links,codex_tokens,codex_tokens_by_model,rate_limits,codex_native_usage,copilot,ctime_ns:x";
pub(super) const EVENT: &str = "extra,seq,k,o:u,b:u,t,id,n,code_mode,code_mode_open,parent,script,y,poll,reply,reply_o:u,reply_b:u,reply_t,reply_e,reply_f,reply_m,item,peer,peer_sock,peer_pid,peer_start,u";
pub(super) const SIGNAL: &str = "seq,k,o:u,t,at:u,n,v";
pub(super) const USAGE: &str = "message_id,model,input:u,cached_input:u,output:u,reasoning_output:u,total:u,model_input:u,model_output:u,model_cache_write:u,model_cache_read:u,billing_input:u,billing_output:u,billing_cache_read:u,billing_cache_write_5m:u,billing_cache_write_1h:u,billing_web_search_requests:u,billing_speed,billing_service_tier,billing_prompt_size:u,billing_timestamp,billing_split_unknown,record_ids";
pub(super) const CODEX: &str = "seq,model,input:u,output:u,cache_write:u,cache_read:u,t";

fn bad(_: decode::DecodeError) -> StorageError {
    StorageError::ScopeChanged
}

/// The SQL size probe precedes payload decoding, including wide opaque keys.
/// Keyset pages are charged before payload decoding. The selected-read scope
/// owns the aggregate budget; family length is not a separate format limit.
pub(super) fn rows(
    tx: &mut PgTxn<'_>,
    table: &str,
    columns: &str,
    predicate: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<Values>, StorageError> {
    read_rows(tx, table, columns, predicate, params, false)
}

pub(super) fn transfer_rows(
    tx: &mut PgTxn<'_>,
    table: &str,
    columns: &str,
    predicate: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<Values>, StorageError> {
    read_rows(tx, table, columns, predicate, params, true)
}

fn read_rows(
    tx: &mut PgTxn<'_>,
    table: &str,
    columns: &str,
    predicate: &str,
    params: &[&(dyn ToSql + Sync)],
    one_page: bool,
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
    let keys: &[&str] = match table {
        "files" => &["file_id"],
        "events" | "signals" | "codex_usage" => &["seq"],
        "usage" => &["message_id"],
        "usage_identity" => &["message_id", "record_uuid"],
        "usage_facts" => &["native_offset"],
        "usage_identities" => &["scope", "identity"],
        "usage_dimensions" => &["kind", "dimension"],
        _ => return Err(StorageError::InvalidArguments),
    };
    let predicate = predicate
        .rsplit_once(" ORDER BY ")
        .map_or(predicate, |(base, _)| base);
    let mut cursor: Vec<Box<dyn ToSql + Sync>> = Vec::new();
    let mut decoded = Vec::new();
    loop {
        let mut binds = params.to_vec();
        binds.extend(cursor.iter().map(|v| &**v as &(dyn ToSql + Sync)));
        let after = if cursor.is_empty() {
            String::new()
        } else {
            let slots = (1..=keys.len())
                .map(|i| format!("${}", params.len() + i))
                .collect::<Vec<_>>()
                .join(",");
            format!(" AND ({}) > ({slots})", keys.join(","))
        };
        // Cast unsigned values to text only for transport. Qualify the input
        // columns so ORDER BY cannot resolve the text output alias instead.
        let order = keys
            .iter()
            .map(|key| format!("semon_pg_native_{table}.{key}"))
            .collect::<Vec<_>>()
            .join(",");
        let suffix = format!("WHERE {predicate}{after} ORDER BY {order}");
        let probe = format!("SELECT {sizes}+128 FROM semon_pg_native_{table} {suffix} LIMIT 64");
        let sizes = tx.driver.query(&probe, &binds).map_err(sqlstate::error)?;
        if sizes.is_empty() {
            return Ok(decoded);
        }
        let mut total = 0usize;
        let mut count = 0usize;
        for row in &sizes {
            let next = total
                .checked_add(
                    usize::try_from(row.get::<_, i64>(0))
                        .map_err(|_| StorageError::ScopeChanged)?,
                )
                .ok_or(StorageError::Busy)?;
            if next > 1024 * 1024 {
                break;
            }
            total = next;
            count += 1;
        }
        if count == 0 {
            return Err(StorageError::Busy);
        }
        crate::selected_read::charge(count * 128, total).map_err(|_| StorageError::Busy)?;
        tx.check()?;
        let raw = tx
            .driver
            .query(
                &format!("SELECT {select} FROM semon_pg_native_{table} {suffix} LIMIT {count}"),
                &binds,
            )
            .map_err(sqlstate::error)?;
        if raw.len() != count {
            return Err(StorageError::ScopeChanged);
        }
        let last = raw.last().ok_or(StorageError::ScopeChanged)?;
        let mut next: Vec<Box<dyn ToSql + Sync>> = Vec::new();
        for key in keys {
            if one_page {
                break;
            }
            let at = names
                .iter()
                .position(|name| name.split(':').next() == Some(*key))
                .ok_or(StorageError::ScopeChanged)?;
            if *last.columns()[at].type_() == Type::BYTEA {
                next.push(Box::new(
                    last.try_get::<_, Vec<u8>>(at)
                        .map_err(|_| StorageError::ScopeChanged)?,
                ));
            } else {
                next.push(Box::new(
                    last.try_get::<_, i64>(at)
                        .map_err(|_| StorageError::ScopeChanged)?,
                ));
            }
        }
        let page = raw
            .into_iter()
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
            .collect::<Result<Vec<_>, StorageError>>()?;
        decoded.extend(page);
        if one_page {
            return Ok(decoded);
        }
        cursor = next;
    }
}

pub(super) fn ledger(
    tx: &mut PgTxn<'_>,
    path: &str,
) -> Result<Option<(i64, Ledger)>, StorageError> {
    let found = rows(
        tx,
        "files",
        LEDGER,
        "namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(path,path_digest,$2)",
        &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()],
    )?;
    if found.len() > 1 {
        return Err(StorageError::ScopeChanged);
    }
    found.first().map(decode::ledger).transpose().map_err(bad)
}

pub(super) fn runtime_source_cwds(
    tx: &mut PgTxn<'_>,
    sources: &[crate::model::summary::CatalogSource],
) -> Result<std::collections::BTreeSet<String>, StorageError> {
    if sources.is_empty() || sources.len() > 64 {
        return Err(StorageError::ScopeChanged);
    }
    let mut cwds = std::collections::BTreeSet::new();
    for source in sources {
        let path = source.path.to_string_lossy();
        if ledger(tx, &path)?.as_ref().map(|(_, ledger)| ledger)
            != Some(&crate::source_carrier::ledger(source))
        {
            return Err(StorageError::ScopeChanged);
        }
        let values = rows(tx, "files", "cwd",
            "namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(path,path_digest,$2)",
            &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()])?;
        let value = values.first().ok_or(StorageError::ScopeChanged)?;
        if let Some(cwd) = value.get::<Option<String>>(0).map_err(bad)? {
            cwds.insert(cwd);
        }
    }
    Ok(cwds)
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

/// Qualify the exact small family authorities written by the bundle without
/// restoring all Event/Usage history just to issue an operation receipt.
/// Core/Usage/Tree row effects still run in the caller's same transaction.
pub(super) fn qualify_completion(
    tx: &mut PgTxn<'_>,
    path: &str,
    ledger: &Ledger,
    index: &crate::events::FileIndex,
) -> Result<(), StorageError> {
    use crate::events::native_aux;
    let header = native_aux::codex_header(ledger, index.codex_header.as_ref())
        .map_err(|_| StorageError::Unavailable)?;
    let sidecar = native_aux::claude_sidecar(ledger, index.claude_sidecar.as_ref())
        .map_err(|_| StorageError::Unavailable)?;
    let marker = native_aux::parent_marker(ledger, index.marker_index.as_ref())
        .map_err(|_| StorageError::Unavailable)?;
    if (index.codex_header.is_some() && header.is_none())
        || (index.claude_sidecar.is_some() && sidecar.is_none())
        || (index.marker_index.is_some() && marker.is_none())
    {
        return Err(StorageError::ScopeChanged);
    }
    for (family, expected, check) in [
        ("native_codex_header", header, true),
        (
            "native_claude_sidecar",
            sidecar,
            index.claude_sidecar.is_some()
                || crate::native_sidecar::agent_path(std::path::Path::new(path)),
        ),
        ("native_parent_marker", marker, true),
    ] {
        if !check {
            continue;
        }
        tx.check()?;
        let key = format!("{family}:{path}");
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT value=$3 FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND semon_pg_native_key_matches(key,key_digest,$2)", &[&ns, &key.as_bytes(), &expected.as_ref().map(|s| s.as_bytes())]).map_err(sqlstate::error)?;
        match (expected, rows.as_slice()) {
            (None, []) => {}
            (Some(_), [row]) if row.get::<_, bool>(0) => {}
            _ => return Err(StorageError::ScopeChanged),
        }
    }
    let ns = tx.namespace.as_str().as_bytes();
    let receipt = crate::events::store::header_ledger(ledger);
    let source = tx.driver.query("SELECT s.receipt=$3 AND s.parser_version=$4 AND s.usage_ready=$5 FROM semon_pg_native_usage_current c JOIN semon_pg_native_usage_sources s USING(namespace,source_id) WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND s.namespace=$1 AND c.source_path=$2 AND s.source_path=$2", &[&ns, &path.as_bytes(), &receipt.as_bytes(), &i64::from(crate::events::CACHE_VERSION), &index.native_usage_observations.is_some()]).map_err(sqlstate::error)?;
    if source.len() != 1 || !source[0].get::<_, bool>(0) {
        return Err(StorageError::ScopeChanged);
    }
    tx.check()
}

/// Metadata-only recovery enumeration. Each SQL/decode page is at most 64
/// files; Event, Usage, Tree and proof history never enters this operation.
pub(super) fn file_ledgers(
    tx: &mut PgTxn<'_>,
) -> Result<Vec<(String, Option<Ledger>)>, StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let mut after = 0i64;
    let mut result = Vec::new();
    loop {
        let page = rows(
            tx,
            "files",
            &format!("{LEDGER},path"),
            "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id IN (SELECT file_id FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id>$2 ORDER BY file_id LIMIT 64) ORDER BY file_id",
            &[&ns, &after],
        )?;
        if page.is_empty() {
            return Ok(result);
        }
        for row in page {
            let (id, ledger) = decode::ledger(&row).map_err(bad)?;
            if id <= after {
                return Err(StorageError::ScopeChanged);
            }
            after = id;
            let path = row.get::<String>(9).map_err(bad)?;
            if let Some(bytes) = super::native_keys::read_meta(
                tx,
                format!("source-retired:{path}").as_bytes(),
                16384,
            )? {
                let retired: Option<Ledger> =
                    serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
                if retired.as_ref() == Some(&ledger) {
                    continue;
                }
                return Err(StorageError::ScopeChanged);
            }
            result.push((path, Some(ledger)));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::postgres::{
        PgCancellation,
        tests::{fixture, ns},
        txn::TxnKind,
    };

    #[test]
    fn pg_native126_selected_family_keysets_across_one_mib() {
        let (pool, _factory) = fixture(|_| {});
        let namespace = ns("native130-reader-pages");
        let control = PgCancellation::default();
        pool.transaction(&namespace, TxnKind::Write, &control, |tx| {
            let ns = tx.namespace.as_str().as_bytes();
            tx.driver.execute("INSERT INTO semon_pg_native_signals(namespace,file_id,seq,k,o,at) SELECT $1,1,s,convert_to(repeat('x',8192),'UTF8'),s,s FROM generate_series(0,256) s", &[&ns]).map_err(sqlstate::error)?;
            Ok(())
        }).unwrap();
        pool.transaction(&namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let ns = tx.namespace.as_str().as_bytes().to_vec();
            let loaded = rows(
                tx,
                "signals",
                "seq,k",
                "namespace_digest=sha256($1::bytea) AND namespace=$1 AND file_id=$2 ORDER BY seq",
                &[&ns, &1i64],
            )?;
            assert_eq!(loaded.len(), 257);
            for (i, row) in loaded.iter().enumerate() {
                assert_eq!(row.get::<i64>(0).map_err(bad)?, i as i64);
                assert_eq!(row.get::<String>(1).map_err(bad)?, "x".repeat(8192));
            }
            Ok(())
        })
        .unwrap();
    }
}
