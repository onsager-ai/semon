//! Error type shared by the store.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parquet(parquet::errors::ParquetError),
    Arrow(arrow::error::ArrowError),
    Json(serde_json::Error),
    /// Another process holds `analytics.lock`.
    WriterBusy,
    /// The caller or the on-disk store broke a documented contract.
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// Whether retrying the failed store operation can be safe without changing
    /// its input or on-disk contract.
    pub fn is_operational(&self) -> bool {
        matches!(self, Self::Io(_))
            || matches!(
                self,
                Self::Parquet(parquet::errors::ParquetError::External(source))
                    if source.downcast_ref::<std::io::Error>().is_some()
            )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "analytics i/o: {error}"),
            Self::Parquet(error) => write!(f, "analytics parquet: {error}"),
            Self::Arrow(error) => write!(f, "analytics arrow: {error}"),
            Self::Json(error) => write!(f, "analytics manifest: {error}"),
            Self::WriterBusy => write!(f, "another process is the analytics writer"),
            Self::Invalid(message) => write!(f, "analytics store: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parquet(error) => Some(error),
            Self::Arrow(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::WriterBusy | Self::Invalid(_) => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<parquet::errors::ParquetError> for Error {
    fn from(error: parquet::errors::ParquetError) -> Self {
        Self::Parquet(error)
    }
}

impl From<arrow::error::ArrowError> for Error {
    fn from(error: arrow::error::ArrowError) -> Self {
        Self::Arrow(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_external_parquet_io_is_operational() {
        assert!(
            Error::Parquet(parquet::errors::ParquetError::External(Box::new(
                std::io::Error::other("temporary"),
            )))
            .is_operational()
        );
        assert!(
            !Error::Parquet(parquet::errors::ParquetError::General("bad footer".into()))
                .is_operational()
        );
    }
}
