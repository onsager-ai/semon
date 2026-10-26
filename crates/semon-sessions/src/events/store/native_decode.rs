//! SQLite physical-cell decoding; the native semantics are shared with PG.
use super::*;
use crate::events::native_decode::Values;

pub(super) fn values(row: &Row<'_>, bools: &[usize]) -> rusqlite::Result<Values> {
    use rusqlite::types::ValueRef;
    let mut values = Vec::with_capacity(row.as_ref().column_count());
    for column in 0..row.as_ref().column_count() {
        values.push(match row.get_ref(column)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(v) if bools.contains(&column) => match v {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return Err(rusqlite::Error::InvalidQuery),
            },
            ValueRef::Integer(v) => v.into(),
            ValueRef::Text(v) => Value::String(
                std::str::from_utf8(v)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?
                    .to_owned(),
            ),
            ValueRef::Blob(v) => {
                serde_json::to_value(v).map_err(|_| rusqlite::Error::InvalidQuery)?
            }
            ValueRef::Real(_) => return Err(rusqlite::Error::InvalidQuery),
        });
    }
    Ok(Values(values))
}
