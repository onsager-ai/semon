//! Independent lightweight heartbeats. Full facts and uploads may block without
//! lending their old runtime observations a new timestamp.
use crate::{
    Client, Failure, PushOptions, Result, StateLock, Stop,
    wire::{self, Status, SyncPhase, SyncTarget},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub(crate) struct Progress {
    pub targets: BTreeMap<String, SyncTarget>,
    pub inventory_complete: bool,
    pub phase: SyncPhase,
}

pub(crate) struct Heartbeat {
    end: mpsc::Sender<()>,
    fatal: Arc<Mutex<Option<String>>>,
}
impl Heartbeat {
    pub fn start(
        options: &PushOptions,
        stop: &Stop,
        lock: Arc<StateLock>,
        progress: Arc<Mutex<Progress>>,
    ) -> Result<Self> {
        let mut client = Client::new(options)?
            .with_stop(stop.clone())
            .with_lock(lock.clone());
        client.progress = Arc::clone(&progress);
        let observation_id = wire::new_generation()?;
        let sessions = options.sessions.clone();
        let (end, ended) = mpsc::channel();
        let fatal = Arc::new(Mutex::new(None));
        let shared_fatal = Arc::clone(&fatal);
        let stop = stop.clone();
        thread::Builder::new()
            .name("semon-push-status".into())
            .spawn(move || {
                let _lock = lock;
                let mut sequence = 0;
                loop {
                    if stop.is_stopped() || ended.try_recv().is_ok() {
                        break;
                    }
                    // This collection starts afresh on every heartbeat, on a
                    // worker distinct from the full metadata collector.
                    if let Ok(runtime) = semon_sessions::local_runtime_facts(&sessions) {
                        let next_sequence = sequence + 1;
                        let snapshot = progress.lock().unwrap_or_else(|e| e.into_inner());
                        let mut status = Status {
                            version: 1,
                            observation_id: observation_id.clone(),
                            sequence: next_sequence,
                            observed_at_ms: now_ms(),
                            phase: snapshot.phase,
                            runtime,
                            inventory_complete: snapshot.inventory_complete,
                            targets: snapshot.targets.values().cloned().collect(),
                        };
                        drop(snapshot);
                        let body = loop {
                            match serde_json::to_string(&status) {
                                Ok(body) if body.len() <= crate::mirror::MAX_BODY_BYTES => {
                                    break Some(body);
                                }
                                Ok(_) if !status.targets.is_empty() => {
                                    status.inventory_complete = false;
                                    status.targets.truncate(status.targets.len() / 2);
                                }
                                _ => break None,
                            }
                        };
                        if let Some(body) = body {
                            match client.post("status", body) {
                                Ok((404 | 405, _)) => break,
                                Ok((200..=299, _)) => {
                                    sequence = next_sequence;
                                }
                                Err(Failure::Remote(error))
                                    if error.contains("refused the token") =>
                                {
                                    *shared_fatal.lock().unwrap_or_else(|e| e.into_inner()) =
                                        Some(error);
                                    stop.stop();
                                    break;
                                }
                                Err(Failure::Stopped) => break,
                                // A heartbeat never clears an append restriction.
                                _ => {}
                            }
                        }
                    }
                    match ended.recv_timeout(Duration::from_secs(2)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
            })
            .map_err(|e| format!("status: {e}"))?;
        Ok(Self { end, fatal })
    }
    pub fn fatal(&self) -> Option<String> {
        self.fatal.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
impl Drop for Heartbeat {
    fn drop(&mut self) {
        let _ = self.end.send(());
    }
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
