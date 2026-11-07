//! Request-owned lazy covering proofs from the selected PostgreSQL snapshot.
use super::{Namespace, PgProjectionTransaction, StorageError, backend};
use crate::{
    SessionSourceRange, SessionSourceReader, SessionSourceRef, model::summary::CatalogSource,
};
use std::{
    collections::BTreeMap,
    io::{self, Read, Seek, SeekFrom},
    sync::{Arc, Mutex},
};

pub(super) struct ProofRequest {
    pub(super) source: SessionSourceRef,
    pub(super) ordinal: u64,
    pub(super) reply: std::sync::mpsc::Sender<io::Result<[u8; 32]>>,
}

pub(super) struct LocalProofReader {
    sources: BTreeMap<(String, String), CatalogSource>,
    sender: tokio::sync::mpsc::UnboundedSender<ProofRequest>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    physical: BTreeMap<std::path::PathBuf, crate::events::Stat>,
    blocks: BTreeMap<(std::path::PathBuf, u64), Arc<[u8]>>,
    bytes: usize,
}

impl LocalProofReader {
    pub(super) fn new(
        options: &crate::Options,
        sources: &[CatalogSource],
        sender: tokio::sync::mpsc::UnboundedSender<ProofRequest>,
    ) -> Result<Self, StorageError> {
        let mut captured = BTreeMap::new();
        for source in sources {
            let input = crate::native_evidence::catalog_input(&source.path, options)
                .ok_or_else(|| backend("proof source outside configured roots"))?;
            if captured
                .insert((input.root.as_str().into(), input.path), source.clone())
                .is_some()
            {
                return Err(backend("ambiguous proof source"));
            }
        }
        Ok(Self {
            sources: captured,
            sender,
            state: Mutex::default(),
        })
    }

    pub(super) fn verify(&self) -> io::Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("selected reader state unavailable"))?;
        for (path, before) in &state.physical {
            let file = crate::sealed::LogFile::open(path)?;
            crate::source_carrier::recheck(path, &file, before)?;
        }
        Ok(())
    }
}

impl SessionSourceReader for LocalProofReader {
    fn read_range(
        &self,
        reference: &SessionSourceRef,
        expected: Option<&str>,
        offset: u64,
        max: usize,
    ) -> io::Result<SessionSourceRange> {
        let invalid =
            || io::Error::new(io::ErrorKind::InvalidData, "selected prefix proof changed");
        let source = self
            .sources
            .get(&(reference.root.clone(), reference.path.clone()))
            .ok_or_else(invalid)?;
        let generation = source
            .prefix_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if reference.native_id != source.native_id
            || reference.offset != source.offset
            || reference.prefix_sha256 != source.prefix_sha256
            || reference.tail_sha256 != source.tail_sha256
            || expected.is_some_and(|expected| expected != generation)
            || max == 0
            || max > crate::SESSION_SOURCE_CHUNK_MAX
            || offset
                .checked_add(max as u64)
                .is_none_or(|end| end > source.offset)
        {
            return Err(invalid());
        }
        let mut state = self.state.lock().map_err(|_| invalid())?;
        let mut file = crate::sealed::LogFile::open(&source.path)?;
        let before = crate::events::Stat::from_metadata(&file.metadata()?)?;
        if before.size < source.offset
            || state
                .physical
                .get(&source.path)
                .is_some_and(|prior| prior != &before)
        {
            return Err(invalid());
        }
        crate::source_carrier::recheck(&source.path, &file, &before)?;
        let size = crate::source_chunks::SIZE as u64;
        let end = offset + max as u64;
        let mut result = Vec::with_capacity(max);
        for ordinal in offset / size..=(end - 1) / size {
            let key = (source.path.clone(), ordinal);
            let bytes = if before.changed_ns.is_some() {
                state.blocks.get(&key).cloned()
            } else {
                None
            };
            let bytes = match bytes {
                Some(bytes) => bytes,
                None => {
                    let length = (source.offset - ordinal * size).min(size) as usize;
                    if state.bytes.saturating_add(length) > 1024 * 1024 {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "selected prefix proof byte budget",
                        ));
                    }
                    let (reply, received) = std::sync::mpsc::channel();
                    self.sender
                        .send(ProofRequest {
                            source: reference.clone(),
                            ordinal,
                            reply,
                        })
                        .map_err(|_| invalid())?;
                    let digest = received.recv().map_err(|_| invalid())??;
                    let mut bytes = vec![0; length];
                    file.seek(SeekFrom::Start(ordinal * size))?;
                    file.read_exact(&mut bytes)?;
                    use sha2::{Digest, Sha256};
                    if <[u8; 32]>::from(Sha256::digest(&bytes)) != digest {
                        return Err(invalid());
                    }
                    state.bytes += length;
                    let bytes: Arc<[u8]> = bytes.into();
                    if before.changed_ns.is_some() {
                        state.blocks.insert(key, bytes.clone());
                    }
                    bytes
                }
            };
            let start = ordinal * size;
            result.extend_from_slice(
                &bytes[offset.saturating_sub(start) as usize
                    ..(end - start).min(bytes.len() as u64) as usize],
            );
        }
        crate::source_carrier::recheck(&source.path, &file, &before)?;
        state.physical.insert(source.path.clone(), before);
        Ok(SessionSourceRange {
            generation,
            length: source.offset,
            offset,
            bytes: result.into(),
            cached: true,
        })
    }
}

pub(super) async fn serve(
    tx: &PgProjectionTransaction<'_>,
    namespace: &Namespace,
    request: ProofRequest,
    proofs: &BTreeMap<(String, String), (CatalogSource, String)>,
) {
    let result = async {
        let (source, generation) = proofs
            .get(&(request.source.root.clone(), request.source.path.clone()))
            .ok_or_else(|| backend("unknown selected proof"))?;
        if source.native_id != request.source.native_id
            || source.offset != request.source.offset
            || source.prefix_sha256 != request.source.prefix_sha256
            || source.tail_sha256 != request.source.tail_sha256
        {
            return Err(backend("selected proof source changed"));
        }
        tx.proof_block_generation(
            namespace,
            source
                .path
                .to_str()
                .ok_or_else(|| backend("invalid proof path"))?,
            generation,
            source.offset,
            &source.prefix_sha256,
            &source.tail_sha256,
            request.ordinal,
        )
        .await?
        .ok_or_else(|| backend("selected proof block absent"))
    }
    .await
    .map_err(io::Error::other);
    let _ = request.reply.send(result);
}
