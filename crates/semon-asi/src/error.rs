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
