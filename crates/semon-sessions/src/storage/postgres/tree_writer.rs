//! Private Tree family writer. Publication is atomic inside the caller's
//! existing transaction; it is not an ingest ACK or a complete IndexStore.
use super::{sqlstate, txn::PgTxn};
use crate::{
    events::{
        CACHE_VERSION, Ledger,
        store::header_ledger,
        tree_rows::{TreeRows, witness},
    },
    native_tree::Summary,
    selected_read,
    storage::StorageError,
};
use tokio_postgres::types::ToSql;

pub(super) const HEADER_EXISTS_SQL: &str = "SELECT EXISTS(SELECT 1 FROM semon_pg_native_tree_headers WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path_digest=semon_pg_native_digest($1::bytea,$2::bytea,'tree.path') AND semon_pg_native_key_matches(source_path,sha256($2::bytea),$2))";

#[cfg(test)]
thread_local! {
    static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static DECODED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct Writer<'a, 'tx> {
    tx: &'a mut PgTxn<'tx>,
}

impl Writer<'_, '_> {
    fn effect(&mut self, query: &str, args: &[&(dyn ToSql + Sync)]) -> Result<(), StorageError> {
        self.tx.check()?;
        if self
            .tx
            .driver
            .execute(query, args)
            .map_err(sqlstate::error)?
            != 1
        {
            return Err(StorageError::Conflict);
        }
        self.boundary()
    }

    fn boundary(&self) -> Result<(), StorageError> {
        #[cfg(test)]
        if FAIL_AFTER.with(|point| match point.get() {
            Some(1) => {
                point.set(None);
                true
            }
            Some(n) => {
                point.set(Some(n - 1));
                false
            }
            None => false,
        }) {
            return Err(StorageError::Unavailable);
        }
        self.tx.check()
    }

    fn values(&mut self, projection: i64, summary: &Summary) -> Result<(), StorageError> {
        let rows = TreeRows::new(summary).map_err(|_| StorageError::InvalidArguments)?;
        let ns = self.tx.namespace.as_str().as_bytes().to_vec();
        let label = "INSERT INTO semon_pg_native_tree_labels(namespace,projection_id,label,value) VALUES($1,$2,$3,$4) ON CONFLICT(namespace_digest,projection_id,label) DO UPDATE SET value=excluded.value";
        self.effect(
            label,
            &[
                &ns,
                &projection,
                &b"codex_turn".as_slice(),
                &rows.codex_turn.as_bytes(),
            ],
        )?;
        for (key, value) in rows.labels() {
            self.effect(
                label,
                &[&ns, &projection, &key.as_bytes(), &value.as_bytes()],
            )?;
        }
        // DO UPDATE (including identical values) gives an exact affected-row
        // check while retaining historical models and closed tool names.
        for model in rows.models {
            self.effect("INSERT INTO semon_pg_native_tree_models(namespace,projection_id,model) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,projection_id,model) DO UPDATE SET model=excluded.model", &[&ns,&projection,&model.as_bytes()])?;
        }
        for (id, name) in rows.tools {
            self.effect("INSERT INTO semon_pg_native_tree_tools(namespace,projection_id,tool_id,name,closed) VALUES($1,$2,$3,$4,false) ON CONFLICT(namespace_digest,projection_id,tool_id) DO UPDATE SET name=excluded.name", &[&ns,&projection,&id.as_bytes(),&name.as_bytes()])?;
        }
        for id in rows.closed_tools {
            self.effect("INSERT INTO semon_pg_native_tree_tools(namespace,projection_id,tool_id,name,closed) VALUES($1,$2,$3,NULL,true) ON CONFLICT(namespace_digest,projection_id,tool_id) DO UPDATE SET closed=true", &[&ns,&projection,&id.as_bytes()])?;
        }
        for (id, model) in rows.usage_models {
            self.effect("INSERT INTO semon_pg_native_tree_usage_models(namespace,projection_id,message_id,model) VALUES($1,$2,$3,$4) ON CONFLICT(namespace_digest,projection_id,message_id_digest) DO UPDATE SET model=excluded.model WHERE semon_pg_native_tree_usage_models.namespace=excluded.namespace AND semon_pg_native_key_matches(semon_pg_native_tree_usage_models.message_id,sha256(excluded.message_id),excluded.message_id)", &[&ns,&projection,&id.as_bytes(),&model.as_bytes()])?;
        }
        Ok(())
    }
}

/// Stage real dimensions in the existing transaction. A staged family remains
/// unreadable until the connected publication below has qualified its scalar.
#[cfg(test)]
pub(super) fn stage(
    tx: &mut PgTxn<'_>,
    path: &str,
    ledger: &Ledger,
    expected_header: Option<&Ledger>,
    expected_source: i64,
    summary: &Summary,
) -> Result<(i64, String), StorageError> {
    stage_inner(
        tx,
        path,
        ledger,
        expected_header,
        expected_source,
        summary,
        false,
    )
}

fn stage_inner(
    tx: &mut PgTxn<'_>,
    path: &str,
    ledger: &Ledger,
    expected_header: Option<&Ledger>,
    expected_source: i64,
    summary: &Summary,
    repair: bool,
) -> Result<(i64, String), StorageError> {
    let _scope = selected_read::Scope::ensure();
    tx.check()?;
    selected_read::check().map_err(|_| StorageError::Busy)?;
    if expected_source <= 0 {
        return Err(StorageError::InvalidArguments);
    }
    // The source must already be enrolled by the existing usage producer.
    // Locking must not manufacture source_state or increment its revision.
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let locked = tx.driver.query("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND namespace_digest=sha256($1::bytea) FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if locked.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let receipt = header_ledger(ledger);
    // Only fixed-size physical IDs cross this query. Match the complete file
    // ledger and current usage generation/readiness in SQL before any effect.
    let files = tx.driver.query("SELECT f.file_id FROM semon_pg_native_files f JOIN semon_pg_native_usage_current c ON c.namespace_digest=f.namespace_digest AND c.namespace=f.namespace AND c.source_path_digest=semon_pg_native_digest(f.namespace,f.path,'usage.path') AND semon_pg_native_key_matches(c.source_path,sha256(f.path),f.path) JOIN semon_pg_native_usage_sources s ON s.namespace=c.namespace AND s.source_id=c.source_id AND s.source_path=c.source_path WHERE f.namespace_digest=sha256($1::bytea) AND f.namespace=$1 AND f.path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(f.path,f.path_digest,$2) AND c.source_id=$3 AND s.parser_version=$4 AND s.receipt=$5 AND ((s.usage_ready AND s.checkpoint=$6::text::numeric) OR (NOT s.usage_ready AND s.checkpoint=0 AND f.codex_native_usage IS NULL AND NOT EXISTS(SELECT 1 FROM semon_pg_native_usage_facts z WHERE z.namespace=s.namespace AND z.source_id=s.source_id))) AND f.resume_at=$6::text::numeric AND f.dev=$7::text::numeric AND f.ino=$8::text::numeric AND f.size=$9::text::numeric AND f.mtime_ns=$10 AND f.ctime_ns IS NOT DISTINCT FROM $11 AND f.head_sha256=$12 AND f.tail_sha256=$13", &[&ns,&path.as_bytes(),&expected_source,&i64::from(CACHE_VERSION),&receipt.as_bytes(),&ledger.offset.to_string(),&ledger.stat.dev.to_string(),&ledger.stat.ino.to_string(),&ledger.stat.size.to_string(),&ledger.stat.modified_ns.to_be_bytes().as_slice(),&ledger.stat.changed_ns.map(|v|v.to_be_bytes().to_vec()),&ledger.prefix.as_slice(),&ledger.tail.as_slice()]).map_err(sqlstate::error)?;
    if files.len() != 1 {
        return Err(StorageError::Conflict);
    }
    // An earlier unacknowledged staged family must be reconciled through its
    // exact operation identity, never replayed as another generation. This
    // check is serialized by the same namespace lock as the first writer.
    let pending=tx.driver.query("SELECT projection_id FROM semon_pg_native_tree_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_id=$2 AND NOT complete", &[&ns,&expected_source]).map_err(sqlstate::error)?;
    let repair_projection = if pending.is_empty() {
        None
    } else {
        let marker = metadata(tx, format!("native_tree_failure:{path}").as_bytes())?;
        if !repair || pending.len() != 1 || marker.as_deref() != Some(receipt.as_str()) {
            return Err(StorageError::Conflict);
        }
        let id: i64 = pending[0].get(0);
        let valid:bool=tx.driver.query_one("SELECT receipt=$3 AND version=3 AND work_pending FROM semon_pg_native_tree_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND projection_id=$2", &[&ns,&id,&receipt.as_bytes()]).map_err(sqlstate::error)?.get(0);
        if !valid {
            return Err(StorageError::Conflict);
        }
        Some(id)
    };
    let file: i64 = files[0].get(0);
    let previous_receipt = expected_header.map(header_ledger);
    // An existing header must match the explicitly captured previous ledger.
    // An incomplete or noncurrent generation is not silently made current.
    let headers = tx.driver.query("SELECT h.projection_id,g.source_id FROM semon_pg_native_tree_headers h JOIN semon_pg_native_tree_generations g USING(namespace_digest,namespace,projection_id) WHERE h.namespace_digest=sha256($1::bytea) AND h.namespace=$1 AND h.source_path_digest=semon_pg_native_digest($1::bytea,$2::bytea,'tree.path') AND semon_pg_native_key_matches(h.source_path,sha256($2::bytea),$2) AND h.file_id=$3 AND h.ledger=$4 AND g.version=3 AND ((g.receipt=h.ledger AND g.complete AND NOT g.work_pending) OR (g.projection_id=$5 AND NOT g.complete AND g.work_pending))", &[&ns,&path.as_bytes(),&file,&previous_receipt.as_ref().map(|s|s.as_bytes()),&repair_projection]).map_err(sqlstate::error)?;
    let exists: bool = tx.driver.query_one(HEADER_EXISTS_SQL, &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if (exists && headers.len() != 1) || (!exists && expected_header.is_some()) {
        return Err(StorageError::Conflict);
    }
    let reuse = headers
        .first()
        .filter(|r| r.get::<_, i64>(1) == expected_source)
        .map(|r| r.get::<_, i64>(0))
        .or(repair_projection);
    let mut writer = Writer { tx };
    let projection = match reuse {
        Some(id) => {
            writer.effect("UPDATE semon_pg_native_tree_generations SET receipt=$4,complete=false,work_pending=true WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND projection_id=$2 AND source_id=$3 AND version=3 AND ((complete AND NOT work_pending) OR (projection_id=$5 AND NOT complete AND work_pending))", &[&ns,&id,&expected_source,&receipt.as_bytes(),&repair_projection])?;
            id
        }
        None => {
            let id: i64 = writer.tx.driver.query_one("SELECT coalesce(max(projection_id),0)+1 FROM semon_pg_native_tree_generations WHERE namespace_digest=sha256($1::bytea) AND namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0);
            writer.effect("INSERT INTO semon_pg_native_tree_generations(namespace,projection_id,source_id,version,receipt,complete,work_pending) VALUES($1,$2,$3,3,$4,false,true)", &[&ns,&id,&expected_source,&receipt.as_bytes()])?;
            id
        }
    };
    writer.values(projection, summary)?;
    let digest = persisted_witness(writer.tx, projection)?;
    writer.tx.check()?;
    Ok((projection, digest))
}

/// Only the complete bundle may call this private family operation. No file
/// ledger, Raw acknowledgement or Source revision is advanced here. The sealed
/// stage values never leave this call, so they cannot be reused across txns.
#[allow(dead_code)] // Complete native ingest bundle is still independently pending.
pub(super) fn publish(
    tx: &mut PgTxn<'_>,
    path: &str,
    ledger: &Ledger,
    expected_header: Option<&Ledger>,
    expected_source: i64,
    summary: &Summary,
    normalized_usage: bool,
) -> Result<bool, StorageError> {
    let _scope = selected_read::Scope::ensure();
    // Admission already holds the shared store lock, before SourceState. The
    // host exclusive lock cannot change pricing until this transaction ends.
    let (_, pricing) = super::store_metadata::read(tx)?;
    let ns = tx.namespace.as_str().as_bytes().to_vec();
    let locked = tx.driver.query("SELECT revision FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND namespace_digest=sha256($1::bytea) FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if locked.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let recipe = metadata(tx, b"catalog_semantic_recipe")?;
    let authority = tx.driver.query("SELECT s.usage_ready FROM semon_pg_native_usage_current c JOIN semon_pg_native_usage_sources s USING(namespace,source_id) WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.source_path_digest=semon_pg_native_digest($1,$2,'usage.path') AND semon_pg_native_key_matches(c.source_path,sha256($2::bytea),$2) AND s.source_id=$3", &[&ns,&path.as_bytes(),&expected_source]).map_err(sqlstate::error)?;
    if authority.len() != 1 || authority[0].get::<_, bool>(0) != normalized_usage {
        return Err(StorageError::Conflict);
    }
    let fingerprint_rows = tx.driver.query("SELECT octet_length(codex_pricing_fingerprint)::bigint FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(path,path_digest,$2)", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?;
    if fingerprint_rows.len() != 1 {
        return Err(StorageError::Conflict);
    }
    let bytes = fingerprint_rows[0].get::<_, Option<i64>>(0);
    if bytes.is_some_and(|n| !(0..=1048576).contains(&n)) {
        return Err(StorageError::ScopeChanged);
    }
    selected_read::charge(64, bytes.unwrap_or(0) as usize).map_err(|_| StorageError::Busy)?;
    let fingerprint:Option<Vec<u8>>=tx.driver.query_one("SELECT codex_pricing_fingerprint FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(path,path_digest,$2)", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    let fingerprint = fingerprint
        .map(String::from_utf8)
        .transpose()
        .map_err(|_| StorageError::ScopeChanged)?;
    let interpretation = crate::events::tree_rows::interpretation(&[pricing, recipe, fingerprint])
        .map_err(|_| StorageError::Unavailable)?;
    let (projection, digest) = stage_inner(
        tx,
        path,
        ledger,
        expected_header,
        expected_source,
        summary,
        true,
    )?;
    // The persisted witness is reverified after staging. This covers retained
    // rows and never substitutes a digest of the input Summary.
    if persisted_witness(tx, projection)? != digest {
        return Err(StorageError::Conflict);
    }
    let receipt = header_ledger(ledger);
    let record = crate::events::tree_rows::TreeSummaryRecord::new(
        receipt.clone(),
        Some(digest),
        interpretation,
        summary,
        normalized_usage,
    );
    let encoded = serde_json::to_vec(&record).map_err(|_| StorageError::Unavailable)?;
    let mut writer = Writer { tx };
    let failure = format!("native_tree_failure:{path}");
    if encoded.len() > 1048576 {
        super::native_keys::put_meta(writer.tx, failure.as_bytes(), receipt.as_bytes())?;
        writer.boundary()?;
        // Generation remains incomplete and cannot be returned by readers.
        let changed=writer.tx.driver.execute("UPDATE semon_pg_native_tree_repairs SET state=$3 WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND source_path_digest=semon_pg_native_digest($1::bytea,$2::bytea,'tree.path') AND semon_pg_native_key_matches(source_path,sha256($2::bytea),$2)", &[&ns,&path.as_bytes(),&b"blocked_metadata".as_slice()]).map_err(sqlstate::error)?;
        if changed > 1 {
            return Err(StorageError::Conflict);
        }
        writer.boundary()?;
        return Ok(false);
    }
    let key = format!("native_tree_summary:{path}");
    super::native_keys::put_meta(writer.tx, key.as_bytes(), &encoded)?;
    writer.boundary()?;
    super::native_keys::delete_meta(writer.tx, failure.as_bytes())?;
    writer.boundary()?;
    // SourceState is still locked and the exact file/Usage fences were matched
    // before the first generation effect. Publish header last in this txn.
    writer.effect("UPDATE semon_pg_native_tree_generations SET complete=true,work_pending=false WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND projection_id=$2 AND source_id=$3 AND receipt=$4 AND version=3 AND NOT complete AND work_pending", &[&ns,&projection,&expected_source,&receipt.as_bytes()])?;
    writer.effect("INSERT INTO semon_pg_native_tree_headers(namespace,source_path,file_id,projection_id,ledger) SELECT $1,$2,file_id,$3,$4 FROM semon_pg_native_files WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND path_digest=sha256($2::bytea) AND semon_pg_native_key_matches(path,path_digest,$2) ON CONFLICT(namespace_digest,source_path_digest) DO UPDATE SET file_id=excluded.file_id,projection_id=excluded.projection_id,ledger=excluded.ledger WHERE semon_pg_native_tree_headers.namespace=excluded.namespace AND semon_pg_native_key_matches(semon_pg_native_tree_headers.source_path,sha256(excluded.source_path),excluded.source_path)", &[&ns,&path.as_bytes(),&projection,&receipt.as_bytes()])?;
    Ok(true)
}

pub(super) fn metadata(tx: &mut PgTxn<'_>, key: &[u8]) -> Result<Option<String>, StorageError> {
    super::native_keys::read_meta(tx, key, 1048576)?
        .map(String::from_utf8)
        .transpose()
        .map_err(|_| StorageError::ScopeChanged)
}

// Read only persisted rows, bytea keyset order, one admitted row at a time.
// This is distinct from hashing the input delta: old closed/model rows survive.
pub(super) fn persisted_witness(
    tx: &mut PgTxn<'_>,
    projection: i64,
) -> Result<String, StorageError> {
    let mut writer = witness::Writer::new();
    for (family, key, columns, filter) in [
        ("labels", "label", "label,value", ""),
        ("models", "model", "model", ""),
        (
            "tools",
            "tool_id",
            "tool_id,name",
            " AND NOT closed AND name IS NOT NULL",
        ),
    ] {
        match family {
            "labels" => writer.labels(),
            "models" => writer.models(),
            _ => writer.tools(),
        }
        let predicate = format!(
            "FROM semon_pg_native_tree_{family} WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 AND projection_id=$2 AND ($3::bytea IS NULL OR {key}>$3){filter} ORDER BY {key} LIMIT 1"
        );
        let sizes = columns
            .split(',')
            .map(|col| format!("octet_length({col})::bigint"))
            .collect::<Vec<_>>()
            .join(",");
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            tx.check()?;
            selected_read::checkpoint("pg_tree_writer_witness").map_err(|_| StorageError::Busy)?;
            let lengths = tx
                .driver
                .query(
                    &format!("SELECT {sizes} {predicate}"),
                    &[&tx.namespace.as_str().as_bytes(), &projection, &cursor],
                )
                .map_err(sqlstate::error)?;
            let Some(lengths) = lengths.first() else {
                break;
            };
            let bytes = (0..lengths.len()).try_fold(0usize, |sum, at| {
                usize::try_from(lengths.get::<_, i64>(at))
                    .map(|n| sum.saturating_add(n))
                    .map_err(|_| StorageError::Unavailable)
            })?;
            // Charge eightfold dynamic cells plus fixed row/JSON framing before
            // the driver fetch or UTF-8/JSON decoding of the actual row.
            selected_read::charge(256, bytes).map_err(|_| StorageError::Busy)?;
            tx.check()?;
            let row = tx
                .driver
                .query_one(
                    &format!("SELECT {columns} {predicate}"),
                    &[&tx.namespace.as_str().as_bytes(), &projection, &cursor],
                )
                .map_err(sqlstate::error)?;
            #[cfg(test)]
            DECODED.with(|n| n.set(n.get() + 1));
            let first: Vec<u8> = row.get(0);
            let key = std::str::from_utf8(&first).map_err(|_| StorageError::Unavailable)?;
            match family {
                "models" => writer.model(key),
                _ => {
                    let value: Vec<u8> = row.get(1);
                    let value =
                        std::str::from_utf8(&value).map_err(|_| StorageError::Unavailable)?;
                    if family == "labels" {
                        writer.label(key, value);
                    } else {
                        writer.tool(key, value);
                    }
                }
            }
            cursor = Some(first);
        }
    }
    Ok(writer.finish())
}

#[cfg(test)]
mod tests;
