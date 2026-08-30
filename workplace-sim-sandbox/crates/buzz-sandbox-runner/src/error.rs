//! Error taxonomy for trusted runner infrastructure.

use std::io;

use buzz_sim_protocol::RunState;
use uuid::Uuid;

/// Errors raised by trusted sandbox infrastructure.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// Runner configuration is invalid.
    #[error("invalid runner configuration: {0}")]
    Config(String),
    /// A trusted scenario manifest is invalid.
    #[error("invalid scenario manifest: {0}")]
    Manifest(String),
    /// A requested manifest digest did not match trusted bytes.
    #[error("manifest digest mismatch: expected {expected}, got {actual}")]
    ManifestDigestMismatch {
        /// Digest supplied by the caller.
        expected: String,
        /// Digest computed from trusted bytes.
        actual: String,
    },
    /// A requested resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// An idempotency or immutable-state conflict occurred.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A lifecycle transition violated the state machine.
    #[error("invalid run transition for {run_id}: {current:?} -> {next:?}")]
    InvalidTransition {
        /// Run whose state was being changed.
        run_id: Uuid,
        /// Current persisted state.
        current: RunState,
        /// Requested next state.
        next: RunState,
    },
    /// JSON serialization or deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// YAML deserialization failed.
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    /// A filesystem operation failed.
    #[error("I/O error while {context}: {source}")]
    Io {
        /// Operation being attempted.
        context: String,
        /// Underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// A process could not be started or observed safely.
    #[error("process error: {0}")]
    Process(String),
    /// Workload source violated trusted policy.
    #[error("policy blocked: {0}")]
    Policy(String),
    /// Infrastructure failed independently of player code.
    #[error("infrastructure error: {0}")]
    Infrastructure(String),
}

impl RunnerError {
    /// Creates an I/O error with operation context.
    #[must_use]
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
