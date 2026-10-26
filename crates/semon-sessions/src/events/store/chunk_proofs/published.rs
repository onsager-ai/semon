//! Keep the proof generation still named by published Catalog observations.
//! These are content-free proofs in the existing ledger store, not raw history.
use super::*;
use std::collections::BTreeSet;

pub(super) fn prefix(path: &str, ledger: &str) -> String {
    format!(
        "native_published_chunks:{}:{ledger}:",
        crate::source_carrier::source_hash(path)
    )
}

fn namespaces(prefix: &str) -> [String; 2] {
    [prefix.to_owned(), format!("native_proof_tree:{prefix}:")]
}

// Reference ownership follows Catalog families, independently of proof/meta
// layout. Always address physical main tables, including on assigned readers
// that have connection-local scoped views. History's singleton publisher is
// still unported: nonempty singleton history cannot prove its Source owner in
// a composite current Catalog and must refuse before payload reads or GC.
fn reference_statements(db: &Connection) -> rusqlite::Result<[&'static str; 2]> {
    let mut shapes = Vec::new();
    for table in [
        "session_catalog",
        "session_catalog_sources",
        "session_history_catalog",
    ] {
        let (namespaces, namespace_key, session_key): (i64, i64, i64) = db.query_row(
            "SELECT coalesce(sum(name='namespace'),0),coalesce(max(CASE WHEN name='namespace' THEN pk END),0),coalesce(max(CASE WHEN name='session_key' THEN pk END),0) FROM pragma_table_info(?1,'main')",
            [table], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        )?;
        shapes.push(match (namespaces, namespace_key, session_key) {
            (0, 0, 1) => false,
            (1, 1, 2) => true,
            _ => return Err(rusqlite::Error::InvalidQuery),
        });
    }
    let [catalog, sources, history] = shapes.as_slice() else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    if catalog != sources {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if *catalog
        && !*history
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.session_history_catalog)",
            [],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let current = if *catalog {
        "SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s JOIN main.session_catalog c ON c.namespace=s.namespace AND c.session_key=s.session_key WHERE s.namespace=semon_namespace() AND s.source_path=?1"
    } else {
        "SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s JOIN main.session_catalog c USING(session_key) WHERE s.source_path=?1"
    };
    let retained = match (*sources, *history) {
        (false, false) => {
            "SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s JOIN main.session_history_catalog c USING(session_key) WHERE s.source_path=?1"
        }
        (false, true) => {
            "SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s JOIN main.session_history_catalog c USING(session_key) WHERE c.namespace=semon_namespace() AND s.source_path=?1"
        }
        (true, false) => {
            "SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s JOIN main.session_history_catalog c USING(session_key) WHERE s.namespace=semon_namespace() AND s.source_path=?1"
        }
        (true, true) => {
            "SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s JOIN main.session_history_catalog c ON c.namespace=s.namespace AND c.session_key=s.session_key WHERE s.namespace=semon_namespace() AND s.source_path=?1"
        }
    };
    Ok([current, retained])
}

/// Run before replacing/deleting live proofs, in the same Index transaction.
/// Preserve only generations referenced by Current or retained observations;
/// collect superseded proofs on this source's next mutation. Request-owned WAL
/// snapshots keep collected proofs available until their last reader closes.
pub(super) fn preserve(tx: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let mut referenced = BTreeSet::new();
    referenced.extend(crate::storage::sqlite::proof_generations::referenced(
        tx, path,
    )?);
    for sql in reference_statements(tx)? {
        let mut statement = tx.prepare(sql)?;
        for text in statement.query_map([path], |r| r.get::<_, Option<String>>(0))? {
            // An oversized owner cannot be silently excluded from the reference
            // set: that could collect its still-published proof. Refuse the Index
            // mutation before any retention/GC writes and preserve its old state.
            let text = text?.ok_or(rusqlite::Error::InvalidQuery)?;
            let row: crate::model::summary::CatalogRow =
                serde_json::from_str(&text).map_err(|_| rusqlite::Error::InvalidQuery)?;
            for source in row
                .sources
                .iter()
                .filter(|source| source.path == Path::new(path))
            {
                referenced.insert(crate::source_carrier::ledger_digest(
                    &crate::source_carrier::ledger(source),
                ));
            }
        }
    }
    let key = format!(
        "native_published_chunks_manifest:{}",
        crate::source_carrier::source_hash(path)
    );
    let prior: Option<String> = tx
        .query_row(native_metadata_value(tx)?, [&key], |r| r.get(0))
        .optional()?
        .flatten();
    let mut saved: BTreeSet<String> = prior
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| rusqlite::Error::InvalidQuery)?
        .unwrap_or_default();
    let mut collected = 0;
    for digest in saved.clone() {
        let retained = prefix(path, &digest);
        let header: Option<String> = tx
            .query_row(
                native_metadata_value(tx)?,
                [format!("{retained}header")],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let compatible = header.as_deref().is_some_and(|text| {
            serde_json::from_str::<Header>(text)
                .is_ok_and(|header| header.version == 1 && header.parser_version == CACHE_VERSION)
        });
        if referenced.contains(&digest) && compatible {
            continue;
        }
        if collected >= 16 {
            continue;
        }
        collected += 1;
        for namespace in namespaces(&retained) {
            tx.execute(
                "DELETE FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2",
                params![&namespace, format!("{namespace}~")],
            )?;
        }
        saved.remove(&digest);
    }
    let live = super::prefix(path);
    let header: Option<String> = tx
        .query_row(native_metadata_value(tx)?, [format!("{live}header")], |r| {
            r.get(0)
        })
        .optional()?
        .flatten();
    if let Some(header) = header.and_then(|text| serde_json::from_str::<Header>(&text).ok()) {
        let digest = crate::source_carrier::ledger_digest_value(&header.ledger);
        if header.version == 1
            && header.parser_version == CACHE_VERSION
            && referenced.contains(&digest)
            && !saved.contains(&digest)
        {
            let retained = prefix(path, &digest);
            for (from, to) in namespaces(&live).iter().zip(namespaces(&retained)) {
                tx.execute(
                    "INSERT INTO meta(namespace,key,value) SELECT semon_namespace(),?2 || substr(key,length(?1)+1),value FROM meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?3",
                    params![from, to, format!("{from}~")],
                )?;
            }
            saved.insert(digest);
        }
    }
    if !saved.is_empty() || prior.is_some() {
        let encoded = json(&saved)?;
        if prior.as_deref() != Some(&encoded) {
            tx.execute(native_metadata_put(tx)?, params![key, encoded])?;
        }
    }
    Ok(())
}
