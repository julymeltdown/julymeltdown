use async_trait::async_trait;
use buzz_sim_protocol::{canonical_json_bytes, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    validate_nonempty, ActionPolicy, AgentError, MemoryLedger, NpcActionDraft, NpcAvailability,
    NpcContextBuilder, NpcModelInput, NpcReplyDraft, NpcTurnRequest, PersonaDirectory,
};

/// Hard limit on structured actions proposed by one NPC model turn.
pub const MAX_ACTIONS_PER_TURN: usize = 8;

/// Structured output returned by an NPC model backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcModelOutput {
    /// Optional natural-language reply.
    pub reply: Option<NpcReplyDraft>,
    /// Non-authoritative work action proposals.
    pub actions: Vec<NpcActionDraft>,
    /// Optional private summary for a later authoritative memory event.
    pub memory_note: Option<String>,
}

/// Model backend that turns deterministic NPC context into structured proposals.
#[async_trait]
pub trait NpcModel: Send + Sync {
    /// Generates one reply and zero or more non-authoritative work actions.
    async fn generate(&self, input: &NpcModelInput) -> Result<NpcModelOutput, AgentError>;
}

/// One policy-approved action bound to immutable session and turn identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedNpcAction {
    /// Stable SHA-256 action identifier derived from turn identity, index, and action payload.
    pub action_id: String,
    /// Policy-approved action proposal. A downstream executor still owns world mutation.
    pub action: NpcActionDraft,
}

/// Complete policy-approved result of one NPC model turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatedNpcTurn {
    /// Authoritative simulation session identifier copied from the request.
    pub session_id: Uuid,
    /// Idempotent turn identifier copied from the request.
    pub turn_id: Uuid,
    /// NPC actor identifier copied from the request.
    pub actor_id: String,
    /// Validated optional conversational reply.
    pub reply: Option<NpcReplyDraft>,
    /// Validated action proposals with deterministic identifiers.
    pub actions: Vec<ValidatedNpcAction>,
    /// Bounded private note that may later become an authoritative memory event.
    pub memory_note: Option<String>,
    /// Stable digest of the complete model input.
    pub input_digest: String,
    /// Stable digest of the raw structured model output.
    pub output_digest: String,
}

/// Coordinates context construction, model invocation, and fail-closed proposal validation.
#[derive(Debug, Clone)]
pub struct NpcOrchestrator<M> {
    directory: PersonaDirectory,
    memories: MemoryLedger,
    model: M,
}

impl<M> NpcOrchestrator<M> {
    /// Creates an orchestrator from immutable projections and a model backend.
    #[must_use]
    pub const fn new(directory: PersonaDirectory, memories: MemoryLedger, model: M) -> Self {
        Self {
            directory,
            memories,
            model,
        }
    }

    /// Returns the active persona directory.
    #[must_use]
    pub const fn personas(&self) -> &PersonaDirectory {
        &self.directory
    }

    /// Returns the current authoritative memory projection.
    #[must_use]
    pub const fn memories(&self) -> &MemoryLedger {
        &self.memories
    }

    /// Returns the mutable authoritative memory projection to the turn runtime.
    ///
    /// Model backends never receive this handle; only deterministic post-dispatch logic should
    /// record memories through it.
    #[must_use]
    pub fn memories_mut(&mut self) -> &mut MemoryLedger {
        &mut self.memories
    }
}

impl<M> NpcOrchestrator<M>
where
    M: NpcModel,
{
    /// Executes one model turn without mutating repositories, Buzz, or simulation state.
    ///
    /// The returned actions remain proposals. A downstream command adapter must convert them into
    /// authoritative simulation commands and external tool calls.
    pub async fn orchestrate(
        &self,
        request: &NpcTurnRequest,
        memory_limit: usize,
    ) -> Result<ValidatedNpcTurn, AgentError> {
        let persona = self
            .directory
            .resolve(&request.actor_id)
            .ok_or_else(|| AgentError::UnknownNpc(request.actor_id.clone()))?;
        if persona.availability == NpcAvailability::Offline {
            return Err(AgentError::NpcUnavailable {
                actor_id: request.actor_id.clone(),
            });
        }

        let input =
            NpcContextBuilder::new(&self.directory, &self.memories).build(request, memory_limit)?;
        let input_digest = input.digest()?;
        let output = self.model.generate(&input).await?;
        if output.actions.len() > MAX_ACTIONS_PER_TURN {
            return Err(AgentError::TooManyActions {
                count: output.actions.len(),
                maximum: MAX_ACTIONS_PER_TURN,
            });
        }
        if let Some(note) = &output.memory_note {
            validate_nonempty("memory_note", note).map_err(AgentError::Model)?;
        }

        let policy = ActionPolicy::new(&self.directory);
        if let Some(reply) = &output.reply {
            policy
                .validate_reply(persona, &request.surface, reply)
                .map_err(|violation| AgentError::ReplyRejected { violation })?;
        }

        let output_digest = digest_serializable(&output)?;
        let mut actions = Vec::with_capacity(output.actions.len());
        for (index, action) in output.actions.iter().enumerate() {
            policy
                .validate_action(persona, action)
                .map_err(|violation| AgentError::ActionRejected { index, violation })?;
            actions.push(ValidatedNpcAction {
                action_id: action_id(request, index, action)?,
                action: action.clone(),
            });
        }

        Ok(ValidatedNpcTurn {
            session_id: request.session_id,
            turn_id: request.turn_id,
            actor_id: request.actor_id.clone(),
            reply: output.reply,
            actions,
            memory_note: output.memory_note,
            input_digest,
            output_digest,
        })
    }
}

fn action_id(
    request: &NpcTurnRequest,
    index: usize,
    action: &NpcActionDraft,
) -> Result<String, AgentError> {
    digest_value(&json!({
        "session_id": request.session_id,
        "turn_id": request.turn_id,
        "actor_id": request.actor_id,
        "index": index,
        "action": action,
    }))
}

fn digest_serializable<T: Serialize>(value: &T) -> Result<String, AgentError> {
    let value =
        serde_json::to_value(value).map_err(|error| AgentError::Digest(error.to_string()))?;
    digest_value(&value)
}

fn digest_value(value: &serde_json::Value) -> Result<String, AgentError> {
    let bytes =
        canonical_json_bytes(value).map_err(|error| AgentError::Digest(error.to_string()))?;
    Ok(sha256_hex(&bytes))
}
