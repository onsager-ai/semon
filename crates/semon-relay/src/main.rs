use std::{
    collections::BTreeSet,
    env,
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    thread,
    time::Duration,
};

use semon_relay::{
    HttpTransport, MachineIdentity, PassReport, RECIPIENTS_FILE, RequestSigner, Sender,
    ServeConfig, TlsFiles, Transport, decrypt_envelope, encrypt_envelope, enroll_machine,
    enroll_recipient, init, load_age_identity, load_recipients, read_machine_identity,
    restore_session, restore_session_encrypted, serve_configured, takeover_session,
    takeover_session_encrypted, verify_encrypted_session,
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
        Some("keys") => run_keys(arguments),
        Some("verify") => run_verify(arguments),
        Some("restore") => run_restore(arguments),
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
    let mut projects_arg = None;
    let mut all = false;
    let mut sessions = BTreeSet::new();
    let mut state = default_state()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut machine = None;
    let mut config = default_config()?;
    let mut tls_ca = None;
    let mut insecure_plaintext = false;
    let mut interval = Duration::from_secs(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--once" => set_mode(&mut mode, SendMode::Once)?,
            "--follow" => set_mode(&mut mode, SendMode::Follow)?,
            "--projects" => {
                projects_arg = Some(PathBuf::from(value(&mut arguments, "--projects")?));
            }
            "--all" => all = true,
            "--session" => {
                sessions.insert(value(&mut arguments, "--session")?);
            }
            "--state" => state = value(&mut arguments, "--state")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--insecure-plaintext" => insecure_plaintext = true,
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
    // No implicit default root: shipping every real session by accident
    // (the incident this guard exists for) requires an explicit choice.
    let projects = match (projects_arg, all) {
        (Some(_), true) => {
            return Err(format!(
                "send accepts either --projects PATH or --all, not both\n{}",
                usage()
            ));
        }
        (Some(path), false) => path,
        (None, true) => default_projects()?,
        (None, false) => {
            return Err(format!(
                "send requires exactly one of --projects PATH or --all\n{}",
                usage()
            ));
        }
    };
    let (transport, machine, mut sender) = if insecure_plaintext {
        if tls_ca.is_some() {
            return Err("--insecure-plaintext does not accept --tls-ca".into());
        }
        let machine = machine_identity(machine)?;
        let transport = HttpTransport::new(endpoint, Duration::from_secs(10))
            .map_err(|error| error.to_string())?;
        (transport, machine.clone(), Sender::with_machine(machine))
    } else {
        if machine.is_some() {
            return Err("--machine is available only with --insecure-plaintext".into());
        }
        let identity = MachineIdentity::load(&config).map_err(|error| error.to_string())?;
        let machine = identity.fingerprint();
        let transport = HttpTransport::secure(
            endpoint,
            Duration::from_secs(10),
            RequestSigner::new(identity.signing.clone()),
            tls_ca.as_deref(),
        )
        .map_err(|error| error.to_string())?;
        let recipients =
            load_recipients(&config.join(RECIPIENTS_FILE)).map_err(|error| error.to_string())?;
        let sender = Sender::encrypted(machine.clone(), identity.age, recipients);
        (transport, machine, sender)
    };
    let _ = machine;
    if !sessions.is_empty() {
        sender.set_session_filter(sessions);
    }
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
    let mut config = default_config()?;
    let mut tls_ca = None;
    let mut insecure_plaintext = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--session" => session = Some(value(&mut arguments, "--session")?),
            "--force" => force = true,
            "--projects" => projects = Some(value(&mut arguments, "--projects")?.into()),
            "--state" => state = Some(value(&mut arguments, "--state")?.into()),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--insecure-plaintext" => insecure_plaintext = true,
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown lease argument: {argument}\n{}", usage())),
        }
    }
    let (transport, machine, identity) = transport_context(
        endpoint,
        &config,
        tls_ca.as_deref(),
        insecure_plaintext,
        machine,
    )?;
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
            let takeover = if let Some(identity) = identity {
                takeover_session_encrypted(
                    &projects,
                    &state,
                    &session,
                    &machine,
                    force,
                    &identity.age,
                    &transport,
                )
            } else {
                takeover_session(&projects, &state, &session, &machine, force, &transport)
            }
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
    let mut config = default_config()?;
    let mut tls_ca = None;
    let mut insecure_plaintext = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--insecure-plaintext" => insecure_plaintext = true,
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown orphans argument: {argument}\n{}", usage())),
        }
    }
    let (transport, machine, _) = transport_context(
        endpoint,
        &config,
        tls_ca.as_deref(),
        insecure_plaintext,
        machine,
    )?;
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
    let arguments = arguments.by_ref().collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "enroll")
    {
        return run_receive_enroll(arguments.into_iter().skip(1));
    }
    let mut arguments = arguments.into_iter();
    let mut listen = "127.0.0.1:8734"
        .parse::<SocketAddr>()
        .expect("default listen address is valid");
    let mut directory = None;
    let mut tls_cert = None;
    let mut tls_key = None;
    let mut insecure_plaintext = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--listen" => {
                let raw = value(&mut arguments, "--listen")?;
                listen = raw
                    .parse::<SocketAddr>()
                    .map_err(|error| format!("invalid --listen address {raw}: {error}"))?;
            }
            "--dir" => directory = Some(PathBuf::from(value(&mut arguments, "--dir")?)),
            "--tls-cert" => tls_cert = Some(PathBuf::from(value(&mut arguments, "--tls-cert")?)),
            "--tls-key" => tls_key = Some(PathBuf::from(value(&mut arguments, "--tls-key")?)),
            "--insecure-plaintext" => insecure_plaintext = true,
            "-h" | "--help" => return Err(usage()),
            _ => {
                return Err(format!("unknown receive argument: {argument}\n{}", usage()));
            }
        }
    }
    let directory = directory.ok_or_else(|| "receive requires --dir".to_owned())?;
    let tls = match (tls_cert, tls_key) {
        (Some(certificate), Some(private_key)) => Some(TlsFiles {
            certificate,
            private_key,
        }),
        (None, None) => None,
        _ => return Err("receive requires --tls-cert and --tls-key together".into()),
    };
    serve_configured(
        listen,
        &directory,
        ServeConfig {
            tls,
            insecure_plaintext,
        },
    )
    .map_err(|error| error.to_string())
}

fn run_receive_enroll(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let public_key = arguments
        .next()
        .ok_or_else(|| "receive enroll requires a signing public key".to_owned())?;
    let mut directory = None;
    let mut name = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dir" => directory = Some(PathBuf::from(value(&mut arguments, "--dir")?)),
            "--name" => name = Some(value(&mut arguments, "--name")?),
            _ => {
                return Err(format!(
                    "unknown receive enroll argument: {argument}\n{}",
                    usage()
                ));
            }
        }
    }
    let directory = directory.ok_or_else(|| "receive enroll requires --dir".to_owned())?;
    let name = name.ok_or_else(|| "receive enroll requires --name".to_owned())?;
    let fingerprint =
        enroll_machine(&directory, &public_key, &name).map_err(|error| error.to_string())?;
    println!("enrolled machine name={name} fingerprint={fingerprint}");
    Ok(())
}

fn run_keys(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let command = arguments.next().ok_or_else(|| {
        format!(
            "keys requires init, show, enroll, rewrap, or decrypt-envelope\n{}",
            usage()
        )
    })?;
    match command.as_str() {
        "init" => {
            let mut config = default_config()?;
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--config" => config = value(&mut arguments, "--config")?.into(),
                    _ => {
                        return Err(format!(
                            "unknown keys init argument: {argument}\n{}",
                            usage()
                        ));
                    }
                }
            }
            let outcome = init(&config).map_err(|error| error.to_string())?;
            println!(
                "keys initialized config={} age_created={} signing_created={}",
                config.display(),
                outcome.age_created,
                outcome.signing_created
            );
            Ok(())
        }
        "show" => {
            let mut config = default_config()?;
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--config" => config = value(&mut arguments, "--config")?.into(),
                    _ => {
                        return Err(format!(
                            "unknown keys show argument: {argument}\n{}",
                            usage()
                        ));
                    }
                }
            }
            let identity = MachineIdentity::load(&config).map_err(|error| error.to_string())?;
            println!("age {}", identity.age.to_public());
            println!(
                "signing {}",
                hex::encode(identity.signing.verifying_key().as_bytes())
            );
            println!("machine {}", identity.fingerprint());
            Ok(())
        }
        "enroll" => {
            let public_key = arguments
                .next()
                .ok_or_else(|| "keys enroll requires an age public key".to_owned())?;
            let mut config = default_config()?;
            let mut name = None;
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--config" => config = value(&mut arguments, "--config")?.into(),
                    "--name" => name = Some(value(&mut arguments, "--name")?),
                    _ => {
                        return Err(format!(
                            "unknown keys enroll argument: {argument}\n{}",
                            usage()
                        ));
                    }
                }
            }
            let name = name.ok_or_else(|| "keys enroll requires --name".to_owned())?;
            enroll_recipient(&config, &public_key, &name).map_err(|error| error.to_string())?;
            println!("enrolled recipient name={name}");
            Ok(())
        }
        "rewrap" => run_rewrap(arguments),
        "decrypt-envelope" => run_decrypt_envelope(arguments),
        _ => Err(format!("unknown keys command: {command}\n{}", usage())),
    }
}

fn run_rewrap(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut config = default_config()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut tls_ca = None;
    let mut session = None;
    let mut force = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--session" => session = Some(value(&mut arguments, "--session")?),
            "--force" => force = true,
            _ => {
                return Err(format!(
                    "unknown keys rewrap argument: {argument}\n{}",
                    usage()
                ));
            }
        }
    }
    let (transport, machine, identity) =
        transport_context(endpoint, &config, tls_ca.as_deref(), false, None)?;
    let identity = identity.expect("secure transport loads a machine identity");
    let recipients =
        load_recipients(&config.join(RECIPIENTS_FILE)).map_err(|error| error.to_string())?;
    let sessions = match session {
        Some(session) => vec![session],
        None => transport
            .list_envelope_sessions(&machine)
            .map_err(|error| error.to_string())?,
    };
    for session in &sessions {
        let envelope = transport
            .get_envelope(session, &machine)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("session {session} has no data-key envelope"))?;
        let key = decrypt_envelope(&envelope, &identity.age).map_err(|error| error.to_string())?;
        let replacement = encrypt_envelope(&key, &recipients).map_err(|error| error.to_string())?;
        transport
            .put_envelope(session, &machine, &replacement, true, force)
            .map_err(|error| error.to_string())?;
        println!("rewrapped session={session} forced={force}");
    }
    Ok(())
}

fn run_decrypt_envelope(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut config = default_config()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut tls_ca = None;
    let mut session = None;
    let mut recovery_identity = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--session" => session = Some(value(&mut arguments, "--session")?),
            "--identity" => {
                recovery_identity = Some(PathBuf::from(value(&mut arguments, "--identity")?))
            }
            _ => {
                return Err(format!(
                    "unknown keys decrypt-envelope argument: {argument}\n{}",
                    usage()
                ));
            }
        }
    }
    let session = session.ok_or_else(|| "keys decrypt-envelope requires --session".to_owned())?;
    let recovery_identity =
        recovery_identity.ok_or_else(|| "keys decrypt-envelope requires --identity".to_owned())?;
    let (transport, machine, _) =
        transport_context(endpoint, &config, tls_ca.as_deref(), false, None)?;
    let envelope = transport
        .get_envelope(&session, &machine)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("session {session} has no data-key envelope"))?;
    let identity = load_age_identity(&recovery_identity).map_err(|error| error.to_string())?;
    decrypt_envelope(&envelope, &identity).map_err(|error| error.to_string())?;
    println!(
        "envelope decrypted session={session} identity={}",
        recovery_identity.display()
    );
    Ok(())
}

fn run_verify(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut config = default_config()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut tls_ca = None;
    let mut session = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--session" => session = Some(value(&mut arguments, "--session")?),
            _ => return Err(format!("unknown verify argument: {argument}\n{}", usage())),
        }
    }
    let session = session.ok_or_else(|| "verify requires --session".to_owned())?;
    let (transport, machine, identity) =
        transport_context(endpoint, &config, tls_ca.as_deref(), false, None)?;
    let identity = identity.expect("secure transport loads a machine identity");
    let report = verify_encrypted_session(&session, &identity.age, &machine, &transport)
        .map_err(|error| error.to_string())?;
    println!(
        "verified session={} streams={} frames={}",
        session, report.streams, report.frames
    );
    Ok(())
}

fn run_restore(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut config = default_config()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut tls_ca = None;
    let mut session = None;
    let mut cwd = None;
    let mut projects = None;
    let mut state = None;
    let mut force = false;
    let mut allow_gaps = false;
    let mut json = false;
    let mut insecure_plaintext = false;
    let mut machine = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--session" => session = Some(value(&mut arguments, "--session")?),
            "--cwd" => cwd = Some(PathBuf::from(value(&mut arguments, "--cwd")?)),
            "--projects" => projects = Some(PathBuf::from(value(&mut arguments, "--projects")?)),
            "--state" => state = Some(PathBuf::from(value(&mut arguments, "--state")?)),
            "--force" => force = true,
            "--allow-gaps" => allow_gaps = true,
            "--json" => json = true,
            "--insecure-plaintext" => insecure_plaintext = true,
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown restore argument: {argument}\n{}", usage())),
        }
    }
    let session = session.ok_or_else(|| "restore requires --session".to_owned())?;
    let cwd = cwd.ok_or_else(|| "restore requires --cwd".to_owned())?;
    let projects = projects.map_or_else(default_projects, Ok)?;
    let state = state.map_or_else(default_state, Ok)?;
    let (transport, machine, identity) = transport_context(
        endpoint,
        &config,
        tls_ca.as_deref(),
        insecure_plaintext,
        machine,
    )?;
    let report = if let Some(identity) = identity {
        restore_session_encrypted(
            &projects,
            &state,
            &cwd,
            &session,
            &machine,
            force,
            allow_gaps,
            &identity.age,
            &transport,
        )
    } else {
        restore_session(
            &projects, &state, &cwd, &session, &machine, force, allow_gaps, &transport,
        )
    }
    .map_err(|error| error.to_string())?;
    if json {
        println!("{}", report.to_json());
    } else {
        println!("{}", report.to_text());
    }
    if report.incomplete {
        Err("restore is incomplete because one or more streams contain a gap".into())
    } else {
        Ok(())
    }
}

fn transport_context(
    endpoint: String,
    config: &std::path::Path,
    tls_ca: Option<&std::path::Path>,
    insecure_plaintext: bool,
    machine_override: Option<String>,
) -> Result<(HttpTransport, String, Option<MachineIdentity>), String> {
    if insecure_plaintext {
        if tls_ca.is_some() {
            return Err("--insecure-plaintext does not accept --tls-ca".into());
        }
        let machine = machine_identity(machine_override)?;
        let transport = HttpTransport::new(endpoint, Duration::from_secs(10))
            .map_err(|error| error.to_string())?;
        Ok((transport, machine, None))
    } else {
        if machine_override.is_some() {
            return Err("--machine is available only with --insecure-plaintext".into());
        }
        let identity = MachineIdentity::load(config).map_err(|error| error.to_string())?;
        let machine = identity.fingerprint();
        let transport = HttpTransport::secure(
            endpoint,
            Duration::from_secs(10),
            RequestSigner::new(identity.signing.clone()),
            tls_ca,
        )
        .map_err(|error| error.to_string())?;
        Ok((transport, machine, Some(identity)))
    }
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

fn default_config() -> Result<PathBuf, String> {
    let root = match env::var_os("XDG_CONFIG_HOME") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => home()?.join(".config"),
    };
    Ok(root.join("semon"))
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
    "Usage: semon-relay send (--once | --follow) (--projects PATH | --all) [--session S ...] \
     [--state PATH] [--endpoint URL] [--config PATH] [--tls-ca CERT] [--interval-ms N] \
     [--insecure-plaintext --machine ID]\n\
     Usage: semon-relay receive [--listen 127.0.0.1:8734] --dir PATH \
     [--tls-cert CERT --tls-key KEY] [--insecure-plaintext]\n\
     Usage: semon-relay receive enroll SIGNING_PUBKEY --name NAME --dir PATH\n\
     Usage: semon-relay lease status [--session S] [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Usage: semon-relay lease takeover --session S [--force] [--projects PATH] [--state PATH] \
     [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Usage: semon-relay orphans list [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Usage: semon-relay keys init|show [--config PATH]\n\
     Usage: semon-relay keys enroll AGE_RECIPIENT --name NAME [--config PATH]\n\
     Usage: semon-relay keys rewrap [--session S] [--force] [--endpoint URL] [--tls-ca CERT]\n\
     Usage: semon-relay keys decrypt-envelope --session S --identity PATH [--endpoint URL] [--tls-ca CERT]\n\
     Usage: semon-relay verify --session S [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Usage: semon-relay restore --session S --cwd PATH [--projects PATH] [--state PATH] [--force] [--allow-gaps] [--json] \
     [--endpoint URL] [--config PATH] [--tls-ca CERT] [--insecure-plaintext --machine ID]\n\
     Encrypted signed requests are the default; plaintext requires an explicit loopback-only flag."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_requires_exactly_one_mode() {
        assert!(run_send(["--once", "--follow"].map(str::to_owned).into_iter()).is_err());
    }

    #[test]
    fn send_requires_an_explicit_root() {
        assert!(run_send(["--once"].map(str::to_owned).into_iter()).is_err());
    }

    #[test]
    fn send_rejects_projects_and_all_together() {
        assert!(
            run_send(
                ["--once", "--projects", "/tmp/does-not-matter", "--all"]
                    .map(str::to_owned)
                    .into_iter()
            )
            .is_err()
        );
    }
}
