//! Ordinary Catalog renderer adapter over one acknowledged immutable generation.
use super::super::observations::{self, Origin};
use super::*;
use crate::storage::source_observations::CatalogSearchLookup;
use crate::{
    catalog::Request,
    storage::catalog::{CatalogObservationOwner, CatalogSnapshot, SelectedCatalogRow},
};
use std::io;

fn failed(error: StorageError) -> io::Error {
    io::Error::other(error)
}
fn unavailable<T>() -> io::Result<T> {
    Err(failed(StorageError::NamespaceUpgradePending))
}

impl Snapshot<'_, '_> {
    pub(super) fn row_runtime_binding_with_observations(
        &self,
        row: &CatalogRow,
        owner: Option<CatalogObservationOwner>,
    ) -> io::Result<
        Option<(
            crate::storage::QueryRuntimeBinding,
            Option<crate::storage::selected::PresentationObservations>,
        )>,
    > {
        self.row_observation(row, owner)
            .map(|value| value.map(|(binding, observations, _)| (binding, observations)))
    }

    pub(in crate::storage::postgres::catalog_effects) fn row_observation(
        &self,
        row: &CatalogRow,
        owner: Option<CatalogObservationOwner>,
    ) -> io::Result<
        Option<(
            crate::storage::QueryRuntimeBinding,
            Option<crate::storage::selected::PresentationObservations>,
            Origin,
        )>,
    > {
        self.require_current()?;
        // Runtime callers supply the physical owner. Identity-only callers
        // retain their separate original-publication lookup contract.
        let physical = owner
            .map(|owner| {
                let stored = self
                    .catalog(&row.key, owner == CatalogObservationOwner::History)
                    .map_err(failed)?
                    .ok_or_else(|| failed(StorageError::ScopeChanged))?;
                let mut selected = row.clone();
                selected.lifecycle = stored.lifecycle;
                if serde_json::to_value(&selected).map_err(io::Error::other)?
                    != serde_json::to_value(&stored).map_err(io::Error::other)?
                {
                    return Err(failed(StorageError::ScopeChanged));
                }
                Ok(stored)
            })
            .transpose()?;
        let physical_metadata = physical.as_ref().map(encoded).transpose().map_err(failed)?;
        let history_owner = if owner == Some(CatalogObservationOwner::History) {
            if !self.history_qualified(row)? {
                return Err(failed(StorageError::ScopeChanged));
            }
            Some(
                self.owner(&row.key)
                    .map_err(failed)?
                    .ok_or_else(|| failed(StorageError::ScopeChanged))?,
            )
        } else {
            None
        };
        let mut tx = self
            .tx
            .lock()
            .map_err(|_| failed(StorageError::Unavailable))?;
        let current = super::super::super::model_cache::binding(&mut tx).map_err(failed)?;
        if let Some(history_owner) = history_owner {
            let metadata = physical_metadata
                .as_ref()
                .ok_or_else(|| failed(StorageError::ScopeChanged))?;
            let key = observations::history_key(&history_owner, metadata);
            if let Some(reference) =
                super::super::super::native_keys::read_meta(&mut tx, key.as_bytes(), 512)
                    .map_err(failed)?
            {
                let reference: observations::HistoryReference =
                    serde_json::from_slice(&reference).map_err(io::Error::other)?;
                let acknowledged: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2)", &[&tx.namespace.as_str().as_bytes(),&history_owner.as_bytes()]).map_err(sqlstate::error).map_err(failed)?.get(0);
                if reference.version != 1 || !acknowledged {
                    return Err(failed(StorageError::ScopeChanged));
                }
                let mut original = physical
                    .as_ref()
                    .ok_or_else(|| failed(StorageError::ScopeChanged))?
                    .clone();
                original.lifecycle = reference.lifecycle;
                let observations = observations::publication(
                    &mut tx,
                    &current,
                    &encoded(&original).map_err(failed)?,
                    &reference.receipt,
                )
                .map_err(failed)?;
                return Ok(Some((
                    current,
                    observations,
                    Origin::Native(reference.receipt),
                )));
            }
        }
        let ns = current.namespace.as_str().as_bytes();
        let grant = format!("{}:{}", current.admission, current.stream_epoch);
        let mut metadata = row.clone();
        metadata.lifecycle = crate::model::summary::CatalogLifecycle::Current;
        let current_metadata = serde_json::to_vec(&metadata).map_err(io::Error::other)?;
        metadata.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
        let retained_metadata = serde_json::to_vec(&metadata).map_err(io::Error::other)?;
        // Native publication grants belong to Catalog. History requires its
        // own immutable reference and cannot borrow a matching Catalog grant.
        let originals: Vec<&Vec<u8>> = match owner {
            Some(CatalogObservationOwner::Catalog) => {
                physical_metadata.as_ref().into_iter().collect()
            }
            Some(CatalogObservationOwner::History) => Vec::new(),
            None => vec![&current_metadata, &retained_metadata],
        };
        for original in originals {
            if owner.is_none()
                && row.lifecycle == crate::model::summary::CatalogLifecycle::Current
                && original == &retained_metadata
            {
                continue;
            }
            let key = format!("catalog-native-observation:{:x}", Sha256::digest(original));
            let Some(native) =
                super::super::super::native_keys::read_meta(&mut tx, key.as_bytes(), 256)
                    .map_err(failed)?
            else {
                continue;
            };
            let receipt: observations::NativeReceipt =
                serde_json::from_slice(&native).map_err(io::Error::other)?;
            let observations =
                observations::publication(&mut tx, &current, original, &receipt).map_err(failed)?;
            return Ok(Some((current, observations, Origin::Native(receipt))));
        }
        let reference_kind = owner.map(|owner| match owner {
            CatalogObservationOwner::Catalog => b"current".as_slice(),
            CatalogObservationOwner::History => b"history".as_slice(),
        });
        // Physical ownership was checked above. The proof decoder permits only
        // the original Current row's lifecycle retag to Retained.
        let first_metadata = &current_metadata;
        let second_metadata = &retained_metadata;
        let rows = tx.driver.query("SELECT DISTINCT g.id,g.publication,g.proof,g.admission,g.stream,CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,g.prefix,r.base,CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END,CASE WHEN octet_length(v.metadata)<=1048576 THEN v.metadata END,EXISTS(SELECT 1 FROM semon_pg_native_meta m JOIN semon_pg_native_meta a ON a.namespace_digest=m.namespace_digest AND a.namespace=m.namespace AND a.key_digest=sha256('native_transfer_v1:catalog-owner:'::bytea||m.value) AND a.key='native_transfer_v1:catalog-owner:'::bytea||m.value WHERE m.namespace_digest=f.namespace_digest AND m.namespace=f.namespace AND m.key_digest=sha256('native_transfer_v1:publication:'::bytea||g.publication) AND m.key='native_transfer_v1:publication:'::bytea||g.publication AND a.value=$3) FROM semon_pg_native_selected_refs f JOIN semon_pg_native_selected_generations g USING(namespace_digest,namespace,id,key_digest,session_key) JOIN semon_pg_native_semantic_publications p ON p.namespace_digest=g.namespace_digest AND p.namespace=g.namespace AND p.generation=g.publication AND p.proof_digest=g.proof AND p.admission=g.admission JOIN semon_pg_native_selected_roots r ON r.namespace_digest=p.namespace_digest AND r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest JOIN semon_pg_native_selected_presentations v ON v.namespace_digest=g.namespace_digest AND v.namespace=g.namespace AND v.id=g.id AND v.key_digest=g.key_digest AND v.session_key=g.session_key WHERE f.namespace_digest=sha256($1::bytea) AND f.namespace=$1 AND f.key_digest=sha256($2::bytea) AND f.session_key=$2 AND g.metadata IN ($4,$5) AND ($6::bytea IS NULL OR f.kind=$6) ORDER BY g.id LIMIT 1", &[&ns,&row.key.as_bytes(),&grant.as_bytes(),first_metadata,second_metadata,&reference_kind]).map_err(sqlstate::error).map_err(failed)?;
        let Some(value) = rows.into_iter().next() else {
            return Err(failed(StorageError::ScopeChanged));
        };
        let decode = |at: usize| -> io::Result<String> {
            text(
                value
                    .get::<_, Option<Vec<u8>>>(at)
                    .ok_or_else(|| failed(StorageError::Busy))?,
            )
            .map_err(failed)
        };
        let record = crate::storage::proof_codec::GenerationRecord {
            namespace: current.namespace.as_str().to_owned(),
            id: crate::storage::proof_lifetime::ProofGenerationId(
                text(value.get(0)).map_err(failed)?,
            ),
            session_key: row.key.clone(),
            publication: text(value.get(1)).map_err(failed)?,
            proof: text(value.get(2)).map_err(failed)?,
            admission: u64::try_from(value.get::<_, i64>(3))
                .map_err(|_| failed(StorageError::ScopeChanged))?,
            stream: text(value.get(4)).map_err(failed)?,
            metadata: decode(5)?,
            slots: decode(6)?,
            prefix: text(value.get(7)).map_err(failed)?,
        };
        if !value.get::<_, bool>(11) {
            return Err(failed(StorageError::ScopeChanged));
        }
        let manifest = decode(9)?;
        let presentation = decode(10)?;
        let base = text(value.get(8)).map_err(failed)?;
        crate::selected_read::charge(
            64,
            record.metadata.len() + record.slots.len() + manifest.len() + presentation.len(),
        )?;
        let observations = crate::storage::proof_codec::verify_presentation_observations(
            &row.key,
            &record.metadata,
            &record.slots,
            &presentation,
            &base,
            &manifest,
            &record.proof,
        )
        .map_err(failed)?;
        let id = record.id.clone();
        let binding = crate::storage::proof_codec::observation_binding(
            record,
            &current,
            physical.as_ref().unwrap_or(row),
        )
        .map_err(failed)?;
        Ok(Some((binding, observations, Origin::Imported(id))))
    }
}

impl CatalogSnapshot for Snapshot<'_, '_> {
    fn runtime_observation(
        &self,
        options: &crate::Options,
        row: &CatalogRow,
        owner: CatalogObservationOwner,
        runtime: &str,
    ) -> io::Result<Option<String>> {
        let Some((binding, Some(observations))) =
            self.row_runtime_binding_with_observations(row, Some(owner))?
        else {
            return Err(failed(StorageError::ScopeChanged));
        };
        let captured = observations
            .runtime_for(row)
            .ok_or_else(|| failed(StorageError::ScopeChanged))?;
        let mut tx = self
            .tx
            .lock()
            .map_err(|_| failed(StorageError::Unavailable))?;
        match super::super::super::native_reader::runtime_source_cwds_if_current(
            &mut tx,
            &row.sources,
        )
        .map_err(failed)?
        {
            Some(cwds) => {
                let scoped = if options.facts.is_some() {
                    super::super::super::current_sources::runtime_scope(
                        &mut tx, options, row, runtime, &cwds,
                    )
                    .map_err(failed)?
                } else {
                    self.runtime_local
                        .get_or_init(|| crate::native_evidence::RuntimeObservations::new(options))
                        .scope(row, &cwds)?
                };
                let fresh = binding.witness(&scoped)?;
                if fresh != captured {
                    return Err(failed(StorageError::ScopeChanged));
                }
                Ok(Some(fresh))
            }
            None => {
                // `runtime` is the live raw global input. The saved global
                // witness is already qualified by this publication's binding.
                let current_global = binding.witness(runtime)?;
                if observations.runtime.as_deref() != Some(current_global.as_str()) {
                    return Err(failed(StorageError::ScopeChanged));
                }
                Ok(Some(captured))
            }
        }
    }
    fn runtime_generation(&self, options: &crate::Options) -> io::Result<String> {
        super::super::super::current_sources::runtime_in_snapshot(self, options).map_err(failed)
    }
    fn facts_authority<'a>(
        &'a self,
        path: &std::path::Path,
    ) -> io::Result<Box<dyn crate::facts::source_authority::SourceAuthority + 'a>> {
        super::super::super::current_sources::in_snapshot(self, path).map_err(failed)
    }
    fn runtime_binding(&self) -> Option<crate::storage::QueryRuntimeBinding> {
        let mut tx = self.tx.lock().ok()?;
        super::super::super::model_cache::binding(&mut tx).ok()
    }
    fn row_runtime_binding(
        &self,
        row: &CatalogRow,
    ) -> io::Result<Option<crate::storage::QueryRuntimeBinding>> {
        self.row_runtime_binding_with_observations(row, None)
            .map(|result| result.map(|(binding, _)| binding))
    }

    fn require_current(&self) -> io::Result<()> {
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        tx.check().map_err(failed)?;
        let admitted: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_operations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2)", &[&tx.namespace.as_str().as_bytes(), &self.generation.as_bytes()]).map_err(sqlstate::error).map_err(failed)?.get(0);
        if admitted { Ok(()) } else { unavailable() }
    }
    fn publication_value(&self, key: &str) -> io::Result<Option<String>> {
        self.require_current()?;
        let value = if key == "catalog_backfill_state" {
            let tx = self
                .tx
                .lock()
                .map_err(|_| io::Error::other(StorageError::Unavailable))?;
            tx.check().map_err(failed)?;
            let rows = tx
                .driver
                .query(
                    "SELECT backfill_state FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1",
                    &[&tx.namespace.as_str().as_bytes()],
                )
                .map_err(sqlstate::error)
                .map_err(failed)?;
            rows.first()
                .map(|row| {
                    row.try_get::<_, Option<String>>(0)
                        .map_err(|_| failed(StorageError::ScopeChanged))
                })
                .transpose()?
                .flatten()
        } else {
            match key {
                "catalog_generation" => Some(self.generation.clone()),
                "catalog_version" => Some(crate::model::summary::CATALOG_VERSION.to_string()),
                // Completeness is not inferred from successful metadata staging.
                "catalog_completeness" => Some("partial".into()),
                // Search counts/documents/memberships are effects of the same
                // admitted generation operation; carried owners included.
                "search_version" => Some(crate::catalog_search::VERSION.to_string()),
                "search_generation_current" | "search_generation_retained_history" => {
                    Some(self.generation.clone())
                }
                _ => None,
            }
        };
        Ok(value)
    }
    fn catalog_search_paths(
        &self,
        lookup: &CatalogSearchLookup<'_>,
        limit: usize,
    ) -> io::Result<Vec<String>> {
        self.require_current()?;
        let mut tx = self
            .tx
            .lock()
            .map_err(|_| failed(StorageError::Unavailable))?;
        super::catalog_search_paths(&mut tx, lookup, limit).map_err(failed)
    }
    fn rows(&self, request: &Request) -> io::Result<Vec<SelectedCatalogRow>> {
        self.require_current()?;
        if request.limit > 64 {
            return unavailable();
        }
        // Retained lists use the latest Current row's indexed fields when it
        // exists, matching SQLite's Catalog index. History-only keys fall
        // back to their proof-owned header. The shared renderer overlays the
        // immutable History source context for same-key rows.
        let history = request.scope == crate::CatalogReadScope::RetainedHistory;
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        let ns = tx.namespace.as_str().as_bytes();
        // Filter and keyset BEFORE LIMIT. Metadata stays opaque bytes: native
        // keys can contain NUL, which PostgreSQL JSON/text cannot represent.
        let sid = request.sid.as_ref().map(|v| v.as_bytes());
        let harness = request.harness.as_ref().map(|v| v.as_bytes());
        let repo = request.repo.as_ref().map(|v| v.as_bytes());
        let model = request.model.as_ref().map(|v| v.as_bytes());
        let cursor_last = request.cursor.as_ref().map(|v| v.last);
        let cursor_key = request.cursor.as_ref().map(|v| v.key.as_bytes());
        let inclusive = request.cursor.as_ref().is_some_and(|v| v.inclusive);
        let limit = request.limit as i64 + 1;
        let rows = tx
            .driver
            .query(
                "WITH visible AS (
                SELECT session_key,metadata,last_ms,harness,repo,model,false AS retained
                FROM semon_pg_native_catalog_rows r
                WHERE r.namespace_digest=sha256($1::bytea) AND r.namespace=$1 AND r.generation=$2
                UNION ALL
                SELECT o.session_key,h.metadata,o.last_ms,o.harness,o.repo,o.model,true
                FROM semon_pg_native_catalog_history_owners o
                JOIN semon_pg_native_catalog_history_headers h
                  ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace
                 AND h.generation=o.owner_generation AND h.key_digest=o.key_digest
                 AND h.session_key=o.session_key
                WHERE $11 AND o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2
                  AND NOT EXISTS (
                    SELECT 1 FROM semon_pg_native_catalog_rows r
                    WHERE r.namespace_digest=o.namespace_digest AND r.namespace=o.namespace
                      AND r.generation=o.generation AND r.key_digest=o.key_digest
                      AND r.session_key=o.session_key)
             ), page AS MATERIALIZED (
                SELECT session_key,metadata,last_ms,harness,repo,model,retained
                FROM visible
                WHERE ($3::bytea IS NULL OR session_key=$3)
                  AND ($4::bytea IS NULL OR harness=$4)
                  AND ($5::bytea IS NULL OR repo=$5)
                  AND ($6::bytea IS NULL OR model=$6)
                  AND ($7::bigint IS NULL OR last_ms<$7
                       OR (last_ms=$7 AND (session_key>$8 OR ($9 AND session_key=$8))))
                ORDER BY last_ms DESC,session_key LIMIT $10
             ) SELECT session_key,
                 CASE WHEN sum(octet_length(metadata)) OVER ()<=1048576 THEN metadata END,
                 last_ms,harness,repo,model,retained FROM page ORDER BY last_ms DESC,session_key",
                &[
                    &ns,
                    &self.generation.as_bytes(),
                    &sid,
                    &harness,
                    &repo,
                    &model,
                    &cursor_last,
                    &cursor_key,
                    &inclusive,
                    &limit,
                    &history,
                ],
            )
            .map_err(sqlstate::error)
            .map_err(failed)?;
        let mut output = Vec::new();
        for row in rows {
            let key: Vec<u8> = row.get(0);
            let bytes: Vec<u8> = row
                .get::<_, Option<Vec<u8>>>(1)
                .ok_or_else(|| failed(StorageError::Busy))?;
            crate::selected_read::charge(64, bytes.len())?;
            let mut value: CatalogRow = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if value.key.as_bytes() != key
                || value.sources.len() > 64
                || value.last.unwrap_or(0) != row.get::<_, i64>(2)
                || value.harness.as_bytes() != row.get::<_, Vec<u8>>(3)
                || value.repo.as_deref().unwrap_or("").as_bytes() != row.get::<_, Vec<u8>>(4)
                || value.model.as_bytes() != row.get::<_, Vec<u8>>(5)
            {
                return Err(failed(StorageError::ScopeChanged));
            }
            if row.get::<_, bool>(6) {
                value.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
            }
            output.push(SelectedCatalogRow {
                row: value,
                owner: Some(if row.get::<_, bool>(6) {
                    CatalogObservationOwner::History
                } else {
                    CatalogObservationOwner::Catalog
                }),
            });
        }
        tx.check().map_err(failed)?;
        Ok(output)
    }
    fn search(
        &self,
        query: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection> {
        use crate::catalog_search::{Candidate, Position};
        self.require_current()?;
        let tx = self
            .tx
            .lock()
            .map_err(|_| io::Error::other(StorageError::Unavailable))?;
        let ns = tx.namespace.as_str().as_bytes();
        let generation = self.generation.as_bytes();
        let history = query.scope == crate::CatalogReadScope::RetainedHistory;
        let scope = crate::catalog_search::scope_key(query.scope).as_bytes();
        let run = |sql: &str, args: &[&(dyn tokio_postgres::types::ToSql + Sync)]| {
            tx.driver
                .query(sql, args)
                .map_err(sqlstate::error)
                .map_err(failed)
        };
        // Every history owner carries its retained search document; a missing
        // one is a broken projection, refused rather than read as a miss.
        if history {
            let missing = run(
                "SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND NOT EXISTS (SELECT 1 FROM semon_pg_native_catalog_search_documents d WHERE d.namespace_digest=o.namespace_digest AND d.namespace=o.namespace AND d.generation=$2 AND d.scope=$3 AND d.key_digest=o.key_digest))",
                &[&ns, &generation, &scope],
            )?;
            if missing.first().is_none_or(|row| row.get::<_, bool>(0)) {
                return unavailable();
            }
        }
        let selection = crate::catalog_search::select(
            query,
            |text| {
                crate::catalog_search::anchor_with(text, |gram| {
                    let rows = run(
                        "SELECT members FROM semon_pg_native_catalog_search_counts WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND scope=$3 AND gram=$4",
                        &[&ns, &generation, &scope, &gram.as_bytes()],
                    )?;
                    Ok(rows.first().map(|row| row.get::<_, i64>(0)))
                })
            },
            // Memberships carry no scope; the scope document decides presence.
            |gram, after: Option<&Position>, limit| {
                let rows = run(
                    "SELECT m.last_ms,m.session_key FROM semon_pg_native_catalog_search_memberships m
                     WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.generation=$2 AND m.gram=$4
                       AND EXISTS (SELECT 1 FROM semon_pg_native_catalog_search_documents d
                         WHERE d.namespace_digest=m.namespace_digest AND d.namespace=m.namespace
                           AND d.generation=$2 AND d.scope=$3 AND d.key_digest=m.key_digest)
                       AND ($5::bigint IS NULL OR m.last_ms<$5
                            OR (m.last_ms=$5 AND (m.session_key>$6 OR ($7 AND m.session_key=$6))))
                     ORDER BY m.last_ms DESC,m.session_key LIMIT $8",
                    &[
                        &ns,
                        &generation,
                        &scope,
                        &gram.as_bytes(),
                        &after.map(|v| v.last),
                        &after.map(|v| v.key.as_bytes()),
                        &after.is_some_and(|v| v.inclusive),
                        &(limit as i64),
                    ],
                )?;
                rows.iter()
                    .map(|row| {
                        Ok(Position {
                            inclusive: false,
                            last: row.get(0),
                            key: String::from_utf8(row.get(1)).map_err(io::Error::other)?,
                        })
                    })
                    .collect()
            },
            |key, budget| {
                let rows = run(
                    "SELECT octet_length(fields)::bigint,CASE WHEN octet_length(fields)<=$5::bigint THEN fields END FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND scope=$3 AND key_digest=sha256($4::bytea) AND session_key=$4 AND complete",
                    &[&ns, &generation, &scope, &key.as_bytes(), &(budget as i64)],
                )?;
                let row = rows
                    .first()
                    .ok_or_else(|| failed(StorageError::ScopeChanged))?;
                let fields = row
                    .get::<_, Option<Vec<u8>>>(1)
                    .map(String::from_utf8)
                    .transpose()
                    .map_err(io::Error::other)?;
                Ok((row.get(0), fields))
            },
            // The retained search document follows the latest indexed row
            // when it is still Current. A history-only owner supplies the
            // fallback candidate and its retained lifecycle.
            |key, budget| {
                let rows = run(
                    "WITH live AS (
                        SELECT metadata,last_ms,harness,repo FROM semon_pg_native_catalog_rows
                        WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2
                          AND key_digest=sha256($3::bytea) AND session_key=$3
                     ), retained AS (
                        SELECT h.metadata,o.last_ms,o.harness,o.repo
                        FROM semon_pg_native_catalog_history_owners o
                        JOIN semon_pg_native_catalog_history_headers h
                          ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace
                         AND h.generation=o.owner_generation AND h.key_digest=o.key_digest
                         AND h.session_key=o.session_key
                        WHERE $5 AND o.namespace_digest=sha256($1::bytea) AND o.namespace=$1
                          AND o.generation=$2 AND o.key_digest=sha256($3::bytea) AND o.session_key=$3
                     ), chosen AS (
                        SELECT metadata,last_ms,harness,repo,true AS is_live FROM live
                        UNION ALL
                        SELECT metadata,last_ms,harness,repo,false FROM retained
                        WHERE NOT EXISTS (SELECT 1 FROM live)
                     ) SELECT octet_length(metadata)::bigint,CASE WHEN octet_length(metadata)<=$4::bigint THEN metadata END,
                         last_ms,harness,repo,is_live FROM chosen",
                    &[&ns, &generation, &key.as_bytes(), &(budget as i64), &history],
                )?;
                if rows.len() > 1 {
                    return Err(failed(StorageError::ScopeChanged));
                }
                let text = |bytes: Vec<u8>| String::from_utf8(bytes).map_err(io::Error::other);
                rows.first()
                    .map(|row| {
                        let repo = text(row.get(4))?;
                        Ok(Candidate {
                            owner: if row.get::<_, bool>(5) {
                                CatalogObservationOwner::Catalog
                            } else {
                                CatalogObservationOwner::History
                            },
                            length: row.get(0),
                            encoded: row.get::<_, Option<Vec<u8>>>(1).map(text).transpose()?,
                            last: row.get(2),
                            harness: text(row.get(3))?,
                            repo: (!repo.is_empty()).then_some(repo),
                            lifecycle: if row.get::<_, bool>(5) {
                                crate::model::summary::CatalogLifecycle::Current
                            } else {
                                crate::model::summary::CatalogLifecycle::Retained
                            }
                            .as_str()
                            .to_owned(),
                        })
                    })
                    .transpose()
            },
            || failed(StorageError::ScopeChanged),
        );
        tx.check().map_err(failed)?;
        selection
    }
    fn has_history(&self, key: &str) -> io::Result<bool> {
        self.catalog(key, true)
            .map(|row| row.is_some())
            .map_err(failed)
    }
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>> {
        self.catalog(key, true)
            .map_err(failed)?
            .map(|mut row| {
                row.lifecycle = crate::model::summary::CatalogLifecycle::Retained;
                serde_json::to_string(&row).map_err(io::Error::other)
            })
            .transpose()
    }
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool> {
        self.require_current()?;
        let Some(owner_generation) = self.owner(&row.key).map_err(failed)? else {
            return Ok(false);
        };
        let tx = self
            .tx
            .lock()
            .map_err(|_| failed(StorageError::Unavailable))?;
        tx.check().map_err(failed)?;
        let namespace = tx.namespace.as_str().as_bytes();
        let rows = tx
            .driver
            .query(
                "SELECT session_key=$4,CASE WHEN octet_length(source_generation)=64 THEN source_generation END,version,incomplete FROM semon_pg_native_catalog_history_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3",
                &[
                    &namespace,
                    &owner_generation.as_bytes(),
                    &digest(row.key.as_bytes()),
                    &row.key.as_bytes(),
                ],
            )
            .map_err(sqlstate::error)
            .map_err(failed)?;
        if rows.len() > 1 {
            return Err(failed(StorageError::ScopeChanged));
        }
        let qualified = if let Some(header) = rows.first() {
            if !header.get::<_, bool>(0) {
                return Err(failed(StorageError::ScopeChanged));
            }
            match header.get::<_, Option<Vec<u8>>>(1) {
                Some(generation) => {
                    let generation = String::from_utf8(generation).map_err(io::Error::other)?;
                    let version = u32::try_from(header.get::<_, i64>(2))
                        .map_err(|_| failed(StorageError::ScopeChanged))?;
                    let expected_source_generation =
                        crate::retention::source_generation(&row.sources)
                            .map_err(io::Error::other)?;
                    crate::history_projection::qualified_header(
                        &expected_source_generation,
                        version,
                        &generation,
                        header.get(3),
                    )
                }
                None => false,
            }
        } else {
            return Err(failed(StorageError::ScopeChanged));
        };
        tx.check().map_err(failed)?;
        Ok(qualified)
    }
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow, Option<CatalogObservationOwner>) -> serde_json::Value,
    ) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        crate::storage::selected::selected_details(self, key, sources, &mut |row, owner| {
            status(row, owner)
        })
    }
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<serde_json::Value>> {
        self.require_current()?;
        if generation != self.generation {
            return Ok(None);
        }
        let Some((_, header, _)) = self.selected(key, history, 0, 1).map_err(failed)? else {
            return Ok(None);
        };
        if !(0..=9_007_199_254_740_991).contains(&header.3) {
            return Err(failed(StorageError::ScopeChanged));
        }
        Ok(Some(
            serde_json::json!({"generation":crate::transcript_range::entry_generation(&header.0),"total":header.3}),
        ))
    }
}
