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
    Ok(confirmed(&line))
}
fn confirmed(line: &str) -> bool {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line) == "yes"
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
    let owner_error = "revocation requires the receiver owner's terminal; run this command there.";
    let uncertain = "revocation could not be confirmed; rerun the same command to check its earlier outcome before retrying.";
    // Read registry version before identity decoding so old receiver directories
    // consistently receive the normative unsupported-format recovery message.
    let registry = Registry::read_current(dir).map_err(|e| custody_error(e, dir, uncertain))?;
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
        format!(
            "kind {} was not found for machine {machine}; check --kind, then retry revocation.",
            kind_text.unwrap_or("")
        )
    };
    if kind_text.is_some() && parsed_kind.is_none() {
        return Err(unknown_kind());
    }
    let target = kind_text.map_or_else(
        || format!("machine {machine}"),
        |k| format!("kind {k} for machine {machine}"),
    );
    let success = || format!("Revoked {target}. History and audits kept.");
    let state = registry.owner_state(machine, parsed_kind, action, now()?);
    let id = match state {
        OwnerState::Complete { .. } => {
            println!("{}", success());
            return Ok(());
        }
        OwnerState::UnknownMachine => {
            return Err(format!(
                "machine {machine} was not found; check --machine, then retry revocation."
            ));
        }
        OwnerState::UnknownKind => return Err(unknown_kind()),
        OwnerState::LastKind => {
            return Err(format!(
                "kind {} is the last enabled kind; revoke the enrollment with semon receive revoke --dir {} --machine {machine} instead.",
                kind_text.unwrap_or(""),
                dir.display()
            ));
        }
        OwnerState::Confirm { enrollment_id, .. } => enrollment_id,
    };
    let prompt = format!("Revoke {target}? This stops access. Type yes to confirm: ");
    if !confirm(&mut terminal, &prompt)? {
        writeln!(terminal, "Revocation cancelled; nothing changed.")
            .map_err(|_| uncertain.to_owned())?;
        return Ok(());
    }
    writeln!(terminal, "Confirmation received.").map_err(|_| uncertain.to_owned())?;
    let mut store = Store::open(dir).map_err(|e| custody_error(e, dir, uncertain))?;
    let result = store
        .owner_update(
            dir,
            parsed_kind,
            action,
            &custody.owner().principal,
            &id,
            now()?,
        )
        .map_err(|e| custody_error(e, dir, uncertain))?;
    let OwnerState::Complete { .. } = result else {
        return Err(uncertain.into());
    };
    println!("{}", success());
    Ok(())
}

pub(super) fn machines(dir: &Path) -> Result<(), String> {
    let mut rows =
        semon_sessions::received_registry::read_machines(dir).map_err(|e| e.to_string())?;
    // Shared names sort together, while every row keeps its canonical ID.
    rows.sort_by(|a, b| a.owner_name.cmp(&b.owner_name).then(a.id.cmp(&b.id)));
    println!("ID\tMACHINE\tHOSTNAME\tOWNER NAME");
    for machine in rows {
        let hostname = semon_sessions::read_facts(
            &dir.join("machines")
                .join(&machine.directory)
                .join("facts.json"),
        )
        .map(|f| f.hostname)
        .unwrap_or_default();
        println!(
            "{}\t{}\t{:?}\t{:?}",
            machine.id,
            machine.directory,
            hostname,
            machine.owner_name.unwrap_or_default()
        );
    }
    Ok(())
}

pub(super) fn name(dir: &Path, id: &str, name: Option<&str>) -> Result<(), String> {
    // Opening the registry enforces the receiver owner's existing custody boundary.
    let mut store = Store::open(dir).map_err(|e| e.to_string())?;
    if !store
        .snapshot()
        .map_err(|e| e.to_string())?
        .machines()
        .iter()
        .any(|m| m.id == id)
    {
        return Err(format!(
            "unknown machine ID {id}; use semon receive machines --dir {}",
            dir.display()
        ));
    }
    store
        .name_machine(id, name, now()?)
        .map_err(|e| e.to_string())?;
    println!("Updated name for {id}.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_confirmation_and_cancellation() {
        for yes in ["yes", "yes\n", "yes\r\n"] {
            assert!(confirmed(yes));
        }
        for no in ["", "Yes\n", "YES\n", " yes\n", "yes \n", "y\n", "no\n"] {
            assert!(!confirmed(no));
        }
    }
}
