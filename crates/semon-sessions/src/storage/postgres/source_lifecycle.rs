//! Ordinary source component discovery over one physical Current snapshot.
use super::{model_cache, sqlstate, txn::PgTxn};
use crate::{
    model::{
        relation_stage::{self, LifecycleStore},
        summary::CatalogRow,
    },
    storage::StorageError,
};
use std::{cell::RefCell, collections::BTreeSet, io};

pub(super) fn component_paths(
    tx: &mut PgTxn<'_>,
    paths: &[String],
    keys: &[String],
    native: &[(String, String)],
) -> Result<Option<Vec<String>>, StorageError> {
    if paths.is_empty() || paths.len() > 64 || keys.len() > 64 || native.len() > 64 {
        return Ok(None);
    }
    let Some(generation) = model_cache::generation(tx)? else {
        return Ok(Some(Vec::new()));
    };
    let lookup = Lookup {
        tx: RefCell::new(tx),
        generation,
    };
    let mut seeds = paths.to_vec();
    let mut seeded = Vec::new();
    for key in keys {
        if lookup.exists(key)? {
            seeded.push(key.clone());
        }
    }
    for (harness, id) in native {
        seeded.extend(lookup.native_owners(harness, id)?);
    }
    for key in seeded {
        let row = lookup.owner(&key).map_err(storage)?;
        seeds.extend(
            row.sources
                .iter()
                .map(|s| s.path.to_string_lossy().into_owned()),
        );
    }
    let owners = relation_stage::affected_owners(&lookup, &seeds).map_err(storage)?;
    let tx = lookup.tx.into_inner();
    let ns = tx.namespace.as_str().as_bytes();
    let mut sources = owners
        .values()
        .flat_map(|r| {
            r.sources
                .iter()
                .map(|s| s.path.to_string_lossy().into_owned())
        })
        .collect::<BTreeSet<_>>();
    // A recorded relocation joins both physical source names into the same
    // native reconstruction cohort; it never grants authority to either file.
    for path in paths {
        let moves=tx.driver.query("SELECT new_path FROM semon_pg_native_relocation_candidates WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND old_path=$2 UNION SELECT old_path FROM semon_pg_native_relocation_candidates WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND new_path=$2 LIMIT 65", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
        for moved in moves {
            let bytes: Vec<u8> = moved.get(0);
            sources.insert(String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged)?);
        }
    }
    if sources.len() > 64 {
        return Ok(None);
    }
    tx.check()?;
    Ok(Some(sources.into_iter().collect()))
}

fn storage(error: io::Error) -> StorageError {
    error
        .into_inner()
        .and_then(|error| error.downcast::<StorageError>().ok())
        .map_or(StorageError::Busy, |error| *error)
}

/// Keyed lookups over one physical Current generation. Each call is bounded
/// by its own page; the shared reducer owns the 64 owner / 1 MiB budget.
struct Lookup<'t, 'c> {
    tx: RefCell<&'t mut PgTxn<'c>>,
    generation: String,
}

impl Lookup<'_, '_> {
    fn keys(&self, sql: &str, value: &[u8], extra: &[u8]) -> io::Result<Vec<String>> {
        let tx = self.tx.borrow_mut();
        tx.check().map_err(io::Error::other)?;
        let ns = tx.namespace.as_str().as_bytes().to_vec();
        let rows = tx
            .driver
            .query(sql, &[&ns, &self.generation.as_bytes(), &value, &extra])
            .map_err(sqlstate::error)
            .map_err(io::Error::other)?;
        if rows.len() > 64 {
            return Err(io::Error::other(StorageError::Busy));
        }
        rows.iter()
            .map(|row| String::from_utf8(row.get(0)))
            .collect::<Result<_, _>>()
            .map_err(|_| io::Error::other(StorageError::ScopeChanged))
    }
    fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let found = self.keys("SELECT session_key FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 AND $4::bytea IS NOT NULL", key.as_bytes(), b"").map_err(storage)?;
        Ok(!found.is_empty())
    }
    fn native_owners(&self, harness: &str, id: &str) -> Result<Vec<String>, StorageError> {
        self.keys("SELECT DISTINCT r.session_key FROM semon_pg_native_catalog_sources s JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=s.namespace_digest AND r.namespace=s.namespace AND r.generation=s.generation AND r.key_digest=s.key_digest WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.native_digest=sha256($3::bytea) AND r.harness=$4 ORDER BY r.session_key LIMIT 65", id.as_bytes(), harness.as_bytes()).map_err(storage)
    }
}

impl LifecycleStore for Lookup<'_, '_> {
    fn path_owners(&self, path: &str) -> io::Result<Vec<String>> {
        self.keys("SELECT session_key FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND path_digest=sha256($3::bytea) AND source_path=$3 AND $4::bytea IS NOT NULL ORDER BY session_key LIMIT 65", path.as_bytes(), b"")
    }
    fn owner(&self, key: &str) -> io::Result<CatalogRow> {
        let tx = self.tx.borrow_mut();
        tx.check().map_err(io::Error::other)?;
        let ns = tx.namespace.as_str().as_bytes().to_vec();
        let rows = tx.driver.query("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3", &[&ns, &self.generation.as_bytes(), &key.as_bytes()]).map_err(sqlstate::error).map_err(io::Error::other)?;
        let row = rows
            .first()
            .ok_or_else(|| io::Error::other("missing Current source owner"))?;
        let bytes = row
            .get::<_, Option<Vec<u8>>>(0)
            .ok_or_else(|| io::Error::other(StorageError::Busy))?;
        let row: CatalogRow = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::other(StorageError::ScopeChanged))?;
        if row.key != key {
            return Err(io::Error::other(StorageError::ScopeChanged));
        }
        // Relationship incidence is indexed from the saved selected edges.
        if row
            .query
            .as_ref()
            .and_then(|q| q["selected"]["edges"].as_array())
            .is_none()
        {
            return Err(io::Error::other(StorageError::NamespaceUpgradePending));
        }
        Ok(row)
    }
    fn neighbors(&self, key: &str) -> io::Result<Vec<String>> {
        // Parent/trace incidence in both directions, plus same-harness owners
        // sharing a native ID. Only Current owners of this generation qualify.
        self.keys("WITH peers AS (SELECT peer AS k FROM semon_pg_native_catalog_links WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3 UNION SELECT session_key FROM semon_pg_native_catalog_links WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND peer_digest=sha256($3::bytea) AND peer=$3 UNION SELECT p.session_key FROM semon_pg_native_catalog_sources o JOIN semon_pg_native_catalog_rows orow ON orow.namespace_digest=o.namespace_digest AND orow.namespace=o.namespace AND orow.generation=o.generation AND orow.key_digest=o.key_digest JOIN semon_pg_native_catalog_sources p ON p.namespace_digest=o.namespace_digest AND p.namespace=o.namespace AND p.generation=o.generation AND p.native_digest=o.native_digest JOIN semon_pg_native_catalog_rows prow ON prow.namespace_digest=p.namespace_digest AND prow.namespace=p.namespace AND prow.generation=p.generation AND prow.key_digest=p.key_digest AND prow.harness=orow.harness WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.key_digest=sha256($3::bytea) AND $4::bytea IS NOT NULL) SELECT DISTINCT r.session_key FROM peers JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2 AND r.key_digest=sha256(peers.k) AND r.session_key=peers.k WHERE peers.k<>$3 ORDER BY r.session_key LIMIT 65", key.as_bytes(), b"")
    }
}

/// Resolve an observed non-log input under the same namespace writer fence.
/// The ordinary API has no explicit OpID: the exact revision and full request
/// form its durable replay identity, as for ordinary Catalog publication.
pub(super) fn resolve(
    tx: &mut PgTxn<'_>,
    path: &str,
    revision: i64,
    native: Option<(&str, &str)>,
    log: Option<&str>,
) -> Result<crate::events::Outcome, StorageError> {
    use crate::events::Outcome;
    use sha2::{Digest, Sha256};
    if path.is_empty()
        || path.len() > 4096
        || revision <= 0
        || log.is_some_and(|p| p.is_empty() || p.len() > 4096)
    {
        return Err(StorageError::InvalidArguments);
    }
    let request = serde_json::to_vec(&("resolve-catalog-input-v1", path, revision, native, log))
        .map_err(|_| StorageError::InvalidArguments)?;
    if request.len() > 1048576 {
        return Err(StorageError::Busy);
    }
    let key = format!("source-resolve-v1:{:x}", Sha256::digest(&request));
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let receipt=tx.driver.query("SELECT value=$3 FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND key=$2", &[&ns,&key.as_bytes(),&request]).map_err(sqlstate::error)?;
    if let Some(row) = receipt.first() {
        return if row.get::<_, bool>(0) {
            Ok(Outcome::Written)
        } else {
            Err(StorageError::Conflict)
        };
    }
    if super::native_keys::read_meta(tx, format!("raw-write-inflight:{path}").as_bytes(), 16384)?
        .is_some()
    {
        return Ok(Outcome::Conflict);
    }
    let active:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND active)", &[&ns]).map_err(sqlstate::error)?.get(0);
    if active {
        return Ok(Outcome::Conflict);
    }
    let observed=tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
    if observed.first().map(|r| r.get::<_, i64>(0)) != Some(revision) {
        return Ok(Outcome::Conflict);
    }
    let mut affected = BTreeSet::new();
    if let Some((harness, id)) = native {
        affected.extend(dependent_paths(tx, harness, id)?);
    }
    if let Some(log) = log {
        affected.insert(log.to_owned());
    }
    if affected.len() > 64 {
        return Err(StorageError::Busy);
    }
    for target in affected {
        super::source_journal::invalidate(tx, &target)?;
    }
    if tx.driver.execute("DELETE FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND revision=$3", &[&ns,&path.as_bytes(),&revision]).map_err(sqlstate::error)?!=1 { return Err(StorageError::Conflict); }
    tx.driver.execute("DELETE FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
    tx.driver
        .execute(
            "INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3)",
            &[&ns, &key.as_bytes(), &request],
        )
        .map_err(sqlstate::error)?;
    tx.check()?;
    Ok(Outcome::Written)
}

fn dependent_paths(
    tx: &mut PgTxn<'_>,
    harness: &str,
    id: &str,
) -> Result<BTreeSet<String>, StorageError> {
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let mut affected = BTreeSet::new();
    let runtime = format!("runtime:native:{harness}");
    let dependencies=tx.driver.query("SELECT source_path FROM semon_pg_native_native_dependencies WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND harness IN ($2,$3) AND native_id=$4 LIMIT 65", &[&ns,&harness.as_bytes(),&runtime.as_bytes(),&id.as_bytes()]).map_err(sqlstate::error)?;
    if dependencies.len() > 64 {
        return Err(StorageError::Busy);
    }
    for row in dependencies {
        affected.insert(String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?);
    }
    if let Some(generation) = model_cache::generation(tx)? {
        // Every Source of a same-harness Current owner of this native ID.
        let paths=tx.driver.query("SELECT DISTINCT s.source_path FROM semon_pg_native_catalog_sources s WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.key_digest IN (SELECT o.key_digest FROM semon_pg_native_catalog_sources o JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=o.namespace_digest AND r.namespace=o.namespace AND r.generation=o.generation AND r.key_digest=o.key_digest WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.native_digest=sha256($3::bytea) AND r.harness=$4) ORDER BY s.source_path LIMIT 65", &[&ns,&generation.as_bytes(),&id.as_bytes(),&harness.as_bytes()]).map_err(sqlstate::error)?;
        if paths.len() > 64 {
            return Err(StorageError::Busy);
        }
        for row in paths {
            affected.insert(String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?);
        }
    }
    if affected.len() > 64 {
        return Err(StorageError::Busy);
    }
    Ok(affected)
}

pub(super) fn resume_runtime(
    tx: &mut PgTxn<'_>,
    options: &crate::Options,
    path: &str,
) -> Result<(), StorageError> {
    use sha2::{Digest, Sha256};
    if options.facts.is_none() {
        return Ok(());
    }
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let values=tx.driver.query("SELECT CASE WHEN octet_length(evidence)<=2048 THEN evidence END FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
    let Some(value) = values.first() else {
        return Ok(());
    };
    let bytes: Vec<u8> = value
        .get::<_, Option<Vec<u8>>>(0)
        .ok_or(StorageError::Busy)?;
    let evidence: crate::source_change::RuntimeInputEvidence =
        serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    if evidence.unaffected.len() != 64
        || !evidence.unaffected.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(StorageError::ScopeChanged);
    }
    let Some(id) = crate::events::store::resumed_runtime_native(options, path, &evidence)
        .map_err(|_| StorageError::ScopeChanged)?
    else {
        return Ok(());
    };
    let request = serde_json::to_vec(&("runtime-input-resume-v1", path, &bytes, &id))
        .map_err(|_| StorageError::Unavailable)?;
    let key = format!("runtime-input-resume-v1:{:x}", Sha256::digest(&request));
    let receipt=tx.driver.query("SELECT value=$3 FROM semon_pg_native_meta WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND key_digest=sha256($2::bytea) AND key=$2", &[&ns,&key.as_bytes(),&request]).map_err(sqlstate::error)?;
    if let Some(value) = receipt.first() {
        return if value.get::<_, bool>(0) {
            Ok(())
        } else {
            Err(StorageError::Conflict)
        };
    }
    for affected in dependent_paths(tx, "claude", &id)? {
        super::source_journal::invalidate(tx, &affected)?;
    }
    tx.driver
        .execute(
            "INSERT INTO semon_pg_native_meta(namespace,key,value) VALUES($1,$2,$3)",
            &[&ns, &key.as_bytes(), &request],
        )
        .map_err(sqlstate::error)?;
    tx.check()
}

pub(super) fn relocation_candidate(
    tx: &mut PgTxn<'_>,
    path: &str,
    id: &str,
) -> Result<Option<String>, StorageError> {
    use crate::events::native_commit::relocation_namespace;
    let Some((root, archived)) = relocation_namespace(path) else {
        return Ok(None);
    };
    let Some(generation) = model_cache::generation(tx)? else {
        return Ok(None);
    };
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    // Codex Current Sources of the same native ID, by the native index.
    let rows=tx.driver.query("SELECT CASE WHEN sum(octet_length(s.metadata)) OVER ()<=1048576 THEN s.metadata END FROM semon_pg_native_catalog_sources s JOIN semon_pg_native_catalog_rows r ON r.namespace_digest=s.namespace_digest AND r.namespace=s.namespace AND r.generation=s.generation AND r.key_digest=s.key_digest WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.generation=$2 AND s.native_digest=sha256($3::bytea) AND r.harness=$4 ORDER BY s.source_path LIMIT 65", &[&ns,&generation.as_bytes(),&id.as_bytes(),&b"codex".as_slice()]).map_err(sqlstate::error)?;
    if rows.len() > 64 {
        return Err(StorageError::Busy);
    }
    let mut candidates = BTreeSet::new();
    for row in rows {
        let bytes = row.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?;
        let source: crate::model::summary::CatalogSource =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        let Some(old) = source.path.to_str() else {
            return Err(StorageError::ScopeChanged);
        };
        if source.native_id == id
            && old != path
            && relocation_namespace(old).is_some_and(|(r, a)| r == root && a != archived)
        {
            candidates.insert(old.to_owned());
        }
    }
    if candidates.is_empty() {
        let rows=tx.driver.query("SELECT old_path FROM semon_pg_native_relocation_candidates WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND new_path=$2 LIMIT 2", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
        for row in rows {
            candidates
                .insert(String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?);
        }
    }
    if candidates.len() != 1 {
        return Ok(None);
    }
    let old = candidates.into_iter().next().unwrap();
    let announced:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_relocation_candidates WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND old_path=$2 AND new_path=$3)", &[&ns,&old.as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if !announced
        && !std::fs::symlink_metadata(&old).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
    {
        return Ok(None);
    }
    Ok(Some(old))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn relocate(
    tx: &mut PgTxn<'_>,
    old: &str,
    path: &str,
    id: &str,
    stat: &crate::events::Stat,
    prior: &crate::events::Ledger,
    summary: crate::native_tree::Summary,
) -> Result<bool, StorageError> {
    use sha2::{Digest, Sha256};
    let request = serde_json::to_vec(&("native-relocation-v1", old, path, id, stat, prior))
        .map_err(|_| StorageError::InvalidArguments)?;
    let key = format!("native-relocation-ack:{:x}", Sha256::digest(&request));
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if let Some(ack) = super::native_keys::read_meta(tx, key.as_bytes(), 1048576)? {
        return if ack == request {
            Ok(true)
        } else {
            Err(StorageError::Conflict)
        };
    }
    if relocation_candidate(tx, path, id)?.as_deref() != Some(old) {
        return Err(StorageError::Conflict);
    }
    let Some((actual, mut index)) = super::native_reader::file(tx, old)? else {
        return Err(StorageError::Conflict);
    };
    if actual != *prior || super::native_reader::ledger(tx, path)?.is_some() {
        return Err(StorageError::Conflict);
    }
    if index.codex_header.as_ref().and_then(|h| h.id.as_deref()) != Some(id) {
        return Err(StorageError::ScopeChanged);
    }
    let active:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_stage_items WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND active)", &[&ns]).map_err(sqlstate::error)?.get(0);
    if active
        || crate::events::Stat::of(std::path::Path::new(path))
            .ok()
            .as_ref()
            != Some(stat)
    {
        return Err(StorageError::Conflict);
    }
    let pending = tx.driver.query("SELECT CASE WHEN octet_length(evidence)<=1048576 THEN evidence END FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns, &path.as_bytes()]).map_err(sqlstate::error)?;
    if let Some(pending) = pending.first() {
        let bytes = pending
            .get::<_, Option<Vec<u8>>>(0)
            .ok_or(StorageError::Busy)?;
        let evidence: crate::source_change::RelocationEvidence =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::Conflict)?;
        if evidence.from_path != old
            || evidence.native_id != id
            || evidence.before != prior.stat
            || evidence.after != *stat
            || evidence.length != prior.offset
            || evidence.prefix
                != format!(
                    "{:x}",
                    sha2::digest::generic_array::GenericArray::from(prior.prefix)
                )
        {
            return Err(StorageError::Conflict);
        }
    }
    let mut after = prior.clone();
    after.stat = *stat;
    after.stat.size = prior.stat.size.min(stat.size);
    index.tree_summary = Some(summary);
    tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace", &[&ns,&path.as_bytes(),&request]).map_err(sqlstate::error)?;
    // Reuse the ordinary native commit algorithm to make a complete new path
    // generation. The old native families and immutable proofs remain retained.
    super::native_bundle::stage(
        tx,
        crate::events::native_commit::Input {
            path,
            ledger: &after,
            index: &index,
            changes: None,
            expected: None,
            semantic_change: true,
        },
    )?;
    let proof = crate::events::native_commit::CatalogRelocation {
        version: 1,
        new_path: path.to_owned(),
        native_id: id.to_owned(),
        prior: prior.clone(),
        catalog_before: None,
        after: after.clone(),
    };
    super::native_keys::put_meta(
        tx,
        format!("native_catalog_relocation:{old}").as_bytes(),
        &serde_json::to_vec(&proof).map_err(|_| StorageError::Unavailable)?,
    )?;
    tx.driver.execute("INSERT INTO semon_pg_native_relocation_candidates(namespace,old_path,new_path) VALUES($1,$2,$3) ON CONFLICT DO NOTHING", &[&ns,&old.as_bytes(),&path.as_bytes()]).map_err(sqlstate::error)?;
    super::source_journal::invalidate(tx, old)?;
    tx.driver.execute("UPDATE semon_pg_source_invalidations SET reason='removed' WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&old.as_bytes()]).map_err(sqlstate::error)?;
    if after.stat != *stat {
        let evidence = crate::source_change::AppendEvidence {
            generation: format!(
                "{:032x}",
                u128::from_be_bytes(
                    prior.prefix[..16]
                        .try_into()
                        .map_err(|_| StorageError::ScopeChanged)?
                )
            ),
            before: after.stat,
            after: *stat,
        };
        super::native_keys::put_meta(
            tx,
            format!("native_relocation_append:{path}").as_bytes(),
            &serde_json::to_vec(&evidence).map_err(|_| StorageError::Unavailable)?,
        )?;
    }
    super::native_keys::put_meta(
        tx,
        format!("source-retired:{old}").as_bytes(),
        &serde_json::to_vec(&Some(prior)).map_err(|_| StorageError::Unavailable)?,
    )?;
    // Semantic staging still visits the outgoing Current path. Keep its exact
    // removal claim until successor publication acknowledges that revision.
    // The original physical Stat cannot be recovered from the mutable append
    // witness, so retain the verbatim request alongside its durable ACK.
    super::native_keys::put_meta(
        tx,
        format!("native-relocation-receipt:{old}").as_bytes(),
        &request,
    )?;
    super::native_keys::put_meta(tx, key.as_bytes(), &request)?;
    tx.check()?;
    Ok(true)
}

/// A completed relocation has already retained both native generations. Replay
/// only that exact removal observation, including during semantic staging; this
/// performs no withdrawal, claim mutation or native body/filesystem I/O.
pub(super) fn relocation_removed(
    tx: &mut PgTxn<'_>,
    old: &str,
    expected: &crate::events::Ledger,
) -> Result<bool, StorageError> {
    use crate::events::native_commit::{CatalogRelocation, relocation_namespace};
    use sha2::{Digest, Sha256};
    let Some(bytes) = super::native_keys::read_meta(
        tx,
        format!("native_catalog_relocation:{old}").as_bytes(),
        16384,
    )? else {
        return Ok(false);
    };
    let proof: CatalogRelocation =
        serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
    if proof.version != 1
        || &proof.prior != expected
        || old == proof.new_path
        || !relocation_namespace(old).is_some_and(|(root, archived)| {
            relocation_namespace(&proof.new_path)
                .is_some_and(|(new_root, new_archived)| root == new_root && archived != new_archived)
        })
        || super::native_reader::ledger(tx, old)?
            .map(|(_, ledger)| ledger)
            .as_ref()
            != Some(expected)
        || super::native_reader::ledger(tx, &proof.new_path)?.map(|(_, ledger)| ledger)
            != Some(proof.after.clone())
    {
        return Ok(false);
    }
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let paths = vec![old.as_bytes().to_vec(), proof.new_path.as_bytes().to_vec()];
    let pending: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=ANY($2::bytea[]))", &[&ns, &paths]).map_err(sqlstate::error)?.get(0);
    if pending {
        return Ok(false);
    }
    let candidate: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_relocation_candidates WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND old_path=$2 AND new_path=$3)", &[&ns, &old.as_bytes(), &proof.new_path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if !candidate
        || super::native_keys::read_meta(tx, format!("source-retired:{old}").as_bytes(), 16384)?
            != Some(serde_json::to_vec(&Some(expected)).map_err(|_| StorageError::Unavailable)?)
    {
        return Ok(false);
    }
    let Some(request) = super::native_keys::read_meta(
        tx,
        format!("native-relocation-receipt:{old}").as_bytes(),
        16384,
    )? else {
        return Ok(false);
    };
    let (kind, from, to, id, stat, prior): (
        String,
        String,
        String,
        String,
        crate::events::Stat,
        crate::events::Ledger,
    ) = serde_json::from_slice(&request).map_err(|_| StorageError::ScopeChanged)?;
    let ack = format!("native-relocation-ack:{:x}", Sha256::digest(&request));
    if kind != "native-relocation-v1"
        || from != old
        || to != proof.new_path
        || id != proof.native_id
        || prior != proof.prior
        || stat.size < prior.offset
        || super::native_keys::read_meta(tx, ack.as_bytes(), 16384)?.as_ref() != Some(&request)
    {
        return Ok(false);
    }
    for (path, ledger) in [(old, expected), (proof.new_path.as_str(), &proof.after)] {
        if super::native_keys::read_meta(
            tx,
            format!("raw-write-inflight:{path}").as_bytes(),
            16384,
        )?
        .is_some()
        {
            return Ok(false);
        }
        let Some(header) = super::native_keys::read_meta(
            tx,
            format!("native_codex_header:{path}").as_bytes(),
            1048576,
        )? else {
            return Ok(false);
        };
        let header: crate::events::native_aux::CodexHeaderRecord =
            serde_json::from_slice(&header).map_err(|_| StorageError::ScopeChanged)?;
        if header.version != 1
            || header.parser_version != crate::events::CACHE_VERSION
            || header.ledger != crate::events::store::header_ledger(ledger)
            || header.header.id.as_deref() != Some(proof.native_id.as_str())
        {
            return Ok(false);
        }
        super::native_pages::current(tx, path, ledger)?;
    }
    tx.check()?;
    Ok(true)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIntent {
    request: Vec<u8>,
    revision: i64,
    previous: Option<crate::source_change::AppendEvidence>,
}
fn raw_request(
    tx: &PgTxn<'_>,
    input: &crate::Input,
    intent: &crate::source_change::WriteIntent,
) -> Result<Vec<u8>, StorageError> {
    if intent.operation.is_empty()
        || intent.operation.len() > 512
        || intent.root != input.root.as_str()
        || intent.path.len() > 4096
    {
        return Err(StorageError::InvalidArguments);
    }
    let bytes = serde_json::to_vec(&(
        "raw-source-write-v1",
        tx.namespace.as_str(),
        input.root.as_str(),
        &input.path,
        intent,
    ))
    .map_err(|_| StorageError::InvalidArguments)?;
    if bytes.len() > 16384 {
        return Err(StorageError::Busy);
    }
    Ok(bytes)
}
fn raw_intent_key(intent: &crate::source_change::WriteIntent) -> String {
    format!("raw-write-intent:{}", intent.operation)
}
fn raw_ack_key(intent: &crate::source_change::WriteIntent) -> String {
    format!("raw-write-ack:{}", intent.operation)
}

pub(super) fn write_begin(
    tx: &mut PgTxn<'_>,
    options: &crate::Options,
    input: &crate::Input,
    intent: &crate::source_change::WriteIntent,
) -> Result<(i64, Option<crate::source_change::AppendEvidence>), StorageError> {
    #[cfg(test)]
    let _sql_scope = crate::source_change::SqlLockScope::enter();
    let request = raw_request(tx, input, intent)?;
    if input.full_path(options).to_str() != Some(intent.path.as_str()) {
        return Err(StorageError::InvalidArguments);
    }
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if let Some(bytes) =
        super::native_keys::read_meta(tx, raw_intent_key(intent).as_bytes(), 131072)?
    {
        let prior: RawIntent =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if prior.request != request {
            return Err(StorageError::Conflict);
        }
        return Ok((prior.revision, prior.previous));
    }
    if model_cache::generation(tx)? != intent.head
        || super::native_reader::ledger(tx, &intent.path)?.map(|(_, l)| l) != intent.ledger
    {
        return Err(StorageError::Conflict);
    }
    let actual = crate::events::Stat::of(&input.full_path(options)).ok();
    if actual != intent.before {
        return Err(StorageError::Conflict);
    }
    if let Some(c) = &intent.continuation {
        let ledger = intent.ledger.as_ref().ok_or(StorageError::ScopeChanged)?;
        let pinned = super::native_pages::current(tx, &intent.path, ledger)?;
        let header: crate::events::native_pages::chunks::Header =
            serde_json::from_str(&pinned.chunks_header).map_err(|_| StorageError::ScopeChanged)?;
        if pinned.id.0 != c.proof
            || header.resume.as_ref() != Some(&c.state)
            || c.offset != ledger.offset
            || c.prefix != ledger.prefix
            || intent.before_hash != Some(ledger.prefix)
        {
            return Err(StorageError::Conflict);
        }
    }
    let inflight = format!("raw-write-inflight:{}", intent.path);
    if super::native_keys::read_meta(tx, inflight.as_bytes(), 16384)?.is_some() {
        return Err(StorageError::Conflict);
    }
    let previous = model_cache::append_evidence(tx, &intent.path)?;
    // Existing append evidence can be continued; unknown pending evidence may
    // not be overwritten to manufacture a fresh source write.
    let mutation:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2)", &[&ns,&intent.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if mutation && previous.is_none() {
        return Err(StorageError::Conflict);
    }
    super::source_journal::invalidate(tx, &intent.path)?;
    let revision:i64=tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&intent.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if input.root == crate::InputRoot::Claude
        && input.path.starts_with("sessions/")
        && let Some(facts) = options
            .facts
            .as_ref()
            .map(|p| crate::read_facts(p))
            .transpose()
            .map_err(|_| StorageError::ScopeChanged)?
    {
        let evidence = crate::source_change::RuntimeInputEvidence {
            unaffected: crate::native_evidence::recorded_runtime_fingerprint_excluding(
                options,
                &facts,
                Some(std::path::Path::new(&intent.path)),
            )
            .map_err(|_| StorageError::ScopeChanged)?,
        };
        let bytes = serde_json::to_vec(&evidence).map_err(|_| StorageError::Unavailable)?;
        tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace", &[&ns,&intent.path.as_bytes(),&bytes]).map_err(sqlstate::error)?;
    }
    if input.root == crate::InputRoot::Claude && input.path.starts_with("sessions/") {
        let old = crate::read_regular_at_most(&input.full_path(options), 65536)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["sessionId"].as_str().map(str::to_owned));
        if let Some(old) = old {
            for affected in dependent_paths(tx, "claude", &old)? {
                super::source_journal::invalidate(tx, &affected)?;
            }
        }
    }
    let state = RawIntent {
        request: request.clone(),
        revision,
        previous: previous.clone(),
    };
    super::native_keys::put_meta(
        tx,
        raw_intent_key(intent).as_bytes(),
        &serde_json::to_vec(&state).map_err(|_| StorageError::Unavailable)?,
    )?;
    super::native_keys::put_meta(tx, inflight.as_bytes(), &request)?;
    tx.check()?;
    Ok((revision, previous))
}

pub(super) fn write_complete(
    tx: &mut PgTxn<'_>,
    options: &crate::Options,
    input: &crate::Input,
    intent: &crate::source_change::WriteIntent,
    revision: i64,
    completion: &crate::source_change::WriteCompletion,
) -> Result<(), StorageError> {
    use sha2::{Digest, Sha256};
    #[cfg(test)]
    let _sql_scope = crate::source_change::SqlLockScope::enter();
    let request = raw_request(tx, input, intent)?;
    let ack = serde_json::to_vec(&(&request, revision, completion))
        .map_err(|_| StorageError::InvalidArguments)?;
    if ack.len() > 131072 {
        return Err(StorageError::Busy);
    }
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if let Some(prior) = super::native_keys::read_meta(tx, raw_ack_key(intent).as_bytes(), 131072)?
    {
        return if prior == ack {
            Ok(())
        } else {
            Err(StorageError::Conflict)
        };
    }
    let stored = super::native_keys::read_meta(tx, raw_intent_key(intent).as_bytes(), 131072)?
        .ok_or(StorageError::ScopeChanged)?;
    let state: RawIntent =
        serde_json::from_slice(&stored).map_err(|_| StorageError::ScopeChanged)?;
    let inflight = format!("raw-write-inflight:{}", intent.path);
    if state.request != request
        || state.revision != revision
        || super::native_keys::read_meta(tx, inflight.as_bytes(), 16384)?.as_ref() != Some(&request)
        || model_cache::generation(tx)? != intent.head
        || super::native_reader::ledger(tx, &intent.path)?.map(|(_, l)| l) != intent.ledger
    {
        return Err(StorageError::Conflict);
    }
    let claims=tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&intent.path.as_bytes()]).map_err(sqlstate::error)?;
    if claims.first().map(|r| r.get::<_, i64>(0)) != Some(revision) {
        return Err(StorageError::Conflict);
    }
    let path = input.full_path(options);
    if crate::events::Stat::of(&path).ok() != Some(completion.after)
        || completion.after.size != completion.to
    {
        return Err(StorageError::ScopeChanged);
    }
    if completion.evidence
        != crate::source_change::append_plan(
            intent.generation.clone(),
            intent.before,
            state.previous,
            intent.from,
            completion.after,
            completion.to,
        )
    {
        return Err(StorageError::ScopeChanged);
    }
    if let Some(evidence) = &completion.evidence {
        let before = intent.before.ok_or(StorageError::ScopeChanged)?;
        if !evidence.qualifies(&evidence.before, &completion.after)
            || intent.generation.as_deref() != Some(evidence.generation.as_str())
            || before.size != intent.from
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    let runtime = crate::native_evidence::query_runtime_generation(options)
        .map_err(|_| StorageError::ScopeChanged)?;
    let facts = options
        .facts
        .as_ref()
        .map(|p| {
            let facts = crate::read_facts(p).map_err(|_| StorageError::ScopeChanged)?;
            Ok::<_, StorageError>(format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&facts).map_err(|_| StorageError::Unavailable)?)
            ))
        })
        .transpose()?;
    if runtime != completion.runtime || facts != completion.facts {
        return Err(StorageError::Conflict);
    }
    resume_runtime(tx, options, &intent.path)?;
    if let Some(evidence) = &completion.evidence {
        let bytes = serde_json::to_vec(evidence).map_err(|_| StorageError::Unavailable)?;
        tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace", &[&ns,&intent.path.as_bytes(),&bytes]).map_err(sqlstate::error)?;
    }
    // Metadata records the qualified interpretation; it never contains Raw body.
    super::native_keys::put_meta(tx, b"raw-write-runtime", completion.runtime.as_bytes())?;
    if let Some(facts) = &completion.facts {
        super::native_keys::put_meta(tx, b"raw-write-facts", facts.as_bytes())?;
    }
    super::native_keys::put_meta(tx, raw_ack_key(intent).as_bytes(), &ack)?;
    super::native_keys::delete_meta(tx, inflight.as_bytes())?;
    tx.check()?;
    Ok(())
}

fn facts_request(
    tx: &PgTxn<'_>,
    intent: &crate::source_change::FactsIntent,
) -> Result<Vec<u8>, StorageError> {
    if intent.operation.is_empty()
        || intent.operation.len() > 512
        || intent.path.is_empty()
        || intent.path.len() > 4096
    {
        return Err(StorageError::InvalidArguments);
    }
    if intent.wanted.version != 2 || !matches!(intent.before.version, 0 | 2) {
        return Err(StorageError::ScopeChanged);
    }
    let request = serde_json::to_vec(&("facts-source-write-v1", tx.namespace.as_str(), intent))
        .map_err(|_| StorageError::InvalidArguments)?;
    if request.len() > 1048576 {
        return Err(StorageError::Busy);
    }
    Ok(request)
}
pub(super) fn facts_begin(
    tx: &mut PgTxn<'_>,
    intent: &crate::source_change::FactsIntent,
    actual: &crate::Facts,
) -> Result<(), StorageError> {
    let request = facts_request(tx, intent)?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let key = format!("facts-write-intent:{}", intent.operation);
    if let Some(prior) = super::native_keys::read_meta(tx, key.as_bytes(), 1048576)? {
        return if prior == request {
            Ok(())
        } else {
            Err(StorageError::Conflict)
        };
    }
    // Active/archive rollout membership changes are admitted: completion
    // invalidates exactly the changed rollout Sources (or every Codex Source
    // when the selection itself appears/disappears) for ordinary catch-up.
    if model_cache::generation(tx)? != intent.head
        || actual != &intent.before
        || super::native_keys::read_meta(tx, b"facts-write-inflight", 1048576)?.is_some()
    {
        return Err(StorageError::Conflict);
    }
    super::native_keys::put_meta(tx, key.as_bytes(), &request)?;
    super::native_keys::put_meta(tx, b"facts-write-inflight", &request)?;
    tx.check()
}
pub(super) fn facts_complete(
    tx: &mut PgTxn<'_>,
    options: &crate::Options,
    intent: &crate::source_change::FactsIntent,
    actual: &crate::Facts,
    pids: &[(u32, String)],
) -> Result<(), StorageError> {
    let request = facts_request(tx, intent)?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    let ack = format!("facts-write-ack:{}", intent.operation);
    if let Some(prior) = super::native_keys::read_meta(tx, ack.as_bytes(), 1048576)? {
        return if prior == request {
            Ok(())
        } else {
            Err(StorageError::Conflict)
        };
    }
    if actual != &intent.wanted
        || model_cache::generation(tx)? != intent.head
        || super::native_keys::read_meta(
            tx,
            format!("facts-write-intent:{}", intent.operation).as_bytes(),
            1048576,
        )?
        .as_ref()
            != Some(&request)
        || super::native_keys::read_meta(tx, b"facts-write-inflight", 1048576)?.as_ref()
            != Some(&request)
    {
        return Err(StorageError::Conflict);
    }
    let changes = crate::facts::changes(&intent.before, &intent.wanted);
    let mut affected = BTreeSet::new();
    for (harness, id) in &changes.native {
        affected.extend(dependent_paths(tx, harness, id)?);
    }
    for pid in &changes.pids {
        affected.extend(dependent_paths(tx, "runtime:pid", &pid.to_string())?);
    }
    for (pid, id) in pids {
        if changes.pids.contains(pid) {
            affected.extend(dependent_paths(tx, "claude", id)?);
        }
    }
    if let Some(generation) = model_cache::generation(tx)? {
        let selected = |facts: &crate::Facts| {
            facts
                .codex_rollouts
                .iter()
                .flatten()
                .chain(facts.codex_provisional_rollouts.iter().flatten())
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        let old = selected(&intent.before);
        let new = selected(&intent.wanted);
        let every =
            intent.before.codex_rollouts.is_none() != intent.wanted.codex_rollouts.is_none();
        // A global or whole-selection change affects every (Codex) Current
        // Source. It invalidates them by keyset pages of 64, each checking the
        // deadline, so catch-up grows with the Catalog instead of refusing.
        // Otherwise only the changed rollout paths use the Source path index.
        let targets = if changes.global || every {
            None
        } else {
            Some(
                old.symmetric_difference(&new)
                    .map(|relative| {
                        options
                            .codex_home
                            .join(relative)
                            .to_string_lossy()
                            .into_owned()
                            .into_bytes()
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let mut after: Option<Vec<u8>> = None;
        loop {
            tx.check()?;
            let rows=tx.driver.query("SELECT DISTINCT source_path FROM semon_pg_native_catalog_sources WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND ($3::bytea[] IS NULL OR source_path=ANY($3::bytea[])) AND (NOT $4 OR key_digest IN (SELECT key_digest FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND harness=$5)) AND ($6::bytea IS NULL OR source_path>$6) ORDER BY source_path LIMIT 64", &[&ns,&generation.as_bytes(),&targets,&(every&&!changes.global),&b"codex".as_slice(),&after]).map_err(sqlstate::error)?;
            let Some(last) = rows.last() else { break };
            after = Some(last.get(0));
            for row in &rows {
                let path = String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?;
                let codex = std::path::Path::new(&path)
                    .strip_prefix(&options.codex_home)
                    .ok()
                    .and_then(|p| p.to_str());
                if (changes.global || (every && codex.is_some()) || targets.is_some())
                    && !affected.contains(&path)
                {
                    super::source_journal::invalidate(tx, &path)?;
                }
            }
        }
        for relative in old.symmetric_difference(&new) {
            if crate::is_input_path("codex", relative) {
                affected.insert(
                    options
                        .codex_home
                        .join(relative)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    // A repository change invalidates that cwd's Sources by the same pages.
    for cwd in &changes.repos {
        let mut after: Option<Vec<u8>> = None;
        loop {
            tx.check()?;
            let rows=tx.driver.query("SELECT path FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND cwd=$2 AND ($3::bytea IS NULL OR path>$3) ORDER BY path LIMIT 64", &[&ns,&cwd.as_bytes(),&after]).map_err(sqlstate::error)?;
            let Some(last) = rows.last() else { break };
            after = Some(last.get(0));
            for row in &rows {
                let path = String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?;
                if !affected.contains(&path) {
                    super::source_journal::invalidate(tx, &path)?;
                }
            }
        }
    }
    if affected.len() > 64 {
        return Err(StorageError::Busy);
    }
    for path in affected {
        super::source_journal::invalidate(tx, &path)?;
    }
    // Completion promises durable Facts and catch-up work, never NativeReady.
    super::native_keys::put_meta(tx, ack.as_bytes(), &request)?;
    super::native_keys::delete_meta(tx, b"facts-write-inflight")?;
    tx.check()
}
