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
    [
        prefix.to_owned(),
        format!("native_proof_tree:{prefix}:"),
    ]
}

/// Run before replacing/deleting live proofs, in the same Index transaction.
/// Preserve only generations referenced by Current or retained observations;
/// collect superseded proofs on this source's next mutation. Request-owned WAL
/// snapshots keep collected proofs available until their last reader closes.
pub(super) fn preserve(tx: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let mut referenced = BTreeSet::new();
    let mut statement = tx.prepare(
        "SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END
         FROM session_catalog_sources s JOIN session_catalog c USING(session_key) WHERE s.source_path=?1
         UNION ALL
         SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END
         FROM session_catalog_sources s JOIN session_history_catalog c USING(session_key) WHERE s.source_path=?1"
    )?;
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
    drop(statement);
    let key = format!(
        "native_published_chunks_manifest:{}",
        crate::source_carrier::source_hash(path)
    );
    let prior: Option<String> = tx
        .query_row(NATIVE_METADATA_VALUE, [&key], |r| r.get(0))
        .optional()?
        .flatten();
    let mut saved: BTreeSet<String> = prior
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| rusqlite::Error::InvalidQuery)?
        .unwrap_or_default();
    for digest in saved.clone() {
        let retained = prefix(path, &digest);
        let header: Option<String> = tx
            .query_row(NATIVE_METADATA_VALUE, [format!("{retained}header")], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        let compatible = header.as_deref().is_some_and(|text| {
            serde_json::from_str::<Header>(text)
                .is_ok_and(|header| header.version == 1 && header.parser_version == CACHE_VERSION)
        });
        if referenced.contains(&digest) && compatible {
            continue;
        }
        for namespace in namespaces(&retained) {
            tx.execute(
                "DELETE FROM meta WHERE key>=?1 AND key<?2",
                params![&namespace, format!("{namespace}~")],
            )?;
        }
        saved.remove(&digest);
    }
    let live = super::prefix(path);
    let header: Option<String> = tx
        .query_row(NATIVE_METADATA_VALUE, [format!("{live}header")], |r| {
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
                    "INSERT INTO meta(key,value) SELECT ?2 || substr(key,length(?1)+1),value FROM meta WHERE key>=?1 AND key<?3",
                    params![from, to, format!("{from}~")],
                )?;
            }
            saved.insert(digest);
        }
    }
    if !saved.is_empty() || prior.is_some() {
        let encoded = json(&saved)?;
        if prior.as_deref() != Some(&encoded) {
            tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,encoded])?;
        }
    }
    Ok(())
}
