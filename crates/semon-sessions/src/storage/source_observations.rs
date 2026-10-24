//! Durable source observation revisions, consumed as publication claims.
//! Recording observations grants no Raw access and does not publish a Catalog.
use super::StorageError;

pub type SourceClaims = Vec<(String, Option<i64>)>;

pub trait SourceObservationStore: Send {
    /// Record newly discovered paths atomically with their invalidations.
    fn discover(&mut self, paths: &[String], complete: bool) -> Result<(), StorageError>;
    /// Advance distinct paths only if every captured claim is still current.
    /// A conflict changes no row; this never acknowledges or publishes work.
    fn invalidate(&mut self, expected: &[(String, Option<i64>)]) -> Result<bool, StorageError>;
    /// Revisions are durable CAS witnesses, not worker leases.
    fn claims(&self, paths: &[String]) -> Result<SourceClaims, StorageError>;
    fn pending(&self, limit: usize) -> Result<Vec<String>, StorageError>;
    fn backfill_state(&self) -> Result<Option<String>, StorageError>;
}
