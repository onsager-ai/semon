use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, Write},
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
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
        Some("forget") => run_forget(arguments),
        Some("snapshot") => run_snapshot(arguments),
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
    let mut codex_sessions = None;
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
            "--codex-sessions" => {
                codex_sessions = Some(PathBuf::from(value(&mut arguments, "--codex-sessions")?))
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
    let codex_only = projects_arg.is_none() && codex_sessions.is_some() && !all;
    let projects = match (
        projects_arg.or_else(|| codex_sessions.clone().filter(|_| !all)),
        all,
    ) {
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
                "send requires --projects PATH, --codex-sessions PATH, or --all\n{}",
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
    if !codex_only {
        if let Some(root) = codex_sessions {
            sender.add_root(root);
        } else if all {
            let root = default_codex_sessions()?;
            if root.exists() {
                sender.add_root(root);
            }
        }
    }
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
    let mut harness = "claude".to_owned();
    let mut codex_sessions = None;
    let mut state = None;
    let mut force = false;
    let mut allow_gaps = false;
    let mut json = false;
    let mut insecure_plaintext = false;
    let mut machine = None;
    let mut memory_root = None;
    let mut memory_target = None;
    let mut memory_manifest = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--session" => session = Some(value(&mut arguments, "--session")?),
            "--memory-root-id" => memory_root = Some(value(&mut arguments, "--memory-root-id")?),
            "--memory-target" => {
                memory_target = Some(PathBuf::from(value(&mut arguments, "--memory-target")?))
            }
            "--memory-manifest" => {
                memory_manifest = Some(value(&mut arguments, "--memory-manifest")?)
            }
            "--cwd" => cwd = Some(PathBuf::from(value(&mut arguments, "--cwd")?)),
            "--harness" => harness = value(&mut arguments, "--harness")?,
            "--codex-sessions" => {
                codex_sessions = Some(PathBuf::from(value(&mut arguments, "--codex-sessions")?))
            }
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
    let projects = match harness.as_str() {
        "claude" if codex_sessions.is_none() => projects.map_or_else(default_projects, Ok)?,
        "codex" if projects.is_none() => codex_sessions.map_or_else(default_codex_sessions, Ok)?,
        _ => return Err("restore accepts --harness claude with --projects, or --harness codex with --codex-sessions".into()),
    };
    validate_memory_selection(
        memory_root.as_deref(),
        memory_target.as_deref(),
        memory_manifest.as_deref(),
    )?;
    if memory_root.is_some() && insecure_plaintext {
        return Err("memory recovery requires an enrolled encrypted machine".into());
    }
    let state = state.map_or_else(default_state, Ok)?;
    let (transport, machine, identity) = transport_context(
        endpoint,
        &config,
        tls_ca.as_deref(),
        insecure_plaintext,
        machine,
    )?;
    let tips = transport
        .lease_tips(&session, &machine)
        .map_err(|error| error.to_string())?;
    if tips
        .tips
        .iter()
        .any(|tip| tip.stream.starts_with("codex/") != (harness == "codex"))
    {
        return Err("receiver streams do not match requested --harness".into());
    }
    let report = if let Some(ref identity) = identity {
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
    let memory = if let (Some(root), Some(target)) = (&memory_root, &memory_target) {
        identity
            .as_ref()
            .ok_or_else(|| "memory recovery requires encrypted identity".to_owned())
            .and_then(|identity| {
                semon_relay::restore_snapshot_for_session(
                    root,
                    memory_manifest.as_deref(),
                    target,
                    &machine,
                    &identity.age,
                    &transport,
                    Some(&session),
                )
                .map_err(|error| error.to_string())
            })
            .map(|report| serde_json::json!({"status":"restored","root":root,"report":report}))
    } else {
        Ok(serde_json::json!({"status":"not_selected"}))
    };
    let memory_error = memory.as_ref().err().cloned();
    let memory = memory.unwrap_or_else(|error| serde_json::json!({"status":"failed","root":memory_root,"target":memory_target,"error":error}));
    let mut output = report.to_json();
    output["recovery_status"] = serde_json::json!({"memory":memory,"sidecars":"only explicitly selected memory roots are restored; all other sidecars are omitted"});
    if memory_error.is_some() {
        output["incomplete"] = true.into();
        output["next_step"] = serde_json::Value::Null;
    }
    if json {
        println!("{output}");
    } else {
        let mut text = report.to_text();
        if memory_error.is_some()
            && let Some(command) = report.next_step()
        {
            text = text
                .lines()
                .filter(|line| *line != command)
                .collect::<Vec<_>>()
                .join("\n");
        }
        println!(
            "{text}\nmemory recovery: {}\nsidecars: only explicitly selected roots; all other sidecars omitted",
            output["recovery_status"]["memory"]
        );
    }
    if let Some(error) = memory_error {
        Err(format!(
            "session files were restored, but memory recovery failed: {error}; review partial recovery before resuming"
        ))
    } else if report.incomplete {
        Err("restore is incomplete because one or more streams contain a gap".into())
    } else {
        Ok(())
    }
}

fn validate_memory_selection(
    root: Option<&str>,
    target: Option<&std::path::Path>,
    manifest: Option<&str>,
) -> Result<(), String> {
    if root.is_some() != target.is_some() || (manifest.is_some() && root.is_none()) {
        return Err("memory recovery requires both --memory-root-id and --memory-target; --memory-manifest is optional".into());
    }
    for id in root.into_iter().chain(manifest) {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("memory identifiers must be lowercase SHA-256".into());
        }
    }
    if let Some(target) = target {
        match fs::symlink_metadata(target) {
            Ok(_) => {
                return Err(
                    "memory recovery requires a new target; existing paths are never overwritten"
                        .into(),
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
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

fn run_forget(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut selector = semon_relay::ForgetSelector::default();
    let mut state = default_state()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut config = default_config()?;
    let mut machine = None;
    let mut tls_ca = None;
    let mut insecure_plaintext = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--session" => selector.session = Some(value(&mut arguments, "--session")?),
            "--before" => selector.before_ns = Some(semon_relay::forget_before_day(&value(&mut arguments, "--before")?)?),
            "--memory" => {
                let root = PathBuf::from(value(&mut arguments, "--memory")?);
                if root.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
                    return Err("--memory root must not contain ..".into());
                }
                let root = if root.is_absolute() { root } else { env::current_dir().map_err(|error| error.to_string())?.join(root) };
                let label = root.to_str().ok_or("--memory root is not UTF-8")?;
                use sha2::{Digest, Sha256};
                selector.memory_root = Some(hex::encode(Sha256::digest(label.as_bytes())));
            }
            "--memory-id" => selector.memory_root = Some(value(&mut arguments, "--memory-id")?),
            "--trace" => return Err("--trace selects semantic content; it cannot select replicated carrier frames and does not delete server copies".into()),
            "--state" => state = value(&mut arguments, "--state")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--machine" => machine = Some(value(&mut arguments, "--machine")?),
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--insecure-plaintext" => insecure_plaintext = true,
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown forget argument: {argument}\n{}", usage())),
        }
    }
    selector.validate().map_err(|error| error.to_string())?;
    let (transport, machine, _) = transport_context(
        endpoint,
        &config,
        tls_ca.as_deref(),
        insecure_plaintext,
        machine,
    )?;
    let id = semon_relay::queue_forget(&state, &transport.deletion_scope(), &selector)
        .map_err(|error| error.to_string())?;
    let result = semon_relay::flush_forgets(&state, &machine, &transport)
        .map_err(|error| error.to_string())?;
    println!(
        "forget id={id} acknowledged={} pending_on_server={}",
        result.acknowledged, result.pending
    );
    if result.pending > 0 {
        println!("pending on server; the sender retries until acknowledged");
        for failure in result.failures {
            eprintln!("semon-relay: {failure}");
        }
    }
    println!("Carrier files and copies another machine already restored are not deleted.");
    Ok(())
}

fn print_report(report: &PassReport) -> io::Result<()> {
    if report.deletions.acknowledged > 0 || report.deletions.pending > 0 {
        println!(
            "relay deletions acknowledged={} pending_on_server={}",
            report.deletions.acknowledged, report.deletions.pending
        );
        for failure in &report.deletions.failures {
            eprintln!("relay deletion pending: {failure}");
        }
    }
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

fn default_codex_sessions() -> Result<PathBuf, String> {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .map(|home| home.join("sessions"))
        .ok_or_else(|| "CODEX_HOME or HOME is required".into())
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
    "Usage: semon-relay send (--once | --follow) (--projects PATH | --codex-sessions PATH | --all) [--session S ...] \
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
     Usage: semon-relay forget (--session S | --before YYYY-MM-DD | --memory ROOT | --memory-id ID) [--state PATH] [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Usage: semon-relay restore [--harness claude|codex] [--codex-sessions PATH] --session S --cwd PATH [--projects PATH] [--state PATH] [--force] [--allow-gaps] [--json] [--memory-root-id SHA256 --memory-target NEW_DIR [--memory-manifest SHA256]] \
     [--endpoint URL] [--config PATH] [--tls-ca CERT] [--insecure-plaintext --machine ID]\n\
     Usage: semon-relay snapshot capture|follow --root PATH [--root-id SHA256] [--parent SHA256] [--session SESSION] [--epoch N] [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Usage: semon-relay snapshot list --root-id SHA256 [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Usage: semon-relay snapshot restore --root-id SHA256 --target NEW_DIR [--manifest SHA256] [--endpoint URL] [--config PATH] [--tls-ca CERT]\n\
     Encrypted signed requests are the default; plaintext requires an explicit loopback-only flag."
        .into()
}

fn run_snapshot(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    use semon_relay::{
        SnapshotCache, inspect_snapshot_manifest, list_snapshot_history, load_snapshot_packet,
        persist_snapshot_packet, publish_snapshot, restore_snapshot, snapshot_root_id,
    };
    use serde_json::json;
    let action = arguments
        .next()
        .ok_or_else(|| "snapshot requires capture, follow, list or restore".to_owned())?;
    if !matches!(action.as_str(), "capture" | "follow" | "list" | "restore") {
        return Err("unknown snapshot action".into());
    }
    let mut config = default_config()?;
    let mut endpoint = DEFAULT_ENDPOINT.to_owned();
    let mut tls_ca = None;
    let mut root = None;
    let mut root_id = None;
    let mut parent = None;
    let mut selected = None;
    let mut target = None;
    let mut epoch = None;
    let mut session = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = value(&mut arguments, "--config")?.into(),
            "--endpoint" => endpoint = value(&mut arguments, "--endpoint")?,
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(&mut arguments, "--tls-ca")?)),
            "--root" => root = Some(PathBuf::from(value(&mut arguments, "--root")?)),
            "--root-id" => root_id = Some(value(&mut arguments, "--root-id")?),
            "--parent" => parent = Some(value(&mut arguments, "--parent")?),
            "--manifest" => selected = Some(value(&mut arguments, "--manifest")?),
            "--target" => target = Some(PathBuf::from(value(&mut arguments, "--target")?)),
            "--session" => session = Some(value(&mut arguments, "--session")?),
            "--epoch" => {
                epoch = Some(
                    value(&mut arguments, "--epoch")?
                        .parse::<u64>()
                        .map_err(|_| "invalid epoch")?,
                )
            }
            _ => return Err(format!("unknown snapshot argument: {argument}")),
        }
    }
    let root = match root {
        Some(path) if path.is_absolute() => Some(path),
        Some(path) => Some(
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(path),
        ),
        None => None,
    };
    let root_id = root_id
        .or_else(|| {
            root.as_ref()
                .map(|p| snapshot_root_id(&p.to_string_lossy()))
        })
        .ok_or_else(|| "snapshot requires --root or --root-id".to_owned())?;
    let scope = snapshot_root_id(&endpoint);
    let (transport, machine, identity) =
        transport_context(endpoint, &config, tls_ca.as_deref(), false, None)?;
    let identity =
        identity.ok_or_else(|| "snapshots require an enrolled encrypted machine".to_owned())?;
    if action == "list" {
        let list =
            list_snapshot_history(&root_id, &machine, &transport).map_err(|e| e.to_string())?;
        // List public history only; ciphertext need not fill the terminal.
        let manifests=list["manifests"].as_array().ok_or_else(||"invalid history".to_owned())?.iter().map(|m|json!({"id":m["id"],"machine":m["machine"],"parent":m["parent"],"taken_at_wall":m["taken_at_wall"]})).collect::<Vec<_>>();
        println!(
            "{}",
            json!({"root":root_id,"heads":list["heads"],"manifests":manifests})
        );
        return Ok(());
    }
    if action == "restore" {
        let target = target
            .ok_or_else(|| "snapshot restore requires --target (a new directory)".to_owned())?;
        let report = restore_snapshot(
            &root_id,
            selected.as_deref(),
            &target,
            &machine,
            &identity.age,
            &transport,
        )
        .map_err(|e| e.to_string())?;
        println!("{report}");
        return Ok(());
    }
    let root = root.ok_or_else(|| "snapshot capture/follow requires --root".to_owned())?;
    let recipients = load_recipients(&config.join(RECIPIENTS_FILE)).map_err(|e| e.to_string())?;
    let outbox = config.join("snapshot-outbox").join(scope);
    fs::create_dir_all(&outbox).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&outbox, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let pending = outbox.join(format!("{root_id}.json"));

    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_else(|_| hex::encode(semon_relay::generate_data_key()));
    let mut cache = SnapshotCache::default();
    loop {
        let pass = (|| -> Result<(), String> {
            if pending.exists() {
                let packet = load_snapshot_packet(&pending).map_err(|e| e.to_string())?;
                if packet.manifest["root"] != root_id || packet.manifest["machine"] != machine {
                    return Err("pending snapshot scope mismatch".into());
                }
                publish_snapshot(&packet, &transport).map_err(|e| e.to_string())?;
                fs::remove_file(&pending).map_err(|e| e.to_string())?;
            }
            let list =
                list_snapshot_history(&root_id, &machine, &transport).map_err(|e| e.to_string())?;
            let manifests = list["manifests"]
                .as_array()
                .ok_or_else(|| "invalid snapshot history".to_owned())?;
            let previous = if let Some(ref id) = parent {
                manifests.iter().find(|m| m["id"] == *id)
            } else {
                manifests.iter().rev().find(|m| m["machine"] == machine)
            };
            if parent.is_some() && previous.is_none() {
                return Err("explicit parent is missing from this root".into());
            }
            let wall = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis() as u64;
            let associated_epoch = if let Some(ref session) = session {
                let status = transport
                    .lease_status(Some(session), &machine)
                    .map_err(|error| error.to_string())?;
                Some(snapshot_associated_epoch(
                    &status,
                    session,
                    &machine,
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|error| error.to_string())?
                        .as_millis() as u64,
                    epoch,
                )?)
            } else {
                epoch
            };
            let packet = cache
                .capture(
                    &root,
                    &root_id,
                    &machine,
                    previous.and_then(|m| m["id"].as_str()),
                    wall,
                    snapshot_monotonic_ms()?,
                    boot.trim(),
                    associated_epoch,
                    session.as_deref(),
                    &recipients,
                )
                .map_err(|e| e.to_string())?;
            if let Some(previous) = previous {
                let previous_id = previous["id"].clone();
                let previous=transport.snapshot_request("/v1/snapshots/get",&json!({"root":root_id,"machine":machine,"kind":"manifest","id":previous["id"]})).map_err(|e|e.to_string())?;
                if previous["id"] != previous_id || previous["root"] != root_id {
                    return Err("previous snapshot substitution".into());
                }
                let old = inspect_snapshot_manifest(&previous, &identity.age)
                    .map_err(|e| e.to_string())?;
                let new = inspect_snapshot_manifest(&packet.manifest, &identity.age)
                    .map_err(|e| e.to_string())?;
                if old["entries"] == new["entries"]
                    && old["session"] == new["session"]
                    && old["epoch"] == new["epoch"]
                {
                    return Ok(());
                }
            }
            persist_snapshot_packet(&pending, &packet).map_err(|e| e.to_string())?;
            publish_snapshot(&packet, &transport).map_err(|e| e.to_string())?;
            fs::remove_file(&pending).map_err(|e| e.to_string())?;
            println!(
                "{}",
                json!({"root":root_id,"manifest":packet.manifest["id"],"blobs":packet.blobs.len()})
            );
            parent = None;
            Ok(())
        })();
        if action == "capture" {
            return pass;
        }
        if let Err(error) = pass {
            eprintln!("snapshot pending: {error}");
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn snapshot_associated_epoch(
    status: &semon_relay::LeaseStatus,
    session: &str,
    machine: &str,
    now: u64,
    expected: Option<u64>,
) -> Result<u64, String> {
    let row = status
        .rows
        .iter()
        .find(|row| row.session == session)
        .ok_or_else(|| "associated session has no lease".to_owned())?;
    if row.holder_machine != machine || row.lease_expires_at_ms <= now {
        return Err("associated session lease is not currently held by this machine".into());
    }
    if expected.is_some_and(|epoch| epoch != row.epoch) {
        return Err("--epoch disagrees with observed session lease".into());
    }
    Ok(row.epoch)
}

fn snapshot_monotonic_ms() -> Result<u64, String> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok((value.tv_sec as u64).saturating_mul(1000) + (value.tv_nsec as u64) / 1_000_000)
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
    #[test]
    fn memory_selection_requires_explicit_new_target_and_valid_ids() {
        let root = "a".repeat(64);
        assert!(validate_memory_selection(Some(&root), None, None).is_err());
        assert!(validate_memory_selection(None, None, Some(&root)).is_err());
        assert!(
            validate_memory_selection(Some("wrong"), Some(std::path::Path::new("/missing")), None)
                .is_err()
        );
        let existing = std::env::temp_dir();
        assert!(validate_memory_selection(Some(&root), Some(&existing), None).is_err());
        let new = existing.join(format!(
            "semon-new-memory-{}",
            hex::encode(semon_relay::generate_data_key())
        ));
        assert!(validate_memory_selection(Some(&root), Some(&new), Some(&root)).is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("missing", &new).unwrap();
            assert!(validate_memory_selection(Some(&root), Some(&new), None).is_err());
            fs::remove_file(new).unwrap();
        }
    }
    #[test]
    fn snapshot_association_requires_current_holder_and_observed_epoch() {
        let status = semon_relay::LeaseStatus {
            rows: vec![semon_relay::LeaseRow {
                session: "session".into(),
                epoch: 7,
                holder_machine: "machine".into(),
                lease_expires_at_ms: 100,
            }],
            takeovers: vec![],
        };
        assert_eq!(
            snapshot_associated_epoch(&status, "session", "machine", 99, None).unwrap(),
            7
        );
        assert_eq!(
            snapshot_associated_epoch(&status, "session", "machine", 99, Some(7)).unwrap(),
            7
        );
        assert!(snapshot_associated_epoch(&status, "session", "other", 99, None).is_err());
        assert!(snapshot_associated_epoch(&status, "session", "machine", 100, None).is_err());
        assert!(snapshot_associated_epoch(&status, "session", "machine", 99, Some(6)).is_err());
        assert!(snapshot_associated_epoch(&status, "missing", "machine", 99, None).is_err());
    }
}
