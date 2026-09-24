use std::{
    env,
    fs::OpenOptions,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    str::FromStr,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use semon_store::{
    ForgetSelector, LogFilter, OccurrenceSelector, REPLICATION_ENDPOINT_ENV, TraceId, TraceStore,
    day_bounds_ns, render_occurrence_line, ship,
};

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("semon: {error}");
            ExitCode::FAILURE
        }
    }
}

enum Command {
    Ship(ShipArgs),
    Log(LogArgs),
    Forensic(ForensicArgs),
    Forget(ForgetArgs),
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

/// Arguments for `semon forensic`. Exactly one of `trace`, `session`, `day`
/// is `Some` by the time parsing succeeds — enforced in
/// [`parse_forensic_args`], not left to `run_forensic` to discover.
struct ForensicArgs {
    store: PathBuf,
    trace: Option<String>,
    session: Option<String>,
    day: Option<String>,
    out: Option<PathBuf>,
}

/// Arguments for `semon forget --forensic`. Exactly one of `trace`,
/// `session`, `before` is `Some` by the time parsing succeeds — enforced in
/// [`parse_forget_args`], not left to `run_forget` to discover.
///
/// There is no `forensic` field: `--forensic` is a required flag (see issue
/// #20 — required so the command reads as what it does in a shell history,
/// and so other targets can be added later without a breaking change), but
/// once [`parse_forget_args`] has confirmed it was given, its value carries
/// no further information for `run_forget` to act on.
#[derive(Debug)]
struct ForgetArgs {
    store: PathBuf,
    trace: Option<String>,
    session: Option<String>,
    before: Option<String>,
    yes: bool,
}

fn parse_args() -> Result<Command, String> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("ship") => parse_ship_args(arguments).map(Command::Ship),
        Some("log") => parse_log_args(arguments).map(Command::Log),
        Some("forensic") => parse_forensic_args(arguments).map(Command::Forensic),
        Some("forget") => parse_forget_args(arguments).map(Command::Forget),
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

fn parse_forensic_args(
    mut arguments: impl Iterator<Item = String>,
) -> Result<ForensicArgs, String> {
    let mut store = default_store_path();
    let mut trace = None;
    let mut session = None;
    let mut day = None;
    let mut out = None;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--store" => store = value()?.into(),
            "--trace" => trace = Some(value()?),
            "--session" => session = Some(value()?),
            "--day" => day = Some(value()?),
            "--out" => out = Some(value()?.into()),
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }

    let selected = [trace.is_some(), session.is_some(), day.is_some()]
        .into_iter()
        .filter(|is_set| *is_set)
        .count();
    if selected != 1 {
        return Err(format!(
            "forensic: exactly one of --trace, --session, or --day is required (got {selected})\n\
             {}",
            usage()
        ));
    }

    Ok(ForensicArgs {
        store,
        trace,
        session,
        day,
        out,
    })
}

fn parse_forget_args(mut arguments: impl Iterator<Item = String>) -> Result<ForgetArgs, String> {
    let mut store = default_store_path();
    let mut forensic = false;
    let mut trace = None;
    let mut session = None;
    let mut before = None;
    let mut yes = false;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--store" => store = value()?.into(),
            "--forensic" => forensic = true,
            "--trace" => trace = Some(value()?),
            "--session" => session = Some(value()?),
            "--before" => before = Some(value()?),
            "--yes" => yes = true,
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }

    // Required as a flag even though it is currently the only target: a
    // reader of a shell history should be able to see what this command
    // does, and this leaves room for other forget targets later without a
    // breaking change (issue #20).
    if !forensic {
        return Err(format!("forget: --forensic is required\n{}", usage()));
    }

    let selected = [trace.is_some(), session.is_some(), before.is_some()]
        .into_iter()
        .filter(|is_set| *is_set)
        .count();
    if selected != 1 {
        return Err(format!(
            "forget: exactly one of --trace, --session, or --before is required (got {selected}); \
             this is the first destructive, irreversible command in the tool, and a bare \
             invocation must not silently empty the forensic region\n\
             {}",
            usage()
        ));
    }

    Ok(ForgetArgs {
        store,
        trace,
        session,
        before,
        yes,
    })
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Ship(args) => run_ship(args).map(|message| println!("{message}")),
        Command::Log(args) => run_log(args).map(|message| println!("{message}")),
        Command::Forensic(args) => run_forensic(args),
        Command::Forget(args) => run_forget(args),
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

/// The one-line stderr warning `semon forensic` writes before any output —
/// on every invocation, before touching stdout or `--out`, so redirecting
/// stdout to a file still shows it (see
/// `docs/design/forensic-retention-and-exposure.md`, Decision 3).
const FORENSIC_WARNING: &str = "semon forensic: raw output may contain prompts, responses, \
     source code, credentials, and machine paths captured verbatim.";

const TRACE_SELECTOR_NOTE: &str = "--trace selects complete source lines linked to the trace; \
     unprojected raw records have no trace links and must be selected by session or time.";

/// The only CLI path that reads `raw_carrier_records`, directly or via
/// [`semon_store::TraceStore::fetch_raw_carrier_records_for_occurrences`].
/// `run_log` and `run_ship` must never gain such a call — behaviorally
/// enforced (drop the table, and only the raw-reading calls this delegates
/// to may fail) by `semon-store`'s own
/// `raw_reads_fail_but_log_and_canonical_reads_survive_dropping_the_raw_region`;
/// see the comment on this file's (test-only) trailing note for why that
/// check has to live at the store layer rather than here.
fn run_forensic(args: ForensicArgs) -> Result<(), String> {
    eprintln!("{FORENSIC_WARNING}");

    let store = TraceStore::open(&args.store).map_err(|error| error.to_string())?;
    let records = if let Some(trace) = args.trace.as_deref() {
        eprintln!("semon forensic: {TRACE_SELECTOR_NOTE}");
        let trace_id = TraceId::from_str(trace).map_err(|error| error.to_string())?;
        store
            .fetch_raw_carrier_records(&trace_id)
            .map_err(|error| error.to_string())?
    } else if let Some(session) = args.session.as_deref() {
        store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session(session))
            .map_err(|error| error.to_string())?
    } else if let Some(day) = args.day.as_deref() {
        let (start, end) = parse_day(day)?;
        store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::TimestampRange(
                start, end,
            ))
            .map_err(|error| error.to_string())?
    } else {
        // Unreachable: parse_forensic_args requires exactly one selector.
        return Err("no selector given".to_owned());
    };

    write_forensic_records(&records, args.out.as_deref())
}

/// Writes one raw record's verbatim bytes per line, to `out` (created
/// `0600`) when given, otherwise to stdout — bulk selection to stdout must
/// work without `--out` (see the policy doc, Decision 3: `--out` is an
/// option, not a requirement).
fn write_forensic_records(
    records: &[semon_store::RawCarrierRecord],
    out: Option<&Path>,
) -> Result<(), String> {
    let mut writer: Box<dyn Write> = match out {
        Some(path) => {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            options.mode(0o600);
            let file = options.open(path).map_err(|error| error.to_string())?;
            // `mode()` only governs permissions at creation; re-assert them
            // in case `path` already existed with looser ones, for the same
            // reason the store file re-asserts on open rather than trusting
            // what it finds.
            #[cfg(unix)]
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
            Box::new(file)
        }
        None => Box::new(io::stdout()),
    };

    for record in records {
        let bytes = record.bytes();
        writer.write_all(bytes).map_err(|error| error.to_string())?;
        // Captured lines already end in the newline `read_until(b'\n')` kept
        // at capture time; only add one when the stored bytes lack it, so
        // "one raw record per line" doesn't become one record plus one blank
        // line — which it did before this fix, doubling every line count.
        if !bytes.ends_with(b"\n") {
            writer.write_all(b"\n").map_err(|error| error.to_string())?;
        }
    }
    writer.flush().map_err(|error| error.to_string())?;
    Ok(())
}

/// Permanently deletes forensic (`raw_carrier_records`) rows matching one
/// selector. `canonical_traces` and `occurrences` are never touched, so
/// `semon log` is unaffected by this command (see
/// `docs/design/trace-identity-and-occurrences.md`).
///
/// This is the first destructive, irreversible command in the tool, so it
/// carries two guards beyond `parse_forget_args`'s mandatory-selector check:
/// without `--yes` it prompts interactively, stating the exact row count and
/// that the action cannot be undone, and if stdin is not a TTY and `--yes`
/// is absent it errors instead of silently proceeding non-interactively.
///
/// `--session`/`--before` select each raw row's own session/timestamp. A raw
/// row written before the session/sequence link existed has neither value
/// after migration. Rather than let such a row silently sit outside what
/// those selectors can reach, this reports how many exist and that only
/// `--trace` reaches them. Conversely, an unprojected row has no trace link,
/// so a trace selector cannot reach it and the command reports that limit.
fn run_forget(args: ForgetArgs) -> Result<(), String> {
    let mut store = TraceStore::open(&args.store).map_err(|error| error.to_string())?;

    // Declared outside the branch below so `ForgetSelector::Trace`'s
    // borrow can outlive the `if`/`else if` that constructs it.
    let trace_id;
    let selector = if let Some(trace) = args.trace.as_deref() {
        trace_id = TraceId::from_str(trace).map_err(|error| error.to_string())?;
        ForgetSelector::Trace(&trace_id)
    } else if let Some(session) = args.session.as_deref() {
        ForgetSelector::Session(session)
    } else if let Some(before) = args.before.as_deref() {
        let (start, _end) = parse_day(before)?;
        ForgetSelector::Before(start)
    } else {
        // Unreachable: parse_forget_args requires exactly one selector.
        return Err("no selector given".to_owned());
    };

    if matches!(selector, ForgetSelector::Trace(_)) {
        eprintln!("forget --forensic: {TRACE_SELECTOR_NOTE}");
        eprintln!(
            "forget --forensic: deleting a linked trace removes its whole raw source line; other projected traces and occurrences remain."
        );
    }

    if !matches!(selector, ForgetSelector::Trace(_)) {
        let unlinked = store
            .count_unlinked_raw_records()
            .map_err(|error| error.to_string())?;
        if unlinked > 0 {
            eprintln!(
                "forget --forensic: {unlinked} raw record(s) predate this store's \
                 session/sequence link and cannot be reached by --session or --before; \
                 use --trace to remove them individually."
            );
        }
    }

    let count = store
        .count_forensic_forget(selector)
        .map_err(|error| error.to_string())?;
    if count == 0 {
        println!("forget --forensic: no matching raw records; nothing to do");
        return Ok(());
    }

    if !args.yes {
        if !io::stdin().is_terminal() {
            return Err(
                "forget --forensic: stdin is not a terminal and --yes was not given; \
                 refusing to delete forensic data non-interactively without explicit consent"
                    .to_owned(),
            );
        }

        eprint!(
            "forget --forensic: this will permanently delete {count} raw record(s) from \
             raw_carrier_records. This cannot be undone. canonical_traces and occurrences \
             (the log) are not affected. Proceed? [y/N] "
        );
        io::stderr().flush().map_err(|error| error.to_string())?;

        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        let answer = answer.trim().to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            println!("forget --forensic: aborted; no records deleted");
            return Ok(());
        }
    }

    let deleted = store
        .forget_forensic(selector)
        .map_err(|error| error.to_string())?;
    println!("forget --forensic: permanently deleted {deleted} raw record(s)");
    Ok(())
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
         Renders the occurrence log. Never reads raw_carrier_records.\n\
         \n\
         Usage: semon forensic [--store PATH] (--trace ID | --session ID | --day YYYY-MM-DD) [--out FILE]\n\
         Reads raw_carrier_records — the only command that does. Exactly one\n\
         of --trace, --session, --day is required. Without --out, writes to\n\
         stdout; with it, writes to FILE created 0600 instead.\n\
         \n\
         Usage: semon forget --forensic [--store PATH] (--before YYYY-MM-DD | --session ID | --trace ID) [--yes]\n\
         Permanently deletes matching rows from raw_carrier_records only;\n\
         canonical_traces and occurrences (the log) are never touched.\n\
         Irreversible. --forensic is required. Exactly one selector is\n\
         required — a bare invocation is refused rather than deleting\n\
         everything. Without --yes, prompts interactively and errors if\n\
         stdin is not a terminal.\n\
         --session and --before match a raw record's own session/timestamp.\n\
         --trace matches projected content instead: it removes every raw\n\
         source line linked to that trace, including bytes for other traces\n\
         on those lines. Their canonical traces and occurrences remain.\n\
         Unprojected raw records have no trace links, so --trace cannot select them."
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

    fn unique_temp_path(label: &str, extension: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "semon-cli-test-{label}-{}-{unique}.{extension}",
            std::process::id()
        ))
    }

    #[test]
    fn forensic_rejects_zero_or_multiple_selectors() {
        let zero = parse_forensic_args(std::iter::empty());
        assert!(zero.is_err(), "zero selectors must be rejected");

        let two = parse_forensic_args(
            [
                "--trace".to_owned(),
                "a".repeat(64),
                "--session".to_owned(),
                "session-a".to_owned(),
            ]
            .into_iter(),
        );
        assert!(two.is_err(), "two selectors must be rejected");

        let one = parse_forensic_args(["--session".to_owned(), "session-a".to_owned()].into_iter());
        assert!(one.is_ok(), "exactly one selector must be accepted");
    }

    /// Captures one trace with a known raw record, returning the store path
    /// and trace id for forensic-command tests.
    fn store_with_one_capture(label: &str, session: &str, timestamp: i64) -> (PathBuf, String) {
        let path = unique_temp_db_path(label);
        let mut store = TraceStore::open(&path).unwrap();
        let core =
            SemanticCore::from_value(json!({"kind": "intent", "content": format!("{label} body")}))
                .unwrap();
        let trace_id = core.trace_id().unwrap().as_str().to_owned();
        store
            .capture(
                &core,
                NewRawCarrierRecord::new("codex", b"raw-forensic-bytes"),
                NewOccurrence {
                    session,
                    sequence: 0,
                    timestamp,
                    repo: "semon",
                    repo_source: RepoSource::GitRemote,
                    parent_sequence: None,
                    agent: None,
                    authored_by: AuthoredBy::Human,
                },
            )
            .unwrap();
        drop(store);
        (path, trace_id)
    }

    #[test]
    fn forensic_by_trace_writes_verbatim_bytes_to_an_out_file_created_0600() {
        let (store_path, trace_id) = store_with_one_capture("forensic-trace", "session-a", 0);
        let out_path = unique_temp_path("forensic-trace", "out");

        run_forensic(ForensicArgs {
            store: store_path.clone(),
            trace: Some(trace_id),
            session: None,
            day: None,
            out: Some(out_path.clone()),
        })
        .unwrap();

        let contents = std::fs::read(&out_path).unwrap();
        assert_eq!(contents, b"raw-forensic-bytes\n");

        #[cfg(unix)]
        {
            let mode = std::fs::metadata(&out_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "--out file must be created 0600, got {mode:o}");
        }

        let _ = std::fs::remove_file(&store_path);
        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn forensic_by_session_selects_only_that_sessions_records() {
        let (store_path, _) = store_with_one_capture("forensic-session-a", "session-a", 0);
        {
            // Add a second capture, under a different session, into the same store.
            let mut store = TraceStore::open(&store_path).unwrap();
            let core =
                SemanticCore::from_value(json!({"kind": "intent", "content": "other"})).unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"other-session-bytes"),
                    NewOccurrence {
                        session: "session-b",
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
        let out_path = unique_temp_path("forensic-session", "out");

        run_forensic(ForensicArgs {
            store: store_path.clone(),
            trace: None,
            session: Some("session-a".to_owned()),
            day: None,
            out: Some(out_path.clone()),
        })
        .unwrap();

        let contents = std::fs::read_to_string(&out_path).unwrap();
        assert!(contents.contains("raw-forensic-bytes"));
        assert!(!contents.contains("other-session-bytes"));

        let _ = std::fs::remove_file(&store_path);
        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn forensic_by_day_selects_only_that_days_records() {
        let (year, month, day) = (2026, 9, 18);
        let (start, _) = day_bounds_ns(year, month, day);
        let (store_path, _) = store_with_one_capture("forensic-day-in", "session-a", start + 1);
        {
            let mut store = TraceStore::open(&store_path).unwrap();
            let core =
                SemanticCore::from_value(json!({"kind": "intent", "content": "next day"})).unwrap();
            let (next_start, _) = day_bounds_ns(year, month, day + 1);
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"next-day-bytes"),
                    NewOccurrence {
                        session: "session-a",
                        sequence: 1,
                        timestamp: next_start + 1,
                        repo: "semon",
                        repo_source: RepoSource::GitRemote,
                        parent_sequence: Some(0),
                        agent: None,
                        authored_by: AuthoredBy::Human,
                    },
                )
                .unwrap();
        }
        let out_path = unique_temp_path("forensic-day", "out");

        run_forensic(ForensicArgs {
            store: store_path.clone(),
            trace: None,
            session: None,
            day: Some(format!("{year:04}-{month:02}-{day:02}")),
            out: Some(out_path.clone()),
        })
        .unwrap();

        let contents = std::fs::read_to_string(&out_path).unwrap();
        assert!(contents.contains("raw-forensic-bytes"));
        assert!(!contents.contains("next-day-bytes"));

        let _ = std::fs::remove_file(&store_path);
        let _ = std::fs::remove_file(&out_path);
    }

    // The behavioral mirror of `log_renders_identically_after_the_raw_region_is_dropped`
    // — that `run_log`/`run_ship` never depend on `raw_carrier_records`
    // while `run_forensic` fails hard once it's gone — lives in
    // `semon-store`'s own tests as
    // `raw_reads_fail_but_log_and_canonical_reads_survive_dropping_the_raw_region`,
    // not here. `run_log`, `run_ship`, and `run_forensic` are thin wrappers
    // over `TraceStore::log`, `ship`, and
    // `TraceStore::fetch_raw_carrier_records[_for_occurrences]` respectively
    // (each opens the store, delegates to exactly one of those, and maps
    // the error) — so a test pinning which of *those* touch raw already
    // pins this boundary. It has to live there and not here for a concrete
    // reason: proving a `DROP TABLE` sticks requires never reopening the
    // store afterward, and every one of these CLI entry points calls
    // `TraceStore::open`, which unconditionally re-runs the additive schema
    // (`CREATE TABLE IF NOT EXISTS raw_carrier_records ...`) on every open —
    // silently recreating an externally-dropped table, empty, the moment
    // *any* command (including `run_forensic` itself) next opens the store.
    // A CLI-process-level version of this test — drop the table, then call
    // `run_log`/`run_ship`/`run_forensic` as if they were separate `semon`
    // invocations against the same file — was tried and does not fail as
    // expected: the first such call's own `TraceStore::open` heals the
    // table before its query runs, so `run_forensic` observes an empty
    // table rather than a missing one and returns an empty success instead
    // of an error.

    #[test]
    fn forget_rejects_missing_forensic_flag() {
        let error =
            parse_forget_args(["--trace".to_owned(), "a".repeat(64)].into_iter()).unwrap_err();
        assert!(error.contains("--forensic is required"), "{error}");
    }

    #[test]
    fn forget_rejects_zero_or_multiple_selectors() {
        let zero = parse_forget_args(["--forensic".to_owned()].into_iter());
        assert!(zero.is_err(), "zero selectors must be rejected");

        let two = parse_forget_args(
            [
                "--forensic".to_owned(),
                "--trace".to_owned(),
                "a".repeat(64),
                "--session".to_owned(),
                "session-a".to_owned(),
            ]
            .into_iter(),
        );
        assert!(two.is_err(), "two selectors must be rejected");

        let one = parse_forget_args(
            [
                "--forensic".to_owned(),
                "--session".to_owned(),
                "session-a".to_owned(),
            ]
            .into_iter(),
        );
        assert!(one.is_ok(), "exactly one selector must be accepted");
    }

    #[test]
    fn forget_by_trace_deletes_only_that_trace_and_leaves_others_intact() {
        let (store_path, target_trace_id) =
            store_with_one_capture("forget-trace-target", "session-a", 0);
        let other_trace_id;
        {
            let mut store = TraceStore::open(&store_path).unwrap();
            let core =
                SemanticCore::from_value(json!({"kind": "intent", "content": "other trace"}))
                    .unwrap();
            other_trace_id = core.trace_id().unwrap().as_str().to_owned();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"other-trace-bytes"),
                    NewOccurrence {
                        session: "session-b",
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

        run_forget(ForgetArgs {
            store: store_path.clone(),
            trace: Some(target_trace_id.clone()),
            session: None,
            before: None,
            yes: true,
        })
        .unwrap();

        let store = TraceStore::open(&store_path).unwrap();
        let target_id = TraceId::from_str(&target_trace_id).unwrap();
        let other_id = TraceId::from_str(&other_trace_id).unwrap();
        assert!(
            store
                .fetch_raw_carrier_records(&target_id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.fetch_raw_carrier_records(&other_id).unwrap().len(),
            1,
            "the other trace's raw record must survive"
        );
        // The log is unaffected: forget never touches occurrences or
        // canonical_traces.
        assert_eq!(store.log(&LogFilter::default()).unwrap().len(), 2);

        let _ = std::fs::remove_file(&store_path);
    }

    #[test]
    fn forget_by_session_leaves_a_shared_traces_other_session_intact() {
        // Shaped around the failure this had: forgetting one session used
        // to widen to every raw row sharing a trace_id, so a trace shared
        // between two sessions lost *both* sessions' raw rows when only one
        // was named. The fixture below shares one trace across two
        // sessions specifically to catch that.
        let store_path = unique_temp_db_path("forget-session-shared-trace");
        let shared = json!({"kind": "intent", "content": "captured in both sessions"});
        {
            let mut store = TraceStore::open(&store_path).unwrap();
            let core = SemanticCore::from_value(shared).unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"session-a-bytes"),
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
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"session-b-bytes"),
                    NewOccurrence {
                        session: "session-b",
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

        run_forget(ForgetArgs {
            store: store_path.clone(),
            trace: None,
            session: Some("session-a".to_owned()),
            before: None,
            yes: true,
        })
        .unwrap();

        let store = TraceStore::open(&store_path).unwrap();
        let remaining = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-b"))
            .unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "session-b's copy of the shared trace must survive"
        );
        assert_eq!(remaining[0].bytes(), b"session-b-bytes");
        let forgotten = store
            .fetch_raw_carrier_records_for_occurrences(OccurrenceSelector::Session("session-a"))
            .unwrap();
        assert!(forgotten.is_empty());

        let _ = std::fs::remove_file(&store_path);
    }

    #[test]
    fn forensic_by_session_on_a_shared_trace_returns_only_that_sessions_capture() {
        // The read-side counterpart to the test above: `semon forensic
        // --session` used to widen to every raw row sharing a matched
        // trace's trace_id, over-reading the other session's forensic
        // bytes.
        let store_path = unique_temp_db_path("forensic-session-shared-trace");
        let shared = json!({"kind": "intent", "content": "shared for forensic read"});
        {
            let mut store = TraceStore::open(&store_path).unwrap();
            let core = SemanticCore::from_value(shared).unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"session-a-forensic-bytes"),
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
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"session-b-forensic-bytes"),
                    NewOccurrence {
                        session: "session-b",
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
        let out_path = unique_temp_path("forensic-session-shared", "out");

        run_forensic(ForensicArgs {
            store: store_path.clone(),
            trace: None,
            session: Some("session-a".to_owned()),
            day: None,
            out: Some(out_path.clone()),
        })
        .unwrap();

        let contents = std::fs::read_to_string(&out_path).unwrap();
        assert!(contents.contains("session-a-forensic-bytes"));
        assert!(!contents.contains("session-b-forensic-bytes"));

        let _ = std::fs::remove_file(&store_path);
        let _ = std::fs::remove_file(&out_path);
    }

    #[test]
    fn forget_without_yes_on_non_tty_stdin_errors() {
        let (store_path, target_trace_id) =
            store_with_one_capture("forget-non-tty", "session-a", 0);

        let error = run_forget(ForgetArgs {
            store: store_path.clone(),
            trace: Some(target_trace_id),
            session: None,
            before: None,
            yes: false,
        })
        .unwrap_err();

        assert!(
            error.contains("not a terminal"),
            "expected a non-TTY refusal, got: {error}"
        );

        // And nothing was actually deleted.
        let store = TraceStore::open(&store_path).unwrap();
        assert_eq!(store.log(&LogFilter::default()).unwrap().len(), 1);

        let _ = std::fs::remove_file(&store_path);
    }

    #[test]
    fn forget_with_no_matching_records_is_a_no_op() {
        let (store_path, _) = store_with_one_capture("forget-no-match", "session-a", 0);

        // Selecting a session with no captures at all matches zero raw
        // records, so this must succeed without requiring confirmation
        // (there's a TTY check inside that branch that this path never
        // reaches) and without deleting anything.
        run_forget(ForgetArgs {
            store: store_path.clone(),
            trace: None,
            session: Some("no-such-session".to_owned()),
            before: None,
            yes: false,
        })
        .unwrap();

        let store = TraceStore::open(&store_path).unwrap();
        assert_eq!(store.log(&LogFilter::default()).unwrap().len(), 1);

        let _ = std::fs::remove_file(&store_path);
    }

    #[test]
    fn forget_before_deletes_the_pre_cutoff_capture_of_a_recurring_trace_and_leaves_the_later_one()
    {
        // Shaped around the failure this had: "before" used to require
        // *every* occurrence of a trace to predate the cutoff, so a trace
        // that recurred after the cutoff kept its pre-cutoff raw record too
        // — measured on a real week as 52% of eligible captures surviving
        // while the command reported success. The fixture below captures
        // the *same* trace once before the cutoff and once after, so a
        // per-trace implementation and a per-capture one disagree on it.
        let (year, month, day) = (2026, 9, 18);
        let (start, _) = day_bounds_ns(year, month, day);
        let store_path = unique_temp_db_path("forget-before-recurring-trace");
        let recurring = json!({"kind": "intent", "content": "recurring content"});
        {
            let mut store = TraceStore::open(&store_path).unwrap();
            let core = SemanticCore::from_value(recurring).unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"pre-cutoff-bytes"),
                    NewOccurrence {
                        session: "session-a",
                        sequence: 0,
                        timestamp: start - 1,
                        repo: "semon",
                        repo_source: RepoSource::GitRemote,
                        parent_sequence: None,
                        agent: None,
                        authored_by: AuthoredBy::Human,
                    },
                )
                .unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"post-cutoff-bytes"),
                    NewOccurrence {
                        session: "session-a",
                        sequence: 1,
                        timestamp: start,
                        repo: "semon",
                        repo_source: RepoSource::GitRemote,
                        parent_sequence: Some(0),
                        agent: None,
                        authored_by: AuthoredBy::Human,
                    },
                )
                .unwrap();
        }

        run_forget(ForgetArgs {
            store: store_path.clone(),
            trace: None,
            session: None,
            before: Some(format!("{year:04}-{month:02}-{day:02}")),
            yes: true,
        })
        .unwrap();

        let store = TraceStore::open(&store_path).unwrap();
        let recurring_id =
            SemanticCore::from_value(json!({"kind": "intent", "content": "recurring content"}))
                .unwrap()
                .trace_id()
                .unwrap();
        let remaining = store.fetch_raw_carrier_records(&recurring_id).unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "the post-cutoff capture of the same trace must survive"
        );
        assert_eq!(remaining[0].bytes(), b"post-cutoff-bytes");
        // The log itself is unaffected either way.
        assert_eq!(store.log(&LogFilter::default()).unwrap().len(), 2);

        let _ = std::fs::remove_file(&store_path);
    }
}
