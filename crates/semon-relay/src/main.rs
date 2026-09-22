use std::{
    env,
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    thread,
    time::Duration,
};

use semon_relay::{
    HttpTransport, PassReport, Sender, Transport, read_machine_identity, serve, takeover_session,
};

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
        Some("lease") => run_lease(arguments),
        Some("orphans") => run_orphans(arguments),
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
    let mut machine = None;
    let mut interval = Duration::from_secs(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--once" => set_mode(&mut mode, SendMode::Once)?,
            "--follow" => set_mode(&mut mode, SendMode::Follow)?,
            "--projects" => projects = value(&mut arguments, "--projects")?.into(),
            "--state" => state = value(&mut arguments, "--state")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
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
    let machine = machine_identity(machine)?;
    let mut sender = Sender::with_machine(machine);
    match mode {
        SendMode::Once => {
            let report = sender
                .run_pass(&projects, &state, &transport)
                .map_err(|error| error.to_string())?;
            print_report(&report).map_err(|error| error.to_string())?;
            if report.had_failures() {
                Err("one or more streams failed or were fenced; source state was retained".into())
            } else {
                Ok(())
            }
        }
        SendMode::Follow => {
            let mut backoff = Duration::from_secs(1);
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

fn run_lease(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let command = arguments
        .next()
        .ok_or_else(|| format!("lease requires status or takeover\n{}", usage()))?;
    let mut session = None;
    let mut force = false;
    let mut projects = None;
    let mut state = None;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut machine = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--session" => session = Some(value(&mut arguments, "--session")?),
            "--force" => force = true,
            "--projects" => projects = Some(value(&mut arguments, "--projects")?.into()),
            "--state" => state = Some(value(&mut arguments, "--state")?.into()),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown lease argument: {argument}\n{}", usage())),
        }
    }
    let machine = machine_identity(machine)?;
    let transport =
        HttpTransport::new(endpoint, Duration::from_secs(10)).map_err(|error| error.to_string())?;
    match command.as_str() {
        "status" => {
            if force {
                return Err("lease status does not accept --force".into());
            }
            let status = transport
                .lease_status(session.as_deref(), &machine)
                .map_err(|error| error.to_string())?;
            for row in status.rows {
                println!(
                    "lease session={} epoch={} holder={} expires_at_ms={}",
                    row.session, row.epoch, row.holder_machine, row.lease_expires_at_ms
                );
            }
            for record in status.takeovers {
                println!(
                    "takeover session={} previous_epoch={} epoch={} previous_holder={} holder={} forced={} taken_at_ms={}",
                    record.session,
                    record.previous_epoch,
                    record.epoch,
                    record.previous_holder,
                    record.holder_machine,
                    record.forced,
                    record.taken_at_ms
                );
            }
            Ok(())
        }
        "takeover" => {
            let session = session.ok_or_else(|| "lease takeover requires --session".to_owned())?;
            let projects = projects.map_or_else(default_projects, Ok)?;
            let state = state.map_or_else(default_state, Ok)?;
            let takeover =
                takeover_session(&projects, &state, &session, &machine, force, &transport)
                    .map_err(|error| error.to_string())?;
            println!(
                "takeover session={} epoch={} holder={} forced={} streams={}",
                session,
                takeover.row.epoch,
                takeover.row.holder_machine,
                force,
                takeover.tips.len()
            );
            Ok(())
        }
        _ => Err(format!("unknown lease command: {command}\n{}", usage())),
    }
}

fn run_orphans(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    match arguments.next().as_deref() {
        Some("list") => {}
        Some(command) => return Err(format!("unknown orphans command: {command}\n{}", usage())),
        None => return Err(format!("orphans requires list\n{}", usage())),
    }
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut machine = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown orphans argument: {argument}\n{}", usage())),
        }
    }
    let machine = machine_identity(machine)?;
    let transport =
        HttpTransport::new(endpoint, Duration::from_secs(10)).map_err(|error| error.to_string())?;
    for orphan in transport
        .list_orphans(&machine)
        .map_err(|error| error.to_string())?
    {
        println!(
            "orphan session={} stream={} generation={} fenced_epoch={} frames={} first_seq={} last_seq={}",
            orphan.session,
            orphan.stream,
            orphan.generation,
            orphan.fenced_epoch,
            orphan.frames,
            orphan.first_seq,
            orphan.last_seq
        );
    }
    Ok(())
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
            "relay stream session={} stream={} generation={} epoch={} fenced={} acked_lines={} \
             orphan_acked_lines={} acked_bytes={} lag_p50_ms={} lag_p95_ms={} lag_max_ms={} \
             backlog_lines={} backlog_bytes={}{}",
            stream.session,
            stream.stream,
            stream.generation,
            stream.epoch,
            stream.fenced,
            stream.lines_acked,
            stream.orphan_lines_acked,
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

fn machine_identity(override_value: Option<String>) -> Result<String, String> {
    match override_value {
        Some(machine) if machine.trim().is_empty() => Err("--machine must not be empty".into()),
        Some(machine) => Ok(machine),
        None => read_machine_identity().map_err(|error| error.to_string()),
    }
}

fn usage() -> String {
    "Usage: semon-relay send (--once | --follow) [--projects PATH] [--state PATH] \
     [--endpoint http://127.0.0.1:PORT/v1/frames] [--machine ID] [--interval-ms N]\n\
     Usage: semon-relay receive --listen 127.0.0.1:PORT --dir PATH\n\
     Usage: semon-relay lease status [--session S] [--endpoint URL] [--machine ID]\n\
     Usage: semon-relay lease takeover --session S [--force] [--projects PATH] [--state PATH] \
     [--endpoint URL] [--machine ID]\n\
     Usage: semon-relay orphans list [--endpoint URL] [--machine ID]\n\
     The relay sends plaintext and therefore accepts loopback IP endpoints only."
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
