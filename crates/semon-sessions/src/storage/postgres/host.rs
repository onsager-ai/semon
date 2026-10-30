//! Ordinary host Source mutations use the selected namespace, never cache paths.
use super::{
    model_cache, native_keys, sqlstate,
    txn::{PgTxn, TxnKind},
};
use crate::storage::{StorageError, StoreBackend, StoreConfig};

fn binding(tx: &mut PgTxn<'_>, config: &StoreConfig) -> Result<(), StorageError> {
    if config.expected_stream_epoch.as_deref()
        != Some(model_cache::binding(tx)?.stream_epoch.as_str())
    {
        return Err(StorageError::ScopeChanged);
    }
    Ok(())
}
fn archive(tx: &mut PgTxn<'_>, path: &str) -> Result<(), StorageError> {
    if let Some(bytes) =
        native_keys::read_meta(tx, format!("archive_event_index:{path}").as_bytes(), 16384)?
    {
        let value = std::str::from_utf8(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        crate::events::store::validate_archive_marker(value)?;
    }
    Ok(())
}
pub(crate) fn enqueue_sources(
    config: &StoreConfig,
    options: &crate::Options,
    inputs: &[crate::Input],
) -> Result<(), StorageError> {
    if inputs.len() > 64 {
        return Err(StorageError::Busy);
    }
    let paths = inputs
        .iter()
        .map(|input| {
            if !crate::is_input_path(input.root.as_str(), &input.path)
                || !input.path.ends_with(".jsonl")
            {
                return Err(StorageError::InvalidArguments);
            }
            input
                .full_path(options)
                .to_str()
                .map(str::to_owned)
                .ok_or(StorageError::InvalidArguments)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if paths.iter().map(String::len).sum::<usize>() > 1024 * 1024 {
        return Err(StorageError::Busy);
    }
    let StoreBackend::Postgres { pool, control } = &config.backend else {
        return Err(StorageError::InvalidArguments);
    };
    pool.transaction(&config.namespace, TxnKind::Write, control, |tx| {
        binding(tx, config)?;
        for path in &paths {
            archive(tx, path)?;
            super::source_journal::invalidate(tx, path)?;
        }
        native_keys::put_meta(tx, b"catalog_push_driven", b"1")
    })
}
pub(crate) fn complete_relocation(
    config: &StoreConfig,
    options: &crate::Options,
    input: &crate::Input,
    evidence: &crate::source_change::RelocationEvidence,
) -> Result<(), StorageError> {
    let path = input
        .full_path(options)
        .to_str()
        .ok_or(StorageError::InvalidArguments)?
        .to_owned();
    let encoded = serde_json::to_vec(evidence).map_err(|_| StorageError::Unavailable)?;
    if encoded.len() > 1024 * 1024 {
        return Err(StorageError::Busy);
    }
    let StoreBackend::Postgres { pool, control } = &config.backend else {
        return Err(StorageError::InvalidArguments);
    };
    pool.transaction(&config.namespace, TxnKind::Write, control, |tx| {
        binding(tx, config)?;
        archive(tx, &path)?;
        archive(tx, &evidence.from_path)?;
        let ns = config.namespace.as_str().as_bytes();
        tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
        let written = tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace AND semon_pg_native_source_mutations.evidence=excluded.evidence", &[&ns, &path.as_bytes(), &encoded]).map_err(sqlstate::error)?;
        if written != 1 {
            return Err(StorageError::Conflict);
        }
        tx.driver.execute("INSERT INTO semon_pg_native_relocation_candidates(namespace,old_path,new_path) VALUES($1,$2,$3) ON CONFLICT DO NOTHING", &[&ns,&evidence.from_path.as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?;
        super::source_journal::invalidate(tx, &path)?;
        super::source_journal::invalidate(tx, &evidence.from_path)
    })
}
pub(crate) fn catalog_work_pending(config: &StoreConfig) -> Result<bool, StorageError> {
    let StoreBackend::Postgres { pool, control } = &config.backend else {
        return Err(StorageError::InvalidArguments);
    };
    pool.transaction(&config.namespace, TxnKind::ReadSnapshot, control, |tx| {
        binding(tx, config)?;
        let ns = config.namespace.as_str().as_bytes();
        Ok(tx.driver.query_one("SELECT semantic_pending OR backfill_state IS DISTINCT FROM 'complete' OR EXISTS(SELECT 1 FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0))
    })
}
pub(crate) fn indexed_codex_sources(
    config: &StoreConfig,
    options: &crate::Options,
    id: &str,
) -> Result<Vec<crate::Input>, StorageError> {
    if id.is_empty() || id.len() > 4096 {
        return Err(StorageError::InvalidArguments);
    }
    let StoreBackend::Postgres { pool, control } = &config.backend else {
        return Err(StorageError::InvalidArguments);
    };
    pool.transaction(&config.namespace, TxnKind::ReadSnapshot, control, |tx| {
        binding(tx, config)?;
        let generation = model_cache::generation(tx)?.ok_or(StorageError::ScopeChanged)?;
        let rows = tx.driver.query("SELECT DISTINCT s.source_path FROM semon_pg_native_catalog_sources s JOIN semon_pg_native_catalog_rows r USING(namespace_digest,namespace,generation,key_digest) WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.native_digest=sha256($3::bytea) AND r.harness=$4 AND convert_from(s.metadata,'UTF8')::jsonb->>'native_id'=convert_from($3,'UTF8') LIMIT 2", &[&config.namespace.as_str().as_bytes(),&generation.as_bytes(),&id.as_bytes(),&b"codex".as_slice()]).map_err(sqlstate::error)?;
        let mut inputs = Vec::new();
        for row in rows {
            let path = String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?;
            if let Some(relative) = std::path::Path::new(&path).strip_prefix(&options.codex_home).ok().and_then(|relative| relative.to_str())
                && crate::is_input_path("codex", relative)
            {
                inputs.push(crate::Input { root: crate::InputRoot::Codex, path: relative.into() });
            }
        }
        Ok(inputs)
    })
}
