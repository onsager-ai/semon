//! Operator grants for caller-authenticated users. Push tokens are not principals.
//! Unix custody only; no authentication mechanism or body-read transport lives here.

use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::body_cache::CacheDirectory;

pub const ACL_FILE: &str = "grants.json";
const FILE_CAP: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Analytics,
    BodyRead,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    principal: String,
    machine: String,
    namespace: String,
    permissions: Vec<Permission>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: u32,
    grants: Vec<Entry>,
}

/// Opaque evidence of a receiver decision, not origin authority or a cache admission.
#[derive(Debug, PartialEq, Eq)]
pub struct Grant {
    version: String,
}

impl Grant {
    /// SHA-256 of the canonical format/scope/principal/permission-set grant.
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Granted(Grant),
    /// Uniform for missing/invalid policy, unknown subjects/scopes and no permission.
    Denied,
}

/// A validated immutable policy; evaluation does no I/O and reveals only one denial.
pub struct Policy {
    grants: Vec<Entry>,
}

impl Policy {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() as u64 > FILE_CAP {
            return None;
        }
        let document: Document = serde_json::from_slice(bytes).ok()?;
        if document.format != 1 {
            return None;
        }
        let mut scopes = BTreeSet::new();
        for entry in &document.grants {
            if !subject(&entry.principal)
                || !semon_sessions::is_machine_name(&entry.machine)
                || !identifier(&entry.namespace)
                || entry.permissions.is_empty()
                || entry.permissions.iter().collect::<BTreeSet<_>>().len()
                    != entry.permissions.len()
                || !scopes.insert((&entry.principal, &entry.machine, &entry.namespace))
            {
                return None;
            }
        }
        let mut grants = document.grants;
        for entry in &mut grants {
            entry.permissions.sort();
        }
        Some(Self { grants })
    }

    pub fn decide(
        &self,
        principal: &str,
        machine: &str,
        namespace: &str,
        permission: Permission,
    ) -> Decision {
        let Some(entry) = self.grants.iter().find(|entry| {
            entry.principal == principal
                && entry.machine == machine
                && entry.namespace == namespace
                && entry.permissions.contains(&permission)
        }) else {
            return Decision::Denied;
        };
        // Struct field order and sorted permissions define the canonical representation.
        let canonical = serde_json::to_vec(&(1u32, entry)).expect("grant serialization");
        Decision::Granted(Grant {
            version: hex::encode(Sha256::digest(canonical)),
        })
    }
}

fn subject(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Read-at-decision: revocation/replacement is effective on the next call.
/// Missing, malformed, insecure or unreadable ACLs all produce `Denied`.
/// The operator creates a dedicated 0700 directory and edits its 0600 grants file.
pub struct Acls {
    dir: PathBuf,
}

impl Acls {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn decide(
        &self,
        principal: &str,
        machine: &str,
        namespace: &str,
        permission: Permission,
    ) -> Decision {
        let load = || -> Option<Policy> {
            let dir = CacheDirectory::open(&self.dir).ok()?;
            let file = dir.open_file(Path::new(ACL_FILE), libc::O_RDONLY).ok()?;
            let mut bytes = Vec::new();
            file.take(FILE_CAP + 1).read_to_end(&mut bytes).ok()?;
            Policy::parse(&bytes)
        };
        load().map_or(Decision::Denied, |policy| {
            policy.decide(principal, machine, namespace, permission)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
        sync::atomic::{AtomicU64, Ordering},
    };

    const GRANTS: &str = r#"{"format":1,"grants":[{"principal":"alice","machine":"laptop","namespace":"work","permissions":["analytics"]},{"principal":"bob","machine":"laptop","namespace":"work","permissions":["body_read"]}]}"#;
    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub(super) struct Fixture(pub PathBuf);
    impl Fixture {
        pub(super) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "receiver-acl-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn write(&self, text: &str) {
            fs::write(self.0.join(ACL_FILE), text).unwrap();
            fs::set_permissions(self.0.join(ACL_FILE), fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn permissions_and_uniform_denials() {
        let policy = Policy::parse(GRANTS.as_bytes()).unwrap();
        assert!(matches!(
            policy.decide("alice", "laptop", "work", Permission::Analytics),
            Decision::Granted(_)
        ));
        assert!(matches!(
            policy.decide("bob", "laptop", "work", Permission::BodyRead),
            Decision::Granted(_)
        ));
        for (principal, machine, namespace, permission) in [
            ("alice", "laptop", "work", Permission::BodyRead),
            ("bob", "laptop", "work", Permission::Analytics),
            ("unknown", "laptop", "work", Permission::Analytics),
            ("alice", "other", "work", Permission::Analytics),
            ("alice", "laptop", "other", Permission::Analytics),
        ] {
            assert_eq!(
                policy.decide(principal, machine, namespace, permission),
                Decision::Denied
            );
        }
    }

    #[test]
    fn invalid_documents_refuse_and_versions_bind_permission_set() {
        let base = serde_json::from_str::<serde_json::Value>(GRANTS).unwrap();
        let mut unknown = base.clone();
        unknown["extra"] = true.into();
        let mut duplicate = base.clone();
        duplicate["grants"]
            .as_array_mut()
            .unwrap()
            .push(base["grants"][0].clone());
        let mut permission = base.clone();
        permission["grants"][0]["permissions"] = serde_json::json!(["analytics", "analytics"]);
        let mut entry_extra = base.clone();
        entry_extra["grants"][0]["extra"] = true.into();
        for bad in [unknown, duplicate, permission, entry_extra] {
            assert!(Policy::parse(&serde_json::to_vec(&bad).unwrap()).is_none());
        }
        assert!(Policy::parse(b"invalid").is_none());
        assert!(Policy::parse(br#"{"format":1,"format":1,"grants":[]}"#).is_none());
        let before = Policy::parse(GRANTS.as_bytes()).unwrap().decide(
            "alice",
            "laptop",
            "work",
            Permission::Analytics,
        );
        let changed = GRANTS.replacen("[\"analytics\"]", "[\"body_read\",\"analytics\"]", 1);
        let after = Policy::parse(changed.as_bytes()).unwrap().decide(
            "alice",
            "laptop",
            "work",
            Permission::Analytics,
        );
        assert_ne!(before, after);
        let reordered = changed.replacen(
            "[\"body_read\",\"analytics\"]",
            "[\"analytics\",\"body_read\"]",
            1,
        );
        assert_eq!(
            after,
            Policy::parse(reordered.as_bytes()).unwrap().decide(
                "alice",
                "laptop",
                "work",
                Permission::Analytics
            )
        );
    }

    #[test]
    fn reload_modes_symlinks_and_push_tokens() {
        let fixture = Fixture::new();
        let acls = Acls::new(fixture.0.clone());
        let decide = || acls.decide("alice", "laptop", "work", Permission::Analytics);
        assert_eq!(decide(), Decision::Denied);
        fixture.write(GRANTS);
        assert!(matches!(decide(), Decision::Granted(_)));
        fixture.write(r#"{"format":1,"grants":[]}"#);
        assert_eq!(decide(), Decision::Denied);
        fixture.write(GRANTS);
        fs::set_permissions(fixture.0.join(ACL_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(decide(), Decision::Denied);
        fixture.write(GRANTS);
        fs::rename(fixture.0.join(ACL_FILE), fixture.0.join("saved")).unwrap();
        symlink("saved", fixture.0.join(ACL_FILE)).unwrap();
        assert_eq!(decide(), Decision::Denied);
        fs::remove_file(fixture.0.join(ACL_FILE)).unwrap();
        fixture.write(GRANTS);
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(decide(), Decision::Denied);
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        let token = crate::tokens::add_token(&fixture.0, "laptop").unwrap();
        assert_eq!(
            acls.decide(&token, "laptop", "work", Permission::BodyRead),
            Decision::Denied
        );
        assert_eq!(
            acls.decide("laptop", "laptop", "work", Permission::BodyRead),
            Decision::Denied
        );
        let alias = Fixture::new();
        symlink(&fixture.0, alias.0.join("link")).unwrap();
        assert_eq!(
            Acls::new(alias.0.join("link")).decide(
                "alice",
                "laptop",
                "work",
                Permission::Analytics
            ),
            Decision::Denied
        );
    }
}
