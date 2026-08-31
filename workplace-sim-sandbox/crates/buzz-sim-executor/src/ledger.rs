use std::collections::BTreeMap;

use buzz_sim_protocol::VerificationAccepted;
use serde::{Deserialize, Serialize};

use crate::{BuzzMessageReceipt, ExecutionError, GitHubReceipt, SimulationReceipt};

/// Successful external receipt stored for idempotent replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationReceipt {
    /// Buzz accepted a message.
    Buzz(BuzzMessageReceipt),
    /// GitHub accepted or replayed a work mutation.
    GitHub(GitHubReceipt),
    /// The sandbox runner accepted objective verification.
    Verification(VerificationAccepted),
    /// The simulation kernel accepted a simulation-only command.
    Simulation(SimulationReceipt),
}

/// One immutable successful ledger entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionLedgerEntry {
    /// Digest of the fully resolved gateway command.
    pub fingerprint: String,
    /// Successful gateway receipt.
    pub receipt: OperationReceipt,
}

/// Receipt storage used to avoid repeating completed side effects.
pub trait ExecutionLedger: Send {
    /// Looks up one successful operation by deterministic identifier.
    fn lookup(&self, operation_id: &str) -> Option<ExecutionLedgerEntry>;

    /// Records one successful operation or verifies an identical replay.
    fn record_success(
        &mut self,
        operation_id: &str,
        fingerprint: &str,
        receipt: &OperationReceipt,
    ) -> Result<(), ExecutionError>;
}

/// In-memory execution ledger suitable for one process and deterministic tests.
#[derive(Debug, Clone, Default)]
pub struct MemoryExecutionLedger {
    entries: BTreeMap<String, ExecutionLedgerEntry>,
}

impl MemoryExecutionLedger {
    /// Returns the number of successful operations retained by the ledger.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no successful operations have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl ExecutionLedger for MemoryExecutionLedger {
    fn lookup(&self, operation_id: &str) -> Option<ExecutionLedgerEntry> {
        self.entries.get(operation_id).cloned()
    }

    fn record_success(
        &mut self,
        operation_id: &str,
        fingerprint: &str,
        receipt: &OperationReceipt,
    ) -> Result<(), ExecutionError> {
        if let Some(existing) = self.entries.get(operation_id) {
            if existing.fingerprint == fingerprint && existing.receipt == *receipt {
                return Ok(());
            }
            return Err(ExecutionError::LedgerConflict {
                operation_id: operation_id.to_string(),
            });
        }
        self.entries.insert(
            operation_id.to_string(),
            ExecutionLedgerEntry {
                fingerprint: fingerprint.to_string(),
                receipt: receipt.clone(),
            },
        );
        Ok(())
    }
}
