//! Hashed push bearer custody.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
pub use zeroize::Zeroizing;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[serde(rename = "push")]
    MirrorPush,
}
/// No serializer; exposure is explicit for future protected provisioning only.
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn generate() -> Result<Self, crate::enrollment::Error> {
        Ok(Self::new(crate::enrollment::random_id()?))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_bytes()).into()
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bearer<K = Kind> {
    pub credential_id: String,
    pub enrollment_id: String,
    pub kind: K,
    pub sha256: [u8; 32],
    pub generation: u64,
    /// Inclusive Unix seconds.
    pub valid_from: u64,
    /// Exclusive Unix seconds.
    pub valid_until: u64,
    pub revoked_at: Option<u64>,
}
pub(crate) fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for i in 0..32 {
        difference |= a[i] ^ b[i];
    }
    std::hint::black_box(difference) == 0
}
