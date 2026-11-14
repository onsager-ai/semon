//! Exercise real controlling-terminal confirmation independently of stdin.
#![cfg(unix)]
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Command, Output, Stdio},
    thread,
};

const URL: &str = "https://receiver.example:443";
fn install(base: &Path) {
    use semon_push::{
        credentials::Kind, enrollment::Binding, pairing::ActivationInput, provision_wire::Activated,
    };
    let private = semon_push::provision_wire::decode_fixed::<138>("MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgL_E8qFEyPcpHM_jv-bQj0alxU3tkrY_1OSwOWVOwhs-hRANCAATA08frMi95YZiy3BekFDqwQHAS2SIpv7yA1ckqs3drjLLJqCyFKRx7VY_MJawLw_RbjwOyGKFasQHvnfrO1MUQ").unwrap();
    semon_push::origin_custody::install(
        base,
        URL,
        "laptop",
        &ActivationInput {
            receiver_id: "receiver".into(),
            machine: "laptop".into(),
            binding: Binding::Inapplicable,
            kinds: vec![(Kind::MirrorPush, false)],
            client_certificate_pins: vec![],
            valid_until: u64::MAX,
        },
        &Activated {
            version: 1,
            enrollment_id: "enrollment".into(),
            credentials: vec![(Kind::MirrorPush, "private-bearer".into())],
            client_certificate: None,
        },
        &private,
        &[],
    )
    .unwrap();
}
fn command(config: &Path, receiver: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_semon"));
    command
        .args(["disconnect", "--receiver", receiver])
        .env("XDG_CONFIG_HOME", config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn terminal(mut command: Command, answer: Option<&'static [u8]>) -> (Output, String) {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty initializes two fresh owned descriptors.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let mut master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let slave_fd = slave.as_raw_fd();
    // SAFETY: only async-signal-safe libc calls execute before exec; descriptor
    // remains live through spawn. Child becomes a session leader with this tty.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 || libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    drop(slave);
    let reader = thread::spawn(move || {
        let mut transcript = Vec::new();
        let mut answered = false;
        loop {
            let mut byte = [0];
            match master.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => transcript.extend_from_slice(&byte),
            }
            if !answered && transcript.ends_with(b"Type yes to confirm: ") {
                answered = true;
                // Ctrl-D is EOF on the canonical terminal; no newline is injected.
                master.write_all(answer.unwrap_or(b"\x04")).unwrap();
            }
        }
        String::from_utf8(transcript).unwrap().replace("\r\n", "\n")
    });
    let output = child.wait_with_output().unwrap();
    (output, reader.join().unwrap())
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn disconnect_cancel_confirm_repeat_unknown_and_no_terminal() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("semon/origin");
    install(&base);
    let prompt = "Disconnect receiver receiver.example:443 on this machine? Type yes to confirm: ";
    for answer in [
        Some(b"no\n".as_slice()),
        Some(b" yes\n".as_slice()),
        Some(b"YES\n".as_slice()),
        None,
    ] {
        let (output, tty) = terminal(command(temp.path(), "https://RECEIVER.example/"), answer);
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(tty.starts_with(prompt), "{tty}");
        assert!(
            tty.ends_with("Disconnect cancelled; nothing changed.\n"),
            "{tty}"
        );
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert!(semon_push::origin_custody::load(&base, Some(URL)).is_ok());
    }
    // A pipe with literal yes is never a controlling terminal.
    let mut without_tty = command(temp.path(), URL);
    unsafe {
        without_tty.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = without_tty.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "semon: disconnect requires the local owner's terminal; run this command there.\n"
    );
    assert!(output.stdout.is_empty());
    let (output, tty) = terminal(
        command(temp.path(), "https://unknown.example"),
        Some(b"yes\n"),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(tty.is_empty());
    assert_eq!(
        text(&output.stderr),
        "semon: receiver unknown.example:443 was not found; use the HTTPS URL given to connect as --receiver URL, then retry.\n"
    );
    let (output, tty) = terminal(command(temp.path(), URL), Some(b"yes\n"));
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(tty, format!("{prompt}yes\nConfirmation received.\n"));
    let success = "Disconnected receiver receiver.example:443 on this machine. History, keys and audits kept. Receiver access was not revoked.\n";
    assert_eq!(text(&output.stdout), success);
    assert!(output.stderr.is_empty());
    let (output, tty) = terminal(command(temp.path(), "https://receiver.example/"), None);
    assert!(output.status.success());
    assert_eq!(text(&output.stdout), success);
    assert!(tty.is_empty(), "repeat must not prompt: {tty}");
    assert!(output.stderr.is_empty());
    assert!(semon_push::origin_custody::load(&base, Some(URL)).is_err());
}

#[test]
fn all_receiver_commands_refuse_secret_url_parts_without_echoing_them() {
    for url in [
        "https://user:super-secret@receiver.example",
        "https://receiver.example?super-secret",
    ] {
        for args in [
            vec!["connect", "--receiver", url, "--machine", "laptop"],
            vec!["push", "--receiver", url],
            vec!["disconnect", "--receiver", url],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_semon"))
                .args(args)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert!(!text(&output.stderr).contains("super-secret"));
            assert!(output.stdout.is_empty());
        }
    }
}
