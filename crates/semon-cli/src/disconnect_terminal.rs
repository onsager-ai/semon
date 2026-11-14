//! Local-only disconnect; confirmation never reads piped stdin.
use semon_push::origin_custody::{self, DISCONNECT_OWNER, DISCONNECT_UNCERTAIN, Disconnect};
use std::{
    fs::OpenOptions,
    io::{IsTerminal, Write},
};

fn success(host: &str) -> String {
    format!(
        "Disconnected receiver {host} on this machine. History, keys and audits kept. Receiver access was not revoked."
    )
}

pub(super) fn run(receiver: &str) -> Result<(), String> {
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| DISCONNECT_OWNER)?;
    if !terminal.is_terminal() {
        return Err(DISCONNECT_OWNER.into());
    }
    let base = origin_custody::default_directory().map_err(|_| DISCONNECT_UNCERTAIN)?;
    let operation = Disconnect::open(&base, receiver)?;
    let host = origin_custody::receiver_host(receiver)?;
    if !operation.is_disconnected() {
        let prompt = format!("Disconnect receiver {host} on this machine? Type yes to confirm: ");
        if !super::receive_owner::confirm(&mut terminal, &prompt)
            .map_err(|_| DISCONNECT_UNCERTAIN)?
        {
            writeln!(terminal, "Disconnect cancelled; nothing changed.")
                .map_err(|_| DISCONNECT_UNCERTAIN)?;
            return Ok(());
        }
        writeln!(terminal, "Confirmation received.").map_err(|_| DISCONNECT_UNCERTAIN)?;
    }
    operation.commit()?;
    println!("{}", success(&host));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_success_sanitized_host() {
        assert_eq!(
            success("[::1]:443"),
            "Disconnected receiver [::1]:443 on this machine. History, keys and audits kept. Receiver access was not revoked."
        );
    }
}
