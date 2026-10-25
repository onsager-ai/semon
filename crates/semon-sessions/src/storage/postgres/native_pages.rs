//! Private immutable native file pages. No Catalog authority, Raw grant or GC.
#![allow(dead_code)] // Reader awaits the complete PG IndexStore/selected consumer.
use super::{sqlstate, txn::PgTxn};
use crate::{
    events::{FileIndex, Ledger, native_pages::{self as codec, Plan}},
    storage::{StorageError, proof_lifetime::ProofGenerationId},
};
use sha2::{Digest, Sha256};

const MAX: usize = 1024 * 1024;

pub(super) struct Prepared {
    chunks: Plan,
    launch: Option<Plan>,
}

pub(super) fn prepare(ledger: &Ledger, index: &FileIndex) -> Result<Prepared, StorageError> {
    let chunks = index.source_chunks.as_ref().ok_or(StorageError::ScopeChanged)?;
    let chunks = codec::chunks::plan(ledger, chunks)
        .map_err(|_| StorageError::Unavailable)?.ok_or(StorageError::ScopeChanged)?;
    let launch = index.handoff_index.as_ref().map(|calls| {
        codec::launch::plan(ledger, calls).map_err(|_| StorageError::Unavailable)?
            .ok_or(StorageError::ScopeChanged)
    }).transpose()?;
    for plan in std::iter::once(&chunks).chain(launch.iter()) {
        if plan.header.len() > MAX || plan.pages.iter().chain(plan.tree.iter().flatten()).any(|p| p.len() > MAX) {
            return Err(StorageError::ScopeChanged);
        }
    }
    Ok(Prepared { chunks, launch })
}

fn effect(tx: &PgTxn<'_>) -> Result<(), StorageError> {
    super::native_bundle::page_boundary(tx)
}

fn put(tx: &mut PgTxn<'_>, id: &[u8], kind: i16, key: &str, payload: &str) -> Result<(), StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes();
    let digest: Vec<u8> = Sha256::digest(payload.as_bytes()).to_vec();
    tx.driver.execute("INSERT INTO semon_pg_native_page_blobs(namespace,digest,payload) VALUES($1,$2,$3) ON CONFLICT DO NOTHING", &[&ns,&digest,&payload.as_bytes()]).map_err(sqlstate::error)?;
    effect(tx)?;
    let equal: bool = tx.driver.query_one("SELECT payload=$3 FROM semon_pg_native_page_blobs WHERE namespace=$1 AND digest=$2", &[&ns,&digest,&payload.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if !equal { return Err(StorageError::ScopeChanged); }
    let changed = tx.driver.execute("INSERT INTO semon_pg_native_page_refs(namespace,generation,kind,page_key,digest) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING", &[&ns,&id,&kind,&key.as_bytes(),&digest]).map_err(sqlstate::error)?;
    if changed == 0 {
        let equal: bool = tx.driver.query_one("SELECT digest=$5 FROM semon_pg_native_page_refs WHERE namespace=$1 AND generation=$2 AND kind=$3 AND page_key=$4", &[&ns,&id,&kind,&key.as_bytes(),&digest]).map_err(sqlstate::error)?.get(0);
        if !equal { return Err(StorageError::ScopeChanged); }
    }
    effect(tx)
}

pub(super) fn write(tx: &mut PgTxn<'_>, source: i64, path: &str, ledger: &Ledger, prepared: Prepared) -> Result<ProofGenerationId, StorageError> {
    tx.check()?;
    let admission = u64::try_from(source).map_err(|_| StorageError::ScopeChanged)?;
    if admission == 0 { return Err(StorageError::ScopeChanged); }
    let receipt = crate::events::store::header_ledger(ledger);
    let binding = serde_json::to_string(&(path, &receipt, &prepared.chunks.header, prepared.launch.as_ref().map(|p| &p.header))).map_err(|_| StorageError::Unavailable)?;
    // Actual Usage source generation, exact ledger and native header roots.
    // This ID is not a Catalog publication generation.
    let id = ProofGenerationId::derive(tx.namespace.as_str(), admission, "native-file", &binding);
    let ns = tx.namespace.as_str().as_bytes();
    let launch = prepared.launch.as_ref().map(|p| p.header.as_bytes());
    let changed = tx.driver.execute("INSERT INTO semon_pg_native_page_generations(namespace,generation,source_id,source_path,ledger,chunks_header,launch_header) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING", &[&ns,&id.0.as_bytes(),&source,&path.as_bytes(),&receipt.as_bytes(),&prepared.chunks.header.as_bytes(),&launch]).map_err(sqlstate::error)?;
    if changed == 0 {
        let equal: bool = tx.driver.query_one("SELECT source_id=$3 AND source_path=$4 AND ledger=$5 AND chunks_header=$6 AND launch_header IS NOT DISTINCT FROM $7 FROM semon_pg_native_page_generations WHERE namespace=$1 AND generation=$2", &[&ns,&id.0.as_bytes(),&source,&path.as_bytes(),&receipt.as_bytes(),&prepared.chunks.header.as_bytes(),&launch]).map_err(sqlstate::error)?.get(0);
        if !equal { return Err(StorageError::ScopeChanged); }
    }
    effect(tx)?;
    for (kind, plan) in [(0i16, Some(&prepared.chunks)), (1, prepared.launch.as_ref())] {
        let Some(plan) = plan else { continue; };
        for (at, payload) in plan.pages.iter().enumerate() {
            put(tx, id.0.as_bytes(), kind, &format!("{at:010}"), payload)?;
        }
        for (level, nodes) in plan.tree.iter().enumerate() {
            for (at, node) in nodes.iter().enumerate() {
                put(tx, id.0.as_bytes(), 2, &format!("{level:03}:{at:010}"), node)?;
            }
        }
    }
    let ns = tx.namespace.as_str().as_bytes();
    let path_digest: Vec<u8> = Sha256::digest(path.as_bytes()).to_vec();
    let changed = tx.driver.execute("INSERT INTO semon_pg_native_page_current(namespace,path_digest,source_path,generation) VALUES($1,$2,$3,$4) ON CONFLICT(namespace_digest,path_digest) DO UPDATE SET generation=excluded.generation WHERE semon_pg_native_page_current.source_path=excluded.source_path", &[&ns,&path_digest,&path.as_bytes(),&id.0.as_bytes()]).map_err(sqlstate::error)?;
    if changed != 1 { return Err(StorageError::ScopeChanged); }
    effect(tx)?;
    Ok(id)
}

/// Namespace-bound exact immutable generation lookup. This is metadata only;
/// Catalog lease ownership and selected-read publication are still unported.
pub(super) struct Pinned {
    namespace: String,
    pub(super) id: ProofGenerationId,
    pub(super) chunks_header: String,
    pub(super) launch_header: Option<String>,
}

pub(super) fn open(tx: &mut PgTxn<'_>, path: &str, ledger: &Ledger, id: ProofGenerationId) -> Result<Pinned, StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes();
    let rows = tx.driver.query("SELECT source_path,ledger,CASE WHEN octet_length(chunks_header)<=$3 THEN chunks_header END,CASE WHEN octet_length(launch_header)<=$3 THEN launch_header END,launch_header IS NULL FROM semon_pg_native_page_generations WHERE namespace=$1 AND generation=$2", &[&ns,&id.0.as_bytes(),&(MAX as i32)]).map_err(sqlstate::error)?;
    let row = rows.first().ok_or(StorageError::ScopeChanged)?;
    if row.get::<_, Vec<u8>>(0) != path.as_bytes() || row.get::<_, Vec<u8>>(1) != crate::events::store::header_ledger(ledger).as_bytes() { return Err(StorageError::ScopeChanged); }
    let chunks = row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::ScopeChanged)?;
    let launch = row.get::<_, Option<Vec<u8>>>(3);
    if launch.is_none() && !row.get::<_, bool>(4) { return Err(StorageError::ScopeChanged); }
    tx.check()?;
    crate::selected_read::charge(0, chunks.len() + launch.as_ref().map_or(0, Vec::len)).map_err(|_| StorageError::Busy)?;
    let chunks_header = String::from_utf8(chunks).map_err(|_| StorageError::ScopeChanged)?;
    let header: codec::chunks::Header = serde_json::from_str(&chunks_header).map_err(|_| StorageError::ScopeChanged)?;
    if header.version != 1 || header.parser_version != crate::events::CACHE_VERSION || header.consumed_offset != Some(ledger.offset) || header.ledger != crate::events::store::header_ledger(ledger) { return Err(StorageError::ScopeChanged); }
    Ok(Pinned { namespace: tx.namespace.as_str().to_owned(), id, chunks_header, launch_header: launch.map(String::from_utf8).transpose().map_err(|_| StorageError::ScopeChanged)? })
}

impl Pinned {
    pub(super) fn page(&self, tx: &mut PgTxn<'_>, kind: i16, key: &str) -> Result<String, StorageError> {
        tx.check()?;
        if tx.namespace.as_str() != self.namespace || key.len() > 32 || !(0..=2).contains(&kind) { return Err(StorageError::ScopeChanged); }
        let rows = tx.driver.query("SELECT r.digest,CASE WHEN octet_length(b.payload)<=$5 THEN b.payload END FROM semon_pg_native_page_refs r JOIN semon_pg_native_page_blobs b ON b.namespace=r.namespace AND b.digest=r.digest WHERE r.namespace=$1 AND r.generation=$2 AND r.kind=$3 AND r.page_key=$4", &[&tx.namespace.as_str().as_bytes(),&self.id.0.as_bytes(),&kind,&key.as_bytes(),&(MAX as i32)]).map_err(sqlstate::error)?;
        let row = rows.first().ok_or(StorageError::ScopeChanged)?;
        let payload = row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::ScopeChanged)?;
        tx.check()?;
        crate::selected_read::charge(0, payload.len()).map_err(|_| StorageError::Busy)?;
        if Sha256::digest(&payload).as_slice() != row.get::<_, Vec<u8>>(0) { return Err(StorageError::ScopeChanged); }
        String::from_utf8(payload).map_err(|_| StorageError::ScopeChanged)
    }
}
