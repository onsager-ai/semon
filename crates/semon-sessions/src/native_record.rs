//! Private exact-byte evidence locations, bound to the containing native ledger.
use crate::sealed::LogFile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Seek, SeekFrom};

#[cfg(test)]
thread_local! {
    pub(crate) static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) offset: u64,
    length: u64,
    sha256: [u8; 32],
}

impl Record {
    pub(crate) fn capture(bytes: &[u8], offset: u64) -> Self {
        Self {
            offset,
            length: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
        }
    }

    pub(crate) fn end(&self) -> Option<u64> {
        (self.length != 0)
            .then(|| self.offset.checked_add(self.length))
            .flatten()
    }

    pub(crate) fn verify(&self, file: &mut LogFile) -> io::Result<()> {
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = file.take(self.length);
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut length = 0;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            length += read as u64;
            #[cfg(test)]
            READ_BYTES.with(|count| count.set(count.get() + read as u64));
            hash.update(&buffer[..read]);
        }
        if length != self.length || <[u8; 32]>::from(hash.finalize()) != self.sha256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native evidence changed",
            ));
        }
        Ok(())
    }

    pub(crate) fn read(&self, file: &mut LogFile) -> io::Result<Vec<u8>> {
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(self.length).read_to_end(&mut bytes)?;
        #[cfg(test)]
        READ_BYTES.with(|count| count.set(count.get() + bytes.len() as u64));
        if bytes.len() as u64 != self.length
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != self.sha256
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native evidence changed",
            ));
        }
        Ok(bytes)
    }
}
