//! Independent lightweight heartbeats. Full facts and uploads may block without
//! lending their old runtime observations a new timestamp.
use crate::{
    Client, Failure, PushOptions, Result, StateLock, Stop,
    wire::{self, Status, SyncPhase, SyncTarget},
};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Fixed source/discovery priority, never promoted by an upload attempt or ACK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TargetRank {
    pub live: bool,
    pub foreground: bool,
    pub modified_ns: Option<i128>,
    pub small: Reverse<u64>,
}

#[derive(Default)]
pub(crate) struct Progress {
    pub targets: BTreeMap<String, SyncTarget>,
    ranks: BTreeMap<String, TargetRank>,
    order: BTreeSet<(TargetRank, Reverse<String>)>,
    pub inventory_complete: bool,
    pub phase: SyncPhase,
    pub observation_id: String,
    pub sequence: u64,
}

impl Progress {
    pub fn new(observation_id: String) -> Self {
        Self {
            observation_id,
            ..Self::default()
        }
    }

    pub fn clear_targets(&mut self) {
        self.targets.clear();
        self.ranks.clear();
        self.order.clear();
    }

    pub fn prune_targets(&mut self, present: &BTreeSet<String>) {
        self.targets.retain(|key, _| present.contains(key));
        self.ranks.retain(|key, _| present.contains(key));
        self.order.retain(|(_, Reverse(key))| present.contains(key));
    }

    /// Bounded ranked admission; updating cold history cannot displace a
    /// fresher foreground descriptor. Stable path breaks otherwise equal ties.
    pub fn admit_target(&mut self, key: String, target: SyncTarget, rank: TargetRank) {
        let entry = (rank, Reverse(key.clone()));
        if !self.targets.contains_key(&key) && self.targets.len() >= wire::MAX_STATUS_TARGETS {
            self.inventory_complete = false;
            let Some(worst) = self.order.first() else {
                return;
            };
            if entry <= *worst {
                return;
            }
            let (_, Reverse(removed)) = self.order.pop_first().expect("ranked target");
            self.targets.remove(&removed);
            self.ranks.remove(&removed);
        }
        if let Some(previous) = self.ranks.insert(key.clone(), rank) {
            self.order.remove(&(previous, Reverse(key.clone())));
        }
        self.order.insert(entry);
        self.targets.insert(key, target);
    }

    pub fn ordered_targets(&self) -> Vec<SyncTarget> {
        self.order
            .iter()
            .rev()
            .filter_map(|(_, Reverse(key))| self.targets.get(key).cloned())
            .collect()
    }
}

pub(crate) struct Heartbeat {
    end: mpsc::Sender<()>,
    owned_stop: Stop,
    fatal: Arc<Mutex<Option<String>>>,
}
impl Heartbeat {
    pub fn start(
        options: &PushOptions,
        stop: &Stop,
        lock: Arc<StateLock>,
        progress: Arc<Mutex<Progress>>,
    ) -> Result<Self> {
        let owned_stop = stop.child();
        let mut client = Client::new(options)?
            .with_stop(owned_stop.clone())
            .with_lock(lock.clone());
        client.progress = Arc::clone(&progress);
        {
            let mut context = progress.lock().unwrap_or_else(|e| e.into_inner());
            if context.observation_id.is_empty() {
                context.observation_id = wire::new_generation()?;
            }
        }
        let sessions = options.sessions.clone();
        let (end, ended) = mpsc::channel();
        let fatal = Arc::new(Mutex::new(None));
        let shared_fatal = Arc::clone(&fatal);
        let caller_stop = stop.clone();
        let stop = owned_stop.clone();
        thread::Builder::new()
            .name("semon-push-status".into())
            .spawn(move || {
                let _lock = lock;
                if !status_advertised(&client) {
                    return;
                }
                loop {
                    if stop.is_stopped() || ended.try_recv().is_ok() {
                        break;
                    }
                    // This collection starts afresh on every heartbeat, on a
                    // worker distinct from the full metadata collector.
                    if let Ok(runtime) = semon_sessions::local_runtime_facts(&sessions) {
                        let mut snapshot = progress.lock().unwrap_or_else(|e| e.into_inner());
                        snapshot.sequence += 1;
                        let mut status = Status {
                            version: 1,
                            observation_id: snapshot.observation_id.clone(),
                            sequence: snapshot.sequence,
                            observed_at_ms: now_ms(),
                            phase: snapshot.phase,
                            runtime,
                            inventory_complete: snapshot.inventory_complete,
                            targets: snapshot.ordered_targets(),
                        };
                        drop(snapshot);
                        let body = bounded_body(&mut status);
                        if let Some(body) = body {
                            match client.post("status", body) {
                                Ok((404 | 405, _)) => break,
                                Ok((200..=299, _)) => {}
                                Err(Failure::Remote(error))
                                    if error.contains("refused the token") =>
                                {
                                    *shared_fatal.lock().unwrap_or_else(|e| e.into_inner()) =
                                        Some(error);
                                    caller_stop.stop();
                                    break;
                                }
                                Err(Failure::Stopped) => break,
                                // A heartbeat never clears an append restriction.
                                _ => {}
                            }
                        }
                    }
                    match ended.recv_timeout(Duration::from_secs(10)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
            })
            .map_err(|e| format!("status: {e}"))?;
        Ok(Self {
            end,
            fatal,
            owned_stop,
        })
    }
    pub fn fatal(&self) -> Option<String> {
        self.fatal.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.owned_stop.stop();
        let _ = self.end.send(());
    }
}
// Targets are already ranked highest first; byte trimming must retain that
// prefix rather than root/path order, or row admission alone loses latest data.
fn bounded_body(status: &mut Status) -> Option<String> {
    loop {
        match serde_json::to_string(status) {
            Ok(body) if body.len() <= crate::mirror::MAX_BODY_BYTES => return Some(body),
            Ok(_) if !status.targets.is_empty() => {
                status.inventory_complete = false;
                status.targets.truncate(status.targets.len() / 2);
            }
            _ => return None,
        }
    }
}

// An older receiver may route unknown paths through cookie authentication and
// answer 401 to a valid push-only token. Discover the POST-only route before
// treating optional status POST refusal as authoritative token rejection.
fn status_advertised(client: &Client) -> bool {
    let Ok(http) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return false;
    };
    let request = http
        .head(format!("{}/v1/mirror/status", client.url))
        .bearer_auth(client.token.expose())
        .header("connection", "close");
    let lock = client.lock.clone();
    matches!(
        client.stop.run("status-capability", move || {
            let _lock = lock;
            request.send().map(|response| {
                response.status().as_u16() == 405
                    && response
                        .headers()
                        .get_all(reqwest::header::ALLOW)
                        .iter()
                        .filter_map(|value| value.to_str().ok())
                        .any(|allow| {
                            allow
                                .split(',')
                                .any(|method| method.trim().eq_ignore_ascii_case("POST"))
                        })
            })
        }),
        Ok(Some(Ok(true)))
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_trimming_preserves_the_ranked_latest_prefix_and_index_bookkeeping() {
        let mut progress = Progress::new("a".repeat(32));
        let cold = TargetRank {
            live: false,
            foreground: false,
            modified_ns: Some(0),
            small: Reverse(3),
        };
        let quoted = "\"".repeat(220);
        for index in 0..wire::MAX_STATUS_TARGETS {
            let path =
                format!("sessions/{quoted}/{quoted}/{quoted}/{quoted}/cold-{index:04}.jsonl");
            assert!(semon_sessions::is_input_path("codex", &path));
            let target = SyncTarget {
                root: "codex".into(),
                path: path.clone(),
                generation: "b".repeat(32),
                target_bytes: 3,
                acked_bytes: 0,
                head_sha256: wire::head_sha256(b"{}\n"),
            };
            progress.admit_target(format!("codex/{path}"), target, cold);
        }
        let latest = TargetRank {
            foreground: true,
            modified_ns: Some(1),
            ..cold
        };
        let target = SyncTarget {
            root: "codex".into(),
            path: "sessions/latest.jsonl".into(),
            generation: "c".repeat(32),
            target_bytes: 3,
            acked_bytes: 3,
            head_sha256: wire::head_sha256(b"{}\n"),
        };
        progress.admit_target("codex/sessions/latest.jsonl".into(), target.clone(), latest);
        // Generation/prefix updates of an admitted key must not duplicate rank
        // entries, and repeated old ACKs must not evict the latest descriptor.
        let mut rewritten = target.clone();
        rewritten.generation = "d".repeat(32);
        progress.admit_target(
            "codex/sessions/latest.jsonl".into(),
            rewritten.clone(),
            latest,
        );
        for index in 0..32 {
            let mut history = target.clone();
            history.path = format!("sessions/old-{index:04}.jsonl");
            progress.admit_target(format!("codex/{}", history.path), history, cold);
        }
        assert_eq!(progress.targets.len(), wire::MAX_STATUS_TARGETS);
        assert_eq!(progress.ranks.len(), progress.targets.len());
        assert_eq!(progress.order.len(), progress.targets.len());
        let mut status = Status {
            version: 1,
            observation_id: progress.observation_id.clone(),
            sequence: 1,
            observed_at_ms: now_ms(),
            phase: SyncPhase::Syncing,
            runtime: semon_sessions::Facts {
                version: semon_sessions::FACTS_VERSION,
                ..semon_sessions::Facts::default()
            },
            inventory_complete: true,
            targets: progress.ordered_targets(),
        };
        assert!(serde_json::to_string(&status).unwrap().len() > crate::mirror::MAX_BODY_BYTES);
        let body = bounded_body(&mut status).unwrap();
        assert!(body.len() <= crate::mirror::MAX_BODY_BYTES);
        let parsed: Status = serde_json::from_str(&body).unwrap();
        assert!(!parsed.inventory_complete);
        assert!(parsed.targets.len() < wire::MAX_STATUS_TARGETS);
        assert_eq!(parsed.targets[0], rewritten);
        let mut present = progress.targets.keys().cloned().collect::<BTreeSet<_>>();
        present.remove("codex/sessions/latest.jsonl");
        progress.prune_targets(&present);
        assert_eq!(progress.ranks.len(), progress.targets.len());
        assert_eq!(progress.order.len(), progress.targets.len());
        assert!(
            !progress
                .ordered_targets()
                .iter()
                .any(|target| target.path == "sessions/latest.jsonl")
        );
        progress.clear_targets();
        assert!(
            progress.targets.is_empty() && progress.ranks.is_empty() && progress.order.is_empty()
        );
    }
}
