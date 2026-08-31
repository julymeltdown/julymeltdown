use std::collections::BTreeSet;

use async_trait::async_trait;
use buzz_sim_protocol::{VerificationAccepted, VerificationRequest};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// External subsystem responsible for one side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayKind {
    /// Buzz channel or direct-message delivery.
    Buzz,
    /// GitHub branch, pull-request, or review mutation.
    GitHub,
    /// Objective sandbox verification submission.
    Verification,
    /// Authoritative simulation command application.
    Simulation,
}

/// Bounded failure returned by an external gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?} gateway failure {code:?}: {message}")]
#[serde(deny_unknown_fields)]
pub struct GatewayFailure {
    /// Subsystem that failed.
    pub kind: GatewayKind,
    /// Stable machine-readable failure code.
    pub code: String,
    /// Bounded diagnostic that must not contain credentials.
    pub message: String,
    /// Whether retrying with the same operation ID is permitted.
    pub retryable: bool,
}

impl GatewayFailure {
    /// Creates a retryable gateway failure.
    #[must_use]
    pub fn retryable(
        kind: GatewayKind,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
            retryable: true,
        }
    }

    /// Creates a permanent gateway failure.
    #[must_use]
    pub fn permanent(
        kind: GatewayKind,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }
}

/// Destination for one Buzz message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "destination", rename_all = "snake_case", deny_unknown_fields)]
pub enum BuzzDestination {
    /// Existing Buzz channel.
    Channel {
        /// Stable channel identifier.
        channel_id: String,
    },
    /// Direct message addressed to one simulation actor.
    DirectMessage {
        /// Stable recipient actor identifier.
        recipient_actor_id: String,
    },
}

/// Idempotent Buzz message command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuzzMessageCommand {
    /// Deterministic idempotency key.
    pub operation_id: String,
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// NPC sending the message.
    pub actor_id: String,
    /// Message destination.
    pub destination: BuzzDestination,
    /// Validated message body.
    pub body: String,
}

/// Receipt returned after Buzz accepts a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuzzMessageReceipt {
    /// Stable Buzz message identifier.
    pub message_id: String,
}

/// Boundary used to publish NPC messages into Buzz.
#[async_trait]
pub trait BuzzGateway: Send {
    /// Sends one idempotent message command.
    async fn send_message(
        &mut self,
        command: &BuzzMessageCommand,
    ) -> Result<BuzzMessageReceipt, GatewayFailure>;
}

/// Idempotent GitHub mutation resolved from one validated NPC action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitHubCommand {
    /// Create a branch at one exact session commit.
    CreateBranch {
        /// Deterministic idempotency key.
        operation_id: String,
        /// Simulation session identifier.
        session_id: Uuid,
        /// Stable NPC actor identifier.
        actor_id: String,
        /// GitHub login bound to the NPC.
        actor_login: String,
        /// Scenario-local repository identifier.
        repository_id: String,
        /// Destination repository host.
        host: String,
        /// Destination repository owner.
        owner: String,
        /// Destination repository name.
        name: String,
        /// New branch name.
        branch_name: String,
        /// Exact commit used as the branch head.
        from_sha: String,
        /// Human-readable reason for creating the branch.
        purpose: String,
    },
    /// Request the configured session reviewers on a pull request.
    RequestReview {
        /// Deterministic idempotency key.
        operation_id: String,
        /// Simulation session identifier.
        session_id: Uuid,
        /// Stable NPC actor identifier.
        actor_id: String,
        /// GitHub login bound to the requesting NPC.
        actor_login: String,
        /// Scenario-local repository identifier.
        repository_id: String,
        /// Destination repository host.
        host: String,
        /// Destination repository owner.
        owner: String,
        /// Destination repository name.
        name: String,
        /// Positive pull-request number.
        pull_request: u64,
        /// Canonical GitHub logins selected by the session review route.
        reviewer_logins: BTreeSet<String>,
    },
    /// Open a pull request from an existing session branch.
    OpenPullRequest {
        /// Deterministic idempotency key.
        operation_id: String,
        /// Simulation session identifier.
        session_id: Uuid,
        /// Stable NPC actor identifier.
        actor_id: String,
        /// GitHub login bound to the NPC.
        actor_login: String,
        /// Scenario-local repository identifier.
        repository_id: String,
        /// Destination repository host.
        host: String,
        /// Destination repository owner.
        owner: String,
        /// Destination repository name.
        name: String,
        /// Existing source branch.
        branch_name: String,
        /// Protected destination branch.
        base_branch: String,
        /// Pull-request title.
        title: String,
        /// Pull-request body.
        body: String,
    },
    /// Submit a non-approving review comment to a pull request.
    ReviewPullRequest {
        /// Deterministic idempotency key.
        operation_id: String,
        /// Simulation session identifier.
        session_id: Uuid,
        /// Stable NPC actor identifier.
        actor_id: String,
        /// GitHub login bound to the reviewing NPC.
        actor_login: String,
        /// Scenario-local repository identifier.
        repository_id: String,
        /// Destination repository host.
        host: String,
        /// Destination repository owner.
        owner: String,
        /// Destination repository name.
        name: String,
        /// Positive pull-request number.
        pull_request: u64,
        /// Validated review body.
        body: String,
    },
}

/// Receipt returned by a GitHub work gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "receipt", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitHubReceipt {
    /// Branch exists at the requested commit.
    BranchCreated {
        /// Branch name.
        branch_name: String,
        /// Full commit identifier at the branch head.
        commit_sha: String,
    },
    /// Reviewer assignment was accepted.
    ReviewRequested {
        /// Pull-request number.
        pull_request: u64,
        /// Canonical reviewer logins.
        reviewer_logins: BTreeSet<String>,
    },
    /// Pull request exists for the requested branch.
    PullRequestOpened {
        /// Pull-request number.
        pull_request: u64,
        /// Canonical web URL.
        url: String,
    },
    /// Pull-request review was accepted.
    PullRequestReviewed {
        /// Pull-request number.
        pull_request: u64,
        /// GitHub review identifier.
        review_id: u64,
    },
}

/// Boundary used for GitHub work mutations after session provisioning.
#[async_trait]
pub trait GitHubGateway: Send {
    /// Executes one idempotent GitHub command.
    async fn execute(&mut self, command: &GitHubCommand) -> Result<GitHubReceipt, GatewayFailure>;
}

/// Idempotent sandbox verification submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationCommand {
    /// Deterministic idempotency key derived from the NPC action.
    pub operation_id: String,
    /// Exact objective verification request.
    pub request: VerificationRequest,
}

/// Boundary used to submit objective sandbox verification.
#[async_trait]
pub trait VerificationGateway: Send {
    /// Submits one exact verification request.
    async fn submit(
        &mut self,
        command: &VerificationCommand,
    ) -> Result<VerificationAccepted, GatewayFailure>;
}

/// Authoritative non-GitHub simulation mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum SimulationCommand {
    /// Escalate a risk or decision to another actor.
    Escalate {
        /// Deterministic idempotency key.
        operation_id: String,
        /// Simulation session identifier.
        session_id: Uuid,
        /// NPC raising the escalation.
        actor_id: String,
        /// Actor receiving the escalation.
        target_actor_id: String,
        /// Validated escalation summary.
        summary: String,
    },
    /// Schedule a meeting in simulation time.
    ScheduleMeeting {
        /// Deterministic idempotency key.
        operation_id: String,
        /// Simulation session identifier.
        session_id: Uuid,
        /// NPC organizing the meeting.
        actor_id: String,
        /// Stable participant actor identifiers.
        participant_actor_ids: BTreeSet<String>,
        /// Validated meeting agenda.
        agenda: String,
        /// Number of work blocks consumed.
        duration_blocks: u8,
    },
}

/// Receipt returned by the authoritative simulation gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationReceipt {
    /// Authoritative event identifier created by the simulation kernel.
    pub event_id: Uuid,
}

/// Boundary used for authoritative simulation-only commands.
#[async_trait]
pub trait SimulationGateway: Send {
    /// Applies one idempotent simulation command.
    async fn apply(
        &mut self,
        command: &SimulationCommand,
    ) -> Result<SimulationReceipt, GatewayFailure>;
}
