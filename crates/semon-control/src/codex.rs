//! One Codex app-server connection. Native events own live state; no transcript polling.
use crate::{
    Adapter, Answer, Delivery, Harness, Kind, NewRequest, PendingRequest, RequestId, RequestStore,
    ResolvedReason, Source, monotonic_ms,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::{fs::MetadataExt, net::UnixStream},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};

/// Qualified native protocol release. Other versions are refused by the launcher.
pub const VERSION: &str = "0.160.0";
const DEADLINE: Duration = Duration::from_secs(10);
const REQUEST_TTL: u64 = 120_000;

/// A live native connection, shared with the local viewer.
pub struct Codex {
    pub(crate) store: Arc<RequestStore>,
    inner: Mutex<Live>,
    socket: std::path::PathBuf,
    jobs: mpsc::Sender<Job>,
    /// Only the confined launcher can supply owner authority.
    pub(crate) writable: bool,
    // Continuity check only; owner authority comes from the qualified executor isolation.
    server: Option<(u32, String)>,
}
struct Live {
    thread: String,
    active: Option<String>,
    last_turn: Option<Value>,
    generation: String,
    connected: bool,
    reason: Option<String>,
    requests: BTreeMap<String, RequestId>,
    items: BTreeMap<(String, String), Value>,
    actions: BTreeMap<String, Value>,
}
struct Job {
    frame: Value,
    action: Option<String>,
    answer: Option<RequestId>,
    expires: Instant,
    active: Option<String>,
    reply: mpsc::Sender<io::Result<Value>>,
}
fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn process_stamp(pid: u32) -> io::Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .ok_or_else(|| error("native server identity unavailable"))?
        .1
        .split_whitespace()
        .collect();
    // A killed child can retain its PID/start time until LocalSession reaps it.
    // That identity must not authorize a new listener on a reused socket inode.
    if matches!(fields.first(), Some(&"Z" | &"X" | &"x")) {
        return Err(error("native server is no longer live"));
    }
    fields
        .get(19)
        .copied()
        .map(str::to_owned)
        .ok_or_else(|| error("native server identity unavailable"))
}

#[cfg(target_os = "linux")]
fn verify_native_peer(stream: &UnixStream, pid: u32, uid: u32) -> io::Result<()> {
    let credentials = rustix::net::sockopt::socket_peercred(stream)
        .map_err(|_| error("native transport peer identity unavailable"))?;
    if credentials.pid.as_raw_nonzero().get() as u32 != pid || credentials.uid.as_raw() != uid {
        return Err(error("native transport peer is not the pinned controller"));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn verify_native_peer(_: &UnixStream, _: u32, _: u32) -> io::Result<()> {
    Err(error("native transport peer identity is not qualified"))
}
fn identity(socket: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(socket)?;
    use std::os::unix::fs::FileTypeExt;
    if !metadata.file_type().is_socket() {
        return Err(error("expected native Unix socket"));
    }
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}
fn send(socket: &mut WebSocket<UnixStream>, frame: &Value) -> io::Result<()> {
    socket
        .send(Message::Text(frame.to_string().into()))
        .map_err(|_| error("native transport lost; delivery unknown"))
}
fn receive(socket: &mut WebSocket<UnixStream>) -> io::Result<Option<Value>> {
    match socket.read() {
        Ok(Message::Text(text)) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| error("malformed native frame")),
        Ok(Message::Close(_)) => Err(error("native connection closed")),
        Ok(_) => Ok(None),
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(_) => Err(error("native connection lost")),
    }
}
impl Codex {
    /// Attaches for observation only. An existing terminal's UID/socket is not owner proof.
    pub fn observe(socket: &Path, thread: &str, store: Arc<RequestStore>) -> io::Result<Arc<Self>> {
        Self::connect(socket, Some(thread), None, store, None)
    }
    pub(crate) fn connect(
        socket: &Path,
        thread_id: Option<&str>,
        cwd: Option<&Path>,
        store: Arc<RequestStore>,
        server_pid: Option<u32>,
    ) -> io::Result<Arc<Self>> {
        let server = server_pid
            .map(|pid| process_stamp(pid).map(|stamp| (pid, stamp)))
            .transpose()?;
        let writable = server.is_some();
        // Stable Codex publishes a rendezvous symlink to its protected physical socket.
        let socket = socket.canonicalize()?;
        let metadata = fs::metadata(&socket)?;
        let parent = fs::metadata(
            socket
                .parent()
                .ok_or_else(|| error("missing socket parent"))?,
        )?;
        let uid = fs::metadata("/proc/self")?.uid();
        if metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
            || parent.uid() != uid
            || parent.mode() & 0o077 != 0
        {
            return Err(error("native socket must be private to the owner"));
        }
        let generation = format!("{}:{}", identity(&socket)?, RequestId::random()?);
        let stream = UnixStream::connect(&socket)?;
        if let Some((pid, _)) = &server {
            verify_native_peer(&stream, *pid, uid)?;
        }
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(1 << 20))
            .max_frame_size(Some(1 << 20));
        let (mut transport, _) =
            tungstenite::client::client_with_config("ws://localhost/", stream, Some(config))
                .map_err(|_| error("native handshake failed"))?;
        send(
            &mut transport,
            &json!({"id":"semon:init","method":"initialize","params":{"clientInfo":{"name":"semon","title":"Semon","version":"0.1.0"},"capabilities":{"experimentalApi":true}}}),
        )?;
        let deadline = Instant::now() + DEADLINE;
        let mut early = Vec::new();
        let initialized = loop {
            if Instant::now() >= deadline {
                return Err(error("initialize timed out"));
            }
            if let Some(frame) = receive(&mut transport)? {
                if frame["id"] == "semon:init" {
                    break frame;
                }
                if early.len() >= 256 {
                    return Err(error("native attach event budget exceeded"));
                }
                early.push(frame);
            }
        };
        if initialized.get("error").is_some() {
            return Err(error("initialize rejected"));
        }
        // The launched package is pinned; passive attachment remains read-only.
        send(&mut transport, &json!({"method":"initialized"}))?;
        let params = match thread_id {
            Some(id) => json!({"threadId":id}),
            None => {
                json!({"cwd":cwd.ok_or_else(|| error("missing workspace"))?,"environments":[{"environmentId":"semon-local","cwd":cwd}],"approvalPolicy":"on-request","sandbox":"read-only"})
            }
        };
        send(
            &mut transport,
            &json!({"id":"semon:thread","method":if thread_id.is_some() {"thread/resume"} else {"thread/start"},"params":params}),
        )?;
        let receipt = loop {
            if Instant::now() >= deadline {
                return Err(error("thread attach timed out"));
            }
            if let Some(frame) = receive(&mut transport)? {
                if frame["id"] == "semon:thread" {
                    break frame;
                }
                if early.len() >= 256 {
                    return Err(error("native attach event budget exceeded"));
                }
                early.push(frame);
            }
        };
        let native = &receipt["result"]["thread"];
        let id = native["id"]
            .as_str()
            .ok_or_else(|| error("thread attach rejected"))?
            .to_owned();
        let active = native["turns"]
            .as_array()
            .and_then(|turns| turns.iter().rev().find(|t| t["status"] == "inProgress"))
            .and_then(|t| t["id"].as_str())
            .map(str::to_owned);
        let (jobs, receiver) = mpsc::channel();
        let driver = Arc::new(Self {
            store,
            socket: socket.to_owned(),
            inner: Mutex::new(Live {
                thread: id,
                active,
                last_turn: native["turns"]
                    .as_array()
                    .and_then(|turns| turns.last())
                    .cloned(),
                generation,
                connected: true,
                reason: None,
                requests: BTreeMap::new(),
                items: BTreeMap::new(),
                actions: BTreeMap::new(),
            }),
            jobs,
            writable,
            server,
        });
        for frame in early {
            driver.event(&frame);
        }
        let weak = Arc::downgrade(&driver);
        thread::spawn(move || {
            let mut pending: BTreeMap<String, Job> = BTreeMap::new();
            loop {
                let Some(driver) = weak.upgrade() else {
                    break;
                };
                if !lock(&driver.inner).connected {
                    break;
                }
                while let Ok(job) = receiver.try_recv() {
                    if Instant::now() >= job.expires {
                        let _ = job
                            .reply
                            .send(Err(error("dispatch deadline expired before send")));
                        continue;
                    }
                    if lock(&driver.inner).active != job.active {
                        let _ = job.reply.send(Err(error(
                            "stale active turn before dispatch; nothing sent",
                        )));
                        continue;
                    }
                    if send(&mut transport, &job.frame).is_err() {
                        driver.lost();
                        let _ = job.reply.send(Err(error("delivery unknown")));
                        return;
                    }
                    if job.answer.is_some() {
                        let _ = job
                            .reply
                            .send(Ok(json!({"delivery":"sent","outcome":"unknown"})));
                    } else if let Some(id) = job.frame["id"].as_str() {
                        pending.insert(id.into(), job);
                    }
                }
                match receive(&mut transport) {
                    Ok(Some(frame)) => {
                        if let Some(job) = frame["id"].as_str().and_then(|id| pending.remove(id)) {
                            if let Some(id) = &job.action {
                                driver.finish(id, &frame);
                            }
                            let _ = job.reply.send(Ok(frame));
                        } else {
                            driver.event(&frame);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        driver.lost();
                        break;
                    }
                }
                pending.retain(|_, job| {
                    if Instant::now() < job.expires {
                        return true;
                    }
                    if let Some(id) = &job.action {
                        driver.finish(id, &json!({"delivery":"unknown"}));
                    }
                    let _ = job
                        .reply
                        .send(Err(error("delivery unknown; do not replay")));
                    false
                });
                driver.store.tick(monotonic_ms());
            }
        });
        Ok(driver)
    }
    /// Establishes a fresh observation/dispatch generation without replaying any write.
    pub fn reconnect(&self) -> io::Result<Arc<Self>> {
        self.lost();
        if let Some((pid, stamp)) = &self.server
            && (process_stamp(*pid).as_ref().ok() != Some(stamp)
                || !lock(&self.inner)
                    .generation
                    .starts_with(&format!("{}:", identity(&self.socket)?)))
        {
            return Err(error(
                "native server restarted; launch a fresh isolated session explicitly",
            ));
        }
        Self::connect(
            &self.socket,
            Some(&self.thread_id()),
            None,
            self.store.clone(),
            self.server.as_ref().map(|(pid, _)| *pid),
        )
    }
    /// Native thread selected for this connection.
    pub fn thread_id(&self) -> String {
        lock(&self.inner).thread.clone()
    }
    /// Viewer projection. Historical messages remain with semon-sessions collection.
    pub fn snapshot(&self) -> Value {
        let live = lock(&self.inner);
        let reason = live.reason.clone().or_else(|| {
            (!self.writable).then(|| {
                "Existing terminal attachment has no qualified owner isolation; answer in Codex"
                    .into()
            })
        });
        let enabled = live.connected && self.writable;
        let requests: Vec<_> = self.store.snapshot(monotonic_ms()).requests.iter().filter(|r| r.session == live.thread).map(|r| json!({"id":r.id.to_string(),"kind":r.kind.as_str(),"payload":if r.payload.is_null(){json!({})}else{r.payload.clone()},"hash":r.payload_sha256,"state":r.state.to_json(),"reason":match &r.answerable {crate::Answerable::Yes=>None,crate::Answerable::No(reason)=>Some(reason)},"remainingMs":r.expires_ms.saturating_sub(monotonic_ms())})).collect();
        json!({"thread":live.thread,"generation":live.generation,"activeTurn":live.active,"lastTurn":live.last_turn,"connected":live.connected,"capabilities":{"input":enabled,"steer":enabled,"interrupt":enabled,"commandApproval":enabled,"fileApproval":enabled,"questions":enabled},"reason":reason,"requests":requests,"actions":live.actions})
    }
    fn queue(
        &self,
        frame: Value,
        action: Option<String>,
        answer: Option<RequestId>,
        active: Option<String>,
    ) -> io::Result<Value> {
        let (reply, response) = mpsc::channel();
        self.jobs
            .send(Job {
                frame,
                action,
                answer,
                expires: Instant::now() + DEADLINE,
                active,
                reply,
            })
            .map_err(|_| error("native connection ended"))?;
        response
            .recv_timeout(DEADLINE)
            .map_err(|_| error("delivery unknown; do not replay"))?
    }
    /// One exact-bound viewer operation. Retries with the same id only read its saved outcome.
    pub fn command(&self, value: &Value, source: Source) -> io::Result<Value> {
        let id = value["id"]
            .as_str()
            .and_then(RequestId::parse)
            .ok_or_else(|| error("invalid command id"))?
            .to_string();
        {
            let mut live = lock(&self.inner);
            if let Some(previous) = live.actions.get(&id) {
                if previous["command"] != *value {
                    return Err(error("duplicate command id has different payload"));
                }
                return Ok(previous.clone());
            }
            if value["expires"].as_u64().is_none_or(|expiry| {
                expiry <= crate::wall_ms() || expiry > crate::wall_ms().saturating_add(30_000)
            }) {
                return Err(error("expired control command"));
            }
            if !self.writable
                || !live.connected
                || value["thread"] != live.thread
                || value["generation"] != live.generation
            {
                return Err(error("read-only or stale connection target"));
            }
            if value.get("activeTurn") != Some(&json!(live.active)) {
                return Err(error("stale active turn"));
            }
            if !matches!(value["op"].as_str(), Some("send" | "interrupt" | "answer")) {
                return Err(error("unsupported operation"));
            }
            if live.actions.len() >= 1024 {
                return Err(error(
                    "control operation budget exhausted; restart explicitly",
                ));
            }
            live.actions.insert(
                id.clone(),
                json!({"command":value,"delivery":"dispatching"}),
            );
        }
        if !self.store.record_control(
            &json!({"event":"control_intent","id":id,"command":value,"source":source.to_json()}),
        ) {
            self.finish(
                &id,
                &json!({"delivery":"not_sent","reason":"journal unavailable"}),
            );
            return Err(error("journal unavailable; nothing sent"));
        }
        let outcome = (|| -> io::Result<Value> {
            if value["op"] == "answer" {
                let request = value["request"]
                    .as_str()
                    .and_then(RequestId::parse)
                    .ok_or_else(|| error("invalid request id"))?;
                let answer = if value["answer"]["decision"] == "allow" {
                    Answer::Allow
                } else if value["answer"]["decision"] == "deny" {
                    Answer::Deny { message: None }
                } else {
                    let answers: BTreeMap<String, Vec<String>> =
                        serde_json::from_value(value["answer"]["answers"].clone())
                            .map_err(|_| error("invalid native answers"))?;
                    Answer::CodexQuestions(answers)
                };
                self.store
                    .answer(
                        self,
                        request,
                        answer,
                        value["hash"].as_str().unwrap_or(""),
                        source,
                        &monotonic_ms,
                    )
                    .map(|state| json!({"delivery":"sent","state":state.to_json()}))
                    .map_err(|e| error(e.to_string()))
            } else {
                let active = value["activeTurn"].as_str();
                let (method, params) = if value["op"] == "interrupt" {
                    let turn = active.ok_or_else(|| error("no active turn"))?;
                    (
                        "turn/interrupt",
                        json!({"threadId":value["thread"],"turnId":turn}),
                    )
                } else {
                    let text = value["text"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty() && s.len() <= 65536)
                        .ok_or_else(|| error("invalid input"))?;
                    let input = json!([{"type":"text","text":text}]);
                    match active {
                        Some(turn) => (
                            "turn/steer",
                            json!({"threadId":value["thread"],"expectedTurnId":turn,"input":input}),
                        ),
                        None => (
                            "turn/start",
                            json!({"threadId":value["thread"],"clientUserMessageId":id,"input":input}),
                        ),
                    }
                };
                self.queue(
                    json!({"id":format!("semon:{id}"),"method":method,"params":params}),
                    Some(id.clone()),
                    None,
                    active.map(str::to_owned),
                )
            }
        })();
        match outcome {
            Ok(receipt) => {
                self.finish(&id, &receipt);
                Ok(receipt)
            }
            Err(e) => {
                self.finish(
                    &id,
                    &json!({"delivery":"unknown_or_refused","reason":e.to_string()}),
                );
                Err(e)
            }
        }
    }
    fn finish(&self, id: &str, receipt: &Value) {
        if let Some(action) = lock(&self.inner).actions.get_mut(id) {
            action["receipt"] = receipt.clone();
            action["delivery"] = json!(if receipt.get("result").is_some() {
                "accepted"
            } else if receipt.get("error").is_some() {
                "rejected"
            } else {
                "inspect"
            });
        }
        self.store
            .record_control(&json!({"event":"control_receipt","id":id,"receipt":receipt}));
    }
    pub(crate) fn lost(&self) {
        let mut live = lock(&self.inner);
        live.connected = false;
        live.reason = Some(
            "Native connection lost. Reconnect explicitly; uncertain writes are never replayed"
                .into(),
        );
        self.store.connection_lost(&live.thread, monotonic_ms());
        for action in live
            .actions
            .values_mut()
            .filter(|a| a["delivery"] == "dispatching")
        {
            action["delivery"] = json!("unknown");
        }
    }
    fn event(&self, frame: &Value) {
        let mut live = lock(&self.inner);
        let p = &frame["params"];
        let server_request = frame.get("id").is_some() && frame["method"].is_string();
        if p["threadId"] != live.thread && !(server_request && p.get("threadId").is_none()) {
            return;
        }
        let now = monotonic_ms();
        match frame["method"].as_str() {
            Some("turn/started") => {
                live.active = p["turn"]["id"].as_str().map(str::to_owned);
            }
            Some("turn/completed") => {
                live.last_turn = Some(p["turn"].clone());
                if p["turn"]["id"].as_str() == live.active.as_deref() {
                    live.active = None;
                }
                live.items.clear();
            }
            Some("item/started") => {
                if let (Some(turn), Some(item)) = (p["turnId"].as_str(), p["item"]["id"].as_str())
                    && live.items.len() < 256
                {
                    live.items
                        .insert((turn.into(), item.into()), p["item"].clone());
                }
            }
            Some("serverRequest/resolved") => {
                if let Some(id) = live.requests.get(&p["requestId"].to_string()) {
                    self.store
                        .resolve(*id, ResolvedReason::AnsweredOrCleared, now);
                }
            }
            Some(method) if server_request => {
                let retained = self
                    .store
                    .snapshot(now)
                    .requests
                    .into_iter()
                    .map(|r| r.id)
                    .collect::<std::collections::BTreeSet<_>>();
                live.requests.retain(|_, id| retained.contains(id));
                let key = frame["id"].to_string();
                if live.requests.contains_key(&key) {
                    return;
                }
                if !frame["id"].is_i64() && !frame["id"].is_string() {
                    return;
                }
                let item = p["turnId"]
                    .as_str()
                    .zip(p["itemId"].as_str())
                    .and_then(|(turn, item)| live.items.get(&(turn.into(), item.into())));
                let target_bound = p["threadId"] == live.thread
                    && p["turnId"].is_string()
                    && p["itemId"].is_string();
                let kind = if method == "item/tool/requestUserInput" {
                    Kind::Question
                } else {
                    Kind::Permission
                };
                let uncertain = self.store.snapshot(now).requests.iter().any(|old| {
                    old.payload["params"]["threadId"] == p["threadId"]
                        && old.payload["params"]["turnId"] == p["turnId"]
                        && old.payload["params"]["itemId"] == p["itemId"]
                        && matches!(
                            old.state,
                            crate::State::Left(crate::LeftReason::DeliveryUnknown)
                        )
                });
                let reason = if !target_bound {
                    Some("Unqualified native request without exact thread/turn/item identity; answer in Codex".into())
                } else if uncertain {
                    Some("Previous answer delivery is uncertain; inspect/answer in Codex".into())
                } else if !self.writable {
                    Some("Owner isolation is not qualified for this terminal".into())
                } else if (method == "item/commandExecution/requestApproval"
                    && p["command"].is_string()
                    && p["cwd"].is_string()
                    && p["environmentId"] == "semon-local")
                    || (method == "item/fileChange/requestApproval"
                        && item
                            .is_some_and(|i| i["type"] == "fileChange" && i["changes"].is_array()))
                    || kind == Kind::Question
                {
                    None
                } else {
                    Some("Unqualified native request or missing exact command/diff".into())
                };
                let payload = json!({"method":method,"nativeId":frame["id"],"generation":live.generation,"params":p,"item":item});
                if let Ok(id) = self.store.register(
                    NewRequest {
                        session: live.thread.clone(),
                        harness: Harness::Codex,
                        harness_ref: key.clone(),
                        kind,
                        payload,
                        read_only: reason,
                        match_key: None,
                        hook_wait: false,
                        expires_ms: now.saturating_add(
                            p["autoResolutionMs"]
                                .as_u64()
                                .unwrap_or(REQUEST_TTL)
                                .min(REQUEST_TTL),
                        ),
                    },
                    now,
                ) {
                    live.requests.insert(key, id);
                }
            }
            _ => {}
        }
    }
}
impl Adapter for Codex {
    fn deliver(&self, request: &PendingRequest, answer: &Answer) -> Delivery {
        let live = lock(&self.inner);
        if !self.writable
            || !live.connected
            || request.session != live.thread
            || request.payload["generation"] != live.generation
            || request.payload["params"]["turnId"].as_str() != live.active.as_deref()
        {
            return Delivery::Closed(ResolvedReason::AnsweredOrCleared);
        }
        let result = match answer {
            Answer::Allow => json!({"decision":"accept"}),
            Answer::Deny { .. } => json!({"decision":"decline"}),
            Answer::CodexQuestions(answers) => {
                json!({"answers":answers.iter().map(|(id,answers)| (id,json!({"answers":answers}))).collect::<BTreeMap<_,_>>()})
            }
            _ => return Delivery::Closed(ResolvedReason::AnsweredOrCleared),
        };
        drop(live);
        let _ = self.queue(
            json!({"id":request.payload["nativeId"],"result":result}),
            None,
            Some(request.id),
            request.payload["params"]["turnId"]
                .as_str()
                .map(str::to_owned),
        );
        // Response frames have no acknowledgement. Resolution cannot attribute a winner.
        Delivery::AwaitConfirmation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    #[test]
    fn a_zombie_controller_cannot_supply_live_authority() {
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert!(process_stamp(child.id()).is_ok());
        child.kill().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        // Do not try_wait: the zombie/start-time case is the regression.
        while process_stamp(child.id()).is_ok() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let dead = process_stamp(child.id());
        child.wait().unwrap();
        assert!(dead.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn socket_peer_must_match_the_exact_controller_not_just_owner_uid() {
        let (stream, _) = UnixStream::pair().unwrap();
        let uid = fs::metadata("/proc/self").unwrap().uid();
        verify_native_peer(&stream, std::process::id(), uid).unwrap();
        assert!(verify_native_peer(&stream, std::process::id() + 1, uid).is_err());
        assert!(verify_native_peer(&stream, std::process::id(), uid.wrapping_add(1)).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_same_owner_replacement_receives_no_native_protocol_frames() {
        let root = crate::journal::tests::scratch("native-peer-mismatch");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("replacement.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut controller = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let store = Arc::new(RequestStore::new(
            crate::Journal::open(&root.join("journal.jsonl")).unwrap(),
        ));
        let result = Codex::connect(
            &path,
            Some("retained-thread"),
            None,
            store,
            Some(controller.id()),
        );
        controller.kill().unwrap();
        controller.wait().unwrap();
        let message = match result {
            Err(error) => error.to_string(),
            Ok(_) => panic!("replacement gained controller authority"),
        };
        assert!(message.contains("not the pinned controller"));
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        use std::io::Read;
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    }
    // Mock protocol regression, deliberately separate from checked-in real-native evidence.
    #[test]
    fn a_consumed_write_with_a_lost_reply_is_unknown_and_never_replayed() {
        let root = crate::journal::tests::scratch("native-driver-unknown");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("mock.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let (seen, frames) = mpsc::channel();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let read = |socket: &mut WebSocket<UnixStream>| -> Value {
                serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap()
            };
            let init = read(&mut socket);
            send(&mut socket, &json!({"id":init["id"],"result":{}})).unwrap();
            assert_eq!(read(&mut socket)["method"], "initialized");
            let attach = read(&mut socket);
            send(
                &mut socket,
                &json!({"id":attach["id"],"result":{"thread":{"id":"native-thread","turns":[]}}}),
            )
            .unwrap();
            seen.send(read(&mut socket)).unwrap();
            // Native may have consumed the request. Drop before its reply, not a rejection.
        });
        let store = Arc::new(RequestStore::new(
            crate::Journal::open(&root.join("journal.jsonl")).unwrap(),
        ));
        let driver = Codex::connect(
            &path,
            Some("native-thread"),
            None,
            store,
            Some(std::process::id()),
        )
        .unwrap();
        driver.event(&json!({"id":99,"method":"item/future/requestApproval","params":{"threadId":"native-thread"}}));
        let snap = driver.snapshot();
        assert!(
            snap["requests"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("without exact")
        );
        let command = json!({"id":RequestId::random().unwrap().to_string(),"op":"send","thread":snap["thread"],"generation":snap["generation"],"activeTurn":null,"expires":crate::wall_ms()+25_000,"text":"hello"});
        let source = Source {
            window: "test-owner".into(),
            peer_pid: 0,
            peer_uid: 0,
        };
        assert!(driver.command(&command, source.clone()).is_err());
        let frame = frames.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(frame["method"], "turn/start");
        let saved = driver.command(&command, source).unwrap();
        assert_eq!(saved["command"], command);
        assert_ne!(saved["delivery"], "accepted");
        assert!(frames.try_recv().is_err());
        assert_eq!(driver.snapshot()["connected"], false);
        server.join().unwrap();
    }
}
