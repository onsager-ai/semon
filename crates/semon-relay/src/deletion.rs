//! Durable deletion requests. Each request owns a separate private file, so
//! a sender acknowledging one cannot overwrite a concurrently queued request.
//! Queues are scoped to the receiver origin and never delete carrier files.
use crate::{
    Transport, TransportError,
    state::{StateError, write_atomic},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForgetSelector {
    pub session: Option<String>,
    pub before_ns: Option<u64>,
    pub memory_root: Option<String>,
}
impl ForgetSelector {
    pub fn validate(&self) -> Result<(), StateError> {
        if self.session.is_none() && self.before_ns.is_none() && self.memory_root.is_none() {
            return Err(StateError::Invalid(
                "forget needs --session, --before or --memory".into(),
            ));
        }
        if self.session.as_ref().is_some_and(|value| {
            value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)
        }) {
            return Err(StateError::Invalid("invalid forget session".into()));
        }
        if let Some(root) = &self.memory_root
            && (self.session.is_some()
                || root.len() != 64
                || !root.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(StateError::Invalid(
                "memory needs a 64-hex root id and cannot select a session".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn to_value(&self) -> Value {
        json!({"session":self.session,"before_ns":self.before_ns,"memory_root":self.memory_root})
    }
    pub(crate) fn from_value(value: &Value) -> Result<Self, StateError> {
        let selector = Self {
            session: optional_string(value, "session")?,
            before_ns: match value.get("before_ns") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .ok_or_else(|| StateError::Invalid("invalid before_ns".into()))?,
                ),
            },
            memory_root: optional_string(value, "memory_root")?,
        };
        selector.validate()?;
        Ok(selector)
    }
    pub(crate) fn matches(&self, frame: &crate::Frame) -> bool {
        self.memory_root.is_none()
            && self
                .session
                .as_ref()
                .is_none_or(|session| session == &frame.key.session)
            && self
                .before_ns
                .is_none_or(|before| frame.sender_wall_ns < before)
    }
}
fn optional_string(value: &Value, name: &str) -> Result<Option<String>, StateError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(StateError::Invalid(format!("invalid {name}"))),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ForgetReport {
    pub frames: u64,
    pub bytes: u64,
}
impl ForgetReport {
    pub(crate) fn to_value(self, id: &str) -> Value {
        json!({"id":id,"frames":self.frames,"bytes":self.bytes})
    }
    pub(crate) fn from_value(value: &Value, id: &str) -> Result<Self, TransportError> {
        if value["id"].as_str() != Some(id) {
            return Err(TransportError::InvalidAck(
                "forget acknowledgement id mismatch".into(),
            ));
        }
        Ok(Self {
            frames: value["frames"]
                .as_u64()
                .ok_or_else(|| TransportError::InvalidAck("forget frames missing".into()))?,
            bytes: value["bytes"]
                .as_u64()
                .ok_or_else(|| TransportError::InvalidAck("forget bytes missing".into()))?,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForgetFlushReport {
    pub acknowledged: usize,
    pub pending: usize,
    pub failures: Vec<String>,
}
fn queue_dir(state: &Path, origin: &str) -> PathBuf {
    let mut name = state.as_os_str().to_owned();
    name.push(".forget");
    PathBuf::from(name).join(hex::encode(Sha256::digest(origin.as_bytes())))
}
fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn private_directory(path: &Path) -> Result<(), StateError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(StateError::Invalid(
                "forget queue must be a real directory".into(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if !fs::symlink_metadata(path)?.file_type().is_dir() {
                        return Err(StateError::Invalid(
                            "forget queue directory replaced".into(),
                        ));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn queue_forget(
    state: &Path,
    origin: &str,
    selector: &ForgetSelector,
) -> Result<String, StateError> {
    selector.validate()?;
    let mut selector = selector.clone();
    if selector.memory_root.is_some() && selector.before_ns.is_none() {
        selector.before_ns = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| StateError::Invalid("clock predates UNIX epoch".into()))?
                .as_nanos()
                .min(u128::from(u64::MAX)) as u64,
        );
    }
    let id = hex::encode(crate::generate_data_key());
    let dir = queue_dir(state, origin);
    private_directory(dir.parent().expect("queue has parent"))?;
    private_directory(&dir)?;
    let path = dir.join(format!("{id}.json"));
    write_atomic(
        &path,
        &serde_json::to_vec(&json!({"id":id,"selector":selector.to_value()}))?,
    )?;
    Ok(id)
}

pub fn flush_forgets(
    state: &Path,
    machine: &str,
    transport: &impl Transport,
) -> Result<ForgetFlushReport, StateError> {
    let dir = queue_dir(state, &transport.deletion_scope());
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ForgetFlushReport::default());
        }
        Err(error) => return Err(error.into()),
    };
    let mut report = ForgetFlushReport::default();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let bytes = match fs::read(entry.path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let value: Value = serde_json::from_slice(&bytes)?;
        let id = value["id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or_else(|| StateError::Invalid("invalid queued forget id".into()))?;
        if entry.file_name() != format!("{id}.json").as_str() {
            return Err(StateError::Invalid(
                "forget id does not match queue filename".into(),
            ));
        }
        let selector = ForgetSelector::from_value(&value["selector"])?;
        let result = if let Some(root) = &selector.memory_root {
            transport
                .snapshot_request(
                    "/v1/snapshots/forget",
                    &json!({"root":root,"machine":machine,
                "before_wall_ms":selector.before_ns.map(|before| before / 1_000_000)}),
                )
                .and_then(|response| {
                    if [
                        "removed_manifests",
                        "removed_blobs",
                        "retained_referenced_blobs",
                    ]
                    .iter()
                    .any(|field| response[*field].as_u64().is_none())
                    {
                        Err(TransportError::InvalidAck(
                            "snapshot forget counters missing".into(),
                        ))
                    } else {
                        Ok(())
                    }
                })
        } else {
            transport.forget(id, &selector, machine).map(|_| ())
        };
        match result {
            Ok(()) => {
                match fs::remove_file(entry.path()) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                #[cfg(unix)]
                fs::File::open(&dir)?.sync_all()?;
                report.acknowledged += 1;
            }
            Err(error) => {
                report.pending += 1;
                report.failures.push(error.to_string());
            }
        }
    }
    Ok(report)
}

/// Start of a UTC calendar day, in the frame's nanosecond clock.
pub fn forget_before_day(day: &str) -> Result<u64, String> {
    let parts: Vec<_> = day.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err("--before needs YYYY-MM-DD".into());
    }
    let parse = |part: &str| {
        part.parse::<u64>()
            .map_err(|_| "--before needs YYYY-MM-DD".to_owned())
    };
    let (year, month, date) = (parse(parts[0])?, parse(parts[1])?, parse(parts[2])?);
    let leap = |year| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1970..=2554).contains(&year)
        || !(1..=12).contains(&month)
        || date == 0
        || date > months[(month - 1) as usize]
    {
        return Err("--before is not a supported UTC date".into());
    }
    let days = (1970..year)
        .map(|year| if leap(year) { 366 } else { 365 })
        .sum::<u64>()
        + months[..(month - 1) as usize].iter().sum::<u64>()
        + date
        - 1;
    days.checked_mul(86_400_000_000_000)
        .ok_or_else(|| "--before exceeds the frame clock".into())
}

pub(crate) fn validate_request_id(id: &str) -> Result<(), crate::ReceiveError> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(crate::ReceiveError::RequestField("id"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Stub {
        online: Cell<bool>,
        scope: &'static str,
        requests: RefCell<Vec<String>>,
    }
    impl Transport for Stub {
        fn send(&self, _: &crate::Frame) -> Result<u64, TransportError> {
            unreachable!()
        }
        fn deletion_scope(&self) -> String {
            self.scope.into()
        }
        fn forget(
            &self,
            id: &str,
            _: &ForgetSelector,
            _: &str,
        ) -> Result<ForgetReport, TransportError> {
            self.requests.borrow_mut().push(id.into());
            if self.online.get() {
                Ok(ForgetReport::default())
            } else {
                Err(TransportError::Unavailable("offline".into()))
            }
        }
    }
    #[test]
    fn requests_survive_failures_restart_and_receiver_changes_until_acknowledged() {
        let root = std::env::temp_dir().join(format!(
            "relay-delete-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let state = root.join("state.json");
        let transport = Stub {
            online: Cell::new(false),
            scope: "receiver-a",
            requests: RefCell::default(),
        };
        let selector = ForgetSelector {
            session: Some("session".into()),
            ..Default::default()
        };
        let first = queue_forget(&state, &transport.deletion_scope(), &selector).unwrap();
        assert_eq!(
            flush_forgets(&state, "machine", &transport)
                .unwrap()
                .pending,
            1
        );
        let second = queue_forget(&state, &transport.deletion_scope(), &selector).unwrap();
        let other = Stub {
            online: Cell::new(true),
            scope: "receiver-b",
            requests: RefCell::default(),
        };
        assert_eq!(
            flush_forgets(&state, "machine", &other).unwrap(),
            ForgetFlushReport::default()
        );
        assert!(other.requests.borrow().is_empty());
        transport.online.set(true);
        assert_eq!(
            flush_forgets(&state, "machine", &transport)
                .unwrap()
                .acknowledged,
            2
        );
        assert!(transport.requests.borrow().contains(&first));
        assert!(transport.requests.borrow().contains(&second));
        assert_eq!(
            flush_forgets(&state, "machine", &transport).unwrap(),
            ForgetFlushReport::default()
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn memory_cutoff_is_fixed_and_queue_files_are_private() {
        let root = std::env::temp_dir().join(format!(
            "relay-memory-delete-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let state = root.join("state.json");
        let selector = ForgetSelector {
            memory_root: Some("a".repeat(64)),
            ..Default::default()
        };
        let id = queue_forget(&state, "receiver", &selector).unwrap();
        let dir = queue_dir(&state, "receiver");
        let path = dir.join(format!("{id}.json"));
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(value["selector"]["before_ns"].as_u64().unwrap() > 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn dates_are_utc_validated_and_selectors_never_default_to_everything() {
        assert_eq!(forget_before_day("1970-01-01").unwrap(), 0);
        assert_eq!(forget_before_day("1970-01-02").unwrap(), 86_400_000_000_000);
        assert!(forget_before_day("2026-02-29").is_err());
        assert!(forget_before_day("2024-02-29").is_ok());
        assert!(forget_before_day("2026-13-01").is_err());
        assert!(ForgetSelector::default().validate().is_err());
    }
}
