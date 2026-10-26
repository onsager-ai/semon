//! Host-owned Source identity and stream binding, outside the cache database.
use super::{LogicalSourceNamespace, StorageError, StoreBackend, StoreConfig};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostSource {
    namespace: String,
    stream_epoch: String,
}

impl HostSource {
    /// Hosted callers supply the actual MachineSourceUUID and host-owned epoch.
    pub fn new(
        namespace: LogicalSourceNamespace,
        stream_epoch: String,
    ) -> Result<Self, StorageError> {
        if stream_epoch.len() != 64 || !stream_epoch.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidArguments);
        }
        Ok(Self {
            namespace: namespace.as_str().to_owned(),
            stream_epoch,
        })
    }

    pub fn namespace(&self) -> Result<LogicalSourceNamespace, StorageError> {
        LogicalSourceNamespace::new(self.namespace.clone())
    }

    /// Standalone identity is persisted by the host configuration, never by a
    /// fresh cache or its physical location. Concurrent creators publish once.
    pub fn load_or_create(path: &Path) -> std::io::Result<Self> {
        use std::io;
        fn read(path: &Path) -> io::Result<HostSource> {
            let file = fs::File::open(path)?;
            let mut bytes = Vec::new();
            file.take(4097).read_to_end(&mut bytes)?;
            if bytes.len() > 4096 {
                return Err(io::Error::other("invalid host Source binding"));
            }
            let source: HostSource = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            HostSource::new(
                source.namespace().map_err(io::Error::other)?,
                source.stream_epoch,
            )
            .map_err(io::Error::other)
        }
        match read(path) {
            Ok(source) => return Ok(source),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("host Source config has no parent"))?;
        fs::create_dir_all(parent)?;
        let mut entropy = [0u8; 48];
        fs::File::open("/dev/urandom")?.read_exact(&mut entropy)?;
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let id = hex(&entropy[..16]);
        let source = Self::new(
            LogicalSourceNamespace::new(format!(
                "{}-{}-{}-{}-{}",
                &id[..8],
                &id[8..12],
                &id[12..16],
                &id[16..20],
                &id[20..]
            ))
            .map_err(io::Error::other)?,
            hex(&entropy[16..]),
        )
        .map_err(io::Error::other)?;
        let temporary = parent.join(format!(".source-{}-{}", std::process::id(), id));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(&source).map_err(io::Error::other)?)?;
        file.sync_all()?;
        let result = fs::hard_link(&temporary, path);
        fs::remove_file(&temporary)?;
        match result {
            Ok(()) => {
                fs::File::open(parent)?.sync_all()?;
                Ok(source)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read(path),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn config(&self, cache: &Path) -> Result<StoreConfig, StorageError> {
        Ok(StoreConfig {
            backend: StoreBackend::Sqlite {
                database: super::local_database(cache),
            },
            namespace: self.namespace()?,
            expected_stream_epoch: Some(self.stream_epoch.clone()),
        })
    }
}
