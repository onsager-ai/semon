//! Optional PostgreSQL resource/transaction foundation, not a selectable native
//! session backend. #479 still needs the shared #478 ingest/read/publication ports.
//! Credentials, connect deadlines and TLS (including cancellation TLS) belong to
//! the authorized host. This module never reads environment credentials.
mod cancel;
pub(crate) mod current_sources;
mod catalog_effects;
pub(crate) mod index_store;
mod model_cache;
mod native_bundle;
mod native_commit;
mod native_core;
mod native_ingest;
mod native_keys;
mod native_pages;
mod native_reader;
mod native_recovery;
mod native_usage;
mod native_usage_identity;
mod operations;
mod pool;
mod reported_facts;
mod schema;
mod semantic_events;
mod semantic_stage;
mod source_journal;
mod source_lifecycle;
mod sqlstate;
mod store_metadata;
pub(crate) mod tree_checkpoint;
mod tree_writer;
mod txn;

pub use cancel::PgCancellation;
pub use operations::{OperationIdentity, Reconciliation};
pub use pool::{ConnectFuture, PgConnection, PgConnectionFactory, PgPool, PoolLimits};
pub use source_journal::PgSourceJournal;
pub use store_metadata::PgStoreMetadata;

/// Lossless GUC transport for the common opaque UTF-8 bytes, including NUL.
fn namespace_context(namespace: &super::LogicalSourceNamespace) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(namespace.as_str().len() * 2);
    for byte in namespace.as_str().bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod native_ingest_tests;

#[cfg(test)]
mod native_recovery_tests;

#[cfg(test)]
mod proxy_test;

pub(crate) fn configured_query_catalog(
    options: &crate::Options,
    config: &super::StoreConfig,
    pool: PgPool,
    control: PgCancellation,
) -> Result<Box<dyn super::query::QueryCatalogStore>, super::StorageError> {
    let generation = configured_generation(config, &pool, &control)?;
    Ok(Box::new(catalog_effects::snapshot::query::QueryCatalog {
        pool,
        namespace: config.namespace.clone(),
        control,
        generation,
        options: options.clone(),
    }))
}

pub(crate) fn create_native_store(
    config: &super::StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
) -> Result<(), super::StorageError> {
    let epoch = config
        .expected_stream_epoch
        .as_deref()
        .filter(|epoch| epoch.len() == 64 && epoch.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(super::StorageError::InvalidArguments)?;
    pool.transaction(&config.namespace, txn::TxnKind::Write, control, |tx| {
        let inserted = tx.driver.execute(
            "INSERT INTO semon_pg_source_state(namespace) VALUES($1) ON CONFLICT DO NOTHING",
            &[&config.namespace.as_str().as_bytes()],
        ).map_err(sqlstate::error)?;
        if inserted != 1 { return Err(super::StorageError::Conflict); }
        tx.driver.execute(
            "INSERT INTO semon_pg_native_semantic_runtime(namespace,singleton,admission,stream_epoch) VALUES($1,true,1,$2)",
            &[&config.namespace.as_str().as_bytes(), &epoch.as_bytes()],
        ).map_err(sqlstate::error)?;
        Ok(())
    })
}

pub(crate) fn require_configured(
    config: &super::StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
) -> Result<(), super::StorageError> {
    pool.transaction(
        &config.namespace,
        txn::TxnKind::ReadSnapshot,
        control,
        |tx| {
            let binding = model_cache::binding(tx)?;
            if config.expected_stream_epoch.as_deref() != Some(binding.stream_epoch.as_str()) {
                return Err(super::StorageError::ScopeChanged);
            }
            Ok(())
        },
    )
}

fn configured_generation(
    config: &super::StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
) -> Result<String, super::StorageError> {
    pool.transaction(
        &config.namespace,
        txn::TxnKind::ReadSnapshot,
        control,
        |tx| {
            let binding = model_cache::binding(tx)?;
            if config.expected_stream_epoch.as_deref() != Some(binding.stream_epoch.as_str()) {
                return Err(super::StorageError::ScopeChanged);
            }
            model_cache::generation(tx)?.ok_or(super::StorageError::ScopeChanged)
        },
    )
}

pub(crate) fn catalog_page(
    config: &super::StoreConfig,
    pool: &PgPool,
    control: &PgCancellation,
    options: &crate::Options,
    request: &crate::catalog::Request,
) -> Result<crate::ViewerReply, super::StorageError> {
    let generation = configured_generation(config, pool, control)?;
    catalog_effects::snapshot::with_admitted_generation(
        pool,
        &config.namespace,
        control,
        &generation,
        |snapshot| {
            crate::catalog::read_page_snapshot(
                options,
                config.namespace.as_str(),
                request,
                snapshot,
            )
            .map_err(|_| super::StorageError::Unavailable)
        },
    )
}
