use std::{
    env,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
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
    Version,
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

struct SessionsArgs {
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
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--remote" | "--remote-config" | "--remote-ca" | "--tls-ca" => return Err(
                "Encrypted Relay viewing is retired. Preserve existing keys/data and use the pinned legacy reader or semon-relay-history; see docs/encrypted-relay-retirement.md. Use --machines for generic mirrored sources.".into()
            ),
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
    "Usage: semon sessions [--claude-home PATH] [--claude-json PATH] [--codex-home PATH] [--proc-root PATH] [--cache PATH] [--all | --since DURATION] [--session ID] [--facts FILE] [--json | --model-json] [--watch] [--serve [--listen 127.0.0.1:PORT]] [--machines DIR [--no-local]]\n\
         Shows a read-only tree of local Claude Code and Codex sessions. --model-json writes the viewer's\n\
         session model (sessions, handoffs, turns, busy) instead; it reads every log, --all/--since trim the output.\n\
         --facts takes the machine's side (hostname, live processes, repositories) from FILE instead of this machine.\n\
         --machine DIR (repeated, with --model-json or --serve): one view over several machines' homes, DIR/{claude,codex,proc}.\n\
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
         --max-bytes caps each machine's copy (20G by default); a push past it gets 507."
.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_encrypted_flags_fail_before_default_source_discovery() {
        for flag in ["--remote", "--remote-config", "--remote-ca", "--tls-ca"] {
            let error = match parse_sessions_args(
                [flag, "/private/untouched"].map(str::to_owned).into_iter(),
            ) {
                Err(error) => error,
                Ok(_) => panic!("retired flag accepted: {flag}"),
            };
            assert!(error.contains("Encrypted Relay viewing is retired"));
            assert!(error.contains("semon-relay-history"));
            assert!(!usage().contains(flag));
        }
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
