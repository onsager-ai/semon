//! Storage contracts for Agent Session Intelligence analytics.

pub mod error;
pub mod governance;
pub mod schema;

pub use error::{Error, Result};
pub use governance::{
    ContributorWitness, CoverageWitness, Destination, ExportAuthority, ExportWitness,
    GovernanceError, GovernedRow, Policy, PolicyBundle, Principal, Scope, Source,
    body_read_allowed, coverage_allowed, coverage_witness, evaluate, local_witness,
    project_derived_row, provider_egress_allowed, sensitivity_floor, serialize_coverage_witness,
    serialize_governed_row,
};
