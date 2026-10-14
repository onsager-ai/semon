//! Content-free SHA-256 continuation at a whole proof-chunk boundary. The
//! standard compression function is provided by sha2; never persist raw tails.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrefixState {
    pub(crate) at: u64,
    words: [u32; 8],
}
impl Default for PrefixState {
    fn default() -> Self {
        Self {
            at: 0,
            words: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
        }
    }
}

fn compress(words: &mut [u32; 8], bytes: &[u8]) {
    debug_assert_eq!(bytes.len() % 64, 0);
    for bytes in bytes.as_chunks::<64>().0 {
        let block = (*bytes).into();
        sha2::compress256(words, std::slice::from_ref(&block));
    }
}
impl PrefixState {
    pub(crate) fn advance(&mut self, bytes: &[u8]) {
        assert_eq!(bytes.len() % super::SIZE, 0);
        self.advance_blocks(bytes);
    }
    pub(crate) fn advance_blocks(&mut self, bytes: &[u8]) {
        assert_eq!(bytes.len() % 64, 0);
        compress(&mut self.words, bytes);
        self.at += bytes.len() as u64;
    }
    pub(crate) fn finish(&self, tail: &[u8]) -> [u8; 32] {
        let mut words = self.words;
        let full = tail.len() / 64 * 64;
        compress(&mut words, &tail[..full]);
        let mut padded = tail[full..].to_vec();
        padded.push(0x80);
        while padded.len() % 64 != 56 {
            padded.push(0);
        }
        padded.extend_from_slice(
            &(self.at.wrapping_add(tail.len() as u64).wrapping_mul(8)).to_be_bytes(),
        );
        compress(&mut words, &padded);
        let mut result = [0; 32];
        for (part, word) in result.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            part.copy_from_slice(&word.to_be_bytes());
        }
        result
    }
}
