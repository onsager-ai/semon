//! Relocation reuses one indexed generation after native identity and complete
//! consumed-prefix verification. It never infers identity from a filename.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogRelocation {
    version: u32,
    new_path: String,
    native_id: String,
    prior: Ledger,
    catalog_before: Option<Ledger>,
    after: Ledger,
}
fn catalog_key(path: &str) -> String {
    format!("native_catalog_relocation:{path}")
}
fn source_ledger(source: &crate::model::summary::CatalogSource) -> Ledger {
    Ledger {
        stat: Stat {
            dev: source.dev,
            ino: source.ino,
            size: source.size,
            modified_ns: source.modified_ns,
            changed_ns: source.changed_ns,
        },
        offset: source.offset,
        prefix: source.prefix_sha256,
        tail: source.tail_sha256,
    }
}
fn catalog_evidence(
    connection: &Connection,
    old: &str,
) -> rusqlite::Result<Option<CatalogRelocation>> {
    let value: Option<Option<String>> = connection.query_row("SELECT CASE WHEN octet_length(value)<=16384 THEN value ELSE NULL END FROM meta WHERE key=?1",[catalog_key(old)],|row|row.get(0)).optional()?;
    Ok(value
        .flatten()
        .and_then(|value| serde_json::from_str(&value).ok())
        .filter(|proof: &CatalogRelocation| proof.version == 1))
}
pub(super) fn replaces(
    connection: &Connection,
    old: &crate::model::summary::CatalogSource,
    current: &[crate::model::summary::CatalogSource],
) -> rusqlite::Result<bool> {
    let Some(proof) = catalog_evidence(connection, old.path.to_string_lossy().as_ref())? else {
        return Ok(false);
    };
    let original = source_ledger(old);
    if proof.native_id != old.native_id
        || (proof.prior != original && proof.catalog_before.as_ref() != Some(&original))
    {
        return Ok(false);
    }
    for source in current {
        if source.path.to_str() == Some(&proof.new_path)
            && source.native_id == proof.native_id
            && source_ledger(source) == proof.after
            && ledger_row(connection, &proof.new_path)?
                .is_some_and(|(_, ledger)| ledger == proof.after)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn advance(
    connection: &Transaction<'_>,
    path: &str,
    expected: Option<&Ledger>,
    next: &Ledger,
    append: bool,
) -> rusqlite::Result<()> {
    let olds = connection
        .prepare("SELECT old_path FROM catalog_relocation_candidates WHERE new_path=?1")?
        .query_map([path], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for old in olds {
        let proof = catalog_evidence(connection, &old)?;
        if let Some(mut proof) = proof.filter(|proof| append && Some(&proof.after) == expected) {
            proof.after = next.clone();
            connection.execute(
                "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
                params![catalog_key(&old), json(&proof)?],
            )?;
        } else {
            connection.execute("DELETE FROM meta WHERE key=?1", [catalog_key(&old)])?;
        }
    }
    Ok(())
}
pub(super) fn acknowledge(connection: &Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let pending: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM catalog_relocation_candidates c JOIN session_catalog_invalidations i ON i.source_path=c.old_path OR i.source_path=c.new_path WHERE c.old_path=?1)",[path],|row|row.get(0))?;
    if !pending {
        connection.execute("DELETE FROM meta WHERE key=?1", [catalog_key(path)])?;
        connection.execute(
            "DELETE FROM catalog_relocation_candidates WHERE old_path=?1",
            [path],
        )?;
    }
    Ok(())
}

pub(super) fn reuse(
    store: &mut SqliteStore,
    path: &str,
    stat: &Stat,
    native_id: &str,
    file: &mut crate::sealed::LogFile,
) -> Result<Option<String>, StoreError> {
    let candidates = {
        let mut statement = store.connection.prepare("SELECT DISTINCT source_path FROM session_catalog_sources WHERE lifecycle='current' AND harness='codex' AND native_id=?1 AND source_path<>?2 LIMIT 2").map_err(failure)?;
        statement
            .query_map(params![native_id, path], |row| row.get::<_, String>(0))
            .map_err(failure)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failure)?
    };
    let [old] = candidates.as_slice() else {
        return Ok(None);
    };
    fn namespace(path: &str) -> Option<(&str, bool)> {
        path.rsplit_once("/archived_sessions/")
            .map(|(root, _)| (root, true))
            .or_else(|| {
                path.rsplit_once("/sessions/")
                    .map(|(root, _)| (root, false))
            })
    }
    let (Some((root, archived)), Some((old_root, old_archived))) =
        (namespace(path), namespace(old))
    else {
        return Ok(None);
    };
    let candidate: bool=store.connection.query_row("SELECT EXISTS(SELECT 1 FROM catalog_relocation_candidates WHERE old_path=?1 AND new_path=?2 AND native_id=?3)",params![old,path,native_id],|row|row.get(0)).map_err(failure)?;
    if root != old_root
        || archived == old_archived
        || (!candidate
            && !fs::symlink_metadata(old)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound))
    {
        return Ok(None);
    }
    let Some((_, prior)) = ledger_row(&store.connection, old).map_err(failure)? else {
        return Ok(None);
    };
    if stat.size < prior.offset {
        return Ok(None);
    }
    let Some((_, metadata)) = store.read_tree(old, &prior.stat).map_err(failure)? else {
        return Ok(None);
    };
    let header = read_codex_header(
        &mut store
            .connection
            .prepare(NATIVE_METADATA_VALUE)
            .map_err(failure)?,
        old,
        &prior,
    )
    .map_err(failure)?;
    if header.as_ref().and_then(|header| header.id.as_deref()) != Some(native_id) {
        return Ok(None);
    }
    let proof: Option<Option<String>>=store.connection.query_row("SELECT CASE WHEN octet_length(evidence)<=2048 THEN evidence ELSE NULL END FROM session_source_mutations WHERE source_path=?1",[path],|row|row.get(0)).optional().map_err(failure)?;
    let trusted = proof
        .flatten()
        .and_then(|text| {
            serde_json::from_str::<crate::source_change::RelocationEvidence>(&text).ok()
        })
        .is_some_and(|evidence| {
            evidence.from_path == *old
                && evidence.native_id == native_id
                && evidence.before == prior.stat
                && evidence.after == *stat
                && evidence.generation.len() == 32
                && evidence
                    .generation
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                && evidence.length == prior.offset
                && evidence.length == prior.stat.size
                && (evidence.prefix
                    == format!(
                        "{:x}",
                        sha2::digest::generic_array::GenericArray::from(prior.prefix)
                    ))
        });
    if !trusted && prior.stat != *stat {
        let (prefix, tail) = super::super::consumed_hashes(file, prior.offset)
            .map_err(|error| StoreError::Data(error.to_string()))?;
        if prefix != prior.prefix || tail != prior.tail {
            return Ok(None);
        }
    }
    // Catalog can lag a committed append. Qualify only its selected older
    // consumed prefix, outside the SQLite writer reservation, when necessary.
    let previous = store.connection.query_row("SELECT CASE WHEN octet_length(c.metadata)<=1048576 THEN c.metadata ELSE NULL END FROM session_catalog_sources s JOIN session_catalog c USING(session_key) WHERE s.lifecycle='current' AND s.source_path=?1 LIMIT 1",[old],|row|row.get::<_,Option<String>>(0)).optional().map_err(failure)?.flatten().and_then(|text|serde_json::from_str::<crate::model::summary::CatalogRow>(&text).ok()).and_then(|row|row.sources.into_iter().find(|source|source.path.to_str()==Some(old) && source.native_id==native_id)).map(|source|source_ledger(&source));
    let catalog_before = match previous {
        Some(previous) if previous != prior && previous.offset <= prior.offset => {
            let (prefix, _) = super::super::consumed_hashes(file, previous.offset)
                .map_err(|error| StoreError::Data(error.to_string()))?;
            (prefix == previous.prefix).then_some(previous)
        }
        _ => None,
    };
    if Stat::of(Path::new(path)).ok().as_ref() != Some(stat) {
        return Ok(None);
    }
    let prefix = prior.prefix;
    let mut relocated = prior.clone();
    relocated.stat = *stat;
    // If the move contains new bytes, keep those bytes uncommitted. The
    // verified old prefix and its SHA continuation qualify exactly this append.
    relocated.stat.size = prior.stat.size.min(stat.size);
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failure)?;
    if ledger_row(&tx, old)
        .map_err(failure)?
        .map(|(_, ledger)| ledger)
        .as_ref()
        != Some(&prior)
        || ledger_row(&tx, path).map_err(failure)?.is_some()
    {
        return Ok(None);
    }
    tx.execute(
        "UPDATE files SET path=?1,dev=?2,ino=?3,size=?4,mtime_ns=?5,ctime_ns=?6 WHERE path=?7",
        params![
            path,
            int(relocated.stat.dev),
            int(relocated.stat.ino),
            int(relocated.stat.size),
            relocated.stat.modified_ns.to_be_bytes().as_slice(),
            relocated
                .stat
                .changed_ns
                .map(|value| value.to_be_bytes().to_vec()),
            old
        ],
    )
    .map_err(failure)?;
    write_codex_header(&tx, path, &relocated, header.as_ref()).map_err(failure)?;
    write_tree_summary(&tx, path, &relocated, Some(&metadata.summary)).map_err(failure)?;
    write_parent_marker(&tx, path, &relocated, Some(&metadata.marker)).map_err(failure)?;
    chunk_proofs::write(&tx, path, &relocated, Some(&metadata.chunks)).map_err(failure)?;
    launch_calls::write(&tx, path, &relocated, Some(&metadata.calls)).map_err(failure)?;
    if relocated.stat != *stat {
        let evidence = crate::source_change::AppendEvidence {
            generation: format!(
                "{:032x}",
                u128::from_be_bytes(prefix[..16].try_into().expect("SHA width"))
            ),
            before: relocated.stat,
            after: *stat,
        };
        tx.execute(
            "INSERT OR REPLACE INTO session_source_mutations VALUES(?1,?2)",
            params![path, json(&evidence).map_err(failure)?],
        )
        .map_err(failure)?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO catalog_relocation_candidates VALUES(?1,?2,?3)",
        params![old, path, native_id],
    )
    .map_err(failure)?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
        params![
            catalog_key(old),
            json(&CatalogRelocation {
                version: 1,
                new_path: path.into(),
                native_id: native_id.into(),
                prior,
                catalog_before,
                after: relocated
            })
            .map_err(failure)?
        ],
    )
    .map_err(failure)?;
    invalidate_catalog_source(&tx, old, "removed").map_err(failure)?;
    invalidate_catalog_source(&tx, path, "changed").map_err(failure)?;
    tx.commit().map_err(failure)?;
    Ok(Some(old.clone()))
}
