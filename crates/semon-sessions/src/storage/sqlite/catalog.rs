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
    qualified: bool,
) -> rusqlite::Result<Vec<CatalogRow>> {
    if let Some(sid) = &request.sid {
        return read_range(
            connection,
            qualified,
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
            qualified,
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
                qualified,
                &index,
                &filters,
                "last_ms < ?",
                args,
                remaining,
            )?);
        }
        found
    } else {
        read_range(connection, qualified, &index, &filters, "1", base, limit)?
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
    qualified: bool,
    index: &str,
    filters: &str,
    range: &str,
    mut args: Vec<SqlValue>,
    limit: usize,
) -> rusqlite::Result<Vec<CatalogRow>> {
    let (table, index, namespace_filter) = if qualified {
        (
            "main.session_catalog",
            if index == "sqlite_autoindex_session_catalog_1" {
                "storage_ns_catalog_identity".to_owned()
            } else {
                format!("storage_ns_{index}")
            },
            "namespace=semon_namespace() AND ",
        )
    } else {
        ("session_catalog", index.to_owned(), "")
    };
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
) -> Result<ViewerReply, Box<dyn std::error::Error>> {
    let mut connection = binding.connection()?;
    let transaction = connection.transaction()?;
    let qualified = binding.validate(&transaction, &binding.config().namespace)?;
    let reply = crate::catalog::read_page_snapshot(
        options,
        machine,
        request,
        &AdmittedSnapshot(&transaction, qualified),
    )?;
    transaction.commit()?;
    Ok(reply)
}

pub(crate) fn source_proof_observation(
    binding: &crate::storage::NativeReaderBinding,
    _options: &Options,
    root: &str,
    absolute: &str,
) -> Result<Option<crate::storage::catalog::SourceProofObservation>, Box<dyn std::error::Error>> {
    let mut connection = binding.connection()?;
    let transaction = connection.transaction()?;
    let qualified = binding.validate(&transaction, &binding.config().namespace)?;
    let sql = if qualified {
        "SELECT c.session_key,CASE WHEN length(CAST(c.metadata AS BLOB))<=1048576 THEN c.metadata ELSE NULL END FROM main.session_catalog_sources s INDEXED BY storage_ns_session_catalog_current_source_path JOIN main.session_catalog c ON c.namespace=s.namespace AND c.session_key=s.session_key WHERE s.namespace=semon_namespace() AND s.lifecycle='current' AND s.source_path=?1 ORDER BY s.session_key LIMIT 1"
    } else {
        "SELECT c.session_key,CASE WHEN length(CAST(c.metadata AS BLOB))<=1048576 THEN c.metadata ELSE NULL END FROM session_catalog_sources s INDEXED BY session_catalog_current_source_path JOIN session_catalog c ON c.session_key=s.session_key WHERE s.lifecycle='current' AND s.source_path=?1 ORDER BY s.session_key LIMIT 1"
    };
    let encoded: Option<(String, Option<String>)> = transaction.query_row(
        sql,
        [absolute], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
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
        "SELECT dev,ino,size,mtime_ns,resume_at,head_sha256,tail_sha256,ctime_ns FROM files WHERE path=?1",
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
        "SELECT value FROM meta WHERE key='catalog_generation'",
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

impl CatalogSnapshot for Transaction<'_> {
    fn require_current(&self) -> io::Result<()> {
        crate::events::store::require_current(self)
    }
    fn publication_value(&self, key: &str) -> io::Result<Option<String>> {
        self.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(io::Error::other)
    }
    fn rows(&self, request: &Request) -> io::Result<Vec<CatalogRow>> {
        rows(self, request, false).map_err(io::Error::other)
    }
    fn search(
        &self,
        query: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection> {
        crate::catalog_search::read(self, query).map_err(io::Error::other)
    }
    fn has_history(&self, key: &str) -> io::Result<bool> {
        self.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_history_catalog WHERE session_key=?1)",
            [key],
            |row| row.get(0),
        )
        .map_err(io::Error::other)
    }
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>> {
        crate::history_projection::catalog(self, key).map_err(io::Error::other)
    }
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool> {
        crate::history_projection::qualified(self, row).map_err(io::Error::other)
    }
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow) -> Value,
    ) -> io::Result<Option<Value>> {
        crate::causal_trace::selected_details(self, key, sources, &mut |row| status(row))
            .map_err(io::Error::other)
    }
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<Value>> {
        crate::transcript_range::observation(self, key, generation, history)
            .map_err(io::Error::other)
    }
}

struct AdmittedSnapshot<'a, 'b>(&'a Transaction<'b>, bool);

fn require_ported_helper(qualified: bool) -> io::Result<()> {
    if qualified {
        return Err(io::Error::other(crate::storage::StorageError::NamespaceUpgradePending));
    }
    Ok(())
}
impl CatalogSnapshot for AdmittedSnapshot<'_, '_> {
    fn require_current(&self) -> io::Result<()> {
        Ok(())
    }
    fn publication_value(&self, key: &str) -> io::Result<Option<String>> {
        self.0.publication_value(key)
    }
    fn rows(&self, request: &Request) -> io::Result<Vec<CatalogRow>> {
        rows(self.0, request, self.1).map_err(io::Error::other)
    }
    fn search(
        &self,
        query: crate::catalog_search::Query<'_>,
    ) -> io::Result<crate::catalog_search::Selection> {
        require_ported_helper(self.1)?;
        self.0.search(query)
    }
    fn has_history(&self, key: &str) -> io::Result<bool> {
        self.0.has_history(key)
    }
    fn history_catalog(&self, key: &str) -> io::Result<Option<String>> {
        self.0.history_catalog(key)
    }
    fn history_qualified(&self, row: &CatalogRow) -> io::Result<bool> {
        require_ported_helper(self.1)?;
        self.0.history_qualified(row)
    }
    fn selected_details(
        &self,
        key: &str,
        sources: &[CatalogSource],
        status: &mut dyn FnMut(&CatalogRow) -> Value,
    ) -> io::Result<Option<Value>> {
        require_ported_helper(self.1)?;
        self.0.selected_details(key, sources, status)
    }
    fn transcript_observation(
        &self,
        key: &str,
        generation: &str,
        history: bool,
    ) -> io::Result<Option<Value>> {
        require_ported_helper(self.1)?;
        self.0.transcript_observation(key, generation, history)
    }
}

#[cfg(test)]
mod namespace_reader_tests {
    use super::*;

    #[test]
    fn unported_indexed_reader_refuses_with_typed_storage_error_before_sql() {
        // No native tables exist: reaching a legacy helper would be an SQL
        // error, rather than the required typed namespace refusal.
        let mut db = Connection::open_in_memory().unwrap();
        let tx = db.transaction().unwrap();
        let snapshot = AdmittedSnapshot(&tx, true);
        let error = snapshot.transcript_observation("same", "generation", true).unwrap_err();
        assert_eq!(
            error.get_ref().unwrap().downcast_ref::<crate::storage::StorageError>(),
            Some(&crate::storage::StorageError::NamespaceUpgradePending)
        );
        assert!(require_ported_helper(false).is_ok());
    }
}
