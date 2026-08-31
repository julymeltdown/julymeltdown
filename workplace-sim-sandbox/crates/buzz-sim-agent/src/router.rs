use async_trait::async_trait;

use crate::{
    NpcActionCommand, NpcActionDraft, NpcActionExecutor, NpcActionExecutorError, NpcActionReceipt,
};

/// External port for Buzz channel and direct-message side effects.
#[async_trait]
pub trait BuzzActionPort: Send + Sync {
    /// Executes one already validated Buzz action command.
    async fn execute_buzz_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError>;
}

/// External port for GitHub branch, pull-request, and review side effects.
#[async_trait]
pub trait GitHubActionPort: Send + Sync {
    /// Executes one already validated GitHub action command.
    async fn execute_github_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError>;
}

/// External port for objective sandbox-verification requests.
#[async_trait]
pub trait VerificationActionPort: Send + Sync {
    /// Executes one already validated verification action command.
    async fn execute_verification_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError>;
}

/// External port for organizational escalation and meeting side effects.
#[async_trait]
pub trait OrganizationActionPort: Send + Sync {
    /// Executes one already validated organization action command.
    async fn execute_organization_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError>;
}

/// Routes each validated NPC action category to exactly one external backend.
#[derive(Debug, Clone)]
pub struct RoutedNpcActionExecutor<B, G, V, O> {
    buzz: B,
    github: G,
    verification: V,
    organization: O,
}

impl<B, G, V, O> RoutedNpcActionExecutor<B, G, V, O> {
    /// Creates a fail-closed action router from four independent ports.
    #[must_use]
    pub const fn new(buzz: B, github: G, verification: V, organization: O) -> Self {
        Self {
            buzz,
            github,
            verification,
            organization,
        }
    }

    /// Returns the Buzz action port.
    #[must_use]
    pub const fn buzz(&self) -> &B {
        &self.buzz
    }

    /// Returns the GitHub action port.
    #[must_use]
    pub const fn github(&self) -> &G {
        &self.github
    }

    /// Returns the verification action port.
    #[must_use]
    pub const fn verification(&self) -> &V {
        &self.verification
    }

    /// Returns the organization action port.
    #[must_use]
    pub const fn organization(&self) -> &O {
        &self.organization
    }
}

#[async_trait]
impl<B, G, V, O> NpcActionExecutor for RoutedNpcActionExecutor<B, G, V, O>
where
    B: BuzzActionPort,
    G: GitHubActionPort,
    V: VerificationActionPort,
    O: OrganizationActionPort,
{
    async fn execute(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        match &command.action {
            NpcActionDraft::SendMessage { .. } => self.buzz.execute_buzz_action(command).await,
            NpcActionDraft::CreateBranch { .. }
            | NpcActionDraft::RequestReview { .. }
            | NpcActionDraft::OpenPullRequest { .. }
            | NpcActionDraft::ReviewPullRequest { .. } => {
                self.github.execute_github_action(command).await
            }
            NpcActionDraft::RunVerification { .. } => {
                self.verification
                    .execute_verification_action(command)
                    .await
            }
            NpcActionDraft::Escalate { .. } | NpcActionDraft::ScheduleMeeting { .. } => {
                self.organization
                    .execute_organization_action(command)
                    .await
            }
        }
    }
}
