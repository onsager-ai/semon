//! Fixed child mappings; no physical SQL or IDs cross the transfer boundary.
use super::*;
use crate::storage::transfer::{NativeRowKind,TransferRecord,native_rows,reported::CaptureState};
use crate::storage::postgres::native_reader::{SIGNAL,USAGE,CODEX,transfer_rows};
use tokio_postgres::types::ToSql;
fn spec(family:u8)->Result<(NativeRowKind,&'static str,&'static str,&'static [usize],bool),StorageError>{
    use NativeRowKind::*;
    Ok(match family {
        8=>(Signal,"signals",SIGNAL,&[0],false),
        9=>(MessageUsage,"usage",USAGE,&[0],false),
        10=>(CopiedUsage,"usage_identity","message_id,record_uuid",&[0,1],false),
        11=>(CodexUsage,"codex_usage",CODEX,&[0],false),
        12=>(UsageFact,"usage_facts","native_offset:u,identity,payload,record,outcome,model_outcome",&[0],true),
        13=>(UsageIdentity,"usage_identities","scope,identity,native_offset:u",&[0,1],true),
        14=>(UsageDimension,"usage_dimensions","kind,dimension,value",&[0,1],true),
        _=>return Err(StorageError::ScopeChanged),
    })
}
fn parent(tx:&mut Inbox<'_,'_>,path:&str,source:bool)->Result<i64,StorageError>{
    let ns=tx.0.namespace.as_str().as_bytes().to_vec();
    let sql=if source {"SELECT source_id FROM semon_pg_native_usage_current WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path_digest=semon_pg_native_digest($1,$2,'usage.path') AND source_path=$2"}else{"SELECT file_id FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND path=$2"};
    Ok(tx.0.driver.query_one(sql,&[&ns,&path.as_bytes()]).map_err(sqlstate::error)?.get(0))
}
pub(super) fn capture(tx:&mut Inbox<'_,'_>,state:&mut CaptureState)->Result<transfer::CaptureProgress,StorageError>{
    if state.native_family==15 {state.native_family=0;state.native_key=None;transfer::reported::save_capture(tx,state)?;return Ok(state.progress());}
    let (kind,table,columns,keys,source)=spec(state.native_family)?;
    let path=state.native_path.clone().ok_or(StorageError::ScopeChanged)?;
    let id=parent(tx,&path,source)?;
    let ns=tx.0.namespace.as_str().as_bytes().to_vec();
    let names=columns.split(',').collect::<Vec<_>>();
    let mut binds:Vec<Box<dyn ToSql+Sync>>=vec![Box::new(ns),Box::new(id)];
    let mut key_names=Vec::new();let mut slots=Vec::new();
    let after=if let Some(cursor)=&state.native_key {
        let cursor:Vec<serde_json::Value>=serde_json::from_str(cursor).map_err(|_|StorageError::ScopeChanged)?;
        if cursor.len()!=keys.len(){return Err(StorageError::ScopeChanged);}
        for (index,value) in keys.iter().zip(cursor) {
            let column=names[*index];key_names.push(column.split(':').next().ok_or(StorageError::ScopeChanged)?);
            let mut slot=format!("${}",binds.len()+1);
            let bind:Box<dyn ToSql+Sync>=if column.ends_with(":u"){slot.push_str("::text::numeric");Box::new((value.as_i64().ok_or(StorageError::ScopeChanged)? as u64).to_string())}else if column=="seq"{Box::new(value.as_i64().ok_or(StorageError::ScopeChanged)?)}else{Box::new(value.as_str().ok_or(StorageError::ScopeChanged)?.as_bytes().to_vec())};
            binds.push(bind);slots.push(slot);
        }
        format!(" AND ({})>({})",key_names.join(","),slots.join(","))
    } else {String::new()};
    let predicate=format!("namespace_digest=sha256($1::bytea) AND namespace=$1 AND {}=$2{after}",if source{"source_id"}else{"file_id"});
    let refs=binds.iter().map(|v|&**v as &(dyn ToSql+Sync)).collect::<Vec<_>>();
    let rows=transfer_rows(tx.0,table,columns,&predicate,&refs)?;
    if rows.is_empty(){state.native_family+=1;state.native_key=None;}
    else {native_rows::append(tx,state,rows.into_iter().map(|v|TransferRecord::NativeRow{path:path.clone(),kind,values:v.0}).collect(),keys)?;}
    transfer::reported::save_capture(tx,state)?;Ok(state.progress())
}
pub(super) fn apply(tx:&mut Inbox<'_,'_>,record:&TransferRecord)->Result<(),StorageError>{
    native_rows::validate(record)?;
    let TransferRecord::NativeRow{path,kind,values}=record else{return Err(StorageError::InvalidArguments);};
    let (_,table,columns,_,source)=(8..=14).map(spec).collect::<Result<Vec<_>,_>>()?.into_iter().find(|s|s.0==*kind).ok_or(StorageError::InvalidArguments)?;
    let id=parent(tx,path,source)?;let mut row=vec![serde_json::Value::from(id)];row.extend(values.clone());
    super::native::insert(tx,table,&format!("{},{columns}",if source{"source_id"}else{"file_id"}),row)
}
