use buzz_sim_agent::{
    ActionPolicy, ConversationSurface, NpcActionDraft, NpcModelOutput, ValidatedNpcTurn,
};
use buzz_sim_github::{ActorKind, RepositoryAccess, ResolvedActor};
use buzz_sim_protocol::{RepositoryRevision, VerificationRequest, VERIFICATION_PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    command_fingerprint, expected_action_id, reply_operation_id, verification_run_id,
    BuzzDestination, BuzzGateway, BuzzMessageCommand, ExecutionContext, ExecutionError,
    ExecutionLedger, GatewayFailure, GitHubCommand, GitHubGateway, OperationReceipt,
    RepositoryExecutionTarget, SimulationCommand, SimulationGateway, VerificationCommand,
    VerificationGateway,
};

/// One successfully executed or replayed NPC operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutedOperation {
    /// Deterministic idempotency key.
    pub operation_id: String,
    /// Whether the receipt was read from the execution ledger without repeating the side effect.
    pub replayed: bool,
    /// Successful gateway receipt.
    pub receipt: OperationReceipt,
}

/// Complete successful execution of one validated NPC turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutedNpcTurn {
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// NPC turn identifier.
    pub turn_id: Uuid,
    /// NPC actor identifier.
    pub actor_id: String,
    /// Reply and action receipts in deterministic execution order.
    pub operations: Vec<ExecutedOperation>,
}

/// Failure returned after deterministic preflight or a partially completed external dispatch.
#[derive(Debug, thiserror::Error)]
#[error("NPC turn execution failed at action {failed_action_index:?}: {source}")]
pub struct TurnExecutionFailure {
    /// Operation that failed, when an operation ID was available.
    pub failed_operation_id: Option<Box<str>>,
    /// Zero-based NPC action index; `None` identifies reply or turn-level validation.
    pub failed_action_index: Option<usize>,
    /// Successful operations completed before the failure.
    pub completed: Box<Vec<ExecutedOperation>>,
    /// Deterministic or gateway failure.
    #[source]
    pub source: ExecutionError,
}

impl TurnExecutionFailure {
    fn preflight(
        operation_id: Option<String>,
        action_index: Option<usize>,
        source: ExecutionError,
    ) -> Self {
        Self {
            failed_operation_id: operation_id.map(String::into_boxed_str),
            failed_action_index: action_index,
            completed: Box::new(Vec::new()),
            source,
        }
    }

    fn after_dispatch(
        operation: &PreparedOperation,
        completed: Vec<ExecutedOperation>,
        source: ExecutionError,
    ) -> Self {
        Self {
            failed_operation_id: Some(operation.operation_id.clone().into_boxed_str()),
            failed_action_index: operation.action_index,
            completed: Box::new(completed),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "gateway", content = "command", rename_all = "snake_case")]
enum ResolvedCommand {
    Buzz(BuzzMessageCommand),
    GitHub(GitHubCommand),
    Verification(VerificationCommand),
    Simulation(SimulationCommand),
}

#[derive(Debug, Clone)]
struct PreparedOperation {
    operation_id: String,
    action_index: Option<usize>,
    command: ResolvedCommand,
    fingerprint: String,
}

/// Executes validated NPC turns through independently injected external gateways.
#[derive(Debug)]
pub struct NpcActionExecutor<B, G, V, S, L> {
    buzz: B,
    github: G,
    verification: V,
    simulation: S,
    ledger: L,
}

impl<B, G, V, S, L> NpcActionExecutor<B, G, V, S, L> {
    /// Creates an executor from external gateways and a successful-receipt ledger.
    #[must_use]
    pub const fn new(buzz: B, github: G, verification: V, simulation: S, ledger: L) -> Self {
        Self {
            buzz,
            github,
            verification,
            simulation,
            ledger,
        }
    }

    /// Returns the immutable successful-receipt ledger.
    #[must_use]
    pub const fn ledger(&self) -> &L {
        &self.ledger
    }

    /// Returns the mutable successful-receipt ledger for durable checkpoint integration.
    #[must_use]
    pub fn ledger_mut(&mut self) -> &mut L {
        &mut self.ledger
    }
}

impl<B, G, V, S, L> NpcActionExecutor<B, G, V, S, L>
where
    B: BuzzGateway,
    G: GitHubGateway,
    V: VerificationGateway,
    S: SimulationGateway,
    L: ExecutionLedger,
{
    /// Revalidates and executes one NPC turn in deterministic reply-then-action order.
    ///
    /// All deterministic validation finishes before the first side effect. External failures stop
    /// execution immediately. Retrying the same turn replays stored successful receipts and
    /// resumes from the first incomplete operation.
    pub async fn execute_turn(
        &mut self,
        context: &ExecutionContext,
        turn: &ValidatedNpcTurn,
        surface: &ConversationSurface,
    ) -> Result<ExecutedNpcTurn, TurnExecutionFailure> {
        let prepared = prepare_turn(context, turn, surface)?;
        let mut completed = Vec::with_capacity(prepared.len());

        for operation in prepared {
            if let Some(existing) = self.ledger.lookup(&operation.operation_id) {
                if existing.fingerprint != operation.fingerprint
                    || !receipt_matches(&operation.command, &existing.receipt)
                {
                    return Err(TurnExecutionFailure::after_dispatch(
                        &operation,
                        completed,
                        ExecutionError::LedgerConflict {
                            operation_id: operation.operation_id.clone(),
                        },
                    ));
                }
                completed.push(ExecutedOperation {
                    operation_id: operation.operation_id,
                    replayed: true,
                    receipt: existing.receipt,
                });
                continue;
            }

            let receipt = match self.dispatch(&operation.command).await {
                Ok(receipt) => receipt,
                Err(source) => {
                    return Err(TurnExecutionFailure::after_dispatch(
                        &operation,
                        completed,
                        ExecutionError::Gateway(source),
                    ));
                }
            };
            if let Err(source) = self.ledger.record_success(
                &operation.operation_id,
                &operation.fingerprint,
                &receipt,
            ) {
                return Err(TurnExecutionFailure::after_dispatch(
                    &operation, completed, source,
                ));
            }
            completed.push(ExecutedOperation {
                operation_id: operation.operation_id,
                replayed: false,
                receipt,
            });
        }

        Ok(ExecutedNpcTurn {
            session_id: turn.session_id,
            turn_id: turn.turn_id,
            actor_id: turn.actor_id.clone(),
            operations: completed,
        })
    }

    async fn dispatch(
        &mut self,
        command: &ResolvedCommand,
    ) -> Result<OperationReceipt, GatewayFailure> {
        match command {
            ResolvedCommand::Buzz(command) => self
                .buzz
                .send_message(command)
                .await
                .map(OperationReceipt::Buzz),
            ResolvedCommand::GitHub(command) => self
                .github
                .execute(command)
                .await
                .map(OperationReceipt::GitHub),
            ResolvedCommand::Verification(command) => self
                .verification
                .submit(command)
                .await
                .map(OperationReceipt::Verification),
            ResolvedCommand::Simulation(command) => self
                .simulation
                .apply(command)
                .await
                .map(OperationReceipt::Simulation),
        }
    }
}

fn prepare_turn(
    context: &ExecutionContext,
    turn: &ValidatedNpcTurn,
    surface: &ConversationSurface,
) -> Result<Vec<PreparedOperation>, TurnExecutionFailure> {
    if context.session_id() != turn.session_id {
        return Err(TurnExecutionFailure::preflight(
            None,
            None,
            ExecutionError::SessionMismatch {
                expected: context.session_id(),
                actual: turn.session_id,
            },
        ));
    }

    let persona = context.persona(&turn.actor_id).ok_or_else(|| {
        TurnExecutionFailure::preflight(
            None,
            None,
            ExecutionError::UnknownNpc {
                actor_id: turn.actor_id.clone(),
            },
        )
    })?;
    let actor = context.actor(&turn.actor_id).ok_or_else(|| {
        TurnExecutionFailure::preflight(
            None,
            None,
            ExecutionError::UnknownActor {
                actor_id: turn.actor_id.clone(),
            },
        )
    })?;
    if actor.kind != ActorKind::Npc {
        return Err(TurnExecutionFailure::preflight(
            None,
            None,
            ExecutionError::ActorKindMismatch {
                actor_id: turn.actor_id.clone(),
                actual: actor.kind,
            },
        ));
    }

    verify_output_digest(turn)?;

    let policy = ActionPolicy::new(context.personas());
    let mut operations = Vec::with_capacity(turn.actions.len() + usize::from(turn.reply.is_some()));

    if let Some(reply) = &turn.reply {
        policy
            .validate_reply(persona, surface, reply)
            .map_err(|violation| {
                TurnExecutionFailure::preflight(
                    None,
                    None,
                    ExecutionError::ReplyRejected { violation },
                )
            })?;
        let operation_id = reply_operation_id(turn, surface, reply)
            .map_err(|source| TurnExecutionFailure::preflight(None, None, source))?;
        let destination = match surface {
            ConversationSurface::DirectMessage => BuzzDestination::DirectMessage {
                recipient_actor_id: context.player_actor_id().to_string(),
            },
            ConversationSurface::Channel { channel_id } => BuzzDestination::Channel {
                channel_id: channel_id.clone(),
            },
        };
        push_prepared(
            &mut operations,
            operation_id.clone(),
            None,
            ResolvedCommand::Buzz(BuzzMessageCommand {
                operation_id,
                session_id: turn.session_id,
                actor_id: turn.actor_id.clone(),
                destination,
                body: reply.body.clone(),
            }),
        )?;
    }

    for (index, validated) in turn.actions.iter().enumerate() {
        let expected = expected_action_id(turn, index, &validated.action).map_err(|source| {
            TurnExecutionFailure::preflight(Some(validated.action_id.clone()), Some(index), source)
        })?;
        if expected != validated.action_id {
            return Err(TurnExecutionFailure::preflight(
                Some(validated.action_id.clone()),
                Some(index),
                ExecutionError::ActionIdMismatch {
                    index,
                    expected,
                    actual: validated.action_id.clone(),
                },
            ));
        }
        policy
            .validate_action(persona, &validated.action)
            .map_err(|violation| {
                TurnExecutionFailure::preflight(
                    Some(validated.action_id.clone()),
                    Some(index),
                    ExecutionError::ActionRejected { index, violation },
                )
            })?;
        let command = resolve_action(
            context,
            turn,
            actor,
            &validated.action_id,
            &validated.action,
        )
        .map_err(|source| {
            TurnExecutionFailure::preflight(Some(validated.action_id.clone()), Some(index), source)
        })?;
        push_prepared(
            &mut operations,
            validated.action_id.clone(),
            Some(index),
            command,
        )?;
    }

    Ok(operations)
}

fn verify_output_digest(turn: &ValidatedNpcTurn) -> Result<(), TurnExecutionFailure> {
    let output = NpcModelOutput {
        reply: turn.reply.clone(),
        actions: turn
            .actions
            .iter()
            .map(|validated| validated.action.clone())
            .collect(),
        memory_note: turn.memory_note.clone(),
    };
    let actual = command_fingerprint(&output)
        .map_err(|source| TurnExecutionFailure::preflight(None, None, source))?;
    if actual == turn.output_digest {
        Ok(())
    } else {
        Err(TurnExecutionFailure::preflight(
            None,
            None,
            ExecutionError::OutputDigestMismatch {
                expected: turn.output_digest.clone(),
                actual,
            },
        ))
    }
}

fn push_prepared(
    operations: &mut Vec<PreparedOperation>,
    operation_id: String,
    action_index: Option<usize>,
    command: ResolvedCommand,
) -> Result<(), TurnExecutionFailure> {
    let fingerprint = command_fingerprint(&command).map_err(|source| {
        TurnExecutionFailure::preflight(Some(operation_id.clone()), action_index, source)
    })?;
    operations.push(PreparedOperation {
        operation_id,
        action_index,
        command,
        fingerprint,
    });
    Ok(())
}

fn resolve_action(
    context: &ExecutionContext,
    turn: &ValidatedNpcTurn,
    actor: &ResolvedActor,
    operation_id: &str,
    action: &NpcActionDraft,
) -> Result<ResolvedCommand, ExecutionError> {
    match action {
        NpcActionDraft::SendMessage {
            channel_id, body, ..
        } => Ok(ResolvedCommand::Buzz(BuzzMessageCommand {
            operation_id: operation_id.to_string(),
            session_id: turn.session_id,
            actor_id: turn.actor_id.clone(),
            destination: BuzzDestination::Channel {
                channel_id: channel_id.clone(),
            },
            body: body.clone(),
        })),
        NpcActionDraft::CreateBranch {
            repository_id,
            branch_name,
            purpose,
        } => {
            let repository = require_repository(
                context,
                actor,
                &turn.actor_id,
                repository_id,
                RepositoryAccess::Write,
            )?;
            Ok(ResolvedCommand::GitHub(GitHubCommand::CreateBranch {
                operation_id: operation_id.to_string(),
                session_id: turn.session_id,
                actor_id: turn.actor_id.clone(),
                actor_login: actor.github_login.clone(),
                repository_id: repository_id.clone(),
                host: repository.destination.host.clone(),
                owner: repository.destination.owner.clone(),
                name: repository.destination.name.clone(),
                branch_name: branch_name.clone(),
                from_sha: repository.head_commit_sha.clone(),
                purpose: purpose.clone(),
            }))
        }
        NpcActionDraft::RequestReview {
            repository_id,
            pull_request,
        } => {
            let repository = require_repository(
                context,
                actor,
                &turn.actor_id,
                repository_id,
                RepositoryAccess::Read,
            )?;
            let reviewer_logins = context.reviewer_logins(repository_id)?;
            Ok(ResolvedCommand::GitHub(GitHubCommand::RequestReview {
                operation_id: operation_id.to_string(),
                session_id: turn.session_id,
                actor_id: turn.actor_id.clone(),
                actor_login: actor.github_login.clone(),
                repository_id: repository_id.clone(),
                host: repository.destination.host.clone(),
                owner: repository.destination.owner.clone(),
                name: repository.destination.name.clone(),
                pull_request: *pull_request,
                reviewer_logins,
            }))
        }
        NpcActionDraft::OpenPullRequest {
            repository_id,
            branch_name,
            title,
            body,
        } => {
            let repository = require_repository(
                context,
                actor,
                &turn.actor_id,
                repository_id,
                RepositoryAccess::Write,
            )?;
            Ok(ResolvedCommand::GitHub(GitHubCommand::OpenPullRequest {
                operation_id: operation_id.to_string(),
                session_id: turn.session_id,
                actor_id: turn.actor_id.clone(),
                actor_login: actor.github_login.clone(),
                repository_id: repository_id.clone(),
                host: repository.destination.host.clone(),
                owner: repository.destination.owner.clone(),
                name: repository.destination.name.clone(),
                branch_name: branch_name.clone(),
                base_branch: repository.destination.default_branch.clone(),
                title: title.clone(),
                body: body.clone(),
            }))
        }
        NpcActionDraft::ReviewPullRequest {
            repository_id,
            pull_request,
            body,
        } => {
            let repository = require_repository(
                context,
                actor,
                &turn.actor_id,
                repository_id,
                RepositoryAccess::Read,
            )?;
            Ok(ResolvedCommand::GitHub(GitHubCommand::ReviewPullRequest {
                operation_id: operation_id.to_string(),
                session_id: turn.session_id,
                actor_id: turn.actor_id.clone(),
                actor_login: actor.github_login.clone(),
                repository_id: repository_id.clone(),
                host: repository.destination.host.clone(),
                owner: repository.destination.owner.clone(),
                name: repository.destination.name.clone(),
                pull_request: *pull_request,
                body: body.clone(),
            }))
        }
        NpcActionDraft::RunVerification {
            repository_id,
            commit_sha,
            manifest_digest,
        } => {
            let repository = require_repository(
                context,
                actor,
                &turn.actor_id,
                repository_id,
                RepositoryAccess::Read,
            )?;
            if !repository.head_commit_sha.eq_ignore_ascii_case(commit_sha) {
                return Err(ExecutionError::HeadCommitMismatch {
                    repository_id: repository_id.clone(),
                    expected: repository.head_commit_sha.clone(),
                    actual: commit_sha.clone(),
                });
            }
            if !repository
                .manifest_digest
                .eq_ignore_ascii_case(manifest_digest)
            {
                return Err(ExecutionError::ManifestDigestMismatch {
                    repository_id: repository_id.clone(),
                    expected: repository.manifest_digest.clone(),
                    actual: manifest_digest.clone(),
                });
            }
            let request = VerificationRequest {
                version: VERIFICATION_PROTOCOL_VERSION,
                run_id: verification_run_id(operation_id)?,
                session_id: turn.session_id,
                scenario_id: context.scenario_id().to_string(),
                scenario_version: context.scenario_version().to_string(),
                expected_manifest_digest: repository.manifest_digest.clone(),
                repositories: vec![RepositoryRevision {
                    repository_id: repository_id.clone(),
                    clone_url: repository.destination.clone_url(),
                    base_commit_sha: repository.base_commit_sha.clone(),
                    head_commit_sha: repository.head_commit_sha.clone(),
                }],
            };
            Ok(ResolvedCommand::Verification(VerificationCommand {
                operation_id: operation_id.to_string(),
                request,
            }))
        }
        NpcActionDraft::Escalate {
            target_actor_id,
            summary,
        } => Ok(ResolvedCommand::Simulation(SimulationCommand::Escalate {
            operation_id: operation_id.to_string(),
            session_id: turn.session_id,
            actor_id: turn.actor_id.clone(),
            target_actor_id: target_actor_id.clone(),
            summary: summary.clone(),
        })),
        NpcActionDraft::ScheduleMeeting {
            participant_actor_ids,
            agenda,
            duration_blocks,
        } => Ok(ResolvedCommand::Simulation(
            SimulationCommand::ScheduleMeeting {
                operation_id: operation_id.to_string(),
                session_id: turn.session_id,
                actor_id: turn.actor_id.clone(),
                participant_actor_ids: participant_actor_ids.clone(),
                agenda: agenda.clone(),
                duration_blocks: *duration_blocks,
            },
        )),
    }
}

fn require_repository<'a>(
    context: &'a ExecutionContext,
    actor: &ResolvedActor,
    actor_id: &str,
    repository_id: &str,
    required: RepositoryAccess,
) -> Result<&'a RepositoryExecutionTarget, ExecutionError> {
    let repository =
        context
            .repository(repository_id)
            .ok_or_else(|| ExecutionError::MissingRepository {
                repository_id: repository_id.to_string(),
            })?;
    let actual = actor.access_for(repository_id);
    if actual.is_some_and(|access| access.allows(required)) {
        Ok(repository)
    } else {
        Err(ExecutionError::RepositoryAccessDenied {
            actor_id: actor_id.to_string(),
            repository_id: repository_id.to_string(),
            required,
            actual,
        })
    }
}

fn receipt_matches(command: &ResolvedCommand, receipt: &OperationReceipt) -> bool {
    matches!(
        (command, receipt),
        (ResolvedCommand::Buzz(_), OperationReceipt::Buzz(_))
            | (ResolvedCommand::GitHub(_), OperationReceipt::GitHub(_))
            | (
                ResolvedCommand::Verification(_),
                OperationReceipt::Verification(_)
            )
            | (
                ResolvedCommand::Simulation(_),
                OperationReceipt::Simulation(_)
            )
    )
}
