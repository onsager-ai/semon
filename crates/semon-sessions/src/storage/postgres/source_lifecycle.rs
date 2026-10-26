//! Ordinary source component discovery over one physical Current snapshot.
use super::{model_cache, sqlstate, txn::PgTxn};
use crate::{
    model::{
        relation_stage::{self, LifecycleStore},
        summary::CatalogRow,
    },
    storage::StorageError,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

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
    let ns = tx.namespace.as_str().as_bytes();
    let values=tx.driver.query("SELECT session_key,CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 ORDER BY session_key LIMIT 65", &[&ns,&generation.as_bytes()]).map_err(sqlstate::error)?;
    if values.len() > 64 {
        return Err(StorageError::Busy);
    }
    let mut rows = BTreeMap::new();
    let mut used = 0usize;
    for value in values {
        let key: Vec<u8> = value.get(0);
        let bytes: Vec<u8> = value
            .get::<_, Option<Vec<u8>>>(1)
            .ok_or(StorageError::Busy)?;
        used = used.saturating_add(bytes.len());
        if used > 1048576 {
            return Err(StorageError::Busy);
        }
        let row: CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        if row.key.as_bytes() != key {
            return Err(StorageError::ScopeChanged);
        }
        rows.insert(row.key.clone(), row);
    }
    struct Lookup {
        rows: BTreeMap<String, CatalogRow>,
        links: BTreeMap<String, BTreeSet<String>>,
    }
    impl LifecycleStore for Lookup {
        fn path_owners(&self, path: &str) -> io::Result<Vec<String>> {
            Ok(self
                .rows
                .values()
                .filter(|r| r.sources.iter().any(|s| s.path.to_str() == Some(path)))
                .map(|r| r.key.clone())
                .collect())
        }
        fn owner(&self, key: &str) -> io::Result<CatalogRow> {
            self.rows
                .get(key)
                .cloned()
                .ok_or_else(|| io::Error::other("missing Current source owner"))
        }
        fn neighbors(&self, key: &str) -> io::Result<Vec<String>> {
            Ok(self.links.get(key).into_iter().flatten().cloned().collect())
        }
    }
    let mut links: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut connect = |a: &str, b: &str| {
        if a != b && rows.contains_key(a) && rows.contains_key(b) {
            links.entry(a.to_owned()).or_default().insert(b.to_owned());
            links.entry(b.to_owned()).or_default().insert(a.to_owned());
        }
    };
    for row in rows.values() {
        if let Some(parent) = &row.parent {
            connect(&row.key, parent);
        }
        for other in rows.values() {
            if row.harness == other.harness
                && row
                    .native_ids
                    .iter()
                    .any(|id| other.native_ids.contains(id))
            {
                connect(&row.key, &other.key);
            }
        }
        let edges = row
            .query
            .as_ref()
            .and_then(|q| q["selected"]["edges"].as_array())
            .ok_or(StorageError::NamespaceUpgradePending)?;
        for edge in edges {
            for field in ["from", "to"] {
                if let Some(owner) = edge["query"][field].as_str() {
                    connect(&row.key, owner);
                }
            }
        }
    }
    let mut seeds = paths.to_vec();
    for row in rows.values() {
        if keys.contains(&row.key)
            || native
                .iter()
                .any(|(h, id)| row.harness == *h && row.native_ids.contains(id))
        {
            seeds.extend(
                row.sources
                    .iter()
                    .map(|s| s.path.to_string_lossy().into_owned()),
            );
        }
    }
    let lookup = Lookup { rows, links };
    let owners =
        relation_stage::affected_owners(&lookup, &seeds).map_err(|_| StorageError::Busy)?;
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
        let owners=tx.driver.query("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 65", &[&ns,&generation.as_bytes()]).map_err(sqlstate::error)?;
        if owners.len() > 64 {
            return Err(StorageError::Busy);
        }
        let mut used = 0usize;
        for owner in owners {
            let bytes: Vec<u8> = owner
                .get::<_, Option<Vec<u8>>>(0)
                .ok_or(StorageError::Busy)?;
            used = used.saturating_add(bytes.len());
            if used > 1048576 {
                return Err(StorageError::Busy);
            }
            let owner: CatalogRow =
                serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
            if owner.harness == harness && owner.native_ids.iter().any(|n| n == id) {
                affected.extend(
                    owner
                        .sources
                        .iter()
                        .map(|s| s.path.to_string_lossy().into_owned()),
                );
            }
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
    let rows=tx.driver.query("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 LIMIT 65", &[&ns,&generation.as_bytes()]).map_err(sqlstate::error)?;
    if rows.len() > 64 {
        return Err(StorageError::Busy);
    }
    let mut candidates = BTreeSet::new();
    let mut used = 0usize;
    for row in rows {
        let bytes = row.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?;
        used = used.saturating_add(bytes.len());
        if used > 1048576 {
            return Err(StorageError::Busy);
        }
        let row: CatalogRow =
            serde_json::from_slice(&bytes).map_err(|_| StorageError::ScopeChanged)?;
        for source in row.sources {
            let Some(old) = source.path.to_str() else {
                return Err(StorageError::ScopeChanged);
            };
            if row.harness == "codex"
                && source.native_id == id
                && old != path
                && relocation_namespace(old).is_some_and(|(r, a)| r == root && a != archived)
            {
                candidates.insert(old.to_owned());
            }
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
    let pending: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2)", &[&ns, &path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if pending { return Err(StorageError::Conflict); }
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
    tx.driver.execute("DELETE FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND reason='removed'", &[&ns,&old.as_bytes()]).map_err(sqlstate::error)?;
    super::native_keys::put_meta(tx, key.as_bytes(), &request)?;
    tx.check()?;
    Ok(true)
}

#[derive(serde::Serialize,serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIntent { request:Vec<u8>, revision:i64, previous:Option<crate::source_change::AppendEvidence> }
fn raw_request(tx:&PgTxn<'_>,input:&crate::Input,intent:&crate::source_change::WriteIntent)->Result<Vec<u8>,StorageError> {
    if intent.operation.is_empty() || intent.operation.len()>512 || intent.root!=input.root.as_str() || intent.path.len()>4096 {return Err(StorageError::InvalidArguments);}
    let bytes=serde_json::to_vec(&("raw-source-write-v1",tx.namespace.as_str(),input.root.as_str(),&input.path,intent)).map_err(|_|StorageError::InvalidArguments)?;
    if bytes.len()>16384 {return Err(StorageError::Busy);} Ok(bytes)
}
fn raw_intent_key(intent:&crate::source_change::WriteIntent)->String {format!("raw-write-intent:{}",intent.operation)}
fn raw_ack_key(intent:&crate::source_change::WriteIntent)->String {format!("raw-write-ack:{}",intent.operation)}

pub(super) fn write_begin(tx:&mut PgTxn<'_>,options:&crate::Options,input:&crate::Input,intent:&crate::source_change::WriteIntent)->Result<(i64,Option<crate::source_change::AppendEvidence>),StorageError> {
    let request=raw_request(tx,input,intent)?;
    if input.full_path(options).to_str()!=Some(intent.path.as_str()) {return Err(StorageError::InvalidArguments);}
    let ns=tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if let Some(bytes)=super::native_keys::read_meta(tx,raw_intent_key(intent).as_bytes(),32768)? {
        let prior:RawIntent=serde_json::from_slice(&bytes).map_err(|_|StorageError::ScopeChanged)?;
        if prior.request!=request {return Err(StorageError::Conflict);}return Ok((prior.revision,prior.previous));
    }
    if model_cache::generation(tx)?!=intent.head || super::native_reader::ledger(tx,&intent.path)?.map(|(_,l)|l)!=intent.ledger {return Err(StorageError::Conflict);}
    let actual=crate::events::Stat::of(&input.full_path(options)).ok();
    if actual!=intent.before {return Err(StorageError::Conflict);}
    if let Some(before)=&intent.before {
        if Some(crate::source_change::bytes_hash(&input.full_path(options),before.size).map_err(|_|StorageError::ScopeChanged)?)!=intent.before_hash {return Err(StorageError::Conflict);}
    } else if intent.before_hash.is_some() {return Err(StorageError::ScopeChanged);}
    let inflight=format!("raw-write-inflight:{}",intent.path);
    if super::native_keys::read_meta(tx,inflight.as_bytes(),16384)?.is_some() {return Err(StorageError::Conflict);}
    let previous=model_cache::append_evidence(tx,&intent.path)?;
    // Existing append evidence can be continued; unknown pending evidence may
    // not be overwritten to manufacture a fresh source write.
    let mutation:bool=tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_source_mutations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2)", &[&ns,&intent.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if mutation && previous.is_none() {return Err(StorageError::Conflict);}
    super::source_journal::invalidate(tx,&intent.path)?;
    let revision:i64=tx.driver.query_one("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&intent.path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if input.root==crate::InputRoot::Claude && input.path.starts_with("sessions/") {
        if let Some(facts)=options.facts.as_ref().map(|p|crate::read_facts(p)).transpose().map_err(|_|StorageError::ScopeChanged)? {
            let evidence=crate::source_change::RuntimeInputEvidence{unaffected:crate::native_evidence::recorded_runtime_fingerprint_excluding(options,&facts,Some(std::path::Path::new(&intent.path))).map_err(|_|StorageError::ScopeChanged)?};
            let bytes=serde_json::to_vec(&evidence).map_err(|_|StorageError::Unavailable)?;
            tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace", &[&ns,&intent.path.as_bytes(),&bytes]).map_err(sqlstate::error)?;
        }
    }
    let state=RawIntent{request:request.clone(),revision,previous:previous.clone()};
    super::native_keys::put_meta(tx,raw_intent_key(intent).as_bytes(),&serde_json::to_vec(&state).map_err(|_|StorageError::Unavailable)?)?;
    super::native_keys::put_meta(tx,inflight.as_bytes(),&request)?;
    tx.check()?;Ok((revision,previous))
}

pub(super) fn write_complete(tx:&mut PgTxn<'_>,options:&crate::Options,input:&crate::Input,intent:&crate::source_change::WriteIntent,revision:i64,completion:&crate::source_change::WriteCompletion)->Result<(),StorageError> {
    use sha2::{Digest,Sha256};
    let request=raw_request(tx,input,intent)?;
    let ack=serde_json::to_vec(&(&request,revision,completion)).map_err(|_|StorageError::InvalidArguments)?;
    if ack.len()>32768 {return Err(StorageError::Busy);}
    let ns=tx.namespace.as_str().as_bytes().to_vec();
    tx.driver.query_one("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if let Some(prior)=super::native_keys::read_meta(tx,raw_ack_key(intent).as_bytes(),32768)? {return if prior==ack {Ok(())} else {Err(StorageError::Conflict)};}
    let stored=super::native_keys::read_meta(tx,raw_intent_key(intent).as_bytes(),32768)?.ok_or(StorageError::ScopeChanged)?;
    let state:RawIntent=serde_json::from_slice(&stored).map_err(|_|StorageError::ScopeChanged)?;
    let inflight=format!("raw-write-inflight:{}",intent.path);
    if state.request!=request || state.revision!=revision || super::native_keys::read_meta(tx,inflight.as_bytes(),16384)?.as_ref()!=Some(&request) || model_cache::generation(tx)?!=intent.head || super::native_reader::ledger(tx,&intent.path)?.map(|(_,l)|l)!=intent.ledger {return Err(StorageError::Conflict);}
    let claims=tx.driver.query("SELECT revision FROM semon_pg_source_invalidations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2", &[&ns,&intent.path.as_bytes()]).map_err(sqlstate::error)?;
    if claims.first().map(|r|r.get::<_,i64>(0))!=Some(revision) {return Err(StorageError::Conflict);}
    let path=input.full_path(options);
    if crate::events::Stat::of(&path).ok()!=Some(completion.after) || completion.after.size!=completion.to || crate::source_change::bytes_hash(&path,completion.to).map_err(|_|StorageError::ScopeChanged)?!=completion.hash {return Err(StorageError::ScopeChanged);}
    if let Some(evidence)=&completion.evidence {
        let before=intent.before.ok_or(StorageError::ScopeChanged)?;
        if !evidence.qualifies(&evidence.before,&completion.after) || intent.generation.as_deref()!=Some(evidence.generation.as_str()) || before.size!=intent.from || crate::source_change::bytes_hash(&path,before.size).map_err(|_|StorageError::ScopeChanged)?!=intent.before_hash.ok_or(StorageError::ScopeChanged)? {return Err(StorageError::ScopeChanged);}
    }
    let runtime=crate::native_evidence::query_runtime_generation(options).map_err(|_|StorageError::ScopeChanged)?;
    let facts=options.facts.as_ref().map(|p| {
        let facts=crate::read_facts(p).map_err(|_|StorageError::ScopeChanged)?;
        Ok::<_,StorageError>(format!("{:x}",Sha256::digest(serde_json::to_vec(&facts).map_err(|_|StorageError::Unavailable)?)))
    }).transpose()?;
    if runtime!=completion.runtime || facts!=completion.facts {return Err(StorageError::Conflict);}
    resume_runtime(tx,options,&intent.path)?;
    if let Some(evidence)=&completion.evidence {
        let bytes=serde_json::to_vec(evidence).map_err(|_|StorageError::Unavailable)?;
        tx.driver.execute("INSERT INTO semon_pg_native_source_mutations(namespace,source_path,evidence) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path) DO UPDATE SET evidence=excluded.evidence WHERE semon_pg_native_source_mutations.namespace=excluded.namespace", &[&ns,&intent.path.as_bytes(),&bytes]).map_err(sqlstate::error)?;
    }
    // Metadata records the qualified interpretation; it never contains Raw body.
    super::native_keys::put_meta(tx,b"raw-write-runtime",completion.runtime.as_bytes())?;
    if let Some(facts)=&completion.facts {super::native_keys::put_meta(tx,b"raw-write-facts",facts.as_bytes())?;}
    super::native_keys::put_meta(tx,raw_ack_key(intent).as_bytes(),&ack)?;
    super::native_keys::delete_meta(tx,inflight.as_bytes())?;
    tx.check()?;Ok(())
}
