//! Physical witnesses for request-owned, content-qualified local carriers.
use crate::{
    events::{Ledger, Stat},
    model::summary::CatalogSource,
    sealed::LogFile,
};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

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
        io::Error::other(SourceFailure {
            stage: self,
            cause: cause.into(),
        })
    }

    // Byte reads already expose meaningful I/O kinds (for example EOF).
    // Add the closed stage without changing callers' kind-based decisions.
    pub(crate) fn wrap_preserving_kind(self, cause: io::Error) -> io::Error {
        io::Error::new(cause.kind(), SourceFailure { stage: self, cause })
    }

    pub(crate) fn of(error: &io::Error) -> Option<Self> {
        error
            .get_ref()?
            .downcast_ref::<SourceFailure>()
            .map(|failure| failure.stage)
    }
}

const LEDGER_DIGEST_DOMAIN: &[u8] = b"semon-ledger-v1\0";
const INTEREST_LIMIT: usize = 16;
const INTEREST_TTL: Duration = Duration::from_secs(30 * 60);

pub(crate) fn ledger_digest(ledger: &Ledger) -> String {
    ledger_digest_value(&crate::events::store::header_ledger(ledger))
}

pub(crate) fn ledger_digest_value(encoded_ledger: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(LEDGER_DIGEST_DOMAIN);
    digest.update(encoded_ledger.as_bytes());
    format!("{:x}", digest.finalize())
}

pub(crate) fn source_hash(path: &str) -> String {
    format!("{:x}", Sha256::digest(path.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogPublishInterest {
    pub(crate) source_hash: String,
    pub(crate) scope: String,
    pub(crate) path: String,
    pub(crate) catalog_ledger_digest: String,
    pub(crate) catalog_offset: u64,
}

#[derive(Default)]
struct DiagnosticInterestSet(VecDeque<(String, Option<CatalogPublishInterest>, Instant)>);

impl DiagnosticInterestSet {
    fn note_at(
        &mut self,
        source_hash: String,
        snapshot: Option<CatalogPublishInterest>,
        now: Instant,
    ) {
        self.expire_at(now);
        self.0.retain(|(known, _, _)| known != &source_hash);
        while self.0.len() >= INTEREST_LIMIT {
            self.0.pop_front();
        }
        self.0.push_back((source_hash, snapshot, now));
    }

    fn expire_at(&mut self, now: Instant) {
        while self.0.front().is_some_and(|(_, _, inserted)| {
            now.saturating_duration_since(*inserted) >= INTEREST_TTL
        }) {
            self.0.pop_front();
        }
    }

    #[cfg(test)]
    fn active_at(&mut self, now: Instant) -> Vec<String> {
        self.expire_at(now);
        self.0.iter().map(|(hash, _, _)| hash.clone()).collect()
    }

    fn publish_interests_at(&mut self, scope: &str, now: Instant) -> Vec<CatalogPublishInterest> {
        self.expire_at(now);
        self.0
            .iter()
            .filter_map(|(_, snapshot, _)| snapshot.as_ref())
            .filter(|interest| interest.scope == scope)
            .cloned()
            .collect()
    }
}

fn diagnostic_interest_set() -> &'static Mutex<DiagnosticInterestSet> {
    static INTEREST: OnceLock<Mutex<DiagnosticInterestSet>> = OnceLock::new();
    INTEREST.get_or_init(|| Mutex::new(DiagnosticInterestSet::default()))
}

pub(crate) fn note_diagnostic_interest(scope: &Path, path: &str, ledger: &Ledger) {
    let source_hash = source_hash(path);
    let snapshot = CatalogPublishInterest {
        source_hash: source_hash.clone(),
        scope: scope.to_string_lossy().into_owned(),
        path: path.to_owned(),
        catalog_ledger_digest: ledger_digest(ledger),
        catalog_offset: ledger.offset,
    };
    if let Ok(mut interests) = diagnostic_interest_set().lock() {
        interests.note_at(source_hash, Some(snapshot), Instant::now());
    }
}

pub(crate) fn diagnostic_publish_interests(scope: &Path) -> Vec<CatalogPublishInterest> {
    diagnostic_interest_set()
        .lock()
        .map(|mut interests| {
            interests.publish_interests_at(&scope.to_string_lossy(), Instant::now())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn ledger_digest_uses_the_header_encoding_and_a_domain_tag() {
        let ledger = Ledger {
            stat: Stat {
                dev: 1,
                ino: 2,
                size: 3,
                modified_ns: 4,
                changed_ns: Some(5),
            },
            offset: 3,
            prefix: [6; 32],
            tail: [7; 32],
        };
        let encoded = crate::events::store::header_ledger(&ledger);
        assert_eq!(ledger_digest(&ledger), ledger_digest_value(&encoded));
        assert_ne!(
            ledger_digest(&ledger),
            format!("{:x}", Sha256::digest(encoded))
        );
    }

    #[test]
    fn diagnostic_interest_is_bounded_ttl_limited_and_oldest_evicted() {
        let now = Instant::now();
        let mut interests = DiagnosticInterestSet::default();
        for index in 0..INTEREST_LIMIT {
            interests.note_at(
                format!("source-{index}"),
                None,
                now + Duration::from_secs(index as u64),
            );
        }
        interests.note_at("source-new".into(), None, now + Duration::from_secs(16));
        let active = interests.active_at(now + Duration::from_secs(16));
        assert_eq!(active.len(), INTEREST_LIMIT);
        assert!(!active.contains(&"source-0".to_owned()));
        assert!(active.contains(&"source-new".to_owned()));
        assert!(
            interests
                .active_at(now + INTEREST_TTL + Duration::from_secs(20))
                .is_empty()
        );
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
