use std::collections::BTreeMap;

use buzz_sim_protocol::{canonical_json_bytes, sha256_hex};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    validate_id, validate_nonempty, AgentError, KnowledgeEntry, MemoryLedger, MemoryRecord,
    NpcPersona, PersonaDirectory,
};

/// Conversation surface on which an NPC turn occurs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "surface", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationSurface {
    /// A private direct-message conversation with the player.
    DirectMessage,
    /// A message posted to one Buzz channel.
    Channel {
        /// Stable channel identifier.
        channel_id: String,
    },
}

/// Authoritative world facts visible to an NPC for one turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldSnapshot {
    /// Current season week, from one through twelve.
    pub week: u8,
    /// Current two-week sprint, from one through six.
    pub sprint: u8,
    /// Current work block in the 480-block probation season.
    pub work_block: u16,
    /// Stable active incident identifier, when one is visible to this NPC.
    pub active_incident: Option<String>,
    /// Scenario facts explicitly visible to this NPC, sorted by stable key.
    pub visible_facts: BTreeMap<String, String>,
}

/// Free-form player input plus immutable turn identity and visible world state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcTurnRequest {
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Idempotent turn identifier selected before invoking the model.
    pub turn_id: Uuid,
    /// NPC actor identifier.
    pub actor_id: String,
    /// Player text passed to the model without semantic rewriting.
    pub player_input: String,
    /// Conversation surface for disclosure validation.
    pub surface: ConversationSurface,
    /// Authoritative world state visible for this turn.
    pub world: WorldSnapshot,
}

/// Complete deterministic input supplied to an NPC model backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcModelInput {
    /// Full persona, including private motivations and permitted work authority.
    pub persona: NpcPersona,
    /// Facts and beliefs known to this NPC, including non-disclosable knowledge.
    pub knowledge: Vec<KnowledgeEntry>,
    /// Visible memories ordered by authoritative session sequence.
    pub memories: Vec<MemoryRecord>,
    /// Original turn request and free-form player input.
    pub request: NpcTurnRequest,
}

impl NpcModelInput {
    /// Computes a stable SHA-256 over canonical JSON model input.
    pub fn digest(&self) -> Result<String, AgentError> {
        let value = serde_json::to_value(self)
            .map_err(|error| AgentError::Digest(error.to_string()))?;
        let bytes = canonical_json_bytes(&value)
            .map_err(|error| AgentError::Digest(error.to_string()))?;
        Ok(sha256_hex(&bytes))
    }
}

/// Constructs least-knowledge model context from personas and authoritative memories.
#[derive(Debug, Clone, Copy)]
pub struct NpcContextBuilder<'a> {
    directory: &'a PersonaDirectory,
    memories: &'a MemoryLedger,
}

impl<'a> NpcContextBuilder<'a> {
    /// Creates a context builder over immutable persona and memory projections.
    #[must_use]
    pub const fn new(directory: &'a PersonaDirectory, memories: &'a MemoryLedger) -> Self {
        Self {
            directory,
            memories,
        }
    }

    /// Validates one request and constructs deterministic model input.
    pub fn build(
        &self,
        request: &NpcTurnRequest,
        memory_limit: usize,
    ) -> Result<NpcModelInput, AgentError> {
        validate_turn_request(request)?;
        let persona = self
            .directory
            .resolve(&request.actor_id)
            .ok_or_else(|| AgentError::UnknownNpc(request.actor_id.clone()))?;
        let memories = self
            .memories
            .visible_to(request.session_id, persona, memory_limit);
        Ok(NpcModelInput {
            persona: persona.clone(),
            knowledge: persona.knowledge.clone(),
            memories,
            request: request.clone(),
        })
    }
}

fn validate_turn_request(request: &NpcTurnRequest) -> Result<(), AgentError> {
    validate_id(&request.actor_id).map_err(|reason| AgentError::InvalidPersona {
        actor_id: request.actor_id.clone(),
        reason,
    })?;
    validate_nonempty("player_input", &request.player_input).map_err(|reason| {
        AgentError::InvalidPersona {
            actor_id: request.actor_id.clone(),
            reason,
        }
    })?;
    if !(1..=12).contains(&request.world.week) {
        return Err(AgentError::InvalidPersona {
            actor_id: request.actor_id.clone(),
            reason: format!("world week {} is outside 1..=12", request.world.week),
        });
    }
    if !(1..=6).contains(&request.world.sprint) {
        return Err(AgentError::InvalidPersona {
            actor_id: request.actor_id.clone(),
            reason: format!("world sprint {} is outside 1..=6", request.world.sprint),
        });
    }
    if request.world.work_block == 0 || request.world.work_block > 480 {
        return Err(AgentError::InvalidPersona {
            actor_id: request.actor_id.clone(),
            reason: format!(
                "world work block {} is outside 1..=480",
                request.world.work_block
            ),
        });
    }
    if let ConversationSurface::Channel { channel_id } = &request.surface {
        validate_id(channel_id).map_err(|reason| AgentError::InvalidPersona {
            actor_id: request.actor_id.clone(),
            reason,
        })?;
    }
    for (key, value) in &request.world.visible_facts {
        validate_id(key).map_err(|reason| AgentError::InvalidPersona {
            actor_id: request.actor_id.clone(),
            reason,
        })?;
        validate_nonempty("visible world fact", value).map_err(|reason| {
            AgentError::InvalidPersona {
                actor_id: request.actor_id.clone(),
                reason,
            }
        })?;
    }
    Ok(())
}
