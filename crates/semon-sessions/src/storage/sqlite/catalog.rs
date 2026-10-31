//! Native Catalog SQL and snapshot lifecycle. Rendering/identity stay shared.
use crate::CatalogReadScope;
use crate::storage::catalog::CatalogSnapshot;
use crate::{
    Options,
    catalog::Request,
    model::summary::{CatalogRow, CatalogSource},
    viewer::ViewerReply,
};
use rusqlite::{Connection, OptionalExtension, Transaction, types::Value as SqlValue};
use serde_json::Value;
use std::io;

/// Two direct keyset ranges avoid scanning earlier keys when many sessions
/// share the same timestamp. OFFSET and mixed-order tuple comparisons do not
/// provide that property. All ranges are read in one SQLite snapshot.
pub(crate) fn rows(
    connection: &Connection,
    request: &Request,
) -> rusqlite::Result<Vec<CatalogRow>> {
    if let Some(sid) = &request.sid {
        return read_range(
            connection,
            "sqlite_autoindex_session_catalog_1",
            if request.require_complete && request.scope == CatalogReadScope::Current {
                "lifecycle='current' AND "
            } else {
                ""
            },
            "session_key = ?",
            vec![SqlValue::Text(sid.clone())],
            1,
        );
    }
    let (filters, index) = match (request.harness.is_some(), request.repo.is_some()) {
        (false, false) => ("", "session_catalog_order"),
        (true, false) => ("harness = ? AND ", "session_catalog_harness"),
        (false, true) => ("repo = ? AND ", "session_catalog_repo"),
        (true, true) => (
            "harness = ? AND repo = ? AND ",
            "session_catalog_harness_repo",
        ),
    };
    let mut filters = filters.to_owned();
    let mut index = index.to_owned();
    if request.repo.as_deref() == Some("") {
        filters = filters.replace("repo = ?", "repo IS NULL");
    }
    if request.model.is_some() {
        filters.push_str("json_extract(metadata,'$.model') = ? AND ");
        index.push_str("_model");
    }
    let (filters, index) = match request.scope {
        CatalogReadScope::Current => (
            format!("lifecycle='current' AND {filters}"),
            index.replacen("session_catalog_", "session_catalog_current_", 1),
        ),
        CatalogReadScope::RetainedHistory => (filters, index),
    };
    let mut base = Vec::new();
    if let Some(value) = &request.harness {
        base.push(SqlValue::Text(value.clone()));
    }
    if let Some(value) = &request.repo
        && !value.is_empty()
    {
        base.push(SqlValue::Text(value.clone()));
    }
    if let Some(value) = &request.model {
        base.push(SqlValue::Text(value.clone()));
    }
    let limit = request.limit + 1;
    let mut found = if let Some(cursor) = &request.cursor {
        let mut args = base.clone();
        args.push(SqlValue::Integer(cursor.last));
        args.push(SqlValue::Text(cursor.key.clone()));
        let mut found = read_range(
            connection,
            &index,
            &filters,
            if cursor.inclusive {
                "last_ms = ? AND session_key >= ?"
            } else {
                "last_ms = ? AND session_key > ?"
            },
            args,
            limit,
        )?;
        let remaining = limit - found.len();
        if remaining > 0 {
            let mut args = base;
            args.push(SqlValue::Integer(cursor.last));
            found.extend(read_range(
                connection,
                &index,
                &filters,
                "last_ms < ?",
                args,
                remaining,
            )?);
        }
        found
    } else {
        read_range(connection, &index, &filters, "1", base, limit)?
    };
    // Capacity returned by SQLite is bounded by the requested page plus one.
    found.shrink_to_fit();
    Ok(found)
}

#[cfg(test)]
thread_local! {
    pub(crate) static SQL_STEPS: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

fn read_range(
    connection: &Connection,
    index: &str,
    filters: &str,
    range: &str,
    mut args: Vec<SqlValue>,
    limit: usize,
) -> rusqlite::Result<Vec<CatalogRow>> {
    // The current namespace schema leads every Catalog index with namespace,
    // including the (namespace,session_key) primary key autoindex.
    let (table, namespace_filter) = ("main.session_catalog", "namespace=semon_namespace() AND ");
    let sql = format!(
        "SELECT metadata,session_key,last_ms,harness,repo FROM {table} INDEXED BY {index} WHERE {namespace_filter}{filters}{range} ORDER BY last_ms DESC,session_key ASC LIMIT ?"
    );
    args.push(SqlValue::Integer(limit as i64));
    let mut statement = connection.prepare(&sql)?;
    let mut found = Vec::new();
    {
        let mut selected = statement.query(rusqlite::params_from_iter(args))?;
        while let Some(row) = selected.next()? {
            let encoded: String = row.get(0)?;
            let metadata: CatalogRow = serde_json::from_str(&encoded).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            if metadata.key != row.get::<_, String>(1)?
                || metadata.last.unwrap_or(0) != row.get::<_, i64>(2)?
                || metadata.harness != row.get::<_, String>(3)?
                || metadata.repo != row.get::<_, Option<String>>(4)?
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            found.push(metadata);
        }
    }
    #[cfg(test)]
    SQL_STEPS.with(|steps| {
        steps.set(steps.get() + statement.get_status(rusqlite::StatementStatus::VmStep))
    });
    Ok(found)
}

pub(crate) fn read_page(
    binding: &crate::storage::NativeReaderBinding,
    options: &Options,
    machine: &str,
    request: &Request,
    admit_search: bool,
    admission: &mut Option<crate::catalog::CatalogSearchAdmission>,
) -> Result<ViewerReply, Box<dyn std::error::Error>> {
    let mut connection = binding.connection()?;
    let transaction = connection.transaction()?;
    let reply = crate::catalog::read_page_snapshot_with_search(
        options,
        machine,
        request,
        &admitted_snapshot(binding, &transaction)?,
        admit_search,
        admission,
    )
    .inspect_err(|cause| {
        if crate::native_timing::enabled() {
            let sqlite_error = cause.downcast_ref::<rusqlite::Error>().or_else(|| {
                cause
                    .downcast_ref::<io::Error>()?
                    .get_ref()?
                    .downcast_ref::<rusqlite::Error>()
            });
            let details = sqlite_error.map(super::error_class);
            let kind = details.map(|(kind, _)| kind);
            let extended_code = details.and_then(|(_, code)| code);
            eprintln!(
                "semon sessions native: catalog_page_sqlite_refused kind={kind:?} extended_code={extended_code:?}"
            );
        }
    })?;
    transaction.commit()?;
    Ok(reply)
}

pub(crate) fn page_ready(binding: &crate::storage::NativeReaderBinding) -> io::Result<bool> {
    let mut connection = binding.connection().map_err(io::Error::other)?;
    connection
        .busy_timeout(std::time::Duration::from_millis(100))
        .map_err(io::Error::other)?;
    let transaction = connection.transaction().map_err(io::Error::other)?;
    let snapshot = admitted_snapshot(binding, &transaction).map_err(io::Error::other)?;
    snapshot.require_current()?;
    if snapshot.publication_value("catalog_generation")?.is_none() {
        return Ok(false);
    }
    let version = snapshot.publication_value("catalog_version")?;
    let expected_version = crate::model::summary::CATALOG_VERSION.to_string();
    if version.as_deref() != Some(expected_version.as_str()) {
        return Err(io::Error::other("catalog version"));
    }
    drop(snapshot);
    transaction.commit().map_err(io::Error::other)?;
    Ok(true)
}

pub(crate) fn history_identity(
    binding: &crate::storage::NativeReaderBinding,
    options: &Options,
    source: &str,
    key: &str,
) -> Result<Option<crate::CatalogSessionIdentity>, crate::CatalogIdentityError> {
    let mut db = binding
        .connection()
        .map_err(crate::CatalogIdentityError::from)?;
    let tx = db
        .transaction()
        .map_err(|_| crate::CatalogIdentityError::Unavailable)?;
    let selected = crate::catalog::catalog_selection_in_snapshot(
        options,
        source,
        key,
        CatalogReadScope::RetainedHistory,
        &admitted_snapshot(binding, &tx).map_err(crate::CatalogIdentityError::from)?,
    )?;
    tx.commit()
        .map_err(|_| crate::CatalogIdentityError::Unavailable)?;
    Ok(selected.map(|(identity, _)| identity))
}

pub(crate) fn source_proof_observation(
    binding: &crate::storage::NativeReaderBinding,
    _options: &Options,
    root: &str,
    absolute: &str,
) -> Result<Option<crate::storage::catalog::SourceProofObservation>, Box<dyn std::error::Error>> {
    let mut connection = binding.connection()?;
    let transaction = connection.transaction()?;
    if !binding.validate(&transaction, &binding.config().namespace)? {
        return Err(io::Error::other("namespace reader unqualified").into());
    }
    let sql = "SELECT c.session_key,CASE WHEN length(CAST(c.metadata AS BLOB))<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s INDEXED BY session_catalog_current_source_path JOIN main.session_catalog c ON c.namespace=s.namespace AND c.session_key=s.session_key WHERE s.namespace=semon_namespace() AND s.lifecycle='current' AND s.source_path=?1 ORDER BY s.session_key LIMIT 1";
    let encoded: Option<(String, Option<String>)> = transaction
        .query_row(sql, [absolute], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((key, encoded)) = encoded else {
        return Ok(None);
    };
    let encoded = encoded.ok_or_else(|| io::Error::other("source metadata bound"))?;
    // Decode only the selected relationship metadata, never a native body.
    if encoded.len() > 1024 * 1024 {
        return Err(io::Error::other("source metadata bound").into());
    }
    let row: CatalogRow = serde_json::from_str(&encoded)?;
    if row.key != key || row.harness != root || row.sources.len() > 64 {
        return Err(io::Error::other("source mapping changed").into());
    }
    let source = row
        .sources
        .iter()
        .find(|source| source.path.to_str() == Some(absolute))
        .ok_or_else(|| io::Error::other("source mapping absent"))?;
    type Revision = (
        u64,
        u64,
        u64,
        [u8; 16],
        u64,
        [u8; 32],
        [u8; 32],
        Option<[u8; 16]>,
    );
    let revision: Option<Revision> = transaction.query_row(
        "SELECT dev,ino,size,mtime_ns,resume_at,head_sha256,tail_sha256,ctime_ns FROM main.files WHERE namespace=semon_namespace() AND path=?1",
        [absolute], |row| Ok((row.get::<_,i64>(0)? as u64,row.get::<_,i64>(1)? as u64,row.get::<_,i64>(2)? as u64,row.get(3)?,row.get::<_,i64>(4)? as u64,row.get(5)?,row.get(6)?,row.get(7)?)),
    ).optional()?;
    if revision
        != Some((
            source.dev,
            source.ino,
            source.size,
            source.modified_ns.to_be_bytes(),
            source.offset,
            source.prefix_sha256,
            source.tail_sha256,
            source.changed_ns.map(i128::to_be_bytes),
        ))
    {
        return Err(io::Error::other("source ledger changed").into());
    }
    let generation: String = transaction.query_row(
        "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key='catalog_generation'",
        [],
        |row| row.get(0),
    )?;
    transaction.commit()?;
    Ok(Some(crate::storage::catalog::SourceProofObservation {
        key,
        source: source.clone(),
        generation,
    }))
}

pub(crate) struct AdmittedSnapshot<'a, 'b>(
    &'a Transaction<'b>,
    Option<crate::storage::QueryRuntimeBinding>,
    crate::storage::QueryRuntimeBinding,
    std::cell::OnceCell<crate::native_evidence::RuntimeObservations>,
);

/// Metadata from the caller's validated snapshot, never an Options-derived
/// connection. Each CWD belongs to the exact published native prefix.
pub(crate) fn runtime_source_cwds(
    db: &Connection,
    sources: &[CatalogSource],
) -> io::Result<std::collections::BTreeSet<String>> {
    if sources.is_empty() || sources.len() > 64 {
        return Err(io::Error::other(crate::storage::StorageError::ScopeChanged));
    }
    let mut cwds = std::collections::BTreeSet::new();
    for source in sources {
        let path = source.path.to_string_lossy();
        let ledger = crate::events::store::ledger_row(db, &path).map_err(io::Error::other)?;
        if ledger.as_ref().map(|(_, ledger)| ledger) != Some(&crate::source_carrier::ledger(source))
        {
            return Err(io::Error::other(crate::storage::StorageError::ScopeChanged));
        }
        let cwd: Option<String> = db
            .query_row(
                "SELECT cwd FROM main.files WHERE namespace=semon_namespace() AND path=?1",
                [path.as_ref()],
                |row| row.get(0),
            )
            .map_err(io::Error::other)?;
        if let Some(cwd) = cwd {
            cwds.insert(cwd);
        }
    }
    Ok(cwds)
}

/// Bound Home/Query readers select through the same admitted snapshot as
/// Catalog pages. Only the current namespace schema is readable.
pub(crate) fn admitted_snapshot<'a, 'b>(
    binding: &crate::storage::NativeReaderBinding,
    transaction: &'a Transaction<'b>,
) -> Result<AdmittedSnapshot<'a, 'b>, crate::storage::StorageError> {
    if !binding.validate(transaction, &binding.config().namespace)? {
        return Err(crate::storage::StorageError::NamespaceUpgradePending);
    }
    Ok(AdmittedSnapshot(
        transaction,
        binding.runtime_binding()?,
        binding.runtime_binding_at(transaction)?,
        std::cell::OnceCell::new(),
    ))
}

impl CatalogSnapshot for AdmittedSnapshot<'_, '_> {
    fn runtime_observation(
        &self,
        options: &Options,
        row: &CatalogRow,
        _runtime: &str,
    ) -> io::Result<Option<String>> {
        let binding = self.row_runtime_binding(row)?;
        let cwds = runtime_source_cwds(self.0, &row.sources)?;
        let scoped = self
            .3
            .get_or_init(|| crate::native_evidence::RuntimeObservations::new(options))
            .scope(row, &cwds)?;
        binding.map(|binding| binding.witness(&scoped)).transpose()
    }
    fn runtime_binding(&self) -> Option<crate::storage::QueryRuntimeBinding> {
        self.1.clone()
    }
    fn row_runtime_binding(
        &self,
        row: &CatalogRow,
    ) -> io::Result<Option<crate::storage::QueryRuntimeBinding>> {
        if super::proof_generations::imported_catalog(self.0, &self.2).map_err(io::Error::other)?
            || row
                .query
                .as_ref()
                .and_then(|query| query["runtime"].as_str())
                .is_some_and(|saved| saved.starts_with("source-runtime-v1:"))
        {
            return super::proof_generations::observation_binding(self.0, &self.2, row)
                .map(Some)
                .map_err(io::Error::other);
        }
        Ok(self.runtime_binding())
    }
    fn facts_authority<'a>(
        &'a self,
        path: &std::path::Path,
    ) -> io::Result<Box<dyn crate::facts::source_authority::SourceAuthority + 'a>> {
        Ok(Box::new(crate::facts::source_authority::Sources::sqlite(
            path,
        )?))
    }
    fn require_current(&self) -> io::Result<()> {
        Ok(())
    }
    fn publication_value(&self, key: &str) -> io::Result<Option<String>> {
        if key == "catalog_backfill_state" {
            return self.0.query_row(
                "SELECT backfill_state FROM main.storage_source_journal WHERE namespace=semon_namespace()",
                [], |row| row.get(0),
            ).map_err(io::Error::other);
        }
        let sql = "SELECT value FROM main.meta WHERE namespace=semon_namespace() AND key=?1";
        let tx = self.0;
        tx.query_row(sql, [key], |row| row.get(0))
            .optional()
            .map_err(io::Error::other)
    }
    fn catalog_search_paths(
        &self,
        lookup: &crate::events::CatalogSearchLookup<'_>,
        limit: usize,
    ) -> io::Result<Vec<String>> {
        super::source_journal::catalog_search_paths(
            self.0,
            self.2.namespace.as_str(),
            lookup,
            limit,
        )
        .map_err(io::Error::other)
    }
    fn rows(&self, request: &Request) -> io::Result<Vec<CatalogRow>> {
        rows(self.0, request).map_err(io::Error::other)
    }
    fn search(
        &self,
        query: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection> {
        crate::catalog_search::read_admitted(self.0, query).map_err(|cause| {
            #[cfg(test)]
            eprintln!("catalog_snapshot_search_failed: {cause}");
            io::Error::other(cause)
        })
    }
    fn has_history(&self, key: &str) -> io::Result<bool> {
        let sql = "SELECT EXISTS(SELECT 1 FROM main.session_history_catalog WHERE namespace=semon_namespace() AND session_key=?1)";
        let tx = self.0;
        tx.query_row(sql, [key], |row| row.get(0))
            .map_err(io::Error::other)
    }
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>> {
        crate::history_projection::catalog(self.0, key).map_err(io::Error::other)
    }
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool> {
        crate::history_projection::qualified(self.0, row).map_err(io::Error::other)
    }
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow) -> Value,
    ) -> io::Result<Option<Value>> {
        // The owned lookup scopes every selected table to this namespace.
        crate::storage::selected::selected_details(
            &super::selected::Context::owned(self.0),
            key,
            sources,
            &mut |row| status(row),
        )
    }
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<Value>> {
        crate::transcript_range::observation(self.0, key, generation, history)
            .map_err(io::Error::other)
    }
}
