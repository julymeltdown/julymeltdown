#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Persona, knowledge, memory, and LLM orchestration contracts for workplace NPCs.
//!
//! Model output is deliberately non-authoritative. This crate constructs least-knowledge context,
//! validates disclosure and work authority, and emits deterministic proposals for downstream
//! simulation and GitHub command adapters.

mod action;
mod context;
mod error;
mod memory;
mod orchestrator;
mod persona;

pub use action::{ActionPolicy, NpcActionDraft, NpcReplyDraft, PolicyViolation};
pub use context::{
    ConversationSurface, NpcContextBuilder, NpcModelInput, NpcTurnRequest, WorldSnapshot,
};
pub use error::AgentError;
pub use memory::{MemoryAudience, MemoryLedger, MemoryRecord, MemoryRecordOutcome};
pub use orchestrator::{
    NpcModel, NpcModelOutput, NpcOrchestrator, ValidatedNpcAction, ValidatedNpcTurn,
    MAX_ACTIONS_PER_TURN,
};
pub use persona::{
    CharacterPresentation, KnowledgeDisclosure, KnowledgeEntry, KnowledgeStance, NpcAvailability,
    NpcCapability, NpcGoal, NpcPersona, PersonaDirectory, PersonaPack, PERSONA_PACK_VERSION,
};
