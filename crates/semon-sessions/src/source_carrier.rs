//! Physical witnesses for request-owned, content-qualified local carriers.
use crate::{
    events::{Ledger, Stat},
    model::summary::CatalogSource,
    sealed::LogFile,
};
use std::{io, path::Path};

/// Closed diagnostics shared by Web and focused Query. Never serialize causes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureStage {
    StatGrew,
    StatShrank,
    Observed,
    Mode,
    ProofOpenDb,
    HeaderStore,
    HeaderMissing,
    HeaderVersion,
    HeaderParser,
    HeaderLedger,
    HeaderCount,
    PageMissing,
    PageHash,
    ProofChanged,
    Budget,
    Recheck,
}

impl FailureStage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::StatGrew => "stat_grew",
            Self::StatShrank => "stat_shrank",
            Self::Observed => "observed",
            Self::Mode => "mode",
            Self::ProofOpenDb => "proof_open_db",
            Self::HeaderStore => "header_store",
            Self::HeaderMissing => "header_missing",
            Self::HeaderVersion => "header_version",
            Self::HeaderParser => "header_parser",
            Self::HeaderLedger => "header_ledger",
            Self::HeaderCount => "header_count",
            Self::PageMissing => "page_missing",
            Self::PageHash => "page_hash",
            Self::ProofChanged => "proof_changed",
            Self::Budget => "budget",
            Self::Recheck => "recheck",
        }
    }

    pub(crate) fn wrap(self, cause: impl Into<io::Error>) -> io::Error {
        io::Error::new(
            io::ErrorKind::Other,
            SourceFailure {
                stage: self,
                cause: cause.into(),
            },
        )
    }

    pub(crate) fn of(error: &io::Error) -> Option<Self> {
        error
            .get_ref()?
            .downcast_ref::<SourceFailure>()
            .map(|failure| failure.stage)
    }
}

#[derive(Debug)]
struct SourceFailure {
    stage: FailureStage,
    cause: io::Error,
}
impl std::fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native source proof rejected at {:?}", self.stage)
    }
}
impl std::error::Error for SourceFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

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
