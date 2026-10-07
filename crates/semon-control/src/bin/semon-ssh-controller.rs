//! Private SSH controller. Explicit fresh launch only; IPC never starts a session.
use semon_control::{RequestId, Source, local::LocalSession};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn refusal() -> io::Error {
    io::Error::other("Controller unavailable; retain and inspect this launch identity")
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn request(mut input: impl Read) -> io::Result<Value> {
    let mut bytes = Vec::new();
    input.by_ref().take(131073).read_to_end(&mut bytes)?;
    if bytes.len() > 131072 {
        return Err(refusal());
    }
    serde_json::from_slice(&bytes).map_err(|_| refusal())
}
fn private(path: &Path, directory: bool) -> io::Result<()> {
    let uid = fs::metadata("/proc/self")?.uid();
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != uid
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
    {
        return Err(refusal());
    }
    Ok(())
}
fn atomic(path: &Path, value: Value) -> io::Result<()> {
    let temp = path.with_extension("new");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)?;
    file.write_all(value.to_string().as_bytes())?;
    file.sync_all()?;
    fs::rename(temp, path)?;
    fs::File::open(path.parent().ok_or_else(refusal)?)?.sync_all()
}
fn command(
    session: &LocalSession,
    root: &Path,
    binding: &Value,
    value: &Value,
    source: Source,
) -> io::Result<Value> {
    let id = value["id"]
        .as_str()
        .filter(|id| RequestId::parse(id).is_some())
        .ok_or_else(refusal)?;
    let directory = root.join("native-receipts");
    if !directory.exists() {
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    private(&directory, true)?;
    let path = directory.join(format!("{id}.json"));
    if !path.exists() && fs::read_dir(&directory)?.take(1025).count() >= 1024 {
        return Err(refusal());
    }
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(value)?));
    if path.exists() || path.is_symlink() {
        private(&path, false)?;
        let record: Value = serde_json::from_slice(&fs::read(&path)?)?;
        if record["command_sha256"] != fingerprint || record["binding"] != *binding {
            return Err(refusal());
        }
        // Missing native results are inspection only, including after restart.
        return Ok(json!({"delivery":record["delivery"],"reconciled":true}));
    }
    let mut record = json!({"id":id,"binding":binding,"thread":value["thread"],"command_sha256":fingerprint,"delivery":"unknown"});
    if value["op"] == "send" {
        record["native_identity"] = json!({"generation":value["generation"],"input_sha256":format!("{:x}",Sha256::digest(value["text"].as_str().ok_or_else(refusal)?.as_bytes()))});
    }
    record["model_generation"] = serde_json::from_slice::<Value>(&fs::read(
        root.join("model-generation.json"),
    )?)?["generation"]
        .clone();
    atomic(&path, record.clone())?;
    let result = session.driver.command(value, source);
    let snapshot = session.driver.snapshot();
    let delivery = snapshot["actions"][id]["delivery"]
        .as_str()
        .unwrap_or("unknown");
    if matches!(delivery, "accepted" | "rejected") {
        record["delivery"] = json!(delivery);
        atomic(&path, record)?;
    }
    result
}
fn renew(
    session: &mut LocalSession,
    root: &Path,
    binding: &Value,
    value: &Value,
) -> io::Result<Value> {
    let id = value["id"]
        .as_str()
        .filter(|id| RequestId::parse(id).is_some())
        .ok_or_else(refusal)?;
    let generation = value["model_generation"].as_u64().ok_or_else(refusal)?;
    let directory = root.join("native-renewals");
    if !directory.exists() {
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    private(&directory, true)?;
    let path = directory.join(format!("{id}.json"));
    if path.exists() {
        private(&path, false)?;
        let proof: Value = serde_json::from_slice(&fs::read(path)?)?;
        if proof["binding"] != *binding
            || proof["generation"] != generation
            || proof["state"] != "accepted"
        {
            return Err(refusal());
        }
        return Ok(Value::Null);
    }
    if fs::read_dir(&directory)?.take(1025).count() >= 1024 {
        return Err(refusal());
    }
    let active = root.join("renewal-active.json");
    if active.exists() {
        private(&active, false)?;
        let previous: Value = serde_json::from_slice(&fs::read(&active)?)?;
        if previous["state"] != "accepted" {
            return Err(refusal());
        }
    }
    let version: Value = serde_json::from_slice(&fs::read(root.join("model-generation.json"))?)?;
    if generation < version["generation"].as_u64().ok_or_else(refusal)?
        || !session.driver.snapshot()["activeTurn"].is_null()
    {
        return Err(refusal());
    }
    let key = value["model_key"].as_str().ok_or_else(refusal)?;
    let mut claim = json!({"id":id,"binding":binding,"thread":session.driver.thread_id(),"generation":generation,"state":"unknown"});
    atomic(&path, claim.clone())?;
    atomic(&active, claim.clone())?;
    session.renew_credential(key)?;
    atomic(
        &root.join("model-generation.json"),
        json!({"generation":generation,"renewal":id}),
    )?;
    claim["state"] = json!("accepted");
    atomic(&path, claim.clone())?;
    atomic(&active, claim)?;
    atomic(
        &root.join("renewal.json"),
        json!({"id":id,"thread":session.driver.thread_id(),"generation":generation}),
    )?;
    Ok(Value::Null)
}
fn main() -> io::Result<()> {
    let mut input = request(io::stdin().lock())?;
    if input["version"] != 1 || input["method"] != "openai_api_key" {
        return Err(refusal());
    }
    let launch = input["launch"]
        .as_str()
        .filter(|id| RequestId::parse(id).is_some())
        .ok_or_else(refusal)?
        .to_owned();
    let root = PathBuf::from(input["root"].as_str().ok_or_else(refusal)?);
    if !root.is_absolute() || root.canonicalize()? != root {
        return Err(refusal());
    }
    private(&root, true)?;
    let expires = input["expires"]
        .as_u64()
        .filter(|expiry| now() < *expiry && *expiry <= now() + 300)
        .ok_or_else(refusal)?;
    let deadline = Instant::now() + Duration::from_secs(expires.saturating_sub(now()));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join("controller.lock"))?;
    private(&root.join("controller.lock"), false)?;
    lock.try_lock().map_err(|_| refusal())?;
    // A previous accepted launch is never recreated, even if its controller is gone.
    let accepted = root.join("launch.json");
    if accepted.exists() || accepted.is_symlink() {
        return Err(refusal());
    }
    let binding = input["binding"].clone();
    if !binding.is_object() {
        return Err(refusal());
    }
    let workspace = root.join("workspace");
    private(&workspace, true)?;
    atomic(
        &accepted,
        json!({"version":1,"launch":launch,"binding":binding,"expires":expires,"state":"claimed"}),
    )?;
    let binary = PathBuf::from(input["binary"].as_str().ok_or_else(refusal)?);
    let config = root.join("model.toml");
    private(&config, false)?;
    let mut key = input["model_key"]
        .as_str()
        .filter(|key| {
            !key.is_empty() && key.len() <= 4096 && !key.bytes().any(|byte| byte.is_ascii_control())
        })
        .ok_or_else(refusal)?
        .to_owned();
    input["model_key"] = Value::Null;
    atomic(
        &root.join("model-generation.json"),
        json!({"generation":binding["model_generation"]}),
    )?;
    let mut session = LocalSession::launch_with_api_key(
        &binary,
        &workspace,
        &root.join("session"),
        Some(&config),
        Some(&key),
    )?;
    key.clear();
    if Instant::now() >= deadline {
        session.end()?;
        return Err(refusal());
    }
    let thread = session.driver.thread_id();
    atomic(
        &accepted,
        json!({"version":1,"launch":launch,"binding":binding,"expires":expires,"state":"running","thread":thread,"home":session.home}),
    )?;
    let ipc = root.join("control.sock");
    let listener = UnixListener::bind(&ipc)?;
    fs::set_permissions(&ipc, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut cause = "expired";
    loop {
        if Instant::now() >= deadline || now() >= expires {
            break;
        }
        let (mut stream, _) = match listener.accept() {
            Ok(stream) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
                continue;
            }
            Err(error) => return Err(error),
        };
        let peer = rustix::net::sockopt::socket_peercred(&stream).map_err(|_| refusal())?;
        if peer.uid.as_raw() != fs::metadata(&root)?.uid() {
            continue;
        }
        // One stalled caller cannot retain the controller beyond its lease.
        let timeout =
            Duration::from_millis(500).min(deadline.saturating_duration_since(Instant::now()));
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let value = match request(&stream) {
            Ok(value) => value,
            Err(_) => {
                let _ = stream.write_all(b"{\"error\":\"invalid private request\"}");
                continue;
            }
        };
        if Instant::now() >= deadline || now() >= expires {
            break;
        }
        let source = Source {
            window: "ssh-private-owner".into(),
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
            Some("send" | "answer") => {
                let mut control = value.clone();
                let snapshot = session.driver.snapshot();
                if control["thread"] != snapshot["thread"]
                    || control["expires"]
                        .as_u64()
                        .is_none_or(|expiry| expiry > expires * 1000)
                {
                    Err(refusal())
                } else {
                    control["op"] = json!(if value["op"] == "answer" {
                        "answer"
                    } else {
                        "send"
                    });
                    command(&session, &root, &binding, &control, source)
                }
            }
            Some("renew") => {
                if value["method"] != "openai_api_key" {
                    Err(refusal())
                } else {
                    renew(&mut session, &root, &binding, &value)
                }
            }
            Some("end") => session.end().map(|()| {
                ended = true;
                Value::Null
            }),
            _ => Err(refusal()),
        };
        let mut snapshot = session.driver.snapshot();
        // Human native permission flows stay private. Hosted clients cannot infer
        // permission to answer from this controller's existence.
        for capability in ["commandApproval", "fileApproval", "questions", "interrupt"] {
            snapshot["capabilities"][capability] = json!(false);
        }
        if let Some(actions) = snapshot["actions"].as_object_mut() {
            for action in actions.values_mut() {
                if let Some(command) = action.get_mut("command").and_then(Value::as_object_mut) {
                    command.remove("text");
                }
            }
        }
        if let Ok(bytes) = fs::read(root.join("renewal.json"))
            && let Ok(renewal) = serde_json::from_slice::<Value>(&bytes)
        {
            snapshot["renewalId"] = renewal["id"].clone();
        }
        let response = match result {
            Ok(receipt) => {
                json!({"snapshot":snapshot,"receipt":receipt,"ended":false,"shutdown_pending":ended})
            }
            Err(_) => {
                json!({"snapshot":snapshot,"error":"Control unavailable or outcome unknown; inspect before sending again"})
            }
        };
        let _ = stream.write_all(response.to_string().as_bytes());
        if ended {
            cause = "native_end";
            break;
        }
    }
    session.end()?;
    atomic(
        &root.join("controller-ended.json"),
        json!({"version":1,"ended":true,"thread":thread,"launch":launch,"binding":binding,"cause":cause,"native_reaped":true}),
    )?;
    fs::remove_file(ipc)?;
    Ok(())
}
