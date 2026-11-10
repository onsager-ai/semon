//! Analytics tables: facts and dimensions, with a class, unit and doc for every column.
//!
//! The declarations are the single source for the Arrow schemas written to Parquet,
//! the Hub export (which keys on the class) and the generated schema reference.
//! Timestamps are UTC milliseconds and enums are closed sets. Analytics holds no
//! `body` column; the class exists so that no table can gain one unnoticed.

use arrow::datatypes::{DataType, Field, Schema, TimeUnit};

/// Bumped on any change to a table, column, type, class or enum set. A store whose
/// manifest carries another version is refused (asi-store.md, Version bumps).
pub const ASI_SCHEMA_VERSION: u32 = 3;

/// Name of the generation column the writer appends to every file.
pub const GEN_COLUMN: &str = "_gen";

/// Name of the column that scopes a row to a session, and is looked up in the live map.
pub const SESSION_COLUMN: &str = "session_uid";

/// What a column is for. Every column declares one; `Column::new` takes it as an
/// argument, so a missing class does not compile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    /// Identity: UIDs and natural dimension keys.
    Key,
    /// A closed set of values, listed in `Column::values`.
    Enum,
    /// A measure or count.
    Number,
    /// A UTC timestamp.
    Time,
    /// A display name or free-form short label.
    Name,
    /// A file-system path (tokenised on Hub export).
    Path,
    /// Free text. Never present in analytics.
    Body,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Type {
    Text,
    Integer,
    Float,
    Boolean,
    /// UTC milliseconds.
    Timestamp,
}

impl Type {
    pub fn arrow(self) -> DataType {
        match self {
            Self::Text => DataType::Utf8,
            Self::Integer => DataType::Int64,
            Self::Float => DataType::Float64,
            Self::Boolean => DataType::Boolean,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Column {
    pub name: &'static str,
    pub data_type: Type,
    pub nullable: bool,
    pub class: Class,
    /// The closed value set of an `Enum` column; empty otherwise.
    pub values: &'static [&'static str],
    /// The unit of a measure (`ms`, `tokens`, `usd`); empty when it has none.
    pub unit: &'static str,
    pub doc: &'static str,
}

impl Column {
    pub const fn new(
        name: &'static str,
        data_type: Type,
        nullable: bool,
        class: Class,
        doc: &'static str,
    ) -> Self {
        Self {
            name,
            data_type,
            nullable,
            class,
            values: &[],
            unit: "",
            doc,
        }
    }
}

/// How a table relates to the live map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Rows belong to a session and are live only when `(session_uid, _gen)` is in
    /// the live map. A session is recomputed whole, so its older rows go dead.
    Session,
    /// Append-only rows, never dead (`fact_label`).
    Append,
    /// Rewritten whole by every publish that touches it.
    Dimension,
}

#[derive(Debug)]
pub struct Table {
    pub name: &'static str,
    pub grain: &'static str,
    pub kind: Kind,
    /// The `Time` column that orders level-1 files after the namespace.
    pub sort_time: Option<&'static str>,
    pub columns: &'static [Column],
}

impl Table {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    /// The declared columns, which is what callers hand to the writer.
    pub fn arrow_schema(&self) -> Schema {
        Schema::new(self.fields())
    }

    /// The declared columns plus the writer's `_gen`, which is what files hold.
    pub fn file_schema(&self) -> Schema {
        let mut fields = self.fields();
        fields.push(Field::new(GEN_COLUMN, DataType::Int64, false));
        Schema::new(fields)
    }

    fn fields(&self) -> Vec<Field> {
        self.columns
            .iter()
            .map(|column| Field::new(column.name, column.data_type.arrow(), column.nullable))
            .collect()
    }
}

/// Schema of `analytics/live/g{generation}.parquet`.
pub fn live_map_schema() -> Schema {
    Schema::new(vec![
        Field::new(SESSION_COLUMN, DataType::Utf8, false),
        Field::new(GEN_COLUMN, DataType::Int64, false),
    ])
}

const fn col(
    name: &'static str,
    ty: Type,
    nullable: bool,
    class: Class,
    doc: &'static str,
) -> Column {
    Column::new(name, ty, nullable, class, doc)
}

const fn key(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Text, false, Class::Key, doc)
}

const fn key_opt(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Text, true, Class::Key, doc)
}

const fn int_key(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Integer, false, Class::Key, doc)
}

const fn int_key_opt(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Integer, true, Class::Key, doc)
}

const fn en(name: &'static str, values: &'static [&'static str], doc: &'static str) -> Column {
    let mut column = col(name, Type::Text, false, Class::Enum, doc);
    column.values = values;
    column
}

const fn en_opt(name: &'static str, values: &'static [&'static str], doc: &'static str) -> Column {
    let mut column = en(name, values, doc);
    column.nullable = true;
    column
}

const fn num(name: &'static str, unit: &'static str, doc: &'static str) -> Column {
    let mut column = col(name, Type::Integer, false, Class::Number, doc);
    column.unit = unit;
    column
}

const fn num_opt(name: &'static str, unit: &'static str, doc: &'static str) -> Column {
    let mut column = num(name, unit, doc);
    column.nullable = true;
    column
}

const fn real_opt(name: &'static str, unit: &'static str, doc: &'static str) -> Column {
    let mut column = col(name, Type::Float, true, Class::Number, doc);
    column.unit = unit;
    column
}

const fn real(name: &'static str, unit: &'static str, doc: &'static str) -> Column {
    let mut column = real_opt(name, unit, doc);
    column.nullable = false;
    column
}

const fn flag(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Boolean, false, Class::Enum, doc)
}

const fn flag_opt(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Boolean, true, Class::Enum, doc)
}

const fn ts(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Timestamp, false, Class::Time, doc)
}

const fn ts_opt(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Timestamp, true, Class::Time, doc)
}

const fn label(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Text, false, Class::Name, doc)
}

const fn label_opt(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Text, true, Class::Name, doc)
}

const fn path_opt(name: &'static str, doc: &'static str) -> Column {
    col(name, Type::Text, true, Class::Path, doc)
}

const USAGE_OWNER: &[&str] = &["exclusive", "copied"];
const CONTEXT_BAND: &[&str] = &["under_50k", "50k_to_150k", "over_150k"];
const PRICE_STATUS: &[&str] = &["priced", "unpriced"];
const ACTIVITY: &[&str] = &[
    "coordinate",
    "wait",
    "integrate",
    "verify",
    "recover",
    "implement",
    "explore",
    "setup",
    "report",
    "think",
    "unknown",
];
const WASTE_KIND: &[&str] = &[
    "wait",
    "failed_call",
    "retry",
    "redundant_read",
    "context_refill",
];
const TRIGGER: &[&str] = &[
    "tool_error",
    "empty_result",
    "test_or_ci_failure",
    "external_error",
    "compaction",
    "polling",
    "worker_result",
    "instruction_change",
    "missing_input",
    "none_visible",
];
const INITIATOR: &[&str] = &["user", "manager", "peer", "self", "unknown"];
const TARGET_TYPE: &[&str] = &["session", "turn", "step", "compaction", "lane", "work_item"];
const ROLE: &[&str] = &["manager", "worker", "reviewer", "solo", "unknown"];
const PARENT_METHOD: &[&str] = &["native", "workspace", "none"];
const STATUS: &[&str] = &["active", "ended", "unknown"];
const SOURCE_STATUS: &[&str] = &["complete", "missing"];
const COMPUTATION_STATUS: &[&str] = &["current", "pending", "failed", "frozen"];
const RATE_KIND: &[&str] = &[
    "input",
    "output",
    "cache_read",
    "cache_write_5m",
    "cache_write_1h",
    "reasoning",
    "web_search_request",
];
const PRICE_UNIT: &[&str] = &["mtok", "request"];

pub static FACT_STEP: Table = Table {
    name: "fact_step",
    grain: "One model invocation.",
    kind: Kind::Session,
    sort_time: Some("ts"),
    columns: &[
        key("step_uid", "Step UID."),
        key("turn_uid", "Turn UID."),
        key("session_uid", "Session UID."),
        key("model_key", "Key into dim_model."),
        key(
            "repo_key",
            "Key into dim_repo; never a path or a hash of one.",
        ),
        int_key_opt("date_key", "UTC start date as yyyymmdd; key into dim_date."),
        ts_opt(
            "ts",
            "Start of the model call, when the harness recorded one.",
        ),
        num("input_tokens", "tokens", "Uncached input tokens."),
        num("output_tokens", "tokens", "Output tokens."),
        num(
            "cache_read_tokens",
            "tokens",
            "Input tokens read from the cache.",
        ),
        num(
            "cache_write_5m_tokens",
            "tokens",
            "Input tokens written to the 5-minute cache.",
        ),
        num(
            "cache_write_1h_tokens",
            "tokens",
            "Input tokens written to the 1-hour cache.",
        ),
        num_opt(
            "reasoning_tokens",
            "tokens",
            "Reasoning tokens, when the harness reported them separately.",
        ),
        num(
            "web_search_requests",
            "requests",
            "Billed web-search requests.",
        ),
        label_opt(
            "speed",
            "Speed mode the harness reported; it can change the rate.",
        ),
        label_opt(
            "service_tier",
            "Service tier the harness reported; it can change the rate.",
        ),
        num(
            "prompt_size_tokens",
            "tokens",
            "Prompt size that selects a context tier.",
        ),
        flag(
            "split_unknown",
            "True when the cache-write TTL split was not reported.",
        ),
        en(
            "usage_owner",
            USAGE_OWNER,
            "Spend sums only `exclusive` usage; `copied` repeats it.",
        ),
        num(
            "context_tokens",
            "tokens",
            "Uncached plus cached input of the call.",
        ),
        en("context_band", CONTEXT_BAND, "Band of context_tokens."),
        num_opt(
            "model_ms",
            "ms",
            "Model time; null when the harness does not report it.",
        ),
        num_opt(
            "tool_ms",
            "ms",
            "Time in the tool calls this step requested.",
        ),
        real_opt(
            "usd_est",
            "usd",
            "Estimate under price_version; null when unpriced.",
        ),
        real_opt(
            "input_usd",
            "usd",
            "Recorded uncached-input cost component.",
        ),
        real_opt(
            "cache_read_usd",
            "usd",
            "Recorded cache-read cost component.",
        ),
        real_opt(
            "cache_write_usd",
            "usd",
            "Recorded cache-write cost component.",
        ),
        real_opt("output_usd", "usd", "Recorded output cost component."),
        en(
            "price_status",
            PRICE_STATUS,
            "`unpriced` is never summed as zero.",
        ),
        key("price_version", "Price version used for usd_est."),
        en(
            "activity",
            ACTIVITY,
            "Deterministic activity label, or unknown when rule evidence is absent.",
        ),
        en_opt(
            "waste_kind",
            WASTE_KIND,
            "Waste label, when the step is waste.",
        ),
        en_opt(
            "trigger",
            TRIGGER,
            "What triggered a failure or waste step.",
        ),
        flag_opt(
            "ok",
            "False when the step's main action failed; null when unknown.",
        ),
    ],
};

pub static FACT_TOOL_CALL: Table = Table {
    name: "fact_tool_call",
    grain: "One tool call.",
    kind: Kind::Session,
    sort_time: Some("ts"),
    columns: &[
        key("session_uid", "Session UID."),
        key("step_uid", "Step UID of the requesting step."),
        num("ord", "", "Position of the call within its step."),
        ts_opt("ts", "Start of the call, when the harness recorded one."),
        key("tool_key", "Key into dim_tool."),
        path_opt("target_path", "Path the call targets, when it has one."),
        label_opt("command_tag", "Normalised command family of a shell call."),
        flag_opt("ok", "False when the call failed; null when unrecorded."),
        num_opt("exit_code", "", "Process exit code, when the call has one."),
        num_opt("tool_ms", "ms", "Duration of the call."),
    ],
};

pub static FACT_TURN: Table = Table {
    name: "fact_turn",
    grain: "One turn.",
    kind: Kind::Session,
    sort_time: Some("started_at"),
    columns: &[
        key("turn_uid", "Turn UID."),
        key("session_uid", "Session UID."),
        en("initiator", INITIATOR, "Who started the turn."),
        ts_opt("started_at", "Start of the turn, when known."),
        ts_opt("ended_at", "End of the turn; null while it runs."),
        num("step_count", "steps", "Steps in the turn."),
    ],
};

pub static FACT_SESSION: Table = Table {
    name: "fact_session",
    grain: "One session (aggregate grain).",
    kind: Kind::Session,
    sort_time: Some("started_at"),
    columns: &[
        key("session_uid", "Session UID."),
        int_key_opt("date_key", "UTC start date as yyyymmdd; key into dim_date."),
        ts_opt("started_at", "Start of the session, when known."),
        ts_opt("ended_at", "End of the session; null while it runs."),
        num(
            "input_tokens",
            "tokens",
            "Uncached input tokens over exclusive usage.",
        ),
        num(
            "output_tokens",
            "tokens",
            "Output tokens over exclusive usage.",
        ),
        num(
            "cache_read_tokens",
            "tokens",
            "Cache-read tokens over exclusive usage.",
        ),
        num(
            "cache_write_5m_tokens",
            "tokens",
            "5-minute cache-write tokens, exclusive usage.",
        ),
        num(
            "cache_write_1h_tokens",
            "tokens",
            "1-hour cache-write tokens, exclusive usage.",
        ),
        num_opt(
            "reasoning_tokens",
            "tokens",
            "Reasoning tokens over exclusive usage, when complete.",
        ),
        num(
            "web_search_requests",
            "requests",
            "Web-search requests over exclusive usage.",
        ),
        real_opt("usd_est", "usd", "Estimate over priced exclusive steps."),
        real_opt(
            "reported_usd",
            "usd",
            "Spend the harness reported itself, when it does.",
        ),
        num_opt(
            "wall_ms",
            "ms",
            "Wall-clock time when both bounds are known.",
        ),
        num_opt(
            "model_ms",
            "ms",
            "Summed model time when every step reports it.",
        ),
        num("step_count", "steps", "Steps in the session."),
        num("failure_count", "steps", "Steps with ok = false."),
        flag("usage_complete", "True when every step reported its usage."),
        en(
            "source_status",
            SOURCE_STATUS,
            "Source cohort status at publication.",
        ),
        en(
            "computation_status",
            COMPUTATION_STATUS,
            "Recorded computation status at publication.",
        ),
        num(
            "exclusive_step_count",
            "steps",
            "Exclusive steps represented by this row.",
        ),
        num(
            "unknown_step_count",
            "steps",
            "Steps with unknown recorded values.",
        ),
        num(
            "unclassified_step_count",
            "steps",
            "Steps with no deterministic activity label.",
        ),
        num_opt(
            "taxonomy_version",
            "",
            "Taxonomy version when categorical labels were admitted.",
        ),
        num(
            "transform_version",
            "",
            "Transform version that produced the row.",
        ),
        num("rules_version", "", "Rules version that produced the row."),
        key("price_version", "Price version that produced the row."),
    ],
};

pub static FACT_COMPACTION: Table = Table {
    name: "fact_compaction",
    grain: "One compaction.",
    kind: Kind::Session,
    sort_time: Some("ts"),
    columns: &[
        key("compaction_uid", "Compaction UID."),
        key("session_uid", "Session UID."),
        key_opt("step_uid", "Step the compaction followed, when known."),
        ts_opt(
            "ts",
            "Time of the compaction, when the harness recorded it.",
        ),
    ],
};

pub static FACT_LABEL: Table = Table {
    name: "fact_label",
    grain: "One label assertion.",
    kind: Kind::Append,
    sort_time: Some("created_at"),
    columns: &[
        key("label_id", "Durable label ID."),
        en("target_type", TARGET_TYPE, "Kind of the labelled target."),
        key("target_uid", "UID of the labelled target."),
        label("field", "Labelled field."),
        label("value", "Asserted value from the field's closed taxonomy."),
        label("labeller", "`rule:x`, `model:y` or `human:z`."),
        num("taxonomy_version", "", "Taxonomy version of the field."),
        real_opt("confidence", "", "Labeller confidence from 0 to 1."),
        ts("created_at", "Time of the assertion."),
        num("revision", "", "Durable revision that wrote the label."),
    ],
};

pub static DIM_MACHINE: Table = Table {
    name: "dim_machine",
    grain: "One machine namespace.",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        key("machine_key", "Namespace; the machine's configured name."),
        label("display_name", "Display name."),
    ],
};

pub static DIM_HARNESS: Table = Table {
    name: "dim_harness",
    grain: "One harness.",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        key("harness_key", "Harness identifier."),
        label("display_name", "Display name."),
    ],
};

pub static DIM_TOOL: Table = Table {
    name: "dim_tool",
    grain: "One tool name.",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        key("tool_key", "Tool name as the harness reports it."),
        label("display_name", "Display name."),
    ],
};

pub static DIM_DATE: Table = Table {
    name: "dim_date",
    grain: "One UTC calendar day.",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        int_key("date_key", "yyyymmdd."),
        ts("day_start", "Start of the day."),
        num("year", "", "Calendar year."),
        num("month", "", "Month, 1 to 12."),
        num("day", "", "Day of the month."),
        num("iso_week", "", "ISO week number."),
        num("weekday", "", "ISO weekday, 1 (Monday) to 7."),
    ],
};

pub static DIM_SESSION: Table = Table {
    name: "dim_session",
    grain: "One session.",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        key("session_uid", "Session UID."),
        key("machine_key", "Key into dim_machine."),
        key("harness_key", "Key into dim_harness."),
        en(
            "role",
            ROLE,
            "Role from native lineage first; unknown is never root.",
        ),
        key_opt("parent_session_uid", "Parent session UID."),
        en("parent_method", PARENT_METHOD, "How the parent was found."),
        key_opt("lane_root_uid", "Session UID of the lane's root."),
        key(
            "repo_key",
            "Key into dim_repo; never a path or a hash of one.",
        ),
        label_opt("branch", "Branch at the start of the session."),
        en("status", STATUS, "Session status."),
        ts_opt("started_at", "Start of the session, when known."),
        ts_opt("ended_at", "End of the session; null while it runs."),
    ],
};

pub static DIM_REPO: Table = Table {
    name: "dim_repo",
    grain: "One repository identity.",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        key(
            "repo_key",
            "Join key: the repo_id of a remote, a random local ID, or `unknown`.",
        ),
        label_opt(
            "repo_id",
            "Sanitised remote repo_id; null for a local repository.",
        ),
        path_opt(
            "repo_path",
            "Local repository path; null for a remote and for the shared `unknown` key.",
        ),
        label("display_name", "Display name."),
    ],
};

pub static DIM_MODEL: Table = Table {
    name: "dim_model",
    grain: "One immutable model identity.",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        key("model_key", "Model identifier as the harness reports it."),
        label("model", "Model name."),
        label_opt("provider", "Provider name."),
    ],
};

pub static DIM_PRICE: Table = Table {
    name: "dim_price",
    grain: "One rate of one price version (SCD2).",
    kind: Kind::Dimension,
    sort_time: None,
    columns: &[
        key("price_version", "Price version."),
        label("provider", "Provider name."),
        label("model", "Model name."),
        en("rate_kind", RATE_KIND, "Billing input the rate applies to."),
        label_opt("tier", "Pricing tier, when the rate is tiered."),
        label_opt("speed", "Speed mode the rate applies to; null for all."),
        label_opt(
            "service_tier",
            "Service tier the rate applies to; null for all.",
        ),
        num_opt(
            "threshold_tokens",
            "tokens",
            "Context size above which the tier applies.",
        ),
        en(
            "unit",
            PRICE_UNIT,
            "What usd_per_unit prices: a million tokens or one request.",
        ),
        real("usd_per_unit", "usd", "Price per `unit`."),
        ts("valid_from", "Start of validity."),
        ts_opt("valid_to", "End of validity; null while current."),
        ts("learned_at", "When Semon learned the rate."),
    ],
};

/// Every analytics table, facts first.
pub static TABLES: [&Table; 14] = [
    &FACT_STEP,
    &FACT_TOOL_CALL,
    &FACT_TURN,
    &FACT_SESSION,
    &FACT_COMPACTION,
    &FACT_LABEL,
    &DIM_MACHINE,
    &DIM_HARNESS,
    &DIM_TOOL,
    &DIM_DATE,
    &DIM_SESSION,
    &DIM_REPO,
    &DIM_MODEL,
    &DIM_PRICE,
];

pub fn table(name: &str) -> Option<&'static Table> {
    TABLES.iter().copied().find(|table| table.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_names_are_unique_and_complete() {
        let names: HashSet<_> = TABLES.iter().map(|table| table.name).collect();
        assert_eq!(names.len(), TABLES.len());
        for expected in [
            "fact_step",
            "fact_tool_call",
            "fact_turn",
            "fact_session",
            "fact_compaction",
            "fact_label",
            "dim_machine",
            "dim_harness",
            "dim_tool",
            "dim_date",
            "dim_session",
            "dim_repo",
            "dim_model",
            "dim_price",
        ] {
            assert!(names.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn column_names_are_unique_and_documented() {
        for table in TABLES {
            assert!(!table.grain.is_empty(), "{} has no grain", table.name);
            let mut seen = HashSet::new();
            for column in table.columns {
                assert!(
                    seen.insert(column.name),
                    "{}.{} is declared twice",
                    table.name,
                    column.name
                );
                assert_ne!(column.name, GEN_COLUMN, "{} declares _gen", table.name);
                assert!(
                    !column.doc.is_empty(),
                    "{}.{} has no doc",
                    table.name,
                    column.name
                );
            }
        }
    }

    #[test]
    fn classes_match_their_columns() {
        for table in TABLES {
            for column in table.columns {
                let id = format!("{}.{}", table.name, column.name);
                assert_ne!(
                    column.class,
                    Class::Body,
                    "{id}: analytics has no body column"
                );
                let is_enum = column.class == Class::Enum;
                if is_enum && column.data_type == Type::Text {
                    assert!(!column.values.is_empty(), "{id}: enum without values");
                } else {
                    assert!(
                        column.values.is_empty(),
                        "{id}: values on a non-enum text column"
                    );
                }
                match column.class {
                    Class::Time => assert_eq!(column.data_type, Type::Timestamp, "{id}"),
                    Class::Number => assert!(
                        matches!(column.data_type, Type::Integer | Type::Float),
                        "{id}"
                    ),
                    _ => assert_ne!(column.data_type, Type::Timestamp, "{id}"),
                }
                if column.class != Class::Number {
                    assert!(column.unit.is_empty(), "{id}: unit on a non-number");
                }
                let mut values = HashSet::new();
                for value in column.values {
                    assert!(values.insert(*value), "{id}: duplicate value {value}");
                }
            }
        }
    }

    #[test]
    fn kinds_have_their_columns() {
        for table in TABLES {
            let session = table.column(SESSION_COLUMN);
            if table.kind == Kind::Session {
                let session = session.unwrap_or_else(|| panic!("{} lacks session_uid", table.name));
                assert_eq!(session.class, Class::Key);
                assert!(!session.nullable);
            }
            match table.sort_time {
                Some(name) => {
                    let column = table.column(name).expect("sort column exists");
                    assert_eq!(column.class, Class::Time, "{}", table.name);
                }
                None => assert_eq!(table.kind, Kind::Dimension, "{}", table.name),
            }
        }
    }

    #[test]
    fn arrow_schemas_follow_the_declaration() {
        for table in TABLES {
            let declared = table.arrow_schema();
            assert_eq!(declared.fields().len(), table.columns.len());
            let file = table.file_schema();
            assert_eq!(file.fields().len(), table.columns.len() + 1);
            assert_eq!(file.field(table.columns.len()).name(), GEN_COLUMN);
            for (column, field) in table.columns.iter().zip(declared.fields()) {
                assert_eq!(column.name, field.name());
                assert_eq!(column.nullable, field.is_nullable());
                assert_eq!(column.data_type.arrow(), *field.data_type());
            }
        }
    }

    #[test]
    fn billing_inputs_are_kept_apart() {
        let step = table("fact_step").unwrap();
        for name in [
            "cache_write_5m_tokens",
            "cache_write_1h_tokens",
            "web_search_requests",
            "speed",
            "service_tier",
            "prompt_size_tokens",
            "split_unknown",
        ] {
            assert!(step.column(name).is_some(), "fact_step lacks {name}");
        }
        assert!(step.column("cache_write_tokens").is_none());
        let price = table("dim_price").unwrap();
        let rates = price.column("rate_kind").unwrap().values;
        for kind in ["cache_write_5m", "cache_write_1h", "web_search_request"] {
            assert!(rates.contains(&kind), "dim_price lacks rate kind {kind}");
        }
        assert_eq!(price.column("unit").unwrap().values, ["mtok", "request"]);
    }

    #[test]
    fn local_paths_are_never_keys() {
        let repo = table("dim_repo").unwrap();
        assert_eq!(repo.column("repo_path").unwrap().class, Class::Path);
        assert!(repo.column("repo_path").unwrap().nullable);
        assert_eq!(repo.column("repo_key").unwrap().class, Class::Key);
        for table in TABLES {
            for column in table.columns {
                if column.name.contains("path") {
                    assert_eq!(column.class, Class::Path, "{}.{}", table.name, column.name);
                }
            }
        }
    }

    #[test]
    fn lookup_finds_tables() {
        assert_eq!(table("fact_step").map(|t| t.name), Some("fact_step"));
        assert!(table("fact_nothing").is_none());
    }
}
