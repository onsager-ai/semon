//! Complete physical trace family; namespace selection never depends on Raw paths.
use crate::storage::{StorageError, catalog::TracePublicationStore};
use rusqlite::{Connection, params};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_trace_headers(namespace TEXT NOT NULL,session_key TEXT NOT NULL,generation TEXT NOT NULL,source_generation TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,session_key)) STRICT, WITHOUT ROWID;
CREATE TABLE session_trace_turns(namespace TEXT NOT NULL,session_key TEXT NOT NULL,turn_id TEXT NOT NULL,start_id TEXT,metadata TEXT NOT NULL,PRIMARY KEY(namespace,session_key,turn_id)) STRICT, WITHOUT ROWID;
CREATE INDEX session_trace_starts ON session_trace_turns(namespace,session_key,start_id,turn_id);
CREATE INDEX session_trace_turn_query_at ON session_trace_turns(namespace,session_key,json_extract(metadata,'$.at'),turn_id);
CREATE INDEX session_trace_turn_query_last ON session_trace_turns(namespace,session_key,turn_id) WHERE json_extract(metadata,'$.last')=1;
CREATE TABLE session_trace_edges(namespace TEXT NOT NULL,session_key TEXT NOT NULL,edge_id TEXT NOT NULL,metadata TEXT NOT NULL,PRIMARY KEY(namespace,session_key,edge_id)) STRICT, WITHOUT ROWID;
CREATE INDEX session_trace_edge_identity ON session_trace_edges(namespace,edge_id,session_key);
CREATE INDEX session_trace_from ON session_trace_edges(namespace,CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END,session_key);
CREATE INDEX session_trace_to ON session_trace_edges(namespace,CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END,session_key);
CREATE INDEX session_trace_query_at ON session_trace_edges(namespace,json_extract(metadata,'$.at'),session_key,edge_id);
CREATE INDEX session_trace_query_done ON session_trace_edges(namespace,json_extract(metadata,'$.done'),session_key,edge_id);
CREATE INDEX session_trace_query_status ON session_trace_edges(namespace,json_extract(metadata,'$.status'),session_key,edge_id);
CREATE TABLE session_trace_bookmarks(namespace TEXT NOT NULL,target TEXT NOT NULL,session_key TEXT NOT NULL,edge_id TEXT NOT NULL,PRIMARY KEY(namespace,target,session_key,edge_id)) STRICT, WITHOUT ROWID;
CREATE INDEX session_trace_bookmark_owner ON session_trace_bookmarks(namespace,session_key);
";

pub(crate) const TABLES: &[&str] = &[
    "session_trace_headers",
    "session_trace_turns",
    "session_trace_edges",
    "session_trace_bookmarks",
];

pub(crate) fn validate(db: &Connection) -> Result<(), StorageError> {
    // Validate exact ordered column/primary-key profiles, not just a namespace
    // column. Existing unsupported data is refused before any enrollment write.
    for (table, expected) in [
        (
            TABLES[0],
            "namespace:TEXT:1:1,session_key:TEXT:1:2,generation:TEXT:1:0,source_generation:TEXT:1:0,metadata:TEXT:1:0",
        ),
        (
            TABLES[1],
            "namespace:TEXT:1:1,session_key:TEXT:1:2,turn_id:TEXT:1:3,start_id:TEXT:0:0,metadata:TEXT:1:0",
        ),
        (
            TABLES[2],
            "namespace:TEXT:1:1,session_key:TEXT:1:2,edge_id:TEXT:1:3,metadata:TEXT:1:0",
        ),
        (
            TABLES[3],
            "namespace:TEXT:1:1,target:TEXT:1:2,session_key:TEXT:1:3,edge_id:TEXT:1:4",
        ),
    ] {
        let shape: Option<String> = db.query_row("SELECT group_concat(name||':'||type||':'||[notnull]||':'||pk,',') FROM (SELECT * FROM pragma_table_info(?1,'main') ORDER BY cid)", [table], |r| r.get(0)).map_err(super::failed)?;
        let strict: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema='main' AND name=?1 AND type='table' AND strict=1 AND wr=1)", [table], |r| r.get(0)).map_err(super::failed)?;
        if shape.as_deref() != Some(expected) || !strict {
            return Err(StorageError::NamespaceUpgradePending);
        }
    }
    for (index, expected, partial) in [
        (
            "session_trace_starts",
            "namespace,session_key,start_id,turn_id",
            0,
        ),
        (
            "session_trace_turn_query_at",
            "namespace,session_key,<expression>,turn_id",
            0,
        ),
        (
            "session_trace_turn_query_last",
            "namespace,session_key,turn_id",
            1,
        ),
        (
            "session_trace_edge_identity",
            "namespace,edge_id,session_key",
            0,
        ),
        (
            "session_trace_from",
            "namespace,<expression>,session_key",
            0,
        ),
        ("session_trace_to", "namespace,<expression>,session_key", 0),
        (
            "session_trace_query_at",
            "namespace,<expression>,session_key,edge_id",
            0,
        ),
        (
            "session_trace_query_done",
            "namespace,<expression>,session_key,edge_id",
            0,
        ),
        (
            "session_trace_query_status",
            "namespace,<expression>,session_key,edge_id",
            0,
        ),
        ("session_trace_bookmark_owner", "namespace,session_key", 0),
    ] {
        let shape: Option<String> = db.query_row("SELECT group_concat(coalesce(name,'<expression>'),',') FROM (SELECT * FROM pragma_index_xinfo(?1,'main') WHERE key=1 ORDER BY seqno)", [index], |r|r.get(0)).map_err(super::failed)?;
        let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_index_list((SELECT tbl_name FROM main.sqlite_schema WHERE type='index' AND name=?1),'main') WHERE name=?1 AND origin='c' AND [unique]=0 AND partial=?2)", params![index, partial], |r|r.get(0)).map_err(super::failed)?;
        if shape.as_deref() != Some(expected) || !valid {
            return Err(StorageError::NamespaceUpgradePending);
        }
    }
    Ok(())
}

pub(crate) struct TraceWriter<'a> {
    db: &'a Connection,
    namespace: Option<&'a str>,
}
impl<'a> TraceWriter<'a> {
    pub(crate) fn unbound(db: &'a Connection) -> Self {
        Self {
            db,
            namespace: None,
        }
    }
    pub(super) fn admitted(
        db: &'a Connection,
        source: &'a super::semantic::AdmittedSource,
    ) -> Result<Self, StorageError> {
        source.validate_publication(db)?;
        validate(db)?;
        Ok(Self {
            db,
            namespace: Some(source.namespace()),
        })
    }
}
impl TracePublicationStore for TraceWriter<'_> {
    type Error = rusqlite::Error;
    fn invalid(error: std::io::Error) -> Self::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
    }
    fn matches_header(&self, key: &str, generation: &str, source: &str) -> rusqlite::Result<bool> {
        let sql = if self.namespace.is_some() {
            "SELECT EXISTS(SELECT 1 FROM main.session_trace_headers WHERE namespace=?1 AND session_key=?2 AND generation=?3 AND source_generation=?4)"
        } else {
            "SELECT EXISTS(SELECT 1 FROM main.session_trace_headers WHERE session_key=?2 AND generation=?3 AND source_generation=?4)"
        };
        self.db
            .query_row(sql, params![self.namespace, key, generation, source], |r| {
                r.get(0)
            })
    }
    fn header(
        &self,
        key: &str,
        generation: &str,
        source: &str,
        metadata: &str,
    ) -> rusqlite::Result<bool> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_trace_headers VALUES(?1,?2,?3,?4,?5) ON CONFLICT(namespace,session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,metadata=excluded.metadata WHERE session_trace_headers.generation!=excluded.generation OR session_trace_headers.source_generation!=excluded.source_generation"
        } else {
            "INSERT INTO main.session_trace_headers VALUES(?2,?3,?4,?5) ON CONFLICT(session_key) DO UPDATE SET generation=excluded.generation,source_generation=excluded.source_generation,metadata=excluded.metadata WHERE session_trace_headers.generation!=excluded.generation OR session_trace_headers.source_generation!=excluded.source_generation"
        };
        self.db
            .prepare_cached(sql)?
            .execute(params![self.namespace, key, generation, source, metadata])
            .map(|n| n > 0)
    }
    fn clear(&self, key: &str) -> rusqlite::Result<()> {
        for table in [
            "session_trace_turns",
            "session_trace_edges",
            "session_trace_bookmarks",
        ] {
            let predicate = if self.namespace.is_some() {
                "namespace=?1 AND "
            } else {
                ""
            };
            self.db
                .prepare_cached(&format!(
                    "DELETE FROM main.{table} WHERE {predicate}session_key=?2"
                ))?
                .execute(params![self.namespace, key])?;
        }
        Ok(())
    }
    fn turn(
        &self,
        key: &str,
        id: Option<&str>,
        start: Option<&str>,
        metadata: &str,
    ) -> rusqlite::Result<()> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_trace_turns VALUES(?1,?2,?3,?4,?5)"
        } else {
            "INSERT INTO main.session_trace_turns VALUES(?2,?3,?4,?5)"
        };
        self.db
            .prepare_cached(sql)?
            .execute(params![self.namespace, key, id, start, metadata])?;
        Ok(())
    }
    fn edge(&self, key: &str, id: Option<&str>, metadata: &str) -> rusqlite::Result<()> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_trace_edges VALUES(?1,?2,?3,?4)"
        } else {
            "INSERT INTO main.session_trace_edges VALUES(?2,?3,?4)"
        };
        self.db
            .prepare_cached(sql)?
            .execute(params![self.namespace, key, id, metadata])?;
        Ok(())
    }
    fn bookmark(&self, key: &str, target: &str, edge: &str) -> rusqlite::Result<()> {
        let sql = if self.namespace.is_some() {
            "INSERT INTO main.session_trace_bookmarks VALUES(?1,?2,?3,?4)"
        } else {
            "INSERT INTO main.session_trace_bookmarks VALUES(?2,?3,?4)"
        };
        self.db
            .prepare_cached(sql)?
            .execute(params![self.namespace, target, key, edge])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_trace_family_keeps_colliding_foreign_facts_and_rolls_back() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        validate(&db).unwrap();
        let tx = db.transaction().unwrap();
        for namespace in ["a", "b"] {
            let writer = TraceWriter {
                db: &tx,
                namespace: Some(namespace),
            };
            assert!(writer.header("same", namespace, "source", "{}").unwrap());
            writer
                .turn("same", Some("turn"), Some("edge"), "{\"last\":1}")
                .unwrap();
            writer
                .edge("same", Some("edge"), "{\"from\":\"same\",\"to\":\"peer\"}")
                .unwrap();
            writer.bookmark("same", "target", "edge").unwrap();
        }
        tx.commit().unwrap();
        let tx = db.transaction().unwrap();
        let writer = TraceWriter {
            db: &tx,
            namespace: Some("a"),
        };
        writer.header("same", "next", "source", "{}").unwrap();
        writer.clear("same").unwrap();
        for table in &TABLES[1..] {
            assert_eq!(
                tx.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE namespace='b'"),
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                tx.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE namespace='a'"),
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
        tx.rollback().unwrap();
        for table in TABLES {
            assert_eq!(
                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
        assert_eq!(
            db.query_row(
                "SELECT generation FROM session_trace_headers WHERE namespace='a'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "a"
        );
    }
}
