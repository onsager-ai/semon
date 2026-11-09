//! Storage contracts for Agent Session Intelligence analytics.

pub mod error;
pub mod governance;
pub mod schema;

pub use error::{Error, Result};
pub use governance::{
    Destination, GovernanceError, Policy, PolicyBundle, Principal, Scope, Source, evaluate,
};
