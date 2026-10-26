//! Content-free proofs for bounded reads of an indexed native generation.
//! These belong to the existing event ledger, never to the canonical trace store.
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom};

mod prefix_state;
pub(crate) use prefix_state::PrefixState;

pub(crate) const SIZE: usize = 64 * 1024;

#[cfg(test)]
thread_local! {
    pub(crate) static READ_BLOCKS: std::cell::RefCell<std::collections::BTreeSet<usize>> = const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
    pub(crate) static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chunks {
    pub(crate) offset: u64,
    pub(crate) hashes: Vec<[u8; 32]>,
    pub(crate) resume: Option<PrefixState>,
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
        #[cfg(test)]
        crate::events::SOURCE_HASHED_BYTES
            .with(|count| count.set(count.get() + bytes.len() as u64));
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
        resume: None,
    };
    let mut state = PrefixState::default();
    let mut tail = Vec::new();
    let mut position = 0;
    let mut buffer = vec![0; SIZE];
    while position < offset {
        let len = (offset - position).min(SIZE as u64) as usize;
        let bytes = &mut buffer[..len];
        file.read_exact(bytes)?;
        #[cfg(test)]
        {
            READ_BYTES.with(|count| count.set(count.get() + len as u64));
            crate::events::SOURCE_HASHED_BYTES.with(|count| {
                count.set(count.get() + len as u64 * if len == SIZE { 4 } else { 3 })
            });
        }
        if len == SIZE {
            state.advance(bytes);
        }
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
    chunks.resume = Some(state);
    #[cfg(test)]
    crate::events::SOURCE_HASHED_BYTES.with(|count| count.set(count.get() + tail.len() as u64));
    Ok(Captured {
        prefix: prefix.finalize().into(),
        tail: Sha256::digest(tail).into(),
        before: before.finalize().into(),
        parsed: parsed.finalize().into(),
        chunks,
    })
}

/// Resume only after receiver-owned append evidence has bound the old and new
/// physical revisions. Validate continuation against the old full-prefix SHA
/// and its boundary proof before deriving the appended generation.
pub(crate) fn capture_append(
    file: &mut (impl Read + Seek),
    offset: u64,
    from: u64,
    old: &Chunks,
    expected: [u8; 32],
) -> io::Result<Captured> {
    if from > offset || !old.valid(from) {
        return Err(unavailable());
    }
    let mut state = old.resume.clone().ok_or_else(unavailable)?;
    let boundary = from / SIZE as u64 * SIZE as u64;
    if state.at != boundary {
        return Err(unavailable());
    }
    let remainder = if boundary < from {
        old.read(file, (boundary / SIZE as u64) as usize)?
    } else {
        Vec::new()
    };
    if state.finish(&remainder) != expected {
        return Err(unavailable());
    }
    let mut chunks = Chunks {
        offset,
        hashes: old.hashes[..(boundary / SIZE as u64) as usize].to_vec(),
        resume: None,
    };
    file.seek(SeekFrom::Start(boundary))?;
    let mut position = boundary;
    let mut parsed = Sha256::new();
    let mut last = Vec::new();
    let mut buffer = vec![0; SIZE];
    while position < offset {
        let len = (offset - position).min(SIZE as u64) as usize;
        let bytes = &mut buffer[..len];
        file.read_exact(bytes)?;
        let split = from.saturating_sub(position).min(len as u64) as usize;
        // Re-read old boundary bytes, including after parser I/O.
        if position == boundary && bytes[..split] != remainder {
            return Err(unavailable());
        }
        parsed.update(&bytes[split..]);
        chunks.hashes.push(Sha256::digest(&*bytes).into());
        if len == SIZE {
            state.advance(bytes);
            last.clear();
        } else {
            last = bytes.to_vec();
        }
        #[cfg(test)]
        {
            READ_BYTES.with(|count| count.set(count.get() + len as u64));
            crate::events::SOURCE_HASHED_BYTES.with(|count| {
                count.set(
                    count.get()
                        + len as u64 * if len == SIZE { 2 } else { 1 }
                        + (len - split) as u64,
                )
            });
        }
        position += len as u64;
    }
    let prefix = state.finish(&last);
    chunks.resume = Some(state);
    let width = offset.min(4096) as usize;
    let mut tail = vec![0; width];
    file.seek(SeekFrom::Start(offset - width as u64))?;
    file.read_exact(&mut tail)?;
    #[cfg(test)]
    {
        READ_BYTES.with(|count| count.set(count.get() + width as u64));
        crate::events::SOURCE_HASHED_BYTES.with(|count| {
            count.set(count.get() + remainder.len() as u64 + last.len() as u64 + width as u64)
        });
    }
    Ok(Captured {
        prefix,
        tail: Sha256::digest(&tail).into(),
        before: expected,
        parsed: parsed.finalize().into(),
        chunks,
    })
}

/// Reuse a persisted native SHA continuation without rereading its old prefix.
/// The receiver fence and exact old ledger authorize unchanged earlier blocks;
/// the affected boundary is independently compared against the old full digest.
pub(crate) fn continue_prefix(file:&mut(impl Read+Seek),state:&PrefixState,from:u64,expected:[u8;32],to:u64,mut check:impl FnMut()->io::Result<()>)->io::Result<[u8;32]> {
    #[cfg(test)] crate::source_change::SOURCE_SQL_LOCK.with(|v|assert!(!v.get(),"history I/O under SourceState lock"));
    if from>to || state.at!=from/SIZE as u64*SIZE as u64 {return Err(unavailable());}
    let boundary=state.at;
    let mut old=vec![0;(from-boundary) as usize];
    check()?;file.seek(SeekFrom::Start(boundary))?;file.read_exact(&mut old)?;
    if state.finish(&old)!=expected {return Err(unavailable());}
    #[cfg(test)] {crate::events::SOURCE_READ_BYTES.with(|n|n.set(n.get()+old.len() as u64));crate::events::SOURCE_HASHED_BYTES.with(|n|n.set(n.get()+old.len() as u64));}
    let mut state=state.clone();let mut at=boundary;let mut buffer=vec![0;SIZE];
    file.seek(SeekFrom::Start(boundary))?;
    while at<to {
        check()?;crate::selected_read::check()?;
        let len=(to-at).min(SIZE as u64) as usize;file.read_exact(&mut buffer[..len])?;
        if at==boundary && buffer[..old.len()]!=old {return Err(unavailable());}
        #[cfg(test)] {crate::events::SOURCE_READ_BYTES.with(|n|n.set(n.get()+len as u64));crate::events::SOURCE_HASHED_BYTES.with(|n|n.set(n.get()+len as u64));}
        if len<SIZE {return Ok(state.finish(&buffer[..len]));}
        state.advance(&buffer);at+=len as u64;
    }
    Ok(state.finish(&[]))
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
    fn continuation_matches_full_sha_across_boundaries_and_refuses_corruption() {
        let bytes: Vec<u8> = (0..SIZE * 5 + 137).map(|n| (n % 251) as u8).collect();
        for from in [
            0,
            1,
            55,
            56,
            63,
            64,
            SIZE - 1,
            SIZE,
            SIZE + 19,
            SIZE * 4 + 3,
        ] {
            let old = capture(&mut Cursor::new(&bytes), from as u64, 0).unwrap();
            for to in [from, from + 1, from + 63, from + SIZE] {
                if to > bytes.len() {
                    continue;
                }
                let full = capture(&mut Cursor::new(&bytes), to as u64, from as u64).unwrap();
                let resumed = capture_append(
                    &mut Cursor::new(&bytes),
                    to as u64,
                    from as u64,
                    &old.chunks,
                    old.prefix,
                )
                .unwrap();
                assert_eq!(resumed.prefix, full.prefix);
                assert_eq!(resumed.tail, full.tail);
                assert_eq!(resumed.parsed, full.parsed);
                assert_eq!(resumed.chunks, full.chunks);
            }
            let mut corrupted = old.chunks.clone();
            corrupted.resume.as_mut().unwrap().at += 64;
            assert!(
                capture_append(
                    &mut Cursor::new(&bytes),
                    from as u64 + 1,
                    from as u64,
                    &corrupted,
                    old.prefix
                )
                .is_err()
            );
        }
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
