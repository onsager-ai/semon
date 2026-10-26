//! Physical mapping of existing native page metadata, not Raw replay.
use super::*;
use crate::storage::transfer::{Category, NativePageKind, NativeSource, TransferRecord, native_pages, reported::CaptureState};
use sha2::{Digest,Sha256};

fn prefix(path:&str,kind:NativePageKind)->String {
    let hash=format!("{:x}",Sha256::digest(path.as_bytes()));
    match kind {
        NativePageKind::Chunks=>format!("native_source_chunks:{hash}:"),
        NativePageKind::Launch=>format!("native_launch_calls:{hash}:"),
        NativePageKind::Tree=>format!("native_proof_tree:native_source_chunks:{hash}::"),
    }
}
fn metadata(tx:&Inbox<'_>,key:&str)->Result<Option<String>,StorageError> {
    let value:Option<Option<String>>=tx.0.query_row("SELECT CASE WHEN octet_length(value)<=1048576 THEN value END FROM meta WHERE namespace=?1 AND key=?2",params![tx.1,key],|r|r.get(0)).optional().map_err(failed)?;
    value.map(|v|v.ok_or(StorageError::Busy)).transpose()
}

pub(super) fn capture(tx:&mut Inbox<'_>,state:&mut CaptureState)->Result<transfer::CaptureProgress,StorageError> {
    let path=state.native_path.clone().ok_or(StorageError::ScopeChanged)?;
    if state.native_family==3 {
        let (generation,receipt,checkpoint,parser_version,usage_ready):(i64,String,i64,u32,bool)=tx.0.query_row("SELECT s.source_id,s.receipt,s.checkpoint,s.parser_version,s.usage_ready FROM native_usage_current c JOIN native_usage_sources s USING(namespace,source_id) WHERE c.namespace=?1 AND c.source_path=?2",params![tx.1,path],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(failed)?;
        let source=NativeSource { path:path.clone(),generation:u64::try_from(generation).map_err(|_|StorageError::ScopeChanged)?,receipt,checkpoint:checkpoint as u64,parser_version,usage_ready,chunks_header:metadata(tx,&format!("{}header",prefix(&path,NativePageKind::Chunks)))?.ok_or(StorageError::ScopeChanged)?,launch_header:metadata(tx,&format!("{}header",prefix(&path,NativePageKind::Launch)))? };
        let record=TransferRecord::NativeSource(source);native_pages::validate(&record)?;
        transfer::reported::append_capture(tx,state,Category::PageGenerations,vec![record])?;
        state.native_family=4;state.native_key=None;
    } else if state.native_family<=6 {
        let kind=match state.native_family {4=>NativePageKind::Chunks,5=>NativePageKind::Launch,_=>NativePageKind::Tree};
        let prefix=prefix(&path,kind);
        let after=state.native_key.as_ref().map(|k|format!("{prefix}{k}"));
        let rows=tx.0.prepare("SELECT key,CASE WHEN octet_length(value)<=1048576 THEN value END FROM meta WHERE namespace=?1 AND key>=?2 AND key<?3 AND key<>?4 AND (?5 IS NULL OR key>?5) ORDER BY key LIMIT 64").map_err(failed)?.query_map(params![tx.1,prefix,format!("{prefix}~"),format!("{prefix}header"),after],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?))).map_err(failed)?.collect::<rusqlite::Result<Vec<_>>>().map_err(failed)?;
        if rows.is_empty(){state.native_family+=1;state.native_key=None;}
        else {
            let records=rows.into_iter().map(|(key,value)|Ok(TransferRecord::NativePage {path:path.clone(),kind,key:key.strip_prefix(&prefix).ok_or(StorageError::ScopeChanged)?.into(),payload:value.ok_or(StorageError::Busy)?})).collect::<Result<Vec<_>,StorageError>>()?;
            native_pages::append(tx,state,Category::PageBlobs,records)?;
        }
    } else {
        let mut records=Vec::new();
        for prefix in ["native_claude_sidecar:","native_codex_header:","native_parent_marker:"] {
            let key=format!("{prefix}{path}");
            if state.native_key.as_ref().is_some_and(|old|key<=*old) {continue;}
            if let Some(value)=metadata(tx,&key)? {records.push(TransferRecord::NativeMetadata{path:path.clone(),key,value});}
        }
        if records.is_empty(){state.native_family=0;state.native_key=None;}
        else {native_pages::append(tx,state,Category::NativeHeaders,records)?;}
    }
    transfer::reported::save_capture(tx,state)?;Ok(state.progress())
}

pub(super) fn apply(tx:&mut Inbox<'_>,record:&TransferRecord)->Result<(),StorageError> {
    native_pages::validate(record)?;
    match record {
        TransferRecord::NativeSource(source)=>{
            tx.0.execute("INSERT INTO native_usage_sources(namespace,source_id,source_path,receipt,checkpoint,parser_version,usage_ready) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![tx.1,source.generation as i64,source.path,source.receipt,source.checkpoint as i64,source.parser_version,source.usage_ready]).map_err(failed)?;
            tx.0.execute("INSERT INTO native_usage_current(namespace,source_path,source_id) VALUES(?1,?2,?3)",params![tx.1,source.path,source.generation as i64]).map_err(failed)?;
            tx.put(&format!("{}header",prefix(&source.path,NativePageKind::Chunks)),source.chunks_header.as_bytes())?;
            if let Some(header)=&source.launch_header{tx.put(&format!("{}header",prefix(&source.path,NativePageKind::Launch)),header.as_bytes())?;}
        }
        TransferRecord::NativePage{path,kind,key,payload}=>tx.put(&format!("{}{key}",prefix(path,*kind)),payload.as_bytes())?,
        TransferRecord::NativeMetadata{key,value,..}=>tx.put(key,value.as_bytes())?,
        _=>return Err(StorageError::InvalidArguments),
    }
    Ok(())
}
