//! Origin warehouse storage. Its source keys and maintenance are local to one Index.

use std::sync::OnceLock;

use rusqlite::{Connection, OptionalExtension};

pub(super) const SCHEMA: &str = include_str!("warehouse.sql");

/// Check the authored DDL, including keys and indexes, on every qualified open.
/// The SQL text in sqlite_schema omits the statement terminator.
pub(super) fn qualify(connection: &Connection) -> rusqlite::Result<bool> {
    static OBJECTS: OnceLock<Vec<(&str, &str)>> = OnceLock::new();
    let objects = OBJECTS.get_or_init(|| {
        SCHEMA
            .split(';')
            .filter_map(|statement| {
                let sql = statement.get(statement.find("CREATE ")?..)?.trim_end();
                let name = sql.split_ascii_whitespace().nth(2)?.split('(').next()?;
                Some((name, sql))
            })
            .collect()
    });
    let mut query = connection.prepare_cached("SELECT sql FROM sqlite_schema WHERE name=?1")?;
    for &(name, expected) in objects {
        let sql = query
            .query_row([name], |row| row.get::<_, String>(0))
            .optional()?;
        if sql.as_deref() != Some(expected) {
            return Ok(false);
        }
    }
    Ok(true)
}
