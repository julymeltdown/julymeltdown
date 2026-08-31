use std::collections::{BTreeMap, BTreeSet};

use buzz_sim_protocol::{canonical_json_bytes, sha256_hex};
use serde_json::json;
use uuid::Uuid;

use crate::{
    AgentError, MemoryAudience, MemoryRecord, MemoryRecordOutcome, NpcActionDispatchResult,
    NpcActionDispatcher, NpcActionDraft, NpcActionExecutor, NpcDispatchError, NpcModel,
    NpcOrchestrator, NpcTurnRequest, ValidatedNpcAction, ValidatedNpcTurn,
};

/// Maximum private memory-note bytes accepted from one model turn.
pub const MAX_MEMORY_NOTE_BYTES: usize = 2 * 1024;

#[derive(Debug, Clone)]
struct CachedTurn {
    request_digest: String,
    turn: ValidatedNpcTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkloadCharge {
    session_id: Uuid,
    actor_id: String,
    cost: u8,
}

#[derive(Debug, Clone, Default)]
struct NpcWorkloadLedger {
    charges: BTreeMap<String, WorkloadCharge>,
}

impl NpcWorkloadLedger {
    fn current(&self, session_id: Uuid, actor_id: &str, base: u8) -> u8 {
        let charged = self
            .charges
            .values()
            .filter(|charge| charge.session_id == session_id && charge.actor_id == actor_id)
            .map(|charge| u16::from(charge.cost))
            .sum::<u16>();
        u16::from(base)
            .saturating_add(charged)
            .min(100)
            .try_into()
            .unwrap_or(100)
    }

    fn pending_cost(&self, turn: &ValidatedNpcTurn) -> Result<u8, NpcRuntimeError> {
        let mut total = 0_u16;
        for action in &turn.actions {
            let cost = action_cost(&action.action);
            if let Some(existing) = self.charges.get(&action.action_id) {
                if existing.session_id != turn.session_id
                    || existing.actor_id != turn.actor_id
                    || existing.cost != cost
                {
                    return Err(NpcRuntimeError::WorkloadReplayConflict {
                        action_id: action.action_id.clone(),
                    });
                }
                continue;
            }
            total = total.saturating_add(u16::from(cost));
        }
        u8::try_from(total).map_err(|_| NpcRuntimeError::WorkloadCostOverflow)
    }

    fn record(
        &mut self,
        turn: &ValidatedNpcTurn,
        action: &ValidatedNpcAction,
    ) -> Result<bool, NpcRuntimeError> {
        let record = WorkloadCharge {
            session_id: turn.session_id,
            actor_id: turn.actor_id.clone(),
            cost: action_cost(&action.action),
        };
        match self.charges.get(&action.action_id) {
            Some(existing) if existing == &record => Ok(false),
            Some(_) => Err(NpcRuntimeError::WorkloadReplayConflict {
                action_id: action.action_id.clone(),
            }),
            None => {
                self.charges.insert(action.action_id.clone(), record);
                Ok(true)
            }
        }
    }
}

/// Authoritative result of orchestrating and dispatching one NPC turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcTurnRuntimeResult {
    /// Policy-approved model turn used for this execution or replay.
    pub turn: ValidatedNpcTurn,
    /// Ordered external side-effect receipts.
    pub dispatch: NpcActionDispatchResult,
    /// NPC workload after charging newly completed actions.
    pub current_workload: u8,
    /// Result of persisting the optional private memory note.
    pub memory_outcome: Option<MemoryRecordOutcome>,
}

/// Coordinates deterministic model replay, action dispatch, workload accounting, and memory.
#[derive(Debug, Clone)]
pub struct NpcTurnRuntime<M, E> {
    orchestrator: NpcOrchestrator<M>,
    dispatcher: NpcActionDispatcher<E>,
    turns: BTreeMap<(Uuid, Uuid), CachedTurn>,
    workloads: NpcWorkloadLedger,
}

impl<M, E> NpcTurnRuntime<M, E> {
    /// Creates a runtime with empty turn, dispatch, and workload projections.
    #[must_use]
    pub fn new(orchestrator: NpcOrchestrator<M>, executor: E) -> Self {
        Self {
            orchestrator,
            dispatcher: NpcActionDispatcher::new(executor),
            turns: BTreeMap::new(),
            workloads: NpcWorkloadLedger::default(),
        }
    }

    /// Returns the NPC orchestrator and its current authoritative memory projection.
    #[must_use]
    pub const fn orchestrator(&self) -> &NpcOrchestrator<M> {
        &self.orchestrator
    }

    /// Returns the action dispatcher and its receipt projection.
    #[must_use]
    pub const fn dispatcher(&self) -> &NpcActionDispatcher<E> {
        &self.dispatcher
    }

    /// Returns the current session-local workload for one known NPC.
    #[must_use]
    pub fn current_workload(&self, session_id: Uuid, actor_id: &str) -> u8 {
        let base = self
            .orchestrator
            .personas()
            .resolve(actor_id)
            .map_or(0, |persona| persona.workload);
        self.workloads.current(session_id, actor_id, base)
    }
}

impl<M, E> NpcTurnRuntime<M, E>
where
    M: NpcModel,
    E: NpcActionExecutor,
{
    /// Runs one free-text NPC turn through model, policy, external actions, workload, and memory.
    ///
    /// Replaying the same session and turn identity with identical input reuses the validated model
    /// output and dispatcher receipts. Reusing the identity with different input fails before model
    /// or executor invocation. A partial external failure charges only the completed prefix, keeps
    /// the validated turn cached, and defers memory persistence until a later retry fully succeeds.
    pub async fn run_turn(
        &mut self,
        request: &NpcTurnRequest,
        memory_limit: usize,
        memory_sequence: u64,
    ) -> Result<NpcTurnRuntimeResult, NpcRuntimeError> {
        let request_digest = runtime_request_digest(request, memory_limit, memory_sequence)?;
        let key = (request.session_id, request.turn_id);

        let turn = if let Some(cached) = self.turns.get(&key) {
            if cached.request_digest != request_digest {
                return Err(NpcRuntimeError::TurnReplayConflict {
                    session_id: request.session_id,
                    turn_id: request.turn_id,
                });
            }
            cached.turn.clone()
        } else {
            let current = self.current_workload(request.session_id, &request.actor_id);
            let persona = self
                .orchestrator
                .personas()
                .resolve(&request.actor_id)
                .ok_or_else(|| AgentError::UnknownNpc(request.actor_id.clone()))?;
            if current >= 100 {
                return Err(NpcRuntimeError::WorkloadExhausted {
                    actor_id: persona.id.clone(),
                    current,
                    required: 0,
                });
            }

            let turn = self.orchestrator.orchestrate(request, memory_limit).await?;
            if let Some(note) = &turn.memory_note {
                if note.len() > MAX_MEMORY_NOTE_BYTES {
                    return Err(NpcRuntimeError::MemoryNoteTooLong {
                        bytes: note.len(),
                        maximum: MAX_MEMORY_NOTE_BYTES,
                    });
                }
            }
            self.turns.insert(
                key,
                CachedTurn {
                    request_digest,
                    turn: turn.clone(),
                },
            );
            turn
        };

        let current = self.current_workload(turn.session_id, &turn.actor_id);
        let required = self.workloads.pending_cost(&turn)?;
        if u16::from(current) + u16::from(required) > 100 {
            return Err(NpcRuntimeError::WorkloadExhausted {
                actor_id: turn.actor_id.clone(),
                current,
                required,
            });
        }

        let dispatch = match self.dispatcher.dispatch(&turn).await {
            Ok(result) => {
                self.charge_completed_prefix(&turn, turn.actions.len())?;
                result
            }
            Err(error) => {
                let completed = completed_prefix_length(&turn, &error)?;
                self.charge_completed_prefix(&turn, completed)?;
                return Err(NpcRuntimeError::Dispatch(error));
            }
        };

        let memory_outcome = self.persist_memory(&turn, memory_sequence)?;
        Ok(NpcTurnRuntimeResult {
            current_workload: self.current_workload(turn.session_id, &turn.actor_id),
            turn,
            dispatch,
            memory_outcome,
        })
    }

    fn charge_completed_prefix(
        &mut self,
        turn: &ValidatedNpcTurn,
        completed: usize,
    ) -> Result<(), NpcRuntimeError> {
        for action in turn.actions.iter().take(completed) {
            self.workloads.record(turn, action)?;
        }
        Ok(())
    }

    fn persist_memory(
        &mut self,
        turn: &ValidatedNpcTurn,
        sequence: u64,
    ) -> Result<Option<MemoryRecordOutcome>, NpcRuntimeError> {
        let Some(note) = &turn.memory_note else {
            return Ok(None);
        };
        let record = MemoryRecord::new(
            turn.turn_id,
            turn.session_id,
            sequence,
            turn.actor_id.clone(),
            MemoryAudience::ActorOnly {
                actor_id: turn.actor_id.clone(),
            },
            note.clone(),
            related_fact_ids(turn),
        )?;
        self.orchestrator
            .memories_mut()
            .record(record)
            .map(Some)
            .map_err(NpcRuntimeError::from)
    }
}

/// Deterministic rejection or execution failure from [`NpcTurnRuntime`].
#[derive(Debug, thiserror::Error)]
pub enum NpcRuntimeError {
    /// Persona, model, policy, digest, or memory validation failed.
    #[error(transparent)]
    Agent(#[from] AgentError),
    /// External action dispatch failed after any completed prefix was accounted for.
    #[error(transparent)]
    Dispatch(#[from] NpcDispatchError),
    /// The same turn identity was reused with different input or runtime parameters.
    #[error("turn {turn_id} in session {session_id} was replayed with different input")]
    TurnReplayConflict {
        /// Simulation session identifier.
        session_id: Uuid,
        /// Conflicting turn identifier.
        turn_id: Uuid,
    },
    /// An NPC cannot accept the proposed action workload.
    #[error(
        "NPC {actor_id:?} workload {current} cannot accept {required} additional workload points"
    )]
    WorkloadExhausted {
        /// NPC actor identifier.
        actor_id: String,
        /// Current workload from zero through one hundred.
        current: u8,
        /// Additional workload required by actions not yet completed.
        required: u8,
    },
    /// One stable action identifier was associated with different workload ownership or cost.
    #[error("NPC action {action_id:?} has conflicting workload accounting")]
    WorkloadReplayConflict {
        /// Conflicting action identifier.
        action_id: String,
    },
    /// The bounded action set could not be represented as an eight-bit workload delta.
    #[error("NPC action workload cost overflow")]
    WorkloadCostOverflow,
    /// The optional private model memory note exceeded its runtime limit.
    #[error("NPC memory note contained {bytes} bytes; maximum is {maximum}")]
    MemoryNoteTooLong {
        /// Observed note byte length.
        bytes: usize,
        /// Configured maximum note byte length.
        maximum: usize,
    },
    /// A dispatcher failure referenced an action absent from the validated turn.
    #[error("dispatcher failure referenced unknown NPC action {action_id:?}")]
    UnknownDispatchAction {
        /// Unknown action identifier.
        action_id: String,
    },
}

fn runtime_request_digest(
    request: &NpcTurnRequest,
    memory_limit: usize,
    memory_sequence: u64,
) -> Result<String, NpcRuntimeError> {
    let value = json!({
        "request": request,
        "memory_limit": memory_limit,
        "memory_sequence": memory_sequence,
    });
    let bytes =
        canonical_json_bytes(&value).map_err(|error| AgentError::Digest(error.to_string()))?;
    Ok(sha256_hex(&bytes))
}

fn action_cost(action: &NpcActionDraft) -> u8 {
    match action {
        NpcActionDraft::SendMessage { .. } => 1,
        NpcActionDraft::CreateBranch { .. } => 4,
        NpcActionDraft::RequestReview { .. } => 2,
        NpcActionDraft::OpenPullRequest { .. } => 5,
        NpcActionDraft::ReviewPullRequest { .. } => 3,
        NpcActionDraft::RunVerification { .. } => 2,
        NpcActionDraft::Escalate { .. } => 2,
        NpcActionDraft::ScheduleMeeting {
            duration_blocks, ..
        } => *duration_blocks,
    }
}

fn completed_prefix_length(
    turn: &ValidatedNpcTurn,
    error: &NpcDispatchError,
) -> Result<usize, NpcRuntimeError> {
    let failed_action_id = match error {
        NpcDispatchError::ExecutorFailed { action_id, .. }
        | NpcDispatchError::InvalidReceipt { action_id, .. } => Some(action_id.as_str()),
        NpcDispatchError::ReceiptActionMismatch { expected, .. } => Some(expected.as_str()),
        NpcDispatchError::Serialization(_)
        | NpcDispatchError::SequenceOverflow
        | NpcDispatchError::InvalidActionId { .. }
        | NpcDispatchError::InvalidTurnDigest { .. }
        | NpcDispatchError::DuplicateActionId { .. }
        | NpcDispatchError::ActionReplayConflict { .. }
        | NpcDispatchError::ReceiptReplayConflict { .. }
        | NpcDispatchError::InvalidExecutorError { .. } => None,
    };

    let Some(failed_action_id) = failed_action_id else {
        return Ok(0);
    };
    turn.actions
        .iter()
        .position(|action| action.action_id == failed_action_id)
        .ok_or_else(|| NpcRuntimeError::UnknownDispatchAction {
            action_id: failed_action_id.to_string(),
        })
}

fn related_fact_ids(turn: &ValidatedNpcTurn) -> BTreeSet<String> {
    let mut facts = turn
        .reply
        .as_ref()
        .map_or_else(BTreeSet::new, |reply| reply.fact_ids.clone());
    for action in &turn.actions {
        if let NpcActionDraft::SendMessage { fact_ids, .. } = &action.action {
            facts.extend(fact_ids.iter().cloned());
        }
    }
    facts
}
