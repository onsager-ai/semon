//! Existing PG immutable page generations and blobs in the common transfer.
use super::*;
use crate::storage::transfer::{
    Category, NativePageKind, NativeSource, TransferRecord, native_pages, reported::CaptureState,
};
use sha2::{Digest, Sha256};

fn kind(kind: NativePageKind) -> i16 {
    match kind {
        NativePageKind::Chunks => 0,
        NativePageKind::Launch => 1,
        NativePageKind::Tree => 2,
    }
}

pub(super) fn capture(
    tx: &mut Inbox<'_, '_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    if state.native_family >= 8 {
        return super::rows::capture(tx, state);
    }
    let path = state
        .native_path
        .clone()
        .ok_or(StorageError::ScopeChanged)?;
    if state.native_family == 3 {
        let row=tx.0.driver.query_one("SELECT g.source_id,g.ledger,coalesce(s.checkpoint::text,'0'),coalesce(s.parser_version,0),coalesce(s.usage_ready,false),g.generation,CASE WHEN octet_length(g.chunks_header)<=1048576 THEN g.chunks_header END,CASE WHEN octet_length(g.launch_header)<=1048576 THEN g.launch_header END,g.launch_header IS NULL FROM semon_pg_native_page_generations g LEFT JOIN semon_pg_native_usage_sources s USING(namespace_digest,namespace,source_id) WHERE g.namespace_digest=sha256($1::bytea) AND g.namespace=$1 AND g.source_path=$2 AND (($3::text IS NULL AND EXISTS(SELECT 1 FROM semon_pg_native_page_current c WHERE c.namespace=g.namespace AND c.namespace_digest=g.namespace_digest AND c.source_path=g.source_path AND c.generation=g.generation)) OR ($3::text IS NOT NULL AND encode(sha256(decode('73656d6f6e2d6c65646765722d763100','hex')||g.ledger),'hex')=$3))",&[&ns,&path.as_bytes(),&state.native_backing]).map_err(sqlstate::error)?;
        let launch: Option<Vec<u8>> = row.get(7);
        if launch.is_none() && !row.get::<_, bool>(8) {
            return Err(StorageError::Busy);
        }
        let text =
            |bytes: Vec<u8>| String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged);
        let mut source = NativeSource {
            path: path.clone(),
            backing: state.native_backing.clone(),
            generation: u64::try_from(row.get::<_, i64>(0))
                .map_err(|_| StorageError::ScopeChanged)?,
            receipt: text(row.get(1))?,
            checkpoint: row
                .get::<_, String>(2)
                .parse()
                .map_err(|_| StorageError::ScopeChanged)?,
            parser_version: u32::try_from(row.get::<_, i64>(3))
                .map_err(|_| StorageError::ScopeChanged)?,
            usage_ready: row.get(4),
            chunks_header: text(row.get::<_, Option<Vec<u8>>>(6).ok_or(StorageError::Busy)?)?,
            launch_header: launch.map(text).transpose()?,
        };
        if source.backing.is_some() {
            let header: crate::events::native_pages::chunks::Header =
                serde_json::from_str(&source.chunks_header)
                    .map_err(|_| StorageError::ScopeChanged)?;
            source.checkpoint = header.consumed_offset.ok_or(StorageError::ScopeChanged)?;
            source.parser_version = header.parser_version;
            source.usage_ready = false;
        }
        if source.id(tx.0.namespace.as_str())?.0.as_bytes() != row.get::<_, Vec<u8>>(5) {
            return Err(StorageError::ScopeChanged);
        }
        let record = TransferRecord::NativeSource(source);
        native_pages::validate(&record)?;
        transfer::reported::append_capture(tx, state, Category::PageGenerations, vec![record])?;
        state.native_family = 4;
        state.native_key = None;
    } else if state.native_family <= 6 {
        let page_kind = match state.native_family {
            4 => NativePageKind::Chunks,
            5 => NativePageKind::Launch,
            _ => NativePageKind::Tree,
        };
        let cursor = state.native_key.as_ref().map(|s| s.as_bytes());
        let suffix = "FROM semon_pg_native_page_generations c JOIN semon_pg_native_page_refs r USING(namespace_digest,namespace,generation) JOIN semon_pg_native_page_blobs b USING(namespace_digest,namespace,digest) WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.source_path=$2 AND (($5::text IS NULL AND EXISTS(SELECT 1 FROM semon_pg_native_page_current p WHERE p.namespace=c.namespace AND p.namespace_digest=c.namespace_digest AND p.source_path=c.source_path AND p.generation=c.generation)) OR ($5::text IS NOT NULL AND encode(sha256(decode('73656d6f6e2d6c65646765722d763100','hex')||c.ledger),'hex')=$5)) AND r.kind=$3 AND ($4::bytea IS NULL OR r.page_key>$4) ORDER BY r.page_key";
        let sizes =
            tx.0.driver
                .query(
                    &format!("SELECT octet_length(b.payload) {suffix} LIMIT 64"),
                    &[
                        &ns,
                        &path.as_bytes(),
                        &kind(page_kind),
                        &cursor,
                        &state.native_backing,
                    ],
                )
                .map_err(sqlstate::error)?;
        let mut count = 0;
        let mut bytes = 0usize;
        for row in &sizes {
            let size =
                usize::try_from(row.get::<_, i32>(0)).map_err(|_| StorageError::ScopeChanged)?;
            if bytes
                .checked_add(size)
                .is_none_or(|n| n > transfer::MAX_PAGE_BYTES)
            {
                break;
            }
            bytes += size;
            count += 1;
        }
        if count == 0 && !sizes.is_empty() {
            return Err(StorageError::Busy);
        }
        let rows =
            tx.0.driver
                .query(
                    &format!("SELECT r.page_key,b.payload {suffix} LIMIT {count}"),
                    &[
                        &ns,
                        &path.as_bytes(),
                        &kind(page_kind),
                        &cursor,
                        &state.native_backing,
                    ],
                )
                .map_err(sqlstate::error)?;
        if rows.is_empty() {
            state.native_family += 1;
            state.native_key = None;
        } else {
            let records = rows
                .into_iter()
                .map(|r| {
                    Ok(TransferRecord::NativePage {
                        path: path.clone(),
                        backing: state.native_backing.clone(),
                        kind: page_kind,
                        key: String::from_utf8(r.get(0)).map_err(|_| StorageError::ScopeChanged)?,
                        payload: String::from_utf8(r.get::<_, Vec<u8>>(1))
                            .map_err(|_| StorageError::ScopeChanged)?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            native_pages::append(tx, state, Category::PageBlobs, records)?;
        }
    } else {
        let next = tx.0.driver.query("SELECT encode(sha256(decode('73656d6f6e2d6c65646765722d763100','hex')||g.ledger),'hex') AS digest FROM semon_pg_native_page_generations g WHERE g.namespace_digest=sha256($1::bytea) AND g.namespace=$1 AND g.source_path=$2 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_page_current p WHERE p.namespace_digest=g.namespace_digest AND p.namespace=g.namespace AND p.source_path=g.source_path AND p.generation=g.generation) AND ($3::text IS NULL OR encode(sha256(decode('73656d6f6e2d6c65646765722d763100','hex')||g.ledger),'hex')>$3) ORDER BY digest LIMIT 1", &[&ns,&path.as_bytes(),&state.native_backing]).map_err(sqlstate::error)?.into_iter().next().map(|r|r.get::<_,String>(0));
        if let Some(next) = next {
            state.native_backing = Some(next);
            state.native_family = 3;
            state.native_key = None;
            transfer::reported::save_capture(tx, state)?;
            return Ok(state.progress());
        }
        let mut records = Vec::new();
        for prefix in [
            "native_claude_sidecar:",
            "native_codex_header:",
            "native_parent_marker:",
        ] {
            let key = format!("{prefix}{path}");
            if state.native_key.as_ref().is_some_and(|old| key <= *old) {
                continue;
            }
            if let Some(value) = tx.get(&key, transfer::MAX_PAGE_BYTES)? {
                records.push(TransferRecord::NativeMetadata {
                    path: path.clone(),
                    key,
                    value: String::from_utf8(value).map_err(|_| StorageError::ScopeChanged)?,
                });
            }
        }
        if records.is_empty() {
            state.native_family = 8;
            state.native_key = None;
            state.native_backing = None;
        } else {
            native_pages::append(tx, state, Category::NativeHeaders, records)?;
        }
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply(tx: &mut Inbox<'_, '_>, record: &TransferRecord) -> Result<(), StorageError> {
    native_pages::validate(record)?;
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    match record {
        TransferRecord::NativeSource(source) => {
            if source.backing.is_none() {
                let (_, ledger) = super::super::native_reader::ledger(tx.0, &source.path)?
                    .ok_or(StorageError::ScopeChanged)?;
                native_pages::validate_source_ledger(source, &ledger)?;
                tx.0.driver.execute("INSERT INTO semon_pg_native_usage_sources(namespace,source_id,source_path,receipt,checkpoint,parser_version,usage_ready) VALUES($1,$2,$3,$4,$5::text::numeric,$6,$7)",&[&ns,&(source.generation as i64),&source.path.as_bytes(),&source.receipt.as_bytes(),&source.checkpoint.to_string(),&i64::from(source.parser_version),&source.usage_ready]).map_err(sqlstate::error)?;
                tx.0.driver.execute("INSERT INTO semon_pg_native_usage_current(namespace,source_path,source_id) VALUES($1,$2,$3)",&[&ns,&source.path.as_bytes(),&(source.generation as i64)]).map_err(sqlstate::error)?;
            }
            let id = source.id(tx.0.namespace.as_str())?;
            let launch = source.launch_header.as_ref().map(|s| s.as_bytes());
            tx.0.driver.execute("INSERT INTO semon_pg_native_page_generations(namespace,generation,source_id,source_path,ledger,chunks_header,launch_header) VALUES($1,$2,$3,$4,$5,$6,$7)",&[&ns,&id.0.as_bytes(),&(source.generation as i64),&source.path.as_bytes(),&source.receipt.as_bytes(),&source.chunks_header.as_bytes(),&launch]).map_err(sqlstate::error)?;
            if source.backing.is_none() {
                tx.0.driver.execute("INSERT INTO semon_pg_native_page_current(namespace,path_digest,source_path,generation) VALUES($1,sha256($2::bytea),$2,$3)",&[&ns,&source.path.as_bytes(),&id.0.as_bytes()]).map_err(sqlstate::error)?;
            }
        }
        TransferRecord::NativePage {
            path,
            backing,
            kind: page_kind,
            key,
            payload,
        } => {
            let id:Vec<u8>=tx.0.driver.query_one("SELECT g.generation FROM semon_pg_native_page_generations g WHERE g.namespace_digest=sha256($1::bytea) AND g.namespace=$1 AND g.source_path=$2 AND (($3::text IS NULL AND EXISTS(SELECT 1 FROM semon_pg_native_page_current p WHERE p.namespace=g.namespace AND p.namespace_digest=g.namespace_digest AND p.source_path=g.source_path AND p.generation=g.generation)) OR ($3::text IS NOT NULL AND encode(sha256(decode('73656d6f6e2d6c65646765722d763100','hex')||g.ledger),'hex')=$3))",&[&ns,&path.as_bytes(),backing]).map_err(sqlstate::error)?.get(0);
            if *page_kind == NativePageKind::Tree {
                let header: Option<Vec<u8>> = tx.0.driver.query_one("SELECT CASE WHEN octet_length(chunks_header)<=1048576 THEN chunks_header END FROM semon_pg_native_page_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&ns,&id]).map_err(sqlstate::error)?.get(0);
                let header = String::from_utf8(header.ok_or(StorageError::Busy)?)
                    .map_err(|_| StorageError::ScopeChanged)?;
                native_pages::verify_tree_node(&header, key, payload, |page_kind, key| {
                    let bytes: Option<Vec<u8>> = tx.0.driver.query_one("SELECT CASE WHEN octet_length(b.payload)<=1048576 THEN b.payload END FROM semon_pg_native_page_refs r JOIN semon_pg_native_page_blobs b USING(namespace_digest,namespace,digest) WHERE r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2 AND r.kind=$3 AND r.page_key=$4", &[&ns,&id,&kind(page_kind),&key.as_bytes()]).map_err(sqlstate::error)?.get(0);
                    String::from_utf8(bytes.ok_or(StorageError::Busy)?)
                        .map_err(|_| StorageError::ScopeChanged)
                })?;
            }
            let digest = Sha256::digest(payload.as_bytes()).to_vec();
            tx.0.driver.execute("INSERT INTO semon_pg_native_page_blobs(namespace,digest,payload) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",&[&ns,&digest,&payload.as_bytes()]).map_err(sqlstate::error)?;
            let same:bool=tx.0.driver.query_one("SELECT payload=$3 FROM semon_pg_native_page_blobs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND digest=$2",&[&ns,&digest,&payload.as_bytes()]).map_err(sqlstate::error)?.get(0);
            if !same {
                return Err(StorageError::ScopeChanged);
            }
            tx.0.driver.execute("INSERT INTO semon_pg_native_page_refs(namespace,generation,kind,page_key,digest) VALUES($1,$2,$3,$4,$5)",&[&ns,&id,&kind(*page_kind),&key.as_bytes(),&digest]).map_err(sqlstate::error)?;
        }
        TransferRecord::NativeMetadata { key, value, .. } => tx.put(key, value.as_bytes())?,
        _ => return Err(StorageError::InvalidArguments),
    }
    Ok(())
}
