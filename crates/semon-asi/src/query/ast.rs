//! Pinned DuckDB JSON AST grammar. New nodes *and new fields* fail closed.
//! No SQL is bound/executed to discover whether a table/function is acceptable.
use std::collections::BTreeSet;

use duckdb::{Connection, params};
use serde_json::Value;

use super::{Failure, SEMANTIC_VIEWS};
use crate::reference::FUNCTIONS;
use crate::schema::TABLES;

pub(super) fn serialize_sql(db: &Connection, sql: &str) -> Result<String, Failure> {
    // JsonSerializeBind checks the first argument's exact type before DuckDB
    // adds implicit argument casts. An untyped prepared parameter fails binding.
    db.query_row(
        "SELECT CAST(json_serialize_sql(CAST(? AS VARCHAR)) AS VARCHAR)",
        params![sql],
        |row| row.get(0),
    )
    .map_err(|error| syntax_error(&error.to_string()))
}

fn syntax_error(message: &str) -> Failure {
    // This parser sees only caller SQL, never analytics values or body text.
    // Bound diagnostic size so even a malformed query cannot bypass byte caps.
    Failure::new(
        "sql_syntax",
        &message.chars().take(4096).collect::<String>(),
        "query",
    )
}

pub(super) fn check(db: &Connection, sql: &str) -> Result<(), Failure> {
    statement_count(sql)?;
    let json = serialize_sql(db, sql)?;
    let value: Value =
        serde_json::from_str(&json).map_err(|_| Failure::denied("AST serialization"))?;
    if value["error"].as_bool() == Some(true) {
        // json_serialize_sql refuses every non-SELECT before serialization.
        // Parse diagnostics may quote caller SQL, but never store/body contents.
        return if value["error_type"].as_str() == Some("not implemented") {
            Err(Failure::denied("non-SELECT statement"))
        } else {
            Err(syntax_error(
                value["error_message"]
                    .as_str()
                    .unwrap_or("Could not parse query"),
            ))
        };
    }
    let statements = value["statements"]
        .as_array()
        .ok_or_else(|| Failure::denied("statements"))?;
    if statements.len() > 1 {
        return Err(Failure::new(
            "multiple_statements",
            "Exactly one SELECT is required",
            "statements",
        ));
    }
    let statement = statements
        .first()
        .ok_or_else(|| Failure::new("sql_syntax", "Empty query", "query"))?;
    walk(statement, &BTreeSet::new(), 0)
}

// This lexical pass only detects multiple statements and names an obvious
// statement refusal. It never grants permission: the DuckDB AST must still pass.
// SQL strings, quoted identifiers, dollar strings and nested comments can contain
// semicolons. Keeping them opaque avoids both false positives and split payloads.
fn statement_count(sql: &str) -> Result<(), Failure> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut statements = 0;
    let mut content = false;
    let mut first_word = None;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"--") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if bytes[i] == b';' {
            if content {
                statements += 1;
                content = false;
            }
            i += 1;
            continue;
        }
        if !content && statements == 0 && bytes[i].is_ascii_alphabetic() {
            let end = bytes[i..]
                .iter()
                .position(|b| !b.is_ascii_alphabetic())
                .map_or(bytes.len(), |n| i + n);
            first_word = Some(sql[i..end].to_ascii_uppercase());
        }
        content = true;
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == quote {
                    i += 1;
                    if i < bytes.len() && bytes[i] == quote {
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if bytes[i] == b'$' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'$' {
                i += 1;
                let delimiter = &bytes[start..i];
                while i < bytes.len() && !bytes[i..].starts_with(delimiter) {
                    i += 1;
                }
                i = (i + delimiter.len()).min(bytes.len());
            }
        } else {
            i += 1;
        }
    }
    statements += usize::from(content);
    if statements > 1 {
        return Err(Failure::new(
            "multiple_statements",
            "Exactly one SELECT is required",
            "statements",
        ));
    }
    if let Some(word) = first_word.filter(|w| {
        [
            "ATTACH", "DETACH", "COPY", "INSTALL", "LOAD", "SET", "RESET", "PRAGMA", "CREATE",
            "ALTER", "INSERT", "DELETE", "UPDATE", "DROP", "CALL", "EXPLAIN", "DESCRIBE", "SHOW",
            "EXPORT", "IMPORT", "VACUUM", "BEGIN", "COMMIT", "ROLLBACK",
        ]
        .contains(&w.as_str())
    }) {
        return Err(Failure::denied(word));
    }
    Ok(())
}

fn fields(value: &Value, allowed: &[&str]) -> Result<(), Failure> {
    let object = value
        .as_object()
        .ok_or_else(|| Failure::denied("AST object"))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(Failure::denied(format!("AST field {key}")));
    }
    Ok(())
}

fn tag<'a>(value: &'a Value, key: &str) -> Result<&'a str, Failure> {
    value[key]
        .as_str()
        .ok_or_else(|| Failure::denied(format!("AST {key}")))
}

fn qualification(value: &Value, key: &str, allowed: &[&str]) -> Result<(), Failure> {
    if let Some(actual) = value.get(key)
        && !actual.as_str().is_some_and(|s| allowed.contains(&s))
    {
        return Err(Failure::denied(format!("qualified {key}={actual}")));
    }
    Ok(())
}

fn unqualified(value: &Value, keys: &[&str]) -> Result<(), Failure> {
    for key in keys {
        qualification(value, key, &[""])?;
    }
    Ok(())
}

fn function(value: &Value) -> Result<(), Failure> {
    let name = tag(value, "function_name")?.to_ascii_lowercase();
    if !FUNCTIONS.contains(&name.as_str()) {
        return Err(Failure::denied(name));
    }
    // libpg_query's SystemFuncName emits schema="main" for [] and {}
    // constructors. No schema inference or arbitrary qualification is allowed;
    // the fresh locked database has no caller-defined functions to shadow these.
    let schemas: &[&str] = if ["list_value", "struct_pack"].contains(&name.as_str()) {
        &["", "main"]
    } else {
        &[""]
    };
    qualification(value, "schema", schemas)?;
    qualification(value, "catalog", &[""])?;
    if value["export_state"].as_bool() == Some(true) {
        return Err(Failure::denied("export_state"));
    }
    Ok(())
}

fn logical_type(value: &Value) -> Result<(), Failure> {
    fields(value, &["id", "type_info"])?;
    let id = tag(value, "id")?;
    if id == "UNBOUND" {
        let info = &value["type_info"];
        fields(info, &["type", "alias", "extension_info", "expr"])?;
        if info["type"].as_str() != Some("UNBOUND_TYPE_INFO")
            || info["alias"].as_str().is_some_and(|s| !s.is_empty())
            || !info["extension_info"].is_null()
        {
            return Err(Failure::denied("unbound type metadata"));
        }
        return walk(&info["expr"], &BTreeSet::new(), 0);
    }
    if ![
        "NULL",
        "BOOLEAN",
        "TINYINT",
        "SMALLINT",
        "INTEGER",
        "BIGINT",
        "HUGEINT",
        "UTINYINT",
        "USMALLINT",
        "UINTEGER",
        "UBIGINT",
        "UHUGEINT",
        "FLOAT",
        "DOUBLE",
        "VARCHAR",
        "BLOB",
        "DATE",
        "TIME",
        "TIMESTAMP",
        "TIMESTAMP_S",
        "TIMESTAMP_MS",
        "TIMESTAMP_NS",
        "TIMESTAMP WITH TIME ZONE",
        "INTERVAL",
        "DECIMAL",
    ]
    .contains(&id)
    {
        return Err(Failure::denied(format!("type {id}")));
    }
    if let Some(info) = value.get("type_info").filter(|v| !v.is_null()) {
        // Decimal's sole extra metadata is width/scale. Refuse aliases and user types.
        fields(info, &["type", "alias", "extension_info", "width", "scale"])?;
        if id != "DECIMAL"
            || info["type"].as_str() != Some("DECIMAL_TYPE_INFO")
            || info["alias"].as_str().is_some_and(|s| !s.is_empty())
            || !info["extension_info"].is_null()
        {
            return Err(Failure::denied("type_info"));
        }
    }
    Ok(())
}

/// Context travels down the tree, so CTE names are usable only within their
/// lexical scope. A CTE declaration is checked just like the outer SELECT.
fn walk(value: &Value, ctes: &BTreeSet<String>, depth: usize) -> Result<(), Failure> {
    if depth > 128 {
        return Err(Failure::denied("AST depth"));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                walk(value, ctes, depth + 1)?;
            }
            return Ok(());
        }
        Value::Object(_) => (),
        _ => return Ok(()),
    }
    let mut scope = ctes.clone();
    if let Some(map) = value.get("cte_map") {
        fields(map, &["map"])?;
        if let Some(entries) = map.get("map") {
            for entry in entries
                .as_array()
                .ok_or_else(|| Failure::denied("cte_map"))?
            {
                fields(entry, &["key", "value"])?;
                scope.insert(tag(entry, "key")?.to_ascii_lowercase());
            }
        }
    }
    if value["type"].as_str() == Some("RECURSIVE_CTE_NODE") {
        scope.insert(tag(value, "cte_name")?.to_ascii_lowercase());
    }
    let allowed: &[&str] = if value.get("class").is_some() {
        let class = tag(value, "class")?;
        let ty = tag(value, "type")?;
        let types: &[&str] = match class {
            "CONSTANT" => &["VALUE_CONSTANT"],
            "TYPE" => &["TYPE"],
            "COLUMN_REF" => &["COLUMN_REF"],
            "FUNCTION" => &["FUNCTION"],
            "CAST" => &["OPERATOR_CAST", "CAST"],
            "STAR" => &["STAR", "TABLE_STAR"],
            "COMPARISON" => &[
                "COMPARE_EQUAL",
                "COMPARE_NOTEQUAL",
                "COMPARE_LESSTHAN",
                "COMPARE_GREATERTHAN",
                "COMPARE_LESSTHANOREQUALTO",
                "COMPARE_GREATERTHANOREQUALTO",
                "COMPARE_DISTINCT_FROM",
                "COMPARE_NOT_DISTINCT_FROM",
            ],
            "CONJUNCTION" => &["CONJUNCTION_AND", "CONJUNCTION_OR"],
            "BETWEEN" => &["COMPARE_BETWEEN", "COMPARE_NOT_BETWEEN"],
            "CASE" => &["CASE_EXPR"],
            "PARAMETER" => &["VALUE_PARAMETER"],
            "POSITIONAL_REFERENCE" => &["POSITIONAL_REFERENCE"],
            "SUBQUERY" => &["SUBQUERY"],
            "OPERATOR" => &[
                "OPERATOR_NOT",
                "OPERATOR_IS_NULL",
                "OPERATOR_IS_NOT_NULL",
                "COMPARE_IN",
                "COMPARE_NOT_IN",
                "OPERATOR_COALESCE",
                "OPERATOR_NULLIF",
                "ARRAY_EXTRACT",
                "ARRAY_SLICE",
                "STRUCT_EXTRACT",
                "ARRAY_CONSTRUCTOR",
                "ARROW",
            ],
            "WINDOW" => &[
                "WINDOW_AGGREGATE",
                "WINDOW_RANK",
                "WINDOW_RANK_DENSE",
                "WINDOW_NTILE",
                "WINDOW_ROW_NUMBER",
                "WINDOW_FIRST_VALUE",
                "WINDOW_LAST_VALUE",
                "WINDOW_LEAD",
                "WINDOW_LAG",
            ],
            _ => return Err(Failure::denied(format!("AST class {class}"))),
        };
        if !types.contains(&ty) {
            return Err(Failure::denied(format!("AST type {ty}")));
        }
        match class {
            "TYPE" => {
                unqualified(value, &["schema", "catalog"])?;
                let name = tag(value, "type_name")?.to_ascii_uppercase();
                if ![
                    "BOOLEAN",
                    "TINYINT",
                    "SMALLINT",
                    "INTEGER",
                    "BIGINT",
                    "HUGEINT",
                    "UTINYINT",
                    "USMALLINT",
                    "UINTEGER",
                    "UBIGINT",
                    "UHUGEINT",
                    "FLOAT",
                    "DOUBLE",
                    "VARCHAR",
                    "BLOB",
                    "DATE",
                    "TIME",
                    "TIMESTAMP",
                    "TIMESTAMP_MS",
                    "TIMESTAMP_NS",
                    "TIMESTAMP_S",
                    "TIMESTAMP WITH TIME ZONE",
                    "INTERVAL",
                    "DECIMAL",
                    "LIST",
                    "ARRAY",
                    "STRUCT",
                    "MAP",
                    "JSON",
                ]
                .contains(&name.as_str())
                {
                    return Err(Failure::denied(format!("type {name}")));
                }
                &[
                    "class",
                    "type",
                    "alias",
                    "query_location",
                    "catalog",
                    "schema",
                    "type_name",
                    "children",
                ]
            }
            "FUNCTION" => {
                function(value)?;
                &[
                    "class",
                    "type",
                    "alias",
                    "query_location",
                    "function_name",
                    "schema",
                    "catalog",
                    "children",
                    "filter",
                    "order_bys",
                    "distinct",
                    "is_operator",
                    "export_state",
                ]
            }
            "WINDOW" => {
                function(value)?;
                &[
                    "class",
                    "type",
                    "alias",
                    "query_location",
                    "function_name",
                    "schema",
                    "catalog",
                    "children",
                    "partitions",
                    "orders",
                    "start",
                    "end",
                    "start_expr",
                    "end_expr",
                    "offset_expr",
                    "default_expr",
                    "ignore_nulls",
                    "filter_expr",
                    "exclude_clause",
                    "distinct",
                    "arg_orders",
                ]
            }
            "CONSTANT" => {
                fields(
                    value,
                    &["class", "type", "alias", "query_location", "value"],
                )?;
                fields(&value["value"], &["type", "is_null", "value"])?;
                logical_type(&value["value"]["type"])?;
                return Ok(()); // literal payloads are data, never AST nodes
            }
            "CAST" => {
                logical_type(&value["cast_type"])?;
                &[
                    "class",
                    "type",
                    "alias",
                    "query_location",
                    "child",
                    "cast_type",
                    "try_cast",
                ]
            }
            "COLUMN_REF" => &["class", "type", "alias", "query_location", "column_names"],
            "STAR" => {
                if value["columns"].as_bool() == Some(true)
                    || !value["expr"].is_null()
                    || value["replace_list"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())
                    || value["rename_list"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())
                {
                    return Err(Failure::denied("COLUMNS/REPLACE/RENAME"));
                }
                &[
                    "class",
                    "type",
                    "alias",
                    "query_location",
                    "relation_name",
                    "exclude_list",
                    "replace_list",
                    "columns",
                    "expr",
                    "qualified_exclude_list",
                    "rename_list",
                ]
            }
            "COMPARISON" => &["class", "type", "alias", "query_location", "left", "right"],
            "CONJUNCTION" | "OPERATOR" => &["class", "type", "alias", "query_location", "children"],
            "BETWEEN" => &[
                "class",
                "type",
                "alias",
                "query_location",
                "input",
                "lower",
                "upper",
            ],
            "CASE" => &[
                "class",
                "type",
                "alias",
                "query_location",
                "case_checks",
                "else_expr",
            ],
            "PARAMETER" => &["class", "type", "alias", "query_location", "identifier"],
            "POSITIONAL_REFERENCE" => &["class", "type", "alias", "query_location", "index"],
            "SUBQUERY" => &[
                "class",
                "type",
                "alias",
                "query_location",
                "subquery_type",
                "subquery",
                "child",
                "comparison_type",
            ],
            _ => return Err(Failure::denied(class)),
        }
    } else if let Some(ty) = value.get("type").and_then(Value::as_str) {
        match ty {
            "SELECT_NODE" => &[
                "type",
                "modifiers",
                "cte_map",
                "select_list",
                "from_table",
                "where_clause",
                "group_expressions",
                "group_sets",
                "aggregate_handling",
                "having",
                "sample",
                "qualify",
            ],
            "SET_OPERATION_NODE" => &[
                "type",
                "modifiers",
                "cte_map",
                "setop_type",
                "left",
                "right",
                "setop_all",
                "children",
            ],
            "RECURSIVE_CTE_NODE" => &[
                "type",
                "modifiers",
                "cte_map",
                "cte_name",
                "union_all",
                "left",
                "right",
                "aliases",
                "key_targets",
            ],
            "BASE_TABLE" => {
                let name = tag(value, "table_name")?.to_ascii_lowercase();
                unqualified(value, &["schema_name", "catalog_name"])?;
                if !TABLES.iter().any(|t| t.name == name)
                    && !SEMANTIC_VIEWS.iter().any(|v| v.name == name)
                    && !scope.contains(&name)
                {
                    return Err(Failure::denied(name));
                }
                &[
                    "type",
                    "alias",
                    "sample",
                    "query_location",
                    "schema_name",
                    "table_name",
                    "column_name_alias",
                    "catalog_name",
                    "at_clause",
                ]
            }
            "EMPTY" => &["type", "alias", "sample", "query_location"],
            "JOIN" => &[
                "type",
                "alias",
                "sample",
                "query_location",
                "left",
                "right",
                "condition",
                "join_type",
                "ref_type",
                "using_columns",
                "delim_flipped",
                "duplicate_eliminated_columns",
                "is_implicit",
            ],
            "SUBQUERY" => &[
                "type",
                "alias",
                "sample",
                "query_location",
                "subquery",
                "column_name_alias",
            ],
            "TABLE_FUNCTION" => &[
                "type",
                "alias",
                "sample",
                "query_location",
                "function",
                "column_name_alias",
                "with_ordinality",
            ],
            "EXPRESSION_LIST" => &[
                "type",
                "alias",
                "sample",
                "query_location",
                "expected_names",
                "expected_types",
                "values",
            ],
            "LIMIT_MODIFIER" => &["type", "limit", "offset"],
            "DISTINCT_MODIFIER" => &["type", "distinct_on_targets"],
            "ORDER_MODIFIER" => &["type", "orders"],
            "ORDER_DEFAULT" | "ASCENDING" | "DESCENDING" => &["type", "null_order", "expression"],
            _ => return Err(Failure::denied(format!("AST node {ty}"))),
        }
    } else if value.get("node").is_some() {
        &["node", "named_param_map"]
    } else if value.get("query").is_some() {
        &["aliases", "query", "materialized", "key_targets"]
    } else if value.get("map").is_some() || value.as_object().is_some_and(|o| o.is_empty()) {
        &["map"]
    } else if value.get("key").is_some() {
        &["key", "value"]
    } else if value.get("when_expr").is_some() {
        &["when_expr", "then_expr"]
    } else {
        return Err(Failure::denied("unknown AST object"));
    };
    fields(value, allowed)?;
    if value.get("sample").is_some_and(|v| !v.is_null())
        || value.get("at_clause").is_some_and(|v| !v.is_null())
    {
        return Err(Failure::denied("sample/time travel"));
    }
    for (key, child) in value.as_object().unwrap() {
        if key == "cast_type" {
            continue;
        }
        walk(child, &scope, depth + 1)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn check_tree(value: &Value) -> Result<(), Failure> {
    walk(value, &BTreeSet::new(), 0)
}
