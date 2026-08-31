use std::collections::BTreeMap;

use async_trait::async_trait;
use buzz_sim_protocol::RepositoryRevision;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    validate_full_git_object_id, CreatedRepository, DestinationRepository, GitHubApiError,
    GitHubRepositoryApi, RepositoryGrant, SeedPlan, SessionProvisioningPlan,
};

/// Result returned after copying one exact source commit into a destination repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeededRepository {
    /// Stable scenario-local repository identifier.
    pub repository_id: String,
    /// Full commit identifier now at the destination session branch.
    pub head_commit_sha: String,
}

/// Boundary for the Git transport that fetches and pushes one exact commit.
#[async_trait]
pub trait RepositorySeeder: Send + Sync {
    /// Executes the shell-neutral operations from one validated seed plan.
    async fn seed_repository(&self, plan: &SeedPlan) -> Result<SeededRepository, GitHubApiError>;
}

/// One repository successfully created and seeded for a simulation session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionedRepository {
    /// Stable scenario-local repository identifier.
    pub repository_id: String,
    /// GitHub repository facts returned by repository creation.
    pub created: CreatedRepository,
    /// Exact commit installed by the seeder.
    pub seeded: SeededRepository,
}

/// Fully provisioned session state returned to the simulation service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionedSession {
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Repositories sorted by scenario-local identifier.
    pub repositories: Vec<ProvisionedRepository>,
    /// Grants in the deterministic order in which they were applied.
    pub grants: Vec<RepositoryGrant>,
    /// Current destination head commit by scenario-local repository identifier.
    pub head_commits: BTreeMap<String, String>,
}

impl ProvisionedSession {
    /// Builds the exact repository set accepted by the sandbox verification protocol.
    pub fn verification_revisions(
        &self,
        plan: &SessionProvisioningPlan,
    ) -> Result<Vec<RepositoryRevision>, crate::ProvisioningError> {
        plan.verification_revisions(&self.head_commits)
    }
}

/// Provisioning phase that produced the primary failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningFailurePhase {
    /// Creating an empty destination repository failed.
    Create,
    /// Copying the exact source commit into the destination failed.
    Seed,
    /// Applying one actor grant failed.
    Grant,
}

/// Failure encountered while deleting one partially created repository.
#[derive(Debug, thiserror::Error)]
#[error("rollback failed for repository {repository_id:?}: {error}")]
pub struct RollbackFailure {
    /// Scenario-local repository identifier.
    pub repository_id: String,
    /// GitHub deletion error.
    pub error: GitHubApiError,
}

/// Provisioning failure preserving both the primary error and every rollback error.
#[derive(Debug, thiserror::Error)]
#[error(
    "GitHub session provisioning {phase:?} phase failed for repository {repository_id:?}, actor {actor_id:?}: {source}"
)]
pub struct SessionProvisioningFailure {
    /// Phase that failed.
    pub phase: ProvisioningFailurePhase,
    /// Repository involved in the primary failure, when available.
    pub repository_id: Option<String>,
    /// Actor involved in the primary failure, when available.
    pub actor_id: Option<String>,
    /// Primary GitHub transport or seeding failure.
    #[source]
    pub source: GitHubApiError,
    /// Best-effort rollback failures, in attempted deletion order.
    pub rollback_failures: Vec<RollbackFailure>,
}

/// Coordinates GitHub repository creation, exact commit seeding, actor grants, and rollback.
#[derive(Debug, Clone)]
pub struct GitHubSessionProvisioner<A, S> {
    api: A,
    seeder: S,
}

impl<A, S> GitHubSessionProvisioner<A, S> {
    /// Creates a provisioner from independent REST and Git transport implementations.
    #[must_use]
    pub const fn new(api: A, seeder: S) -> Self {
        Self { api, seeder }
    }
}

impl<A, S> GitHubSessionProvisioner<A, S>
where
    A: GitHubRepositoryApi,
    S: RepositorySeeder,
{
    /// Provisions all repositories and grants from a deterministic compiled plan.
    ///
    /// Repositories are created and seeded before any actor receives access. Any failure causes a
    /// best-effort reverse-order deletion of every repository created during this invocation.
    pub async fn provision(
        &self,
        plan: &SessionProvisioningPlan,
    ) -> Result<ProvisionedSession, SessionProvisioningFailure> {
        let mut created_destinations = Vec::<DestinationRepository>::new();
        let mut repositories = Vec::<ProvisionedRepository>::new();
        let mut head_commits = BTreeMap::new();

        for seed in &plan.seeds {
            let created = match self.api.create_repository(&seed.destination).await {
                Ok(created) => created,
                Err(source) => {
                    return Err(self
                        .failure_with_rollback(
                            ProvisioningFailurePhase::Create,
                            Some(seed.source.repository_id.clone()),
                            None,
                            source,
                            &created_destinations,
                        )
                        .await);
                }
            };
            created_destinations.push(seed.destination.clone());

            let seeded = match self.seeder.seed_repository(seed).await {
                Ok(seeded) => seeded,
                Err(source) => {
                    return Err(self
                        .failure_with_rollback(
                            ProvisioningFailurePhase::Seed,
                            Some(seed.source.repository_id.clone()),
                            None,
                            source,
                            &created_destinations,
                        )
                        .await);
                }
            };
            if seeded.repository_id != seed.source.repository_id {
                let source = GitHubApiError::InvalidResponse {
                    operation: "seed_repository",
                    reason: format!(
                        "seeder returned repository {:?} for {:?}",
                        seeded.repository_id, seed.source.repository_id
                    ),
                };
                return Err(self
                    .failure_with_rollback(
                        ProvisioningFailurePhase::Seed,
                        Some(seed.source.repository_id.clone()),
                        None,
                        source,
                        &created_destinations,
                    )
                    .await);
            }
            if let Err(error) = validate_full_git_object_id(&seeded.head_commit_sha) {
                let source = GitHubApiError::InvalidResponse {
                    operation: "seed_repository",
                    reason: error.to_string(),
                };
                return Err(self
                    .failure_with_rollback(
                        ProvisioningFailurePhase::Seed,
                        Some(seed.source.repository_id.clone()),
                        None,
                        source,
                        &created_destinations,
                    )
                    .await);
            }

            head_commits.insert(
                seed.source.repository_id.clone(),
                seeded.head_commit_sha.clone(),
            );
            repositories.push(ProvisionedRepository {
                repository_id: seed.source.repository_id.clone(),
                created,
                seeded,
            });
        }

        let mut applied_grants = Vec::with_capacity(plan.grants.len());
        for grant in &plan.grants {
            if let Err(source) = self.api.grant_repository_access(grant).await {
                return Err(self
                    .failure_with_rollback(
                        ProvisioningFailurePhase::Grant,
                        Some(grant.repository_id.clone()),
                        Some(grant.actor_id.clone()),
                        source,
                        &created_destinations,
                    )
                    .await);
            }
            applied_grants.push(grant.clone());
        }

        Ok(ProvisionedSession {
            session_id: plan.session_id,
            repositories,
            grants: applied_grants,
            head_commits,
        })
    }

    async fn failure_with_rollback(
        &self,
        phase: ProvisioningFailurePhase,
        repository_id: Option<String>,
        actor_id: Option<String>,
        source: GitHubApiError,
        created_destinations: &[DestinationRepository],
    ) -> SessionProvisioningFailure {
        let mut rollback_failures = Vec::new();
        for destination in created_destinations.iter().rev() {
            if let Err(error) = self.api.delete_repository(destination).await {
                rollback_failures.push(RollbackFailure {
                    repository_id: destination.repository_id.clone(),
                    error,
                });
            }
        }
        SessionProvisioningFailure {
            phase,
            repository_id,
            actor_id,
            source,
            rollback_failures,
        }
    }
}
