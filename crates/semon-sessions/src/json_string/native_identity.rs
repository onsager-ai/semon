//! Content-free continuation for exact native field identity comparisons.
//! A digest is an indexed candidate filter; only Equal after reading both
//! complete qualified descriptors grants the native producer equality.
//! The caller owns source generations, Records authority and post-I/O fences.

use super::{JSON_STRING_CHUNK_DECODED_MAX, invalid, scalar};
use crate::source_chunks::PrefixState;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stream {
    start: u64,
    end: u64,
    pub(crate) at: u64,
    // A compression/comparison boundary may split the UTF-8 encoding of one
    // scalar. Re-read that scalar and skip these decoded bytes; never save a
    // raw or decoded tail in the durable cursor.
    skip: u8,
}
pub(crate) struct Decoded {
    pub(crate) bytes: Vec<u8>,
    pub(crate) consumed: usize,
    pub(crate) complete: bool,
}
impl Stream {
    pub(crate) fn new(start: u64, end: u64) -> io::Result<Self> {
        if start > end {
            return Err(invalid());
        }
        Ok(Self {
            start,
            end,
            at: start,
            skip: 0,
        })
    }
    pub(crate) fn complete(&self) -> bool {
        self.at == self.end && self.skip == 0
    }
    /// Input starts at at and contains only descriptor content, excluding its
    /// quotes. At most max decoded bytes become request-local output, even
    /// when a native identifier or escaped key has arbitrary length.
    pub(crate) fn step(&mut self, bytes: &[u8], max: usize) -> io::Result<Decoded> {
        if max == 0 || max > JSON_STRING_CHUNK_DECODED_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native identity decoded budget must be 1..=128 KiB",
            ));
        }
        if self.at < self.start || self.at > self.end || self.skip > 3 {
            return Err(invalid());
        }
        let remaining = self.end - self.at;
        let input = &bytes[..bytes
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX))];
        let at_end = input.len() as u64 == remaining;
        let mut decoded = Vec::with_capacity(input.len().min(max));
        let mut consumed = 0;
        while consumed < input.len() && decoded.len() < max {
            let Some((value, width)) = scalar(&input[consumed..], at_end)? else {
                break;
            };
            let mut scratch = [0; 4];
            let encoded = value.encode_utf8(&mut scratch).as_bytes();
            let skip = usize::from(self.skip);
            if skip >= encoded.len() {
                return Err(invalid());
            }
            let take = (max - decoded.len()).min(encoded.len() - skip);
            decoded.extend_from_slice(&encoded[skip..skip + take]);
            if skip + take == encoded.len() {
                consumed += width;
                self.at = self.at.checked_add(width as u64).ok_or_else(invalid)?;
                self.skip = 0;
            } else {
                self.skip += take as u8;
            }
        }
        if self.at == self.end && self.skip != 0 {
            return Err(invalid());
        }
        Ok(Decoded {
            bytes: decoded,
            consumed,
            complete: self.complete(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fingerprint {
    pub(crate) stream: Stream,
    hash: PrefixState,
    receipt: Option<Receipt>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Receipt {
    pub(crate) decoded_bytes: u64,
    pub(crate) sha256: [u8; 32],
}
impl Fingerprint {
    pub(crate) fn new(start: u64, end: u64) -> io::Result<Self> {
        Ok(Self {
            stream: Stream::new(start, end)?,
            hash: PrefixState::default(),
            receipt: None,
        })
    }
    pub(crate) fn receipt(&self) -> Option<Receipt> {
        self.receipt
    }
    /// Persist only at an SHA-256 block boundary. If a provider fragment ends
    /// earlier, replay its sub-block tail on the next attempt from the saved
    /// scalar/skip cursor. One complete final tail is hashed once at finish.
    pub(crate) fn step(&mut self, bytes: &[u8], max: usize) -> io::Result<usize> {
        if self.receipt.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native identity fingerprint already complete",
            ));
        }
        if max < 64 || max > JSON_STRING_CHUNK_DECODED_MAX || max % 64 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native identity hash budget must be a 64-byte multiple",
            ));
        }
        let before = self.stream.clone();
        let mut decoded = self.stream.step(bytes, max)?;
        if !decoded.complete && decoded.bytes.len() % 64 != 0 {
            let full = decoded.bytes.len() / 64 * 64;
            self.stream = before;
            if full == 0 {
                return Ok(0);
            }
            decoded = self.stream.step(bytes, full)?;
        }
        if decoded.complete {
            self.receipt = Some(Receipt {
                decoded_bytes: self
                    .hash
                    .at
                    .checked_add(decoded.bytes.len() as u64)
                    .ok_or_else(invalid)?,
                sha256: self.hash.finish(&decoded.bytes),
            });
        } else {
            self.hash.advance_blocks(&decoded.bytes);
        }
        Ok(decoded.consumed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Comparison {
    Equal,
    Different,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Compare {
    pub(crate) left: Stream,
    pub(crate) right: Stream,
    pub(crate) compared: u64,
    pub(crate) result: Option<Comparison>,
}
impl Compare {
    pub(crate) fn new(left: (u64, u64), right: (u64, u64)) -> io::Result<Self> {
        Ok(Self {
            left: Stream::new(left.0, left.1)?,
            right: Stream::new(right.0, right.1)?,
            compared: 0,
            result: None,
        })
    }
    /// Compares canonical decoded bytes, including escaped keys and Unicode.
    /// A candidate digest match is never accepted here without native bytes.
    /// Partial fragments advance only the common compared prefix; both tails
    /// are replayed from content-free cursors rather than saved as text.
    pub(crate) fn step(
        &mut self,
        left: &[u8],
        right: &[u8],
        max: usize,
    ) -> io::Result<(usize, usize)> {
        if self.result.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native identity comparison already complete",
            ));
        }
        let old_left = self.left.clone();
        let old_right = self.right.clone();
        let mut l = self.left.step(left, max)?;
        let mut r = self.right.step(right, max)?;
        let common = l.bytes.len().min(r.bytes.len());
        if l.bytes[..common] != r.bytes[..common] {
            self.result = Some(Comparison::Different);
            return Ok((l.consumed, r.consumed));
        }
        if (l.complete && l.bytes.len() < r.bytes.len())
            || (r.complete && r.bytes.len() < l.bytes.len())
        {
            self.result = Some(Comparison::Different);
            return Ok((l.consumed, r.consumed));
        }
        if common == 0 {
            self.left = old_left;
            self.right = old_right;
            if l.complete && r.complete {
                self.result = Some(Comparison::Equal)
            }
            return Ok((0, 0));
        }
        if l.bytes.len() != common {
            self.left = old_left;
            l = self.left.step(left, common)?;
        }
        if r.bytes.len() != common {
            self.right = old_right;
            r = self.right.step(right, common)?;
        }
        self.compared = self
            .compared
            .checked_add(common as u64)
            .ok_or_else(invalid)?;
        if l.complete && r.complete {
            self.result = Some(Comparison::Equal)
        }
        Ok((l.consumed, r.consumed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn encoded(text: &str) -> Vec<u8> {
        let bytes = serde_json::to_vec(text).unwrap();
        bytes[1..bytes.len() - 1].to_vec()
    }
    #[test]
    fn fingerprint_resumes_at_byte_boundaries_without_decoded_tail() {
        let text = format!("{}\n{}", "aé𝄞".repeat(1000), "字".repeat(1000));
        let bytes = encoded(&text);
        for fragment in [17, 64, 91, 4096] {
            let mut cursor = Fingerprint::new(0, bytes.len() as u64).unwrap();
            while cursor.receipt().is_none() {
                let at = cursor.stream.at as usize;
                // A sub-block fragment requests more next time rather than
                // storing its native text in the continuation.
                let mut end = (at + fragment).min(bytes.len());
                loop {
                    let previous = cursor.clone();
                    cursor.step(&bytes[at..end], 64).unwrap();
                    if cursor != previous {
                        break;
                    }
                    end = (end + fragment).min(bytes.len());
                    assert!(end > at);
                }
                let saved = serde_json::to_vec(&cursor).unwrap();
                assert!(saved.len() < 512);
                cursor = serde_json::from_slice(&saved).unwrap();
            }
            assert_eq!(
                cursor.receipt(),
                Some(Receipt {
                    decoded_bytes: text.len() as u64,
                    sha256: Sha256::digest(text.as_bytes()).into()
                })
            );
        }
    }
    #[test]
    fn equal_digest_candidates_need_exact_native_comparison() {
        let text = format!("{}tail", "é𝄞".repeat(1000));
        let left = encoded(&text);
        let right = text
            .chars()
            .map(|c| {
                if c.len_utf16() == 1 {
                    format!("\\u{:04x}", c as u32)
                } else {
                    let mut utf16 = [0; 2];
                    c.encode_utf16(&mut utf16);
                    format!("\\u{:04x}\\u{:04x}", utf16[0], utf16[1])
                }
            })
            .collect::<String>()
            .into_bytes();
        let mut cursor = Compare::new((0, left.len() as u64), (0, right.len() as u64)).unwrap();
        while cursor.result.is_none() {
            let l = cursor.left.at as usize;
            let r = cursor.right.at as usize;
            cursor
                .step(
                    &left[l..(l + 97).min(left.len())],
                    &right[r..(r + 97).min(right.len())],
                    17,
                )
                .unwrap();
            let state = serde_json::to_vec(&cursor).unwrap();
            assert!(state.len() < 512);
            cursor = serde_json::from_slice(&state).unwrap();
        }
        assert_eq!(cursor.result, Some(Comparison::Equal));
        assert_eq!(cursor.compared, text.len() as u64);
        let longer = encoded(&(text.clone() + "more"));
        let mut cursor = Compare::new((0, left.len() as u64), (0, longer.len() as u64)).unwrap();
        while cursor.result.is_none() {
            cursor
                .step(
                    &left[cursor.left.at as usize..],
                    &longer[cursor.right.at as usize..],
                    64,
                )
                .unwrap();
        }
        assert_eq!(cursor.result, Some(Comparison::Different));
    }
    #[test]
    fn decoded_page_residency_does_not_grow_with_identity_length() {
        for size in [0, 128, 1024, 8192] {
            let bytes = encoded(&"𝄞é".repeat(size));
            let mut stream = Stream::new(0, bytes.len() as u64).unwrap();
            let mut actual = Vec::new();
            while !stream.complete() {
                let page = stream.step(&bytes[stream.at as usize..], 64).unwrap();
                assert!(page.bytes.len() <= 64);
                actual.extend(page.bytes);
                assert!(serde_json::to_vec(&stream).unwrap().len() < 128);
            }
            assert_eq!(actual, "𝄞é".repeat(size).as_bytes());
        }
    }
}
