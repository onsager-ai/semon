use semon_push::{
    credentials::Kind,
    enrollment::{self, OwnerAction, OwnerState, Registry, Store},
};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, IsTerminal, Write},
    path::Path,
};

pub(super) fn confirm(terminal: &mut File, prompt: &str) -> Result<bool, String> {
    write!(terminal, "{prompt}").map_err(|e| e.to_string())?;
    terminal.flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::BufReader::new(terminal.try_clone().map_err(|e| e.to_string())?)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line
        .strip_suffix('\n')
        .unwrap_or(&line)
        .strip_suffix('\r')
        .unwrap_or(line.strip_suffix('\n').unwrap_or(&line))
        == "yes")
}
fn now() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|n| n.as_secs())
        .map_err(|e| e.to_string())
}
fn custody_error(error: enrollment::Error, dir: &Path, fallback: &str) -> String {
    match error {
        enrollment::Error::UnsupportedVersion => {
            enrollment::UNSUPPORTED_FORMAT.replace("DIR", &dir.display().to_string())
        }
        enrollment::Error::Full => {
            enrollment::STATE_FULL.replace("DIR", &dir.display().to_string())
        }
        enrollment::Error::AuditUnavailable => {
            enrollment::AUDIT_UNAVAILABLE.replace("DIR", &dir.display().to_string())
        }
        _ => fallback.into(),
    }
}
pub(super) fn run(
    dir: &Path,
    machine: &str,
    kind_text: Option<&str>,
    action: OwnerAction,
) -> Result<(), String> {
    let revoke = action == OwnerAction::Revoke;
    let owner_error = if revoke {
        "revocation requires the receiver owner's terminal; run this command there."
    } else {
        "enable requires the receiver owner's terminal; run this command there."
    };
    let uncertain = if revoke {
        "revocation could not be confirmed; rerun the same command to check its earlier outcome before retrying."
    } else {
        "enable could not be confirmed; rerun the same command to check its earlier outcome before retrying."
    };
    let custody = semon_push::receiver_custody::ReceiverCustody::open_existing(dir)
        .map_err(|_| owner_error.to_owned())?;
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| owner_error.to_owned())?;
    if !terminal.is_terminal() {
        return Err(owner_error.into());
    }
    let parsed_kind = kind_text.and_then(Kind::parse);
    let unknown_kind = || {
        if revoke {
            format!(
                "kind {} was not found for machine {machine}; check --kind, then retry revocation.",
                kind_text.unwrap_or("")
            )
        } else {
            format!(
                "enable refused; machine {machine} or kind {} was not found. Check --machine and --kind, then retry.",
                kind_text.unwrap_or("")
            )
        }
    };
    if kind_text.is_some() && parsed_kind.is_none() {
        return Err(unknown_kind());
    }
    let target = kind_text.map_or_else(
        || format!("machine {machine}"),
        |k| format!("kind {k} for machine {machine}"),
    );
    let success = |endpoint: &str| {
        if revoke {
            format!("Revoked {target}. History and audits kept.")
        } else {
            format!(
                "Re-enable approved for kind {} on machine {machine}; waiting for origin renewal. A running watch will renew automatically; otherwise run semon push --receiver {endpoint} on the origin.",
                kind_text.unwrap_or("")
            )
        }
    };
    let registry = Registry::read_current(dir).map_err(|e| custody_error(e, dir, uncertain))?;
    let state = registry.owner_state(machine, parsed_kind, action, now()?);
    let id = match state {
        OwnerState::Complete { endpoint } => {
            println!("{}", success(&endpoint));
            return Ok(());
        }
        OwnerState::UnknownMachine => {
            return Err(if revoke {
                format!("machine {machine} was not found; check --machine, then retry revocation.")
            } else {
                unknown_kind()
            });
        }
        OwnerState::UnknownKind => return Err(unknown_kind()),
        OwnerState::LastKind => {
            return Err(format!(
                "kind {} is the last enabled kind; revoke the enrollment with semon receive revoke --dir {} --machine {machine} instead.",
                kind_text.unwrap_or(""),
                dir.display()
            ));
        }
        OwnerState::AlreadyEnabled => {
            return Err(format!(
                "kind {} is already enabled for machine {machine}; check the kind before retrying.",
                kind_text.unwrap_or("")
            ));
        }
        OwnerState::NoAuthority { endpoint } => {
            return Err(format!(
                "enable refused; no valid access remains.\nTo restore access, pair again: on the receiver, run semon receive enroll --machine {machine} with its listener/TLS options and confirm replacement if prompted; on the origin, run semon connect --to {endpoint}, enter the code, and approve replacement."
            ));
        }
        OwnerState::Confirm { enrollment_id, .. } => enrollment_id,
    };
    let prompt = if revoke {
        format!("Revoke {target}? This stops access. Type yes to confirm: ")
    } else {
        format!(
            "Allow fresh access for kind {} on machine {machine}? Type yes to confirm: ",
            kind_text.unwrap_or("")
        )
    };
    if !confirm(&mut terminal, &prompt)? {
        writeln!(
            terminal,
            "{}",
            if revoke {
                "Revocation cancelled; nothing changed."
            } else {
                "Enable cancelled; nothing changed."
            }
        )
        .map_err(|_| uncertain.to_owned())?;
        return Ok(());
    }
    writeln!(terminal, "Confirmation received.").map_err(|_| uncertain.to_owned())?;
    let mut store = Store::open(dir).map_err(|e| custody_error(e, dir, uncertain))?;
    let result = store
        .owner_update(
            dir,
            machine,
            parsed_kind,
            action,
            &custody.owner().principal,
            &id,
            now()?,
        )
        .map_err(|e| custody_error(e, dir, uncertain))?;
    let OwnerState::Complete { endpoint } = result else {
        return Err(uncertain.into());
    };
    println!("{}", success(&endpoint));
    Ok(())
}
