//! Storage contracts for Agent Session Intelligence analytics.
//!
//! The crate holds the analytics schema ([`schema`]), the single-writer
//! generation store with its manifest ([`store`]) and tiered compaction
//! ([`compact`]). It does not depend on `semon-sessions`.

pub mod compact;
pub mod coverage;
pub mod durable;
pub mod egress;
pub mod error;
pub mod export;
pub mod governance;
pub mod input;
pub mod objects;
pub mod projection;
pub mod provenance;
pub mod publish;
#[cfg(feature = "query-engine")]
pub mod query;
pub mod reference;
pub mod schema;
pub mod store;

mod parquet_io;

#[cfg(all(test, feature = "duckdb-tests"))]
mod duckdb_smoke;
#[cfg(test)]
mod testutil;

pub use compact::CompactionPolicy;
pub use coverage::{CoverageGroup, CoverageKey, CoverageReport, CoverageTotals};
pub use error::{Error, Result};
pub use export::{
    ExportReceipt, ExportSummary, ResetSummary, export_snapshot, reset_snapshot, verify_export,
};
pub use governance::{
    ContributorWitness, CoverageWitness, Destination, ExportAuthority, ExportWitness,
    GovernanceError, GovernedRow, Policy, PolicyBundle, Principal, Scope, Source,
    body_read_allowed, coverage_allowed, coverage_witness, evaluate, local_witness,
    project_derived_row, provider_egress_allowed, sensitivity_floor, serialize_coverage_witness,
    serialize_governed_row,
};
pub use input::{
    ANALYTICS_TRANSFORM_VERSION, AdmittedBatch, AdmittedSession, AnalyticsInput, InputBudget,
    Reconciliation, SessionChange,
};
pub use projection::{Cell, ProjectedSession, Row, project};
pub use publish::{PublishError, PublishProgress, publish_cycle};
pub use store::{
    Clock, FileEntry, Manifest, RETIREMENT_DELAY_MS, SourceMissing, Store, SystemClock, TableBatch,
    Update, Versions, WriterLock, qualify_local_store, read_live_map, read_manifest,
};
