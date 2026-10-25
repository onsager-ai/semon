//! Private, incomplete Tree generation writer. Scalar completion is deliberately
//! closed: the current PG contract has no store-owned pricing metadata owner.
//! Staging never publishes a header, a complete generation, or an ingest ACK.
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
        if self.tx.driver.execute(query, args).map_err(sqlstate::error)? != 1 {
            return Err(StorageError::Conflict);
        }
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
        self.effect(label, &[&ns, &projection, &b"codex_turn".as_slice(), &rows.codex_turn.as_bytes()])?;
        for (key, value) in rows.labels() {
            self.effect(label, &[&ns, &projection, &key.as_bytes(), &value.as_bytes()])?;
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
            self.effect("INSERT INTO semon_pg_native_tree_usage_models(namespace,projection_id,message_id,model) VALUES($1,$2,$3,$4) ON CONFLICT(namespace_digest,projection_id,message_id) DO UPDATE SET model=excluded.model", &[&ns,&projection,&id.as_bytes(),&model.as_bytes()])?;
        }
        Ok(())
    }
}

/// Stage real persisted dimensions, returning their generation and canonical
/// witness to the future complete bundle. The caller MUST propagate errors to
/// its existing WriteTxn. No header or scalar is published here. In particular,
/// this is not a successful partial IndexStore or an ingest receipt.
///
/// Missing prerequisite: a verified store-owned `pricing_recompute_target`
/// metadata port/lock alongside source-owned `catalog_semantic_recipe`. PG's
/// present source-only meta RLS cannot represent the first owner. Until that
/// exists there is intentionally no scalar/completion constructor to call.
#[allow(dead_code)] // Narrow private family entry, awaiting that complete bundle.
pub(super) fn stage(
    tx: &mut PgTxn<'_>,
    path: &str,
    ledger: &Ledger,
    expected_header: Option<&Ledger>,
    expected_source: i64,
    summary: &Summary,
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
    let locked = tx.driver.query("SELECT revision FROM semon_pg_source_state WHERE namespace=$1 AND namespace_digest=sha256($1::bytea) FOR UPDATE", &[&ns]).map_err(sqlstate::error)?;
    if locked.len() != 1 {
        return Err(StorageError::ScopeChanged);
    }
    let receipt = header_ledger(ledger);
    // Only fixed-size physical IDs cross this query. Match the complete file
    // ledger and current usage generation/readiness in SQL before any effect.
    let files = tx.driver.query("SELECT f.file_id FROM semon_pg_native_files f JOIN semon_pg_native_usage_current c ON c.namespace=f.namespace AND c.source_path=f.path JOIN semon_pg_native_usage_sources s ON s.namespace=c.namespace AND s.source_id=c.source_id AND s.source_path=c.source_path WHERE f.namespace=$1 AND f.path=$2 AND c.source_id=$3 AND s.parser_version=$4 AND s.receipt=$5 AND s.usage_ready AND s.checkpoint=$6::text::numeric AND f.resume_at=$6::text::numeric AND f.dev=$7::text::numeric AND f.ino=$8::text::numeric AND f.size=$9::text::numeric AND f.mtime_ns=$10 AND f.ctime_ns IS NOT DISTINCT FROM $11 AND f.head_sha256=$12 AND f.tail_sha256=$13", &[&ns,&path.as_bytes(),&expected_source,&i64::from(CACHE_VERSION),&receipt.as_bytes(),&ledger.offset.to_string(),&ledger.stat.dev.to_string(),&ledger.stat.ino.to_string(),&ledger.stat.size.to_string(),&ledger.stat.modified_ns.to_be_bytes().as_slice(),&ledger.stat.changed_ns.map(|v|v.to_be_bytes().to_vec()),&ledger.prefix.as_slice(),&ledger.tail.as_slice()]).map_err(sqlstate::error)?;
    if files.len() != 1 {
        return Err(StorageError::Conflict);
    }
    let file: i64 = files[0].get(0);
    let previous_receipt = expected_header.map(header_ledger);
    // An existing header must match the explicitly captured previous ledger.
    // An incomplete or noncurrent generation is not silently made current.
    let headers = tx.driver.query("SELECT h.projection_id,g.source_id FROM semon_pg_native_tree_headers h JOIN semon_pg_native_tree_generations g USING(namespace,projection_id) WHERE h.namespace=$1 AND h.source_path=$2 AND h.file_id=$3 AND h.ledger=$4 AND g.receipt=h.ledger AND g.version=3 AND g.complete AND NOT g.work_pending", &[&ns,&path.as_bytes(),&file,&previous_receipt.as_ref().map(|s|s.as_bytes())]).map_err(sqlstate::error)?;
    let exists: bool = tx.driver.query_one("SELECT EXISTS(SELECT 1 FROM semon_pg_native_tree_headers WHERE namespace=$1 AND source_path=$2)", &[&ns,&path.as_bytes()]).map_err(sqlstate::error)?.get(0);
    if (exists && headers.len() != 1) || (!exists && expected_header.is_some()) {
        return Err(StorageError::Conflict);
    }
    let reuse = headers.first().filter(|r| r.get::<_, i64>(1) == expected_source).map(|r| r.get::<_, i64>(0));
    let mut writer = Writer { tx };
    let projection = match reuse {
        Some(id) => {
            writer.effect("UPDATE semon_pg_native_tree_generations SET receipt=$4,complete=false,work_pending=true WHERE namespace=$1 AND projection_id=$2 AND source_id=$3 AND version=3 AND complete AND NOT work_pending", &[&ns,&id,&expected_source,&receipt.as_bytes()])?;
            id
        }
        None => {
            let id: i64 = writer.tx.driver.query_one("SELECT coalesce(max(projection_id),0)+1 FROM semon_pg_native_tree_generations WHERE namespace=$1", &[&ns]).map_err(sqlstate::error)?.get(0);
            writer.effect("INSERT INTO semon_pg_native_tree_generations(namespace,projection_id,source_id,version,receipt,complete,work_pending) VALUES($1,$2,$3,3,$4,false,true)", &[&ns,&id,&expected_source,&receipt.as_bytes()])?;
            id
        }
    };
    writer.values(projection, summary)?;
    let digest = persisted_witness(writer.tx, projection)?;
    writer.tx.check()?;
    Ok((projection, digest))
}

// Read only persisted rows, bytea keyset order, one admitted row at a time.
// This is distinct from hashing the input delta: old closed/model rows survive.
fn persisted_witness(tx: &mut PgTxn<'_>, projection: i64) -> Result<String, StorageError> {
    let mut writer = witness::Writer::new();
    for (family, key, columns, filter) in [
        ("labels", "label", "label,value", ""),
        ("models", "model", "model", ""),
        ("tools", "tool_id", "tool_id,name", " AND NOT closed AND name IS NOT NULL"),
    ] {
        match family {
            "labels" => writer.labels(),
            "models" => writer.models(),
            _ => writer.tools(),
        }
        let predicate = format!("FROM semon_pg_native_tree_{family} WHERE namespace=$1 AND projection_id=$2 AND ($3::bytea IS NULL OR {key}>$3){filter} ORDER BY {key} LIMIT 1");
        let sizes = columns.split(',').map(|col|format!("octet_length({col})::bigint")).collect::<Vec<_>>().join(",");
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            tx.check()?;
            selected_read::checkpoint("pg_tree_writer_witness").map_err(|_| StorageError::Busy)?;
            let lengths = tx.driver.query(&format!("SELECT {sizes} {predicate}"), &[&tx.namespace.as_str().as_bytes(),&projection,&cursor]).map_err(sqlstate::error)?;
            let Some(lengths) = lengths.first() else { break; };
            let bytes = (0..lengths.len()).try_fold(0usize, |sum, at| {
                usize::try_from(lengths.get::<_, i64>(at)).map(|n|sum.saturating_add(n)).map_err(|_|StorageError::Unavailable)
            })?;
            // Charge eightfold dynamic cells plus fixed row/JSON framing before
            // the driver fetch or UTF-8/JSON decoding of the actual row.
            selected_read::charge(256, bytes).map_err(|_| StorageError::Busy)?;
            tx.check()?;
            let row = tx.driver.query_one(&format!("SELECT {columns} {predicate}"), &[&tx.namespace.as_str().as_bytes(),&projection,&cursor]).map_err(sqlstate::error)?;
            #[cfg(test)]
            DECODED.with(|n|n.set(n.get()+1));
            let first: Vec<u8> = row.get(0);
            let key = std::str::from_utf8(&first).map_err(|_|StorageError::Unavailable)?;
            match family {
                "models" => writer.model(key),
                _ => {
                    let value: Vec<u8> = row.get(1);
                    let value = std::str::from_utf8(&value).map_err(|_|StorageError::Unavailable)?;
                    if family == "labels" { writer.label(key,value); } else { writer.tool(key,value); }
                }
            }
            cursor = Some(first);
        }
    }
    Ok(writer.finish())
}

#[cfg(test)]
mod tests;
