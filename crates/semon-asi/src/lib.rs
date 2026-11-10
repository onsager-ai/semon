//! Storage contracts for Agent Session Intelligence analytics.
//!
//! The crate holds the analytics schema ([`schema`]), the single-writer
//! generation store with its manifest ([`store`]) and tiered compaction
//! ([`compact`]). It does not depend on `semon-sessions`.

pub mod compact;
pub mod error;
pub mod governance;
pub mod input;
pub mod projection;
pub mod publish;
pub mod schema;
pub mod store;

mod parquet_io;

#[cfg(all(test, feature = "duckdb-tests"))]
mod duckdb_smoke;
#[cfg(test)]
mod testutil;

pub use compact::CompactionPolicy;
pub use error::{Error, Result};
pub use governance::{
    ContributorWitness, CoverageWitness, Destination, ExportAuthority, ExportWitness,
    GovernanceError, GovernedRow, Policy, PolicyBundle, Principal, Scope, Source,
    body_read_allowed, coverage_allowed, coverage_witness, evaluate, local_witness,
    project_derived_row, provider_egress_allowed, sensitivity_floor, serialize_coverage_witness,
    serialize_governed_row,
};
pub use input::{
    ANALYTICS_TRANSFORM_VERSION, AdmittedBatch, AdmittedSession, AnalyticsInput, Reconciliation,
    SessionChange,
};
pub use projection::{Cell, ProjectedSession, Row, project};
pub use publish::{PublishError, publish_cycle};
pub use store::{
    Clock, FileEntry, Manifest, RETIREMENT_DELAY_MS, SourceMissing, Store, SystemClock, TableBatch,
    Update, Versions, read_live_map, read_manifest,
};
