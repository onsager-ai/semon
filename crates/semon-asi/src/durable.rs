//! Author-owned durable facts. Analytics only opens this file read-only.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::Path;

pub use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, params};

use crate::projection::{Cell, Row};
use crate::{Error, Result, Store, TableBatch, WriterLock};

pub const FILE: &str = "asi-durable.sqlite3";
const VERSION: i64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Categorical,
    Internal,
    Body,
}

pub struct Column {
    pub name: &'static str,
    pub class: Class,
    sql: &'static str,
}
pub struct Table {
    pub name: &'static str,
    pub columns: &'static [Column],
    unique: &'static str,
}
macro_rules! c {
    ($name:literal, $class:ident, $sql:literal) => {
        Column {
            name: $name,
            class: Class::$class,
            sql: $sql,
        }
    };
}
/// The sole durable column/class contract. Revision and IDs are internal metadata.
pub const TABLES: &[Table] = &[
    Table {
        name: "label",
        unique: "",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("target_type", Categorical, "TEXT NOT NULL"),
            c!("target_uid", Internal, "TEXT NOT NULL"),
            c!("field", Categorical, "TEXT NOT NULL"),
            c!("value", Categorical, "TEXT NOT NULL"),
            c!("mechanism_text", Body, "TEXT"),
            c!("evidence_step_ids", Internal, "TEXT NOT NULL"),
            c!("labeler", Categorical, "TEXT NOT NULL"),
            c!("taxonomy_version", Categorical, "INTEGER NOT NULL"),
            c!(
                "confidence",
                Categorical,
                "REAL CHECK(confidence BETWEEN 0 AND 1)"
            ),
            c!("created_at", Internal, "INTEGER NOT NULL"),
        ],
    },
    Table {
        name: "adjudication",
        unique: "",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("label_ids", Internal, "TEXT NOT NULL"),
            c!("final_value", Categorical, "TEXT NOT NULL"),
            c!("adjudicator", Categorical, "TEXT NOT NULL"),
            c!("reason", Body, "TEXT"),
            c!("target_type", Categorical, "TEXT NOT NULL"),
            c!("target_uid", Internal, "TEXT NOT NULL"),
            c!("field", Categorical, "TEXT NOT NULL"),
            c!("taxonomy_version", Categorical, "INTEGER NOT NULL"),
            c!("created_at", Internal, "INTEGER NOT NULL"),
        ],
    },
    Table {
        name: "lane",
        unique: "",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("title", Body, "TEXT"),
            c!("root_session_id", Internal, "TEXT"),
            c!("work_item_id", Internal, "TEXT"),
        ],
    },
    Table {
        name: "work_item",
        unique: "",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("tracker", Categorical, "TEXT"),
            c!("ref", Internal, "TEXT"),
            c!("value_class", Categorical, "TEXT"),
            c!("size", Categorical, "TEXT"),
            c!("priority", Categorical, "TEXT"),
            c!("opened_at", Internal, "INTEGER"),
            c!("closed_at", Internal, "INTEGER"),
        ],
    },
    Table {
        name: "change",
        unique: "",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("repo", Internal, "TEXT"),
            c!("kind", Categorical, "TEXT"),
            c!("ref", Internal, "TEXT"),
            c!("opened_at", Internal, "INTEGER"),
            c!("merged_at", Internal, "INTEGER"),
            c!("reverted_at", Internal, "INTEGER"),
        ],
    },
    Table {
        name: "link",
        unique: ", UNIQUE(from_type, from_id, change_id, method)",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("from_type", Categorical, "TEXT NOT NULL"),
            c!("from_id", Internal, "TEXT NOT NULL"),
            c!("change_id", Internal, "TEXT NOT NULL"),
            c!("method", Categorical, "TEXT NOT NULL"),
            c!("evidence", Categorical, "TEXT NOT NULL"),
            c!(
                "confidence",
                Categorical,
                "REAL CHECK(confidence BETWEEN 0 AND 1)"
            ),
        ],
    },
    Table {
        name: "outcome",
        unique: ", UNIQUE(change_id)",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("change_id", Internal, "TEXT NOT NULL"),
            c!("ci_on_merge", Categorical, "TEXT"),
            c!("later_fix_change_id", Internal, "TEXT"),
            c!("escaped_defect", Categorical, "INTEGER"),
            c!("survival_30d", Categorical, "INTEGER"),
            c!("window_closed_at", Internal, "INTEGER"),
        ],
    },
    Table {
        name: "intervention",
        unique: "",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("lever", Categorical, "TEXT"),
            c!("hypothesis", Body, "TEXT"),
            c!("primary_metric", Categorical, "TEXT"),
            c!("guardrails", Body, "TEXT"),
            c!("preregistered_at", Internal, "INTEGER"),
        ],
    },
    Table {
        name: "assignment",
        unique: ", UNIQUE(intervention_id, unit_type, unit_id)",
        columns: &[
            c!("id", Internal, "TEXT PRIMARY KEY NOT NULL"),
            c!("revision", Internal, "INTEGER NOT NULL"),
            c!("intervention_id", Internal, "TEXT NOT NULL"),
            c!("unit_type", Categorical, "TEXT NOT NULL"),
            c!("unit_id", Internal, "TEXT NOT NULL"),
            c!("arm", Categorical, "TEXT NOT NULL"),
            c!("assigned_at", Internal, "INTEGER NOT NULL"),
        ],
    },
];

fn sql(error: rusqlite::Error) -> Error {
    Error::invalid(format!("durable SQLite: {error}"))
}
fn table(name: &str) -> Result<&'static Table> {
    TABLES
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| Error::invalid("unknown durable table"))
}
fn validate(conn: &Connection) -> Result<()> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(sql)?;
    if version != VERSION {
        return Err(Error::invalid(format!(
            "durable format {version} refused; expected {VERSION}; preserve or export the file"
        )));
    }
    Ok(())
}

pub struct Writer {
    conn: Connection,
    _lock: WriterLock,
}
impl Writer {
    /// Shares the analytics author lock, so only one author-owned writer runs.
    pub fn open(root: &Path) -> Result<Self> {
        let lock = WriterLock::acquire(root)?;
        let path = root.join(FILE);
        let exists = path.try_exists()?;
        // Check the version before any writable open or pragma.
        if exists {
            Reader::open(root)?;
        }
        let conn = Connection::open(&path).map_err(sql)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(sql)?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(sql)?;
        if !exists {
            let tx = conn.unchecked_transaction().map_err(sql)?;
            tx.execute_batch("CREATE TABLE revision (singleton INTEGER PRIMARY KEY CHECK(singleton=1), value INTEGER NOT NULL); INSERT INTO revision VALUES(1,0);").map_err(sql)?;
            for t in TABLES {
                let cols = t
                    .columns
                    .iter()
                    .map(|c| format!("\"{}\" {}", c.name, c.sql))
                    .collect::<Vec<_>>()
                    .join(",");
                tx.execute_batch(&format!(
                    "CREATE TABLE \"{}\" ({}{});",
                    t.name, cols, t.unique
                ))
                .map_err(sql)?;
            }
            for name in ["label", "adjudication"] {
                for operation in ["UPDATE", "DELETE"] {
                    tx.execute_batch(&format!("CREATE TRIGGER {name}_{operation} BEFORE {operation} ON {name} BEGIN SELECT RAISE(ABORT, 'insert-only durable assertion'); END;")).map_err(sql)?;
                }
            }
            tx.pragma_update(None, "user_version", VERSION)
                .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        Ok(Self { conn, _lock: lock })
    }

    /// Insert one author-assigned ID. Arrays are JSON text; timestamps are UTC ms.
    /// A failed validation/constraint rolls back both the row and the revision.
    pub fn insert(&mut self, name: &str, id: &str, values: BTreeMap<String, Value>) -> Result<u64> {
        let t = table(name)?;
        if id.is_empty() || id.contains('/') {
            return Err(Error::invalid(
                "durable id must be one nonempty UID component",
            ));
        }
        if values
            .keys()
            .any(|key| key == "id" || key == "revision" || !t.columns.iter().any(|c| c.name == key))
        {
            return Err(Error::invalid("unknown durable column"));
        }
        for column in t.columns {
            if let Some(value) = values.get(column.name) {
                let valid = match value {
                    Value::Null => !column.sql.contains("NOT NULL"),
                    Value::Text(_) => column.sql.starts_with("TEXT"),
                    Value::Integer(_) => {
                        column.sql.starts_with("INTEGER") || column.sql.starts_with("REAL")
                    }
                    Value::Real(number) => column.sql.starts_with("REAL") && number.is_finite(),
                    Value::Blob(_) => false,
                };
                if !valid {
                    return Err(Error::invalid(format!(
                        "invalid durable column type: {}",
                        column.name
                    )));
                }
            }
        }
        let tx = self.conn.transaction().map_err(sql)?;
        if name == "label" || name == "adjudication" {
            let target = text(&values, "target_type")?;
            if !crate::schema::TARGET_TYPE.contains(&target) {
                return Err(Error::invalid("invalid label target type"));
            }
        }
        if name == "label" {
            let _: Vec<String> = serde_json::from_str(text(&values, "evidence_step_ids")?)?;
        }
        if name == "adjudication" {
            let ids: Vec<String> = serde_json::from_str(text(&values, "label_ids")?)?;
            if ids.is_empty() {
                return Err(Error::invalid("adjudication requires labels"));
            }
            for label in ids {
                let row = tx.query_row("SELECT target_type,target_uid,field,taxonomy_version FROM label WHERE id=?1", [label], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?, r.get::<_,i64>(3)?))).map_err(sql)?;
                if row.0 != text(&values, "target_type")?
                    || row.1 != text(&values, "target_uid")?
                    || row.2 != text(&values, "field")?
                    || values.get("taxonomy_version") != Some(&Value::Integer(row.3))
                {
                    return Err(Error::invalid(
                        "adjudication labels disagree with its target, field or taxonomy",
                    ));
                }
            }
        }
        let revision: i64 = tx
            .query_row(
                "UPDATE revision SET value=value+1 WHERE singleton=1 RETURNING value",
                [],
                |r| r.get(0),
            )
            .map_err(sql)?;
        let names = values
            .keys()
            .map(|k| format!(",\"{k}\""))
            .collect::<String>();
        let placeholders = vec!["?"; values.len() + 2].join(",");
        let mut args = vec![Value::Text(id.into()), Value::Integer(revision)];
        args.extend(values.into_values());
        tx.execute(
            &format!(
                "INSERT INTO \"{}\" (id,revision{names}) VALUES ({placeholders})",
                t.name
            ),
            rusqlite::params_from_iter(args),
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(revision as u64)
    }
}
fn text<'a>(values: &'a BTreeMap<String, Value>, key: &str) -> Result<&'a str> {
    match values.get(key) {
        Some(Value::Text(s)) => Ok(s),
        _ => Err(Error::invalid(format!("{key} must be text"))),
    }
}

pub struct Reader {
    conn: Connection,
}
impl Reader {
    pub fn open(root: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            root.join(FILE),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql)?;
        conn.busy_timeout(std::time::Duration::ZERO).map_err(sql)?;
        validate(&conn)?;
        Ok(Self { conn })
    }
    pub fn body(&self, kind: &str, id: &str) -> Result<Option<BTreeMap<String, String>>> {
        let t = table(kind)?;
        let columns: Vec<_> = t
            .columns
            .iter()
            .filter(|c| c.class == Class::Body)
            .collect();
        if columns.is_empty() {
            return Err(Error::invalid("durable target has no view body"));
        }
        let names = columns.iter().map(|c| c.name).collect::<Vec<_>>().join(",");
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {names} FROM \"{}\" WHERE id=?1", t.name))
            .map_err(sql)?;
        let mut rows = stmt.query([id]).map_err(sql)?;
        let Some(row) = rows.next().map_err(sql)? else {
            return Ok(None);
        };
        let mut body = BTreeMap::new();
        for (i, c) in columns.iter().enumerate() {
            if let Some(value) = row.get::<_, Option<String>>(i).map_err(sql)? {
                body.insert(c.name.into(), value);
            }
        }
        Ok(Some(body))
    }
    pub fn backup(&self, destination: &Path) -> Result<()> {
        let reserved = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let result = (|| {
            let mut target = Connection::open(destination).map_err(sql)?;
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut target).map_err(sql)?;
            loop {
                match backup.step(128).map_err(sql)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => {}
                    _ => return Err(Error::invalid("durable backup busy; retry the backup")),
                }
            }
            drop(backup);
            drop(target);
            reserved.sync_all()?;
            if let Some(parent) = destination.parent().filter(|p| !p.as_os_str().is_empty()) {
                File::open(parent)?.sync_all()?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(destination);
        }
        result
    }
    fn export(&mut self, checkpoint: u64) -> Result<(u64, Vec<Row>)> {
        let tx = self.conn.transaction().map_err(sql)?;
        let revision: i64 = tx
            .query_row("SELECT value FROM revision WHERE singleton=1", [], |r| {
                r.get(0)
            })
            .map_err(sql)?;
        if revision < 0 || (revision as u64) < checkpoint {
            return Err(Error::invalid(
                "durable revision is behind analytics checkpoint",
            ));
        }
        let mut stmt = tx.prepare("SELECT 'label/'||id,target_type,target_uid,field,value,labeler,taxonomy_version,confidence,created_at,revision FROM label WHERE revision>?1 UNION ALL SELECT 'adjudication/'||id,target_type,target_uid,field,final_value,'human:'||adjudicator,taxonomy_version,NULL,created_at,revision FROM adjudication WHERE revision>?1 ORDER BY revision").map_err(sql)?;
        let mut query = stmt
            .query(params![
                i64::try_from(checkpoint)
                    .map_err(|_| Error::invalid("durable checkpoint overflow"))?
            ])
            .map_err(sql)?;
        let mut out = Vec::new();
        while let Some(r) = query.next().map_err(sql)? {
            let mut row = Row::new();
            for (i, key) in [
                "label_id",
                "target_type",
                "target_uid",
                "field",
                "value",
                "labeller",
            ]
            .iter()
            .enumerate()
            {
                row.insert(*key, Cell::Text(Some(r.get(i).map_err(sql)?)));
            }
            row.insert(
                "taxonomy_version",
                Cell::Integer(Some(r.get(6).map_err(sql)?)),
            );
            row.insert("confidence", Cell::Float(r.get(7).map_err(sql)?));
            row.insert("created_at", Cell::Timestamp(Some(r.get(8).map_err(sql)?)));
            row.insert("revision", Cell::Integer(Some(r.get(9).map_err(sql)?)));
            out.push(row);
        }
        Ok((revision as u64, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label() -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("target_type".into(), Value::Text("lane".into())),
            ("target_uid".into(), Value::Text("lane/l1".into())),
            ("field".into(), Value::Text("activity".into())),
            ("value".into(), Value::Text("coding".into())),
            ("mechanism_text".into(), Value::Text("PRIVATE BODY".into())),
            ("evidence_step_ids".into(), Value::Text("[]".into())),
            ("labeler".into(), Value::Text("model:test".into())),
            ("taxonomy_version".into(), Value::Integer(1)),
            ("created_at".into(), Value::Integer(1)),
        ])
    }
    fn adjudication() -> BTreeMap<String, Value> {
        let mut values = label();
        for name in ["value", "mechanism_text", "evidence_step_ids", "labeler"] {
            values.remove(name);
        }
        values.extend([
            ("label_ids".into(), Value::Text("[\"a\"]".into())),
            ("final_value".into(), Value::Text("review".into())),
            ("adjudicator".into(), Value::Text("reviewer".into())),
            ("reason".into(), Value::Text("PRIVATE REASON".into())),
        ]);
        values
    }
    #[test]
    fn assertions_are_insert_only_and_failed_adjudication_rolls_back_revision() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = Writer::open(dir.path()).unwrap();
        assert_eq!(writer.insert("label", "a", label()).unwrap(), 1);
        let mut bad = adjudication();
        bad.insert("field".into(), Value::Text("other".into()));
        assert!(writer.insert("adjudication", "bad", bad).is_err());
        let mut missing = adjudication();
        missing.insert(
            "label_ids".into(),
            Value::Text("[\"a\",\"missing\"]".into()),
        );
        assert!(writer.insert("adjudication", "missing", missing).is_err());
        assert_eq!(
            writer.insert("adjudication", "a", adjudication()).unwrap(),
            2
        );
        for name in ["label", "adjudication"] {
            assert!(
                writer
                    .conn
                    .execute(&format!("UPDATE {name} SET id='b'"), [])
                    .is_err()
            );
            assert!(
                writer
                    .conn
                    .execute(&format!("DELETE FROM {name}"), [])
                    .is_err()
            );
        }
        assert!(matches!(Writer::open(dir.path()), Err(Error::WriterBusy)));
    }
    #[test]
    fn fresh_export_after_commit_is_atomic_idempotent_and_body_free() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut writer = Writer::open(dir.path()).unwrap();
            writer.insert("label", "a", label()).unwrap();
            writer.insert("adjudication", "a", adjudication()).unwrap();
        } // Simulated crash boundary: no analytics export yet.
        let before = std::fs::read(dir.path().join(FILE)).unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let mut tables = Vec::new();
        let revision = append_export(&store, &mut tables).unwrap();
        assert_eq!(revision, 2);
        assert_eq!(tables[0].batch.num_rows(), 2);
        store
            .publish(&crate::Update {
                tables,
                durable_revision: revision,
                ..Default::default()
            })
            .unwrap();
        let mut tables = Vec::new();
        assert_eq!(append_export(&store, &mut tables).unwrap(), 2);
        assert!(tables.is_empty());
        for file in store.manifest().files_of("fact_label") {
            for batch in
                crate::parquet_io::read_batches(&store.analytics_dir().join(&file.path), None)
                    .unwrap()
            {
                for t in TABLES {
                    for c in t.columns {
                        if c.class == Class::Body {
                            assert!(batch.schema().field_with_name(c.name).is_err());
                        }
                    }
                }
                let values = batch
                    .column_by_name("labeller")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<arrow::array::StringArray>()
                    .unwrap();
                assert_eq!(values.value(1), "human:reviewer");
                assert!(!format!("{batch:?}").contains("PRIVATE"));
            }
        }
        assert_eq!(before, std::fs::read(dir.path().join(FILE)).unwrap());
        drop(store);
        // Rebuilding analytics starts its checkpoint at zero, retaining durable bytes.
        std::fs::remove_dir_all(dir.path().join("analytics")).unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut tables = Vec::new();
        assert_eq!(append_export(&store, &mut tables).unwrap(), 2);
        assert_eq!(tables[0].batch.num_rows(), 2);
        assert_eq!(before, std::fs::read(dir.path().join(FILE)).unwrap());
    }
    #[test]
    fn all_analytics_schemas_exclude_durable_body_columns() {
        for table in crate::schema::TABLES {
            for durable in TABLES {
                for column in durable.columns {
                    if column.class == Class::Body {
                        assert!(table.file_schema().field_with_name(column.name).is_err());
                    }
                }
            }
        }
    }

    #[cfg(feature = "duckdb-tests")]
    #[test]
    fn current_label_prefers_exported_adjudication_over_later_model_label() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut writer = Writer::open(dir.path()).unwrap();
            writer.insert("label", "a", label()).unwrap();
            writer.insert("adjudication", "a", adjudication()).unwrap();
            writer.insert("label", "later", label()).unwrap();
        }
        let mut reader = Reader::open(dir.path()).unwrap();
        let (_, rows) = reader.export(0).unwrap();
        let batch = crate::publish::rows_to_batch(&crate::schema::FACT_LABEL, &rows).unwrap();
        let file = dir.path().join("labels.parquet");
        crate::parquet_io::write_file(&file, &batch).unwrap();
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "CREATE VIEW fact_label AS SELECT * FROM read_parquet('{}')",
            file.to_str().unwrap().replace('\'', "''")
        ))
        .unwrap();
        let view = crate::reference::SEMANTIC_VIEWS
            .iter()
            .find(|v| v.name == "current_label")
            .unwrap();
        conn.execute_batch(&format!("CREATE VIEW current_label AS {}", view.sql))
            .unwrap();
        let id: String = conn
            .query_row("SELECT label_id FROM current_label", [], |r| r.get(0))
            .unwrap();
        assert_eq!(id, "adjudication/a");
    }

    #[test]
    fn mismatch_refuses_before_writable_open_and_backup_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut writer = Writer::open(dir.path()).unwrap();
            writer.insert("label", "a", label()).unwrap();
        }
        let destination = dir.path().join("backup.sqlite3");
        let reader = Reader::open(dir.path()).unwrap();
        reader.backup(&destination).unwrap();
        let copy =
            Connection::open_with_flags(&destination, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            copy.query_row("SELECT count(*) FROM label", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let bytes = std::fs::read(&destination).unwrap();
        assert!(reader.backup(&destination).is_err());
        assert_eq!(bytes, std::fs::read(&destination).unwrap());
        drop(reader);
        let conn = Connection::open(dir.path().join(FILE)).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        let before = std::fs::read(dir.path().join(FILE)).unwrap();
        assert!(Writer::open(dir.path()).is_err());
        assert!(Reader::open(dir.path()).is_err());
        assert_eq!(before, std::fs::read(dir.path().join(FILE)).unwrap());
    }
}

/// Read-only outbox drain; the manifest commits rows and checkpoint together.
pub(crate) fn append_export<C: crate::Clock>(
    store: &Store<C>,
    tables: &mut Vec<TableBatch>,
) -> Result<u64> {
    let root = store
        .analytics_dir()
        .parent()
        .ok_or_else(|| Error::invalid("analytics root missing"))?;
    if !root.join(FILE).try_exists()? {
        if store.manifest().durable_revision != 0 {
            return Err(Error::invalid("checkpointed durable file missing"));
        }
        return Ok(0);
    }
    let (revision, rows) = Reader::open(root)?.export(store.manifest().durable_revision)?;
    if !rows.is_empty() {
        tables.push(TableBatch {
            table: &crate::schema::FACT_LABEL,
            batch: crate::publish::rows_to_batch(&crate::schema::FACT_LABEL, &rows)?,
        });
    }
    Ok(revision)
}
