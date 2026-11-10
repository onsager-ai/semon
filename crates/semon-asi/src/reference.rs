//! Deterministic, generated ASI schema reference.
//!
//! This module is deliberately independent of DuckDB so the schema tool can use the
//! same definitions in ordinary `semon-asi` builds.

use std::fmt::Write;

use crate::schema::{
    ACTIVITY, ASI_SCHEMA_VERSION, COMPUTATION_STATUS, CONTEXT_BAND, Class, Column, INITIATOR, Kind,
    PARENT_METHOD, PRICE_STATUS, PRICE_UNIT, RATE_KIND, ROLE, SOURCE_STATUS, STATUS, TARGET_TYPE,
    TRIGGER, Table, Type, USAGE_OWNER, WASTE_KIND, table as schema_table,
};

/// A named closed enum set in the analytics schema.
pub struct EnumDefinition {
    pub name: &'static str,
    pub values: &'static [&'static str],
}

/// A column exposed by a semantic view.
pub struct ViewColumn {
    pub name: &'static str,
    pub data_type: Type,
    pub nullable: bool,
}

/// A generated query view. `sql` is owned by the query engine; the output shape is
/// declared here so that this module remains free of DuckDB.
pub struct SemanticView {
    pub name: &'static str,
    pub columns: &'static [ViewColumn],
    pub sql: &'static str,
}

/// A table lookup failure for schema consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownTable(pub String);

impl std::fmt::Display for UnknownTable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "unknown ASI table: {}", self.0)
    }
}

impl std::error::Error for UnknownTable {}

pub const ENUMS: &[EnumDefinition] = &[
    EnumDefinition {
        name: "usage_owner",
        values: USAGE_OWNER,
    },
    EnumDefinition {
        name: "context_band",
        values: CONTEXT_BAND,
    },
    EnumDefinition {
        name: "price_status",
        values: PRICE_STATUS,
    },
    EnumDefinition {
        name: "activity",
        values: ACTIVITY,
    },
    EnumDefinition {
        name: "waste_kind",
        values: WASTE_KIND,
    },
    EnumDefinition {
        name: "trigger",
        values: TRIGGER,
    },
    EnumDefinition {
        name: "initiator",
        values: INITIATOR,
    },
    EnumDefinition {
        name: "target_type",
        values: TARGET_TYPE,
    },
    EnumDefinition {
        name: "role",
        values: ROLE,
    },
    EnumDefinition {
        name: "parent_method",
        values: PARENT_METHOD,
    },
    EnumDefinition {
        name: "status",
        values: STATUS,
    },
    EnumDefinition {
        name: "source_status",
        values: SOURCE_STATUS,
    },
    EnumDefinition {
        name: "computation_status",
        values: COMPUTATION_STATUS,
    },
    EnumDefinition {
        name: "rate_kind",
        values: RATE_KIND,
    },
    EnumDefinition {
        name: "price_unit",
        values: PRICE_UNIT,
    },
];

/// Single source for query-function validation and the generated reference.
/// Keep functions pure and conservative; catalog, file, environment, dynamic SQL
/// and extension functions are deliberately absent. Operators are functions in
/// DuckDB's AST.
pub const FUNCTIONS: &[&str] = &[
    "+",
    "-",
    "*",
    "/",
    "//",
    "%",
    "**",
    "=",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "~~",
    "!~~",
    "abs",
    "avg",
    "ceil",
    "ceiling",
    "count",
    "count_star",
    "floor",
    "greatest",
    "least",
    "length",
    "lower",
    "upper",
    "trim",
    "ltrim",
    "rtrim",
    "round",
    "sqrt",
    "sum",
    "min",
    "max",
    "concat",
    "concat_ws",
    "contains",
    "starts_with",
    "ends_with",
    "substring",
    "substr",
    "replace",
    "split_part",
    "date_trunc",
    "date_part",
    "date_diff",
    "strftime",
    "epoch_ms",
    "make_date",
    "to_timestamp",
    "isfinite",
    "isinf",
    "isnan",
    "stddev_pop",
    "stddev_samp",
    "variance",
    "var_pop",
    "var_samp",
    "median",
    "quantile_cont",
    "quantile_disc",
    "bool_and",
    "bool_or",
    "first",
    "last",
    "arg_min",
    "arg_max",
    "string_agg",
    "list",
    "list_value",
    "list_sort",
    "struct_pack",
    "json_extract",
    "json_extract_string",
    "json_valid",
    "to_json",
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    "first_value",
    "last_value",
    "ntile",
    "range",
    "generate_series",
    "unnest",
];

const COST_BY_DAY: &[ViewColumn] = &[
    ViewColumn {
        name: "date_key",
        data_type: Type::Integer,
        nullable: true,
    },
    ViewColumn {
        name: "repo_key",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "model_key",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "price_version",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "usd_est",
        data_type: Type::Float,
        nullable: true,
    },
    ViewColumn {
        name: "unpriced_steps",
        data_type: Type::Integer,
        nullable: false,
    },
];
const STEP_FAILURE: &[ViewColumn] = &[
    ViewColumn {
        name: "step_uid",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "turn_uid",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "session_uid",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "model_key",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "repo_key",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "date_key",
        data_type: Type::Integer,
        nullable: true,
    },
    ViewColumn {
        name: "ts",
        data_type: Type::Timestamp,
        nullable: true,
    },
    ViewColumn {
        name: "input_tokens",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "output_tokens",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "cache_read_tokens",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "cache_write_5m_tokens",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "cache_write_1h_tokens",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "reasoning_tokens",
        data_type: Type::Integer,
        nullable: true,
    },
    ViewColumn {
        name: "web_search_requests",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "speed",
        data_type: Type::Text,
        nullable: true,
    },
    ViewColumn {
        name: "service_tier",
        data_type: Type::Text,
        nullable: true,
    },
    ViewColumn {
        name: "prompt_size_tokens",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "split_unknown",
        data_type: Type::Boolean,
        nullable: false,
    },
    ViewColumn {
        name: "usage_owner",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "context_tokens",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "context_band",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "model_ms",
        data_type: Type::Integer,
        nullable: true,
    },
    ViewColumn {
        name: "tool_ms",
        data_type: Type::Integer,
        nullable: true,
    },
    ViewColumn {
        name: "usd_est",
        data_type: Type::Float,
        nullable: true,
    },
    ViewColumn {
        name: "input_usd",
        data_type: Type::Float,
        nullable: true,
    },
    ViewColumn {
        name: "cache_read_usd",
        data_type: Type::Float,
        nullable: true,
    },
    ViewColumn {
        name: "cache_write_usd",
        data_type: Type::Float,
        nullable: true,
    },
    ViewColumn {
        name: "output_usd",
        data_type: Type::Float,
        nullable: true,
    },
    ViewColumn {
        name: "price_status",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "price_version",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "activity",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "waste_kind",
        data_type: Type::Text,
        nullable: true,
    },
    ViewColumn {
        name: "trigger",
        data_type: Type::Text,
        nullable: true,
    },
    ViewColumn {
        name: "ok",
        data_type: Type::Boolean,
        nullable: true,
    },
    ViewColumn {
        name: "_gen",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "tool_calls",
        data_type: Type::Integer,
        nullable: false,
    },
];
const LANE_SESSION: &[ViewColumn] = &[
    ViewColumn {
        name: "lane_root_uid",
        data_type: Type::Text,
        nullable: true,
    },
    ViewColumn {
        name: "session_uid",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "parent_session_uid",
        data_type: Type::Text,
        nullable: true,
    },
    ViewColumn {
        name: "role",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "repo_key",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "status",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "started_at",
        data_type: Type::Timestamp,
        nullable: true,
    },
    ViewColumn {
        name: "ended_at",
        data_type: Type::Timestamp,
        nullable: true,
    },
];
const CURRENT_LABEL: &[ViewColumn] = &[
    ViewColumn {
        name: "label_id",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "target_type",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "target_uid",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "field",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "value",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "labeller",
        data_type: Type::Text,
        nullable: false,
    },
    ViewColumn {
        name: "taxonomy_version",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "confidence",
        data_type: Type::Float,
        nullable: true,
    },
    ViewColumn {
        name: "created_at",
        data_type: Type::Timestamp,
        nullable: false,
    },
    ViewColumn {
        name: "revision",
        data_type: Type::Integer,
        nullable: false,
    },
    ViewColumn {
        name: "_gen",
        data_type: Type::Integer,
        nullable: false,
    },
];

/// Versioned with the query engine. Spend is at step grain, never multiplied by a
/// tool-call join. Missing prices remain visible as a count and never contribute zero.
pub const SEMANTIC_VIEWS: &[SemanticView] = &[
    SemanticView {
        name: "cost_by_day",
        columns: COST_BY_DAY,
        sql: "SELECT date_key, repo_key, model_key, price_version, sum(usd_est) FILTER (WHERE price_status = 'priced') AS usd_est, count(*) FILTER (WHERE price_status = 'unpriced') AS unpriced_steps FROM fact_step WHERE usage_owner = 'exclusive' GROUP BY ALL",
    },
    SemanticView {
        name: "step_failure",
        columns: STEP_FAILURE,
        sql: "SELECT s.*, coalesce(t.tool_calls, 0) AS tool_calls FROM fact_step s LEFT JOIN (SELECT step_uid, count(*) AS tool_calls FROM fact_tool_call GROUP BY step_uid) t USING (step_uid) WHERE s.ok = false",
    },
    SemanticView {
        name: "lane_session",
        columns: LANE_SESSION,
        sql: "SELECT lane_root_uid, session_uid, parent_session_uid, role, repo_key, status, started_at, ended_at FROM dim_session",
    },
    SemanticView {
        name: "current_label",
        columns: CURRENT_LABEL,
        sql: "SELECT * FROM fact_label QUALIFY row_number() OVER (PARTITION BY target_type, target_uid, field ORDER BY starts_with(labeller, 'human:') DESC, revision DESC, created_at DESC, label_id DESC) = 1",
    },
];

/// Looks up a declared analytics table for the single-table schema response.
pub fn table(name: &str) -> Result<&'static Table, UnknownTable> {
    schema_table(name).ok_or_else(|| UnknownTable(name.into()))
}

/// Renders the canonical schema JSON with stable ordering and a trailing newline.
pub fn json() -> String {
    let mut output = String::new();
    output.push_str("{\n  \"asi_schema_version\": ");
    output.push_str(&ASI_SCHEMA_VERSION.to_string());
    output.push_str(",\n  \"classes\": [");
    for (index, class) in classes().iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        push_json(&mut output, class);
    }
    output.push_str("],\n  \"enums\": [\n");
    for (index, definition) in ENUMS.iter().enumerate() {
        output.push_str("    {\"name\": ");
        push_json(&mut output, definition.name);
        output.push_str(", \"values\": ");
        push_json_strings(&mut output, definition.values);
        output.push('}');
        comma_line(&mut output, index + 1 == ENUMS.len());
    }
    output.push_str("  ],\n  \"tables\": [\n");
    for (index, definition) in crate::schema::TABLES.iter().enumerate() {
        push_table_json(&mut output, definition, 4);
        comma_line(&mut output, index + 1 == crate::schema::TABLES.len());
    }
    output.push_str("  ],\n  \"views\": [\n");
    for (index, view) in SEMANTIC_VIEWS.iter().enumerate() {
        output.push_str("    {\"name\": ");
        push_json(&mut output, view.name);
        output.push_str(", \"columns\": [");
        for (column_index, column) in view.columns.iter().enumerate() {
            if column_index > 0 {
                output.push_str(", ");
            }
            push_view_column_json(&mut output, column);
        }
        output.push_str("]}");
        comma_line(&mut output, index + 1 == SEMANTIC_VIEWS.len());
    }
    output.push_str("  ],\n  \"functions\": ");
    push_json_strings(&mut output, FUNCTIONS);
    output.push_str("\n}\n");
    output
}

/// Renders the human-readable schema reference with stable ordering and a trailing newline.
pub fn markdown() -> String {
    let mut output = String::from("# ASI schema reference\n\n");
    writeln!(output, "ASI schema version: `{ASI_SCHEMA_VERSION}`.").unwrap();
    output.push_str("\nGenerated by `semon_asi::reference`; do not edit this file by hand. Set `SEMON_REGENERATE_ASI_SCHEMA=1` when running the reference drift test to rewrite it.\n\n## Classes\n\n");
    for class in classes() {
        writeln!(output, "- `{class}`").unwrap();
    }
    output.push_str("\n## Enums\n\n");
    for definition in ENUMS {
        writeln!(
            output,
            "- `{}`: {}",
            definition.name,
            definition
                .values
                .iter()
                .map(|value| format!("`{value}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .unwrap();
    }
    output.push_str("\n## Tables\n");
    for definition in crate::schema::TABLES {
        writeln!(
            output,
            "\n### `{}`\n\nGrain: {} Kind: `{}`.{}",
            definition.name,
            definition.grain,
            kind_name(definition.kind),
            definition
                .sort_time
                .map(|time| format!(" Sort time: `{time}`."))
                .unwrap_or_default()
        )
        .unwrap();
        output.push_str("\n| Column | Type | Nullable | Class | Unit | Values | Description |\n| --- | --- | --- | --- | --- | --- | --- |\n");
        for column in definition.columns {
            writeln!(
                output,
                "| `{}` | `{}` | {} | `{}` | {} | {} | {} |",
                column.name,
                type_name(column.data_type),
                column.nullable,
                class_name(column.class),
                display_unit(column.unit),
                display_values(column.values),
                column.doc
            )
            .unwrap();
        }
    }
    output.push_str("\n## Semantic views\n");
    for view in SEMANTIC_VIEWS {
        writeln!(
            output,
            "\n### `{}`\n\n| Column | Type | Nullable |\n| --- | --- | --- |",
            view.name
        )
        .unwrap();
        for column in view.columns {
            writeln!(
                output,
                "| `{}` | `{}` | {} |",
                column.name,
                type_name(column.data_type),
                column.nullable
            )
            .unwrap();
        }
    }
    output.push_str("\n## Function allowlist\n\n");
    for function in FUNCTIONS {
        writeln!(output, "- `{function}`").unwrap();
    }
    output
}

fn classes() -> [&'static str; 7] {
    ["key", "enum", "number", "time", "name", "path", "body"]
}
fn class_name(class: Class) -> &'static str {
    match class {
        Class::Key => "key",
        Class::Enum => "enum",
        Class::Number => "number",
        Class::Time => "time",
        Class::Name => "name",
        Class::Path => "path",
        Class::Body => "body",
    }
}
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Session => "session",
        Kind::Append => "append",
        Kind::Dimension => "dimension",
    }
}
fn type_name(data_type: Type) -> &'static str {
    match data_type {
        Type::Text => "text",
        Type::Integer => "integer",
        Type::Float => "float",
        Type::Boolean => "boolean",
        Type::Timestamp => "timestamp_ms_utc",
    }
}
fn display_unit(unit: &str) -> String {
    if unit.is_empty() {
        "".into()
    } else {
        format!("`{unit}`")
    }
}
fn display_values(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn push_json(output: &mut String, value: &str) {
    output.push_str(&serde_json::to_string(value).expect("static reference strings serialize"));
}
fn push_json_strings(output: &mut String, values: &[&str]) {
    output.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        push_json(output, value);
    }
    output.push(']');
}
fn comma_line(output: &mut String, last: bool) {
    if !last {
        output.push(',');
    }
    output.push('\n');
}
fn push_column_json(output: &mut String, column: &Column) {
    output.push_str("{\"name\": ");
    push_json(output, column.name);
    output.push_str(", \"type\": ");
    push_json(output, type_name(column.data_type));
    output.push_str(", \"nullable\": ");
    output.push_str(&column.nullable.to_string());
    output.push_str(", \"class\": ");
    push_json(output, class_name(column.class));
    output.push_str(", \"unit\": ");
    push_json(output, column.unit);
    output.push_str(", \"values\": ");
    push_json_strings(output, column.values);
    output.push_str(", \"description\": ");
    push_json(output, column.doc);
    output.push('}');
}
fn push_view_column_json(output: &mut String, column: &ViewColumn) {
    output.push_str("{\"name\": ");
    push_json(output, column.name);
    output.push_str(", \"type\": ");
    push_json(output, type_name(column.data_type));
    output.push_str(", \"nullable\": ");
    output.push_str(&column.nullable.to_string());
    output.push('}');
}
fn push_table_json(output: &mut String, table: &Table, indent: usize) {
    let pad = " ".repeat(indent);
    output.push_str(&pad);
    output.push_str("{\"name\": ");
    push_json(output, table.name);
    output.push_str(", \"grain\": ");
    push_json(output, table.grain);
    output.push_str(", \"kind\": ");
    push_json(output, kind_name(table.kind));
    output.push_str(", \"sort_time\": ");
    match table.sort_time {
        Some(value) => push_json(output, value),
        None => output.push_str("null"),
    };
    output.push_str(", \"columns\": [");
    for (index, column) in table.columns.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        push_column_json(output, column);
    }
    output.push_str("]}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn reference_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference")
            .join(name)
    }

    fn checked_in(name: &str, expected: String) {
        let path = reference_path(name);
        if std::env::var_os("SEMON_REGENERATE_ASI_SCHEMA").is_some() {
            fs::write(&path, expected).unwrap();
            return;
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            expected,
            "{} is stale; regenerate with SEMON_REGENERATE_ASI_SCHEMA=1 cargo test -p semon-asi reference::tests::checked_in_reference_files",
            path.display()
        );
    }

    #[test]
    fn checked_in_reference_files() {
        checked_in("asi-schema.json", json());
        checked_in("asi-schema.md", markdown());
    }

    #[test]
    fn every_table_view_and_function_is_rendered() {
        let json = serde_json::from_str::<serde_json::Value>(&json()).unwrap();
        let markdown = markdown();
        for table in crate::schema::TABLES {
            let rendered = json["tables"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["name"].as_str() == Some(table.name))
                .unwrap();
            let section = markdown
                .split(&format!("### `{}`", table.name))
                .nth(1)
                .unwrap()
                .split("\n### ")
                .next()
                .unwrap();
            for column in table.columns {
                assert!(
                    rendered["columns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["name"].as_str() == Some(column.name)),
                    "JSON omits {}.{}",
                    table.name,
                    column.name
                );
                assert!(
                    section.contains(&format!("`{}`", column.name)),
                    "Markdown omits {}.{}",
                    table.name,
                    column.name
                );
            }
        }
        for view in SEMANTIC_VIEWS {
            let rendered = json["views"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["name"].as_str() == Some(view.name))
                .unwrap();
            let section = markdown
                .split(&format!("### `{}`", view.name))
                .nth(1)
                .unwrap()
                .split("\n### ")
                .next()
                .unwrap();
            for column in view.columns {
                assert!(
                    rendered["columns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["name"].as_str() == Some(column.name)),
                    "JSON omits {}.{}",
                    view.name,
                    column.name
                );
                assert!(
                    section.contains(&format!("`{}`", column.name)),
                    "Markdown omits {}.{}",
                    view.name,
                    column.name
                );
            }
        }
        let rendered = json["functions"].as_array().unwrap();
        for function in FUNCTIONS {
            assert!(
                rendered
                    .iter()
                    .any(|entry| entry.as_str() == Some(function)),
                "JSON omits {function}"
            );
            assert!(markdown.contains(&format!("- `{function}`")));
        }
    }

    #[test]
    fn table_lookup_is_exact() {
        assert_eq!(table("fact_step").unwrap().name, "fact_step");
        assert_eq!(
            table("fact_nothing").unwrap_err(),
            UnknownTable("fact_nothing".into())
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        assert_eq!(json(), json());
        assert_eq!(markdown(), markdown());
    }
}
