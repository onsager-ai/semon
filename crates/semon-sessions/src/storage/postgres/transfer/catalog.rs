//! Private current Catalog mappings. Transfer does not manufacture an ACK.
use super::*;
use crate::storage::transfer::{
    CatalogRowKind, TransferRecord, catalog_rows, reported::CaptureState,
};
use serde_json::Value;
use tokio_postgres::types::{ToSql, Type};

fn spec(kind: CatalogRowKind) -> (&'static str, &'static str) {
    use CatalogRowKind::*;
    match kind {
        PublicationReceipt => (
            "semon_pg_native_semantic_publications",
            "generation,epoch,admission,base_generation,source_revision,proof_digest,claims,metadata",
        ),
        PresentationRoot => (
            "semon_pg_native_selected_roots",
            "publication,base,manifest,digest",
        ),
        ProofGeneration => (
            "semon_pg_native_selected_generations",
            "id,session_key,publication,proof,admission,stream,metadata,slots,prefix",
        ),
        Presentation => (
            "semon_pg_native_selected_presentations",
            "id,session_key,metadata",
        ),
        ProofSource => (
            "semon_pg_native_selected_sources",
            "id,session_key,path,ledger",
        ),
        ProofRef => ("semon_pg_native_selected_refs", "session_key,kind,id"),
        Dependency => (
            "semon_pg_native_catalog_dependencies",
            "source_path,harness,native_id",
        ),
        Description => (
            "semon_pg_native_descriptions",
            "session_key,version,inputs,metadata",
        ),
        HistoryProgress => (HISTORY_PROGRESS, "session_key,metadata"),
        Current => ("semon_pg_native_catalog_rows", "session_key,metadata"),
        Recipe => (
            "semon_pg_native_catalog_recipe_headers",
            "session_key,recipe_generation,source_generation,version,total",
        ),
        Slot => (
            "semon_pg_native_catalog_recipe_rows",
            "session_key,slot,metadata",
        ),
        History => (HISTORY, "session_key,metadata"),
        HistoryRecipe => (
            HISTORY,
            "session_key,recipe_generation,source_generation,version,total,incomplete",
        ),
        HistorySlot => (HISTORY_SLOTS, "session_key,slot,metadata"),
        HistoryOriginal => (HISTORY_ORIGINAL, "session_key,original_header"),
        TraceHeader => (
            "semon_pg_native_catalog_trace_headers",
            "session_key,trace_generation,source_generation,metadata",
        ),
        TraceTurn => (
            "semon_pg_native_catalog_trace_turns",
            "session_key,turn_id,start_id,metadata",
        ),
        TraceEdge => (
            "semon_pg_native_catalog_trace_edges",
            "session_key,edge_id,metadata",
        ),
        TraceBookmark => (
            "semon_pg_native_catalog_trace_bookmarks",
            "session_key,target,edge_id",
        ),
        SearchDocument => (
            "semon_pg_native_catalog_search_documents",
            "scope,session_key,fields,complete",
        ),
        SearchCount => (
            "semon_pg_native_catalog_search_counts",
            "scope,gram,members",
        ),
    }
}
const HISTORY: &str = "(SELECT o.namespace,o.namespace_digest,o.generation,h.session_key,h.metadata,h.recipe_generation,coalesce(h.source_generation,''::bytea) AS source_generation,h.version,h.total,h.incomplete FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key) AS history";
const HISTORY_SLOTS: &str = "(SELECT o.namespace,o.namespace_digest,o.generation,s.session_key,s.slot,s.metadata FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_slots s ON s.namespace_digest=o.namespace_digest AND s.namespace=o.namespace AND s.generation=o.owner_generation AND s.key_digest=o.key_digest AND s.session_key=o.session_key) AS history_slots";
const HISTORY_PROGRESS: &str = "(SELECT o.namespace,o.namespace_digest,o.generation,p.session_key,p.metadata FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_progress p ON p.namespace_digest=o.namespace_digest AND p.namespace=o.namespace AND p.generation=o.owner_generation AND p.key_digest=o.key_digest AND p.session_key=o.session_key) AS history_progress";
const HISTORY_ORIGINAL: &str = "(SELECT o.namespace,o.namespace_digest,o.generation,h.session_key,h.original_header FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key WHERE h.original_header IS NOT NULL) AS history_original";

fn verify_authority(
    tx: &mut Inbox<'_, '_>,
    kind: CatalogRowKind,
    values: &[Value],
) -> Result<(), StorageError> {
    use CatalogRowKind::*;
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    let text = |i: usize| values[i].as_str().ok_or(StorageError::ScopeChanged);
    let decode = |bytes: Vec<u8>| String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged);
    match kind {
        PresentationRoot => {
            let exists: bool = tx.0.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_semantic_publications WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND proof_digest=$3)", &[&ns,&text(0)?.as_bytes(),&text(3)?.as_bytes()]).map_err(sqlstate::error)?.get(0);
            if !exists {
                return Err(StorageError::ScopeChanged);
            }
        }
        ProofGeneration => {
            let record = catalog_rows::proof_generation(tx.0.namespace.as_str(), values)?;
            let exists: bool = tx.0.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_semantic_publications p JOIN semon_pg_native_selected_roots r ON r.namespace_digest=p.namespace_digest AND r.namespace=p.namespace AND r.publication=p.generation AND r.digest=p.proof_digest WHERE p.namespace_digest=sha256($1::bytea) AND p.namespace=$1 AND p.generation=$2 AND p.proof_digest=$3 AND p.admission=$4 AND convert_from(p.metadata,'UTF8')::jsonb ? convert_from($5::bytea,'UTF8'))", &[&ns,&record.publication.as_bytes(),&record.proof.as_bytes(),&(record.admission as i64),&record.metadata.as_bytes()]).map_err(sqlstate::error)?.get(0);
            if !exists {
                return Err(StorageError::ScopeChanged);
            }
        }
        Presentation => {
            let row = tx.0.driver.query_one("SELECT CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,CASE WHEN octet_length(g.slots)<=1048576 THEN g.slots END,CASE WHEN octet_length(r.manifest)<=1048576 THEN r.manifest END FROM semon_pg_native_selected_generations g JOIN semon_pg_native_selected_roots r ON r.namespace_digest=g.namespace_digest AND r.namespace=g.namespace AND r.publication=g.publication AND r.digest=g.proof WHERE g.namespace_digest=sha256($1::bytea) AND g.namespace=$1 AND g.id=$2 AND g.key_digest=sha256($3::bytea) AND g.session_key=$3", &[&ns,&text(0)?.as_bytes(),&text(1)?.as_bytes()]).map_err(sqlstate::error)?;
            let metadata = decode(row.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?)?;
            let slots = decode(row.get::<_, Option<Vec<u8>>>(1).ok_or(StorageError::Busy)?)?;
            let manifest = decode(row.get::<_, Option<Vec<u8>>>(2).ok_or(StorageError::Busy)?)?;
            crate::storage::proof_codec::verify_presentation_entry(
                text(1)?,
                &metadata,
                &slots,
                text(2)?,
                &manifest,
            )?;
        }
        ProofSource => {
            let row = tx.0.driver.query_one("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_selected_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND id=$2 AND key_digest=sha256($3::bytea) AND session_key=$3", &[&ns,&text(0)?.as_bytes(),&text(1)?.as_bytes()]).map_err(sqlstate::error)?;
            catalog_rows::verify_source_edge(
                &decode(row.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?)?,
                text(2)?,
                text(3)?,
            )?;
        }
        ProofRef => {
            let row = tx.0.driver.query_one("SELECT CASE WHEN octet_length(g.metadata)<=1048576 THEN g.metadata END,(SELECT count(*) FROM semon_pg_native_selected_sources s WHERE s.namespace_digest=g.namespace_digest AND s.namespace=g.namespace AND s.id=g.id AND s.key_digest=g.key_digest AND s.session_key=g.session_key) FROM semon_pg_native_selected_generations g JOIN semon_pg_native_selected_presentations p USING(namespace_digest,namespace,id,key_digest,session_key) WHERE g.namespace_digest=sha256($1::bytea) AND g.namespace=$1 AND g.id=$2 AND g.key_digest=sha256($3::bytea) AND g.session_key=$3", &[&ns,&text(2)?.as_bytes(),&text(0)?.as_bytes()]).map_err(sqlstate::error)?;
            let metadata: crate::model::summary::CatalogRow = serde_json::from_slice(
                &row.get::<_, Option<Vec<u8>>>(0).ok_or(StorageError::Busy)?,
            )
            .map_err(|_| StorageError::ScopeChanged)?;
            if usize::try_from(row.get::<_, i64>(1)).ok() != Some(metadata.sources.len()) {
                return Err(StorageError::ScopeChanged);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Check the fixed authoritative families in one bounded SQL statement before
/// any page commits. Namespace binding remains explicit on every relation.
pub(super) fn occupied(tx: &mut Inbox<'_, '_>) -> Result<bool, StorageError> {
    let ns = tx.0.namespace.as_str().as_bytes();
    let mut terms = CatalogRowKind::ALL.iter().map(|kind| {
        let (table, _) = spec(*kind);
        format!("EXISTS(SELECT 1 FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1)")
    }).collect::<Vec<_>>();
    // Include orphaned physical rows too. The visible-owner joins above must
    // not make a corrupt destination appear empty before earlier pages commit.
    for table in [
        "catalog_sources",
        "catalog_links",
        "catalog_history_headers",
        "catalog_history_slots",
        "catalog_history_sources",
        "catalog_history_progress",
    ] {
        terms.push(format!("EXISTS(SELECT 1 FROM semon_pg_native_{table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1)"));
    }
    Ok(tx
        .0
        .driver
        .query_one(&format!("SELECT {}", terms.join(" OR ")), &[&ns])
        .map_err(sqlstate::error)?
        .get(0))
}

pub(super) fn capture(
    tx: &mut Inbox<'_, '_>,
    state: &mut CaptureState,
) -> Result<transfer::CaptureProgress, StorageError> {
    let index = usize::from(state.native_family - 32);
    let Some(kind) = CatalogRowKind::ALL.get(index).copied() else {
        transfer::reported::finish_capture(tx, state)?;
        return Ok(state.progress());
    };
    let generation = match super::super::model_cache::generation(tx.0)? {
        Some(generation) => Some(generation),
        None => tx
            .get(catalog_rows::IMPORTED_GENERATION, 64)?
            .map(|bytes| String::from_utf8(bytes).map_err(|_| StorageError::ScopeChanged))
            .transpose()?,
    };
    let Some(generation) = generation else {
        let ns = tx.0.namespace.as_str().as_bytes();
        let occupied: bool = tx.0.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1) OR EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1)", &[&ns]).map_err(sqlstate::error)?.get(0);
        if occupied {
            return Err(StorageError::ScopeChanged);
        }
        transfer::reported::finish_capture(tx, state)?;
        return Ok(state.progress());
    };
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    let (table, columns) = spec(kind);
    let names = columns.split(',').collect::<Vec<_>>();
    let keys = kind
        .keys()
        .iter()
        .map(|i| names[*i])
        .collect::<Vec<_>>()
        .join(",");
    let mut binds: Vec<Box<dyn ToSql + Sync>> =
        vec![Box::new(ns), Box::new(generation.as_bytes().to_vec())];
    let after = if let Some(cursor) = &state.native_key {
        let cursor: Vec<Value> =
            serde_json::from_str(cursor).map_err(|_| StorageError::ScopeChanged)?;
        if cursor.len() != kind.keys().len() {
            return Err(StorageError::ScopeChanged);
        }
        for value in cursor {
            if let Some(value) = value.as_i64() {
                binds.push(Box::new(value));
            } else {
                binds.push(Box::new(
                    value
                        .as_str()
                        .ok_or(StorageError::ScopeChanged)?
                        .as_bytes()
                        .to_vec(),
                ));
            }
        }
        format!(
            " AND ({keys})>({})",
            (3..binds.len() + 1)
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else {
        String::new()
    };
    let refs = binds
        .iter()
        .map(|v| &**v as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    // Descriptions are namespace-owned rather than publication-generation rows.
    let generation_filter = if kind.namespace_owned() {
        " AND $2::bytea IS NOT NULL"
    } else {
        " AND generation=$2"
    };
    let suffix = format!(
        "FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1{generation_filter}{after} ORDER BY {keys}"
    );
    let size = names
        .iter()
        .map(|n| format!("coalesce(octet_length({n}::text),0)::bigint"))
        .collect::<Vec<_>>()
        .join("+");
    let sizes =
        tx.0.driver
            .query(&format!("SELECT {size} {suffix} LIMIT 64"), &refs)
            .map_err(sqlstate::error)?;
    let mut count = 0;
    let mut bytes = 0usize;
    for row in &sizes {
        let size = usize::try_from(row.get::<_, i64>(0)).map_err(|_| StorageError::ScopeChanged)?;
        if bytes
            .checked_add(size)
            .is_none_or(|n| n > transfer::MAX_PAGE_BYTES)
        {
            break;
        }
        bytes += size;
        count += 1;
    }
    if count == 0 && !sizes.is_empty() {
        return Err(StorageError::Busy);
    }
    let rows =
        tx.0.driver
            .query(&format!("SELECT {columns} {suffix} LIMIT {count}"), &refs)
            .map_err(sqlstate::error)?;
    if rows.is_empty() {
        state.native_family += 1;
        state.native_key = None;
    } else {
        let mut records = Vec::new();
        for row in rows {
            let mut values = Vec::new();
            for (i, column) in row.columns().iter().enumerate() {
                values.push(match *column.type_() {
                    Type::BYTEA => row
                        .get::<_, Option<Vec<u8>>>(i)
                        .map(|b| {
                            String::from_utf8(b)
                                .map(Value::String)
                                .map_err(|_| StorageError::ScopeChanged)
                        })
                        .transpose()?
                        .unwrap_or(Value::Null),
                    Type::INT8 => row
                        .get::<_, Option<i64>>(i)
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                    Type::BOOL => row
                        .get::<_, Option<bool>>(i)
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                    _ => return Err(StorageError::ScopeChanged),
                });
            }
            verify_authority(tx, kind, &values)?;
            records.push(TransferRecord::CatalogRow {
                generation: generation.clone(),
                kind,
                values,
            });
        }
        catalog_rows::append(tx, state, records)?;
    }
    transfer::reported::save_capture(tx, state)?;
    Ok(state.progress())
}

pub(super) fn apply(tx: &mut Inbox<'_, '_>, record: &TransferRecord) -> Result<(), StorageError> {
    catalog_rows::validate(record)?;
    let TransferRecord::CatalogRow {
        generation,
        kind,
        values,
    } = record
    else {
        return Err(StorageError::InvalidArguments);
    };
    catalog_rows::bind_imported_generation(tx, generation)?;
    verify_authority(tx, *kind, values)?;
    catalog_rows::bind_imported_publication(tx, generation, *kind, values)?;
    use CatalogRowKind::*;
    let ns = tx.0.namespace.as_str().as_bytes().to_vec();
    let text = |i: usize| values[i].as_str().ok_or(StorageError::ScopeChanged);
    if *kind == HistoryOriginal {
        let changed = tx.0.driver.execute("UPDATE semon_pg_native_catalog_history_headers SET original_header=$4 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3", &[&ns,&generation.as_bytes(),&text(0)?.as_bytes(),&text(1)?.as_bytes()]).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
        tx.0.driver.execute("UPDATE semon_pg_native_catalog_history_owners SET original=true WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3", &[&ns,&generation.as_bytes(),&text(0)?.as_bytes()]).map_err(sqlstate::error)?;
        return Ok(());
    }
    if matches!(kind, Current | History) {
        let metadata = text(1)?;
        let row: crate::model::summary::CatalogRow =
            serde_json::from_str(metadata).map_err(|_| StorageError::ScopeChanged)?;
        let key = row.key.as_bytes();
        if *kind == Current {
            tx.0.driver.execute("INSERT INTO semon_pg_native_catalog_rows(namespace,generation,key_digest,session_key,metadata,last_ms,harness,repo,model) VALUES($1,$2,sha256($3::bytea),$3,$4,$5,$6,$7,$8)", &[&ns,&generation.as_bytes(),&key,&metadata.as_bytes(),&row.last.unwrap_or(0),&row.harness.as_bytes(),&row.repo.as_deref().unwrap_or("").as_bytes(),&row.model.as_bytes()]).map_err(sqlstate::error)?;
            // Rebuild the producer's physical incident index from the exact
            // transferred metadata; this is content, never publication authority.
            let edges = row
                .query
                .as_ref()
                .and_then(|query| query["selected"]["edges"].as_array())
                .into_iter()
                .flatten()
                .flat_map(|edge| ["from", "to"].map(move |field| edge["query"][field].as_str()));
            let peers = row
                .parent
                .as_deref()
                .into_iter()
                .chain(edges.flatten())
                .filter(|peer| *peer != row.key)
                .collect::<std::collections::BTreeSet<_>>();
            for peer in peers {
                tx.0.driver.execute("INSERT INTO semon_pg_native_catalog_links(namespace,generation,key_digest,session_key,peer_digest,peer) VALUES($1,$2,sha256($3::bytea),$3,sha256($4::bytea),$4)", &[&ns,&generation.as_bytes(),&key,&peer.as_bytes()]).map_err(sqlstate::error)?;
            }
        } else {
            tx.0.driver.execute("INSERT INTO semon_pg_native_catalog_history_headers(namespace,generation,key_digest,session_key,recipe_generation,source_generation,version,total,incomplete,metadata,original_header,last_ms,harness,repo,model) VALUES($1,$2,sha256($3::bytea),$3,'',NULL,0,0,true,$4,NULL,$5,$6,$7,$8)", &[&ns,&generation.as_bytes(),&key,&metadata.as_bytes(),&row.last.unwrap_or(0),&row.harness.as_bytes(),&row.repo.as_deref().unwrap_or("").as_bytes(),&row.model.as_bytes()]).map_err(sqlstate::error)?;
            tx.0.driver.execute("INSERT INTO semon_pg_native_catalog_history_owners(namespace,generation,key_digest,session_key,owner_generation,original,last_ms,harness,repo,model) VALUES($1,$2,sha256($3::bytea),$3,$2,false,$4,$5,$6,$7)", &[&ns,&generation.as_bytes(),&key,&row.last.unwrap_or(0),&row.harness.as_bytes(),&row.repo.as_deref().unwrap_or("").as_bytes(),&row.model.as_bytes()]).map_err(sqlstate::error)?;
        }
        let table = if *kind == Current {
            "catalog_sources"
        } else {
            "catalog_history_sources"
        };
        for source in &row.sources {
            let path = source.path.to_str().ok_or(StorageError::ScopeChanged)?;
            let metadata = serde_json::to_vec(source).map_err(|_| StorageError::ScopeChanged)?;
            tx.0.driver.execute(&format!("INSERT INTO semon_pg_native_{table}(namespace,generation,key_digest,session_key,path_digest,source_path,metadata,native_digest) VALUES($1,$2,sha256($3::bytea),$3,sha256($4::bytea),$4,$5,sha256($6::bytea))"), &[&ns,&generation.as_bytes(),&key,&path.as_bytes(),&metadata,&source.native_id.as_bytes()]).map_err(sqlstate::error)?;
        }
        return Ok(());
    }
    if *kind == HistoryRecipe {
        let changed = tx.0.driver.execute("UPDATE semon_pg_native_catalog_history_headers SET recipe_generation=$4,source_generation=$5,version=$6,total=$7,incomplete=$8 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=sha256($3::bytea) AND session_key=$3", &[&ns,&generation.as_bytes(),&text(0)?.as_bytes(),&text(1)?.as_bytes(),&text(2)?.as_bytes(),&values[3].as_i64().ok_or(StorageError::ScopeChanged)?,&values[4].as_i64().ok_or(StorageError::ScopeChanged)?,&values[5].as_bool().ok_or(StorageError::ScopeChanged)?]).map_err(sqlstate::error)?;
        if changed != 1 {
            return Err(StorageError::ScopeChanged);
        }
        return Ok(());
    }
    let (table, columns) = match kind {
        PublicationReceipt => (
            "semantic_publications",
            "generation,epoch,admission,base_generation,source_revision,proof_digest,claims,metadata",
        ),
        PresentationRoot => ("selected_roots", "publication,base,manifest,digest"),
        ProofGeneration => (
            "selected_generations",
            "id,session_key,publication,proof,admission,stream,metadata,slots,prefix",
        ),
        Presentation => ("selected_presentations", "id,session_key,metadata"),
        ProofSource => ("selected_sources", "id,session_key,path,ledger"),
        ProofRef => ("selected_refs", "session_key,kind,id"),
        Dependency => ("catalog_dependencies", "source_path,harness,native_id"),
        Description => ("descriptions", "session_key,version,inputs,metadata"),
        HistoryProgress => ("catalog_history_progress", "session_key,metadata"),
        Recipe => (
            "catalog_recipe_headers",
            "session_key,recipe_generation,source_generation,version,total",
        ),
        Slot => ("catalog_recipe_rows", "session_key,slot,metadata"),
        HistorySlot => ("catalog_history_slots", "session_key,slot,metadata"),
        TraceHeader => (
            "catalog_trace_headers",
            "session_key,trace_generation,source_generation,metadata",
        ),
        TraceTurn => (
            "catalog_trace_turns",
            "session_key,turn_id,start_id,metadata",
        ),
        TraceEdge => ("catalog_trace_edges", "session_key,edge_id,metadata"),
        TraceBookmark => ("catalog_trace_bookmarks", "session_key,target,edge_id"),
        SearchDocument => (
            "catalog_search_documents",
            "scope,session_key,fields,complete",
        ),
        SearchCount => ("catalog_search_counts", "scope,gram,members"),
        _ => return Err(StorageError::InvalidArguments),
    };
    let mut names = vec!["namespace", "generation"];
    let mut slots = vec!["$1".to_owned(), "$2".to_owned()];
    let mut binds: Vec<Box<dyn ToSql + Sync>> =
        vec![Box::new(ns), Box::new(generation.as_bytes().to_vec())];
    if kind.namespace_owned() {
        names.pop();
        slots.pop();
        binds.pop();
    }
    for (name, value) in columns.split(',').zip(values) {
        names.push(name);
        slots.push(format!("${}", binds.len() + 1));
        let bind: Box<dyn ToSql + Sync> = if [
            "version",
            "total",
            "slot",
            "members",
            "epoch",
            "admission",
            "source_revision",
        ]
        .contains(&name)
        {
            Box::new(value.as_i64().ok_or(StorageError::ScopeChanged)?)
        } else if name == "complete" {
            Box::new(value.as_bool().ok_or(StorageError::ScopeChanged)?)
        } else {
            Box::new(value.as_str().map(|s| s.as_bytes().to_vec()))
        };
        binds.push(bind);
        let digest = match name {
            "session_key" => Some("key_digest"),
            "source_path" | "path" => Some("path_digest"),
            "harness" => Some("harness_digest"),
            "native_id" => Some("native_digest"),
            "turn_id" => Some("turn_digest"),
            "edge_id" => Some("edge_digest"),
            "target" => Some("target_digest"),
            _ => None,
        };
        if let Some(digest) = digest {
            names.push(digest);
            slots.push(format!("sha256(${}::bytea)", binds.len()));
        }
    }
    let refs = binds
        .iter()
        .map(|v| &**v as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    tx.0.driver
        .execute(
            &format!(
                "INSERT INTO semon_pg_native_{table}({}) VALUES({})",
                names.join(","),
                slots.join(",")
            ),
            &refs,
        )
        .map_err(sqlstate::error)?;
    if *kind == CatalogRowKind::ProofSource {
        bind_backing(tx, values)?;
    }
    Ok(())
}

fn bind_backing(tx: &mut Inbox<'_, '_>, values: &[Value]) -> Result<(), StorageError> {
    let text = |i: usize| values[i].as_str().ok_or(StorageError::ScopeChanged);
    let saved = tx.0.driver.query_one("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN metadata END FROM semon_pg_native_selected_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND id=$2 AND key_digest=sha256($3::bytea) AND session_key=$3", &[&tx.0.namespace.as_str().as_bytes(),&text(0)?.as_bytes(),&text(1)?.as_bytes()]).map_err(sqlstate::error)?;
    let metadata: crate::model::summary::CatalogRow = serde_json::from_slice(
        &saved
            .get::<_, Option<Vec<u8>>>(0)
            .ok_or(StorageError::Busy)?,
    )
    .map_err(|_| StorageError::ScopeChanged)?;
    let source = metadata
        .sources
        .iter()
        .find(|source| {
            source.path.to_str() == text(2).ok()
                && crate::source_carrier::ledger_digest(&crate::source_carrier::ledger(source))
                    == text(3).unwrap_or("")
        })
        .ok_or(StorageError::ScopeChanged)?;
    let ledger = crate::source_carrier::ledger(source);
    let receipt = crate::events::store::header_ledger(&ledger);
    let backing = tx.0.driver.query("SELECT generation FROM semon_pg_native_page_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path=$2 AND ledger=$3 LIMIT 65", &[&tx.0.namespace.as_str().as_bytes(),&text(2)?.as_bytes(),&receipt.as_bytes()]).map_err(sqlstate::error)?;
    if backing.is_empty() || backing.len() > 64 {
        return Err(StorageError::ScopeChanged);
    }
    for row in backing {
        let id = String::from_utf8(row.get(0)).map_err(|_| StorageError::ScopeChanged)?;
        let pinned = super::super::native_pages::open(
            tx.0,
            text(2)?,
            &ledger,
            crate::storage::proof_lifetime::ProofGenerationId(id),
        )?;
        let header: crate::events::native_pages::chunks::Header =
            serde_json::from_str(&pinned.chunks_header).map_err(|_| StorageError::ScopeChanged)?;
        let pages = header.count.div_ceil(1024);
        let mut width = pages;
        let mut nodes = 0usize;
        while width > 0 {
            nodes = nodes.checked_add(width).ok_or(StorageError::ScopeChanged)?;
            if width == 1 {
                break;
            }
            width = width.div_ceil(2);
        }
        let counts = tx.0.driver.query_one("SELECT count(*) FILTER(WHERE kind=0),count(*) FILTER(WHERE kind=2) FROM semon_pg_native_page_refs WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2", &[&tx.0.namespace.as_str().as_bytes(),&pinned.id.0.as_bytes()]).map_err(sqlstate::error)?;
        if usize::try_from(counts.get::<_, i64>(0)).ok() != Some(pages)
            || usize::try_from(counts.get::<_, i64>(1)).ok() != Some(nodes)
        {
            return Err(StorageError::ScopeChanged);
        }
    }
    // The selected edge names an original publication and exact Native
    // ledger. Bind only backing generations actually carried above; this
    // grants neither a publication operation nor an ordinary reader lease.
    let changed = tx.0.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) SELECT g.namespace,s.publication,sha256(g.source_path),g.source_path,g.generation,g.source_id,coalesce(u.usage_ready AND u.receipt=g.ledger,false) FROM semon_pg_native_selected_generations s JOIN semon_pg_native_page_generations g ON g.namespace_digest=s.namespace_digest AND g.namespace=s.namespace AND g.source_path=$4 LEFT JOIN semon_pg_native_usage_sources u ON u.namespace_digest=g.namespace_digest AND u.namespace=g.namespace AND u.source_id=g.source_id WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.id=$2 AND s.key_digest=sha256($3::bytea) AND s.session_key=$3 AND g.ledger=$5 ON CONFLICT(namespace_digest,generation,path_digest,page_generation) DO NOTHING", &[&tx.0.namespace.as_str().as_bytes(), &text(0)?.as_bytes(), &text(1)?.as_bytes(), &text(2)?.as_bytes(), &receipt.as_bytes()]).map_err(sqlstate::error)?;
    if changed == 0 {
        let exists: bool = tx.0.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_selected_generations s ON s.namespace_digest=p.namespace_digest AND s.namespace=p.namespace AND s.publication=p.generation JOIN semon_pg_native_page_generations g ON g.namespace_digest=p.namespace_digest AND g.namespace=p.namespace AND g.generation=p.page_generation WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.id=$2 AND s.key_digest=sha256($3::bytea) AND s.session_key=$3 AND p.path_digest=sha256($4::bytea) AND p.source_path=$4 AND g.source_path=$4 AND g.ledger=$5)", &[&tx.0.namespace.as_str().as_bytes(), &text(0)?.as_bytes(), &text(1)?.as_bytes(), &text(2)?.as_bytes(), &receipt.as_bytes()]).map_err(sqlstate::error)?.get(0);
        if !exists {
            return Err(StorageError::ScopeChanged);
        }
    }
    // The ordinary Catalog reader leases the captured head, while a selected
    // generation retains its original publication. Bind both addresses to the
    // same verified ledger; neither address grants a lease or host Raw access.
    let generation = tx
        .get(catalog_rows::IMPORTED_GENERATION, 64)?
        .ok_or(StorageError::ScopeChanged)?;
    tx.0.driver.execute("INSERT INTO semon_pg_native_catalog_proofs(namespace,generation,path_digest,source_path,page_generation,usage_generation,usage_ready) SELECT p.namespace,$6,p.path_digest,p.source_path,p.page_generation,p.usage_generation,p.usage_ready FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_selected_generations s ON s.namespace_digest=p.namespace_digest AND s.namespace=p.namespace AND s.publication=p.generation JOIN semon_pg_native_page_generations g ON g.namespace_digest=p.namespace_digest AND g.namespace=p.namespace AND g.generation=p.page_generation WHERE s.namespace_digest=sha256($1::bytea) AND s.namespace=$1 AND s.id=$2 AND s.key_digest=sha256($3::bytea) AND s.session_key=$3 AND p.path_digest=sha256($4::bytea) AND p.source_path=$4 AND g.source_path=$4 AND g.ledger=$5 ON CONFLICT DO NOTHING", &[&tx.0.namespace.as_str().as_bytes(),&text(0)?.as_bytes(),&text(1)?.as_bytes(),&text(2)?.as_bytes(),&receipt.as_bytes(),&generation]).map_err(sqlstate::error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::transfer::reported::tests::selected_receipts::{
        originals, selected_receipts,
    };
    use crate::storage::transfer::reported::tests::{apply, root, seed_catalog, sqlite, stage};

    #[test]
    fn pg_current_transfer225_occupied_description_refused_before_native_commit() {
        let root = root();
        let source = sqlite(&root, "occupied-pg-source");
        crate::storage::transfer::reported::tests::seed_native_usage(
            &source,
            &root.join("native.jsonl"),
            130,
        );
        let (pool, _) = super::super::super::tests::fixture(|_| {});
        let control = PgCancellation::default();
        let config = StoreConfig {
            backend: crate::storage::StoreBackend::Postgres {
                pool: pool.clone(),
                control: control.clone(),
            },
            namespace: source.namespace.clone(),
            expected_stream_epoch: source.expected_stream_epoch.clone(),
        };
        config.create_native_store().unwrap();
        let mut writer = crate::storage::transfer::reported::tests::cache(&config);
        writer.save_session_description("existing", 1, "inputs", "owned destination");
        writer.publish_session_descriptions();
        assert_eq!(
            writer
                .session_description("existing", 1, "inputs")
                .as_deref(),
            Some("owned destination")
        );
        drop(writer);
        let manifest = stage(&source, &config, &[123; 32]);
        assert_eq!(
            config.transfer_inbox().apply_page(&manifest, 1),
            Err(StorageError::Conflict)
        );
        pool.transaction(&config.namespace, TxnKind::Transfer, &control, |tx| {
            let ns = config.namespace.as_str().as_bytes();
            let row = tx.driver.query_one("SELECT (SELECT metadata FROM semon_pg_native_descriptions WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND session_key=$2),(SELECT count(*) FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1)", &[&ns, &b"existing".as_slice()]).map_err(sqlstate::error)?;
            assert_eq!(row.get::<_, Vec<u8>>(0), b"owned destination");
            assert_eq!(row.get::<_, i64>(1), 0);
            Ok(())
        }).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pg_current_transfer225_real_catalog_rows_and_recipes_reverse_without_raw() {
        const CHILD: &str = "SEMON_TRANSFER273_CHILD";
        if std::env::var_os(CHILD).is_none() {
            assert!(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "storage::postgres::transfer::catalog::tests::pg_current_transfer225_real_catalog_rows_and_recipes_reverse_without_raw",
                        "--nocapture",
                    ])
                    .env(CHILD, "1")
                    .status()
                    .unwrap()
                    .success()
            );
            return;
        }
        let root = root();
        let source = sqlite(&root, "pg-catalog-source");
        let prior = StoreConfig {
            backend: crate::storage::StoreBackend::Sqlite {
                database: root.join("pg-catalog-reverse.sqlite3"),
            },
            namespace: crate::storage::LogicalSourceNamespace::new("prior-admission").unwrap(),
            expected_stream_epoch: source.expected_stream_epoch.clone(),
        };
        prior.create_native_store().unwrap();
        crate::events::store::configured::NativeOwnership::admit(&prior).unwrap();
        let reverse = sqlite(&root, "pg-catalog-reverse");
        assert_ne!(
            crate::events::store::configured::NativeOwnership::restore_transfer(&source).unwrap().admission(),
            crate::events::store::configured::NativeOwnership::restore_transfer(&reverse).unwrap().admission(),
        );
        use crate::storage::proof_lifetime::ProofReaderLease;
        let (options, initial) = seed_catalog(&source, &root);
        let original_facts = crate::facts::read_facts(options.facts.as_ref().unwrap()).unwrap();
        let mut host = crate::storage::transfer::reported::tests::selected_receipts::AuthorizedOriginals::default();
        originals(&source, &options, &mut host);
        let owner =
            crate::events::store::configured::NativeOwnership::restore_transfer(&source).unwrap();
        let runtime = crate::storage::NativeReaderBinding::Semantic(owner)
            .reader_runtime()
            .unwrap();
        let original_id = runtime.current_generation(&initial[0].key).unwrap();
        let mut original_lease = runtime
            .selected_generation(&initial[0].key, &original_id, &"f".repeat(64), 30_000)
            .unwrap();
        let expected =
            crate::storage::transfer::reported::tests::advance_catalog(&source, &options);
        // The supported SQLite proof contract predates the optional physical
        // source-ID hint. Its original ledger/header/root remain authoritative.
        originals(&source, &options, &mut host);
        let original_page = original_lease
            .transcript_records(
                &options,
                Some((&host, crate::SessionSourceReadMode::ProviderOnly)),
                0,
                1,
                &crate::SelectedReadControl::default(),
            )
            .unwrap();
        assert_eq!(original_page.snapshot.proof_generation.0, original_id.0);
        assert_eq!(
            original_page.records[0].1.as_ref().unwrap()["message"]["content"],
            "sanitized 0"
        );
        original_lease.release().unwrap();
        let original_selected = selected_receipts(&source, &options, &host);
        // Capturing the SQLite source performs no PG work. Discard an idle
        // fixture connection before the server's ten-second idle-session cap.
        let (pool, _) = super::super::super::tests::fixture(|limits| {
            limits.idle_timeout = std::time::Duration::from_secs(1);
        });
        let control = PgCancellation::default();
        let config = StoreConfig {
            backend: crate::storage::StoreBackend::Postgres {
                pool: pool.clone(),
                control: control.clone(),
            },
            namespace: source.namespace.clone(),
            expected_stream_epoch: source.expected_stream_epoch.clone(),
        };
        config.create_native_store().unwrap();
        let manifest = stage(&source, &config, &[120; 32]);
        for category in [
            transfer::Category::Operations,
            transfer::Category::Proofs,
            transfer::Category::ProofReferences,
        ] {
            assert!(
                manifest
                    .categories
                    .iter()
                    .find(|c| c.category == category)
                    .unwrap()
                    .rows
                    > 0
            );
        }
        std::fs::remove_dir_all(&options.claude_home).unwrap();
        apply(&config, &manifest);
        let pages = source
            .transfer_inbox()
            .checkpoint(&manifest)
            .unwrap()
            .sequence;
        let mut checked_forgery = false;
        let mut retained_ref = false;
        let mut retained_backing = false;
        let mut retained_page = false;
        let mut checked_tree_forgery = false;
        let mut checked_missing_backing = false;
        for sequence in 1..=pages {
            for mut record in source
                .transfer_inbox()
                .staged_page(&manifest, sequence)
                .unwrap()
                .records
            {
                if let TransferRecord::NativeSource(source) = &record {
                    retained_backing |= source.backing.is_some();
                }
                if !checked_missing_backing
                    && let TransferRecord::CatalogRow {
                        kind: CatalogRowKind::ProofSource,
                        values,
                        ..
                    } = &record
                {
                    assert!(matches!(
                        transaction(&config, &pool, &control, |tx| {
                            let ns = tx.0.namespace.as_str().as_bytes();
                            tx.0.driver.execute("DELETE FROM semon_pg_native_page_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?;
                            bind_backing(tx, values)
                        }),
                        Err(StorageError::ScopeChanged)
                    ));
                    checked_missing_backing = true;
                }
                if let TransferRecord::NativePage { backing, .. } = &record {
                    retained_page |= backing.is_some();
                }
                if !checked_tree_forgery
                    && let TransferRecord::NativePage {
                        kind: transfer::NativePageKind::Tree,
                        payload,
                        ..
                    } = &mut record
                {
                    *payload = serde_json::to_string(&[0u8; 32]).unwrap();
                    assert!(matches!(
                        transaction(&config, &pool, &control, |tx| super::super::pages::apply(
                            tx, &record
                        )),
                        Err(StorageError::ScopeChanged)
                    ));
                    checked_tree_forgery = true;
                }
                if let TransferRecord::CatalogRow {
                    kind: CatalogRowKind::ProofRef,
                    values,
                    ..
                } = &record
                {
                    retained_ref |= values[1].as_str() == Some("history");
                }
                if !checked_forgery
                    && let TransferRecord::CatalogRow {
                        kind: CatalogRowKind::ProofGeneration,
                        values,
                        ..
                    } = &mut record
                {
                    values[0] = Value::String("0".repeat(64));
                    assert!(matches!(
                        transaction(&config, &pool, &control, |tx| super::apply(tx, &record)),
                        Err(StorageError::ScopeChanged)
                    ));
                    checked_forgery = true;
                }
            }
        }
        assert!(
            retained_ref,
            "successor must retain the original selected generation"
        );
        assert!(
            retained_backing && retained_page,
            "retained selected references require original Native backing pages"
        );
        assert!(
            checked_tree_forgery,
            "real backing tree nodes must be checked against their children and original root"
        );
        assert!(
            checked_missing_backing,
            "missing Native generations must roll back without granting a proof edge"
        );
        assert!(
            checked_forgery,
            "real publisher must contribute original selected proof authority"
        );
        assert_eq!(
            crate::storage::transfer::reported::tests::cache(&config)
                .session_description(&expected[0].key, 1, "transfer-inputs")
                .as_deref(),
            Some("sanitized transfer description")
        );
        pool.transaction(&config.namespace,TxnKind::ReadSnapshot,&control,|tx| {
            let ns = config.namespace.as_str().as_bytes();
            let rows = tx.driver.query("SELECT metadata FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 ORDER BY session_key", &[&ns]).map_err(sqlstate::error)?;
            let actual = rows.iter().map(|row|serde_json::from_slice::<crate::model::summary::CatalogRow>(&row.get::<_,Vec<u8>>(0)).unwrap()).collect::<Vec<_>>();
            assert_eq!(serde_json::to_value(actual).unwrap(),serde_json::to_value(&expected).unwrap());
            let slots:i64 = tx.driver.query_one("SELECT count(*) FROM semon_pg_native_catalog_recipe_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0);
            assert!(slots>64);
            let proofs:i64 = tx.driver.query_one("SELECT count(DISTINCT p.page_generation) FROM semon_pg_native_catalog_proofs p JOIN semon_pg_native_page_generations g ON g.namespace_digest=p.namespace_digest AND g.namespace=p.namespace AND g.generation=p.page_generation AND g.source_path=p.source_path WHERE p.namespace_digest=sha256($1::bytea) AND p.namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0);
            assert!(proofs>=2,"current and retained publications must reach distinct original Native page generations");
            assert!(super::super::super::model_cache::generation(tx)?.is_none(),"content transfer must not fabricate a publication ACK");
            Ok(())
        }).unwrap();
        let admitted = config
            .transfer_inbox()
            .admit_catalog_from(&source.transfer_inbox(), &manifest, &options)
            .unwrap();
        assert_eq!(
            config
                .transfer_inbox()
                .admit_catalog_from(&source.transfer_inbox(), &manifest, &options)
                .unwrap(),
            admitted
        );
        assert_eq!(config.catalog_generation().unwrap(), admitted);
        super::super::super::catalog_effects::snapshot::with_admitted_generation(
            &pool,
            &config.namespace,
            &control,
            &admitted,
            |snapshot| {
                let rows = snapshot
                    .rows(&crate::catalog::Request::parse("limit=60").unwrap())
                    .map_err(|_| StorageError::ScopeChanged)?;
                assert_eq!(
                    serde_json::to_value(rows).unwrap(),
                    serde_json::to_value(&expected).unwrap()
                );
                Ok(())
            },
        )
        .unwrap();
        let ordinary = |config: &StoreConfig| {
            let mut options = options.clone();
            options.source = crate::storage::HostSource::new(
                config.namespace.clone(),
                config.expected_stream_epoch.clone().unwrap(),
            )
            .unwrap()
            .with_store(config.clone())
            .unwrap();
            let mut query = crate::Query::from_sources(std::sync::Arc::new(
                crate::SessionSources::with_machines(vec![(String::new(), options)]),
            ));
            let originals = host.clone();
            let readers =
                move |_: &str, _: &str| -> std::io::Result<Box<dyn crate::SessionSourceReader>> {
                    Ok(Box::new(originals.clone()))
                };
            let listed = query
                .call_catalog_at(
                    "list_sessions",
                    &serde_json::json!({"limit":60}),
                    Some(&readers),
                    crate::SessionSourceReadMode::ProviderOnly,
                    1_790_000_000_000,
                )
                .unwrap();
            assert!(!listed["sessions"].as_array().unwrap().is_empty());
            let fields = query
                .call_selected_at(
                    "local",
                    &expected[0].key,
                    "get_session",
                    &serde_json::json!({"id":expected[0].key}),
                    Some(&host),
                    crate::SessionSourceReadMode::ProviderOnly,
                    1_790_000_000_000,
                )
                .unwrap();
            let body = query
                .call_selected_at(
                    "local",
                    &expected[0].key,
                    "read_transcript",
                    &serde_json::json!({"id":expected[0].key,"after":0,"limit":1}),
                    Some(&host),
                    crate::SessionSourceReadMode::ProviderOnly,
                    1_790_000_000_000,
                )
                .unwrap();
            assert!(body.to_string().contains("sanitized 0"));
            (listed, fields, body)
        };
        let forward_ordinary = {
            let _guard = crate::storage::postgres::native_ingest_tests::Guard::new();
            ordinary(&config)
        };
        pool.transaction(&config.namespace, TxnKind::ReadSnapshot, &control, |tx| {
            let ns = config.namespace.as_str().as_bytes();
            let empty = b"".as_slice();
            let plan = tx
                .driver
                .query(
                    &format!(
                        "EXPLAIN (ANALYZE, BUFFERS) {}",
                        super::super::native::WITHDRAWN_SOURCE
                    ),
                    &[&ns, &empty, &empty, &empty],
                )
                .map_err(sqlstate::error)?;
            for row in plan {
                eprintln!("transfer273_withdrawn_plan {}", row.get::<_, String>(0));
            }
            Ok(())
        })
        .unwrap();
        let backwards = stage(&config, &reverse, &[121; 32]);
        for category in [
            transfer::Category::PageGenerations,
            transfer::Category::PageBlobs,
            transfer::Category::CatalogCurrent,
            transfer::Category::Recipes,
            transfer::Category::Slots,
            transfer::Category::Trace,
            transfer::Category::Search,
            transfer::Category::Operations,
            transfer::Category::Proofs,
            transfer::Category::ProofReferences,
            transfer::Category::Metadata,
        ] {
            assert_eq!(
                manifest
                    .categories
                    .iter()
                    .find(|c| c.category == category)
                    .unwrap(),
                backwards
                    .categories
                    .iter()
                    .find(|c| c.category == category)
                    .unwrap()
            );
        }
        apply(&reverse, &backwards);
        let mut wrong_epoch = reverse.clone();
        wrong_epoch.expected_stream_epoch = Some("b".repeat(64));
        assert!(wrong_epoch.transfer_inbox().admit_catalog_from(&config.transfer_inbox(), &backwards, &options).is_err());
        assert_eq!(
            reverse
                .transfer_inbox()
                .admit_catalog_from(&config.transfer_inbox(), &backwards, &options)
                .unwrap(),
            admitted
        );
        assert_eq!(reverse.catalog_generation().unwrap(), admitted);
        assert_eq!(ordinary(&reverse), forward_ordinary);
        assert_eq!(
            crate::facts::read_facts(options.facts.as_ref().unwrap()).unwrap(),
            original_facts
        );
        assert_eq!(
            selected_receipts(&reverse, &options, &host),
            original_selected,
            "formal current/retained reader must use exact original receipts after reverse with Raw absent"
        );
        assert_eq!(
            crate::storage::transfer::reported::tests::cache(&reverse)
                .session_description(&expected[0].key, 1, "transfer-inputs")
                .as_deref(),
            Some("sanitized transfer description")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
