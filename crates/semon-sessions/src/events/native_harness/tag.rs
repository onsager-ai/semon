//! Native signal tag classification. Only its existing allowlisted ASCII
//! prefix is retained; the supplied qualified whole-value SHA is the native
//! display suffix, never identity/equality authority.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    prefix: String,
    length: u64,
    invalid: bool,
}
impl Cursor {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        if self.invalid {
            return;
        }
        for &byte in bytes {
            if !(byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b'-' | b'.' | b':' | b'/' | b'[' | b']' | b'+' | b' '
                ))
            {
                self.invalid = true;
                break;
            }
            if self.prefix.len() < super::super::TAG_MAX {
                self.prefix.push(char::from(byte));
            }
            self.length = self.length.saturating_add(1);
        }
    }
    pub(crate) fn rejected(&self) -> bool {
        self.invalid
    }
    pub(crate) fn result(&self, digest: &[u8; 32]) -> Option<String> {
        if self.invalid || self.length == 0 {
            return None;
        }
        Some(if self.length > super::super::TAG_MAX as u64 {
            let suffix: String = digest[..4]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            format!("{}#{suffix}", &self.prefix[..super::super::TAG_MAX - 9])
        } else {
            self.prefix.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn reference(text: &str) -> Option<String> {
        if text.is_empty()
            || !text.bytes().all(|b| {
                b.is_ascii_alphanumeric()
                    || matches!(
                        b,
                        b'_' | b'-' | b'.' | b':' | b'/' | b'[' | b']' | b'+' | b' '
                    )
            })
        {
            return None;
        }
        Some(if text.len() > 64 {
            format!("{}#{}", &text[..55], crate::events::short_hash(text))
        } else {
            text.to_owned()
        })
    }
    #[test]
    fn streamed_tags_preserve_native_classification_and_hash_suffix() {
        for size in [0, 1, 55, 64, 65, 128, 1024, 8192, 200_000] {
            for text in [
                "m".repeat(size),
                format!("{}λ", "m".repeat(size)),
                format!("{}\n", "m".repeat(size)),
                format!("{} /a-b_[x]+.1:", "m".repeat(size)),
            ] {
                let mut cursor = Cursor::default();
                for part in text.as_bytes().chunks(127) {
                    cursor.push(part);
                    cursor = serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
                }
                assert_eq!(
                    cursor.result(&Sha256::digest(text.as_bytes()).into()),
                    reference(&text)
                );
                assert!(serde_json::to_vec(&cursor).unwrap().len() < 256);
                assert_eq!(crate::events::bounded(&text), reference(&text));
            }
        }
    }
}
