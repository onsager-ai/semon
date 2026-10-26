//! Existing PG immutable page generations and blobs in the common transfer.
use super::*;
use crate::storage::transfer::{Category,NativePageKind,NativeSource,TransferRecord,native_pages,reported::CaptureState};
use sha2::{Digest,Sha256};

fn kind(kind:NativePageKind)->i16 {match kind {NativePageKind::Chunks=>0,NativePageKind::Launch=>1,NativePageKind::Tree=>2}}

pub(super) fn capture(tx:&mut Inbox<'_,'_>,state:&mut CaptureState)->Result<transfer::CaptureProgress,StorageError> {
    let ns=tx.0.namespace.as_str().as_bytes().to_vec();
    if state.native_family>=8 { return super::rows::capture(tx,state); }
    let path=state.native_path.clone().ok_or(StorageError::ScopeChanged)?;
    if state.native_family==3 {
        let row=tx.0.driver.query_one("SELECT s.source_id,s.receipt,s.checkpoint::text,s.parser_version,s.usage_ready,g.generation,CASE WHEN octet_length(g.chunks_header)<=1048576 THEN g.chunks_header END,CASE WHEN octet_length(g.launch_header)<=1048576 THEN g.launch_header END,g.launch_header IS NULL FROM semon_pg_native_usage_current c JOIN semon_pg_native_usage_sources s USING(namespace_digest,namespace,source_id) JOIN semon_pg_native_page_current p ON p.namespace=c.namespace AND p.namespace_digest=c.namespace_digest AND p.source_path=c.source_path JOIN semon_pg_native_page_generations g ON g.namespace=p.namespace AND g.namespace_digest=p.namespace_digest AND g.generation=p.generation AND g.source_id=s.source_id WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.source_path_digest=semon_pg_native_digest($1,$2,'usage.path') AND c.source_path=$2",&[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
        let launch:Option<Vec<u8>>=row.get(7);
        if launch.is_none() && !row.get::<_,bool>(8){return Err(StorageError::Busy);}
        let text=|bytes:Vec<u8>|String::from_utf8(bytes).map_err(|_|StorageError::ScopeChanged);
        let source=NativeSource {path:path.clone(),generation:u64::try_from(row.get::<_,i64>(0)).map_err(|_|StorageError::ScopeChanged)?,receipt:text(row.get(1))?,checkpoint:row.get::<_,String>(2).parse().map_err(|_|StorageError::ScopeChanged)?,parser_version:u32::try_from(row.get::<_,i64>(3)).map_err(|_|StorageError::ScopeChanged)?,usage_ready:row.get(4),chunks_header:text(row.get::<_,Option<Vec<u8>>>(6).ok_or(StorageError::Busy)?)?,launch_header:launch.map(text).transpose()?};
        if source.id(tx.0.namespace.as_str())?.0.as_bytes()!=row.get::<_,Vec<u8>>(5){return Err(StorageError::ScopeChanged);}
        let record=TransferRecord::NativeSource(source);native_pages::validate(&record)?;
        transfer::reported::append_capture(tx,state,Category::PageGenerations,vec![record])?;
        state.native_family=4;state.native_key=None;
    } else if state.native_family<=6 {
        let page_kind=match state.native_family {4=>NativePageKind::Chunks,5=>NativePageKind::Launch,_=>NativePageKind::Tree};
        let cursor=state.native_key.as_ref().map(|s|s.as_bytes());
        let rows=tx.0.driver.query("SELECT r.page_key,CASE WHEN octet_length(b.payload)<=1048576 THEN b.payload END FROM semon_pg_native_page_current c JOIN semon_pg_native_page_refs r USING(namespace_digest,namespace,generation) JOIN semon_pg_native_page_blobs b USING(namespace_digest,namespace,digest) WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.path_digest=sha256($2::bytea) AND c.source_path=$2 AND r.kind=$3 AND ($4::bytea IS NULL OR r.page_key>$4) ORDER BY r.page_key LIMIT 64",&[&ns,&path.as_bytes(),&kind(page_kind),&cursor]).map_err(sqlstate::error)?;
        if rows.is_empty(){state.native_family+=1;state.native_key=None;}
        else {
            let records=rows.into_iter().map(|r|Ok(TransferRecord::NativePage{path:path.clone(),kind:page_kind,key:String::from_utf8(r.get(0)).map_err(|_|StorageError::ScopeChanged)?,payload:String::from_utf8(r.get::<_,Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?).map_err(|_|StorageError::ScopeChanged)?})).collect::<Result<Vec<_>,StorageError>>()?;
            native_pages::append(tx,state,Category::PageBlobs,records)?;
        }
    } else {
        let mut records=Vec::new();
        for prefix in ["native_claude_sidecar:","native_codex_header:","native_parent_marker:"] {
            let key=format!("{prefix}{path}");if state.native_key.as_ref().is_some_and(|old|key<=*old){continue;}
            if let Some(value)=tx.get(&key,transfer::MAX_PAGE_BYTES)? {records.push(TransferRecord::NativeMetadata{path:path.clone(),key,value:String::from_utf8(value).map_err(|_|StorageError::ScopeChanged)?});}
        }
        if records.is_empty(){state.native_family=8;state.native_key=None;}else{native_pages::append(tx,state,Category::NativeHeaders,records)?;}
    }
    transfer::reported::save_capture(tx,state)?;Ok(state.progress())
}

pub(super) fn apply(tx:&mut Inbox<'_,'_>,record:&TransferRecord)->Result<(),StorageError> {
    native_pages::validate(record)?;
    let ns=tx.0.namespace.as_str().as_bytes().to_vec();
    match record {
        TransferRecord::NativeSource(source)=>{
            tx.0.driver.execute("INSERT INTO semon_pg_native_usage_sources(namespace,source_id,source_path,receipt,checkpoint,parser_version,usage_ready) VALUES($1,$2,$3,$4,$5::text::numeric,$6,$7)",&[&ns,&(source.generation as i64),&source.path.as_bytes(),&source.receipt.as_bytes(),&source.checkpoint.to_string(),&i64::from(source.parser_version),&source.usage_ready]).map_err(sqlstate::error)?;
            tx.0.driver.execute("INSERT INTO semon_pg_native_usage_current(namespace,source_path,source_id) VALUES($1,$2,$3)",&[&ns,&source.path.as_bytes(),&(source.generation as i64)]).map_err(sqlstate::error)?;
            let id=source.id(tx.0.namespace.as_str())?;
            let launch=source.launch_header.as_ref().map(|s|s.as_bytes());
            tx.0.driver.execute("INSERT INTO semon_pg_native_page_generations(namespace,generation,source_id,source_path,ledger,chunks_header,launch_header) VALUES($1,$2,$3,$4,$5,$6,$7)",&[&ns,&id.0.as_bytes(),&(source.generation as i64),&source.path.as_bytes(),&source.receipt.as_bytes(),&source.chunks_header.as_bytes(),&launch]).map_err(sqlstate::error)?;
            tx.0.driver.execute("INSERT INTO semon_pg_native_page_current(namespace,path_digest,source_path,generation) VALUES($1,sha256($2::bytea),$2,$3)",&[&ns,&source.path.as_bytes(),&id.0.as_bytes()]).map_err(sqlstate::error)?;
        }
        TransferRecord::NativePage{path,kind:page_kind,key,payload}=>{
            let id:Vec<u8>=tx.0.driver.query_one("SELECT generation FROM semon_pg_native_page_current WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND source_path=$2",&[&ns,&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
            let digest=Sha256::digest(payload.as_bytes()).to_vec();
            tx.0.driver.execute("INSERT INTO semon_pg_native_page_blobs(namespace,digest,payload) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",&[&ns,&digest,&payload.as_bytes()]).map_err(sqlstate::error)?;
            let same:bool=tx.0.driver.query_one("SELECT payload=$3 FROM semon_pg_native_page_blobs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND digest=$2",&[&ns,&digest,&payload.as_bytes()]).map_err(sqlstate::error)?.get(0);
            if !same {return Err(StorageError::ScopeChanged);}
            tx.0.driver.execute("INSERT INTO semon_pg_native_page_refs(namespace,generation,kind,page_key,digest) VALUES($1,$2,$3,$4,$5)",&[&ns,&id,&kind(*page_kind),&key.as_bytes(),&digest]).map_err(sqlstate::error)?;
        }
        TransferRecord::NativeMetadata{key,value,..}=>tx.put(key,value.as_bytes())?,
        _=>return Err(StorageError::InvalidArguments),
    }
    Ok(())
}
