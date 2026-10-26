//! One current keyset-page reducer shared by the native engine adapters.
use crate::storage::StorageError;

pub(crate) const ROWS: usize = 64;
pub(crate) const BYTES: usize = 1024 * 1024;

pub(crate) struct Page {
    pub(crate) paths: Vec<String>,
    pub(crate) next: Option<String>,
    pub(crate) done: bool,
}

impl Page {
    pub(crate) fn plan(after: &str, paths: Vec<String>) -> Result<Self, StorageError> {
        if paths.len() > ROWS || paths.iter().map(String::len).sum::<usize>() > BYTES {
            return Err(StorageError::Busy);
        }
        let mut prior = after;
        for path in &paths {
            if path.is_empty() || path.as_bytes() <= prior.as_bytes() {
                return Err(StorageError::ScopeChanged);
            }
            prior = path;
        }
        Ok(Self {
            next: paths.last().cloned(),
            done: paths.is_empty(),
            paths,
        })
    }
}
