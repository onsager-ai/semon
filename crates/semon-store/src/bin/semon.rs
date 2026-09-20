use std::{env, path::PathBuf, process::ExitCode};

use semon_store::{
    LogFilter, REPLICATION_ENDPOINT_ENV, TraceStore, day_bounds_ns, render_occurrence_line, ship,
};

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("semon: {error}");
            ExitCode::FAILURE
        }
    }
}

enum Command {
    Ship(ShipArgs),
    Log(LogArgs),
}

struct ShipArgs {
    store: PathBuf,
    endpoint: Option<String>,
}

struct LogArgs {
    store: PathBuf,
    repo: Option<String>,
    day: Option<String>,
    limit: Option<u32>,
}

fn parse_args() -> Result<Command, String> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("ship") => parse_ship_args(arguments).map(Command::Ship),
        Some("log") => parse_log_args(arguments).map(Command::Log),
        Some("-h" | "--help") => Err(usage()),
        Some(command) => Err(format!("unknown command: {command}\n{}", usage())),
        None => Err(usage()),
    }
}

fn parse_ship_args(mut arguments: impl Iterator<Item = String>) -> Result<ShipArgs, String> {
    let mut store = default_store_path();
    let mut endpoint = env::var(REPLICATION_ENDPOINT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--store" => store = value()?.into(),
            "--endpoint" => endpoint = Some(value()?),
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(ShipArgs { store, endpoint })
}

fn parse_log_args(mut arguments: impl Iterator<Item = String>) -> Result<LogArgs, String> {
    let mut store = default_store_path();
    let mut repo = None;
    let mut day = None;
    let mut limit = None;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--store" => store = value()?.into(),
            "--repo" => repo = Some(value()?),
            "--day" => day = Some(value()?),
            "--limit" => {
                let raw = value()?;
                limit =
                    Some(raw.parse::<u32>().map_err(|_| {
                        format!("--limit expects a non-negative integer, got {raw}")
                    })?);
            }
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(LogArgs {
        store,
        repo,
        day,
        limit,
    })
}

fn run(command: Command) -> Result<String, String> {
    match command {
        Command::Ship(args) => run_ship(args),
        Command::Log(args) => run_log(args),
    }
}

fn run_ship(args: ShipArgs) -> Result<String, String> {
    let Some(endpoint) = args
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(format!(
            "ship: no endpoint configured ({REPLICATION_ENDPOINT_ENV} unset); skipping"
        ));
    };

    let store = TraceStore::open(&args.store).map_err(|error| error.to_string())?;
    let report = ship(&store, Some(endpoint)).map_err(|error| error.to_string())?;
    Ok(format!(
        "shipped {} trace(s) to {endpoint}",
        report.shipped()
    ))
}

/// Renders the occurrence log for `semon log`.
///
/// This reads only [`TraceStore::log`], which never touches
/// `raw_carrier_records` — the gate from #11. It must stay that way: no path
/// through this function may add a raw-record read, even indirectly.
fn run_log(args: LogArgs) -> Result<String, String> {
    let store = TraceStore::open(&args.store).map_err(|error| error.to_string())?;
    let timestamp_range = args.day.as_deref().map(parse_day).transpose()?;
    let filter = LogFilter {
        repo: args.repo,
        timestamp_range,
        limit: args.limit,
    };
    let rows = store.log(&filter).map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Ok("(no occurrences)".to_owned());
    }
    Ok(rows
        .iter()
        .map(render_occurrence_line)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Parses a `YYYY-MM-DD` UTC calendar date into `[start, end)` nanoseconds
/// since the Unix epoch.
fn parse_day(date: &str) -> Result<(i64, i64), String> {
    let invalid = || format!("--day expects YYYY-MM-DD, got {date}");
    let mut parts = date.splitn(3, '-');
    let year = parts.next().ok_or_else(invalid)?;
    let month = parts.next().ok_or_else(invalid)?;
    let day = parts.next().ok_or_else(invalid)?;
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return Err(invalid());
    }
    let year: i64 = year.parse().map_err(|_| invalid())?;
    let month: u32 = month.parse().map_err(|_| invalid())?;
    let day: u32 = day.parse().map_err(|_| invalid())?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(invalid());
    }
    Ok(day_bounds_ns(year, month, day))
}

fn default_store_path() -> PathBuf {
    if let Some(root) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("semon/traces.sqlite3");
    }
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/semon/traces.sqlite3")
}

fn usage() -> String {
    format!(
        "Usage: semon ship [--store PATH] [--endpoint URL]\n\
         Endpoint defaults to ${REPLICATION_ENDPOINT_ENV}; when unset, ship succeeds without reading the store.\n\
         \n\
         Usage: semon log [--store PATH] [--repo NAME] [--day YYYY-MM-DD] [--limit N]\n\
         Renders the occurrence log. Never reads raw_carrier_records."
    )
}

#[cfg(test)]
mod tests {
    use semon_store::{AuthoredBy, NewOccurrence, NewRawCarrierRecord, RepoSource, SemanticCore};
    use serde_json::json;

    use super::*;

    #[test]
    fn unconfigured_ship_succeeds_without_opening_the_store() {
        let args = ShipArgs {
            store: PathBuf::from("this-store-does-not-exist.sqlite3"),
            endpoint: None,
        };

        let message = run_ship(args).unwrap();

        assert!(message.contains("skipping"));
    }

    fn unique_temp_db_path(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "semon-cli-test-{label}-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    #[test]
    fn log_renders_captured_occurrences_and_never_touches_raw_records() {
        let path = unique_temp_db_path("log-render");
        {
            let mut store = TraceStore::open(&path).unwrap();
            let core =
                SemanticCore::from_value(json!({"kind": "intent", "content": "fix the bug"}))
                    .unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"raw bytes"),
                    NewOccurrence {
                        session: "session-a",
                        sequence: 0,
                        timestamp: 0,
                        repo: "semon",
                        repo_source: RepoSource::GitRemote,
                        parent_sequence: None,
                        agent: None,
                        authored_by: AuthoredBy::Human,
                    },
                )
                .unwrap();
        }

        let message = run_log(LogArgs {
            store: path.clone(),
            repo: None,
            day: None,
            limit: None,
        })
        .unwrap();

        assert!(message.contains("session-a#0"));
        assert!(message.contains("fix the bug"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn log_on_an_empty_store_says_so() {
        let path = unique_temp_db_path("log-empty");
        TraceStore::open(&path).unwrap();

        let message = run_log(LogArgs {
            store: path.clone(),
            repo: None,
            day: None,
            limit: None,
        })
        .unwrap();

        assert_eq!(message, "(no occurrences)");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_day_rejects_malformed_dates() {
        assert!(parse_day("2026-9-19").is_err());
        assert!(parse_day("2026/09/19").is_err());
        assert!(parse_day("not-a-date").is_err());
    }

    #[test]
    fn parse_day_accepts_a_well_formed_date() {
        let (start, end) = parse_day("2026-09-19").unwrap();
        assert_eq!(end - start, 86_400 * 1_000_000_000);
    }
}
