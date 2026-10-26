//! A request-owned host reader qualified by leased immutable PG native proofs.
//! The host still authorizes bytes; Catalog metadata never opens a local file.
use super::*;
use crate::storage::postgres::{PgCancellation, PgPool, native_pages, txn::TxnKind};
use crate::storage::{LogicalSourceNamespace, proof_lifetime::ProofGenerationId};
use crate::{SessionSourceRange, SessionSourceReader, SessionSourceRef};
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
    if reference.native_id != source.native_id
        || reference.offset != source.offset
        || reference.prefix_sha256 != source.prefix_sha256
        || reference.tail_sha256 != source.tail_sha256
    {
        return Err(StorageError::ScopeChanged);
    }
    snapshot::with_admitted_generation(pool, namespace, control, generation, |_| {
        read(&Reader {
            pool,
            namespace,
            control,
            generation,
            source,
            reference,
            host,
        })
        .map_err(|_| StorageError::ScopeChanged)
    })
}

impl SessionSourceReader for Reader<'_> {
    fn read_range(
        &self,
        reference: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<SessionSourceRange> {
        if reference != self.reference
            || max == 0
            || max > crate::source_reader::SESSION_SOURCE_CHUNK_MAX
            || offset >= self.source.offset
        {
            return Err(io::Error::other(StorageError::InvalidArguments));
        }
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
        if part.offset != start
            || part.bytes.len() != length
            || part.length < self.source.offset
            || expected.is_some_and(|g| g != part.generation)
            || <[u8; 32]>::from(Sha256::digest(&part.bytes)) != hash
        {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        let begin = (offset - start) as usize;
        let end = length.min(begin.saturating_add(max));
        Ok(SessionSourceRange {
            generation: part.generation,
            length: self.source.offset,
            offset,
            bytes: Arc::from(&part.bytes[begin..end]),
            cached: part.cached,
        })
    }
}

fn text(value: Vec<u8>) -> Result<String, StorageError> {
    String::from_utf8(value).map_err(|_| StorageError::ScopeChanged)
}

/// Multi-source request adapter. Neither a recipe nor a path grants host access.
pub(in crate::storage::postgres) struct SelectedRecords<'a> {
    pub pool: &'a PgPool,
    pub namespace: &'a LogicalSourceNamespace,
    pub control: &'a PgCancellation,
    pub generation: &'a str,
    pub host: &'a dyn SessionSourceReader,
    pub sources: Vec<(&'a CatalogSource, SessionSourceRef)>,
    pub pins: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    pub bytes_left: std::sync::Mutex<usize>,
}
impl SelectedRecords<'_> {
    fn reference(&self, path: &std::path::Path) -> io::Result<&SessionSourceRef> {
        self.sources.iter().find(|(source, _)| source.path == path).map(|(_, reference)| reference).ok_or_else(|| io::Error::other(StorageError::ScopeChanged))
    }
    pub fn read(&self, path: &std::path::Path, offset: u64) -> io::Result<(Option<serde_json::Value>,u64)> {
        let reference = self.reference(path)?;
        let part = self.read_range(reference, None, offset, 1)?;
        crate::source_reader::read_native_record(self, reference, &part.generation, offset)
    }
    pub fn prompt(&self, file: &crate::model::SlotFile, offset: u64) -> Option<(serde_json::Value,u64)> {
        let reference = self.reference(&file.path).ok()?;
        file.prompts.record_from(&mut SelectedFile { reader: self, reference, position: offset }, offset)
    }
    pub fn finish(&self) -> io::Result<()> {
        for (_, reference) in &self.sources {
            if self.pins.lock().map_err(|_|io::Error::other(StorageError::Unavailable))?.contains_key(&format!("{}:{}",reference.root,reference.path)) && reference.offset > 0 {
                self.read_range(reference, None, reference.offset - 1, 1)?;
            }
        }
        Ok(())
    }
}
impl SessionSourceReader for SelectedRecords<'_> {
    fn read_range(&self, reference: &SessionSourceRef, expected: Option<&str>, offset: u64, max: usize) -> io::Result<SessionSourceRange> {
        let (source, saved) = self.sources.iter().find(|(_, saved)| saved == reference).ok_or_else(||io::Error::other(StorageError::ScopeChanged))?;
        let key = format!("{}:{}", reference.root, reference.path);
        let mut pins = self.pins.lock().map_err(|_|io::Error::other(StorageError::Unavailable))?;
        let pin = pins.get(&key).map(String::as_str);
        if expected.zip(pin).is_some_and(|(a,b)|a!=b) { return Err(io::Error::other(StorageError::ScopeChanged)); }
        let reader = Reader { pool:self.pool, namespace:self.namespace, control:self.control, generation:self.generation, source, reference:saved, host:self.host };
        let part = reader.read_range(reference, pin.or(expected), offset, max)?;
        let mut left = self.bytes_left.lock().map_err(|_|io::Error::other(StorageError::Unavailable))?;
        *left = left.checked_sub(part.bytes.len()).ok_or_else(||io::Error::other(StorageError::Busy))?;
        pins.entry(key).or_insert_with(||part.generation.clone());
        Ok(part)
    }
}
struct SelectedFile<'a, 'b> {
    reader: &'a SelectedRecords<'b>, reference: &'a SessionSourceRef, position: u64,
}
impl std::io::Read for SelectedFile<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let max = buffer.len().min(crate::source_reader::SESSION_SOURCE_CHUNK_MAX).min(self.reference.offset.saturating_sub(self.position) as usize);
        if max == 0 { return Ok(0); }
        let part = self.reader.read_range(self.reference, None, self.position, max)?;
        buffer[..part.bytes.len()].copy_from_slice(&part.bytes);
        self.position += part.bytes.len() as u64;
        Ok(part.bytes.len())
    }
}
impl std::io::Seek for SelectedFile<'_, '_> {
    fn seek(&mut self, from: std::io::SeekFrom) -> io::Result<u64> {
        let position = match from {
            std::io::SeekFrom::Start(at) => Some(at),
            std::io::SeekFrom::Current(at) => self.position.checked_add_signed(at),
            std::io::SeekFrom::End(at) => self.reference.offset.checked_add_signed(at),
        }.filter(|at| *at <= self.reference.offset).ok_or_else(||io::Error::other(StorageError::InvalidArguments))?;
        self.position = position;
        Ok(position)
    }
}

impl crate::query::transcript::QueryRecords for SelectedRecords<'_> {
    fn reader(&self)->&dyn SessionSourceReader { self }
    fn source_ref(&self,path:&std::path::Path)->Option<&SessionSourceRef> { self.reference(path).ok() }
    fn record_bytes(&self,path:&std::path::Path,offset:u64,max:u64)->io::Result<Option<Vec<u8>>> {
        let reference=self.reference(path)?;
        if offset>=reference.offset {return Ok(None);}
        let bound=usize::try_from(max).map_err(io::Error::other)?.min(crate::tx::LINE_MAX as usize);
        crate::source_reader::read_record_bytes_bounded(self,reference,offset,bound,&mut (bound.saturating_add(1))).map(Some)
    }
    fn prompt(&self,file:&crate::model::SlotFile,offset:u64)->Option<(serde_json::Value,u64)> { self.prompt(file,offset) }
}
