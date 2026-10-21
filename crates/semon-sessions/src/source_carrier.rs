//! Physical witnesses for request-owned, content-qualified local carriers.
use crate::{
    events::{Ledger, Stat},
    model::summary::CatalogSource,
    sealed::LogFile,
};
use std::{io, path::Path};

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RevisionReason {
    Identity,
    ChangeTime,
    SizeGrew,
    SizeShrank,
    Modified,
    MissingChangeToken,
}

impl RevisionReason {
    pub(crate) fn error(self) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, RevisionChanged(self))
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::ChangeTime => "change_time",
            Self::SizeGrew => "size_grew",
            Self::SizeShrank => "size_shrank",
            Self::Modified => "modified",
            Self::MissingChangeToken => "missing_change_token",
        }
    }
}

#[derive(Debug)]
pub(crate) struct RevisionChanged(pub(crate) RevisionReason);
impl std::fmt::Display for RevisionChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("selected local source changed during read")
    }
}
impl std::error::Error for RevisionChanged {}

pub(crate) fn ledger(source: &CatalogSource) -> Ledger {
    Ledger {
        stat: Stat {
            dev: source.dev,
            ino: source.ino,
            size: source.size,
            modified_ns: source.modified_ns,
            changed_ns: source.changed_ns,
        },
        offset: source.offset,
        prefix: source.prefix_sha256,
        tail: source.tail_sha256,
    }
}

pub(crate) fn mismatch(expected: &Stat, actual: &Stat) -> Option<RevisionReason> {
    if (expected.dev, expected.ino) != (actual.dev, actual.ino) {
        Some(RevisionReason::Identity)
    } else if actual.size < expected.size {
        Some(RevisionReason::SizeShrank)
    } else if actual.size > expected.size {
        Some(RevisionReason::SizeGrew)
    } else if expected.changed_ns.is_none() || actual.changed_ns.is_none() {
        Some(RevisionReason::MissingChangeToken)
    } else if expected.changed_ns != actual.changed_ns {
        Some(RevisionReason::ChangeTime)
    } else if expected.modified_ns != actual.modified_ns {
        Some(RevisionReason::Modified)
    } else {
        None
    }
}

/// Qualifying bytes never qualifies a concurrent revision or a different named
/// file. Call after block verification, before exposing any of those bytes.
pub(crate) fn recheck(path: &Path, file: &LogFile, before: &Stat) -> io::Result<()> {
    let named = std::fs::symlink_metadata(path)?;
    if !named.is_file()
        || Stat::from_metadata(&file.metadata()?)? != *before
        || Stat::from_metadata(&named)? != *before
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local source changed during read",
        ));
    }
    Ok(())
}
