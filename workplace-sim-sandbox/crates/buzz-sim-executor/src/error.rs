use buzz_sim_agent::PolicyViolation;
use buzz_sim_github::{ActorKind, RepositoryAccess};
use uuid::Uuid;

use crate::GatewayFailure;

/// Deterministic validation or external execution failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionError {
    /// Static execution context is invalid.
    #[error("invalid execution context: {reason}")]
    InvalidContext {
        /// Stable validation reason.
        reason: String,
    },
    /// Turn session does not match the active execution session.
    #[error("turn session {actual} does not match active session {expected}")]
    SessionMismatch {
        /// Active session identifier.
        expected: Uuid,
        /// Rejected turn session identifier.
        actual: Uuid,
    },
    /// The structured output digest no longer matches the turn payload.
    #[error("turn output digest mismatch: expected {expected}, got {actual}")]
    OutputDigestMismatch {
        /// Digest emitted by the NPC orchestrator.
        expected: String,
        /// Digest recomputed immediately before execution.
        actual: String,
    },
    /// NPC has no current persona.
    #[error("unknown NPC actor {actor_id:?}")]
    UnknownNpc {
        /// Missing NPC actor identifier.
        actor_id: String,
    },
    /// An actor referenced by an action is absent from the GitHub actor directory.
    #[error("unknown execution actor {actor_id:?}")]
    UnknownActor {
        /// Missing actor identifier.
        actor_id: String,
    },
    /// Current actor binding is not an NPC identity.
    #[error("actor {actor_id:?} has kind {actual:?}, expected npc")]
    ActorKindMismatch {
        /// Stable actor identifier.
        actor_id: String,
        /// Current actor kind.
        actual: ActorKind,
    },
    /// A logical repository is absent from the active session.
    #[error("unknown session repository {repository_id:?}")]
    MissingRepository {
        /// Missing logical repository identifier.
        repository_id: String,
    },
    /// Current GitHub actor binding does not authorize the requested repository access.
    #[error(
        "actor {actor_id:?} lacks {required:?} access to {repository_id:?}; current={actual:?}"
    )]
    RepositoryAccessDenied {
        /// Stable actor identifier.
        actor_id: String,
        /// Logical repository identifier.
        repository_id: String,
        /// Required access level.
        required: RepositoryAccess,
        /// Current actor access, when any.
        actual: Option<RepositoryAccess>,
    },
    /// The deterministic action identifier does not match the action payload.
    #[error("action {index} id mismatch: expected {expected}, got {actual}")]
    ActionIdMismatch {
        /// Zero-based action index.
        index: usize,
        /// Recomputed action identifier.
        expected: String,
        /// Supplied action identifier.
        actual: String,
    },
    /// A reply no longer satisfies the current persona policy.
    #[error("reply rejected at execution time: {violation}")]
    ReplyRejected {
        /// Current policy violation.
        violation: PolicyViolation,
    },
    /// An action no longer satisfies the current persona policy.
    #[error("action {index} rejected at execution time: {violation}")]
    ActionRejected {
        /// Zero-based action index.
        index: usize,
        /// Current policy violation.
        violation: PolicyViolation,
    },
    /// Verification requested a commit other than the active session head.
    #[error(
        "verification commit mismatch for {repository_id:?}: expected {expected}, got {actual}"
    )]
    HeadCommitMismatch {
        /// Logical repository identifier.
        repository_id: String,
        /// Current session head commit.
        expected: String,
        /// Requested commit.
        actual: String,
    },
    /// Verification requested an untrusted manifest digest.
    #[error(
        "verification manifest mismatch for {repository_id:?}: expected {expected}, got {actual}"
    )]
    ManifestDigestMismatch {
        /// Logical repository identifier.
        repository_id: String,
        /// Trusted manifest digest.
        expected: String,
        /// Requested manifest digest.
        actual: String,
    },
    /// No current reviewers are configured for a review request.
    #[error("no reviewers configured for repository {repository_id:?}")]
    NoReviewersConfigured {
        /// Logical repository identifier.
        repository_id: String,
    },
    /// One deterministic operation ID resolved to different command material.
    #[error("execution ledger conflict for operation {operation_id}")]
    LedgerConflict {
        /// Conflicting operation identifier.
        operation_id: String,
    },
    /// Canonical serialization or identifier derivation failed.
    #[error("execution digest failure: {0}")]
    Digest(String),
    /// An external gateway rejected or failed an operation.
    #[error(transparent)]
    Gateway(#[from] GatewayFailure),
}

impl ExecutionError {
    /// Returns whether retrying with the same operation identifier is permitted.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Gateway(failure) if failure.retryable)
    }
}
