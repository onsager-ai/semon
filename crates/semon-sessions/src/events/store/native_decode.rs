//! SQLite physical-cell decoding; the native semantics are shared with PG.
use super::*;
use crate::events::native_decode::Values;

pub(crate) fn values(row: &Row<'_>, bools: &[usize]) -> rusqlite::Result<Values> {
    use rusqlite::types::{Type, ValueRef};
    let mut values = Vec::with_capacity(row.as_ref().column_count());
    for column in 0..row.as_ref().column_count() {
        values.push(match row.get_ref(column)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(v) if bools.contains(&column) => match v {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return Err(malformed(column, Type::Integer)),
            },
            ValueRef::Integer(v) => v.into(),
            ValueRef::Text(v) => Value::String(
                std::str::from_utf8(v)
                    .map_err(|_| malformed(column, Type::Text))?
                    .to_owned(),
            ),
            ValueRef::Blob(v) => {
                serde_json::to_value(v).map_err(|_| malformed(column, Type::Blob))?
            }
            ValueRef::Real(_) => return Err(malformed(column, Type::Real)),
        });
    }
    Ok(Values(values))
}

/// Malformed current native cells remain data errors, so callers can refuse the
/// affected file without hiding healthy files or misclassifying a driver error.
pub(super) fn malformed(column: usize, kind: rusqlite::types::Type) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, kind, "invalid current native row".into())
}
