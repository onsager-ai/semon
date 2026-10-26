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
        Self::keyed(after, paths.into_iter().map(|path| (path.clone(), Some(path))).collect())
    }

    pub(crate) fn keyed(after: &str, entries: Vec<(String, Option<String>)>) -> Result<Self, StorageError> {
        if entries.len() > ROWS || entries.iter().map(|(key, path)| key.len() + path.as_ref().map_or(0, String::len)).sum::<usize>() > BYTES {
            return Err(StorageError::Busy);
        }
        let mut prior = after;
        for (key, _) in &entries {
            if key.is_empty() || key.as_bytes() <= prior.as_bytes() {
                return Err(StorageError::ScopeChanged);
            }
            prior = key;
        }
        Ok(Self {
            next: entries.last().map(|(key, _)| key.clone()),
            done: entries.is_empty(),
            paths: entries.into_iter().filter_map(|(_, path)| path).collect(),
        })
    }
}
