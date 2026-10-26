//! A request-owned host reader qualified by leased immutable PG native proofs.
//! The host still authorizes bytes; Catalog metadata never opens a local file.
use super::*;
use crate::{SessionSourceReader, SessionSourceRef, SessionSourceRange};
use crate::storage::postgres::{PgPool, PgCancellation, txn::TxnKind, native_pages};
use crate::storage::{LogicalSourceNamespace, proof_lifetime::ProofGenerationId};
use std::{io, sync::Arc};

struct Reader<'a> {
    pool: &'a PgPool,
    namespace: &'a LogicalSourceNamespace,
    control: &'a PgCancellation,
    generation: &'a str,
    source: &'a CatalogSource,
    reference: &'a SessionSourceRef,
    host: &'a dyn SessionSourceReader,
}

pub(in crate::storage::postgres) fn with_reader<T>(
    pool: &PgPool,
    namespace: &LogicalSourceNamespace,
    control: &PgCancellation,
    generation: &str,
    source: &CatalogSource,
    reference: &SessionSourceRef,
    host: &dyn SessionSourceReader,
    read: impl FnOnce(&dyn SessionSourceReader) -> io::Result<T>,
) -> Result<T, StorageError> {
    if reference.native_id != source.native_id || reference.offset != source.offset || reference.prefix_sha256 != source.prefix_sha256 || reference.tail_sha256 != source.tail_sha256 { return Err(StorageError::ScopeChanged); }
    snapshot::with_admitted_generation(pool, namespace, control, generation, |_| {
        read(&Reader { pool, namespace, control, generation, source, reference, host }).map_err(|_| StorageError::ScopeChanged)
    })
}

impl SessionSourceReader for Reader<'_> {
    fn read_range(&self, reference: &SessionSourceRef, expected: Option<&str>, offset: u64, max: usize) -> io::Result<SessionSourceRange> {
        if reference != self.reference || max == 0 || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX || offset >= self.source.offset { return Err(io::Error::other(StorageError::InvalidArguments)); }
        let size = crate::source_chunks::SIZE;
        let index = usize::try_from(offset / size as u64).map_err(io::Error::other)?;
        let start = index as u64 * size as u64;
        let length = (self.source.offset - start).min(size as u64) as usize;
        let hash = self.pool.transaction(self.namespace, TxnKind::ReadSnapshot, self.control, |tx| {
            let ns = self.namespace.as_str().as_bytes();
            let rows = tx.driver.query("SELECT p.source_path=$3 AND s.source_path=$3 AND s.metadata=$4,CASE WHEN octet_length(p.page_generation)=64 THEN p.page_generation END FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_catalog_sources s USING(namespace_digest,namespace,generation,path_digest) WHERE p.namespace_digest=sha256($1::bytea) AND p.namespace=$1 AND p.generation=$2 AND p.path_digest=sha256($3::bytea) AND EXISTS(SELECT 1 FROM semon_pg_native_catalog_leases l WHERE l.namespace_digest=p.namespace_digest AND l.namespace=p.namespace AND l.generation=p.generation AND l.expires_at>(extract(epoch FROM clock_timestamp())*1000)::bigint) LIMIT 65", &[&ns, &self.generation.as_bytes(), &path(self.source)?.as_bytes(), &encoded(self.source)?]).map_err(sqlstate::error)?;
            if rows.is_empty() || rows.len() > 64 { return Err(StorageError::ScopeChanged); }
            for row in &rows {
                if !row.get::<_, bool>(0) { return Err(StorageError::ScopeChanged); }
            }
            let id = text(rows[0].get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::ScopeChanged)?)?;
            let source = self.source;
            let ledger = crate::events::Ledger { stat: crate::events::Stat { dev: source.dev, ino: source.ino, size: source.size, modified_ns: source.modified_ns, changed_ns: source.changed_ns }, offset: source.offset, prefix: source.prefix_sha256, tail: source.tail_sha256 };
            native_pages::open(tx, path(source)?, &ledger, ProofGenerationId(id))?.chunk_hash(tx, index)
        }).map_err(io::Error::other)?;
        // The authorized host supplies bytes. Its independent generation pin
        // and the persisted native hash must both match before parsing.
        let part = self.host.read_range(reference, expected, start, length)?;
        if part.offset != start || part.bytes.len() != length || part.length < self.source.offset || expected.is_some_and(|g| g != part.generation) || <[u8; 32]>::from(Sha256::digest(&part.bytes)) != hash { return Err(io::Error::other(StorageError::ScopeChanged)); }
        let begin = (offset - start) as usize;
        let end = length.min(begin.saturating_add(max));
        Ok(SessionSourceRange { generation: part.generation, length: self.source.offset, offset, bytes: Arc::from(&part.bytes[begin..end]), cached: part.cached })
    }
}

fn text(value: Vec<u8>) -> Result<String, StorageError> { String::from_utf8(value).map_err(|_| StorageError::ScopeChanged) }
