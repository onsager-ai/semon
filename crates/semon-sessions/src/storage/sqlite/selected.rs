//! SQLite selected-context lookups. Connection and dialect stay in this adapter.
use crate::storage::selected::ContextLookup;
use rusqlite::{Connection, OptionalExtension, params};
use std::io;

pub(crate) struct Context<'a> {
    db: &'a Connection,
    owned: bool,
}
impl<'a> Context<'a> {
    pub(crate) fn local(db: &'a Connection) -> Self {
        Self { db, owned: false }
    }
    pub(crate) fn owned(db: &'a Connection) -> Self {
        Self { db, owned: true }
    }
    // Only authored statements below enter this adapter-local scope helper.
    // No SQL text or callback is exposed through the metadata port.
    fn sql(&self, sql: &str) -> String {
        if !self.owned {
            return sql.to_owned();
        }
        let mut sql = sql.to_owned();
        for table in [
            "session_trace_headers",
            "session_trace_turns",
            "session_trace_edges",
            "session_history_catalog",
            "session_catalog",
        ] {
            sql = sql.replace(
                table,
                &format!("(SELECT * FROM main.{table} WHERE namespace=semon_namespace())"),
            );
        }
        sql
    }
}
impl ContextLookup for Context<'_> {
    fn details(&self, key: &str, source: &str) -> io::Result<Option<(Option<String>, bool)>> {
        if !self.db.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name='session_trace_headers')", [], |r|r.get::<_,bool>(0)).map_err(io::Error::other)? { return Ok(None); }
        self.db.query_row(&self.sql("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN json_set(json_extract(metadata,'$.details'),'$.busy',json_extract(metadata,'$.busy'),'$.content_generation',json_extract(metadata,'$.content_generation')) ELSE NULL END,EXISTS(SELECT 1 FROM session_catalog c WHERE c.session_key=?1 AND c.lifecycle='current') FROM session_trace_headers WHERE session_key=?1 AND source_generation=?2"), params![key,source], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(io::Error::other)
    }
    fn relative(&self, key: &str) -> io::Result<Option<String>> {
        self.db.query_row(&self.sql("SELECT CASE WHEN octet_length(COALESCE(h.metadata,c.metadata))<=1048576 THEN json_object('key',c.session_key,'name',json_extract(COALESCE(h.metadata,c.metadata),'$.name'),'harness',c.harness,'kind',COALESCE(json_extract(COALESCE(h.metadata,c.metadata),'$.presentation_kind'),json_extract(COALESCE(h.metadata,c.metadata),'$.kind')),'parent',json_extract(COALESCE(h.metadata,c.metadata),'$.parent'),'last',json_extract(COALESCE(h.metadata,c.metadata),'$.last'),'cost',json_extract(COALESCE(h.metadata,c.metadata),'$.cost'),'_status_catalog',json_set(COALESCE(h.metadata,c.metadata),'$.lifecycle',c.lifecycle)) ELSE NULL END FROM session_catalog c LEFT JOIN session_history_catalog h USING(session_key) WHERE c.session_key=?1"), [key], |r| r.get(0)).optional().map(Option::flatten).map_err(io::Error::other)
    }
    fn descendants(&self, key: &str) -> io::Result<Vec<String>> {
        self.db.prepare(&self.sql("WITH RECURSIVE family(session_key) AS (VALUES(?1) UNION SELECT c.session_key FROM session_catalog c JOIN family f ON c.parent_key=f.session_key LIMIT 101) SELECT session_key FROM family WHERE session_key!=?1")).map_err(io::Error::other)?.query_map([key], |r| r.get(0)).map_err(io::Error::other)?.collect::<rusqlite::Result<_>>().map_err(io::Error::other)
    }
    fn header(&self, key: &str) -> io::Result<Option<(String, bool)>> {
        self.db.query_row(&self.sql("SELECT source_generation,COALESCE(json_extract(metadata,'$.relationships_incomplete'),0) FROM session_trace_headers WHERE session_key=?1"),[key],|r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(io::Error::other)
    }
    fn turn(&self, key: &str, id: &str) -> io::Result<Option<String>> {
        self.db.query_row(&self.sql("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_trace_turns WHERE session_key=?1 AND turn_id=?2"),params![key,id],|r|r.get(0)).optional().map_err(io::Error::other)
    }
    fn edge(&self, key: &str, id: &str) -> io::Result<Option<String>> {
        self.db.query_row(&self.sql("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata ELSE NULL END FROM session_trace_edges WHERE session_key=?1 AND edge_id=?2"),params![key,id],|r|r.get(0)).optional().map_err(io::Error::other)
    }
    fn target_turns(&self, key: &str, id: &str) -> io::Result<Vec<String>> {
        self.db.prepare(&self.sql("SELECT turn_id FROM session_trace_turns WHERE session_key=?1 AND start_id=?2 ORDER BY turn_id LIMIT 2")).map_err(io::Error::other)?.query_map(params![key,id], |r|r.get(0)).map_err(io::Error::other)?.collect::<rusqlite::Result<_>>().map_err(io::Error::other)
    }
    fn participant(&self, key: &str) -> io::Result<Option<String>> {
        self.db.query_row(&self.sql("SELECT CASE WHEN octet_length(metadata)<=1048576 THEN json_object('name',json_extract(metadata,'$.name'),'harness',json_extract(metadata,'$.harness'),'model',json_extract(metadata,'$.model'),'kind',json_extract(metadata,'$.kind'),'state',json_extract(metadata,'$.state'),'start',json_extract(metadata,'$.start'),'last',json_extract(metadata,'$.last'),'calls',json_extract(metadata,'$.details.calls'),'usd',json_extract(metadata,'$.family_usd'),'activity',json_extract(metadata,'$.details.activity'),'live_observation',EXISTS(SELECT 1 FROM session_catalog WHERE session_key=?1 AND lifecycle='current')) ELSE NULL END FROM session_trace_headers WHERE session_key=?1"),[key],|r|r.get(0)).optional().map(Option::flatten).map_err(io::Error::other)
    }
    fn preview_header(&self, key: &str) -> io::Result<Option<(String, Option<String>)>> {
        self.db.query_row(&self.sql("SELECT source_generation,json_extract(metadata,'$.content_generation') FROM session_trace_headers WHERE session_key=?1"),[key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(io::Error::other)
    }
    fn preview_catalog(&self, key: &str) -> io::Result<Option<String>> {
        let history = self
            .db
            .query_row(
                &self.sql("SELECT metadata FROM session_history_catalog WHERE session_key=?1"),
                [key],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        Ok(history.or(self
            .db
            .query_row(
                &self.sql("SELECT metadata FROM session_catalog WHERE session_key=?1"),
                [key],
                |r| r.get(0),
            )
            .optional()
            .map_err(io::Error::other)?))
    }
}
