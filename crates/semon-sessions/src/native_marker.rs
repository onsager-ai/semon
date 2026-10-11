//! Content-free locations of the first parent marker or preceding agent output.
//! Prompt paths and marker text remain exclusively in the native source.
use crate::{Marker, events::Ledger, sealed::LogFile};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};

#[cfg(test)]
thread_local! {
    pub(crate) static READ_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Index {
    complete: Option<Record>,
    eof: Option<Record>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    offset: u64,
    length: u64,
    sha256: [u8; 32],
}

fn decision(value: &Value) -> Option<Option<Marker>> {
    if crate::is_agent_output(value) {
        Some(None)
    } else {
        crate::user_text(value)
            .and_then(crate::parse_marker)
            .map(Some)
    }
}

impl Index {
    pub(crate) fn begin_append(&mut self) {
        self.eof = None;
    }

    pub(crate) fn observe(&mut self, bytes: &[u8], offset: u64, complete: bool, value: &Value) {
        if self.complete.is_some() || !value.is_object() || decision(value).is_none() {
            return;
        }
        let record = Record {
            offset,
            length: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
        };
        if complete {
            self.complete = Some(record);
        } else {
            self.eof = Some(record);
        }
    }

    /// Repair only missing metadata in an existing consumed prefix; do not
    /// replay its event rows. New lines are observed by the event parser itself.
    pub(crate) fn repair(file: &mut LogFile, consumed: u64) -> io::Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(file.take(consumed));
        let mut index = Self::default();
        let mut offset = 0;
        let mut bytes = Vec::new();
        while reader.read_until(b'\n', &mut bytes)? != 0 {
            #[cfg(test)]
            READ_BYTES.with(|count| count.set(count.get() + bytes.len() as u64));
            if let Ok(value) = serde_json::from_slice(&bytes) {
                index.observe(&bytes, offset, true, &value);
            }
            offset += bytes.len() as u64;
            if index.complete.is_some() {
                break;
            }
            bytes.clear();
        }
        Ok(index)
    }

    pub(crate) fn record(&self) -> Option<&Record> {
        self.complete.as_ref().or(self.eof.as_ref())
    }

    pub(crate) fn valid(&self, ledger: &Ledger) -> bool {
        let complete = self
            .complete
            .as_ref()
            .is_none_or(|record| record.end().is_some_and(|end| end <= ledger.offset));
        let eof = self.eof.as_ref().is_none_or(|record| {
            self.complete.is_none()
                && record.offset == ledger.offset
                && record.end() == Some(ledger.stat.size)
        });
        complete && eof
    }

    /// Verify selected bytes independently before commit, including an EOF
    /// record outside the complete-prefix proof and a repaired prefix locator.
    pub(crate) fn verify(&self, file: &mut LogFile) -> io::Result<()> {
        if let Some(record) = self.record() {
            record.verify(file)?;
        }
        Ok(())
    }
}

impl Record {
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
                "native parent marker changed",
            ));
        }
        Ok(())
    }

    pub(crate) fn read(&self, file: &mut LogFile) -> io::Result<Option<Marker>> {
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
                "native parent marker changed",
            ));
        }
        let value = serde_json::from_slice(&bytes)?;
        decision(&value).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid native parent marker location",
            )
        })
    }
}
