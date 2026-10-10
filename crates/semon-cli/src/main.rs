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
    day_bounds_ns, format_day_ns, render_occurrence_line, ship,
};

mod forensic_export;

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
    Version,
    Ship(ShipArgs),
    Log(LogArgs),
    Forensic(ForensicArgs),
    Forget(ForgetArgs),
    Sessions(SessionsArgs),
    Control(Vec<String>),
    Push(PushArgs),
    Receive(ReceiveArgs),
    Query(QueryArgs),
    Mcp(HomeArgs),
}

/// The agent homes, `/proc`, cache and facts `semon sessions` reads, or
/// several machines' homes.
#[derive(Default)]
struct HomeArgs {
    options: semon_sessions::Options,
    /// `--machine DIR`, repeated: several machines' homes in one model.
    machines: Vec<PathBuf>,
    /// `--since DURATION`: how far back the model reads (the window).
    /// `--all` (in `options`) reads everything.
    window: Option<std::time::Duration>,
}

impl HomeArgs {
    /// Takes one of the home options, `semon sessions`' names for them;
    /// `false` when `argument` isn't one.
    fn take(
        &mut self,
        argument: &str,
        value: &mut dyn FnMut() -> Result<String, String>,
    ) -> Result<bool, String> {
        match argument {
            "--claude-home" => self.options.claude_home = value()?.into(),
            "--claude-json" => self.options.claude_json = value()?.into(),
            "--codex-home" => self.options.codex_home = value()?.into(),
            "--copilot-home" => self.options.copilot_home = value()?.into(),
            "--proc-root" => self.options.proc_root = value()?.into(),
            "--cache" => self.options.cache = value()?.into(),
            "--facts" => self.options.facts = Some(value()?.into()),
            "--machine" => self.machines.push(PathBuf::from(value()?)),
            "--since" => self.window = Some(semon_sessions::parse_duration(&value()?)?),
            "--all" => self.options.all = true,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn check(&self) -> Result<(), String> {
        if self.options.all && self.window.is_some() {
            return Err("--since and --all are exclusive".into());
        }
        Ok(())
    }

    /// The read surface over these homes, within the window: `--since`,
    /// 30 days by default, or everything with `--all`.
    fn query(&self) -> semon_sessions::Query {
        let options = semon_sessions::Options {
            since: self.window.unwrap_or(semon_sessions::DEFAULT_WINDOW),
            ..self.options.clone()
        };
        if self.machines.is_empty() {
            semon_sessions::Query::new(options)
        } else {
            semon_sessions::Query::with_machines(machine_options(&options, &self.machines))
        }
    }
}

struct QueryArgs {
    home: HomeArgs,
    tool: String,
    arguments: serde_json::Value,
    json: bool,
}

struct PushArgs {
    options: semon_push::PushOptions,
    watch: bool,
}

/// `semon receive`: serve pushes, or manage the tokens that may push.
enum ReceiveArgs {
    Serve(semon_push::serve::ServeOptions),
    TokenAdd { dir: PathBuf, name: String },
    TokenRevoke { dir: PathBuf, name: String },
    TokenList { dir: PathBuf },
}

struct RemoteSessionsArgs {
    endpoint: String,
    config: PathBuf,
    tls_ca: Option<PathBuf>,
}

struct SessionsArgs {
    remote: Option<RemoteSessionsArgs>,
    options: semon_sessions::Options,
    /// `--machine DIR`, repeated: several machines' homes in one model.
    machines: Vec<PathBuf>,
    /// `--machines DIR`: every machine a receiver wrote under
    /// `DIR/machines/`, followed while serving.
    received: Option<PathBuf>,
    /// This machine's own homes too; `--no-local` leaves them out.
    local: bool,
    json: bool,
    model_json: bool,
    watch: bool,
    serve: bool,
    listen: String,
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

/// Arguments for `semon forensic`. Exactly one selector or `export_store`
/// is `Some` by the time parsing succeeds — enforced in
/// [`parse_forensic_args`], not left to `run_forensic` to discover.
struct ForensicArgs {
    store: PathBuf,
    trace: Option<String>,
    session: Option<String>,
    day: Option<String>,
    out: Option<PathBuf>,
    export_store: Option<PathBuf>,
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
    relay: Option<RelayForgetArgs>,
}

#[derive(Debug)]
struct RelayForgetArgs {
    endpoint: String,
    state: PathBuf,
    config: PathBuf,
    tls_ca: Option<PathBuf>,
}

fn parse_args() -> Result<Command, String> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("control") => Ok(Command::Control(arguments.collect())),
        Some("--version") if arguments.next().is_none() => Ok(Command::Version),
        Some("sessions") => parse_sessions_args(arguments).map(Command::Sessions),
        Some("push") => parse_push_args(arguments).map(Command::Push),
        Some("receive") => parse_receive_args(arguments).map(Command::Receive),
        Some("query") => parse_query_args(arguments).map(Command::Query),
        Some("mcp") => parse_mcp_args(arguments).map(Command::Mcp),
        Some("ship") => parse_ship_args(arguments).map(Command::Ship),
        Some("log") => parse_log_args(arguments).map(Command::Log),
        Some("forensic") => parse_forensic_args(arguments).map(Command::Forensic),
        Some("forget") => parse_forget_args(arguments).map(Command::Forget),
        Some("-h" | "--help") => Err(usage()),
        Some(command) => Err(format!("unknown command: {command}\n{}", usage())),
        None => Err(usage()),
    }
}

fn parse_sessions_args(
    mut arguments: impl Iterator<Item = String>,
) -> Result<SessionsArgs, String> {
    let mut options = semon_sessions::Options::default();
    let mut json = false;
    let mut model_json = false;
    let mut watch = false;
    let mut serve = false;
    let mut listen = "127.0.0.1:0".to_owned();
    let mut listen_given = false;
    let mut machines = Vec::new();
    let mut received = None;
    let mut local = true;
    let mut remote_endpoint = None;
    let mut remote_config = None;
    let mut remote_ca = None;
    let mut local_sources_given = false;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        if matches!(
            argument.as_str(),
            "--claude-home" | "--claude-json" | "--codex-home" | "--proc-root" | "--facts"
        ) {
            local_sources_given = true;
        }
        match argument.as_str() {
            "--remote" => remote_endpoint = Some(value()?),
            "--remote-config" => remote_config = Some(PathBuf::from(value()?)),
            "--remote-ca" | "--tls-ca" => remote_ca = Some(PathBuf::from(value()?)),
            "--machines" => received = Some(PathBuf::from(value()?)),
            "--no-local" => local = false,
            "--claude-home" => options.claude_home = value()?.into(),
            "--claude-json" => options.claude_json = value()?.into(),
            "--codex-home" => options.codex_home = value()?.into(),
            "--copilot-home" => options.copilot_home = value()?.into(),
            "--proc-root" => options.proc_root = value()?.into(),
            "--cache" => options.cache = value()?.into(),
            "--all" => options.all = true,
            "--since" => options.since = semon_sessions::parse_duration(&value()?)?,
            "--session" => options.session = Some(value()?),
            "--facts" => options.facts = Some(value()?.into()),
            "--machine" => machines.push(PathBuf::from(value()?)),
            "--json" => json = true,
            "--model-json" => model_json = true,
            "--watch" => watch = true,
            "--serve" => serve = true,
            "--listen" => {
                listen = value()?;
                listen_given = true;
            }
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    let remote = if let Some(endpoint) = remote_endpoint {
        if serve
            || model_json
            || !machines.is_empty()
            || received.is_some()
            || !local
            || local_sources_given
        {
            return Err("--remote supports the metadata tree (--json/--watch/--all/--since/--session); local machine roots, --serve and --model-json are exclusive".into());
        }
        let config = match remote_config {
            Some(config) => config,
            None => {
                let root = env::var_os("XDG_CONFIG_HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
                    .ok_or("--remote-config is required when HOME is unset")?;
                root.join("semon")
            }
        };
        Some(RemoteSessionsArgs {
            endpoint,
            config,
            tls_ca: remote_ca,
        })
    } else {
        if remote_config.is_some() || remote_ca.is_some() {
            return Err("--remote-config and --remote-ca require --remote".into());
        }
        None
    };
    if serve && (json || watch) {
        return Err("--serve cannot be combined with --json or --watch".into());
    }
    if model_json && (serve || json || watch) {
        return Err("--model-json cannot be combined with --serve, --json or --watch".into());
    }
    if model_json && options.session.is_some() {
        return Err(
            "--model-json returns every session; --session is not supported with it".into(),
        );
    }
    if !serve && listen_given {
        return Err("--listen requires --serve".into());
    }
    if !machines.is_empty() && !(model_json || serve) {
        return Err("--machine works with --model-json or --serve".into());
    }
    if received.is_some() {
        if !machines.is_empty() {
            return Err("--machine and --machines are exclusive".into());
        }
        if watch || !(model_json || serve || json) {
            return Err("--machines works with --serve, --model-json or --json".into());
        }
    } else if !local {
        return Err("--no-local requires --machines".into());
    }
    Ok(SessionsArgs {
        remote,
        options,
        machines,
        received,
        local,
        json,
        model_json,
        watch,
        serve,
        listen,
    })
}

/// `semon query <tool> [VALUE] [--argument VALUE …] [--json]` and the home
/// options. Each `--name VALUE` is the tool's argument `name` (dashes for
/// underscores), typed by its schema; `VALUE` alone fills the tool's
/// positional argument.
fn parse_query_args(mut arguments: impl Iterator<Item = String>) -> Result<QueryArgs, String> {
    let tools = semon_sessions::query_tools();
    let tool = match arguments.next() {
        Some(name) if !name.starts_with('-') => name,
        _ => return Err(query_usage()),
    };
    let Some(spec) = tools.iter().find(|spec| spec.name == tool) else {
        return Err(format!("unknown tool: {tool}\n{}", query_usage()));
    };
    let mut home = HomeArgs::default();
    let mut json = false;
    let mut values = serde_json::Map::new();
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        if home.take(&argument, &mut value)? {
            continue;
        }
        match argument.as_str() {
            "--json" => json = true,
            "-h" | "--help" => return Err(query_usage()),
            flag if flag.starts_with("--") => {
                // `--since` is the window; a tool's own `since` is `--newer-than`.
                let key = match flag {
                    "--newer-than" => "since".to_owned(),
                    _ => flag["--".len()..].replace('-', "_"),
                };
                let Some(property) = spec.input_schema["properties"].get(&key) else {
                    return Err(format!("{tool} has no argument {flag}\n{}", query_usage()));
                };
                let raw = value()?;
                let parsed = if property["type"] == "integer" {
                    serde_json::Value::from(
                        raw.parse::<i64>()
                            .map_err(|_| format!("{flag} expects an integer, got {raw}"))?,
                    )
                } else {
                    serde_json::Value::String(raw)
                };
                values.insert(key, parsed);
            }
            positional => {
                let Some(key) = spec.positional else {
                    return Err(format!("{tool} takes no positional argument: {positional}"));
                };
                if values.contains_key(key) {
                    return Err(format!("{tool}: {key} given twice"));
                }
                values.insert(
                    key.to_owned(),
                    serde_json::Value::String(positional.to_owned()),
                );
            }
        }
    }
    home.check()?;
    Ok(QueryArgs {
        home,
        tool,
        arguments: serde_json::Value::Object(values),
        json,
    })
}

fn parse_mcp_args(mut arguments: impl Iterator<Item = String>) -> Result<HomeArgs, String> {
    let mut home = HomeArgs::default();
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        if home.take(&argument, &mut value)? {
            continue;
        }
        return Err(match argument.as_str() {
            "-h" | "--help" => usage(),
            _ => format!("unknown argument: {argument}"),
        });
    }
    home.check()?;
    Ok(home)
}

fn parse_push_args(mut arguments: impl Iterator<Item = String>) -> Result<PushArgs, String> {
    let mut sessions = semon_sessions::Options::default();
    let mut url = None;
    let mut token_file = None;
    let mut state = None;
    let mut watch = false;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--to" => url = Some(value()?),
            "--token-file" => token_file = Some(PathBuf::from(value()?)),
            "--state" => state = Some(PathBuf::from(value()?)),
            "--claude-home" => sessions.claude_home = value()?.into(),
            "--claude-json" => sessions.claude_json = value()?.into(),
            "--codex-home" => sessions.codex_home = value()?.into(),
            "--copilot-home" => sessions.copilot_home = value()?.into(),
            "--proc-root" => sessions.proc_root = value()?.into(),
            "--cache" => sessions.cache = value()?.into(),
            "--watch" => watch = true,
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    let url = url.ok_or("push requires --to URL")?;
    let token_file = token_file.ok_or("push requires --token-file PATH")?;
    let state = state.unwrap_or_else(|| semon_push::default_state_path(&url));
    Ok(PushArgs {
        options: semon_push::PushOptions {
            url,
            credential: semon_push::Credential::File(token_file),
            sessions,
            state,
        },
        watch,
    })
}

/// `semon receive --dir DIR [--listen ADDR] [--tls-cert PEM --tls-key PEM]`,
/// or `semon receive token (add NAME | revoke NAME | list) --dir DIR`.
fn parse_receive_args(arguments: impl Iterator<Item = String>) -> Result<ReceiveArgs, String> {
    let mut arguments = arguments.peekable();
    if arguments.peek().is_some_and(|argument| argument == "token") {
        arguments.next();
        return parse_receive_token_args(arguments);
    }
    let mut dir = None;
    let mut listen = semon_push::serve::DEFAULT_LISTEN.to_owned();
    let mut certificate = None;
    let mut private_key = None;
    let mut max_bytes = semon_push::mirror::DEFAULT_MAX_BYTES;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--listen" => listen = value()?,
            "--tls-cert" => certificate = Some(PathBuf::from(value()?)),
            "--tls-key" => private_key = Some(PathBuf::from(value()?)),
            "--max-bytes" => max_bytes = parse_size(&value()?)?,
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown receive argument: {argument}")),
        }
    }
    let dir = dir.ok_or("receive requires --dir DIR")?;
    let listen = listen
        .parse::<std::net::SocketAddr>()
        .map_err(|error| format!("invalid --listen address {listen}: {error}"))?;
    let tls = match (certificate, private_key) {
        (Some(certificate), Some(private_key)) => Some(semon_push::serve::TlsFiles {
            certificate,
            private_key,
        }),
        (None, None) => None,
        _ => return Err("TLS requires both --tls-cert and --tls-key".into()),
    };
    Ok(ReceiveArgs::Serve(semon_push::serve::ServeOptions {
        dir,
        listen,
        tls,
        max_bytes,
    }))
}

/// A byte count: digits, optionally followed by K, M, G or T (powers of
/// 1024), as in `20G`.
fn parse_size(raw: &str) -> Result<u64, String> {
    let invalid = || format!("invalid size {raw}: use bytes, or a number with K, M, G or T");
    let (digits, shift) = match raw.char_indices().last() {
        Some((at, unit @ ('K' | 'M' | 'G' | 'T' | 'k' | 'm' | 'g' | 't'))) => {
            let shift = match unit.to_ascii_uppercase() {
                'K' => 10,
                'M' => 20,
                'G' => 30,
                _ => 40,
            };
            (&raw[..at], shift)
        }
        _ => (raw, 0),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    digits
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(1u64 << shift))
        .ok_or_else(invalid)
}

fn parse_receive_token_args(
    mut arguments: impl Iterator<Item = String>,
) -> Result<ReceiveArgs, String> {
    let action = arguments
        .next()
        .ok_or("receive token requires add NAME, revoke NAME or list")?;
    let mut name = None;
    let mut dir = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dir" => {
                dir = Some(PathBuf::from(
                    arguments.next().ok_or("--dir requires a value")?,
                ));
            }
            "-h" | "--help" => return Err(usage()),
            flag if flag.starts_with('-') => {
                return Err(format!("unknown receive token argument: {flag}"));
            }
            _ if name.is_none() && action != "list" => name = Some(argument.clone()),
            _ => return Err(format!("unexpected receive token argument: {argument}")),
        }
    }
    let dir = dir.ok_or("receive token requires --dir DIR")?;
    let name = || {
        name.clone()
            .ok_or_else(|| format!("receive token {action} requires NAME"))
    };
    match action.as_str() {
        "add" => Ok(ReceiveArgs::TokenAdd { name: name()?, dir }),
        "revoke" => Ok(ReceiveArgs::TokenRevoke { name: name()?, dir }),
        "list" => Ok(ReceiveArgs::TokenList { dir }),
        _ => Err(format!(
            "unknown receive token action: {action} (add, revoke or list)"
        )),
    }
}

fn run_receive(args: ReceiveArgs) -> Result<(), String> {
    use semon_push::tokens;
    match args {
        ReceiveArgs::Serve(options) => {
            let server = semon_push::serve::Server::bind(&options)?;
            let address = server
                .local_addr()
                .map_err(|error| format!("the listening address: {error}"))?;
            eprintln!(
                "semon receive: listening on {}://{address}, writing {}",
                server.scheme(),
                options.dir.join(semon_push::mirror::MACHINES_DIR).display()
            );
            if server.tokens() == 0 {
                eprintln!(
                    "semon receive: no token yet, so every push is refused (401); add one with `semon receive token add NAME --dir {}`",
                    options.dir.display()
                );
            }
            server.run()
        }
        ReceiveArgs::TokenAdd { dir, name } => {
            let token = tokens::add_token(&dir, &name)?;
            println!("{token}");
            eprintln!(
                "semon receive: added a token for {name}. It is shown only this once: put it in a 0600 file on {name} and run `semon push --to URL --token-file FILE` there."
            );
            Ok(())
        }
        ReceiveArgs::TokenRevoke { dir, name } => {
            tokens::revoke_token(&dir, &name)?;
            eprintln!(
                "semon receive: revoked {name}'s token; a running receiver refuses it from its next request"
            );
            Ok(())
        }
        ReceiveArgs::TokenList { dir } => {
            for name in tokens::token_names(&dir)? {
                println!("{name}");
            }
            Ok(())
        }
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
    let mut export_store = None;
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
            "--export-store" if export_store.is_none() => export_store = Some(value()?.into()),
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }

    let selected = [
        trace.is_some(),
        session.is_some(),
        day.is_some(),
        export_store.is_some(),
    ]
    .into_iter()
    .filter(|is_set| *is_set)
    .count();
    if selected != 1 {
        return Err(format!(
            "forensic: exactly one of --trace, --session, --day, or --export-store is required (got {selected})\n\
             {}",
            usage()
        ));
    }
    if export_store.is_some() && out.is_some() {
        return Err("forensic: --export-store writes a new directory and cannot use --out".into());
    }

    Ok(ForensicArgs {
        store,
        trace,
        session,
        day,
        out,
        export_store,
    })
}

fn parse_forget_args(mut arguments: impl Iterator<Item = String>) -> Result<ForgetArgs, String> {
    let mut store = default_store_path();
    let mut forensic = false;
    let mut trace = None;
    let mut session = None;
    let mut before = None;
    let mut yes = false;
    let mut relay_endpoint = None;
    let mut relay_state = None;
    let mut relay_config = None;
    let mut relay_ca = None;
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
            "--relay-endpoint" => relay_endpoint = Some(value()?),
            "--relay-state" => relay_state = Some(PathBuf::from(value()?)),
            "--relay-config" => relay_config = Some(PathBuf::from(value()?)),
            "--relay-ca" => relay_ca = Some(PathBuf::from(value()?)),
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

    let relay = match relay_endpoint {
        Some(endpoint) => {
            if trace.is_some() {
                return Err("--trace cannot select replicated carrier frames; use --session or --before for server deletion".into());
            }
            Some(RelayForgetArgs {
                endpoint,
                state: relay_state
                    .ok_or("--relay-endpoint requires --relay-state (the sender state path)")?,
                config: relay_config.ok_or(
                    "--relay-endpoint requires --relay-config (the enrolled identity directory)",
                )?,
                tls_ca: relay_ca,
            })
        }
        None => {
            if relay_state.is_some() || relay_config.is_some() || relay_ca.is_some() {
                return Err("relay options require --relay-endpoint".into());
            }
            None
        }
    };
    Ok(ForgetArgs {
        store,
        trace,
        session,
        before,
        yes,
        relay,
    })
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Control(args) => run_control(args),
        Command::Version => {
            println!("semon {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Sessions(args) => run_sessions(args),
        Command::Push(args) => run_push(&args),
        Command::Receive(args) => run_receive(args),
        Command::Query(args) => run_query(args),
        Command::Mcp(home) => run_mcp(&home),
        Command::Ship(args) => run_ship(args).map(|message| println!("{message}")),
        Command::Log(args) => run_log(args).map(|message| println!("{message}")),
        Command::Forensic(args) => run_forensic(args),
        Command::Forget(args) => run_forget(args),
    }
}

/// `semon push`, stopped by Ctrl-C or SIGTERM through the same
/// [`semon_push::Stop`] an embedding app uses. Once it has returned, the
/// signal is raised again with its default action, so the process ends
/// the way it did before: by that signal.
fn run_push(args: &PushArgs) -> Result<(), String> {
    let stop = semon_push::Stop::new();
    interrupt::stop_on_signal(&stop);
    let result = semon_push::push_until(&args.options, args.watch, &stop);
    if interrupt::received()
        && let Err(error) = &result
    {
        // Said here, as `main` would, since raising the signal below ends
        // the process before `main` sees the result.
        eprintln!("semon: {error}");
    }
    if stop.is_stopped() {
        eprintln!("semon push: stopped");
    }
    interrupt::raise_received();
    result
}

/// Ctrl-C and SIGTERM, turned into a [`semon_push::Stop`]. The handler only
/// stores the signal's number, which is async-signal-safe, and puts the
/// default action back, so a second Ctrl-C ends the process at once; a
/// thread turns the number into the stop. `signal` and `raise` are the C
/// library's, which `std` already links; SIGINT (2) and SIGTERM (15) have
/// these numbers on every Unix Rust supports.
#[cfg(unix)]
mod interrupt {
    use std::{
        os::raw::c_int,
        sync::atomic::{AtomicI32, Ordering},
        thread,
        time::Duration,
    };

    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;
    const SIG_DFL: usize = 0;
    const SIG_IGN: usize = 1;

    static RECEIVED: AtomicI32 = AtomicI32::new(0);

    unsafe extern "C" {
        fn signal(signum: c_int, handler: usize) -> usize;
        fn raise(signum: c_int) -> c_int;
    }

    extern "C" fn on_signal(signum: c_int) {
        RECEIVED.store(signum, Ordering::SeqCst);
        // SAFETY: `signal` is async-signal-safe, and SIG_DFL is a valid
        // disposition.
        unsafe { signal(signum, SIG_DFL) };
    }

    pub fn stop_on_signal(stop: &semon_push::Stop) {
        let handler: extern "C" fn(c_int) = on_signal;
        for signum in [SIGINT, SIGTERM] {
            // SAFETY: `on_signal` has the handler's C signature and does
            // only async-signal-safe work.
            let previous = unsafe { signal(signum, handler as *const () as usize) };
            if previous == SIG_IGN {
                // Started with it ignored (`cmd &` in a script): it stays so.
                // SAFETY: SIG_IGN is a valid disposition.
                unsafe { signal(signum, SIG_IGN) };
            }
        }
        let stop = stop.clone();
        let _ = thread::Builder::new()
            .name("semon-signals".to_owned())
            .spawn(move || {
                while RECEIVED.load(Ordering::SeqCst) == 0 {
                    thread::sleep(Duration::from_millis(50));
                }
                stop.stop();
            });
    }

    /// Whether Ctrl-C or SIGTERM came.
    pub fn received() -> bool {
        RECEIVED.load(Ordering::SeqCst) != 0
    }

    /// Ends the process by the signal that stopped the push, if one did.
    pub fn raise_received() {
        let signum = RECEIVED.load(Ordering::SeqCst);
        if signum != 0 {
            // SAFETY: the handler already put the default action back, so
            // this ends the process as the signal would have.
            unsafe { raise(signum) };
        }
    }
}

/// Elsewhere Ctrl-C ends the process as it always did; the OS releases the
/// push's state lock with it.
#[cfg(not(unix))]
mod interrupt {
    pub fn stop_on_signal(_: &semon_push::Stop) {}
    pub fn received() -> bool {
        false
    }
    pub fn raise_received() {}
}

/// The model of several machines' homes, as one viewer serves it: each
/// `DIR` holds `claude/`, `codex/` and `proc/`, and `facts.json` when its
/// facts were recorded elsewhere; the metadata cache goes in `DIR/.semon/`.
fn print_machines_model(args: &SessionsArgs) -> Result<(), String> {
    let mut core =
        semon_sessions::ViewerCore::with_machines(machine_options(&args.options, &args.machines));
    print_route(&mut core, "/api/model", "the model of these machines")
}

/// Prints one of a core's routes, the viewer's model or its tree, as it
/// would serve it.
fn print_route(
    core: &mut semon_sessions::ViewerCore,
    path: &str,
    what: &str,
) -> Result<(), String> {
    let reply = core.respond("GET", path, "", None);
    let body = String::from_utf8_lossy(&reply.body);
    if reply.status != 200 {
        return Err(format!("{what}: {} {body}", reply.status));
    }
    println!("{body}");
    Ok(())
}

/// `--machines DIR`: this machine's homes (unless `--no-local`), and the
/// received machines under `DIR/machines/`.
fn received_machines(
    args: &SessionsArgs,
    dir: &Path,
) -> (
    Vec<(String, semon_sessions::Options)>,
    semon_sessions::ReceivedMachines,
) {
    if !dir.join("machines").is_dir() {
        eprintln!(
            "semon: {} doesn't exist yet; machines are shown as they are received",
            dir.join("machines").display()
        );
    }
    let local = if args.local {
        vec![(String::new(), args.options.clone())]
    } else {
        Vec::new()
    };
    (
        local,
        semon_sessions::ReceivedMachines::new(dir, &args.options),
    )
}

/// `--machines DIR` with `--model-json` or `--json`: the model, or the tree,
/// of every machine at once.
fn print_received(args: &SessionsArgs, dir: &Path) -> Result<(), String> {
    let (local, received) = received_machines(args, dir);
    let mut core = semon_sessions::ViewerCore::with_received(local, received);
    let (path, what) = if args.model_json {
        ("/api/model", "the model of these machines")
    } else {
        ("/api/tree", "the tree of these machines")
    };
    let printed = print_route(&mut core, path, what);
    if core.machines() == 0 {
        return Err(format!(
            "no machines: none under {} and --no-local",
            dir.join("machines").display()
        ));
    }
    printed
}

/// Each `--machine DIR`'s options.
fn machine_options(
    options: &semon_sessions::Options,
    machines: &[PathBuf],
) -> Vec<(String, semon_sessions::Options)> {
    machines
        .iter()
        .map(|dir| {
            let facts = dir.join("facts.json");
            let options = semon_sessions::Options {
                claude_home: dir.join("claude"),
                codex_home: dir.join("codex"),
                copilot_home: dir.join("copilot"),
                proc_root: dir.join("proc"),
                cache: dir.join(".semon").join("sessions-index.json"),
                facts: facts.is_file().then_some(facts),
                ..options.clone()
            };
            (dir.display().to_string(), options)
        })
        .collect()
}

fn run_control(args: Vec<String>) -> Result<(), String> {
    let usage = "Usage: semon control codex --codex NATIVE_PACKAGE/bin/codex --workspace PATH --state NEW_PRIVATE_DIR [--config MODEL_CONFIG]
       semon control observe --socket PATH --thread NATIVE_ID --codex-home PATH --journal PATH";
    let mode = args.first().ok_or_else(|| usage.to_owned())?;
    let mut values = std::collections::BTreeMap::new();
    let mut rest = args[1..].iter();
    while let Some(name) = rest.next() {
        if ![
            "--codex",
            "--workspace",
            "--state",
            "--config",
            "--socket",
            "--thread",
            "--codex-home",
            "--journal",
        ]
        .contains(&name.as_str())
            || values.contains_key(name)
        {
            return Err(usage.into());
        }
        values.insert(
            name.clone(),
            rest.next().ok_or_else(|| usage.to_owned())?.clone(),
        );
    }
    let allowed: &[&str] = match mode.as_str() {
        "codex" => &["--codex", "--workspace", "--state", "--config"],
        "observe" => &["--socket", "--thread", "--codex-home", "--journal"],
        _ => return Err(usage.into()),
    };
    if values.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(usage.into());
    }
    let value = |key: &str| values.get(key).cloned().ok_or_else(|| usage.to_owned());
    let (driver, home, _session) = match mode.as_str() {
        "codex" => {
            let session = semon_control::local::LocalSession::launch(
                Path::new(&value("--codex")?),
                Path::new(&value("--workspace")?),
                Path::new(&value("--state")?),
                values.get("--config").map(Path::new),
            )
            .map_err(|e| e.to_string())?;
            // Quote owner-selected paths in the printed terminal command.
            let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
            eprintln!(
                "Codex thread {}. After the first message, attach a terminal: CODEX_HOME={} {} --remote {} resume {}",
                session.driver.thread_id(),
                quote(&session.home.to_string_lossy()),
                quote(&value("--codex")?),
                quote(&format!("unix://{}", session.socket.display())),
                session.driver.thread_id()
            );
            (session.driver.clone(), session.home.clone(), Some(session))
        }
        "observe" => {
            let store = std::sync::Arc::new(semon_control::RequestStore::new(
                semon_control::Journal::open(Path::new(&value("--journal")?))
                    .map_err(|e| e.to_string())?,
            ));
            let driver = semon_control::codex::Codex::observe(
                Path::new(&value("--socket")?),
                &value("--thread")?,
                store,
            )
            .map_err(|e| e.to_string())?;
            (driver, PathBuf::from(value("--codex-home")?), None)
        }
        _ => return Err(usage.into()),
    };
    let sessions = semon_sessions::Options {
        codex_home: home.clone(),
        claude_home: home.join("absent-claude"),
        copilot_home: home.join("absent-copilot"),
        cache: home.join("semon-index.json"),
        all: true,
        ..Default::default()
    };
    semon_sessions::serve_with_control(
        semon_sessions::ServeOptions {
            sessions,
            machines: Vec::new(),
            received: None,
            listen: "127.0.0.1:0".into(),
        },
        driver,
    )
    .map_err(|e| e.to_string())
}

fn run_sessions(args: SessionsArgs) -> Result<(), String> {
    if let Some(remote) = &args.remote {
        return run_remote_sessions(&args, remote);
    }
    if let Some(dir) = args.received.clone() {
        if !args.serve {
            return print_received(&args, &dir);
        }
        let (machines, received) = received_machines(&args, &dir);
        return semon_sessions::serve(semon_sessions::ServeOptions {
            machines,
            received: Some(received),
            sessions: args.options,
            listen: args.listen,
        })
        .map_err(|error| error.to_string());
    }
    if args.serve {
        return semon_sessions::serve(semon_sessions::ServeOptions {
            machines: machine_options(&args.options, &args.machines),
            received: None,
            sessions: args.options,
            listen: args.listen,
        })
        .map_err(|error| error.to_string());
    }
    if args.model_json && !args.machines.is_empty() {
        return print_machines_model(&args);
    }
    if args.model_json {
        let json = semon_sessions::model_json(&args.options).map_err(|error| error.to_string())?;
        println!("{json}");
        return Ok(());
    }
    loop {
        let nodes = semon_sessions::collect(&args.options).map_err(|error| error.to_string())?;
        if args.watch {
            print!("\x1b[2J\x1b[H");
        }
        if args.json {
            println!("{}", semon_sessions::render_json(&nodes));
        } else {
            print!("{}", semon_sessions::render_text(&nodes));
        }
        io::stdout().flush().map_err(|error| error.to_string())?;
        if !args.watch {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn run_remote_sessions(args: &SessionsArgs, remote: &RemoteSessionsArgs) -> Result<(), String> {
    let identity =
        semon_relay::MachineIdentity::load(&remote.config).map_err(|error| error.to_string())?;
    let transport = semon_relay::HttpTransport::secure(
        remote.endpoint.clone(),
        std::time::Duration::from_secs(10),
        semon_relay::RequestSigner::new(identity.signing.clone()),
        remote.tls_ca.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let mut last = None;
    let mut last_error = None;
    loop {
        match semon_sessions::collect_remote(&args.options, &identity, &transport) {
            Ok(nodes) => {
                last_error = None;
                let rendered = if args.json {
                    semon_sessions::render_json(&nodes)
                } else {
                    semon_sessions::render_text(&nodes)
                };
                if last.as_ref() != Some(&rendered) {
                    if args.watch {
                        print!("\x1b[2J\x1b[H");
                    }
                    println!("{rendered}");
                    io::stdout().flush().map_err(|error| error.to_string())?;
                    last = Some(rendered);
                }
            }
            Err(error) if args.watch => {
                let error = error.to_string();
                if last_error.as_ref() != Some(&error) {
                    eprintln!("remote view unavailable: {error}");
                    last_error = Some(error);
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        if !args.watch {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

/// One tool's answer as JSON on stdout: pretty, or one line with `--json`.
/// A failure goes to stderr (and, with `--json`, as a JSON error on stdout)
/// and exits nonzero.
fn run_query(args: QueryArgs) -> Result<(), String> {
    let mut query = args.home.query();
    match query.call(&args.tool, &args.arguments) {
        Ok(value) => {
            if args.json {
                println!("{value}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
                );
            }
            Ok(())
        }
        Err(error) => {
            if args.json {
                println!("{}", error.to_json());
            }
            Err(format!("query {}: {error}", args.tool))
        }
    }
}

/// The read surface as an MCP server on stdin and stdout, until stdin ends.
fn run_mcp(home: &HomeArgs) -> Result<(), String> {
    let mut query = home.query();
    semon_sessions::serve_mcp(&mut query, io::stdin().lock(), io::stdout().lock())
        .map_err(|error| error.to_string())
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
        repo: args.repo.clone(),
        timestamp_range,
        limit: args.limit,
    };
    let rows = store.log(&filter).map_err(|error| error.to_string())?;
    if rows.is_empty() {
        if let Some(day) = args.day.as_deref() {
            // A --day-filtered miss is ambiguous: it reads the same whether
            // that UTC day genuinely had no occurrences, or the caller named
            // the wrong day (--day is a UTC calendar day; e.g. Codex's
            // `~/.codex/sessions/YYYY/MM/DD/` directories are named in local
            // time, so a directory-derived date can be one UTC day off).
            // When the store isn't empty (ignoring --day, but still under
            // the same --repo, if given), name the UTC day(s) it does cover
            // so the two cases aren't indistinguishable.
            let span = store
                .timestamp_span(args.repo.as_deref())
                .map_err(|error| error.to_string())?;
            if let Some((earliest, latest)) = span {
                let earliest_day = format_day_ns(earliest);
                let latest_day = format_day_ns(latest);
                let covers = if earliest_day == latest_day {
                    earliest_day
                } else {
                    format!("{earliest_day} to {latest_day}")
                };
                return Ok(format!(
                    "(no occurrences on {day} UTC; store covers {covers})"
                ));
            }
        }
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

    if let Some(directory) = args.export_store {
        forensic_export::export(&args.store, &directory).map_err(|error| error.to_string())?;
        println!(
            "semon forensic: complete store export written to {}",
            directory.display()
        );
        return Ok(());
    }

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
    if count == 0 && args.relay.is_none() {
        println!("Server copies are unaffected; configure --relay-endpoint to propagate deletion.");
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
             raw_carrier_records and request any configured relay deletion. This cannot be undone. canonical_traces and occurrences \
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

    if let Some(relay) = &args.relay {
        use semon_relay::Transport;
        let identity =
            semon_relay::MachineIdentity::load(&relay.config).map_err(|error| error.to_string())?;
        let transport = semon_relay::HttpTransport::secure(
            relay.endpoint.clone(),
            std::time::Duration::from_secs(10),
            semon_relay::RequestSigner::new(identity.signing.clone()),
            relay.tls_ca.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        let server_selector = semon_relay::ForgetSelector {
            session: args.session.clone(),
            before_ns: args
                .before
                .as_deref()
                .map(semon_relay::forget_before_day)
                .transpose()?,
            memory_root: None,
        };
        // Persist before local deletion so offline receivers are retried by the sender.
        semon_relay::queue_forget(&relay.state, &transport.deletion_scope(), &server_selector)
            .map_err(|error| error.to_string())?;
        let result = semon_relay::flush_forgets(&relay.state, &identity.fingerprint(), &transport)
            .map_err(|error| error.to_string())?;
        println!(
            "forget relay: acknowledged={} pending_on_server={}",
            result.acknowledged, result.pending
        );
        for failure in result.failures {
            eprintln!("forget relay pending: {failure}");
        }
        println!("Copies already restored on another machine cannot be erased remotely.");
    } else {
        println!("Server copies are unaffected; configure --relay-endpoint to propagate deletion.");
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

fn query_usage() -> String {
    let tools: Vec<String> = semon_sessions::query_tools()
        .iter()
        .map(|tool| {
            let mut line = format!("  {}", tool.name);
            if let Some(positional) = tool.positional {
                line.push_str(&format!(" {}", positional.to_uppercase()));
            }
            let flags: Vec<String> = tool.input_schema["properties"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(name, _)| Some(name.as_str()) != tool.positional)
                .map(|(name, _)| match name.as_str() {
                    "since" => "[--newer-than VALUE]".to_owned(),
                    _ => format!("[--{} VALUE]", name.replace('_', "-")),
                })
                .collect();
            if !flags.is_empty() {
                line.push(' ');
                line.push_str(&flags.join(" "));
            }
            line
        })
        .collect();
    format!(
        "Usage: semon query TOOL [ARGUMENTS] [--json] [--since DURATION | --all] [--claude-home PATH] [--claude-json PATH] [--codex-home PATH] [--proc-root PATH] [--cache PATH] [--facts FILE] [--machine DIR]...\n\
         The agent read surface: one tool's answer as JSON (one line with --json). It reads the log files modified\n\
         within the window, --since (30d by default), or all of them with --all. A tool's own since argument is\n\
         --newer-than, a filter inside the window. The tools:\n{}",
        tools.join("\n")
    )
}

fn usage() -> String {
    format!(
        "Usage: semon sessions [--remote ENDPOINT --remote-config PATH --remote-ca CERT] [--claude-home PATH] [--claude-json PATH] [--codex-home PATH] [--proc-root PATH] [--cache PATH] [--all | --since DURATION] [--session ID] [--facts FILE] [--json | --model-json] [--watch] [--serve [--listen 127.0.0.1:PORT]] [--machines DIR [--no-local]]\n\
         Shows a read-only tree of local Claude Code and Codex sessions. --model-json writes the viewer's\n\
         session model (sessions, handoffs, turns, busy) instead; it reads every log, --all/--since trim the output.\n\
         --facts takes the machine's side (hostname, live processes, repositories) from FILE instead of this machine.\n\
         --machine DIR (repeated, with --model-json or --serve): one view over several machines' homes, DIR/{{claude,codex,proc}}.\n\
         --machines DIR (with --serve, --model-json or --json): this machine and every machine `semon receive` wrote under\n\
         DIR/machines/, followed while serving; --no-local leaves this machine out. DIR is only read.\n\
         \n\
         Usage: semon query TOOL [ARGUMENTS] [--json] [--since DURATION | --all] [home options as for sessions, and --machine DIR]\n\
         The agent read surface over the same session model: list_sessions, get_session, read_transcript, find,\n\
         stalls. `semon query` alone lists each tool's arguments.\n\
         \n\
         Usage: semon mcp [--since DURATION | --all] [--claude-home PATH] [--claude-json PATH] [--codex-home PATH] [--proc-root PATH] [--cache PATH] [--facts FILE] [--machine DIR]...\n\
         The same tools as a Model Context Protocol server on stdin and stdout. Read-only; no listener. Both read\n\
         the log files modified within the window, --since (30d by default), or all of them with --all.\n\
         \n\
         Usage: semon push --to URL --token-file PATH [--watch] [--state PATH] [--claude-home PATH] [--claude-json PATH] [--codex-home PATH] [--proc-root PATH] [--cache PATH]\n\
         Sends the session logs' input files, redacted, and this machine's facts to a mirror-protocol receiver\n\
         (docs/mirror-protocol.md), appending as they grow. The token file must be mode 0600. --watch keeps going:\n\
         new lines every 2 s, facts every 10 s.\n\
         \n\
         Usage: semon receive --dir DIR [--listen ADDR] [--tls-cert PEM --tls-key PEM] [--max-bytes SIZE]\n\
         Usage: semon receive token (add NAME | revoke NAME | list) --dir DIR\n\
         A mirror-protocol receiver for semon push from your other machines: it writes each machine's copy to\n\
         DIR/machines/NAME/. It listens on 127.0.0.1:8735 by default; another address needs TLS (an operator-supplied\n\
         certificate and key) and at least one token. A token, printed once by token add, decides its machine.\n\
         --max-bytes caps each machine's copy (20G by default); a push past it gets 507.\n\
         \n\
         Usage: semon ship [--store PATH] [--endpoint URL]\n\
         Endpoint defaults to ${REPLICATION_ENDPOINT_ENV}; when unset, ship succeeds without reading the store.\n\
         \n\
         Usage: semon log [--store PATH] [--repo NAME] [--day YYYY-MM-DD] [--limit N]\n\
         Renders the occurrence log. Never reads raw_carrier_records. --day is a UTC\n\
         calendar day (00:00:00Z through 23:59:59Z), not local time — Codex's own\n\
         ~/.codex/sessions/YYYY/MM/DD/ directory names are local dates, so a date taken\n\
         from one can be a UTC day off. If --day matches nothing but the store (under\n\
         the same --repo, if given) is not empty, the days it does cover are reported\n\
         instead of a bare empty result.\n\
         \n\
         Usage: semon forensic [--store PATH] (--trace ID | --session ID | --day YYYY-MM-DD) [--out FILE]\n\
         Reads raw_carrier_records — the only command that does. Exactly one\n\
         of --trace, --session, --day is required. Without --out, writes to\n\
         stdout; with it, writes to FILE created 0600 instead.\n\
         \n\
         Usage: semon forensic [--store PATH] --export-store NEW_DIRECTORY\n\
         Exports all trace, occurrence and forensic tables as a verified SQLite snapshot\n\
         plus a versioned checksum manifest. Source is opened read-only without migration;\n\
         pause store writers during the snapshot. NEW_DIRECTORY must not exist.\n\
         The directory is created 0700 and its files 0600. No data is removed.\n\
         \n\
         Usage: semon forget --forensic [--store PATH] (--before YYYY-MM-DD | --session ID | --trace ID) [--yes] [--relay-endpoint URL --relay-state PATH --relay-config PATH [--relay-ca CERT]]\n\
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
    fn remote_sessions_accept_metadata_flags_and_refuse_local_surface_combinations() {
        let args = parse_sessions_args(
            [
                "--remote",
                "https://receiver",
                "--remote-config",
                "/keys",
                "--remote-ca",
                "/ca",
                "--json",
                "--watch",
                "--session",
                "s",
                "--since",
                "2h",
            ]
            .map(str::to_owned)
            .into_iter(),
        )
        .unwrap();
        assert!(args.remote.is_some() && args.json && args.watch);
        assert_eq!(args.options.session.as_deref(), Some("s"));
        for flag in ["--serve", "--model-json", "--no-local"] {
            assert!(
                parse_sessions_args(
                    ["--remote", "https://receiver", flag]
                        .map(str::to_owned)
                        .into_iter()
                )
                .is_err()
            );
        }
        assert!(
            parse_sessions_args(["--remote-ca", "/ca"].map(str::to_owned).into_iter()).is_err()
        );
    }

    #[test]
    fn model_json_refuses_flags_it_cannot_honour() {
        let parse = |arguments: &[&str]| {
            parse_sessions_args(arguments.iter().map(|argument| (*argument).to_owned()))
        };
        assert!(parse(&["--model-json", "--all"]).is_ok_and(|args| args.model_json));
        for refused in [
            &["--model-json", "--session", "abc"][..],
            &["--model-json", "--json"][..],
            &["--model-json", "--serve"][..],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
        }
    }

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
    fn log_on_a_matching_day_renders_normally_with_no_hint() {
        let path = unique_temp_db_path("log-day-hit");
        let (start, _) = day_bounds_ns(2026, 9, 18);
        {
            let mut store = TraceStore::open(&path).unwrap();
            let core = SemanticCore::from_value(json!({"kind": "intent", "content": "on the day"}))
                .unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"raw bytes"),
                    NewOccurrence {
                        session: "session-a",
                        sequence: 0,
                        timestamp: start + 1,
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
            day: Some("2026-09-18".to_owned()),
            limit: None,
        })
        .unwrap();

        // The output for a --day filter that actually matches is exactly the
        // ordinary rendering: no "store covers" hint appended.
        assert!(message.contains("session-a#0"));
        assert!(message.contains("on the day"));
        assert!(!message.contains("store covers"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn log_on_a_missed_day_names_the_days_the_store_covers() {
        let path = unique_temp_db_path("log-day-miss");
        // A capture on 2026-09-18, queried with --day 2026-09-19 — the exact
        // Codex-local-vs-UTC trap issue #5 names: a store that isn't empty,
        // but has nothing on the requested UTC day.
        let (start, _) = day_bounds_ns(2026, 9, 18);
        {
            let mut store = TraceStore::open(&path).unwrap();
            let core = SemanticCore::from_value(
                json!({"kind": "intent", "content": "captured on the 18th"}),
            )
            .unwrap();
            store
                .capture(
                    &core,
                    NewRawCarrierRecord::new("codex", b"raw bytes"),
                    NewOccurrence {
                        session: "session-a",
                        sequence: 0,
                        timestamp: start + 1,
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
            day: Some("2026-09-19".to_owned()),
            limit: None,
        })
        .unwrap();

        assert_eq!(
            message,
            "(no occurrences on 2026-09-19 UTC; store covers 2026-09-18)"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn log_on_a_missed_day_over_an_empty_store_gives_no_hint() {
        let path = unique_temp_db_path("log-day-miss-empty");
        TraceStore::open(&path).unwrap();

        let message = run_log(LogArgs {
            store: path.clone(),
            repo: None,
            day: Some("2026-09-19".to_owned()),
            limit: None,
        })
        .unwrap();

        // Nothing to name a span from: the plain, pre-existing message.
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
            export_store: None,
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
            export_store: None,
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
            export_store: None,
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
            relay: None,
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
            relay: None,
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
            export_store: None,
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
            relay: None,
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
    fn offline_relay_forget_deletes_local_records_and_keeps_the_server_request() {
        let (store_path, trace) = store_with_one_capture("forget-offline-relay", "session-a", 0);
        let config = store_path.with_extension("relay-keys");
        let state = store_path.with_extension("relay-state");
        semon_relay::init(&config).unwrap();
        // Reserve a closed endpoint: listener is dropped before connecting.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        run_forget(ForgetArgs {
            store: store_path.clone(),
            trace: None,
            session: Some("session-a".into()),
            before: None,
            yes: true,
            relay: Some(RelayForgetArgs {
                endpoint: endpoint.clone(),
                state: state.clone(),
                config: config.clone(),
                tls_ca: None,
            }),
        })
        .unwrap();
        let store = TraceStore::open(&store_path).unwrap();
        assert!(
            store
                .fetch_raw_carrier_records(&TraceId::from_str(&trace).unwrap())
                .unwrap()
                .is_empty()
        );
        let identity = semon_relay::MachineIdentity::load(&config).unwrap();
        let transport = semon_relay::HttpTransport::secure(
            endpoint,
            std::time::Duration::from_secs(1),
            semon_relay::RequestSigner::new(identity.signing.clone()),
            None,
        )
        .unwrap();
        assert_eq!(
            semon_relay::flush_forgets(&state, &identity.fingerprint(), &transport)
                .unwrap()
                .pending,
            1
        );
        drop(store);
        let _ = std::fs::remove_file(store_path);
        std::fs::remove_dir_all(config).unwrap();
        let mut queue = state.as_os_str().to_owned();
        queue.push(".forget");
        std::fs::remove_dir_all(PathBuf::from(queue)).unwrap();
    }

    #[test]
    fn relay_forget_requires_explicit_sender_paths_and_rejects_trace_selectors() {
        let base = [
            "--forensic",
            "--session",
            "session-a",
            "--relay-endpoint",
            "http://127.0.0.1:8734",
        ];
        assert!(
            parse_forget_args(base.map(str::to_owned).into_iter())
                .unwrap_err()
                .contains("--relay-state")
        );
        let valid = [
            "--forensic",
            "--session",
            "session-a",
            "--relay-endpoint",
            "http://127.0.0.1:8734",
            "--relay-state",
            "/state",
            "--relay-config",
            "/keys",
        ];
        assert!(
            parse_forget_args(valid.map(str::to_owned).into_iter())
                .unwrap()
                .relay
                .is_some()
        );
        let trace = [
            "--forensic",
            "--trace",
            "trace-a",
            "--relay-endpoint",
            "http://127.0.0.1:8734",
            "--relay-state",
            "/state",
            "--relay-config",
            "/keys",
        ];
        assert!(
            parse_forget_args(trace.map(str::to_owned).into_iter())
                .unwrap_err()
                .contains("cannot select replicated")
        );
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
            relay: None,
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
            relay: None,
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

    #[test]
    fn receive_sizes_parse_with_binary_units() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("20G").unwrap(), 20u64 << 30);
        assert_eq!(parse_size("5m").unwrap(), 5u64 << 20);
        assert_eq!(parse_size("1T").unwrap(), 1u64 << 40);
        for bad in ["", "G", "-1", "1.5G", "10GB", "99999999999T"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }
}
