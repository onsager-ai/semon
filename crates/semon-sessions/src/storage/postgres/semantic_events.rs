//! Stage-scoped physical native event lookup. Projection and qualification
//! recipes are shared with the ordinary SQLite producer.
use super::{native_reader, txn::PgTxn};
use crate::{events::{FileIndex, Ledger}, model::{relation_stage::{Context, NativeEvent, Location}, summary::CatalogSource}, storage::StorageError};
use crate::events::store::semantic_stage::{self as shared, ProjectedEvent};
use std::{cell::RefCell, collections::BTreeMap};

pub(super) fn project(tx:&mut PgTxn<'_>, context:&Context)->Result<Vec<ProjectedEvent>,StorageError> {
    let mut sources=BTreeMap::<String,(String,CatalogSource,FileIndex)>::new();
    for endpoint in context.endpoints.values().filter(|e|e.row.harness=="claude") {
        for source in &endpoint.row.sources {
            let path=source.path.to_str().ok_or(StorageError::ScopeChanged)?.to_owned();
            if let Some((owner,prior,_))=sources.get(&path) {
                if owner!=&endpoint.row.key || prior!=source { return Err(StorageError::ScopeChanged); }
                continue;
            }
            if sources.len()==64 { return Err(StorageError::Busy); }
            let (ledger,index)=native_reader::file(tx,&path)?.ok_or(StorageError::ScopeChanged)?;
            if !matches_source(&ledger,source) { return Err(StorageError::Conflict); }
            sources.insert(path,(endpoint.row.key.clone(),source.clone(),index));
        }
    }
    let mut events=Vec::new(); let mut bytes=0usize;
    for (path,(_,source,index)) in &sources {
        let mut occurrences=BTreeMap::new();
        for (seq,event) in index.events.iter().enumerate() {
            let kind=serde_json::to_string(&event.k).map_err(|_|StorageError::ScopeChanged)?;
            let occurrence=occurrences.entry((event.o,event.b,kind)).or_insert(0usize);
            let at=*occurrence; *occurrence+=1;
            if !shared::projects_event(event) { continue; }
            let row=shared::project_native_event(NativeEvent { owner:path.clone(),at:Location { source:source.clone(),offset:event.o,block:event.b,kind:event.k,occurrence:at },event:event.clone() },seq).map_err(|_|StorageError::ScopeChanged)?;
            bytes=bytes.saturating_add(row.encoded.len()); if bytes>1048576 { return Err(StorageError::Busy); }
            events.push((row,false,false));
        }
    }
    struct Qualification<'a> { sources:&'a BTreeMap<String,(String,CatalogSource,FileIndex)>, events:RefCell<Vec<(ProjectedEvent,bool,bool)>> }
    impl shared::QualificationPort for Qualification<'_> {
        fn pending(&self)->rusqlite::Result<Vec<(String,String,Option<String>)>> {
            Ok(self.events.borrow().iter().map(|(r,_,_)|(r.native.owner.clone(),r.key.clone(),crate::model::native_copy_identity(&r.native.event))).collect())
        }
        fn copied(&self,path:&str,identity:&str)->rusqlite::Result<bool> {
            let (owner,target,_)=self.sources.get(path).ok_or(rusqlite::Error::InvalidQuery)?;
            if !target.native_role.as_ref().is_some_and(|r|r.get("Top").is_some()) { return Ok(false); }
            Ok(self.sources.iter().any(|(other,(prior_owner,_,index))| other!=path && owner==prior_owner && index.links.continued_in.contains(&target.native_id) && index.events.iter().any(|event|crate::model::native_copy_identity(event).as_deref()==Some(identity))))
        }
        fn owner(&self,path:&str)->rusqlite::Result<String> { self.sources.get(path).map(|s|s.0.clone()).ok_or(rusqlite::Error::InvalidQuery) }
        fn qualify(&self,path:&str,key:&str,_:&str,copied:bool)->rusqlite::Result<()> {
            let mut events=self.events.borrow_mut();
            let row=events.iter_mut().find(|(r,_,_)|r.native.owner==path && r.key==key).ok_or(rusqlite::Error::InvalidQuery)?;
            row.1=copied;row.2=true;Ok(())
        }
        fn done(&self)->rusqlite::Result<bool> { Ok(self.events.borrow().iter().all(|r|r.2)) }
    }
    let qualified=Qualification {sources:&sources,events:RefCell::new(events)};
    if !shared::qualify_with(&qualified).map_err(|_|StorageError::ScopeChanged)? { return Err(StorageError::ScopeChanged); }
    let mut output=qualified.events.into_inner().into_iter().filter(|(_,copied,_)|!*copied).map(|(r,_,_)|r).collect::<Vec<_>>();
    output.sort_by(|a,b|(&a.native.owner,a.seq).cmp(&(&b.native.owner,b.seq)));
    tx.check()?;
    Ok(output)
}
fn matches_source(ledger:&Ledger,source:&CatalogSource)->bool {
    ledger.stat.dev==source.dev && ledger.stat.ino==source.ino && ledger.stat.size==source.size && ledger.stat.mtime_ns==source.modified_ns && ledger.stat.ctime_ns==source.changed_ns && ledger.offset==source.offset && ledger.prefix==source.prefix_sha256 && ledger.tail==source.tail_sha256
}
