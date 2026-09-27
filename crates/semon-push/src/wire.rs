//! The mirror protocol's wire forms (docs/mirror-protocol.md): request and
//! response bodies, standard base64 for the bytes, and the head hash.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How many bytes of a file its head hash covers.
pub const HEAD_BYTES: usize = 4096;
/// The most file bytes one append carries.
pub const CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// `POST <url>/v1/mirror/append`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Append {
    pub root: String,
    pub path: String,
    pub offset: u64,
    /// SHA-256 (lowercase hex) of the first `min(4096, offset + len)` bytes
    /// the file holds once this append is applied.
    pub head_sha256: String,
    /// The bytes, standard base64 with padding.
    pub bytes: String,
    /// Truncate the file first; `offset` is then 0.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

/// The receiver's answer to an append: 200 with the new length, or 409 with
/// its current length and head hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Length {
    pub length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha256: Option<String>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The head hash of a file whose first bytes are `prefix` (only the first
/// [`HEAD_BYTES`] count).
pub fn head_sha256(prefix: &[u8]) -> String {
    sha256_hex(&prefix[..prefix.len().min(HEAD_BYTES)])
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let value = |byte: u8| -> Option<u32> {
        let sextet = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        Some(u32::from(sextet))
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index == bytes.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0;
        for byte in &chunk[..4 - pad] {
            n = (n << 6) | value(*byte)?;
        }
        n <<= 6 * pad as u32;
        let decoded = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&decoded[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_and_matches_the_standard() {
        for (raw, encoded) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(raw), encoded);
            assert_eq!(base64_decode(encoded).unwrap(), raw);
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
        for bad in ["Zg=", "Zg==Zg==", "Z===", "Zm9v!A==", "===="] {
            assert!(base64_decode(bad).is_none(), "{bad}");
        }
    }
}
