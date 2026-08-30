#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Deterministic and isolated code verification for workplace simulations.

pub mod config;
pub mod error;
pub mod manifest;

pub use config::{ApiToken, RunnerConfig};
pub use error::RunnerError;
pub use manifest::*;
