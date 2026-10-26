//! Native proof binding before Catalog effects, in the caller's transaction.
//! This is a prerequisite, NOT Source/runtime admission or a publication ACK.
use super::{MAX, catalog, encoded, path};
use crate::storage::{StorageError, proof_lifetime::ProofGenerationId};
use crate::storage::postgres::{native_pages, native_reader, sqlstate, txn::PgTxn};
use std::collections::BTreeMap;

/// No constructor, serialization or successful publication conversion. The
/// full Ledger stays inside the native adapter; a caller cannot supply a hash
/// and have it accepted as native authority or as a selected ReaderLease.
pub(super) struct NativeReference {
    pub(super) path: String,
    pub(super) usage_generation: i64,
    pub(super) pages: ProofGenerationId,
}

/// Captured from actual database state before model construction. This does
/// not claim runtime/epoch ownership, which has no PG port yet.
pub(super) struct Fence {
    namespace: String,
    revision: i64,
    head: Option<String>,
    claims: BTreeMap<String, Option<i64>>,
}

pub(super) fn capture(tx: &mut PgTxn<'_>, paths: &[String]) -> Result<Fence, StorageError> {
    tx.check()?;
    if paths.len() > 64 || paths.iter().map(String::len).sum::<usize>() > MAX {
        return Err(StorageError::Busy);
    }
    let rows = tx.driver.query(
        "SELECT revision,CASE WHEN octet_length(catalog_generation)<=64 THEN catalog_generation END,catalog_generation IS NULL FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
        &[&tx.namespace.as_str().as_bytes()],
    ).map_err(sqlstate::error)?;
    let row = rows.first().ok_or(StorageError::ScopeChanged)?;
    let head: Option<String> = row.get(1);
    if head.is_none() && !row.get::<_, bool>(2) {
        return Err(StorageError::ScopeChanged);
    }
    if head.as_ref().is_some_and(|head| head.len() != 64 || !head.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(StorageError::ScopeChanged);
    }
    let mut claims = BTreeMap::new();
    for path in paths {
        let rows = tx.driver.query(
            "SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2",
            &[&tx.namespace.as_str().as_bytes(), &path.as_bytes()],
        ).map_err(sqlstate::error)?;
        let revision = rows.first().map(|r| r.get::<_, i64>(0));
        if claims.insert(path.clone(), revision).is_some() {
            return Err(StorageError::InvalidArguments);
        }
        tx.check()?;
    }
    Ok(Fence { namespace: tx.namespace.as_str().to_owned(), revision: row.get(0), head, claims })
}

/// Validate the entire source cohort before the first Catalog effect. The
/// caller must retain the SourceState lock through commit, preventing native
/// advancement between these checks and its metadata writes.
fn bind(
    tx: &mut PgTxn<'_>,
    publication: &crate::slot_projection::Publication<'_>,
) -> Result<Vec<NativeReference>, StorageError> {
    tx.check()?;
    if publication.rows.len() > 64 {
        return Err(StorageError::Busy);
    }
    let mut sources = BTreeMap::new();
    let mut bytes = 0usize;
    for row in publication.rows {
        bytes = bytes.checked_add(encoded(row)?.len()).ok_or(StorageError::Busy)?;
        if bytes > MAX {
            return Err(StorageError::Busy);
        }
        for source in &row.sources {
            let key = path(source)?;
            if let Some(previous) = sources.insert(key, source) {
                // Two rows may share a source, but cannot disagree on its
                // physical witness or native identity within one publication.
                if encoded(previous)? != encoded(source)? {
                    return Err(StorageError::ScopeChanged);
                }
            }
            if sources.len() > 64 {
                return Err(StorageError::Busy);
            }
        }
    }
    let mut refs = Vec::with_capacity(sources.len());
    for (path, source) in sources {
        let (_, ledger) = native_reader::ledger(tx, path)?.ok_or(StorageError::ScopeChanged)?;
        if !catalog::source_matches_ledger(source, &ledger) {
            return Err(StorageError::ScopeChanged);
        }
        // open/current validates the current path, full Ledger, parser/header
        // contract and derived immutable generation against the actual store.
        let pinned = native_pages::current(tx, path, &ledger)?;
        let receipt = crate::events::store::header_ledger(&ledger);
        // Fixed-size SQL result: no receipt, path or header payload is fetched
        // before a size check. The actual Usage generation must be both ready
        // and the owner of the exact native page generation (not any old row).
        let rows = tx.driver.query(
            "SELECT s.source_id FROM semon_pg_native_usage_current c JOIN semon_pg_native_usage_sources s USING(namespace_digest,namespace,source_id) JOIN semon_pg_native_page_generations p USING(namespace_digest,namespace,source_id) WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.source_path_digest=semon_pg_native_digest($1,$2,'usage.path') AND semon_pg_native_key_matches(c.source_path,sha256($2::bytea),$2) AND s.source_path_digest=c.source_path_digest AND semon_pg_native_key_matches(s.source_path,sha256($2::bytea),$2) AND s.parser_version=$3 AND s.usage_ready AND s.receipt=$4 AND p.generation=$5 AND p.source_path=$2 AND p.ledger=$4 LIMIT 2",
            &[&tx.namespace.as_str().as_bytes(), &path.as_bytes(), &i64::from(crate::events::CACHE_VERSION), &receipt.as_bytes(), &pinned.id.0.as_bytes()],
        ).map_err(sqlstate::error)?;
        if rows.len() != 1 {
            return Err(StorageError::ScopeChanged);
        }
        let usage_generation: i64 = rows[0].get(0);
        if usage_generation <= 0 {
            return Err(StorageError::ScopeChanged);
        }
        refs.push(NativeReference { path: path.to_owned(), usage_generation, pages: pinned.id });
        tx.check()?;
    }
    Ok(refs)
}

/// Connected private flow: SourceState is already provisioned by real native
/// ingestion; missing state refuses without creating it. Bind every source
/// BEFORE stage writes any metadata. Returning references is deliberately not
/// an admitted head/claim ACK, durable ownership, page availability or Raw grant.
pub(super) fn stage_bound(
    tx: &mut PgTxn<'_>,
    generation: &str,
    publication: &crate::slot_projection::Publication<'_>,
    fence: &Fence,
) -> Result<Vec<NativeReference>, StorageError> {
    tx.check()?;
    let rows = tx.driver.query(
        "SELECT namespace FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE",
        &[&tx.namespace.as_str().as_bytes()],
    ).map_err(sqlstate::error)?;
    if rows.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    if fence.namespace != tx.namespace.as_str() {
        return Err(StorageError::ScopeChanged);
    }
    let actual = capture(tx, &fence.claims.keys().cloned().collect::<Vec<_>>())?;
    if actual.revision != fence.revision || actual.head != fence.head || actual.claims != fence.claims {
        return Err(StorageError::Conflict);
    }
    if publication.rows.iter().flat_map(|row| &row.sources).any(|source| {
        source.path.to_str().is_none_or(|path| !fence.claims.contains_key(path))
    }) {
        return Err(StorageError::ScopeChanged);
    }
    let refs = bind(tx, publication)?;
    super::stage(tx, generation, publication)?;
    Ok(refs)
}

