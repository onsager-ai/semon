//! Qualification only: actual captured presentation values, never an admitted
//! transfer format or a substitute publication/claim acknowledgement.
use super::*;
use crate::{model::PreparedSources, storage::selected};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const FAMILIES: [&str; 10] = [
    "details",
    "relative",
    "descendants",
    "header",
    "turn",
    "edge",
    "target_turns",
    "participant",
    "preview_header",
    "preview_catalog",
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    codec: String,
    owner: String,
    family: String,
    key: String,
    value: Value,
}

pub(crate) fn capture(
    lookup: &dyn selected::ContextLookup,
    prepared: &PreparedSources,
    generation: &str,
    history: &BTreeSet<String>,
) -> Result<Vec<(String, String)>, StorageError> {
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let uncaptured_runtime = std::cell::Cell::new(false);
    for row in &prepared.rows {
        let projection = prepared
            .transcripts
            .iter()
            .find(|p| p.key == row.key)
            .ok_or(StorageError::ScopeChanged)?;
        let texts = projection
            .slots
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| StorageError::Unavailable)?;
        let mut observations = prepared
            .presentation_observations()
            .cloned()
            .ok_or(StorageError::ScopeChanged)?;
        let context = selected::CapturedContext::capture(
            lookup,
            row,
            &projection.slots,
            &mut |relative, _| {
                observations.capture_relative_status(relative, || {
                    uncaptured_runtime.set(true);
                    // The unchanged fixture captures every incident owner in
                    // the native preparation. Missing runtime evidence refuses;
                    // this diagnostic never invents a CWD/source observation.
                    Err(std::io::Error::other("uncaptured relative runtime"))
                })
            },
        )
        .map_err(|_| StorageError::Unavailable)?;
        let witness = selected::PresentationWitness {
            header_generation: crate::slot_projection::generation(&texts, &row.sources)
                .map_err(|_| StorageError::Unavailable)?,
            version: crate::slot_projection::VERSION,
            total: texts.len(),
            has_history: history.contains(&row.key),
            catalog: [
                (
                    "catalog_version".into(),
                    crate::model::summary::CATALOG_VERSION.to_string(),
                ),
                ("catalog_generation".into(), generation.into()),
                (
                    "slot_projection_catalog_generation".into(),
                    generation.into(),
                ),
                ("catalog_completeness".into(), "partial".into()),
                ("catalog_observed_at".into(), "1790000000000".into()),
            ]
            .into_iter()
            .collect(),
            context,
            relative_statuses: std::mem::take(&mut observations.relative_statuses),
            observations,
        };
        let text = serde_json::to_string(&Some(witness)).map_err(|_| StorageError::Unavailable)?;
        bytes = bytes.saturating_add(text.len());
        if bytes > 8 * MAX_PAGE_BYTES {
            return Err(StorageError::HistoryRetentionBounds);
        }
        rows.push((row.key.clone(), text));
    }
    if uncaptured_runtime.get() {
        return Err(StorageError::ScopeChanged);
    }
    Ok(rows)
}

pub(crate) fn manifest(
    prepared: &PreparedSources,
    rows: &[(String, String)],
) -> Result<String, StorageError> {
    let mut entries = Vec::new();
    for (owner, text) in rows {
        let row = prepared
            .rows
            .iter()
            .find(|row| &row.key == owner)
            .ok_or(StorageError::ScopeChanged)?;
        let metadata = crate::query_projection::SelectedRecipe::prepare(row)
            .map_err(|_| StorageError::ScopeChanged)?
            .metadata;
        let slots = serde_json::to_string(
            &prepared
                .transcripts
                .iter()
                .find(|p| p.key == *owner)
                .map(|p| &p.slots),
        )
        .map_err(|_| StorageError::Unavailable)?;
        entries.push((
            owner.clone(),
            crate::storage::proof_codec::presentation_entry(&metadata, &slots, text)?,
        ));
    }
    String::from_utf8(encode_bounded(
        &crate::storage::proof_codec::PresentationManifest {
            entries,
            observations: prepared.presentation_observations().cloned(),
        },
        MAX_PAGE_BYTES,
    )?)
    .map_err(|_| StorageError::Unavailable)
}

fn split(owner: &str, text: &str) -> Result<Vec<Record>, StorageError> {
    let mut header: Value = serde_json::from_str(text).map_err(|_| StorageError::ScopeChanged)?;
    let mut entries = Vec::new();
    let context = header
        .get_mut("context")
        .and_then(Value::as_object_mut)
        .ok_or(StorageError::ScopeChanged)?;
    if context.len() != FAMILIES.len() || FAMILIES.iter().any(|f| !context.contains_key(*f)) {
        return Err(StorageError::ScopeChanged);
    }
    for family in FAMILIES {
        let map = context[family]
            .as_object()
            .ok_or(StorageError::ScopeChanged)?;
        for (key, value) in map {
            entries.push(Record {
                codec: "selected-context-entry-v1".into(),
                owner: owner.into(),
                family: family.into(),
                key: key.clone(),
                value: value.clone(),
            });
        }
        context.insert(family.into(), json!({}));
    }
    let statuses = header
        .get_mut("relative_statuses")
        .and_then(Value::as_object_mut)
        .ok_or(StorageError::ScopeChanged)?;
    for (key, value) in statuses.iter() {
        entries.push(Record {
            codec: "selected-context-entry-v1".into(),
            owner: owner.into(),
            family: "relative_statuses".into(),
            key: key.clone(),
            value: value.clone(),
        });
    }
    statuses.clear();
    let mut records = vec![Record {
        codec: "selected-context-entry-v1".into(),
        owner: owner.into(),
        family: "header".into(),
        key: String::new(),
        value: header,
    }];
    records.extend(entries);
    Ok(records)
}

fn join(owner: &str, records: &[Record]) -> Result<String, StorageError> {
    let Some(header) = records.first() else {
        return Err(StorageError::ScopeChanged);
    };
    if header.family != "header" || !header.key.is_empty() {
        return Err(StorageError::ScopeChanged);
    }
    let mut value = header.value.clone();
    let mut seen = BTreeSet::new();
    for record in records {
        if record.owner != owner || record.codec != "selected-context-entry-v1" {
            return Err(StorageError::ScopeChanged);
        }
    }
    for record in &records[1..] {
        if !seen.insert((&record.family, &record.key)) {
            return Err(StorageError::ScopeChanged);
        }
        let target = if record.family == "relative_statuses" {
            value.get_mut("relative_statuses")
        } else if FAMILIES.contains(&record.family.as_str()) {
            value
                .get_mut("context")
                .and_then(|context| context.get_mut(&record.family))
        } else {
            return Err(StorageError::ScopeChanged);
        };
        let target = target
            .and_then(Value::as_object_mut)
            .ok_or(StorageError::ScopeChanged)?;
        target.insert(record.key.clone(), record.value.clone());
    }
    serde_json::to_string(&value).map_err(|_| StorageError::Unavailable)
}

pub(crate) fn qualify(
    config: &StoreConfig,
    prepared: &PreparedSources,
    semantic_bytes: usize,
) -> Result<(), StorageError> {
    let backend = match &config.backend {
        StoreBackend::Sqlite { .. } => "sqlite",
        #[cfg(feature = "postgres")]
        StoreBackend::Postgres { .. } => "postgres",
    };
    let base = crate::storage::proof_codec::digest(
        prepared
            .staged_model_witness()
            .map_err(|_| StorageError::Unavailable)?
            .as_bytes(),
    );
    let (rows, manifest) = match &config.backend {
        StoreBackend::Sqlite { .. } => {
            crate::storage::sqlite::proof_generations::measure_presentations(
                config, prepared, &base,
            )?
        }
        #[cfg(feature = "postgres")]
        StoreBackend::Postgres { pool, control } => {
            crate::storage::postgres::measure_presentations(config, pool, control, prepared, &base)?
        }
    };
    let manifest_bytes = manifest.len();
    let root = crate::storage::proof_codec::presentation_digest(&base, &manifest)?;
    let root_record = TransferRecord::CatalogRow {
        generation: base.clone(),
        kind: CatalogRowKind::PresentationRoot,
        values: vec![json!(base), json!(base), json!(manifest), json!(root)],
    };
    catalog_rows::validate(&root_record)?;
    let mut root_page = TransferPage {
        snapshot: Sha256::digest(manifest.as_bytes()).into(),
        sequence: 1,
        operation: Sha256::digest(root.as_bytes()).into(),
        previous: [0; 32],
        category: Category::ProofReferences,
        records: vec![root_record],
        digest: [0; 32],
    };
    root_page.seal()?;
    let root_page_bytes = root_page.encoded()?.len();
    crate::storage::proof_codec::verify_presentation_digest(&base, &manifest, &root)?;
    if crate::storage::proof_codec::verify_presentation_digest(
        &base,
        &(manifest.clone() + " "),
        &root,
    )
    .is_ok()
    {
        return Err(StorageError::ScopeChanged);
    }
    let mut previous = root_page.digest;
    let mut captured = 0usize;
    let mut records_total = 0usize;
    let mut pages_total = root_page_bytes;
    let mut row_max = 0usize;
    let mut page_max = root_page_bytes;
    let mut records_count = 0usize;
    let mut negative = 1usize;
    let mut family_sizes = BTreeMap::<String, (usize, usize, usize)>::new();
    for (owner, original) in rows {
        captured += original.len();
        eprintln!(
            "typed_presentation_owner backend={backend} owner={owner} capture_bytes={}",
            original.len()
        );
        let records = split(&owner, &original)?;
        let mut decoded = Vec::new();
        for record in &records {
            let text = String::from_utf8(encode_bounded(record, MAX_PAGE_BYTES)?)
                .map_err(|_| StorageError::Unavailable)?;
            row_max = row_max.max(text.len());
            records_total += text.len();
            let family = if record.family == "header" && record.key.is_empty() {
                "presentation_header"
            } else {
                &record.family
            };
            let size = family_sizes.entry(family.to_owned()).or_default();
            size.0 += 1;
            size.1 += text.len();
            size.2 = size.2.max(text.len());
            let transfer = TransferRecord::CatalogRow {
                generation: base.clone(),
                kind: CatalogRowKind::Presentation,
                values: vec![json!(root), json!(owner), json!(text)],
            };
            catalog_rows::validate(&transfer)?;
            let mut page = TransferPage {
                snapshot: Sha256::digest(manifest.as_bytes()).into(),
                sequence: records_count as u64 + 2,
                operation: Sha256::digest(
                    serde_json::to_vec(&transfer).map_err(|_| StorageError::Unavailable)?,
                )
                .into(),
                previous,
                category: Category::Metadata,
                records: vec![transfer],
                digest: [0; 32],
            };
            page.seal()?;
            previous = page.digest;
            let encoded = page.encoded()?;
            pages_total += encoded.len();
            page_max = page_max.max(encoded.len());
            let roundtrip: TransferPage =
                serde_json::from_slice(&encoded).map_err(|_| StorageError::ScopeChanged)?;
            roundtrip.encoded()?;
            let TransferRecord::CatalogRow { values, .. } = &roundtrip.records[0] else {
                unreachable!()
            };
            decoded.push(
                serde_json::from_str::<Record>(
                    values[2].as_str().ok_or(StorageError::ScopeChanged)?,
                )
                .map_err(|_| StorageError::ScopeChanged)?,
            );
            records_count += 1;
        }
        let reconstructed = join(&owner, &decoded)?;
        if reconstructed != original {
            return Err(StorageError::ScopeChanged);
        }
        let row = prepared
            .rows
            .iter()
            .find(|r| r.key == owner)
            .ok_or(StorageError::ScopeChanged)?;
        let metadata = crate::query_projection::SelectedRecipe::prepare(row)
            .map_err(|_| StorageError::ScopeChanged)?
            .metadata;
        let slots = serde_json::to_string(
            &prepared
                .transcripts
                .iter()
                .find(|p| p.key == owner)
                .map(|p| &p.slots),
        )
        .map_err(|_| StorageError::Unavailable)?;
        crate::storage::proof_codec::verify_presentation_entry(
            &owner,
            &metadata,
            &slots,
            &reconstructed,
            &manifest,
        )?;
        for (changed_metadata, changed_slots) in [
            (metadata.clone() + " ", slots.clone()),
            (metadata.clone(), slots.clone() + " "),
        ] {
            if crate::storage::proof_codec::verify_presentation_entry(
                &owner,
                &changed_metadata,
                &changed_slots,
                &reconstructed,
                &manifest,
            )
            .is_ok()
            {
                return Err(StorageError::ScopeChanged);
            }
            negative += 1;
        }
        for fault in ["missing", "duplicate", "owner", "value"] {
            let mut changed = decoded.clone();
            match fault {
                "missing" => {
                    changed.pop();
                }
                "duplicate" => {
                    changed.push(changed.last().ok_or(StorageError::ScopeChanged)?.clone())
                }
                "owner" => changed[0].owner.push_str("-foreign"),
                "value" => {
                    changed.last_mut().ok_or(StorageError::ScopeChanged)?.value =
                        json!({"tampered":true})
                }
                _ => unreachable!(),
            }
            let accepted = join(&owner, &changed).and_then(|text| {
                crate::storage::proof_codec::verify_presentation_entry(
                    &owner, &metadata, &slots, &text, &manifest,
                )
            });
            if accepted.is_ok() {
                return Err(StorageError::ScopeChanged);
            }
            negative += 1;
        }
    }
    // Each measured item has the unchanged operation cap. Report the complete
    // aggregate independently; splitting never turns excess aggregate into fit.
    let aggregate = semantic_bytes
        .saturating_add(records_total)
        .saturating_add(manifest_bytes);
    for (family, (count, bytes, largest)) in family_sizes {
        eprintln!(
            "typed_presentation_family backend={backend} family={family} records={count} bytes={bytes} largest={largest}"
        );
    }
    eprintln!(
        "typed_presentation_pg_request_aggregate backend={backend} bytes={aggregate} headroom={} fits={} control_header_excluded=true",
        1_048_576i64 - aggregate as i64,
        aggregate <= 1_048_576
    );
    eprintln!(
        "typed_presentation_totals backend={backend} owners=17 capture_bytes={captured} manifest_bytes={manifest_bytes} manifest_page_bytes={root_page_bytes} typed_records={records_count} typed_record_bytes={records_total} typed_record_max={row_max} transfer_pages={} transfer_page_bytes={pages_total} transfer_page_max={page_max} combined_with_semantic_bytes={aggregate} sqlite_headroom={} aggregate_fits={} allocator_peak_measured=false",
        records_count + 1,
        2_097_152i64 - aggregate as i64,
        aggregate <= 2_097_152
    );
    eprintln!(
        "typed_presentation_cases backend={backend} exact_roundtrip=17 immutable_entry_checks=17 negative={negative} publication_activated=false"
    );
    Ok(())
}
