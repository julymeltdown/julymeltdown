use uuid::Uuid;

use crate::{NpcCapability, PolicyViolation};

/// Errors produced while loading personas, constructing NPC context, or validating model output.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Persona YAML could not be decoded.
    #[error("persona presentation/schema error: {0}")]
    PersonaYaml(String),
    /// Persona pack uses a version this build does not support.
    #[error("unsupported persona pack version {0}")]
    UnsupportedPersonaVersion(u16),
    /// Two persona definitions share one stable actor identifier.
    #[error("duplicate persona id {0:?}")]
    DuplicatePersonaId(String),
    /// Two knowledge entries in one persona share one fact identifier.
    #[error("persona {actor_id:?} contains duplicate knowledge id {fact_id:?}")]
    DuplicateKnowledgeId {
        /// Persona containing the duplicate fact.
        actor_id: String,
        /// Duplicate fact identifier.
        fact_id: String,
    },
    /// One persona field is outside the supported domain.
    #[error("invalid persona {actor_id:?}: {reason}")]
    InvalidPersona {
        /// Persona identifier.
        actor_id: String,
        /// Validation failure.
        reason: String,
    },
    /// A memory event ID was reused with different content.
    #[error("memory event {0} was reused with conflicting content")]
    MemoryConflict(Uuid),
    /// A memory record is malformed.
    #[error("invalid memory record: {0}")]
    InvalidMemory(String),
    /// A requested NPC is not defined by the active persona pack.
    #[error("unknown NPC {0:?}")]
    UnknownNpc(String),
    /// The NPC is currently unavailable and the model was not invoked.
    #[error("NPC {actor_id:?} is unavailable")]
    NpcUnavailable {
        /// Unavailable actor identifier.
        actor_id: String,
    },
    /// The model proposed more actions than one turn may execute.
    #[error("model proposed {count} actions; maximum is {maximum}")]
    TooManyActions {
        /// Proposed action count.
        count: usize,
        /// Hard turn limit.
        maximum: usize,
    },
    /// One proposed action failed deterministic policy validation.
    #[error("action {index} rejected: {violation}")]
    ActionRejected {
        /// Zero-based action index.
        index: usize,
        /// Policy violation.
        violation: PolicyViolation,
    },
    /// The proposed natural-language reply failed disclosure validation.
    #[error("reply rejected: {violation}")]
    ReplyRejected {
        /// Policy violation.
        violation: PolicyViolation,
    },
    /// A model backend failed before returning structured output.
    #[error("NPC model failed: {0}")]
    Model(String),
    /// Canonical serialization or hashing failed.
    #[error("NPC digest failed: {0}")]
    Digest(String),
    /// A capability name was required for an internal conversion but was unavailable.
    #[error("internal capability mapping failed for {0:?}")]
    CapabilityMapping(NpcCapability),
}
