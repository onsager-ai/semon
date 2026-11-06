//! Native normalized indexes and immutable consumed-prefix block proofs.
use super::{
    Namespace, PgProjectionStore, PgProjectionTransaction, ProjectionTag, StorageError, backend,
    ensure, number,
};
use crate::events::{FileIndex, Ledger};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

fn generation(tag: ProjectionTag) -> String {
    format!("{:x}", Sha256::digest(format!("{tag:?}").as_bytes()))
}

fn parent(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(left);
    hash.update(right);
    hash.finalize().into()
}

fn tree(hashes: &[[u8; 32]]) -> Vec<Vec<[u8; 32]>> {
    let mut levels = vec![hashes.to_vec()];
    while levels.last().is_some_and(|level| level.len() > 1) {
        let next = levels.last().unwrap().chunks(2)
            .map(|pair| parent(&pair[0], pair.get(1).unwrap_or(&pair[0])))
            .collect();
        levels.push(next);
    }
    levels
}

fn root(levels: &[Vec<[u8; 32]>]) -> [u8; 32] {
    levels.last().and_then(|level| level.first()).copied()
        .unwrap_or_else(|| Sha256::digest([]).into())
}

impl PgProjectionStore {
    /// Parse one native source with the shared parser, then atomically commit
    /// its normalized rows, consumed ledger and byte proofs. A concurrent writer
    /// refuses this attempt; the caller must rescan rather than acknowledge it.
    /// This performs synchronous native-file I/O; hosts run it off their request
    /// executor. It never resolves or opens a SQLite session database.
    pub async fn index_source(
        &mut self,
        namespace: &Namespace,
        path: &Path,
        harness: &str,
    ) -> Result<ProjectionTag, StorageError> {
        ensure(&self.namespace, namespace)?;
        if !matches!(harness, "claude" | "codex" | "copilot") {
            return Err(StorageError::Backend("unsupported native harness".into()));
        }
        let name = path
            .to_str()
            .ok_or_else(|| StorageError::Backend("invalid native path".into()))?;
        let tx = self.begin(namespace).await?;
        let expected = tx.ledger(namespace, name).await?;
        let previous = tx.load_index(namespace, name).await?;
        tx.commit(namespace).await?;
        let (ledger, index, append) =
            crate::events::scan_postgres_file(path, harness, previous).map_err(backend)?;
        if crate::events::Stat::of(path).map_err(backend)? != ledger.stat {
            return Err(StorageError::Backend(
                "native source changed while indexing".into(),
            ));
        }
        let tx = self.begin(namespace).await?;
        let tag = tx
            .write_index(namespace, name, expected, &ledger, &index, append)
            .await?
            .ok_or_else(|| {
                StorageError::Backend("native index conflict; rescan required".into())
            })?;
        tx.commit(namespace).await?;
        Ok(tag)
    }
}

impl PgProjectionTransaction<'_> {
    pub(crate) async fn load_index(
        &self,
        namespace: &Namespace,
        path: &str,
    ) -> Result<Option<(Ledger, FileIndex)>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let Some(row) = self
            .tx
            .query_opt(
                "SELECT ledger,metadata FROM semon_projection.files WHERE namespace=$1 AND path=$2",
                &[&namespace.binding(), &path],
            )
            .await
            .map_err(backend)?
        else {
            return Ok(None);
        };
        let ledger: Ledger = serde_json::from_str(row.get(0)).map_err(backend)?;
        let mut index: FileIndex = serde_json::from_str(row.get(1)).map_err(backend)?;
        let tag = self
            .ledger(namespace, path)
            .await?
            .ok_or_else(|| StorageError::Backend("normalized ledger unavailable".into()))?;
        if ProjectionTag::of_ledger(&ledger, tag.epoch, tag.projector) != tag
            || tag.projector != crate::model::summary::CATALOG_VERSION
        {
            return Err(StorageError::Backend("normalized ledger mismatch".into()));
        }
        let rows = self.tx.query(
            "SELECT kind,ordinal,record FROM semon_projection.records WHERE namespace=$1 AND path=$2 ORDER BY kind,ordinal",
            &[&namespace.binding(), &path],
        ).await.map_err(backend)?;
        for row in rows {
            let ordinal: i64 = row.get(1);
            let text: &str = row.get(2);
            let len = match row.get::<_, i16>(0) {
                0 => {
                    let len = index.events.len();
                    index
                        .events
                        .push(serde_json::from_str(text).map_err(backend)?);
                    len
                }
                1 => {
                    let len = index.extras.len();
                    index
                        .extras
                        .push(serde_json::from_str(text).map_err(backend)?);
                    len
                }
                2 => {
                    let len = index.signals.len();
                    index
                        .signals
                        .push(serde_json::from_str(text).map_err(backend)?);
                    len
                }
                _ => {
                    return Err(StorageError::Backend(
                        "invalid normalized record kind".into(),
                    ));
                }
            };
            if ordinal != number(len as u64)? {
                return Err(StorageError::Backend(
                    "normalized record sequence gap".into(),
                ));
            }
        }
        let saved = self.tx.query_one(
            "SELECT consumed,resume,root FROM semon_projection.proofs WHERE namespace=$1 AND path=$2 AND generation=$3",
            &[&namespace.binding(), &path, &generation(tag)],
        ).await.map_err(backend)?;
        let resume: Option<&str> = saved.get(1);
        let hashes = self.tx.query(
            "SELECT ordinal,digest FROM semon_projection.proof_blocks WHERE namespace=$1 AND path=$2 AND generation=$3 ORDER BY ordinal",
            &[&namespace.binding(), &path, &generation(tag)],
        ).await.map_err(backend)?;
        let mut chunks = crate::source_chunks::Chunks {
            offset: saved.get::<_, i64>(0) as u64,
            hashes: Vec::with_capacity(hashes.len()),
            resume: resume
                .map(serde_json::from_str)
                .transpose()
                .map_err(backend)?,
        };
        for row in hashes {
            if row.get::<_, i64>(0) != number(chunks.hashes.len() as u64)? {
                return Err(StorageError::Backend("native proof sequence gap".into()));
            }
            chunks.hashes.push(
                row.get::<_, Vec<u8>>(1)
                    .try_into()
                    .map_err(|_| StorageError::Backend("invalid native proof digest".into()))?,
            );
        }
        if !chunks.valid(ledger.offset) || root(&tree(&chunks.hashes)).as_slice() != saved.get::<_, &[u8]>(2) {
            return Err(StorageError::Backend(
                "native proof boundary mismatch".into(),
            ));
        }
        index.source_chunks = Some(Arc::new(chunks));
        Ok(Some((ledger, index)))
    }

    pub(crate) async fn write_index(
        &self,
        namespace: &Namespace,
        path: &str,
        expected: Option<ProjectionTag>,
        ledger: &Ledger,
        index: &FileIndex,
        append: bool,
    ) -> Result<Option<ProjectionTag>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let chunks = index
            .source_chunks
            .as_ref()
            .filter(|chunks| chunks.valid(ledger.offset))
            .ok_or_else(|| StorageError::Backend("native proof boundary mismatch".into()))?;
        // Serialize before changing any durable rows: errors cannot leave a
        // partially applied logical operation in an otherwise committable tx.
        let encoded_ledger = serde_json::to_string(ledger).map_err(backend)?;
        let metadata = serde_json::to_string(index).map_err(backend)?;
        let resume = chunks
            .resume
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(backend)?;
        let mut records = Vec::new();
        for (kind, events) in [(0_i16, &index.events), (1, &index.extras)] {
            for (ordinal, event) in events.iter().enumerate() {
                records.push((
                    kind,
                    number(ordinal as u64)?,
                    serde_json::to_string(event).map_err(backend)?,
                ));
            }
        }
        for (ordinal, signal) in index.signals.iter().enumerate() {
            records.push((
                2_i16,
                number(ordinal as u64)?,
                serde_json::to_string(signal).map_err(backend)?,
            ));
        }
        let next = ProjectionTag::of_ledger(ledger, 0, crate::model::summary::CATALOG_VERSION);
        let Some(tag) = self
            .write_ledger(namespace, path, expected, next, append)
            .await?
        else {
            return Ok(None);
        };
        self.tx.execute(
            "INSERT INTO semon_projection.files(namespace,path,ledger,metadata) VALUES($1,$2,$3,$4) ON CONFLICT(namespace,path) DO UPDATE SET ledger=excluded.ledger,metadata=excluded.metadata",
            &[&namespace.binding(), &path, &encoded_ledger, &metadata],
        ).await.map_err(backend)?;
        self.tx
            .execute(
                "DELETE FROM semon_projection.records WHERE namespace=$1 AND path=$2",
                &[&namespace.binding(), &path],
            )
            .await
            .map_err(backend)?;
        let mut kinds = Vec::with_capacity(records.len());
        let mut ordinals = Vec::with_capacity(records.len());
        let mut values = Vec::with_capacity(records.len());
        for (kind, ordinal, record) in records {
            kinds.push(kind);
            ordinals.push(ordinal);
            values.push(record);
        }
        self.tx.execute(
            "INSERT INTO semon_projection.records(namespace,path,kind,ordinal,record) SELECT $1,$2,r.* FROM unnest($3::smallint[],$4::bigint[],$5::text[]) r",
            &[&namespace.binding(), &path, &kinds, &ordinals, &values],
        ).await.map_err(backend)?;
        let version = generation(tag);
        let levels = tree(&chunks.hashes);
        let proof_root = root(&levels);
        self.tx.execute(
            "INSERT INTO semon_projection.proofs(namespace,path,generation,consumed,prefix,tail,resume,blocks,root) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING",
            &[&namespace.binding(), &path, &version, &number(tag.offset)?, &&tag.prefix_sha256[..], &&tag.tail_sha256[..], &resume, &number(chunks.hashes.len() as u64)?, &&proof_root[..]],
        ).await.map_err(backend)?;
        let ordinals = (0..chunks.hashes.len())
            .map(|ordinal| number(ordinal as u64))
            .collect::<Result<Vec<_>, _>>()?;
        let digests = chunks
            .hashes
            .iter()
            .map(|digest| digest.to_vec())
            .collect::<Vec<_>>();
        self.tx.execute(
            "INSERT INTO semon_projection.proof_blocks(namespace,path,generation,ordinal,digest) SELECT $1,$2,$3,b.* FROM unnest($4::bigint[],$5::bytea[]) b ON CONFLICT DO NOTHING",
            &[&namespace.binding(), &path, &version, &ordinals, &digests],
        ).await.map_err(backend)?;
        let mut node_levels = Vec::new();
        let mut node_ordinals = Vec::new();
        let mut node_hashes = Vec::new();
        for (level, nodes) in levels.iter().enumerate() {
            for (ordinal, digest) in nodes.iter().enumerate() {
                node_levels.push(level as i32);
                node_ordinals.push(number(ordinal as u64)?);
                node_hashes.push(digest.to_vec());
            }
        }
        self.tx.execute(
            "INSERT INTO semon_projection.proof_nodes(namespace,path,generation,level,ordinal,digest) SELECT $1,$2,$3,n.* FROM unnest($4::integer[],$5::bigint[],$6::bytea[]) n ON CONFLICT DO NOTHING",
            &[&namespace.binding(), &path, &version, &node_levels, &node_ordinals, &node_hashes],
        ).await.map_err(backend)?;
        Ok(Some(tag))
    }

    /// Load one covering block witness for an exact consumed generation. It
    /// remains obtainable after later index commits; Raw authorization and byte
    /// verification are still the selected reader's responsibility.
    pub async fn proof_block(
        &self,
        namespace: &Namespace,
        path: &str,
        tag: ProjectionTag,
        ordinal: u64,
    ) -> Result<Option<[u8; 32]>, StorageError> {
        ensure(&self.namespace, namespace)?;
        if tag.projector != crate::model::summary::CATALOG_VERSION {
            return Err(StorageError::Unsupported(
                "unsupported projector version".into(),
            ));
        }
        let row = self.tx.query_opt(
            "SELECT b.digest,p.blocks,p.root FROM semon_projection.proof_blocks b JOIN semon_projection.proofs p USING(namespace,path,generation) WHERE b.namespace=$1 AND b.path=$2 AND b.generation=$3 AND b.ordinal=$4 AND p.consumed=$5 AND p.prefix=$6 AND p.tail=$7",
            &[&namespace.binding(), &path, &generation(tag), &number(ordinal)?, &number(tag.offset)?, &&tag.prefix_sha256[..], &&tag.tail_sha256[..]],
        ).await.map_err(backend)?;
        let Some(row) = row else { return Ok(None) };
        let digest: [u8; 32] = row.get::<_, Vec<u8>>(0).try_into()
            .map_err(|_| StorageError::Backend("invalid native proof digest".into()))?;
        let mut count = row.get::<_, i64>(1) as u64;
        if count != tag.offset.div_ceil(crate::source_chunks::SIZE as u64) || ordinal >= count {
            return Err(StorageError::Backend("native proof boundary mismatch".into()));
        }
        let mut at = ordinal;
        let mut level = 0_i32;
        let mut witness = digest;
        // Each level reads one sibling by primary key, independent of history
        // and total source length. All nodes come from this selected snapshot.
        while count > 1 {
            let sibling = if (at ^ 1) < count { at ^ 1 } else { at };
            let row = self.tx.query_one(
                "SELECT digest FROM semon_projection.proof_nodes WHERE namespace=$1 AND path=$2 AND generation=$3 AND level=$4 AND ordinal=$5",
                &[&namespace.binding(), &path, &generation(tag), &level, &number(sibling)?],
            ).await.map_err(backend)?;
            let other: [u8; 32] = row.get::<_, Vec<u8>>(0).try_into()
                .map_err(|_| StorageError::Backend("invalid native proof node".into()))?;
            witness = if at % 2 == 0 { parent(&witness, &other) } else { parent(&other, &witness) };
            at /= 2;
            count = count.div_ceil(2);
            level += 1;
        }
        if witness.as_slice() != row.get::<_, &[u8]>(2) {
            return Err(StorageError::Backend("native proof root mismatch".into()));
        }
        Ok(Some(digest))
    }
}
