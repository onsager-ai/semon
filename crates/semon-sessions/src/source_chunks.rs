//! Content-free proofs for bounded reads of an indexed native generation.
//! These belong to the existing event ledger, never to the canonical trace store.
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom};

pub(crate) const SIZE: usize = 64 * 1024;

#[cfg(test)]
thread_local! {
    pub(crate) static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chunks {
    pub(crate) offset: u64,
    pub(crate) hashes: Vec<[u8; 32]>,
}

impl Chunks {
    pub(crate) fn valid(&self, offset: u64) -> bool {
        self.offset == offset
            && usize::try_from(offset.div_ceil(SIZE as u64)) == Ok(self.hashes.len())
    }

    pub(crate) fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(self.offset.to_be_bytes());
        for part in &self.hashes {
            hash.update(part);
        }
        hash.finalize().into()
    }

    pub(crate) fn read(&self, file: &mut (impl Read + Seek), index: usize) -> io::Result<Vec<u8>> {
        self.hashes.get(index).ok_or_else(unavailable)?;
        let start = (index as u64)
            .checked_mul(SIZE as u64)
            .filter(|start| *start < self.offset)
            .ok_or_else(unavailable)?;
        let len = (self.offset - start).min(SIZE as u64) as usize;
        let mut bytes = vec![0; len];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut bytes)?;
        #[cfg(test)]
        READ_BYTES.with(|count| count.set(count.get() + len as u64));
        self.verify(index, &bytes)?;
        Ok(bytes)
    }

    /// Local files and request-owned archive readers verify the same indexed
    /// block proof. Provider generation names never replace this byte proof.
    pub(crate) fn verify(&self, index: usize, bytes: &[u8]) -> io::Result<()> {
        let expected = self.hashes.get(index).ok_or_else(unavailable)?;
        let start = (index as u64)
            .checked_mul(SIZE as u64)
            .filter(|start| *start < self.offset)
            .ok_or_else(unavailable)?;
        let len = (self.offset - start).min(SIZE as u64) as usize;
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if bytes.len() != len || actual != *expected {
            return Err(unavailable());
        }
        Ok(())
    }
}

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "native source block generation unavailable",
    )
}

pub(crate) struct Captured {
    pub(crate) prefix: [u8; 32],
    pub(crate) tail: [u8; 32],
    /// The old indexed prefix and the newly parsed complete lines are checked
    /// independently, so metadata never commits against different source bytes.
    pub(crate) before: [u8; 32],
    pub(crate) parsed: [u8; 32],
    pub(crate) chunks: Chunks,
}

/// Replaces the index's existing post-parse prefix hash pass. It does not add a
/// second full-file pass, and incomplete bytes beyond `offset` are excluded.
pub(crate) fn capture(
    file: &mut (impl Read + Seek),
    offset: u64,
    from: u64,
) -> io::Result<Captured> {
    if from > offset {
        return Err(unavailable());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut prefix = Sha256::new();
    let mut before = Sha256::new();
    let mut parsed = Sha256::new();
    let mut chunks = Chunks {
        offset,
        hashes: Vec::new(),
    };
    let mut tail = Vec::new();
    let mut position = 0;
    let mut buffer = vec![0; SIZE];
    while position < offset {
        let len = (offset - position).min(SIZE as u64) as usize;
        let bytes = &mut buffer[..len];
        file.read_exact(bytes)?;
        prefix.update(&*bytes);
        chunks.hashes.push(Sha256::digest(&*bytes).into());
        let split = from.saturating_sub(position).min(len as u64) as usize;
        before.update(&bytes[..split]);
        parsed.update(&bytes[split..]);
        if len >= 4096 {
            tail.clear();
            tail.extend_from_slice(&bytes[len - 4096..]);
        } else {
            tail.extend_from_slice(bytes);
            if tail.len() > 4096 {
                tail.drain(..tail.len() - 4096);
            }
        }
        position += len as u64;
    }
    Ok(Captured {
        prefix: prefix.finalize().into(),
        tail: Sha256::digest(tail).into(),
        before: before.finalize().into(),
        parsed: parsed.finalize().into(),
        chunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn source_proofs_match_the_complete_prefix_and_both_parser_partitions() {
        let bytes: Vec<u8> = (0..SIZE * 2 + 23).map(|i| (i % 251) as u8).collect();
        for offset in [0, 100, SIZE - 3, SIZE + 17, bytes.len()] {
            for from in [0, offset / 2, offset] {
                let captured =
                    capture(&mut Cursor::new(&bytes), offset as u64, from as u64).unwrap();
                assert_eq!(
                    captured.prefix,
                    <[u8; 32]>::from(Sha256::digest(&bytes[..offset]))
                );
                assert_eq!(
                    captured.tail,
                    <[u8; 32]>::from(Sha256::digest(&bytes[offset.saturating_sub(4096)..offset]))
                );
                assert_eq!(
                    captured.before,
                    <[u8; 32]>::from(Sha256::digest(&bytes[..from]))
                );
                assert_eq!(
                    captured.parsed,
                    <[u8; 32]>::from(Sha256::digest(&bytes[from..offset]))
                );
                assert!(captured.chunks.valid(offset as u64));
                for index in 0..captured.chunks.hashes.len() {
                    assert_eq!(
                        captured
                            .chunks
                            .read(&mut Cursor::new(&bytes), index)
                            .unwrap(),
                        bytes[index * SIZE..((index + 1) * SIZE).min(offset)]
                    );
                }
            }
        }
        assert!(capture(&mut Cursor::new(&bytes), 1, 2).is_err());
        assert!(capture(&mut Cursor::new(&bytes), bytes.len() as u64 + 1, 0).is_err());
    }

    #[test]
    fn old_generation_blocks_survive_append_and_refuse_changed_interiors() {
        let original = vec![b'x'; SIZE * 3 + 17];
        let captured = capture(&mut Cursor::new(&original), original.len() as u64, 0).unwrap();
        let mut appended = original.clone();
        appended.extend_from_slice(b"new incomplete bytes");
        assert_eq!(
            captured
                .chunks
                .read(&mut Cursor::new(&appended), 3)
                .unwrap(),
            vec![b'x'; 17]
        );
        appended[SIZE + 8] = b'y';
        assert!(
            captured
                .chunks
                .read(&mut Cursor::new(&appended), 1)
                .is_err()
        );
        assert_eq!(
            captured
                .chunks
                .read(&mut Cursor::new(&appended), 0)
                .unwrap(),
            vec![b'x'; SIZE]
        );
        assert!(
            captured
                .chunks
                .read(&mut Cursor::new(&appended), 4)
                .is_err()
        );
    }
}
