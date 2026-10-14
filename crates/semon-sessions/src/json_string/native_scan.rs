//! Resumable scalar-boundary production for a native JSON string.
//!
//! The cursor contains offsets only. A split UTF-8/escape/surrogate scalar is
//! retried from its beginning, so durable metadata never stores source text.
//! Checkpoints are emitted separately and may be committed as bounded rows.

use super::{invalid, scalar};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    /// First byte after the opening quote, relative to the native record.
    pub(crate) start: u64,
    pub(crate) at: u64,
    checkpoint: u64,
    complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Step {
    /// Bytes consumed from this supplied range; includes a closing quote.
    pub(crate) consumed: usize,
    /// The closing quote's record-relative address, if observed.
    pub(crate) end: Option<u64>,
}

impl Cursor {
    pub(crate) fn new(start: u64) -> Self {
        Self {
            start,
            at: start,
            checkpoint: start,
            complete: false,
        }
    }

    /// `bytes` starts exactly at `at`. The caller binds this cursor and every
    /// emitted checkpoint to the same authorized source/publication generation.
    /// `final_input` means that no further native bytes exist, not a page stop.
    /// A complete scalar may cross `budget` by at most eleven raw bytes.
    pub(crate) fn step(
        &mut self,
        bytes: &[u8],
        final_input: bool,
        budget: usize,
        checkpoint_bytes: usize,
        mut emit: impl FnMut(u64) -> io::Result<()>,
    ) -> io::Result<Step> {
        if self.complete
            || budget == 0
            || checkpoint_bytes == 0
            || self.checkpoint < self.start
            || self.at < self.checkpoint
        {
            return Err(invalid());
        }
        let mut used = 0;
        while used < bytes.len() && used < budget {
            if bytes[used] == b'"' {
                if self.checkpoint != self.at {
                    emit(self.at)?;
                    self.checkpoint = self.at;
                }
                self.complete = true;
                return Ok(Step {
                    consumed: used + 1,
                    end: Some(self.at),
                });
            }
            let remaining = &bytes[used..];
            let since = usize::try_from(self.at - self.checkpoint).map_err(|_| invalid())?;
            let ascii_limit = remaining
                .len()
                .min(budget - used)
                .min(checkpoint_bytes.saturating_sub(since));
            let ascii = remaining[..ascii_limit]
                .iter()
                .position(|byte| *byte < 0x20 || *byte >= 0x80 || matches!(byte, b'"' | b'\\'))
                .unwrap_or(ascii_limit);
            let width = if ascii > 0 {
                ascii
            } else {
                let Some((_, width)) = scalar(remaining, final_input)? else {
                    return Ok(Step {
                        consumed: used,
                        end: None,
                    });
                };
                width
            };
            used += width;
            self.at = self.at.checked_add(width as u64).ok_or_else(invalid)?;
            if self.at - self.checkpoint >= checkpoint_bytes as u64 {
                emit(self.at)?;
                self.checkpoint = self.at;
            }
        }
        if final_input && used == bytes.len() {
            return Err(invalid());
        }
        Ok(Step {
            consumed: used,
            end: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_at_every_scalar_split_preserves_complete_checkpoints() {
        let bytes = b"abc\\uD83D\\uDE00\\n\xe4\xb8\xadxyz\"";
        let mut whole = Cursor::new(17);
        let mut expected = vec![17];
        let step = whole
            .step(bytes, true, bytes.len(), 5, |at| {
                expected.push(at);
                Ok(())
            })
            .unwrap();
        assert_eq!(step.end, Some(17 + bytes.len() as u64 - 1));
        for split in 1..bytes.len() {
            let mut cursor = Cursor::new(17);
            let mut checkpoints = vec![17];
            let first = cursor
                .step(&bytes[..split], false, split, 5, |at| {
                    checkpoints.push(at);
                    Ok(())
                })
                .unwrap();
            let saved = serde_json::to_vec(&cursor).unwrap();
            let mut resumed: Cursor = serde_json::from_slice(&saved).unwrap();
            let second = resumed
                .step(&bytes[first.consumed..], true, bytes.len(), 5, |at| {
                    checkpoints.push(at);
                    Ok(())
                })
                .unwrap();
            assert_eq!(second.end, step.end, "split {split}");
            assert_eq!(checkpoints, expected, "split {split}");
            assert_eq!(first.consumed + second.consumed, bytes.len());
        }
    }

    #[test]
    fn large_string_steps_keep_offsets_only_and_emit_bounded_checkpoints() {
        for history in [0, 128, 1024, 8192] {
            let start = history * 1024;
            let mut cursor = Cursor::new(start);
            let input = vec![b'a'; 128 * 1024];
            let mut emitted = 0;
            let step = cursor
                .step(&input, false, 64 * 1024, 64 * 1024, |_| {
                    emitted += 1;
                    Ok(())
                })
                .unwrap();
            assert_eq!(step.consumed, 64 * 1024);
            assert_eq!(step.end, None);
            assert_eq!(emitted, 1);
            assert!(serde_json::to_vec(&cursor).unwrap().len() < 128);
        }
    }

    #[test]
    fn invalid_and_unterminated_scalars_refuse_completion() {
        for bytes in [
            b"\\uD800x\"".as_slice(),
            b"\xc0\xaf\"",
            b"unfinished",
            b"line\n\"",
        ] {
            let mut cursor = Cursor::new(0);
            assert!(
                cursor
                    .step(bytes, true, bytes.len(), 4, |_| Ok(()))
                    .is_err()
            );
        }
    }
}
