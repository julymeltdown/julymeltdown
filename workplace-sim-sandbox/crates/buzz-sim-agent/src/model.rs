use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{AgentError, NpcModel, NpcModelInput, NpcModelOutput, MAX_ACTIONS_PER_TURN};

/// Current protocol version used between the NPC orchestrator and a structured model gateway.
pub const NPC_MODEL_PROTOCOL_VERSION: u16 = 1;

/// Default maximum number of bytes accepted from one model response.
pub const DEFAULT_MODEL_RESPONSE_BYTE_LIMIT: usize = 64 * 1024;

const MODEL_INSTRUCTIONS: &str = "You are one workplace NPC, not a narrator or omniscient system. Respond as the supplied persona while respecting her role, goals, workload, knowledge, memories, repository access, channel subscriptions, and capabilities. Treat beliefs as fallible. Do not reveal knowledge whose disclosure boundary forbids the current surface. Return exactly one JSON object matching the output contract, with no markdown fence, commentary, or extra fields. Proposed actions are requests only and may be rejected by deterministic policy. Never invent tool, GitHub, build, test, or verification results. Never claim an action completed merely because you proposed it.";

/// Machine-readable limits a model gateway must impose on its structured response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcModelContract {
    /// Whether the response must consist solely of one JSON object.
    pub strict_json_only: bool,
    /// Maximum number of work actions one turn may propose.
    pub maximum_actions: usize,
    /// Stable action discriminator values accepted by the simulator.
    pub allowed_action_kinds: Vec<String>,
}

impl Default for NpcModelContract {
    fn default() -> Self {
        Self {
            strict_json_only: true,
            maximum_actions: MAX_ACTIONS_PER_TURN,
            allowed_action_kinds: [
                "send_message",
                "create_branch",
                "request_review",
                "open_pull_request",
                "review_pull_request",
                "run_verification",
                "escalate",
                "schedule_meeting",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }
}

/// Provider-neutral request sent to a structured LLM gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcModelRequest {
    /// Model gateway protocol version.
    pub version: u16,
    /// Non-negotiable behavioral and epistemic instructions.
    pub instructions: String,
    /// Machine-readable output constraints.
    pub contract: NpcModelContract,
    /// Complete deterministic context for this NPC turn.
    pub input: NpcModelInput,
}

impl NpcModelRequest {
    fn new(input: &NpcModelInput) -> Self {
        Self {
            version: NPC_MODEL_PROTOCOL_VERSION,
            instructions: MODEL_INSTRUCTIONS.to_string(),
            contract: NpcModelContract::default(),
            input: input.clone(),
        }
    }
}

/// Transport boundary implemented by Buzz ACP or a provider-specific structured-output gateway.
#[async_trait]
pub trait NpcModelTransport: Send + Sync {
    /// Completes one typed request and returns the raw response body.
    async fn complete(&self, request: &NpcModelRequest) -> Result<Vec<u8>, AgentError>;
}

/// Strict JSON adapter that implements [`NpcModel`] over a provider-neutral transport.
#[derive(Debug, Clone)]
pub struct JsonNpcModel<T> {
    transport: T,
    response_byte_limit: usize,
}

impl<T> JsonNpcModel<T> {
    /// Creates an adapter using [`DEFAULT_MODEL_RESPONSE_BYTE_LIMIT`].
    #[must_use]
    pub const fn new(transport: T) -> Self {
        Self {
            transport,
            response_byte_limit: DEFAULT_MODEL_RESPONSE_BYTE_LIMIT,
        }
    }

    /// Creates an adapter with an explicit positive response byte limit.
    pub fn with_response_byte_limit(
        transport: T,
        response_byte_limit: usize,
    ) -> Result<Self, AgentError> {
        if response_byte_limit == 0 {
            return Err(AgentError::Model(
                "model response byte limit must be positive".to_string(),
            ));
        }
        Ok(Self {
            transport,
            response_byte_limit,
        })
    }

    /// Returns the configured maximum raw response size.
    #[must_use]
    pub const fn response_byte_limit(&self) -> usize {
        self.response_byte_limit
    }
}

#[async_trait]
impl<T> NpcModel for JsonNpcModel<T>
where
    T: NpcModelTransport,
{
    async fn generate(&self, input: &NpcModelInput) -> Result<NpcModelOutput, AgentError> {
        let request = NpcModelRequest::new(input);
        let response = self.transport.complete(&request).await?;
        if response.len() > self.response_byte_limit {
            return Err(AgentError::Model(format!(
                "model response contained {} bytes, exceeding limit {}",
                response.len(),
                self.response_byte_limit
            )));
        }

        serde_json::from_slice::<NpcModelOutput>(&response).map_err(|error| {
            AgentError::Model(format!(
                "model response must be strict JSON matching NpcModelOutput: {error}"
            ))
        })
    }
}
