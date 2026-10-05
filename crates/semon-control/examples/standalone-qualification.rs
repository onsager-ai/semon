//! Bounded qualification helper. Private bootstrap only on stdin; no replay.
use semon_control::{Source, local::LocalSession};
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn main() -> io::Result<()> {
    let mut bytes = Vec::new();
    io::stdin().take(131073).read_to_end(&mut bytes)?;
    if bytes.len() > 131072 {
        return Err(io::Error::other("private bootstrap too large"));
    }
    let input: Value = serde_json::from_slice(&bytes)?;
    let path = |key: &str| -> io::Result<&Path> {
        input[key]
            .as_str()
            .map(Path::new)
            .ok_or_else(|| io::Error::other("path required"))
    };
    let mut session = LocalSession::launch_with_api_key(
        path("binary")?,
        path("workspace")?,
        path("state")?,
        Some(path("config")?),
        input["api_key"].as_str(),
    )?;
    let snapshot = session.driver.snapshot();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 30_000;
    let command = json!({"id":input["operation"],"op":"send","text":input["text"],"thread":snapshot["thread"],"generation":snapshot["generation"],"activeTurn":snapshot["activeTurn"],"expires":expires});
    let source = || Source {
        window: "standalone-qualification".into(),
        peer_pid: 0,
        peer_uid: 0,
    };
    let receipt = session.driver.command(&command, source())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut approval_sent = false;
    loop {
        let snapshot = session.driver.snapshot();
        if !approval_sent
            && let Some(request) = snapshot["requests"].as_array().and_then(|requests| {
                requests
                    .iter()
                    .find(|request| request["state"]["state"] == "open")
            })
        {
            // Only the separately authorized synthetic command may be approved.
            // The outer namespace/seccomp executor boundary remains in force.
            if request["payload"]["method"] != "item/commandExecution/requestApproval"
                || request["payload"]["params"]["command"] != input["expected_command"]
            {
                return Err(io::Error::other("unexpected qualification approval"));
            }
            let answer = json!({"id":input["approval_operation"],"op":"answer","request":request["id"],"hash":request["hash"],"answer":{"decision":"allow"},"thread":snapshot["thread"],"generation":snapshot["generation"],"activeTurn":snapshot["activeTurn"],"expires":expires});
            session.driver.command(&answer, source())?;
            approval_sent = true;
        }
        if !snapshot["lastTurn"].is_null() && snapshot["activeTurn"].is_null() {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "turn outcome unknown; reconcile, never replay",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
    // Same operation reads its existing receipt even after a turn has completed.
    let duplicate = session.driver.command(&command, source())?;
    if receipt != duplicate["receipt"] {
        return Err(io::Error::other("duplicate changed receipt"));
    }
    let snapshot = session.driver.snapshot();
    session.end()?;
    println!(
        "{}",
        json!({"receipt":receipt,"snapshot":snapshot,"duplicate_prevented":true,"shutdown_reaped":true})
    );
    Ok(())
}
