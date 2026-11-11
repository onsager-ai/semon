use std::path::{Component, Path};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use duckdb::core::{LogicalTypeHandle, LogicalTypeId};
use duckdb::types::Value as SqlValue;
use duckdb::{Config, Connection, params_from_iter};
use serde_json::{Value, json};

use super::{Column, DATA_NOTICE, Failure, QueryResult, Request, ast};
use crate::reference::SEMANTIC_VIEWS;
use crate::schema::{ASI_SCHEMA_VERSION, Kind, TABLES, Type};
use crate::store::{Manifest, read_manifest};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn snapshot(root: &Path) -> Result<Manifest, Failure> {
    // read_manifest performs exactly one file read and validates the schema.
    read_manifest(root)
        .map_err(|error| match error {
            crate::Error::Invalid(message) if message.contains("schema") => Failure::new(
                "schema_mismatch",
                "Analytics schema differs from engine",
                "asi_schema_version",
            ),
            _ => Failure::unavailable(),
        })?
        .ok_or_else(Failure::unavailable)
}

fn paths(root: &Path, manifest: &Manifest) -> Result<Vec<(String, String)>, Failure> {
    let analytics = root
        .join("analytics")
        .canonicalize()
        .map_err(|_| Failure::unavailable())?;
    manifest
        .files
        .iter()
        .filter(|file| TABLES.iter().any(|table| table.name == file.table))
        .map(|file| file.path.as_str())
        .chain(manifest.live_map.as_deref())
        .map(|relative| {
            let path = Path::new(relative);
            if path.extension().and_then(|s| s.to_str()) != Some("parquet")
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                return Err(Failure::unavailable());
            }
            let canonical = analytics
                .join(path)
                .canonicalize()
                .map_err(|_| Failure::unavailable())?;
            if !canonical.starts_with(&analytics) {
                return Err(Failure::unavailable());
            }
            let text = canonical.to_str().ok_or_else(Failure::unavailable)?;
            Ok((relative.to_owned(), quote(text)))
        })
        .collect()
}

pub(super) fn setup(
    root: &Path,
    manifest: &Manifest,
    scratch: &Path,
    memory: &str,
) -> Result<Connection, Failure> {
    let files = paths(root, manifest)?;
    let config = Config::default()
        .enable_autoload_extension(false)
        .and_then(|c| c.threads(2))
        .and_then(|c| c.max_memory(memory))
        .and_then(|c| c.with("allow_community_extensions", "false"))
        .map_err(classify)?;
    let db = Connection::open_in_memory_with_flags(config).map_err(classify)?;
    db.execute_batch(&format!("SET storage_compatibility_version='latest'; SET threads=2; SET memory_limit={memory}; SET temp_directory={scratch}; SET max_temp_directory_size='1GiB'; SET autoinstall_known_extensions=false; SET autoload_known_extensions=false; SET allow_community_extensions=false; LOAD parquet; LOAD json; SET allowed_paths=[{paths}];",
        memory=quote(memory), scratch=quote(scratch.to_str().ok_or_else(Failure::unavailable)?),
        paths=files.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>().join(",")))
        .map_err(classify)?;
    let resolve = |name: &str| -> Result<&str, Failure> {
        files
            .iter()
            .find(|(relative, _)| relative == name)
            .map(|(_, path)| path.as_str())
            .ok_or_else(Failure::unavailable)
    };
    let live = match &manifest.live_map {
        Some(path) => format!("SELECT * FROM read_parquet({})", resolve(path)?),
        None => "SELECT NULL::VARCHAR AS session_uid, NULL::BIGINT AS _gen WHERE false".into(),
    };
    db.execute_batch(&format!("CREATE VIEW _asi_live AS {live}"))
        .map_err(classify)?;
    for table in TABLES {
        let list = manifest
            .files_of(table.name)
            .map(|file| resolve(&file.path))
            .collect::<Result<Vec<_>, _>>()?;
        let source = if list.is_empty() {
            let columns = table
                .columns
                .iter()
                .map(|col| {
                    let ty = match col.data_type {
                        Type::Text => "VARCHAR",
                        Type::Integer => "BIGINT",
                        Type::Float => "DOUBLE",
                        Type::Boolean => "BOOLEAN",
                        Type::Timestamp => "TIMESTAMPTZ",
                    };
                    format!("NULL::{ty} AS \"{}\"", col.name)
                })
                .chain(std::iter::once("NULL::BIGINT AS _gen".into()))
                .collect::<Vec<_>>()
                .join(",");
            format!("SELECT {columns} WHERE false")
        } else {
            format!("SELECT * FROM read_parquet([{}])", list.join(","))
        };
        let query = if matches!(table.kind, Kind::Session) {
            format!(
                "SELECT f.* FROM ({source}) f SEMI JOIN _asi_live l ON f.session_uid=l.session_uid AND f._gen=l._gen"
            )
        } else {
            source
        };
        db.execute_batch(&format!("CREATE VIEW {} AS {query}", table.name))
            .map_err(classify)?;
    }
    for view in SEMANTIC_VIEWS {
        db.execute_batch(&format!("CREATE VIEW {} AS {}", view.name, view.sql))
            .map_err(classify)?;
    }
    db.execute_batch("SET enable_external_access=false; SET lock_configuration=true;")
        .map_err(classify)?;
    Ok(db)
}

/// Cancellation is also active while parsing, binding and fetching. Dropping the
/// guard cancels the sleeper and joins before the connection is destroyed.
struct Watchdog {
    cancel: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watchdog {
    fn start(db: &Connection, deadline: Instant) -> Self {
        let handle = db.interrupt_handle();
        let (cancel, receive) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if receive
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .is_err()
            {
                handle.interrupt();
            }
        });
        Self {
            cancel,
            thread: Some(thread),
        }
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn execute(
    root: &Path,
    request: &Request,
    started: Instant,
) -> Result<QueryResult, Failure> {
    let manifest = snapshot(root)?;
    let scratch = tempfile::tempdir().map_err(|_| Failure::unavailable())?;
    let db = setup(root, &manifest, scratch.path(), "1GiB")?;
    execute_snapshot(&db, &manifest, request, started)
}

pub(super) fn execute_snapshot(
    db: &Connection,
    manifest: &Manifest,
    request: &Request,
    started: Instant,
) -> Result<QueryResult, Failure> {
    let deadline = started + Duration::from_millis(request.timeout_ms);
    if Instant::now() >= deadline {
        return Err(Failure::timeout());
    }
    let _watchdog = Watchdog::start(db, deadline);
    let result = (|| {
        ast::check(db, &request.query)?;
        let params = request
            .params
            .iter()
            .map(|v| match v {
                Value::Null => duckdb::types::Value::Null,
                Value::String(s) => duckdb::types::Value::Text(s.clone()),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        duckdb::types::Value::BigInt(i)
                    } else if let Some(u) = n.as_u64() {
                        duckdb::types::Value::UBigInt(u)
                    } else {
                        duckdb::types::Value::Double(n.as_f64().unwrap())
                    }
                }
                _ => unreachable!("validated request"),
            })
            .collect::<Vec<_>>();
        let mut statement = db.prepare(&request.query).map_err(classify)?;
        // Start streaming, then use the fallible row cursor over that same
        // executed result. Arrow's cursor has no cleanup/reset behavior.
        {
            let _stream = statement
                .stream_arrow(params_from_iter(&params))
                .map_err(classify)?;
        }
        let columns = statement
            .column_names()
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                Ok(Column {
                    name,
                    data_type: type_name(
                        &statement.column_logical_type(i),
                        &statement.column_type(i),
                    )?,
                })
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        let mut result = QueryResult {
            asi_schema_version: ASI_SCHEMA_VERSION,
            snapshot: manifest.generation.to_string(),
            columns,
            rows: Vec::new(),
            row_count: 0,
            truncated: false,
            elapsed_ms: 0,
            data_notice: DATA_NOTICE.into(),
        };
        let overhead = serde_json::to_vec(&result)
            .map_err(|_| Failure::resource())?
            .len()
            + 40;
        if overhead > request.max_bytes {
            return Err(Failure::resource());
        }
        let mut bytes = overhead;
        if (0..result.columns.len()).any(|i| has_blob(&statement.column_logical_type(i))) {
            return Err(Failure::new(
                "unsupported_value",
                "Blob results are unsupported",
                "BLOB",
            ));
        }
        let mut cursor = statement.raw_query();
        while let Some(row) = cursor.next().map_err(classify)? {
            if Instant::now() >= deadline {
                return Err(Failure::timeout());
            }
            if result.rows.len() == request.limit {
                result.truncated = true;
                break;
            }
            let values = (0..result.columns.len())
                .map(|i| {
                    cell(
                        row.get::<_, SqlValue>(i).map_err(classify)?,
                        &statement.column_logical_type(i),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let row_bytes = serde_json::to_vec(&values)
                .map_err(|_| Failure::resource())?
                .len()
                + 1;
            if bytes + row_bytes > request.max_bytes {
                result.truncated = true;
                break;
            }
            bytes += row_bytes;
            result.rows.push(values);
        }
        result.row_count = result.rows.len();
        result.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(result)
    })();
    if Instant::now() >= deadline {
        return Err(Failure::timeout());
    }
    result
}

fn type_name(
    ty: &LogicalTypeHandle,
    arrow: &arrow::datatypes::DataType,
) -> Result<String, Failure> {
    let child = |i| type_name(&ty.child(i), child_arrow(arrow, i)?);
    let name = match ty.id() {
        LogicalTypeId::Decimal => format!("DECIMAL({},{})", ty.decimal_width(), ty.decimal_scale()),
        LogicalTypeId::List => format!("{}[]", child(0)?),
        LogicalTypeId::Struct | LogicalTypeId::Union => {
            let kind = if ty.id() == LogicalTypeId::Struct {
                "STRUCT"
            } else {
                "UNION"
            };
            let fields = (0..ty.num_children())
                .map(|i| {
                    Ok(format!(
                        "\"{}\" {}",
                        ty.child_name(i).replace('"', "\"\""),
                        child(i)?
                    ))
                })
                .collect::<Result<Vec<_>, Failure>>()?;
            format!("{kind}({})", fields.join(", "))
        }
        LogicalTypeId::Map => format!("MAP({}, {})", child(0)?, child(1)?),
        LogicalTypeId::Array => match arrow {
            arrow::datatypes::DataType::FixedSizeList(_, length) => {
                format!("{}[{length}]", child(0)?)
            }
            _ => return Err(Failure::resource()),
        },
        LogicalTypeId::TimestampS => "TIMESTAMP_S".into(),
        LogicalTypeId::TimestampMs => "TIMESTAMP_MS".into(),
        LogicalTypeId::TimestampNs => "TIMESTAMP_NS".into(),
        LogicalTypeId::TimestampTZ => "TIMESTAMP WITH TIME ZONE".into(),
        LogicalTypeId::TimeTZ => "TIME WITH TIME ZONE".into(),
        LogicalTypeId::TimeNs => "TIME_NS".into(),
        LogicalTypeId::SqlNull => "NULL".into(),
        id => format!("{id:?}").to_ascii_uppercase(),
    };
    Ok(ty.get_alias().filter(|s| !s.is_empty()).unwrap_or(name))
}

fn child_arrow(
    ty: &arrow::datatypes::DataType,
    i: usize,
) -> Result<&arrow::datatypes::DataType, Failure> {
    use arrow::datatypes::DataType;
    match ty {
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) => {
            Ok(f.data_type())
        }
        DataType::Struct(fields) => fields
            .get(i)
            .map(|f| f.data_type())
            .ok_or_else(Failure::resource),
        DataType::Union(fields, _) => fields
            .iter()
            .nth(i)
            .map(|(_, f)| f.data_type())
            .ok_or_else(Failure::resource),
        DataType::Map(f, _) => child_arrow(f.data_type(), i),
        _ => Err(Failure::resource()),
    }
}

fn has_blob(ty: &LogicalTypeHandle) -> bool {
    matches!(ty.id(), LogicalTypeId::Blob | LogicalTypeId::Geometry)
        || (0..ty.num_children()).any(|i| has_blob(&ty.child(i)))
}

fn signed(n: i128) -> Value {
    if (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&n) {
        json!(n as i64)
    } else {
        json!(n.to_string())
    }
}

fn unsigned(n: u128) -> Value {
    if n <= 9_007_199_254_740_991 {
        json!(n as u64)
    } else {
        json!(n.to_string())
    }
}

fn cell(value: SqlValue, ty: &LogicalTypeHandle) -> Result<Value, Failure> {
    // duckdb-rs loses child UHUGEINT metadata in containers; reinterpret the
    // decoded signed bits using the logical child type rather than corrupting it.
    if ty.id() == LogicalTypeId::UHugeint
        && let SqlValue::HugeInt(v) = &value
    {
        return Ok(unsigned(*v as u128));
    }
    Ok(match value {
        SqlValue::Null => Value::Null,
        SqlValue::Boolean(v) => json!(v),
        SqlValue::TinyInt(v) => signed(v.into()),
        SqlValue::SmallInt(v) => signed(v.into()),
        SqlValue::Int(v) => signed(v.into()),
        SqlValue::BigInt(v) => signed(v.into()),
        SqlValue::HugeInt(v) => signed(v),
        SqlValue::UTinyInt(v) => unsigned(v.into()),
        SqlValue::USmallInt(v) => unsigned(v.into()),
        SqlValue::UInt(v) => unsigned(v.into()),
        SqlValue::UBigInt(v) => unsigned(v.into()),
        SqlValue::UHugeInt(v) => unsigned(v),
        SqlValue::Float(v) => {
            if v.is_finite() {
                json!(v)
            } else {
                json!(v.to_string())
            }
        }
        SqlValue::Double(v) => {
            if v.is_finite() {
                json!(v)
            } else {
                json!(v.to_string())
            }
        }
        SqlValue::Text(v) | SqlValue::Enum(v) => json!(v),
        SqlValue::Decimal(v) => json!(v.to_string()),
        SqlValue::Blob(_) | SqlValue::Geometry(_) => {
            return Err(Failure::new(
                "unsupported_value",
                "Blob results are unsupported",
                "BLOB",
            ));
        }
        SqlValue::List(items) | SqlValue::Array(items) => {
            let child = ty.child(0);
            Value::Array(
                items
                    .into_iter()
                    .map(|v| cell(v, &child))
                    .collect::<Result<_, _>>()?,
            )
        }
        SqlValue::Struct(fields) => Value::Object(
            fields
                .iter()
                .enumerate()
                .map(|(i, (name, v))| Ok((name.clone(), cell(v.clone(), &ty.child(i))?)))
                .collect::<Result<_, Failure>>()?,
        ),
        SqlValue::Map(entries) => Value::Array(
            entries
                .iter()
                .map(|(key, v)| {
                    Ok(json!([
                        cell(key.clone(), &ty.child(0))?,
                        cell(v.clone(), &ty.child(1))?
                    ]))
                })
                .collect::<Result<_, Failure>>()?,
        ),
        // The wrapper drops the active union tag, so exact typed serialization
        // cannot be recovered. Union construction is absent from the allowlist.
        SqlValue::Union(_) => {
            return Err(Failure::new(
                "unsupported_value",
                "Union result unsupported",
                "UNION",
            ));
        }
        SqlValue::Date32(v) => json!(v.to_string()),
        SqlValue::Timestamp(_, v) | SqlValue::Time64(_, v) => json!(v.to_string()),
        SqlValue::Interval {
            months,
            days,
            nanos,
        } => json!({"months":months,"days":days,"nanos":nanos.to_string()}),
        _ => {
            return Err(Failure::new(
                "unsupported_value",
                "Unsupported DuckDB value",
                "value type",
            ));
        }
    })
}

fn classify(error: duckdb::Error) -> Failure {
    let message = error.to_string().to_ascii_lowercase();
    if [
        "out of memory",
        "memory limit",
        "failed to allocate",
        "temp_directory",
        "maximum temporary directory",
    ]
    .iter()
    .any(|s| message.contains(s))
    {
        Failure::resource()
    } else if message.contains("interrupt") {
        Failure::timeout()
    } else {
        Failure::new(
            "sql_syntax",
            "Query could not be bound or executed",
            "query",
        )
    }
}

#[cfg(test)]
mod provenance_tests {
    use super::*;

    #[test]
    fn local_provenance_is_neither_queryable_nor_an_allowed_file() {
        let directory = tempfile::tempdir().unwrap();
        let mut store =
            crate::Store::open_with_clock(directory.path(), crate::testutil::TestClock::new(0))
                .unwrap();
        store
            .publish(&crate::testutil::update(&[("origin/claude/a", 1)], "a", 0))
            .unwrap();
        assert!(
            !paths(directory.path(), store.manifest())
                .unwrap()
                .iter()
                .any(|(path, _)| path.starts_with("provenance/"))
        );
        assert!(!TABLES.iter().any(|table| table.name == "provenance"));
        let scratch = tempfile::tempdir().unwrap();
        let db = setup(directory.path(), store.manifest(), scratch.path(), "1GiB").unwrap();
        for sql in [
            "SELECT * FROM provenance",
            "SELECT * FROM read_parquet('analytics/provenance/g1.parquet')",
        ] {
            let failure =
                execute_snapshot(&db, store.manifest(), &Request::new(sql), Instant::now())
                    .unwrap_err();
            assert_eq!(failure.error.code, "not_allowed");
        }
    }
}
