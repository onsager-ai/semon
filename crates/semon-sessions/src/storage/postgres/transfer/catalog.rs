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

/// Check the fixed authoritative families in one bounded SQL statement before
/// any page commits. Namespace binding remains explicit on every relation.
pub(super) fn occupied(tx: &mut Inbox<'_, '_>) -> Result<bool, StorageError> {
    let ns = tx.0.namespace.as_str().as_bytes();
    let terms = CatalogRowKind::ALL.iter().map(|kind| {
        let (table, _) = spec(*kind);
        format!("EXISTS(SELECT 1 FROM {table} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1)")
    }).collect::<Vec<_>>();
    Ok(tx.0.driver.query_one(&format!("SELECT {}", terms.join(" OR ")), &[&ns]).map_err(sqlstate::error)?.get(0))
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
    let generation_filter = if kind == CatalogRowKind::Description {
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
            let edges = row.query.as_ref()
                .and_then(|query| query["selected"]["edges"].as_array())
                .into_iter().flatten()
                .flat_map(|edge| ["from", "to"].map(move |field| edge["query"][field].as_str()));
            let peers = row.parent.as_deref().into_iter().chain(edges.flatten())
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
    if *kind == Description {
        names.pop();
        slots.pop();
        binds.pop();
    }
    for (name, value) in columns.split(',').zip(values) {
        names.push(name);
        slots.push(format!("${}", binds.len() + 1));
        let bind: Box<dyn ToSql + Sync> = if ["version", "total", "slot", "members"].contains(&name)
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
            "source_path" => Some("path_digest"),
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::transfer::reported::tests::{apply, root, seed_catalog, sqlite, stage};

    #[test]
    fn pg_current_transfer225_real_catalog_rows_and_recipes_reverse_without_raw() {
        let root = root();
        let source = sqlite(&root, "pg-catalog-source");
        let reverse = sqlite(&root, "pg-catalog-reverse");
        let (options, expected) = seed_catalog(&source, &root);
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
        let manifest = stage(&source, &config, &[120; 32]);
        std::fs::remove_dir_all(&options.claude_home).unwrap();
        apply(&config, &manifest);
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
            assert!(super::super::super::model_cache::generation(tx)?.is_none(),"content transfer must not fabricate a publication ACK");
            Ok(())
        }).unwrap();
        let backwards = stage(&config, &reverse, &[121; 32]);
        for category in [
            transfer::Category::CatalogCurrent,
            transfer::Category::Recipes,
            transfer::Category::Slots,
            transfer::Category::Trace,
            transfer::Category::Search,
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
        assert_eq!(
            crate::storage::transfer::reported::tests::cache(&reverse)
                .session_description(&expected[0].key, 1, "transfer-inputs")
                .as_deref(),
            Some("sanitized transfer description")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
