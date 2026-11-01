//! Physical search effects only. Normalization and posting order are shared.
use super::*;
use crate::storage::catalog::SearchPublicationStore;

impl SearchPublicationStore for Effects<'_, '_> {
    type Error = StorageError;
    fn data(_: serde_json::Error) -> StorageError {
        StorageError::Unavailable
    }
    fn begin(&self, keys: &[String]) -> Result<(), StorageError> {
        if keys.len() > crate::catalog_search::MAX_PUBLICATION_KEYS {
            return Err(StorageError::Busy);
        }
        self.tx.borrow().check()
    }
    fn validate(&self, key: &str) -> Result<(), StorageError> {
        let tx = self.tx.borrow_mut();
        let ns = tx.namespace.as_str().as_bytes();
        let rows = tx.driver.query("SELECT count(*),count(*) FILTER (WHERE octet_length(gram)<=12) FROM (SELECT gram FROM semon_pg_native_catalog_search_memberships WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND key_digest=$3 AND session_key=$4 LIMIT 1025) p", &[&ns, &self.generation.as_bytes(), &digest(key.as_bytes()), &key.as_bytes()]).map_err(sqlstate::error)?;
        let count: i64 = rows[0].get(0);
        if count > 1024 || count != rows[0].get::<_, i64>(1) {
            return Err(StorageError::ScopeChanged);
        }
        tx.check()
    }
    fn remove(&self, _: crate::CatalogReadScope) -> Result<(), StorageError> {
        // Each generation is a new immutable cohort. Replays must compare exact
        // bytes rather than deleting a previous generation's postings.
        self.tx.borrow().check()
    }
    fn document(
        &self,
        scope: crate::CatalogReadScope,
        row: &CatalogRow,
        fields: &str,
        complete: bool,
    ) -> Result<(), StorageError> {
        if fields.len() > MAX {
            return Err(StorageError::Busy);
        }
        self.put(
            "semon_pg_native_catalog_search_documents",
            &["scope", "key_digest", "session_key", "fields", "complete"],
            "scope,key_digest",
            &[
                &crate::catalog_search::scope_key(scope).as_bytes(),
                &digest(row.key.as_bytes()),
                &row.key.as_bytes(),
                &fields.as_bytes(),
                &complete,
            ],
        )
    }
    fn postings(
        &self,
        rows: &[crate::catalog_search::SearchPosting<'_>],
    ) -> Result<(), StorageError> {
        // Bounded arrays avoid one round trip per gram. The full identity is
        // compared on every conflict, including a theoretical digest collision.
        // PG memberships have no scope column; scoped documents select which
        // owners participate in each scope's count and read.
        for rows in rows.chunks(64) {
            let keys = rows
                .iter()
                .map(|posting| posting.row.key.as_bytes().to_vec())
                .collect::<Vec<_>>();
            let grams = rows
                .iter()
                .map(|posting| posting.gram.as_bytes().to_vec())
                .collect::<Vec<_>>();
            let times = rows
                .iter()
                .map(|posting| posting.row.last.unwrap_or(0))
                .collect::<Vec<_>>();
            if grams.iter().any(|gram| gram.len() > 12) {
                return Err(StorageError::Busy);
            }
            let tx = self.tx.borrow_mut();
            tx.check()?;
            let ns = tx.namespace.as_str().as_bytes();
            let changed = tx.driver.execute("INSERT INTO semon_pg_native_catalog_search_memberships(namespace,generation,key_digest,session_key,gram,last_ms) SELECT $1,$2,sha256(k),k,g,t FROM unnest($3::bytea[],$4::bytea[],$5::bigint[]) AS p(k,g,t) ON CONFLICT(namespace_digest,generation,key_digest,gram) DO UPDATE SET generation=excluded.generation WHERE semon_pg_native_catalog_search_memberships.namespace=excluded.namespace AND semon_pg_native_catalog_search_memberships.session_key=excluded.session_key AND semon_pg_native_catalog_search_memberships.last_ms=excluded.last_ms", &[&ns, &self.generation.as_bytes(), &keys, &grams, &times]).map_err(sqlstate::error)?;
            if changed != rows.len() as u64 {
                return Err(StorageError::ScopeChanged);
            }
            boundary(&tx)?;
        }
        Ok(())
    }
    fn counts(&self, scope: crate::CatalogReadScope) -> Result<(), StorageError> {
        let tx = self.tx.borrow_mut();
        tx.check()?;
        let ns = tx.namespace.as_str().as_bytes();
        // Carried owners make the generation unbounded; the aggregate stays in
        // SQL and each owner was admitted with <=1024 grams. No unrelated
        // retained generation participates in this aggregate.
        let row = tx.driver.query_one("WITH physical AS MATERIALIZED (SELECT m.gram FROM semon_pg_native_catalog_search_memberships m JOIN semon_pg_native_catalog_search_documents d ON d.namespace_digest=m.namespace_digest AND d.namespace=m.namespace AND d.generation=m.generation AND d.key_digest=m.key_digest AND d.session_key=m.session_key WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.generation=$2 AND d.scope=$3), valid AS (SELECT coalesce(bool_and(octet_length(gram)<=12),true) AS ok FROM physical), counts AS MATERIALIZED (SELECT gram,count(*) AS members FROM physical WHERE (SELECT ok FROM valid) GROUP BY gram), written AS (INSERT INTO semon_pg_native_catalog_search_counts(namespace,generation,scope,gram,members) SELECT $1,$2,$3,gram,members FROM counts ON CONFLICT(namespace_digest,generation,scope,gram) DO UPDATE SET generation=excluded.generation WHERE semon_pg_native_catalog_search_counts.namespace=excluded.namespace AND semon_pg_native_catalog_search_counts.members=excluded.members RETURNING 1) SELECT (SELECT ok FROM valid) AND (SELECT count(*) FROM counts)=(SELECT count(*) FROM written)", &[&ns, &self.generation.as_bytes(), &crate::catalog_search::scope_key(scope).as_bytes()]).map_err(sqlstate::error)?;
        if !row.get::<_, bool>(0) {
            return Err(StorageError::ScopeChanged);
        }
        boundary(&tx)?;
        Ok(())
    }
}

/// Publication evidence belongs to this exact immutable search generation.
/// Normal publication authors it after carry and counts; transfer carries it.
pub(in crate::storage::postgres) fn publish_state(
    tx: &mut PgTxn<'_>,
    generation: &str,
) -> Result<(), StorageError> {
    #[cfg(test)]
    eprintln!("PG_SEARCH_CHECK check=publish_state state=begin");
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes();
    for scope in [b"current".as_slice(), b"retained_history".as_slice()] {
        tx.driver.execute("INSERT INTO semon_pg_native_catalog_search_coverage(namespace,generation,scope,incomplete) SELECT $1,$2,$3,count(*) FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND scope=$3 AND NOT complete", &[&ns,&generation.as_bytes(),&scope]).map_err(sqlstate::error)?;
        tx.driver.execute("INSERT INTO semon_pg_native_catalog_search_publications(namespace,generation,scope,version,search_generation,backfill_complete,incomplete) SELECT namespace,generation,scope,$4,generation,true,incomplete FROM semon_pg_native_catalog_search_coverage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 AND scope=$3", &[&ns,&generation.as_bytes(),&scope,&crate::catalog_search::VERSION.as_bytes()]).map_err(sqlstate::error)?;
    }
    verify_state(tx, generation)?;
    #[cfg(test)]
    eprintln!("PG_SEARCH_CHECK check=publish_state state=complete");
    Ok(())
}

pub(in crate::storage::postgres) fn verify_state(
    tx: &mut PgTxn<'_>,
    generation: &str,
) -> Result<(), StorageError> {
    tx.check()?;
    let ns = tx.namespace.as_str().as_bytes();
    // Unscoped postings use Current whenever it exists. Retained-only postings
    // must match both the history owner and its exact referenced header; a
    // missing owner/header or NULL timestamp cannot supply valid evidence.
    // Check retained headers independently so zero-posting owners also refuse.
    let valid: bool = tx.driver.query_one("WITH documents AS MATERIALIZED (SELECT scope,key_digest,session_key,complete FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2), owners AS MATERIALIZED (SELECT key_digest,session_key,true AS current FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 UNION ALL SELECT o.key_digest,o.session_key,false FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=o.namespace_digest AND c.namespace=o.namespace AND c.generation=o.generation AND c.key_digest=o.key_digest AND c.session_key=o.session_key)), coverage AS MATERIALIZED (SELECT scope,incomplete FROM semon_pg_native_catalog_search_coverage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2), publications AS MATERIALIZED (SELECT * FROM semon_pg_native_catalog_search_publications WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2) SELECT (SELECT count(*) FROM coverage)=2 AND (SELECT count(*) FROM publications)=2 AND NOT EXISTS(SELECT 1 FROM coverage c WHERE c.scope NOT IN ('current'::bytea,'retained_history'::bytea) OR c.incomplete<0 OR c.incomplete<>(SELECT count(*) FROM documents d WHERE d.scope=c.scope AND NOT d.complete) OR NOT EXISTS(SELECT 1 FROM publications p WHERE p.scope=c.scope AND p.version=$3 AND p.search_generation=$2 AND p.backfill_complete AND p.incomplete=c.incomplete)) AND NOT EXISTS(SELECT 1 FROM owners o WHERE NOT EXISTS(SELECT 1 FROM documents d WHERE d.key_digest=o.key_digest AND d.session_key=o.session_key AND d.scope='retained_history'::bytea) OR (o.current AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.key_digest=o.key_digest AND d.session_key=o.session_key AND d.scope='current'::bytea))) AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.scope NOT IN ('current'::bytea,'retained_history'::bytea) OR NOT EXISTS(SELECT 1 FROM owners o WHERE o.key_digest=d.key_digest AND o.session_key=d.session_key AND (o.current OR d.scope='retained_history'::bytea))) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=o.namespace_digest AND c.namespace=o.namespace AND c.generation=o.generation AND c.key_digest=o.key_digest AND c.session_key=o.session_key) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_headers h WHERE h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key AND h.last_ms=o.last_ms)) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_search_memberships m WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.generation=$2 AND (octet_length(m.gram)>12 OR NOT EXISTS(SELECT 1 FROM documents d WHERE d.key_digest=m.key_digest AND d.session_key=m.session_key AND d.scope='retained_history'::bytea AND d.complete) OR NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=m.namespace_digest AND c.namespace=m.namespace AND c.generation=m.generation AND c.key_digest=m.key_digest AND c.session_key=m.session_key AND c.last_ms=m.last_ms UNION ALL SELECT 1 FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key WHERE o.namespace_digest=m.namespace_digest AND o.namespace=m.namespace AND o.generation=m.generation AND o.key_digest=m.key_digest AND o.session_key=m.session_key AND o.last_ms=m.last_ms AND h.last_ms=m.last_ms AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=m.namespace_digest AND c.namespace=m.namespace AND c.generation=m.generation AND c.key_digest=m.key_digest AND c.session_key=m.session_key))))", &[&ns,&generation.as_bytes(),&crate::catalog_search::VERSION.as_bytes()]).map_err(sqlstate::error)?.get(0);
    #[cfg(test)]
    eprintln!(
        "PG_SEARCH_CHECK check=verify_state part=validity result={}",
        if valid { "passed" } else { "refused" }
    );
    if !valid {
        #[cfg(test)]
        {
            // Keep the authoritative verifier above unchanged. This bounded
            // aggregate probe names its failed validity groups without
            // exposing document identities or content in test logs.
            let probe = tx.driver.query_one("WITH documents AS MATERIALIZED (SELECT scope,key_digest,session_key,complete FROM semon_pg_native_catalog_search_documents WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2), owners AS MATERIALIZED (SELECT key_digest,session_key,true AS current FROM semon_pg_native_catalog_rows WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2 UNION ALL SELECT o.key_digest,o.session_key,false FROM semon_pg_native_catalog_history_owners o WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=o.namespace_digest AND c.namespace=o.namespace AND c.generation=o.generation AND c.key_digest=o.key_digest AND c.session_key=o.session_key)), coverage AS MATERIALIZED (SELECT scope,incomplete FROM semon_pg_native_catalog_search_coverage WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2), publications AS MATERIALIZED (SELECT * FROM semon_pg_native_catalog_search_publications WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2), history_owners AS MATERIALIZED (SELECT namespace_digest,namespace,generation,key_digest,session_key,owner_generation,last_ms FROM semon_pg_native_catalog_history_owners WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2), memberships AS MATERIALIZED (SELECT * FROM semon_pg_native_catalog_search_memberships WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2) SELECT (SELECT count(*) FROM coverage),(SELECT count(*) FROM publications),(SELECT count(*) FROM coverage c WHERE c.scope NOT IN ('current'::bytea,'retained_history'::bytea) OR c.incomplete<0 OR c.incomplete<>(SELECT count(*) FROM documents d WHERE d.scope=c.scope AND NOT d.complete) OR NOT EXISTS(SELECT 1 FROM publications p WHERE p.scope=c.scope AND p.version=$3 AND p.search_generation=$2 AND p.backfill_complete AND p.incomplete=c.incomplete)),(SELECT count(*) FROM owners o WHERE NOT EXISTS(SELECT 1 FROM documents d WHERE d.key_digest=o.key_digest AND d.session_key=o.session_key AND d.scope='retained_history'::bytea) OR (o.current AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.key_digest=o.key_digest AND d.session_key=o.session_key AND d.scope='current'::bytea))),(SELECT count(*) FROM documents d WHERE d.scope NOT IN ('current'::bytea,'retained_history'::bytea) OR NOT EXISTS(SELECT 1 FROM owners o WHERE o.key_digest=d.key_digest AND o.session_key=d.session_key AND (o.current OR d.scope='retained_history'::bytea))),(SELECT count(*) FROM history_owners o WHERE NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=o.namespace_digest AND c.namespace=o.namespace AND c.generation=o.generation AND c.key_digest=o.key_digest AND c.session_key=o.session_key) AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_history_headers h WHERE h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key AND h.last_ms=o.last_ms)),(SELECT count(*) FROM memberships m WHERE octet_length(m.gram)>12),(SELECT count(*) FROM memberships m WHERE NOT EXISTS(SELECT 1 FROM documents d WHERE d.key_digest=m.key_digest AND d.session_key=m.session_key AND d.scope='retained_history'::bytea AND d.complete)),(SELECT count(*) FROM memberships m WHERE NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=m.namespace_digest AND c.namespace=m.namespace AND c.generation=m.generation AND c.key_digest=m.key_digest AND c.session_key=m.session_key AND c.last_ms=m.last_ms UNION ALL SELECT 1 FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key WHERE o.namespace_digest=m.namespace_digest AND o.namespace=m.namespace AND o.generation=m.generation AND o.key_digest=m.key_digest AND o.session_key=m.session_key AND o.last_ms=m.last_ms AND h.last_ms=m.last_ms AND NOT EXISTS(SELECT 1 FROM semon_pg_native_catalog_rows c WHERE c.namespace_digest=m.namespace_digest AND c.namespace=m.namespace AND c.generation=m.generation AND c.key_digest=m.key_digest AND c.session_key=m.session_key)))", &[&ns,&generation.as_bytes(),&crate::catalog_search::VERSION.as_bytes()]);
            match probe {
                Ok(row) => eprintln!(
                    "PG_SEARCH_VALIDITY_DIAG coverage_count={} publication_count={} coverage_readiness_violations={} owner_missing_required_documents={} orphan_or_invalid_scope_documents={} retained_owner_bad_header_or_timestamp={} membership_bad_gram={} membership_missing_complete_document={} membership_bad_authority_timestamp={}",
                    row.get::<_, i64>(0),
                    row.get::<_, i64>(1),
                    row.get::<_, i64>(2),
                    row.get::<_, i64>(3),
                    row.get::<_, i64>(4),
                    row.get::<_, i64>(5),
                    row.get::<_, i64>(6),
                    row.get::<_, i64>(7),
                    row.get::<_, i64>(8),
                ),
                Err(_) => eprintln!("PG_SEARCH_VALIDITY_DIAG result=unavailable"),
            }
        }
        return Err(StorageError::ScopeChanged);
    }
    let mismatch: bool = tx.driver.query_one("WITH physical AS (SELECT d.scope,m.gram,count(*) AS members FROM semon_pg_native_catalog_search_memberships m JOIN semon_pg_native_catalog_search_documents d ON d.namespace_digest=m.namespace_digest AND d.namespace=m.namespace AND d.generation=m.generation AND d.key_digest=m.key_digest AND d.session_key=m.session_key WHERE m.namespace_digest=sha256($1::bytea) AND m.namespace=$1 AND m.generation=$2 GROUP BY d.scope,m.gram), stored AS (SELECT scope,gram,members FROM semon_pg_native_catalog_search_counts WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND generation=$2) SELECT EXISTS(SELECT * FROM physical EXCEPT SELECT * FROM stored) OR EXISTS(SELECT * FROM stored EXCEPT SELECT * FROM physical)", &[&ns,&generation.as_bytes()]).map_err(sqlstate::error)?.get(0);
    #[cfg(test)]
    eprintln!(
        "PG_SEARCH_CHECK check=verify_state part=count_mismatch result={}",
        if mismatch { "refused" } else { "matched" }
    );
    if mismatch {
        return Err(StorageError::ScopeChanged);
    }
    tx.check()
}

/// Publish retained search from the exact original History authority for
/// owners leaving Current. Their prior Search projection may describe a newer
/// Current row than the retained header does.
pub(super) fn publish_retained_history(
    tx: &mut PgTxn<'_>,
    generation: &str,
    key_digests: &[Vec<u8>],
) -> Result<(), StorageError> {
    if key_digests.len() > crate::catalog_search::MAX_PUBLICATION_KEYS {
        return Err(StorageError::Busy);
    }
    if key_digests.is_empty() {
        tx.check()?;
        return Ok(());
    }
    let mut keys = key_digests.to_vec();
    keys.sort();
    if keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(StorageError::ScopeChanged);
    }

    tx.check()?;
    let namespace = tx.namespace.as_str().as_bytes();
    let lengths = tx
        .driver
        .query(
            "SELECT o.key_digest,octet_length(h.metadata)::bigint,octet_length(h.original_header)::bigint FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.key_digest=ANY($3::bytea[]) ORDER BY o.key_digest",
            &[&namespace, &generation.as_bytes(), &keys],
        )
        .map_err(sqlstate::error)?;
    tx.check()?;
    if lengths.len() != keys.len() {
        return Err(StorageError::ScopeChanged);
    }
    let mut total_bytes = 0usize;
    let mut payload_lengths = Vec::with_capacity(lengths.len());
    for (index, row) in lengths.iter().enumerate() {
        let key: Vec<u8> = row.get(0);
        let metadata_length =
            usize::try_from(row.get::<_, i64>(1)).map_err(|_| StorageError::ScopeChanged)?;
        let original_header_length = usize::try_from(
            row.get::<_, Option<i64>>(2)
                .ok_or(StorageError::ScopeChanged)?,
        )
        .map_err(|_| StorageError::ScopeChanged)?;
        if key.as_slice() != keys[index].as_slice() {
            return Err(StorageError::ScopeChanged);
        }
        total_bytes = total_bytes
            .checked_add(metadata_length)
            .and_then(|total| total.checked_add(original_header_length))
            .filter(|total| *total <= crate::catalog_search::MAX_PUBLICATION_BYTES)
            .ok_or(StorageError::Busy)?;
        payload_lengths.push((metadata_length, original_header_length));
    }
    tx.check()?;

    let headers = tx
        .driver
        .query(
            "SELECT o.key_digest,o.session_key,o.owner_generation,o.original,o.last_ms,o.harness,o.repo,o.model,o.query_activity,o.query_active,o.parent_key,h.last_ms,h.harness,h.repo,h.model,h.query_activity,h.query_active,h.parent_key,h.metadata,h.original_header,h.recipe_generation,h.source_generation,h.version,h.total,h.incomplete FROM semon_pg_native_catalog_history_owners o JOIN semon_pg_native_catalog_history_headers h ON h.namespace_digest=o.namespace_digest AND h.namespace=o.namespace AND h.generation=o.owner_generation AND h.key_digest=o.key_digest AND h.session_key=o.session_key WHERE o.namespace_digest=sha256($1::bytea) AND o.namespace=$1 AND o.generation=$2 AND o.key_digest=ANY($3::bytea[]) ORDER BY o.key_digest",
            &[&namespace, &generation.as_bytes(), &keys],
        )
        .map_err(sqlstate::error)?;
    tx.check()?;
    if headers.len() != keys.len() {
        return Err(StorageError::ScopeChanged);
    }

    let mut catalog_rows = Vec::with_capacity(headers.len());
    for (index, header) in headers.iter().enumerate() {
        let key_digest: Vec<u8> = header.get(0);
        let session_key: Vec<u8> = header.get(1);
        let owner_generation: Vec<u8> = header.get(2);
        let original: bool = header.get(3);
        let owner_last: i64 = header.get(4);
        let owner_harness: Vec<u8> = header.get(5);
        let owner_repo: Vec<u8> = header.get(6);
        let owner_model: Vec<u8> = header.get(7);
        let owner_activity: Option<i64> = header.get(8);
        let owner_active: bool = header.get(9);
        let owner_parent: Option<Vec<u8>> = header.get(10);
        let header_last: i64 = header.get(11);
        let header_harness: Vec<u8> = header.get(12);
        let header_repo: Vec<u8> = header.get(13);
        let header_model: Vec<u8> = header.get(14);
        let header_activity: Option<i64> = header.get(15);
        let header_active: bool = header.get(16);
        let header_parent: Option<Vec<u8>> = header.get(17);
        let metadata: Vec<u8> = header.get(18);
        let original_header: Vec<u8> = header
            .get::<_, Option<Vec<u8>>>(19)
            .ok_or(StorageError::ScopeChanged)?;
        let recipe_generation: Vec<u8> = header.get(20);
        let source_generation: Option<Vec<u8>> = header.get(21);
        let version =
            u32::try_from(header.get::<_, i64>(22)).map_err(|_| StorageError::ScopeChanged)?;
        let total: i64 = header.get(23);
        let incomplete: bool = header.get(24);
        if key_digest.as_slice() != keys[index].as_slice()
            || owner_generation.len() != 64
            || !owner_generation.iter().all(|byte| byte.is_ascii_hexdigit())
            || !original
            || metadata.len() != payload_lengths[index].0
            || original_header.len() != payload_lengths[index].1
        {
            return Err(StorageError::ScopeChanged);
        }
        let row: CatalogRow =
            serde_json::from_slice(&metadata).map_err(|_| StorageError::ScopeChanged)?;
        let last = row.last.unwrap_or(0);
        let (activity, active) = crate::storage::query::history_activity(&row);
        let parent = row.parent.as_deref().map(str::as_bytes);
        let expected_source_generation = crate::retention::source_generation(&row.sources)
            .map_err(|_| StorageError::ScopeChanged)?;
        let source_generation = source_generation
            .as_deref()
            .filter(|source| {
                source.len() == 64 && source.iter().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or(StorageError::ScopeChanged)?;
        let original_header: Option<crate::storage::catalog::OriginalHeader> =
            serde_json::from_slice(&original_header).map_err(|_| StorageError::ScopeChanged)?;
        let original_header = original_header.ok_or(StorageError::ScopeChanged)?;
        let recorded_source_generation =
            std::str::from_utf8(source_generation).map_err(|_| StorageError::ScopeChanged)?;
        if digest(row.key.as_bytes()).as_slice() != key_digest.as_slice()
            || row.key.as_bytes() != session_key.as_slice()
            || last != owner_last
            || last != header_last
            || row.lifecycle != crate::model::summary::CatalogLifecycle::Retained
            || row.harness.as_bytes() != owner_harness.as_slice()
            || row.harness.as_bytes() != header_harness.as_slice()
            || row.repo.as_deref().unwrap_or("").as_bytes() != owner_repo.as_slice()
            || row.repo.as_deref().unwrap_or("").as_bytes() != header_repo.as_slice()
            || row.model.as_bytes() != owner_model.as_slice()
            || row.model.as_bytes() != header_model.as_slice()
            || activity != owner_activity
            || activity != header_activity
            || active != owner_active
            || active != header_active
            || parent != owner_parent.as_deref()
            || parent != header_parent.as_deref()
            || !crate::history_projection::qualified_header(
                &expected_source_generation,
                version,
                recorded_source_generation,
                incomplete,
            )
            || total < 0
            || recipe_generation.len() != 64
            || !recipe_generation
                .iter()
                .all(|byte| byte.is_ascii_hexdigit())
            || original_header.0.as_bytes() != recipe_generation.as_slice()
            || original_header.1 != version
            || original_header.2.as_deref() != Some(recorded_source_generation)
            || original_header.3 != total
        {
            return Err(StorageError::ScopeChanged);
        }
        // `name` and `branch` have no SQL mirrors and remain authoritative in
        // this admitted History metadata. This admission mirrors only the
        // indexed owner/header fields above; it does not claim whole-row integrity.
        catalog_rows.push(row);
    }
    tx.check()?;

    let keys = catalog_rows
        .iter()
        .map(|row| row.key.clone())
        .collect::<Vec<_>>();
    let rows = catalog_rows
        .iter()
        .map(|row| crate::catalog_search::ScopedCatalogRow {
            row,
            scopes: crate::catalog_search::SearchScopeEligibility {
                current: false,
                retained_history: true,
            },
        })
        .collect::<Vec<_>>();
    let effects = Effects {
        tx: RefCell::new(&mut *tx),
        generation,
    };
    crate::catalog_search::publish_scoped_rows_port(&effects, &keys, &rows, 64, |_, _| {})?;
    Ok(())
}
