use std::{
    env,
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    thread,
    time::Duration,
};

use semon_relay::{HttpTransport, PassReport, Sender, run_pass, serve};

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8734/v1/frames";

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("semon-relay: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    match arguments.next().as_deref() {
        Some("send") => run_send(arguments),
        Some("receive") => run_receive(arguments),
        Some("-h" | "--help") => Err(usage()),
        Some(command) => Err(format!("unknown command: {command}\n{}", usage())),
        None => Err(usage()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SendMode {
    Once,
    Follow,
}

fn run_send(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut mode = None;
    let mut projects = default_projects()?;
    let mut state = default_state()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut interval = Duration::from_secs(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--once" => set_mode(&mut mode, SendMode::Once)?,
            "--follow" => set_mode(&mut mode, SendMode::Follow)?,
            "--projects" => projects = value(&mut arguments, "--projects")?.into(),
            "--state" => state = value(&mut arguments, "--state")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--interval-ms" => {
                let raw = value(&mut arguments, "--interval-ms")?;
                let milliseconds = raw
                    .parse::<u64>()
                    .map_err(|_| format!("--interval-ms expects an integer, got {raw}"))?;
                if milliseconds == 0 {
                    return Err("--interval-ms must be greater than zero".into());
                }
                interval = Duration::from_millis(milliseconds);
            }
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown send argument: {argument}\n{}", usage())),
        }
    }
    let mode = mode.ok_or_else(|| format!("send requires --once or --follow\n{}", usage()))?;
    let transport =
        HttpTransport::new(endpoint, Duration::from_secs(10)).map_err(|error| error.to_string())?;
    match mode {
        SendMode::Once => {
            let report =
                run_pass(&projects, &state, &transport).map_err(|error| error.to_string())?;
            print_report(&report).map_err(|error| error.to_string())?;
            if report.had_failures() {
                Err("one or more streams remain unacknowledged; a later pass will retry".into())
            } else {
                Ok(())
            }
        }
        SendMode::Follow => {
            let mut backoff = Duration::from_secs(1);
            let mut sender = Sender::default();
            loop {
                let report = sender
                    .run_pass(&projects, &state, &transport)
                    .map_err(|error| error.to_string())?;
                print_report(&report).map_err(|error| error.to_string())?;
                if report.had_failures() {
                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                } else {
                    backoff = Duration::from_secs(1);
                    thread::sleep(interval);
                }
            }
        }
    }
}

fn run_receive(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut listen = None;
    let mut directory = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--listen" => {
                let raw = value(&mut arguments, "--listen")?;
                listen = Some(
                    raw.parse::<SocketAddr>()
                        .map_err(|error| format!("invalid --listen address {raw}: {error}"))?,
                );
            }
            "--dir" => directory = Some(PathBuf::from(value(&mut arguments, "--dir")?)),
            "-h" | "--help" => return Err(usage()),
            _ => {
                return Err(format!("unknown receive argument: {argument}\n{}", usage()));
            }
        }
    }
    let listen = listen.ok_or_else(|| "receive requires --listen".to_owned())?;
    let directory = directory.ok_or_else(|| "receive requires --dir".to_owned())?;
    serve(listen, &directory).map_err(|error| error.to_string())
}

fn print_report(report: &PassReport) -> io::Result<()> {
    for stream in &report.streams {
        println!(
            "relay stream session={} stream={} generation={} acked_lines={} acked_bytes={} \
             lag_p50_ms={} lag_p95_ms={} lag_max_ms={} backlog_lines={} backlog_bytes={}{}",
            stream.session,
            stream.stream,
            stream.generation,
            stream.lines_acked,
            stream.bytes_acked,
            millis(stream.lag.p50),
            millis(stream.lag.p95),
            millis(stream.lag.max),
            stream.backlog_lines,
            stream.backlog_bytes,
            stream
                .failure
                .as_ref()
                .map(|failure| format!(" failure={failure}"))
                .unwrap_or_default(),
        );
    }
    let (lines, bytes, backlog_lines, backlog_bytes) = report.totals();
    println!(
        "relay pass streams={} acked_lines={} acked_bytes={} lag_p50_ms={} lag_p95_ms={} \
         lag_max_ms={} backlog_lines={} backlog_bytes={} source_bytes_read={} \
         pass_duration_ms={}",
        report.streams.len(),
        lines,
        bytes,
        millis(report.lag.p50),
        millis(report.lag.p95),
        millis(report.lag.max),
        backlog_lines,
        backlog_bytes,
        report.source_bytes_read(),
        millis(report.pass_duration),
    );
    io::stdout().flush()
}

fn millis(duration: Duration) -> String {
    format!("{:.3}", duration.as_secs_f64() * 1_000.0)
}

fn set_mode(mode: &mut Option<SendMode>, value: SendMode) -> Result<(), String> {
    if mode.replace(value).is_some() {
        Err("choose exactly one of --once or --follow".into())
    } else {
        Ok(())
    }
}

fn value(arguments: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn default_projects() -> Result<PathBuf, String> {
    Ok(home()?.join(".claude/projects"))
}

fn default_state() -> Result<PathBuf, String> {
    let root = match env::var_os("XDG_STATE_HOME") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => home()?.join(".local/state"),
    };
    Ok(root.join("semon/relay.json"))
}

fn home() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".into())
}

fn usage() -> String {
    "Usage: semon-relay send (--once | --follow) [--projects PATH] [--state PATH] \
     [--endpoint http://127.0.0.1:PORT/v1/frames] [--interval-ms N]\n\
     Usage: semon-relay receive --listen 127.0.0.1:PORT --dir PATH\n\
     M1 sends plaintext and therefore accepts loopback IP endpoints only."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_requires_exactly_one_mode() {
        assert!(run_send(["--once", "--follow"].map(str::to_owned).into_iter()).is_err());
    }
}
