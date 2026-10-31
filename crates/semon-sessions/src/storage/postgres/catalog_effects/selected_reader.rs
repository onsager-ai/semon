//! A request-owned host reader qualified by leased immutable PG native proofs.
//! The host still authorizes bytes; Catalog metadata never opens a local file.
use super::*;
use crate::storage::postgres::{PgCancellation, PgPool, native_pages};
use crate::storage::{LogicalSourceNamespace, proof_lifetime::ProofGenerationId};
use crate::{SessionSourceRange, SessionSourceReader, SessionSourceRef};
use std::{io, sync::Arc};

#[cfg(test)]
std::thread_local! {
    static PID_READ_DIAGNOSTICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(super) fn with_pid_read_diagnostics<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            PID_READ_DIAGNOSTICS.with(|active| active.set(self.0));
        }
    }

    let prior = PID_READ_DIAGNOSTICS.with(|active| active.replace(true));
    let _reset = Reset(prior);
    f()
}

#[cfg(test)]
pub(super) fn pid_read_diagnostics_enabled() -> bool {
    PID_READ_DIAGNOSTICS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn pid_read_diagnostic(
    stage: &'static str,
    outcome: &'static str,
    offset: Option<u64>,
    max: Option<usize>,
) {
    if pid_read_diagnostics_enabled() {
        eprintln!(
            "pg_vertical140_pid_read stage={stage} outcome={outcome} offset={offset:?} max={max:?}"
        );
    }
}

struct Reader<'a, 'b, 'tx> {
    tx: &'a std::sync::Mutex<&'b mut PgTxn<'tx>>,
    generation: &'a str,
    source: &'a CatalogSource,
    reference: &'a SessionSourceRef,
    host: &'a dyn SessionSourceReader,
}

/// A publication may retain several exact prefixes for one physical path.
/// Select its referenced Native generation by the recipe's complete ledger,
/// rather than the publication's single current source-metadata row.
pub(in crate::storage::postgres) fn native_proof(
    tx: &mut PgTxn<'_>,
    generation: &str,
    source: &CatalogSource,
) -> Result<native_pages::Pinned, StorageError> {
    crate::selected_read::checkpoint("captured_proof").map_err(|_| StorageError::Busy)?;
    tx.check()?;
    let ledger = crate::source_carrier::ledger(source);
    let receipt = crate::events::store::header_ledger(&ledger);
    let ns = tx.namespace.as_str().as_bytes();
    let rows = tx.driver.query("SELECT p.source_path=$3 AND g.source_path=$3,CASE WHEN octet_length(p.page_generation)=64 THEN p.page_generation END FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_page_generations g ON g.namespace_digest=p.namespace_digest AND g.namespace=p.namespace AND g.generation=p.page_generation WHERE p.namespace_digest=sha256($1::bytea) AND p.namespace=$1 AND p.generation=$2 AND p.path_digest=sha256($3::bytea) AND g.ledger=$4 AND EXISTS(SELECT 1 FROM semon_pg_native_catalog_leases l WHERE l.namespace_digest=p.namespace_digest AND l.namespace=p.namespace AND l.generation=p.generation AND l.expires_at>(extract(epoch FROM clock_timestamp())*1000)::bigint) LIMIT 65", &[&ns, &generation.as_bytes(), &path(source)?.as_bytes(), &receipt.as_bytes()]).map_err(sqlstate::error)?;
    if rows.is_empty() || rows.len() > 64 || rows.iter().any(|row| !row.get::<_, bool>(0)) {
        return Err(StorageError::ScopeChanged);
    }
    let id = text(
        rows[0]
            .get::<_, Option<Vec<u8>>>(1)
            .ok_or(StorageError::ScopeChanged)?,
    )?;
    native_pages::open(tx, path(source)?, &ledger, ProofGenerationId(id))
}

// Keep proof custody and independent host authority explicit at this boundary.
#[allow(clippy::too_many_arguments)]
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
    snapshot::with_admitted_snapshot(pool, namespace, control, generation, |snapshot| {
        read(&Reader {
            tx: &snapshot.tx,
            generation,
            source,
            reference,
            host,
        })
        .map_err(|_| StorageError::ScopeChanged)
    })
}

impl SessionSourceReader for Reader<'_, '_, '_> {
    fn read_range(
        &self,
        reference: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<SessionSourceRange> {
        let reference_matches = reference == self.reference;
        let size_ok = max > 0 && max <= crate::source_reader::SESSION_SOURCE_CHUNK_MAX;
        let offset_ok = offset < self.source.offset;
        #[cfg(test)]
        pid_read_diagnostic(
            "reader_input",
            if !reference_matches {
                "reference_mismatch"
            } else if !size_ok {
                "invalid_size"
            } else if !offset_ok {
                "offset_out_of_range"
            } else {
                "accepted"
            },
            Some(offset),
            Some(max),
        );
        if !reference_matches || !size_ok || !offset_ok {
            return Err(io::Error::other(StorageError::InvalidArguments));
        }
        let size = crate::source_chunks::SIZE;
        let index_result = usize::try_from(offset / size as u64);
        #[cfg(test)]
        pid_read_diagnostic(
            "native_chunk_index",
            if index_result.is_ok() {
                "accepted"
            } else {
                "refused"
            },
            Some(offset),
            Some(max),
        );
        let index = index_result.map_err(io::Error::other)?;
        let start = index as u64 * size as u64;
        let length = (self.source.offset - start).min(size as u64) as usize;
        let hash = (|| {
            let borrowed_result = self.tx.lock();
            #[cfg(test)]
            pid_read_diagnostic(
                "native_proof_transaction_lock",
                if borrowed_result.is_ok() {
                    "accepted"
                } else {
                    "refused"
                },
                Some(start),
                Some(length),
            );
            let mut borrowed = borrowed_result.map_err(|_| StorageError::Unavailable)?;
            let tx = &mut **borrowed;
            let proof = native_proof(tx, self.generation, self.source);
            #[cfg(test)]
            pid_read_diagnostic(
                "native_proof_lookup",
                if proof.is_ok() { "accepted" } else { "refused" },
                Some(start),
                Some(length),
            );
            let proof = proof?;
            let chunk_hash = proof.chunk_hash(tx, index);
            #[cfg(test)]
            pid_read_diagnostic(
                "native_chunk_hash_lookup",
                if chunk_hash.is_ok() {
                    "accepted"
                } else {
                    "refused"
                },
                Some(start),
                Some(length),
            );
            chunk_hash
        })()
        .map_err(io::Error::other)?;
        // The authorized host supplies bytes. Its independent generation pin
        // and the persisted native hash must both match before parsing.
        let checkpoint = crate::selected_read::checkpoint("captured_body");
        #[cfg(test)]
        pid_read_diagnostic(
            "host_checkpoint",
            if checkpoint.is_ok() { "accepted" } else { "refused" },
            Some(start),
            Some(length),
        );
        checkpoint?;
        let host_result = self.host.read_range(reference, expected, start, length);
        #[cfg(test)]
        pid_read_diagnostic(
            "host_callback",
            if host_result.is_ok() { "returned" } else { "refused" },
            Some(start),
            Some(length),
        );
        let part = host_result?;
        let check = crate::selected_read::check();
        #[cfg(test)]
        pid_read_diagnostic(
            "host_recheck",
            if check.is_ok() { "accepted" } else { "refused" },
            Some(start),
            Some(length),
        );
        check?;
        let response_outcome = if part.offset != start {
            "offset_mismatch"
        } else if part.bytes.len() != length {
            "byte_count_mismatch"
        } else if part.length < self.source.offset {
            "consumed_prefix_short"
        } else if expected.is_some_and(|g| g != part.generation) {
            "generation_pin_mismatch"
        } else {
            "accepted"
        };
        #[cfg(test)]
        pid_read_diagnostic(
            "host_response",
            response_outcome,
            Some(start),
            Some(length),
        );
        if response_outcome != "accepted" {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        let hash_matches = <[u8; 32]>::from(Sha256::digest(&part.bytes)) == hash;
        #[cfg(test)]
        pid_read_diagnostic(
            "native_chunk_hash_verification",
            if hash_matches { "accepted" } else { "mismatch" },
            Some(start),
            Some(length),
        );
        if !hash_matches {
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
pub(in crate::storage::postgres) struct SelectedRecords<'a, 'b, 'tx> {
    pub tx: &'a std::sync::Mutex<&'b mut PgTxn<'tx>>,
    pub generation: &'a str,
    pub host: &'a dyn SessionSourceReader,
    pub sources: Vec<(&'a CatalogSource, SessionSourceRef)>,
    pub pins: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    pub bytes_left: std::sync::Mutex<usize>,
}
impl SelectedRecords<'_, '_, '_> {
    fn reference(&self, path: &std::path::Path) -> io::Result<&SessionSourceRef> {
        let found = self
            .sources
            .iter()
            .find(|(source, _)| source.path == path)
            .map(|(_, reference)| reference);
        #[cfg(test)]
        pid_read_diagnostic(
            "source_path_lookup",
            if found.is_some() { "matched" } else { "missing" },
            None,
            None,
        );
        found.ok_or_else(|| io::Error::other(StorageError::ScopeChanged))
    }
    pub fn read(
        &self,
        path: &std::path::Path,
        offset: u64,
    ) -> io::Result<(Option<serde_json::Value>, u64)> {
        let result = (|| {
            let reference = self.reference(path)?;
            let part = self.read_range(reference, None, offset, 1)?;
            crate::source_reader::read_native_record(self, reference, &part.generation, offset)
        })();
        #[cfg(test)]
        pid_read_diagnostic(
            "line_read",
            match &result {
                Ok((Some(_), _)) => "record",
                Ok((None, _)) => "missing_record",
                Err(_) => "refused",
            },
            Some(offset),
            None,
        );
        result
    }
    pub fn prompt(
        &self,
        file: &crate::model::SlotFile,
        offset: u64,
    ) -> Option<(serde_json::Value, u64)> {
        let result = self.reference(&file.path).ok().and_then(|reference| {
            file.prompts.record_from(
                &mut SelectedFile {
                    reader: self,
                    reference,
                    position: offset,
                },
                offset,
            )
        });
        #[cfg(test)]
        pid_read_diagnostic(
            "prompt_provider",
            if result.is_some() { "record" } else { "missing_record" },
            Some(offset),
            None,
        );
        result
    }
    pub fn finish(&self) -> io::Result<()> {
        for (_, reference) in &self.sources {
            if self
                .pins
                .lock()
                .map_err(|_| io::Error::other(StorageError::Unavailable))?
                .contains_key(&format!("{}:{}", reference.root, reference.path))
                && reference.offset > 0
            {
                self.read_range(reference, None, reference.offset - 1, 1)?;
            }
        }
        Ok(())
    }
}
impl SessionSourceReader for SelectedRecords<'_, '_, '_> {
    fn read_range(
        &self,
        reference: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<SessionSourceRange> {
        let selected = self
            .sources
            .iter()
            .find(|(_, saved)| saved == reference);
        #[cfg(test)]
        pid_read_diagnostic(
            "selected_reference_lookup",
            if selected.is_some() { "matched" } else { "missing" },
            Some(offset),
            Some(max),
        );
        let (source, saved) = selected.ok_or_else(|| io::Error::other(StorageError::ScopeChanged))?;
        let key = format!("{}:{}", reference.root, reference.path);
        let pins_result = self.pins.lock();
        #[cfg(test)]
        pid_read_diagnostic(
            "request_pin_lock",
            if pins_result.is_ok() {
                "accepted"
            } else {
                "refused"
            },
            Some(offset),
            Some(max),
        );
        let mut pins = pins_result.map_err(|_| io::Error::other(StorageError::Unavailable))?;
        let pin = pins.get(&key).map(String::as_str);
        let pin_matches = !expected.zip(pin).is_some_and(|(a, b)| a != b);
        #[cfg(test)]
        pid_read_diagnostic(
            "request_generation_pin",
            if pin_matches { "matched" } else { "mismatch" },
            Some(offset),
            Some(max),
        );
        if !pin_matches {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        let reader = Reader {
            tx: self.tx,
            generation: self.generation,
            source,
            reference: saved,
            host: self.host,
        };
        let left_result = self.bytes_left.lock();
        #[cfg(test)]
        pid_read_diagnostic(
            "request_byte_budget_lock",
            if left_result.is_ok() {
                "accepted"
            } else {
                "refused"
            },
            Some(offset),
            Some(max),
        );
        let mut left = left_result.map_err(|_| io::Error::other(StorageError::Unavailable))?;
        let size = crate::source_chunks::SIZE as u64;
        let start = offset / size * size;
        let physical = source.offset.saturating_sub(start).min(size) as usize;
        let remaining = left.checked_sub(physical);
        #[cfg(test)]
        pid_read_diagnostic(
            "request_byte_budget",
            if remaining.is_some() { "accepted" } else { "exhausted" },
            Some(offset),
            Some(max),
        );
        *left = remaining.ok_or_else(|| io::Error::other(StorageError::Busy))?;
        let part = reader.read_range(reference, pin.or(expected), offset, max)?;
        let generation_bounds_ok = !part.generation.is_empty() && part.generation.len() <= 4096;
        #[cfg(test)]
        pid_read_diagnostic(
            "returned_generation_bounds",
            if generation_bounds_ok { "accepted" } else { "refused" },
            Some(offset),
            Some(max),
        );
        if !generation_bounds_ok {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        pins.entry(key).or_insert_with(|| part.generation.clone());
        Ok(part)
    }
}
struct SelectedFile<'a, 'b, 'c, 'tx> {
    reader: &'a SelectedRecords<'b, 'c, 'tx>,
    reference: &'a SessionSourceRef,
    position: u64,
}
impl std::io::Read for SelectedFile<'_, '_, '_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let max = buffer
            .len()
            .min(crate::source_reader::SESSION_SOURCE_CHUNK_MAX)
            .min(self.reference.offset.saturating_sub(self.position) as usize);
        if max == 0 {
            return Ok(0);
        }
        let part = self
            .reader
            .read_range(self.reference, None, self.position, max)?;
        buffer[..part.bytes.len()].copy_from_slice(&part.bytes);
        self.position += part.bytes.len() as u64;
        Ok(part.bytes.len())
    }
}
impl std::io::Seek for SelectedFile<'_, '_, '_, '_> {
    fn seek(&mut self, from: std::io::SeekFrom) -> io::Result<u64> {
        let position = match from {
            std::io::SeekFrom::Start(at) => Some(at),
            std::io::SeekFrom::Current(at) => self.position.checked_add_signed(at),
            std::io::SeekFrom::End(at) => self.reference.offset.checked_add_signed(at),
        }
        .filter(|at| *at <= self.reference.offset)
        .ok_or_else(|| io::Error::other(StorageError::InvalidArguments))?;
        self.position = position;
        Ok(position)
    }
}

impl crate::query::transcript::QueryRecords for SelectedRecords<'_, '_, '_> {
    fn reader(&self) -> &dyn SessionSourceReader {
        self
    }
    fn source_ref(&self, path: &std::path::Path) -> Option<&SessionSourceRef> {
        self.reference(path).ok()
    }
    fn record_bytes(
        &self,
        path: &std::path::Path,
        offset: u64,
        max: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        let reference = self.reference(path)?;
        if offset >= reference.offset {
            return Ok(None);
        }
        let bound = usize::try_from(max)
            .map_err(io::Error::other)?
            .min(crate::tx::LINE_MAX as usize);
        crate::source_reader::read_record_bytes_bounded(
            self,
            reference,
            offset,
            bound,
            &mut (bound.saturating_add(1)),
        )
        .map(Some)
    }
    fn prompt(
        &self,
        file: &crate::model::SlotFile,
        offset: u64,
    ) -> Option<(serde_json::Value, u64)> {
        self.prompt(file, offset)
    }
}
