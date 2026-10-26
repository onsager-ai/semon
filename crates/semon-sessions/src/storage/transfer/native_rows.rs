//! Fixed native child contracts; no table names or SQL in the portable record.
use super::*;

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub enum NativeRowKind { Signal, MessageUsage, CopiedUsage, CodexUsage, UsageFact, UsageIdentity, UsageDimension }

pub(crate) fn validate(record:&TransferRecord)->Result<(),StorageError> {
    use crate::events::native_decode::{self,Values};
    let TransferRecord::NativeRow{path,kind,values}=record else{return Err(StorageError::InvalidArguments);};
    if path.is_empty(){return Err(StorageError::ScopeChanged);}
    let bad=||StorageError::ScopeChanged;
    let row=Values(values.clone());
    match kind {
        NativeRowKind::Signal=>{if values.len()!=7{return Err(bad());}native_decode::signal(&row).map_err(|_|bad())?;}
        NativeRowKind::MessageUsage=>{if values.len()!=23{return Err(bad());}native_decode::usage(&row).map_err(|_|bad())?;}
        NativeRowKind::CodexUsage=>{if values.len()!=7{return Err(bad());}native_decode::codex_usage(&row).map_err(|_|bad())?;}
        NativeRowKind::CopiedUsage=>{let _: (String,String)=serde_json::from_value(serde_json::Value::Array(values.clone())).map_err(|_|bad())?;}
        NativeRowKind::UsageFact=>{
            let (offset,_,_,_,outcome,model):(i64,String,String,String,String,String)=serde_json::from_value(serde_json::Value::Array(values.clone())).map_err(|_|bad())?;
            if offset<0 || !["accepted","duplicate","conflict"].contains(&outcome.as_str()) || !["unavailable","accepted","duplicate","conflict"].contains(&model.as_str()){return Err(bad());}
        }
        NativeRowKind::UsageIdentity=>{
            let (scope,_,offset):(String,String,i64)=serde_json::from_value(serde_json::Value::Array(values.clone())).map_err(|_|bad())?;
            if offset<0 || !["tree","model"].contains(&scope.as_str()){return Err(bad());}
        }
        NativeRowKind::UsageDimension=>{
            let (kind,_,value):(String,String,String)=serde_json::from_value(serde_json::Value::Array(values.clone())).map_err(|_|bad())?;
            if !["model","cost_model","cost_day"].contains(&kind.as_str()){return Err(bad());}
            let _:serde_json::Value=serde_json::from_str(&value).map_err(|_|bad())?;
        }
    }
    Ok(())
}

pub(crate) fn append(tx:&mut impl InboxTransaction,state:&mut reported::CaptureState,records:Vec<TransferRecord>,keys:&[usize])->Result<(),StorageError> {
    let mut fitted=Vec::new();let mut bytes=4096usize;
    for record in records {
        validate(&record)?;let size=encode_bounded(&record,MAX_PAGE_BYTES)?.len()+1;
        if bytes.checked_add(size).is_none_or(|n|n>MAX_PAGE_BYTES){break;}
        bytes+=size;fitted.push(record);
    }
    let Some(TransferRecord::NativeRow{kind,values,..})=fitted.last() else{return Err(StorageError::Busy);};
    let cursor=keys.iter().map(|i|values.get(*i).cloned().ok_or(StorageError::ScopeChanged)).collect::<Result<Vec<_>,_>>()?;
    state.native_key=Some(serde_json::to_string(&cursor).map_err(|_|StorageError::ScopeChanged)?);
    let category=if *kind==NativeRowKind::Signal{Category::Events}else{Category::Usage};
    reported::append_capture(tx,state,category,fitted)
}
