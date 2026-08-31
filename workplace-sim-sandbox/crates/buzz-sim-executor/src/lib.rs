#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Idempotent execution bridge for policy-approved workplace NPC turns.
//!
//! The executor treats model output as untrusted input even after orchestration. It revalidates
//! current persona and repository authority, resolves immutable gateway commands, records only
//! successful receipts, and resumes retries without repeating completed side effects.

mod context;
mod error;
mod executor;
mod gateway;
mod identity;
mod ledger;

pub use context::{ExecutionContext, RepositoryExecutionTarget};
pub use error::ExecutionError;
pub use executor::{ExecutedNpcTurn, ExecutedOperation, NpcActionExecutor, TurnExecutionFailure};
pub use gateway::{
    BuzzDestination, BuzzGateway, BuzzMessageCommand, BuzzMessageReceipt, GatewayFailure,
    GatewayKind, GitHubCommand, GitHubGateway, GitHubReceipt, SimulationCommand, SimulationGateway,
    SimulationReceipt, VerificationCommand, VerificationGateway,
};
pub use ledger::{ExecutionLedger, ExecutionLedgerEntry, MemoryExecutionLedger, OperationReceipt};

pub(crate) use identity::{
    command_fingerprint, expected_action_id, reply_operation_id, verification_run_id,
};
