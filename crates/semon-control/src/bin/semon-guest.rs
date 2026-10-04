//! Private managed-guest bridge to the existing isolated app-server driver.
//! No TCP listener, terminal exec/resume, or provider credential handling.
use semon_control::{RequestId, Source, local::LocalSession};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::Path,
};

fn request(input: impl Read) -> io::Result<Value> {
    let mut bytes = Vec::new();
    input.take(131073).read_to_end(&mut bytes)?;
    if bytes.len() > 131072 {
        return Err(io::Error::other("request too large"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
fn main() -> io::Result<()> {
    // Bootstrap input arrives through protected coordinator stdin, never argv.
    let mut input = request(io::stdin().lock())?;
    if !matches!(
        input["method"].as_str(),
        Some("openai_api_key" | "chatgpt_siwc")
    ) {
        return Err(io::Error::other("selected authentication method required"));
    }
    let key = input["api_key"]
        .as_str()
        .ok_or_else(|| io::Error::other("API key required"))?
        .to_owned();
    input["api_key"] = Value::Null;
    let config = if input["method"] == "chatgpt_siwc" {
        let path = Path::new("/var/lib/semon/model.toml");
        fs::write(
            path,
            "model_provider=\"openai_chatgpt_plan\"\n[model_providers.openai_chatgpt_plan]\nname=\"ChatGPT plan\"\nbase_url=\"https://api.openai.com/v1\"\nenv_key=\"ACCESS_TOKEN\"\nwire_api=\"responses\"\nrequires_openai_auth=false\nsupports_websockets=false\n",
        )?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Some(path)
    } else {
        None
    };
    let mut session = LocalSession::launch_with_api_key(
        Path::new("/opt/semon/codex/bin/codex"),
        Path::new("/workspace/project"),
        Path::new("/var/lib/semon/session"),
        config,
        Some(&key),
    )?;
    drop(key);
    let ipc = "/var/lib/semon/control.sock";
    let listener = UnixListener::bind(ipc)?;
    fs::set_permissions(ipc, fs::Permissions::from_mode(0o600))?;
    for stream in listener.incoming() {
        let mut stream = stream?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
        let value = match request(&stream) {
            Ok(value) => value,
            Err(_) => {
                let _ = stream.write_all(b"{\"error\":\"invalid private request\"}");
                continue;
            }
        };
        let source = Source {
            window: "managed-owner".into(),
            peer_pid: 0,
            peer_uid: 0,
        };
        let mut ended = false;
        let result = match value["op"].as_str() {
            Some("snapshot") => Ok(Value::Null),
            Some("reconnect") => session.driver.reconnect().map(|driver| {
                session.driver = driver;
                Value::Null
            }),
            Some("renew") => (|| -> io::Result<Value> {
                if value["method"] != input["method"] {
                    Err(io::Error::other("authentication method cannot change"))
                } else {
                    let id = value["id"]
                        .as_str()
                        .filter(|id| RequestId::parse(id).is_some())
                        .ok_or_else(|| io::Error::other("renewal identity required"))?;
                    let key = value["model_key"]
                        .as_str()
                        .ok_or_else(|| io::Error::other("credential required"))?;
                    session.renew_credential(key).and_then(|()| {
                        fs::write(
                            "/var/lib/semon/renewal.json",
                            json!({"id":id,"thread":session.driver.thread_id()}).to_string(),
                        )?;
                        Ok(Value::Null)
                    })
                }
            })(),
            Some("end") => session.end().map(|()| {
                ended = true;
                Value::Null
            }),
            Some("send") => {
                let snapshot = session.driver.snapshot();
                // One operation identity is committed by Hub before dispatch.
                let command = json!({"id":value["id"],"op":"send","thread":value["thread"],
                    "generation":value["generation"],"activeTurn":value["activeTurn"],
                    "expires":value["expires"],"text":value["text"]});
                // Require the caller's exact target; initial input is allowed only
                // with a thread receipt already persisted outside the guest.
                if value["thread"] != snapshot["thread"]
                    || RequestId::parse(value["id"].as_str().unwrap_or("")).is_none()
                {
                    Err(io::Error::other("stale target"))
                } else {
                    session.driver.command(&command, source)
                }
            }
            _ => Err(io::Error::other(
                "unsupported control; approvals require managed boundary qualification",
            )),
        };
        let mut snapshot = session.driver.snapshot();
        // Approval writes remain off even when local namespace probes pass.
        for capability in ["commandApproval", "fileApproval", "questions"] {
            snapshot["capabilities"][capability] = json!(false);
        }
        snapshot["approvalReason"] = json!("Managed-guest authority boundary is not qualified");
        if ended {
            fs::write(
                "/var/lib/semon/ended.json",
                json!({"ended":true,"thread":session.driver.thread_id()}).to_string(),
            )?;
        }
        if let Some(actions) = snapshot["actions"].as_object_mut() {
            for action in actions.values_mut() {
                if let Some(command) = action.get_mut("command").and_then(Value::as_object_mut) {
                    command.remove("text");
                }
            }
        }
        snapshot["capabilities"]["interrupt"] = json!(false);
        if let Ok(bytes) = fs::read("/var/lib/semon/renewal.json")
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            snapshot["renewalId"] = value["id"].clone();
        }
        let response = match result {
            Ok(receipt) => json!({"snapshot":snapshot,"receipt":receipt,"ended":ended}),
            Err(_) => {
                json!({"snapshot":snapshot,"error":"Control unavailable or outcome unknown; inspect before sending again"})
            }
        };
        stream.write_all(response.to_string().as_bytes())?;
        if ended {
            break;
        }
    }
    fs::remove_file(ipc)?;
    Ok(())
}
